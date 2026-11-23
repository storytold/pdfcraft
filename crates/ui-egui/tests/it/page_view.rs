//! The app window at desktop size shows a document's pages: a debug-build assert on the first
//! frame is invisible to a component trial, so the whole app runs here.

use egui_kittest::Harness;

#[test]
fn a_document_tab_shows_its_pages_for_a_few_frames() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "pages.pdf", None, crate::support::fixtures::pdf_pages(2)).unwrap();
            app
        }
    });
    crate::support::settle(&mut h);
    h.run_steps(5);
    assert!(!h.state().views[0].visible_page_rects().is_empty(), "the pages are placed on screen");
}
