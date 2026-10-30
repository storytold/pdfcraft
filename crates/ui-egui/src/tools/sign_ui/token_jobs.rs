//! Cancellable Linux card jobs. Only the UI may publish signed bytes.
use super::{DigitalIdEntry, SignDraft, entry_for, pkcs11_id};
use crate::{Dialog, PdfCraftApp, RightPanel, SignStep, secret_pin::SecretPin, widgets};
use pdfcraft_engine::{DocId, SignOptions, sign::pkcs11};
use std::path::PathBuf;
use std::sync::{
    Arc,
    mpsc::{Receiver, TryRecvError},
};

pub(crate) struct TokenDiscovery {
    receiver: Receiver<Result<Vec<DigitalIdEntry>, String>>,
    _cancel: pkcs11::CancelOnDrop,
}

/// An in-flight card signature. Dropping it cancels the helper and discards late results.
pub struct TokenSigning {
    epoch: u64,
    receiver: Receiver<Result<Arc<Vec<u8>>, String>>,
    _cancel: pkcs11::CancelOnDrop,
    doc: DocId,
    generation: u64,
    saved: Arc<Vec<u8>>,
    path: PathBuf,
    destination: Option<crate::file_watch::Stamp>,
    retry: SignDraft,
    reference: String,
}
impl TokenSigning {
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }
    fn current(&self, app: &PdfCraftApp) -> bool {
        app.active_ids().is_some_and(|(_, id)| id == self.doc)
            && app.session.get(self.doc).is_some_and(|d| d.edit_generation() == self.generation && Arc::ptr_eq(&d.saved_bytes(), &self.saved))
    }
}

impl PdfCraftApp {
    pub(crate) fn start_token_discovery(&mut self) {
        let cancellation = pkcs11::Cancellation::default();
        let owner = cancellation.cancel_on_drop();
        let (tx, rx) = std::sync::mpsc::channel();
        let context = self.ctx.clone();
        match std::thread::Builder::new().name("pdfcraft-signing-identities".into()).spawn(move || {
            let result = cancellation.run(|| {
                pdfcraft_engine::guard(|| {
                    pkcs11::list().map_err(|e| e.to_string()).map(|ids| {
                        ids.into_iter()
                            .map(|id| {
                                let mut entry = entry_for(&pkcs11::reference(&id.certificate), &id.certificate);
                                entry.token = id.friendly_name;
                                entry
                            })
                            .collect()
                    })
                })
                .unwrap_or_else(Err)
            });
            let _ = tx.send(result);
            if let Some(context) = context {
                context.request_repaint();
            }
        }) {
            Ok(_) => self.token_discovery = Some(TokenDiscovery { receiver: rx, _cancel: owner }),
            Err(e) => self.notify(format!("Could not start smartcard discovery: {e}")),
        }
    }

