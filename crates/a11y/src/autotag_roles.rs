//! Heading roles for auto-tagged text objects, from the size and weight of the text each shows.
//!
//! The body style is the size (to the half point) carrying the most characters in the document,
//! and whether most of those characters are bold. A text object is a heading when it shows at most
//! three lines and 200 characters, including a letter, and is either 15% larger than the body or,
//! where the body is not bold, bold at no less than 90% of the body size on at most two lines.
//! Levels rank the heading styles (size, then bold) over the whole document, largest first, capped
//! at 6, and never skip a level after the heading before (the `heading-nesting` check).

use std::collections::HashMap;
use std::rc::Rc;

use pdfcraft_content::{Matrix, Op};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_fonts::pdf::Metrics;

use crate::autotag_lists::{self, Marker};

/// A line this much larger than body text reads as a heading.
const HEADING_SIZE_RATIO: f64 = 1.15;
/// Bold text this much of the body size or more can be a heading by weight.
const WEIGHT_SIZE_RATIO: f64 = 0.9;
const MAX_LINES: usize = 3;
const MAX_WEIGHT_LINES: usize = 2;
const MAX_CHARS: usize = 200;
/// Saved graphics states followed; deeper `q` nesting keeps the current state.
const MAX_SAVED: usize = 64;
/// Distinct fonts measured per document; text in any further font counts as unstyled.
const MAX_FONTS: usize = 256;
/// Characters of an object's text kept to find a list marker: enough for `(viii)` and the space
/// after it, and to tell a marker alone from one followed by text.
const MAX_PREFIX: usize = 16;
/// A `TJ` adjustment at least this far right, in thousandths of the text size, reads as a space
/// between words (`[(1.)-250(Apples)] TJ`).
const WORD_GAP: f64 = 150.0;

/// What one text object shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Evidence {
    /// The size, in user space, carrying the most of the object's characters.
    pub size: f64,
    /// Most of the object's characters are in a bold font.
    pub bold: bool,
    pub lines: usize,
    /// Characters shown, whitespace excluded.
    pub chars: usize,
    pub letters: bool,
    /// The list marker the text starts with ([`crate::autotag_lists`]).
    pub marker: Option<Marker>,
    /// The operator that shows only the object's list marker, when more text follows in later
    /// show operators of the same object: that part can be tagged apart as the item's label.
    pub label_op: Option<usize>,
    /// Where the first shown glyph starts, in default user space.
    pub x: f64,
    pub y: f64,
    /// Where the object's last line ends, and the furthest any of its lines ends, in default user
    /// space: whether a line is full, so the text after it is its wrapped continuation.
    pub end: f64,
    pub right: f64,
    /// How wide the first word of the object's text is, in default user space, or 0 when unknown:
    /// whether it would have fitted at the end of the line before, which then did not wrap.
    pub word: f64,
    /// The baseline of the object's last line, in default user space: the next line is measured
    /// from there.
    pub last_y: f64,
}

/// The structure type for each text object, in document order: `H1`–`H6` or `P`.
pub(crate) fn roles(objects: &[Evidence]) -> Vec<&'static str> {
    const HEADINGS: [&str; 6] = ["H1", "H2", "H3", "H4", "H5", "H6"];
    let Some((body, body_bold)) = body_style(objects) else { return vec!["P"; objects.len()] };
    let candidate = |e: &Evidence| {
        let fits = e.lines >= 1 && e.lines <= MAX_LINES && e.chars <= MAX_CHARS && e.letters;
        let by_size = e.size >= body * HEADING_SIZE_RATIO;
        let by_weight = e.bold && !body_bold && e.lines <= MAX_WEIGHT_LINES && e.size >= body * WEIGHT_SIZE_RATIO;
        fits && (by_size || by_weight)
    };
    let mut styles: Vec<(i64, bool)> = objects.iter().filter(|e| candidate(e)).map(|e| (half_points(e.size), e.bold)).collect();
    styles.sort_by(|a, b| b.cmp(a));
    styles.dedup();
    let ranks: HashMap<(i64, bool), usize> = styles.into_iter().enumerate().map(|(rank, style)| (style, rank)).collect();
    let mut previous = 0usize;
    objects
        .iter()
        .map(|e| {
            if !candidate(e) {
                return "P";
            }
            let rank = ranks.get(&(half_points(e.size), e.bold)).copied().unwrap_or(0);
            let level = rank.saturating_add(1).min(previous.saturating_add(1)).min(HEADINGS.len());
            previous = level;
            HEADINGS.get(level.saturating_sub(1)).copied().unwrap_or("P")
        })
        .collect()
}

