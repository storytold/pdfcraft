//! Synthetic files, no registry changes and no real mail-client launch.
#![cfg(not(target_arch = "wasm32"))]

use pdfcraft_ui_egui::{PdfCraftApp, ShellAction};

// Optional screenshots initialize and tear down native GPU devices. Keep both captures in
// this binary serialized, including Harness::drop; ordinary functional tests stay parallel.
static SHOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn shot_lock(variable: &str) -> Option<std::sync::MutexGuard<'static, ()>> {
    std::env::var_os(variable).map(|_| SHOT_LOCK.lock().unwrap())
}

fn source(label: &str) -> (std::path::PathBuf, String) {
    let directory = std::env::temp_dir().join(format!("pdfcraft-explorer-{}-{label}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("notes.txt");
    std::fs::write(&path, b"Synthetic source for Explorer").unwrap();
    (directory, path.to_string_lossy().into_owned())
}

#[test]
fn conversion_and_late_combines_use_existing_workspaces_without_changing_sources() {
    let (_, file) = source("convert");
    let original = std::fs::read(&file).unwrap();
    let mut app = PdfCraftApp::new();
    app.receive_shell(ShellAction::Convert, std::slice::from_ref(&file));
    assert_eq!(app.session.docs().len(), 1);
    assert!(app.session.docs()[0].dirty && app.session.docs()[0].path.is_none());
    app.receive_shell(ShellAction::Combine, std::slice::from_ref(&file));
    app.receive_shell(ShellAction::Combine, std::slice::from_ref(&file));
    assert!(app.combine_showing());
    assert_eq!(app.combine_draft.len(), 2);
    assert_eq!(app.session.docs().len(), 1, "combine waits for explicit review/confirmation");
    app.combine_staged();
    assert_eq!(app.session.docs().len(), 2);
    assert_eq!(app.session.docs()[1].info.pages.len(), 2);
    assert_eq!(std::fs::read(file).unwrap(), original);
}

#[test]
fn email_failures_keep_the_selection_and_success_preserves_late_arrivals() {
    let (parent, file) = source("email");
    let mut app = PdfCraftApp::new();
    app.receive_shell(ShellAction::Email, std::slice::from_ref(&file));
    app.receive_shell(ShellAction::Email, std::slice::from_ref(&file));
    assert_eq!(app.shell_email_paths.len(), 1, "same pending file is only added once");
    let confirmed = app.shell_email_paths.clone();
    assert!(app.save_shell_email(&confirmed, &parent.join("missing-directory")).is_err());
    assert_eq!(app.shell_email_paths, confirmed);
    let later = parent.join("later.txt");
    std::fs::write(&later, b"later source").unwrap();
    app.receive_shell(ShellAction::Email, &[later.to_string_lossy().into_owned()]);
    let draft = app.save_shell_email(&confirmed, &parent).unwrap();
    assert!(draft.is_file());
    assert!(draft.parent().unwrap().join("001-notes.pdf").is_file());
    assert_eq!(app.shell_email_paths, [later.to_string_lossy().into_owned()]);
    assert!(app.session.docs().is_empty(), "email preparation does not change open documents");
}

#[test]
fn cancelling_the_email_review_writes_nothing_and_the_window_explains_webmail() {
    let _shot = shot_lock("PDFCRAFT_SHELL_SHOT");
    use egui_kittest::{Harness, kittest::Queryable};
    let (directory, file) = source("cancel");
    let before = std::fs::read_dir(&directory).unwrap().count();
    let mut h = Harness::builder().with_size(egui::vec2(850.0, 620.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("Synthetic notes.txt", None, b"Synthetic notes for the Explorer email review.".to_vec()).unwrap();
        app.receive_shell(ShellAction::Email, &[file]);
        app
    });
    h.run_steps(3);
    h.get_by_label("Save email draft…");
    h.get_by_label("The draft contains PDF attachments. Open it with a mail app that supports .eml drafts; in Outlook.com, create a message and attach the saved PDFs yourself.");
    if let Ok(path) = std::env::var("PDFCRAFT_SHELL_SHOT") {
        h.render().unwrap().save(path).unwrap();
    }
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().shell_email_paths.is_empty());
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), before);
}

