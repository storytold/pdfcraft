use super::Cards;
use crate::{PasswordPrompt, PdfCraftApp, secret_pin::SecretPin, widgets};
use std::sync::mpsc::{Receiver, TryRecvError};

pub struct OpenPrompt {
    pub pending: PasswordPrompt,
    pub cards: Cards,
    pub pin: SecretPin,
    pub error: Option<String>,
    receiver: Option<Receiver<Result<pdfcraft_engine::PreparedDocument, String>>>,
    #[cfg(target_os = "linux")]
    cancellation: Option<pdfcraft_engine::sign::pkcs11::CancelOnDrop>,
}
impl OpenPrompt {
    pub(crate) fn new(pending: PasswordPrompt) -> Self {
        Self {
            pending,
            cards: Default::default(),
            pin: Default::default(),
            error: None,
            receiver: None,
            #[cfg(target_os = "linux")]
            cancellation: None,
        }
    }
    #[cfg(target_os = "linux")]
    fn submit(&mut self, context: pdfcraft_engine::OpenContext, ctx: &egui::Context) {
        let Some(entry) = self.cards.selected_entry() else { return };
        let reference = entry.reference.clone();
        let serial = entry.token_serial.clone();
        let pin = std::mem::take(&mut self.pin);
        let (bytes, name, path) = (self.pending.bytes.clone(), self.pending.name.clone(), self.pending.path.clone());
        let ctx = ctx.clone();
        let cancellation = pdfcraft_engine::sign::pkcs11::Cancellation::default();
        self.cancellation = Some(cancellation.cancel_on_drop());
        let (tx, rx) = std::sync::mpsc::channel();
        match std::thread::Builder::new().name("pdfcraft-card-open".into()).spawn(move || {
            let result = cancellation.run(|| {
                pdfcraft_engine::guard(|| {
                    let identity = pdfcraft_engine::sign::pkcs11::find_decryption_identity(&reference).map_err(|e| e.to_string())?;
                    if identity.token_serial() != serial {
                        return Err("The selected card was replaced. Refresh cards and select it again.".into());
                    }
                    let resolver = pdfcraft_engine::certificate_security::TokenResolver { identity: &identity, pin: pin.value() };
                    context.prepare_open_with_security_resolver(name, path, bytes, &resolver).map_err(|e| e.to_string())
                })
                .unwrap_or_else(Err)
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        }) {
            Ok(_) => {
                self.receiver = Some(rx);
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }
    #[cfg(not(target_os = "linux"))]
    fn submit(&mut self, _: pdfcraft_engine::OpenContext, _: &egui::Context) {
        self.pin.clear();
        self.error = Some("Smartcard opening requires Linux.".into());
    }
}

pub(crate) fn show_open(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(mut prompt) = app.certificate_prompt.take() else { return };
    let result = prompt.receiver.as_ref().and_then(|rx| match rx.try_recv() {
        Ok(result) => Some(result),
        Err(TryRecvError::Disconnected) => Some(Err("The card-opening worker stopped.".into())),
        Err(TryRecvError::Empty) => None,
    });
    if let Some(result) = result {
        prompt.receiver = None;
        #[cfg(target_os = "linux")]
        {
            prompt.cancellation = None;
        }
        match result {
            Ok(prepared) => {
                if let Err(e) = app.finish_certificate_open(prompt.pending, prepared) {
                    app.notify(&e);
                }
                return;
            }
            Err(e) => prompt.error = Some(e),
        }
    }
    let mut cancel = false;
    let modal = widgets::show_modal(ctx, "certificate_open", |ui| {
        ui.set_width(480.0);
        ui.heading(tl!("Open with a smartcard"));
        ui.label(prompt.pending.name.chars().take(120).collect::<String>());
        let busy = prompt.receiver.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            let selected = prompt.cards.selected.clone();
            prompt.cards.body(ui);
            if selected != prompt.cards.selected {
                prompt.pin.clear();
                prompt.error = None;
            }
            let label = ui.label(tl!("Token PIN"));
            prompt.pin.field(ui, "open-token-pin").labelled_by(label.id);
            ui.label(tl!("Leave the PIN empty to use your card's PIN pad. PINs are not remembered."));
            if ui.add_enabled(prompt.cards.selected_entry().is_some(), egui::Button::new(tl!("Open with card"))).clicked() {
                prompt.submit(app.session.open_context(), ctx);
            }
        });
        if busy {
            ui.spinner();
            ui.label(tl!("Waiting for the smartcard…"));
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if let Some(e) = &prompt.error {
            ui.colored_label(egui::Color32::RED, e);
        }
        cancel = ui.button(tl!("Cancel")).clicked();
    });
    if cancel || modal.should_close() {
        // Dropping the receiver makes a late worker result unable to admit a document.
        app.cancel_certificate_open(prompt.pending);
    } else {
        app.certificate_prompt = Some(prompt);
    }
}
