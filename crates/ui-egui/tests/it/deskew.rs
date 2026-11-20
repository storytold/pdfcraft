//! Scan & OCR ▸ Straighten page / Straighten all pages in the real shell: the commands are wired,
//! a page with nothing to measure is left alone, and the batch opens its report.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "100").unwrap();
        app
    });
    h.run_steps(6);
    h
}

#[test]
fn the_deskew_commands_are_wired_and_leave_a_blank_page_alone() {
    let mut h = harness();
    // Both commands are handled by the shell, not silently dropped.
    assert!(h.state_mut().execute("ocr.deskew"), "Straighten page is not wired");
    assert!(h.state_mut().execute("ocr.deskew_all"), "Straighten all pages is not wired");
    h.run_steps(2);
    // Two blank pages have nothing to measure: no edit, and the batch still reports.
    let s = h.state();
    let id = s.views[0].id;
    assert_eq!(s.session.get(id).unwrap().can_undo(), None);
    assert!(s.deskew_report.is_some(), "the batch opens its report");
}
