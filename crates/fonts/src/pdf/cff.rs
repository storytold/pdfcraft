//! An editor-only OpenType wrapper for a bare CFF program.
//!
//! A PDF may embed a font as a bare Compact Font Format program (`/FontFile3` with `/Subtype`
//! `/Type1C` or `/CIDFontType0C`): the outlines of an OpenType/CFF font without the sfnt tables a
//! text renderer reads (`cmap`, `head`, `hhea`, `hmtx`, `maxp`). Acrobat Distiller, InDesign and
//! Word write most PostScript-flavoured fonts this way. The inline editor draws a draft with an
//! sfnt text renderer, so without a wrapper such text fell back to an interface face squeezed into
//! the source's widths.
//!
//! [`editor_sfnt`] wraps the program, byte for byte, in the tables that renderer needs: a Unicode
//! `cmap` built the way the page selects glyphs (the PDF's `/Encoding` and `/Differences` by glyph
//! name, else the program's own encoding; CIDs through the charset) and what each code means
//! (`/ToUnicode`, else the encoding), and the advances the PDF's `/Widths` give, else the
//! charstrings'. The wrapper is for looking at only: its `OS/2` table says it may not be
//! embedded, so no edit writes it into a document ([`crate::embedding::validate`] refuses it).

use std::collections::{BTreeMap, HashMap};

use pdfcraft_cos::{DecodeBudget, Dict, Document, Object};
use skrifa::GlyphId;
use skrifa::raw::ps::cff::CffFontRef;
use skrifa::raw::ps::cs::CommandSink;
use skrifa::raw::ps::string::Sid;
use skrifa::raw::types::Fixed;

use super::{Metrics, glyph_unicode, identity_h, only_char};

/// The most glyphs a wrapped program may have (`maxp` and `hmtx` count them in 16 bits).
const MAX_GLYPHS: u32 = 65_535;
/// The most charstrings run for their advance: the glyphs the PDF's `/Widths` don't reach.
const MAX_MEASURED: usize = 4096;

/// Receives a charstring's outline and keeps none of it: only its advance is wanted.
struct NoOutline;

impl CommandSink for NoOutline {
    fn move_to(&mut self, _: Fixed, _: Fixed) {}
    fn line_to(&mut self, _: Fixed, _: Fixed) {}
    fn curve_to(&mut self, _: Fixed, _: Fixed, _: Fixed, _: Fixed, _: Fixed, _: Fixed) {}
    fn close(&mut self) {}
}

/// Whether `bytes` start as a CFF (version 1) program rather than an sfnt.
pub(super) fn is_bare_cff(bytes: &[u8]) -> bool {
    matches!(bytes, [1, _, header, offset_size, ..] if *header >= 4 && (1..=4).contains(offset_size))
}

/// The glyph names `/Differences` gives codes, and the table of the base encoding the font's
/// `/Encoding` names (`None` when it names none: the program's own encoding is the base).
fn pdf_encoding(doc: &Document, font: &Dict) -> (Option<&'static [u32; 256]>, HashMap<u8, Vec<u8>>) {
    let table = |name: &[u8]| -> Option<&'static [u32; 256]> {
        match name {
            b"WinAnsiEncoding" => Some(&crate::encodings::WIN_ANSI),
            b"MacRomanEncoding" => Some(&crate::encodings::MAC_ROMAN),
            b"StandardEncoding" => Some(&crate::encodings::STANDARD),
            _ => None,
        }
    };
    let mut names = HashMap::new();
    let Some(encoding) = font.get(b"Encoding").map(|e| doc.resolve(e)) else { return (None, names) };
    match encoding.as_ref() {
        Object::Name(name) => (table(name), names),
        Object::Dict(dict) => {
            let base = dict.name(b"BaseEncoding").and_then(table);
            if let Some(differences) = dict.get(b"Differences").map(|d| doc.resolve(d)).and_then(|d| d.as_array().cloned()) {
                let mut code = 0u32;
                for item in differences.iter().take(4096) {
                    match item {
                        Object::Int(n) => code = u32::try_from((*n).max(0)).unwrap_or(u32::MAX),
                        Object::Name(name) => {
                            if let Ok(code) = u8::try_from(code) {
                                names.insert(code, name.clone());
                            }
                            code = code.saturating_add(1);
                        }
                        _ => {}
                    }
                }
            }
            (base, names)
        }
        _ => (None, names),
    }
}

