//! Export a PDF ▸ Image, Text and Excel from the real shell (egui_kittest).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::compare::XlsxSheets;
use pdfcraft_ui_egui::PdfCraftApp;

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 100] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn export_dialogs_write_images_and_text() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-export-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app.export_dir_override = Some(d.to_string_lossy().into_owned());
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("export.image"));
    h.run_steps(2);
    h.get_by_label("Export to Image");
    h.get_by_label("Export").click();
    h.run_steps(4);
    assert!(dir.join("doc_page_1.png").exists() && dir.join("doc_page_2.png").exists());
    h.get_by_label_contains("Exported 2 images");
    assert!(h.state_mut().execute("export.text"));
    h.run_steps(2);
    h.get_by_label("Export").click();
    h.run_steps(4);
    assert!(dir.join("doc.txt").exists());
    // Export all images: this document has none, and says so.
    assert!(h.state_mut().execute("export.all_images"));
    h.run_steps(2);
    h.get_by_label("Export All Images");
    h.get_by_label("Export").click();
    h.run_steps(4);
    h.get_by_label_contains("Exported 0 images");
}

/// Export to Excel asks how the sheets go: one sheet for the whole document by default, or one
/// per page; the pages chosen go into one workbook through the save dialog.
#[test]
fn export_to_excel_offers_the_sheets() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-export-excel-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let bytes = app.session.create_from_text("Readings", "Meter 37414.00 kWh on 2026-10-14").unwrap();
        app.open_bytes("readings.pdf", None, bytes.as_ref().clone()).unwrap();
        app.save_override = Some(d.join("whole.xlsx").to_string_lossy().into_owned());
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("export.xlsx"));
    h.run_steps(6);
    h.get_by_label("Export to Excel");
    h.get_by_label("One sheet per page");
    assert_eq!(h.state().export_draft.sheets, XlsxSheets::WholeDocument, "one sheet for the whole document by default");
    h.get_by_label("One sheet for the whole document");
    h.get_by_label("Export").click();
    h.run_steps(4);
    let whole = std::fs::read(dir.join("whole.xlsx")).expect("the workbook is written");
    assert!(whole.starts_with(b"PK"));
    h.get_by_label_contains("Exported to");
    h.state_mut().save_override = Some(dir.join("pages.xlsx").to_string_lossy().into_owned());
    assert!(h.state_mut().execute("export.xlsx"));
    h.run_steps(6);
    h.get_by_label("One sheet per page").click();
    h.run_steps(3);
    h.get_by_label("Export").click();
    h.run_steps(4);
    let pages = std::fs::read(dir.join("pages.xlsx")).expect("the per-page workbook is written");
    assert_ne!(whole, pages, "the sheet is named after the document in one, Page 1 in the other");
    assert_eq!(h.state().export_draft.sheets, XlsxSheets::PerPage, "kept for next time");
    let _ = std::fs::remove_dir_all(&dir);
}
