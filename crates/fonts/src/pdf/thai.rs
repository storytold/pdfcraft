//! Recover a narrowly identified legacy Thai exporter map, not arbitrary missing Unicode.
//!
//! Some THSarabunPSK Identity-H subsets retain original glyph IDs but write Latin/control
//! destinations for Thai in ToUnicode (including an accidental 00ff..01ff range). The intact
//! Thai anchors identify the original nominal Thai order. Only that complete fingerprint is
//! accepted, and then only when every glyph the subset kept advances as its assumed character
//! does (a mark takes none); correct maps and unrelated fonts are never changed. No font
//! artwork is bundled.

use super::{Dict, Document, Metrics, dict, only_char, unicode_map};
use pdfcraft_cos::{DecodeBudget, Object};
use std::collections::BTreeMap;

/// Recover the nominal Thai codes and the presentation marks of this damaged legacy export.
/// The caller must record the repair, and attach the map to a copy of the page's font.
pub fn recovered_thai_map(doc: &Document, font: &Dict) -> Option<Vec<(u16, char)>> {
    use skrifa::{MetadataProvider as _, raw::TableProvider as _};
    let base = String::from_utf8_lossy(font.name(b"BaseFont")?);
    let family = base.rsplit('+').next()?;
    if !matches!(family, "THSarabunPSK" | "THSarabunPSK-Bold" | "THSarabunPSK-Italic" | "THSarabunPSK-BoldItalic")
        || font.name(b"Subtype") != Some(b"Type0")
        || font.name(b"Encoding") != Some(b"Identity-H")
    {
        return None;
    }
    let desc = dict(doc, font.get(b"DescendantFonts").and_then(|a| doc.resolve(a).as_array().and_then(|a| a.first()).cloned()).as_ref())?;
    if desc.name(b"CIDToGIDMap") != Some(b"Identity") {
        return None;
    }
    let map = unicode_map(doc, font, true);
    // Five separated consonants and three contextual marks must agree, together with the
    // corrupt destinations and corrupt range. This is deliberately stricter than font-name
    // detection: a renamed/subsetted font with different glyph order must not be guessed.
    let fingerprint = [
        (217, '\u{0e04}'),
        (226, '\u{0e0d}'),
        (250, '\u{0e25}'),
        (256, '\u{0e2b}'),
        (258, '\u{0e2d}'),
        (356, '\u{0e4c}'),
        (358, '\u{0e31}'),
        (360, '\u{0e47}'),
        (214, '\u{000f}'),
        (240, ')'),
        (248, '#'),
        (261, '>'),
        (263, '@'),
        (265, '>'),
        (273, 'N'),
        (352, 'N'),
        (353, 'O'),
    ];
    let matches = |pairs: &[(u32, char)], last: u16| {
        pairs.iter().all(|&(code, ch)| map.text(code).is_some_and(|t| t.chars().eq([ch])))
            && map.ranges.iter().any(|r| r.lo == 255 && r.hi == 511 && r.units.as_ref() == [last])
    };
    // The same exporter emitted three distinct subset maps. Each must match its entire
    // intact-anchor and corrupt-destination fingerprint, including the oversized range.
    let body = matches(&fingerprint, 0x32);
    let attachment = matches(
        &[
            (217, '\u{0e04}'),
            (256, '\u{0e2b}'),
            (258, '\u{0e2d}'),
            (297, '\u{0e58}'),
            (347, '\u{0e48}'),
            (356, '\u{0e4c}'),
            (358, '\u{0e31}'),
            (360, '\u{0e47}'),
            (214, '\u{000f}'),
            (226, '\u{000f}'),
            (240, '\u{0027}'),
            (248, '/'),
            (250, '1'),
            (265, 'B'),
            (273, 'N'),
            (352, 'N'),
            (353, 'O'),
        ],
        0x32,
    );
    let bold = matches(
        &[
            (217, '\u{0e04}'),
            (226, '\u{0e0d}'),
            (232, '\u{0e13}'),
            (234, '\u{0e15}'),
            (236, '\u{0e17}'),
            (243, '\u{0e1e}'),
            (250, '\u{0e25}'),
            (256, '\u{0e2b}'),
            (258, '\u{0e2d}'),
            (356, '\u{0e4c}'),
            (214, '\u{000f}'),
            (240, '!'),
            (248, '1'),
            (263, '@'),
            (273, 'N'),
            (352, 'N'),
            (353, 'O'),
        ],
        0x30,
    );
    if !(body || attachment || bold) {
        return None;
    }
    let nominal = (214u16..=298).filter_map(|gid| {
        let unicode = u32::from(gid) + if gid <= 271 { 0xd2b } else { 0xd2f };
        char::from_u32(unicode).map(|ch| (gid, ch))
    });
    // Contextual marks have the same character as the nominal mark, with a moved outline.
    let contextual = [
        (343, '\u{0e34}'),
        (344, '\u{0e35}'),
        (347, '\u{0e48}'),
        (351, '\u{0e4c}'),
        (352, '\u{0e48}'),
        (353, '\u{0e49}'),
        (356, '\u{0e4c}'),
        (358, '\u{0e31}'),
        (360, '\u{0e47}'),
    ];
    // The anchors identify the exporter; the font program confirms each glyph's advance class
    // (not its identity, which nothing in such a subset records). Every glyph the
    // subset kept must advance as the character it is given (a vowel or tone mark set on its
    // base takes no advance, every other character does), else the order differs somewhere and
    // nothing is mapped. A glyph the subset dropped is left unmapped: no code can draw it, and
    // its advance is the subsetter's filler, not the glyph's.
    let program = embedded_truetype(doc, &desc)?;
    let face = skrifa::FontRef::new(&program).ok()?;
    let loca = face.loca(None).ok()?;
    let advances = face.glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
    let mut out = Vec::new();
    for (gid, ch) in nominal.chain(contextual) {
        let at = usize::from(gid);
        if loca.get_raw(at).zip(loca.get_raw(at + 1)).is_none_or(|(start, end)| end <= start) {
            continue;
        }
        if advances.advance_width(skrifa::GlyphId::new(u32::from(gid))).is_none_or(|w| (w == 0.0) != is_thai_mark(ch)) {
            return None;
        }
        out.push((gid, ch));
    }
    Some(out)
}

