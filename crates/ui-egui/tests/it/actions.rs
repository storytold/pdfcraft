//! Action Wizard in the real shell (egui_kittest): run a built-in action on files, create an
//! action, and remember it.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::Session;
use pdfcraft_engine::actions::Step;
use pdfcraft_ui_egui::PdfCraftApp;

/// The Action Wizard's name, description and step-argument boxes are named by their labels.
#[test]
fn action_editor_inputs_have_accessible_names() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(2);
    assert!(h.state_mut().execute("actions.wizard"));
    h.run_steps(2);
    h.get_by_label("New Action…").click();
    h.run_steps(2);
    h.get_by_role_and_label(Role::TextInput, "Action name:");
    h.get_by_role_and_label(Role::TextInput, "Description:");
}

#[test]
fn run_and_create_actions() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-actions-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("out")).unwrap();
    let src = Session::new().create_from_text("t", "Draft report").unwrap();
    std::fs::write(dir.join("r.pdf"), &*src).unwrap();
    let d = dir.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.run_inline = true;
        app.export_dir_override = Some(d.join("out").to_string_lossy().into_owned());
        app.action_files_override = Some(vec![d.join("r.pdf").to_string_lossy().into_owned()]);
        app
    });
    h.run_steps(2);
    assert!(h.state_mut().execute("actions.distribution"));
    h.run_steps(2);
    h.get_by_label("Action Wizard");
    h.get_by_label("1. Remove hidden information");
    h.get_by_label("Start").click();
    h.run_steps(3);
    assert_eq!(h.state().toast.clone().unwrap().0, "Prepare for Distribution: 1 file done");
    assert!(std::fs::read(dir.join("out/r.pdf")).unwrap().starts_with(b"%PDF"));

    // A new action of our own.
    assert!(h.state_mut().execute("actions.wizard"));
    h.run_steps(2);
    h.get_by_label("New Action…").click();
    h.run_steps(2);
    {
        let (_, a) = h.state_mut().wizard.editing.as_mut().unwrap();
        a.name = "Stamp drafts".into();
        a.steps.push(Step::AddWatermark("DRAFT".into()));
    }
    h.run_steps(3);
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert_eq!(h.state().custom_actions.len(), 1);
    h.get_by_label("1. Add watermark: DRAFT");
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.set_option("language", "en").unwrap();
    fresh.restore(&saved);
    assert_eq!(fresh.custom_actions, h.state().custom_actions);
    h.get_by_label("Start").click();
    h.run_steps(3);
    assert_eq!(h.state().toast.clone().unwrap().0, "Stamp drafts: 1 file done");
}

#[test]
fn desktop_actions_preserve_duplicate_names_and_results_from_earlier_runs() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-actions-collisions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("first")).unwrap();
    std::fs::create_dir_all(dir.join("second")).unwrap();
    std::fs::create_dir_all(dir.join("out")).unwrap();
    let three_pages = Session::new().create_blank(200.0, 300.0, 3).unwrap();
    let one_page = Session::new().create_blank(200.0, 300.0, 1).unwrap();
    std::fs::write(dir.join("first/report.PDF"), &*three_pages).unwrap();
    std::fs::write(dir.join("second/report.PDF"), &*one_page).unwrap();
    // A previous output must survive too, not just the first file in this batch.
    std::fs::write(dir.join("out/report.PDF"), &*one_page).unwrap();
    let d = dir.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.run_inline = true;
        app.export_dir_override = Some(d.join("out").to_string_lossy().into_owned());
        app.action_files_override =
            Some(vec![d.join("first/report.PDF").to_string_lossy().into_owned(), d.join("second/report.PDF").to_string_lossy().into_owned()]);
        app
    });
    h.run_steps(2);
    for (first, second) in [(2, 3), (4, 5)] {
        // Start closes the wizard: reopen it just as a user does for the next run.
        h.state_mut().execute("actions.distribution");
        h.run_steps(2);
        h.get_by_label("Start").click();
        h.run_steps(3);
        assert_eq!(h.state().toast.clone().unwrap().0, "Prepare for Distribution: 2 files done");
        for (number, pages) in [(first, 3), (second, 1)] {
            let path = dir.join(format!("out/report ({number}).PDF"));
            let mut reopened = Session::new();
            let id = reopened.open(path.to_string_lossy().into_owned(), None, std::sync::Arc::new(std::fs::read(&path).unwrap()), None).unwrap();
            assert_eq!(reopened.get(id).unwrap().info.pages.len(), pages);
        }
    }
    assert_eq!(std::fs::read_dir(dir.join("out")).unwrap().count(), 5);
    assert_eq!(std::fs::read(dir.join("out/report.PDF")).unwrap(), *one_page);
    assert_eq!(std::fs::read(dir.join("first/report.PDF")).unwrap(), *three_pages);
    assert_eq!(std::fs::read(dir.join("second/report.PDF")).unwrap(), *one_page);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_action_wizard_exports_multiple_pdfs_to_word() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    let src = Session::new().create_from_text("Draft", "Batch Word export").unwrap();
    for name in ["first.pdf", "second.PDF"] {
        std::fs::write(dir.path().join(name), &*src).unwrap();
    }
    let d = dir.path().to_path_buf();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.run_inline = true;
        app.export_dir_override = Some(d.join("out").to_string_lossy().into_owned());
        app.action_files_override = Some(["first.pdf", "second.PDF"].iter().map(|name| d.join(name).to_string_lossy().into_owned()).collect());
        app
    });
    h.run_steps(3);
    assert!(h.state_mut().execute("actions.wizard"));
    h.run_steps(3);
    h.get_by_label("New Action…").click();
    h.run_steps(3);
    h.state_mut().wizard.editing.as_mut().unwrap().1.name = "Word copies".into();
    h.state_mut().wizard.add = Step::all().iter().position(|s| s.id() == "export_word").expect("Word export must be offered in the step picker");
    h.run_steps(3);
    h.get_by_label("Add Step").click();
    h.run_steps(3);
    h.state_mut().wizard.editing.as_mut().unwrap().1.steps.push(Step::SetTitle("Too late".into()));
    h.run_steps(3);
    h.get_by_label("Export to Word must be the last step.");
    assert!(h.get_by_label("Save").accesskit_node().is_disabled());
    h.state_mut().wizard.editing.as_mut().unwrap().1.steps.pop();
    h.run_steps(3);
    assert!(!h.get_by_label("Save").accesskit_node().is_disabled());
    h.get_by_label("Save").click();
    h.run_steps(3);
    h.get_by_label("1. Export to Word");
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.set_option("language", "en").unwrap();
    fresh.restore(&saved);
    assert_eq!(fresh.custom_actions, h.state().custom_actions);
    h.get_by_label("Start").click();
    h.run_steps(3);
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some("Word copies: 2 files done"));
    for name in ["first.docx", "second.docx"] {
        assert!(std::fs::read(dir.path().join("out").join(name)).unwrap().starts_with(b"PK\x03\x04"));
    }
    assert_eq!(std::fs::read_dir(dir.path().join("out")).unwrap().count(), 2);
}
