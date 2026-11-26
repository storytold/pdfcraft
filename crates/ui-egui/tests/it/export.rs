//! Export a PDF ▸ Image and Text from the real shell (egui_kittest).

use std::path::Path;
use std::sync::Arc;

use crate::support::AppHarness as _;
use craft::host::{ManualJobs, Picked, ScriptedFiles};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 100] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn export_dialogs_write_images_and_text() {
    let files = Arc::new(ScriptedFiles::answering((0..3).map(|_| Picked::Paths(vec!["/out".into()]))));
    let jobs = Arc::new(ManualJobs::new());
    let host = crate::support::host_with(jobs.clone(), files.clone());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::support::test_app(host);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, FIXTURE.to_vec()).unwrap();
            app
        }
    });
    h.run_steps(4);
    assert!(h.execute("export.image"));
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Export to Image");
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
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Export All Images");
    h.get_by_label("Export").click();
    h.run_steps(4);
    jobs.run_all();
    h.run_steps(4);
    h.get_by_label_contains("Exported 0 images");
}