/// The character each legacy Windows Thai presentation form U+F700 + index stands for: ฐ and ญ
/// without descenders, and the vowels and tone marks moved left, lower or both (the PUA
/// assignments of <https://linux.thai.net/~thep/th-otf/shaping.html>).
const PUA_BASES: [char; 27] = ['ฐ', 'ิ', 'ี', 'ึ', 'ื', '่', '้', '๊', '๋', '์', '่', '้', '๊', '๋', '์', 'ญ', 'ั', 'ํ', '็', '่', '้', '๊', '๋', '์', 'ุ', 'ู', 'ฺ'];

/// Whether a Thai character is a vowel or tone mark set above or below its base, with no advance
/// of its own (U+0E31, U+0E34–U+0E3A, U+0E47–U+0E4E). Sara Am and the other vowels advance.
fn is_thai_mark(ch: char) -> bool {
    matches!(u32::from(ch), 0x0e31 | 0x0e34..=0x0e3a | 0x0e47..=0x0e4e)
}

/// The glyph a legacy THSarabunPSK program draws Thai character `ch` with: U+0E01–U+0E3A are
/// glyphs 214–271 and U+0E3F–U+0E5B glyphs 272–298 (the order `recovered_thai_map` assumes).
fn legacy_glyph(ch: u32) -> Option<u32> {
    match ch {
        0x0e01..=0x0e3a => ch.checked_sub(0xd2b),
        0x0e3f..=0x0e5b => ch.checked_sub(0xd2f),
        _ => None,
    }
}

/// The TrueType program (`/FontFile2`) of a CIDFont, decoded within the editor's size cap.
fn embedded_truetype(doc: &Document, descendant: &Dict) -> Option<Vec<u8>> {
    let descriptor = dict(doc, descendant.get(b"FontDescriptor"))?;
    let program = doc.resolve(descriptor.get(b"FontFile2")?);
    let Object::Stream(program) = program.as_ref() else { return None };
    DecodeBudget::new(super::MAX_SFNT, 0).decode_within(&doc.with_direct_parms(program), super::MAX_SFNT).ok()
}

/// Source glyph positions and the original PDF character codes that draw them. Empty codes
/// represent whitespace advances: no glyph is added to the font or substituted for a space.
pub struct ExistingThaiRun {
    pub shaped: crate::source_shaping::ShapedRun,
    pub codes: Vec<Vec<u8>>,
}

