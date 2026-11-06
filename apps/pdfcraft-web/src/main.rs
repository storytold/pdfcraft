//! PdfCraft in the browser. Build: `cd apps/pdfcraft-web && trunk build --release`
//! (trunk generates the JS loader; no handwritten JS — plan/execution-plan.md §1).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_arch = "wasm32")]
fn main() {
    use pdfcraft_ui_egui::PdfCraftApp;

    let options = eframe::WebOptions { renderer: eframe::Renderer::Glow, ..Default::default() };
    wasm_bindgen_futures::spawn_local(async move {
        let Some(canvas) = app_canvas() else { return };
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(|cc| {
                    let mut app = PdfCraftApp::new();
                    // "printcraft": settings saved under the app's former name, PrintCraft.
                    if let Some(json) = cc.storage.and_then(|s| s.get_string("pdfcraft").or_else(|| s.get_string("printcraft"))) {
                        app.restore(&json);
                    }
                    // `?file=<url>` opens a PDF from a URL (same-origin or CORS-enabled).
                    if let Some(url) = query_param("file") {
                        let inbox = app.startup_inbox.clone();
                        let failed = app.failed_inbox.clone();
                        let ctx = cc.egui_ctx.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            let name = url.rsplit('/').next().unwrap_or("document.pdf").split('?').next().unwrap_or("document.pdf").to_string();
                            match fetch_bytes(&url).await {
                                Ok(bytes) => {
                                    if let Ok(mut q) = inbox.lock() {
                                        q.push((name, bytes));
                                    }
                                }
                                Err(e) => {
                                    eframe::web_sys::console::error_1(&format!("PdfCraft: could not fetch {url}: {e}").into());
                                    // Shown in the app too, not only in the console (#173).
                                    if let Ok(mut q) = failed.lock() {
                                        q.push((name, e));
                                    }
                                }
                            }
                            ctx.request_repaint();
                        });
                    }
                    // Files the system hands an installed copy ("Open with", double-click), through
                    // the manifest's `file_handlers`; a no-op in browsers without File Handling.
                    launch_queue::listen(app.startup_inbox.clone(), app.failed_inbox.clone(), cc.egui_ctx.clone());
                    // eframe never raises a close request on a browser reload or tab close, so `guard_quit` never
                    // prompts; ask the browser to confirm instead while a document has unsaved work (#812).
                    warn_before_unload(app.unsaved_flag.clone());
                    // Host modes (`host` below, over `craft-web-host`): `?host=parent`, a
                    // same-origin page embedding PdfCraft in an iframe (such as the Nextcloud
                    // app), or `data-host="page"` on the canvas, a host that runs the module in
                    // its own document.
                    if let Some(bridge) = app_canvas().as_ref().and_then(host::Bridge::connect) {
                        // `?author=`: the host's name for the user signs their comments,
                        // unless they chose a name in PdfCraft (the web default is "Guest").
                        if let Some(author) = query_param("author").map(|a| a.trim().to_string()).filter(|a| !a.is_empty())
                            && app.comment_prefs.author == "Guest"
                        {
                            app.comment_prefs.author = author.chars().take(200).collect();
                        }
                        bridge.listen(app.startup_inbox.clone(), cc.egui_ctx.clone());
                        app.host_save = Some(bridge.save_fn());
                        app.host_dirty = Some(bridge.dirty_fn());
                        bridge.post_ready();
                    } else if query_param("host").as_deref() == Some("parent") {
                        eframe::web_sys::console::warn_1(
                            &"PdfCraft: ?host=parent needs a framing page on the same origin; saves download instead".into(),
                        );
                    }
                    Ok(Box::new(app))
                }),
            )
            .await;
        if let Err(e) = started {
            eframe::web_sys::console::error_1(&e);
            if let Some(bridge) = app_canvas().as_ref().and_then(host::Bridge::connect) {
                bridge.post_failed(&e.as_string().unwrap_or_else(|| format!("{e:?}")));
            }
        }
    });
}