fn half_points(size: f64) -> i64 {
    // Sizes are finite and bounded by `evidence`; `as` saturates in any case.
    (size * 2.0).round() as i64
}

/// The size carrying the most characters, and whether most of them are bold.
fn body_style(objects: &[Evidence]) -> Option<(f64, bool)> {
    let mut by_size: HashMap<i64, (usize, usize)> = HashMap::new();
    for e in objects.iter().filter(|e| e.chars > 0) {
        let entry = by_size.entry(half_points(e.size)).or_default();
        entry.0 = entry.0.saturating_add(e.chars);
        if e.bold {
            entry.1 = entry.1.saturating_add(e.chars);
        }
    }
    let (size, (chars, bold)) = by_size.into_iter().filter(|&(s, _)| s > 0).max_by_key(|&(s, (n, _))| (n, s))?;
    Some((size as f64 / 2.0, bold.saturating_mul(2) > chars))
}

#[derive(Clone)]
struct State {
    ctm: Matrix,
    tm: Matrix,
    tlm: Matrix,
    size: f64,
    leading: f64,
    char_spacing: f64,
    word_spacing: f64,
    /// Horizontal scaling, `Tz` / 100.
    h_scale: f64,
    /// Text render mode 3 or 7: shown text that paints nothing, such as an OCR layer or a
    /// clip-only object.
    invisible: bool,
    metrics: Option<Rc<Metrics>>,
}

/// Font metrics for a whole document, measured once per font object and capped at
/// [`MAX_FONTS`].
#[derive(Default)]
pub(crate) struct Fonts {
    by_ref: HashMap<ObjRef, Option<Rc<Metrics>>>,
    measured: usize,
}

impl Fonts {
    fn get(&mut self, doc: &Document, font: &Object) -> Option<Rc<Metrics>> {
        let key = match font {
            Object::Ref(r) => Some(*r),
            _ => None,
        };
        if let Some(found) = key.and_then(|r| self.by_ref.get(&r)) {
            return found.clone();
        }
        if self.measured >= MAX_FONTS {
            return None;
        }
        self.measured = self.measured.saturating_add(1);
        let metrics = doc.resolve(font).as_dict().map(|d| Rc::new(Metrics::from_dict(doc, d)));
        if let Some(r) = key {
            self.by_ref.insert(r, metrics.clone());
        }
        metrics
    }
}

#[derive(Default)]
struct Shown {
    by_size: HashMap<i64, (usize, usize)>,
    lines: usize,
    shown: bool,
    moved: bool,
    letters: bool,
    /// The start of the text, without leading white space, up to [`MAX_PREFIX`] characters.
    prefix: String,
    /// Text past [`MAX_PREFIX`] was left out of `prefix`, so a marker there is not alone.
    prefix_cut: bool,
    /// The start of the current line after the first, kept the same way.
    /// and where that line starts.
    line: Option<(String, f64)>,
    /// A line after the first starts at the same indent with the marker that would follow the
    /// first line's: the object holds a list of its own.
    later_marker: bool,
    /// The next text shown is a separate word: since the last text, the text was moved, a space
    /// was shown, or `TJ` adjustments added up to [`WORD_GAP`]. Shows that follow each other
    /// directly join (`(1) Tj (.) Tj` is `1.`).
    gap: bool,
    start: Option<(f64, f64)>,
    /// Where the current line's text ends so far, and the furthest any line ended.
    end: f64,
    right: f64,
    /// The first operator that shows something, the start of what it shows, and whether text past
    /// that start was left out.
    first_show: Option<(usize, String, bool)>,
    /// A later operator shows something too.
    shown_after: bool,
    /// Where the first word of the first line ends, once it has started, and whether it has ended.
    word_end: Option<f64>,
    word_done: bool,
    /// The baseline of the last line shown so far.
    last_y: f64,
}

