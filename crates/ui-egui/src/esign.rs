//! Request-signatures UI, hosted through CollabCraft.
//!
//! The dialog itself lives in `craft_ui::esign`. This module holds it on the
//! PdfCraft app, opens it from the chrome, and drains send requests so the
//! host can encode PDF bytes and submit them.

use craft_cloud::Identity;
use craft_ui::esign::{EsignDialog, EsignLabels, SendRequest};

/// Re-export the backend contract the host implements.
pub use craft_ui::esign::{Answer, EsignBackend, Replier, Request};

/// Whether the dialog has heard that envelopes are available.
pub fn available(dialog: &EsignDialog) -> bool {
    dialog.available()
}

/// Open the dialog for the active document.
pub fn open(dialog: &mut EsignDialog, document_id: String, title: String, etag: Option<String>, sender: Identity) {
    dialog.open(document_id, title, etag, sender);
}

/// Draw every frame so capability probes keep running.
pub fn pump(dialog: &mut EsignDialog, ctx: &egui::Context) {
    dialog.show(ctx);
}

pub fn take_send(dialog: &mut EsignDialog) -> Option<SendRequest> {
    dialog.take_send_request()
}

pub fn note_create(dialog: &mut EsignDialog, result: Result<craft_esign::Envelope, String>) {
    dialog.note_create_result(result);
}

pub fn take_completed(dialog: &mut EsignDialog) -> Option<Vec<u8>> {
    dialog.take_completed_bytes()
}

pub fn set_backend(dialog: &mut EsignDialog, backend: impl EsignBackend + 'static) {
    dialog.set_backend(backend);
}

pub fn labels() -> EsignLabels {
    EsignLabels::default()
}

pub fn request_button(ui: &mut egui::Ui, dialog: &EsignDialog) -> bool {
    let labels = labels();
    matches!(craft_ui::esign::request_button(ui, &labels, dialog.available(), &[]), Some(craft_ui::menu::SplitHit::Primary))
}
