//! What content editing and redaction need from a PDF font: how to split a string into character
//! codes, each code's advance width, the glyph height (ascent, descent), and what each code means
//! (Unicode) — and back, which text the font can show. Widths come from the font dictionary
//! (`/Widths`, `/W`, `/DW`, `/MissingWidth`); a simple font with no `/Widths` uses its embedded
//! sfnt program's advances, else the exact published standard-14 metrics ([`crate::Std14`]).
//!
//! Meanings come from `/ToUnicode`, else the encoding (`/Encoding` base and `/Differences` glyph
//! names, ISO 32000-2 Annex D). A code may deliberately have **no** meaning: where the font's own
//! built-in encoding governs and we hold no table for it — Symbol, ZapfDingbats, MacExpert — this
//! module reports nothing rather than the Latin letter at that code. Callers must treat a missing
//! meaning as "unknown", never as "absent": `redact` matches on this map, and a wrong character
//! there is worse than no character.

use std::borrow::Cow;
use std::collections::{BinaryHeap, HashMap, HashSet};

use pdfcraft_cos::{DecodeBudget, Dict, Document, Object};

/// How the bytes of a string map to codes.
#[derive(Clone, Debug, PartialEq)]
enum Codes {
    One,
    Two,
    /// Codespace ranges `(length, low, high)` from an embedded CMap.
    Ranges(Vec<(usize, Vec<u8>, Vec<u8>)>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    codes: Codes,
    /// Simple fonts: `/FirstChar` and `/Widths`.
    first: u32,
    widths: Vec<f64>,
    /// Composite fonts: CID → width, and CID ranges with one width.
    cid_widths: HashMap<u32, f64>,
    cid_ranges: Spans<f64>,
    /// Code → CID for embedded non-identity CMaps (`cidrange` / `cidchar`).
    cid_map: Spans<u32>,
    default: f64,
    /// Composite fonts with an explicit `/DW`: `/W` may leave out the glyphs that have that
    /// width (Chrome and Skia do), so a CID missing from `/W` is not a missing glyph.
    explicit_default: bool,
    /// The standard-14 face to measure with when the dictionary carries no `/Widths`.
    std14: Option<crate::Std14>,
    /// Glyph units → text space (0.001, or `/FontMatrix[0]` for Type 3).
    pub scale: f64,
    /// Glyph box in text space per unit of font size.
    pub ascent: f64,
    pub descent: f64,
    pub composite: bool,
    /// Code → Unicode text.
    unicode: Unicode,
    /// `/BaseFont` (or Type 3 descriptor's `/FontName`), and whether it is a subset (`ABCDEF+Name`: other glyphs are missing).
    pub base_font: String,
    pub subset: bool,
    /// Whether the PDF identifies this face as bold or italic. These come from the
    /// font name and, when present, `/FontDescriptor` flags/weight/angle.
    pub bold: bool,
    pub italic: bool,
    /// Whether this font draws Indic text in visual order (pre-base vowel signs before their
    /// consonant): a known legacy Indic font whose Unicode mapping is Indic. See [`legacy_indic_font`].
    pub visual_order: bool,
    /// Bytes per code for writing new text (1 for simple fonts, else the codespace length).
    code_len: usize,
    /// Valid single-character explicit mappings, preindexed once for large full CJK fonts.
    explicit_reverse: HashMap<char, u32>,
    /// Subset CIDFontType2 only: one bit per CID (below 65 536) whose glyph has an outline in the
    /// embedded program. See [`cids_with_outlines`].
    outlined_cids: Vec<u64>,
}

fn nums(doc: &Document, o: Option<&Object>) -> Vec<f64> {
    o.map(|o| doc.resolve(o))
        .and_then(|a| a.as_array().map(|a| a.iter().map(|x| doc.resolve(x).as_f64().unwrap_or(0.0)).collect()))
        .unwrap_or_default()
}

fn dict(doc: &Document, o: Option<&Object>) -> Option<Dict> {
    o.and_then(|o| doc.resolve(o).as_dict().cloned())
}

/// Whether `base_font` (subset tag allowed) names a legacy Indic font family that is laid out in
/// visual order (Kruti Dev, DevLys, Chanakya, Shusha, Shivaji, Walkman, Akruti, APS-DV, DV-TT…).
fn legacy_indic_font(base_font: &str) -> bool {
    let name =
        base_font.split_once('+').filter(|(tag, _)| tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase())).map_or(base_font, |(_, n)| n);
    let name = name.to_ascii_lowercase();
    ["kruti", "devlys", "chanakya", "shusha", "shivaji", "walkman", "akruti"].iter().any(|n| name.contains(n))
        || ["aps-", "dv-"].iter().any(|p| name.starts_with(p))
}

/// A source PDF's sfnt program for an in-memory editor. Never a bundled font or an export
/// fallback. Type 1/Type 3 programs cannot be consumed by an sfnt text renderer.
pub fn embedded_sfnt(doc: &Document, font: &Dict) -> Option<Vec<u8>> {
    embedded_sfnt_within(doc, font, &mut DecodeBudget::new(2 * MAX_SFNT, 0))
}

/// The largest embedded sfnt program read for the editor.
const MAX_SFNT: usize = 16 << 20;

/// [`embedded_sfnt`], decoding against `budget`, which an enumeration of a page's fonts shares:
/// each program decoded is charged whether or not it turns out to be usable.
pub fn embedded_sfnt_within(doc: &Document, font: &Dict, budget: &mut DecodeBudget) -> Option<Vec<u8>> {
    use skrifa::{MetadataProvider as _, raw::TableProvider as _};
    let descendant = dict(doc, font.get(b"DescendantFonts").and_then(|o| doc.resolve(o).as_array().and_then(|a| a.first()).cloned()).as_ref());
    let descriptor = dict(doc, descendant.as_ref().unwrap_or(font).get(b"FontDescriptor"))?;
    for key in [b"FontFile2".as_slice(), b"FontFile3".as_slice()] {
        let Some(object) = descriptor.get(key) else { continue };
        let object = doc.resolve(object);
        let Object::Stream(stream) = object.as_ref() else { continue };
        let Ok(mut bytes) = budget.decode_within(stream, MAX_SFNT) else { continue };
        let Ok(face) = skrifa::FontRef::from_index(&bytes, 0) else { continue };
        if face.metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default()).units_per_em == 0 {
            continue;
        }
        if face.charmap().mappings().next().is_some() {
            return Some(bytes);
        }
        // Some PDF generators keep a Mac format-6 code→glyph cmap. egui accepts Unicode
        // format-4/12 cmaps. Combine the PDF's Unicode meanings with the source glyph IDs;
        // only this in-memory copy gets a new cmap, never the working PDF or a font asset.
        let Ok(cmap) = face.cmap() else { continue };
        let metrics = Metrics::from_dict(doc, font);
        if metrics.composite {
            continue;
        }
        let mut mappings = std::collections::BTreeMap::new();
        for code in 0..=255 {
            let Some(text) = metrics.unicode.text(code) else { continue };
            let Some(ch) = only_char(&text) else { continue };
            let Some(glyph) = cmap.map_codepoint(code).or_else(|| cmap.map_codepoint(code + 0xf000)) else { continue };
            let glyph = glyph.to_u32();
            if glyph > 0 {
                mappings.entry(u32::from(ch)).or_insert(glyph);
            }
        }
        if !mappings.is_empty()
            && append_editor_cmap(&mut bytes, &mappings).is_some()
            && skrifa::FontRef::new(&bytes).is_ok_and(|font| font.charmap().mappings().next().is_some())
        {
            return Some(bytes);
        }
    }
    None
}

