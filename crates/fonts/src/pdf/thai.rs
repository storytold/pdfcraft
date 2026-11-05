//! Recover a narrowly identified legacy Thai exporter map, not arbitrary missing Unicode.
//!
//! Some THSarabunPSK Identity-H subsets retain original glyph IDs but write Latin/control
//! destinations for Thai in ToUnicode (including an accidental 00ff..01ff range). The intact
//! Thai anchors identify the original nominal Thai order. Only that complete fingerprint is
//! accepted; correct maps and unrelated fonts are never changed. No font artwork is bundled.

use super::{Dict, Document, Metrics, dict, only_char, unicode_map};
use pdfcraft_cos::{DecodeBudget, Object};
use std::collections::BTreeMap;

/// Recover the nominal Thai codes and the presentation marks of this damaged legacy export.
/// The caller must record the repair, and attach the map to a copy of the page's font.
pub fn recovered_thai_map(doc: &Document, font: &Dict) -> Option<Vec<(u16, char)>> {
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
    let mut out: Vec<_> = (214u16..=298)
        .filter_map(|gid| {
            let unicode = u32::from(gid) + if gid <= 271 { 0xd2b } else { 0xd2f };
            char::from_u32(unicode).map(|ch| (gid, ch))
        })
        .collect();
    // Contextual marks have the same character as the nominal mark, with a moved outline.
    out.extend([
        (343, '\u{0e34}'),
        (344, '\u{0e35}'),
        (347, '\u{0e48}'),
        (351, '\u{0e4c}'),
        (352, '\u{0e48}'),
        (353, '\u{0e49}'),
        (356, '\u{0e4c}'),
        (358, '\u{0e31}'),
        (360, '\u{0e47}'),
    ]);
    Some(out)
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
        Some(Object::Stream(s)) => Some(budget.decode_within(s, 1 << 17).ok()?),
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
    if family.starts_with("THSarabunPSK")
        && unicode.get(&0x0e01) == Some(&214)
        && unicode.get(&0x0e2d) == Some(&258)
        && (alternate_meanings || unicode.contains_key(&0xf70a))
    {
        for (index, gid) in (342u32..=368).enumerate() {
            if outlined(gid)
                && let Some(code) = match &cid_map {
                    None => u16::try_from(gid).ok(),
                    Some(_) => codes.get(&gid).copied(),
                }
            {
                unicode.insert(0xf700 + index as u32, gid);
                codes.entry(gid).or_insert(code);
            }
        }
    }
    Some((unicode, codes))
}

/// Shape legacy Thai using the PDF's own outlines and return codes for that same font.
/// No font program, family, weight or size is replaced. A subset missing a contextual glyph
/// is refused so the caller can try another subset of the same face or ask for its full font.
pub struct LegacyThaiFont {
    bytes: Vec<u8>,
    codes: BTreeMap<u32, u16>,
    space: f64,
}

impl LegacyThaiFont {
    /// Read and verify the source font once for all wrapping/preview measurements.
    pub fn from_pdf(doc: &Document, font: &Dict, space: f64) -> Result<Option<Self>, String> {
        use skrifa::MetadataProvider as _;
        let m = Metrics::from_dict(doc, font);
        if !m.base_font.rsplit('+').next().unwrap_or(&m.base_font).starts_with("THSarabunPSK") {
            return Ok(None);
        }
        if !space.is_finite() || space < 0.0 {
            return Err("the source space width is invalid".into());
        }
        let bytes = super::embedded_sfnt(doc, font).ok_or("the source Thai font has no readable glyph program")?;
        let face = skrifa::FontRef::new(&bytes).map_err(|e| e.to_string())?;
        let cmap = face.charmap();
        if cmap.map('ก').is_none() || cmap.map('อ').is_none() || cmap.map('\u{f70a}').is_none() {
            return Ok(None);
        }
        let (_, codes) = composite_glyph_maps(doc, font, &bytes, &m, &mut DecodeBudget::new(1 << 17, 0))
            .ok_or("the Thai source glyph codes cannot be recovered safely")?;
        Ok(Some(Self { bytes, codes, space }))
    }

    /// Shape into glyphs that are already in this exact source face.
    pub fn shape(&self, text: &str) -> Result<ExistingThaiRun, String> {
        use skrifa::MetadataProvider as _;
        if text.len() > 262_144 || text.contains(['\n', '\r']) {
            return Err("shape one bounded Thai source row at a time".into());
        }
        let face = skrifa::FontRef::new(&self.bytes).map_err(|e| e.to_string())?;
        require_pua(&face.charmap(), text)?;
        let mut shaped = crate::source_shaping::ShapedRun { glyphs: Vec::new(), advance: 0.0 };
        let mut codes = Vec::new();
        // Legacy PDF subsets often omit the space glyph. Keep its measured PDF advance as a
        // positioning gap, rather than drawing .notdef or changing to another font.
        for piece in text.split_inclusive(' ') {
            let word = piece.strip_suffix(' ').unwrap_or(piece);
            if !word.is_empty() {
                let run = crate::source_shaping::shape_legacy_thai(&self.bytes, word)?;
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
    use super::recovered_thai_map;
    use pdfcraft_cos::{Dict, Document, Object};
    use std::collections::HashMap;

    #[test]
    fn damaged_legacy_thai_map_recovers_consonants_vowels_and_contextual_tones_only_for_its_fingerprint() {
        let mut doc = Document::new_empty();
        let data = b"begincmap 1 begincodespacerange <0000> <ffff> endcodespacerange 18 beginbfrange \
          <00d9><00d9><0e04> <00e2><00e2><0e0d> <00fa><00fa><0e25> <0100><0100><0e2b> <0102><0102><0e2d> \
          <0164><0164><0e4c> <0166><0166><0e31> <0168><0168><0e47> \
          <00d6><00d6><000f> <00f0><00f0><0029> <00f8><00f8><0023> <0105><0105><003e> <0107><0107><0040> \
          <0109><0109><003e> <0111><0111><004e> <0160><0160><004e> <0161><0161><004f> <00ff><01ff><0032> endbfrange endcmap";
        let tu = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(Dict::new(), data)));
        let mut desc = Dict::new();
        desc.set(b"CIDToGIDMap".to_vec(), Object::name("Identity"));
        let mut font = Dict::new();
        font.set(b"Subtype".to_vec(), Object::name("Type0"));
        font.set(b"BaseFont".to_vec(), Object::name("ABCDEF+THSarabunPSK"));
        font.set(b"Encoding".to_vec(), Object::name("Identity-H"));
        font.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(desc)]));
        font.set(b"ToUnicode".to_vec(), Object::Ref(tu));
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
