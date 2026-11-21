//! A Type 3 font without `/Widths` advances each glyph by the width its glyph procedure sets
//! with `d0` or `d1` (vendored hayro-interpret patch 90). ISO 32000-2 Table 110 requires
//! `/Widths` in a Type 3 font, and §9.6.4 has the procedure state the same width: "wx … shall
//! be consistent with the corresponding width in the font's Widths array". A font whose
//! `/Widths` key was misspelt (PDFBOX-2943) gave every glyph no advance, so its text was drawn
//! as one blot and read as characters stacked on one spot. PDFium takes the procedure's width
//! here. A font that has `/Widths` is unchanged: Table 110 gives a code outside `/FirstChar` to
//! `/LastChar` width 0, as Poppler and MuPDF draw it.
//!
//! The glyphs are squares 400 units wide (`a`, set by `d1`, advance 500) and 600 units wide
//! (`b`, set by `d0`, advance 700), shown at 100 pt from x = 20.

use super::render_first_page;
use crate::RenderWarning;
use crate::text::extract_page;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use std::sync::Arc;

/// A page showing `aabb` in a Type 3 font whose dictionary ends with `widths`.
fn type3_page(widths: &str) -> Vec<u8> {
    type3_page_with(widths, "aabb", "700 0 d0 0 0 600 600 re f")
}

/// A page showing `text` in a Type 3 font whose dictionary ends with `widths` and whose `b`
/// glyph is `glyph_b`.
fn type3_page_with(widths: &str, text: &str, glyph_b: &str) -> Vec<u8> {
    let content = format!("BT /F1 100 Tf 20 100 Td ({text}) Tj ET");
    let glyph_a = "500 0 0 0 400 400 d1 0 0 400 400 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] \
             /CharProcs << /a 6 0 R /b 7 0 R >> /Encoding << /Type /Encoding /Differences [97 /a /b] >> {widths} >>"
        ),
        format!("<< /Length {} >>\nstream\n{glyph_a}\nendstream", glyph_a.len()),
        format!("<< /Length {} >>\nstream\n{glyph_b}\nendstream", glyph_b.len()),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n{object}\nendobj\n", i + 1).into_bytes());
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).into_bytes());
    for offset in offsets {
        pdf.extend(format!("{offset:010} 00000 n \n").into_bytes());
    }
    pdf.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).into_bytes());
    pdf
}

/// The columns (start, end) of the rendered page that hold ink, as runs.
fn ink_runs(pdf: Vec<u8>) -> Vec<(u32, u32)> {
    let page = render_first_page(pdf);
    let inked = |x: u32| (0..page.height).any(|y| page.rgba.get(((y * page.width + x) * 4) as usize).is_some_and(|v| *v < 128));
    let mut runs = Vec::new();
    let mut start = None;
    for x in 0..=page.width {
        match (x < page.width && inked(x), start) {
            (true, None) => start = Some(x),
            (false, Some(s)) => {
                runs.push((s, x));
                start = None;
            }
            _ => {}
        }
    }
    runs
}

/// The left edge of each extracted glyph, rounded to a point.
fn glyph_lefts(pdf: Vec<u8>) -> Vec<i32> {
    let pdf = Pdf::new(Arc::new(pdf)).expect("parses");
    let text = extract_page(&pdf, 0, &InterpreterSettings::default()).expect("text");
    text.glyphs.iter().map(|g| g.rect[0].round() as i32).collect()
}

#[test]
fn a_type3_font_without_widths_advances_by_its_glyph_procedures() {
    // Each glyph in its own place: a at 20 and 70 (d1 width 500), b at 120 and 190 (d0 700).
    for widths in ["", "/FirstChar 97 /LastChar 98 /WXdths [300 300]"] {
        assert_eq!(ink_runs(type3_page(widths)), [(20, 60), (70, 110), (120, 180), (190, 250)], "{widths:?}");
        assert_eq!(glyph_lefts(type3_page(widths)), [20, 70, 120, 190], "{widths:?}");
    }
}

#[test]
fn a_code_outside_first_char_to_last_char_still_has_width_zero() {
    // a takes /Widths (300); b, past /LastChar, has width 0 (Table 110), whatever d0 says.
    let widths = "/FirstChar 97 /LastChar 97 /Widths [300]";
    assert_eq!(ink_runs(type3_page(widths)), [(20, 140)]);
    assert_eq!(glyph_lefts(type3_page(widths)), [20, 50, 80]);
}

#[test]
fn widths_the_font_gives_are_kept() {
    // /Widths says 300 for both, whatever the procedures say: the glyphs overlap, as written.
    let widths = "/FirstChar 97 /LastChar 98 /Widths [300 300]";
    assert_eq!(ink_runs(type3_page(widths)), [(20, 170)]);
    assert_eq!(glyph_lefts(type3_page(widths)), [20, 50, 80, 110]);
    // An explicit 0 is an entry too: every glyph stays at x = 20 (text extraction keeps one
    // of each glyph drawn again on the same spot).
    let zero = "/FirstChar 97 /LastChar 98 /Widths [0 0]";
    assert_eq!(glyph_lefts(type3_page(zero)), [20, 20]);
}

#[test]
fn a_code_without_a_usable_procedure_width_still_advances_nothing() {
    // b's d0 has a name for its width and c has no procedure: neither advances, and neither
    // panics.
    assert_eq!(glyph_lefts(type3_page_with("", "bcb", "/x 0 d0 0 0 600 600 re f")), [20, 20]);
}

#[test]
fn a_type3_font_without_widths_is_reported() {
    // The page draws, and says once that a font's /Widths was missing; a font with /Widths
    // (even one that leaves `b` out) says nothing.
    for widths in ["", "/FirstChar 97 /LastChar 98 /WXdths [300 300]"] {
        let page = render_first_page(type3_page(widths));
        assert_eq!(page.warnings, [RenderWarning::Type3WidthsMissing], "{widths:?}");
    }
    for widths in ["/FirstChar 97 /LastChar 98 /Widths [300 300]", "/FirstChar 97 /LastChar 97 /Widths [300]"] {
        assert!(render_first_page(type3_page(widths)).warnings.is_empty(), "{widths:?}");
    }
}
