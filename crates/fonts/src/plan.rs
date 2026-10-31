//! Planning visible Unicode text output for OCR: which characters the standard WinAnsi
//! fonts show as is, which need a Type3 font drawn from the craft-fonts faces, and which have
//! no glyph anywhere (written as `?` — and counted, never silently dropped).
//!
//! The plan is a pure function of the words ([`TextPlan::for_words`]). The content generators
//! (`pdfcraft_ocr::text_layer` / `visible_text_layer`) read the per-character codes and
//! advances from it, and whoever registers the fonts (`pdfcraft_edit::add_type3_fonts`) fills
//! in each face's resource name and object. The build must work without craft-fonts too: there
//! [`CRAFT_FONTS`] is empty, nothing can be planned, and every non-WinAnsi character is
//! reported as a substitution instead.

use std::collections::HashMap;

use pdfcraft_cos::ObjRef;

use crate::{CraftFont, GlyphOutline, craft_glyph, document_japanese_font, face_has, fits_win_ansi, win_ansi_char};

/// Unique characters per Type3 font: the same bound the text-editing fallback uses, so one
/// hostile word cannot balloon into thousands of glyph streams. A face with more characters
/// simply gets another font.
pub const MAX_TYPE3_GLYPHS: usize = 240;

/// Planned glyphs per plan in total (10 full fonts). A page's recognition cannot realistically
/// exceed it; hostile input beyond it degrades to `?` substitutions, which are counted.
pub const MAX_PLAN_GLYPHS: usize = 2400;

/// One planned Type3 font: the glyphs of one craft-fonts face, one character per code
/// (`code = position + 1`, as in the text-editing fallback). `name` and `object` are filled in
/// when the font is added to a page's resources; until then the content generators fall back to
/// the WinAnsi path for the words that reference it.
pub struct PlannedType3 {
    /// The face family the glyphs are drawn from ("Shippori Mincho").
    pub family: &'static str,
    /// (character, outline with its advance) per code, in code order.
    pub glyphs: Vec<(char, GlyphOutline)>,
    /// The resource name the content references (`PCOcr`, `PCOcr1`, …); empty until registered.
    pub name: String,
    /// The built font object, shared by every page that uses the font; set at registration.
    pub object: Option<ObjRef>,
}

impl PlannedType3 {
    fn new(family: &'static str) -> Self {
        PlannedType3 { family, glyphs: Vec::new(), name: String::new(), object: None }
    }

    fn push(&mut self, ch: char, glyph: GlyphOutline) -> u8 {
        self.glyphs.push((ch, glyph));
        self.glyphs.len() as u8
    }
}

/// How the text layers write a plan's characters: each planned character is a
/// `(font index, code)` pair in its font's encoding; a character no face had a glyph for is
/// either the font's `?` code or absent from the run (both counted in [`TextPlan::missing`]).
#[derive(Default)]
pub struct TextPlan {
    /// One planned font per face chunk, in first-use order.
    pub fonts: Vec<PlannedType3>,
    /// Per input word: `(font, code)` per character; empty for a word the standard WinAnsi
    /// fonts write as is (or for which nothing could be planned).
    words: Vec<Vec<(usize, u8)>>,
    /// character → where its glyph lives, across all fonts (codes are font-local).
    index: HashMap<char, (usize, u8)>,
    /// Characters with no code in their word's font: they appear as `?` (or, when not even a
    /// `?` glyph exists, are left out) and are reported, never silently lost.
    missing: usize,
}

