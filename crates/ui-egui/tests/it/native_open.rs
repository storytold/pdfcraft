//! OS file events retain a modal's document, then resume ordinary native intake.
#![cfg(not(target_arch = "wasm32"))]

use super::support;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{DocId, Session};
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};
use pdfcraft_ui_egui::{Dialog, OsEvent, PdfCraftApp, SaveTarget};

struct Files(PathBuf);

impl Files {
    fn new(case: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pdfcraft-native-open-{}-{case}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, text: &str) -> (String, Arc<Vec<u8>>) {
        let bytes = Session::new().create_from_text(name, text).unwrap();
        let path = self.path(name);
        std::fs::write(&path, bytes.as_slice()).unwrap();
        (path, bytes)
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

type Events = Rc<RefCell<Vec<OsEvent>>>;

fn harness(a: &str, b: &str) -> (Harness<'static, PdfCraftApp>, Events) {
    let events: Events = Rc::default();
    let polled = events.clone();
    let (a, b) = (a.to_owned(), b.to_owned());
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_path(&a);
        app.open_path(&b);
        app.active = Some(0);
        app.os_events = Some(Box::new(move || std::mem::take(&mut *polled.borrow_mut())));
        app
    });
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 2);
    assert!(!h.state().run_inline, "new native files must use the actual worker");
    (h, events)
}

fn text(bytes: Arc<Vec<u8>>) -> String {
    let mut renderer = PageRenderer::new(bytes, Default::default());
    let output = renderer.render(RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
    output.text.unwrap().plain_text().trim().to_owned()
}

fn assert_document(app: &PdfCraftApp, id: DocId, rotation: u16, expected_text: &str, dirty: bool) {
    let doc = app.session.get(id).unwrap();
    assert_eq!(doc.info.pages.iter().map(|page| page.rotation).collect::<Vec<_>>(), [rotation]);
    assert_eq!(text(app.session.save_bytes(id).unwrap()), expected_text);
    assert_eq!(doc.dirty, dirty);
    assert_eq!(doc.can_undo().is_some(), dirty);
    assert!(doc.can_redo().is_none());
}

fn rotate(h: &mut Harness<'static, PdfCraftApp>) {
    h.state_mut().run_command("page.rotate_dialog");
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::RotatePages));
}

#[test]
fn native_open_keeps_the_modal_recipient_active_until_cancel() {
    let files = Files::new("modal-focus");
    let (a, original_a) = files.write("a.pdf", "Recipient A");
    let (b, original_b) = files.write("b.pdf", "Recipient B");
    let (mut h, events) = harness(&a, &b);
    let a_id = h.state().views[0].id;
    let b_id = h.state().views[1].id;
    rotate(&mut h);
    events.borrow_mut().push(OsEvent::Open(vec![b.clone()]));
    h.run_steps(3);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(a_id), "OS Open must not change A's modal recipient");
    assert_document(h.state(), a_id, 0, "Recipient A", false);
    assert_document(h.state(), b_id, 0, "Recipient B", false);
    h.get_by_label("Cancel").click();
    support::wait_until(&mut h, "B after modal cancellation", |app| app.active_ids().is_some_and(|(_, id)| id == b_id));
    assert_eq!(h.state().views.len(), 2);
    assert_document(h.state(), a_id, 0, "Recipient A", false);
    assert_document(h.state(), b_id, 0, "Recipient B", false);
    assert_eq!(std::fs::read(a).unwrap(), *original_a);
    assert_eq!(std::fs::read(b).unwrap(), *original_b);
}

