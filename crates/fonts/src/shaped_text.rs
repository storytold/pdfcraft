//! Shaped text written into a PDF: each distinct shaped cluster (a letter with its marks, and the
//! text it stands for) becomes one glyph of a Type 3 font drawn from the craft-fonts face's
//! outlines, with a `/ToUnicode` map so copy and search get each character once. No font file is
//! embedded.
//!
//! [`line_pieces`] lays a line out for drawing: the Unicode bidirectional algorithm, then Arabic,
//! Hebrew and Thai shaped in their faces beside the standard font that draws the rest. Add text,
//! text boxes (comments, callouts), Fill & Sign text, form fields, headers, footers and watermarks
//! all draw through it, so the same text comes out the same in each. Captions, labels and stamps
//! are Helvetica only, and measure their Thai alone ([`thai_width`]).
//!
//! Without a script's face, a text box draws `?` and marks its comment
//! ([`first_undrawable_shaped`]); Add text, form fields, headers, footers and watermarks refuse the
//! text, saying what to do ([`shape_refusal`]).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use pdfcraft_cos::{Dict, Document, Object, PdfString, Stream};

use unicode_bidi::{Level, ParagraphBidiInfo};

use crate::{
    GlyphError, GlyphOutline, ShapedCluster, ShapedScript, latin_has, literal, shape_arabic, shape_hebrew, shape_latin, shape_thai, shaped_glyph,
};

/// Most glyphs in one Type 3 font: its codes are single bytes, 1–240. Text gets one such font per
/// 240 distinct clusters it needs.
pub const GLYPHS_PER_FONT: usize = 240;

/// The Thai block (U+0E00–U+0E7F): letters, vowels, tone marks, digits and signs.
pub fn is_thai(c: char) -> bool {
    matches!(u32::from(c), 0x0E00..=0x0E7F)
}

/// Arabic letters, marks, digits and punctuation (and the presentation forms): shaped with the
/// craft-fonts Arabic face.
pub fn is_arabic(c: char) -> bool {
    matches!(u32::from(c), 0x0600..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFC)
}

/// Hebrew letters, points, cantillation marks and punctuation (and the presentation forms): drawn
/// with the craft-fonts Hebrew face.
pub fn is_hebrew(c: char) -> bool {
    matches!(u32::from(c), 0x0591..=0x05F4 | 0xFB1D..=0xFB4F)
}

/// Direction marks, embeddings, overrides and isolates: they steer the order and are not drawn.
pub fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// The zero-width joiner and non-joiner: they choose the forms of the letters beside them and are
/// not drawn.
fn is_joiner(c: char) -> bool {
    matches!(c, '\u{200C}' | '\u{200D}')
}

/// `line`, none of it shaped, as the standard font draws it: without its direction controls when
/// they leave every character in typing order, else as it is (the font then draws a control as `?`,
/// and [`line_undrawable`] reports it).
fn standard_text(line: &str) -> Cow<'_, str> {
    if line.contains(is_bidi_control) && Line::new(line, false).levels().iter().all(Level::is_ltr) {
        return Cow::Owned(without_controls(line));
    }
    Cow::Borrowed(line)
}

/// `line` without its direction controls.
fn without_controls(line: &str) -> String {
    line.chars().filter(|c| !is_bidi_control(*c)).collect()
}

/// Whether `text` has a character [`line_pieces`] shapes in a script face (Arabic, Hebrew or Thai).
pub fn has_shaped_script(text: &str) -> bool {
    text.chars().any(|c| is_thai(c) || is_arabic(c) || is_hebrew(c))
}

/// Whether the paragraph `para` reads right to left: it has Arabic or Hebrew, and its first
/// strong character is right to left (UAX #9 rules P2 and P3).
pub fn rtl_base(para: &str) -> bool {
    para.chars().any(|c| is_arabic(c) || is_hebrew(c)) && unicode_bidi::get_base_direction(para) == unicode_bidi::Direction::Rtl
}

/// Marks drawn on the letter before them, which a line never breaks before: Thai vowel signs and
/// tone marks (and Sara Am, which the face splits over the consonant), Arabic vowel and Koranic
/// marks, and Hebrew points and cantillation marks.
pub fn joins_previous(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0E31 | 0x0E33..=0x0E3A | 0x0E47..=0x0E4E
            | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7 | 0x06E8 | 0x06EA..=0x06ED
            | 0x0897..=0x089F | 0x08CA..=0x08E1 | 0x08E3..=0x08FF
            | 0x0591..=0x05BD | 0x05BF | 0x05C1 | 0x05C2 | 0x05C4 | 0x05C5 | 0x05C7
    )
}

/// Why text in `script` can't be drawn, for a message an agent or user can act on.
pub fn script_message(script: ShapedScript, e: GlyphError, text: &str) -> String {
    let (name, tag, article) = match script {
        ShapedScript::Arabic => ("Arabic", "Arab", "an"),
        ShapedScript::Hebrew => ("Hebrew", "Hebr", "a"),
        ShapedScript::Thai => return thai_message(e, text),
        ShapedScript::Latin => return latin_message(e, text),
    };
    match e {
        GlyphError::NoFont => format!(
            "{name} text needs PdfCraft's {name} font, which this build doesn't include (to build it in, set CRAFT_FONTS_DIR to a craft-fonts \
             checkout that has {article} {tag} face)"
        ),
        GlyphError::Missing => format!("the {name} font can't show \"{text}\""),
        GlyphError::TooComplex => format!("the {name} font's glyphs for \"{text}\" are too complex"),
    }
}

/// Text a script face couldn't shape: which script, why, and the run it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShapeError {
    pub script: ShapedScript,
    pub error: GlyphError,
    pub text: String,
}

impl ShapeError {
    /// [`script_message`] for this failure.
    pub fn message(&self) -> String {
        script_message(self.script, self.error, &self.text)
    }
}

/// Why Thai text can't be drawn, for a message an agent or user can act on.
pub fn thai_message(e: GlyphError, text: &str) -> String {
    match e {
        GlyphError::NoFont => "Thai text needs a Thai font, and this build has none; rebuild with CRAFT_FONTS_DIR pointing to a craft-fonts \
                               checkout with a Thai face"
            .into(),
        GlyphError::Missing => format!("the Thai font can't show \"{text}\""),
        GlyphError::TooComplex => "the Thai glyphs are too complex; use shorter text".into(),
    }
}

/// Whether a generated appearance draws `c` from the bundled Inter face rather than the standard
/// font: a Latin, Greek or Cyrillic character WinAnsiEncoding lacks (`Γ`, `ř`, `ж`) that the face
/// has. Line breaks and Thai never are.
pub fn latin_fallback(c: char) -> bool {
    !matches!(c, '\n' | '\r') && !is_thai(c) && !crate::win_ansi_encodable(c) && latin_has(c)
}

/// Why the bundled Inter face can't draw `text`, for a message an agent or user can act on.
pub fn latin_message(e: GlyphError, text: &str) -> String {
    match e {
        GlyphError::NoFont | GlyphError::Missing => format!("the bundled Inter face can't show \"{text}\""),
        GlyphError::TooComplex => format!("the bundled Inter face's glyphs for \"{text}\" are too complex"),
    }
}

/// `line` split as a generated appearance draws it, in typing order: with `thai`, its Thai runs
/// shaped with the Thai face; with `latin`, its runs of [`latin_fallback`] characters shaped with
/// the bundled Inter face; and the text between them, left to the standard font. An error when a
/// run can't be shaped (no Thai face, a missing character).
pub fn fallback_pieces(line: &str, thai: bool, latin: bool) -> Result<Vec<Piece>, GlyphError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Run {
        Thai,
        Latin,
        Standard,
    }
    let kind = |c: char| {
        if thai && is_thai(c) {
            Run::Thai
        } else if latin && latin_fallback(c) {
            Run::Latin
        } else {
            Run::Standard
        }
    };
    let mut runs: Vec<(Run, String)> = Vec::new();
    for c in line.chars() {
        let k = kind(c);
        // A combining mark the standard font lacks is drawn on its letter by the bundled face:
        // the letter moves from the standard font's run into a run of the face's.
        if k == Run::Latin
            && is_mark(c)
            && let Some((Run::Standard, s)) = runs.last_mut()
            && let Some(base) = s.chars().last().filter(|b| latin_has(*b) && !matches!(b, '\n' | '\r'))
        {
            s.pop();
            if s.is_empty() {
                runs.pop();
            }
            match runs.last_mut() {
                Some((Run::Latin, s)) => s.extend([base, c]),
                _ => runs.push((Run::Latin, [base, c].into_iter().collect())),
            }
            continue;
        }
        match runs.last_mut() {
            Some((run, s)) if *run == k => s.push(c),
            _ => runs.push((k, c.to_string())),
        }
    }
    runs.into_iter()
        .map(|(run, s)| match run {
            Run::Thai => shape_thai(&s).map(Piece::Shaped),
            Run::Latin => shape_latin(&s).map(Piece::Shaped),
            Run::Standard => Ok(Piece::Latin(s)),
        })
        .collect()
}

/// A combining diacritical mark (U+0300–U+036F), drawn on the letter before it.
fn is_mark(c: char) -> bool {
    matches!(u32::from(c), 0x0300..=0x036F)
}

/// The width of `text` at `size` as [`fallback_pieces`] splits it, the standard font's pieces
/// measured by `standard`. Text with nothing for the fallback faces, or that they can't shape, is
/// measured by `standard` as a whole (drawing then reports why).
pub fn fallback_width(text: &str, size: f64, thai: bool, latin: bool, standard: impl Fn(&str, f64) -> f64) -> f64 {
    if !text.chars().any(|c| (thai && is_thai(c)) || (latin && latin_fallback(c))) {
        return standard(text, size);
    }
    match fallback_pieces(text, thai, latin) {
        Ok(pieces) => pieces_width(&pieces, size, 0.0, standard),
        Err(_) => standard(text, size),
    }
}

