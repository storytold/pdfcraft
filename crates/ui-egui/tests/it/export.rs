//! Export a PDF ▸ Image and Text from the real shell (egui_kittest).

use std::path::Path;
use std::sync::Arc;

use craft::host::{AppInfo, Host, ManualJobs, Picked, ScriptedFiles};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use crate::support::AppHarness as _;

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 100] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn export_dialogs_write_images_and_text() {
    let files = Arc::new(ScriptedFiles::answering((0..3).map(|_| Picked::Paths(vec!["/out".into()]))));
    let jobs = Arc::new(ManualJobs::new());
    let host = Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" }).files(files.clone()).jobs(jobs.clone()).build();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        // PLAN: assumed `PdfCraftApp::with_host(host: Host) -> PdfCraftApp` (L-71)
        let mut app = PdfCraftApp::with_host(host);
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.execute("export.image"));
    h.run_steps(2);
    h.get_by_label("Export to Image");
    h.get_by_label("Export").click();
    h.run_steps(4);
    jobs.run_all();
    h.run_steps(4);
    let first = files.file(Path::new("/out/doc_page_1.png")).expect("page 1 written");
    let second = files.file(Path::new("/out/doc_page_2.png")).expect("page 2 written");
    assert_ne!(first, second, "one file per page");
    h.get_by_label_contains("Exported 2 images");
    assert!(h.execute("export.text"));
    h.run_steps(2);
    h.get_by_label("Export").click();
    h.run_steps(4);
    jobs.run_all();
    h.run_steps(4);
    assert!(files.file(Path::new("/out/doc.txt")).is_some());
    // Export all images: this document has none, and says so.
    assert!(h.execute("export.all_images"));
    h.run_steps(2);
    h.get_by_label("Export All Images");
    h.get_by_label("Export").click();
    h.run_steps(4);
    jobs.run_all();
    h.run_steps(4);
    h.get_by_label_contains("Exported 0 images");
}
