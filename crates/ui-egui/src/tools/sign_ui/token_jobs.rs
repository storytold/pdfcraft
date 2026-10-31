//! Cancellable Linux card jobs. Only the UI may publish signed bytes.
use super::{DigitalIdEntry, SignDraft, entry_for};
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
#[derive(Debug)]
struct SigningError {
    message: String,
    refresh: bool,
}
impl From<&str> for SigningError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl From<String> for SigningError {
    fn from(message: String) -> Self {
        Self { message, refresh: false }
    }
}

pub struct TokenSigning {
    epoch: u64,
    receiver: Receiver<Result<Arc<Vec<u8>>, SigningError>>,
    _cancel: pkcs11::CancelOnDrop,
    doc: DocId,
    generation: u64,
    saved: Arc<Vec<u8>>,
    path: PathBuf,
    destination: Option<crate::file_watch::Stamp>,
    retry: SignDraft,
    selection: DigitalIdEntry,
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
                    pkcs11::signing_identities().map_err(|e| e.to_string()).map(|ids| {
                        ids.into_iter()
                            .map(|id| {
                                let mut entry = entry_for(&pkcs11::reference(id.certificate()), id.certificate());
                                entry.token = Some(id.token_label().into());
                                if id.token_serial().trim().is_empty() {
                                    entry.unusable = Some("This card has no token serial; it cannot be bound safely to a signing selection.".into());
                                }
                                entry.signing_identity = Some(id);
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
                let selected = draft.selected.and_then(|i| self.digital_ids.get(i)).cloned();
                self.digital_ids.retain(|e| !e.path.starts_with("pkcs11:"));
                self.digital_ids.extend(entries);
                let next = selected
                    .as_ref()
                    .and_then(|p| self.digital_ids.iter().position(|e| e.path == p.path && e.signing_identity == p.signing_identity))
                    .or_else(|| {
                        if selected.as_ref().is_some_and(|e| e.path.starts_with("pkcs11:")) {
                            None
                        } else {
                            self.digital_ids.iter().position(|e| e.unusable.is_none())
                        }
                    });
                let next_entry = next.and_then(|i| self.digital_ids.get(i));
                if next != draft.selected || next_entry != selected.as_ref() {
                    draft.pin.clear();
                }
                if selected.as_ref().is_some_and(|e| e.path.starts_with("pkcs11:")) && next.is_none() {
                    draft.step = SignStep::Choose;
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
        selection: DigitalIdEntry,
        pin: SecretPin,
        options: SignOptions,
        mut retry: SignDraft,
        path: PathBuf,
    ) -> Result<(), String> {
        if self.token_signing.is_some() {
            return Err("A smartcard signature is already running.".into());
        }
        let bound = selection.signing_identity.clone().ok_or("Refresh cards and select a signing identity again.")?;
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

        std::thread::Builder::new()
            .name("pdfcraft-card-sign".into())
            .spawn(move || {
                let result = cancellation.run(|| {
                    pdfcraft_engine::guard(|| {
                        let identity = bound.with_pin(pin.value()).map_err(|e| SigningError {
                            refresh: matches!(e, pdfcraft_engine::sign::SignError::TokenChanged),
                            message: e.to_string(),
                        })?;
                        snapshot
                            .sign(&identity)
                            .map_err(|e| SigningError { refresh: matches!(e, pdfcraft_engine::EditError::TokenChanged), message: e.to_string() })
                    })
                    .unwrap_or_else(|e| Err(e.into()))
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
            selection,
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
                let result = result.and_then(|signed| self.publish_token_signature(&job, signed).map_err(SigningError::from));
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

    fn retry_token_signing(&mut self, mut job: TokenSigning, error: SigningError) {
        if !job.current(self) {
            self.notify(error.message);
            return;
        }
        job.retry.pin.clear();
        job.retry.selected = if error.refresh {
            None
        } else {
            self.digital_ids.iter().position(|e| e.path == job.selection.path && e.signing_identity == job.selection.signing_identity)
        };
        if error.refresh {
            self.digital_ids.retain(|e| e.path != job.selection.path);
        }
        if job.retry.selected.is_none() {
            job.retry.step = SignStep::Choose;
        }
        job.retry.error = Some(error.message);
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
    type SignatureSender = std::sync::mpsc::Sender<Result<Arc<Vec<u8>>, SigningError>>;
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
            selection: entry_for(
                "pkcs11:test",
                &sign::pkcs12::open(&std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap(), "test")
                    .unwrap()
                    .certificate,
            ),
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
    fn discovered_card_binding_survives_the_picker_and_refresh() {
        use egui::TextBuffer;
        const CHILD: &str = "PDFCRAFT_BOUND_SELECTION_TEST";
        if let Ok(root) = std::env::var(CHILD) {
            let root = PathBuf::from(root);
            let make_entry = || {
                let identity = pkcs11::signing_identities().unwrap().remove(0);
                let mut entry = entry_for(&pkcs11::reference(identity.certificate()), identity.certificate());
                entry.signing_identity = Some(identity);
                entry
            };
            let (mut app, doc, output, _) = fixture();
            let original = make_entry();
            app.digital_ids = vec![original.clone()];
            app.sign_draft = Some(SignDraft::new(0, None, None, None, &app.digital_ids));
            app.sign_draft.as_mut().unwrap().step = SignStep::SignAs;
            app.sign_draft.as_mut().unwrap().pin.insert_text("disposable-pin", egui::text::CharIndex(0));
            app.pick_override = Some(vec![output.to_string_lossy().into_owned()]);
            app.save_override = None;
            app.finish_signing().unwrap();
            assert!(app.token_signing.is_none(), "the picker result is deferred");
            std::fs::write(root.join("serial"), "replacement-B").unwrap();
            app.process_picked();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while app.token_signing.is_some() && std::time::Instant::now() < deadline {
                frame(&mut app);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(std::fs::read_to_string(root.join("requested-serial")).unwrap(), "original-A");
            assert!(!root.join("authenticated").exists(), "no PIN may authenticate the replacement");
            assert!(!output.exists());
            assert!(!app.session.get(doc).unwrap().is_signed());
            assert!(app.sign_draft.as_ref().unwrap().pin.value().is_none());
            assert!(app.sign_draft.as_ref().unwrap().selected.is_none());
            // A refresh with the same fingerprint at the same row must also clear a typed PIN.
            app.digital_ids = vec![original];
            app.dialog = Some(Dialog::Sign);
            app.sign_draft = Some(SignDraft::new(0, None, None, None, &app.digital_ids));
            app.sign_draft.as_mut().unwrap().pin.insert_text("disposable-pin", egui::text::CharIndex(0));
            let (tx, rx) = std::sync::mpsc::channel();
            let cancellation = pkcs11::Cancellation::default();
            app.token_discovery = Some(TokenDiscovery { receiver: rx, _cancel: cancellation.cancel_on_drop() });
            tx.send(Ok(vec![make_entry()])).unwrap();
            app.poll_token_discovery();
            assert!(app.sign_draft.as_ref().unwrap().selected.is_none());
            assert!(app.sign_draft.as_ref().unwrap().pin.value().is_none());
            return;
        }
        let root = std::env::temp_dir().join(format!("pdfcraft-bound-card-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let fixture = sign::pkcs12::open(include_bytes!("../../../../sign/tests/data/ec-p256.p12"), "test").unwrap();
        std::fs::write(root.join("certificate.json"), serde_json::to_vec(&fixture.certificate.raw).unwrap()).unwrap();
        std::fs::write(root.join("serial"), "original-A").unwrap();
        let helper = root.join("helper");
        std::fs::write(&helper, r#"#!/usr/bin/env python3
import json, os, pathlib, struct, sys
root = pathlib.Path(os.environ['PDFCRAFT_BOUND_SELECTION_TEST'])
def frame():
    n = struct.unpack('>I', sys.stdin.buffer.read(4))[0]
    return sys.stdin.buffer.read(n)
request = json.loads(frame())
pin = frame()
serial = root.joinpath('serial').read_text()
if 'List' in request:
    result = json.dumps([dict(certificate=json.loads(root.joinpath('certificate.json').read_text()), chain=[], label='Test card', serial=serial)]).encode()
    status = 0
else:
    requested = request['Sign']['identity']['serial']
    root.joinpath('requested-serial').write_text(requested)
    if requested == serial:
        root.joinpath('authenticated').write_text('unexpected authentication')
        status, result = 1, b'fixture refuses signatures'
    else:
        status, result = 2, b''
sys.stdout.buffer.write(bytes([status]) + struct.pack('>I', len(result)) + result)
"#).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let test_name = format!("{}::discovered_card_binding_survives_the_picker_and_refresh", module_path!().split_once("::").unwrap().1);
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name, "--nocapture"])
            .env(CHILD, &root)
            .env("PDFCRAFT_PKCS11_HELPER", &helper)
            .output()
            .unwrap();
        assert!(result.status.success(), "{} {}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        assert!(!root.join("authenticated").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stale_card_failure_clears_selection_and_requires_refresh() {
        let (mut app, doc, path, _) = fixture();
        let (mut job, tx, _) = job(&app, doc, path.clone());
        app.digital_ids.push(job.selection.clone());
        job.retry.selected = Some(0);
        app.token_signing = Some(job);
        tx.send(Err(SigningError { message: "Card replaced".into(), refresh: true })).unwrap();
        frame(&mut app);
        let draft = app.sign_draft.as_ref().unwrap();
        assert_eq!(draft.step, SignStep::Choose);
        assert!(draft.selected.is_none());
        assert!(draft.pin.value().is_none());
        assert!(app.digital_ids.is_empty());
        assert!(!path.exists());
        assert!(!app.session.get(doc).unwrap().is_signed());
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
