//! An `/Indexed` image whose colour space breaks ISO 32000-2 §8.6.6.3 (a lookup table shorter
//! than m × (hival + 1) bytes, or a `hival` past 255 or with a fraction) is drawn as other
//! readers draw it, and the render reports `RenderWarning::MalformedIndexedImage`:
//! - a short table is padded with zero bytes, so the entries it has keep their colours;
//! - a `hival` past 255 is taken as 255;
//! - a real `hival` is truncated toward zero.
//!
//! A `hival` that is negative or not a number keeps the upstream handling (the image falls back
//! to DeviceGray), and is reported too. Upstream fell back to DeviceGray for every one of these,
//! painting the indices as near-black grey levels, even those whose entries were present.
//!
//! The images are 8 × 1, samples 0 to 7, each column 25 px wide on a 200 × 200 page (the layout
//! of the seven cases A to G a reviewer compared across readers on #4393).

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderWarning, RenderedPage, RequestKind};

/// The eight entries the tests' tables hold, in order.
const TABLE: &str = "FF0000 00FF00 0000FF FFFF00 FF00FF 00FFFF 808080 000000";
const COLOURS: [[u8; 3]; 8] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0], [255, 0, 255], [0, 255, 255], [128, 128, 128], [0, 0, 0]];
const BLACK: [u8; 3] = [0, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];
const DRAW_IMAGE: &str = "q 200 0 0 200 0 0 cm /Im Do Q";
const MALFORMED: [RenderWarning; 1] = [RenderWarning::MalformedIndexedImage];

/// The first `n` hex entries of `TABLE`.
fn entries(n: usize) -> String {
    TABLE.split(' ').take(n).collect::<Vec<_>>().join(" ")
}

/// A 200 × 200 page whose `/Im` image is eight samples wide (indices 0 to 7) and covers the
/// page, with `/P` naming `colour_space` as a resource. `extra` objects are numbered from 6.
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
            "<< /Type /XObject /Subtype /Image /Width 8 /Height 1 /BitsPerComponent 8 \
             /ColorSpace {colour_space} /Filter /ASCIIHexDecode /Length 17 >>\nstream\n0001020304050607>\nendstream"
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

/// The centre pixel of each of the eight columns (indices 0 to 7), RGB.
fn columns(page: &RenderedPage) -> [[u8; 3]; 8] {
    std::array::from_fn(|i| {
        let at = (100 * page.width as usize + 12 + 25 * i) * 4;
        let px = page.rgba.get(at..at + 3).expect("pixel inside the page");
        [px[0], px[1], px[2]]
    })
}

/// The first `n` of `COLOURS`, then black: a table of `n` entries padded with zero bytes.
fn padded(n: usize) -> [[u8; 3]; 8] {
    std::array::from_fn(|i| if i < n { COLOURS[i] } else { BLACK })
}

/// Index `i` drawn as the grey level `i` / 255: the DeviceGray fallback.
fn grey_fallback() -> [[u8; 3]; 8] {
    std::array::from_fn(|i| [i as u8; 3])
}

#[track_caller]
fn assert_page(colour_space: &str, extra: &[&[u8]], want: [[u8; 3]; 8], warned: bool) {
    let page = render(pdf(colour_space, DRAW_IMAGE, extra));
    assert_eq!(columns(&page), want, "{colour_space}: the columns");
    let want_warnings: &[RenderWarning] = if warned { &MALFORMED } else { &[] };
    assert_eq!(page.warnings, want_warnings, "{colour_space}: the warnings");
}

/// Case A: a full table draws its eight colours, with no warning.
#[test]
fn a_full_table_draws_its_colours() {
    assert_page(&format!("[/Indexed /DeviceRGB 7 <{TABLE}>]"), &[], COLOURS, false);
    assert_page(&format!("[/Indexed /DeviceRGB 7.0 <{TABLE}>]"), &[], COLOURS, false);
}

