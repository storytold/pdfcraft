//! The app's rules for one optimization at a time and the exclusion with Enhance scanned file, driven as a user fires the commands.

use std::sync::Arc;

use craft::host::{ManualJobs, Picked, ScriptedFiles};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

use crate::support;
use crate::support::AppHarness as _;

/// A harness whose jobs wait until the test steps them, with a document open and the answer to Reduce's "where to save" given.
fn held() -> (Arc<ManualJobs>, Harness<'static, PdfCraftApp>) {
    let jobs = Arc::new(ManualJobs::new());
    let queue = jobs.clone();
    let out = std::env::temp_dir().join(format!("pdfcraft-optimize-held-{}.pdf", std::process::id()));
    // Every Reduce asks once where to save: each ask is answered with `out`.
    let files = Arc::new(ScriptedFiles::answering((0..4).map(|_| Picked::Paths(vec![out.clone()]))));
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = support::test_app(support::host_with(queue, files));
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "notes.txt", None, b"hello".to_vec()).unwrap();
            app
        }
    });
    h.run_steps(2);
    (jobs, h)
}

#[test]
fn a_second_run_is_refused_while_one_is_going() {
    let (jobs, mut h) = held();
    assert!(h.execute("optimize.reduce"));
    h.run_steps(2);
    assert!(h.state().optimize_run.is_some(), "the first run is going");
    assert!(h.execute("optimize.reduce"));
    h.run_steps(2);
    assert!(support::shown_notice(&h).is_some(), "the second run is refused with a notice");
    assert!(jobs.run_next(), "the first run's job is kept");
    assert!(!jobs.run_next(), "and no second job was queued");
}

#[test]
fn enhancing_scans_refuses_optimizing() {
    let (_jobs, mut h) = held();
    let ctx_h = h.ctx.clone();
    h.state_mut().start_scan_clean(&ctx_h);
    h.run_steps(2);
    assert!(h.state().scan_clean_run.is_some(), "the enhancement is going");
    assert!(h.execute("optimize.reduce"));
    h.run_steps(2);
    assert!(support::shown_notice(&h).is_some());
    assert!(h.state().optimize_run.is_none());
}

#[test]
fn optimizing_refuses_enhancing_scans() {
    let (_jobs, mut h) = held();
    assert!(h.execute("optimize.reduce"));
    h.run_steps(2);
    assert!(h.state().optimize_run.is_some(), "the run is going");
    let ctx_h = h.ctx.clone();
    h.state_mut().start_scan_clean(&ctx_h);
    h.run_steps(2);
    assert!(support::shown_notice(&h).is_some());
    assert!(h.state().scan_clean_run.is_none());
}