    pub(crate) fn poll_token_discovery(&mut self) {
        if self.dialog != Some(Dialog::Sign) || self.sign_draft.is_none() {
            self.token_discovery = None;
            return;
        }
        let Some(job) = self.token_discovery.as_ref() else { return };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("The smartcard discovery worker stopped.".into()),
        };
        self.token_discovery = None;
        let Some(draft) = self.sign_draft.as_mut() else { return };
        match result {
            Ok(entries) => {
                let selected = draft.selected.and_then(|i| self.digital_ids.get(i)).map(|e| e.path.clone());
                self.digital_ids.retain(|e| !e.path.starts_with("pkcs11:"));
                self.digital_ids.extend(entries);
                let next = selected
                    .as_ref()
                    .and_then(|p| self.digital_ids.iter().position(|e| &e.path == p))
                    .or_else(|| self.digital_ids.iter().position(|e| e.unusable.is_none()));
                let next_path = next.and_then(|i| self.digital_ids.get(i)).map(|e| &e.path);
                if next != draft.selected || next_path != selected.as_ref() {
                    draft.pin.clear();
                }
                draft.selected = next;
                draft.confirm_remove = None;
                if draft.step == SignStep::Choose {
                    draft.error = None;
                }
            }
            Err(e) => draft.error = Some(format!("Smartcard identities could not be listed: {e}")),
        }
    }

    pub(crate) fn start_token_signing(
        &mut self,
        doc: DocId,
        reference: String,
        pin: SecretPin,
        options: SignOptions,
        mut retry: SignDraft,
        path: PathBuf,
    ) -> Result<(), String> {
        if self.token_signing.is_some() {
            return Err("A smartcard signature is already running.".into());
        }
        retry.pin.clear();
        retry.password.clear();
        let document = self.session.get(doc).ok_or("The document is closed.")?;
        let (generation, saved) = (document.edit_generation(), document.saved_bytes());
        let snapshot = self.session.prepare_sign(doc, options).map_err(|e| e.to_string())?;
        let cancellation = pkcs11::Cancellation::default();
        let owner = cancellation.cancel_on_drop();
        let (tx, rx) = std::sync::mpsc::channel();
        let context = self.ctx.clone();
        let destination = crate::file_watch::Stamp::at(&path.to_string_lossy());
        let selected_reference = reference.clone();
        std::thread::Builder::new()
            .name("pdfcraft-card-sign".into())
            .spawn(move || {
                let result = cancellation.run(|| {
                    pdfcraft_engine::guard(|| {
                        let identity = pkcs11_id(&reference, pin.value())?;
                        snapshot.sign(&identity).map_err(|e| e.to_string())
                    })
                    .unwrap_or_else(Err)
                });
                let _ = tx.send(result);
                if let Some(context) = context {
                    context.request_repaint();
                }
            })
            .map_err(|e| format!("Could not start smartcard signing: {e}"))?;
        self.token_discovery = None;
        self.token_signing_sequence = self.token_signing_sequence.wrapping_add(1);
        self.token_signing = Some(TokenSigning {
            epoch: self.token_signing_sequence,
            receiver: rx,
            _cancel: owner,
            doc,
            generation,
            saved,
            path,
            destination,
            retry,
            reference: selected_reference,
        });
        Ok(())
    }

    /// Cancel without changing the document or writing the pending signed file.
    pub fn cancel_token_signing(&mut self) {
        self.token_signing = None;
    }

    pub(crate) fn show_token_signing(&mut self, ctx: &egui::Context) -> bool {
        let Some(job) = self.token_signing.take() else { return false };
        if !job.current(self) || self.close_request.is_some() || self.dialog.is_some() {
            self.notify_tr("Smartcard signing cancelled because the document or destination changed");
            return false;
        }
        let mut cancel = false;
        let modal = widgets::show_modal(ctx, "smartcard_signing", |ui| {
            ui.set_width(420.0);
            ui.heading(tl!("Signing with a smartcard"));
            ui.spinner();
            ui.label(tl!("Waiting for the smartcard. Follow any instructions on its PIN pad."));
            let response = ui.button(tl!("Cancel signing"));
            cancel = response.clicked() || response.is_pointer_button_down_on();
        });
        // Input takes priority over a result that arrived in this same frame.
        if cancel || modal.should_close() {
            ctx.request_repaint();
            return true;
        }
        match job.receiver.try_recv() {
            Ok(result) => {
                let result = result.and_then(|signed| self.publish_token_signature(&job, signed));
                if let Err(error) = result {
                    self.retry_token_signing(job, error);
                }
                ctx.request_repaint();
                return true;
            }
            Err(TryRecvError::Disconnected) => {
                self.retry_token_signing(job, "The smartcard signing worker stopped.".into());
                ctx.request_repaint();
                return true;
            }
            Err(TryRecvError::Empty) => {}
        }
        self.token_signing = Some(job);
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
        true
    }

    fn retry_token_signing(&mut self, mut job: TokenSigning, error: String) {
        if !job.current(self) {
            self.notify(error);
            return;
        }
        job.retry.selected = self.digital_ids.iter().position(|e| e.path == job.reference);
        if job.retry.selected.is_none() {
            job.retry.step = SignStep::Choose;
        }
        job.retry.error = Some(error);
        self.sign_draft = Some(job.retry);
        self.dialog = Some(Dialog::Sign);
    }

    fn publish_token_signature(&mut self, job: &TokenSigning, signed: Arc<Vec<u8>>) -> Result<(), String> {
        if !self.commit_typing_in(job.doc) || !job.current(self) || job.destination != crate::file_watch::Stamp::at(&job.path.to_string_lossy()) {
            return Err("The document or destination changed; the smartcard signature was not saved.".into());
        }
        self.session.check_save(job.doc, &signed).map_err(|e| e.to_string())?;
        crate::editing::write_atomically(&job.path.to_string_lossy(), &signed).map_err(|e| format!("Could not save: {e}"))?;
        self.watch.known_now(job.doc, &job.path.to_string_lossy());
        self.session.mark_signed(job.doc, signed, Some(job.path.to_string_lossy().into_owned())).map_err(|e| e.to_string())?;
        self.sign_graphic = job.retry.graphic.clone();
        if let Some(view) = self.views.iter_mut().find(|v| v.id == job.doc) {
            view.invalidate_content();
        }
        self.right = Some(RightPanel::Signatures);
        self.notify_fmt("Signed and saved to {path}", &[("path", &job.path.display().to_string())]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_engine::{Edit, sign};
    type SignatureSender = std::sync::mpsc::Sender<Result<Arc<Vec<u8>>, String>>;
    fn fixture() -> (PdfCraftApp, DocId, PathBuf, Arc<Vec<u8>>) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "pdfcraft-sign-job-{}-{}.pdf",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        let mut app = PdfCraftApp::new();
        app.session = pdfcraft_engine::Session::new().with_clock(|| 1_800_000_000);
        let blank = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes("job.pdf", None, blank.to_vec()).unwrap();
        let doc = app.active_ids().unwrap().1;
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
        let identity = sign::pkcs12::open(&bytes, "test").unwrap();
        let signed = app.session.sign(doc, &identity, SignOptions::default()).unwrap();
        (app, doc, path, signed)
    }
    fn job(app: &PdfCraftApp, doc: DocId, path: PathBuf) -> (TokenSigning, SignatureSender, pkcs11::Cancellation) {
        let (tx, rx) = std::sync::mpsc::channel();
        let cancellation = pkcs11::Cancellation::default();
        let document = app.session.get(doc).unwrap();
        let draft = super::super::SignDraft::new(0, None, None, None, &[]);
        let job = TokenSigning {
            epoch: 1,
            receiver: rx,
            _cancel: cancellation.cancel_on_drop(),
            doc,
            generation: document.edit_generation(),
            saved: document.saved_bytes(),
            destination: crate::file_watch::Stamp::at(&path.to_string_lossy()),
            path,
            retry: draft,
            reference: "pkcs11:test".into(),
        };
        (job, tx, cancellation)
    }
    fn frame(app: &mut PdfCraftApp) {
        let context = egui::Context::default();
        let mut output = context.run_ui(Default::default(), |ui| {
            app.show_token_signing(ui.ctx());
        });
        // This input-only frame has no renderer to upload the font texture.
        output.textures_delta.clear();
    }
    #[test]
    fn late_signature_does_not_save_or_discard_an_edited_document() {
        let (mut app, doc, path, signed) = fixture();
        let (job, tx, _) = job(&app, doc, path.clone());
        app.token_signing = Some(job);
        app.session.apply(doc, Edit::SetInfo { key: "Title".into(), value: "Keep this unsaved edit".into() }).unwrap();
        tx.send(Ok(signed)).unwrap();
        frame(&mut app);
        assert!(!path.exists());
        assert_eq!(app.session.get(doc).unwrap().info_value("Title").as_deref(), Some("Keep this unsaved edit"));
        assert!(app.session.get(doc).unwrap().dirty);
        assert!(app.token_signing.is_none());
    }
    #[test]
    fn destination_changed_during_signing_is_never_overwritten() {
        let (mut app, doc, path, signed) = fixture();
        let (job, tx, _) = job(&app, doc, path.clone());
        app.token_signing = Some(job);
        std::fs::write(&path, b"new file from another application").unwrap();
        tx.send(Ok(signed)).unwrap();
        frame(&mut app);
        assert_eq!(std::fs::read(&path).unwrap(), b"new file from another application");
        assert!(!app.session.get(doc).unwrap().is_signed());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn cancellation_drops_ready_results_and_cancels_pending_helpers() {
        let (mut app, doc, path, signed) = fixture();
        let (job, tx, cancellation) = job(&app, doc, path.clone());
        tx.send(Ok(signed)).unwrap();
        app.token_signing = Some(job);
        app.cancel_token_signing();
        frame(&mut app);
        assert!(!path.exists());
        assert!(!app.session.get(doc).unwrap().is_signed());
        assert!(cancellation.run(pkcs11::list).unwrap_err().to_string().contains("cancelled"));
    }
    #[test]
    fn pending_signature_keeps_frames_responsive_and_can_be_cancelled() {
        let (mut app, doc, path, _) = fixture();
        let (job, _sender, _) = job(&app, doc, path);
        app.token_signing = Some(job);
        let started = std::time::Instant::now();
        for _ in 0..5 {
            frame(&mut app);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(app.token_signing.is_some());
        app.cancel_token_signing();
    }
    #[test]
    fn successful_signature_is_published_only_for_the_original_document() {
        let (mut app, doc, path, signed) = fixture();
        let (job, tx, _) = job(&app, doc, path.clone());
        app.token_signing = Some(job);
        tx.send(Ok(signed.clone())).unwrap();
        frame(&mut app);
        assert_eq!(std::fs::read(&path).unwrap(), *signed);
        assert!(app.session.get(doc).unwrap().is_signed());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn changing_tabs_or_closing_the_source_discards_the_result() {
        for close in [false, true] {
            let (mut app, doc, path, signed) = fixture();
            let (job, tx, _) = job(&app, doc, path.clone());
            app.token_signing = Some(job);
            if close {
                app.session.close(doc);
            } else {
                app.active = None;
            }
            tx.send(Ok(signed)).unwrap();
            frame(&mut app);
            assert!(!path.exists());
            assert!(app.token_signing.is_none());
        }
    }
    #[test]
    fn discovery_results_do_not_replace_a_file_identity_selection() {
        let (mut app, _, _, _) = fixture();
        let certificate = sign::pkcs12::open(&std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap(), "test")
            .unwrap()
            .certificate;
        app.digital_ids = vec![entry_for("chosen.p12", &certificate)];
        app.start_signing(0, None, None, None);
        let (tx, rx) = std::sync::mpsc::channel();
        let cancellation = pkcs11::Cancellation::default();
        app.token_discovery = Some(TokenDiscovery { receiver: rx, _cancel: cancellation.cancel_on_drop() });
        tx.send(Ok(vec![entry_for("pkcs11:found", &certificate)])).unwrap();
        app.poll_token_discovery();
        assert_eq!(app.sign_draft.as_ref().unwrap().selected, Some(0));
        assert_eq!(app.digital_ids[0].path, "chosen.p12");
        assert_eq!(app.digital_ids.len(), 2);
    }
    #[test]
    fn smartcard_progress_dialog_stays_responsive_and_cancel_discards_the_signature() {
        use egui_kittest::kittest::Queryable;
        let (mut app, doc, path, signed) = fixture();
        let (job, sender, cancellation) = job(&app, doc, path.clone());
        app.token_signing = Some(job);
        let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_| app);
        harness.run_steps(6);
        harness.get_by_label("Signing with a smartcard");
        if let Ok(path) = std::env::var("PDFCRAFT_SMARTCARD_SIGNING_SHOT") {
            harness.render().unwrap().save(path).unwrap();
        }
        let position = harness.get_by_label("Cancel signing").rect().center();
        // Put the click and ready result in this same frame, without the driver's queued hover frame.
        harness.input_mut().events.push(egui::Event::PointerMoved(position));
        for pressed in [true, false] {
            harness.input_mut().events.push(egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
        }
        sender.send(Ok(signed.clone())).unwrap();
        harness.run_steps(2);
        assert!(harness.state().token_signing.is_none());
        assert!(sender.send(Ok(signed)).is_err(), "cancelled jobs have no receiver for a late signature");
        assert!(!path.exists());
        assert!(cancellation.run(pkcs11::list).is_err());
    }
}
