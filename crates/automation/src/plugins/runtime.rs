//! The session's plug-ins: the installed ones, where they are kept, their settings and catalogs.

use std::collections::BTreeMap;
use std::sync::Arc;

use craft_plugin::fetch::Fetch;
use craft_plugin::{Manifest, Source, bundle};
use serde_json::{Map, Value, json};

use super::{APP, Candidate, Pending};

/// An installed plug-in.
#[derive(Clone, Debug)]
pub struct InstalledPlugin {
    pub manifest: Manifest,
    /// The SHA-256 of its bundle (lower-case hex).
    pub sha256: String,
    /// Where it came from.
    pub source: Source,
    /// Kept in the user's `Plug-ins` folder; otherwise for this session only.
    pub stored: bool,
    pub(crate) wasm: Arc<Vec<u8>>,
}

impl InstalledPlugin {
    /// What `plugin_list` says about it: its manifest, kind, SHA-256, source and whether it is
    /// kept in the user's folder.
    pub fn describe(&self) -> Value {
        let mut v = serde_json::to_value(&self.manifest).unwrap_or_else(|_| json!({"id": self.manifest.id}));
        v["kind"] = json!(self.manifest.kind);
        v["sha256"] = json!(self.sha256);
        v["source"] = serde_json::to_value(&self.source).unwrap_or(Value::Null);
        v["stored"] = json!(self.stored);
        v
    }
}

/// Each plug-in's own settings and the catalogs added: `<settings folder>/plugin-settings.json`
/// beside the `Plug-ins` folder, when the session keeps plug-ins there.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PluginPrefs {
    /// Each plug-in's settings, by plug-in id (`MAX_SETTINGS_BYTES` each).
    pub settings: BTreeMap<String, Map<String, Value>>,
    /// Plug-in catalogs added with `plugin_catalog_add` (`index.json` URLs).
    pub catalogs: Vec<String>,
}

/// The settings file's name in the settings folder.
pub const PREFS_FILE: &str = "plugin-settings.json";
/// Largest settings file read: every plug-in's settings at their limit would be far more, but a
/// file this size is already a damaged one.
#[cfg(not(target_arch = "wasm32"))]
const MAX_PREFS_BYTES: u64 = 8 << 20;

/// The session's plug-ins. The desktop app opens the store ([`Runtime::open_store`]); without one,
/// plug-ins are installed for the session only.
#[derive(Default)]
pub struct Runtime {
    /// The user's `Plug-ins` folder.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) store: Option<craft_plugin::store::Store>,
    /// Where [`PluginPrefs`] are kept (with the store): a file in the settings folder the host
    /// chose, never a path a tool argument names.
    prefs_file: Option<pdfcraft_platform::settings_file::SettingsFile>,
    plugins: Vec<InstalledPlugin>,
    /// Plug-in folders that couldn't be read, and plug-ins this version can't run.
    problems: Vec<String>,
    /// How catalogs and GitHub releases are downloaded; `None`: only files and `file://` catalogs.
    pub(crate) fetch: Option<Box<dyn Fetch + Send>>,
    pub(crate) pending: Option<Pending>,
    /// The plug-ins' settings and catalogs.
    pub prefs: PluginPrefs,
}