/// Cases B and F: a short table is padded with zero bytes. The entries it has keep their
/// colours, and the rest are black over DeviceRGB.
#[test]
fn a_short_table_is_padded_with_zero_bytes() {
    // B: hival 7 needs 24 bytes; 6 are given. Indices 0 and 1 used to draw black.
    assert_page(&format!("[/Indexed /DeviceRGB 7 <{}>]", entries(2)), &[], padded(2), true);
    // F: one entry short.
    assert_page(&format!("[/Indexed /DeviceRGB 7 <{}>]", entries(7)), &[], padded(7), true);
    // One byte short: entry 1 is 00FF plus a zero byte, green; indices past hival take entry 1.
    let mut want = [COLOURS[1]; 8];
    want[0] = COLOURS[0];
    assert_page("[/Indexed /DeviceRGB 1 <FF0000 00FF>]", &[], want, true);
}

/// Case C: an index past `hival` takes the nearest entry (ISO 32000-2 §8.6.6.3: "shall be
/// adjusted to the nearest value within that range"); a table longer than needed is fine.
#[test]
fn an_index_past_hival_takes_the_last_entry() {
    let mut want = COLOURS;
    want[4..].fill(COLOURS[3]);
    assert_page(&format!("[/Indexed /DeviceRGB 3 <{TABLE}>]"), &[], want, false);
}

/// Case D: a `hival` past 255 is taken as 255, and the table padded to 256 entries.
#[test]
fn a_hival_past_255_is_clamped() {
    assert_page(&format!("[/Indexed /DeviceRGB 300 <{TABLE}>]"), &[], COLOURS, true);
    // With a table long enough for 301 entries, the clamp alone is reported.
    let long = format!("<{TABLE}{}>", "00".repeat(3 * 293));
    assert_page(&format!("[/Indexed /DeviceRGB 300 {long}]"), &[], COLOURS, true);
    // 256, one past.
    assert_page(&format!("[/Indexed /DeviceRGB 256 <{TABLE}{}>]", "00".repeat(3 * 249)), &[], COLOURS, true);
    // 255, the largest, with a full table draws without a warning.
    assert_page(&format!("[/Indexed /DeviceRGB 255 <{TABLE}{}>]", "00".repeat(3 * 248)), &[], COLOURS, false);
}

/// Case E: a real `hival` is truncated toward zero, as hayro read it before patch (60).
#[test]
fn a_real_hival_is_truncated() {
    assert_page(&format!("[/Indexed /DeviceRGB 7.5 <{TABLE}>]"), &[], COLOURS, true);
    // 6.9 is 6, not 7: index 7 takes entry 6.
    let mut want = COLOURS;
    want[7] = COLOURS[6];
    assert_page(&format!("[/Indexed /DeviceRGB 6.9 <{TABLE}>]"), &[], want, true);
    // -0.5 is 0 (toward zero; a real above -1 truncates to 0): every index takes entry 0.
    assert_page(&format!("[/Indexed /DeviceRGB -0.5 <{TABLE}>]"), &[], [COLOURS[0]; 8], true);
    // 255.5 is 255; 255.0 written as a real is the integer 255.
    let full = format!("<{TABLE}{}>", "00".repeat(3 * 248));
    assert_page(&format!("[/Indexed /DeviceRGB 255.5 {full}]"), &[], COLOURS, true);
    assert_page(&format!("[/Indexed /DeviceRGB 255.0 {full}]"), &[], COLOURS, false);
}

/// Case G: inline images, named through the resources or given in place, are padded too.
#[test]
fn inline_images_with_a_short_table_are_padded_too() {
    let inline = |cs: &str| format!("q 200 0 0 200 0 0 cm BI /W 8 /H 1 /BPC 8 /CS {cs} /F /AHx ID 0001020304050607> EI Q");
    let short = format!("[/Indexed /DeviceRGB 7 <{}>]", entries(2));
    // Named through the page's resources.
    let page = render(pdf_with_resource("CS0", &short, &inline("/CS0"), &[]));
    assert_eq!(columns(&page), padded(2));
    assert_eq!(page.warnings, MALFORMED);
    // Given in place, with the inline abbreviations.
    let page = render(pdf("/DeviceRGB", &inline(&format!("[/I /RGB 7 <{}>]", entries(2))), &[]));
    assert_eq!(columns(&page), padded(2));
    assert_eq!(page.warnings, MALFORMED);
    // A `hival` with no reading falls back and is reported, named or in place.
    let page = render(pdf(&format!("[/Indexed /DeviceRGB -1 <{TABLE}>]"), &inline("/P"), &[]));
    assert_eq!((columns(&page), page.warnings), (grey_fallback(), MALFORMED.to_vec()));
    let page = render(pdf("/DeviceRGB", &inline(&format!("[/I /RGB -1 <{TABLE}>]")), &[]));
    assert_eq!((columns(&page), page.warnings), (grey_fallback(), MALFORMED.to_vec()));
    // A well-formed one draws without a warning.
    let page = render(pdf(&format!("[/Indexed /DeviceRGB 7 <{TABLE}>]"), &inline("/P"), &[]));
    assert_eq!(columns(&page), COLOURS);
    assert!(page.warnings.is_empty(), "{:?}", page.warnings);
}

