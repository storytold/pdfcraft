//! The face, size and colour a text box's text is drawn in when PdfCraft draws its appearance:
//! `/DA` (ISO 32000-2 Table 177; the font named there is looked up in the interactive form's
//! `/DR` font resources, §12.7.4.3) and `/DS`, the default style string (Table 177), which is
//! read for its CSS `font`, `font-family`, `font-size`, `font-weight`, `font-style` and `color`.
//!
//! Only the twelve standard text faces are drawn (Helvetica, Times and Courier, each regular,
//! bold, italic and bold italic, in WinAnsiEncoding): a font that isn't one of them is drawn in
//! Helvetica, as before. Symbol and ZapfDingbats are not text faces for this purpose.

use pdfcraft_cos::{Dict, Document, Object};
use pdfcraft_fonts::{Std14, Std14Match as Match};

use crate::Rgb;
use crate::appearance::parse_da;

/// `/DS` past this many bytes is read only this far (a style string is a line or two).
const MAX_DS: usize = 4096;

/// Font sizes a text box is drawn at, as for `/DA`.
const MAX_SIZE: f64 = 400.0;

/// How a text box's text is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub face: Std14,
    pub size: f64,
    pub color: Rgb,
}

/// The style of `d`'s text: `/DA`, with what `/DS` sets taking its place. `dr_face` is the face
/// the `/DA` font resource names in the form's `/DR` ([`dr_face`]); `None` when it isn't there,
/// and the common resource names (`/Helv`, `/Cour`, `/TiRo`, …) are read instead.
pub fn text_style(d: &Dict, dr_face: Option<Std14>) -> TextStyle {
    let (da_color, da_size) = parse_da(d);
    let da_face = dr_face.or_else(|| da_font_name(d).as_deref().and_then(resource_face)).unwrap_or(Std14::Helvetica);
    let mut style = TextStyle { face: da_face, size: da_size, color: da_color };
    let Some(ds) = d.get(b"DS").and_then(Object::as_string).map(|s| s.to_text()) else { return style };
    let ds = parse_ds(&ds);
    if let Some(size) = ds.size {
        style.size = size;
    }
    // `/DS` colours are 8-bit (`#RRGGBB`): one that rounds to the same as `/DA`'s keeps `/DA`'s.
    if let Some(color) = ds.color
        && bytes(color) != bytes(da_color)
    {
        style.color = color;
    }
    let base = ds.face.unwrap_or(style.face);
    style.face = styled(base, ds.bold.unwrap_or(is_bold(base)), ds.italic.unwrap_or(is_italic(base)));
    style
}

/// The face the `/DA` font of `d` names in the document's `/AcroForm /DR /Font` (§12.7.4.3):
/// `Some` when that resource is there — the standard text face its `/BaseFont` names, or
/// Helvetica for any other font — and `None` when it isn't.
pub fn dr_face(doc: &Document, d: &Dict) -> Option<Std14> {
    let name = da_font_name(d)?;
    let root = doc.trailer().get(b"Root").map(|r| doc.resolve(r))?;
    let form = root.as_dict()?.get(b"AcroForm").map(|f| doc.resolve(f))?;
    let dr = form.as_dict()?.get(b"DR").map(|r| doc.resolve(r))?;
    let fonts = dr.as_dict()?.get(b"Font").map(|f| doc.resolve(f))?;
    let font = fonts.as_dict()?.get(name.as_bytes()).map(|f| doc.resolve(f))?;
    let font = font.as_dict()?;
    let base = font.name(b"BaseFont").map(String::from_utf8_lossy);
    Some(base.as_deref().and_then(base_font_face).unwrap_or(Std14::Helvetica))
}

