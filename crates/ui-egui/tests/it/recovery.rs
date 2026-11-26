//! Autosave and crash recovery: a session is "crashed" by dropping the app without a clean
//! quit, and a new session must offer, recover or discard what was autosaved.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::support::AppHarness as _;
use craft::host::{AppInfo, Host, ManualJobs, Picked, ScriptedFiles, Wake};
use craft::platform::autosave::RecoveryStore;
use egui::Key;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::Edit;
use pdfcraft_ui_egui::{AppServices, PdfCraftApp};

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

/// A recovery folder of this test's own, empty.
fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-recovery-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Snapshot files (the copy and its sidecar), not the running instances' lock files.
fn files_in(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|d| d.flatten().filter(|e| e.path().extension().and_then(std::ffi::OsStr::to_str) != Some("lock")).count())
        .unwrap_or(0)
}

/// The path of the file with `extension` in `dir`.
fn file_with(dir: &Path, extension: &str) -> PathBuf {
    std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().and_then(|x| x.to_str()) == Some(extension)).unwrap()
}

/// What the harness drives: the app's host services.
struct Rig {
    files: Arc<ScriptedFiles>,
    jobs: Arc<ManualJobs>,
}

/// An app with recovery in `dir`, built through the host the way the application builds it.
fn new_app(dir: &Path) -> (PdfCraftApp, Rig) {
    let files = Arc::new(ScriptedFiles::new());
    let jobs = Arc::new(ManualJobs::new());
    let info = AppInfo { id: "pdfcraft", name: "PdfCraft", version: env!("CARGO_PKG_VERSION") };
    let host = Host::builder(info).jobs(jobs.clone()).files(files.clone()).build();
    let wake = Wake::none();
    let clipboard = craft::platform::clipboard::Clipboard::new(craft::platform::clipboard::SystemClipboard::new());
    let (intake, _) = craft::platform::open::FileIntake::new(wake.clone());
    let watch = craft::platform::watch::FileWatch::new(wake.clone());
    // A recovery folder turns autosave and recovery on.
    let services = AppServices::new(wake, clipboard, Some(dir.to_path_buf()), None, intake, watch);
    let mut app = PdfCraftApp::with_host(Arc::new(host), services);
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    (app, Rig { files, jobs })
}

fn harness_of(app: PdfCraftApp) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        app
    });
    h.run_steps(4);
    h
}

/// Steps the harness, running the background jobs, until `done` holds.
fn settle(h: &mut Harness<'static, PdfCraftApp>, rig: &Rig, what: &str, done: impl Fn(&PdfCraftApp) -> bool) {
    for _ in 0..400 {
        rig.jobs.run_all();
        h.step();
        if done(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("{what} never happened");
}

/// A session that edits a document, autosaves, and then "crashes" (is dropped).
fn crashed_session(dir: &Path, bytes: Vec<u8>, password: Option<&str>, path: Option<&str>) {
    let (app, rig) = new_app(dir);
    let mut h = harness_of(app);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "work.pdf", path.map(str::to_string), bytes).unwrap();
    if let Some(pw) = password {
        // The open is queued: the prompt comes when the worker finds the document locked.
        settle(&mut h, &rig, "the password prompt", |app| app.password_prompt.is_some());
        h.submit_password(Some(pw.to_string()));
    }
    settle(&mut h, &rig, "the document opening", |app| app.views.len() == 1);
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, Edit::DeletePages { pages: vec![0] }));
    h.state_mut().autosave_now();
    // Dropped without a clean quit: the recovery entry stays.
}

/// What a store opened after the crash lists.
fn leftovers(dir: &Path) -> Vec<craft::platform::autosave::RecoveryEntry> {
    RecoveryStore::open(dir).unwrap().leftovers()
}

/// The label of the question's "don't save" button on this platform.
fn dont_save() -> &'static str {
    // PLAN: the words come from `DialogWords` (`unsaved_dont_save` on macOS, `unsaved_no` elsewhere); literal here as the catalog's English.
    "Don't save"
}