#[test]
fn email_picker_cancellation_retains_selection_and_client_failure_keeps_saved_bundle() {
    use egui_kittest::{Harness, kittest::Queryable};
    let (directory, file) = source("picker");
    let opened = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let callback = opened.clone();
    let mut h = Harness::builder().with_size(egui::vec2(850.0, 620.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("Synthetic notes.txt", None, b"Synthetic notes for the Explorer email review.".to_vec()).unwrap();
        app.receive_shell(ShellAction::Email, &[file]);
        app.open_email_draft = Some(Box::new(move |path| {
            assert!(path.is_file(), "opening only follows a completed write");
            callback.lock().unwrap().push(path.to_path_buf());
            Err("synthetic client refusal".into())
        }));
        app
    });
    h.run_steps(3);
    h.state_mut().pick_override = Some(vec![]);
    h.get_by_label("Save email draft…").click();
    h.run_steps(3);
    assert_eq!(h.state().shell_email_paths.len(), 1);
    assert!(opened.lock().unwrap().is_empty());
    h.state_mut().pick_override = Some(vec![directory.to_string_lossy().into_owned()]);
    h.get_by_label("Save email draft…").click();
    for _ in 0..3000 {
        h.run_steps(1);
        if h.state().shell_email_paths.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(h.state().shell_email_paths.is_empty());
    let saved = opened.lock().unwrap();
    assert_eq!(saved.len(), 1);
    assert!(saved[0].is_file());
    assert!(saved[0].parent().unwrap().join("001-notes.pdf").is_file());
    assert!(h.state().toast.as_ref().unwrap().0.contains("synthetic client refusal"));
}

#[test]
fn combine_review_renders_late_sources_and_failed_rows() {
    let _shot = shot_lock("PDFCRAFT_SHELL_COMBINE_SHOT");
    use egui_kittest::Harness;
    let (_, file) = source("combine-review");
    let missing = "synthetic-missing-image.png".to_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1080.0, 760.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.receive_shell(ShellAction::Combine, std::slice::from_ref(&file));
        app.receive_shell(ShellAction::Combine, &[file, missing]);
        app
    });
    h.run_steps(4);
    assert_eq!(h.state().combine_draft.len(), 3);
    assert!(h.state().combine_draft[2].problem.is_some());
    if let Ok(path) = std::env::var("PDFCRAFT_SHELL_COMBINE_SHOT") {
        h.render().unwrap().save(path).unwrap();
    }
}

#[test]
fn incoming_conversion_and_combination_wait_for_rotate_and_keep_arrival_order() {
    use egui_kittest::{Harness, kittest::Queryable};
    let (directory, file) = source("modal-order");
    let later = directory.join("later.txt");
    std::fs::write(&later, b"Later conversion").unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1080.0, 760.0)).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let original = pdfcraft_engine::Session::new().create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes("original.pdf", None, original.as_ref().clone()).unwrap();
        app.run_command("page.rotate_dialog");
        app
    });
    h.run_steps(3);
    let original = h.state().views[0].id;
    h.state_mut().receive_shell(ShellAction::Convert, std::slice::from_ref(&file));
    h.state_mut().receive_shell(ShellAction::Combine, &[file]);
    h.state_mut().receive_shell(ShellAction::Convert, &[later.to_string_lossy().into_owned()]);
    h.run_steps(3);
    assert_eq!(h.state().active, Some(0), "the pending Rotate action still targets the original");
    assert_eq!(h.state().views.len(), 1);
    assert!(h.state().combine_draft.is_empty());
    h.get_by_label("OK").click();
    h.run_steps(5);
    assert_eq!(h.state().session.get(original).unwrap().info.pages[0].rotation, 90);
    assert_eq!(h.state().views.len(), 3);
    assert_eq!(h.state().active, Some(2), "Convert → Combine → Convert retains arrival order");
    assert_eq!(h.state().combine_draft.len(), 1);
    for view in &h.state().views[1..] {
        assert_eq!(h.state().session.get(view.id).unwrap().info.pages[0].rotation, 0, "incoming documents are never rotated by the old dialog");
    }
    assert_eq!(h.state().session.get(h.state().views[2].id).unwrap().name, "later.pdf");
}

#[test]
fn explorer_combine_waits_for_an_existing_background_open() {
    use egui_kittest::Harness;

    let (directory, source) = source("background-open");
    let existing = directory.join("existing.pdf");
    let bytes = pdfcraft_engine::Session::new().create_from_text("Existing", "Synthetic existing PDF").unwrap();
    std::fs::write(&existing, bytes.as_slice()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(850.0, 620.0)).build_eframe(|_| PdfCraftApp::new());
    h.run_steps(2);
    h.state_mut().open_path(&existing.to_string_lossy());
    h.state_mut().receive_shell(ShellAction::Combine, &[source]);
    assert!(h.state().combine_draft.is_empty(), "an incoming request waits until opening finishes");
    for _ in 0..1000 {
        h.run_steps(1);
        if !h.state().combine_draft.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(h.state().session.docs().len(), 1);
    assert_eq!(h.state().combine_draft.len(), 1);
    assert!(h.state().combine_showing(), "the earlier open cannot replace the later Combine workspace");
    std::fs::remove_dir_all(directory).unwrap();
}