/// The CIDs (below 65 536) of a Type 0 font over an embedded TrueType program whose glyph has
/// outline data, through `/CIDToGIDMap`; empty when the program or the map can't be read.
/// Subsetters keep glyph ids and empty the glyphs they drop, so an empty glyph is a missing one.
fn cids_with_outlines(doc: &Document, font: &Dict) -> Vec<u64> {
    use skrifa::raw::TableProvider as _;
    let Some(descendant) = dict(doc, font.get(b"DescendantFonts").and_then(|o| doc.resolve(o).as_array().and_then(|a| a.first()).cloned()).as_ref())
    else {
        return Vec::new();
    };
    let Some(program) = dict(doc, descendant.get(b"FontDescriptor")).and_then(|d| d.get(b"FontFile2").cloned()) else { return Vec::new() };
    let program = doc.resolve(&program);
    let Object::Stream(stream) = program.as_ref() else { return Vec::new() };
    let Ok(bytes) = stream.decoded_within(16 << 20) else { return Vec::new() };
    let Some(loca) = skrifa::FontRef::from_index(&bytes, 0).ok().and_then(|face| face.loca(None).ok()) else { return Vec::new() };
    let outlined = |gid: u32| loca.get_raw(gid as usize).zip(loca.get_raw(gid as usize + 1)).is_some_and(|(start, end)| end > start);
    match descendant.get(b"CIDToGIDMap").map(|o| doc.resolve(o)).as_deref() {
        None | Some(Object::Name(_)) => bit_set_below_64k((0..loca.len().min(0x1_0000) as u32).filter(|&gid| outlined(gid))),
        Some(Object::Stream(map)) => {
            let Ok(map) = map.decoded_within(1 << 17) else { return Vec::new() };
            bit_set_below_64k(
                map.as_chunks::<2>()
                    .0
                    .iter()
                    .enumerate()
                    .filter(|(_, gid)| outlined(u32::from(u16::from_be_bytes(**gid))))
                    .map(|(cid, _)| cid as u32),
            )
        }
        Some(_) => Vec::new(),
    }
}

/// A simple font with no `/Widths` is drawn (and found by text search) with its embedded
/// program's own advances, so measure with them too; a standard-14 guess from the name can be
/// wider and leave the end of a line outside a redaction mark (#2130). The glyph is looked up
/// the way the renderer does: the encoding's character in the Unicode cmap, else the code itself
/// (as in a symbolic cmap), else `.notdef`. Only sfnt programs are read; others keep the guess.
fn embedded_widths(doc: &Document, font: &Dict, m: &mut Metrics) {
    use skrifa::instance::{LocationRef, Size};
    use skrifa::{GlyphId, MetadataProvider as _, raw::TableProvider as _};
    let Some(descriptor) = dict(doc, font.get(b"FontDescriptor")) else { return };
    for key in [b"FontFile2".as_slice(), b"FontFile3".as_slice()] {
        let Some(object) = descriptor.get(key) else { continue };
        let object = doc.resolve(object);
        let Object::Stream(stream) = object.as_ref() else { continue };
        let Ok(bytes) = stream.decoded_within(16 << 20) else { continue };
        let Ok(face) = skrifa::FontRef::from_index(&bytes, 0) else { continue };
        let upem = f64::from(face.metrics(Size::unscaled(), LocationRef::default()).units_per_em);
        if upem == 0.0 {
            continue;
        }
        let advances = face.glyph_metrics(Size::unscaled(), LocationRef::default());
        let (charmap, cmap) = (face.charmap(), face.cmap().ok());
        let found = |g: Option<GlyphId>| g.filter(|g| *g != GlyphId::NOTDEF);
        m.widths = (0..=255u32)
            .map(|code| {
                let glyph = found(m.unicode.encoding.get(&code).and_then(|t| only_char(t)).and_then(|c| charmap.map(c)))
                    .or_else(|| [0, 0xF000, 0xF100, 0xF200].into_iter().find_map(|o| found(cmap.as_ref()?.map_codepoint(code + o))));
                // A subset holds only the glyphs it draws: 0 keeps `has_glyph` false for the rest.
                match glyph {
                    None if m.subset => 0.0,
                    _ => advances.advance_width(glyph.unwrap_or(GlyphId::NOTDEF)).map_or(0.0, |w| f64::from(w) * 1000.0 / upem),
                }
            })
            .collect();
        m.first = 0;
        return;
    }
}

/// Replace an existing cmap directory entry with a bounded Unicode format-12 table.
/// Checked ranges prevent damaged sfnt offsets from indexing outside the source program.
fn append_editor_cmap(bytes: &mut Vec<u8>, mappings: &std::collections::BTreeMap<u32, u32>) -> Option<()> {
    if mappings.len() > 65536 {
        return None;
    }
    let count = u16::from_be_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
    if count > 256 {
        return None;
    }
    let mut entry = None;
    let mut head = None;
    for i in 0..count {
        let at = 12 + i * 16;
        let record = bytes.get(at..at + 16)?;
        if record.get(..4)? == b"cmap" {
            entry = Some(at);
        }
        if record.get(..4)? == b"head" {
            head = Some(u32::from_be_bytes(record.get(8..12)?.try_into().ok()?) as usize);
        }
    }
    let entry = entry?;
    let mut cmap = Vec::with_capacity(28 + mappings.len() * 12);
    cmap.extend_from_slice(&[0, 0, 0, 1, 0, 3, 0, 10, 0, 0, 0, 12, 0, 12, 0, 0]);
    cmap.extend_from_slice(&(16_u32 + u32::try_from(mappings.len()).ok()? * 12).to_be_bytes());
    cmap.extend_from_slice(&0_u32.to_be_bytes());
    cmap.extend_from_slice(&u32::try_from(mappings.len()).ok()?.to_be_bytes());
    for (&ch, &gid) in mappings {
        cmap.extend_from_slice(&ch.to_be_bytes());
        cmap.extend_from_slice(&ch.to_be_bytes());
        cmap.extend_from_slice(&gid.to_be_bytes());
    }
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    let offset = u32::try_from(bytes.len()).ok()?;
    let checksum = |data: &[u8]| {
        data.chunks(4).fold(0_u32, |sum, c| {
            let mut word = [0; 4];
            for (a, b) in word.iter_mut().zip(c) {
                *a = *b;
            }
            sum.wrapping_add(u32::from_be_bytes(word))
        })
    };
    bytes.get_mut(entry + 4..entry + 8)?.copy_from_slice(&checksum(&cmap).to_be_bytes());
    bytes.get_mut(entry + 8..entry + 12)?.copy_from_slice(&offset.to_be_bytes());
    bytes.get_mut(entry + 12..entry + 16)?.copy_from_slice(&u32::try_from(cmap.len()).ok()?.to_be_bytes());
    bytes.extend_from_slice(&cmap);
    if let Some(head) = head {
        bytes.get_mut(head.checked_add(8)?..head.checked_add(12)?)?.fill(0);
        let adjustment = 0xb1b0afba_u32.wrapping_sub(checksum(bytes));
        bytes.get_mut(head + 8..head + 12)?.copy_from_slice(&adjustment.to_be_bytes());
    }
    Some(())
}

