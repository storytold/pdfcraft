//! What content editing and redaction need from a PDF font: how to split a string into character
//! codes, each code's advance width, the glyph height (ascent, descent), and what each code means
//! (Unicode) — and back, which text the font can show. Widths come from the font dictionary
//! (`/Widths`, `/W`, `/DW`, `/MissingWidth`); the standard 14 fonts without widths use
//! approximations. Meanings come from `/ToUnicode`, else the encoding (`/Encoding` base and
//! `/Differences` glyph names, ISO 32000-2 Annex D).

use std::collections::HashMap;

use pdfcraft_cos::{Dict, Document, Object};

/// How the bytes of a string map to codes.
#[derive(Clone, Debug, PartialEq)]
enum Codes {
    One,
    Two,
    /// Codespace ranges `(length, low, high)` from an embedded CMap.
    Ranges(Vec<(usize, Vec<u8>, Vec<u8>)>),
}

#[derive(Clone, Debug, PartialEq)]
enum Std14 {
    Helvetica,
    Times,
    Courier,
    Symbolic,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    codes: Codes,
    /// Simple fonts: `/FirstChar` and `/Widths`.
    first: u32,
    widths: Vec<f64>,
    /// Composite fonts: CID → width, and CID ranges with one width.
    cid_widths: HashMap<u32, f64>,
    cid_ranges: Vec<(u32, u32, f64)>,
    /// Code → CID for embedded non-identity CMaps (`cidrange` / `cidchar`).
    cid_map: Vec<(u32, u32, u32)>,
    default: f64,
    std14: Option<Std14>,
    /// Glyph units → text space (0.001, or `/FontMatrix[0]` for Type 3).
    pub scale: f64,
    /// Glyph box in text space per unit of font size.
    pub ascent: f64,
    pub descent: f64,
    pub composite: bool,
    /// Code → Unicode text.
    unicode: HashMap<u32, String>,
    /// `/BaseFont` (or Type 3 descriptor's `/FontName`), and whether it is a subset (`ABCDEF+Name`: other glyphs are missing).
    pub base_font: String,
    pub subset: bool,
    /// Whether the PDF identifies this face as bold or italic. These come from the
    /// font name and, when present, `/FontDescriptor` flags/weight/angle.
    pub bold: bool,
    pub italic: bool,
    /// Bytes per code for writing new text (1 for simple fonts, else the codespace length).
    code_len: usize,
}

fn nums(doc: &Document, o: Option<&Object>) -> Vec<f64> {
    o.map(|o| doc.resolve(o))
        .and_then(|a| a.as_array().map(|a| a.iter().map(|x| doc.resolve(x).as_f64().unwrap_or(0.0)).collect()))
        .unwrap_or_default()
}

fn dict(doc: &Document, o: Option<&Object>) -> Option<Dict> {
    o.and_then(|o| doc.resolve(o).as_dict().cloned())
}

fn hex_bytes(tok: &str) -> Option<Vec<u8>> {
    let t = tok.trim_start_matches('<').trim_end_matches('>');
    if !t.len().is_multiple_of(2) || t.is_empty() {
        return None;
    }
    (0..t.len()).step_by(2).map(|i| u8::from_str_radix(&t[i..i + 2], 16).ok()).collect()
}

fn be(b: &[u8]) -> u32 {
    b.iter().fold(0u32, |a, x| (a << 8) | u32::from(*x))
}

fn style_from_name(name: &str) -> (bool, bool) {
    let name = name.to_ascii_lowercase();
    let name = name.split_once('+').map_or(name.as_str(), |(_, n)| n);
    (
        ["bold", "black", "heavy", "semibold", "demi"].iter().any(|s| name.contains(s)),
        ["italic", "oblique", "slanted"].iter().any(|s| name.contains(s)),
    )
}

