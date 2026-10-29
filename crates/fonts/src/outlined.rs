//! Text drawn as filled glyph outlines, for generated appearances that have nowhere to keep a font.
//!
//! An annotation's appearance stream is built from its dictionary alone (`pdfcraft_annot::
//! appearance::build`), with direct resources, so it can't carry a Type 3 font (its glyph
//! procedures would have to be indirect streams). Text the standard fonts can't draw, which would
//! come out as `?`, is drawn instead as the outlines of a craft-fonts face: today Korean, with the
//! Gothic face. A line that needs outlines is drawn entirely from the face, Latin included, so it
//! keeps one look.
//!
//! The outlines are only the picture. So that the words stay searchable and copyable where they
//! are drawn — also once the appearance is flattened into the page and `/Contents` or `/V` is gone
//! — each outlined line also carries an invisible text layer ([`outlined_text_layer`], rendering
//! mode 3, as OCR text layers are) in a Type 0 font addressed by UTF-16 over a non-embedded
//! Adobe-Korea1 CID font ([`outline_text_font`]). Its glyphs are never painted.

use pdfcraft_cos::{Dict, Document, Object, PdfString};

use crate::{GlyphError, ShapedScript, korean_has, shape_korean, shaped_glyph, win_ansi_encodable};

/// The resource name of [`outline_text_font`] in an appearance's `/Font` dictionary.
pub const OUTLINE_TEXT_FONT: &str = "PCKoText";

/// The font of the invisible text layer: `/Type0` with the predefined `UniKS-UTF16-H` CMap, so a
/// string is the line's UTF-16BE and any reader maps it back to Unicode, over a non-embedded
/// Adobe-Korea1 CID font (ISO 32000-2 §9.7.5.2). Every character advances one em (`/DW 1000`).
/// The descriptor is direct, as an appearance's resources must be; [`hoist_font_descriptors`]
/// makes it the indirect object §9.8.1 asks for when the appearance is stored in a document.
pub fn outline_text_font() -> Dict {
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name("NanumGothic"));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"FontBBox".to_vec(), Object::Array([0, -150, 1000, 880].iter().map(|v| Object::Int(*v)).collect()));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    descriptor.set(b"Ascent".to_vec(), Object::Int(880));
    descriptor.set(b"Descent".to_vec(), Object::Int(-150));
    descriptor.set(b"CapHeight".to_vec(), Object::Int(700));
    descriptor.set(b"StemV".to_vec(), Object::Int(80));
    let mut info = Dict::new();
    info.set(b"Registry".to_vec(), Object::String(PdfString::literal(b"Adobe".to_vec())));
    info.set(b"Ordering".to_vec(), Object::String(PdfString::literal(b"Korea1".to_vec())));
    info.set(b"Supplement".to_vec(), Object::Int(2));
    let mut cid = Dict::new();
    cid.set(b"Type".to_vec(), Object::name("Font"));
    cid.set(b"Subtype".to_vec(), Object::name("CIDFontType0"));
    cid.set(b"BaseFont".to_vec(), Object::name("NanumGothic"));
    cid.set(b"CIDSystemInfo".to_vec(), Object::Dict(info));
    cid.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    cid.set(b"DW".to_vec(), Object::Int(1000));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type0"));
    font.set(b"BaseFont".to_vec(), Object::name("NanumGothic"));
    font.set(b"Encoding".to_vec(), Object::name("UniKS-UTF16-H"));
    font.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(cid)]));
    font
}

/// The invisible text of an outlined line `s` at baseline (`x`, `y`) in [`OUTLINE_TEXT_FONT`],
/// squeezed horizontally (`Tz`) to the `width` the outlines take, so a selection covers them.
/// Self-contained (`q … Q`), so the text state it sets doesn't leak into what follows. Empty for
/// an empty line or non-finite numbers.
pub fn outlined_text_layer(s: &str, size: f64, x: f64, y: f64, width: f64) -> String {
    let count = s.chars().count();
    if count == 0 || !(size.is_finite() && size > 0.0 && x.is_finite() && y.is_finite() && width.is_finite()) {
        return String::new();
    }
    let natural = count as f64 * size;
    let scale = (width / natural * 100.0).clamp(1.0, 1000.0);
    let hex: String = s.encode_utf16().map(|u| format!("{u:04X}")).collect();
    format!("q BT 3 Tr /{OUTLINE_TEXT_FONT} {} Tf {} Tz 1 0 0 1 {} {} Tm <{hex}> Tj ET Q\n", num(size), num(scale), num(x), num(y))
}

