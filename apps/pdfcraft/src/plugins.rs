//! Plug-ins for the window (RFC-0046): the Plug-ins menu and window run automation's `plugin_*`
//! tools over the window's own session, so a plug-in does in the app exactly what it does in
//! `pdfcraft-cli run` and MCP. The installed plug-ins live in `<settings folder>/Plug-ins`.
//! Downloads (catalogs, GitHub releases) go through craft-update's HTTPS client, the one the
//! update check uses, on the Plug-ins window's worker thread.

use std::sync::{Arc, Mutex, PoisonError};

use craft_update::http::Fetched;
use pdfcraft_automation::plugins::craft_plugin::MAX_BUNDLE_BYTES;
use pdfcraft_automation::plugins::craft_plugin::fetch::{Fetch, follow_redirects, read_local_file};
use pdfcraft_automation::plugins::{self, NoNetwork, Request, Runtime};
use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::Session;
use pdfcraft_ui_egui::PluginBackend;
use serde_json::{Value, json};

/// craft-update's client as craft-plugin's [`Fetch`]. (craft-plugin's own `http` feature
/// implements it for the craft-update of its own craft-libs revision, another copy than the one
/// PdfCraft pins, so PdfCraft wraps its own.)
struct UpdateHttp(craft_update::Http);

impl Fetch for UpdateHttp {
    fn get(&mut self, url: &str, max: usize) -> Result<Option<Vec<u8>>, String> {
        if url.starts_with("https://api.github.com/") {
            return match self.0.get_api(url, None)? {
                Fetched::Body { text, .. } if text.len() > max => Err(format!("GitHub's answer is larger than {max} bytes")),
                Fetched::Body { text, .. } => Ok(Some(text.into_bytes())),
                Fetched::NotFound => Ok(None),
                Fetched::RateLimited { .. } => Err("GitHub's API limit for this hour is used up: try again later".into()),
                Fetched::NotModified => Err("GitHub answered 304 Not Modified to a request without a cache".into()),
            };
        }
        // Every redirect stays on GitHub's hosts over HTTPS; a HEAD that fails ends the walk.
        let end = follow_redirects(url, &mut |u| self.0.redirect_target(u).ok().flatten())?;
        self.0.get_bytes(&end, max)
    }
}

/// An HTTPS client for plug-in downloads, when one can be made.
fn http() -> Option<Box<dyn Fetch + Send>> {
    match craft_update::Http::new(&format!("PdfCraft/{}", env!("CARGO_PKG_VERSION"))) {
        Ok(http) => Some(Box::new(UpdateHttp(http))),
        Err(e) => {
            log::warn!("plug-in downloads: {e}");
            None
        }
    }
}

/// The window's plug-ins: one runtime (installed plug-ins, settings, catalogs, the reviewed
/// bundle), shared with the Plug-ins window's worker thread.
pub(crate) struct Backend {
    rt: Arc<Mutex<Runtime>>,
}

impl Backend {
    /// Plug-ins kept in `<settings_dir>/Plug-ins`, or for this session only without a settings
    /// folder.
    pub(crate) fn new(settings_dir: Option<&std::path::Path>) -> Backend {
        let mut rt = Runtime::default();
        if let Some(dir) = settings_dir {
            rt.open_store(dir);
        }
        rt.set_fetch(http());
        Backend { rt: Arc::new(Mutex::new(rt)) }
    }
}

/// A tool's answer as one JSON value.
fn first_json(content: Vec<Content>) -> Value {
    content.into_iter().find_map(|c| if let Content::Json(v) = c { Some(v) } else { None }).unwrap_or(Value::Null)
}

/// The bundle a Plug-ins window job names, downloaded or read without holding the runtime, then
/// reviewed and kept for the install that follows.
fn review(rt: &Mutex<Runtime>, args: &Value) -> Result<Value, String> {
    let req = Request::parse(args)?;
    let catalogs = rt.lock().unwrap_or_else(PoisonError::into_inner).prefs.catalogs.clone();
    let mut fetch = http().unwrap_or_else(|| Box::new(NoNetwork));
    let mut read = |path: &str| read_local_file(std::path::Path::new(path), MAX_BUNDLE_BYTES);
    let candidate = plugins::obtain_with(&req, fetch.as_mut(), &catalogs, &mut read)?;
    let mut rt = rt.lock().unwrap_or_else(PoisonError::into_inner);
    let (_, mut v) = rt.review(&candidate)?;
    let request = plugins::install_request(args);
    v["sha256"] = json!(rt.hold(request.clone(), candidate));
    v["request"] = request;
    Ok(v)
}

