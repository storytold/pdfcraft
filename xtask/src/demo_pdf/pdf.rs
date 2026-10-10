//! Small helpers for writing PDF objects and content streams with `lopdf`.

use std::fmt::Write as _;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat, dictionary};

/// An sRGB colour with components in `0.0..=1.0`.
#[derive(Clone, Copy, Debug)]
pub struct Rgb(pub f32, pub f32, pub f32);

impl Rgb {
    pub const fn hex(v: u32) -> Self {
        Rgb(((v >> 16) & 0xff) as f32 / 255.0, ((v >> 8) & 0xff) as f32 / 255.0, (v & 0xff) as f32 / 255.0)
    }

    pub fn array(self) -> Object {
        Object::Array(vec![self.0.into(), self.1.into(), self.2.into()])
    }
}

/// The showcase palette (mirrors the CSS custom properties in showcase.html).
pub mod colors {
    use super::Rgb;
    pub const INK: Rgb = Rgb::hex(0x16213b);
    pub const INK_2: Rgb = Rgb::hex(0x4a5268);
    pub const INK_3: Rgb = Rgb::hex(0x7b8196);
    pub const PAPER: Rgb = Rgb::hex(0xfbf8f2);
    pub const RULE: Rgb = Rgb::hex(0xe2dccf);
    pub const FIELD: Rgb = Rgb::hex(0xfffdf8);
    pub const ACCENT: Rgb = Rgb::hex(0xe4572e);
    pub const BLUE: Rgb = Rgb::hex(0x2a6fdb);
    pub const TEAL: Rgb = Rgb::hex(0x1a9e8f);
    pub const GOLD: Rgb = Rgb::hex(0xe9a100);
    pub const HIGHLIGHT: Rgb = Rgb::hex(0xffe066);
    pub const WHITE: Rgb = Rgb(1.0, 1.0, 1.0);
}

/// A PDF text string: plain literal for ASCII, UTF-16BE with BOM otherwise.
pub fn text(s: &str) -> Object {
    if s.is_ascii() {
        Object::string_literal(s)
    } else {
        let mut bytes = vec![0xfe, 0xff];
        for unit in s.encode_utf16() {
            bytes.extend(unit.to_be_bytes());
        }
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

pub fn name(s: &str) -> Object {
    Object::Name(s.as_bytes().to_vec())
}

pub fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Object {
    Object::Array(vec![x0.into(), y0.into(), x1.into(), y1.into()])
}

pub fn floats(values: &[f32]) -> Object {
    Object::Array(values.iter().map(|&v| v.into()).collect())
}

/// A current time stamp in both PDF (`D:...Z`) and ISO 8601 forms.
#[derive(Clone, Debug)]
pub struct Stamp {
    pub unix: i64,
    pub pdf: String,
    pub iso: String,
}

impl Stamp {
    pub fn now() -> Self {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        Self::from_unix(secs)
    }

    pub fn from_unix(secs: i64) -> Self {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (h, mi, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
        // Civil-from-days (Howard Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        Stamp { unix: secs, pdf: format!("D:{y:04}{m:02}{d:02}{h:02}{mi:02}{s:02}Z"), iso: format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z") }
    }

    /// The same moment shifted by `minutes` (used to order threaded replies).
    pub fn plus_minutes(&self, minutes: i64) -> Self {
        Self::from_unix(self.unix + minutes * 60)
    }
}

/// The standard 14 fonts used by the xtask-written parts of the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    Helv,
    HelvBold,
    Times,
    TimesItalic,
    Courier,
    ZapfDingbats,
}

impl Font {
    pub const ALL: [Font; 6] = [Font::Helv, Font::HelvBold, Font::Times, Font::TimesItalic, Font::Courier, Font::ZapfDingbats];

    /// Resource name, following the conventional AcroForm names.
    pub fn res(self) -> &'static str {
        match self {
            Font::Helv => "Helv",
            Font::HelvBold => "HeBo",
            Font::Times => "TiRo",
            Font::TimesItalic => "TiIt",
            Font::Courier => "Cour",
            Font::ZapfDingbats => "ZaDb",
        }
    }

    fn base_font(self) -> &'static str {
        match self {
            Font::Helv => "Helvetica",
            Font::HelvBold => "Helvetica-Bold",
            Font::Times => "Times-Roman",
            Font::TimesItalic => "Times-Italic",
            Font::Courier => "Courier",
            Font::ZapfDingbats => "ZapfDingbats",
        }
    }

    /// Advance width of `s` at `size` points (WinAnsi text; Helvetica metrics are
    /// exact, the others are close enough for layout of short labels).
    pub fn width(self, s: &str, size: f32) -> f32 {
        let units: u32 = s
            .chars()
            .map(|c| match self {
                Font::Helv => helvetica_width(c, false),
                Font::HelvBold => helvetica_width(c, true),
                Font::Courier => 600,
                Font::Times | Font::TimesItalic => helvetica_width(c, false) * 9 / 10,
                Font::ZapfDingbats => 800,
            })
            .sum();
        units as f32 * size / 1000.0
    }
}

