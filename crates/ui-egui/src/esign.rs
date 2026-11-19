//! Request-signatures UI: craft-ui's real esign feature (storytold/craft-libs#661), not a local
//! placeholder.
//!
//! Nobody connects a backend here. `EsignDialog::set_backend` is never called anywhere in this
//! repository, so `EsignDialog::available` stays false. `request_button` (re-exported below)
//! already returns `None` while that holds, so the title bar's request-signatures button draws
//! nothing: see `chrome.rs`'s call to it.
//!
//! Independent of [`crate::cloud::available`] on purpose: an e-sign service and a cloud storage
//! account are different things to have configured. Gating one on the other was one of the
//! review's findings against the local stand-in this module carried before #661 merged.

pub use craft_ui::esign::*;

/// Drive the dialog every frame. [`EsignDialog::show`] draws it, self-gating on whether it is
/// open the same way the share dialog and the cloud conflict modal do. Anything the dialog's own
/// UI assembled into a [`SendRequest`] gets encoded and submitted the same way a cloud Save
/// would encode and submit a document: see `cloud.rs`'s `CloudApp::encode`.
pub fn pump(app: &mut crate::PdfCraftApp, ctx: &egui::Context) {
    app.esign.show(ctx);
    if let Some(request) = app.esign.take_send_request() {
        match encode_active_document(app) {
            Ok(document) => {
                let create = Request::Create { draft: Box::new(request.draft), document, idempotency_key: request.idempotency_key, send: true };
                app.esign.submit_request(create);
            }
            Err(error) => app.esign.note_create_result(Err(error)),
        }
    }
    // A finished ceremony's signed bytes open as their own document, the way a cloud read would
    // (`CloudApp::open_bytes`): a provider's sealing can change the signed bytes relative to the
    // one the envelope was created from, so this is never assumed to be the same document.
    if let Some(bytes) = app.esign.take_completed_bytes() {
        let _ = crate::PdfCraftApp::open_bytes(app, "Signed.pdf", None, bytes);
    }
}

/// The active document's bytes, the same clean full copy `CloudApp::encode` produces for a
/// cloud Save. An envelope's signed artifact must not carry revisions the ceremony never saw.
fn encode_active_document(app: &mut crate::PdfCraftApp) -> Result<Vec<u8>, String> {
    let index = app.active.ok_or_else(|| "no document is open".to_owned())?;
    let id = app.views.get(index).ok_or_else(|| "no document is open".to_owned())?.id;
    app.commit_typing_in(id);
    let bytes = app.session.save_full_bytes(id).map_err(|e| e.to_string())?;
    app.session.check_save(id, &bytes).map_err(|e| e.to_string())?;
    Ok((*bytes).clone())
}

#[cfg(test)]
#[path = "esign_tests.rs"]
mod tests;