#[test]
fn an_inline_device_space_name_ignores_a_resource_of_the_same_name() {
    // ISO 32000-2 §8.6.8 (the CS operator): the device colour space names never refer to
    // resources in the ColorSpace subdictionary, so a malformed Indexed resource that shares the
    // name (here the inline abbreviation `/RGB`, and `/DeviceRGB`) is not used.
    let data: String = TABLE.split(' ').collect();
    for (resource, cs) in [("RGB", "/RGB"), ("DeviceRGB", "/DeviceRGB")] {
        let content = format!("q 200 0 0 200 0 0 cm BI /W 8 /H 1 /BPC 8 /CS {cs} /F /AHx ID {data}> EI Q");
        let page = render(pdf_with_resource(resource, "[/Indexed /DeviceRGB 7 <FF0000 00FF00>]", &content, &[]));
        assert_eq!(columns(&page), COLOURS, "{cs}: the device space draws");
        assert!(page.warnings.is_empty(), "{cs}: {:?}", page.warnings);
    }
}

#[test]
fn a_page_with_several_malformed_images_reports_the_warning_once() {
    let content = format!("{DRAW_IMAGE} {DRAW_IMAGE} 0 0 1 rg 0 0 10 10 re f");
    let page = render(pdf(&format!("[/Indexed /DeviceRGB 7 <{}>]", entries(2)), &content, &[]));
    assert_eq!(page.warnings, MALFORMED);
    assert_eq!(columns(&page), padded(2));
    let at = (195 * page.width as usize + 5) * 4;
    assert_eq!(page.rgba.get(at..at + 3), Some(&[0u8, 0, 255][..]), "content after the images still draws");
}

#[test]
fn a_lookup_string_given_by_reference_is_read() {
    // A lookup is a byte string or a stream (ISO 32000-2 §8.6.6.3), and either may be indirect.
    // A string by reference used to be missed: trying it as a stream consumed the reference.
    let full = format!("<{TABLE}>");
    assert_page("[/Indexed /DeviceRGB 7 6 0 R]", &[full.as_bytes()], COLOURS, false);
    let short = format!("<{}>", entries(2));
    assert_page("[/Indexed /DeviceRGB 7 6 0 R]", &[short.as_bytes()], padded(2), true);
    // Something that is neither a string nor a stream is no table: every entry is padding.
    assert_page("[/Indexed /DeviceRGB 7 6 0 R]", &[b"42"], padded(0), true);
}

#[test]
fn padding_holds_at_the_bounds_of_hival_and_the_base() {
    // Four components (DeviceCMYK): hival 1 needs 8 bytes. With 7, the missing byte is entry 1's
    // black, and zero there is what the full table holds, so both draw the same.
    let full = render(pdf("[/Indexed /DeviceCMYK 1 <FF000000 00FF0000>]", DRAW_IMAGE, &[]));
    assert!(full.warnings.is_empty(), "{:?}", full.warnings);
    let short = render(pdf("[/Indexed /DeviceCMYK 1 <FF000000 00FF00>]", DRAW_IMAGE, &[]));
    assert_eq!(short.warnings, MALFORMED);
    assert_eq!(columns(&short), columns(&full));
    assert_ne!(columns(&full)[0], WHITE, "the image is drawn");
    // One component (DeviceGray): hival 0 needs one byte, and an empty table pads it.
    assert_page("[/Indexed /DeviceGray 0 <80>]", &[], [[128; 3]; 8], false);
    assert_page("[/Indexed /DeviceGray 0 <>]", &[], [BLACK; 8], true);
    // hival 255 over DeviceRGB needs 768 bytes: 767 is one short.
    assert_page(&format!("[/Indexed /DeviceRGB 255 <{TABLE}{}>]", "00".repeat(3 * 248 - 1)), &[], COLOURS, true);
}

