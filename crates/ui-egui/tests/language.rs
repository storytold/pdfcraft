//! Tests find widgets by their English labels, so they must not depend on the language of the
//! machine they run on.

use pdfcraft_ui_egui::PdfCraftApp;

#[test]
fn the_test_app_speaks_english_whatever_the_locale() {
    let app = PdfCraftApp::new_for_test();
    assert_eq!(app.language, "en");
}

#[test]
fn tests_build_their_app_with_new_for_test() {
    // `PdfCraftApp::new()` follows the system language; a test using it fails on a machine set
    // to another language (run `LANG=de_DE.UTF-8 LC_ALL=de_DE.UTF-8 cargo test -p pdfcraft-ui-egui`).
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "rs") || path.file_name().is_some_and(|n| n == "language.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        if source.contains(concat!("PdfCraftApp::", "new()")) {
            offenders.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    assert!(offenders.is_empty(), "use PdfCraftApp::new_for_test() in: {offenders:?}");
}