#[cfg(target_arch = "wasm32")]
fn warn_before_unload(unsaved: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use eframe::wasm_bindgen::{JsCast, closure::Closure};
    let Some(window) = web_sys::window() else { return };
    let handler = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        if unsaved.load(std::sync::atomic::Ordering::Relaxed) {
            event.prevent_default();
        }
    });
    if window.add_event_listener_with_callback("beforeunload", handler.as_ref().unchecked_ref()).is_ok() {
        handler.forget();
    }
}

#[cfg(target_arch = "wasm32")]
fn query_param(key: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    web_sys::UrlSearchParams::new_with_str(&search).ok()?.get(key)
}

#[cfg(target_arch = "wasm32")]
async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    use eframe::wasm_bindgen::JsCast;
    let window = web_sys::window().ok_or("no window")?;
    let resp = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url)).await.map_err(|e| format!("{e:?}"))?;
    let resp: web_sys::Response = resp.dyn_into().map_err(|_| "not a response")?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let buf = wasm_bindgen_futures::JsFuture::from(resp.array_buffer().map_err(|e| format!("{e:?}"))?).await.map_err(|e| format!("{e:?}"))?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

/// The app's canvas: the one element `index.html` and every embedding page must provide, with
/// the id the app looks up.
#[cfg(target_arch = "wasm32")]
fn app_canvas() -> Option<web_sys::HtmlCanvasElement> {
    use eframe::wasm_bindgen::JsCast;
    web_sys::window()?.document()?.get_element_by_id("pdfcraft").and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
}

/// Host modes: how the page embedding PdfCraft passes files in and takes saves back. The messages
/// and the two transports are `craft-web-host`'s (storytold/craft-libs), the same contract as the
/// other Craft apps' bridges, with the `pdfcraft:` prefix. This module is PdfCraft's part of it.
///
/// - `?host=parent` is the `postMessage` bridge for a same-origin page that embeds PdfCraft in an
///   iframe (such as the Nextcloud app). With a parent on another origin there is no bridge: the
///   app runs as if unframed and saves download (#2167). `data-host="page"` on the app's canvas
///   is for a host that loads the module into its own document, with no iframe: the same messages
///   travel as DOM `CustomEvent`s on that canvas. `?host=parent` wins when both are present.
/// - `pdfcraft:ready` once the app is listening; send files now. PdfCraft reads bytes, so it
///   says `file: false` and a host sends `bytes`.
/// - `pdfcraft:open` (from the host): `{ name, bytes }` holding a PDF. It opens in a tab, like a
///   `?file=` URL; a file over [`host::MAX_OPEN_BYTES`] is refused before it is copied.
/// - `pdfcraft:save` (to the host): `{ name, saveAs, bytes }` when the user saves. The host stores
///   the file (and reports its own errors).
/// - `pdfcraft:dirty` whenever unsaved work appears or goes away, `pdfcraft:failed` when the app
///   couldn't start.
#[cfg(target_arch = "wasm32")]
mod host {
    use std::rc::Rc;

    use craft_web_host::web::{FileData, Host, Message};

    /// The app id in the host messages (`pdfcraft:ready`).
    const APP: &str = "pdfcraft";

    /// The name a file gets when the host sent none (the crate says `document` for a blank name).
    const DEFAULT_NAME: &str = "document.pdf";

    /// The largest file the host may hand over to open, checked before it is copied (the app's own readers have their own limits).
    pub(super) const MAX_OPEN_BYTES: u64 = 512 << 20;

    #[derive(Clone)]
    pub struct Bridge {
        host: Rc<Host>,
    }

    impl Bridge {
        /// The bridge for this page, or `None` when PdfCraft isn't hosted: no `?host=parent` and
        /// no `data-host="page"` on the canvas, an unframed `?host=parent` page, or a parent on
        /// another origin (messages only go to this origin, so the browser would drop every save
        /// without an error while the document showed as saved, #2167).
        pub fn connect(canvas: &web_sys::HtmlCanvasElement) -> Option<Self> {
            Some(Self { host: Rc::new(Host::connect(APP, canvas)?) })
        }