impl Shown {
    /// `bytes` shown by operator `at`. How far the text position moves, in text space.
    fn show(&mut self, state: &State, at: usize, bytes: &[u8]) -> f64 {
        let Some(metrics) = state.metrics.as_ref() else { return 0.0 };
        let advance = advance(state, metrics, bytes);
        if state.invisible {
            return advance;
        }
        let text = metrics.decode(bytes);
        let chars = text.chars().filter(|c| !c.is_whitespace()).count();
        if chars == 0 {
            // A shown space still separates words (`[(1.)( )(Apples)] TJ`).
            self.gap |= !text.is_empty();
            return advance;
        }
        self.letters |= text.chars().any(char::is_alphabetic);
        // Where the first glyph other than a space is drawn.
        let lead: f64 = metrics
            .codes(bytes)
            .into_iter()
            .take_while(|&(code, len)| blank(metrics, code, len))
            .map(|(code, len)| glyph_advance(state, metrics, code, len))
            .sum();
        let [.., x, y] = Matrix::translate(lead, 0.0).then(&state.tm).then(&state.ctm).0;
        if self.start.is_none() {
            self.start = Some((x, y));
            (self.end, self.right) = (x, x);
        }
        self.last_y = y;
        if self.shown && self.moved {
            self.close_line();
            self.line = Some((String::new(), x));
            self.end = x;
        }
        // A line move sets `gap` too.
        self.word_done |= self.word_end.is_some() && self.gap;
        if !self.word_done {
            self.first_word(state, metrics, bytes);
        }
        let ends = Matrix::translate(advance, 0.0).then(&state.tm).then(&state.ctm).0[4];
        if ends.is_finite() {
            self.end = self.end.max(ends);
            self.right = self.right.max(self.end);
        }
        let gap = std::mem::take(&mut self.gap);
        self.prefix_cut |= keep_start(&mut self.prefix, &text, gap);
        if let Some((line, _)) = self.line.as_mut() {
            keep_start(line, &text, gap);
        }
        match &mut self.first_show {
            None => {
                let mut start = String::new();
                let cut = keep_start(&mut start, &text, false);
                self.first_show = Some((at, start, cut));
            }
            Some((first, start, cut)) if *first == at => *cut |= keep_start(start, &text, false),
            Some(_) => self.shown_after = true,
        }
        if !self.shown {
            (self.shown, self.lines) = (true, 1);
        } else if self.moved {
            self.lines = self.lines.saturating_add(1);
        }
        self.moved = false;
        let [_, _, c, d, _, _] = state.tm.then(&state.ctm).0;
        let size = (state.size * c.hypot(d)).abs();
        if !size.is_finite() || size <= 0.0 || size > 1e6 {
            return advance;
        }
        let entry = self.by_size.entry(half_points(size)).or_default();
        entry.0 = entry.0.saturating_add(chars);
        if metrics.bold {
            entry.1 = entry.1.saturating_add(chars);
        }
        advance
    }

    /// Follow the first word of the first line through `bytes`, shown at the text position.
    fn first_word(&mut self, state: &State, metrics: &Metrics, bytes: &[u8]) {
        let mut at = 0.0;
        for (code, len) in metrics.codes(bytes) {
            if blank(metrics, code, len) {
                self.word_done |= self.word_end.is_some();
            } else if !self.word_done {
                let ends = Matrix::translate(at + glyph_advance(state, metrics, code, len), 0.0).then(&state.tm).then(&state.ctm).0[4];
                self.word_end = Some(ends).filter(|e| e.is_finite()).or(self.word_end);
            }
            if self.word_done {
                return;
            }
            at += glyph_advance(state, metrics, code, len);
        }
    }

    /// A line move counts once text is shown after it, so blank lines and trailing moves do not.
    fn new_line(&mut self) {
        self.moved = self.shown;
        self.gap = true;
    }

    /// A wrapped line that only looks like a marker (`A. Smith` after `1. Written by`, or text at
    /// a hanging indent) does not count.
    fn close_line(&mut self) {
        let Some((line, x)) = self.line.take() else { return };
        let (Some(first), Some(next), Some((start, _))) = (autotag_lists::marker(&self.prefix), autotag_lists::marker(&line), self.start) else {
            return;
        };
        if first.followed_by(&next) && (x - start).abs() <= autotag_lists::INDENT_TOLERANCE {
            self.later_marker = true;
        }
    }

