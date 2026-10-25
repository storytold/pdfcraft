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
                    // eframe never raises a close request on a browser reload or tab close, so `guard_quit` never
                    // prompts; ask the browser to confirm instead while a document has unsaved work (#812).
                    warn_before_unload(app.unsaved_flag.clone());
                    // Host modes (`host` below): `?host=parent`, a same-origin page embedding
                    // PdfCraft in an iframe (such as the Nextcloud app), or `data-host="page"`
                    // on the canvas, a host that runs the module in its own document.
                    if host_requested()
                        && let Some(bridge) = host::Bridge::new()
                    {
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
            if host_requested()
                && let Some(bridge) = host::Bridge::new()
            {
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

/// Whether a host mode was asked for: `?host=parent` (the iframe bridge) or
/// `data-host="page"` on the canvas (the host document itself). Whether a bridge can actually
/// be built is [`host::Bridge::new`]'s answer.
#[cfg(target_arch = "wasm32")]
fn host_requested() -> bool {
    query_param("host").as_deref() == Some("parent") || page_host_enabled(app_canvas().and_then(|c| c.get_attribute("data-host")).as_deref())
}

/// Host mode on the canvas is opt-in by attribute value: only the exact `page` turns it on
/// (`data-host="page"`). Everything else — `Page`, ` page`, `parent` (the URL opt-in), empty
/// or missing — keeps the normal app with downloads.
#[cfg(any(target_arch = "wasm32", test))]
fn page_host_enabled(data_host: Option<&str>) -> bool {
    data_host == Some("page")
}

/// Host modes: how the page embedding PdfCraft passes files in and takes saves back. Both are
/// opt-in and carry the same messages; only the transport differs, so only how an event goes
/// out and how `open` comes in change.
///
/// `?host=parent` — the `postMessage` bridge for a same-origin page that embeds PdfCraft in an
/// iframe (such as the Nextcloud app). Messages are plain objects with a `type`, posted to the
/// parent window:
///
/// - `pdfcraft:ready` (to the parent): the app is listening; send files now.
/// - `pdfcraft:open` (from the parent): `{ name, bytes }`, where `bytes` is an `ArrayBuffer` or
///   `Uint8Array` holding a PDF. It opens in a tab, like a `?file=` URL.
/// - `pdfcraft:save` (to the parent): `{ name, saveAs, bytes }` when the user saves; `bytes` is
///   a transferred `ArrayBuffer`. The parent stores the file (and reports its own errors).
/// - `pdfcraft:dirty` (to the parent): `{ dirty }` whenever unsaved work appears or goes away.
/// - `pdfcraft:failed` (to the parent): `{ error }` when the app couldn't start.
///
/// Only the parent window on the app's own origin is heard or answered. With a parent on
/// another origin there is no bridge: the app runs as if unframed and saves download.
///
/// `data-host="page"` on the app's canvas — for a host
/// that loads the wasm-bindgen module into its own document and renders on one of its own
/// canvases, with no iframe. The same messages travel as DOM `CustomEvent`s dispatched on that
/// canvas: the event name carries the `type`, and `detail` holds the same fields without it
/// (`pdfcraft:ready` carries `version`, `pdfcraft:save` carries `name`, `saveAs` and a `bytes`
/// `ArrayBuffer`). The host opens a file by dispatching a `pdfcraft:open` `CustomEvent` on the
/// canvas with `detail: { name, bytes }`. Events do not bubble, so nothing outside the canvas
/// hears them, and the canvas belongs to the document that loaded the module, so there is no
/// origin check. An unframed page that asks for `?host=parent` still gets no host mode: only
/// the exact attribute value `page` opts in.
#[cfg(target_arch = "wasm32")]
mod host {
    use eframe::wasm_bindgen::{JsCast, JsValue, closure::Closure};

    use super::page_host_enabled;

    #[derive(Clone)]
    enum Transport {
        /// A same-origin parent frame: send by `postMessage` with the origin, listen for
        /// `message` events on the window.
        Parent { window: web_sys::Window, parent: web_sys::Window, origin: String },
        /// The host document itself: send by dispatching a `CustomEvent` on the app's canvas,
        /// and listen on it for `pdfcraft:open`.
        Page { canvas: web_sys::HtmlCanvasElement },
    }

    #[derive(Clone)]
    pub struct Bridge {
        transport: Transport,
    }

    impl Bridge {
        /// Picks the transport: `?host=parent` needs a framing page with a same-origin parent;
        /// otherwise `data-host="page"` on the app's canvas opts the host document itself in. `None` when the parent is on another origin (or when
        /// `?host=parent` is used unframed): messages only go to this origin, so the browser
        /// would drop every save without an error while the document showed as saved (#2167).
        pub fn new() -> Option<Self> {
            let window = web_sys::window()?;
            let canvas = super::app_canvas();
            if super::query_param("host").as_deref() != Some("parent") {
                if page_host_enabled(canvas.as_ref().and_then(|c| c.get_attribute("data-host")).as_deref()) {
                    return Some(Self { transport: Transport::Page { canvas: canvas? } });
                }
                return None;
            }
            let parent = window.parent().ok().flatten()?;
            if js_sys::Object::is(&parent, &window) {
                return None;
            }
            let origin = window.location().origin().ok()?;
            // Reading another origin's location throws, which is `None` too.
            if parent.location().origin().ok()? != origin {
                return None;
            }
            Some(Self { transport: Transport::Parent { window, parent, origin } })
        }

        /// One outbound message, whatever the transport. `fields` is the payload: the parent
        /// transport wraps it with a `type` in the posted object, the page transport puts it in
        /// the event's `detail` as-is (the event name carries the type). `transfer` hands an
        /// `ArrayBuffer` to the receiver instead of copying it (parent transport).
        fn send(&self, name: &str, fields: &[(&str, JsValue)], transfer: Option<&JsValue>) -> Result<(), String> {
            match &self.transport {
                Transport::Parent { parent, origin, .. } => {
                    let mut all = Vec::with_capacity(fields.len().saturating_add(1));
                    all.push(("type", JsValue::from_str(name)));
                    all.extend_from_slice(fields);
                    let Some(m) = message(&all) else { return Err("couldn't build the message".to_string()) };
                    let sent = match transfer {
                        Some(t) => parent.post_message_with_transfer(&m, origin, &js_sys::Array::of1(t)),
                        None => parent.post_message(&m, origin),
                    };
                    sent.map_err(|e| js_error(&e))
                }
                Transport::Page { canvas } => {
                    let Some(detail) = message(fields) else { return Err("couldn't build the message".to_string()) };
                    let init = web_sys::CustomEventInit::new();
                    // The builder methods (`bubbles(..)`, `detail(..)`) are deprecated in
                    // web-sys 0.3.106 and fail clippy -D warnings; use the setters.
                    init.set_bubbles(false);
                    init.set_detail(&detail);
                    let event = web_sys::CustomEvent::new_with_event_init_dict(name, &init).map_err(|e| js_error(&e))?;
                    // The bool says whether a listener called `preventDefault`; not a send failure.
                    canvas.dispatch_event(&event).map(|_| ()).map_err(|e| js_error(&e))?;
                    Ok(())
                }
            }
        }

        pub fn post_ready(&self) {
            let _ = self.send("pdfcraft:ready", &[("version", env!("CARGO_PKG_VERSION").into())], None);
        }

        /// The app couldn't start (no WebGL, for example): let the host say so.
        pub fn post_failed(&self, error: &str) {
            let _ = self.send("pdfcraft:failed", &[("error", error.into())], None);
        }

        /// Hear `pdfcraft:open` — from the same-origin parent (a `message` on the window) or
        /// from the host document (a `pdfcraft:open` on the canvas) — and queue the file like a
        /// `?file=` download.
        pub fn listen(&self, inbox: pdfcraft_ui_egui::Inbox, ctx: egui::Context) {
            match &self.transport {
                Transport::Parent { window, parent, origin } => {
                    let parent = parent.clone();
                    let origin = origin.clone();
                    let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
                        if event.origin() != origin || !event.source().is_some_and(|s| js_sys::Object::is(&s, &parent)) {
                            return;
                        }
                        let data = event.data();
                        if string(&data, "type").as_deref() != Some("pdfcraft:open") {
                            return;
                        }
                        queue_open(&data, &inbox, &ctx);
                    });
                    if window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).is_ok() {
                        // The listener lives as long as the page.
                        on_message.forget();
                    }
                }
                Transport::Page { canvas } => {
                    let on_open = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                        let Some(open) = event.dyn_ref::<web_sys::CustomEvent>() else { return };
                        queue_open(&open.detail(), &inbox, &ctx);
                    });
                    if canvas.add_event_listener_with_callback("pdfcraft:open", on_open.as_ref().unchecked_ref()).is_ok() {
                        // The listener lives as long as the page.
                        on_open.forget();
                    }
                }
            }
        }

        pub fn save_fn(&self) -> pdfcraft_ui_egui::HostSaveFn {
            let bridge = self.clone();
            Box::new(move |name: &str, bytes: &[u8], save_as: bool| {
                let buffer = js_sys::Uint8Array::from(bytes).buffer();
                bridge.send("pdfcraft:save", &[("name", name.into()), ("saveAs", save_as.into()), ("bytes", buffer.clone().into())], Some(&buffer))
            })
        }

        pub fn dirty_fn(&self) -> pdfcraft_ui_egui::HostDirtyFn {
            let bridge = self.clone();
            let mut last = false;
            Box::new(move |dirty: bool| {
                if dirty != last {
                    last = dirty;
                    let _ = bridge.send("pdfcraft:dirty", &[("dirty", dirty.into())], None);
                }
            })
        }
    }

    /// The `pdfcraft:open` payload, `{ name, bytes }` where `bytes` is an `ArrayBuffer` or
    /// `Uint8Array` holding a PDF: queue it like a `?file=` download and wake the app.
    fn queue_open(data: &JsValue, inbox: &pdfcraft_ui_egui::Inbox, ctx: &egui::Context) {
        let name = string(data, "name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "document.pdf".to_string());
        let Ok(bytes) = js_sys::Reflect::get(data, &JsValue::from_str("bytes")) else { return };
        let bytes = if let Some(array) = bytes.dyn_ref::<js_sys::Uint8Array>() {
            array.to_vec()
        } else if bytes.is_instance_of::<js_sys::ArrayBuffer>() {
            js_sys::Uint8Array::new(&bytes).to_vec()
        } else {
            return;
        };
        if let Ok(mut q) = inbox.lock() {
            q.push((name, bytes));
        }
        ctx.request_repaint();
    }

    fn message(fields: &[(&str, JsValue)]) -> Option<js_sys::Object> {
        let m = js_sys::Object::new();
        for (key, value) in fields {
            js_sys::Reflect::set(&m, &JsValue::from_str(key), value).ok()?;
        }
        Some(m)
    }

    fn string(data: &JsValue, key: &str) -> Option<String> {
        js_sys::Reflect::get(data, &JsValue::from_str(key)).ok()?.as_string()
    }

    fn js_error(value: &JsValue) -> String {
        value.as_string().unwrap_or_else(|| format!("{value:?}"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("pdfcraft-web targets wasm32: run `trunk serve` or `trunk build --release` in apps/pdfcraft-web");
}

#[cfg(test)]
mod page_host_tests {
    use super::page_host_enabled;

    #[test]
    fn only_the_exact_value_page_enables_the_page_host_mode() {
        assert!(page_host_enabled(Some("page")));
        for off in [None, Some(""), Some("Page"), Some("PAGE"), Some(" page"), Some("page "), Some("parent")] {
            assert!(!page_host_enabled(off), "{off:?} must keep host mode off");
        }
    }
}