/// The font resource name of the last `Tf` in `/DA` (without the slash).
pub fn da_font_name(d: &Dict) -> Option<String> {
    let da = d.get(b"DA").and_then(Object::as_string)?;
    let da = String::from_utf8_lossy(&da.bytes);
    let toks: Vec<&str> = da.split_whitespace().collect();
    let tf = toks.iter().rposition(|t| *t == "Tf")?;
    let name = toks.get(tf.checked_sub(2)?)?.strip_prefix('/')?;
    (!name.is_empty()).then(|| name.to_string())
}

/// The face a font resource name stands for: the names PDF writers commonly give the standard
/// fonts in `/DR` (`Helv`, `HeBo`, `Cour`, `TiRo`, …), or a standard face's own name.
pub fn resource_face(name: &str) -> Option<Std14> {
    Some(match name {
        "Helv" => Std14::Helvetica,
        "HeBo" => Std14::HelveticaBold,
        "HeOb" | "HeIt" => Std14::HelveticaOblique,
        "HeBO" | "HeBI" => Std14::HelveticaBoldOblique,
        "Cour" => Std14::Courier,
        "CoBo" => Std14::CourierBold,
        "CoOb" | "CoIt" => Std14::CourierOblique,
        "CoBO" | "CoBI" => Std14::CourierBoldOblique,
        "TiRo" => Std14::TimesRoman,
        "TiBo" => Std14::TimesBold,
        "TiIt" => Std14::TimesItalic,
        "TiBI" => Std14::TimesBoldItalic,
        _ => return base_font_face(name),
    })
}

/// The standard text face a `/BaseFont` (or family) name asks for: the spec's names and their
/// usual aliases (Arial, Times New Roman, Courier New), not a guess from a family hint.
fn base_font_face(name: &str) -> Option<Std14> {
    match Std14::recognise(name)? {
        (Std14::Symbol | Std14::ZapfDingbats, _) | (_, Match::FamilyHint) => None,
        (face, Match::Alias) => Some(face),
    }
}

/// `CSS` family names: the generic ones, else a standard face's name or alias.
fn family_face(name: &str) -> Option<Std14> {
    let name = name.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    match name.to_ascii_lowercase().as_str() {
        "serif" => Some(Std14::TimesRoman),
        "monospace" => Some(Std14::Courier),
        "sans-serif" => Some(Std14::Helvetica),
        _ => base_font_face(name),
    }
}

fn is_bold(f: Std14) -> bool {
    matches!(
        f,
        Std14::HelveticaBold
            | Std14::HelveticaBoldOblique
            | Std14::TimesBold
            | Std14::TimesBoldItalic
            | Std14::CourierBold
            | Std14::CourierBoldOblique
    )
}

fn is_italic(f: Std14) -> bool {
    matches!(
        f,
        Std14::HelveticaOblique
            | Std14::HelveticaBoldOblique
            | Std14::TimesItalic
            | Std14::TimesBoldItalic
            | Std14::CourierOblique
            | Std14::CourierBoldOblique
    )
}

/// `base`'s family in the given weight and slant.
fn styled(base: Std14, bold: bool, italic: bool) -> Std14 {
    use Std14::*;
    match (base, bold, italic) {
        (TimesRoman | TimesBold | TimesItalic | TimesBoldItalic, false, false) => TimesRoman,
        (TimesRoman | TimesBold | TimesItalic | TimesBoldItalic, true, false) => TimesBold,
        (TimesRoman | TimesBold | TimesItalic | TimesBoldItalic, false, true) => TimesItalic,
        (TimesRoman | TimesBold | TimesItalic | TimesBoldItalic, true, true) => TimesBoldItalic,
        (Courier | CourierBold | CourierOblique | CourierBoldOblique, false, false) => Courier,
        (Courier | CourierBold | CourierOblique | CourierBoldOblique, true, false) => CourierBold,
        (Courier | CourierBold | CourierOblique | CourierBoldOblique, false, true) => CourierOblique,
        (Courier | CourierBold | CourierOblique | CourierBoldOblique, true, true) => CourierBoldOblique,
        (_, false, false) => Helvetica,
        (_, true, false) => HelveticaBold,
        (_, false, true) => HelveticaOblique,
        (_, true, true) => HelveticaBoldOblique,
    }
}

