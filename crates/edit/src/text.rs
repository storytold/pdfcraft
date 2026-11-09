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
pub use fallback::strip_subset_tag;
use regions::{alignment_frame, group_blocks, is_centred, rewrite_changed_words, rewrite_styled_block};
pub use session::TextSession;

mod blocks;
mod editor;
mod fallback;
pub mod forms;
mod interp;
mod lines;
mod rewrite;

#[cfg(test)]
use blocks::wrap;
pub use blocks::{
    BlockKey, BlockStyle, RangeStyle, ReadingBlocks, RunSources, TextBlock, block_overlapping, clear_block, reading_blocks, reading_blocks_checked,
    replace_block, text_blocks,
};
use blocks::{kept_words, paragraph_separator, row_break};
pub use editor::{TextEditLayout, TextEditorFont, register_text_font, text_edit_layout, text_editor_fonts};
use editor::{builtin_thai_choice, layout_draft};
#[cfg(test)]
pub(crate) use fallback::SOURCE_SPACE_CALLS;
use fallback::{
    MAX_TYPE3_GLYPHS, Type3Glyph, fallback_face, needs_type3, no_fallback_font, no_glyph_for, same_base_font, same_font, source_encodes,
    source_family, source_program, source_show, source_space, source_width, type3_faces, type3_glyph, write_type3_glyphs,
};
#[cfg(test)]
use fallback::{add_type3_font, is_type3_fallback};
pub(crate) use fallback::{glyph_path, type3_font_dict, unique_font_name};
use interp::{Origin, Paint, union};
pub(crate) use lines::{
    ContentStream, FormBudget, Read, content_streams, content_streams_within, dict_in_place, drawn_before_damage, first_op_after_damage, form_call,
    joined, resource_dict, rewritten_contents, splice,
};
pub use lines::{MAX_FORM_DEPTH, MAX_FORM_VISITS, glyph_text_lines, text_lines};
use lines::{page_dict, word_lines};
pub use rewrite::{LineEdit, replace_line, rewrite_block};
use rewrite::{coincident_ops, rewrite_block_from};
pub(crate) use rewrite::{paragraph_ops, reveal_ops, selectable_blocks, show_starts, translate_blocks};
#[cfg(test)]
pub(crate) use rewrite::{replace_line_with_limit, rewrite_block_with_limit};

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
    use super::*;

    /// The clip regions reading uses (#3503): intersected regions keep only what both cover,
    /// regions apart leave nothing, turned ones keep their shape, and touching is not showing.
    #[test]
    fn clip_regions_intersect_and_show_what_they_cover() {
        use interp::{intersect, region, shows};
        let square = |x: f64, y: f64| region(&Matrix::IDENTITY, [x, y, x + 10.0, y + 10.0]).unwrap();
        let area = |p: &[[f64; 2]]| p.iter().zip(p.iter().cycle().skip(1)).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum::<f64>().abs() / 2.0;
        // Squares overlapping by half each way share a quarter.
        let both = intersect(&square(0.0, 0.0), &square(5.0, 5.0));
        assert!((area(&both) - 25.0).abs() < 1e-9, "{both:?}");
        assert!(shows([6.0, 6.0, 7.0, 7.0], &both) && !shows([1.0, 1.0, 2.0, 2.0], &both));
        // Squares apart share nothing, so nothing shows, not even a box across both.
        let none = intersect(&square(0.0, 0.0), &square(20.0, 0.0));
        assert!(!shows([-100.0, -100.0, 100.0, 100.0], &none), "{none:?}");
        // Squares that meet only along an edge share no area either: a box across that edge
        // does not show (#5075 review).
        let edge = intersect(&square(0.0, 0.0), &square(10.0, 0.0));
        assert!(!shows([8.0, 2.0, 12.0, 4.0], &edge), "{edge:?}");
        // Nothing intersected with anything is nothing, whichever side it is on.
        assert!(!shows([1.0, 1.0, 2.0, 2.0], &intersect(&square(0.0, 0.0), &Vec::new())));
        assert!(!shows([1.0, 1.0, 2.0, 2.0], &intersect(&Vec::new(), &square(0.0, 0.0))));
        // A box touching an edge shares no area; one a hair inside does.
        assert!(!shows([10.0, 0.0, 12.0, 5.0], &square(0.0, 0.0)));
        assert!(shows([9.99, 0.0, 12.0, 5.0], &square(0.0, 0.0)));
        // A square turned 45° keeps its diamond shape: a box in the corner of its bounding box is
        // outside it, the centre is inside.
        let c = std::f64::consts::FRAC_1_SQRT_2;
        let diamond = region(&Matrix([c, c, -c, c, 0.0, 0.0]), [0.0, 0.0, 10.0, 10.0]).unwrap();
        assert!(!shows([-7.0, 0.0, -6.5, 0.5], &diamond) && shows([-0.5, 6.5, 0.5, 7.5], &diamond));
        // A corner repeated (an edge of no length) does not hide what the region covers.
        let doubled = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!(shows([4.0, 4.0, 6.0, 6.0], &doubled));
        // A rectangle of no area is a region that covers nothing; a corner that isn't finite
        // gives no region to clip with at all.
        let flat = region(&Matrix::IDENTITY, [0.0, 0.0, 10.0, 0.0]).unwrap();
        assert!(!shows([-100.0, -100.0, 100.0, 100.0], &flat), "{flat:?}");
        assert!(region(&Matrix::IDENTITY, [0.0, 0.0, f64::INFINITY, 10.0]).is_none());
        // However thin, a rectangle with area keeps it, and so does one far from the origin.
        let thin = region(&Matrix::IDENTITY, [0.0, 0.0, 1.0, 1e-10]).unwrap();
        assert!(shows([0.25, 0.0, 0.75, 1.0], &thin) && !shows([0.25, 1.0, 0.75, 2.0], &thin));
        let far = region(&Matrix([1.0, 0.0, 0.0, 1.0, 1e12, 1e12]), [0.0, 0.0, 100.0, 100.0]).unwrap();
        assert!(shows([1e12 + 10.0, 1e12 + 10.0, 1e12 + 20.0, 1e12 + 20.0], &far) && !shows([1e12 + 200.0, 1e12, 1e12 + 300.0, 1e12 + 10.0], &far));
    }

    /// A stream that sets clip after clip, each turned a little more, keeps its region bounded:
    /// past the corner cap a region becomes its bounding box, which still holds all it held.
    #[test]
    fn clip_after_clip_keeps_the_region_bounded() {
        use interp::{intersect, region, shows};
        let mut clip = region(&Matrix::IDENTITY, [-100.0, -100.0, 100.0, 100.0]).unwrap();
        for i in 1..=10_000 {
            let (s, c) = (f64::from(i) * 0.001).sin_cos();
            let turned = region(&Matrix([c, s, -s, c, 0.0, 0.0]), [-60.0, -60.0, 60.0, 60.0]).unwrap();
            clip = intersect(&clip, &turned);
            assert!(clip.len() <= 32 && clip.iter().flatten().all(|v| v.is_finite()), "clip {i}: {} corners", clip.len());
        }
        // Every square is centred on the origin, so the middle still shows.
        assert!(shows([-1.0, -1.0, 1.0, 1.0], &clip));
    }

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
    fn thai_text_wraps_between_dictionary_words_and_keeps_marks_on_their_word() {
        // Without word breaks the whole run is one word and overflows a single row.
        assert_eq!(wrapped("ภาษาไทยไม่มีช่องว่าง", 8.0), ["ภาษาไทย", "ไม่มี", "ช่องว่าง"]);
        // Mai Yamok stays with the word it repeats; pieces join without a space, words keep theirs.
        assert_eq!(wrapped("เด็กๆชอบกิน ขนม", 6.0), ["เด็กๆ", "ชอบกิน", "ขนม"]);
        assert_eq!(wrapped("ไฟล์ PDF ประปาส่วนภูมิภาค", 9.0), ["ไฟล์ PDF", "ประปาส่วน", "ภูมิภาค"]);
        // A wrap between Thai words consumed no separator, so a rejoin puts back no space.
        let rows = wrap("ภาษาไทยไม่มีช่องว่าง", 8.0, |s| s.chars().count() as f64);
        assert_eq!(rows.iter().map(|(_, soft, gap)| (*soft, *gap)).collect::<Vec<_>>(), [(false, 0), (true, 0), (true, 0)]);
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
