//! Rust/WASM bindings to the bundled native mobile host; no handwritten JavaScript.
use eframe::wasm_bindgen::{self, JsCast, JsValue, closure::Closure};
use pdfcraft_ui_egui::{
    PdfCraftApp,
    host_files::{HostDocument, HostFiles},
};
use std::cell::Cell;
use std::rc::Rc;

const MAX_DOCUMENT: u32 = 64 * 1024 * 1024;

#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
    async fn invoke(command: &str, args: JsValue, options: JsValue) -> Result<JsValue, JsValue>;
}

fn error(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}

fn object(fields: &[(&str, JsValue)]) -> Result<JsValue, String> {
    let object = js_sys::Object::new();
    for (key, value) in fields {
        js_sys::Reflect::set(&object, &JsValue::from_str(key), value).map_err(error)?;
    }
    Ok(object.into())
}

async fn opened(command: &str) -> Result<Option<HostDocument>, String> {
    let selected = invoke(command, JsValue::NULL, JsValue::UNDEFINED).await.map_err(error)?;
    if selected.is_null() || selected.is_undefined() {
        return Ok(None);
    }
    let name = js_sys::Reflect::get(&selected, &"name".into()).map_err(error)?.as_string().ok_or("The file picker returned no document name")?;
    let ticket = js_sys::Reflect::get(&selected, &"ticket".into()).map_err(error)?;
    let response = invoke("mobile_read", object(&[("ticket", ticket)])?, JsValue::UNDEFINED).await.map_err(error)?;
    let bytes = if let Some(array) = response.dyn_ref::<js_sys::Uint8Array>() {
        array.clone()
    } else if response.is_instance_of::<js_sys::ArrayBuffer>() {
        js_sys::Uint8Array::new(&response)
    } else {
        return Err("The file host returned an invalid document transfer".into());
    };
    if bytes.length() == 0 || bytes.length() > MAX_DOCUMENT {
        return Err("Mobile documents must contain 1 byte to 64 MiB".into());
    }
    let origin = js_sys::Reflect::get(&selected, &"origin".into()).map_err(error)?.as_string();
    Ok(Some(HostDocument { name, bytes: bytes.to_vec(), origin }))
}

pub fn attach(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(window) = web_sys::window() else { return };
    // A regular browser can still preview the mobile bundle without native bindings.
    if !js_sys::Reflect::get(&window, &"__TAURI__".into()).is_ok_and(|value| !value.is_undefined() && !value.is_null()) {
        return;
    }
    app.host_files = Some(HostFiles::new(
        Box::new(|reply| {
            wasm_bindgen_futures::spawn_local(async move {
                reply(opened("mobile_open").await);
            });
            Ok(())
        }),
        Box::new(|request, reply| {
            let bytes = request.bytes;
            if bytes.is_empty() || bytes.len() > MAX_DOCUMENT as usize {
                return Err("Mobile documents must contain 1 byte to 64 MiB".into());
            }
            let name: JsValue = js_sys::encode_uri_component(&request.name).into();
            let mut headers = vec![("x-pdfcraft-name", name)];
            if let Some(origin) = request.origin {
                headers.push(("x-pdfcraft-origin", origin.into()));
            }
            let options = object(&[("headers", object(&headers)?)])?;
            let data: JsValue = js_sys::Uint8Array::from(bytes.as_slice()).into();
            wasm_bindgen_futures::spawn_local(async move {
                let saved = invoke("mobile_save", data, options)
                    .await
                    .map_err(error)
                    .and_then(|value| value.as_bool().ok_or_else(|| "The file host returned no save confirmation".into()));
                reply(saved);
            });
            Ok(())
        }),
    ));
    // A bounded native queue retains cold-start and warm document-open intents.
    let busy = Rc::new(Cell::new(false));
    let Some(inbox) = app.host_files.as_ref().map(|host| host.incoming.clone()) else { return };
    let failures = app.failed_inbox.clone();
    let ctx = ctx.clone();
    let poll = Closure::<dyn FnMut()>::new(move || {
        if inbox.lock().is_ok_and(|queue| queue.len() >= 8) {
            return;
        }
        if busy.replace(true) {
            return;
        }
        let busy = busy.clone();
        let inbox = inbox.clone();
        let failures = failures.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            match opened("mobile_incoming").await {
                Ok(Some(file)) => {
                    if let Ok(mut queue) = inbox.lock() {
                        queue.push(file);
                    }
                }
                Err(error) => {
                    if let Ok(mut queue) = failures.lock() {
                        queue.push(("document.pdf".into(), error));
                    }
                }
                Ok(None) => {}
            }
            busy.set(false);
            ctx.request_repaint();
        });
    });
    if window.set_interval_with_callback_and_timeout_and_arguments_0(poll.as_ref().unchecked_ref(), 750).is_ok() {
        poll.forget();
    }
}
