//! DCTDecode data is read from its start-of-image marker (hayro-syntax patch 56, storytold/craft-libs#2711), and an
//! image that can't be decoded is reported (`RenderWarning::ImageNotDecoded`).
//!
//! The barcode in PDFBOX-5992's `broken-2.pdf` is an inline JPEG written `ID` CR LF LF: one
//! end-of-line more than the single white-space character ISO 32000-2 §8.9.7 puts after `ID`, so
//! its data began with a newline instead of the JPEG's start-of-image marker (§7.4.8) and the
//! decoder refused it. The page drew without the barcode and said nothing. PDFium, MuPDF and
//! Poppler draw it, and skip bytes before the marker in image XObjects too.
//!
//! Each page is 40 pt square with an 8 × 8 black JPEG drawn over its lower-left 20 pt.

use super::{one_page_pdf, render_first_page};
use crate::RenderWarning;

/// An 8 × 8 black greyscale baseline JPEG (contributor-original, written for this test).
const BLACK_JPEG: &[u8] = &[
    0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xff, 0xdb, 0x00, 0x43,
    0x00, 0x10, 0x0b, 0x0c, 0x0e, 0x0c, 0x0a, 0x10, 0x0e, 0x0d, 0x0e, 0x12, 0x11, 0x10, 0x13, 0x18, 0x28, 0x1a, 0x18, 0x16, 0x16, 0x18, 0x31, 0x23,
    0x25, 0x1d, 0x28, 0x3a, 0x33, 0x3d, 0x3c, 0x39, 0x33, 0x38, 0x37, 0x40, 0x48, 0x5c, 0x4e, 0x40, 0x44, 0x57, 0x45, 0x37, 0x38, 0x50, 0x6d, 0x51,
    0x57, 0x5f, 0x62, 0x67, 0x68, 0x67, 0x3e, 0x4d, 0x71, 0x79, 0x70, 0x64, 0x78, 0x5c, 0x65, 0x67, 0x63, 0xff, 0xc0, 0x00, 0x0b, 0x08, 0x00, 0x08,
    0x00, 0x08, 0x01, 0x01, 0x11, 0x00, 0xff, 0xc4, 0x00, 0x14, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x07, 0xff, 0xc4, 0x00, 0x14, 0x10, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3f, 0x00, 0x3f, 0x7f, 0xff, 0xd9,
];

/// A page drawing the JPEG as an inline image whose data follows `ID` and `after_id`.
fn inline_page(after_id: &[u8], jpeg: &[u8]) -> Vec<u8> {
    let mut content = b"q 20 0 0 20 0 0 cm BI /W 8 /H 8 /CS /G /BPC 8 /F /DCT ID".to_vec();
    content.extend_from_slice(after_id);
    content.extend_from_slice(jpeg);
    content.extend_from_slice(b"\nEI Q");
    one_page_pdf(&content, "")
}

/// A page drawing an image XObject whose DCTDecode stream data is `data`.
fn xobject_page(data: &[u8]) -> Vec<u8> {
    let content = "q 20 0 0 20 0 0 cm /Im Do Q";
    let mut pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /XObject << /Im 5 0 R >> >> >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         5 0 obj << /Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode /Length {} >> stream\n",
        content.len(),
        data.len()
    )
    .into_bytes();
    pdf.extend_from_slice(data);
    pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
    pdf
}

/// Whether the image was drawn (the middle of its square is black), and the page's warnings.
fn drawn(pdf: Vec<u8>) -> (bool, Vec<RenderWarning>) {
    let page = render_first_page(pdf);
    // The square covers x 0..20 and, from the top of the 40 px page, y 20..40.
    let at = |x: u32, y: u32| page.rgba.get(((y * page.width + x) * 4) as usize..).and_then(|p| p.get(..3)).map(|p| p.to_vec());
    (at(10, 30) == Some(vec![0, 0, 0]), page.warnings)
}

#[test]
fn an_inline_jpeg_after_one_white_space_draws_as_before() {
    for after_id in [&b" "[..], b"\n", b"\r\n"] {
        assert_eq!(drawn(inline_page(after_id, BLACK_JPEG)), (true, vec![]), "{after_id:?}");
    }
}

/// `ID` CR LF LF, as in PDFBOX-5992's `broken-2.pdf`, and other white space too many.
#[test]
fn an_inline_jpeg_after_more_white_space_than_one_draws() {
    for after_id in [&b"\r\n\n"[..], b"\n\n", b"    ", b"\n\t\x0c\x00"] {
        assert_eq!(drawn(inline_page(after_id, BLACK_JPEG)), (true, vec![]), "{after_id:?}");
    }
}

/// Other bytes before the marker are skipped as well, as the other readers skip them.
#[test]
fn bytes_before_the_start_of_image_marker_are_skipped() {
    assert_eq!(drawn(inline_page(b" xx", BLACK_JPEG)), (true, vec![]), "inline");
    for before in [&b"\n"[..], b"\r\n\n", b"xx"] {
        let mut data = before.to_vec();
        data.extend_from_slice(BLACK_JPEG);
        assert_eq!(drawn(xobject_page(&data)), (true, vec![]), "XObject after {before:?}");
    }
    assert_eq!(drawn(xobject_page(BLACK_JPEG)), (true, vec![]), "XObject as written");
}

/// Data with no start-of-image marker anywhere is not drawn, and the page says an image was
/// left out: it was dropped without a word before.
#[test]
fn an_image_that_does_not_decode_is_reported() {
    let no_marker = &BLACK_JPEG[2..];
    assert_eq!(drawn(inline_page(b"\n", no_marker)), (false, vec![RenderWarning::ImageNotDecoded]), "inline");
    assert_eq!(drawn(xobject_page(no_marker)), (false, vec![RenderWarning::ImageNotDecoded]), "XObject");
    // A marker cut off at the end of the data is no marker.
    assert_eq!(drawn(xobject_page(b"\n\xff\xd8")), (false, vec![RenderWarning::ImageNotDecoded]), "a cut marker");
}