        pub fn post_ready(&self) {
            let _ = self.host.post_ready(env!("CARGO_PKG_VERSION"), false);
        }

        /// The app couldn't start (no WebGL, for example): let the host say so.
        pub fn post_failed(&self, error: &str) {
            let _ = self.host.post_failed(error, None);
        }

        /// Hear `pdfcraft:open` and queue the file like a `?file=` download.
        pub fn listen(&self, inbox: pdfcraft_ui_egui::Inbox, ctx: egui::Context) {
            self.host.listen(move |message| {
                if let Message::Open { name, data } = message {
                    queue_open(&name, &data, &inbox, &ctx);
                }
            });
        }

        pub fn save_fn(&self) -> pdfcraft_ui_egui::HostSaveFn {
            let host = self.host.clone();
            Box::new(move |name: &str, bytes: &[u8], save_as: bool| host.post_save(name, bytes, None, save_as))
        }

        pub fn dirty_fn(&self) -> pdfcraft_ui_egui::HostDirtyFn {
            let host = self.host.clone();
            // A host starts out clean: the first report is a change only when it is dirty.
            let mut last = false;
            Box::new(move |dirty: bool| {
                if dirty != last {
                    last = dirty;
                    host.post_dirty(dirty);
                }
            })
        }
    }

    /// The `pdfcraft:open` payload, `{ name, bytes }` where `bytes` holds a PDF: queue it like a
    /// `?file=` download and wake the app. The crate has cleaned the name (and cut it to 255
    /// characters); a blank one comes as `document` and becomes `document.pdf`.
    fn queue_open(name: &str, data: &FileData, inbox: &pdfcraft_ui_egui::Inbox, ctx: &egui::Context) {
        let name = if name == "document" { DEFAULT_NAME } else { name }.to_string();
        let FileData::Bytes(view) = data else {
            eframe::web_sys::console::warn_1(&format!("PdfCraft: host file {name} came as a File; PdfCraft reads bytes only; not opened").into());
            return;
        };
        // A file from the host is untrusted input: refuse an oversized one before copying it into the app's memory.
        if data.size() > MAX_OPEN_BYTES as f64 {
            eframe::web_sys::console::warn_1(
                &format!("PdfCraft: host file is {} bytes, over the {} MiB host open limit; not opened", data.size(), MAX_OPEN_BYTES >> 20).into(),
            );
            return;
        }
        if let Ok(mut q) = inbox.lock() {
            q.push((name, view.to_vec()));
        }
        ctx.request_repaint();
    }
}

/// The name a file opened from the system shows under: the first name the browser gave that isn't
/// blank (the file's own name, then the handle's), or `document.pdf` when there is none.
#[cfg(any(target_arch = "wasm32", test))]
fn file_name(names: impl IntoIterator<Item = Option<String>>) -> String {
    names.into_iter().flatten().find(|n| !n.trim().is_empty()).unwrap_or_else(|| "document.pdf".to_string())
}

/// Refuse a file from the system that is over `limit` bytes before it is read into memory. The size
/// comes from the browser: a size that is not a number is refused too.
#[cfg(any(target_arch = "wasm32", test))]
fn check_size(size: f64, limit: u64) -> Result<(), String> {
    // `!(a <= b)` and not `a > b`: NaN compares false both ways and must be refused.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    if !(size <= limit as f64) {
        return Err(format!("{size} bytes, over the {} MiB limit", limit >> 20));
    }
    Ok(())
}

/// File Handling: an installed copy of the page (a PWA) is registered by `manifest.webmanifest`'s
/// `file_handlers` as a PDF handler, so the system can open PDFs with it. The browser passes the
/// files through `window.launchQueue` as `FileSystemFileHandle`s; each opens in a tab, like a
/// `?file=` download. `web-sys` has no stable binding for the queue, so it is reached through
/// `Reflect`. Browsers without the API (Firefox, Safari, an uninstalled tab) have no
/// `launchQueue`, and then this does nothing.
#[cfg(target_arch = "wasm32")]
mod launch_queue {
    use std::sync::PoisonError;

