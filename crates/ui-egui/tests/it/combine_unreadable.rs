//! Combine files: when several files can't be read, the one notice names all of them.

use crate::support;
use crate::support::AppHarness as _;
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

#[test]
fn every_unreadable_path_is_named() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-combine-unreadable-{}", std::process::id()));
    let paths: Vec<String> = ["first-missing.pdf", "second-missing.pdf"].iter().map(|n| dir.join(n).to_string_lossy().into_owned()).collect();
    let mut h: Harness<'static, PdfCraftApp> = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        support::install_runtime(&cc.egui_ctx);
        support::inline_app()
    });
    h.set_option("language", "en").unwrap();
    h.run_steps(2);
    let ctx = h.ctx.clone();
    h.state_mut().combine_paths(&ctx, &paths);
    // The files are read on a worker thread; the notice comes with the answer.
    assert!(support::step_until(&mut h, |h| support::shown_notice(h).is_some()), "a notice is shown");
    let notice = support::shown_notice(&h).expect("a notice is shown");
    assert!(notice.contains("first-missing.pdf"), "the first failure is named: {notice}");
    assert!(notice.contains("second-missing.pdf"), "the last failure is named: {notice}");
    assert!(h.state().combine_draft.is_empty(), "nothing readable, nothing listed");
}
