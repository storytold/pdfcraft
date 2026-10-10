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

fn descriptor_flags(doc: &Document, descriptor: &Dict) -> u32 {
    let value = descriptor.get(b"Flags").and_then(|v| doc.resolve(v).as_f64()).unwrap_or(0.0) as i64;
    u32::try_from(value).unwrap_or(0)
}

/// Most entries a CMap may add, so a small stream of overlapping `bfrange`s can't take minutes
/// or gigabytes; a full 2-byte code space is 65 536. Entries past it are ignored.
const MAX_CMAP_ENTRIES: usize = 1 << 20;
/// Most codespace or CID ranges kept from a CMap; Adobe's largest published CMaps have a few
/// thousand.
const MAX_CMAP_RANGES: usize = 1 << 16;

/// A token of a CMap stream (PostScript syntax, ISO 32000-2 §9.10.3).
#[derive(Debug, PartialEq)]
enum Tok<'a> {
    /// A `<…>` string; `None` when it isn't an even number of hex digits.
    Hex(Option<Vec<u8>>),
    Open,
    Close,
    /// An operator, number or name (with its `/`), a `(…)` string or a `<<`/`>>`.
    Word(&'a [u8]),
}

/// Splits a CMap into tokens without assuming whitespace between them (`<01><0041>` is two
/// strings), skipping `%` comments.
struct Lexer<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn at(&self, i: usize) -> Option<u8> {
        self.data.get(i).copied()
    }
}