    fn finish(mut self) -> Evidence {
        self.close_line();
        let chars = self.by_size.values().fold(0usize, |n, v| n.saturating_add(v.0));
        let Some((size, (count, bold))) = self.by_size.into_iter().max_by_key(|&(s, (n, _))| (n, s)) else {
            return Evidence::default();
        };
        let (x, y) = self.start.unwrap_or_default();
        let (end, right) = (self.end, self.right.max(self.end));
        Evidence {
            size: size as f64 / 2.0,
            bold: bold.saturating_mul(2) > count,
            lines: self.lines,
            chars,
            letters: self.letters,
            // An object of several marked lines is a list drawn as one object, which stays a paragraph.
            marker: autotag_lists::marker(&self.prefix).filter(|_| !self.later_marker).map(|m| Marker { alone: m.alone && !self.prefix_cut, ..m }),
            label_op: self
                .first_show
                .filter(|(_, text, cut)| self.shown_after && !cut && autotag_lists::marker(text).is_some_and(|m| m.alone))
                .map(|(at, ..)| at),
            x,
            y,
            end,
            right,
            word: self.word_end.map_or(0.0, |w| (w - x).max(0.0)),
            last_y: self.last_y,
        }
    }
}

/// How far showing `bytes` moves the text position, in text space (§9.4.4): each glyph's width at
/// the font size, plus character spacing and word spacing for a single-byte space, scaled
/// horizontally.
fn advance(state: &State, metrics: &Metrics, bytes: &[u8]) -> f64 {
    let total: f64 = metrics.codes(bytes).into_iter().map(|(code, len)| glyph_advance(state, metrics, code, len)).sum();
    if total.is_finite() { total } else { 0.0 }
}

/// Whether `code` shows white space: by its text, or, without one, a single-byte space. Word spacing
/// still follows the byte (§9.3.3), but a font whose code 32 draws a glyph shows no space there, and
/// a two-byte font's space is not code 32.
fn blank(metrics: &Metrics, code: u32, len: usize) -> bool {
    match metrics.text_of(code) {
        Some(text) if !text.is_empty() => text.chars().all(char::is_whitespace),
        _ => metrics.is_space(code, len),
    }
}

/// How far one glyph moves the text position, in text space.
fn glyph_advance(state: &State, metrics: &Metrics, code: u32, len: usize) -> f64 {
    let spacing = state.char_spacing + if metrics.is_space(code, len) { state.word_spacing } else { 0.0 };
    (metrics.width(code) * state.size + spacing) * state.h_scale
}

/// Add `text` to `kept`, up to [`MAX_PREFIX`] characters: without leading white space when `kept`
/// is empty, and after a space when `gap` separates it from what `kept` holds. Whether text other
/// than white space was left out.
fn keep_start(kept: &mut String, text: &str, gap: bool) -> bool {
    let text = if kept.is_empty() { text.trim_start() } else { text };
    let count = kept.chars().count();
    if count < MAX_PREFIX && gap && !kept.is_empty() && !kept.ends_with(char::is_whitespace) && !text.starts_with(char::is_whitespace) {
        kept.push(' ');
    }
    let room = MAX_PREFIX.saturating_sub(kept.chars().count());
    kept.extend(text.chars().take(room));
    text.chars().skip(room).any(|c| !c.is_whitespace())
}