fn descriptor_flags(doc: &Document, descriptor: &Dict) -> u32 {
    let value = descriptor.get(b"Flags").and_then(|v| doc.resolve(v).as_f64()).unwrap_or(0.0) as i64;
    u32::try_from(value).unwrap_or(0)
}

/// Most mappings a CMap may add, where each `bfchar` or array-form entry counts one and each sequential
/// range counts two (it covers at least two codes). So a CMap with at most this many codes is kept whole;
/// mappings past the budget are ignored, so a stream of them can't take minutes or gigabytes.
const MAX_CMAP_ENTRIES: usize = 1 << 20;
/// Most codespace or CID ranges kept from a CMap; Adobe's largest published CMaps have a few
/// thousand.
const MAX_CMAP_RANGES: usize = 1 << 16;
/// Largest decoded Encoding or ToUnicode CMap stream read; real CMaps stay well under it, and a larger
/// one is not decoded at all.
const MAX_CMAP_BYTES: usize = 16 << 20;

/// Code → Unicode of a font. Explicit mappings and sequential ranges are kept as declared, so a range
/// costs one record however many codes it covers; where two overlap, the one declared later wins.
#[derive(Clone, Debug, Default, PartialEq)]
struct Unicode {
    /// Simple fonts: the encoding table and `/Differences`, which ToUnicode overrides.
    encoding: HashMap<u32, String>,
    /// `bfchar` and array-form entries: the text of each code, with the declaration order of its last one.
    explicit: HashMap<u32, (u32, String)>,
    /// Sequential `bfrange` records, in declaration order.
    ranges: Vec<BfRange>,
    /// Which record covers each stretch of codes; see [`cover_index`].
    cover: Vec<(u64, Option<usize>)>,
    /// The latest-declared range covering each code below 65 536, as an index into `ranges` (`u32::MAX` for
    /// none); empty without ranges. See [`owners_below_64k`].
    low: Vec<u32>,
    /// One bit per code below 65 536 that has an explicit entry; empty without any. A miss then skips the
    /// hash lookup. See [`bit_set_below_64k`].
    explicit_low: Vec<u64>,
}

/// A sequential `bfrange`: code `lo + k` (up to `hi`) maps to `units` with its last unit moved on by `k`.
#[derive(Clone, Debug, PartialEq)]
struct BfRange {
    lo: u32,
    hi: u32,
    /// Declaration order, shared with the explicit entries.
    seq: u32,
    units: Box<[u16]>,
}

impl Unicode {
    /// Whether `code` may have an explicit entry; false is certain for codes below 65 536 without one.
    #[inline]
    fn may_be_explicit(&self, code: u32) -> bool {
        code >= 0x1_0000 || self.explicit_low.get(code as usize / 64).is_some_and(|w| (*w >> (code % 64)) & 1 == 1)
    }

    /// The latest-declared range that covers `code`.
    #[inline]
    fn covering(&self, code: u32) -> Option<&BfRange> {
        if code < 0x1_0000 {
            return self.ranges.get(*self.low.get(code as usize)? as usize);
        }
        let step = self.cover.partition_point(|&(start, _)| start <= u64::from(code));
        let &(_, record) = self.cover.get(step.checked_sub(1)?)?;
        self.ranges.get(record?)
    }

    /// The text of `code`: its explicit entry, unless a range declared after it covers the code;
    /// otherwise the covering range, otherwise the encoding.
    #[inline]
    fn text(&self, code: u32) -> Option<Cow<'_, str>> {
        let range = self.covering(code);
        let explicit = if self.may_be_explicit(code) { self.explicit.get(&code) } else { None };
        if let Some((seq, text)) = explicit
            && range.is_none_or(|r| r.seq < *seq)
        {
            return Some(Cow::Borrowed(text.as_str()));
        }
        match range {
            Some(r) => Some(Cow::Owned(r.text_at(code))),
            None => self.encoding.get(&code).map(|t| Cow::Borrowed(t.as_str())),
        }
    }
}

impl BfRange {
    /// The text of `code`, which lies in this range.
    fn text_at(&self, code: u32) -> String {
        // The offset is below 65 536 (the width limit in `parse_to_unicode`), so truncating it is exact.
        let offset = code.saturating_sub(self.lo) as u16;
        let last = self.units.len().saturating_sub(1);
        let units = self.units.iter().enumerate().map(|(i, &u)| if i == last { u.wrapping_add(offset) } else { u });
        char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
    }