/// How well a glyph's name says it is the glyph for a character: several glyphs can stand for one
/// (`one` and `one.numr`, `slash` and a `fraction` the PDF reads as `/`), and the plain one is
/// what typing that character should draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Fit {
    /// The name is the character's own (`one`, `uni0031`).
    Named,
    /// A variant of it (`one.numr`), or a name read as it.
    Variant,
    /// Only the PDF says so (`/ToUnicode`), or the glyph has no name.
    Told,
}

/// How well `name` fits `ch` ([`Fit`]).
fn fit(name: Option<&[u8]>, ch: char) -> Fit {
    let Some(name) = name.and_then(|n| std::str::from_utf8(n).ok()) else { return Fit::Told };
    let own = crate::encodings::NAMES.binary_search_by(|(n, _)| (*n).cmp(name)).ok().and_then(|i| crate::encodings::NAMES.get(i)).map(|(_, u)| *u);
    if own == Some(u32::from(ch)) || (!name.contains('.') && !name.contains('_') && own.is_none() && glyph_unicode(name) == Some(ch)) {
        Fit::Named
    } else if glyph_unicode(name) == Some(ch) {
        Fit::Variant
    } else {
        Fit::Told
    }
}

/// Unicode → glyph and glyph → advance (font units) as the PDF font `font` shows the program.
struct Mapped {
    unicode: BTreeMap<u32, (u32, Fit)>,
    advances: HashMap<u32, f64>,
}

impl Mapped {
    fn new() -> Self {
        Self { unicode: BTreeMap::new(), advances: HashMap::new() }
    }

    /// `code` draws `glyph` (named `name`): its `/Widths` advance and what it means are kept.
    fn add(&mut self, metrics: &Metrics, code: u32, glyph: u32, name: Option<&[u8]>, glyphs: u32, upem: f64) {
        if glyph == 0 || glyph >= glyphs {
            return;
        }
        if let Some(width) = pdf_advance(metrics, code, upem) {
            self.advances.entry(glyph).or_insert(width);
        }
        if let Some(ch) = metrics.text_of(code).as_deref().and_then(only_char) {
            self.map(ch, glyph, fit(name, ch));
        }
    }

    /// `ch` is drawn by `glyph`, unless a glyph that fits it better already is (the first of equals stays).
    fn map(&mut self, ch: char, glyph: u32, fit: Fit) {
        // U+FFFD says "no meaning known", and a control character is not typed.
        if ch == '\u{FFFD}' || ch.is_control() {
            return;
        }
        let entry = self.unicode.entry(u32::from(ch)).or_insert((glyph, fit));
        if fit < entry.1 {
            *entry = (glyph, fit);
        }
    }
}

/// The advance (font units) the PDF gives `code`. A simple font's `/Widths` are the page's
/// advances for the codes they cover; a code outside them has none to give (its `/MissingWidth`
/// is not the glyph's own).
fn pdf_advance(metrics: &Metrics, code: u32, upem: f64) -> Option<f64> {
    let width = if metrics.composite {
        metrics.width(code)
    } else {
        let index = usize::try_from(code.checked_sub(metrics.first)?).ok()?;
        metrics.widths.get(index)? * metrics.scale
    } * upem;
    (width.is_finite() && (0.0..=f64::from(u16::MAX)).contains(&width)).then_some(width)
}

/// A simple font: each code draws the glyph its `/Differences` name or base encoding names, else
/// the one the program's own encoding gives it. Glyphs no code reaches are found by name.
fn map_simple(doc: &Document, font: &Dict, face: &CffFontRef<'_>, metrics: &Metrics, glyphs: u32, upem: f64) -> Mapped {
    let mut names: Vec<Option<&[u8]>> = vec![None; usize::try_from(glyphs).unwrap_or(0)];
    let mut by_name: HashMap<&[u8], u32> = HashMap::new();
    if !face.is_cid()
        && let Some(charset) = face.charset()
    {
        for glyph in 1..glyphs {
            let Some(name) = charset.string_id(GlyphId::new(glyph)).ok().and_then(|sid| face.string(sid)) else { continue };
            by_name.entry(name).or_insert(glyph);
            if let Some(slot) = usize::try_from(glyph).ok().and_then(|g| names.get_mut(g)) {
                *slot = Some(name);
            }
        }
    }
    let name_of = |glyph: u32| usize::try_from(glyph).ok().and_then(|g| names.get(g).copied().flatten());
    // A base encoding names a glyph for each code (Annex D): the glyph of that name draws it.
    let named =
        |ch: char| crate::encodings::NAMES.iter().filter(|(_, u)| *u == u32::from(ch)).find_map(|(name, _)| by_name.get(name.as_bytes()).copied());
    let (base, differences) = pdf_encoding(doc, font);
    let own = face.encoding();
    let mut mapped = Mapped::new();
    for code in 0..=u8::MAX {
        let glyph = match differences.get(&code) {
            Some(name) => by_name.get(name.as_slice()).copied(),
            None => match base {
                Some(table) => table.get(usize::from(code)).and_then(|u| char::from_u32(*u)).filter(|c| *c != '\0').and_then(named),
                None => own.as_ref().and_then(|e| e.map(code)).map(|g| g.to_u32()),
            },
        };
        if let Some(glyph) = glyph {
            mapped.add(metrics, u32::from(code), glyph, name_of(glyph), glyphs, upem);
        }
    }
    // Glyphs no code reaches (a complete program has many) answer to their names.
    for glyph in 1..glyphs {
        let Some(name) = name_of(glyph) else { continue };
        if let Some(ch) = std::str::from_utf8(name).ok().and_then(glyph_unicode) {
            mapped.map(ch, glyph, fit(Some(name), ch));
        }
    }
    mapped
}