    use eframe::wasm_bindgen::{JsCast, JsValue, closure::Closure};

    pub fn listen(inbox: pdfcraft_ui_egui::Inbox, failed: pdfcraft_ui_egui::FailedInbox, ctx: egui::Context) {
        let Some(window) = web_sys::window() else { return };
        let Ok(queue) = js_sys::Reflect::get(&window, &JsValue::from_str("launchQueue")) else { return };
        if queue.is_undefined() || queue.is_null() {
            return;
        }
        let Some(set_consumer) =
            js_sys::Reflect::get(&queue, &JsValue::from_str("setConsumer")).ok().and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        else {
            return;
        };
        let consumer = Closure::<dyn FnMut(JsValue)>::new(move |params: JsValue| {
            let Ok(files) = js_sys::Reflect::get(&params, &JsValue::from_str("files")) else { return };
            if !js_sys::Array::is_array(&files) {
                return;
            }
            // Read the files of one launch one after another, in the order the system gave them,
            // and queue each as soon as it is read: tabs open in that order, the first appears
            // without waiting for the rest, and the batch's last file ends up active, as for any
            // startup batch. Whether a startup file takes focus at all (it doesn't once the user
            // has started working) stays the app's decision.
            let handles: Vec<JsValue> = js_sys::Array::from(&files).iter().collect();
            let (inbox, failed, ctx) = (inbox.clone(), failed.clone(), ctx.clone());
            wasm_bindgen_futures::spawn_local(async move {
                for handle in &handles {
                    match read(handle).await {
                        // A poisoned lock still holds a usable queue; take it rather than drop the file.
                        Ok(file) => inbox.lock().unwrap_or_else(PoisonError::into_inner).push(file),
                        Err((name, e)) => {
                            eframe::web_sys::console::error_1(&format!("PdfCraft: could not open {name}: {e}").into());
                            failed.lock().unwrap_or_else(PoisonError::into_inner).push((name, e));
                        }
                    }
                    ctx.request_repaint();
                }
            });
        });
        // The queue holds launches made before a consumer was set and delivers them now; the
        // consumer lives as long as the page.
        match set_consumer.call1(&queue, consumer.as_ref()) {
            Ok(_) => consumer.forget(),
            Err(e) => eframe::web_sys::console::error_1(&format!("PdfCraft: could not receive files opened from the system: {e:?}").into()),
        }
    }