/// A piece of a line of shaped text, in drawing order.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    /// Shown in the text's own standard font.
    Latin(String),
    Shaped(Vec<ShapedCluster>),
}

fn mirrored(c: char) -> char {
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

/// The embedding level a paragraph that reads right to left when `rtl` starts at.
fn base_level(rtl: bool) -> Level {
    if rtl { Level::rtl() } else { Level::ltr() }
}

/// A line of text as its paragraph lays it out: what it says, whether its paragraph reads right
/// to left ([`rtl_base`]), and the embedding levels its paragraph resolved for it. UAX #9 (§3.4)
/// resolves levels for a whole paragraph and only then reorders each line, so a line a paragraph
/// wraps onto keeps what is open before it (an override, embedding or isolate) and what its
/// neighbours resolve. [`wrap`] gives such lines; [`Line::new`] is a line that is its own
/// paragraph, as an unwrapped line is.
///
/// Measuring ([`Line::width`]), drawing ([`Line::appearance_pieces`]), fit checks
/// ([`Line::reach`]) and `/ActualText` ([`Line::reordered`]) all read the same levels, so they
/// agree on every line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub text: String,
    pub rtl: bool,
    /// The level of each byte of `text`, resolved over its paragraph, with this line's own
    /// trailing white space reset (rule L1); empty when the line is resolved alone (its own
    /// paragraph, or one with nothing right to left in it).
    levels: Vec<Level>,
}

impl Line {
    /// `text` as a paragraph of its own that reads right to left when `rtl`.
    pub fn new(text: impl Into<String>, rtl: bool) -> Self {
        Self { text: text.into(), rtl, levels: Vec::new() }
    }

    /// `lines`, wrapped in order from the paragraph `para` (each a run of it, the spaces where it
    /// broke dropped between them), with the levels `para` resolves for them in a paragraph that
    /// reads right to left when `rtl`. A line that isn't found where it should be is resolved
    /// alone.
    pub fn in_paragraph(para: &str, rtl: bool, lines: Vec<String>) -> Vec<Line> {
        // unicode-bidi indexes the first level of a paragraph, so an empty one would panic.
        let info = (!para.is_empty()).then(|| ParagraphBidiInfo::new(para, Some(base_level(rtl))));
        // A paragraph with nothing right to left orders each line as it would alone.
        let Some(info) = info.filter(ParagraphBidiInfo::has_rtl) else {
            return lines.into_iter().map(|text| Line::new(text, rtl)).collect();
        };
        let mut from = 0;
        lines
            .into_iter()
            .map(|text| {
                let start = if text.is_empty() { None } else { para.get(from..).and_then(|rest| rest.find(&text)).map(|i| from.saturating_add(i)) };
                let Some(start) = start else { return Line::new(text, rtl) };
                let end = start.saturating_add(text.len());
                from = end;
                // The paragraph's levels and classes for just this line, so that rule L1 (which
                // reads nothing else) runs on the line alone, without copying the whole
                // paragraph's levels for every line it wraps onto.
                let (Some(levels), Some(classes)) = (info.levels.get(start..end), info.original_classes.get(start..end)) else {
                    return Line::new(text, rtl);
                };
                let line = ParagraphBidiInfo {
                    text: &text,
                    original_classes: classes.to_vec(),
                    levels: levels.to_vec(),
                    paragraph_level: info.paragraph_level,
                    is_pure_ltr: false,
                };
                let levels = line.reordered_levels(0..text.len());
                if levels.len() == text.len() { Line { text, rtl, levels } } else { Line::new(text, rtl) }
            })
            .collect()
    }

    /// The level of each byte of the line, ready to reorder (rule L1 and those before it): its
    /// paragraph's, or its own as a paragraph.
    fn levels(&self) -> Cow<'_, [Level]> {
        if !self.levels.is_empty() && self.levels.len() == self.text.len() {
            return Cow::Borrowed(&self.levels);
        }
        if self.text.is_empty() {
            return Cow::Owned(Vec::new());
        }
        Cow::Owned(ParagraphBidiInfo::new(&self.text, Some(base_level(self.rtl))).reordered_levels(0..self.text.len()))
    }

    /// Whether its paragraph leaves every character of the line in typing order: no level is
    /// right to left. (A line resolved alone answers by its own text instead.)
    fn in_typing_order(&self) -> bool {
        self.levels.iter().all(|l| l.is_ltr())
    }

    /// The line as the standard font draws it, when nothing of it is shaped or reordered: its
    /// paragraph keeps it in typing order, or it does on its own ([`standard_text`]).
    fn standard(&self) -> Option<Cow<'_, str>> {
        if self.rtl || has_shaped_script(&self.text) || !self.in_typing_order() {
            return None;
        }
        Some(if self.levels.is_empty() { standard_text(&self.text) } else { Cow::Owned(without_controls(&self.text)) })
    }

    /// The line in drawing order: [`line_pieces`], with its paragraph's levels.
    pub fn pieces(&self) -> Result<Vec<Piece>, ShapeError> {
        pieces_by_levels(&self.text, &self.levels())
    }

    /// [`appearance_pieces`], with its paragraph's levels.
    pub fn appearance_pieces(&self) -> Vec<Piece> {
        if let Some(text) = self.standard() {
            return plain(&text);
        }
        self.pieces().or_else(|_| thai_pieces(&self.text)).unwrap_or_else(|_| plain(&self.text))
    }

    /// [`line_width`], with its paragraph's levels.
    pub fn width(&self, size: f64, latin: impl Fn(&str, f64) -> f64) -> f64 {
        if let Some(text) = self.standard() {
            return latin(&text, size);
        }
        pieces_width(&self.appearance_pieces(), size, 0.0, latin)
    }

    /// [`line_reach`], with its paragraph's levels.
    pub fn reach(&self) -> Option<[f64; 2]> {
        if !has_shaped_script(&self.text) {
            return None;
        }
        pieces_reach(&self.appearance_pieces())
    }

    /// Whether `pieces`, this line laid out, are painted in an order other than the one typed, so
    /// that the line needs its typing order beside it ([`actual_text_span`]): right-to-left text
    /// shaped in its face ([`reordered`]), or anything else a right-to-left level reversed. A line
    /// whose drawing order reads back to what was typed on its own ([`Self::reads_back_from_drawing`])
    /// doesn't.
    pub fn reordered(&self, pieces: &[Piece]) -> bool {
        if self.reads_back_from_drawing() {
            return false;
        }
        reordered(pieces) || !drawn_chars(pieces).filter(|c| !is_bidi_control(*c)).eq(self.text.chars().filter(|c| !is_bidi_control(*c)))
    }

    /// Whether the line's drawing order gives back its typing order with no other order possible:
    /// a line of a right-to-left paragraph with no left-to-right letter and no direction control,
    /// its right-to-left letters at the paragraph's level and its numbers one above (UAX #9 with no
    /// embedding or override reaching it). A reader rebuilds such a line from its glyphs alone, so
    /// it is written without `/ActualText`: some readers (poppler's) turn the `/ActualText` of
    /// right-to-left text around again, and read it backwards.
    fn reads_back_from_drawing(&self) -> bool {
        use unicode_bidi::{BidiClass, bidi_class};
        if !self.rtl || self.text.chars().any(|c| is_bidi_control(c) || bidi_class(c) == BidiClass::L) {
            return false;
        }
        let levels = self.levels();
        let mut any_rtl = false;
        let fits = self.text.char_indices().all(|(i, c)| {
            let Some(level) = levels.get(i) else { return false };
            match bidi_class(c) {
                BidiClass::R | BidiClass::AL => {
                    any_rtl = true;
                    level.number() == 1
                }
                _ => level.number() <= 2,
            }
        });
        fits && any_rtl
    }
}

/// The characters `pieces` stand for, in drawing order.
fn drawn_chars(pieces: &[Piece]) -> impl Iterator<Item = char> + '_ {
    pieces.iter().flat_map(|p| -> Box<dyn Iterator<Item = char> + '_> {
        match p {
            Piece::Latin(s) => Box::new(s.chars()),
            Piece::Shaped(clusters) => Box::new(clusters.iter().flat_map(|c| c.text.chars())),
        }
    })
}

/// The runs of equal level of a line whose bytes are at `levels`, in drawing order (rule L2: from
/// the highest level down to the lowest odd one, each stretch of runs at that level or higher is
/// reversed), as unicode-bidi orders the runs of a line, here for levels its paragraph resolved.
fn visual_runs(levels: &[Level]) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    for (i, level) in levels.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if levels.get(run.start) == Some(level) => run.end = i.saturating_add(1),
            _ => runs.push(i..i.saturating_add(1)),
        }
    }
    let level_of = |run: &Range<usize>| levels.get(run.start).map_or(0, Level::number);
    let highest = runs.iter().map(level_of).max().unwrap_or(0);
    let lowest_odd = runs.iter().map(level_of).min().map_or(1, |l| l | 1);
    let mut level = highest;
    while level >= lowest_odd {
        let mut i = 0;
        while i < runs.len() {
            if runs.get(i).is_some_and(|run| level_of(run) < level) {
                i += 1;
                continue;
            }
            let mut end = i.saturating_add(1);
            while runs.get(end).is_some_and(|run| level_of(run) >= level) {
                end = end.saturating_add(1);
            }
            if let Some(stretch) = runs.get_mut(i..end) {
                stretch.reverse();
            }
            i = end;
        }
        // `level` is at least `lowest_odd`, which is odd, so at least 1: the loop ends at 0.
        level = level.saturating_sub(1);
    }
    runs
}

/// `line` in drawing order (the Unicode bidirectional algorithm, UAX #9), in a paragraph that
/// reads right to left when `rtl` ([`rtl_base`]): its Arabic, Hebrew and Thai shaped in their
/// faces, the rest left to the standard font. A right-to-left run comes out reversed, its brackets
/// mirrored; direction marks steer the order and are dropped. The line is a paragraph of its own;
/// a line a paragraph wraps onto is laid out with that paragraph's levels ([`Line::pieces`]).
///
/// The one layout for text PdfCraft writes: Add text, text boxes, Fill & Sign text, form fields,
/// headers, footers and watermarks call it, so they agree on what is shaped, in which face, and
/// in what order.
pub fn line_pieces(line: &str, rtl: bool) -> Result<Vec<Piece>, ShapeError> {
    Line::new(line, rtl).pieces()
}

