//! Arabic text written into PDFs: shaping (joining forms, lam-alef ligatures, mark placement) and
//! glyph outlines, with any face that has Arabic: the craft-fonts `Arab` face, a font installed on
//! the machine ([`crate::system`]) or a font program embedded in the document; and Hebrew and Thai
//! shaping with the matching craft-fonts face.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use harfrust::{BufferClusterLevel, Direction, ShapeOptions, ShaperData, UnicodeBuffer, script};
use skrifa::instance::{LocationRef, Size};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId, MetadataProvider};

use crate::{GlyphError, GlyphOutline};

/// One cluster of shaped text: the glyphs that show one character (a letter's body and
/// dots, say), or several characters for a ligature. Em units.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapedCluster {
    /// Face that owns the glyph ids.
    pub script: ShapedScript,
    /// Glyph ids, each with its offset from the cluster's pen position (marks sit above or below).
    pub glyphs: Vec<(u32, [f64; 2])>,
    pub advance: f64,
    /// The characters it shows, in logical order.
    pub text: String,
}

/// Most glyphs [`ArabicFace::glyph_texts`] names (a font has at most 65 536).
const MAX_GLYPH_TEXTS: usize = 1 << 16;

#[derive(Clone)]
enum FaceData {
    Static(&'static [u8]),
    Shared(Arc<[u8]>),
}

/// A face Arabic text can be shaped and drawn with. Cheap to clone (the font bytes are shared).
#[derive(Clone)]
pub struct ArabicFace {
    /// Family and style names ("Traditional Arabic", "Bold"), for the PDF's font descriptor.
    pub family: String,
    pub style: String,
    pub bold: bool,
    data: FaceData,
    index: u32,
}

impl std::fmt::Debug for ArabicFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArabicFace").field("family", &self.family).field("style", &self.style).field("index", &self.index).finish()
    }
}

impl ArabicFace {
    /// The craft-fonts `Arab` face; `None` when the build has none.
    pub fn craft() -> Option<ArabicFace> {
        let f = crate::document_arabic_font()?;
        Some(ArabicFace { family: f.family.to_string(), style: f.style.to_string(), bold: false, data: FaceData::Static(f.bytes), index: 0 })
    }

    /// A face from font bytes (an installed file or a program embedded in a PDF). `None` unless
    /// face `index` parses and maps Arabic letters.
    pub fn from_shared(family: String, style: String, bold: bool, bytes: Arc<[u8]>, index: u32) -> Option<ArabicFace> {
        let face = ArabicFace { family, style, bold, data: FaceData::Shared(bytes), index };
        face.shaper().ok().filter(|s| s.has('\u{0627}'))?;
        Some(face)
    }

    fn bytes(&self) -> &[u8] {
        match &self.data {
            FaceData::Static(b) => b,
            FaceData::Shared(b) => b,
        }
    }

    /// Whether the face has a glyph for every one of `chars` (a quick look at its `cmap`, without
    /// preparing it for shaping).
    pub fn covers(&self, chars: &[char]) -> bool {
        FontRef::from_index(self.bytes(), self.index).is_ok_and(|font| {
            let map = font.charmap();
            chars.iter().all(|c| map.map(*c).is_some())
        })
    }

    /// How many glyphs the face has (`None` if it doesn't parse).
    pub fn glyph_count(&self) -> Option<u32> {
        FontRef::from_index(self.bytes(), self.index).ok()?.maxp().ok().map(|m| u32::from(m.num_glyphs()))
    }