/// Codespace ranges and CID mappings from an embedded CMap stream.
#[allow(clippy::type_complexity)]
fn parse_cmap(data: &[u8]) -> (Vec<(usize, Vec<u8>, Vec<u8>)>, Vec<(u32, u32, u32)>) {
    let text = String::from_utf8_lossy(data);
    let toks: Vec<&str> = text.split_whitespace().collect();
    let (mut spaces, mut cids) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < toks.len() {
        match toks[i] {
            "begincodespacerange" => {
                i += 1;
                while i + 1 < toks.len() && toks[i] != "endcodespacerange" {
                    if let (Some(lo), Some(hi)) = (hex_bytes(toks[i]), hex_bytes(toks[i + 1]))
                        && lo.len() == hi.len()
                    {
                        spaces.push((lo.len(), lo, hi));
                    }
                    i += 2;
                }
            }
            "begincidrange" => {
                i += 1;
                while i + 2 < toks.len() && toks[i] != "endcidrange" {
                    if let (Some(lo), Some(hi), Ok(c)) = (hex_bytes(toks[i]), hex_bytes(toks[i + 1]), toks[i + 2].parse::<u32>()) {
                        cids.push((be(&lo), be(&hi), c));
                    }
                    i += 3;
                }
            }
            "begincidchar" => {
                i += 1;
                while i + 1 < toks.len() && toks[i] != "endcidchar" {
                    if let (Some(code), Ok(c)) = (hex_bytes(toks[i]), toks[i + 1].parse::<u32>()) {
                        cids.push((be(&code), be(&code), c));
                    }
                    i += 2;
                }
            }
            _ => {}
        }
        i += 1;
    }
    spaces.sort_by_key(|s| s.0);
    (spaces, cids)
}

impl Metrics {
    /// Metrics for text without a usable font (Helvetica-like).
    pub fn fallback() -> Self {
        Metrics {
            codes: Codes::One,
            first: 0,
            widths: Vec::new(),
            cid_widths: HashMap::new(),
            cid_ranges: Vec::new(),
            cid_map: Vec::new(),
            default: 500.0,
            std14: Some(Std14::Helvetica),
            scale: 0.001,
            ascent: 0.9,
            descent: -0.25,
            composite: false,
            unicode: (32..127u8).map(|c| (u32::from(c), char::from(c).to_string())).collect(),
            base_font: "Helvetica".into(),
            subset: false,
            bold: false,
            italic: false,
            code_len: 1,
        }
    }

    pub fn from_dict(doc: &Document, font: &Dict) -> Self {
        let mut m = Self::read_metrics(doc, font);
        let base = font
            .name(b"BaseFont")
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .or_else(|| {
                (font.name(b"Subtype") == Some(b"Type3"))
                    .then(|| dict(doc, font.get(b"FontDescriptor")))
                    .flatten()
                    .and_then(|d| d.name(b"FontName").map(|name| String::from_utf8_lossy(name).into_owned()))
            })
            .unwrap_or_default();
        m.subset = base.len() > 7 && base.as_bytes()[6] == b'+' && base[..6].bytes().all(|b| b.is_ascii_uppercase());
        let (bold, italic) = style_from_name(&base);
        m.bold |= bold;
        m.italic |= italic;
        m.base_font = base;
        m.code_len = match &m.codes {
            Codes::One => 1,
            Codes::Two => 2,
            Codes::Ranges(r) => r.last().map_or(2, |x| x.0),
        };
        m.unicode = unicode_map(doc, font, m.composite);
        m
    }