#[test]
fn a_hival_that_would_overflow_a_size_is_clamped_without_panicking() {
    // Values whose (hival + 1) × components would overflow a 32-bit or 64-bit size, or that do
    // not fit an integer at all, are taken as 255.
    for hival in ["4294967295", "4294967296", "18446744073709551615", "99999999999999999999999"] {
        assert_page(&format!("[/Indexed /DeviceRGB {hival} <{}>]", entries(2)), &[], padded(2), true);
    }
}

/// A `hival` that is negative or not a number has no reading to recover: the image keeps the
/// upstream handling (it falls back to DeviceGray), and the warning is still reported.
#[test]
fn a_negative_or_non_numeric_hival_falls_back_with_a_warning() {
    for hival in ["-1", "-1.5", "-2147483649", "/One", "null"] {
        assert_page(&format!("[/Indexed /DeviceRGB {hival} <{TABLE}>]"), &[], grey_fallback(), true);
    }
}

#[test]
fn an_empty_or_missing_lookup_is_all_padding() {
    for lookup in ["<>", "()", "", "null"] {
        assert_page(&format!("[/Indexed /DeviceRGB 1 {lookup}]"), &[], padded(0), true);
    }
}

#[test]
fn a_lookup_stream_is_measured_after_decoding() {
    let short: &[u8] = b"<< /Length 14 /Filter /ASCIIHexDecode >>\nstream\nFF0000 00FF00>\nendstream";
    assert_page("[/Indexed /DeviceRGB 7 6 0 R]", &[short], padded(2), true);
    let mut want = [COLOURS[1]; 8];
    want[0] = COLOURS[0];
    assert_page("[/Indexed /DeviceRGB 1 6 0 R]", &[short], want, false);
    let empty: &[u8] = b"<< /Length 0 >>\nstream\n\nendstream";
    assert_page("[/Indexed /DeviceRGB 1 6 0 R]", &[empty], padded(0), true);
}

#[test]
fn a_lookup_stream_that_cannot_be_decoded_is_all_padding() {
    let broken: &[u8] = b"<< /Length 6 /Filter /FlateDecode >>\nstream\nnot-zl\nendstream";
    assert_page("[/Indexed /DeviceRGB 1 6 0 R]", &[broken], padded(0), true);
}

/// An Indexed space whose base cannot be read keeps the upstream handling (DeviceGray), whatever
/// its `hival`; only a `hival` with no reading is reported.
#[test]
fn an_unreadable_base_keeps_its_fallback() {
    for (hival, warned) in [("1", false), ("300", false), ("-1", true)] {
        assert_page(&format!("[/Indexed /NoSuchSpace {hival} <{TABLE}>]"), &[], grey_fallback(), warned);
    }
}

/// A fill colour set in such a space (`cs`, `sc`) draws through the same recovered table, and
/// the recovery is reported as `ColourFallback`, as the upstream DeviceGray fallback was.
#[test]
fn a_fill_colour_in_a_short_table_is_padded_and_reported() {
    let fill = |index: u8| format!("/P cs {index} sc 0 0 200 200 re f");
    let short = format!("[/Indexed /DeviceRGB 7 <{}>]", entries(2));
    let page = render(pdf(&short, &fill(1), &[]));
    assert_eq!(columns(&page)[0], COLOURS[1], "entry 1 is green");
    assert_eq!(page.warnings, vec![RenderWarning::ColourFallback]);
    let page = render(pdf(&short, &fill(5), &[]));
    assert_eq!(columns(&page)[0], BLACK, "entry 5 is padding");
    let page = render(pdf(&format!("[/Indexed /DeviceRGB 7 <{TABLE}>]"), &fill(1), &[]));
    assert_eq!(columns(&page)[0], COLOURS[1]);
    assert!(page.warnings.is_empty(), "{:?}", page.warnings);
}