    /// The lowest code of this range whose own text is `c` and that `valid` accepts. Only a single
    /// unit (a BMP character, or U+FFFD through a surrogate unit) or a surrogate pair can decode to
    /// one character `c`; longer destinations always decode to several.
    fn first_code(&self, c: char, valid: impl Fn(u32) -> bool) -> Option<u32> {
        let width = self.hi.checked_sub(self.lo)?;
        match &*self.units {
            [base] if c == '\u{FFFD}' => {
                // The offsets whose unit is a surrogate or U+FFFD, in order; each decodes to U+FFFD.
                let mut cursor = 0u32;
                loop {
                    let unit = (u32::from(*base) + cursor) & 0xFFFF;
                    let offset = cursor.checked_add(match unit {
                        0..=0xD7FF => 0xD800 - unit,
                        0xD800..=0xDFFF | 0xFFFD => 0,
                        0xE000..=0xFFFC => 0xFFFD - unit,
                        _ => 0x1_0000 - unit + 0xD800,
                    })?;
                    if offset > width {
                        return None;
                    }
                    if let Some(code) = self.lo.checked_add(offset)
                        && valid(code)
                    {
                        return Some(code);
                    }
                    cursor = offset.checked_add(1)?;
                }
            }
            [base] => {
                let offset = u32::from(u16::try_from(u32::from(c)).ok()?.wrapping_sub(*base));
                let code = self.lo.checked_add(offset).filter(|_| offset <= width)?;
                valid(code).then_some(code)
            }
            [lead, last] => {
                let v = u32::from(c).checked_sub(0x1_0000)?;
                if u32::from(*lead) != 0xD800 + (v >> 10) {
                    return None;
                }
                let low = u16::try_from(0xDC00 + (v & 0x3FF)).ok()?;
                let offset = u32::from(low.wrapping_sub(*last));
                let code = self.lo.checked_add(offset).filter(|_| offset <= width)?;
                valid(code).then_some(code)
            }
            _ => None,
        }
    }
}

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
            cid_ranges: Spans::default(),
            cid_map: Spans::default(),
            default: 500.0,
            explicit_default: false,
            std14: Some(crate::Std14::Helvetica),
            scale: 0.001,
            ascent: 0.9,
            descent: -0.25,
            composite: false,
            unicode: Unicode { encoding: (32..127u8).map(|c| (u32::from(c), char::from(c).to_string())).collect(), ..Unicode::default() },
            base_font: "Helvetica".into(),
            subset: false,
            bold: false,
            italic: false,
            visual_order: false,
            code_len: 1,
            explicit_reverse: (32..127u8).map(|c| (char::from(c), u32::from(c))).collect(),
            outlined_cids: Vec::new(),
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
        if m.composite && m.subset {
            m.outlined_cids = cids_with_outlines(doc, font);
        }
        m.code_len = match &m.codes {
            Codes::One => 1,
            Codes::Two => 2,
            Codes::Ranges(r) => r.last().map_or(2, |x| x.0),
        };
        m.unicode = unicode_map(doc, font, m.composite);
        if m.std14.is_some() {
            embedded_widths(doc, font, &mut m);
        }
        let mut reverse = HashMap::new();
        for (&code, text) in m.unicode.explicit.iter().map(|(c, (_, t))| (c, t)).chain(m.unicode.encoding.iter()) {
            if let Some(ch) = only_char(text)
                && m.shows(code, ch)
            {
                keep(&mut reverse, ch, code);
            }
        }
        m.explicit_reverse = reverse;
        // Only a font known to be laid out in visual order counts: a missing GSUB proves nothing
        // (bare CFF and subset fonts often drop it), and the font program is never decoded here.
        if legacy_indic_font(&m.base_font) {
            let indic = |c: char| matches!(u32::from(c), 0x0900..=0x0D7F);
            m.visual_order = m.unicode.explicit.values().any(|(_, t)| t.chars().any(indic))
                || m.unicode.encoding.values().any(|t| t.chars().any(indic))
                || m.unicode.ranges.iter().any(|r| r.units.iter().any(|&u| matches!(u, 0x0900..=0x0D7F)));
        }
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
                && let Ok(data) = s.decoded_within(MAX_CMAP_BYTES)
            {
                let (spaces, cids) = parse_cmap(&data);
                if !spaces.is_empty() {
                    m.codes = Codes::Ranges(spaces);
                }
                m.cid_map = Spans::new(cids);
            }
            let desc = font
                .get(b"DescendantFonts")
                .map(|d| doc.resolve(d))
                .and_then(|a| a.as_array().and_then(|a| a.first().cloned()))
                .and_then(|d| doc.resolve(&d).as_dict().cloned())
                .unwrap_or_default();
            if let Some(dw) = desc.get(b"DW").and_then(|d| doc.resolve(d).as_f64()) {
                m.default = dw;
                m.explicit_default = true;
            }
            if let Some(w) = desc.get(b"W").map(|w| doc.resolve(w)).and_then(|w| w.as_array().cloned()) {
                let mut ranges = Vec::new();
                let mut i = 0;
                while i < w.len() {
                    let Some(c0) = doc.resolve(&w[i]).as_f64().map(|v| v as u32) else { break };
                    match w.get(i + 1).map(|o| doc.resolve(o)) {
                        Some(a) if a.as_array().is_some() => {
                            for (k, x) in a.as_array().into_iter().flatten().enumerate() {
                                // A run past the last CID ends there: wrapping would give CID 0 its width.
                                let Some(cid) = u32::try_from(k).ok().and_then(|k| c0.checked_add(k)) else { break };
                                if let Some(v) = doc.resolve(x).as_f64() {
                                    m.cid_widths.insert(cid, v);
                                }
                            }
                            i += 2;
                        }
                        Some(c1) => {
                            if let (Some(c1), Some(v)) = (c1.as_f64(), w.get(i + 2).and_then(|x| doc.resolve(x).as_f64()))
                                && ranges.len() < MAX_CMAP_ENTRIES
                            {
                                ranges.push((c0, c1 as u32, v));
                            }
                            i += 3;
                        }
                        None => break,
                    }
                }
                m.cid_ranges = Spans::new(ranges);
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
                let base = String::from_utf8_lossy(font.name(b"BaseFont").unwrap_or(b"")).into_owned();
                // The real face, bold and italic included: the published metrics differ by style,
                // so Times-Bold is not Times-Roman and neither is Helvetica scaled.
                m.std14 = crate::Std14::from_base_font(&base).or(Some(crate::Std14::Helvetica));
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
        self.cid_map.find(code).map_or(0, |(lo, _, c)| c.saturating_add(code.saturating_sub(*lo)))
    }

    /// The advance of `code` in text space per unit of font size.
    pub fn width(&self, code: u32) -> f64 {
        if self.composite {
            let cid = self.cid(code);
            let w = self.cid_widths.get(&cid).copied().or_else(|| self.cid_ranges.find(cid).map(|r| r.2));
            return w.unwrap_or(self.default) * self.scale;
        }
        if let Some(w) = code.checked_sub(self.first).and_then(|i| self.widths.get(i as usize)) {
            return w * self.scale;
        }
        match self.std14 {
            // A font with no /Widths is one of the standard 14; use its published metrics.
            // Codes the font's encoding leaves undefined fall back to /MissingWidth.
            Some(f) => match u8::try_from(self.std14_code(f, code)).map(|c| f.width(c)).unwrap_or(0.0) {
                0.0 => self.default * self.scale,
                w => w / 1000.0,
            },
            None => self.default * self.scale,
        }
    }

    /// Where the standard-14 table, indexed by WinAnsi, holds the glyph `code` draws in this font:
    /// its encoding (base and `/Differences`) names the glyph, so a remapped code is measured as
    /// the glyph it now shows (#2894). Symbol and ZapfDingbats are indexed by their own encodings,
    /// and a glyph WinAnsi has no code for keeps the code itself.
    fn std14_code(&self, face: crate::Std14, code: u32) -> u32 {
        if matches!(face, crate::Std14::Symbol | crate::Std14::ZapfDingbats) {
            return code;
        }
        self.unicode.encoding.get(&code).and_then(|t| only_char(t)).and_then(crate::win_ansi_byte).map_or(code, u32::from)
    }

    /// Word spacing applies to the single-byte code 32 (§9.3.3).
    pub fn is_space(&self, code: u32, len: usize) -> bool {
        code == 32 && len == 1
    }
}

impl Metrics {
    /// The text a string shows (codes without a known meaning are left out).
    pub fn decode(&self, s: &[u8]) -> String {
        self.codes(s).into_iter().filter_map(|(c, _)| self.text_of(c)).collect()
    }