/// Unicode-to-glyph and glyph-to-PDF-code maps for a two-byte Identity-H TrueType font.
/// Used by the editor-only cmap adapter and by direct source-font shaping.
pub(super) fn composite_glyph_maps(
    doc: &Document,
    font: &Dict,
    bytes: &[u8],
    metrics: &Metrics,
    budget: &mut DecodeBudget,
) -> Option<(BTreeMap<u32, u32>, BTreeMap<u32, u16>)> {
    use skrifa::{MetadataProvider as _, raw::TableProvider as _};
    if font.name(b"Encoding") != Some(b"Identity-H") {
        return None;
    }
    let descendant = dict(doc, font.get(b"DescendantFonts").and_then(|a| doc.resolve(a).as_array().and_then(|a| a.first()).cloned()).as_ref())?;
    let cid_map = match descendant.get(b"CIDToGIDMap").map(|m| doc.resolve(m)).as_deref() {
        None => None,
        Some(Object::Name(n)) if n == b"Identity" => None,
        Some(Object::Stream(s)) => Some(budget.decode_within(&doc.with_direct_parms(s), 1 << 17).ok()?),
        _ => return None,
    };
    let face = skrifa::FontRef::new(bytes).ok()?;
    let loca = face.loca(None).ok()?;
    let advances = face.glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
    let outlined = |gid: u32| loca.get_raw(gid as usize).zip(loca.get_raw(gid as usize + 1)).is_some_and(|(a, b)| b > a);
    let glyph_of = |cid: u16| -> Option<u32> {
        match &cid_map {
            None => Some(u32::from(cid)),
            Some(map) => {
                let at = usize::from(cid).checked_mul(2)?;
                Some(u32::from(u16::from_be_bytes(map.get(at..at + 2)?.try_into().ok()?)))
            }
        }
    };
    let mut unicode = BTreeMap::new();
    let mut codes = BTreeMap::new();
    for code in 0..=u16::MAX {
        let Some(gid) = glyph_of(code) else { continue };
        if gid == 0 || gid as usize >= loca.len().saturating_sub(1) {
            continue;
        }
        let text = metrics.text_of(u32::from(code));
        let ch = text.as_deref().and_then(only_char);
        let space = ch == Some(' ') && advances.advance_width(skrifa::GlyphId::new(gid)).is_some_and(|w| w > 0.0);
        if !outlined(gid) && !space {
            continue;
        }
        codes.entry(gid).or_insert(code);
        if let Some(ch) = ch.filter(|c| !c.is_control()) {
            unicode.entry(u32::from(ch)).and_modify(|old: &mut u32| *old = (*old).min(gid)).or_insert(gid);
        }
    }
    // The verified THSarabunPSK glyph order is retained by these PDF subsets. Restore the
    // Windows Thai PUA alternates for HarfRust's legacy shaper, only where outlines survive.
    // See https://linux.thai.net/~thep/th-otf/shaping.html (legacy PUA assignments).
    let family = metrics.base_font.rsplit('+').next().unwrap_or(&metrics.base_font);
    let alternate_meanings = metrics.text_of(352).is_some_and(|t| t == "่") && metrics.text_of(353).is_some_and(|t| t == "้");
    // Each alternate the subset kept must advance as its form does: U+F700 and U+F70F are ฐ and
    // ญ without their descenders, the rest marks with no advance of their own. Any that doesn't
    // means another glyph order, and then none is used: the wrong mark would draw silently.
    let kept: Vec<(usize, u32)> = (342u32..=368).enumerate().filter(|&(_, gid)| outlined(gid)).collect();
    let in_order =
        kept.iter().all(|&(index, gid)| advances.advance_width(skrifa::GlyphId::new(gid)).is_some_and(|w| (w > 0.0) == matches!(index, 0 | 15)));
    // Where the PDF says what an alternate's code is (an intact ToUnicode gives the character it
    // stands for), it must be that alternate's base character: two alternates of the same kind
    // in another order pass the advance check, but not this one.
    let code_of = |gid: u32| match &cid_map {
        None => u16::try_from(gid).ok(),
        Some(_) => codes.get(&gid).copied(),
    };
    let meanings_agree = kept.iter().all(|&(index, gid)| {
        let stated = code_of(gid).and_then(|code| metrics.text_of(u32::from(code))).and_then(|t| only_char(&t));
        stated.filter(|c| matches!(u32::from(*c), 0x0e01..=0x0e5b)).is_none_or(|c| PUA_BASES.get(index) == Some(&c))
    });
    if family.starts_with("THSarabunPSK")
        && unicode.get(&0x0e01) == Some(&214)
        && unicode.get(&0x0e2d) == Some(&258)
        && (alternate_meanings || unicode.contains_key(&0xf70a))
        && in_order
        && meanings_agree
    {
        for (index, gid) in kept {
            let code = match &cid_map {
                None => u16::try_from(gid).ok(),
                Some(_) => codes.get(&gid).copied(),
            };
            if let Some(code) = code {
                unicode.insert(0xf700 + index as u32, gid);
                codes.entry(gid).or_insert(code);
            }
        }
    }
    Some((unicode, codes))
}

/// Shape Thai in a THSarabunPSK source font's own outlines and return codes for that same font.
/// No font program, family, weight or size is replaced. A program with OpenType Thai mark
/// positioning (a complete face loaded with Load font, say) is shaped by that layout; one
/// without it must be a verified legacy positional program, shaped with its nominal glyphs and
/// Windows PUA alternates. A subset missing a glyph the text needs is refused, so the caller can
/// try another subset of the same face or ask for its full font.
pub struct LegacyThaiFont {
    bytes: Vec<u8>,
    codes: BTreeMap<u32, u16>,
    space: f64,
    /// The program positions Thai marks itself (GPOS `mark`/`mkmk`).
    positioned: bool,
}

