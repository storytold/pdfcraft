//! The `.notdef` glyph: no text showing operator may refer to it, in any text rendering mode and
//! in any content stream (ISO 19005-2 and 19005-3 6.2.11.8, ISO 19005-4 6.2.10.9).
//!
//! Which glyph a code selects is read from the font dictionary, and a code is reported only where
//! the dictionary alone settles that the glyph is `.notdef`:
//!
//! - a simple font (`Type1`, `MMType1`, or a nonsymbolic `TrueType`) whose encoding gives the code
//!   the name `.notdef` in `/Differences`, or whose base encoding is one of the four of ISO 32000-2
//!   Annex D and the code is below 32 and not in `/Differences`: those encodings name no glyph
//!   there (D.2, D.4), and a code without a glyph in the font selects `.notdef` (9.6.5.2, 9.6.5.4);
//! - a Type 0 font with the `Identity-H` or `Identity-V` CMap, whose codes are two bytes and are
//!   the CIDs (9.7.5.2): CID 0 of a `CIDFontType0` font, the glyph every CIDFont has in place of
//!   `.notdef` (9.7.4.2), and of a `CIDFontType2` font the CIDs its `/CIDToGIDMap` takes to glyph
//!   0, TrueType's `.notdef` (Table 115), CID 0 itself when the map is `Identity` or absent.
//!
//! Everything else (a font's built-in encoding, a symbolic TrueType font, a Type 3 font, another
//! CMap, a glyph a font program may lack) isn't decided here, and nothing is reported for it.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::Op;
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object};

/// The bytes of a `/CIDToGIDMap` read: two for each CID up to 65535, the largest PDF/A allows
/// (ISO 19005-2 6.1.13). A CID past what is read isn't decided.
const MAX_CID_TO_GID: usize = 2 * 65_536;

/// The most the `/CIDToGIDMap` streams of a document decode, all together.
const MAX_MAPS_DECODED: usize = 64 << 20;

/// The simple-font encodings of ISO 32000-2 Annex D. None of them names a glyph for codes 0 to 31.
const BASE_ENCODINGS: &[&[u8]] = &[b"StandardEncoding", b"MacRomanEncoding", b"WinAnsiEncoding", b"MacExpertEncoding"];

/// What a font's dictionary says about the codes that select `.notdef`.
#[derive(Debug, PartialEq)]
enum Glyphs {
    /// One byte a code; the codes that select `.notdef`, as a set of 256 bits.
    Simple([u64; 4]),
    /// Two bytes a code, which is the CID. Without a map CID 0 is `.notdef`; with one (a glyph
    /// index of two big-endian bytes for each CID), the CIDs it takes to glyph 0 are.
    Identity { map: Option<Vec<u8>> },
    /// Not decided from the dictionary.
    Unknown,
}

impl Glyphs {
    /// Whether the string `s` shown in this font has a code that selects `.notdef`.
    fn shows_notdef(&self, s: &[u8]) -> bool {
        match self {
            Glyphs::Simple(set) => s.iter().any(|&c| set.get(usize::from(c / 64)).is_some_and(|w| (w >> (c % 64)) & 1 == 1)),
            // An odd last byte is no code: it shows nothing.
            Glyphs::Identity { map } => s.as_chunks::<2>().0.iter().any(|&code| {
                let cid = usize::from(u16::from_be_bytes(code));
                match map {
                    None => cid == 0,
                    // A CID past the map isn't decided.
                    Some(m) => cid.checked_mul(2).and_then(|at| m.get(at..at.checked_add(2)?)).is_some_and(|gid| gid == [0, 0]),
                }
            }),
            Glyphs::Unknown => false,
        }
    }
}

/// The fonts text refers to `.notdef` with, found as content is read.
pub(crate) struct Notdef<'a> {
    doc: &'a Document,
    budget: DecodeBudget,
    glyphs: HashMap<ObjRef, Glyphs>,
    found: Vec<ObjRef>,
    seen: HashSet<ObjRef>,
}