impl TextPlan {
    /// Plan the output for `words`, in order: characters the WinAnsi fonts can't show get codes
    /// in Type3 fonts built from the craft-fonts faces (the document face preferred, then the
    /// rest of the build input in manifest order); a word whose face can't show a character
    /// substitutes its `?`. Faces are chosen per word, so a word of one script stays one face;
    /// characters already planned for an earlier word keep their code. Built without
    /// craft-fonts, the plan reports every non-WinAnsi character as a substitution.
    pub fn for_words<'a>(words: impl IntoIterator<Item = &'a str>) -> TextPlan {
        let faces = document_faces();
        let mut coverage = Coverage::default();
        let mut plan = TextPlan::default();
        for word in words {
            if fits_win_ansi(word) {
                // The WinAnsi path substitutes '?' behind the caller's back; count it.
                plan.missing += word.chars().filter(|&c| win_ansi_char(c).is_none()).count();
                plan.words.push(Vec::new());
                continue;
            }
            let Some(face) = choose_face(&faces, word, &mut coverage) else {
                // No face for this word's script: the WinAnsi path writes '?' for what it
                // can't encode.
                plan.missing += word.chars().filter(|&c| win_ansi_char(c).is_none()).count();
                plan.words.push(Vec::new());
                continue;
            };
            let mut enc: Vec<(usize, u8)> = Vec::with_capacity(word.len());
            let mut run: Option<usize> = None;
            for ch in word.chars() {
                if let Some(&where_) = plan.index.get(&ch) {
                    enc.push(where_);
                    run = Some(where_.0);
                    continue;
                }
                if let Ok(glyph) = craft_glyph(face, ch)
                    && plan.planned_glyphs() < MAX_PLAN_GLYPHS
                    && let Some((fi, code)) = plan.add_glyph(face, ch, glyph)
                {
                    plan.index.insert(ch, (fi, code));
                    enc.push((fi, code));
                    run = Some(fi);
                    continue;
                }
                // The face has no glyph for the character (or the plan is full): substitute
                // the font's '?', in the font the surrounding run is set in.
                if let Some((fi, code)) = plan.question_mark(face, run.filter(|&f| plan.fonts[f].family == face.family)) {
                    enc.push((fi, code));
                    run = Some(fi);
                }
                plan.missing += 1;
            }
            plan.words.push(enc);
        }
        plan
    }

    /// The encoding of word `i` (parallel to the slice [`Self::for_words`] planned): a list of
    /// `(font, code)` per character, or an empty slice for a word the WinAnsi fonts write.
    /// Out-of-range indexes degrade to the empty slice rather than panicking — callers pass the
    /// same word list they planned with.
    pub fn encoding(&self, word: usize) -> &[(usize, u8)] {
        self.words.get(word).map(Vec::as_slice).unwrap_or_default()
    }

    /// How many characters no font could show (they appear as `?`, or are left out when the
    /// word's face has no `?` glyph either).
    pub fn missing(&self) -> usize {
        self.missing
    }

    /// The advance of one planned code, in em units (the Type3 glyphs' own widths). `0.0` for
    /// anything out of range.
    pub fn advance(&self, font: usize, code: u8) -> f64 {
        self.fonts.get(font).and_then(|f| f.glyphs.get(usize::from(code).checked_sub(1)?)).map_or(0.0, |(_, g)| g.width)
    }

    /// The sum of the advances of a word's planned codes, in em units.
    pub fn advance_of(&self, encoding: &[(usize, u8)]) -> f64 {
        encoding.iter().map(|&(f, c)| self.advance(f, c)).sum()
    }

    /// Whether every font the encoding references has been registered (has a resource name).
    /// Unregistered plans fall back to the WinAnsi path, the same output as before the
    /// Unicode mapping existed.
    pub fn registered(&self, encoding: &[(usize, u8)]) -> bool {
        encoding.iter().all(|&(f, _)| self.fonts.get(f).is_some_and(|font| !font.name.is_empty()))
    }

    /// Characters with a planned glyph so far.
    fn planned_glyphs(&self) -> usize {
        self.fonts.iter().map(|f| f.glyphs.len()).sum()
    }

    /// Add a glyph to the face's current font (a fresh one when that font is full).
    fn add_glyph(&mut self, face: &CraftFont, ch: char, glyph: GlyphOutline) -> Option<(usize, u8)> {
        let fi = self.font_of_face(face.family)?;
        let code = self.fonts[fi].push(ch, glyph);
        Some((fi, code))
    }

    /// The index of the face's open font, or a fresh one. `None` without craft-fonts (the
    /// face itself comes from there, so this cannot happen for a planned face).
    fn font_of_face(&mut self, family: &'static str) -> Option<usize> {
        if let Some(fi) = self.fonts.iter().rposition(|f| f.family == family && f.glyphs.len() < MAX_TYPE3_GLYPHS) {
            return Some(fi);
        }
        if self.fonts.len() >= MAX_PLAN_GLYPHS / MAX_TYPE3_GLYPHS {
            return None;
        }
        self.fonts.push(PlannedType3::new(family));
        Some(self.fonts.len() - 1)
    }

    /// The code of the font's `?` stand-in, adding the glyph when the font lacks it.
    /// `prefer` is the run's current font; `None` without craft-fonts.
    fn question_mark(&mut self, face: &CraftFont, prefer: Option<usize>) -> Option<(usize, u8)> {
        if let Some(fi) = prefer
            && let Some(code) = self.fonts[fi].glyphs.iter().position(|(c, _)| *c == '?')
        {
            return Some((fi, code as u8 + 1));
        }
        let fi = self.font_of_face(face.family)?;
        if let Some(code) = self.fonts[fi].glyphs.iter().position(|(c, _)| *c == '?') {
            return Some((fi, code as u8 + 1));
        }
        let glyph = craft_glyph(face, '?').ok()?;
        let code = self.fonts[fi].push('?', glyph);
        Some((fi, code))
    }
}

