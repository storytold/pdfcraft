use super::interp::{Carry, interpret};
use super::lines::{content_streams, page_dict, reading_lines, rewritten_contents, splice, text_lines};
use super::rewrite::{LineEdit, coincident_ops, rewrite_block};
use super::{Document, EditError, HashMap, Object, Op, Pieces, TextLine, group_blocks, regions};

/// An editable text region: adjacent runs on visual rows, followed by aligned rows.
/// A region may contain several source fonts, sizes, colours and weights.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    /// The lines' text joined with spaces (lines that ended in a hyphen are joined directly).
    pub text: String,
    pub rect: [f64; 4],
    pub base_font: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// The text fill colour as RGB, used by the visual editor and preserved when no colour
    /// override is requested.
    pub color: [f64; 3],
    /// Counterclockwise baseline angle in page user space, in degrees.
    pub rotation: f64,
    /// Indexes into [`text_lines`].
    pub lines: Vec<usize>,
    /// Visual rows; each contains indexes into [`text_lines`], including mixed-style runs.
    pub rows: Vec<Vec<usize>>,
    /// Text for the inline editor, with a newline only between visual rows.
    pub editor_text: String,
    /// What a width change reflows. A wrap rejoins with a space; a break that was typed, or a
    /// short line that still had room for the next word, stays a newline.
    pub reflow_text: String,
    /// Byte ranges in `editor_text` and their source line indexes.
    pub spans: Vec<(std::ops::Range<usize>, usize)>,
    /// Every line is invisible text (rendering mode 3): an OCR layer over a scanned image, whose
    /// pixels show the words. A rewrite draws the new text visibly (see [`BlockStyle::cover`]).
    pub invisible: bool,
}

impl TextBlock {
    /// Preserve styles outside the changed ranges; inserted characters inherit the surrounding
    /// source run. The texts are aligned word by word, so words left untouched between two edits
    /// keep their own runs (#2198); past [`MAX_CHANGED_WORDS`] the draft counts as one change.
    /// Byte ranges are always UTF-8 boundaries. Linear time, including large edits.
    pub fn edited_spans(&self, draft: &str) -> Vec<(std::ops::Range<usize>, usize)> {
        if self.editor_text.len() > 4_000_000 || draft.len() > 4_000_000 {
            return vec![(0..draft.len(), self.lines.first().copied().unwrap_or(0))];
        }
        let old: Vec<_> = self.editor_text.chars().collect();
        let new: Vec<_> = draft.char_indices().collect();
        let chars: Vec<_> = new.iter().map(|(_, c)| *c).collect();
        let mut original = Vec::with_capacity(old.len());
        let mut span = 0;
        for (byte, _) in self.editor_text.char_indices() {
            while self.spans.get(span).is_some_and(|(r, _)| byte >= r.end) {
                span += 1;
            }
            original.push(self.spans.get(span).or_else(|| self.spans.last()).map_or(0, |(_, i)| *i));
        }
        let mut sources = Vec::with_capacity(new.len());
        let slices = |text: &[char]| words(text).into_iter().map(|w| text.get(w).unwrap_or_default().to_vec()).collect::<Vec<_>>();
        let (old_words, new_words) = (slices(&old), slices(&chars));
        match edit_script(&old_words, &new_words, MAX_CHANGED_WORDS) {
            Some(script) => {
                // `(o, n)`: where the current change starts in the old and new text; `(x, y)`: where it ends.
                let ((mut o, mut n), (mut x, mut y)) = ((0, 0), (0, 0));
                let (mut a, mut b) = (old_words.iter(), new_words.iter());
                for step in script {
                    match step {
                        Step::Keep => {
                            replaced(&original, o, old.get(o..x).unwrap_or_default(), chars.get(n..y).unwrap_or_default(), &mut sources);
                            let len = a.next().map_or(0, Vec::len);
                            b.next();
                            sources.extend(original.get(x..x.saturating_add(len)).unwrap_or_default());
                            (x, y) = (x.saturating_add(len), y.saturating_add(len));
                            (o, n) = (x, y);
                        }
                        Step::Delete => x = x.saturating_add(a.next().map_or(0, Vec::len)),
                        Step::Insert => y = y.saturating_add(b.next().map_or(0, Vec::len)),
                    }
                }
                replaced(&original, o, old.get(o..x).unwrap_or_default(), chars.get(n..y).unwrap_or_default(), &mut sources);
            }
            None => replaced(&original, 0, &old, &chars, &mut sources),
        }
        let fallback = original.first().copied().unwrap_or(0);
        let mut result: Vec<(std::ops::Range<usize>, usize)> = Vec::new();
        for (i, (byte, ch)) in new.iter().enumerate() {
            let source = sources.get(i).copied().unwrap_or(fallback);
            let end = byte.saturating_add(ch.len_utf8());
            if let Some((range, _)) = result.last_mut().filter(|(_, prev)| *prev == source) {
                range.end = end;
            } else {
                result.push((*byte..end, source));
            }
        }
        result
    }
}

