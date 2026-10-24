//! Browser-only CollabCraft login popup (user-gesture path).

use wasm_bindgen::JsCast as _;

/// Open the CollabCraft login page in a popup so the user returns to PDFCraft
/// after authenticating.
///
/// Called from the UI thread when the user chooses Sign in, not from the async
/// account worker (popup blockers require a user activation).
pub fn offer_login_popup(ctx: &egui::Context) {
    let Some(window) = web_sys::window() else { return };
    let Ok(path) = window.location().pathname() else { return };
    if path.starts_with("/auth/") {
        return;
    }
    let is_retry = ctx.memory(|m| m.data.get_temp::<bool>(egui::Id::new("pdfcraft:popup_retry")).unwrap_or(false));
    if is_retry {
        ctx.memory_mut(|m| m.data.remove::<bool>(egui::Id::new("pdfcraft:popup_retry")));
        return;
    }

    let popup_result =
        window.open_with_url_and_target_and_features("/auth/login", "_blank", "popup,width=520,height=620,menubar=no,toolbar=no,location=no");

    match popup_result {
        Ok(Some(popup)) => {
            let ctx = ctx.clone();
            let handle = std::rc::Rc::new(std::cell::Cell::new(None::<i32>));
            let handle_in = handle.clone();
            let cb = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
                if popup.closed().unwrap_or(true) {
                    if let (Some(id), Some(w)) = (handle_in.take(), web_sys::window()) {
                        w.clear_interval_with_handle(id);
                    } else {
                        return;
                    }
                    ctx.memory_mut(|m| {
                        m.data.insert_temp(egui::Id::new("pdfcraft:auth_done"), true);
                    });
                    ctx.request_repaint();
                }
            });
            if let Ok(id) = window.set_interval_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), 750) {
                handle.set(Some(id));
            }
            cb.forget();
        }
        _ => {
            let _ = window.location().set_href("/auth/login");
        }
    }
}
