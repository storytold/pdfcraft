//! The real Prepare a form UI, with a generated original ONNX fixture and no network.
#![cfg(not(target_arch = "wasm32"))]
use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_ui_egui::{PdfCraftApp, form_detection_ui::Installer};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn harness(directory: PathBuf, installer: Option<Installer>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.run_inline = true;
        app.form_detection.directory = Some(directory.clone());
        app.form_detection.installer = installer.clone();
        let paper = app.session.create_from_text("form", "Name: ______________________").unwrap();
        app.open_bytes("paper.pdf", None, paper.to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    h
}
fn fields(h: &Harness<'_, PdfCraftApp>) -> usize {
    let app = h.state();
    app.session.get(app.active_ids().unwrap().1).unwrap().form.len()
}
fn fixture(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(pdfcraft_form_detect::MODEL_FILE), pdfcraft_form_detect::test_model::bytes()).unwrap();
}

#[test]
fn first_use_download_requires_consent_then_detects_and_reuses_the_model() {
    let directory = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let install: Installer = Arc::new(move |dir, progress| {
        count.fetch_add(1, Ordering::Relaxed);
        assert!(progress(0, 100));
        fixture(dir);
        assert!(progress(100, 100));
        Ok(dir.join(pdfcraft_form_detect::MODEL_FILE))
    });
    let mut h = harness(directory.path().into(), Some(install));
    assert!(h.state_mut().execute("form.prepare"));
    h.run_steps(2);
    h.get_by_label("Download and detect");
    assert_eq!(fields(&h), 0);
    assert_eq!(calls.load(Ordering::Relaxed), 0, "no download before explicit consent");
    h.get_by_label("Download and detect").click();
    h.run_steps(3);
    assert_eq!(fields(&h), 3);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let id = h.state().active_ids().unwrap().1;
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Detect form fields"));
    h.state_mut().execute("edit.undo");
    assert_eq!(fields(&h), 0);
    // No reload or network request: retain the already verified detector between jobs.
    std::fs::remove_file(directory.path().join(pdfcraft_form_detect::MODEL_FILE)).unwrap();
    h.state_mut().execute("form.detect");
    h.run_steps(3);
    assert_eq!(fields(&h), 3);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn installed_model_runs_offline_without_a_prompt_and_survives_save_reopen() {
    let directory = tempfile::tempdir().unwrap();
    fixture(directory.path());
    let install: Installer = Arc::new(|_, _| panic!("an installed model must never download"));
    let mut h = harness(directory.path().into(), Some(install));
    h.state_mut().execute("form.prepare");
    h.run_steps(3);
    assert_eq!(fields(&h), 3);
    assert!(!h.state().form_detection.is_prompting());
    let id = h.state().active_ids().unwrap().1;
    let bytes = h.state_mut().session.save_bytes(id).unwrap();
    let new = h.state_mut().session.open("saved.pdf", None, bytes, None).unwrap();
    assert_eq!(h.state().session.get(new).unwrap().form.len(), 3);
}

#[test]
fn failed_download_is_retryable_without_changing_the_document() {
    let directory = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let install: Installer = Arc::new(move |dir, _| {
        if count.fetch_add(1, Ordering::Relaxed) == 0 {
            return Err(pdfcraft_form_detect::Error::Download("connection interrupted".into()));
        }
        fixture(dir);
        Ok(dir.join(pdfcraft_form_detect::MODEL_FILE))
    });
    let mut h = harness(directory.path().into(), Some(install));
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    h.get_by_label("Download and detect").click();
    h.run_steps(3);
    assert_eq!(fields(&h), 0);
    h.get_by_label("Try again").click();
    h.run_steps(3);
    assert_eq!(fields(&h), 3);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[test]
fn cancelling_the_prompt_or_a_live_download_keeps_the_document_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let (entered, receiver) = std::sync::mpsc::channel();
    let install: Installer = Arc::new(move |_, progress| {
        entered.send(()).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while progress(10, 100) && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Err(pdfcraft_form_detect::Error::Cancelled)
    });
    let mut h = harness(directory.path().into(), Some(install));
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(fields(&h), 0);
    assert!(!h.state().form_detection.is_prompting());
    h.state_mut().execute("form.detect");
    h.run_steps(2);
    h.state_mut().run_inline = false;
    h.get_by_label("Download and detect").click();
    h.run_steps(2);
    receiver.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    for _ in 0..100 {
        h.run_steps(1);
        if !h.state().form_detection.is_running() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!h.state().form_detection.is_running());
    assert_eq!(fields(&h), 0);
    assert!(!directory.path().join(pdfcraft_form_detect::MODEL_FILE).exists());
    let id = h.state().active_ids().unwrap().1;
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), None);
}

#[test]
fn consent_blocks_document_shortcuts_and_escape_restores_them() {
    let directory = tempfile::tempdir().unwrap();
    let mut h = harness(directory.path().into(), None);
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    assert!(h.state().form_detection.is_prompting());
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::W);
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 1, "the consent dialog owns document shortcuts");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().form_detection.is_prompting());

    let id = h.state().active_ids().unwrap().1;
    h.state_mut().session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    h.state_mut().execute("form.detect");
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(2);
    assert_eq!(h.state().session.get(id).unwrap().info.pages[0].rotation, 90, "undo cannot edit behind the dialog");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(2);
    assert_eq!(h.state().session.get(id).unwrap().info.pages[0].rotation, 0, "undo works after cancellation");
}

#[test]
fn download_blocks_close_and_escape_releases_shortcuts_before_the_worker_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let (entered, started) = std::sync::mpsc::channel();
    let (release, finish) = std::sync::mpsc::channel();
    let finish = std::sync::Mutex::new(finish);
    let install: Installer = Arc::new(move |_, progress| {
        assert!(progress(1, 100));
        entered.send(()).unwrap();
        let _ = finish.lock().unwrap().recv_timeout(std::time::Duration::from_secs(5));
        Err(pdfcraft_form_detect::Error::Cancelled)
    });
    let mut h = harness(directory.path().into(), Some(install));
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    h.state_mut().run_inline = false;
    h.get_by_label("Download and detect").click();
    h.run_steps(2);
    started.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::W);
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 1, "the download dialog owns document shortcuts");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().form_detection.is_running(), "the cancelled worker is still waiting");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::W);
    h.run_steps(2);
    assert!(h.state().views.is_empty(), "cancellation releases shortcuts before the worker exits");
    release.send(()).unwrap();
}