#[test]
fn native_open_during_rotate_changes_and_saves_only_the_original_recipient() {
    let files = Files::new("modal-save");
    let (a, original_a) = files.write("a.pdf", "Recipient A");
    let (b, original_b) = files.write("b.pdf", "Recipient B");
    let (mut h, events) = harness(&a, &b);
    let a_id = h.state().views[0].id;
    let b_id = h.state().views[1].id;
    rotate(&mut h);
    events.borrow_mut().push(OsEvent::Open(vec![b.clone()]));
    h.run_steps(3);
    // Assert after OK as well: a focus-only test cannot prove which document the edit reaches.
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_document(h.state(), a_id, 90, "Recipient A", true);
    assert_document(h.state(), b_id, 0, "Recipient B", false);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(b_id));
    assert_eq!(h.state().views.len(), 2);
    h.state_mut().active = Some(0);
    assert!(h.state_mut().save_active(SaveTarget::InPlace));
    let saved = Arc::new(std::fs::read(&a).unwrap());
    assert_ne!(saved.as_slice(), original_a.as_slice());
    let mut reopened = Session::new();
    let id = reopened.open("saved A", Some(a), saved.clone(), None).unwrap();
    assert_eq!(reopened.get(id).unwrap().info.pages[0].rotation, 90);
    assert_eq!(text(saved), "Recipient A");
    let saved_b = Arc::new(std::fs::read(&b).unwrap());
    assert_eq!(saved_b.as_slice(), original_b.as_slice());
    let reopened_b = reopened.open("saved B", Some(b), saved_b.clone(), None).unwrap();
    assert_eq!(reopened.get(reopened_b).unwrap().info.pages[0].rotation, 0);
    assert_eq!(text(saved_b), "Recipient B");
    assert_eq!(h.state().session.save_bytes(b_id).unwrap().as_slice(), original_b.as_slice());
}

#[test]
fn native_open_waits_to_read_new_files_and_retains_event_order() {
    let files = Files::new("unread-order");
    let (a, _) = files.write("a.pdf", "Recipient A");
    let (b, _) = files.write("b.pdf", "Recipient B");
    let c = files.path("c.pdf");
    let (mut h, events) = harness(&a, &b);
    let a_id = h.state().views[0].id;
    let b_id = h.state().views[1].id;
    rotate(&mut h);
    events.borrow_mut().push(OsEvent::Open(vec![c.clone(), b.clone(), c.clone(), b]));
    h.run_steps(3);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(a_id));
    assert_eq!(h.state().views.len(), 2);
    assert!(h.state().toast.is_none(), "the queued missing file must not be read under the dialog");
    files.write("c.pdf", "Created after the OS event");
    h.get_by_label("Cancel").click();
    support::wait_until(&mut h, "C and the final B focus", |app| app.views.len() == 3 && app.active_ids().is_some_and(|(_, id)| id == b_id));
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 3, "duplicates focus the existing tab");
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(b_id));
    let c_id = h.state().views[2].id;
    assert_eq!(h.state().session.get(c_id).unwrap().path.as_deref(), Some(c.as_str()));
    assert_document(h.state(), c_id, 0, "Created after the OS event", false);
    assert_document(h.state(), a_id, 0, "Recipient A", false);
    assert_document(h.state(), b_id, 0, "Recipient B", false);
}

#[test]
fn native_open_without_a_modal_raises_and_focuses_an_existing_tab() {
    let files = Files::new("unblocked");
    let (a, original_a) = files.write("a.pdf", "Recipient A");
    let (b, original_b) = files.write("b.pdf", "Recipient B");
    let (mut h, events) = harness(&a, &b);
    let b_id = h.state().views[1].id;
    events.borrow_mut().push(OsEvent::Open(vec![b.clone(), b.clone()]));
    h.step();
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(b_id));
    assert_eq!(h.state().views.len(), 2);
    let commands: Vec<_> = h.output().viewport_output.values().flat_map(|viewport| viewport.commands.clone()).collect();
    assert!(commands.contains(&egui::ViewportCommand::Minimized(false)));
    assert!(commands.contains(&egui::ViewportCommand::Focus));
    assert_document(h.state(), b_id, 0, "Recipient B", false);
    assert_eq!(std::fs::read(a).unwrap(), *original_a);
    assert_eq!(std::fs::read(b).unwrap(), *original_b);
}