/// Make every direct font descriptor under `resources`' `/Font` entries (a Type 0 font's
/// descendant, or a simple font) an indirect object of `doc`, as ISO 32000-2 §9.8.1 requires. For
/// an appearance built with direct resources ([`outline_text_font`]) when it is stored.
pub fn hoist_font_descriptors(doc: &mut Document, resources: &mut Dict) {
    let Some(Object::Dict(fonts)) = resources.get(b"Font").cloned() else { return };
    let mut fonts = fonts;
    let names: Vec<Vec<u8>> = fonts.iter().map(|(k, _)| k.clone()).collect();
    for name in names {
        let Some(Object::Dict(mut font)) = fonts.get(&name).cloned() else { continue };
        if let Some(Object::Dict(d)) = font.get(b"FontDescriptor").cloned() {
            font.set(b"FontDescriptor".to_vec(), Object::Ref(doc.add(Object::Dict(d))));
        }
        if let Some(Object::Array(descendants)) = font.get(b"DescendantFonts").cloned() {
            let hoisted = descendants
                .into_iter()
                .map(|d| match d {
                    Object::Dict(mut cid) => {
                        if let Some(Object::Dict(fd)) = cid.get(b"FontDescriptor").cloned() {
                            cid.set(b"FontDescriptor".to_vec(), Object::Ref(doc.add(Object::Dict(fd))));
                        }
                        Object::Dict(cid)
                    }
                    other => other,
                })
                .collect();
            font.set(b"DescendantFonts".to_vec(), Object::Array(hoisted));
        }
        fonts.set(name, Object::Dict(font));
    }
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
}

/// The face outlines are drawn from: the Korean Gothic, regular.
const SERIF: bool = false;
const BOLD: bool = false;

/// Whether `s` has a character the standard fonts can't draw (line breaks don't count), so that
/// it is drawn with [`outlined_path`] rather than in Helvetica.
pub fn needs_outlines(s: &str) -> bool {
    s.chars().any(|c| !matches!(c, '\n' | '\r') && !win_ansi_encodable(c))
}

/// The first character of `s` that neither the standard fonts nor the outline face can draw: the
/// one an appearance would still show as `?`. `None` when all of it can be drawn. Without a
/// Korean face this is [`crate::first_undrawable`].
pub fn first_unoutlinable(s: &str) -> Option<char> {
    // Korean only for now: whatever the Korean face has (Hangul, Hanja, CJK punctuation). A line
    // that needs outlines is drawn wholly from the face, so its Latin must be in the face too;
    // checked per paragraph, which holds for every line it is wrapped into.
    s.split(['\n', '\r']).filter(|para| needs_outlines(para)).find_map(|para| {
        let lacks = |c: char| c != '\t' && !korean_has(SERIF, BOLD, c);
        para.chars().find(|&c| !win_ansi_encodable(c) && lacks(c)).or_else(|| para.chars().find(|&c| lacks(c)))
    })
}

/// The advance of `s` at `size` points in the outline face, or `None` when it can't shape `s`.
pub fn outlined_width(s: &str, size: f64) -> Option<f64> {
    // A tab is a space, as in WinAnsi text.
    let clusters = shape_korean(&s.replace('\t', " "), SERIF, BOLD).ok()?;
    let w = clusters.iter().map(|c| c.advance).sum::<f64>() * size;
    w.is_finite().then_some(w)
}

/// Path construction operators (`m`, `l`, `h`) for `s` at `size` points with its baseline
/// starting at (`x`, `y`), to be filled by the caller (`f`). Inside no text object: these are
/// path operators. Errors when the face is missing or lacks a character.
pub fn outlined_path(s: &str, size: f64, x: f64, y: f64) -> Result<String, GlyphError> {
    if !(size.is_finite() && x.is_finite() && y.is_finite()) {
        return Err(GlyphError::TooComplex);
    }
    let script = ShapedScript::Korean { serif: SERIF, bold: BOLD };
    let mut out = String::new();
    let mut pen = 0.0;
    for cluster in shape_korean(&s.replace('\t', " "), SERIF, BOLD)? {
        for (id, [dx, dy]) in &cluster.glyphs {
            let glyph = shaped_glyph(script, *id)?;
            for contour in &glyph.contours {
                for (i, p) in contour.iter().enumerate() {
                    let px = x + (pen + dx + p[0]) * size;
                    let py = y + (dy + p[1]) * size;
                    out.push_str(&format!("{} {} {}\n", num(px), num(py), if i == 0 { "m" } else { "l" }));
                }
                if !contour.is_empty() {
                    out.push_str("h\n");
                }
            }
        }
        pen += cluster.advance;
    }
    Ok(out)
}

