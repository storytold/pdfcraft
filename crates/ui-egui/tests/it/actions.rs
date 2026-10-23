//! Action Wizard in the real shell (egui_kittest): run a built-in action on files, create an
//! action, and remember it.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Session;
use pdfcraft_engine::actions::Step;
use pdfcraft_ui_egui::PdfCraftApp;

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