/// The faces that can draw document text, in preference order: the Japanese document face
/// (serif document text) first, then the rest of the build input in manifest order.
fn document_faces() -> Vec<&'static CraftFont> {
    let mut out = Vec::new();
    if let Some(d) = document_japanese_font() {
        out.push(d);
    }
    for f in crate::CRAFT_FONTS {
        if !out.iter().any(|d| std::ptr::eq(*d, f)) {
            out.push(f);
        }
    }
    out
}

/// The face for a word: the first (in preference order) whose glyphs cover every character,
/// else the one covering the most characters (ties go to the earlier face). `None` when no
/// face covers any character of the word.
fn choose_face<'f>(faces: &[&'f CraftFont], word: &str, coverage: &mut Coverage) -> Option<&'f CraftFont> {
    if let Some(f) = faces.iter().enumerate().find(|(i, f)| word.chars().all(|c| coverage.has(*i, f, c))).map(|(_, f)| *f) {
        return Some(f);
    }
    faces
        .iter()
        .enumerate()
        .map(|(i, f)| (word.chars().filter(|&c| coverage.has(i, f, c)).count(), std::cmp::Reverse(i), *f))
        .max_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)))
        .filter(|(covered, _, _)| *covered > 0)
        .map(|(_, _, f)| f)
}

/// Which faces have a glyph for which characters, remembered for one plan: a page of words
/// asks the same (face, character) pairs over and over, and each fresh answer parses the font.
#[derive(Default)]
struct Coverage {
    known: std::collections::HashMap<(usize, char), bool>,
    lookups: usize,
}

