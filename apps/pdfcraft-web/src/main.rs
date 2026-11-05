//! PdfCraft in the browser. Build: `cd apps/pdfcraft-web && trunk build --release`
//! (trunk generates the JS loader; no handwritten JS — plan/execution-plan.md §1).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(all(target_arch = "wasm32", feature = "mobile-host"))]
mod mobile;

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
                    // eframe never raises a close request on a browser reload or tab close, so `guard_quit` never
                    // prompts; ask the browser to confirm instead while a document has unsaved work (#812).
                    warn_before_unload(app.unsaved_flag.clone());
                    #[cfg(feature = "mobile-host")]
                    mobile::attach(&mut app, &cc.egui_ctx);
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
    const MAX_OPEN_BYTES: u64 = 512 << 20;

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

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("pdfcraft-web targets wasm32: run `trunk serve` or `trunk build --release` in apps/pdfcraft-web");
}
