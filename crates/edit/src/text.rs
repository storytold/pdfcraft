//! Edit a PDF ▸ Edit text: the text lines already on a page, and replacing a line's text in place.
//!
//! A *line* is a run of compatible text-showing operators on the same baseline, read
//! from one content stream. Replacing it rewrites only that stream (as a new object, since content
//! streams may be shared): the first operator of the line shows the new text, the others are
//! dropped, and every positioning operator stays, so the lines after it keep their places
//! (`Td`, `T*` and friends move from the line matrix, which showing text doesn't change).
//!
//! The new text uses the line's own font when every character has a code and a glyph in it;
//! otherwise it is set in Helvetica (WinAnsi), and the result says the font was substituted.
//! Reflowing a paragraph to a new width is not done here (see `edit.text-reflow`).

use std::collections::HashMap;
use std::rc::Rc;

use pdfcraft_content::{Matrix, Op, Pieces, parse, serialize_ops};
use pdfcraft_cos::{DecodeBudget, Dict, Document, Object, PdfString, Stream};
use pdfcraft_fonts::pdf::Metrics;
use pdfcraft_fonts::{CraftFont, GlyphError, japanese_glyph_from};

use crate::EditError;

#[path = "text_regions.rs"]
mod regions;
#[path = "text_session.rs"]
mod session;
use regions::{alignment_frame, group_blocks, is_centred, rewrite_styled_block};
pub use session::TextSession;

mod blocks;
mod editor;
mod fallback;
mod interp;
mod lines;
mod rewrite;

use blocks::paragraph_separator;
#[cfg(test)]
use blocks::wrap;
pub use blocks::{
    BlockKey, BlockStyle, RangeStyle, ReadingBlocks, TextBlock, block_overlapping, clear_block, reading_blocks, reading_blocks_checked,
    replace_block, text_blocks,
};
use editor::layout_draft;
pub use editor::{TextEditLayout, TextEditorFont, register_text_font, text_edit_layout, text_editor_fonts};
#[cfg(test)]
pub(crate) use fallback::SOURCE_SPACE_CALLS;
use fallback::{
    MAX_TYPE3_GLYPHS, Type3Glyph, fallback_face, needs_type3, no_fallback_font, no_glyph_for, same_base_font, same_font, source_encodes,
    source_family, source_program, source_show, source_space, source_width, strip_subset_tag, type3_faces, type3_glyph, write_type3_glyphs,
};
#[cfg(test)]
use fallback::{add_type3_font, is_type3_fallback};
use interp::{Origin, Paint, union};
use lines::page_dict;
pub(crate) use lines::{FormBudget, content_streams, content_streams_within, form_call, rewritten_contents, splice};
pub use lines::{MAX_FORM_DEPTH, MAX_FORM_VISITS, glyph_text_lines, text_lines};
pub use rewrite::{LineEdit, replace_line, rewrite_block};
use rewrite::{coincident_ops, rewrite_block_from};
#[cfg(test)]
pub(crate) use rewrite::{replace_line_with_limit, rewrite_block_with_limit};
pub(crate) use rewrite::{reveal_ops, selectable_blocks, translate_blocks};

