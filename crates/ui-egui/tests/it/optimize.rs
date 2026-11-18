//! The app's rules for one optimization at a time and the exclusion with Enhance scanned file, driven as a user fires the commands.
//!
//! PLAN: `mod optimize;` belongs in `tests/it/main.rs` (pdf-shell-w9's hunk). The held host is assumed
//! `support::held_app(jobs: Arc<ManualJobs>) -> PdfCraftApp` (a `PdfCraftApp::with_host` over a host with `ManualJobs`, pdf-hub-w1 L-71) and
//! `support::shown_notice(&Harness) -> Option<String>` (pdf-chrome-w9).

use std::sync::Arc;

use craft::host::ManualJobs;
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

use crate::support;
use crate::support::AppHarness as _;

/// A harness whose jobs wait until the test steps them, with a document open and the answer to Reduce's "where to save" given.
fn held() -> (Arc<ManualJobs>, Harness<'static, PdfCraftApp>) {
    let jobs = Arc::new(ManualJobs::new());
    let queue = jobs.clone();
    let out = std::env::temp_dir().join(format!("pdfcraft-optimize-held-{}.pdf", std::process::id()));
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = support::held_app(queue);
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.open_bytes("notes.txt", None, b"hello".to_vec()).unwrap();
        app.save_override = Some(out.to_string_lossy().into_owned());
        app
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
