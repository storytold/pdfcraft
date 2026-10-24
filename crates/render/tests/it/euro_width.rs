//! A standard 14 font named without `/Widths` draws each glyph within the advance its standard
//! metrics give (ISO 32000-2 §9.6.2.2), including the Euro at WinAnsiEncoding code 128 (Annex D,
//! Table D.2): the bundled stand-ins draw their € up to 722 units wide, and it overlapped the
//! glyph after it in Helvetica and Times.

use pdfcraft_fonts::Std14;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
use std::sync::Arc;

/// 100 pt at scale 1 is 100 pixels to the em, so a width of 556 units is 55.6 pixels.
const SIZE: f64 = 100.0;
const EURO: u8 = 0x80;

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

/// The runs of columns holding dark pixels, as (first, last) column, of `text` (a PDF literal
/// string body) shown at 100 pt in the standard font `name` with WinAnsiEncoding and no
/// `/Widths`, starting 20 pt from the left edge.
fn ink_runs(name: &str, text: &str) -> Vec<(u32, u32)> {
    let body = format!("BT /F1 {SIZE} Tf 20 40 Td ({text}) Tj ET");
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 160] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /{name} /Encoding /WinAnsiEncoding >>"),
    ]);
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

/// `a€b`: the € keeps clear of both neighbours, its ink is no wider than its advance, and each
/// glyph sits where the advances the writer measures with (`Std14`) put it: 556 units for the
/// Helvetica family, 500 for Times and 600 for Courier.
#[test]
fn the_euro_fits_its_standard_advance() {
    for (name, font, euro) in [
        ("Helvetica", Std14::Helvetica, 556),
        ("Helvetica-Bold", Std14::HelveticaBold, 556),
        ("Times-Roman", Std14::TimesRoman, 500),
        ("Times-Bold", Std14::TimesBold, 500),
        ("Courier", Std14::Courier, 600),
        ("Courier-Bold", Std14::CourierBold, 600),
    ] {
        assert_eq!(font.width(EURO), f64::from(euro), "{name}: the writer's Euro width");
        let advance = font.advance(EURO, SIZE);
        let a = font.advance(b'a', SIZE);

        let runs = ink_runs(name, "a\\200b");
        let [_, (euro_start, euro_end), (b_start, _)] = runs[..] else { panic!("{name}: a, € and b apart: {runs:?}") };
        let [(alone_euro, _)] = ink_runs(name, "\\200")[..] else { panic!("{name}: one €") };
        let [(alone_b, _)] = ink_runs(name, "b")[..] else { panic!("{name}: one b") };

        assert!(f64::from(euro_end - euro_start + 1) <= advance + 1.0, "{name}: the € is no wider than {euro} units: {runs:?}");
        assert!((f64::from(euro_start) - f64::from(alone_euro) - a).abs() <= 1.0, "{name}: the € starts one `a` on: {runs:?}");
        assert!((f64::from(b_start) - f64::from(alone_b) - a - advance).abs() <= 1.0, "{name}: the b starts {euro} units after the €: {runs:?}");
    }
}

/// `€€`: two Euros side by side stay two glyphs, the second one advance on.
#[test]
fn two_euros_stay_apart() {
    for (name, font) in [("Helvetica", Std14::Helvetica), ("Times-Roman", Std14::TimesRoman), ("Times-Bold", Std14::TimesBold)] {
        let runs = ink_runs(name, "\\200\\200");
        let [(first, _), (second, _)] = runs[..] else { panic!("{name}: two separate Euros: {runs:?}") };
        assert!((f64::from(second - first) - font.advance(EURO, SIZE)).abs() <= 1.0, "{name}: {runs:?}");
    }
}

/// The slanted faces: a lone € is no wider than its advance either. (Their neighbours can share
/// a column with it through the slant alone, so the gaps of `a€b` aren't a measure there.)
#[test]
fn a_slanted_euro_fits_its_standard_advance() {
    for (name, font) in [
        ("Helvetica-Oblique", Std14::HelveticaOblique),
        ("Helvetica-BoldOblique", Std14::HelveticaBoldOblique),
        ("Times-BoldItalic", Std14::TimesBoldItalic),
    ] {
        let runs = ink_runs(name, r"\200");
        let [(start, end)] = runs[..] else { panic!("{name}: one €: {runs:?}") };
        assert!(f64::from(end - start + 1) <= font.advance(EURO, SIZE) + 1.0, "{name}: {runs:?}");
    }
}