#[test]
fn a_crashed_session_is_offered_and_recovered_unsaved() {
    let dir = folder("recover");
    crashed_session(&dir, fixture(3), None, Some("/docs/work.pdf"));
    assert_eq!(leftovers(&dir).len(), 1);
    let (app, rig) = new_app(&dir);
    let mut h = harness_of(app);
    h.get_by_role_and_label(Role::Dialog, "Recover Unsaved Work");
    h.get_by_label_contains("work.pdf");
    h.get_by_label("Recover").click();
    settle(&mut h, &rig, "recovered document", |app| app.views.len() == 1);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert!(doc.dirty, "recovered changes are unsaved");
    assert_eq!(doc.path.as_deref(), Some("/docs/work.pdf"), "the original path is kept, as what Save offers (#3923)");
    assert_eq!(doc.info.pages.len(), 2, "the edit survived the crash");
    assert_eq!(files_in(&dir), 2, "the entry stays until the document is saved or closed");
}

#[test]
fn discarding_removes_the_autosaves() {
    let dir = folder("discard");
    crashed_session(&dir, fixture(2), None, None);
    let (app, _rig) = new_app(&dir);
    let mut h = harness_of(app);
    h.get_by_label("Discard").click();
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    assert_eq!(files_in(&dir), 0);
}

#[test]
fn saving_or_closing_clears_the_entry_and_nothing_is_written_for_clean_documents() {
    let dir = folder("save");
    let (app, rig) = new_app(&dir);
    let mut h = harness_of(app);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "a.pdf", None, fixture(2)).unwrap();
    settle(&mut h, &rig, "the document opening", |app| app.views.len() == 1);
    h.state_mut().autosave_now();
    assert_eq!(files_in(&dir), 0, "no changes, no autosave");
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::RotatePages { pages: vec![0], degrees: 90 });
    h.state_mut().autosave_now();
    assert_eq!(files_in(&dir), 2);
    let out = std::env::temp_dir().join(format!("pdfcraft-recovery-saved-{}.pdf", std::process::id()));
    rig.files.queue(Picked::Paths(vec![out.clone()]));
    let ctx_h = h.ctx.clone();
    h.state_mut().save_active(&ctx_h, pdfcraft_ui_egui::SaveTarget::InPlace);
    settle(&mut h, &rig, "the save clearing the entry", |_| files_in(&dir) == 0);
    // Edit again, autosave, then close and discard.
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::RotatePages { pages: vec![0], degrees: 90 });
    h.state_mut().autosave_now();
    assert_eq!(files_in(&dir), 2);
    h.state_mut().close_tab(0);
    assert_eq!(files_in(&dir), 0, "closed: nothing to recover");
    let _ = std::fs::remove_file(out);
}

/// Whether any recovery snapshot in `dir` holds `text`, in its bytes or in a decoded stream.
fn snapshot_holds(dir: &Path, text: &[u8]) -> bool {
    let found = |data: &[u8]| data.windows(text.len()).any(|w| w == text);
    let copies = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "pdf"));
    copies.filter_map(|p| std::fs::read(p).ok()).any(|bytes| {
        found(&bytes)
            || pdfcraft_cos::Document::open(Arc::new(bytes)).is_ok_and(|doc| {
                doc.scan_objects().any(|(_, o)| matches!(&*o, pdfcraft_cos::Object::Stream(st) if st.decoded().is_ok_and(|d| found(&d))))
            })
    })
}

/// Applying redactions replaces the recovery snapshot at once: the one autosaved before still holds
/// what the marks covered, until the next autosave, and for good after a crash.
#[test]
fn applying_redactions_replaces_the_recovery_snapshot_at_once() {
    let dir = folder("redact");
    let (app, rig) = new_app(&dir);
    let mut h = harness_of(app);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "a.pdf", None, fixture(1)).unwrap();
    settle(&mut h, &rig, "the document opening", |app| app.views.len() == 1);
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, Edit::RotatePages { pages: vec![0], degrees: 90 }));
    h.state_mut().autosave_now();
    assert!(snapshot_holds(&dir, b"Confidential"), "the autosave before the redaction holds the page's text");
    // Redact pages: the current page, then apply.
    assert!(h.execute("redact.mark"));
    h.run_steps(2);
    h.execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    h.execute("redact.apply");
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    assert!(!snapshot_holds(&dir, b"Confidential"), "a recovery snapshot still holds the redacted text");
    assert_eq!(files_in(&dir), 2, "the redacted document is autosaved in its place");
}