/// Indirect font objects shared by every page, appearance stream and the AcroForm /DR.
#[derive(Clone, Debug)]
pub struct Fonts {
    ids: Vec<(Font, ObjectId)>,
}

impl Fonts {
    pub fn add_to(doc: &mut Document) -> Self {
        let ids = Font::ALL
            .iter()
            .map(|&font| {
                let mut dict = dictionary! {
                    "Type" => "Font",
                    "Subtype" => "Type1",
                    "BaseFont" => font.base_font(),
                };
                if font != Font::ZapfDingbats {
                    dict.set("Encoding", "WinAnsiEncoding");
                }
                (font, doc.add_object(dict))
            })
            .collect();
        Fonts { ids }
    }

    pub fn id(&self, font: Font) -> ObjectId {
        self.ids.iter().find(|(f, _)| *f == font).map(|(_, id)| *id).expect("font registered")
    }

    /// A `/Font` resource dictionary naming every standard font.
    pub fn dict(&self) -> Dictionary {
        self.ids.iter().map(|(f, id)| (f.res(), Object::Reference(*id))).collect()
    }

    /// Full `/Resources` dictionary with these fonts (plus optional ExtGStates).
    pub fn resources(&self) -> Dictionary {
        dictionary! { "Font" => self.dict() }
    }
}

/// Add a Form XObject with bounding box `[0 0 w h]`.
pub fn form_xobject(doc: &mut Document, w: f32, h: f32, content: Content, resources: Dictionary) -> ObjectId {
    let dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "FormType" => 1,
        "BBox" => rect(0.0, 0.0, w, h),
        "Resources" => resources,
    };
    doc.add_object(Stream::new(dict, content.into_bytes()))
}

/// A tiny content-stream builder. Coordinates are in PDF user space.
#[derive(Default, Debug)]
pub struct Content(String);

fn num(v: f32) -> String {
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

impl Content {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0.into_bytes()
    }

    /// Append raw operators.
    pub fn raw(&mut self, ops: &str) -> &mut Self {
        self.0.push_str(ops);
        self.0.push('\n');
        self
    }

    fn ops(&mut self, operands: &[f32], op: &str) -> &mut Self {
        for v in operands {
            self.0.push_str(&num(*v));
            self.0.push(' ');
        }
        self.raw(op)
    }

    pub fn save(&mut self) -> &mut Self {
        self.raw("q")
    }
    pub fn restore(&mut self) -> &mut Self {
        self.raw("Q")
    }
    pub fn fill_color(&mut self, c: Rgb) -> &mut Self {
        self.ops(&[c.0, c.1, c.2], "rg")
    }
    pub fn stroke_color(&mut self, c: Rgb) -> &mut Self {
        self.ops(&[c.0, c.1, c.2], "RG")
    }
    pub fn line_width(&mut self, w: f32) -> &mut Self {
        self.ops(&[w], "w")
    }
    pub fn dash(&mut self, on: f32, off: f32) -> &mut Self {
        let _ = writeln!(self.0, "[{} {}] 0 d", num(on), num(off));
        self
    }
    pub fn round_caps(&mut self) -> &mut Self {
        self.raw("1 J 1 j")
    }
    pub fn cm(&mut self, a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) -> &mut Self {
        self.ops(&[a, b, c, d, e, f], "cm")
    }
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) -> &mut Self {
        self.ops(&[x, y, w, h], "re")
    }
    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.ops(&[x, y], "m")
    }
    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.ops(&[x, y], "l")
    }
    pub fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32) -> &mut Self {
        self.ops(&[x1, y1, x2, y2, x3, y3], "c")
    }
    pub fn close(&mut self) -> &mut Self {
        self.raw("h")
    }
    pub fn fill(&mut self) -> &mut Self {
        self.raw("f")
    }
    pub fn stroke(&mut self) -> &mut Self {
        self.raw("S")
    }
    pub fn fill_stroke(&mut self) -> &mut Self {
        self.raw("B")
    }
    pub fn clip(&mut self) -> &mut Self {
        self.raw("W n")
    }
    pub fn gs(&mut self, name: &str) -> &mut Self {
        self.raw(&format!("/{name} gs"))
    }

    pub fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) -> &mut Self {
        let r = r.min(w / 2.0).min(h / 2.0);
        let k = 0.552_284_8 * r;
        self.move_to(x + r, y)
            .line_to(x + w - r, y)
            .curve_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r)
            .line_to(x + w, y + h - r)
            .curve_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h)
            .line_to(x + r, y + h)
            .curve_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r)
            .line_to(x, y + r)
            .curve_to(x, y + r - k, x + r - k, y, x + r, y)
            .close()
    }

    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32) -> &mut Self {
        let (kx, ky) = (0.552_284_8 * rx, 0.552_284_8 * ry);
        self.move_to(cx + rx, cy)
            .curve_to(cx + rx, cy + ky, cx + kx, cy + ry, cx, cy + ry)
            .curve_to(cx - kx, cy + ry, cx - rx, cy + ky, cx - rx, cy)
            .curve_to(cx - rx, cy - ky, cx - kx, cy - ry, cx, cy - ry)
            .curve_to(cx + kx, cy - ry, cx + rx, cy - ky, cx + rx, cy)
            .close()
    }

    /// One line of text with its baseline starting at (x, y).
    pub fn text(&mut self, font: Font, size: f32, x: f32, y: f32, s: &str) -> &mut Self {
        let _ = writeln!(self.0, "BT /{} {} Tf {} {} Td {} Tj ET", font.res(), num(size), num(x), num(y), pdf_string(s));
        self
    }

    /// Text with letter-spacing (`Tc`), for tracked capitals.
    pub fn tracked_text(&mut self, font: Font, size: f32, x: f32, y: f32, tracking: f32, s: &str) -> &mut Self {
        let _ = writeln!(self.0, "BT /{} {} Tf {} Tc {} {} Td {} Tj 0 Tc ET", font.res(), num(size), num(tracking), num(x), num(y), pdf_string(s));
        self
    }
}