/// `line` in drawing order, its bytes at `levels` (one each, ready to reorder).
fn pieces_by_levels(line: &str, levels: &[Level]) -> Result<Vec<Piece>, ShapeError> {
    if line.is_empty() {
        return Ok(Vec::new());
    }
    // [`Line::levels`] gives one per byte; anything else is left in typing order, never dropped.
    if levels.len() != line.len() {
        return Ok(plain(line));
    }
    let runs = visual_runs(levels);
    let mut pieces = Vec::new();
    for run in runs {
        let run_rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        // Runs lie on character boundaries; a missing slice is skipped, never a panic.
        let Some(text) = line.get(run) else { continue };
        // Arabic and Hebrew letters take their face; so do spaces and punctuation between them
        // (Hebrew's when they follow Hebrew), which the shaper then mirrors. Latin text (and what
        // the faces lack) keeps the standard font.
        let mut segments: Vec<(Option<ShapedScript>, String)> = Vec::new();
        for c in text.chars().filter(|c| !is_bidi_control(*c)) {
            let after_hebrew = segments.last().is_some_and(|(kind, _)| *kind == Some(ShapedScript::Hebrew));
            let script = if is_thai(c) {
                Some(ShapedScript::Thai)
            } else if is_hebrew(c) || (run_rtl && after_hebrew && !c.is_alphanumeric() && crate::hebrew_has(c)) {
                Some(ShapedScript::Hebrew)
            } else if is_arabic(c) || (run_rtl && !c.is_alphanumeric() && crate::arabic_has(c)) {
                Some(ShapedScript::Arabic)
            } else {
                None
            };
            // A joiner beside Latin text has no letter to join there.
            if script.is_none() && is_joiner(c) {
                continue;
            }
            match segments.last_mut() {
                Some((kind, s)) if *kind == script => s.push(c),
                _ => segments.push((script, c.to_string())),
            }
        }
        if run_rtl {
            segments.reverse();
        }
        for (script, s) in segments {
            pieces.push(if let Some(script) = script {
                let shaped = match script {
                    ShapedScript::Arabic => shape_arabic(&s, run_rtl),
                    ShapedScript::Hebrew => shape_hebrew(&s, run_rtl),
                    ShapedScript::Thai => shape_thai(&s),
                    // Not a script this split gives out; shaped as such all the same.
                    ShapedScript::Latin => shape_latin(&s),
                };
                Piece::Shaped(shaped.map_err(|error| ShapeError { script, error, text: s })?)
            } else if run_rtl {
                Piece::Latin(s.chars().rev().map(mirrored).collect())
            } else {
                Piece::Latin(s)
            });
        }
    }
    Ok(pieces)
}

/// A line left whole to the standard font.
fn plain(line: &str) -> Vec<Piece> {
    vec![Piece::Latin(line.to_string())]
}

/// [`line_pieces`] for a generated appearance, which draws what it can rather than refusing: a
/// line a face can't shape (this build has none, or it lacks a character) keeps its Thai shaped,
/// as before Arabic and Hebrew were drawn ([`thai_pieces`]), and is otherwise left in typing order
/// to the standard font, whose encoder writes `?` for what it can't draw.
/// [`first_undrawable_shaped`] reports that. Measuring, sizing and drawing all go through this, so
/// they agree on every line.
pub fn appearance_pieces(line: &str, rtl: bool) -> Vec<Piece> {
    if !rtl && !has_shaped_script(line) {
        return plain(&standard_text(line));
    }
    line_pieces(line, rtl).or_else(|_| thai_pieces(line)).unwrap_or_else(|_| plain(line))
}

/// The width of `text` at `size` as an appearance draws it ([`appearance_pieces`]) in a paragraph
/// of direction `rtl`: shaped text by its clusters' advances, the rest measured by `latin`.
pub fn line_width(text: &str, rtl: bool, size: f64, latin: impl Fn(&str, f64) -> f64) -> f64 {
    if !rtl && !has_shaped_script(text) {
        return latin(&standard_text(text), size);
    }
    pieces_width(&appearance_pieces(text, rtl), size, 0.0, latin)
}

/// The widest a space between two words of `para` can be drawn at `size`: the standard font's,
/// or, in a paragraph with Arabic or Hebrew, the script face's where that is wider. Which of them
/// draws a space depends on the words around it (it takes the face inside a right-to-left run), so
/// wrapping counts the widest and a wrapped line never comes out wider than it was measured.
fn space_width(para: &str, size: f64, latin: &impl Fn(&str, f64) -> f64) -> f64 {
    let faces = [(is_arabic as fn(char) -> bool, shape_arabic as fn(&str, bool) -> _), (is_hebrew, shape_hebrew)];
    faces
        .into_iter()
        .filter(|(has, _)| para.chars().any(has))
        .filter_map(|(_, shape)| shape(" ", true).ok())
        .map(|clusters| clusters.iter().map(|c| c.advance * size).sum::<f64>())
        .fold(latin(" ", size), f64::max)
}

/// `text` wrapped `width` wide at `size` for a box that draws its lines with [`line_pieces`]: each
/// line with whether its paragraph reads right to left ([`rtl_base`]; every line of a paragraph
/// takes its paragraph's direction) and the levels its paragraph resolves for it ([`Line`]), so a
/// line keeps an override, embedding or isolate opened on an earlier one. Words are measured as
/// they will be drawn ([`line_width`]), the spaces between them at their widest (`space_width`),
/// and a mark stays on its letter.
pub fn wrap(text: &str, size: f64, width: f64, latin: impl Fn(&str, f64) -> f64) -> Vec<Line> {
    let mut out = Vec::new();
    // A CRLF pair is one break, a lone CR or LF is one break each (as `wrap_with` reads them).
    for para in text.replace("\r\n", "\n").split(['\n', '\r']) {
        let rtl = rtl_base(para);
        let space = space_width(para, size, &latin);
        let measure = |s: &str, size: f64| if s == " " { space } else { line_width(s, rtl, size, &latin) };
        out.extend(Line::in_paragraph(para, rtl, crate::wrap_with(para, size, width, measure)));
    }
    out
}

/// The marked-content operator that opens a line painted in drawing order and gives its typing
/// order (`/ActualText`, ISO 32000-2 §14.9.4), so copy and search read `line` as it was typed,
/// marks, ligatures, digits and mixed-script punctuation included. The caller closes it with `EMC`.
pub fn actual_text_span(line: &str) -> String {
    let logical: String = line.encode_utf16().map(|u| format!("{u:04X}")).collect();
    format!("/Span <</ActualText <FEFF{logical}>>> BDC\n")
}

/// `line` split into its Thai runs, shaped with the Thai face, and the text between them, left to
/// the standard font. Thai runs left to right, so the pieces are in typing order.
/// [`GlyphError::NoFont`] when there's Thai and this build has no Thai face. What
/// [`appearance_pieces`] falls back to, and how Helvetica-only text measures ([`thai_width`]).
fn thai_pieces(line: &str) -> Result<Vec<Piece>, GlyphError> {
    let mut runs: Vec<(bool, String)> = Vec::new();
    for c in line.chars() {
        match runs.last_mut() {
            Some((thai, s)) if *thai == is_thai(c) => s.push(c),
            _ => runs.push((is_thai(c), c.to_string())),
        }
    }
    runs.into_iter().map(|(thai, s)| if thai { shape_thai(&s).map(Piece::Shaped) } else { Ok(Piece::Latin(s)) }).collect()
}

/// The width of `text` at `size`: its Thai shaped, the rest measured by `latin`. Text the Thai
/// face can't shape is measured by `latin` as a whole (drawing then reports why). For text that
/// is drawn in Helvetica alone (captions, labels, stamps); what [`line_pieces`] draws is measured
/// by [`line_width`].
pub fn thai_width(text: &str, size: f64, latin: impl Fn(&str, f64) -> f64) -> f64 {
    if !text.chars().any(is_thai) {
        return latin(text, size);
    }
    match thai_pieces(text) {
        Ok(pieces) => pieces_width(&pieces, size, 0.0, latin),
        Err(_) => latin(text, size),
    }
}

/// The width of `pieces` at `size`, with `spacing` after each shaped cluster (as `Tc` moves the
/// pen; the caller measures it into `latin`'s widths).
pub fn pieces_width(pieces: &[Piece], size: f64, spacing: f64, latin: impl Fn(&str, f64) -> f64) -> f64 {
    pieces
        .iter()
        .map(|p| match p {
            Piece::Latin(s) => latin(s, size),
            // Each cluster is shown on its own, so the spacing follows each once.
            Piece::Shaped(clusters) => clusters.iter().map(|c| c.advance * size + spacing).sum::<f64>(),
        })
        .sum()
}

/// The shaped clusters of `lines`' pieces, for [`ShapedFonts::new`].
pub fn clusters<'a>(lines: impl IntoIterator<Item = &'a [Piece]>) -> impl Iterator<Item = &'a ShapedCluster> {
    lines.into_iter().flatten().flat_map(|p| match p {
        Piece::Shaped(clusters) => clusters.as_slice(),
        Piece::Latin(_) => &[],
    })
}

/// What makes two clusters the same Type 3 glyph: text, glyphs, offsets and advance (bit for bit;
/// equal clusters come from the same font units).
type ClusterKey = (ShapedScript, String, Vec<(u32, u64, u64)>, u64);

fn cluster_key(c: &ShapedCluster) -> ClusterKey {
    (c.script, c.text.clone(), c.glyphs.iter().map(|(id, [x, y])| (*id, x.to_bits(), y.to_bits())).collect(), c.advance.to_bits())
}

