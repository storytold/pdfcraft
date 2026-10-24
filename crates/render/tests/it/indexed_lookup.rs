//! An `/Indexed` image whose colour space ISO 32000-2 §8.6.6.3 rules out (a lookup table
//! shorter than m × (hival + 1) bytes, or a `hival` that is not an integer in 0..=255) is not
//! drawn, and the render reports `RenderWarning::MalformedIndexedImage`. It used to fall back to
//! DeviceGray and paint its indices as near-black, even indices whose entries were present.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderWarning, RenderedPage, RequestKind};

/// A 200 × 200 page whose `/Im` image is two samples wide (indices 0 and 1) and covers the page,
/// with `/P` naming `colour_space` as a resource. `extra` objects are numbered from 6.
fn pdf(colour_space: &str, content: &str, extra: &[&[u8]]) -> Vec<u8> {
    pdf_with_resource("P", colour_space, content, extra)
}

/// As `pdf`, with the colour space resource named `resource` rather than `/P`.
fn pdf_with_resource(resource: &str, colour_space: &str, content: &str, extra: &[&[u8]]) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
             /Resources << /XObject << /Im 5 0 R >> /ColorSpace << /{resource} {colour_space} >> >> >>"
        )
        .into_bytes(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
        format!(
            "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /BitsPerComponent 8 \
             /ColorSpace {colour_space} /Filter /ASCIIHexDecode /Length 5 >>\nstream\n0001>\nendstream"
        )
        .into_bytes(),
    ];
    objs.extend(extra.iter().map(|o| o.to_vec()));
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let size = objs.len() + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

fn render(bytes: Vec<u8>) -> RenderedPage {
    let page = PageRenderer::new(Arc::new(bytes), RenderConfig::default()).render(RenderRequest {
        page: 0,
        kind: RequestKind::Pixels,
        tile: None,
        scale: 1.0,
        tag: 0,
        preview: false,
    });
    assert!(page.error.is_none(), "{:?}", page.error);
    assert_eq!((page.width, page.height), (200, 200));
    page
}

/// The left (index 0) and right (index 1) halves' centre pixels, RGB.
fn halves(page: &RenderedPage) -> [[u8; 3]; 2] {
    [50usize, 150].map(|x| {
        let at = (100 * page.width as usize + x) * 4;
        let px = page.rgba.get(at..at + 3).expect("pixel inside the page");
        [px[0], px[1], px[2]]
    })
}

const WHITE: [[u8; 3]; 2] = [[255, 255, 255], [255, 255, 255]];
const RED_GREEN: [[u8; 3]; 2] = [[255, 0, 0], [0, 255, 0]];
const DRAW_IMAGE: &str = "q 200 0 0 200 0 0 cm /Im Do Q";

fn assert_skipped(colour_space: &str, extra: &[&[u8]]) {
    let page = render(pdf(colour_space, DRAW_IMAGE, extra));
    assert_eq!(halves(&page), WHITE, "{colour_space}: the image must not be drawn");
    assert_eq!(page.warnings, vec![RenderWarning::MalformedIndexedImage], "{colour_space}: the skip is reported");
}

fn assert_drawn(colour_space: &str, extra: &[&[u8]]) {
    let page = render(pdf(colour_space, DRAW_IMAGE, extra));
    assert_eq!(halves(&page), RED_GREEN, "{colour_space}: palette entries 0 and 1");
    assert!(page.warnings.is_empty(), "{colour_space}: {:?}", page.warnings);
}

#[test]
fn a_lookup_shorter_than_hival_needs_skips_the_image_with_a_warning() {
    // hival 7 over DeviceRGB needs 24 bytes; 6 are given. Indices 0 and 1 used to draw black.
    assert_skipped("[/Indexed /DeviceRGB 7 <FF0000 00FF00>]", &[]);
    // One byte short of two entries.
    assert_skipped("[/Indexed /DeviceRGB 1 <FF0000 00FF>]", &[]);
}

#[test]
fn a_complete_lookup_draws_its_colours() {
    assert_drawn("[/Indexed /DeviceRGB 1 <FF0000 00FF00>]", &[]);
    assert_drawn("[/Indexed /DeviceRGB 7 <FF0000 00FF00 000000 000000 000000 000000 000000 000000>]", &[]);
}

#[test]
fn a_lookup_longer_than_needed_uses_its_first_entries() {
    assert_drawn("[/Indexed /DeviceRGB 1 <FF0000 00FF00 0000FF FFFF00>]", &[]);
}