impl LegacyThaiFont {
    /// Read and verify the source font once for all wrapping/preview measurements. `None` for
    /// another family, a font with no Thai, and a program without Thai mark positioning whose
    /// Thai glyphs are not in the legacy order: the nominal and alternate marks this shaper
    /// assumes can't be trusted there, so it is shaped as an ordinary source font.
    ///
    /// Its streams are decoded against `budget`, which the caller shares between every font it
    /// reads: what they keep is the programs decoded and the editor cmaps added to them.
    pub fn from_pdf(doc: &Document, font: &Dict, space: f64, budget: &mut DecodeBudget) -> Result<Option<Self>, String> {
        let m = Metrics::from_dict(doc, font);
        if !m.base_font.rsplit('+').next().unwrap_or(&m.base_font).starts_with("THSarabunPSK") {
            return Ok(None);
        }
        if !space.is_finite() || space < 0.0 {
            return Err("the source space width is invalid".into());
        }
        let bytes = super::embedded_sfnt_within(doc, font, budget).ok_or("the source Thai font has no readable glyph program")?;
        let positioned = crate::source_shaping::has_thai_layout(&bytes);
        let (unicode, codes) = composite_glyph_maps(doc, font, &bytes, &m, budget).ok_or("the Thai source glyph codes cannot be recovered safely")?;
        // A subset keeps only the glyphs its document used, so no particular letter or alternate
        // is required here: [`Self::shape`] refuses a text whose glyphs, or required alternates
        // ([`require_pua`]), the subset lacks. Without its own positioning, every Thai glyph it
        // does keep must be where the legacy order puts that character, or one of the PUA
        // alternates for a mark (or for ฐ and ญ without their descenders).
        let mut thai = unicode.range(0x0e01..=0x0e5b).peekable();
        if thai.peek().is_none() {
            return Ok(None);
        }
        let usable = positioned
            || thai.all(|(&ch, &gid)| {
                legacy_glyph(ch) == Some(gid)
                    || ((342..=368).contains(&gid) && (char::from_u32(ch).is_some_and(is_thai_mark) || matches!(ch, 0x0e0d | 0x0e10)))
            });
        Ok(usable.then_some(Self { bytes, codes, space, positioned }))
    }

    /// Shape into glyphs that are already in this exact source face.
    pub fn shape(&self, text: &str) -> Result<ExistingThaiRun, String> {
        use skrifa::MetadataProvider as _;
        if text.len() > 262_144 || text.contains(['\n', '\r']) {
            return Err("shape one bounded Thai source row at a time".into());
        }
        // Only the legacy shaper needs the PUA alternates; a positioned program moves its marks.
        if !self.positioned {
            let face = skrifa::FontRef::new(&self.bytes).map_err(|e| e.to_string())?;
            require_pua(&face.charmap(), text)?;
        }
        let mut shaped = crate::source_shaping::ShapedRun { glyphs: Vec::new(), advance: 0.0 };
        let mut codes = Vec::new();
        // Legacy PDF subsets often omit the space glyph. Keep its measured PDF advance as a
        // positioning gap, rather than drawing .notdef or changing to another font.
        for piece in text.split_inclusive(' ') {
            let word = piece.strip_suffix(' ').unwrap_or(piece);
            if !word.is_empty() {
                let run = if self.positioned {
                    crate::source_shaping::shape(&self.bytes, word)?
                } else {
                    crate::source_shaping::shape_legacy_thai(&self.bytes, word)?
                };
                for mut glyph in run.glyphs {
                    let code = self
                        .codes
                        .get(&glyph.id.to_u32())
                        .ok_or("the Thai subset lacks a required contextual glyph; load the complete original face")?;
                    glyph.position[0] += shaped.advance;
                    codes.push(code.to_be_bytes().to_vec());
                    shaped.glyphs.push(glyph);
                }
                shaped.advance += run.advance;
            }
            if piece.ends_with(' ') {
                shaped.glyphs.push(crate::source_shaping::ShapedGlyph {
                    id: skrifa::GlyphId::NOTDEF,
                    position: [shaped.advance, 0.0],
                    text: " ".into(),
                });
                codes.push(Vec::new());
                shaped.advance += self.space;
            }
            if shaped.glyphs.len() > 65_534 || !shaped.advance.is_finite() {
                return Err("Thai source row exceeds the shaping budget".into());
            }
        }
        Ok(ExistingThaiRun { shaped, codes })
    }
}