impl<'a> Notdef<'a> {
    pub(crate) fn new(doc: &'a Document) -> Self {
        Notdef { doc, budget: DecodeBudget::new(MAX_MAPS_DECODED, 0), glyphs: HashMap::new(), found: Vec::new(), seen: HashSet::new() }
    }

    /// The text showing operator `op` (`Tj`, `TJ`, `'` or `"`), in the font `font`.
    pub(crate) fn shows(&mut self, font: ObjRef, op: &Op) {
        if self.seen.contains(&font) {
            return;
        }
        if !self.glyphs.contains_key(&font) {
            let read = glyphs(self.doc, font, &mut self.budget);
            self.glyphs.insert(font, read);
        }
        let Some(glyphs) = self.glyphs.get(&font) else { return };
        if *glyphs == Glyphs::Unknown {
            return;
        }
        // The string is the last operand of each (ISO 32000-2 9.4.3, Table 107); `TJ`'s is an
        // array of strings and numbers.
        let notdef = match (op.op.as_slice(), op.back(0)) {
            (b"Tj" | b"'" | b"\"", Some(Object::String(s))) => glyphs.shows_notdef(&s.bytes),
            (b"TJ", Some(Object::Array(a))) => a.iter().filter_map(Object::as_string).any(|s| glyphs.shows_notdef(&s.bytes)),
            _ => false,
        };
        if notdef && self.seen.insert(font) {
            self.found.push(font);
        }
    }

    /// The fonts found, in the order text first referred to `.notdef` with them.
    pub(crate) fn found(self) -> Vec<ObjRef> {
        self.found
    }
}

/// What the font object `font` says about its `.notdef` codes.
fn glyphs(doc: &Document, font: ObjRef, budget: &mut DecodeBudget) -> Glyphs {
    let font = doc.resolve(&Object::Ref(font));
    let Some(font) = font.as_dict() else { return Glyphs::Unknown };
    match font.name(b"Subtype") {
        Some(b"Type1" | b"MMType1") => simple(doc, font, false),
        Some(b"TrueType") => simple(doc, font, true),
        Some(b"Type0") => composite(doc, font, budget),
        _ => Glyphs::Unknown,
    }
}

/// A simple font's `.notdef` codes, from its `/Encoding` (ISO 32000-2 9.6.5).
fn simple(doc: &Document, font: &Dict, truetype: bool) -> Glyphs {
    // A symbolic TrueType font finds its glyphs by code in its own `cmap`, not by the encoding's
    // names (9.6.5.4): only a font that is nonsymbolic (bit 6 of `/Flags`) and not symbolic (bit 3)
    // goes by them.
    if truetype {
        let flags = font
            .get(b"FontDescriptor")
            .map(|d| doc.resolve(d))
            .and_then(|d| d.as_dict().and_then(|d| d.get(b"Flags")).map(|f| doc.resolve(f)))
            .and_then(|f| f.as_int());
        if !flags.is_some_and(|f| f & 32 != 0 && f & 4 == 0) {
            return Glyphs::Unknown;
        }
    }
    let base = |n: &[u8]| BASE_ENCODINGS.contains(&n);
    let encoding = font.get(b"Encoding").map(|e| doc.resolve(e));
    let (base_known, differences) = match encoding.as_deref() {
        Some(Object::Name(n)) if base(n) => (true, None),
        // Without `/BaseEncoding` the base is the font's built-in encoding, or for a font that
        // isn't embedded one the dictionary doesn't settle (Table 112): only `/Differences` counts.
        Some(Object::Dict(d)) => {
            let base_known = d.get(b"BaseEncoding").map(|b| doc.resolve(b)).is_some_and(|b| b.as_name().is_some_and(base));
            (base_known, d.get(b"Differences").map(|x| doc.resolve(x)))
        }
        _ => return Glyphs::Unknown,
    };
    let mut set = [0u64; 4];
    if base_known && let Some(low) = set.first_mut() {
        *low = u64::from(u32::MAX);
    }
    // `[code name name … code name …]`: each name goes to the code after the last one (Table 112).
    if let Some(entries) = differences.as_deref().and_then(Object::as_array) {
        let mut code: Option<usize> = None;
        for entry in entries {
            match &*doc.resolve(entry) {
                Object::Int(n) => code = usize::try_from(*n).ok(),
                Object::Name(name) => {
                    let Some(c) = code.filter(|c| *c < 256) else { continue };
                    let (word, bit) = (c / 64, c % 64);
                    if let Some(w) = set.get_mut(word) {
                        if name.as_slice() == b".notdef" {
                            *w |= 1 << bit;
                        } else {
                            *w &= !(1 << bit);
                        }
                    }
                    code = c.checked_add(1);
                }
                // Codes are integers: after anything else the names that follow have no code.
                _ => code = None,
            }
        }
    }
    if set == [0; 4] { Glyphs::Unknown } else { Glyphs::Simple(set) }
}