    /// The Unicode text of one code, if known.
    #[inline]
    pub fn text_of(&self, code: u32) -> Option<Cow<'_, str>> {
        self.unicode.text(code)
    }

    /// Whether the font has a glyph for `code` (subset fonts lack the glyphs they don't use).
    /// Only asked for codes the font's Unicode mapping gives: a subset's `ToUnicode` lists the
    /// glyphs it kept, while its `/W` leaves out those of the default width (#1443).
    fn has_glyph(&self, code: u32) -> bool {
        if self.composite {
            let cid = self.cid(code);
            // CID 0 is `.notdef` (an unmapped code lands there too), whatever ToUnicode says: Word
            // maps it to a space and gives it a width, and typing with it draws boxes (#3223).
            if cid == 0 {
                return false;
            }
            // A CID /W leaves out takes /DW, and Skia (Chrome) leaves out every CID of that width: for
            // those, only the subset's program says whether the glyph is there.
            return self.cid_widths.contains_key(&cid)
                || self.cid_ranges.find(cid).is_some()
                || !self.subset
                || self.outlined_cids.get(cid as usize / 64).is_some_and(|w| (*w >> (cid % 64)) & 1 == 1)
                // No readable TrueType program to ask (e.g. CFF or missing): an explicit /DW
                // still says /W may leave out glyphs of that width (#1443).
                || (self.outlined_cids.is_empty() && self.explicit_default);
        }
        match code.checked_sub(self.first).and_then(|i| self.widths.get(i as usize)) {
            Some(w) => *w > 0.0 || code == 32,
            None => self.widths.is_empty() && !self.subset,
        }
    }

    /// Whether `code` shows exactly `c` and has a glyph for it.
    fn shows(&self, code: u32, c: char) -> bool {
        self.unicode.text(code).is_some_and(|t| t.chars().eq(std::iter::once(c))) && self.has_glyph(code)
    }

    /// The bytes that show `text` in this font, or `None` if some character has no code or
    /// no glyph in it (the caller then substitutes another font).
    pub fn encode(&self, text: &str) -> Option<Vec<u8>> {
        // The lowest code showing each wanted character. Explicit entries and the encoding are
        // checked one by one; each range is asked for the wanted characters it can show.
        let wanted: HashSet<char> = text.chars().collect();
        let mut reverse: HashMap<char, u32> = HashMap::new();
        for &ch in &wanted {
            if let Some(&code) = self.explicit_reverse.get(&ch) {
                keep(&mut reverse, ch, code);
            }
        }
        for range in &self.unicode.ranges {
            for &c in &wanted {
                if let Some(code) = range.first_code(c, |m| self.shows(m, c)) {
                    keep(&mut reverse, c, code);
                }
            }
        }
        let mut out = Vec::with_capacity(text.len() * self.code_len);
        for ch in text.chars() {
            let code = *reverse.get(&ch)?;
            let bytes = code.to_be_bytes();
            out.extend_from_slice(&bytes[4 - self.code_len.clamp(1, 4)..]);
        }
        Some(out)
    }
}

/// Most UTF-16 units kept from one ToUnicode destination: the CMap format allows destinations of up
/// to 512 bytes, and every glyph a sequential range covers builds its text from the whole of one.
const MAX_DESTINATION_UNITS: usize = 256;

/// The UTF-16 code units of CMap hex bytes, at most [`MAX_DESTINATION_UNITS`] of them.
fn code_units(b: &[u8]) -> Vec<u16> {
    b.chunks(2).take(MAX_DESTINATION_UNITS).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect()
}

/// UTF-16BE (with surrogates) from CMap hex bytes.
fn utf16(b: &[u8]) -> String {
    String::from_utf16_lossy(&code_units(b))
}

/// The character `s` is made of, if it is exactly one.
fn only_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

/// Keeps the lowest code found for `c`.
fn keep(reverse: &mut HashMap<char, u32>, c: char, code: u32) {
    reverse.entry(c).and_modify(|r| *r = (*r).min(code)).or_insert(code);
}

/// Spends `cost` of the budget; `false` when less is left.
fn spend(budget: &mut usize, cost: usize) -> bool {
    let Some(left) = budget.checked_sub(cost) else { return false };
    *budget = left;
    true
}

/// Adds an explicit mapping; `false` once the budget is spent.
fn add_explicit(out: &mut Unicode, budget: &mut usize, seq: &mut u32, code: u32, text: String) -> bool {
    if !spend(budget, 1) {
        return false;
    }
    *seq = seq.saturating_add(1);
    out.explicit.insert(code, (*seq, text));
    true
}

/// Adds a sequential range, which covers at least two codes and so costs two; `false` once the budget is spent.
fn add_range(out: &mut Unicode, budget: &mut usize, seq: &mut u32, lo: u32, hi: u32, units: Box<[u16]>) -> bool {
    if !spend(budget, 2) {
        return false;
    }
    *seq = seq.saturating_add(1);
    out.ranges.push(BfRange { lo, hi, seq: *seq, units });
    true
}

