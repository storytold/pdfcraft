//! Autosave and crash recovery: a session is "crashed" by dropping the app without a clean
//! quit, and a new session must offer, recover or discard what was autosaved.

mod support;

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
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

/// Snapshot files (the PDF and its sidecar), not the running instances' lock files.
fn files_in(s: &RecoveryStore) -> usize {
    std::fs::read_dir(s.dir()).map(|d| d.flatten().filter(|e| e.path().extension().and_then(|x| x.to_str()) != Some("lock")).count()).unwrap_or(0)
}

/// A session that edits a document, autosaves, and then "crashes" (is dropped).
fn crashed_session(s: &RecoveryStore, bytes: Vec<u8>, password: Option<&str>, path: Option<&str>) {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
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
        app.set_option("language", "en").unwrap();
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
    support::wait_until(&mut h, "recovered document", |app| app.views.len() == 1);
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
    app.set_option("language", "en").unwrap();
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
            app.set_option("language", "en").unwrap();
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

/// `bytes` protected with an open password (AES-256).
fn encrypted(bytes: Vec<u8>, password: &str) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: password,
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [2; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

#[test]
fn encrypted_documents_are_autosaved_encrypted_and_recovered_with_the_password() {
    let bytes = encrypted(fixture(2), "pw");
    let s = store("encrypted");
    crashed_session(&s, bytes, Some("pw"), None);
    let meta = s.list().remove(0);
    assert!(meta.encrypted);
    let saved = s.read(&meta.key).unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains("Confidential"), "no plaintext on disk");
    let mut h = harness(s.clone());
    h.get_by_label_contains("password-protected");
    h.get_by_label("Recover").click();
    support::wait_until(&mut h, "recovery password prompt", |app| app.password_prompt.is_some());
    let field = h.get_by_role(Role::PasswordInput);
    field.focus();
    field.type_text("pw");
    h.run_steps(2);
    h.key_press(Key::Enter);
    support::wait_until(&mut h, "unlocked recovery", |app| app.views.len() == 1);
    let app = h.state();
    assert_eq!(app.views.len(), 1);
    let d = app.session.get(app.views[0].id).unwrap();
    assert!(d.dirty);
    assert_eq!(d.info.pages.len(), 1);
    let _ = Modifiers::NONE;
}

/// #813: cancelling the password prompt of a recovered snapshot must not hand its path and
/// recovery entry to the next encrypted document that is unlocked (Save would then overwrite
/// the snapshot's original file and delete the snapshot).
#[test]
fn a_cancelled_recovery_password_does_not_attach_to_the_next_unlocked_document() {
    let s = store("cancelled");
    crashed_session(&s, encrypted(fixture(2), "pw-a"), Some("pw-a"), Some("/docs/A.pdf"));
    assert_eq!(s.list().len(), 1);
    let mut h = harness(s.clone());
    h.get_by_label("Recover").click();
    support::wait_until(&mut h, "recovery password prompt", |app| app.password_prompt.is_some());
    assert!(h.state().password_prompt.is_some(), "the snapshot asks for its password");
    // The new modal registers its layer before its controls become interactive.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.query_by_label("Cancel").is_none_or(|node| node.accesskit_node().is_disabled()) {
        h.step();
        assert!(std::time::Instant::now() < deadline, "the password Cancel button did not become enabled");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.state().password_prompt.is_none());
    assert!(h.state().views.is_empty());

    let app = h.state_mut();
    app.open_bytes("B.pdf", Some("/docs/B.pdf".into()), encrypted(fixture(1), "pw-b")).unwrap();
    app.submit_password(Some("pw-b".into()));
    support::wait_until(&mut h, "unlocked B", |app| app.views.len() == 1);
    let app = h.state();
    assert_eq!(app.views.len(), 1);
    let b = app.session.get(app.views[0].id).unwrap();
    assert_eq!(b.path.as_deref(), Some("/docs/B.pdf"), "B keeps its own path");
    assert!(!b.dirty, "B is clean: it is not the recovered document");
    assert_eq!(s.list().len(), 1, "A's snapshot is still in the store");
    assert_eq!(app.recoverable.len(), 1, "and on offer again");

    // Closing B (clean, so no prompt) must not remove A's snapshot either.
    h.state_mut().execute("file.close");
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    assert_eq!(s.list().len(), 1, "A's snapshot survives closing B");
}