impl Runtime {
    /// Keep plug-ins in `<settings_dir>/Plug-ins` (and their settings in [`PREFS_FILE`] beside
    /// it), and read the ones installed.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_store(&mut self, settings_dir: &std::path::Path) {
        let store = craft_plugin::store::Store::new(craft_plugin::store::plugins_dir(settings_dir));
        self.store = Some(store);
        self.reload();
        let file = pdfcraft_platform::settings_file::SettingsFile::new(settings_dir.join(PREFS_FILE));
        let read = file.read(MAX_PREFS_BYTES).map_err(|e| e.to_string());
        match read.and_then(|bytes| bytes.map(|b| serde_json::from_slice::<PluginPrefs>(&b).map_err(|e| e.to_string())).transpose()) {
            Ok(Some(prefs)) => self.prefs = prefs,
            Ok(None) => {}
            Err(e) => self.problems.push(format!("{}: {e}", file.path().display())),
        }
        self.prefs.catalogs.truncate(super::MAX_CATALOGS);
        self.prefs_file = Some(file);
    }

    /// Read the plug-ins in the user's folder again (one installed or removed by another window).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn reload(&mut self) {
        let Some(store) = &self.store else { return };
        let listing = store.list_for(&APP);
        self.plugins.retain(|p| !p.stored);
        self.problems = listing.problems.into_iter().map(|p| format!("{}: {}", p.dir.display(), p.error)).collect();
        // The store keeps whatever was installed; only plug-ins for this PdfCraft are offered.
        self.problems.extend(listing.not_runnable.into_iter().map(|(i, why)| format!("{}: {why}", i.manifest.name)));
        for (i, _) in listing.runnable {
            self.keep(InstalledPlugin {
                manifest: i.manifest,
                sha256: i.record.sha256,
                source: i.record.source,
                stored: true,
                wasm: Arc::new(i.bundle.wasm),
            });
        }
    }

    /// Write [`Runtime::prefs`] to the settings folder (when the store is open). A failure is
    /// logged: the settings stay in effect for the session.
    pub fn save_prefs(&self) {
        if let Some(file) = &self.prefs_file {
            let written =
                serde_json::to_vec_pretty(&self.prefs).map_err(|e| e.to_string()).and_then(|bytes| file.write(&bytes).map_err(|e| e.to_string()));
            if let Err(e) = written {
                log::warn!("plug-in settings ({}): {e}", file.path().display());
            }
        }
    }

    /// The installed plug-ins.
    pub fn list(&self) -> &[InstalledPlugin] {
        &self.plugins
    }

    /// Why plug-in folders couldn't be used.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Plug-in `id` (ignoring case, as the store does).
    pub fn get(&self, id: &str) -> Option<&InstalledPlugin> {
        self.plugins.iter().find(|p| p.manifest.id.eq_ignore_ascii_case(id))
    }

    /// Whether plug-ins are kept in the user's folder (else for this session only).
    pub fn has_store(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.store.is_some();
        #[cfg(target_arch = "wasm32")]
        false
    }

    /// Whether catalogs and GitHub releases can be downloaded.
    pub fn has_network(&self) -> bool {
        self.fetch.is_some()
    }

    /// The fetcher downloads use, taken out (a frontend's worker thread uses it and gives it back
    /// with [`Runtime::set_fetch`]).
    pub fn take_fetch(&mut self) -> Option<Box<dyn Fetch + Send>> {
        self.fetch.take()
    }

    /// Download with `fetch` from now on.
    pub fn set_fetch(&mut self, fetch: Option<Box<dyn Fetch + Send>>) {
        self.fetch = fetch;
    }

    /// Put `plugin` in the list, in place of one with the same id.
    fn keep(&mut self, plugin: InstalledPlugin) -> InstalledPlugin {
        match self.plugins.iter_mut().find(|p| p.manifest.id.eq_ignore_ascii_case(&plugin.manifest.id)) {
            Some(slot) => *slot = plugin.clone(),
            None => self.plugins.push(plugin.clone()),
        }
        plugin
    }

    /// What an install confirmation shows about `c`: the manifest (name, version, author, licence,
    /// commands, permissions), its kind, the source, the SHA-256, whether a catalog lists it and
    /// what it replaces. An error when the bundle isn't a plug-in PdfCraft can run.
    pub fn review(&self, c: &Candidate) -> Result<(Manifest, Value), String> {
        let (_, manifest) = bundle::open(&c.bytes)?;
        manifest.target(&APP).map_err(|e| format!("{} can't be installed: {e}", manifest.name))?;
        c.source.check()?;
        let old = self.get(&manifest.id);
        let mut v = serde_json::to_value(&manifest).unwrap_or_else(|_| json!({"id": manifest.id}));
        v["kind"] = json!(manifest.kind);
        v["sha256"] = json!(bundle::sha256_hex(&c.bytes));
        v["source"] = serde_json::to_value(&c.source).unwrap_or(Value::Null);
        v["inCatalog"] = json!(c.in_catalog);
        v["replaces"] = json!(old.map(|o| &o.manifest.version));
        v["permissionsChanged"] = json!(old.is_some_and(|o| craft_plugin::permissions::changed(&o.manifest, &manifest)));
        v["stored"] = json!(self.has_store());
        Ok((manifest, v))
    }

    /// Keep `c`, named by the `plugin_install` arguments `request` (without `review` and
    /// `sha256`), for the install that follows its review (one at a time). Returns its SHA-256,
    /// which that install passes. A frontend that downloaded `c` itself
    /// ([`super::obtain_with`]) hands it over here.
    pub fn hold(&mut self, request: Value, c: Candidate) -> String {
        let sha256 = bundle::sha256_hex(&c.bytes);
        self.pending = Some(Pending { sha256: sha256.clone(), request: super::install_request(&request), candidate: c });
        sha256
    }

    /// The bundle reviewed last, when it is `sha256` and `request` named it.
    pub(crate) fn take_held(&mut self, sha256: &str, request: &Value) -> Option<Candidate> {
        let held = self.pending.take_if(|h| h.sha256.eq_ignore_ascii_case(sha256) && h.request == *request)?;
        Some(held.candidate)
    }

    /// Install `c` (already reviewed): into the user's folder when the session has one, else for
    /// the session. Replaces an installed plug-in with the same id; its settings stay.
    pub fn install(&mut self, c: Candidate) -> Result<InstalledPlugin, String> {
        let (manifest, _) = self.review(&c)?;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(store) = &self.store {
            let (i, _) = store.install_for(&APP, &c.bytes, c.source)?;
            let plugin = InstalledPlugin {
                manifest: i.manifest,
                sha256: i.record.sha256,
                source: i.record.source,
                stored: true,
                wasm: Arc::new(i.bundle.wasm),
            };
            return Ok(self.keep(plugin));
        }
        let sha256 = bundle::sha256_hex(&c.bytes);
        let (b, _) = bundle::open(&c.bytes)?;
        Ok(self.keep(InstalledPlugin { manifest, sha256, source: c.source, stored: false, wasm: Arc::new(b.wasm) }))
    }

    /// Remove plug-in `id` and its settings; whether it was installed.
    pub fn remove(&mut self, id: &str) -> Result<bool, String> {
        let Some(p) = self.get(id).cloned() else { return Ok(false) };
        #[cfg(not(target_arch = "wasm32"))]
        if p.stored
            && let Some(store) = &self.store
        {
            store.remove(&p.manifest.id)?;
        }
        self.plugins.retain(|q| !q.manifest.id.eq_ignore_ascii_case(&p.manifest.id));
        let had_settings = self.prefs.settings.keys().any(|k| k.eq_ignore_ascii_case(&p.manifest.id));
        self.prefs.settings.retain(|k, _| !k.eq_ignore_ascii_case(&p.manifest.id));
        if had_settings {
            self.save_prefs();
        }
        Ok(true)
    }
}