/// A Type 0 font's `.notdef` codes, when its CMap is `Identity-H` or `Identity-V` (ISO 32000-2
/// 9.7.5.2, Table 116).
fn composite(doc: &Document, font: &Dict, budget: &mut DecodeBudget) -> Glyphs {
    let identity = font.get(b"Encoding").map(|e| doc.resolve(e)).is_some_and(|e| matches!(e.as_name(), Some(b"Identity-H" | b"Identity-V")));
    if !identity {
        return Glyphs::Unknown;
    }
    let descendant = font.get(b"DescendantFonts").map(|d| doc.resolve(d)).and_then(|d| d.as_array().and_then(|a| a.first().map(|f| doc.resolve(f))));
    let Some(cid_font) = descendant.as_deref().and_then(Object::as_dict) else { return Glyphs::Unknown };
    match cid_font.name(b"Subtype") {
        Some(b"CIDFontType0") => Glyphs::Identity { map: None },
        Some(b"CIDFontType2") => match cid_font.get(b"CIDToGIDMap").map(|m| doc.resolve(m)).as_deref() {
            None => Glyphs::Identity { map: None },
            Some(Object::Name(n)) if n.as_slice() == b"Identity" => Glyphs::Identity { map: None },
            // Its `/DecodeParms` may be given by reference: a predictor changes every byte of the
            // map (ISO 32000-2 7.4.4.4).
            Some(Object::Stream(s)) => match budget.decode(&doc.with_direct_parms(s)) {
                Ok(mut data) => {
                    data.truncate(MAX_CID_TO_GID);
                    Glyphs::Identity { map: Some(data) }
                }
                Err(_) => Glyphs::Unknown,
            },
            _ => Glyphs::Unknown,
        },
        _ => Glyphs::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_read_code_by_code() {
        let simple = Glyphs::Simple([1 | 1 << 31, 0, 0, 1 << 1]);
        assert!(simple.shows_notdef(b"A\x00"));
        assert!(simple.shows_notdef(b"\x1f"));
        assert!(simple.shows_notdef(b"\xc1"));
        assert!(!simple.shows_notdef(b"AB \x20\xc0"));
        assert!(!simple.shows_notdef(b""));
        let identity = Glyphs::Identity { map: None };
        assert!(identity.shows_notdef(b"\x00\x41\x00\x00"));
        // Codes are two bytes: a zero byte inside one isn't CID 0, and an odd last byte is no code.
        assert!(!identity.shows_notdef(b"\x41\x00\x00"));
        let mapped = Glyphs::Identity { map: Some(vec![0, 7, 0, 0, 0, 3]) };
        assert!(!mapped.shows_notdef(b"\x00\x00\x00\x02"));
        assert!(mapped.shows_notdef(b"\x00\x01"));
        // Past the map: not decided.
        assert!(!mapped.shows_notdef(b"\x00\x03\xff\xff"));
        assert!(!Glyphs::Unknown.shows_notdef(b"\x00\x00"));
    }
}
