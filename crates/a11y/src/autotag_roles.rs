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
}

impl Shown {
    fn show(&mut self, state: &State, bytes: &[u8]) {
        let Some(metrics) = state.metrics.as_ref().filter(|_| !state.invisible) else { return };
        let text = metrics.decode(bytes);
        let chars = text.chars().filter(|c| !c.is_whitespace()).count();
        if chars == 0 {
            return;
        }
        self.letters |= text.chars().any(char::is_alphabetic);
        if !self.shown {
            (self.shown, self.lines) = (true, 1);
        } else if self.moved {
            self.lines = self.lines.saturating_add(1);
        }
        self.moved = false;
        let [_, _, c, d, _, _] = state.tm.then(&state.ctm).0;
        let size = (state.size * c.hypot(d)).abs();
        if !size.is_finite() || size <= 0.0 || size > 1e6 {
            return;
        }
        let entry = self.by_size.entry(half_points(size)).or_default();
        entry.0 = entry.0.saturating_add(chars);
        if metrics.bold {
            entry.1 = entry.1.saturating_add(chars);
        }
    }

    /// A line move counts once text is shown after it, so blank lines and trailing moves do not.
    fn new_line(&mut self) {
        self.moved = self.shown;
    }

    fn finish(self) -> Evidence {
        let chars = self.by_size.values().fold(0usize, |n, v| n.saturating_add(v.0));
        let Some((size, (count, bold))) = self.by_size.into_iter().max_by_key(|&(s, (n, _))| (n, s)) else {
            return Evidence::default();
        };
        Evidence { size: size as f64 / 2.0, bold: bold.saturating_mul(2) > count, lines: self.lines, chars, letters: self.letters }
    }
}

/// The evidence for each `(BT, ET)` operator span of a page, in order. `resources` is the page's
/// `/Font` resource dictionary; `fonts` is shared by the document's pages.
pub(crate) fn evidence(doc: &Document, fonts: &mut Fonts, resources: &Dict, ops: &[Op], spans: &[(usize, usize)]) -> Vec<Evidence> {
    let mut by_name: HashMap<Vec<u8>, Option<Rc<Metrics>>> = HashMap::new();
    let mut state =
        State { ctm: Matrix::IDENTITY, tm: Matrix::IDENTITY, tlm: Matrix::IDENTITY, size: 0.0, leading: 0.0, invisible: false, metrics: None };
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
            b"Tr" => state.invisible = matches!(op.num(0), Some(m) if m == 3.0 || m == 7.0),
            b"Td" | b"TD" => {
                if let Some([x, y]) = op.nums::<2>() {
                    if op.op == b"TD" {
                        state.leading = -y;
                    }
                    state.tlm = Matrix::translate(x, y).then(&state.tlm);
                    state.tm = state.tlm;
                    if y != 0.0
                        && let Some(o) = object
                    {
                        o.new_line();
                    }
                }
            }
            b"Tm" => {
                if let Some(m) = op.nums::<6>() {
                    let moved = m[5] != state.tlm.0[5];
                    (state.tm, state.tlm) = (Matrix(m), Matrix(m));
                    if moved && let Some(o) = object {
                        o.new_line();
                    }
                }
            }
            b"T*" | b"'" | b"\"" => {
                state.tlm = Matrix::translate(0.0, -state.leading).then(&state.tlm);
                state.tm = state.tlm;
                if let Some(o) = object {
                    o.new_line();
                    if op.op != b"T*"
                        && let Some(Object::String(s)) = op.operands.last()
                    {
                        o.show(&state, &s.bytes);
                    }
                }
            }
            b"Tj" => {
                if let (Some(o), Some(Object::String(s))) = (object, op.operands.first()) {
                    o.show(&state, &s.bytes);
                }
            }
            b"TJ" => {
                if let (Some(o), Some(Object::Array(items))) = (object, op.operands.first()) {
                    for item in items {
                        if let Object::String(s) = item {
                            o.show(&state, &s.bytes);
                        }
                    }
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
        Evidence { size, bold: false, lines: 1, chars, letters: true }
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
        assert_eq!(found[0], Evidence { size: 18.0, bold: true, lines: 1, chars: 5, letters: true });
        assert_eq!(found[1], Evidence { size: 12.0, bold: true, lines: 3, chars: 11, letters: true });
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
}