/// Verify every context-dependent PUA alternate before asking the legacy shaper to use it.
/// Otherwise a subset missing an alternate would silently draw the wrong high/left mark.
fn require_pua(cmap: &skrifa::charmap::Charmap<'_>, text: &str) -> Result<(), String> {
    let mut base = 0u32;
    let mut above = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let c = u32::from(ch);
        let ascender = matches!(base, 0xe1b | 0xe1d | 0xe1f);
        let pua = match c {
            0xe31 if ascender => Some(0xf710),
            0xe34..=0xe37 if ascender => Some(0xf701 + c - 0xe34),
            0xe47 if ascender => Some(0xf712),
            0xe4d if ascender => Some(0xf711),
            0xe48..=0xe4c => {
                let high = above || chars.peek().is_some_and(|c| *c == 'ำ');
                if ascender {
                    Some((if high { 0xf713 } else { 0xf705 }) + c - 0xe48)
                } else if !high {
                    Some(0xf70a + c - 0xe48)
                } else {
                    None
                }
            }
            0xe38..=0xe3a if matches!(base, 0xe0e | 0xe0f) => Some(0xf718 + c - 0xe38),
            0xe38..=0xe3a if base == 0xe0d => Some(0xf70f),
            0xe38..=0xe3a if base == 0xe10 => Some(0xf700),
            0xe33 if ascender => Some(0xf711),
            _ => None,
        };
        if pua.is_some_and(|p| char::from_u32(p).is_none_or(|c| cmap.map(c).is_none())) {
            return Err(format!(
                "the Thai subset lacks a required contextual mark for {ch} (U+{c:04X}); load the complete original face or explicitly select a replacement font"
            ));
        }
        if matches!(c, 0xe01..=0xe2e) {
            base = c;
            above = false;
        } else if matches!(c, 0xe31 | 0xe34..=0xe37 | 0xe47 | 0xe4d..=0xe4e) {
            above = true;
        } else if !matches!(c, 0xe38..=0xe3a | 0xe48..=0xe4c) {
            base = 0;
            above = false;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{LegacyThaiFont, Metrics, composite_glyph_maps, is_thai_mark, recovered_thai_map};
    use pdfcraft_cos::{DecodeBudget, Dict, Document, Object, Stream};
    use std::collections::HashMap;

    /// Where table `tag` starts in an sfnt.
    fn table(program: &[u8], tag: &[u8; 4]) -> usize {
        let count = u16::from_be_bytes([program[4], program[5]]) as usize;
        let record = (0..count).map(|i| 12 + i * 16).find(|&r| &program[r..r + 4] == tag).unwrap();
        u32::from_be_bytes(program[record + 8..record + 12].try_into().unwrap()) as usize
    }

    /// Set glyph `gid`'s advance (a long `hmtx` entry).
    fn set_advance(program: &mut [u8], gid: usize, advance: u16) {
        let long = u16::from_be_bytes(program[table(program, b"hhea") + 34..][..2].try_into().unwrap()) as usize;
        assert!(gid < long, "a long hmtx entry");
        let at = table(program, b"hmtx") + gid * 4;
        program[at..at + 2].copy_from_slice(&advance.to_be_bytes());
    }

    /// Empty glyph `gid`, as a subsetter drops it: its data is left as the previous glyph's padding.
    fn drop_glyph(program: &mut [u8], gid: usize) {
        let long = program[table(program, b"head") + 51] == 1;
        let loca = table(program, b"loca");
        if long {
            let next: [u8; 4] = program[loca + (gid + 1) * 4..][..4].try_into().unwrap();
            program[loca + gid * 4..][..4].copy_from_slice(&next);
        } else {
            let next: [u8; 2] = program[loca + (gid + 1) * 2..][..2].try_into().unwrap();
            program[loca + gid * 2..][..2].copy_from_slice(&next);
        }
    }

    /// Inter's outlines standing in for a THSarabunPSK subset (no proprietary font is used): the
    /// glyphs where the exporter's order has a Thai mark or a mark-like alternate take no
    /// advance, the others 500 units.
    fn thai_ordered_program() -> Vec<u8> {
        let mut program = crate::script::INTER_REGULAR.to_vec();
        for gid in 214..=368usize {
            let mark = match gid {
                214..=298 => is_thai_mark(char::from_u32(gid as u32 + if gid <= 271 { 0xd2b } else { 0xd2f }).unwrap()),
                342..=368 => !matches!(gid, 342 | 357),
                _ => continue,
            };
            set_advance(&mut program, gid, if mark { 0 } else { 500 });
        }
        program
    }

    /// A THSarabunPSK Identity-H font over `program`, with `to_unicode`.
    fn thai_font(doc: &mut Document, program: &[u8], to_unicode: &[u8]) -> Dict {
        let file = Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), program))));
        thai_font_over(doc, file, Object::name("Identity"), to_unicode)
    }

    /// [`thai_font`] over the `/FontFile2` stream `file` and the `/CIDToGIDMap` `cid_to_gid`.
    fn thai_font_over(doc: &mut Document, file: Object, cid_to_gid: Object, to_unicode: &[u8]) -> Dict {
        let tu = doc.add(Object::Stream(Stream::flate(Dict::new(), to_unicode)));
        let mut descriptor = Dict::new();
        descriptor.set(b"FontFile2".to_vec(), file);
        let mut desc = Dict::new();
        desc.set(b"CIDToGIDMap".to_vec(), cid_to_gid);
        desc.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
        let mut font = Dict::new();
        font.set(b"Subtype".to_vec(), Object::name("Type0"));
        font.set(b"BaseFont".to_vec(), Object::name("ABCDEF+THSarabunPSK"));
        font.set(b"Encoding".to_vec(), Object::name("Identity-H"));
        font.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(desc)]));
        font.set(b"ToUnicode".to_vec(), Object::Ref(tu));
        font
    }

    /// `data` as a Flate stream under a PNG predictor whose `/DecodeParms` is given by reference:
    /// read without that dictionary, each row's tag byte stays in the decoded bytes.
    fn predicted(doc: &mut Document, data: &[u8]) -> Object {
        let rows: Vec<u8> = data.iter().flat_map(|byte| [0, *byte]).collect();
        let mut parms = Dict::new();
        parms.set(b"Predictor".to_vec(), Object::Int(12));
        parms.set(b"Columns".to_vec(), Object::Int(1));
        let parms = doc.add(Object::Dict(parms));
        let mut stream = Stream::flate(Dict::new(), &rows);
        stream.dict.set(b"DecodeParms".to_vec(), Object::Ref(parms));
        Object::Ref(doc.add(Object::Stream(stream)))
    }

    /// An intact legacy map: nominal Thai and two tone alternates.
    const INTACT: &[u8] = b"2 beginbfrange <00d6> <010f> <0e01> <0110> <012a> <0e3f> endbfrange 2 beginbfchar <0160> <0e48> <0161> <0e49> endbfchar";

    /// The exporter's damaged map: intact anchors, Latin/control destinations for the other Thai
    /// codes, and the oversized range first.
    const DAMAGED: &[u8] = b"begincmap 1 begincodespacerange <0000> <ffff> endcodespacerange \
          1 beginbfrange <00ff><01ff><0032> endbfrange 17 beginbfrange \
          <00d9><00d9><0e04> <00e2><00e2><0e0d> <00fa><00fa><0e25> <0100><0100><0e2b> <0102><0102><0e2d> \
          <0164><0164><0e4c> <0166><0166><0e31> <0168><0168><0e47> \
          <00d6><00d6><000f> <00f0><00f0><0029> <00f8><00f8><0023> <0105><0105><003e> <0107><0107><0040> \
          <0109><0109><003e> <0111><0111><004e> <0160><0160><004e> <0161><0161><004f> endbfrange endcmap";

    /// The anchors identify the exporter, but the glyph order is still checked glyph by glyph: a
    /// mark where a letter belongs (or the reverse) anywhere in the kept glyphs refuses the whole
    /// map, and a glyph the subset dropped is left unmapped whatever its filler advance says.
    #[test]
    fn recovered_thai_map_checks_every_kept_glyph_against_the_font_program() {
        let mut doc = Document::new_empty();
        let font = thai_font(&mut doc, &thai_ordered_program(), DAMAGED);
        let recovered: HashMap<_, _> = recovered_thai_map(&doc, &font).unwrap().into_iter().collect();
        assert_eq!(recovered[&262], 'ั');
        assert_eq!(recovered[&297], '๘');
        // 259–261 are ฮ, ฯ and ะ, between the last anchor (อ, 258) and the first mark: a font that
        // put Mai Han-akat one glyph earlier passes every anchor and only this check sees it.
        let mut shifted = thai_ordered_program();
        set_advance(&mut shifted, 261, 0);
        set_advance(&mut shifted, 262, 500);
        let shifted = thai_font(&mut doc, &shifted, DAMAGED);
        assert!(recovered_thai_map(&doc, &shifted).is_none(), "a kept glyph that advances unlike its character refuses the map");
        let mut dropped = thai_ordered_program();
        drop_glyph(&mut dropped, 262);
        set_advance(&mut dropped, 262, 500);
        let dropped = thai_font(&mut doc, &dropped, DAMAGED);
        let recovered = recovered_thai_map(&doc, &dropped).unwrap();
        assert!(recovered.iter().all(|(gid, _)| *gid != 262), "a dropped glyph is not mapped");
        assert!(recovered.iter().any(|(gid, _)| *gid == 263));
        let mut unreadable = font.clone();
        let Some(Object::Array(desc)) = unreadable.get(b"DescendantFonts").cloned() else { panic!("fixture") };
        let Some(Object::Dict(mut desc)) = desc.first().cloned() else { panic!("fixture") };
        desc.remove(b"FontDescriptor");
        unreadable.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(desc)]));
        assert!(recovered_thai_map(&doc, &unreadable).is_none(), "without the program nothing is verified, so nothing is mapped");
    }

    /// `program` without the tables a legacy subsetter leaves out.
    fn without_tables(program: &[u8], tags: &[&[u8; 4]]) -> Vec<u8> {
        let mut program = program.to_vec();
        let count = u16::from_be_bytes([program[4], program[5]]) as usize;
        let records: Vec<Vec<u8>> =
            program[12..12 + count * 16].chunks(16).filter(|r| !tags.iter().any(|t| &r[..4] == t.as_slice())).map(<[u8]>::to_vec).collect();
        program[4..6].copy_from_slice(&(records.len() as u16).to_be_bytes());
        for (i, record) in records.iter().enumerate() {
            program[12 + i * 16..28 + i * 16].copy_from_slice(record);
        }
        program
    }

    /// The font program and `/CIDToGIDMap` are read with `/DecodeParms` given by reference, as a
    /// viewer reads them: the map repair, the glyph codes and the shaper see the real bytes.
    #[test]
    fn thai_font_streams_are_read_with_decode_parms_given_by_reference() {
        let stripped = without_tables(&thai_ordered_program(), &[b"cmap", b"GSUB", b"GPOS"]);
        let mut doc = Document::new_empty();
        let program = predicted(&mut doc, &stripped);
        let damaged = thai_font_over(&mut doc, program.clone(), Object::name("Identity"), DAMAGED);
        assert!(recovered_thai_map(&doc, &damaged).is_some(), "the program is read with its predictor");
        let gids: Vec<u8> = (0u16..=368).flat_map(u16::to_be_bytes).collect();
        let map = predicted(&mut doc, &gids);
        let font = thai_font_over(&mut doc, program, map, INTACT);
        let metrics = Metrics::from_dict(&doc, &font);
        let (map, _) = composite_glyph_maps(&doc, &font, &stripped, &metrics, &mut DecodeBudget::new(1 << 20, 0)).unwrap();
        assert_eq!(map.get(&0x0e01), Some(&214), "the CIDToGIDMap is read with its predictor");
        let shaped =
            LegacyThaiFont::from_pdf(&doc, &font, 0.25, &mut DecodeBudget::new(1 << 26, 0)).unwrap().expect("a legacy subset").shape("ก่").unwrap();
        assert_eq!(shaped.shaped.glyphs.iter().map(|g| g.id.to_u32()).collect::<Vec<_>>(), [214, 352]);
    }

    /// A legacy subset that kept a Latin-only cmap of its own still reaches its Thai glyphs through
    /// the PDF's verified maps, and keeps its own mappings.
    #[test]
    fn a_latin_only_cmap_does_not_hide_a_legacy_subsets_thai_glyphs() {
        use skrifa::MetadataProvider as _;
        let program = without_tables(&thai_ordered_program(), &[b"GSUB", b"GPOS"]);
        let latin = skrifa::FontRef::new(&program).unwrap().charmap().map('A').unwrap();
        let mut doc = Document::new_empty();
        let font = thai_font(&mut doc, &program, INTACT);
        let shaped =
            LegacyThaiFont::from_pdf(&doc, &font, 0.25, &mut DecodeBudget::new(1 << 26, 0)).unwrap().expect("a legacy subset").shape("ก่").unwrap();
        assert_eq!(shaped.shaped.glyphs.iter().map(|g| g.id.to_u32()).collect::<Vec<_>>(), [214, 352]);
        let editor = crate::pdf::embedded_sfnt(&doc, &font).unwrap();
        assert_eq!(skrifa::FontRef::new(&editor).unwrap().charmap().map('A'), Some(latin), "the program's own mappings stay");
    }

    /// A THSarabunPSK program with its own Thai mark positioning is shaped by that layout; one
    /// without it only if every Thai glyph it keeps is where the legacy order has that character
    /// (the legacy shaper skips the positioning check). A subset needs no particular glyph.
    #[test]
    fn thai_source_programs_are_shaped_by_their_own_layout_or_as_verified_legacy() {
        let unicode = b"2 beginbfrange <00d6> <010f> <0e01> <0110> <012a> <0e3f> endbfrange 2 beginbfchar <0160> <0e48> <0161> <0e49> endbfchar";
        // `Some(positioned)` when the program can be shaped in place.
        let mode = |program: &[u8], unicode: &[u8]| {
            let mut doc = Document::new_empty();
            let font = thai_font(&mut doc, program, unicode);
            LegacyThaiFont::from_pdf(&doc, &font, 0.25, &mut DecodeBudget::new(1 << 26, 0)).unwrap().map(|f| f.positioned)
        };
        let stripped = without_tables(&thai_ordered_program(), &[b"cmap", b"GSUB", b"GPOS"]);
        assert_eq!(mode(&stripped, unicode), Some(false), "a legacy positional subset");
        let positioned = without_tables(&thai_ordered_program(), &[b"cmap", b"GSUB"]);
        assert_eq!(mode(&positioned, unicode), Some(true), "a face with Thai mark positioning uses it");
        // ก at another glyph than the legacy order's.
        let moved = b"2 beginbfrange <00d7> <010f> <0e02> <0110> <012a> <0e3f> endbfrange 1 beginbfchar <0024> <0e01> endbfchar";
        assert_eq!(mode(&stripped, moved), None, "out of the legacy order and without its own positioning");
        assert_eq!(mode(&positioned, moved), Some(true), "its own layout doesn't depend on the legacy order");
        // Some of the legacy glyphs, no ก or อ among them.
        assert_eq!(mode(&stripped, b"1 beginbfrange <00d7> <00e0> <0e02> endbfrange"), Some(false));
        assert_eq!(mode(&stripped, b"1 beginbfchar <0024> <0041> endbfchar"), None, "no Thai at all");
    }

    /// The Windows PUA alternates are used only when each one the subset kept advances as its
    /// form does: one mark out of place withdraws them all, so the shaper refuses instead of
    /// drawing another mark.
    #[test]
    fn legacy_thai_alternates_need_their_glyph_order_confirmed() {
        let unicode = b"2 beginbfrange <00d6> <010f> <0e01> <0110> <012a> <0e3f> endbfrange 2 beginbfchar <0160> <0e48> <0161> <0e49> endbfchar";
        let alternates_with = |program: &[u8], unicode: &[u8]| {
            let mut doc = Document::new_empty();
            let font = thai_font(&mut doc, program, unicode);
            let metrics = Metrics::from_dict(&doc, &font);
            let (map, _) = composite_glyph_maps(&doc, &font, program, &metrics, &mut DecodeBudget::new(1 << 20, 0)).unwrap();
            map.range(0xf700..=0xf71a).map(|(u, g)| (*u, *g)).collect::<Vec<_>>()
        };
        let alternates = |program: &[u8]| alternates_with(program, unicode);
        let ordered = alternates(&thai_ordered_program());
        assert!(ordered.contains(&(0xf70a, 352)) && ordered.contains(&(0xf710, 358)), "{ordered:?}");
        let mut misplaced = thai_ordered_program();
        set_advance(&mut misplaced, 352, 500);
        assert!(alternates(&misplaced).is_empty(), "one misplaced alternate withdraws them all");
        // A dropped alternate is simply absent; the others still come from the kept glyphs.
        let mut dropped = thai_ordered_program();
        drop_glyph(&mut dropped, 353);
        let kept = alternates(&dropped);
        assert!(kept.contains(&(0xf70a, 352)) && !kept.iter().any(|(_, g)| *g == 353), "{kept:?}");
        // Two marks of the same kind (both zero-advance) in another order: the PDF's own map says
        // glyph 343 is ี and 344 is ิ, the reverse of U+F701 and U+F702. None is used.
        let stated = b"2 beginbfrange <00d6> <010f> <0e01> <0110> <012a> <0e3f> endbfrange 4 beginbfchar <0157> <0e35> <0158> <0e34> <0160> <0e48> <0161> <0e49> endbfchar";
        assert!(alternates_with(&thai_ordered_program(), stated).is_empty(), "a stated meaning out of the PUA order withdraws them all");
        let agreeing = b"2 beginbfrange <00d6> <010f> <0e01> <0110> <012a> <0e3f> endbfrange 4 beginbfchar <0157> <0e34> <0158> <0e35> <0160> <0e48> <0161> <0e49> endbfchar";
        assert!(alternates_with(&thai_ordered_program(), agreeing).contains(&(0xf701, 343)));
    }

    #[test]
    fn damaged_legacy_thai_map_recovers_consonants_vowels_and_contextual_tones_only_for_its_fingerprint() {
        let mut doc = Document::new_empty();
        let data = b"begincmap 1 begincodespacerange <0000> <ffff> endcodespacerange 18 beginbfrange \
          <00d9><00d9><0e04> <00e2><00e2><0e0d> <00fa><00fa><0e25> <0100><0100><0e2b> <0102><0102><0e2d> \
          <0164><0164><0e4c> <0166><0166><0e31> <0168><0168><0e47> \
          <00d6><00d6><000f> <00f0><00f0><0029> <00f8><00f8><0023> <0105><0105><003e> <0107><0107><0040> \
          <0109><0109><003e> <0111><0111><004e> <0160><0160><004e> <0161><0161><004f> <00ff><01ff><0032> endbfrange endcmap";
        let mut font = thai_font(&mut doc, &thai_ordered_program(), data);
        let tu = font.get(b"ToUnicode").and_then(Object::as_ref).unwrap();
        // The corrupt range is declared before explicit anchors in the actual exporter.
        let data = String::from_utf8_lossy(data)
            .replace("18 beginbfrange", "1 beginbfrange <00ff><01ff><0032> endbfrange 17 beginbfrange")
            .replace(" <00ff><01ff><0032> endbfrange endcmap", " endbfrange endcmap");
        doc.set(tu, Object::Stream(pdfcraft_cos::Stream::flate(Dict::new(), data.as_bytes())));
        let recovered: HashMap<_, _> = recovered_thai_map(&doc, &font).unwrap().into_iter().collect();
        let title: String = [240, 248, 261, 214, 263, 253].into_iter().map(|code| recovered[&code]).collect();
        assert_eq!(title, "ประกาศ");
        assert_eq!(recovered[&265], 'ิ');
        assert_eq!(recovered[&273], 'เ');
        assert_eq!(recovered[&352], '่');
        assert_eq!(recovered[&353], '้');
        font.set(b"BaseFont".to_vec(), Object::name("ABCDEF+Unrelated"));
        assert!(recovered_thai_map(&doc, &font).is_none());
        font.set(b"BaseFont".to_vec(), Object::name("ABCDEF+THSarabunPSK"));
        doc.set(tu, Object::Stream(pdfcraft_cos::Stream::flate(Dict::new(), b"1 beginbfchar <00d9><0e04> endbfchar")));
        assert!(recovered_thai_map(&doc, &font).is_none(), "one plausible anchor cannot authorize guessing a map");
    }
}
