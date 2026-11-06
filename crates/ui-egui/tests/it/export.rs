//! Export a PDF ▸ Image and Text from the real shell (egui_kittest).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::export::ImageFormat;
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

#[test]
fn export_dialog_writes_svg_pages() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-export-ui-svg-{}", std::process::id()));
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
    h.get_by_label("Resolution");
    // SVG is vector: the dialog stops asking for a resolution.
    h.state_mut().export_draft.format = ImageFormat::Svg;
    h.run_steps(2);
    assert!(h.query_by_label("Resolution").is_none());
    h.get_by_label("Export").click();
    h.run_steps(4);
    for n in [1, 2] {
        let svg = std::fs::read_to_string(dir.join(format!("doc_page_{n}.svg"))).unwrap();
        assert!(svg.starts_with("<svg viewBox=\"0 0 200 100\""), "page {n}: {svg}");
    }
    assert!(!dir.join("doc_page_1.png").exists());
    h.get_by_label_contains("Exported 2 images");
    let _ = std::fs::remove_dir_all(&dir);
}