#[test]
fn a_hival_outside_0_to_255_or_not_an_integer_skips_the_image() {
    let long = format!("<FF0000 00FF00{}>", "00".repeat(3 * 300));
    assert_skipped(&format!("[/Indexed /DeviceRGB 300 {long}]"), &[]);
    assert_skipped("[/Indexed /DeviceRGB -1 <FF0000 00FF00>]", &[]);
    assert_skipped("[/Indexed /DeviceRGB 1.5 <FF0000 00FF00 0000FF>]", &[]);
    assert_skipped("[/Indexed /DeviceRGB /One <FF0000 00FF00>]", &[]);
    // 255, the largest hival, with a full table still draws.
    assert_drawn(&format!("[/Indexed /DeviceRGB 255 <FF0000 00FF00{}>]", "00".repeat(3 * 254)), &[]);
}

#[test]
fn an_empty_or_missing_lookup_skips_the_image_without_panicking() {
    assert_skipped("[/Indexed /DeviceRGB 1 <>]", &[]);
    assert_skipped("[/Indexed /DeviceRGB 1 ()]", &[]);
    assert_skipped("[/Indexed /DeviceRGB 1]", &[]);
    assert_skipped("[/Indexed /DeviceRGB 0 null]", &[]);
}

#[test]
fn a_lookup_stream_is_measured_after_decoding() {
    let short: &[u8] = b"<< /Length 14 /Filter /ASCIIHexDecode >>\nstream\nFF0000 00FF00>\nendstream";
    assert_skipped("[/Indexed /DeviceRGB 7 6 0 R]", &[short]);
    assert_drawn("[/Indexed /DeviceRGB 1 6 0 R]", &[short]);
    let empty: &[u8] = b"<< /Length 0 >>\nstream\n\nendstream";
    assert_skipped("[/Indexed /DeviceRGB 1 6 0 R]", &[empty]);
}

#[test]
fn inline_images_with_a_short_lookup_are_skipped_too() {
    let inline = |cs: &str| format!("q 200 0 0 200 0 0 cm BI /W 2 /H 1 /BPC 8 /CS {cs} /F /AHx ID 0001> EI Q");
    // Named through the page's resources.
    let page = render(pdf("[/Indexed /DeviceRGB 7 <FF0000 00FF00>]", &inline("/P"), &[]));
    assert_eq!(halves(&page), WHITE);
    assert_eq!(page.warnings, vec![RenderWarning::MalformedIndexedImage]);
    // Given in place, with the inline abbreviations.
    let page = render(pdf("/DeviceRGB", &inline("[/I /RGB 7 <FF0000 00FF00>]"), &[]));
    assert_eq!(halves(&page), WHITE);
    assert_eq!(page.warnings, vec![RenderWarning::MalformedIndexedImage]);
    // And a well-formed one still draws.
    let page = render(pdf("[/Indexed /DeviceRGB 1 <FF0000 00FF00>]", &inline("/P"), &[]));
    assert_eq!(halves(&page), RED_GREEN);
    assert!(page.warnings.is_empty(), "{:?}", page.warnings);
}

#[test]
fn an_inline_device_space_name_ignores_a_resource_of_the_same_name() {
    // ISO 32000-2 §8.6.8 (the CS operator): the device colour space names never refer to resources in the
    // ColorSpace subdictionary, so a malformed Indexed resource that shares the name (here the
    // inline abbreviation `/RGB`, and `/DeviceRGB`) does not stop the image from drawing.
    for (resource, cs) in [("RGB", "/RGB"), ("DeviceRGB", "/DeviceRGB")] {
        let content = format!("q 200 0 0 200 0 0 cm BI /W 2 /H 1 /BPC 8 /CS {cs} /F /AHx ID FF000000FF00> EI Q");
        let page = render(pdf_with_resource(resource, "[/Indexed /DeviceRGB 7 <FF0000 00FF00>]", &content, &[]));
        assert_eq!(halves(&page), RED_GREEN, "{cs}: the device space draws");
        assert!(page.warnings.is_empty(), "{cs}: {:?}", page.warnings);
    }
}

#[test]
fn a_page_with_several_malformed_images_reports_the_warning_once_and_draws_the_rest() {
    let content = format!("{DRAW_IMAGE} {DRAW_IMAGE} 0 0 1 rg 0 0 10 10 re f");
    let page = render(pdf("[/Indexed /DeviceRGB 7 <FF0000 00FF00>]", &content, &[]));
    assert_eq!(page.warnings, vec![RenderWarning::MalformedIndexedImage]);
    let at = (195 * page.width as usize + 5) * 4;
    assert_eq!(page.rgba.get(at..at + 3), Some(&[0u8, 0, 255][..]), "content after the skipped images still draws");
}