/// Changed words [`TextBlock::edited_spans`] aligns before it treats the draft as one change.
/// Myers' diff costs O((N + M) · D), so bounding D keeps it linear.
const MAX_CHANGED_WORDS: usize = 64;

/// `text` split into words, each with the whitespace after it, as char ranges.
fn words(text: &[char]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, pair) in text.windows(2).enumerate() {
        if let [a, b] = pair
            && a.is_whitespace()
            && !b.is_whitespace()
        {
            out.push(start..i + 1);
            start = i + 1;
        }
    }
    if start < text.len() {
        out.push(start..text.len());
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Keep,
    Delete,
    Insert,
}

/// Myers' shortest edit script from `a` to `b`, or `None` when it takes more than `max` steps.
fn edit_script<T: PartialEq>(a: &[T], b: &[T], max: usize) -> Option<Vec<Step>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = max.min(a.len().saturating_add(b.len())) as isize;
    // Furthest x reached on each diagonal k = x - y, stored at k + max + 1.
    let at = |v: &[isize], k: isize| usize::try_from(k + max + 1).ok().and_then(|i| v.get(i).copied()).unwrap_or(0);
    let mut v = vec![0isize; (2 * max + 3) as usize];
    let mut trace = Vec::new();
    for d in 0..=max {
        trace.push(v.clone());
        for k in (-d..=d).step_by(2) {
            let down = k == -d || (k != d && at(&v, k - 1) < at(&v, k + 1));
            let mut x = if down { at(&v, k + 1) } else { at(&v, k - 1) + 1 };
            let mut y = x - k;
            while x < n && y < m && a.get(x as usize) == b.get(y as usize) {
                x += 1;
                y += 1;
            }
            if let Some(slot) = usize::try_from(k + max + 1).ok().and_then(|i| v.get_mut(i)) {
                *slot = x;
            }
            if x >= n && y >= m {
                return Some(backtrack(&trace, n, m, max));
            }
        }
    }
    None
}

/// Walk Myers' saved frontiers back from the end to the edit script.
fn backtrack(trace: &[Vec<isize>], n: isize, m: isize, max: isize) -> Vec<Step> {
    let at = |v: &[isize], k: isize| usize::try_from(k + max + 1).ok().and_then(|i| v.get(i).copied()).unwrap_or(0);
    let mut steps = Vec::new();
    let (mut x, mut y) = (n, m);
    for (d, v) in trace.iter().enumerate().rev() {
        let (d, k) = (d as isize, x - y);
        let prev_k = if k == -d || (k != d && at(v, k - 1) < at(v, k + 1)) { k + 1 } else { k - 1 };
        let prev_x = at(v, prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            steps.push(Step::Keep);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            steps.push(if x == prev_x { Step::Insert } else { Step::Delete });
        }
        (x, y) = (prev_x, prev_y);
    }
    steps.reverse();
    steps
}

/// `old`, at char `at` of the source, became `new`: characters kept at either end keep their
/// runs, and the rest inherit the run just before the change (or the first one it replaces).
fn replaced(original: &[usize], at: usize, old: &[char], new: &[char], out: &mut Vec<usize>) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old.iter().rev().zip(new.iter().rev()).take(old.len().min(new.len()).saturating_sub(prefix)).take_while(|(a, b)| a == b).count();
    let start = at.saturating_add(prefix);
    let inherited = start.checked_sub(1).and_then(|i| original.get(i)).or_else(|| original.get(start)).copied().unwrap_or(0);
    for i in 0..new.len() {
        let source = if i < prefix {
            original.get(at.saturating_add(i)).copied()
        } else if i >= new.len().saturating_sub(suffix) {
            original.get(at.saturating_add(old.len()).saturating_sub(new.len() - i)).copied()
        } else {
            Some(inherited)
        };
        out.push(source.unwrap_or(inherited));
    }
}

