//! PdfCraft in the browser: the Web Worker that recognises text, so reading a scanned page doesn't
//! stop the page responding and several pages are read at once. Trunk builds it alongside
//! `pdfcraft-web` (see apps/pdfcraft-web/index.html); the page starts and feeds the workers
//! (crates/ui-egui/src/ocr_ui.rs).
//!
//! Messages are plain objects with a `type`:
//!
//! - `ready` (to the page): the worker is listening; send the models now.
//! - `models` (from the page): `{ detection, recognition }`, each a `Uint8Array` holding a model
//!   file. Answered with `loaded`, or `failed` with an `error`.
//! - `page` (from the page): `{ id, width, height, pixels }`, the RGBA pixels of a rendered page
//!   as a `Uint8Array`. Answered with `lines` `{ id, lines }`, the recognised lines as
//!   `pdfcraft_ocr::lines_to_json` writes them, or `failed` `{ id, error }`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::{JsCast, closure::Closure};

    let Ok(scope) = js_sys::global().dyn_into::<web_sys::DedicatedWorkerGlobalScope>() else {
        return;
    };
    let reply_to = scope.clone();
    let mut ocr = None;
    let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
        let _ = reply_to.post_message(&worker::handle(&mut ocr, &e.data()));
    });
    scope.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    // The handler lives as long as the worker.
    on_message.forget();
    let _ = scope.post_message(&worker::message(&[("type", "ready".into())]));
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("pdfcraft-ocr-worker runs in a browser: trunk builds it with pdfcraft-web");
}

#[cfg(target_arch = "wasm32")]
mod worker {
    use pdfcraft_ocr::Ocr;
    use wasm_bindgen::{JsCast, JsValue};

    /// The models are about 12 MB together.
    const MAX_MODEL_BYTES: u32 = 64 << 20;
    /// The renderer keeps pages far smaller; anything larger isn't a page.
    const MAX_PIXEL_BYTES: u64 = 1 << 30;

    pub fn message(fields: &[(&str, JsValue)]) -> JsValue {
        let o = js_sys::Object::new();
        for (k, v) in fields {
            let _ = js_sys::Reflect::set(&o, &(*k).into(), v);
        }
        o.into()
    }

    /// Answer one message from the page.
    pub fn handle(ocr: &mut Option<Ocr>, data: &JsValue) -> JsValue {
        let get = |k: &str| js_sys::Reflect::get(data, &k.into()).unwrap_or(JsValue::UNDEFINED);
        let id = get("id");
        let reply = match get("type").as_string().as_deref() {
            Some("models") => load(&get("detection"), &get("recognition")).map(|o| {
                *ocr = Some(o);
                message(&[("type", "loaded".into())])
            }),
            Some("page") => match ocr.as_ref() {
                None => Err("the text recognition models aren't loaded".into()),
                Some(o) => recognize(o, &get("width"), &get("height"), &get("pixels"))
                    .map(|lines| message(&[("type", "lines".into()), ("id", id.clone()), ("lines", lines.into())])),
            },
            _ => Err("unknown message".into()),
        };
        reply.unwrap_or_else(|error| message(&[("type", "failed".into()), ("id", id), ("error", error.into())]))
    }

    fn bytes(v: &JsValue, max: u64) -> Result<Vec<u8>, String> {
        let a = v.dyn_ref::<js_sys::Uint8Array>().ok_or("expected a Uint8Array")?;
        if u64::from(a.length()) > max {
            return Err(format!("{} bytes is too large", a.length()));
        }
        Ok(a.to_vec())
    }

    fn load(detection: &JsValue, recognition: &JsValue) -> Result<Ocr, String> {
        let max = u64::from(MAX_MODEL_BYTES);
        Ocr::from_bytes(bytes(detection, max)?, bytes(recognition, max)?).map_err(|e| e.to_string())
    }

    fn recognize(ocr: &Ocr, width: &JsValue, height: &JsValue, pixels: &JsValue) -> Result<String, String> {
        let side = |v: &JsValue| v.as_f64().filter(|v| v.fract() == 0.0 && (1.0..=f64::from(u32::MAX)).contains(v)).map(|v| v as u32);
        let (Some(w), Some(h)) = (side(width), side(height)) else {
            return Err("the page size is not a positive whole number".into());
        };
        let px = bytes(pixels, MAX_PIXEL_BYTES)?;
        if px.len() as u64 != u64::from(w) * u64::from(h) * 4 {
            return Err("the pixels don't match the page size".into());
        }
        ocr.recognize(&px, w, h).map(|lines| pdfcraft_ocr::lines_to_json(&lines)).map_err(|e| e.to_string())
    }
}
