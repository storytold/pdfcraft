//! Combine files: when several files can't be read, the one notice names all of them.

use pdfcraft_ui_egui::PdfCraftApp;

#[test]
fn every_unreadable_path_is_named() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-combine-unreadable-{}", std::process::id()));
    let paths: Vec<String> = ["first-missing.pdf", "second-missing.pdf"].iter().map(|n| dir.join(n).to_string_lossy().into_owned()).collect();
    let mut app = PdfCraftApp::new();
    app.combine_paths(&paths);
    let (notice, _) = app.toast.clone().expect("a notice is shown");
    assert!(notice.contains("first-missing.pdf"), "the first failure is named: {notice}");
    assert!(notice.contains("second-missing.pdf"), "the last failure is named: {notice}");
}