/// The resource name an appearance stream gives `face` (the usual `/DR` names).
pub fn resource_name(face: Std14) -> &'static str {
    match face {
        Std14::HelveticaBold => "HeBo",
        Std14::HelveticaOblique => "HeOb",
        Std14::HelveticaBoldOblique => "HeBO",
        Std14::TimesRoman => "TiRo",
        Std14::TimesBold => "TiBo",
        Std14::TimesItalic => "TiIt",
        Std14::TimesBoldItalic => "TiBI",
        Std14::Courier => "Cour",
        Std14::CourierBold => "CoBo",
        Std14::CourierOblique => "CoOb",
        Std14::CourierBoldOblique => "CoBO",
        _ => "Helv",
    }
}

/// `face`'s `CSS` family, weight and style, for writing `/DS` and `/RC`.
pub fn css(face: Std14) -> (&'static str, &'static str, &'static str) {
    let family = match styled(face, false, false) {
        Std14::TimesRoman => "Times",
        Std14::Courier => "Courier",
        _ => "Helvetica",
    };
    (family, if is_bold(face) { "bold" } else { "normal" }, if is_italic(face) { "italic" } else { "normal" })
}

fn bytes(c: Rgb) -> [u8; 3] {
    // Clamped to 0..=1 first, so the product is 0..=255 and the cast exact.
    c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// What a `/DS` string sets.
#[derive(Debug, Default, PartialEq)]
struct Ds {
    face: Option<Std14>,
    bold: Option<bool>,
    italic: Option<bool>,
    size: Option<f64>,
    color: Option<Rgb>,
}

/// Read a default style string: `;`-separated `CSS` declarations. Unknown or malformed ones are
/// skipped; later ones win.
fn parse_ds(ds: &str) -> Ds {
    let end = ds.char_indices().map(|(i, _)| i).find(|i| *i >= MAX_DS).unwrap_or(ds.len());
    let ds = ds.get(..end).unwrap_or_default();
    let mut out = Ds::default();
    for decl in ds.split(';') {
        let Some((prop, value)) = decl.split_once(':') else { continue };
        let value = value.trim();
        match prop.trim().to_ascii_lowercase().as_str() {
            "font" => font_shorthand(value, &mut out),
            "font-family" => {
                if let Some(face) = value.split(',').find_map(family_face) {
                    out.face = Some(face);
                }
            }
            "font-size" => {
                if let Some(size) = font_size(value, true) {
                    out.size = Some(size);
                }
            }
            "font-weight" => {
                if let Some(bold) = weight(value) {
                    out.bold = Some(bold);
                }
            }
            "font-style" => {
                if let Some(italic) = slant(value) {
                    out.italic = Some(italic);
                }
            }
            "color" => {
                if let Some(c) = css_color(value) {
                    out.color = Some(c);
                }
            }
            _ => {}
        }
    }
    out
}

/// `font: [style] [weight] size[/line-height] family[, family…]`, read leniently: the size is
/// the token with a unit, keywords set weight and slant, and the rest is the family list (some
/// writers put the family first).
fn font_shorthand(value: &str, out: &mut Ds) {
    let mut family = Vec::new();
    for tok in value.split_whitespace() {
        if let Some(size) = font_size(tok, false) {
            out.size = Some(size);
        } else if let Some(bold) = weight(tok).filter(|_| !tok.eq_ignore_ascii_case("normal")) {
            out.bold = Some(bold);
        } else if let Some(italic) = slant(tok).filter(|_| !tok.eq_ignore_ascii_case("normal")) {
            out.italic = Some(italic);
        } else if !matches!(tok.to_ascii_lowercase().as_str(), "normal" | "small-caps") {
            family.push(tok);
        }
    }
    if let Some(face) = family.join(" ").split(',').find_map(family_face) {
        out.face = Some(face);
    }
}

/// A size in points: `14pt` (with a line height, `14pt/16pt`), or a bare number where
/// `unitless` allows it. Positive and finite only; capped as `/DA` sizes are.
fn font_size(tok: &str, unitless: bool) -> Option<f64> {
    let tok = tok.split('/').next()?.trim();
    let num = match tok.strip_suffix("pt").or_else(|| tok.strip_suffix("PT")) {
        Some(n) => n,
        None if unitless => tok,
        None => return None,
    };
    num.parse::<f64>().ok().filter(|s| s.is_finite() && *s > 0.0).map(|s| s.min(MAX_SIZE))
}

fn weight(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "bold" | "bolder" | "600" | "700" | "800" | "900" => Some(true),
        "normal" | "lighter" | "100" | "200" | "300" | "400" | "500" => Some(false),
        _ => None,
    }
}

