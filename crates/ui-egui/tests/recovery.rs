//! Autosave and crash recovery: a session is "crashed" by dropping the app without a clean
//! quit, and a new session must offer, recover or discard what was autosaved.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Edit;
use pdfcraft_ui_egui::{PdfCraftApp, RecoveryStore};

fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Confidential {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn store(tag: &str) -> RecoveryStore {
    let dir = std::env::temp_dir().join(format!("pdfcraft-recovery-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    RecoveryStore::new(dir)
}

fn files_in(s: &RecoveryStore) -> usize {
    std::fs::read_dir(s.dir()).map(|d| d.count()).unwrap_or(0)
}

/// A session that edits a document, autosaves, and then "crashes" (is dropped).
fn crashed_session(s: &RecoveryStore, bytes: Vec<u8>, password: Option<&str>, path: Option<&str>) {
    let mut app = PdfCraftApp::new();
    app.enable_recovery(s.clone());
    match password {
        Some(pw) => {
            app.open_bytes("work.pdf", path.map(str::to_string), bytes).unwrap();
            app.submit_password(Some(pw.to_string()));
        }
        None => app.open_bytes("work.pdf", path.map(str::to_string), bytes).unwrap(),
    }
    assert!(app.apply_edit(Edit::DeletePages { pages: vec![0] }));
    app.autosave_now();
    // Dropped without a clean quit: the recovery entry stays.
}

fn harness(s: RecoveryStore) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.enable_recovery(s);
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn a_crashed_session_is_offered_and_recovered_unsaved() {
    let s = store("recover");
    crashed_session(&s, fixture(3), None, Some("/docs/work.pdf"));
    assert_eq!(s.list().len(), 1);
    let mut h = harness(s.clone());
    h.get_by_label("Recover unsaved documents?");
    h.get_by_label("work.pdf");
    h.get_by_label("Recover").click();
    h.run_steps(4);
    let app = h.state();
    assert_eq!(app.views.len(), 1);
    let doc = app.session.get(app.views[0].id).unwrap();
    assert!(doc.dirty, "recovered changes are unsaved");
    assert_eq!(doc.path.as_deref(), Some("/docs/work.pdf"), "Save goes back to the original file");
    assert_eq!(doc.info.pages.len(), 2, "the edit survived the crash");
    assert_eq!(s.list().len(), 1, "the entry stays until the document is saved or closed");
}

#[test]
fn discarding_removes_the_autosaves() {
    let s = store("discard");
    crashed_session(&s, fixture(2), None, None);
    let mut h = harness(s.clone());
    h.get_by_label("Discard").click();
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    assert_eq!(files_in(&s), 0);
}

#[test]
fn saving_or_closing_clears_the_entry_and_nothing_is_written_for_clean_documents() {
    let s = store("save");
    let mut app = PdfCraftApp::new();
    app.enable_recovery(s.clone());
    app.open_bytes("a.pdf", None, fixture(2)).unwrap();
    app.autosave_now();
    assert_eq!(files_in(&s), 0, "no changes, no autosave");
    app.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 });
    app.autosave_now();
    assert_eq!(s.list().len(), 1);
    let out = std::env::temp_dir().join(format!("pdfcraft-recovery-saved-{}.pdf", std::process::id()));
    app.save_override = Some(out.to_string_lossy().into_owned());
    assert!(app.save_active(pdfcraft_ui_egui::SaveTarget::InPlace));
    assert_eq!(files_in(&s), 0, "saved: nothing to recover");
    // Edit again, autosave, then close and discard.
    app.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 });
    app.autosave_now();
    assert_eq!(s.list().len(), 1);
    app.close_tab(0);
    assert_eq!(files_in(&s), 0, "closed: nothing to recover");
    let _ = std::fs::remove_file(out);
}

#[test]
fn quitting_cleanly_leaves_nothing_behind() {
    let s = store("quit");
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe({
        let s = s.clone();
        move |_cc| {
            let mut app = PdfCraftApp::new();
            app.enable_recovery(s);
            app.open_bytes("q.pdf", None, fixture(2)).unwrap();
            app
        }
    });
    h.run_steps(3);
    h.state_mut().apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 });
    h.state_mut().autosave_now();
    assert_eq!(s.list().len(), 1);
    // Quit → "Don't save" → the app closes and the autosave is removed.
    h.state_mut().close_request = Some(pdfcraft_ui_egui::CloseRequest::Quit);
    h.run_steps(2);
    h.get_by_label("Don't save").click();
    h.run_steps(4);
    assert!(h.state().views.is_empty());
    h.state_mut().shutdown_recovery();
    assert_eq!(files_in(&s), 0);
}

#[test]
fn encrypted_documents_are_autosaved_encrypted_and_recovered_with_the_password() {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(fixture(2))).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "pw",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [2; 32],
    })
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let s = store("encrypted");
    crashed_session(&s, bytes, Some("pw"), None);
    let meta = s.list().remove(0);
    assert!(meta.encrypted);
    let saved = s.read(&meta.key).unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains("Confidential"), "no plaintext on disk");
    let mut h = harness(s.clone());
    h.get_by_label_contains("password-protected");
    h.get_by_label("Recover").click();
    h.run_steps(3);
    let field = h.get_by_role(Role::PasswordInput);
    field.focus();
    field.type_text("pw");
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(4);
    let app = h.state();
    assert_eq!(app.views.len(), 1);
    let d = app.session.get(app.views[0].id).unwrap();
    assert!(d.dirty);
    assert_eq!(d.info.pages.len(), 1);
    let _ = Modifiers::NONE;
}

#[test]
fn incomplete_entries_are_ignored_and_cleaned_up() {
    let s = store("partial");
    std::fs::create_dir_all(s.dir()).unwrap();
    std::fs::write(s.dir().join("orphan.pdf"), b"%PDF-1.7").unwrap();
    std::fs::write(s.dir().join("broken.json"), b"{ not json").unwrap();
    assert!(s.list().is_empty());
    assert_eq!(files_in(&s), 0);
}

/// Through the control channel, the reply to a click comes after the click's effects: state read
/// right after it already reflects them (found driving the live app).
#[test]
fn control_click_effects_are_visible_when_the_reply_arrives() {
    let s = store("control");
    crashed_session(&s, fixture(2), None, None);
    let slot: std::sync::Arc<std::sync::Mutex<Option<pdfcraft_ui_egui::control::ControlClient>>> = Default::default();
    let (slot2, s2) = (slot.clone(), s.clone());
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
        let mut app = PdfCraftApp::new();
        *slot2.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
        app.enable_recovery(s2);
        app
    });
    h.run_steps(4);
    let c = slot.lock().unwrap().take().unwrap();
    let call = |h: &mut Harness<'static, PdfCraftApp>, m: &str, p: serde_json::Value| {
        let rx = c.send(m, p);
        for _ in 0..30 {
            h.step();
            if let Ok(r) = rx.try_recv() {
                return r.unwrap();
            }
        }
        panic!("no reply to {m}");
    };
    assert_eq!(call(&mut h, "ui.state", serde_json::json!({}))["dialog"], "Recovery");
    call(&mut h, "ui.click", serde_json::json!({ "label": "Discard" }));
    assert_eq!(files_in(&s), 0, "discarded by the time the click is answered");
    assert_eq!(call(&mut h, "ui.state", serde_json::json!({}))["dialog"], serde_json::Value::Null);
}