/// One line of existing text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    /// The line's text as shown.
    pub text: String,
    /// Its box in user space.
    pub rect: [f64; 4],
    /// The boxes of the words it draws, in user space: from each word's first glyph to the end of
    /// its last, without the spaces and `TJ` adjustments around it. `rect` also spans gaps that
    /// draw nothing, such as the adjustment an applied redaction leaves where text was removed.
    pub ink: Vec<[f64; 4]>,
    /// The font's resource name and `/BaseFont` (or Type 3 descriptor's `/FontName`), and its size in text space.
    pub font: String,
    pub base_font: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// The current fill colour as RGB.
    pub color: [f64; 3],
    /// Whether new text in this font can only be shown by substituting another font (no
    /// Unicode mapping for the line, so nothing could be reused).
    pub decodable: bool,
    /// Whether the font requires visual-order Indic text.
    pub visual_order: bool,
    /// The Japanese or Hebrew fallback face the font subsets, when the line is in one (see [`same_font`]).
    face: Option<String>,
    /// Its box along and across its baseline, `[along0, across0, along1, across1]` (see
    /// [`TextLine::frame_rect`]). Upright text has `rect`; turned text has its own box, which is
    /// narrower than `rect` turned into the same frame.
    frame: [f64; 4],
    /// The content stream piece the line starts in, and its operators (indices into the page's
    /// streams parsed joined).
    stream: usize,
    ops: Vec<usize>,
    /// Where the line starts: text matrix (text space), the state there, and its `BT`.
    origin: Origin,
    /// Explicit marked-content scope (when present); distinct scopes must not be guessed into one paragraph.
    scope: Option<usize>,
    /// The layers and graphics state parameters it is drawn in; see [`Paint`].
    paint: Paint,
    /// Right-to-left `/ActualText` already supplies typing order; glyph text still needs bidi
    /// recovery. Left-to-right ActualText (e.g. a Latin ligature) joins its line as usual.
    logical_order: bool,
    /// Some of the line's text is `/ActualText`, which is already in logical order: visual-order
    /// Indic recovery would scramble it.
    actual_text: bool,
    /// Whether any of the line's runs draws in a clipping mode (`4 Tr` to `7 Tr`). The join below
    /// does not require one rendering mode, so a line that starts as ink can continue as a mask;
    /// [`refuse_clipping_text`] reads this rather than the origin's mode alone.
    clips: bool,
    /// Why the line can't be edited: its font's writing mode (its CMap's `WMode`) couldn't be read,
    /// so which way its glyphs advance is unknown. The line is listed as horizontal text.
    unreadable_mode: Option<String>,
    /// The runs the line joins, in content order: each one's text, the end of its words in
    /// `ink`, and whether joining put a space before it. Empty when the line's text was put
    /// into another order (see [`TextLine::runs`]).
    runs: Vec<(String, usize, bool)>,
    /// This line continues the previous one because a rewrite wrapped it (`/PCSoft`).
    soft_break: bool,
    /// This line starts after a break a rewrite kept on purpose (`/PCHard`).
    hard_break: bool,
    /// When `soft_break`: the exact number of separator characters the wrap consumed (0 for a
    /// Chinese/Japanese word split), from `/PCSoft`'s `/N`; 1 when the marker carried none
    /// (#2529). Meaningless otherwise.
    soft_gap: usize,
}

impl TextLine {
    /// Refuses editing a line whose font's writing mode couldn't be read (§9.7.4.3).
    fn ensure_editable(&self) -> Result<(), EditError> {
        self.unreadable_mode.as_ref().map_or(Ok(()), |why| Err(EditError::Invalid(why.clone())))
    }

    /// Origin of the source baseline in PDF user space, independent of font ascent/descent.
    pub fn baseline(&self) -> [f64; 2] {
        [self.origin.x, self.origin.baseline]
    }

    /// Leading used when a single source run wraps into additional rows.
    pub fn wrap_leading(&self) -> f64 {
        if self.origin.state.leading > 0.0 { self.origin.state.leading * self.origin.k } else { self.size * 1.2 }
    }
}

#[cfg(test)]
mod tests {
    use super::lines::reading_lines;
    use super::rewrite::{CLIPPING_SCAN_LIMIT_REASON, CLIPPING_TEXT_REASON, ClippingIndex, coincident_line, refuse_clipping_text};
    use super::*;

    /// A contributor-original single-font page, generated in memory.
    fn eligibility_page(content: &str) -> Document {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_owned(),
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        Document::open(std::sync::Arc::new(bytes)).unwrap()
    }

    /// Independent line positions/operators keep performance tests free of grouping costs.
    fn eligibility_lines(count: usize) -> Vec<TextLine> {
        let doc = eligibility_page("BT /F1 12 Tf 40 60 Td (sample) Tj ET");
        let template = text_lines(&doc, 0).unwrap().remove(0);
        (0..count)
            .map(|i| {
                let mut line = template.clone();
                let y = i as f64 * 30.0;
                line.rect = [40.0, y, 90.0, y + 12.0];
                line.origin.baseline = y;
                line.ops = vec![i];
                line
            })
            .collect()
    }