#[test]
fn native_open_reports_a_read_failure_and_continues_in_order() {
    let files = Files::new("read-failure");
    let (a, _) = files.write("a.pdf", "Recipient A");
    let (b, _) = files.write("b.pdf", "Recipient B");
    let (c, _) = files.write("c.pdf", "Recipient C");
    let missing = files.path("missing.pdf");
    let (mut h, events) = harness(&a, &b);
    let b_id = h.state().views[1].id;
    events.borrow_mut().push(OsEvent::Open(vec![missing, c.clone(), b]));
    support::wait_until(&mut h, "C after the failed read and final B focus", |app| {
        app.views.len() == 3 && app.active_ids().is_some_and(|(_, id)| id == b_id)
    });
    assert!(h.state().toast.as_ref().is_some_and(|(message, _)| message.contains("Couldn't read missing.pdf")));
    assert_eq!(h.state().views.len(), 3);
    let c_id = h.state().views[2].id;
    assert_eq!(h.state().session.get(c_id).unwrap().path.as_deref(), Some(c.as_str()));
    assert_document(h.state(), c_id, 0, "Recipient C", false);
    assert_document(h.state(), b_id, 0, "Recipient B", false);
}

fn encrypted_pdf() -> Vec<u8> {
    let plain = Session::new().create_from_text("secret", "Secret C").unwrap();
    let mut document = pdfcraft_cos::Document::open(plain).unwrap();
    document
        .set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            user_password: "secret",
            owner_password: "owner",
            permissions: -4,
            encrypt_metadata: true,
            seed: [17; 32],
        })
        .unwrap();
    pdfcraft_cos::write_full(&document, &Default::default()).unwrap()
}

#[test]
fn password_cancel_requests_a_frame_before_delivering_a_queued_os_open() {
    let files = Files::new("password-cancel-wake");
    let (a, original_a) = files.write("a.pdf", "Recipient A");
    let (b, original_b) = files.write("b.pdf", "Recipient B");
    let (mut h, events) = harness(&a, &b);
    let a_id = h.state().views[0].id;
    let b_id = h.state().views[1].id;
    h.state_mut().open_bytes("secret.pdf", None, encrypted_pdf()).unwrap();
    assert!(h.state().password_prompt.is_some());
    events.borrow_mut().push(OsEvent::Open(vec![b.clone()]));
    h.step();
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(a_id));
    assert_eq!(h.state().views.len(), 2);
    // Clear egui's settling demand without running application logic or consuming its queue.
    for _ in 0..2 {
        let _ = h.ctx.run_logic(&egui::RawInput::default(), |_| {});
    }
    assert!(!h.ctx.has_requested_repaint());
    let wakes = Arc::new(AtomicUsize::new(0));
    let requested = wakes.clone();
    h.ctx.set_request_repaint_callback(move |_| {
        requested.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(wakes.load(Ordering::SeqCst), 0);
    h.state_mut().submit_password(None);
    assert!(h.ctx.has_requested_repaint(), "password cancellation must request the frame that admits the queued OS path");
    assert_eq!(wakes.load(Ordering::SeqCst), 1, "Cancel wakes the integration before another app frame runs");
    assert!(h.state().password_prompt.is_none());
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(a_id));
    // This frame is justified by the wake asserted above, rather than forced until delivery.
    h.step();
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(b_id));
    assert_eq!(h.state().views.len(), 2);
    assert_document(h.state(), a_id, 0, "Recipient A", false);
    assert_document(h.state(), b_id, 0, "Recipient B", false);
    assert_eq!(std::fs::read(a).unwrap(), *original_a);
    assert_eq!(std::fs::read(b).unwrap(), *original_b);
}

