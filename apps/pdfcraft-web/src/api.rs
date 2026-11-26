//! `window.pdfcraft`: the page command API (`craft-web-api`, storytold/craft-libs#413) over the app's
//! UI control channel (`craft::automation::ControlChannel`), opt-in with `?control`.
//!
//! The crate owns the JavaScript side (`request`, `execute`, `commands`, `docInspect`, `uiInspect`,
//! `screenshot`, as promises with a timeout). This module is PdfCraft's part: a [`Request`] that
//! sends each call to the app as a `ControlRequest` and waits for its `ControlResponse`, and the table that
//! names PdfCraft's methods: its control channel drives the UI, so `commands` is `ui.commands`,
//! `execute` runs a registry command with `ui.command`, `docInspect` is `ui.state` and `uiInspect`
//! is `ui.inspect`.
//!
//! The channel keeps the accessibility tree of every frame, so it is off unless the page asks for it
//! (`?control`), as FigCraft's is. A call that gets no reply in `REPLY_DEADLINE` is withdrawn through
//! its `Dispatch`: one still queued is dropped, and one the app already started may still apply, which
//! the error says.

use serde_json::{Value, json};

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
    use std::sync::mpsc::{Receiver, Sender, TryRecvError};

    use craft::automation::protocol::REPLY_DEADLINE;
    use craft::automation::{ControlRequest, ControlResponse, Dispatch};
    use craft_web_api::methods::Methods;
    use craft_web_api::page::{PageApi, Request};
    use serde_json::{Value, json};
    use wasm_bindgen_futures::JsFuture;

    use super::command_params;

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

    /// Put `window.pdfcraft` on the page, sending its calls to the app on `requests` (the app
    /// holds the other end, `attach_control`). An existing `window.pdfcraft` keeps the functions
    /// it already has.
    pub fn install(requests: Sender<ControlRequest>) {
        let request: Request = Rc::new(move |method, params| {
            let params = if method == "ui.command" { command_params(params) } else { params };
            let (control, replies) = ControlRequest::new(method, params);
            let control = control.with_deadline();
            let dispatch = control.dispatch.clone();
            // A send that fails drops the request with its reply sender, so `wait` ends at once
            // with "the app dropped the request".
            let _ = requests.send(control);
            Box::pin(wait(replies, dispatch))
        });
        if let Err(e) = Rc::new(PageApi::new(APP, methods(), request)).install() {
            eframe::web_sys::console::warn_1(&format!("PdfCraft: couldn't install window.{APP}: {e:?}").into());
        }
    }

    /// The app's reply (already the protocol's `{ok, result}` object), polled about once a frame.
    /// When [`REPLY_DEADLINE`] passes first, the call gives up through its [`Dispatch`], which says
    /// whether the command may still run.
    async fn wait(replies: Receiver<ControlResponse>, dispatch: Dispatch) -> Value {
        let give_up_at = js_sys::Date::now() + REPLY_DEADLINE.as_millis() as f64;
        loop {
            match replies.try_recv() {
                Ok(reply) => return reply,
                Err(TryRecvError::Empty) if js_sys::Date::now() >= give_up_at => return json!({"ok": false, "error": dispatch.give_up()}),
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
}
