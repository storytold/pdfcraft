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
    let mut cancel = false;
    let mut cancel_pressed = false;
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
        let response = ui.button(tl!("Cancel"));
        cancel = response.clicked();
        cancel_pressed = response.is_pointer_button_down_on();
    });
    if cancel || modal.should_close() {
        // Dropping the receiver makes a late worker result unable to admit a document.
        app.cancel_certificate_open(prompt.pending);
        return;
    }
    // Keep a ready result pending while Cancel is held. Release decides whether the
    // click completes or is abandoned by dragging away; Escape still takes priority.
    if cancel_pressed {
        app.certificate_prompt = Some(prompt);
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        return;
    }
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
    app.certificate_prompt = Some(prompt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn ready_open_result_respects_cancel_and_escape() {
        for action in ["cancel", "abort_press", "escape", "accept"] {
            let mut app = PdfCraftApp::new();
            let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
            app.open_bytes("keep.pdf", None, bytes.to_vec()).unwrap();
            let original = app.views[0].id;
            app.session.apply(original, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Unsaved".into() }).unwrap();
            let mut prepared = Some(app.session.prepare_open("new.pdf", None, bytes.clone(), None).unwrap());
            let pending = PasswordPrompt {
                name: "new.pdf".into(),
                path: None,
                bytes,
                #[cfg(not(target_arch = "wasm32"))]
                stamp: None,
                input: String::new(),
                error: None,
                activate: true,
                recovered: None,
                then: None,
                after_open: None,
            };
            let mut prompt = OpenPrompt::new(pending);
            let (sender, receiver) = std::sync::mpsc::channel();
            prompt.receiver = Some(receiver);
            app.certificate_prompt = Some(prompt);
            let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_| app);
            h.run_steps(4);
            if matches!(action, "cancel" | "abort_press") {
                let pos = h.get_by_label("Cancel").rect().center();
                h.input_mut().events.push(egui::Event::PointerMoved(pos));
                h.run_steps(1);
                h.input_mut().events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                });
                sender.send(Ok(prepared.take().unwrap())).unwrap();
                h.run_steps(1);
                assert!(h.state().certificate_prompt.is_some(), "holding Cancel must neither cancel nor admit a ready result");
                assert_eq!(h.state().views.len(), 1);
                let release = if action == "abort_press" { pos - egui::vec2(100.0, 0.0) } else { pos };
                h.input_mut().events.push(egui::Event::PointerMoved(release));
                h.input_mut().events.push(egui::Event::PointerButton {
                    pos: release,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                });
            } else if action == "escape" {
                h.input_mut().events.push(egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                });
            }
            if !matches!(action, "cancel" | "abort_press") {
                sender.send(Ok(prepared.take().unwrap())).unwrap();
            }
            h.run_steps(2);
            assert!(h.state().certificate_prompt.is_none());
            assert_eq!(h.state().views.len(), if matches!(action, "accept" | "abort_press") { 2 } else { 1 }, "{action}");
            assert_eq!(h.state().session.get(original).unwrap().info_value("Title").as_deref(), Some("Unsaved"));
            assert!(h.state().session.get(original).unwrap().dirty);
        }
    }
}
