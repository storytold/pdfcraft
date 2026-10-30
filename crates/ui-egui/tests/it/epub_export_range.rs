//! The EPUB export is the whole document: a leftover page range from another export must not
//! disable the Export button, since the range is not shown for a book.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Session;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::marks::{PageRange, Subset};

#[test]
fn an_empty_page_range_does_not_stop_an_epub_export() {
    let doc = Session::new().create_from_text("t", "Words in a book").unwrap().to_vec();
    let dir = std::env::temp_dir().join(format!("pdfcraft-epub-range-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.export_dir_override = Some(out.clone());
        app.open_bytes("book.pdf", None, doc.clone()).unwrap();
        // Pages 50 to 60 of a one-page document select nothing.
        app.export_draft.range = PageRange { all: false, from: 50, to: 60, subset: Subset::All };
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("export.epub"));
    h.run_steps(2);
    h.get_by_label("Export to EPUB");
    h.get_by_label("Export").click();
    h.run_steps(4);
    assert!(dir.join("book.epub").exists(), "the book was written");
    let _ = std::fs::remove_dir_all(dir);
}