#[cfg(test)]
thread_local! {
    /// Code span records read by CID and width lookups, for the tests that keep a lookup logarithmic.
    static SPAN_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts one span record read (tests only).
#[inline]
fn count_span_reads() {
    #[cfg(test)]
    SPAN_READS.with(|c| c.set(c.get().saturating_add(1)));
}

/// Code spans `(lo, hi, value)` in declaration order, where the first-declared span covering a code
/// wins, indexed so that a lookup is one binary search.
#[derive(Clone, Debug, Default, PartialEq)]
struct Spans<T> {
    spans: Vec<(u32, u32, T)>,
    /// The span covering each stretch of codes; see [`stretches`].
    cover: Vec<(u64, Option<usize>)>,
}

impl<T> Spans<T> {
    fn new(spans: Vec<(u32, u32, T)>) -> Self {
        let bounds: Vec<(u32, u32)> = spans.iter().map(|s| (s.0, s.1)).collect();
        Spans { cover: stretches(&bounds, false), spans }
    }

    fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The first-declared span covering `code`.
    fn find(&self, code: u32) -> Option<&(u32, u32, T)> {
        let step = self.cover.partition_point(|&(start, _)| {
            count_span_reads();
            start <= u64::from(code)
        });
        let &(_, record) = self.cover.get(step.checked_sub(1)?)?;
        self.spans.get(record?)
    }
}

/// For each stretch of codes, from its first code on, the latest-declared range covering it (`None`
/// where no range does), sorted by code; a lookup is one binary search over it.
fn cover_index(ranges: &[BfRange]) -> Vec<(u64, Option<usize>)> {
    let spans: Vec<(u32, u32)> = ranges.iter().map(|r| (r.lo, r.hi)).collect();
    stretches(&spans, true)
}

/// For each stretch of codes, from its first code on, the span `(lo, hi)` covering it (`None` where
/// none does), sorted by code: the latest-declared one when `later_wins`, else the first-declared.
/// At most `u32::MAX` spans are indexed.
fn stretches(spans: &[(u32, u32)], later_wins: bool) -> Vec<(u64, Option<usize>)> {
    // Heap keys, largest for the winning record; the mapping is its own inverse.
    let key = |record: u32| if later_wins { record } else { !record };
    // Openings (first code, record) and closings (one past the last code), each in code order. The cover
    // can only change at one of their codes, so the sweep visits those codes in order.
    let mut opens: Vec<(u32, u32)> = spans.iter().zip(0..u32::MAX).map(|(&(lo, _), i)| (lo, i)).collect();
    opens.sort_unstable();
    let mut closes: Vec<u64> = spans.iter().take(opens.len()).map(|&(_, hi)| u64::from(hi) + 1).collect();
    closes.sort_unstable();
    // Keys of the records that have opened and not yet closed; the top is the winning one.
    let mut open: BinaryHeap<u32> = BinaryHeap::new();
    let mut cover: Vec<(u64, Option<usize>)> = Vec::new();
    let (mut next_open, mut next_close) = (0, 0);
    loop {
        let p = match (opens.get(next_open), closes.get(next_close)) {
            (Some(&(lo, _)), Some(&end)) => u64::from(lo).min(end),
            (Some(&(lo, _)), None) => u64::from(lo),
            (None, Some(&end)) => end,
            (None, None) => break,
        };
        while let Some(&(lo, record)) = opens.get(next_open)
            && u64::from(lo) <= p
        {
            open.push(key(record));
            next_open += 1;
        }
        while closes.get(next_close).is_some_and(|&end| end <= p) {
            next_close += 1;
        }
        while open.peek().is_some_and(|&top| spans.get(key(top) as usize).is_none_or(|&(_, hi)| u64::from(hi) < p)) {
            open.pop();
        }
        let winner = open.peek().map(|&top| key(top) as usize);
        if cover.last().map(|&(_, w)| w) != Some(winner) {
            cover.push((p, winner));
        }
    }
    cover
}

/// One bit for each code below 65 536 in `codes`; empty when there is none.
/// Sized to the highest such code, so a font with a few low codes costs a few words, not 8 KB.
fn bit_set_below_64k(codes: impl Iterator<Item = u32>) -> Vec<u64> {
    let low: Vec<u32> = codes.filter(|&code| code < 0x1_0000).collect();
    let Some(&max) = low.iter().max() else { return Vec::new() };
    let mut bits = vec![0u64; max as usize / 64 + 1];
    for code in low {
        if let Some(word) = bits.get_mut(code as usize / 64) {
            *word |= 1u64 << (code % 64);
        }
    }
    bits
}

/// The latest-declared range covering each code below 65 536, read off the stretches of `cover`; empty
/// when no range covers such a code. The stretches are disjoint, so this takes at most 65 536 steps
/// however many ranges there are.
///
/// The table ends with the last covered code: a code past it has no covering range, and a font whose
/// ranges cover a few low codes costs that many entries, not a fixed 256 KB.
fn owners_below_64k(cover: &[(u64, Option<usize>)]) -> Vec<u32> {
    let stretch_end = |i: usize| cover.get(i + 1).map_or(0x1_0000, |&(next, _)| next.min(0x1_0000));
    let len = cover
        .iter()
        .enumerate()
        .filter(|&(_, &(start, winner))| winner.is_some() && start < 0x1_0000)
        .map(|(i, _)| stretch_end(i))
        .max()
        .unwrap_or(0);
    let mut owner = vec![u32::MAX; usize::try_from(len).unwrap_or(0)];
    for (i, &(start, winner)) in cover.iter().enumerate() {
        let Some(record) = winner else { continue };
        // Record indices are below MAX_CMAP_ENTRIES, which fits in a u32.
        let record = record as u32;
        for code in start..stretch_end(i) {
            if let Some(slot) = owner.get_mut(code as usize) {
                *slot = record;
            }
        }
    }
    owner
}

/// `bfchar`/`bfrange` entries of a ToUnicode CMap, at most [`MAX_CMAP_ENTRIES`] mappings of them.
fn parse_to_unicode(data: &[u8], out: &mut Unicode) {
    parse_to_unicode_within(data, out, MAX_CMAP_ENTRIES);
}

/// [`parse_to_unicode`] with `budget` explicit entries, so tests can use a small one.
fn parse_to_unicode_within(data: &[u8], out: &mut Unicode, mut budget: usize) {
    let mut seq = 0u32;
    let mut toks = Lexer::new(data);
    'scan: while let Some(t) = toks.next() {
        match t {
            Tok::Word(b"beginbfchar") => {
                while let Some([c, u]) = entry(&mut toks, b"endbfchar") {
                    if let (Tok::Hex(Some(c)), Tok::Hex(Some(u))) = (c, u) {
                        let added = add_explicit(out, &mut budget, &mut seq, be(&c), utf16(&u));
                        if !added {
                            break 'scan;
                        }
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
                                        let added = add_explicit(out, &mut budget, &mut seq, c, utf16(&u));
                                        if !added {
                                            break 'scan;
                                        }
                                    }
                                    // A bad entry still stands for its code.
                                    _ => {}
                                }
                                code = c.checked_add(1).filter(|c| *c <= hi);
                            }
                        }
                        Tok::Hex(Some(u)) if hi >= lo && hi - lo < 65536 => {
                            let added = if lo == hi {
                                add_explicit(out, &mut budget, &mut seq, lo, utf16(&u))
                            } else {
                                add_range(out, &mut budget, &mut seq, lo, hi, code_units(&u).into_boxed_slice())
                            };
                            if !added {
                                break 'scan;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out.cover = cover_index(&out.ranges);
    out.low = owners_below_64k(&out.cover);
    out.explicit_low = bit_set_below_64k(out.explicit.keys().copied());
}

/// A glyph name's text, by the Adobe Glyph List *algorithm* — the published naming rules, not
/// the AGL data table, which this crate does not hold.
///
/// A name is cut at its first `.`, because a suffix marks a variant of the same character rather
/// than a different one (`a.sc` is small-capital `a`, `one.oldstyle` is still `1`). What remains
/// is split on `_` into components, so a ligature name yields several characters (`f_i` is "fi").
/// Each component is read as an Annex D name, as `uniXXXX` with one or more four-hex-digit values,
/// or as `uXXXX` through `uXXXXXX`.
///
/// Returns `None` rather than a partial answer: a name is either fully understood or not, and half
/// a ligature would be worse than admitting we cannot read it.
pub fn glyph_text(name: &str) -> Option<String> {
    let stem = name.split('.').next().filter(|s| !s.is_empty())?;
    let mut out = String::new();
    for part in stem.split('_') {
        out.push_str(&glyph_component(part)?);
    }
    Some(out).filter(|s| !s.is_empty())
}

/// One `_`-separated component of a glyph name.
fn glyph_component(part: &str) -> Option<String> {
    if let Ok(i) = crate::encodings::NAMES.binary_search_by(|(n, _)| (*n).cmp(part)) {
        return char::from_u32(crate::encodings::NAMES[i].1).map(String::from);
    }
    // `from_str_radix` accepts a leading `+`, which is not a glyph name, so the digits are checked.
    let hexish = |h: &str| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit());
    if let Some(hex) = part.strip_prefix("uni").filter(|h| h.len() >= 4 && h.len() % 4 == 0 && hexish(h)) {
        let mut s = String::new();
        for i in (0..hex.len()).step_by(4) {
            // A lone surrogate half is not a character; such a name is not usable text.
            s.push(char::from_u32(u32::from_str_radix(hex.get(i..i.saturating_add(4))?, 16).ok()?)?);
        }
        return Some(s);
    }
    let hex = part.strip_prefix('u').filter(|h| (4..=6).contains(&h.len()) && hexish(h))?;
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32).map(String::from)
}

/// [`glyph_text`] for names that stand for exactly one character.
pub fn glyph_unicode(name: &str) -> Option<char> {
    let mut chars = glyph_text(name)?.chars().collect::<Vec<_>>().into_iter();
    chars.next().filter(|_| chars.len() == 0)
}

/// Code → Unicode for a font: its ToUnicode CMap (for a composite font, also the name
/// `/Identity-H` or `/Identity-V`, read as code = Unicode), else (simple fonts) its encoding.
fn unicode_map(doc: &Document, font: &Dict, composite: bool) -> Unicode {
    let mut out = Unicode::default();
    if !composite {
        let enc = font.get(b"Encoding").map(|e| doc.resolve(e));
        let base_name = match enc.as_deref() {
            Some(Object::Name(n)) => Some(n.clone()),
            Some(Object::Dict(d)) => d.name(b"BaseEncoding").map(<[u8]>::to_vec),
            _ => None,
        };
        // Symbol and ZapfDingbats carry their own built-in encodings, which are not Latin
        // (ISO 32000-2 9.6.6.1): Symbol code 0x61 is `alpha`, not `a`. We do not hold those two
        // tables, so with no base encoding named they get no implicit mapping at all. Reporting
        // nothing is honest; reporting Latin letters silently corrupts text extraction, find,
        // copy and — because `redact` matches on this map — redaction.
        //
        // Deliberately keyed on the *name*, not on the `/FontDescriptor` symbolic flag. A great
        // many ordinary Latin TrueType subsets set that flag and still rely on a Latin encoding,
        // so suppressing on the flag would lose real text on a large share of real documents.
        // An explicitly named base encoding is a deliberate statement and is always honoured,
        // even on Symbol.
        let symbolic_builtin = matches!(
            font.name(b"BaseFont").map(|b| crate::Std14::from_base_font(&String::from_utf8_lossy(b))),
            Some(Some(crate::Std14::Symbol | crate::Std14::ZapfDingbats))
        );
        let table: Option<&[u32; 256]> = match base_name.as_deref() {
            Some(b"WinAnsiEncoding") => Some(&crate::encodings::WIN_ANSI),
            Some(b"MacRomanEncoding") => Some(&crate::encodings::MAC_ROMAN),
            Some(b"StandardEncoding") => Some(&crate::encodings::STANDARD),
            // MacExpert is small caps, oldstyle figures and fractions, not Latin at these codes,
            // and we hold no table for it either.
            Some(b"MacExpertEncoding") => None,
            _ if symbolic_builtin => None,
            // Standard is the default for a simple font that names no base encoding.
            _ => Some(&crate::encodings::STANDARD),
        };
        for (code, u) in table.into_iter().flatten().enumerate() {
            if let Some(c) = char::from_u32(*u).filter(|_| *u != 0) {
                out.encoding.insert(code as u32, c.to_string());
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
                        // A name we cannot read removes any inherited meaning for that code,
                        // rather than leaving the base encoding's letter standing in for it.
                        match glyph_text(&String::from_utf8_lossy(n)) {
                            Some(t) => out.encoding.insert(code, t),
                            None => out.encoding.remove(&code),
                        };
                        code = code.saturating_add(1);
                    }
                    _ => {}
                }
            }
        }
    }
    match font.get(b"ToUnicode").map(|t| doc.resolve(t)).as_deref() {
        Some(Object::Stream(s)) => {
            if let Ok(data) = s.decoded_within(MAX_CMAP_BYTES) {
                parse_to_unicode(&data, &mut out);
            }
        }
        // `/ToUnicode /Identity-H` is not a stream, as ISO 32000-2 §9.10.3 requires, but CAD
        // exporters (WSCAD among them) write it for composite fonts whose two-byte codes are the
        // UTF-16 code units. Acrobat shows them as code = Unicode, and so does the renderer's
        // text layer; without it the text could be seen but not edited or found. The same rule
        // (code = Unicode, no controls or lone surrogates) is in the vendored renderer,
        // `Type0Font::char_code_to_unicode` in vendor/hayro-interpret/src/font/cid.rs: keep both alike.
        Some(Object::Name(n)) if composite && matches!(n.as_slice(), b"Identity-H" | b"Identity-V") => {
            parse_to_unicode(&identity_to_unicode(), &mut out);
        }
        _ => {}
    }
    out
}