    fn read_metrics(doc: &Document, font: &Dict) -> Self {
        let mut m = Metrics::fallback();
        m.std14 = None;
        let subtype = font.name(b"Subtype").unwrap_or(b"Type1").to_vec();
        let descriptor;
        if subtype == b"Type0" {
            m.composite = true;
            m.default = 1000.0;
            m.codes = Codes::Two;
            if let Some(e) = font.get(b"Encoding").map(|e| doc.resolve(e))
                && let Object::Stream(s) = &*e
                && let Ok(data) = s.decoded()
            {
                let (spaces, cids) = parse_cmap(&data);
                if !spaces.is_empty() {
                    m.codes = Codes::Ranges(spaces);
                }
                m.cid_map = cids;
            }
            let desc = font
                .get(b"DescendantFonts")
                .map(|d| doc.resolve(d))
                .and_then(|a| a.as_array().and_then(|a| a.first().cloned()))
                .and_then(|d| doc.resolve(&d).as_dict().cloned())
                .unwrap_or_default();
            if let Some(dw) = desc.get(b"DW").and_then(|d| doc.resolve(d).as_f64()) {
                m.default = dw;
            }
            if let Some(w) = desc.get(b"W").map(|w| doc.resolve(w)).and_then(|w| w.as_array().cloned()) {
                let mut i = 0;
                while i < w.len() {
                    let Some(c0) = doc.resolve(&w[i]).as_f64().map(|v| v as u32) else { break };
                    match w.get(i + 1).map(|o| doc.resolve(o)) {
                        Some(a) if a.as_array().is_some() => {
                            for (k, x) in a.as_array().into_iter().flatten().enumerate() {
                                if let Some(v) = doc.resolve(x).as_f64() {
                                    m.cid_widths.insert(c0 + k as u32, v);
                                }
                            }
                            i += 2;
                        }
                        Some(c1) => {
                            if let (Some(c1), Some(v)) = (c1.as_f64(), w.get(i + 2).and_then(|x| doc.resolve(x).as_f64())) {
                                m.cid_ranges.push((c0, c1 as u32, v));
                            }
                            i += 3;
                        }
                        None => break,
                    }
                }
            }
            descriptor = dict(doc, desc.get(b"FontDescriptor"));
        } else {
            m.first = font.get(b"FirstChar").and_then(|f| doc.resolve(f).as_f64()).unwrap_or(0.0).max(0.0) as u32;
            m.widths = nums(doc, font.get(b"Widths"));
            descriptor = dict(doc, font.get(b"FontDescriptor"));
            m.default = descriptor.as_ref().and_then(|d| d.get(b"MissingWidth")).and_then(|w| doc.resolve(w).as_f64()).unwrap_or(0.0);
            if subtype == b"Type3" {
                let fm = nums(doc, font.get(b"FontMatrix"));
                if fm.len() == 6 {
                    m.scale = fm[0];
                    let bbox = nums(doc, font.get(b"FontBBox"));
                    if bbox.len() == 4 && fm[3] != 0.0 {
                        m.ascent = (bbox[3] * fm[3]).max(0.5);
                        m.descent = (bbox[1] * fm[3]).min(-0.1);
                    }
                }
            } else if m.widths.is_empty() {
                let base = String::from_utf8_lossy(font.name(b"BaseFont").unwrap_or(b"")).to_ascii_lowercase();
                m.std14 = Some(if base.contains("courier") {
                    Std14::Courier
                } else if base.contains("times") {
                    Std14::Times
                } else if base.contains("symbol") || base.contains("dingbats") {
                    Std14::Symbolic
                } else {
                    Std14::Helvetica
                });
            }
        }
        if let Some(d) = descriptor {
            let flags = d.get(b"Flags").and_then(|v| doc.resolve(v).as_f64()).unwrap_or(0.0).max(0.0) as u32;
            let angle = d.get(b"ItalicAngle").and_then(|v| doc.resolve(v).as_f64()).unwrap_or(0.0);
            let weight = d.get(b"FontWeight").and_then(|v| doc.resolve(v).as_f64()).unwrap_or(0.0);
            m.italic |= flags & 64 != 0 || angle.abs() > 0.1;
            m.bold |= flags & 262_144 != 0 || weight >= 600.0;
            // Type 3 descriptors may omit these metrics; the glyph-space FontBBox still applies.
            let ascent = d.get(b"Ascent").and_then(|v| doc.resolve(v).as_f64());
            let descent = d.get(b"Descent").and_then(|v| doc.resolve(v).as_f64());
            // Fonts often claim 0; never shrink the glyph box below a sensible minimum.
            if ascent.is_some() || subtype != b"Type3" {
                let a = ascent.unwrap_or(0.0) / 1000.0;
                m.ascent = if a > 0.3 { a.min(1.5) } else { 0.9 };
            }
            if descent.is_some() || subtype != b"Type3" {
                let de = descent.unwrap_or(0.0) / 1000.0;
                m.descent = if de < -0.05 { de.max(-0.6) } else { -0.25 };
            }
        }
        m
    }

    /// Split a string into `(code, byte length)`.
    pub fn codes(&self, s: &[u8]) -> Vec<(u32, usize)> {
        let mut out = Vec::with_capacity(s.len());
        let mut i = 0;
        while i < s.len() {
            let len = match &self.codes {
                Codes::One => 1,
                Codes::Two => 2,
                Codes::Ranges(r) => r
                    .iter()
                    .find(|(len, lo, hi)| s.get(i..i + len).is_some_and(|b| b.iter().zip(lo.iter().zip(hi)).all(|(x, (l, h))| x >= l && x <= h)))
                    .map_or(r.first().map_or(1, |x| x.0), |x| x.0),
            };
            let len = len.min(s.len() - i).max(1);
            out.push((be(&s[i..i + len]), len));
            i += len;
        }
        out
    }

    fn cid(&self, code: u32) -> u32 {
        if self.cid_map.is_empty() {
            return code;
        }
        self.cid_map.iter().find(|(lo, hi, _)| code >= *lo && code <= *hi).map_or(0, |(lo, _, c)| c + (code - lo))
    }

