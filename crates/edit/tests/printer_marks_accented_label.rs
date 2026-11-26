//! The page information line keeps accented letters: WinAnsi bytes from 0x80 up must reach the
//! content stream as escapes, not be replaced with U+FFFD.

use pdfcraft_edit::PrinterMarks;

#[test]
fn accented_label_is_written_as_winansi_escapes() {
    let marks = PrinterMarks { label: "Café.pdf".into(), ..PrinterMarks::default() };
    let content = marks.content([0.0, 0.0, 612.0, 792.0], [0.0, 0.0, 612.0, 792.0], 1, 1);
    assert!(!content.contains('\u{FFFD}'), "no replacement character in the stream: {content:?}");
    assert!(content.contains("(Caf\\351.pdf  1 of 1)"), "é is the WinAnsi byte 0xE9 written as \\351: {content:?}");
}