    /// One mask must not trigger one full-page copy search per ordinary paragraph.
    #[test]
    fn clipping_index_one_mask_work_is_linear() {
        let work = |count| {
            let mut lines = eligibility_lines(count);
            lines[0].clips = true;
            let mut index = ClippingIndex::new(&lines);
            let before = index.remaining;
            for line in &lines[1..] {
                assert_eq!(index.refusal(&[line]), None);
            }
            before - index.remaining
        };
        let small = work(1024);
        let large = work(2048);
        assert!(small <= 1024 && large <= small * 2 + 2, "{small} -> {large}");
    }

    /// Spatial pruning also bounds work when a page has many noncoincident masks.
    #[test]
    fn clipping_index_many_separated_masks_uses_spatial_pruning() {
        let mut lines = eligibility_lines(4096);
        for (i, line) in lines.iter_mut().enumerate() {
            line.clips = i % 2 == 0;
        }
        let mut index = ClippingIndex::new(&lines);
        let before = index.remaining;
        for line in lines.iter().filter(|line| !line.clips) {
            assert_eq!(index.refusal(&[line]), None);
        }
        assert!(before - index.remaining < lines.len() * 32);
    }

    /// Compare with the original operator-set oracle, including invisible copies with
    /// different text, shared operators, mixed modes and nonfinite geometry.
    #[test]
    fn clipping_index_agrees_with_rewrite_drop_sets() {
        let mut lines = eligibility_lines(160);
        for (i, line) in lines.iter_mut().enumerate() {
            let y = (i % 20) as f64 * 30.0;
            line.rect = [40.0, y, 90.0, y + 12.0];
            line.origin.baseline = y + (i % 3) as f64;
            line.text = format!("word {}", i % 7);
            line.origin.state.render_mode = (i % 8) as i64;
            line.clips = i % 11 == 0;
            line.size = 8.0 + (i % 5) as f64;
            if i % 13 == 0 {
                line.ops.push(0);
            }
            if i % 17 == 0 {
                line.rect[0] = f64::NAN;
            }
            if i % 19 == 0 {
                line.rect[2] = f64::INFINITY;
            }
            if i % 23 == 0 {
                line.rect.swap(0, 2);
            }
            if i % 29 == 0 {
                line.origin.tm = [0.0, 1.0, -1.0, 0.0, 0.0, 0.0];
            }
        }
        let mut index = ClippingIndex::new(&lines);
        for targets in lines.chunks(2) {
            let targets: Vec<_> = targets.iter().collect();
            let mut drops = coincident_ops(&lines, &targets);
            drops.extend(targets.iter().flat_map(|line| line.ops.iter().copied()));
            let expected = refuse_clipping_text(&lines, &drops).err().map(|_| CLIPPING_TEXT_REASON);
            assert_eq!(index.refusal(&targets), expected);
        }
        // A differently-worded invisible copy is still coincident with visible clipping text.
        let mut lines = eligibility_lines(2);
        lines[0].clips = true;
        lines[1].rect = lines[0].rect;
        lines[1].origin.baseline = lines[0].origin.baseline;
        lines[1].origin.state.render_mode = 3;
        lines[1].text = "different OCR text".into();
        assert_eq!(ClippingIndex::new(&lines).refusal(&[&lines[1]]), Some(CLIPPING_TEXT_REASON));
    }

    /// A nonmask copy can indirectly drop a mask through a shared operator ID.
    #[test]
    fn clipping_index_includes_indirect_shared_operator_copies() {
        let mut lines = eligibility_lines(3);
        lines[0].clips = true;
        lines[1].ops.push(0);
        lines[2].rect = lines[1].rect;
        lines[2].origin.baseline = lines[1].origin.baseline;
        assert!(!coincident_line(&lines[0], &lines[2]));
        assert!(coincident_line(&lines[1], &lines[2]));
        let drops = coincident_ops(&lines, &[&lines[2]]);
        assert!(refuse_clipping_text(&lines, &drops).is_err());
        assert_eq!(ClippingIndex::new(&lines).refusal(&[&lines[2]]), Some(CLIPPING_TEXT_REASON));
    }

