//! The type a web download is offered as follows the file's extension (it was `application/pdf`
//! for every file).

use pdfcraft_ui_egui::download_mime_type;

#[test]
fn png_downloads_as_image_png() {
    assert_eq!(download_mime_type("area.png"), "image/png");
    assert_eq!(download_mime_type("Area.PNG"), "image/png");
}

#[test]
fn other_files_get_their_own_types() {
    assert_eq!(download_mime_type("report.pdf"), "application/pdf");
    assert_eq!(download_mime_type("measurements.csv"), "text/csv");
    assert_eq!(download_mime_type("fields.xfdf"), "application/vnd.adobe.xfdf");
    assert_eq!(download_mime_type("photo.jpeg"), "image/jpeg");
    assert_eq!(download_mime_type("page.tif"), "image/tiff");
}

#[test]
fn unknown_extensions_are_plain_bytes() {
    assert_eq!(download_mime_type("data.bin"), "application/octet-stream");
    assert_eq!(download_mime_type("no extension"), "application/octet-stream");
}