#[test]
fn native_open_password_retry_or_cancel_retains_later_requests() {
    for cancel in [false, true] {
        let files = Files::new(if cancel { "password-cancel" } else { "password-retry" });
        let (a, _) = files.write("a.pdf", "Recipient A");
        let (b, _) = files.write("b.pdf", "Recipient B");
        let encrypted = encrypted_pdf();
        let c = files.path("secret.pdf");
        std::fs::write(&c, &encrypted).unwrap();
        let (mut h, events) = harness(&a, &b);
        let a_id = h.state().views[0].id;
        let b_id = h.state().views[1].id;
        events.borrow_mut().push(OsEvent::Open(vec![c.clone(), b]));
        support::wait_until(&mut h, "C's native password prompt", |app| app.password_prompt.is_some());
        h.run_steps(3);
        assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(a_id));
        assert_eq!(h.state().password_prompt.as_ref().unwrap().path.as_deref(), Some(c.as_str()));
        h.state_mut().submit_password(Some("wrong".into()));
        support::wait_until(&mut h, "the wrong-password reply", |app| app.password_prompt.as_ref().is_some_and(|prompt| prompt.error.is_some()));
        assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(a_id));
        assert_eq!(h.state().views.len(), 2, "a wrong password creates no tab");
        h.run_steps(3);
        assert_eq!(h.state().password_prompt.as_ref().unwrap().path.as_deref(), Some(c.as_str()));
        if cancel {
            h.get_by_label("Cancel").click();
        } else {
            h.state_mut().submit_password(Some("secret".into()));
        }
        support::wait_until(&mut h, "B after password completion", |app| {
            app.password_prompt.is_none() && app.views.len() == if cancel { 2 } else { 3 } && app.active_ids().is_some_and(|(_, id)| id == b_id)
        });
        assert_document(h.state(), a_id, 0, "Recipient A", false);
        assert_document(h.state(), b_id, 0, "Recipient B", false);
        if !cancel {
            let doc = h.state().session.get(h.state().views[2].id).unwrap();
            assert_eq!(doc.path.as_deref(), Some(c.as_str()));
            assert_eq!(doc.info.pages[0].rotation, 0);
            let mut renderer =
                PageRenderer::new(doc.bytes.clone(), pdfcraft_render::RenderConfig { password: Some(Arc::from("secret")), ..Default::default() });
            let out = renderer.render(RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
            assert_eq!(out.text.unwrap().plain_text().trim(), "Secret C");
            assert!(!doc.dirty && doc.can_undo().is_none() && doc.can_redo().is_none());
        }
        assert_eq!(std::fs::read(c).unwrap(), encrypted);
    }
}

#[test]
fn pending_duplicate_native_events_keep_first_order_and_allow_later_reopens() {
    let files = Files::new("pending-dedup");
    let (a, original_a) = files.write("a.pdf", "Recipient A");
    let (b, original_b) = files.write("b.pdf", "Recipient B");
    let (mut h, events) = harness(&a, &b);
    let a_id = h.state().views[0].id;
    let b_id = h.state().views[1].id;
    rotate(&mut h);
    events.borrow_mut().push(OsEvent::Open(vec![b.clone(), a.clone(), b.clone()]));
    h.step();
    events.borrow_mut().push(OsEvent::Open(vec![a.clone(), b.clone()]));
    h.step();
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(a_id));
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(a_id), "B,A,B coalesces to B,A across pending batches");
    events.borrow_mut().push(OsEvent::Open(vec![b.clone()]));
    h.step();
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(b_id), "B is a fresh request after its earlier admission");
    assert_eq!(h.state().views.len(), 2);
    assert_document(h.state(), a_id, 0, "Recipient A", false);
    assert_document(h.state(), b_id, 0, "Recipient B", false);
    assert_eq!(std::fs::read(a).unwrap(), *original_a);
    assert_eq!(std::fs::read(b).unwrap(), *original_b);
}