    /// The advance of `code` in text space per unit of font size.
    pub fn width(&self, code: u32) -> f64 {
        if self.composite {
            let cid = self.cid(code);
            let w = self.cid_widths.get(&cid).copied().or_else(|| self.cid_ranges.iter().find(|(a, b, _)| cid >= *a && cid <= *b).map(|r| r.2));
            return w.unwrap_or(self.default) * self.scale;
        }
        if let Some(w) = code.checked_sub(self.first).and_then(|i| self.widths.get(i as usize)) {
            return w * self.scale;
        }
        match &self.std14 {
            Some(Std14::Courier) => 0.6,
            Some(Std14::Symbolic) => 0.75,
            Some(f) => {
                let c = char::from_u32(code).filter(|c| !c.is_control()).unwrap_or('n');
                let w = crate::helvetica_width(&c.to_string(), 1.0);
                if *f == Std14::Times { w * 0.92 } else { w }
            }
            None => self.default * self.scale,
        }
    }

    /// Word spacing applies to the single-byte code 32 (§9.3.3).
    pub fn is_space(&self, code: u32, len: usize) -> bool {
        code == 32 && len == 1
    }
}

impl Metrics {
    /// The text a string shows (codes without a known meaning are left out).
    pub fn decode(&self, s: &[u8]) -> String {
        self.codes(s).into_iter().filter_map(|(c, _)| self.unicode.get(&c).cloned()).collect()
    }

    /// The Unicode text of one code, if known.
    pub fn text_of(&self, code: u32) -> Option<&str> {
        self.unicode.get(&code).map(String::as_str)
    }

    /// Whether the font has a glyph for `code` (subset fonts lack the glyphs they don't use).
    fn has_glyph(&self, code: u32) -> bool {
        if self.composite {
            let cid = self.cid(code);
            return self.cid_widths.contains_key(&cid) || self.cid_ranges.iter().any(|(a, b, _)| cid >= *a && cid <= *b) || !self.subset;
        }
        match code.checked_sub(self.first).and_then(|i| self.widths.get(i as usize)) {
            Some(w) => *w > 0.0 || code == 32,
            None => self.widths.is_empty() && !self.subset,
        }
    }

    /// The bytes that show `text` in this font, or `None` if some character has no code or
    /// no glyph in it (the caller then substitutes another font).
    pub fn encode(&self, text: &str) -> Option<Vec<u8>> {
        let mut reverse: HashMap<&str, u32> = HashMap::new();
        for (code, t) in &self.unicode {
            if self.has_glyph(*code) {
                reverse.entry(t.as_str()).and_modify(|c| *c = (*c).min(*code)).or_insert(*code);
            }
        }
        let mut out = Vec::with_capacity(text.len() * self.code_len);
        let mut buf = [0u8; 4];
        for ch in text.chars() {
            let code = *reverse.get(ch.encode_utf8(&mut buf) as &str)?;
            let bytes = code.to_be_bytes();
            out.extend_from_slice(&bytes[4 - self.code_len.clamp(1, 4)..]);
        }
        Some(out)
    }
}

/// UTF-16BE (with surrogates) from CMap hex bytes.
fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
    String::from_utf16_lossy(&units)
}