/// The Type 3 fonts that draw a set of shaped clusters, one glyph per distinct cluster. The
/// default has none (for text with nothing shaped).
#[derive(Default)]
pub struct ShapedFonts {
    index: HashMap<ClusterKey, usize>,
    /// Each font's resource name.
    names: Vec<String>,
}

impl ShapedFonts {
    /// Build the fonts for `clusters` into `fonts`, under names starting `prefix` that neither
    /// `taken` (the page's fonts) nor `fonts` already has. `outline` gives a glyph's outline in its
    /// script's face (the craft-fonts faces, or a stand-in in tests). `store` keeps each new
    /// object and returns what refers to it: `doc.add` as a reference, or the object itself for
    /// an appearance that isn't in a document yet, whose streams [`store_streams`] then moves in.
    ///
    /// Every outline is read before anything is stored, so a failure (and the cluster it came
    /// from) leaves the document as it was.
    pub fn new<'a>(
        clusters: impl IntoIterator<Item = &'a ShapedCluster>,
        outline: impl Fn(ShapedScript, u32) -> Result<GlyphOutline, GlyphError>,
        taken: &Dict,
        fonts: &mut Dict,
        prefix: &str,
        store: &mut dyn FnMut(Object) -> Object,
    ) -> Result<Self, (GlyphError, &'a ShapedCluster)> {
        let mut keys: Vec<&ShapedCluster> = Vec::new();
        let mut index: HashMap<ClusterKey, usize> = HashMap::new();
        for c in clusters {
            index.entry(cluster_key(c)).or_insert_with(|| {
                keys.push(c);
                keys.len() - 1
            });
        }
        let mut outlines = HashMap::new();
        for c in &keys {
            for (id, _) in &c.glyphs {
                if let std::collections::hash_map::Entry::Vacant(entry) = outlines.entry((c.script, *id)) {
                    entry.insert(outline(c.script, *id).map_err(|e| (e, *c))?);
                }
            }
        }
        // A name no font on the page has, so items never replace each other's fonts (object
        // numbers alone don't do: a full save renumbers objects but keeps resource names).
        let mut taken_names: HashSet<Vec<u8>> = taken.iter().chain(fonts.iter()).map(|(name, _)| name.clone()).collect();
        let mut names = Vec::new();
        for (i, chunk) in keys.chunks(GLYPHS_PER_FONT).enumerate() {
            let font = type3_font(chunk, &outlines, store);
            let font = store(Object::Dict(font));
            let number = font.as_ref().map_or(i + 1, |r| r.num as usize);
            let mut name = format!("{prefix}{number}");
            let mut suffix = 0usize;
            while taken_names.contains(name.as_bytes()) {
                suffix = suffix.saturating_add(1);
                name = format!("{prefix}{number}_{suffix}");
            }
            taken_names.insert(name.clone().into_bytes());
            fonts.set(name.clone().into_bytes(), font);
            names.push(name);
        }
        Ok(ShapedFonts { index, names })
    }

    /// The font resource name and code that show `c`, if it was one of the clusters.
    pub fn code(&self, c: &ShapedCluster) -> Option<(&str, u8)> {
        let &k = self.index.get(&cluster_key(c))?;
        let name = self.names.get(k / GLYPHS_PER_FONT)?;
        // Codes start at 1; k % 240 + 1 is at most 240.
        Some((name.as_str(), u8::try_from(k % GLYPHS_PER_FONT + 1).ok()?))
    }

    /// Show `clusters` inside `BT … ET` from `pen` on baseline `y` at `size`, each placed with its
    /// own `Tm`, `spacing` after each and `word_spacing` more after a space. `current` is the
    /// shaped font the text state has selected (`None` after another font); returns the pen
    /// after the clusters.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &self,
        out: &mut Vec<u8>,
        clusters: &[ShapedCluster],
        mut pen: f64,
        y: f64,
        size: f64,
        spacing: f64,
        word_spacing: f64,
        current: &mut Option<usize>,
    ) -> f64 {
        for c in clusters {
            let Some(&k) = self.index.get(&cluster_key(c)) else { continue };
            let Some(name) = self.names.get(k / GLYPHS_PER_FONT) else { continue };
            let code = u8::try_from(k % GLYPHS_PER_FONT + 1).unwrap_or(1);
            if *current != Some(k / GLYPHS_PER_FONT) {
                out.extend(format!("/{name} {} Tf ", n(size)).bytes());
                *current = Some(k / GLYPHS_PER_FONT);
            }
            out.extend(format!("1 0 0 1 {} {} Tm ", n(pen), n(y)).bytes());
            out.extend(literal(&[code]));
            out.extend_from_slice(b" Tj\n");
            pen += c.advance * size + spacing + if c.text == " " { word_spacing } else { 0.0 };
        }
        pen
    }

    /// Show a line's `pieces` inside `BT … ET` from `x` on baseline `y` at `size`: shaped clusters
    /// in their Type 3 fonts, the rest in the font `latin` (which the text state has selected when
    /// `current` is `None`), written by `encode` and `measure`d. `current` is as for [`Self::show`].
    #[allow(clippy::too_many_arguments)]
    pub fn show_line(
        &self,
        out: &mut Vec<u8>,
        pieces: &[Piece],
        x: f64,
        y: f64,
        size: f64,
        latin: &str,
        encode: impl Fn(&str) -> Vec<u8>,
        measure: impl Fn(&str) -> f64,
        current: &mut Option<usize>,
    ) {
        let mut pen = x;
        for p in pieces {
            match p {
                Piece::Latin(s) => {
                    if current.take().is_some() {
                        out.extend(format!("/{latin} {} Tf ", n(size)).bytes());
                    }
                    out.extend(format!("1 0 0 1 {} {} Tm ", n(pen), n(y)).bytes());
                    out.extend(literal(&encode(s)));
                    out.extend_from_slice(b" Tj\n");
                    pen += measure(s);
                }
                Piece::Shaped(clusters) => pen = self.show(out, clusters, pen, y, size, 0.0, 0.0, current),
            }
        }
    }
}

/// `lines` as a generated appearance not yet in a document draws them: each line's pieces, its
/// Thai shaped, and the Type 3 fonts for that Thai built into `fonts` (the form's font resources)
/// as direct objects, whose streams [`store_streams`] moves into the document with the
/// appearance. Thai the face can't draw, and every line when a glyph can't be read, is left to the
/// standard font, whose encoder writes `?` for it; [`first_undrawable`] reports that.
pub fn appearance_lines(lines: &[String], fonts: &mut Dict) -> (Vec<Vec<Piece>>, ShapedFonts) {
    fallback_appearance_lines(lines, true, false, fonts)
}

/// [`appearance_lines`] with the pieces [`fallback_pieces`] splits each line into: its Thai with
/// `thai`, and with `latin` the Latin, Greek and Cyrillic characters the standard fonts lack, drawn
/// from the bundled Inter face. Their Type 3 fonts are named `PCTh…` when Thai is drawn, else
/// `PCLa…`. A line whose runs can't be shaped, and every line when a glyph can't be read, is left
/// to the standard font, which writes `?` for what it lacks; the caller reports that.
pub fn fallback_appearance_lines(lines: &[String], thai: bool, latin: bool, fonts: &mut Dict) -> (Vec<Vec<Piece>>, ShapedFonts) {
    let plain = |l: &String| vec![Piece::Latin(l.clone())];
    let needs = |c: char| (thai && is_thai(c)) || (latin && latin_fallback(c));
    let pieces: Vec<Vec<Piece>> =
        lines.iter().map(|l| if l.chars().any(needs) { fallback_pieces(l, thai, latin).unwrap_or_else(|_| plain(l)) } else { plain(l) }).collect();
    let prefix = if thai { "PCTh" } else { "PCLa" };
    match ShapedFonts::new(clusters(pieces.iter().map(Vec::as_slice)), shaped_glyph, &Dict::new(), fonts, prefix, &mut |o| o).map_err(|(e, _)| e) {
        Ok(shaped) => (pieces, shaped),
        Err(_) => (lines.iter().map(plain).collect(), ShapedFonts::default()),
    }
}

/// `lines` as a generated appearance not yet in a document draws them with [`line_pieces`]: as
/// [`wrap`] gives them, each with its paragraph's direction, their Arabic, Hebrew and Thai shaped
/// and in drawing order ([`appearance_pieces`], which says what becomes of a line a face can't
/// shape). The Type 3 fonts for the shaped text are built into `fonts` (the form's font resources)
/// as direct objects, whose streams [`store_streams`] moves into the document with the appearance.
/// Every line is left to the standard font, whose encoder writes `?` for what it can't draw, when
/// a glyph can't be read.
pub fn shaped_appearance_lines(lines: &[Line], fonts: &mut Dict) -> (Vec<Vec<Piece>>, ShapedFonts) {
    fallback_shaped_lines(lines, true, false, fonts)
}

/// [`shaped_appearance_lines`] with each face asked for: with `shaped`, the Arabic, Hebrew and
/// Thai from the script faces ([`appearance_pieces`]), else each line whole; and with `latin`,
/// the Latin, Greek and Cyrillic characters the standard fonts lack from the bundled Inter face
/// ([`fallback_pieces`]), what is left to the standard font split again.
pub fn fallback_shaped_lines(lines: &[Line], shaped: bool, latin: bool, fonts: &mut Dict) -> (Vec<Vec<Piece>>, ShapedFonts) {
    let pieces = lines.iter().map(|l| with_latin_fallback(if shaped { l.appearance_pieces() } else { plain(&l.text) }, latin)).collect();
    appearance_fonts(pieces, lines.iter().map(|l| l.text.as_str()), fonts)
}

/// `pieces` with, when `latin`, each piece left to the standard font split by [`fallback_pieces`]
/// (a piece the bundled face can't shape stays whole, and is reported as before).
fn with_latin_fallback(pieces: Vec<Piece>, latin: bool) -> Vec<Piece> {
    if !latin {
        return pieces;
    }
    pieces
        .into_iter()
        .flat_map(|p| match p {
            Piece::Latin(s) if s.chars().any(latin_fallback) => fallback_pieces(&s, false, true).unwrap_or_else(|_| vec![Piece::Latin(s)]),
            other => vec![other],
        })
        .collect()
}