/// How a listing of [`text_blocks`] records a paragraph so that it can be found again.
///
/// The index of a paragraph is its position in a list regrouped from geometry on every read, so
/// an edit that merges or splits paragraphs makes the same index name another one (#2516). A key
/// names the paragraph by what it says and where it is; a paragraph that no longer exists as
/// such has no match, instead of a neighbour taking its place.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockKey {
    text: String,
    rect: [f64; 4],
}

impl BlockKey {
    /// Two boxes are the same box within this many points.
    const TOLERANCE: f64 = 0.01;

    pub fn of(block: &TextBlock) -> Self {
        Self { text: block.text.clone(), rect: block.rect }
    }

    /// The paragraph's box when it was recorded (user space).
    pub fn rect(&self) -> [f64; 4] {
        self.rect
    }

    /// The paragraph in `blocks` with this text and box. `hint` is tried first, so two
    /// paragraphs drawn on top of one another are told apart by their order.
    pub fn find(&self, blocks: &[TextBlock], hint: usize) -> Option<usize> {
        let matches = |b: &TextBlock| b.text == self.text && b.rect.iter().zip(&self.rect).all(|(a, b)| (a - b).abs() <= Self::TOLERANCE);
        match blocks.get(hint) {
            Some(b) if matches(b) => Some(hint),
            _ => blocks.iter().position(matches),
        }
    }
}

/// The paragraph in `blocks` whose box overlaps `rect` the most, if any overlaps it at all. An
/// edit rewrites a paragraph in place, so this is the paragraph it became.
pub fn block_overlapping(blocks: &[TextBlock], rect: [f64; 4]) -> Option<usize> {
    // `f64::min` and `max` skip a NaN, which would make a box that isn't a number overlap anything.
    let area = |b: &TextBlock| {
        if !rect.iter().chain(&b.rect).all(|v| v.is_finite()) {
            return 0.0;
        }
        let w = b.rect[2].min(rect[2]) - b.rect[0].max(rect[0]);
        let h = b.rect[3].min(rect[3]) - b.rect[1].max(rect[1]);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    };
    blocks.iter().enumerate().map(|(i, b)| (i, area(b))).filter(|(_, a)| *a > 0.0).max_by(|(_, a), (_, b)| a.total_cmp(b)).map(|(i, _)| i)
}

/// The paragraphs on a page (0-based).
pub fn text_blocks(doc: &Document, page: usize) -> Result<Vec<TextBlock>, EditError> {
    let lines = text_lines(doc, page)?;
    Ok(group_blocks(&lines))
}

/// Remove a paragraph's glyphs for a temporary editor background. Numeric TJ adjustments
/// preserve the text matrix, so unselected text positioned relative to it stays in place.
/// Call this on a clone: it is a preview operation, not a document edit.
pub fn clear_block(doc: &mut Document, page: usize, block: usize) -> Result<(), EditError> {
    let lines = text_lines(doc, page)?;
    let blocks = group_blocks(&lines);
    let target = blocks.get(block).ok_or_else(|| EditError::Invalid("the paragraph no longer exists".into()))?;
    let targets: Vec<&TextLine> = target.lines.iter().filter_map(|i| lines.get(*i)).collect();
    targets.iter().try_for_each(|line| line.ensure_editable())?;
    let mut drops = coincident_ops(&lines, &targets);
    drops.extend(target.lines.iter().filter_map(|i| lines.get(*i)).flat_map(|l| l.ops.iter().copied()));
    let p = page_dict(doc, page)?;
    let res = p.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let streams = content_streams(doc, &p.dict)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let shown = interpret(doc, &ops, &res, &mut HashMap::new(), &mut Carry::new(), None, None)?;
    let mut patches = HashMap::new();
    for s in shown.into_iter().filter(|s| drops.contains(&s.op)) {
        if s.state.render_mode >= 4 {
            return Err(EditError::Invalid("text used as a clipping path cannot be previewed for editing".into()));
        }
        let denominator = s.state.size * s.state.scale;
        let adjustment = if denominator != 0.0 { -1000.0 * s.advance / denominator } else { 0.0 };
        if !adjustment.is_finite() || (denominator == 0.0 && s.advance != 0.0) {
            return Err(EditError::Invalid("the paragraph's text advance cannot be previewed".into()));
        }
        let mut replacement = Vec::new();
        if let Some(op) = ops.get(s.op) {
            if op.is("\"") {
                replacement.push(Op::new("Tw", vec![op.operands.first().cloned().unwrap_or(Object::Int(0))]));
                replacement.push(Op::new("Tc", vec![op.operands.get(1).cloned().unwrap_or(Object::Int(0))]));
            }
            if op.is("'") || op.is("\"") {
                replacement.push(Op::new("T*", vec![]));
            }
        }
        replacement.push(Op::new("TJ", vec![Object::Array(vec![pdfcraft_content::num(adjustment)])]));
        patches.insert(s.op, replacement);
    }
    let data = splice(&pieces, &ops, |i| patches.remove(&i).map_or((Vec::new(), true), |v| (v, false)));
    let contents = rewritten_contents(doc, &streams, data);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

/// The paragraphs on a page as a reader sees them, including text drawn by form XObjects
/// (Export to Word, HTML and RTF). Read-only: indexes into this list are not
/// [`text_blocks`] indexes, so they can't be passed to [`replace_block`] or [`rewrite_block`].
/// [`reading_blocks_checked`] also says whether text was left out.
pub fn reading_blocks(doc: &Document, page: usize) -> Result<Vec<TextBlock>, EditError> {
    reading_blocks_checked(doc, page).map(|r| r.blocks)
}

/// A page's paragraphs as a reader sees them ([`reading_blocks_checked`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadingBlocks {
    /// The paragraphs, as [`reading_blocks`] returns them.
    pub blocks: Vec<TextBlock>,
    /// The page draws forms nested deeper than [`MAX_FORM_DEPTH`], more of them than
    /// [`MAX_FORM_VISITS`], or forms that run past what the page's content left of its decode
    /// budget. Reading leaves those forms out to stay bounded on hostile files, so their text
    /// is missing from `blocks`; [`crate::reading_images`] walks the same forms and misses
    /// their images too.
    pub truncated: bool,
}