/// A composite font written with `Identity-H`: each two-byte code is a CID, which the program's
/// charset maps to a glyph (a program that isn't CID-keyed numbers its glyphs by CID).
fn map_composite(face: &CffFontRef<'_>, metrics: &Metrics, glyphs: u32, upem: f64) -> Mapped {
    let charset = face.is_cid().then(|| face.charset()).flatten();
    let mut mapped = Mapped::new();
    for code in 0..=u16::MAX {
        if metrics.text_of(u32::from(code)).is_none() {
            continue;
        }
        let glyph = match &charset {
            Some(charset) => charset.glyph_id(Sid::new(code)).ok().map(|g| g.to_u32()),
            None => (!face.is_cid()).then_some(u32::from(code)),
        };
        if let Some(glyph) = glyph {
            mapped.add(metrics, u32::from(code), glyph, None, glyphs, upem);
        }
    }
    mapped
}

/// The advance (font units) of each glyph. A glyph a character reaches takes its charstring's own
/// advance, for up to [`MAX_MEASURED`] of them: a subset's `/Widths` often carry one filler width
/// for the codes its text doesn't use, and typing such a character must not take the filler.
/// Otherwise the PDF's advance, else half an em.
fn advances(face: &CffFontRef<'_>, mapped: &Mapped, glyphs: u32, upem: f64) -> Vec<u16> {
    let reachable: std::collections::HashSet<u32> = mapped.unicode.values().map(|(glyph, _)| *glyph).collect();
    let mut subfonts = HashMap::new();
    let mut measured = 0usize;
    (0..glyphs)
        .map(|glyph| {
            let mut own = || {
                if !reachable.contains(&glyph) || measured >= MAX_MEASURED {
                    return None;
                }
                measured += 1;
                let index = face.subfont_index(GlyphId::new(glyph))?;
                if !subfonts.contains_key(&index) {
                    subfonts.insert(index, face.subfont(index, &[]).ok()?);
                }
                let width = face.evaluate_charstring(subfonts.get(&index)?, GlyphId::new(glyph), &[], &mut NoOutline).ok()??;
                Some(f64::from(width.to_f32()))
            };
            let width = own().or_else(|| mapped.advances.get(&glyph).copied());
            let width = width.filter(|w| w.is_finite()).unwrap_or(upem / 2.0);
            width.round().clamp(0.0, f64::from(u16::MAX)) as u16
        })
        .collect()
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        for (to, from) in word.iter_mut().zip(chunk) {
            *to = *from;
        }
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// `value` font units as a 16-bit signed table field.
fn units(value: f64) -> [u8; 2] {
    (value.round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16).to_be_bytes()
}

/// The `cmap` table: one Unicode (3, 10) format 12 subtable, a group per character.
fn cmap_table(unicode: &BTreeMap<u32, (u32, Fit)>) -> Option<Vec<u8>> {
    let count = u32::try_from(unicode.len()).ok()?;
    let mut cmap = Vec::with_capacity(28 + unicode.len() * 12);
    cmap.extend_from_slice(&[0, 0, 0, 1, 0, 3, 0, 10, 0, 0, 0, 12, 0, 12, 0, 0]);
    cmap.extend_from_slice(&count.checked_mul(12)?.checked_add(16)?.to_be_bytes());
    cmap.extend_from_slice(&0u32.to_be_bytes());
    cmap.extend_from_slice(&count.to_be_bytes());
    for (&ch, &(glyph, _)) in unicode {
        cmap.extend_from_slice(&ch.to_be_bytes());
        cmap.extend_from_slice(&ch.to_be_bytes());
        cmap.extend_from_slice(&glyph.to_be_bytes());
    }
    Some(cmap)
}

/// The sfnt (`OTTO`) holding `cff` with the tables a text renderer needs, for showing the PDF font
/// `font` in the inline editor. `None` when the program can't be read, maps no character, or its
/// metrics would be read past `budget`'s allowance.
pub(super) fn editor_sfnt(doc: &Document, font: &Dict, cff: &[u8], budget: &mut DecodeBudget) -> Option<Vec<u8>> {
    let face = CffFontRef::new_cff(cff, 0, None).ok()?;
    let glyphs = face.num_glyphs();
    if glyphs == 0 || glyphs > MAX_GLYPHS {
        return None;
    }
    let upem = u16::try_from(face.upem()).ok().filter(|u| (16..=16_384).contains(u))?;
    let em = f64::from(upem);
    let metrics = Metrics::from_dict_within(doc, font, budget);
    if metrics.past_allowance {
        return None;
    }
    let mapped = if metrics.composite {
        if !identity_h(doc, font) {
            return None;
        }
        map_composite(&face, &metrics, glyphs, em)
    } else {
        map_simple(doc, font, &face, &metrics, glyphs, em)
    };
    if mapped.unicode.is_empty() {
        return None;
    }
    let advances = advances(&face, &mapped, glyphs, em);
    let glyph_count = u16::try_from(glyphs).ok()?;
    // The descriptor's box, in font units; a font that gives none stands on the usual one.
    let ascent = if metrics.ascent.is_finite() && metrics.ascent > 0.0 { metrics.ascent } else { 0.8 } * em;
    let descent = if metrics.descent.is_finite() && metrics.descent < 0.0 { metrics.descent } else { -0.2 } * em;
    let widest = advances.iter().copied().max().unwrap_or(upem);
    let average = advances.iter().map(|w| u64::from(*w)).sum::<u64>() / u64::from(glyphs);

    let mut head = Vec::with_capacity(54);
    head.extend_from_slice(&[0, 1, 0, 0, 0, 1, 0, 0]); // version 1.0, fontRevision 1.0
    head.extend_from_slice(&[0; 4]); // checksumAdjustment: nothing here verifies it
    head.extend_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
    head.extend_from_slice(&3u16.to_be_bytes()); // baseline and left side bearing at 0
    head.extend_from_slice(&upem.to_be_bytes());
    head.extend_from_slice(&[0; 16]); // created, modified
    for value in [0.0, descent, f64::from(widest), ascent] {
        head.extend_from_slice(&units(value));
    }
    let style = u16::from(metrics.bold) | (u16::from(metrics.italic) << 1);
    head.extend_from_slice(&style.to_be_bytes()); // macStyle
    head.extend_from_slice(&8u16.to_be_bytes()); // lowestRecPPEM
    head.extend_from_slice(&[0, 2, 0, 0, 0, 0]); // direction hint, loca format, glyph format

    let mut hhea = Vec::with_capacity(36);
    hhea.extend_from_slice(&[0, 1, 0, 0]);
    hhea.extend_from_slice(&units(ascent));
    hhea.extend_from_slice(&units(descent));
    hhea.extend_from_slice(&[0, 0]); // lineGap
    hhea.extend_from_slice(&widest.to_be_bytes());
    hhea.extend_from_slice(&[0; 4]); // min side bearings
    hhea.extend_from_slice(&units(f64::from(widest))); // xMaxExtent
    hhea.extend_from_slice(&[0, 1, 0, 0, 0, 0]); // caret slope 1/0, offset 0
    hhea.extend_from_slice(&[0; 8]); // reserved
    hhea.extend_from_slice(&[0, 0]); // metricDataFormat
    hhea.extend_from_slice(&glyph_count.to_be_bytes());

    let mut maxp = vec![0, 0, 0x50, 0]; // version 0.5: a CFF font gives only its glyph count
    maxp.extend_from_slice(&glyph_count.to_be_bytes());

    let mut hmtx = Vec::with_capacity(advances.len() * 4);
    for advance in &advances {
        hmtx.extend_from_slice(&advance.to_be_bytes());
        hmtx.extend_from_slice(&[0, 0]);
    }

    let mut os2 = Vec::with_capacity(96);
    os2.extend_from_slice(&3u16.to_be_bytes()); // version
    os2.extend_from_slice(&units(average as f64));
    os2.extend_from_slice(&(if metrics.bold { 700u16 } else { 400 }).to_be_bytes());
    os2.extend_from_slice(&5u16.to_be_bytes()); // usWidthClass: medium
    // fsType: restricted. This copy is for the editor to draw with, never to embed again.
    os2.extend_from_slice(&2u16.to_be_bytes());
    os2.extend_from_slice(&[0; 20]); // subscript, superscript and strikeout boxes
    os2.extend_from_slice(&[0; 2]); // sFamilyClass
    os2.extend_from_slice(&[0; 10]); // panose
    os2.extend_from_slice(&[0; 16]); // ulUnicodeRange
    os2.extend_from_slice(b"    "); // achVendID
    let selection = if metrics.bold || metrics.italic { (u16::from(metrics.bold) << 5) | u16::from(metrics.italic) } else { 0x40 };
    os2.extend_from_slice(&selection.to_be_bytes());
    let first = mapped.unicode.keys().next().copied().unwrap_or(0).min(0xFFFF) as u16;
    let last = mapped.unicode.keys().next_back().copied().unwrap_or(0).min(0xFFFF) as u16;
    os2.extend_from_slice(&first.to_be_bytes());
    os2.extend_from_slice(&last.to_be_bytes());
    os2.extend_from_slice(&units(ascent)); // sTypoAscender
    os2.extend_from_slice(&units(descent)); // sTypoDescender
    os2.extend_from_slice(&[0, 0]); // sTypoLineGap
    os2.extend_from_slice(&(ascent.round().clamp(0.0, f64::from(u16::MAX)) as u16).to_be_bytes()); // usWinAscent
    os2.extend_from_slice(&((-descent).round().clamp(0.0, f64::from(u16::MAX)) as u16).to_be_bytes()); // usWinDescent
    os2.extend_from_slice(&[0; 8]); // ulCodePageRange
    os2.extend_from_slice(&units(ascent * 0.6)); // sxHeight
    os2.extend_from_slice(&units(ascent * 0.9)); // sCapHeight
    os2.extend_from_slice(&[0; 6]); // default char, break char, max context

    let name = vec![0, 0, 0, 0, 0, 6]; // format 0, no records
    let mut post = vec![0, 3, 0, 0]; // version 3: no glyph names
    post.extend_from_slice(&[0; 28]);

    let cmap = cmap_table(&mapped.unicode)?;
    // Tags in ascending order, as the table directory is searched.
    let tables: [(&[u8; 4], &[u8]); 9] = [
        (b"CFF ", cff),
        (b"OS/2", &os2),
        (b"cmap", &cmap),
        (b"head", &head),
        (b"hhea", &hhea),
        (b"hmtx", &hmtx),
        (b"maxp", &maxp),
        (b"name", &name),
        (b"post", &post),
    ];
    let mut out = Vec::with_capacity(12 + tables.len() * 16 + tables.iter().map(|(_, t)| t.len() + 3).sum::<usize>());
    out.extend_from_slice(b"OTTO");
    out.extend_from_slice(&[0, 9, 0, 128, 0, 3, 0, 16]); // 9 tables: searchRange 128, entrySelector 3, rangeShift 16
    let mut offset = 12 + tables.len() * 16;
    for (tag, table) in &tables {
        out.extend_from_slice(*tag);
        out.extend_from_slice(&checksum(table).to_be_bytes());
        out.extend_from_slice(&u32::try_from(offset).ok()?.to_be_bytes());
        out.extend_from_slice(&u32::try_from(table.len()).ok()?.to_be_bytes());
        offset = offset.checked_add(table.len().checked_add(3)? & !3)?;
    }
    for (_, table) in &tables {
        out.extend_from_slice(table);
        out.resize(out.len().checked_add(3)? & !3, 0);
    }
    use skrifa::MetadataProvider as _;
    skrifa::FontRef::new(&out).is_ok_and(|font| font.charmap().mappings().next().is_some()).then_some(out)
}

#[cfg(test)]
mod tests {
    use pdfcraft_cos::{Dict, Document, Object, Stream};
    use skrifa::MetadataProvider as _;

    use super::*;

    /// A Type 2 charstring (or DICT) operand.
    fn num(v: i32) -> Vec<u8> {
        match v {
            -107..=107 => vec![(v + 139) as u8],
            108..=1131 => vec![((v - 108) >> 8) as u8 + 247, ((v - 108) & 255) as u8],
            _ => panic!("operand {v} out of this helper's range"),
        }
    }

    /// A DICT offset operand of fixed length, so the Top DICT's size doesn't depend on its values.
    fn offset(v: usize) -> Vec<u8> {
        [vec![29], (v as i32).to_be_bytes().to_vec()].concat()
    }

    /// A glyph: its advance, then a triangle from (`x`, 0).
    fn glyph(advance: i32, x: i32) -> Vec<u8> {
        [num(advance), num(x), num(0), vec![21], num(200), num(0), vec![5], num(0), num(200), vec![5], vec![14]].concat()
    }

    /// A bare CFF program of four glyphs: `.notdef`, `A` (advance 500), `B` (600) and `space`
    /// (250), with an encoding of its own that puts them at codes 65, 66 and 32.
    fn bare_cff() -> Vec<u8> {
        // SIDs of the standard strings `A`, `B` and `space`.
        bare_cff_named([34, 35, 1])
    }

    /// [`bare_cff`] with its three glyphs named by these standard string ids instead.
    fn bare_cff_named(sids: [u8; 3]) -> Vec<u8> {
        let charstrings = [vec![14], glyph(500, 100), glyph(600, 50), [num(250), vec![14]].concat()];
        let mut index = vec![0, charstrings.len() as u8, 1, 1];
        let mut end = 1usize;
        for cs in &charstrings {
            end += cs.len();
            index.push(end as u8);
        }
        index.extend(charstrings.concat());
        let private = [num(400), vec![20]].concat(); // defaultWidthX
        let header = [1, 0, 4, 1];
        let names = [vec![0, 1, 1, 1, 5], b"Test".to_vec()].concat();
        let charset = [0, 0, sids[0], 0, sids[1], 0, sids[2]];
        let encoding = [0, 3, 65, 66, 32];
        let top_len = 29;
        let charset_at = header.len() + names.len() + 5 + top_len + 2 + 2;
        let encoding_at = charset_at + charset.len();
        let charstrings_at = encoding_at + encoding.len();
        let private_at = charstrings_at + index.len();
        let top = [
            offset(charset_at),
            vec![15],
            offset(encoding_at),
            vec![16],
            offset(charstrings_at),
            vec![17],
            offset(private.len()),
            offset(private_at),
            vec![18],
        ]
        .concat();
        assert_eq!(top.len(), top_len);
        [
            header.to_vec(),
            names,
            vec![0, 1, 1, 1, 1 + top_len as u8],
            top,
            vec![0, 0],
            vec![0, 0],
            charset.to_vec(),
            encoding.to_vec(),
            index,
            private,
        ]
        .concat()
    }

    /// A simple font over `program`, with `/Widths` for codes 32 to 66 (`widths` gives code 65 and 66).
    fn type1c(doc: &mut Document, program: &[u8], encoding: Option<Object>, widths: [i64; 2]) -> Dict {
        let mut stream = Dict::new();
        stream.set(b"Subtype".to_vec(), Object::name("Type1C"));
        let mut descriptor = Dict::new();
        descriptor.set(b"FontName".to_vec(), Object::name("ABCDEF+Test"));
        descriptor.set(b"Flags".to_vec(), Object::Int(32));
        descriptor.set(b"Ascent".to_vec(), Object::Int(750));
        descriptor.set(b"Descent".to_vec(), Object::Int(-250));
        descriptor.set(b"FontFile3".to_vec(), Object::Ref(doc.add(Object::Stream(Stream::flate(stream, program)))));
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name("ABCDEF+Test"));
        font.set(b"FirstChar".to_vec(), Object::Int(32));
        font.set(b"LastChar".to_vec(), Object::Int(66));
        let mut all = vec![Object::Int(0); 35];
        all[0] = Object::Int(250);
        all[33] = Object::Int(widths[0]);
        all[34] = Object::Int(widths[1]);
        font.set(b"Widths".to_vec(), Object::Array(all));
        if let Some(encoding) = encoding {
            font.set(b"Encoding".to_vec(), encoding);
        }
        font.set(b"FontDescriptor".to_vec(), Object::Ref(doc.add(Object::Dict(descriptor))));
        font
    }

    fn advance(face: &skrifa::FontRef<'_>, glyph: u32) -> f32 {
        face.glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default()).advance_width(GlyphId::new(glyph)).unwrap()
    }

    #[test]
    fn a_bare_cff_program_reads_as_an_sfnt_with_the_pages_glyphs_and_widths() {
        let mut doc = Document::new_empty();
        let font = type1c(&mut doc, &bare_cff(), Some(Object::name("WinAnsiEncoding")), [510, 600]);
        let sfnt = crate::pdf::embedded_sfnt(&doc, &font).expect("the editor can read a /Type1C program");
        let face = skrifa::FontRef::new(&sfnt).unwrap();
        let glyph = |c| face.charmap().map(c).map(|g| g.to_u32());
        assert_eq!((glyph('A'), glyph('B'), glyph(' '), glyph('C')), (Some(1), Some(2), Some(3), None));
        // Each glyph a character reaches keeps its own advance: a subset's /Widths (510 here) may
        // be a filler for the codes its text doesn't use.
        assert_eq!((advance(&face, 1), advance(&face, 2), advance(&face, 3)), (500.0, 600.0, 250.0));
        // The outlines are the program's own, unchanged.
        assert!(sfnt.windows(bare_cff().len()).any(|w| w == bare_cff()));
        let outline = face.outline_glyphs().get(GlyphId::new(1)).expect("glyph A has an outline");
        let mut path: Vec<skrifa::outline::pen::PathElement> = Vec::new();
        outline.draw(skrifa::instance::Size::unscaled(), &mut path).unwrap();
        assert!(path.len() >= 3, "{path:?}");
        // For looking at only: an edit never embeds this copy.
        assert!(crate::embedding::validate(&sfnt).is_err());
    }

    #[test]
    fn codes_draw_what_the_pdf_encoding_names_and_mean_what_tounicode_says() {
        // /Differences puts glyph B at code 65: the letter B is drawn by glyph 2.
        let mut doc = Document::new_empty();
        let mut encoding = Dict::new();
        encoding.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(65), Object::name("B")]));
        let font = type1c(&mut doc, &bare_cff(), Some(Object::Dict(encoding)), [640, 0]);
        let sfnt = crate::pdf::embedded_sfnt(&doc, &font).unwrap();
        let face = skrifa::FontRef::new(&sfnt).unwrap();
        assert_eq!(face.charmap().map('B').map(|g| g.to_u32()), Some(2));
        assert_eq!(advance(&face, 2), 600.0);
        // Glyph A is reached by no code now: its name still finds it, with its charstring's advance.
        assert_eq!(face.charmap().map('A').map(|g| g.to_u32()), Some(1));
        assert_eq!(advance(&face, 1), 500.0);

        // No /Encoding: the program's own puts glyph 1 at code 65, and /ToUnicode says what it means.
        let mut doc = Document::new_empty();
        let mut font = type1c(&mut doc, &bare_cff(), None, [500, 600]);
        let cmap = b"1 begincodespacerange <00> <FF> endcodespacerange 1 beginbfchar <41> <03A9> endbfchar";
        font.set(b"ToUnicode".to_vec(), Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), cmap)))));
        let sfnt = crate::pdf::embedded_sfnt(&doc, &font).unwrap();
        let face = skrifa::FontRef::new(&sfnt).unwrap();
        assert_eq!(face.charmap().map('\u{3A9}').map(|g| g.to_u32()), Some(1));
        assert_eq!(face.charmap().map('B').map(|g| g.to_u32()), Some(2));
    }

    #[test]
    fn a_character_two_glyphs_stand_for_is_drawn_by_the_one_named_for_it() {
        // Glyph 1 is `fraction`, glyph 2 `slash` (standard strings 99 and 16); the PDF reads both
        // codes as `/`, as a file that sets "1/8" with a fraction bar does. Typing `/` draws the
        // slash, though the fraction comes first.
        let mut doc = Document::new_empty();
        let mut font = type1c(&mut doc, &bare_cff_named([99, 16, 1]), None, [500, 600]);
        let cmap = b"1 begincodespacerange <00> <FF> endcodespacerange 2 beginbfchar <41> <002F> <42> <002F> endbfchar";
        font.set(b"ToUnicode".to_vec(), Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), cmap)))));
        let sfnt = crate::pdf::embedded_sfnt(&doc, &font).unwrap();
        let face = skrifa::FontRef::new(&sfnt).unwrap();
        assert_eq!(face.charmap().map('/').map(|g| g.to_u32()), Some(2));
        // A base encoding names its glyphs: WinAnsi's code 47 is `slash`, whatever else reads as `/`.
        let mut doc = Document::new_empty();
        let font = type1c(&mut doc, &bare_cff_named([99, 16, 1]), Some(Object::name("WinAnsiEncoding")), [500, 600]);
        let sfnt = crate::pdf::embedded_sfnt(&doc, &font).unwrap();
        let face = skrifa::FontRef::new(&sfnt).unwrap();
        assert_eq!(face.charmap().map('/').map(|g| g.to_u32()), Some(2));
        assert_eq!(face.charmap().map('\u{2044}').map(|g| g.to_u32()), Some(1), "the fraction bar keeps its own character");
        // "No meaning known" and control characters are never typed, so they map nothing.
        assert_eq!(fit(Some(b"one"), '1'), Fit::Named);
        assert_eq!(fit(Some(b"one.numr"), '1'), Fit::Variant);
        assert_eq!(fit(Some(b"uni0031"), '1'), Fit::Named);
        assert_eq!(fit(Some(b"fraction"), '/'), Fit::Told);
        assert_eq!(fit(None, '1'), Fit::Told);
        let mut mapped = Mapped::new();
        mapped.map('\u{FFFD}', 1, Fit::Named);
        mapped.map('\u{1}', 1, Fit::Named);
        assert!(mapped.unicode.is_empty());
    }

    #[test]
    fn a_composite_font_maps_its_cids_through_identity_h_only() {
        let composite = |doc: &mut Document, encoding: &str| {
            let mut stream = Dict::new();
            stream.set(b"Subtype".to_vec(), Object::name("CIDFontType0C"));
            let mut descriptor = Dict::new();
            descriptor.set(b"FontFile3".to_vec(), Object::Ref(doc.add(Object::Stream(Stream::flate(stream, &bare_cff())))));
            let mut descendant = Dict::new();
            descendant.set(b"Subtype".to_vec(), Object::name("CIDFontType0"));
            descendant.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
            descendant.set(b"W".to_vec(), Object::Array(vec![Object::Int(2), Object::Array(vec![Object::Int(620)])]));
            let cmap = b"1 begincodespacerange <0000> <FFFF> endcodespacerange 2 beginbfchar <0001> <0041> <0002> <0042> endbfchar";
            let mut font = Dict::new();
            font.set(b"Type".to_vec(), Object::name("Font"));
            font.set(b"Subtype".to_vec(), Object::name("Type0"));
            font.set(b"BaseFont".to_vec(), Object::name("ABCDEF+Test"));
            font.set(b"Encoding".to_vec(), Object::name(encoding));
            font.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(descendant)]));
            font.set(b"ToUnicode".to_vec(), Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), cmap)))));
            font
        };
        let mut doc = Document::new_empty();
        let font = composite(&mut doc, "Identity-H");
        let sfnt = crate::pdf::embedded_sfnt(&doc, &font).expect("a /CIDFontType0C program");
        let face = skrifa::FontRef::new(&sfnt).unwrap();
        // The program isn't CID-keyed, so a CID is its glyph's number.
        assert_eq!((face.charmap().map('A').map(|g| g.to_u32()), face.charmap().map('B').map(|g| g.to_u32())), (Some(1), Some(2)));
        assert_eq!(advance(&face, 2), 600.0);
        // Another CMap's codes aren't CIDs: nothing is guessed.
        let font = composite(&mut doc, "UniJIS-UTF16-H");
        assert!(crate::pdf::embedded_sfnt(&doc, &font).is_none());
    }

    #[test]
    fn damaged_programs_are_refused_without_panicking() {
        let whole = bare_cff();
        assert!(is_bare_cff(&whole));
        for not_cff in [&b""[..], b"OTTO\0\x09", b"\0\x01\0\0", b"ttcf", &[1, 0, 2, 1], &[1, 0, 4, 9]] {
            assert!(!is_bare_cff(not_cff), "{not_cff:?}");
        }
        // Every truncation, and every single byte overwritten, either wraps or is refused.
        for cut in 0..whole.len() {
            let mut doc = Document::new_empty();
            let font = type1c(&mut doc, &whole[..cut], Some(Object::name("WinAnsiEncoding")), [500, 600]);
            let _ = crate::pdf::embedded_sfnt(&doc, &font);
        }
        for at in 0..whole.len() {
            for value in [0u8, 0x7f, 0xff] {
                let mut damaged = whole.clone();
                damaged[at] = value;
                let mut doc = Document::new_empty();
                let font = type1c(&mut doc, &damaged, None, [500, 600]);
                if let Some(sfnt) = crate::pdf::embedded_sfnt(&doc, &font) {
                    assert!(skrifa::FontRef::new(&sfnt).is_ok());
                }
            }
        }
    }
}
