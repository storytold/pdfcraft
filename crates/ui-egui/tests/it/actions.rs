//! Action Wizard in the real shell (egui_kittest): run a built-in action on files, create an
//! action, and remember it.

use std::path::Path;
use std::sync::Arc;

use craft::host::{ManualJobs, Picked, ScriptedFiles};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Session;
use pdfcraft_ui_egui::PdfCraftApp;

use crate::support;
use crate::support::AppHarness as _;

/// An app over the scripted file service and manual jobs.
fn harness(files: &Arc<ScriptedFiles>, jobs: &Arc<ManualJobs>) -> Harness<'static, PdfCraftApp> {
    let host = support::host_with(jobs.clone(), files.clone());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = support::test_app(host);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app
        }
    });
    h.run_steps(2);
    h
}

/// Runs the queued job and lets the frame take its result.
fn finish(h: &mut Harness<'static, PdfCraftApp>, jobs: &ManualJobs) {
    h.run_steps(3);
    jobs.run_all();
    h.run_steps(3);
}

#[test]
fn run_and_create_actions() {
    let src = Session::new().create_from_text("t", "Draft report").unwrap();
    let picks = || [Picked::Paths(vec!["/in/r.pdf".into()]), Picked::Paths(vec!["/out".into()])];
    let files = Arc::new(ScriptedFiles::answering(picks().into_iter().chain(picks())).with_file("/in/r.pdf", src.to_vec()));
    let jobs = Arc::new(ManualJobs::new());
    let mut h = harness(&files, &jobs);
    assert!(h.execute("actions.distribution"));
    h.run_steps(2);
    h.get_by_role_and_label(Role::Dialog, "Action Wizard");
    // The chosen action's description lists its numbered steps, one per line.
    h.get_by_label_contains("1. Remove hidden information");
    h.get_by_label("Start").click();
    finish(&mut h, &jobs);
    assert_eq!(support::shown_notice(&h).as_deref(), Some("Prepare for Distribution: 1 file done"));
    assert!(files.file(Path::new("/out/r.pdf")).unwrap().starts_with(b"%PDF"));

    // A new action of our own, made with the controls.
    assert!(h.execute("actions.wizard"));
    h.run_steps(2);
    let name = h.get_by_role_and_label(Role::TextInput, "Action name");
    name.focus();
    name.type_text("Stamp drafts");
    h.run_steps(2);
    h.get_by_label("Create").click();
    h.run_steps(3);
    h.get_by_label("Step to add").click();
    h.run_steps(2);
    h.get_by_label("Add watermark").click();
    h.run_steps(2);
    h.get_by_label("Add Step").click();
    h.run_steps(2);
    let arg = h.get_by_role_and_label(Role::TextInput, "1. Add watermark");
    arg.focus();
    arg.type_text("DRAFT");
    h.run_steps(2);
    // Exiting overlays remain in the accessibility tree while hidden during their fade.
    h.get_by(|node| node.role() == Role::Button && !node.is_hidden() && node.label().as_deref() == Some("Save")).click();
    h.run_steps(3);
    assert_eq!(h.state().custom_actions.len(), 1);
    h.get_by_label("1. Add watermark: DRAFT");
    let saved = h.state().settings_snapshot();
    let mut fresh = crate::control::new_app();
    fresh.set_option(&egui::Context::default(), "language", "en").unwrap();
    fresh.apply_settings(&saved);
    assert_eq!(fresh.custom_actions, h.state().custom_actions);
    h.get_by_label("Start").click();
    finish(&mut h, &jobs);
    assert_eq!(support::shown_notice(&h).as_deref(), Some("Stamp drafts: 1 file done"));
}

#[test]
fn desktop_actions_preserve_duplicate_names_and_results_from_earlier_runs() {
    let three_pages = Session::new().create_blank(200.0, 300.0, 3).unwrap();
    let one_page = Session::new().create_blank(200.0, 300.0, 1).unwrap();
    let picks = || [Picked::Paths(vec!["/first/report.PDF".into(), "/second/report.PDF".into()]), Picked::Paths(vec!["/out".into()])];
    let files = Arc::new(
        ScriptedFiles::answering(picks().into_iter().chain(picks()))
            .with_file("/first/report.PDF", three_pages.to_vec())
            .with_file("/second/report.PDF", one_page.to_vec())
            // A previous output must survive too, not just the first file in this batch.
            .with_file("/out/report.PDF", one_page.to_vec()),
    );
    let jobs = Arc::new(ManualJobs::new());
    let mut h = harness(&files, &jobs);
    for (first, second) in [(2, 3), (4, 5)] {
        // Start closes the wizard: reopen it just as a user does for the next run.
        h.execute("actions.distribution");
        h.run_steps(2);
        h.get_by_label("Start").click();
        finish(&mut h, &jobs);
        assert_eq!(support::shown_notice(&h).as_deref(), Some("Prepare for Distribution: 2 files done"));
        for (number, pages) in [(first, 3), (second, 1)] {
            let bytes = files.file(Path::new(&format!("/out/report ({number}).PDF"))).unwrap();
            let mut reopened = Session::new();
            let id = reopened.open(format!("report ({number}).PDF"), None, Arc::new(bytes), None).unwrap();
            assert_eq!(reopened.get(id).unwrap().info.pages.len(), pages);
        }
    }
    assert!(files.file(Path::new("/out/report (6).PDF")).is_none());
    assert_eq!(files.file(Path::new("/out/report.PDF")).unwrap(), one_page.to_vec());
    assert_eq!(files.file(Path::new("/first/report.PDF")).unwrap(), three_pages.to_vec());
    assert_eq!(files.file(Path::new("/second/report.PDF")).unwrap(), one_page.to_vec());
}