    /// The file behind one handle, `(name, bytes)`, or `(name, error)`. A file from the system is
    /// untrusted input: an oversized one is refused before it is read into memory.
    async fn read(handle: &JsValue) -> Result<(String, Vec<u8>), (String, String)> {
        // The handle's name is known before `getFile()` runs, so errors on the way still name the
        // file the user opened (the file's own name, read later, is normally the same).
        let handle_name = super::file_name([js_sys::Reflect::get(handle, &JsValue::from_str("name")).ok().and_then(|n| n.as_string())]);
        let named = |e: &str| (handle_name.clone(), e.to_string());
        let get_file = js_sys::Reflect::get(handle, &JsValue::from_str("getFile"))
            .ok()
            .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
            .ok_or_else(|| named("not a file (a folder?)"))?;
        let promise = get_file
            .call0(handle)
            .ok()
            .and_then(|p| p.dyn_into::<js_sys::Promise>().ok())
            .ok_or_else(|| named("the browser did not return the file"))?;
        let file: web_sys::File = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .ok()
            .and_then(|f| f.dyn_into().ok())
            .ok_or_else(|| named("the file could not be read (moved, or no permission?)"))?;
        let name = super::file_name([Some(file.name()), Some(handle_name.clone())]);
        super::check_size(file.size(), super::host::MAX_OPEN_BYTES).map_err(|e| (name.clone(), e))?;
        let buf =
            wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await.map_err(|e| (name.clone(), format!("could not read the file: {e:?}")))?;
        Ok((name, js_sys::Uint8Array::new(&buf).to_vec()))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("pdfcraft-web targets wasm32: run `trunk serve` or `trunk build --release` in apps/pdfcraft-web");
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    fn read(rel: &str) -> String {
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
    }

    /// The installed app is offered for PDFs only if the page links a manifest that Trunk copies
    /// into the site, and that manifest declares a PDF file handler inside the app's scope.
    #[test]
    fn manifest_registers_pdf_file_handler() {
        let html = read("index.html");
        assert!(html.contains(r#"<link rel="manifest" href="manifest.webmanifest" />"#), "index.html must link the manifest");
        assert!(html.contains(r#"<link data-trunk rel="copy-file" href="manifest.webmanifest" />"#), "Trunk must copy the manifest");

        let m: serde_json::Value = serde_json::from_str(&read("manifest.webmanifest")).expect("manifest is JSON");
        assert_eq!(m["scope"], "./");
        let handlers = m["file_handlers"].as_array().expect("file_handlers");
        let pdf = handlers.iter().find(|h| h["accept"]["application/pdf"].is_array()).expect("a handler for application/pdf");
        assert!(pdf["accept"]["application/pdf"].as_array().into_iter().flatten().any(|e| e == ".pdf"));
        // Relative to the manifest, so the site works under any path (packaging/web/package.sh).
        assert_eq!(pdf["action"], "./", "the action must stay inside the scope");
        // Installability: a name, a start URL inside the scope and an app window.
        assert_eq!(m["name"], "PdfCraft");
        assert_eq!(m["start_url"], "./");
        assert_eq!(m["display"], "standalone");
        // A second PDF opens as a tab in the window already open, not in a new window.
        let modes = m["launch_handler"]["client_mode"].as_array().expect("launch_handler.client_mode");
        assert_eq!(modes.first().and_then(|v| v.as_str()), Some("focus-existing"));
    }

    /// Every manifest icon is a file the `copy-dir` of `assets/app-icon/hicolor` into `icons/`
    /// puts in the site; a missing icon makes the browser refuse to install the app.
    #[test]
    fn manifest_icons_exist() {
        let html = read("index.html");
        assert!(html.contains(r#"href="../../assets/app-icon/hicolor" data-target-path="icons""#));
        let m: serde_json::Value = serde_json::from_str(&read("manifest.webmanifest")).expect("manifest is JSON");
        let icons = m["icons"].as_array().expect("icons");
        assert!(icons.iter().any(|i| i["sizes"] == "512x512"), "browsers want a 512 px icon to install");
        for icon in icons {
            let src = icon["src"].as_str().expect("icon src");
            let rel = src.strip_prefix("icons/").expect("icons live under icons/");
            let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/app-icon/hicolor").join(rel);
            assert!(file.is_file(), "{src}: no {}", file.display());
        }
    }

    #[test]
    fn launched_file_keeps_its_name() {
        assert_eq!(super::file_name([Some("Invoice 42.pdf".into())]), "Invoice 42.pdf");
        assert_eq!(super::file_name([Some("   ".into())]), "document.pdf");
        assert_eq!(super::file_name([None]), "document.pdf");
        // The file's own name first, the handle's when the file's is blank.
        assert_eq!(super::file_name([Some("a.pdf".into()), Some("b.pdf".into())]), "a.pdf");
        assert_eq!(super::file_name([Some(String::new()), Some("b.pdf".into())]), "b.pdf");
    }

    #[test]
    fn launched_file_size_limit() {
        let limit = 512u64 << 20;
        assert!(super::check_size(0.0, limit).is_ok());
        assert!(super::check_size(limit as f64, limit).is_ok(), "exactly at the limit opens");
        let over = super::check_size(limit as f64 + 1.0, limit).expect_err("over the limit is refused");
        assert!(over.contains("512 MiB"), "{over}");
        assert!(super::check_size(f64::NAN, limit).is_err(), "a size that is not a number is refused");
        assert!(super::check_size(f64::INFINITY, limit).is_err());
    }
}