/// [`reading_blocks`], saying whether the limits that bound reading left text out. A page
/// whose `/Contents` alone runs past its decode budget is an [`EditError::Invalid`], not a
/// truncated result.
pub fn reading_blocks_checked(doc: &Document, page: usize) -> Result<ReadingBlocks, EditError> {
    let (mut lines, truncated) = reading_lines(doc, page)?;
    // A reader sees letters the matrix leans as italic: that is how word processors draw italic in
    // a face that has none, and the font's name and flags say nothing of it. Editing reads them as
    // the upright face they are.
    for line in lines.iter_mut().filter(|l| leans_right(l.origin.tm, l.origin.ctm)) {
        line.italic = true;
    }
    Ok(ReadingBlocks { blocks: regions::group_reading_blocks(&lines), truncated })
}

/// How far (degrees) text whose letters lean right through its matrix is from upright, at least
/// and at most, to read as italic: LibreOffice leans them 15 degrees.
const ITALIC_LEAN: std::ops::RangeInclusive<f64> = 5.0..=45.0;

/// Whether the text matrix `tm` and the CTM `ctm` lean letters to the right by [`ITALIC_LEAN`]:
/// the angle between text space's y axis and the upright of its baseline on the page, whatever
/// way the baseline turns. Text leaning left, mirrored text and a degenerate matrix are upright.
fn leans_right(tm: [f64; 6], ctm: [f64; 6]) -> bool {
    // Text space's x and y axes on the page.
    let x = (tm[0] * ctm[0] + tm[1] * ctm[2], tm[0] * ctm[1] + tm[1] * ctm[3]);
    let y = (tm[2] * ctm[0] + tm[3] * ctm[2], tm[2] * ctm[1] + tm[3] * ctm[3]);
    let (along, across) = (x.0 * y.0 + x.1 * y.1, x.0 * y.1 - x.1 * y.0);
    across > 0.0 && ITALIC_LEAN.contains(&along.atan2(across).to_degrees())
}

// An ideographic space carries intentional Japanese spacing, including paragraph indentation.
// Keep other whitespace normalization unchanged.
pub(crate) fn paragraph_separator(c: char) -> bool {
    c.is_whitespace() && c != '\u{3000}'
}