#[test]
fn native_open_resumes_after_each_rotate_response_in_normal_and_full_screen_views() {
    for full_screen in [false, true] {
        for response in ["Cancel", "Escape", "OK"] {
            let files = Files::new(&format!("rotate-close-{full_screen}-{response}"));
            let (a, original_a) = files.write("a.pdf", "Recipient A");
            let (b, original_b) = files.write("b.pdf", "Recipient B");
            let (mut h, events) = harness(&a, &b);
            let a_id = h.state().views[0].id;
            let b_id = h.state().views[1].id;
            h.state_mut().apply_full_screen(full_screen);
            rotate(&mut h);
            events.borrow_mut().push(OsEvent::Open(vec![b.clone()]));
            h.step();
            assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(a_id));
            assert_document(h.state(), a_id, 0, "Recipient A", false);
            if response == "Escape" {
                h.key_press(egui::Key::Escape);
            } else {
                h.get_by_label(response).click();
            }
            h.run_steps(2);
            assert!(h.state().dialog.is_none(), "{response}, full_screen={full_screen}");
            assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(b_id));
            assert_eq!(h.state().views.len(), 2);
            let edited = response == "OK";
            assert_document(h.state(), a_id, if edited { 90 } else { 0 }, "Recipient A", edited);
            assert_document(h.state(), b_id, 0, "Recipient B", false);
            assert_eq!(std::fs::read(a).unwrap(), *original_a);
            assert_eq!(std::fs::read(b).unwrap(), *original_b);
        }
    }
}

fn full_screen_pdf(name: &str, content: &str) -> Vec<u8> {
    let bytes = Session::new().create_from_text(name, content).unwrap();
    let mut document = pdfcraft_cos::Document::open(bytes).unwrap();
    let root = document.root().unwrap();
    let pdfcraft_cos::Object::Dict(mut catalog) = document.get(root).as_ref().clone() else { panic!("catalog") };
    catalog.set(b"PageMode".to_vec(), pdfcraft_cos::Object::name("FullScreen"));
    document.set(root, catalog);
    pdfcraft_cos::write_full(&document, &Default::default()).unwrap()
}

#[test]
fn native_full_screen_consent_retains_existing_and_new_recipients() {
    for allow in [false, true] {
        let files = Files::new(&format!("full-screen-{allow}"));
        let (a, original_a) = files.write("a.pdf", "Recipient A");
        let (b, original_b) = files.write("b.pdf", "Recipient B");
        let c = files.path("c.pdf");
        let original_c = full_screen_pdf("C", "Recipient C");
        let (mut h, events) = harness(&a, &b);
        h.state_mut().open_bytes("deck.pdf", None, full_screen_pdf("Deck", "Consent owner")).unwrap();
        let owner = h.state().active_ids().unwrap().1;
        let b_id = h.state().views[1].id;
        h.run_steps(2);
        events.borrow_mut().push(OsEvent::Open(vec![b.clone(), c.clone()]));
        h.run_steps(3);
        assert_eq!(h.state().active_ids().unwrap().1, owner, "existing B must wait for A's consent");
        assert_eq!(h.state().pending_full_screen, Some(owner));
        assert_eq!(h.state().views.len(), 3);
        assert!(h.state().toast.is_none(), "C must not be read while consent is pending");
        std::fs::write(&c, &original_c).unwrap();
        h.get_by_label(if allow { "Full Screen" } else { "Not now" }).click();
        h.step();
        assert!(h.state().pending_full_screen.is_none(), "the answer closes A's consent after native intake");
        assert_eq!(h.state().active_ids().unwrap().1, owner);
        h.step();
        assert_eq!(h.state().active_ids().unwrap().1, b_id, "existing B is admitted before C");
        support::wait_until(&mut h, "C's own full-screen consent", |app| app.views.len() == 4 && app.pending_full_screen.is_some());
        let c_id = h.state().views[3].id;
        assert_eq!(h.state().pending_full_screen, Some(c_id));
        assert_eq!(h.state().active_ids().unwrap().1, c_id);
        assert_eq!(h.state().full_screen, allow);
        assert_document(h.state(), owner, 0, "Consent owner", false);
        assert_document(h.state(), b_id, 0, "Recipient B", false);
        assert_document(h.state(), c_id, 0, "Recipient C", false);
        for (path, original) in [(a, original_a.as_ref()), (b, original_b.as_ref()), (c, &original_c)] {
            let saved = Arc::new(std::fs::read(path).unwrap());
            assert_eq!(saved.as_ref(), original);
            let mut reopened = Session::new();
            let id = reopened.open("unchanged", None, saved, None).unwrap();
            assert_eq!(reopened.get(id).unwrap().info.pages[0].rotation, 0);
        }
    }
}