/// `bfchar`/`bfrange` entries of a ToUnicode CMap.
fn parse_to_unicode(data: &[u8], out: &mut HashMap<u32, String>) {
    let text = String::from_utf8_lossy(data);
    // Tokenise, keeping arrays' brackets as tokens.
    let spaced = text.replace('[', " [ ").replace(']', " ] ");
    let toks: Vec<&str> = spaced.split_whitespace().collect();
    let mut i = 0;
    while i < toks.len() {
        match toks[i] {
            "beginbfchar" => {
                i += 1;
                while i + 1 < toks.len() && toks[i] != "endbfchar" {
                    if let (Some(c), Some(u)) = (hex_bytes(toks[i]), hex_bytes(toks[i + 1])) {
                        out.insert(be(&c), utf16(&u));
                    }
                    i += 2;
                }
            }
            "beginbfrange" => {
                i += 1;
                while i + 2 < toks.len() && toks[i] != "endbfrange" {
                    let (Some(lo), Some(hi)) = (hex_bytes(toks[i]), hex_bytes(toks[i + 1])) else { break };
                    let (lo, hi) = (be(&lo), be(&hi));
                    if toks[i + 2] == "[" {
                        let mut j = i + 3;
                        let mut code = lo;
                        while j < toks.len() && toks[j] != "]" {
                            if let Some(u) = hex_bytes(toks[j]) {
                                out.insert(code, utf16(&u));
                            }
                            code += 1;
                            j += 1;
                        }
                        i = j + 1;
                    } else {
                        if let Some(u) = hex_bytes(toks[i + 2])
                            && hi >= lo
                            && hi - lo < 65536
                        {
                            let mut units: Vec<u16> = u.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
                            for code in lo..=hi {
                                out.insert(code, String::from_utf16_lossy(&units));
                                if let Some(last) = units.last_mut() {
                                    *last = last.wrapping_add(1);
                                }
                            }
                        }
                        i += 3;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// A glyph name's Unicode (Annex D names, `uniXXXX`, `uXXXX[XX]`, single letters).
pub fn glyph_unicode(name: &str) -> Option<char> {
    if let Ok(i) = crate::encodings::NAMES.binary_search_by(|(n, _)| (*n).cmp(name)) {
        return char::from_u32(crate::encodings::NAMES[i].1);
    }
    let hex = name.strip_prefix("uni").filter(|h| h.len() == 4).or_else(|| name.strip_prefix('u').filter(|h| (4..=6).contains(&h.len())))?;
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
}

/// Code → Unicode for a font: its ToUnicode CMap, else (simple fonts) its encoding.
fn unicode_map(doc: &Document, font: &Dict, composite: bool) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    if !composite {
        let enc = font.get(b"Encoding").map(|e| doc.resolve(e));
        let base_name = match enc.as_deref() {
            Some(Object::Name(n)) => Some(n.clone()),
            Some(Object::Dict(d)) => d.name(b"BaseEncoding").map(<[u8]>::to_vec),
            _ => None,
        };
        let table: &[u32; 256] = match base_name.as_deref() {
            Some(b"WinAnsiEncoding") => &crate::encodings::WIN_ANSI,
            Some(b"MacRomanEncoding") => &crate::encodings::MAC_ROMAN,
            _ => &crate::encodings::STANDARD,
        };
        for (code, u) in table.iter().enumerate() {
            if let Some(c) = char::from_u32(*u).filter(|_| *u != 0) {
                out.insert(code as u32, c.to_string());
            }
        }
        if let Some(Object::Dict(d)) = enc.as_deref()
            && let Some(diffs) = d.get(b"Differences").map(|x| doc.resolve(x)).and_then(|x| x.as_array().cloned())
        {
            let mut code = 0u32;
            for item in diffs {
                match &item {
                    Object::Int(n) => code = (*n).max(0) as u32,
                    Object::Name(n) => {
                        match glyph_unicode(&String::from_utf8_lossy(n)) {
                            Some(c) => out.insert(code, c.to_string()),
                            None => out.remove(&code),
                        };
                        code += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some(Object::Stream(s)) = font.get(b"ToUnicode").map(|t| doc.resolve(t)).as_deref()
        && let Ok(data) = s.decoded()
    {
        parse_to_unicode(&data, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use pdfcraft_cos::{Dict, Document, Object, Stream};

    use super::*;

    fn font(doc: &mut Document, entries: Vec<(&str, Object)>) -> Dict {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Font"));
        for (k, v) in entries {
            d.set(k.as_bytes().to_vec(), v);
        }
        let _ = doc;
        d
    }

    #[test]
    fn type3_descriptor_names_the_font_without_changing_its_metrics() {
        let mut doc = Document::new_empty();
        let mut f = font(
            &mut doc,
            vec![
                ("Subtype", Object::name("Type3")),
                (
                    "FontMatrix",
                    Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
                ),
                ("FontBBox", Object::Array(vec![Object::Int(0), Object::Int(-300), Object::Int(1000), Object::Int(1000)])),
            ],
        );
        let before = Metrics::from_dict(&doc, &f);
        let mut descriptor = Dict::new();
        descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
        descriptor.set(b"FontName".to_vec(), Object::name("ExampleMincho-Regular"));
        descriptor.set(b"Flags".to_vec(), Object::Int(6));
        descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
        let reference = doc.add(Object::Dict(descriptor));
        f.set(b"FontDescriptor".to_vec(), Object::Ref(reference));
        let after = Metrics::from_dict(&doc, &f);
        assert_eq!(after.base_font, "ExampleMincho-Regular");
        assert_eq!((after.ascent, after.descent, after.scale), (before.ascent, before.descent, before.scale));
        assert!(!after.bold && !after.italic && !after.subset);
        f.set(b"BaseFont".to_vec(), Object::name("ExplicitName"));
        assert_eq!(Metrics::from_dict(&doc, &f).base_font, "ExplicitName", "an explicit BaseFont takes precedence");
    }

    #[test]
    fn simple_fonts_decode_and_encode_through_their_encoding() {
        let mut doc = Document::new_empty();
        let f = font(
            &mut doc,
            vec![("Subtype", Object::name("Type1")), ("BaseFont", Object::name("Helvetica")), ("Encoding", Object::name("WinAnsiEncoding"))],
        );
        let m = Metrics::from_dict(&doc, &f);
        assert_eq!(m.decode(b"Caf\xe9 \x80 \x93x\x94"), "Café € “x”");
        assert_eq!(m.encode("Café €").as_deref(), Some(&b"Caf\xe9 \x80"[..]));
        assert_eq!(m.encode("Ω"), None, "not in WinAnsi");
        // Differences rename codes by glyph name.
        let mut enc = Dict::new();
        enc.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(65), Object::name("eacute"), Object::name("uni03A9")]));
        let f = font(&mut doc, vec![("Subtype", Object::name("Type1")), ("BaseFont", Object::name("Custom")), ("Encoding", Object::Dict(enc))]);
        let m = Metrics::from_dict(&doc, &f);
        assert_eq!(m.decode(b"AB"), "éΩ");
        assert_eq!(m.encode("Ω").as_deref(), Some(&b"B"[..]));
    }

    #[test]
    fn subset_fonts_only_encode_the_glyphs_they_have() {
        let mut doc = Document::new_empty();
        // Widths for a, b only (97, 98); c (99) is 0.
        let widths = Object::Array(vec![Object::Int(500), Object::Int(520), Object::Int(0)]);
        let f = font(
            &mut doc,
            vec![
                ("Subtype", Object::name("TrueType")),
                ("BaseFont", Object::name("ABCDEF+Arial")),
                ("FirstChar", Object::Int(97)),
                ("Widths", widths),
                ("Encoding", Object::name("WinAnsiEncoding")),
            ],
        );
        let m = Metrics::from_dict(&doc, &f);
        assert!(m.subset);
        assert_eq!(m.encode("ab").as_deref(), Some(&b"ab"[..]));
        assert_eq!(m.encode("abc"), None);
    }

    #[test]
    fn composite_fonts_use_to_unicode() {
        let mut doc = Document::new_empty();
        let cmap = b"/CIDInit /ProcSet findresource begin 1 begincodespacerange <0000> <FFFF> endcodespacerange \
            2 beginbfchar <0003> <0020> <0010> <00E9> endbfchar 1 beginbfrange <0024> <0026> <0041> endbfrange \
            1 beginbfrange <0030> <0031> [<0048> <0069>] endbfrange end";
        let tu = doc.add(Object::Stream(Stream::from_raw(Dict::new(), cmap.to_vec())));
        let mut desc = Dict::new();
        desc.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
        desc.set(
            b"W".to_vec(),
            Object::Array(vec![
                Object::Int(3),
                Object::Array(vec![Object::Int(250)]),
                Object::Int(16),
                Object::Int(16),
                Object::Int(500),
                Object::Int(36),
                Object::Int(49),
                Object::Int(600),
            ]),
        );
        let f = font(
            &mut doc,
            vec![
                ("Subtype", Object::name("Type0")),
                ("BaseFont", Object::name("QWERTY+Noto")),
                ("Encoding", Object::name("Identity-H")),
                ("DescendantFonts", Object::Array(vec![Object::Dict(desc)])),
                ("ToUnicode", Object::Ref(tu)),
            ],
        );
        let m = Metrics::from_dict(&doc, &f);
        assert_eq!(m.decode(&[0, 0x24, 0, 0x25, 0, 3, 0, 0x30, 0, 0x31, 0, 0x10]), "AB Hié");
        assert_eq!(m.encode("Hi A").as_deref(), Some(&[0, 0x30, 0, 0x31, 0, 3, 0, 0x24][..]));
        assert_eq!(m.encode("Z"), None);
    }

    #[test]
    fn glyph_names() {
        assert_eq!(glyph_unicode("quotedblleft"), Some('“'));
        assert_eq!(glyph_unicode("uni20AC"), Some('€'));
        assert_eq!(glyph_unicode("u1F600"), Some('😀'));
        assert_eq!(glyph_unicode("nonsense"), None);
    }
}