#[test]
fn incomplete_entries_are_preserved_until_explicitly_discarded() {
    let s = store("partial");
    std::fs::create_dir_all(s.dir()).unwrap();
    let bytes = fixture(1);
    std::fs::write(s.dir().join("orphan.pdf"), &bytes).unwrap();
    std::fs::write(s.dir().join("broken.json"), b"{ not json").unwrap();
    for _ in 0..2 {
        let entries = s.list();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "orphan");
        assert_eq!(entries[0].path, None);
        assert_eq!(s.read("orphan").unwrap(), bytes);
        assert_eq!(files_in(&s), 2, "listing must not remove any recovery files");
    }
    s.clear();
    assert!(!s.dir().join("orphan.pdf").exists());
    assert_eq!(files_in(&s), 0, "metadata without a snapshot holds no work and is cleared too");
}

#[test]
fn a_failed_metadata_write_keeps_the_completed_snapshot_recoverable() {
    let s = store("metadata-write-failure");
    let meta = pdfcraft_ui_egui::RecoveryMeta {
        key: "snapshot".into(),
        name: "work.pdf".into(),
        path: Some("original.pdf".into()),
        saved_at: 1,
        encrypted: false,
        instance: None,
    };
    // A directory at the sidecar path makes its atomic rename fail after the PDF was written.
    std::fs::create_dir_all(s.dir().join("snapshot.json")).unwrap();
    let bytes = fixture(2);
    assert!(s.write(&meta, &bytes).is_err());
    assert_eq!(s.read("snapshot").unwrap(), bytes);
    let entries = s.list();
    assert_eq!(s.read("snapshot").unwrap(), bytes, "a failed sidecar write must not lose the PDF");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "snapshot");
    assert_eq!(entries[0].path, None, "an unknown original path must require Save As");
}

#[test]
fn missing_corrupt_or_mismatched_metadata_recovers_the_snapshot_without_an_original_path() {
    for case in ["missing", "corrupt", "mismatched"] {
        let s = store(case);
        crashed_session(&s, fixture(3), None, Some("original.pdf"));
        let mut meta = s.list().remove(0);
        let saved = s.read(&meta.key).unwrap();
        let sidecar = s.dir().join(format!("{}.json", meta.key));
        match case {
            "missing" => std::fs::remove_file(&sidecar).unwrap(),
            "corrupt" => std::fs::write(&sidecar, b"{ not json").unwrap(),
            _ => {
                meta.key = "other-snapshot".into();
                std::fs::write(&sidecar, serde_json::to_vec(&meta).unwrap()).unwrap();
            }
        }
        let mut h = harness(s.clone());
        h.get_by_label("Recover").click();
        support::wait_until(&mut h, "recovery without valid metadata", |app| app.views.len() == 1);
        let app = h.state();
        assert_eq!(app.views.len(), 1, "{case}");
        let doc = app.session.get(app.views[0].id).unwrap();
        assert!(doc.dirty);
        assert_eq!(doc.path, None);
        assert_eq!(doc.info.pages.len(), 2, "the edit survived missing metadata: {case}");
        let recovered = s.list();
        assert_eq!(recovered.len(), 1);
        assert_eq!(s.read(&recovered[0].key).unwrap(), saved);
        // The damaged sidecar was replaced with metadata for the recovered snapshot.
        let sidecar: pdfcraft_ui_egui::RecoveryMeta = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
        assert_eq!((sidecar.key.as_str(), sidecar.path.as_deref()), (recovered[0].key.as_str(), None), "{case}");
    }
}