/// Greedy word wrapping to `width` with `advance` giving a string's width. Each `\n` starts a
/// new line. A row may also end inside a word of Chinese or Japanese text ([`crate::linebreak`]);
/// those pieces join without a space. Every white-space character is written as one space, so
/// a run of spaces typed between, before or after words stays (#4199); only the spaces where a
/// row wraps are dropped (recorded as the third value, so a later rejoin can restore the exact
/// count instead of guessing one, #2529). The bool is true when the line continues the previous
/// one because the next word did not fit, rather than because of a newline; the number is how
/// many separator characters stood where that wrap fell (0 for a Chinese/Japanese word split,
/// which has no space of its own; meaningless when the bool is false).
pub(super) fn wrap(text: &str, width: f64, advance: impl Fn(&str) -> f64) -> Vec<(String, bool, usize)> {
    let mut out = Vec::new();
    for row in text.split('\n') {
        let mut line = String::new();
        let mut soft = false;
        // The gap consumed right before `line` started: 0 until a wrap sets it.
        let mut wrap_gap = 0usize;
        let start = out.len();
        // The separators read since the last word: each one boundary between `split` pieces.
        let mut gap = 0usize;
        for (k, word) in row.split(paragraph_separator).enumerate() {
            if k > 0 {
                gap = gap.saturating_add(1);
            }
            if word.is_empty() {
                continue;
            }
            let gap_here = std::mem::take(&mut gap);
            let spaces = " ".repeat(gap_here);
            for (i, piece) in crate::linebreak::pieces(word).into_iter().enumerate() {
                let candidate = if i == 0 { format!("{line}{spaces}{piece}") } else { format!("{line}{piece}") };
                // A row holding only the spaces that indent it doesn't wrap before its first word.
                if line.chars().any(|c| c != ' ') && advance(&candidate) > width {
                    out.push((std::mem::take(&mut line), soft, wrap_gap));
                    soft = true;
                    // A mid-word piece (i > 0) had no separator of its own; the first piece of a
                    // word had exactly the separators just taken.
                    wrap_gap = if i == 0 { gap_here } else { 0 };
                    line = piece.to_string();
                } else {
                    line = candidate;
                }
            }
        }
        line.push_str(&" ".repeat(gap));
        if !line.is_empty() || out.len() == start {
            out.push((line, soft, wrap_gap));
        }
    }
    out
}

/// Replace paragraph `block` (an index into [`text_blocks`]) with `text`, rewrapped to the
/// paragraph's width with its line spacing. The paragraph keeps its first line's position, font
/// (or Helvetica when the font can't show the text), size and colour.
pub fn replace_block(doc: &mut Document, page: usize, block: usize, text: &str) -> Result<LineEdit, EditError> {
    rewrite_block(doc, page, block, Some(text), &BlockStyle::default())
}

/// Formatting for a rewritten paragraph (`None` keeps the paragraph's own).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockStyle {
    /// Disjoint UTF-8 ranges in the replacement, each preserving unspecified source style.
    pub ranges: Vec<RangeStyle>,
    /// Explicit page font resource (from register_text_font or the page font inventory).
    pub resource: Option<String>,
    /// A standard font family, bold, italic.
    pub family: Option<(crate::added::Family, bool, bool)>,
    /// Override weight without choosing a different family (`None` keeps the source weight).
    pub bold: Option<bool>,
    /// Font size in points (user space).
    pub size: Option<f64>,
    /// Fill colour (RGB 0–1).
    pub color: Option<[f64; 3]>,
    pub align: Option<crate::added::Align>,
    /// Rows (the text's lines between `\n`s) aligned other than the paragraph, by index; `None`
    /// and rows past the end follow `align` ([`crate::paragraph_align`]).
    pub aligns: Vec<Option<crate::added::Align>>,
    /// Draw a line under each line of text.
    pub underline: Option<bool>,
    /// Line spacing as a multiple of the font size (1.2 is ordinary).
    pub line_spacing: Option<f64>,
    /// Character spacing (points) and horizontal scaling (percent).
    pub char_spacing: Option<f64>,
    pub scale: Option<f64>,
    /// Move the paragraph by `[dx, dy]` in user space (dragging its box).
    pub offset: Option<[f64; 2]>,
    /// Rewrap to this width in user space (dragging the box's edge).
    pub width: Option<f64>,
    /// For an invisible paragraph ([`TextBlock::invisible`]): fill its old area with this colour
    /// (RGB 0–1) before the new text, hiding the scanned word beneath it.
    pub cover: Option<[f64; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RangeStyle {
    pub range: std::ops::Range<usize>,
    pub family: Option<(crate::added::Family, bool, bool)>,
    pub resource: Option<String>,
    pub size: Option<f64>,
    pub color: Option<[f64; 3]>,
    pub underline: Option<bool>,
    pub char_spacing: Option<f64>,
    pub scale: Option<f64>,
}