/// Whether `pieces` hold right-to-left text shaped in its face, so that they are painted in an
/// order other than the one typed and the line needs its typing order beside it
/// ([`actual_text_span`]). A line left to the standard font, or with only Thai or the bundled
/// face's letters, does not.
pub fn reordered(pieces: &[Piece]) -> bool {
    clusters([pieces]).any(|c| matches!(c.script, ShapedScript::Arabic | ShapedScript::Hebrew))
}

/// The resource-name prefix of the Type 3 fonts that draw `clusters`: `PCAr` with Arabic or
/// Hebrew, as Add text names them; else `PCTh` with Thai, as those fonts were named before other
/// scripts were drawn; else `PCLa`, for the bundled face's letters alone.
pub fn font_prefix<'a>(clusters: impl IntoIterator<Item = &'a ShapedCluster>) -> &'static str {
    let (mut thai, mut rtl) = (false, false);
    for c in clusters {
        match c.script {
            ShapedScript::Arabic | ShapedScript::Hebrew => rtl = true,
            ShapedScript::Thai => thai = true,
            ShapedScript::Latin => {}
        }
    }
    if rtl {
        "PCAr"
    } else if thai {
        "PCTh"
    } else {
        "PCLa"
    }
}

/// The Type 3 fonts for `pieces`' shaped clusters, built into `fonts` as direct objects (named by
/// [`font_prefix`]); when a glyph can't be read, `lines` are all left to the standard font instead.
fn appearance_fonts<'a>(pieces: Vec<Vec<Piece>>, lines: impl Iterator<Item = &'a str>, fonts: &mut Dict) -> (Vec<Vec<Piece>>, ShapedFonts) {
    let prefix = font_prefix(clusters(pieces.iter().map(Vec::as_slice)));
    match ShapedFonts::new(clusters(pieces.iter().map(Vec::as_slice)), shaped_glyph, &Dict::new(), fonts, prefix, &mut |o| o).map_err(|(e, _)| e) {
        Ok(shaped) => (pieces, shaped),
        Err(_) => (lines.map(plain).collect(), ShapedFonts::default()),
    }
}

/// How far the Arabic, Hebrew and Thai glyphs of `line`, drawn with [`line_pieces`], reach below
/// and above its baseline, in em units; `None` without any, or when no face draws them.
pub fn line_reach(line: &str) -> Option<[f64; 2]> {
    if !has_shaped_script(line) {
        return None;
    }
    pieces_reach(&appearance_pieces(line, rtl_base(line)))
}

/// How far the shaped glyphs of `pieces` reach below and above the baseline, in em units.
fn pieces_reach(pieces: &[Piece]) -> Option<[f64; 2]> {
    let mut reach: Option<[f64; 2]> = None;
    for c in clusters([pieces]) {
        for (id, [_, dy]) in &c.glyphs {
            let g = shaped_glyph(c.script, *id).ok()?;
            if !g.contours.is_empty() {
                let b = [g.bbox[1] + dy, g.bbox[3] + dy];
                reach = Some(reach.map_or(b, |r| [r[0].min(b[0]), r[1].max(b[1])]));
            }
        }
    }
    reach
}

/// The first character of `line` left as it is to the standard font that would come out as `?`:
/// one outside WinAnsiEncoding that isn't Thai, or Thai when the Thai face can't draw the line's
/// (this build has none, or it lacks a character). A line break in it counts.
fn first_undrawable_in_line(line: &str) -> Option<char> {
    let thai_fails = line.chars().any(is_thai) && thai_pieces(line).is_err();
    line.chars().find(|c| if is_thai(*c) { thai_fails } else { !crate::win_ansi_encodable(*c) })
}

/// A character of `pieces` the standard font is left to draw and can't, which it writes as `?`;
/// the first as drawn. `None` when every piece can be drawn.
pub fn pieces_undrawable(pieces: &[Piece]) -> Option<char> {
    pieces.iter().find_map(|p| match p {
        Piece::Latin(s) => crate::first_non_win_ansi(s),
        Piece::Shaped(_) => None,
    })
}

/// The first character of `s`, drawn with [`appearance_pieces`] ([`shaped_appearance_lines`]),
/// that would come out as `?`, from the first line that has one ([`line_undrawable`]). Line breaks
/// don't count.
pub fn first_undrawable_shaped(s: &str) -> Option<char> {
    s.split(['\n', '\r']).find_map(line_undrawable)
}

/// [`first_undrawable_shaped`] for one line drawn as it is, so a line break in it counts. On a
/// line the faces shape, that is a character left to the standard font that it can't draw; on a
/// line a face can't shape, every character outside WinAnsiEncoding but the Thai its face still
/// draws. The direction marks of a shaped line don't count: they are never drawn.
pub fn line_undrawable(line: &str) -> Option<char> {
    if !has_shaped_script(line) {
        return crate::first_non_win_ansi(&standard_text(line));
    }
    // Shaped, the pieces are in drawing order; left to the standard font, in typing order.
    match line_pieces(line, rtl_base(line)) {
        // A break is drawn by nothing, whichever font its run would give it to.
        Ok(pieces) => pieces_undrawable(&pieces).or_else(|| line.chars().find(|c| matches!(c, '\n' | '\r'))),
        Err(_) => first_undrawable_in_line(line),
    }
}

/// Why the Thai in `text` can't be drawn ([`thai_message`]): this build has no Thai face, or the
/// face lacks a character. For a text box, which refuses such Thai rather than write `?` (its
/// Arabic and Hebrew it draws as `?` and reports); `None` when all of its Thai draws, or it has
/// none.
pub fn thai_refusal(text: &str) -> Option<String> {
    text.split(['\n', '\r']).filter(|l| l.chars().any(is_thai)).find_map(|l| thai_pieces(l).err().map(|e| thai_message(e, l)))
}

/// Why `text` can't be drawn with [`line_pieces`] ([`ShapeError::message`]): a line has Arabic,
/// Hebrew or Thai, and this build has no face for it or the face lacks a character. For what
/// refuses such text rather than write `?` (form fields, headers, footers, watermarks; Add text
/// refuses the same way); `None` when every line shapes, or none has such a script. Each line is
/// a paragraph of its own ([`rtl_base`]).
pub fn shape_refusal(text: &str) -> Option<String> {
    text.split(['\n', '\r']).filter(|l| has_shaped_script(l)).find_map(|l| line_pieces(l, rtl_base(l)).err().map(|e| e.message()))
}

/// [`type3_font`] written into `doc`, with its glyphs, descriptor and ToUnicode as objects of their
/// own: for content that draws `clusters` (at most [`GLYPHS_PER_FONT`]) as codes 1… directly.
pub fn add_type3_font(
    doc: &mut Document,
    clusters: &[&ShapedCluster],
    outlines: &HashMap<(ShapedScript, u32), GlyphOutline>,
) -> pdfcraft_cos::ObjRef {
    let font = type3_font(clusters, outlines, &mut |o| Object::Ref(doc.add(o)));
    doc.add(Object::Dict(font))
}

/// A Type 3 font with `clusters` as codes 1…, each drawn from its script face's glyph `outlines`.
fn type3_font(clusters: &[&ShapedCluster], outlines: &HashMap<(ShapedScript, u32), GlyphOutline>, store: &mut dyn FnMut(Object) -> Object) -> Dict {
    let mut charprocs = Dict::new();
    let mut differences = vec![Object::Int(1)];
    let mut widths = Vec::with_capacity(clusters.len());
    let mut bbox = [0.0f64; 4];
    let mut to_unicode = Vec::new();
    for (i, c) in clusters.iter().enumerate() {
        let code = i + 1;
        // Glyph space is 1000 units per em.
        let mut paths = Vec::new();
        let mut ink: Option<[f64; 4]> = None;
        for (id, [dx, dy]) in &c.glyphs {
            let Some(g) = outlines.get(&(c.script, *id)) else { continue };
            for contour in &g.contours {
                for (j, p) in contour.iter().enumerate() {
                    paths.extend(format!("{} {} {}\n", n((p[0] + dx) * 1000.0), n((p[1] + dy) * 1000.0), if j == 0 { "m" } else { "l" }).bytes());
                }
                paths.extend_from_slice(b"h\n");
                let b = [g.bbox[0] + dx, g.bbox[1] + dy, g.bbox[2] + dx, g.bbox[3] + dy];
                ink = Some(ink.map_or(b, |a| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]));
            }
        }
        // Rounded outwards so no ink is clipped.
        let [x0, y0, x1, y1] =
            ink.map_or([0.0; 4], |b| [(b[0] * 1000.0).floor(), (b[1] * 1000.0).floor(), (b[2] * 1000.0).ceil(), (b[3] * 1000.0).ceil()]);
        bbox = [bbox[0].min(x0), bbox[1].min(y0), bbox[2].max(x1), bbox[3].max(y1)];
        let mut proc = format!("{} 0 {} {} {} {} d1\n", n(c.advance * 1000.0), n(x0), n(y0), n(x1), n(y1)).into_bytes();
        if !paths.is_empty() {
            proc.extend(paths);
            proc.extend_from_slice(b"f\n");
        }
        let glyph = format!("g{code:02X}");
        charprocs.set(glyph.clone().into_bytes(), store(Object::Stream(Stream::flate(Dict::new(), &proc))));
        differences.push(Object::name(&glyph));
        widths.push(Object::Real((c.advance * 1000.0).round()));
        if !c.text.is_empty() {
            let hex: String = c.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
            to_unicode.push(format!("<{code:02X}> <{hex}>\n"));
        }
    }
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    // A CMap block holds at most 100 entries.
    for block in to_unicode.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n{}endbfchar\n", block.len(), block.concat()));
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let cmap = store(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
    // The face's family when every glyph comes from one script; without the face, the script's
    // name (as Arabic items were always named).
    let family = match clusters.first().map(|c| c.script).filter(|s| clusters.iter().all(|c| c.script == *s)) {
        Some(script) => script.font().map_or(
            match script {
                ShapedScript::Arabic => "Arabic",
                ShapedScript::Hebrew => "Hebrew",
                ShapedScript::Thai => "Thai",
                ShapedScript::Latin => "Inter",
            },
            |f| f.family,
        ),
        None => "PdfCraft Shaped Text",
    };
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(&family.replace(' ', "")));
    descriptor.set(b"FontFamily".to_vec(), Object::String(PdfString::literal(family.as_bytes().to_vec())));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    font.set(b"FontDescriptor".to_vec(), store(Object::Dict(descriptor)));
    font.set(b"FontBBox".to_vec(), Object::Array(bbox.iter().map(|v| Object::Real(*v)).collect()));
    font.set(
        b"FontMatrix".to_vec(),
        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
    );
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(clusters.len() as i64));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"ToUnicode".to_vec(), cmap);
    font
}

