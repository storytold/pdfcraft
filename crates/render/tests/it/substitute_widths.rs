//! Contributor-original synthetic fonts: a simple font the document names but doesn't embed is
//! drawn with a standard-font stand-in, its glyphs placed at the advances `/Widths` gives
//! (ISO 32000-2 §9.6.2.1, Table 109) and their shapes kept.

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
use std::sync::Arc;

const SIZE: f64 = 80.0;

fn pdf(objects: &[String]) -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

/// A page showing `text` at 80 pt in the font dictionary `font`, with the font descriptor
/// `descriptor` as object 6.
fn page(font: &str, descriptor: &str, text: &str) -> Vec<u8> {
    let body = format!("BT /F1 {SIZE} Tf 20 30 Td ({text}) Tj ET");
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 140] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        font.into(),
        descriptor.into(),
    ])
}

/// The runs of columns holding dark pixels, as (first, last) column.
fn ink_runs(bytes: Vec<u8>) -> Vec<(u32, u32)> {
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let image = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(image.error.is_none(), "{:?}", image.error);
    let dark = |x: u32| {
        (0..image.height).any(|y| {
            let at = usize::try_from((y * image.width + x) * 4).unwrap_or(usize::MAX);
            image.rgba.get(at).is_some_and(|r| *r < 128)
        })
    };
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for x in 0..image.width {
        if dark(x) {
            match runs.last_mut() {
                Some(run) if run.1 + 1 == x => run.1 = x,
                _ => runs.push((x, x)),
            }
        }
    }
    runs
}

fn standard(name: &str, text: &str) -> Vec<(u32, u32)> {
    ink_runs(page(&format!("<< /Type /Font /Subtype /Type1 /BaseFont /{name} /Encoding /WinAnsiEncoding >>"), "<< >>", text))
}

fn substituted(base_font: &str, flags: u32, weight: u32, code: u8, width: u32, text: &str) -> Vec<(u32, u32)> {
    let font = format!(
        "<< /Type /Font /Subtype /TrueType /BaseFont /{base_font} /Encoding /WinAnsiEncoding \
         /FirstChar {code} /LastChar {code} /Widths [{width}] /FontDescriptor 6 0 R >>"
    );
    let descriptor = format!(
        "<< /Type /FontDescriptor /FontName /{base_font} /Flags {flags} /FontBBox [-600 -200 2000 1000] \
         /ItalicAngle 0 /Ascent 900 /Descent -200 /CapHeight 700 /StemV 140 /FontWeight {weight} >>"
    );
    ink_runs(page(&font, &descriptor, text))
}

/// A `/Widths` entry wider than the stand-in's glyph moves the next glyph on, and the glyph keeps
/// its own shape: every advance of 600 stretched each glyph of `Arial,Bold` to 600 units, so the
/// stems of `i` and `l` became solid blocks twice their width and the text read as a heavy
/// fixed-pitch face.
#[test]
fn a_substitute_keeps_its_glyph_shapes_at_wider_advances() {
    for (base_font, flags, weight, stand_in) in [
        ("Arial,Bold", 32, 700, "Helvetica-Bold"),
        ("Arial,Bold", 4, 700, "Helvetica-Bold"),
        ("Arial-BoldMT", 32, 700, "Helvetica-Bold"),
        ("ArialMT", 32, 400, "Helvetica"),
    ] {
        let reference = standard(stand_in, "l");
        let [(ref_start, ref_end)] = reference[..] else { panic!("{stand_in}: one stem, {reference:?}") };
        let runs = substituted(base_font, flags, weight, b'l', 600, "ll");
        let [(first, first_end), (second, second_end)] = runs[..] else { panic!("{base_font}: two stems, {runs:?}") };
        for (start, end) in [(first, first_end), (second, second_end)] {
            assert!(
                (end - start).abs_diff(ref_end - ref_start) <= 1,
                "{base_font} /Flags {flags}: the stem is {stand_in}'s, not stretched: {runs:?} vs {reference:?}"
            );
        }
        // 600 units at 80 pt is 48 pixels.
        assert!((second - first).abs_diff(48) <= 1, "{base_font}: the second l is 600 units on: {runs:?}");
        assert!(first.abs_diff(ref_start) <= 1, "{base_font}: the first l starts where {stand_in}'s does: {runs:?} vs {reference:?}");
    }
}

/// A `/Widths` entry narrower than the stand-in's glyph still narrows the glyph to it, so
/// neighbours don't overlap: `W` (944 units in Helvetica-Bold) at an advance of 400.
#[test]
fn a_substitute_still_narrows_glyphs_to_narrower_advances() {
    let runs = substituted("Arial,Bold", 32, 700, b'W', 400, "WW");
    let [(first, first_end), (second, _)] = runs[..] else { panic!("two separate glyphs: {runs:?}") };
    // 400 units at 80 pt is 32 pixels.
    assert!(first_end - first <= 33, "the W is narrowed to its advance: {runs:?}");
    assert!((second - first).abs_diff(32) <= 1, "the second W is 400 units on: {runs:?}");
}