fn slant(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "italic" | "oblique" => Some(true),
        "normal" => Some(false),
        _ => None,
    }
}

/// `#RRGGBB`, `#RGB` or `rgb(r, g, b)` (0–255 or percentages).
fn css_color(v: &str) -> Option<Rgb> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix('#') {
        let digit = |c: char| c.to_digit(16).map(f64::from);
        let ds: Vec<f64> = hex.chars().map(digit).collect::<Option<_>>()?;
        return match ds.as_slice() {
            [r, g, b] => Some([r * 17.0 / 255.0, g * 17.0 / 255.0, b * 17.0 / 255.0]),
            [r1, r0, g1, g0, b1, b0] => Some([(r1 * 16.0 + r0) / 255.0, (g1 * 16.0 + g0) / 255.0, (b1 * 16.0 + b0) / 255.0]),
            _ => None,
        };
    }
    let lower = v.to_ascii_lowercase();
    let inner = lower.strip_prefix("rgb(")?.strip_suffix(')')?;
    let parts: Vec<f64> = inner
        .split(',')
        .map(|p| {
            let p = p.trim();
            let x = match p.strip_suffix('%') {
                Some(pc) => pc.trim().parse::<f64>().ok()? / 100.0,
                None => p.parse::<f64>().ok()? / 255.0,
            };
            x.is_finite().then(|| x.clamp(0.0, 1.0))
        })
        .collect::<Option<_>>()?;
    match parts.as_slice() {
        [r, g, b] => Some([*r, *g, *b]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use pdfcraft_cos::PdfString;

    use super::*;

    fn dict(da: &str, ds: Option<&str>) -> Dict {
        let mut d = Dict::new();
        d.set(b"DA".to_vec(), PdfString::literal(da.as_bytes().to_vec()));
        if let Some(ds) = ds {
            d.set(b"DS".to_vec(), PdfString::text(ds));
        }
        d
    }

    #[test]
    fn da_resource_names_pick_the_standard_faces() {
        assert_eq!(text_style(&dict("0 g /Cour 12 Tf", None), None).face, Std14::Courier);
        assert_eq!(text_style(&dict("1 0 0 rg /TiRo 16 Tf", None), None).face, Std14::TimesRoman);
        assert_eq!(text_style(&dict("/HeBo 9 Tf 0 g", None), None).face, Std14::HelveticaBold);
        assert_eq!(text_style(&dict("/Courier-Bold 9 Tf", None), None).face, Std14::CourierBold);
        // Unknown, symbolic and family-hint-only names keep Helvetica.
        for da in ["/F1 12 Tf", "/ZaDb 12 Tf", "/Symbol 12 Tf", "/MySerif 12 Tf", "12 Tf", "", "/ Tf"] {
            assert_eq!(text_style(&dict(da, None), None).face, Std14::Helvetica, "{da:?}");
        }
        // A `/DR` lookup outranks the name.
        assert_eq!(text_style(&dict("/Cour 12 Tf", None), Some(Std14::TimesRoman)).face, Std14::TimesRoman);
    }

    #[test]
    fn ds_font_size_and_colour_win_over_da() {
        let s = text_style(&dict("0 g /Helv 12 Tf", Some("font: Times 14pt; color: #008000")), None);
        assert_eq!((s.face, s.size), (Std14::TimesRoman, 14.0));
        assert!((s.color[1] - 128.0 / 255.0).abs() < 1e-9 && s.color[0] == 0.0 && s.color[2] == 0.0, "{:?}", s.color);
        let s = text_style(
            &dict(
                "1 0 0 rg /Cour 9 Tf",
                Some("font-family: 'Helvetica', sans-serif; font-size: 20; font-weight: bold; font-style: italic; color: rgb(0, 0, 255)"),
            ),
            None,
        );
        assert_eq!((s.face, s.size, s.color), (Std14::HelveticaBoldOblique, 20.0, [0.0, 0.0, 1.0]));
        // CSS order, a line height, a #RGB colour and a generic family.
        let s = text_style(&dict("0 g /Helv 12 Tf", Some("font: italic bold 10pt/12pt serif; color:#f00")), None);
        assert_eq!((s.face, s.size, s.color), (Std14::TimesBoldItalic, 10.0, [1.0, 0.0, 0.0]));
        // Bold from the style keeps the DA family.
        assert_eq!(text_style(&dict("/Cour 12 Tf", Some("font-weight:700")), None).face, Std14::CourierBold);
    }

    /// PdfCraft writes `/DS` from `/DA` (8-bit colour): `/DA`'s exact colour, size and face stay.
    #[test]
    fn pdfcraft_written_ds_matches_da() {
        let s = text_style(&dict("0.1 0.2 0.3 rg /Helv 12.5 Tf", Some("font: Helvetica 12.5pt; text-align:left; color:#1A334D")), None);
        assert_eq!(s, TextStyle { face: Std14::Helvetica, size: 12.5, color: [0.1, 0.2, 0.3] });
    }

    #[test]
    fn malformed_ds_falls_back_to_da() {
        let da = TextStyle { face: Std14::Courier, size: 9.0, color: [1.0, 0.0, 0.0] };
        let huge = format!("color: #00ff00;{}", "x".repeat(1 << 20));
        let past_cap = format!("{}; font-size: 30pt", " ".repeat(MAX_DS));
        for ds in [
            "font-size: NaN",
            "font-size: inf",
            "font-size: -3pt",
            "font-size: 0",
            "font: NaNpt Zzz",
            "font: 1e999pt",
            "color: #12",
            "color: #zzzzzz",
            "color: rgb(1, 2)",
            "color: rgb(NaN, 0, 0)",
            "color: #ff0000é",
            ":::;;;",
            "font-family: Wingdings, 'Comic Sans'",
            "é: ü; ;",
            past_cap.as_str(),
        ] {
            assert_eq!(text_style(&dict("1 0 0 rg /Cour 9 Tf", Some(ds)), None), da, "{ds:?}");
        }
        // Read up to the cap: what comes first still counts, and nothing past it is scanned.
        assert_eq!(text_style(&dict("1 0 0 rg /Cour 9 Tf", Some(&huge)), None).color, [0.0, 1.0, 0.0]);
        // Sizes are capped as /DA's are.
        assert_eq!(text_style(&dict("/Helv 9 Tf", Some("font-size: 1e300pt")), None).size, MAX_SIZE);
    }

    #[test]
    fn css_round_trips_through_the_parser() {
        for face in [Std14::Helvetica, Std14::TimesBold, Std14::CourierOblique, Std14::HelveticaBoldOblique, Std14::TimesBoldItalic] {
            let (family, weight, style) = css(face);
            let ds = format!("font-family:{family};font-weight:{weight};font-style:{style}");
            assert_eq!(text_style(&dict("/Helv 9 Tf", Some(&ds)), None).face, face);
        }
    }
}