/// Most levels of dictionaries and arrays [`store_streams`] looks into: a Type 3 font in a form's
/// resources is four deep (`/Font` → font → `/CharProcs` → glyph).
const STREAM_DEPTH: usize = 8;

/// Move the streams written into `d` (the fonts [`ShapedFonts::new`] built for an appearance not
/// yet in a document) into `doc` as objects of their own, leaving references: a stream is never a
/// direct object (ISO 32000-2 §7.3.8).
pub fn store_streams(doc: &mut Document, d: &mut Dict) {
    fn walk(doc: &mut Document, o: &mut Object, depth: usize) {
        if depth > STREAM_DEPTH {
            return;
        }
        match o {
            Object::Stream(_) => {
                let Object::Stream(mut s) = std::mem::replace(o, Object::Null) else { return };
                walk_dict(doc, &mut s.dict, depth + 1);
                *o = Object::Ref(doc.add(Object::Stream(s)));
            }
            Object::Dict(d) => walk_dict(doc, d, depth + 1),
            Object::Array(a) => a.iter_mut().for_each(|o| walk(doc, o, depth + 1)),
            _ => {}
        }
    }
    fn walk_dict(doc: &mut Document, d: &mut Dict, depth: usize) {
        let keys: Vec<Vec<u8>> =
            d.iter().filter(|(_, v)| matches!(v, Object::Stream(_) | Object::Dict(_) | Object::Array(_))).map(|(k, _)| k.clone()).collect();
        for k in keys {
            if let Some(v) = d.get_mut(&k) {
                walk(doc, v, depth);
            }
        }
    }
    walk_dict(doc, d, 0);
}