impl Coverage {
    /// Whether face number `i` (`face`) has `ch`; asks the font once per pair.
    fn has(&mut self, i: usize, face: &CraftFont, ch: char) -> bool {
        *self.known.entry((i, ch)).or_insert_with(|| {
            self.lookups += 1;
            face_has(face.bytes, ch)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Face coverage is asked once per (face, character): repeating words cost no new font
    /// lookups.
    #[test]
    fn face_coverage_is_remembered_per_face_and_character() {
        let faces = document_faces();
        let Some(face) = faces.first() else {
            eprintln!("skipping: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
            return;
        };
        let mut coverage = Coverage::default();
        let first = (0..50).map(|_| choose_face(&faces, "日本語", &mut coverage)).last().flatten();
        assert!(first.is_some());
        assert!(coverage.lookups <= faces.len() * 3, "{} lookups for 3 characters", coverage.lookups);
        assert_eq!(coverage.has(0, face, '日'), face_has(face.bytes, '日'));
    }

    /// A page's vocabulary: German umlauts are WinAnsi, and a character no bundled face has
    /// (here an emoji) is counted wherever it appears.
    #[test]
    fn win_ansi_words_get_no_encoding_and_count_only_real_losses() {
        let plan = TextPlan::for_words(["Zürcher", "café", "size"]);
        assert!(plan.words.iter().all(Vec::is_empty), "all WinAnsi: no Type3 fonts needed");
        assert!(plan.fonts.is_empty());
        assert_eq!(plan.missing(), 0);
        let none = TextPlan::for_words(["plain text"]);
        assert_eq!(none.missing(), 0);
        // U+1F600 has no WinAnsi code and no glyph in any bundled face (with craft-fonts the
        // JIS faces even cover the snowman): counted, shown as '?'.
        let emoji = TextPlan::for_words(["\u{1f600}!"]);
        assert_eq!(emoji.missing(), 1);
    }

    #[test]
    fn without_craft_fonts_every_non_win_ansi_character_is_reported() {
        if !crate::CRAFT_FONTS.is_empty() {
            eprintln!("built with craft-fonts: the planning tests with faces cover this instead");
            return;
        }
        let plan = TextPlan::for_words(["Привет", "日本", "abc"]);
        assert!(plan.words.iter().all(Vec::is_empty));
        assert!(plan.fonts.is_empty());
        assert_eq!(plan.missing(), 8, "six Cyrillic + two CJK characters, each reported");
    }

    #[test]
    fn encodings_are_aligned_and_total_the_advances() {
        if crate::CRAFT_FONTS.is_empty() {
            eprintln!("skipping: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
            assert!(TextPlan::for_words(["日本"]).fonts.is_empty());
            return;
        }
        let plan = TextPlan::for_words(["日本語", "語"]);
        assert_eq!(plan.encoding(0).len(), 3);
        assert_eq!(plan.encoding(1).len(), 1);
        // 語 planned once keeps its code (and font) in both words.
        let (f, c) = plan.encoding(1)[0];
        assert_eq!(plan.encoding(0)[2], (f, c), "the repeated character keeps its code");
        assert!(plan.advance(f, c) > 0.0);
        assert_eq!(plan.advance(f, 0), 0.0, "codes start at 1");
        assert_eq!(plan.encoding(7), &[], "out of range degrades to the WinAnsi path");
        assert!(plan.advance_of(plan.encoding(0)) > plan.advance_of(plan.encoding(1)));
    }

    #[test]
    fn hostile_input_stays_bounded() {
        // Thousands of DISTINCT characters: the covered ones fill fonts up to the caps, the
        // rest are reported substitutions.
        let word: String = (0..4000u32).filter_map(|i| char::from_u32(0x4e00 + i)).collect();
        let plan = TextPlan::for_words([word.as_str()]);
        assert!(plan.planned_glyphs() <= MAX_PLAN_GLYPHS);
        assert!(plan.fonts.iter().all(|f| f.glyphs.len() <= MAX_TYPE3_GLYPHS));
        assert!(plan.missing() > 0, "characters beyond the caps are reported, never silently dropped");
    }

    #[test]
    fn substitution_and_unknown_scripts_come_out_counted() {
        if crate::CRAFT_FONTS.is_empty() {
            eprintln!("skipping: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
            return;
        }
        let plan = TextPlan::for_words(["日本\u{1f4a9}語"]);
        // The emoji has no glyph in any bundled face; the kana/kanji do.
        assert_eq!(plan.missing(), 1);
        assert_eq!(plan.encoding(0).len(), 4, "the emoji still has an encoding: the font's ?");
    }
}