/// An autosave that could not be written leaves the document due, so the next one writes it.
/// Renewing the snapshot after a redaction removes the old one first, so nothing else is left.
#[cfg(unix)]
#[test]
fn a_failed_autosave_is_written_by_the_next_one() {
    let dir = folder("retry");
    let (app, rig) = new_app(&dir);
    let mut h = harness_of(app);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "a.pdf", None, fixture(1)).unwrap();
    settle(&mut h, &rig, "the document opening", |app| app.views.len() == 1);
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, Edit::RotatePages { pages: vec![0], degrees: 90 }));
    // A file where the folder belongs: the folder can't be made, so nothing can be written.
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::write(&dir, b"").unwrap();
    h.state_mut().autosave_now();
    assert!(dir.is_file(), "the folder can't be made");
    std::fs::remove_file(&dir).unwrap();
    h.state_mut().autosave_now();
    assert_eq!(files_in(&dir), 2, "the snapshot that failed is written by the next autosave");
}

#[test]
fn quitting_cleanly_leaves_nothing_behind() {
    let dir = folder("quit");
    let (mut app, rig) = new_app(&dir);
    app.open_bytes(&egui::Context::default(), "q.pdf", None, fixture(2)).unwrap();
    let mut h = harness_of(app);
    settle(&mut h, &rig, "the document opening", |app| app.views.len() == 1);
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::RotatePages { pages: vec![0], degrees: 90 });
    h.state_mut().autosave_now();
    assert_eq!(files_in(&dir), 2);
    // The window's close → the question → "Don't save" → the app closes and the autosave is removed.
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    h.run_steps(2);
    h.get_by_label(dont_save()).click();
    h.run_steps(6);
    assert!(h.state().views.is_empty());
    assert_eq!(files_in(&dir), 0);
}

/// `bytes` protected with an open password (AES-256).
fn encrypted(bytes: Vec<u8>, password: &str) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
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

/// Types `password` into the password prompt and submits it.
fn type_password(h: &mut Harness<'static, PdfCraftApp>, password: &str) {
    // The prompt is laid out in the pass after the one that set it.
    for _ in 0..10 {
        if h.query_by_role_and_label(Role::PasswordInput, "Password").is_some() {
            break;
        }
        h.run_steps(1);
    }
    let field = h.get_by_role_and_label(Role::PasswordInput, "Password");
    field.focus();
    field.type_text(password);
    h.run_steps(2);
    h.key_press(Key::Enter);
}

/// Check that an encrypted document is autosaved without plaintext and recovered only after the
/// password is entered.
#[test]
fn encrypted_documents_are_autosaved_encrypted_and_recovered_with_the_password() {
    let dir = folder("encrypted");
    crashed_session(&dir, encrypted(fixture(2), "pw"), Some("pw"), None);
    let entry = leftovers(&dir).remove(0);
    assert!(entry.meta.protected);
    let saved = std::fs::read(&entry.path).unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains("Confidential"), "no plaintext on disk");
    let (app, rig) = new_app(&dir);
    let mut h = harness_of(app);
    h.get_by_label_contains("Password-protected");
    h.get_by_label("Recover").click();
    settle(&mut h, &rig, "recovery password prompt", |app| app.password_prompt.is_some());
    type_password(&mut h, "pw");
    settle(&mut h, &rig, "unlocked recovery", |app| app.views.len() == 1);
    let app = h.state();
    let d = app.session.get(app.views[0].id).unwrap();
    assert!(d.dirty);
    assert_eq!(d.info.pages.len(), 1);
}