/// A coordinate with at most three decimals, without trailing zeros.
fn num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_layer_is_invisible_utf16_squeezed_to_the_outlines() {
        let layer = outlined_text_layer("한 A", 10.0, 5.0, 7.0, 15.0);
        assert_eq!(layer, "q BT 3 Tr /PCKoText 10 Tf 50 Tz 1 0 0 1 5 7 Tm <D55C00200041> Tj ET Q\n");
        assert_eq!(outlined_text_layer("", 10.0, 0.0, 0.0, 1.0), "");
        assert_eq!(outlined_text_layer("한", f64::NAN, 0.0, 0.0, 1.0), "");
        // A character beyond the BMP is a surrogate pair, which UTF16-H addresses.
        assert!(outlined_text_layer("𠀋", 10.0, 0.0, 0.0, 10.0).contains("<D840DC0B>"));
        let font = outline_text_font();
        assert_eq!(font.name(b"Encoding"), Some(&b"UniKS-UTF16-H"[..]));
        // Stored, the descriptor becomes an indirect object.
        let mut doc = Document::new_empty();
        let mut fonts = Dict::new();
        fonts.set(OUTLINE_TEXT_FONT.as_bytes().to_vec(), Object::Dict(font));
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        hoist_font_descriptors(&mut doc, &mut res);
        let Some(Object::Dict(fonts)) = res.get(b"Font") else { panic!() };
        let Some(Object::Dict(font)) = fonts.get(OUTLINE_TEXT_FONT.as_bytes()) else { panic!() };
        let Some(Object::Array(d)) = font.get(b"DescendantFonts") else { panic!() };
        let Some(Object::Dict(cid)) = d.first() else { panic!() };
        assert!(matches!(cid.get(b"FontDescriptor"), Some(Object::Ref(_))));
    }

    #[test]
    fn only_text_the_standard_fonts_cannot_draw_needs_outlines() {
        assert!(!needs_outlines("Café — 5€\r\nok"));
        assert!(needs_outlines("안녕 Ada"));
        assert!(needs_outlines("签名"));
        assert_eq!(first_unoutlinable("Café\nok"), None);
    }

    #[test]
    fn korean_is_outlined_with_the_korean_face() {
        if crate::document_korean_fonts_for_style(false, false).is_empty() {
            eprintln!("skipping: built without a craft-fonts Kore face (set CRAFT_FONTS_DIR)");
            assert_eq!(first_unoutlinable("안녕 Ada"), Some('안'));
            assert_eq!(outlined_path("안녕", 12.0, 0.0, 0.0), Err(GlyphError::NoFont));
            assert!(outlined_width("안녕", 12.0).is_none());
            return;
        }
        assert_eq!(first_unoutlinable("안녕하세요, Ada! 2026년"), None);
        // Arabic is in neither the standard fonts nor the Korean face.
        assert_eq!(first_unoutlinable("안녕 \u{0627}"), Some('\u{0627}'));
        // Latin lines stay in Helvetica, so only a paragraph with Korean needs its Latin in the face.
        assert_eq!(first_unoutlinable("Ada\t€\n안녕\tAda"), None);
        assert!(outlined_path("안녕\tAda", 10.0, 0.0, 0.0).is_ok(), "a tab is drawn as a space");
        let w = outlined_width("안녕", 10.0).unwrap();
        assert!(w > 15.0 && w < 25.0, "two syllables are about two em: {w}");
        let path = outlined_path("안녕", 10.0, 100.0, 200.0).unwrap();
        assert!(path.contains(" m\n") && path.contains(" l\n") && path.ends_with("h\n"), "{path}");
        // Every point lies near the baseline origin, within the advance.
        for line in path.lines().filter(|l| l.ends_with(" m") || l.ends_with(" l")) {
            let mut it = line.split(' ');
            let (x, y): (f64, f64) = (it.next().unwrap().parse().unwrap(), it.next().unwrap().parse().unwrap());
            assert!((99.0..=100.0 + w + 1.0).contains(&x) && (197.0..=210.0).contains(&y), "{line}");
        }
        assert_eq!(outlined_path("안녕", f64::NAN, 0.0, 0.0), Err(GlyphError::TooComplex));
        assert_eq!(outlined_path("", 12.0, 0.0, 0.0), Ok(String::new()));
    }
}