    /// A dense adversarial page cannot spend an unbounded amount of work proving safety.
    #[test]
    fn clipping_index_budget_exhaustion_is_explicit_and_conservative() {
        let mut lines = eligibility_lines(100);
        for (i, line) in lines.iter_mut().enumerate() {
            line.rect = [40.0, 60.0, 90.0, 72.0];
            line.origin.baseline = 60.0;
            line.clips = i < 99;
            line.text = format!("unique {i}");
        }
        let mut index = ClippingIndex::new(&lines);
        index.remaining = 10;
        assert_eq!(index.refusal(&[&lines[99]]), Some(CLIPPING_SCAN_LIMIT_REASON));
        assert_eq!(index.remaining, 0);
        assert_eq!(index.refusal(&[&lines[99]]), Some(CLIPPING_SCAN_LIMIT_REASON));
        assert_eq!(index.refusal(&[&lines[0]]), Some(CLIPPING_TEXT_REASON));
    }

    /// The form-aware reading path must propagate every ActualText run's mask flag too.
    #[test]
    fn reading_lines_keeps_later_actual_text_clipping_runs() {
        for mode in 4..=7 {
            let doc = eligibility_page(&format!("/Span << /ActualText (logical) >> BDC BT /F1 12 Tf 40 60 Td (ink) Tj {mode} Tr (mask) Tj ET EMC"));
            let (lines, truncated) = reading_lines(&doc, 0).unwrap();
            assert!(!truncated);
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0].text, "logical");
            assert!(lines[0].clips, "mode {mode}");
        }
    }

    /// A synthetic paragraph for key matching and overlap tests.
    fn block(text: &str, rect: [f64; 4]) -> TextBlock {
        TextBlock {
            text: text.into(),
            rect,
            base_font: String::new(),
            size: 12.0,
            bold: false,
            italic: false,
            color: [0.0; 3],
            rotation: 0.0,
            lines: Vec::new(),
            rows: Vec::new(),
            editor_text: text.into(),
            reflow_text: text.into(),
            spans: Vec::new(),
            invisible: false,
            non_editable_reason: None,
        }
    }

    #[test]
    fn a_block_key_finds_its_paragraph_after_others_move_and_not_a_neighbour_in_its_place() {
        let (a, b, c) = (block("Alpha", [0.0, 80.0, 50.0, 100.0]), block("Bravo", [0.0, 50.0, 50.0, 60.0]), block("Charlie", [0.0, 0.0, 50.0, 20.0]));
        let key = BlockKey::of(&b);
        assert_eq!(key.find(&[a.clone(), b.clone(), c.clone()], 1), Some(1));
        // Alpha and Bravo became one paragraph: Bravo's number now belongs to Charlie.
        let merged = block("Alpha Bravo", [0.0, 50.0, 50.0, 100.0]);
        assert_eq!(key.find(&[merged, c.clone()], 1), None);
        // A paragraph that split in two shifts the others down.
        assert_eq!(key.find(&[a.clone(), block("x", [0.0; 4]), b.clone(), c], 1), Some(2));
        // The same words somewhere else, or other words in the same place, are not the paragraph.
        assert_eq!(key.find(&[block("Bravo", [0.0, 40.0, 50.0, 50.0])], 0), None);
        assert_eq!(key.find(&[block("Bravo!", b.rect)], 0), None);
        assert_eq!(key.find(&[], 0), None);
        // Two paragraphs drawn on top of one another are told apart by the hint.
        assert_eq!(key.find(&[b.clone(), b], 1), Some(1));
        // A box that is not a number matches nothing, and cannot panic.
        let nan = BlockKey::of(&block("Bravo", [f64::NAN; 4]));
        assert_eq!(nan.find(&[block("Bravo", [f64::NAN; 4])], 0), None);
    }

    #[test]
    fn the_block_that_overlaps_most_is_the_edited_paragraph() {
        let blocks = [block("a", [0.0, 80.0, 50.0, 100.0]), block("b", [0.0, 40.0, 50.0, 70.0])];
        assert_eq!(block_overlapping(&blocks, [0.0, 60.0, 50.0, 95.0]), Some(0));
        assert_eq!(block_overlapping(&blocks, [0.0, 35.0, 50.0, 75.0]), Some(1));
        assert_eq!(block_overlapping(&blocks, [200.0, 0.0, 250.0, 10.0]), None, "a box that overlaps nothing");
        assert_eq!(block_overlapping(&[], [0.0, 0.0, 1.0, 1.0]), None);
        assert_eq!(block_overlapping(&blocks, [f64::NAN; 4]), None);
    }

    #[test]
    fn a_subset_tag_is_the_only_difference_between_subsets_of_one_font() {
        assert!(same_base_font("ABCDEF+Arial", "GHIJKL+Arial"));
        assert!(same_base_font("Arial", "ABCDEF+Arial"));
        assert!(same_base_font("Arial", "Arial"));
        assert!(!same_base_font("ABCDEF+Arial", "GHIJKL+Helvetica"));
        // Five letters are part of the name, not a subset tag.
        assert!(!same_base_font("ABCDE+Helvetica-Bold", "Helvetica-Bold"));
        // A tag with no name after it, or no name at all, names no font.
        assert!(!same_base_font("ABCDEF+", "GHIJKL+"));
        assert!(!same_base_font("", ""));
        assert!(!same_base_font("Arial", ""));
    }

    fn wrapped(text: &str, width: f64) -> Vec<String> {
        wrap(text, width, |s| s.chars().count() as f64).into_iter().map(|(line, _, _)| line).collect()
    }

    #[test]
    fn paragraph_wrapping_keeps_ideographic_spaces() {
        assert_eq!(wrapped("A　　B C", 4.0), ["A　　B", "C"]);
        assert_eq!(wrapped("A\u{a0}B C", 3.0), ["A B", "C"]);
        assert_eq!(wrapped("A\u{2028}B C", 3.0), ["A B", "C"]);
        // `wrap` treats every whitespace character, including `\r`, as a separator to keep
        // (#4199); the caller turns `\r\n` and `\r` into `\n` before calling it.
        assert_eq!(wrapped(" \tA  B\r\nC ", 3.0), ["  A", "B ", "C "]);
    }

    #[test]
    fn a_wrap_is_marked_soft_and_a_newline_is_not() {
        let width = |s: &str| s.chars().count() as f64;
        assert_eq!(wrap("Page 1", 5.0, width), [("Page".into(), false, 0), ("1".into(), true, 1)]);
        assert_eq!(wrap("One\nTwo", 10.0, width), [("One".into(), false, 0), ("Two".into(), false, 0)]);
    }

    /// #2529: a run of spaces that happens to fall at a wrap point is recorded exactly, not
    /// assumed to be one space; a Chinese/Japanese word split, which has no space of its own,
    /// is recorded as none.
    #[test]
    fn a_wrap_records_how_many_spaces_it_consumed() {
        let width = |s: &str| s.chars().count() as f64;
        assert_eq!(wrap("One  Two", 5.0, width), [("One".into(), false, 0), ("Two".into(), true, 2)]);
        assert_eq!(wrap("One   Two", 5.0, width), [("One".into(), false, 0), ("Two".into(), true, 3)]);
        assert_eq!(wrap("日本語の文章です", 3.0, width), [("日本語".into(), false, 0), ("の文章".into(), true, 0), ("です".into(), true, 0)]);
    }

    /// #4199: a run of spaces stays a run, before, between and after words; the spaces where a
    /// row wraps go, and each other white-space character is one space.
    #[test]
    fn paragraph_wrapping_keeps_every_space_typed() {
        assert_eq!(wrapped("a  b   c", 20.0), ["a  b   c"]);
        assert_eq!(wrapped("  a b  ", 20.0), ["  a b  "]);
        assert_eq!(wrapped("a\t\tb", 20.0), ["a  b"]);
        assert_eq!(wrapped("aa  bb  cc", 6.0), ["aa  bb", "cc"]);
        // An indent alone on a row doesn't wrap before its first word.
        assert_eq!(wrapped("    abc", 3.0), ["    abc"]);
        // The rewrite turns `\r\n` and `\r` into `\n` before wrapping.
        assert_eq!(wrapped(" \tA  B\nC ", 3.0), ["  A", "B", "C "]);
        assert_eq!(wrapped("   ", 3.0), ["   "]);
        assert_eq!(wrapped("", 3.0), [""]);
    }

    #[test]
    fn japanese_text_wraps_between_characters_but_not_before_closing_marks() {
        assert_eq!(wrapped("日本語の文章です。", 3.0), ["日本語", "の文章", "です。"]);
        // The comma can't begin a row, so the character before it moves down with it.
        assert_eq!(wrapped("あいう、えお", 3.0), ["あい", "う、え", "お"]);
        // Pieces of one word join without a space; separate words keep theirs.
        assert_eq!(wrapped("Edit PDFを編集", 6.0), ["Edit", "PDFを編集"]);
        assert_eq!(wrapped("一行目　二行目", 4.0), ["一行目　", "二行目"]);
    }

    #[test]
    fn fallback_face_moves_on_when_the_best_face_lacks_a_character() {
        static JPAN: &[&str] = &["Jpan"];
        let mincho = CraftFont { family: "Mincho", style: "Regular", scripts: JPAN, bytes: b"" };
        let gothic = CraftFont { family: "Gothic", style: "Regular", scripts: JPAN, bytes: b"" };
        let faces = [&mincho, &gothic];
        // Only the Gothic face has the Cyrillic letter.
        let has = |face: &CraftFont, ch: char| ch.is_ascii() || face.family == "Gothic";
        assert_eq!(fallback_face(&faces, &['a', 'ф'], has).map(|f| f.family), Some("Gothic"));
        // Characters the best face has keep it.
        assert_eq!(fallback_face(&faces, &['a', 'b'], has).map(|f| f.family), Some("Mincho"));
        // No face has them all: the best face stays, and names the missing glyph.
        assert_eq!(fallback_face(&faces, &['a', 'ф'], |_, ch| ch.is_ascii()).map(|f| f.family), Some("Mincho"));
        assert!(fallback_face(&[], &['a'], has).is_none());
    }

    #[test]
    fn hebrew_and_japanese_fallback_shards_are_recognized_as_ours() {
        static HEBR: &[&str] = &["Hebr"];
        let face = CraftFont { family: "FakeHebrew", style: "Regular", scripts: HEBR, bytes: b"" };
        let glyph = |code, ch| Type3Glyph { code, ch, path: b"0 0 0 0 0 0 d1\nf\n".to_vec(), width: 0.5 };
        let mut doc = Document::new_empty();
        let mut fonts = Dict::new();
        // A replacement split across two Hebrew fonts, then one Japanese font.
        let he = add_type3_font(&mut doc, &mut fonts, &face, true, vec![glyph(1, 'א')]);
        let he1 = add_type3_font(&mut doc, &mut fonts, &face, true, vec![glyph(1, 'ב')]);
        let jp = add_type3_font(&mut doc, &mut fonts, &face, false, vec![glyph(1, 'あ')]);
        assert_eq!([he.as_str(), he1.as_str(), jp.as_str()], ["PCHe", "PCHe1", "PCJp"]);
        for name in [&he, &he1, &jp] {
            let font = fonts.get(name.as_bytes()).and_then(Object::as_dict).cloned().unwrap_or_default();
            assert!(is_type3_fallback(&font), "{name} isn't recognized as a generated fallback font");
        }
        // Someone else's Type 3 font is not ours.
        let mut other = Dict::new();
        other.set(b"Subtype".to_vec(), Object::name("Type3"));
        other.set(b"Name".to_vec(), Object::name("PCHebrewX"));
        assert!(!is_type3_fallback(&other));
    }
}