    /// The text each glyph stands for. First every Arabic letter is shaped in each of its joining
    /// positions (alone, first, middle, last), so a glyph names the letter the font draws it for
    /// however the font gets there; then the character map, and the single, multiple and ligature
    /// substitutions name the rest. Word processors subset fonts keeping glyph ids, so this reads
    /// a PDF's glyphs when its own ToUnicode map is wrong (Word maps several Arabic glyphs of a
    /// face to one letter).
    pub fn glyph_texts(&self) -> HashMap<u32, String> {
        use skrifa::raw::tables::gsub::{SingleSubst, SubstitutionSubtables};
        let mut texts: HashMap<u32, String> = HashMap::new();
        let Ok(font) = FontRef::from_index(self.bytes(), self.index) else { return texts };
        if let Ok(shaper) = self.shaper() {
            const TATWEEL: char = '\u{0640}';
            let letters = ('\u{0621}'..='\u{064A}').chain('\u{066E}'..='\u{06D3}').chain('\u{06D5}'..='\u{06FF}').chain('\u{0750}'..='\u{077F}');
            let letters: Vec<char> = letters.filter(|c| shaper.has(*c) && *c != TATWEEL).collect();
            let mut name = |text: &str| {
                let Ok(clusters) = shaper.shape(text, true) else { return };
                for cluster in &clusters {
                    if let Some((glyph, _)) = cluster.glyphs.first()
                        && cluster.text.chars().any(|c| c != TATWEEL)
                    {
                        texts.entry(*glyph).or_insert_with(|| cluster.text.clone());
                    }
                }
            };
            // Basic letters first: an extended letter that shares a form keeps the basic name.
            for &c in &letters {
                for context in [vec![c], vec![c, TATWEEL], vec![TATWEEL, c, TATWEEL], vec![TATWEEL, c]] {
                    name(&context.iter().collect::<String>());
                }
            }
            // Faces with contextual forms (a yeh before a lam) draw a letter by its neighbours too:
            // every pair of basic letters, alone and inside a word.
            let basic: Vec<char> = letters.iter().copied().filter(|c| ('\u{0621}'..='\u{064A}').contains(c)).collect();
            for &a in &basic {
                for &b in &basic {
                    name(&[a, b].iter().collect::<String>());
                    name(&[TATWEEL, a, b, TATWEEL].iter().collect::<String>());
                }
            }
        }
        // Prefer a plain letter to a presentation form when a glyph has both.
        let presentation = |c: char| matches!(u32::from(c), 0xFB50..=0xFDFF | 0xFE70..=0xFEFF);
        for (code, gid) in font.charmap().mappings() {
            let Some(c) = char::from_u32(code) else { continue };
            let replace = texts.get(&gid.to_u32()).is_none_or(|t| t.chars().all(presentation) && !presentation(c));
            if replace {
                texts.insert(gid.to_u32(), c.to_string());
            }
        }
        let Ok(gsub) = font.gsub() else { return texts };
        let Ok(lookups) = gsub.lookup_list() else { return texts };
        // The joining forms and required ligatures name a glyph first: a stylistic lookup may
        // reach the same glyph from another letter.
        let mut joining: Vec<usize> = Vec::new();
        if let Ok(features) = gsub.feature_list() {
            for record in features.feature_records() {
                let tag = record.feature_tag();
                if [b"isol", b"init", b"medi", b"fina", b"rlig", b"liga", b"ccmp"].iter().any(|t| tag == skrifa::Tag::new(t))
                    && let Ok(feature) = record.feature(features.offset_data())
                {
                    joining.extend(feature.lookup_list_indices().iter().map(|i| usize::from(i.get())));
                }
            }
        }
        // Forms made from forms (a joining form of a ligature) need a few rounds; the joining
        // lookups go first, then all.
        for only_joining in [true, false] {
            for _ in 0..4 {
                let mut added = false;
                let mut add = |texts: &mut HashMap<u32, String>, glyph: u32, text: String| {
                    if !text.is_empty() && !texts.contains_key(&glyph) && texts.len() < MAX_GLYPH_TEXTS {
                        texts.insert(glyph, text);
                        added = true;
                    }
                };
                for (index, lookup) in lookups.lookups().iter().enumerate() {
                    if only_joining && !joining.contains(&index) {
                        continue;
                    }
                    let Ok(lookup) = lookup else { continue };
                    match lookup.subtables() {
                        Ok(SubstitutionSubtables::Single(tables)) => {
                            for table in tables.iter().flatten() {
                                match table {
                                    SingleSubst::Format1(t) => {
                                        let Ok(coverage) = t.coverage() else { continue };
                                        for g in coverage.iter() {
                                            let out = (i32::from(g.to_u16()) + i32::from(t.delta_glyph_id())).rem_euclid(0x1_0000) as u32;
                                            if let Some(text) = texts.get(&u32::from(g.to_u16())).cloned() {
                                                add(&mut texts, out, text);
                                            }
                                        }
                                    }
                                    SingleSubst::Format2(t) => {
                                        let Ok(coverage) = t.coverage() else { continue };
                                        for (g, out) in coverage.iter().zip(t.substitute_glyph_ids()) {
                                            if let Some(text) = texts.get(&u32::from(g.to_u16())).cloned() {
                                                add(&mut texts, u32::from(out.get().to_u16()), text);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        // A letter split into pieces (its body and a kashida, as Word justifies
                        // Arabic): the first piece stands for the letter.
                        Ok(SubstitutionSubtables::Multiple(tables)) => {
                            for table in tables.iter().flatten() {
                                let Ok(coverage) = table.coverage() else { continue };
                                for (g, sequence) in coverage.iter().zip(table.sequences().iter()) {
                                    let Ok(sequence) = sequence else { continue };
                                    let first = sequence.substitute_glyph_ids().first().map(|g| u32::from(g.get().to_u16()));
                                    if let (Some(first), Some(text)) = (first, texts.get(&u32::from(g.to_u16())).cloned()) {
                                        add(&mut texts, first, text);
                                    }
                                }
                            }
                        }
                        Ok(SubstitutionSubtables::Ligature(tables)) => {
                            for table in tables.iter().flatten() {
                                let Ok(coverage) = table.coverage() else { continue };
                                for (first, set) in coverage.iter().zip(table.ligature_sets().iter()) {
                                    let Ok(set) = set else { continue };
                                    for lig in set.ligatures().iter().flatten() {
                                        let parts = std::iter::once(u32::from(first.to_u16()))
                                            .chain(lig.component_glyph_ids().iter().map(|g| u32::from(g.get().to_u16())));
                                        let text: Option<String> = parts.map(|g| texts.get(&g).cloned()).collect();
                                        if let Some(text) = text {
                                            add(&mut texts, u32::from(lig.ligature_glyph().to_u16()), text);
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if !added {
                    break;
                }
            }
        }
        texts
    }

    /// The face ready to shape: parse it once and use the result for a whole piece of text.
    pub fn shaper(&self) -> Result<FaceShaper<'_>, GlyphError> {
        let font = FontRef::from_index(self.bytes(), self.index).map_err(|_| GlyphError::Missing)?;
        let data = ShaperData::new(&font);
        Ok(FaceShaper { font, data })
    }
}

/// A parsed [`ArabicFace`] with its shaping tables.
pub struct FaceShaper<'a> {
    font: FontRef<'a>,
    data: ShaperData,
}

impl FaceShaper<'_> {
    /// Whether the face has a glyph for `c`.
    pub fn has(&self, c: char) -> bool {
        self.font.charmap().map(c).is_some()
    }

    /// Whether the face has Arabic shaping rules (a `GSUB` table). A subset embedded in a PDF
    /// often lacks them, and would then draw every letter in its isolated form.
    pub fn can_join(&self) -> bool {
        self.font.gsub().is_ok()
    }

    /// Shape one directional run of `text`. The clusters come in drawing order, left to right.
    /// [`GlyphError::Missing`] when the face lacks a character.
    pub fn shape(&self, text: &str, rtl: bool) -> Result<Vec<ShapedCluster>, GlyphError> {
        let shaper = self.data.shaper(&self.font).build();
        let scale = 1.0 / f64::from(shaper.units_per_em().max(1));
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
        // Arabic joining applies to a run with Arabic in it; Latin words are shaped as Latin.
        buffer.set_script(if rtl || text.chars().any(is_arabic_letter) { script::ARABIC } else { script::LATIN });
        // One cluster per character, so marks keep their own text for copy and search.
        buffer.set_cluster_level(BufferClusterLevel::Characters);
        let shaped = shaper.shape(buffer, ShapeOptions::new());
        // Clusters are byte offsets into `text`; a cluster shows the characters up to the next one.
        let mut starts: Vec<usize> = shaped.glyph_infos().iter().map(|g| g.cluster as usize).collect();
        starts.sort_unstable();
        starts.dedup();
        let mut out: Vec<(usize, ShapedCluster)> = Vec::new();
        for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            if info.glyph_id == 0 {
                return Err(GlyphError::Missing);
            }
            let start = info.cluster as usize;
            // A cluster's glyphs are adjacent in the shaped run.
            let cluster = match out.last_mut() {
                Some((s, c)) if *s == start => c,
                _ => {
                    let end = starts.get(starts.partition_point(|s| *s <= start)).copied().unwrap_or(text.len());
                    let text = text.get(start..end).unwrap_or_default().to_string();
                    out.push((start, ShapedCluster { script: ShapedScript::Arabic, glyphs: Vec::new(), advance: 0.0, text }));
                    // Just pushed.
                    let Some((_, c)) = out.last_mut() else { continue };
                    c
                }
            };
            let at = [cluster.advance + f64::from(pos.x_offset) * scale, f64::from(pos.y_offset) * scale];
            cluster.glyphs.push((info.glyph_id, at));
            cluster.advance += f64::from(pos.x_advance) * scale;
        }
        // A bracket in a right-to-left run is drawn mirrored; its text is the bracket it shows, as
        // browsers and word processors write it, so text read back in display order comes out right.
        if rtl {
            for (_, c) in &mut out {
                if c.text.chars().any(|ch| mirror(ch) != ch) {
                    c.text = c.text.chars().map(mirror).collect();
                }
            }
        }
        Ok(out.into_iter().map(|(_, c)| c).collect())
    }

    /// Glyph `id`, bounded like [`crate::japanese_glyph`]. Glyphs without outlines (spaces) come
    /// back empty.
    pub fn glyph(&self, id: u32) -> Result<GlyphOutline, GlyphError> {
        let font = &self.font;
        if font.maxp().map_or(true, |m| id >= u32::from(m.num_glyphs())) {
            return Err(GlyphError::Missing);
        }
        let gid = GlyphId::new(id);
        let loc = LocationRef::default();
        let scale = 1.0 / font.metrics(Size::unscaled(), loc).units_per_em.max(1) as f64;
        let width = font.glyph_metrics(Size::unscaled(), loc).advance_width(gid).unwrap_or(0.0) as f64 * scale;
        // Marks have no advance; the widest ligatures (U+FDFD) span a few em.
        if !width.is_finite() || !(0.0..=8.0).contains(&width) {
            return Err(GlyphError::Missing);
        }
        if font.outline_glyphs().get(gid).is_none() {
            return Ok(GlyphOutline { contours: Vec::new(), width, bbox: [0.0, 0.0, width, 0.0] });
        }
        crate::script::bounded_outline(font, gid, width)
    }
}

/// The bracket facing the other way (Unicode Bidi_Mirrored pairs in common use).
fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        other => other,
    }
}

/// Arabic-script letters and marks (not digits or punctuation), which select Arabic shaping.
fn is_arabic_letter(c: char) -> bool {
    matches!(u32::from(c), 0x0620..=0x065F | 0x066E..=0x06D3 | 0x06D5..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFC)
}

/// How many glyphs the font program `bytes` (its first face) has; `None` if it doesn't parse.
pub fn program_glyph_count(bytes: &[u8]) -> Option<u32> {
    FontRef::new(bytes).ok()?.maxp().ok().map(|m| u32::from(m.num_glyphs()))
}

/// The face owning a shaped glyph id. IDs from different faces must never share a cache key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShapedScript {
    Arabic,
    Hebrew,
    Thai,
}

impl ShapedScript {
    pub fn font(self) -> Option<&'static crate::CraftFont> {
        match self {
            Self::Arabic => crate::document_arabic_font(),
            Self::Hebrew => crate::document_hebrew_font(false),
            Self::Thai => crate::document_thai_font(),
        }
    }
}

fn face(script: ShapedScript) -> Option<&'static (FontRef<'static>, ShaperData)> {
    static FACE: OnceLock<Option<(FontRef<'static>, ShaperData)>> = OnceLock::new();
    static HEBREW: OnceLock<Option<(FontRef<'static>, ShaperData)>> = OnceLock::new();
    static THAI: OnceLock<Option<(FontRef<'static>, ShaperData)>> = OnceLock::new();
    let cache = match script {
        ShapedScript::Arabic => &FACE,
        ShapedScript::Hebrew => &HEBREW,
        ShapedScript::Thai => &THAI,
    };
    cache
        .get_or_init(|| {
            let font = FontRef::new(script.font()?.bytes).ok()?;
            let data = ShaperData::new(&font);
            Some((font, data))
        })
        .as_ref()
}

/// Whether the craft-fonts Arabic face has a glyph for `c` (false without the face).
pub fn arabic_has(c: char) -> bool {
    face(ShapedScript::Arabic).is_some_and(|(font, _)| font.charmap().map(c).is_some())
}

/// Shape one directional run of `text` with the craft-fonts Arabic face. The clusters come in
/// drawing order, left to right. [`GlyphError::NoFont`] without the face;
/// [`GlyphError::Missing`] when it lacks a character.
pub fn shape_arabic(text: &str, rtl: bool) -> Result<Vec<ShapedCluster>, GlyphError> {
    shape_script(text, rtl, ShapedScript::Arabic)
}

/// Whether the Hebrew document face has a glyph for `c` (false without the face).
pub fn hebrew_has(c: char) -> bool {
    face(ShapedScript::Hebrew).is_some_and(|(font, _)| font.charmap().map(c).is_some())
}

/// Shape one directional run of `text` with the Hebrew document face, clusters in drawing order.
/// Points (niqqud) and cantillation marks stay in their letter's cluster, positioned by the face.
pub fn shape_hebrew(text: &str, rtl: bool) -> Result<Vec<ShapedCluster>, GlyphError> {
    shape_script(text, rtl, ShapedScript::Hebrew)
}

pub fn thai_has(c: char) -> bool {
    face(ShapedScript::Thai).is_some_and(|(font, _)| font.charmap().map(c).is_some())
}

/// Shape Thai with grapheme clusters so Sara Am decomposition and stacked marks keep their
/// original Unicode text exactly once, including when wrapped or copied from the saved PDF.
pub fn shape_thai(text: &str) -> Result<Vec<ShapedCluster>, GlyphError> {
    shape_script(text, false, ShapedScript::Thai)
}

fn shape_script(text: &str, rtl: bool, kind: ShapedScript) -> Result<Vec<ShapedCluster>, GlyphError> {
    let (font, data) = face(kind).ok_or(GlyphError::NoFont)?;
    let shaper = data.shaper(font).build();
    let scale = 1.0 / f64::from(shaper.units_per_em().max(1));
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
    buffer.set_script(match kind {
        ShapedScript::Arabic => script::ARABIC,
        ShapedScript::Hebrew => script::HEBREW,
        ShapedScript::Thai => script::THAI,
    });
    // Preserve the historical Arabic per-character mapping; Hebrew and Thai marks stay with their base.
    buffer.set_cluster_level(match kind {
        ShapedScript::Arabic => BufferClusterLevel::Characters,
        ShapedScript::Hebrew | ShapedScript::Thai => BufferClusterLevel::MonotoneGraphemes,
    });
    let shaped = shaper.shape(buffer, ShapeOptions::new());
    // Clusters are byte offsets into `text`; a cluster shows the characters up to the next one.
    let mut starts: Vec<usize> = shaped.glyph_infos().iter().map(|g| g.cluster as usize).collect();
    starts.sort_unstable();
    starts.dedup();
    let mut out: Vec<(usize, ShapedCluster)> = Vec::new();
    for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
        if info.glyph_id == 0 {
            return Err(GlyphError::Missing);
        }
        let start = info.cluster as usize;
        // A cluster's glyphs are adjacent in the shaped run.
        let cluster = match out.last_mut() {
            Some((s, c)) if *s == start => c,
            _ => {
                let end = starts.get(starts.partition_point(|s| *s <= start)).copied().unwrap_or(text.len());
                let text = text.get(start..end).unwrap_or_default().to_string();
                out.push((start, ShapedCluster { script: kind, glyphs: Vec::new(), advance: 0.0, text }));
                // Just pushed.
                let Some((_, c)) = out.last_mut() else { continue };
                c
            }
        };
        let at = [cluster.advance + f64::from(pos.x_offset) * scale, f64::from(pos.y_offset) * scale];
        cluster.glyphs.push((info.glyph_id, at));
        cluster.advance += f64::from(pos.x_advance) * scale;
    }
    Ok(out.into_iter().map(|(_, c)| c).collect())
}

/// Glyph `id` of the craft-fonts Arabic face, bounded like [`crate::japanese_glyph`]. Glyphs
/// without outlines (spaces) come back empty.
pub fn arabic_glyph(id: u32) -> Result<GlyphOutline, GlyphError> {
    shaped_glyph(ShapedScript::Arabic, id)
}

/// A bounded outline from the same face that produced the shaped glyph id.
pub fn shaped_glyph(script: ShapedScript, id: u32) -> Result<GlyphOutline, GlyphError> {
    let (font, _) = face(script).ok_or(GlyphError::NoFont)?;
    glyph(font, id)
}

/// Glyph `id` of the face in `bytes`, bounded like [`shaped_glyph`]: for text shaped with
/// [`crate::source_shaping::shape`] in a face no [`ShapedScript`] names.
pub fn face_glyph(bytes: &[u8], id: u32) -> Result<GlyphOutline, GlyphError> {
    glyph(&FontRef::new(bytes).map_err(|_| GlyphError::NoFont)?, id)
}

fn glyph(font: &FontRef, id: u32) -> Result<GlyphOutline, GlyphError> {
    if font.maxp().map_or(true, |m| id >= u32::from(m.num_glyphs())) {
        return Err(GlyphError::Missing);
    }
    let gid = GlyphId::new(id);
    let loc = LocationRef::default();
    let scale = 1.0 / font.metrics(Size::unscaled(), loc).units_per_em.max(1) as f64;
    let width = font.glyph_metrics(Size::unscaled(), loc).advance_width(gid).unwrap_or(0.0) as f64 * scale;
    // Marks have no advance; the widest ligatures (U+FDFD) span a few em.
    if !width.is_finite() || !(0.0..=8.0).contains(&width) {
        return Err(GlyphError::Missing);
    }
    if font.outline_glyphs().get(gid).is_none() {
        return Ok(GlyphOutline { contours: Vec::new(), width, bbox: [0.0, 0.0, width, 0.0] });
    }
    crate::script::bounded_outline(font, gid, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face_or_skip() -> bool {
        if crate::document_arabic_font().is_none() {
            eprintln!("skipping the Arabic shaping checks: built without a craft-fonts Arab face (set CRAFT_FONTS_DIR)");
            assert_eq!(shape_arabic("ب", true), Err(GlyphError::NoFont));
            assert_eq!(arabic_glyph(1).map(|_| ()), Err(GlyphError::NoFont));
            assert!(!arabic_has('ب'));
            return false;
        }
        true
    }

    #[test]
    fn letters_take_their_joining_forms() {
        if !face_or_skip() {
            return;
        }
        let ids =
            |s: &str| shape_arabic(s, true).unwrap().iter().flat_map(|c| c.glyphs.iter().map(|g| g.0)).collect::<std::collections::BTreeSet<_>>();
        let (isolated, joined) = (ids("ب"), ids("ببب"));
        // Initial, medial and final forms are glyphs the isolated letter doesn't use.
        assert!(joined.difference(&isolated).count() >= 2, "{isolated:?} {joined:?}");
        // One cluster per letter, however many glyphs the face draws it with (Noto Sans Arabic:
        // body and dots).
        let c = shape_arabic("ببب", true).unwrap();
        assert!(c.len() == 3 && c.iter().all(|c| c.text == "ب" && c.advance > 0.0 && !c.glyphs.is_empty()), "{c:?}");
    }

    #[test]
    fn right_to_left_runs_are_drawn_from_their_last_letter() {
        if !face_or_skip() {
            return;
        }
        let g = shape_arabic("سلام", true).unwrap();
        let drawn: String = g.iter().map(|g| g.text.as_str()).collect();
        assert_eq!(drawn.chars().rev().collect::<String>(), "سلام");
        assert!(shape_arabic("سلام", false).unwrap().iter().map(|g| g.text.as_str()).collect::<String>() == "سلام");
    }

    #[test]
    fn marks_keep_their_own_text_and_no_advance() {
        if !face_or_skip() {
            return;
        }
        let c = shape_arabic("بَ", true).unwrap();
        assert_eq!(c.len(), 2, "{c:?}");
        let mark = c.iter().find(|c| c.text == "\u{064E}").unwrap();
        assert_eq!(mark.advance, 0.0);
        let outline = arabic_glyph(mark.glyphs[0].0).unwrap();
        assert!(!outline.contours.is_empty() && outline.bbox[1] > 0.0, "the fatha sits above the baseline");
    }

    #[test]
    fn spaces_and_missing_characters() {
        if !face_or_skip() {
            return;
        }
        let space = shape_arabic(" ", true).unwrap();
        assert!(space[0].advance > 0.0);
        assert!(arabic_glyph(space[0].glyphs[0].0).unwrap().contours.is_empty());
        assert!(arabic_has('ب') && !arabic_has('日'));
        assert_eq!(shape_arabic("日", true), Err(GlyphError::Missing));
        assert_eq!(arabic_glyph(u32::MAX).map(|_| ()), Err(GlyphError::Missing));
        assert!(shape_arabic("", true).unwrap().is_empty());
    }

    #[test]
    fn faces_from_bytes_must_be_arabic_fonts() {
        let junk: Arc<[u8]> = Arc::from(&b"not a font"[..]);
        assert!(ArabicFace::from_shared("X".into(), "Regular".into(), false, junk, 0).is_none());
        // Inter parses but has no Arabic.
        let inter: Arc<[u8]> = Arc::from(&include_bytes!("../../../assets/fonts/Inter-Regular.ttf")[..]);
        assert!(ArabicFace::from_shared("Inter".into(), "Regular".into(), false, inter.clone(), 0).is_none());
        assert!(ArabicFace::from_shared("Inter".into(), "Regular".into(), false, inter, 7).is_none());
    }

    #[test]
    fn glyph_texts_name_joining_forms_by_their_letter() {
        // The usual default (Arial, Tahoma, …) rather than a decorative face.
        let Some(face) =
            crate::arabic_candidates(None, &[], false, false).iter().find_map(|f| f.load().filter(|f| f.shaper().is_ok_and(|s| s.can_join())))
        else {
            eprintln!("skipping: no installed Arabic font with shaping rules");
            return;
        };
        let texts = face.glyph_texts();
        let s = face.shaper().unwrap();
        // Each glyph of a joined word stands for its own letter: the medial beh is "ب".
        for c in s.shape("ببب", true).unwrap() {
            for (id, _) in &c.glyphs {
                if let Some(t) = texts.get(id) {
                    // Beh, or one of its presentation forms (U+FE8F–U+FE92), which the editor reads as beh.
                    let beh = |t: &str| t == "ب" || t.chars().all(|c| ('\u{FE8F}'..='\u{FE92}').contains(&c));
                    assert!(beh(t) || t.chars().all(|c| !c.is_alphabetic()), "{face:?}: glyph {id} reads {t:?}");
                }
            }
        }
        let medial = s.shape("ببب", true).unwrap()[1].glyphs[0].0;
        let medial = texts.get(&medial).cloned().unwrap_or_default();
        // Fonts may share one glyph between beh's initial and medial forms.
        assert!(medial == "ب" || ('\u{FE8F}'..='\u{FE92}').contains(&medial.chars().next().unwrap_or(' ')), "{face:?}: {medial:?}");
        assert!(face.glyph_count().is_some_and(|n| n > 0));
        assert_eq!(program_glyph_count(b"not a font"), None);
    }

    #[test]
    fn an_installed_face_joins_letters_and_shapes_latin() {
        let Some(face) = crate::installed_arabic_fonts().iter().find_map(|f| f.load()) else {
            eprintln!("skipping: no installed Arabic font on this machine");
            return;
        };
        let s = face.shaper().unwrap();
        let ids = |t: &str| s.shape(t, true).unwrap().iter().flat_map(|c| c.glyphs.iter().map(|g| g.0)).collect::<std::collections::BTreeSet<_>>();
        if s.can_join() {
            assert!(ids("ببب").difference(&ids("ب")).count() >= 2, "{face:?} joins beh");
        }
        // Spaces and Latin text shape without Arabic substitutions, left to right.
        let latin = s.shape("PDF 12", false);
        if let Ok(latin) = latin {
            assert_eq!(latin.iter().map(|c| c.text.as_str()).collect::<String>(), "PDF 12");
        }
        assert!(s.glyph(u32::MAX).is_err());
    }
}

#[cfg(test)]
mod thai_tests {
    use super::*;

    #[test]
    fn thai_clusters_preserve_sara_am_and_stacked_marks() {
        if crate::document_thai_font().is_none() {
            assert_eq!(shape_thai("น้ำ"), Err(GlyphError::NoFont));
            assert!(!thai_has('ก'));
            return;
        }
        for text in ["น้ำ", "กิ่", "ผู้ใช้", "ปู่", "เก้า", "สวัสดีภาษาไทย", "๑๒๓", ""]
        {
            let clusters = shape_thai(text).unwrap();
            assert_eq!(clusters.iter().map(|c| c.text.as_str()).collect::<String>(), text);
            for c in clusters {
                assert_eq!(c.script, ShapedScript::Thai);
                assert!(c.advance.is_finite() && c.advance >= 0.0);
                for (id, pos) in c.glyphs {
                    assert!(pos.iter().all(|v| v.is_finite()));
                    shaped_glyph(c.script, id).unwrap();
                }
            }
        }
        let stacked = shape_thai("กิ่").unwrap();
        assert_eq!(stacked.len(), 1, "base, vowel and tone must wrap together");
        assert!(stacked[0].glyphs.len() >= 3);
        assert!(stacked[0].glyphs.iter().any(|(_, p)| p[0] != 0.0 || p[1] != 0.0));
        assert_eq!(shape_thai("日"), Err(GlyphError::Missing));
        assert_eq!(shaped_glyph(ShapedScript::Thai, u32::MAX).map(|_| ()), Err(GlyphError::Missing));
    }
}

#[cfg(test)]
mod hebrew_tests {
    use super::*;

    #[test]
    fn hebrew_runs_are_drawn_from_their_last_letter_with_points_on_their_letter() {
        if crate::document_hebrew_font(false).is_none() {
            eprintln!("skipping the Hebrew shaping checks: built without a craft-fonts Hebr face (set CRAFT_FONTS_DIR)");
            assert_eq!(shape_hebrew("ש", true), Err(GlyphError::NoFont));
            assert!(!hebrew_has('ש'));
            return;
        }
        let g = shape_hebrew("שלום", true).unwrap();
        assert!(g.iter().all(|c| c.script == ShapedScript::Hebrew && c.advance > 0.0 && !c.glyphs.is_empty()), "{g:?}");
        let drawn: String = g.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(drawn.chars().rev().collect::<String>(), "שלום");
        // A point (qamats) is one cluster with its letter, so copy and search get it once.
        let pointed = shape_hebrew("שָׁ", true).unwrap();
        assert_eq!(pointed.len(), 1, "{pointed:?}");
        assert_eq!(pointed[0].text, "שָׁ");
        assert!(pointed[0].glyphs.len() >= 2);
        for c in shape_hebrew("שָׁלוֹם 123", true).unwrap() {
            for (id, pos) in c.glyphs {
                assert!(pos.iter().all(|v| v.is_finite()));
                shaped_glyph(c.script, id).unwrap();
            }
        }
        assert!(hebrew_has('א') && hebrew_has('\u{05B8}') && !hebrew_has('日'));
        assert_eq!(shape_hebrew("日", true), Err(GlyphError::Missing));
        assert!(shape_hebrew("", true).unwrap().is_empty());
    }
}
