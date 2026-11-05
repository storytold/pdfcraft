//! Request-signatures UI, pending craft-libs#160.
//!
//! This dialog's state lives here, the same way it did when it was hosted
//! through CollabCraft. No backend exists in this repository today: see
//! `crates/ui-egui/src/cloud.rs`'s module doc for why. Wiring a real one in,
//! once craft-libs#160 ships a reference implementation, is [`set_backend`];
//! nothing above that seam should need to change.
//!
//! [`available`] is always false until a backend connects, so the title
//! bar's request-signatures button stays hidden rather than shown disabled.

/// Who a signature request is sent as. Shown to the people who receive it.
#[derive(Clone, Debug, Default)]
pub struct Identity {
    pub id: String,
    pub display: String,
    pub email: Option<String>,
}

/// The contract an e-sign provider implements. Pending craft-libs#160.
pub trait EsignBackend: Send + Sync {}

/// Request-signatures dialog state. Local-only placeholder (see the module doc): nothing
/// populates its backend today.
#[derive(Default)]
pub struct EsignDialog {
    backend: Option<Box<dyn EsignBackend>>,
}

impl EsignDialog {
    pub fn new() -> EsignDialog {
        EsignDialog::default()
    }
}

/// Whether a signing backend is connected.
pub fn available(dialog: &EsignDialog) -> bool {
    dialog.backend.is_some()
}

/// Open the dialog for the active document. A no-op while no backend is connected.
pub fn open(dialog: &mut EsignDialog, _document_id: String, _title: String, _etag: Option<String>, _sender: Identity) {
    let _ = available(dialog);
}

/// Draw every frame so a connected backend's capability probe and inbox keep running. A no-op
/// while none is connected.
pub fn pump(_dialog: &mut EsignDialog, _ctx: &egui::Context) {}

/// Wire in a backend. Once craft-libs#160 ships one, an app calls this the same way it would
/// have called the old `EsignBackend`.
pub fn set_backend(dialog: &mut EsignDialog, backend: impl EsignBackend + 'static) {
    dialog.backend = Some(Box::new(backend));
}

/// The title bar's "Request signatures" button. Drawn only when [`available`] is true, so
/// never today.
pub fn request_button(ui: &mut egui::Ui, dialog: &EsignDialog) -> bool {
    available(dialog) && ui.button(tl!("Request signatures")).clicked()
}