/// #813: cancelling the password prompt of a recovered snapshot must not hand its path and
/// recovery entry to the next encrypted document that is unlocked (Save would then overwrite
/// the snapshot's original file and delete the snapshot).
#[test]
fn a_cancelled_recovery_password_does_not_attach_to_the_next_unlocked_document() {
    let dir = folder("cancelled");
    crashed_session(&dir, encrypted(fixture(2), "pw-a"), Some("pw-a"), Some("/docs/A.pdf"));
    assert_eq!(leftovers(&dir).len(), 1);
    let (app, rig) = new_app(&dir);
    let mut h = harness_of(app);
    h.get_by_label("Recover").click();
    settle(&mut h, &rig, "recovery password prompt", |app| app.password_prompt.is_some());
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
    app.open_bytes(&egui::Context::default(), "B.pdf", Some("/docs/B.pdf".into()), encrypted(fixture(1), "pw-b")).unwrap();
    settle(&mut h, &rig, "B's password prompt", |app| app.password_prompt.is_some());
    h.submit_password(Some("pw-b".into()));
    settle(&mut h, &rig, "unlocked B", |app| app.views.len() == 1);
    let app = h.state();
    let b = app.session.get(app.views[0].id).unwrap();
    assert_eq!(b.path.as_deref(), Some("/docs/B.pdf"), "B keeps its own path");
    assert!(!b.dirty, "B is clean: it is not the recovered document");
    assert_eq!(files_in(&dir), 2, "A's snapshot is still in the store");
    h.run_steps(3);
    assert!(h.query_by_label("Recover").is_some(), "and on offer again");

    // Closing B (clean, so no prompt) must not remove A's snapshot either.
    h.execute("file.close");
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    assert_eq!(files_in(&dir), 2, "A's snapshot survives closing B");
}

#[test]
fn missing_or_corrupt_metadata_recovers_the_snapshot_without_an_original_path() {
    for case in ["missing", "corrupt"] {
        let dir = folder(case);
        crashed_session(&dir, fixture(3), None, Some("original.pdf"));
        let entry = leftovers(&dir).remove(0);
        let saved = std::fs::read(&entry.path).unwrap();
        let sidecar = file_with(&dir, "json");
        match case {
            "missing" => std::fs::remove_file(&sidecar).unwrap(),
            _ => std::fs::write(&sidecar, b"{ not json").unwrap(),
        }
        let (app, rig) = new_app(&dir);
        let mut h = harness_of(app);
        h.get_by_label("Recover").click();
        settle(&mut h, &rig, "recovery without valid metadata", |app| app.views.len() == 1);
        let app = h.state();
        let doc = app.session.get(app.views[0].id).unwrap();
        assert!(doc.dirty);
        assert_eq!(doc.path, None, "{case}");
        assert_eq!(doc.info.pages.len(), 2, "the edit survived missing metadata: {case}");
        assert_eq!(std::fs::read(&entry.path).unwrap(), saved, "{case}");
        // The damaged sidecar was replaced with metadata for the recovered snapshot.
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(file_with(&dir, "json")).unwrap()).unwrap();
        assert!(json.get("original").is_none_or(serde_json::Value::is_null), "{case}");
        assert!(json.get("name").is_some(), "{case}");
    }
}

/// Check that an encrypted snapshot without metadata still asks for its password and is recovered without
/// rewriting its bytes.
#[test]
fn an_encrypted_snapshot_without_metadata_still_requires_its_password() {
    let dir = folder("encrypted-orphan");
    crashed_session(&dir, encrypted(fixture(2), "pw"), Some("pw"), Some("original.pdf"));
    let entry = leftovers(&dir).remove(0);
    let saved = std::fs::read(&entry.path).unwrap();
    std::fs::remove_file(file_with(&dir, "json")).unwrap();
    let (app, rig) = new_app(&dir);
    let mut h = harness_of(app);
    h.get_by_label("Recover").click();
    settle(&mut h, &rig, "orphan recovery password prompt", |app| app.password_prompt.is_some());
    assert!(h.state().views.is_empty());
    type_password(&mut h, "pw");
    settle(&mut h, &rig, "unlocked orphan recovery", |app| app.views.len() == 1);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert!(doc.dirty);
    assert_eq!(doc.path, None);
    assert_eq!(doc.info.pages.len(), 1);
    assert_eq!(std::fs::read(&entry.path).unwrap(), saved, "recovery must not rewrite encrypted bytes");
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(file_with(&dir, "json")).unwrap()).unwrap();
    assert_eq!(json["protected"], true, "the replacement metadata records that the snapshot is encrypted");
}