#[test]
fn an_encrypted_snapshot_without_metadata_still_requires_its_password() {
    let s = store("encrypted-orphan");
    crashed_session(&s, encrypted(fixture(2), "pw"), Some("pw"), Some("original.pdf"));
    let meta = s.list().remove(0);
    let saved = s.read(&meta.key).unwrap();
    std::fs::remove_file(s.dir().join(format!("{}.json", meta.key))).unwrap();
    let mut h = harness(s.clone());
    h.get_by_label("Recover").click();
    support::wait_until(&mut h, "orphan recovery password prompt", |app| app.password_prompt.is_some());
    assert!(h.state().views.is_empty());
    let field = h.get_by_role(Role::PasswordInput);
    field.focus();
    field.type_text("pw");
    h.run_steps(2);
    h.key_press(Key::Enter);
    support::wait_until(&mut h, "unlocked orphan recovery", |app| app.views.len() == 1);
    let app = h.state();
    assert_eq!(app.views.len(), 1);
    let doc = app.session.get(app.views[0].id).unwrap();
    assert!(doc.dirty);
    assert_eq!(doc.path, None);
    assert_eq!(doc.info.pages.len(), 1);
    assert_eq!(s.read(&meta.key).unwrap(), saved, "recovery must not rewrite encrypted bytes");
    let sidecar: pdfcraft_ui_egui::RecoveryMeta =
        serde_json::from_slice(&std::fs::read(s.dir().join(format!("{}.json", meta.key))).unwrap()).unwrap();
    assert!(sidecar.encrypted, "the replacement metadata records that the snapshot is encrypted");
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
        app.set_option("language", "en").unwrap();
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

/// A running instance's autosaves are its own (#2166): a second instance started meanwhile
/// does not offer them, so it cannot discard them either; once the first is gone, they are
/// offered. A sidecar naming a lock file outside the folder is a plain leftover. Each instance
/// is a store of its own here: a clone shares its instance.
#[test]
fn a_running_instances_autosaves_are_not_offered_to_a_second_instance() {
    let s = store("live");
    let instance = || RecoveryStore::new(s.dir());
    let mut first = PdfCraftApp::new();
    first.set_option("language", "en").unwrap();
    first.enable_recovery(instance());
    first.open_bytes("work.pdf", None, fixture(2)).unwrap();
    assert!(first.apply_edit(Edit::DeletePages { pages: vec![0] }));
    first.autosave_now();
    assert_eq!(files_in(&s), 2);
    let second = harness(instance());
    assert!(second.state().recoverable.is_empty() && second.state().dialog.is_none(), "the first instance is still running");
    drop(second);
    drop(first);
    let third = harness(instance());
    assert_eq!(third.state().recoverable.len(), 1, "offered once the first instance is gone");
    assert_eq!(files_in(&s), 2, "listing removes no snapshot");
    drop(third);
    let meta =
        std::fs::read_dir(s.dir()).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().and_then(|x| x.to_str()) == Some("json")).unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
    json["instance"] = "../../outside.lock".into();
    std::fs::write(&meta, json.to_string()).unwrap();
    let fourth = harness(instance());
    assert_eq!(fourth.state().recoverable.len(), 1, "a name outside the folder is not looked at, and the snapshot is offered");
}

/// Snapshots are unsaved copies of open documents, so on Unix only the owner may open the
/// folder: a new one is made that way, and one an earlier version left with the default mode is
/// tightened when an instance starts.
#[cfg(unix)]
#[test]
fn the_recovery_folder_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |s: &RecoveryStore| std::fs::metadata(s.dir()).unwrap().permissions().mode() & 0o777;
    let fresh = store("owner-only");
    crashed_session(&fresh, fixture(2), None, None);
    assert_eq!(files_in(&fresh), 2);
    assert_eq!(mode(&fresh), 0o700);

    let earlier = store("owner-only-earlier");
    std::fs::create_dir_all(earlier.dir()).unwrap();
    std::fs::set_permissions(earlier.dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
    earlier.claim();
    assert_eq!(mode(&earlier), 0o700);
}