/// `plugin_update` for the window: a newer release's review, or `update: null`.
fn update(rt: &Mutex<Runtime>, args: &Value) -> Result<Value, String> {
    let id = args["id"].as_str().unwrap_or_default();
    let p = rt.lock().unwrap_or_else(PoisonError::into_inner).get(id).cloned().ok_or_else(|| format!("no plug-in `{id}` is installed"))?;
    let mut fetch = http().unwrap_or_else(|| Box::new(NoNetwork));
    let Some((mut install, version)) = plugins::update_for(fetch.as_mut(), &p)? else {
        return Ok(json!({ "id": p.manifest.id, "update": Value::Null }));
    };
    install["review"] = json!(true);
    let reviewed = review(rt, &install)?;
    Ok(json!({ "id": p.manifest.id, "update": version, "review": reviewed, "install": plugins::install_request(&install) }))
}

/// `plugin_catalog_list` for the window: the packages, with the version installed here.
fn catalog_list(rt: &Mutex<Runtime>, args: &Value) -> Result<Value, String> {
    let urls: Vec<String> = match args["url"].as_str() {
        Some(url) => vec![url.to_string()],
        None => rt.lock().unwrap_or_else(PoisonError::into_inner).prefs.catalogs.clone(),
    };
    let mut fetch = http().unwrap_or_else(|| Box::new(NoNetwork));
    let (mut packages, mut warnings) = (Vec::new(), Vec::new());
    for url in &urls {
        match plugins::catalog_packages_with(fetch.as_mut(), url) {
            Ok((p, w)) => {
                packages.extend(p);
                warnings.extend(w.into_iter().map(|w| format!("{url}: {w}")));
            }
            Err(e) => warnings.push(format!("{url}: {e}")),
        }
    }
    let rt = rt.lock().unwrap_or_else(PoisonError::into_inner);
    for pkg in &mut packages {
        pkg["installed"] = json!(pkg["id"].as_str().and_then(|id| rt.get(id)).map(|p| p.manifest.version.clone()));
    }
    Ok(json!({ "catalogs": urls, "packages": packages, "warnings": warnings }))
}

impl PluginBackend for Backend {
    fn call(&mut self, session: &mut Session, tool: &str, args: &Value) -> Result<Value, String> {
        let mut rt = self.rt.lock().unwrap_or_else(PoisonError::into_inner);
        let mut a = Automation::from_session(std::mem::take(session)).with_plugin_runtime(std::mem::take(&mut *rt));
        let result = a.call(tool, args);
        *rt = a.take_plugin_runtime();
        *session = a.into_session();
        result.map(first_json).map_err(|e| e.to_string())
    }

    fn job(&mut self, tool: &str, args: Value) -> Box<dyn FnOnce() -> Result<Value, String> + Send> {
        let rt = Arc::clone(&self.rt);
        let tool = tool.to_string();
        Box::new(move || match tool.as_str() {
            "plugin_install" => review(&rt, &args),
            "plugin_update" => update(&rt, &args),
            "plugin_catalog_list" => catalog_list(&rt, &args),
            other => Err(format!("`{other}` doesn't run in the background")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window's session goes into the tools and comes back, documents and all.
    #[test]
    fn a_tool_runs_over_the_windows_session_and_gives_it_back() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-plugin-backend-{}", std::process::id()));
        let mut backend = Backend::new(Some(&dir));
        let mut session = Session::new();
        let bytes = session.create_from_text("Plug-ins", "Hello").unwrap();
        let id = session.open("a.pdf", None, bytes, None).unwrap();
        let listed = backend.call(&mut session, "plugin_list", &json!({})).unwrap();
        assert_eq!(listed["stored"], json!(true));
        assert!(session.get(id).is_some(), "the session came back");
        let refused = backend.call(&mut session, "plugin_run", &json!({ "id": "org.example.none", "doc": id.0 })).unwrap_err();
        assert!(refused.contains("no plug-in"), "{refused}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