fn is_cmap_space(b: u8) -> bool {
    matches!(b, 0 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

fn is_cmap_delimiter(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Tok<'a>;

    fn next(&mut self) -> Option<Tok<'a>> {
        loop {
            let b = self.at(self.pos)?;
            if is_cmap_space(b) {
                self.pos += 1;
            } else if b == b'%' {
                while self.at(self.pos).is_some_and(|c| c != b'\n' && c != b'\r') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
        let start = self.pos;
        let b = self.at(start)?;
        self.pos += 1;
        match b {
            b'[' => Some(Tok::Open),
            b']' => Some(Tok::Close),
            b'<' | b'>' if self.at(self.pos) == Some(b) => {
                self.pos += 1;
                Some(Tok::Word(self.data.get(start..self.pos)?))
            }
            b'<' => {
                let mut digits = Vec::new();
                let mut valid = true;
                loop {
                    match self.at(self.pos) {
                        // Unterminated: what's left is one bad string.
                        None => return Some(Tok::Hex(None)),
                        Some(b'>') => break,
                        Some(c) if is_cmap_space(c) => {}
                        Some(c) => match (c as char).to_digit(16) {
                            // A hex digit is below 16, so it fits a u8.
                            Some(d) => digits.push(d as u8),
                            None => valid = false,
                        },
                    }
                    self.pos += 1;
                }
                self.pos += 1;
                if !valid || digits.is_empty() || !digits.len().is_multiple_of(2) {
                    return Some(Tok::Hex(None));
                }
                // `chunks` never yields an empty slice, so `p[0]` exists.
                Some(Tok::Hex(Some(digits.chunks(2).map(|p| p[0] << 4 | p.get(1).copied().unwrap_or(0)).collect())))
            }
            b'(' => {
                // Literal strings nest and escape their parentheses; only skipping them matters.
                let mut depth = 1usize;
                while depth > 0 {
                    match self.at(self.pos) {
                        None => break,
                        Some(b'\\') => self.pos += 1,
                        Some(b'(') => depth += 1,
                        Some(b')') => depth -= 1,
                        _ => {}
                    }
                    self.pos += 1;
                }
                Some(Tok::Word(self.data.get(start..self.pos.min(self.data.len()))?))
            }
            _ => {
                // A name keeps its `/`; other delimiters (`)`, `>`, `{`, `}`) stand alone.
                if b == b'/' || !is_cmap_delimiter(b) {
                    while self.at(self.pos).is_some_and(|c| !is_cmap_space(c) && !is_cmap_delimiter(c)) {
                        self.pos += 1;
                    }
                }
                Some(Tok::Word(self.data.get(start..self.pos)?))
            }
        }
    }
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

/// The next entry of a `begin… end…` section: `N` tokens, or `None` at its `end` operator or
/// the end of the stream.
fn entry<'a, const N: usize>(toks: &mut Lexer<'a>, end: &[u8]) -> Option<[Tok<'a>; N]> {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        match toks.next()? {
            Tok::Word(w) if w == end => return None,
            t => out.push(t),
        }
    }
    out.try_into().ok()
}

fn number(t: &Tok) -> Option<u32> {
    match t {
        Tok::Word(w) => std::str::from_utf8(w).ok()?.parse().ok(),
        _ => None,
    }
}

/// Codespace ranges and CID mappings from an embedded CMap stream, at most
/// [`MAX_CMAP_RANGES`] of each (every glyph searches them).
#[allow(clippy::type_complexity)]
fn parse_cmap(data: &[u8]) -> (Vec<(usize, Vec<u8>, Vec<u8>)>, Vec<(u32, u32, u32)>) {
    let (mut spaces, mut cids) = (Vec::new(), Vec::new());
    let mut toks = Lexer::new(data);
    while let Some(t) = toks.next() {
        match t {
            Tok::Word(b"begincodespacerange") => {
                while let Some([lo, hi]) = entry(&mut toks, b"endcodespacerange") {
                    if let (Tok::Hex(Some(lo)), Tok::Hex(Some(hi))) = (lo, hi)
                        && lo.len() == hi.len()
                        && spaces.len() < MAX_CMAP_RANGES
                    {
                        spaces.push((lo.len(), lo, hi));
                    }
                }
            }
            Tok::Word(b"begincidrange") => {
                while let Some([lo, hi, c]) = entry(&mut toks, b"endcidrange") {
                    if let (Tok::Hex(Some(lo)), Tok::Hex(Some(hi)), Some(c)) = (&lo, &hi, number(&c))
                        && cids.len() < MAX_CMAP_RANGES
                    {
                        cids.push((be(lo), be(hi), c));
                    }
                }
            }
            Tok::Word(b"begincidchar") => {
                while let Some([code, c]) = entry(&mut toks, b"endcidchar") {
                    if let (Tok::Hex(Some(code)), Some(c)) = (&code, number(&c))
                        && cids.len() < MAX_CMAP_RANGES
                    {
                        cids.push((be(code), be(code), c));
                    }
                }
            }
            _ => {}
        }
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
            let flags = descriptor_flags(doc, &d);
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
        self.cid_map.iter().find(|(lo, hi, _)| code >= *lo && code <= *hi).map_or(0, |(lo, _, c)| c.saturating_add(code - lo))
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

/// `bfchar`/`bfrange` entries of a ToUnicode CMap, at most [`MAX_CMAP_ENTRIES`] of them.
fn parse_to_unicode(data: &[u8], out: &mut HashMap<u32, String>) {
    let mut budget = MAX_CMAP_ENTRIES;
    let mut toks = Lexer::new(data);
    while let Some(t) = toks.next() {
        match t {
            Tok::Word(b"beginbfchar") => {
                while let Some([c, u]) = entry(&mut toks, b"endbfchar") {
                    if let (Tok::Hex(Some(c)), Tok::Hex(Some(u))) = (c, u) {
                        let Some(left) = budget.checked_sub(1) else { return };
                        budget = left;
                        out.insert(be(&c), utf16(&u));
                    }
                }
            }
            Tok::Word(b"beginbfrange") => {
                while let Some([lo, hi, dst]) = entry(&mut toks, b"endbfrange") {
                    let (Tok::Hex(Some(lo)), Tok::Hex(Some(hi))) = (lo, hi) else { break };
                    let (lo, hi) = (be(&lo), be(&hi));
                    match dst {
                        // One destination per code, from `lo` up to `hi`.
                        Tok::Open => {
                            let mut code = (hi >= lo).then_some(lo);
                            for t in toks.by_ref() {
                                let Some(c) = code else {
                                    if t == Tok::Close {
                                        break;
                                    }
                                    continue;
                                };
                                match t {
                                    Tok::Close => break,
                                    Tok::Hex(Some(u)) => {
                                        let Some(left) = budget.checked_sub(1) else { return };
                                        budget = left;
                                        out.insert(c, utf16(&u));
                                    }
                                    // A bad entry still stands for its code.
                                    _ => {}
                                }
                                code = c.checked_add(1).filter(|c| *c <= hi);
                            }
                        }
                        Tok::Hex(Some(u)) if hi >= lo && hi - lo < 65536 => {
                            let mut units: Vec<u16> = u.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
                            for code in lo..=hi {
                                let Some(left) = budget.checked_sub(1) else { return };
                                budget = left;
                                out.insert(code, String::from_utf16_lossy(&units));
                                if let Some(last) = units.last_mut() {
                                    *last = last.wrapping_add(1);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
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
                    // Out-of-range codes map nothing a simple font can show.
                    Object::Int(n) => code = u32::try_from((*n).max(0)).unwrap_or(u32::MAX),
                    Object::Name(n) => {
                        match glyph_unicode(&String::from_utf8_lossy(n)) {
                            Some(c) => out.insert(code, c.to_string()),
                            None => out.remove(&code),
                        };
                        code = code.saturating_add(1);
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

    fn metrics_with_flags(flags: Option<Object>) -> Metrics {
        let mut doc = Document::new_empty();
        let mut descriptor = Dict::new();
        if let Some(flags) = flags {
            descriptor.set(b"Flags".to_vec(), flags);
        }
        let font = font(&mut doc, vec![("Subtype", Object::name("TrueType")), ("FontDescriptor", Object::Dict(descriptor))]);
        Metrics::from_dict(&doc, &font)
    }

    #[test]
    fn font_descriptor_flags_stay_within_the_u32_domain() {
        let valid = metrics_with_flags(Some(Object::Int(64 | 262_144)));
        assert!(valid.italic && valid.bold);

        for flags in [Some(Object::Int(-1)), Some(Object::Int(4_294_967_296)), Some(Object::Bool(true)), None] {
            let metrics = metrics_with_flags(flags);
            assert!(!metrics.italic && !metrics.bold);
        }
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

    fn to_unicode(cmap: &[u8]) -> HashMap<u32, String> {
        let mut out = HashMap::new();
        parse_to_unicode(cmap, &mut out);
        out
    }

    #[test]
    fn cmap_tokens_need_no_space_between_them() {
        // Hex strings, brackets and operators may touch; comments and literal strings are skipped.
        let cmap = b"/CIDSystemInfo << /Registry (Adobe (x) \\) beginbfchar) >> def % beginbfchar <0001> <0058>\n\
            2 beginbfchar<0003><0020><0010><00 E9>endbfchar \
            1 beginbfrange<0024><0026><0041>endbfrange 1 beginbfrange<0030><0031>[<0048><0069>]endbfrange";
        let map = to_unicode(cmap);
        let mut got: Vec<_> = map.iter().map(|(c, u)| (*c, u.as_str())).collect();
        got.sort_unstable();
        assert_eq!(got, [(3, " "), (0x10, "é"), (0x24, "A"), (0x25, "B"), (0x26, "C"), (0x30, "H"), (0x31, "i")]);
        let (spaces, cids) = parse_cmap(b"1 begincodespacerange<00><FF>endcodespacerange 1 begincidrange<20><7E>1 endcidrange");
        assert_eq!(spaces, [(1, vec![0], vec![0xff])]);
        assert_eq!(cids, [(0x20, 0x7e, 1)]);
    }

    #[test]
    fn malformed_cmaps_never_panic() {
        // Non-ASCII bytes inside hex strings once split a UTF-8 character and panicked.
        for cmap in [
            "1 beginbfchar <0é0> <0041> <AéB> <0042> endbfchar".as_bytes(),
            "1 beginbfrange <0é0> <00é> <0041> endbfrange".as_bytes(),
            b"1 beginbfchar <0001> <004",
            b"1 beginbfchar <123> <0041> <0002> <zz> <0003> endbfchar",
            b"1 beginbfrange <FFFF> <0001> [<0041>] endbfrange",
            b"1 beginbfrange <0001> <0002> [<0041> (",
            b"1 beginbfrange <0001>",
            b"\xff\xfe beginbfchar \x80<\xc3\xa9> <0041> endbfchar %",
        ] {
            to_unicode(cmap);
            parse_cmap(cmap);
        }
        // Code counters at the top of the range stop instead of overflowing.
        let map = to_unicode(b"1 beginbfrange <FFFFFFFE> <FFFFFFFF> [<0041> <0042> <0043>] endbfrange");
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&u32::MAX).map(String::as_str), Some("B"));
        // A malformed array entry keeps its place, so the next one maps the next code.
        let map = to_unicode(b"1 beginbfrange <0001> <0003> [<0041> <zz> <0043>] endbfrange");
        assert_eq!((map.get(&2), map.get(&3).map(String::as_str)), (None, Some("C")));
        let mut m = Metrics::fallback();
        (_, m.cid_map) = parse_cmap(b"1 begincidrange <0000> <FFFF> 4294967295 endcidrange");
        assert_eq!(m.cid(5), u32::MAX);
        let mut doc = Document::new_empty();
        let mut enc = Dict::new();
        enc.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(i64::MAX), Object::name("A"), Object::name("B")]));
        let f = font(&mut doc, vec![("Subtype", Object::name("Type1")), ("Encoding", Object::Dict(enc))]);
        assert_eq!(unicode_map(&doc, &f, false).get(&u32::MAX).map(String::as_str), Some("B"));
    }

    #[test]
    fn a_cmap_adds_at_most_max_entries() {
        // 200 ranges of 65 536 codes would be 13 million strings from 4 KB of stream.
        let mut cmap = b"200 beginbfrange ".to_vec();
        for k in 0..200u32 {
            cmap.extend(format!("<{:08X}> <{:08X}> <0041> ", k << 16, (k << 16) | 0xffff).bytes());
        }
        cmap.extend(b"endbfrange");
        assert_eq!(to_unicode(&cmap).len(), MAX_CMAP_ENTRIES);
        let mut many = b"begincidrange ".to_vec();
        for k in 0..MAX_CMAP_RANGES + 10 {
            many.extend(format!("<{k:08X}> <{k:08X}> 1 ").bytes());
        }
        assert_eq!(parse_cmap(&many).1.len(), MAX_CMAP_RANGES);
    }
}