/// The evidence for each `(BT, ET)` operator span of a page, in order. `resources` is the page's
/// `/Font` resource dictionary; `fonts` is shared by the document's pages.
pub(crate) fn evidence(doc: &Document, fonts: &mut Fonts, resources: &Dict, ops: &[Op], spans: &[(usize, usize)]) -> Vec<Evidence> {
    let mut by_name: HashMap<Vec<u8>, Option<Rc<Metrics>>> = HashMap::new();
    let mut state = State {
        ctm: Matrix::IDENTITY,
        tm: Matrix::IDENTITY,
        tlm: Matrix::IDENTITY,
        size: 0.0,
        leading: 0.0,
        char_spacing: 0.0,
        word_spacing: 0.0,
        h_scale: 1.0,
        invisible: false,
        metrics: None,
    };
    let (mut saved, mut unsaved) = (Vec::new(), 0usize);
    let mut out = Vec::with_capacity(spans.len());
    let mut spans = spans.iter().peekable();
    let mut current: Option<(usize, Shown)> = None;
    for (at, op) in ops.iter().enumerate() {
        if current.is_none() && spans.peek().is_some_and(|s| s.0 == at) {
            current = spans.next().map(|s| (s.1, Shown::default()));
        }
        let object = current.as_mut().map(|c| &mut c.1);
        match op.op.as_slice() {
            b"q" if saved.len() < MAX_SAVED => saved.push(state.clone()),
            b"q" => unsaved = unsaved.saturating_add(1),
            b"Q" if unsaved > 0 => unsaved -= 1,
            b"Q" => {
                if let Some(s) = saved.pop() {
                    state = s;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    state.ctm = Matrix(m).then(&state.ctm);
                }
            }
            b"BT" => (state.tm, state.tlm) = (Matrix::IDENTITY, Matrix::IDENTITY),
            b"Tf" => {
                state.size = op.num(1).unwrap_or(state.size);
                state.metrics = op
                    .name(0)
                    .and_then(|name| by_name.entry(name.to_vec()).or_insert_with(|| resources.get(name).and_then(|f| fonts.get(doc, f))).clone());
            }
            b"TL" => state.leading = op.num(0).unwrap_or(state.leading),
            b"Tc" => state.char_spacing = op.num(0).filter(|n| n.is_finite()).unwrap_or(state.char_spacing),
            b"Tw" => state.word_spacing = op.num(0).filter(|n| n.is_finite()).unwrap_or(state.word_spacing),
            b"Tz" => state.h_scale = op.num(0).filter(|n| n.is_finite()).map_or(state.h_scale, |n| n / 100.0),
            b"Tr" => state.invisible = matches!(op.num(0), Some(m) if m == 3.0 || m == 7.0),
            b"Td" | b"TD" => {
                if let Some([x, y]) = op.nums::<2>() {
                    if op.op == b"TD" {
                        state.leading = -y;
                    }
                    state.tlm = Matrix::translate(x, y).then(&state.tlm);
                    state.tm = state.tlm;
                    if let Some(o) = object {
                        o.gap = true;
                        if y != 0.0 {
                            o.new_line();
                        }
                    }
                }
            }
            b"Tm" => {
                if let Some(m) = op.nums::<6>() {
                    let moved = m[5] != state.tlm.0[5];
                    (state.tm, state.tlm) = (Matrix(m), Matrix(m));
                    if let Some(o) = object {
                        o.gap = true;
                        if moved {
                            o.new_line();
                        }
                    }
                }
            }
            b"T*" | b"'" | b"\"" => {
                state.tlm = Matrix::translate(0.0, -state.leading).then(&state.tlm);
                state.tm = state.tlm;
                if let Some(o) = object {
                    o.new_line();
                    if op.op == b"\""
                        && let (Some(w), Some(c)) = (op.num(0), op.num(1))
                    {
                        (state.word_spacing, state.char_spacing) = (w, c);
                    }
                    if op.op != b"T*"
                        && let Some(Object::String(s)) = op.operands.last()
                    {
                        let moved = o.show(&state, at, &s.bytes);
                        state.tm = Matrix::translate(moved, 0.0).then(&state.tm);
                    }
                }
            }
            b"Tj" => {
                if let (Some(o), Some(Object::String(s))) = (object, op.operands.first()) {
                    let moved = o.show(&state, at, &s.bytes);
                    state.tm = Matrix::translate(moved, 0.0).then(&state.tm);
                }
            }
            b"TJ" => {
                if let (Some(o), Some(Object::Array(items))) = (object, op.operands.first()) {
                    // The adjustments between two strings add up: `-250 250` leaves no gap.
                    let mut adjusted = 0.0;
                    for item in items {
                        match item {
                            // An empty string shows nothing, so the adjustments around it add up.
                            Object::String(s) if s.bytes.is_empty() => {}
                            Object::String(s) => {
                                o.gap |= adjusted <= -WORD_GAP;
                                adjusted = 0.0;
                                let moved = o.show(&state, at, &s.bytes);
                                state.tm = Matrix::translate(moved, 0.0).then(&state.tm);
                            }
                            other => {
                                let Some(n) = other.as_f64().filter(|n| n.is_finite()) else { continue };
                                adjusted += n;
                                state.tm = Matrix::translate(-n / 1000.0 * state.size * state.h_scale, 0.0).then(&state.tm);
                            }
                        }
                    }
                    // A gap after the last string separates it from the next show (`[(1.) -300] TJ`).
                    o.gap |= adjusted <= -WORD_GAP;
                }
            }
            _ => {}
        }
        if current.as_ref().is_some_and(|c| c.0 == at)
            && let Some((_, o)) = current.take()
        {
            out.push(o.finish());
        }
    }
    out.resize(spans.len().saturating_add(out.len()), Evidence::default());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(size: f64, chars: usize) -> Evidence {
        Evidence { size, bold: false, lines: 1, chars, letters: true, ..Evidence::default() }
    }

    fn bold(e: Evidence) -> Evidence {
        Evidence { bold: true, ..e }
    }

    #[test]
    fn a_line_larger_than_the_body_is_a_heading_and_body_text_is_not() {
        let objects = [text(20.0, 10), text(12.0, 400), text(12.0, 300)];
        assert_eq!(roles(&objects), ["H1", "P", "P"]);
    }

    #[test]
    fn the_body_is_the_size_with_the_most_characters_not_the_most_objects() {
        // Ten short 9 pt captions and three 12 pt body lines: the body lines hold more characters,
        // so 12 pt is the body. Counting objects would make 9 pt the body and the body lines headings.
        let mut objects = vec![text(12.0, 150); 3];
        objects.extend(std::iter::repeat_n(text(9.0, 5), 10));
        assert!(roles(&objects).iter().all(|r| *r == "P"));
    }

    #[test]
    fn bold_at_body_size_is_a_heading_only_where_the_body_is_not_bold() {
        let plain_body = [bold(text(12.0, 15)), text(12.0, 500)];
        assert_eq!(roles(&plain_body), ["H1", "P"]);
        let bold_body = [bold(text(12.0, 15)), bold(text(12.0, 500))];
        assert_eq!(roles(&bold_body), ["P", "P"]);
    }

    #[test]
    fn long_text_numbers_alone_and_four_lines_stay_paragraphs() {
        let long = text(20.0, MAX_CHARS + 1);
        let number = Evidence { letters: false, ..text(20.0, 2) };
        let four_lines = Evidence { lines: 4, ..text(20.0, 40) };
        let two_bold_lines = Evidence { lines: 3, ..bold(text(12.0, 40)) };
        assert_eq!(roles(&[long, number, four_lines, two_bold_lines, text(12.0, 2_000)]), ["P"; 5]);
    }

    #[test]
    fn levels_rank_styles_largest_first_and_never_skip_a_level() {
        // Styles: 24 pt, 18 pt, bold 12 pt. The first heading met is 18 pt, ranked second, which
        // would skip H1: it becomes H1, and the bold heading after it H2, not H3.
        let objects = [text(18.0, 10), bold(text(12.0, 10)), text(24.0, 10), text(18.0, 10), bold(text(12.0, 10)), text(12.0, 3_000)];
        assert_eq!(roles(&objects), ["H1", "H2", "H1", "H2", "H3", "P"]);
    }

    #[test]
    fn levels_stop_at_six() {
        let mut objects: Vec<Evidence> = (0..8).map(|i| text(40.0 - f64::from(i) * 2.0, 10)).collect();
        objects.push(text(12.0, 5_000));
        assert_eq!(roles(&objects), ["H1", "H2", "H3", "H4", "H5", "H6", "H6", "H6", "P"]);
    }

    #[test]
    fn a_document_without_text_has_only_paragraphs() {
        assert_eq!(roles(&[Evidence::default(), Evidence::default()]), ["P", "P"]);
        assert!(roles(&[]).is_empty());
    }

    /// The evidence for each `BT`..`ET` of `content`, drawn in Helvetica-Bold as `/F1`.
    fn walk(content: &[u8]) -> Vec<Evidence> {
        let doc = Document::new_empty();
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name("Helvetica-Bold"));
        let mut resources = Dict::new();
        resources.set(b"F1".to_vec(), Object::Dict(font));
        let ops = pdfcraft_content::parse(content).ops;
        let at = |name: &[u8]| -> Vec<usize> { ops.iter().enumerate().filter(|(_, o)| o.op == name).map(|(i, _)| i).collect() };
        let spans: Vec<(usize, usize)> = at(b"BT").into_iter().zip(at(b"ET")).collect();
        evidence(&doc, &mut Fonts::default(), &resources, &ops, &spans)
    }

    #[test]
    fn size_comes_from_the_font_size_the_text_matrix_and_the_ctm() {
        let found = walk(
            b"q 2 0 0 2 0 0 cm BT /F1 1 Tf 9 0 0 9 10 10 Tm (Title) Tj ET Q BT /F1 12 Tf 10 50 Td (one) Tj T* (two) ' 0 -14 Td (three) Tj ET BT ET",
        );
        let style = |e: &Evidence| (e.size, e.bold, e.lines, e.chars, e.letters, e.marker, e.label_op, e.x, e.y);
        assert_eq!(style(&found[0]), (18.0, true, 1, 5, true, None, None, 20.0, 20.0));
        assert_eq!(style(&found[1]), (12.0, true, 3, 11, true, None, None, 10.0, 50.0));
        assert_eq!(found[2], Evidence::default());
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn line_moves_leading_invisible_text_and_absurd_sizes() {
        let found = walk(
            b"BT /F1 10 Tf 1 0 0 1 0 90 Tm (a) Tj 1 0 0 1 40 90 Tm (b) Tj 1 0 0 1 0 70 Tm (c) Tj ET               BT /F1 10 Tf 14 TL 0 0 Td (a) Tj 0 0 (b) \" 0 -12 TD (c) Tj T* (d) Tj ET               BT /F1 10 Tf 3 Tr (hidden) Tj 7 Tr (clip) Tj 0 Tr (shown) Tj ET               BT /F1 2000000 Tf (huge) Tj ET",
        );
        assert_eq!(found[0].lines, 2, "a Tm on the same baseline is not a new line");
        assert_eq!(found[1].lines, 4, "\" and TD each start a line, and T* uses TD's leading");
        assert_eq!(found[2].chars, 5, "render mode 3 and 7 text is not counted");
        assert_eq!(found[3], Evidence::default(), "a size past 1e6 is dropped");
    }

    #[test]
    fn a_list_marker_is_read_across_kerning_split_shows_and_leading_space_and_its_own_show_is_found() {
        let found = walk(
            b"BT /F1 12 Tf [(1)-15(.)-250(Apples)] TJ ET \
              BT /F1 12 Tf (                    2. Pears) Tj ET \
              BT /F1 12 Tf (3.) Tj 14 0 Td (Plums) Tj ET \
              BT /F1 12 Tf [(4.)-250(Figs)] TJ ET \
              BT /F1 12 Tf [(5.)( )(Kiwis)] TJ ET \
              BT /F1 12 Tf (6) Tj (.) Tj ( Limes) Tj ET \
              BT /F1 12 Tf (7. A long item) Tj 0 -14 Td (that wraps) Tj ET \
              BT /F1 12 Tf (8. One) Tj 0 -14 Td (9. Two) Tj ET \
              BT /F1 12 Tf (10.              Pears) Tj ET \
              BT /F1 12 Tf [(11.) -250 250 (Ugli)] TJ ET \
              BT /F1 12 Tf [(12.) -300] TJ (Dates) Tj ET \
              BT /F1 12 Tf [(13.) -100 () -100 (Yuzu)] TJ ET \
              BT /F1 12 Tf (14.              Apples) Tj (more) Tj ET",
        );
        let markers: Vec<Option<u32>> = found.iter().map(|e| e.marker.filter(|m| !m.alone).map(|m| m.value)).collect();
        // `8.` holds two marked lines, a list of its own: it is not one item. `10.` and `14.` have
        // their text past the kept start. `11.` is drawn against `Ugli`: the adjustments cancel.
        // `12.` is followed by a gap at the end of its `TJ`; `13.` by one split by an empty string.
        let expected = [Some(1), Some(2), Some(3), Some(4), Some(5), Some(6), Some(7), None, Some(10), None, Some(12), Some(13), Some(14)];
        assert_eq!(markers, expected);
        // Only the marker shown by an operator of its own can be tagged apart: the `(3.) Tj`.
        let labels: Vec<bool> = found.iter().map(|e| e.label_op.is_some()).collect();
        // `12.` is its own show too. `14.` is not: text past its kept start was left out.
        let shown_apart: Vec<usize> = labels.iter().enumerate().filter(|(_, l)| **l).map(|(i, _)| i).collect();
        assert_eq!(shown_apart, [2, 10]);
    }

    #[test]
    fn a_wrapped_line_that_only_looks_like_a_marker_keeps_the_item() {
        let found = walk(
            b"BT /F1 12 Tf 20 100 Td (1. Written by John) Tj 12 -14 Td (A. Smith and others) Tj ET \
              BT /F1 12 Tf 20 70 Td (2. Decided in Roe) Tj 12 -14 Td (v. Wade, 1973) Tj ET \
              BT /F1 12 Tf 20 40 Td (3. Built) Tj 0 -14 Td (c. 1900 by) Tj ET \
              BT /F1 12 Tf 20 10 Td (4. One) Tj 0 -14 Td (5. Two) Tj ET \
              BT /F1 12 Tf 20 -20 Td (6. One) Tj 0 -14 Td [-1000 (7. example)] TJ ET",
        );
        let markers: Vec<Option<u32>> = found.iter().map(|e| e.marker.map(|m| m.value)).collect();
        // Only `4.` continues its first line's marker at the same indent. `7.` is moved 12 pt
        // right by its `TJ` adjustment, so it is a wrapped line, not a second item.
        assert_eq!(markers, [Some(1), Some(2), Some(3), None, Some(6)]);
    }

    #[test]
    fn a_line_ends_where_its_glyphs_end() {
        let found = walk(
            b"BT /F1 12 Tf 20 100 Td (Apples) Tj ET \
              BT /F1 12 Tf 20 80 Td (Apples) Tj (Pears) Tj ET \
              BT /F1 12 Tf 20 60 Td (Apples) Tj 0 -14 Td (Pears) Tj ET",
        );
        assert!(found[0].end > found[0].x, "{found:?}");
        // A second show continues after the first.
        assert!(found[1].end > found[0].end, "{found:?}");
        // The last line ends where its own text does; the furthest line end is kept apart.
        assert!(found[2].end < found[2].right && found[2].right == found[0].end, "{found:?}");
        // The last line's baseline is kept apart from the first's.
        assert!(found[2].last_y < found[2].y && found[1].last_y == found[1].y, "{found:?}");
        // Left of the page origin, a line still ends right of where it starts.
        let left = walk(b"BT /F1 12 Tf -200 100 Td (Apples) Tj ET");
        assert!(left[0].right > left[0].x && left[0].right < 0.0, "{left:?}");
    }

    #[test]
    fn the_first_word_is_measured_across_shows_up_to_a_space_or_a_move() {
        let found = walk(
            b"BT /F1 12 Tf 20 100 Td (Apples and pears) Tj ET \
              BT /F1 12 Tf 20 80 Td (  App) Tj (les) Tj ( and) Tj ET \
              BT /F1 12 Tf 20 60 Td (Apples) Tj 0 -14 Td (and pears) Tj ET \
              BT /F1 12 Tf 20 40 Td [(Apples) -300 (and)] TJ ET \
              BT /F1 12 Tf 20 20 Td (Apples) Tj ET",
        );
        let whole = found[4].end - found[4].x;
        // The word ends at the space, joins shows that follow each other, and ends at a line move
        // or a word gap.
        for e in &found[..4] {
            assert!((e.word - whole).abs() < 1e-6, "{found:?}");
        }
        assert!(whole > 0.0);
    }

    #[test]
    fn a_tj_adjustment_before_the_first_glyph_moves_where_the_text_starts() {
        let found = walk(b"BT /F1 12 Tf 20 100 Td [0 (1. Apples)] TJ ET BT /F1 12 Tf 20 85 Td [-1000 (2. Pears)] TJ ET");
        assert!(found[1].x > found[0].x, "{found:?}");
        assert_eq!(found[0].x, 20.0);
    }
}