/// Encode `s` as a WinAnsi literal string `( ... )` for a content stream.
pub fn pdf_string(s: &str) -> String {
    let mut out = String::from("(");
    for byte in winansi(s) {
        match byte {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(byte as char);
            }
            0x20..=0x7e => out.push(byte as char),
            _ => {
                let _ = write!(out, "\\{byte:03o}");
            }
        }
    }
    out.push(')');
    out
}

/// Map text to WinAnsiEncoding bytes (unmappable characters become `?`).
///
/// Mirrors `pdfcraft_fonts::win_ansi` (ISO 32000-2 Annex D), which xtask doesn't depend on;
/// keep the 0x80–0x9F rows in step with `crates/fonts/src/encodings.rs`.
pub fn winansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            '\u{20}'..='\u{7e}' => c as u8,
            '\u{a0}'..='\u{ff}' => c as u32 as u8,
            '\u{20ac}' => 0x80,
            '\u{201a}' => 0x82,
            '\u{0192}' => 0x83,
            '\u{201e}' => 0x84,
            '\u{2026}' => 0x85,
            '\u{2020}' => 0x86,
            '\u{2021}' => 0x87,
            '\u{02c6}' => 0x88,
            '\u{2030}' => 0x89,
            '\u{0160}' => 0x8a,
            '\u{2039}' => 0x8b,
            '\u{0152}' => 0x8c,
            '\u{017d}' => 0x8e,
            '\u{2018}' => 0x91,
            '\u{2019}' => 0x92,
            '\u{201c}' => 0x93,
            '\u{201d}' => 0x94,
            '\u{2022}' => 0x95,
            '\u{2013}' => 0x96,
            '\u{2014}' => 0x97,
            '\u{02dc}' => 0x98,
            '\u{2122}' => 0x99,
            '\u{0161}' => 0x9a,
            '\u{203a}' => 0x9b,
            '\u{0153}' => 0x9c,
            '\u{017e}' => 0x9e,
            '\u{0178}' => 0x9f,
            _ => b'?',
        })
        .collect()
}

/// Helvetica / Helvetica-Bold advance widths (1/1000 em) from the public AFM metrics.
fn helvetica_width(c: char, bold: bool) -> u32 {
    const REGULAR: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // ' '..'/'
        556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, // '0'..'?'
        1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, // '@'..'O'
        667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, // 'P'..'_'
        333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, // '`'..'o'
        556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, // 'p'..'~'
    ];
    const BOLD: [u16; 95] = [
        278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, //
        556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, //
        975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, //
        667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556, //
        333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, //
        611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584, //
    ];
    let table = if bold { &BOLD } else { &REGULAR };
    match c {
        ' '..='~' => u32::from(table[c as usize - 0x20]),
        '\u{2014}' => 1000,
        '\u{2013}' => 556,
        '\u{2022}' => 350,
        _ => 556,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_round_trip() {
        let s = Stamp::from_unix(1_790_000_000);
        assert_eq!(s.iso, "2026-09-21T14:13:20Z");
        assert_eq!(s.plus_minutes(0).pdf, s.pdf);
        assert_eq!(s.plus_minutes(60 * 24).iso, "2026-09-22T14:13:20Z");
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(pdf_string("a(b)\\"), "(a\\(b\\)\\\\)");
        assert_eq!(pdf_string("—"), "(\\227)");
    }

    #[test]
    fn winansi_covers_the_0x80_to_0x9f_glyphs() {
        // The 27 defined codes of ISO 32000-2 Annex D WinAnsiEncoding between 0x80 and 0x9F.
        let high = "€‚ƒ„…†‡ˆ‰Š‹ŒŽ‘’“”•–—˜™š›œžŸ";
        let want: Vec<u8> = (0x80..=0x9f).filter(|b| ![0x81, 0x8d, 0x8f, 0x90, 0x9d].contains(b)).collect();
        assert_eq!(winansi(high), want);
        assert_eq!(winansi("č"), b"?");
    }
}
