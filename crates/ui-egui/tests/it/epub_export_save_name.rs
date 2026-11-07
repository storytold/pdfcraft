//! The name typed in the EPUB Save dialog keeps its dots: `report.v2` is saved as
//! `report.v2.epub`, not `report.epub` (which could overwrite another book).

use std::path::Path;

use pdfcraft_ui_egui::epub_name::epub_stem;

#[test]
fn only_a_trailing_epub_extension_is_removed() {
    assert_eq!(epub_stem(Path::new("/tmp/report.v2"), "x"), "report.v2");
    assert_eq!(epub_stem(Path::new("/tmp/report.v2.epub"), "x"), "report.v2");
    assert_eq!(epub_stem(Path::new("/tmp/report.EPUB"), "x"), "report");
    assert_eq!(epub_stem(Path::new("/tmp/report.ePub"), "x"), "report");
    assert_eq!(epub_stem(Path::new("/tmp/report"), "x"), "report");
    assert_eq!(epub_stem(Path::new("/tmp/Café.épub"), "x"), "Café.épub");
}

#[test]
fn odd_names_do_not_panic() {
    assert_eq!(epub_stem(Path::new("/tmp/.epub"), "x"), ".epub");
    assert_eq!(epub_stem(Path::new("/"), "fallback"), "fallback");
    assert_eq!(epub_stem(Path::new(""), "fallback"), "fallback");
    assert_eq!(epub_stem(Path::new("/tmp/é.epub"), "x"), "é");
}