/// A ToUnicode CMap mapping each two-byte code to the same UTF-16 code unit, leaving out the
/// surrogates (which are not characters on their own) and the controls (C0, DEL and C1, as the
/// renderer's text layer does). No range crosses a last-byte boundary (§9.10.3).
fn identity_to_unicode() -> Vec<u8> {
    let mut ranges = vec![(0x20u32, 0x7Eu32), (0xA0, 0xFF)];
    ranges.extend((0x01u32..=0xFF).filter(|h| !(0xD8..=0xDF).contains(h)).map(|h| (h << 8, (h << 8) | 0xFF)));
    let mut cmap = String::from("1 begincodespacerange <0000> <FFFF> endcodespacerange\n");
    // At most 100 entries per section (§9.7.5.2 limits for CMap files).
    for chunk in ranges.chunks(100) {
        cmap.push_str(&format!("{} beginbfrange\n", chunk.len()));
        for (lo, hi) in chunk {
            cmap.push_str(&format!("<{lo:04X}> <{hi:04X}> <{lo:04X}>\n"));
        }
        cmap.push_str("endbfrange\n");
    }
    cmap.into_bytes()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod encoding_tests {
    use pdfcraft_cos::{Dict, Document, Object};

    use super::*;

    /// A simple Type 1 font, optionally symbolic-flagged and optionally with an `/Encoding`.
    fn simple(base: &str, flags: Option<i64>, enc: Option<Object>) -> (Document, Dict) {
        let doc = Document::new_empty();
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Font"));
        d.set(b"Subtype".to_vec(), Object::name("Type1"));
        d.set(b"BaseFont".to_vec(), Object::name(base));
        if let Some(f) = flags {
            let mut desc = Dict::new();
            desc.set(b"Flags".to_vec(), Object::Int(f));
            d.set(b"FontDescriptor".to_vec(), Object::Dict(desc));
        }
        if let Some(e) = enc {
            d.set(b"Encoding".to_vec(), e);
        }
        (doc, d)
    }

    fn differences(items: Vec<Object>) -> Object {
        let mut enc = Dict::new();
        enc.set(b"Differences".to_vec(), Object::Array(items));
        Object::Dict(enc)
    }

    #[test]
    fn glyph_names_follow_the_naming_algorithm() {
        // Annex D names and the two hex forms, as before.
        assert_eq!(glyph_text("quotedblleft").as_deref(), Some("\u{201c}"));
        assert_eq!(glyph_text("uni20AC").as_deref(), Some("\u{20ac}"));
        assert_eq!(glyph_text("u1F600").as_deref(), Some("\u{1f600}"));
        // A suffix marks a variant of the same character, so the stem still reads.
        assert_eq!(glyph_text("a.sc").as_deref(), Some("a"));
        assert_eq!(glyph_text("one.oldstyle").as_deref(), Some("1"));
        assert_eq!(glyph_text("A.alt017").as_deref(), Some("A"));
        // `_` joins components, so a ligature name is more than one character.
        assert_eq!(glyph_text("f_i").as_deref(), Some("fi"));
        assert_eq!(glyph_text("f_f_l").as_deref(), Some("ffl"));
        assert_eq!(glyph_text("a_uni0301").as_deref(), Some("a\u{301}"));
        // `uniXXXX` carries several values, so one name can be a whole cluster.
        assert_eq!(glyph_text("uni00660069").as_deref(), Some("fi"));
        assert_eq!(glyph_text("uni004100300041").as_deref(), Some("A0A"));
        // Both forms combine with a suffix.
        assert_eq!(glyph_text("uni00660069.alt").as_deref(), Some("fi"));

        // Not names: unknown, empty, a half-read ligature, bad hex, and the `+` that
        // `from_str_radix` would otherwise accept.
        assert_eq!(glyph_text("nonsense"), None);
        assert_eq!(glyph_text(""), None);
        assert_eq!(glyph_text(".notdef"), None);
        assert_eq!(glyph_text("f_nonsense"), None, "half a ligature is not an answer");
        assert_eq!(glyph_text("uni00"), None, "not a whole four-digit value");
        assert_eq!(glyph_text("uni0041004"), None);
        assert_eq!(glyph_text("uni+041"), None, "a sign is not a hex digit");
        assert_eq!(glyph_text("u+0041"), None);
        assert_eq!(glyph_text("uniD800"), None, "a lone surrogate half is not a character");
        // Glyph-index names carry no meaning, and must not be guessed at.
        assert_eq!(glyph_text("g3"), None);
        assert_eq!(glyph_text("index42"), None);

        // `glyph_unicode` is the single-character view of the same algorithm.
        assert_eq!(glyph_unicode("uni20AC"), Some('\u{20ac}'));
        assert_eq!(glyph_unicode("a.sc"), Some('a'));
        assert_eq!(glyph_unicode("f_i"), None, "a ligature is not one character");
    }

    #[test]
    fn a_ligature_or_unreadable_name_in_differences_is_handled() {
        // A ligature name maps one code to two characters, so text extraction reads "fi".
        let (doc, d) = simple("Custom", None, Some(differences(vec![Object::Int(65), Object::name("f_i")])));
        let m = Metrics::from_dict(&doc, &d);
        assert_eq!(m.decode(b"A"), "fi");

        // An unreadable name must *remove* the base encoding's letter rather than leave it
        // standing in: code 65 is no longer "A", because the font says it is something else.
        let (doc, d) = simple("Custom", None, Some(differences(vec![Object::Int(65), Object::name("g7")])));
        let m = Metrics::from_dict(&doc, &d);
        assert_eq!(m.text_of(65), None);
        assert_eq!(m.text_of(66).as_deref(), Some("B"), "neighbouring codes keep their meaning");
    }

    #[test]
    fn symbol_and_dingbats_are_not_read_as_latin() {
        // Symbol's built-in encoding puts `alpha` at 0x61. We hold no table for it, so the honest
        // answer is none at all -- never "a", which would corrupt find, copy and redaction.
        for base in ["Symbol", "ZapfDingbats", "ABCDEF+Symbol"] {
            let (doc, d) = simple(base, Some(4), None);
            let m = Metrics::from_dict(&doc, &d);
            assert_eq!(m.text_of(0x61), None, "{base} must not claim 'a'");
            assert_eq!(m.decode(b"abg"), "", "{base}");
        }

        // MacExpert is small caps and oldstyle figures at these codes, and we hold no table.
        let (doc, d) = simple("Times-Roman", None, Some(Object::name("MacExpertEncoding")));
        let m = Metrics::from_dict(&doc, &d);
        assert_eq!(m.text_of(0x61), None);

        // /Differences still applies on top of a built-in encoding we cannot read.
        let (doc, d) = simple("Symbol", Some(4), Some(differences(vec![Object::Int(0x61), Object::name("uni03B1")])));
        let m = Metrics::from_dict(&doc, &d);
        assert_eq!(m.text_of(0x61).as_deref(), Some("\u{3b1}"), "the font told us this one");
        assert_eq!(m.text_of(0x62), None, "and nothing about this one");
    }

    #[test]
    fn a_named_base_encoding_is_always_honoured() {
        // Naming an encoding is a deliberate statement, so it wins even on Symbol.
        for name in ["WinAnsiEncoding", "MacRomanEncoding", "StandardEncoding"] {
            let mut enc = Dict::new();
            enc.set(b"BaseEncoding".to_vec(), Object::name(name));
            let (doc, d) = simple("Symbol", Some(4), Some(Object::Dict(enc)));
            let m = Metrics::from_dict(&doc, &d);
            assert_eq!(m.text_of(0x61).as_deref(), Some("a"), "{name} was asked for by name");
        }
    }

    /// The Annex D.2 alias codes read, through the whole decode path, as the ink they draw.
    ///
    /// This is the map `redact` matches its patterns on (`unicode_map`), so a code that decodes
    /// to an invisible formatting character is a silent miss, not a cosmetic one: "well-known"
    /// written with the 0xAD byte matched no search and no redaction pattern.
    #[test]
    fn the_alias_codes_decode_as_ordinary_ink_and_encode_back_to_the_low_code() {
        let (doc, d) = simple("Helvetica", None, Some(Object::name("WinAnsiEncoding")));
        let m = Metrics::from_dict(&doc, &d);
        assert_eq!(m.decode(b"well\xadknown"), "well-known");
        assert_eq!(m.decode(b"a\xa0b"), "a b");
        assert_eq!(m.encode("well-known a b").as_deref(), Some(&b"well-known a b"[..]), "the low code is the one written");

        let (doc, d) = simple("Helvetica", None, Some(Object::name("MacRomanEncoding")));
        let m = Metrics::from_dict(&doc, &d);
        assert_eq!(m.decode(b"a\xcab"), "a b");
        assert_eq!(m.decode(b"a\xadb"), "a\u{2260}b", "MacRoman 0xAD is `notequal`, not a hyphen");
        assert_eq!(m.encode("a b").as_deref(), Some(&b"a b"[..]));

        // An explicit /Differences name still says what it says: this is a property of those
        // codes in those tables, not of the glyph names.
        let (doc, d) = simple("Helvetica", None, Some(differences(vec![Object::Int(0x41), Object::name("sfthyphen"), Object::name("nbspace")])));
        let m = Metrics::from_dict(&doc, &d);
        assert_eq!(m.decode(b"AB"), "\u{ad}\u{a0}");
    }

    /// Pins a deliberate restraint, so it is not "fixed" into a regression: a great many ordinary
    /// Latin TrueType subsets set the `/FontDescriptor` symbolic flag and still rely on a Latin
    /// encoding. Suppressing the implicit table on the flag would lose real text on a large share
    /// of real documents, so only Symbol and ZapfDingbats -- named, and unambiguous -- suppress it.
    #[test]
    fn the_symbolic_flag_alone_does_not_suppress_the_latin_default() {
        let (doc, d) = simple("SomeSubsetFont", Some(4), None);
        let m = Metrics::from_dict(&doc, &d);
        assert_eq!(m.text_of(0x61).as_deref(), Some("a"));
        assert_eq!(m.decode(b"abc"), "abc");
    }

    #[test]
    fn unusable_programs_still_spend_the_decode_budget() {
        // 128 resources naming one 1 MiB program that isn't a font: each decode is charged,
        // so an enumeration stops decoding once the shared budget is spent.
        let mut doc = Document::new_empty();
        let program = doc.add(pdfcraft_cos::Stream::flate(Dict::new(), &vec![0; 1 << 20]));
        let descriptor: Dict = [(b"FontFile2".to_vec(), Object::Ref(program))].into_iter().collect();
        let descriptor = doc.add(descriptor);
        let font: Dict = [(b"FontDescriptor".to_vec(), Object::Ref(descriptor))].into_iter().collect();
        let mut budget = DecodeBudget::new(64 << 20, 0);
        for _ in 0..128 {
            assert!(embedded_sfnt_within(&doc, &font, &mut budget).is_none());
        }
        assert_eq!(budget.remaining(), 0);
    }
}
