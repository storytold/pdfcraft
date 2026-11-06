//! `window.pdfcraft`: the page command API (`craft-web-api`, storytold/craft-libs#413) over the app's
//! UI control channel (`pdfcraft_ui_egui::control`), opt-in with `?control`.
//!
//! The crate owns the JavaScript side (`request`, `execute`, `commands`, `docInspect`, `uiInspect`,
//! `screenshot`, as promises with a timeout). This module is PdfCraft's part: a [`Request`] that
//! sends each call through the app's [`ControlClient`] and waits for the reply, and the table that
//! names PdfCraft's methods: its control channel drives the UI, so `commands` is `ui.commands`,
//! `execute` runs a registry command with `ui.command`, `docInspect` is `ui.state` and `uiInspect`
//! is `ui.inspect`.
//!
//! The channel keeps the accessibility tree of every frame, so it is off unless the page asks for it
//! (`?control`), as FigCraft's is. PdfCraft's control requests cannot be withdrawn, so a call the page
//! gave up on still runs when the app reaches it, as it does for a desktop client that timed out.

use serde_json::{Value, json};

/// The reply of a control request as the protocol's `{ok, result}` / `{ok: false, error}` object.
fn reply_object(reply: Result<Value, String>) -> Value {
    match reply {
        Ok(result) => json!({"ok": true, "result": result}),
        Err(error) => json!({"ok": false, "error": error}),
    }
}

/// The params of `ui.command`. `execute(command, params)` sends `{command, params}`; `ui.command`
/// takes `{id}` (and `params` for `ui.panel`). A request that already names an `id` is the page's own
/// `request("ui.command", {id})` and goes through unchanged.
fn command_params(params: Value) -> Value {
    match params.get("command").and_then(Value::as_str) {
        Some(command) if params.get("id").is_none() => json!({"id": command, "params": params.get("params").cloned().unwrap_or(Value::Null)}),
        _ => params,
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::install;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::rc::Rc;
    use std::sync::mpsc::{Receiver, TryRecvError};

    use craft_web_api::methods::Methods;
    use craft_web_api::page::{PageApi, Request};
    use pdfcraft_ui_egui::control::{ControlClient, Reply};
    use serde_json::{Value, json};
    use wasm_bindgen_futures::JsFuture;

    use super::{command_params, reply_object};

    /// The app id in `window.<app>`.
    const APP: &str = "pdfcraft";

    /// How often a waiting call looks for the app's reply, in milliseconds (about a frame).
    const POLL_MS: i32 = 16;

    /// PdfCraft's control methods behind each function.
    fn methods() -> Methods {
        Methods {
            commands: Some("ui.commands"),
            execute: Some("ui.command"),
            doc_inspect: Some("ui.state"),
            ui_inspect: Some("ui.inspect"),
            screenshot: Some("ui.screenshot"),
        }
    }

    /// Put `window.pdfcraft` on the page, sending its calls through `client`. An existing
    /// `window.pdfcraft` keeps the functions it already has.
    pub fn install(client: ControlClient) {
        let request: Request = Rc::new(move |method, params| {
            let params = if method == "ui.command" { command_params(params) } else { params };
            Box::pin(wait(client.send(&method, params)))
        });
        if let Err(e) = Rc::new(PageApi::new(APP, methods(), request)).install() {
            eframe::web_sys::console::warn_1(&format!("PdfCraft: couldn't install window.{APP}: {e:?}").into());
        }
    }

    /// The app's reply, polled about once a frame.
    async fn wait(reply: Receiver<Reply>) -> Value {
        loop {
            match reply.try_recv() {
                Ok(reply) => return reply_object(reply),
                Err(TryRecvError::Empty) => sleep(POLL_MS).await,
                Err(TryRecvError::Disconnected) => return json!({"ok": false, "error": "the app dropped the request"}),
            }
        }
    }

    async fn sleep(ms: i32) {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            if let Some(window) = web_sys::window() {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
            }
        });
        let _ = JsFuture::from(promise).await;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn execute_becomes_a_ui_command_by_id() {
        assert_eq!(command_params(json!({"command": "file.word_count", "params": {"a": 1}})), json!({"id": "file.word_count", "params": {"a": 1}}));
        assert_eq!(command_params(json!({"command": "file.save", "params": {}})), json!({"id": "file.save", "params": {}}));
    }

    #[test]
    fn a_request_that_names_an_id_goes_through_unchanged() {
        let own = json!({"id": "file.save"});
        assert_eq!(command_params(own.clone()), own);
        let both = json!({"id": "ui.panel", "command": "x", "params": {"reset": true}});
        assert_eq!(command_params(both.clone()), both, "an id wins over a command");
        assert_eq!(command_params(json!({})), json!({}));
    }

    #[test]
    fn replies_become_the_protocols_objects() {
        assert_eq!(reply_object(Ok(json!({"n": 1}))), json!({"ok": true, "result": {"n": 1}}));
        assert_eq!(reply_object(Err("boom".to_string())), json!({"ok": false, "error": "boom"}));
    }
}