/// Format a number for content streams (up to three decimals).
fn n(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(width: f64) -> GlyphOutline {
        GlyphOutline { contours: vec![vec![[0.0, -0.4], [width, -0.4], [width, 1.1], [0.0, 1.1]]], width, bbox: [0.0, -0.4, width, 1.1] }
    }

    fn cluster(text: &str, id: u32) -> ShapedCluster {
        ShapedCluster { script: ShapedScript::Thai, glyphs: vec![(id, [0.0, 0.0])], advance: 0.5, text: text.into() }
    }

    #[test]
    fn appearance_fonts_keep_their_streams_until_stored_then_refer_to_them() {
        let clusters = [cluster("น้ำ", 1), cluster("ก", 2), cluster("น้ำ", 1)];
        let mut fonts = Dict::new();
        let made = ShapedFonts::new(&clusters, |_, _| Ok(boxed(0.5)), &Dict::new(), &mut fonts, "PCTh", &mut |o| o).unwrap();
        assert_eq!(made.code(&clusters[0]), Some(("PCTh1", 1)));
        assert_eq!(made.code(&clusters[1]), Some(("PCTh1", 2)), "a repeated cluster is one glyph");
        let mut line = Vec::new();
        let pieces = [Piece::Latin("a".into()), Piece::Shaped(clusters[..1].to_vec()), Piece::Latin("b".into())];
        made.show_line(&mut line, &pieces, 0.0, 0.0, 10.0, "Helv", |s| s.as_bytes().to_vec(), |_| 1.0, &mut None);
        let shown = String::from_utf8_lossy(&line).into_owned();
        assert_eq!(shown, "1 0 0 1 0 0 Tm (a) Tj\n/PCTh1 10 Tf 1 0 0 1 1 0 Tm (\u{1}) Tj\n/Helv 10 Tf 1 0 0 1 6 0 Tm (b) Tj\n");
        let mut out = Vec::new();
        let mut current = None;
        let pen = made.show(&mut out, &clusters, 10.0, 20.0, 10.0, 0.0, 0.0, &mut current);
        assert_eq!(pen, 25.0);
        assert_eq!(String::from_utf8_lossy(&out).matches(" Tf").count(), 1, "one font selection for one font");
        let mut doc = Document::new_empty();
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        store_streams(&mut doc, &mut res);
        let font = res.get(b"Font").and_then(Object::as_dict).and_then(|f| f.get(b"PCTh1")).and_then(Object::as_dict).unwrap();
        assert!(matches!(font.get(b"ToUnicode"), Some(Object::Ref(_))));
        let procs = font.get(b"CharProcs").and_then(Object::as_dict).unwrap();
        assert!(procs.iter().count() == 2 && procs.iter().all(|(_, v)| matches!(v, Object::Ref(_))));
    }

    #[test]
    fn a_missing_glyph_names_its_cluster_and_writes_nothing() {
        let clusters = [cluster("ก", 2)];
        let mut fonts = Dict::new();
        let mut stored = 0;
        let made = ShapedFonts::new(&clusters, |_, _| Err(GlyphError::Missing), &Dict::new(), &mut fonts, "PCTh", &mut |o| {
            stored += 1;
            o
        });
        assert!(matches!(made, Err((GlyphError::Missing, c)) if c.text == "ก"));
        assert_eq!((stored, fonts.iter().count()), (0, 0));
    }

    /// #6822: Greek, and the Latin and Cyrillic letters WinAnsiEncoding lacks, are shaped with the
    /// bundled Inter face, which every build has; what WinAnsi holds stays the standard font's.
    #[test]
    fn letters_the_standard_fonts_lack_are_shaped_with_the_bundled_face() {
        assert!(latin_fallback('Γ') && latin_fallback('ř') && latin_fallback('ж'));
        assert!(!latin_fallback('é') && !latin_fallback('a') && !latin_fallback('\n') && !latin_fallback('北') && !latin_fallback('ก'));
        // Text WinAnsi holds is one piece for the standard font, measured by it.
        assert_eq!(fallback_pieces("Café (±)", false, true).unwrap(), [Piece::Latin("Café (±)".into())]);
        assert_eq!(fallback_width("Café", 10.0, false, true, |s, size| s.chars().count() as f64 * size), 40.0);
        // Greek words, with the standard font's space between them.
        let pieces = fallback_pieces("Γειά σου", false, true).unwrap();
        assert!(
            matches!(&pieces[..], [Piece::Shaped(a), Piece::Latin(sp), Piece::Shaped(b)] if sp == " " && a.len() == 4 && b.len() == 3),
            "{pieces:?}"
        );
        let text: String = clusters([pieces.as_slice()]).map(|c| c.text.as_str()).collect();
        assert_eq!(text, "Γειάσου", "each character copies once");
        assert!(clusters([pieces.as_slice()]).all(|c| c.script == ShapedScript::Latin && c.advance > 0.0));
        assert!(fallback_width("Γειά", 10.0, false, true, |_, _| 0.0) > 0.0);
        // Mixed: the Latin stays the standard font's.
        let mixed = fallback_pieces("Name: Γειά ok", false, true).unwrap();
        assert!(matches!(&mixed[..], [Piece::Latin(a), Piece::Shaped(_), Piece::Latin(b)] if a == "Name: " && b == " ok"), "{mixed:?}");
        // A combining accent the standard font lacks takes its letter with it.
        let accent = fallback_pieces("Cafe\u{301}!", false, true).unwrap();
        assert!(
            matches!(&accent[..], [Piece::Latin(a), Piece::Shaped(c), Piece::Latin(b)] if a == "Caf" && b == "!" && c.iter().map(|c| c.text.as_str()).collect::<String>() == "e\u{301}"),
            "{accent:?}"
        );
        // Without `latin`, nothing changes.
        assert_eq!(fallback_pieces("Γειά", false, false).unwrap(), [Piece::Latin("Γειά".into())]);
        // The fonts are built under PCLa names, and a character no face here has stays `?`.
        let mut fonts = Dict::new();
        let (lines, shaped) = fallback_appearance_lines(&["Γειά 北".to_string()], false, true, &mut fonts);
        assert!(fonts.iter().any(|(k, _)| k.starts_with(b"PCLa")));
        assert!(matches!(&lines[0][..], [Piece::Shaped(c), Piece::Latin(rest)] if shaped.code(&c[0]).is_some() && rest == " 北"), "{lines:?}");
        assert_eq!(first_undrawable_in_line(" 北"), Some('北'));
    }

    #[test]
    fn thai_runs_are_shaped_and_the_rest_left_to_the_standard_font() {
        assert_eq!(thai_pieces("Hello").unwrap(), [Piece::Latin("Hello".into())]);
        assert!(joins_previous('\u{0E49}') && joins_previous('\u{0E33}') && !joins_previous('ก') && !joins_previous('a'));
        assert_eq!((first_undrawable_shaped("Café\nпривет"), thai_refusal("Café"), shape_refusal("Café")), (Some('п'), None, None));
        if crate::document_thai_font().is_none() {
            assert_eq!(thai_pieces("น้ำ 1"), Err(GlyphError::NoFont));
            assert!(thai_message(GlyphError::NoFont, "").contains("this build has none"));
            assert_eq!(thai_width("น้ำ", 10.0, |s, size| s.chars().count() as f64 * size), 30.0, "measured by the standard font");
            assert_eq!(first_undrawable_shaped("ok\nชื่อ น้ำ"), Some('ช'));
            assert!(thai_refusal("ok\nน้ำ").is_some_and(|m| m.contains("CRAFT_FONTS_DIR")));
            assert_eq!(shape_refusal("ok\nน้ำ"), thai_refusal("ok\nน้ำ"), "one message for Thai, whoever refuses it");
            return;
        }
        assert_eq!((first_undrawable_shaped("ชื่อ: น้ำ\nOK"), thai_refusal("ชื่อ: น้ำ"), shape_refusal("ชื่อ: น้ำ\nOK")), (None, None, None));
        assert_eq!(first_undrawable_shaped("น้ำ ☃"), Some('☃'));
        // Stacked marks reach above the em box's 0.9, a descender below its 0.3.
        let [_, top] = line_reach("น้ำ").unwrap();
        let [bottom, _] = line_reach("ฎู").unwrap();
        assert!(top > 0.95 && bottom < -0.35, "{top} {bottom}");
        let pieces = thai_pieces("ชื่อ: บุญฤทธิ์ 12").unwrap();
        assert!(matches!(&pieces[..], [Piece::Shaped(_), Piece::Latin(a), Piece::Shaped(_), Piece::Latin(b)] if a == ": " && b == " 12"));
        let text: String = clusters([pieces.as_slice()]).map(|c| c.text.as_str()).collect();
        assert_eq!(text, "ชื่อบุญฤทธิ์");
        assert!(thai_width("น้ำ", 10.0, |_, _| 0.0) > 0.0);
    }

    /// The text of every shaped cluster of `pieces`, in drawing order.
    fn shaped_texts(pieces: &[Piece]) -> Vec<String> {
        clusters([pieces]).map(|c| c.text.clone()).collect()
    }

    #[test]
    fn lines_without_a_shaped_script_are_left_to_the_standard_font() {
        assert_eq!(line_pieces("", false), Ok(Vec::new()));
        assert_eq!(line_pieces("", true), Ok(Vec::new()));
        assert_eq!(line_pieces("Hello (1)", false), Ok(vec![Piece::Latin("Hello (1)".into())]));
        // Direction marks steer the order and are never drawn.
        assert_eq!(line_pieces("a\u{200E}b", false), Ok(vec![Piece::Latin("ab".into())]));
        assert!(!has_shaped_script("Hello ☃") && has_shaped_script("a ب") && has_shaped_script("a ש") && has_shaped_script("a ก"));
        assert!(rtl_base("بت test") && rtl_base("שלום") && !rtl_base("test بت") && !rtl_base("test") && !rtl_base(""));
        assert_eq!(line_width("Hello", false, 10.0, |s, size| s.chars().count() as f64 * size), 50.0);
        assert_eq!(first_undrawable_shaped("Café\r\nок"), Some('о'));
        assert_eq!(first_undrawable_shaped("Café\n— 5€"), None);
        assert_eq!(line_reach("Hg"), None);
        assert_eq!(actual_text_span("aب"), "/Span <</ActualText <FEFF00610628>>> BDC\n");
        // A line never starts with a mark: Arabic fatha and shadda, Hebrew qamats, Thai mai tho.
        assert!(['\u{064E}', '\u{0651}', '\u{05B8}', '\u{0E49}'].into_iter().all(joins_previous));
        assert!(!['ب', 'ש', 'ก', 'a', ' '].into_iter().any(joins_previous));
    }

    #[test]
    fn wrapped_lines_take_their_paragraphs_direction() {
        let chars = |s: &str, size: f64| s.chars().count() as f64 * size;
        let laid = |text: &str, width: f64| -> Vec<(String, bool)> { wrap(text, 1.0, width, chars).into_iter().map(|l| (l.text, l.rtl)).collect() };
        assert_eq!(laid("one two\r\nthree", 5.0), [("one".to_string(), false), ("two".into(), false), ("three".into(), false)]);
        assert_eq!(laid("", 5.0), [(String::new(), false)]);
        // Each paragraph on its own: the Latin one after an Arabic one still reads left to right,
        // and a Latin word that wraps inside the Arabic paragraph keeps that paragraph's direction.
        assert_eq!(laid("سلام\nabc", 100.0).iter().map(|(_, rtl)| *rtl).collect::<Vec<_>>(), [true, false]);
        let laid = laid("سلام abcdefghij", 10.5);
        assert_eq!(laid.last(), Some(&("abcdefghij".to_string(), true)), "{laid:?}");
    }

    #[test]
    fn a_script_without_its_face_says_so_and_its_line_keeps_the_standard_font() {
        for (script, has_face, text) in [
            (ShapedScript::Arabic, crate::document_arabic_font().is_some(), "سلام"),
            (ShapedScript::Hebrew, crate::document_hebrew_font(false).is_some(), "שלום"),
        ] {
            let message = script_message(script, GlyphError::NoFont, text);
            assert!(
                message.contains("CRAFT_FONTS_DIR") && message.contains(if script == ShapedScript::Arabic { "an Arab face" } else { "a Hebr face" })
            );
            if has_face {
                continue;
            }
            let line = format!("ok {text}");
            let err = line_pieces(&line, false).unwrap_err();
            assert_eq!((err.script, err.error, err.message()), (script, GlyphError::NoFont, message));
            // Measured and drawn by the standard font, which writes `?`: reported, never silent.
            assert_eq!(line_width(&line, false, 10.0, |s, size| s.chars().count() as f64 * size), 70.0);
            assert_eq!(first_undrawable_shaped(&line), text.chars().next());
            // What refuses instead says which script and what to do, whichever line has it.
            assert_eq!(shape_refusal(&format!("fine\r\n{line}")), Some(script_message(script, GlyphError::NoFont, text)));
            let mut fonts = Dict::new();
            let (pieces, _) = shaped_appearance_lines(&[Line::new(line.clone(), false)], &mut fonts);
            assert_eq!((pieces, fonts.iter().count()), (vec![vec![Piece::Latin(line)]], 0));
        }
    }

    /// The layout Add text, text boxes and Fill & Sign text share: right-to-left runs come out in
    /// drawing order and shaped, Latin and digits keep their own order and the standard font.
    #[test]
    fn arabic_and_hebrew_lines_are_laid_out_in_drawing_order() {
        if crate::document_arabic_font().is_some() {
            // A right-to-left paragraph: the last letter typed is the first one drawn.
            let pieces = line_pieces("سلام", true).unwrap();
            assert!(matches!(&pieces[..], [Piece::Shaped(_)]), "{pieces:?}");
            let texts = shaped_texts(&pieces);
            assert_eq!((texts.first().map(String::as_str), texts.last().map(String::as_str)), (Some("م"), Some("س")), "{texts:?}");
            // #6879: Latin, then Arabic, in a paragraph that starts left to right. The Latin stays
            // first and in the standard font; the Arabic follows it, shaped, its words swapped.
            let pieces = line_pieces("test بت التناسب", false).unwrap();
            assert!(matches!(&pieces[..], [Piece::Latin(l), Piece::Shaped(_)] if l == "test "), "{pieces:?}");
            let texts = shaped_texts(&pieces);
            assert_eq!(texts.concat().chars().filter(|c| *c != ' ').count(), 9, "every letter is drawn once: {texts:?}");
            assert_eq!(texts.last().map(String::as_str), Some("ب"), "the first word typed is drawn last: {texts:?}");
            // Digits inside right-to-left text read left to right; brackets around them are mirrored.
            let pieces = line_pieces("سلام (12)", true).unwrap();
            let drawn: String = pieces
                .iter()
                .map(|p| match p {
                    Piece::Latin(s) => s.clone(),
                    Piece::Shaped(c) => c.iter().map(|c| c.text.as_str()).collect(),
                })
                .collect();
            assert!(drawn.contains("12") && !drawn.contains("21"), "{drawn}");
            assert_eq!(first_undrawable_shaped("test بت التناسب\nسلام (12)"), None);
            assert_eq!(shape_refusal("test بت التناسب\nسلام (12)"), None);
            // One line, drawn as it is: a break inside it is a character nothing draws.
            assert_eq!((line_undrawable("سلام (12)"), line_undrawable("سلام\rabc")), (None, Some('\r')));
            assert_eq!(first_undrawable_shaped("سلام ☃"), Some('☃'));
            // Shaped advances, not the standard font's guess at `?`.
            assert!(line_width("سلام", true, 10.0, |_, _| 0.0) > 0.0);
            let [bottom, top] = line_reach("سلام").unwrap();
            assert!(bottom < 0.0 && top > 0.0, "{bottom} {top}");
            let mut fonts = Dict::new();
            let (pieces, shaped) = shaped_appearance_lines(&[Line::new("test بت", false), Line::new("plain", false)], &mut fonts);
            assert!(matches!(&pieces[..], [first, second] if first.len() == 2 && second == &[Piece::Latin("plain".into())]), "{pieces:?}");
            let cluster = clusters([pieces[0].as_slice()]).next().unwrap();
            assert!(shaped.code(cluster).is_some_and(|(name, _)| name.starts_with("PCAr")) && fonts.iter().count() == 1);
        }
        if crate::document_hebrew_font(false).is_some() {
            let texts = shaped_texts(&line_pieces("שלום", true).unwrap());
            assert_eq!((texts.first().map(String::as_str), texts.last().map(String::as_str)), (Some("ם"), Some("ש")), "{texts:?}");
            assert_eq!(first_undrawable_shaped("שלום abc"), None);
        }
    }

    /// An appearance draws what it can: a line with a script that can't be shaped keeps its Thai
    /// from the Thai face, as before Arabic and Hebrew were drawn, and no typing order is attached
    /// to a line whose letters come out as `?`.
    #[test]
    fn a_line_with_a_script_that_cannot_be_shaped_keeps_its_thai() {
        assert_eq!(appearance_pieces("plain", false), [Piece::Latin("plain".into())]);
        assert!(!reordered(&appearance_pieces("plain", false)) && pieces_undrawable(&appearance_pieces("a☃", false)) == Some('☃'));
        if crate::document_thai_font().is_none() {
            return;
        }
        assert!(!reordered(&appearance_pieces("น้ำ", false)), "Thai is painted in typing order");
        // Arabic this build can't shape: it has no Arabic face, or the face lacks the character.
        let unshaped = if crate::document_arabic_font().is_none() {
            Some('ب')
        } else {
            (0x0870..=0x08FFu32).filter_map(char::from_u32).find(|c| shape_arabic(&c.to_string(), true).is_err())
        };
        let Some(c) = unshaped else { return };
        let line = format!("น้ำ {c}");
        assert!(line_pieces(&line, false).is_err());
        let pieces = appearance_pieces(&line, false);
        assert!(
            matches!(&pieces[..], [Piece::Shaped(thai), Piece::Latin(rest)] if thai.iter().all(|c| c.script == ShapedScript::Thai) && rest.ends_with(c)),
            "{pieces:?}"
        );
        assert!(!reordered(&pieces));
        assert_eq!(first_undrawable_shaped(&line), Some(c));
        assert!(line_width(&line, false, 10.0, |_, _| 0.0) > 0.0 && line_reach(&line).is_some(), "the Thai is still measured as drawn");
    }

    /// Levels are resolved for the whole paragraph before each wrapped line is reordered (UAX #9
    /// §3.4): a right-to-left override opened before a Hebrew word still reverses the Latin a
    /// paragraph wraps onto its next line, until the override closes, whichever way the paragraph
    /// reads. The line keeps what was typed for `/ActualText`, and is measured as it is drawn.
    /// The Latin needs no face, so this holds in every build.
    #[test]
    fn a_wrapped_line_keeps_its_paragraphs_levels() {
        let chars = |s: &str, size: f64| s.chars().count() as f64 * size;
        let letters = |pieces: &[Piece]| drawn_chars(pieces).filter(char::is_ascii_alphabetic).collect::<String>();
        for (para, first) in
            [("\u{202E}שלום עולם ABC DEF\u{202C}", "\u{202E}שלום עולם"), ("go \u{202E}שלום עולם ABC DEF\u{202C}", "go \u{202E}שלום עולם")]
        {
            let rtl = rtl_base(para);
            assert_eq!(rtl, !para.starts_with("go"), "{para:?}");
            let laid = Line::in_paragraph(para, rtl, vec![first.into(), "ABC DEF\u{202C}".into()]);
            let next = &laid[1];
            assert_eq!((next.text.as_str(), next.rtl), ("ABC DEF\u{202C}", rtl));
            // Drawn right to left, as the paragraph has it; alone, the line would read left to right.
            assert_eq!(letters(&next.pieces().unwrap()), "FEDCBA", "{para:?}");
            assert_eq!(letters(&next.appearance_pieces()), "FEDCBA", "{para:?}");
            assert_eq!(letters(&line_pieces(&next.text, rtl).unwrap()), "ABCDEF", "{para:?}");
            assert!(next.reordered(&next.appearance_pieces()) && !Line::new("ABC DEF", rtl).reordered(&appearance_pieces("ABC DEF", rtl)));
            // Measured as drawn: its space may come from a script face, never wider than wrapping counted.
            assert!(next.width(1.0, chars) <= 7.0 && next.width(1.0, chars) == pieces_width(&next.appearance_pieces(), 1.0, 0.0, chars));
            // The shared wrapper gives the same lines, in the narrowest box (in tenths) that breaks there.
            let breaks_there = |l: &Vec<Line>| l.iter().map(|l| l.text.as_str()).eq([first, "ABC DEF\u{202C}"]);
            let wrapped = (10..300).map(|w| wrap(para, 1.0, f64::from(w) / 10.0, chars)).find(breaks_there).expect(para);
            assert_eq!(letters(&wrapped[1].appearance_pieces()), "FEDCBA", "{para:?}");
        }
        // Where the override closes on the wrapped line, the paragraph's own order takes over: `AB`
        // still reversed, `CD` a left-to-right run of the right-to-left paragraph (alone, the line
        // would read `ABCD`). A line that isn't in the paragraph is resolved alone, and an empty
        // one draws nothing.
        let laid =
            Line::in_paragraph("\u{202E}שלום AB\u{202C} CD", true, vec!["\u{202E}שלום".into(), "AB\u{202C} CD".into(), "XY".into(), String::new()]);
        assert_eq!(letters(&laid[1].pieces().unwrap()), "CDBA");
        assert_eq!(letters(&laid[2].pieces().unwrap()), "XY");
        assert_eq!(laid[3].pieces().unwrap(), []);
        assert_eq!(Line::in_paragraph("", true, vec![String::new()])[0].pieces().unwrap(), []);
    }

    /// A direction control that leaves a left-to-right line in typing order is not drawn, and not
    /// measured or reported either, on its own or at the end of a paragraph wrapped beside Hebrew.
    /// One that would turn Latin text around is still left to the standard font, and reported.
    #[test]
    fn a_direction_control_in_a_left_to_right_line_is_not_drawn() {
        let count = |s: &str, _: f64| s.chars().count() as f64;
        let ab = [Piece::Latin("ab".into())];
        for line in ["a\u{200E}b", "a\u{2066}b\u{2069}", "ab\u{202C}"] {
            assert_eq!((appearance_pieces(line, false), Line::new(line, false).appearance_pieces()), (ab.to_vec(), ab.to_vec()), "{line:?}");
            assert_eq!((line_width(line, false, 1.0, count), Line::new(line, false).width(1.0, count)), (2.0, 2.0), "{line:?}");
            assert_eq!(line_undrawable(line), None, "{line:?}");
        }
        let laid = Line::in_paragraph("שלום \u{202A}cd ab\u{202C}", false, vec!["שלום \u{202A}cd".into(), "ab\u{202C}".into()]);
        assert_eq!(laid[1].appearance_pieces(), ab);
        assert_eq!(line_undrawable("\u{202E}ab"), Some('\u{202E}'));
        assert_eq!(line_undrawable("a\u{200E}☃"), Some('☃'));
    }

    /// Zero-width joiners choose the forms of the Arabic letters beside them, and one beside Latin
    /// text is not drawn: neither reaches the standard font, which would draw `?`.
    #[test]
    fn joiners_choose_arabic_forms_and_never_reach_the_standard_font() {
        if crate::document_arabic_font().is_none() {
            return;
        }
        let glyphs = |s: &str| -> Vec<u32> {
            clusters([line_pieces(s, true).unwrap().as_slice()])
                .filter(|c| !c.text.contains(is_joiner))
                .flat_map(|c| c.glyphs.iter().map(|g| g.0))
                .collect()
        };
        assert_eq!(glyphs("ب\u{200D}ب"), glyphs("بب"), "a joiner keeps them joined");
        assert_ne!(glyphs("ب\u{200C}ب"), glyphs("بب"), "a non-joiner parts them");
        for (line, rtl) in [("ب\u{200D}ب", true), ("می\u{200C}خواهم test", true), ("a\u{200D}ب", false), ("a ب\u{200C} b", false)] {
            let pieces = line_pieces(line, rtl).unwrap();
            assert!(
                pieces.iter().all(|p| !matches!(p, Piece::Latin(s) if s.contains(is_joiner))),
                "{line:?}: {:?}",
                drawn_chars(&pieces).collect::<String>()
            );
            assert_eq!(pieces_undrawable(&pieces), None, "{line:?}");
        }
    }

    /// A right-to-left line its glyphs alone read back from goes without `/ActualText`, which
    /// some readers turn around again: right-to-left words with their marks, punctuation and
    /// numbers. A line with a left-to-right letter, a direction control, or an override reaching
    /// it from an earlier line keeps it, as does one of a left-to-right paragraph.
    #[test]
    fn only_a_line_its_glyphs_cant_give_back_carries_actual_text() {
        for plain in ["مركز المدينة", "שלום עולם", "مَرْحَبًا بِكُم", "שָׁלוֹם", "نعم، لا", "عام 2026", "سنة ١٤٤٧", "(שלום)"]
        {
            let line = Line::new(plain, true);
            assert!(line.reads_back_from_drawing() && !line.reordered(&[]), "{plain:?}");
        }
        for (text, rtl) in
            [("مركز PdfCraft 2026", true), ("\u{202D}שלום\u{202C}", true), ("שלום\u{200F}", true), ("שלום עולם", false), ("2026", true), ("", true)]
        {
            assert!(!Line::new(text, rtl).reads_back_from_drawing(), "{text:?} rtl={rtl}");
        }
        // A left-to-right override opened on the line before draws `עולם` left to right.
        let laid = Line::in_paragraph("\u{202D}שלום עולם\u{202C}", true, vec!["\u{202D}שלום".into(), "עולם\u{202C}".into()]);
        assert!(!laid[1].reads_back_from_drawing(), "the override reaches the wrapped line");
        let laid = Line::in_paragraph("שלום עולם", true, vec!["שלום".into(), "עולם".into()]);
        assert!(laid.iter().all(Line::reads_back_from_drawing), "a plain paragraph's wrapped lines");
    }

    /// A space inside a right-to-left run is drawn from the script face, between Latin words from
    /// the standard font: wrapping counts the wider, so no line is drawn wider than it was measured.
    #[test]
    fn wrapped_mixed_lines_are_never_wider_than_the_box() {
        if crate::document_arabic_font().is_none() {
            return;
        }
        // A standard font with a very narrow space, as Times has beside the Arabic face.
        let latin = |s: &str, size: f64| s.chars().map(|c| if c == ' ' { 0.05 } else { 0.5 }).sum::<f64>() * size;
        let text = "test بت التناسب حل المعادلة test بت التناسب حل المعادلة test بت التناسب";
        for width in [60.0, 75.0, 90.0, 120.0] {
            let laid = wrap(text, 10.0, width, latin);
            assert!(laid.len() > 1, "{laid:?}");
            for line in &laid {
                let drawn = pieces_width(&line.appearance_pieces(), 10.0, 0.0, latin);
                assert!(drawn <= width + 1e-6, "{line:?} is {drawn} wide in {width}");
            }
        }
        assert!(reordered(&appearance_pieces("test بت", false)));
    }
}
