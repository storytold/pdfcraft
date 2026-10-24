//! Text extraction (bootstrap for `pdfcraft-text`, architecture §8).
//!
//! A hayro `Device` that ignores paint and records every glyph: its Unicode value (ToUnicode →
//! encoding → glyph name fallbacks, handled by hayro) and its box in *page view space*, i.e.
//! points with the origin at the top-left of the displayed page, rotation applied, y down. The UI
//! maps view space to the screen with a single scale.
//!
//! On top of the glyph list: line grouping (baseline clustering), word gaps, plain-text output,
//! case-insensitive search and reading-order selection.

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, interpret_page,
};
use kurbo::{Affine, BezPath, Rect, Shape};
use std::collections::{HashMap, HashSet};

/// Semantic exclusions for statistics. Untagged text is retained; no layout guess is made.
#[derive(Clone, Debug, Default)]
pub struct TextFilter {
    pub tables: bool,
    pub code: bool,
    /// Exclude Figure-marked text and invisible text layers, including OCR.
    pub images: bool,
    /// Page-local MCIDs whose structure ancestors match the selected exclusions.
    pub mcids: HashSet<i32>,
}

impl TextFilter {
    fn excludes(&self, tag: &[u8], mcid: Option<i32>) -> bool {
        (self.tables && matches!(tag, b"Table" | b"TR" | b"TH" | b"TD" | b"THead" | b"TBody" | b"TFoot"))
            || (self.code && tag == b"Code")
            || (self.images && tag == b"Figure")
            || mcid.is_some_and(|id| self.mcids.contains(&id))
    }
}

mod search;

/// Whether `(x, y)` lies in or on the convex quadrilateral `quad`, whichever way it winds. A
/// quad with a NaN corner, or no area, holds nothing.
fn quad_contains(quad: &[[f32; 2]; 4], x: f32, y: f32) -> bool {
    let (mut positive, mut negative) = (false, false);
    for (k, a) in quad.iter().enumerate() {
        let b = quad[(k + 1) % 4];
        let cross = (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
        positive |= cross > 0.0;
        negative |= cross < 0.0;
        if cross.is_nan() {
            return false;
        }
    }
    positive != negative
}

/// One glyph on the page.
#[derive(Clone, Debug, PartialEq)]
pub struct TextGlyph {
    /// The glyph's Unicode text: one character, or several for a ligature.
    pub text: String,
    /// Box in page view space (points, y down): [x0, y0, x1, y1]. The bounding box of `quad`.
    pub rect: [f32; 4],
    /// The glyph's em box in page view space, as it is drawn (rotated, skewed or mirrored): the
    /// start and end of its edge towards the next line, then the end and start of the opposite
    /// edge. `quad[0]` → `quad[1]` runs in the writing direction.
    pub quad: [[f32; 2]; 4],
    /// Unit vector of the writing direction in page view space (`[1, 0]`: left to right).
    pub direction: [f32; 2],
}

/// `s` folded for case-insensitive search, one character for one character.
///
/// The dotted and dotless i of Turkish and Azerbaijani (`İ` U+0130, `ı` U+0131) and the Latin
/// `I`/`i` all fold to `i`. `str::to_lowercase` follows no locale: it turns `İ` into `i` plus a
/// combining dot (two characters) and leaves `ı` alone, so neither `İSTANBUL` nor `IRMAK` could
/// be found by typing `istanbul` or `ırmak`. Search can't know the document's language, so the
/// four are treated as one letter; this only adds matches where an `ı` or `İ` is involved.
/// Greek sigma's normal and word-final forms (`σ`/`ς`) both fold to `σ`, as does `Σ`.
pub fn fold_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'I' | 'i' | '\u{130}' | '\u{131}' => out.push('i'),
            'Σ' | 'σ' | 'ς' => out.push('σ'),
            _ => out.extend(c.to_lowercase()),
        }
    }
    out
}

/// The text of one page, in content order, with line structure.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageText {
    pub glyphs: Vec<TextGlyph>,
    /// For each glyph: its line index (lines are in reading order, top to bottom).
    pub line_of: Vec<u32>,
    /// `true` when a word break precedes the glyph on the same line.
    pub space_before: Vec<bool>,
}

impl PageText {
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// Text of glyphs `range` with spaces and line breaks reconstructed.
    pub fn text_of(&self, range: std::ops::Range<usize>) -> String {
        let mut s = String::new();
        let mut prev_line = None;
        for i in range.clone() {
            let Some(g) = self.glyphs.get(i) else { break };
            if i != range.start {
                if prev_line != Some(self.line_of[i]) {
                    s.push('\n');
                } else if self.space_before[i] {
                    s.push(' ');
                }
            }
            s.push_str(&g.text);
            prev_line = Some(self.line_of[i]);
        }
        s
    }

    pub fn plain_text(&self) -> String {
        self.text_of(0..self.glyphs.len())
    }

    /// Case-insensitive search. Returns glyph ranges of every match (words may span line breaks,
    /// which count as a single space).
    pub fn find(&self, needle: &str) -> Vec<std::ops::Range<usize>> {
        self.find_opts(needle, false, false)
    }

    /// Search with Acrobat's find options: case-sensitive, whole words only.
    pub fn find_opts(&self, needle: &str, case_sensitive: bool, whole_words: bool) -> Vec<std::ops::Range<usize>> {
        self.find_opts_limited(needle, case_sensitive, whole_words, usize::MAX)
    }

    /// Search with Acrobat's find options, returning at most `limit` glyph ranges.
    pub fn find_opts_limited(&self, needle: &str, case_sensitive: bool, whole_words: bool, limit: usize) -> Vec<std::ops::Range<usize>> {
        search::find(self, needle, case_sensitive, whole_words, limit)
    }

    /// Glyph ranges of the matches a character matcher finds in the page text (words and lines
    /// separated by one space, original case). Used for pattern search (Search & Redact).
    pub fn find_with(&self, matcher: impl Fn(&[char]) -> Vec<std::ops::Range<usize>>) -> Vec<std::ops::Range<usize>> {
        let mut chars: Vec<char> = Vec::new();
        // `None` is a word or line separator inserted between glyphs. It is not part of either
        // glyph, so a match that starts or ends on it must not take the neighbouring glyph.
        let mut owner: Vec<Option<usize>> = Vec::new();
        for (i, g) in self.glyphs.iter().enumerate() {
            let gap = self.space_before.get(i).copied().unwrap_or(false) || self.line_of.get(i) != self.line_of.get(i.saturating_sub(1));
            if i > 0 && gap && chars.last().is_some_and(|c| *c != ' ') {
                chars.push(' ');
                owner.push(None);
            }
            for c in g.text.chars() {
                chars.push(if c.is_whitespace() { ' ' } else { c });
                owner.push(Some(i));
            }
        }
        matcher(&chars)
            .into_iter()
            .filter_map(|r| {
                if r.start >= r.end || r.end > chars.len() {
                    return None;
                }
                let glyphs = owner.get(r.start..r.end)?;
                let start = glyphs.iter().find_map(|g| *g)?;
                let end = glyphs.iter().rev().find_map(|g| *g)?.checked_add(1)?;
                (start < end).then_some(start..end)
            })
            .collect()
    }

    /// Index of the glyph nearest to a view-space point (for selection anchors).
    ///
    /// A glyph whose drawn em box (`quad`) holds the point wins. Skewed or turned text has bounding
    /// boxes wider than the glyphs, so the box of the letter before overlaps this one and would
    /// otherwise be found first (#3693). Failing that, the nearest bounding box wins, and a tie
    /// goes to the glyph whose centre is closer.
    pub fn nearest(&self, x: f32, y: f32) -> Option<usize> {
        let centre_distance = |g: &TextGlyph| {
            let (cx, cy) = (g.quad.iter().map(|q| q[0]).sum::<f32>() / 4.0, g.quad.iter().map(|q| q[1]).sum::<f32>() / 4.0);
            (cx - x).powi(2) + (cy - y).powi(2)
        };
        let inside = self
            .glyphs
            .iter()
            .enumerate()
            .filter(|(_, g)| quad_contains(&g.quad, x, y))
            .min_by(|a, b| centre_distance(a.1).total_cmp(&centre_distance(b.1)))
            .map(|(i, _)| i);
        if inside.is_some() {
            return inside;
        }
        self.glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let dx = if x < g.rect[0] {
                    g.rect[0] - x
                } else if x > g.rect[2] {
                    x - g.rect[2]
                } else {
                    0.0
                };
                let dy = if y < g.rect[1] {
                    g.rect[1] - y
                } else if y > g.rect[3] {
                    y - g.rect[3]
                } else {
                    0.0
                };
                // Prefer the same line: vertical distance weighs more.
                (i, dx + dy * 3.0, centre_distance(g))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
            .map(|(i, _, _)| i)
    }

    /// The word around glyph `i` as its first and last glyph (double-click): it stops at word
    /// gaps, blank glyphs and line ends.
    pub fn word_at(&self, i: usize) -> Option<(usize, usize)> {
        self.glyphs.get(i)?;
        let blank = |k: usize| self.glyphs.get(k).is_none_or(|g| g.text.trim().is_empty());
        // Glyph `k` (≥ 1) follows glyph `k - 1` with no word gap or line break between them.
        let joined = |k: usize| !self.space_before.get(k).copied().unwrap_or(true) && self.line_of.get(k - 1) == self.line_of.get(k);
        let mut first = i;
        while first > 0 && joined(first) && !blank(first - 1) {
            first -= 1;
        }
        let mut last = i;
        while joined(last + 1) && !blank(last + 1) {
            last += 1;
        }
        Some((first, last))
    }

    /// The line around glyph `i` as its first and last glyph (triple-click). A line's glyphs are
    /// consecutive.
    pub fn line_at(&self, i: usize) -> Option<(usize, usize)> {
        self.glyphs.get(i)?;
        let line = self.line_of.get(i)?;
        let same = |k: usize| self.line_of.get(k) == Some(line);
        let mut first = i;
        while first > 0 && same(first - 1) {
            first -= 1;
        }
        let mut last = i;
        while same(last + 1) {
            last += 1;
        }
        Some((first, last))
    }

    /// Merge the boxes of `range` into one rectangle per line (for highlighting).
    pub fn line_rects(&self, range: std::ops::Range<usize>) -> Vec<[f32; 4]> {
        self.merged_rects(range)
    }

    /// One rectangle per run of consecutive glyphs on one line, for glyph indices in ascending
    /// order (a reading-order range, or [`Self::glyphs_in`]). A run stops at a skipped glyph, so
    /// the rectangle never covers text that is not selected.
    pub fn glyph_rects(&self, glyphs: &[usize]) -> Vec<[f32; 4]> {
        self.merged_rects(glyphs.iter().copied())
    }

    fn merged_rects(&self, glyphs: impl IntoIterator<Item = usize>) -> Vec<[f32; 4]> {
        let mut out: Vec<(u32, usize, [f32; 4])> = Vec::new();
        for i in glyphs {
            let (Some(g), Some(&line)) = (self.glyphs.get(i), self.line_of.get(i)) else { break };
            match out.last_mut() {
                Some((l, last, r)) if *l == line && last.checked_add(1) == Some(i) => {
                    *last = i;
                    union(r, &g.rect);
                }
                _ => out.push((line, i, g.rect)),
            }
        }
        out.into_iter().map(|(_, _, r)| r).collect()
    }

    /// The glyphs whose centre lies inside `rect` (view space, corners in either order), in
    /// reading order: what a column selection covers (Alt/Option-drag, Acrobat's column select).
    /// A glyph only partly inside counts when its centre is, so a rectangle drawn a little into
    /// the next column does not pick up the edge of its text.
    pub fn glyphs_in(&self, rect: [f32; 4]) -> Vec<usize> {
        let (x0, x1) = (rect[0].min(rect[2]), rect[0].max(rect[2]));
        let (y0, y1) = (rect[1].min(rect[3]), rect[1].max(rect[3]));
        let inside = |g: &TextGlyph| {
            let (cx, cy) = ((g.rect[0] + g.rect[2]) / 2.0, (g.rect[1] + g.rect[3]) / 2.0);
            (x0..=x1).contains(&cx) && (y0..=y1).contains(&cy)
        };
        self.glyphs.iter().enumerate().filter(|(_, g)| inside(g)).map(|(i, _)| i).collect()
    }

    /// The text of a column selection ([`Self::glyphs_in`]) as it looks on the page: one row per
    /// visual line, top to bottom. Pieces of different lines that sit side by side (table cells,
    /// newspaper columns) share a row and are separated by a tab, so a pasted table lands in
    /// spreadsheet cells. Rows of right-to-left text put their pieces right to left. When any
    /// selected glyph does not run horizontally (CJK vertical writing, turned text, a page shown
    /// turned by `/Rotate`) rows mean nothing, and the lines come out one per row in reading order
    /// instead.
    pub fn column_text(&self, glyphs: &[usize]) -> String {
        // The selected glyphs of each line, with their box: (glyphs, bbox).
        let mut lines: Vec<(Vec<usize>, [f32; 4])> = Vec::new();
        let mut line_ids: Vec<u32> = Vec::new();
        for &i in glyphs {
            let (Some(g), Some(&line)) = (self.glyphs.get(i), self.line_of.get(i)) else { continue };
            match (lines.last_mut(), line_ids.last()) {
                (Some((members, bb)), Some(&l)) if l == line => {
                    members.push(i);
                    union(bb, &g.rect);
                }
                _ => {
                    lines.push((vec![i], g.rect));
                    line_ids.push(line);
                }
            }
        }
        // Within the layout's own tolerance for reading a glyph as unturned.
        let vertical = glyphs.iter().any(|&i| self.glyphs.get(i).is_some_and(|g| g.direction[1].abs() > WORD_GAP));
        let piece = |members: &[usize]| {
            let mut s = String::new();
            let mut prev: Option<usize> = None;
            for &i in members {
                let Some(g) = self.glyphs.get(i) else { continue };
                let gap = self.space_before.get(i).copied().unwrap_or(false) || prev.and_then(|p| p.checked_add(1)) != Some(i);
                if prev.is_some() && gap {
                    s.push(' ');
                }
                s.push_str(&g.text);
                prev = Some(i);
            }
            s
        };
        if vertical {
            return lines.iter().map(|(members, _)| piece(members)).collect::<Vec<_>>().join("\n");
        }
        // Rows: a line joins the row whose first line its vertical centre falls within.
        let mut order: Vec<usize> = (0..lines.len()).collect();
        order.sort_by(|a, b| {
            let cy = |k: usize| lines.get(k).map_or(0.0, |(_, bb)| (bb[1] + bb[3]) / 2.0);
            cy(*a).total_cmp(&cy(*b))
        });
        let mut rows: Vec<(f32, f32, Vec<usize>)> = Vec::new();
        for k in order {
            let Some((_, bb)) = lines.get(k) else { continue };
            let cy = (bb[1] + bb[3]) / 2.0;
            match rows.last_mut() {
                Some((top, bottom, members)) if cy >= *top && cy <= *bottom => members.push(k),
                _ => rows.push((bb[1], bb[3], vec![k])),
            }
        }
        let rtl = |members: &[usize]| members.iter().any(|&i| self.glyphs.get(i).is_some_and(|g| g.text.chars().any(is_rtl)));
        let mut out = Vec::with_capacity(rows.len());
        for (_, _, mut members) in rows {
            let right_to_left = members.iter().all(|&k| lines.get(k).is_some_and(|(m, _)| rtl(m)));
            members.sort_by(|a, b| {
                let x = |k: usize| lines.get(k).map_or(0.0, |(_, bb)| bb[0]);
                if right_to_left { x(*b).total_cmp(&x(*a)) } else { x(*a).total_cmp(&x(*b)) }
            });
            let cells: Vec<String> = members.iter().filter_map(|&k| lines.get(k)).map(|(m, _)| piece(m)).collect();
            out.push(cells.join("\t"));
        }
        out.join("\n")
    }
}

/// Largest sine of the angle between a glyph's writing direction and a flow's for the glyph to be
/// laid out in that flow's frame. Turning an em box by θ moves its edges along the line by up to
/// h·sin θ, so below the layout's word-gap tolerance (`WORD_GAP` · h) the boxes still read as they
/// would unturned.
const DIRECTION_TOLERANCE: f64 = WORD_GAP as f64;

/// A word gap is wider than the line's typical letter gap by this fraction of the line height.
const WORD_GAP: f32 = 0.15;

/// A glyph with its writing direction to the nearest quarter turn (right, down, left, up) and as
/// a unit vector.
type Drawn = (usize, [f64; 2], TextGlyph);

struct TextDevice {
    /// Every glyph in content order.
    glyphs: Vec<Drawn>,
    /// How many marked content sequences are open.
    depth: usize,
    /// The outermost open `/ActualText` replacement: its depth, text and the glyphs drawn inside
    /// it (with no text of their own yet).
    actual: Option<(usize, String, Vec<Drawn>)>,
    /// Every `/ActualText` replacement made on the page.
    replaced: Vec<Replaced>,
    filter: TextFilter,
    excluded_depth: Option<usize>,
    actual_filtered: bool,
    partial_replacement: bool,
}

/// An `/ActualText` replacement: the glyph standing for it in `TextDevice::glyphs` (none for an
/// empty replacement), its text, and the text and box of each replaced glyph that has Unicode of
/// its own.
struct Replaced {
    at: Option<usize>,
    text: String,
    originals: Vec<(String, [f32; 4])>,
}

/// Grid cell size, in points, for finding the glyphs drawn at a place.
const CELL: f32 = 8.0;

/// Most grid cells searched around one place: a glyph so tall its tolerance spans more is skipped.
const MAX_CELLS: i64 = 4096;

/// Most work (grid cells visited plus glyphs compared) the redraw dedupe in `TextDevice::finish`
/// may do on one page. A hostile page stacking ~10^5 glyphs and ~10^5 replacements at one spot
/// would otherwise make ~10^10 comparisons; past the budget the remaining replacements keep
/// their unmarked redraws (the text reads twice, as it would without `/ActualText`).
const MAX_REDRAW_WORK: usize = 1 << 22;

/// Most replacement text bytes the redraw dedupe in `TextDevice::finish` copies on one page.
const MAX_REDRAW_TEXT: usize = 1 << 22;

#[cfg(test)]
thread_local! {
    /// Replacement text bytes given to redraws on this thread.
    static REDRAW_TEXT_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn cell(v: f32) -> i64 {
    // Glyph boxes are finite (`draw_glyph` and `merge` drop the rest); `as` saturates.
    (v / CELL).floor() as i64
}

/// How far a redrawn glyph may sit from the one it redraws: a fifth of its height, as in `redraws`.
fn redraw_tolerance(rect: &[f32; 4]) -> f32 {
    0.2 * (rect[3] - rect[1]).max(0.1)
}

/// One glyph carrying `text`, boxed around `drawn` in their writing direction; `None` when they
/// span several lines or writing directions (one box would break selection and line grouping).
fn merge(drawn: &[Drawn], text: String) -> Option<Drawn> {
    let first = drawn.first()?;
    let (quarter, u) = (first.0, first.1);
    // Across the line: from the writing direction a quarter turn towards the line's bottom.
    let n = [-u[1], u[0]];
    let along = |p: &[f32; 2]| f64::from(p[0]) * u[0] + f64::from(p[1]) * u[1];
    let across = |p: &[f32; 2]| f64::from(p[0]) * n[0] + f64::from(p[1]) * n[1];
    let height = across(&first.2.quad[0]) - across(&first.2.quad[3]);
    let (mut s0, mut s1, mut t0, mut t1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for p in drawn.iter().flat_map(|d| d.2.quad.iter()) {
        (s0, s1) = (s0.min(along(p)), s1.max(along(p)));
        (t0, t1) = (t0.min(across(p)), t1.max(across(p)));
    }
    let one_line = drawn.iter().all(|d| compatible(u, d.1)) && t1 - t0 <= 2.0 * height.abs();
    if !(one_line && s0.is_finite() && s1.is_finite() && t0.is_finite() && t1.is_finite()) {
        return None;
    }
    // The edge towards the next line is the one the first glyph's quad starts on.
    let (near, far) = if height >= 0.0 { (t1, t0) } else { (t0, t1) };
    let point = |s: f64, t: f64| [(s * u[0] + t * n[0]) as f32, (s * u[1] + t * n[1]) as f32];
    let quad = [point(s0, near), point(s1, near), point(s1, far), point(s0, far)];
    let rect = quad.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |r, p| {
        [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]
    });
    Some((quarter, u, TextGlyph { text, rect, quad, direction: first.2.direction }))
}

impl TextDevice {
    fn new() -> Self {
        TextDevice {
            glyphs: Vec::new(),
            depth: 0,
            actual: None,
            replaced: Vec::new(),
            filter: TextFilter::default(),
            excluded_depth: None,
            actual_filtered: false,
            partial_replacement: false,
        }
    }

    /// Ends the open `/ActualText` replacement (ISO 32000-2 §14.9.4): its glyphs become one glyph
    /// carrying the replacement text, boxed around them all. Chrome (Skia) writes one around
    /// each Thai, Indic or Arabic cluster whose shaped glyphs (a tone mark moved clear of a vowel,
    /// a ligature) have no Unicode of their own, so without it those marks are lost and the
    /// words can't be found.
    ///
    /// A replacement spanning several lines or writing directions stays per glyph (each glyph's own
    /// Unicode). An empty one (a soft hyphen at a line break, say) stands for no text at all.
    fn end_actual_text(&mut self) {
        let Some((_, text, drawn)) = self.actual.take() else { return };
        if std::mem::take(&mut self.actual_filtered) {
            // ActualText has no mapping from replacement words to individual glyphs. A mixed
            // span cannot be filtered honestly; a wholly excluded span can simply be omitted.
            self.partial_replacement |= !drawn.is_empty();
            return;
        }
        let Some(glyph) = merge(&drawn, text.clone()) else {
            self.glyphs.extend(drawn.into_iter().filter(|d| !d.2.text.chars().all(char::is_control)));
            return;
        };
        let originals = drawn.iter().filter(|d| !d.2.text.chars().all(char::is_control)).map(|d| (d.2.text.clone(), d.2.rect)).collect();
        let at = if text.chars().all(char::is_control) {
            None
        } else {
            self.glyphs.push(glyph);
            Some(self.glyphs.len() - 1)
        };
        self.replaced.push(Replaced { at, text, originals });
    }

    /// Ends the page. Some producers draw text twice, the copy for an outline, shadow or
    /// glow, and mark only one drawing with `/ActualText` (Canva). The unmarked redraw of a
    /// replacement's glyphs (the same text at the same places, in content order) takes the same
    /// replacement, so the two read alike and line layout drops the copy as a redraw.
    fn finish(&mut self) {
        self.end_actual_text();
        if self.replaced.is_empty() {
            return;
        }
        let mut taken = vec![false; self.glyphs.len()];
        for at in self.replaced.iter().filter_map(|r| r.at) {
            if let Some(t) = taken.get_mut(at) {
                *t = true;
            }
        }
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, g) in self.glyphs.iter().enumerate() {
            if !taken.get(i).copied().unwrap_or(true) {
                grid.entry((cell(g.2.rect[0]), cell(g.2.rect[1]))).or_default().push(i);
            }
        }
        // Redraws found: the glyph range and the replacement standing for it.
        let mut redraws: Vec<(std::ops::Range<usize>, usize)> = Vec::new();
        let mut budget = MAX_REDRAW_WORK;
        'replacements: for (k, r) in self.replaced.iter().enumerate() {
            let Some((_, rect)) = r.originals.first() else { continue };
            let tol = redraw_tolerance(rect);
            let (x0, x1, y0, y1) = (cell(rect[0] - tol), cell(rect[0] + tol), cell(rect[1] - tol), cell(rect[1] + tol));
            if x1.saturating_sub(x0).saturating_add(1).saturating_mul(y1.saturating_sub(y0).saturating_add(1)) > MAX_CELLS {
                continue;
            }
            // Checking one candidate walks its run twice (free, then same).
            let candidate_cost = r.originals.len().saturating_mul(2).saturating_add(1);
            for key in (x0..=x1).flat_map(|x| (y0..=y1).map(move |y| (x, y))) {
                let Some(rest) = budget.checked_sub(1) else { break 'replacements };
                budget = rest;
                for &i in grid.get(&key).map_or(&[][..], Vec::as_slice) {
                    let Some(rest) = budget.checked_sub(candidate_cost) else { break 'replacements };
                    budget = rest;
                    let Some(range) = i.checked_add(r.originals.len()).map(|end| i..end) else { continue };
                    let Some(run) = self.glyphs.get(range.clone()) else { continue };
                    let free = taken.get(range.clone()).is_some_and(|t| t.iter().all(|t| !t));
                    let same = run.iter().zip(&r.originals).all(|(g, (text, o))| {
                        let tol = redraw_tolerance(o);
                        g.2.text == *text && (g.2.rect[0] - o[0]).abs() < tol && (g.2.rect[1] - o[1]).abs() < tol
                    });
                    if free && same {
                        if let Some(t) = taken.get_mut(range.clone()) {
                            t.fill(true);
                        }
                        redraws.push((range, k));
                    }
                }
            }
        }
        if redraws.is_empty() {
            return;
        }
        redraws.sort_by_key(|(range, _)| range.start);
        let glyphs = std::mem::take(&mut self.glyphs);
        let mut redraws = redraws.into_iter().peekable();
        let mut text_left = MAX_REDRAW_TEXT;
        let mut i = 0;
        while let Some(g) = glyphs.get(i) {
            let Some((range, k)) = redraws.next_if(|(range, _)| range.start == i) else {
                self.glyphs.push(g.clone());
                i += 1;
                continue;
            };
            let run = glyphs.get(range.clone()).unwrap_or_default();
            let replacement = self.replaced.get(k).map_or("", |r| r.text.as_str());
            // Each redraw takes its own copy of the replacement, and one long replacement can
            // have many redraws: past the budget a redraw keeps its glyphs, as past the work budget.
            let Some(left) = text_left.checked_sub(replacement.len()) else {
                self.glyphs.extend_from_slice(run);
                i = range.end.max(i + 1);
                continue;
            };
            text_left = left;
            let text = replacement.to_owned();
            #[cfg(test)]
            REDRAW_TEXT_BYTES.with(|c| c.set(c.get() + text.len()));
            if !text.chars().all(char::is_control) {
                match merge(run, text) {
                    Some(glyph) => self.glyphs.push(glyph),
                    None => self.glyphs.extend_from_slice(run),
                }
            }
            i = range.end.max(i + 1);
        }
    }
}

/// Whether `PDFCRAFT_TEXT_DEBUG` is set. `draw_glyph` runs once per glyph and `getenv` scans the
/// whole environment under a lock, so read it once.
fn text_debug() -> bool {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var_os("PDFCRAFT_TEXT_DEBUG").is_some())
}

impl<'a> Device<'a> for TextDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, _: &Paint<'a>, mode: &GlyphDrawMode) {
        if self.excluded_depth.is_some() || (self.filter.images && matches!(mode, GlyphDrawMode::Invisible)) {
            self.actual_filtered |= self.actual.is_some();
            return;
        }
        // Inside an `/ActualText` replacement only the glyph's box matters.
        let replaced = self.actual.is_some();
        let text = match glyph.as_unicode() {
            Some(hayro::hayro_interpret::hayro_cmap::BfString::Char(c)) => c.to_string(),
            Some(hayro::hayro_interpret::hayro_cmap::BfString::String(s)) => s,
            None if replaced => String::new(),
            None => {
                if text_debug() {
                    eprintln!(
                        "NOUNICODE {} {:?}",
                        if matches!(glyph, Glyph::Type3(_)) { "t3" } else { "ol" },
                        (transform * glyph_transform).as_coeffs()
                    );
                }
                return;
            }
        };
        if !replaced && text.chars().all(char::is_control) {
            return;
        }
        // Glyph space → view space. Glyph space uses 1000 units per em for outline glyphs; the em
        // box spans descender (−200) to ascender (800), the advance gives the width.
        let t = transform * glyph_transform;
        if text_debug() {
            let kind = if matches!(glyph, Glyph::Type3(_)) { "t3" } else { "ol" };
            eprintln!("{kind} {text:?} transform={:?} glyph_transform={:?}", transform.as_coeffs(), glyph_transform.as_coeffs());
        }
        let advance = match glyph {
            Glyph::Outline(g) => {
                g.advance_width().filter(|a| *a > 0.0).map(f64::from).unwrap_or_else(|| g.outline().bounding_box().width().max(500.0))
            }
            Glyph::Type3(g) => g.advance_width().filter(|a| a.is_finite() && *a > 0.0).map(f64::from).unwrap_or(600.0),
        };
        // The baseline direction in view space (y down).
        let coefficients = t.as_coeffs();
        let (dx, dy) = if matches!(glyph, Glyph::Outline(g) if g.is_vertical()) {
            // PDF vertical fonts advance along negative glyph-space y (WMode 1), while
            // keeping the glyph outline upright. Its x axis is not the writing direction.
            (-coefficients[2], -coefficients[3])
        } else {
            (coefficients[0], coefficients[1])
        };
        let dir = if dx.abs() >= dy.abs() {
            if dx >= 0.0 { 0 } else { 2 }
        } else if dy > 0.0 {
            1
        } else {
            3
        };
        let em = Rect::new(0.0, -200.0, advance, 800.0);
        let corners = if matches!(glyph, Glyph::Outline(g) if g.is_vertical()) {
            [(em.x0, em.y1), (em.x0, em.y0), (em.x1, em.y0), (em.x1, em.y1)]
        } else {
            [(em.x0, em.y0), (em.x1, em.y0), (em.x1, em.y1), (em.x0, em.y1)]
        }
        .map(|(x, y)| t * kurbo::Point::new(x, y));
        let b = corners.iter().fold(Rect::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |b, p| {
            Rect::new(b.x0.min(p.x), b.y0.min(p.y), b.x1.max(p.x), b.y1.max(p.y))
        });
        if !(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite()) || b.width() > 10_000.0 || b.height() > 10_000.0 {
            return;
        }
        let length = dx.hypot(dy);
        let direction = if length.is_finite() && length > 0.0 { [dx / length, dy / length] } else { [1.0, 0.0] };
        // A glyph whose Unicode is several characters (a ligature such as "ffi") stays one glyph
        // with its own box: the PDF gives no position for the characters inside it.
        let glyph = TextGlyph {
            text,
            rect: [b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32],
            quad: corners.map(|p| [p.x as f32, p.y as f32]),
            direction: direction.map(|d| d as f32),
        };
        match &mut self.actual {
            Some((_, _, drawn)) => drawn.push((dir, direction, glyph)),
            None => self.glyphs.push((dir, direction, glyph)),
        }
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn begin_marked_content(&mut self, tag: &[u8], mcid: Option<i32>) {
        self.depth = self.depth.saturating_add(1);
        if self.excluded_depth.is_none() && self.filter.excludes(tag, mcid) {
            self.excluded_depth = Some(self.depth);
        }
    }
    fn actual_text(&mut self, text: &[u8]) {
        // A replacement nested in another is already covered by the outer one.
        if self.actual.is_none() && self.excluded_depth.is_none() {
            self.actual = Some((self.depth, pdfcraft_cos::PdfString::literal(text).to_text(), Vec::new()));
        }
    }
    fn end_marked_content(&mut self) {
        if self.actual.as_ref().is_some_and(|(depth, _, _)| *depth == self.depth) {
            self.end_actual_text();
        }
        if self.excluded_depth == Some(self.depth) {
            self.excluded_depth = None;
        }
        self.depth = self.depth.saturating_sub(1);
    }
}

/// Extract the text of one page. Panics inside the interpreter are the caller's to catch.
pub(crate) fn extract_page(pdf: &Pdf, page: usize, settings: &InterpreterSettings) -> Option<PageText> {
    extract_page_filtered(pdf, page, settings, TextFilter::default()).ok()
}

pub(crate) fn extract_page_filtered(pdf: &Pdf, page: usize, settings: &InterpreterSettings, filter: TextFilter) -> Result<PageText, String> {
    let pages = pdf.pages();
    let p = pages.get(page).ok_or("the page could not be read")?;
    let (w, h) = p.render_dimensions();
    let cache = InterpreterCache::new();
    // Annotation appearances are included: form-field values and comment text are searchable,
    // as in Acrobat, and match what other extractors report.
    let settings = settings.clone();
    let initial = p.initial_transform(true).to_kurbo();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, w as f64, h as f64), &cache, p.xref(), settings);
    let mut dev = TextDevice::new();
    dev.filter = filter;
    interpret_page(p, &mut ctx, &mut dev);
    // A replacement the content never closed ends with the page.
    dev.finish();
    if dev.partial_replacement {
        return Err("a replacement text span mixes included and excluded content; disable the exclusions to count this page".into());
    }
    let glyphs = dev.glyphs;
    // Text on a curved path: glyphs that each continue the previous one's line while the
    // direction turns. Each such run is one line.
    let turned = |u: [f64; 2]| u[0].abs().min(u[1].abs()) > DIRECTION_TOLERANCE;
    let mut in_curve = vec![false; glyphs.len()];
    // Turned text: curved runs, and straight flows along one writing direction. The two are kept
    // apart so finding a glyph's direction scans only the directions (a few dozen at most: each
    // differs from the others by more than the tolerance), not every curved run.
    let mut curves: Vec<Vec<usize>> = Vec::new();
    let mut straight: Vec<([f64; 2], Vec<usize>)> = Vec::new();
    let mut start = 0;
    for end in 1..=glyphs.len() {
        if end < glyphs.len() && continues(&glyphs[end - 1], &glyphs[end]) {
            continue;
        }
        let run = &glyphs[start..end];
        if run.iter().any(|g| turned(g.1)) && run.iter().any(|g| !compatible(run[0].1, g.1)) {
            in_curve[start..end].fill(true);
            curves.push((start..end).collect());
        }
        start = end;
    }
    let mut quarters: [Vec<usize>; 4] = Default::default();
    for (i, (quarter, u, _)) in glyphs.iter().enumerate() {
        if in_curve[i] {
            continue;
        }
        if !turned(*u) {
            if let Some(q) = quarters.get_mut(*quarter) {
                q.push(i);
            }
        } else if let Some((_, ids)) = straight.iter_mut().find(|(d, _)| compatible(*d, *u)) {
            ids.push(i);
        } else {
            straight.push((*u, vec![i]));
        }
    }
    // A horizontal heading must not force a vertical body back into horizontal line grouping.
    // Group each writing direction independently, then join flows from their topmost position.
    let upright = |quarters: [Vec<usize>; 4]| {
        let mut flows: Vec<Flow> = quarters
            .into_iter()
            .enumerate()
            .filter(|(_, ids)| !ids.is_empty())
            .map(|(quarter, ids)| {
                let rects = ids.iter().map(|&i| to_upright(glyphs[i].2.rect, quarter as u8, w, h)).collect();
                arrange_flow(&glyphs, ids, rects, Some(quarter as u8))
            })
            .collect();
        flows.sort_by(|a, b| a.top.total_cmp(&b.top));
        flows
    };
    let flows = upright(quarters);
    let mut extra: Vec<Flow> = curves
        .into_iter()
        .map(|ids| (None, ids))
        .chain(straight.into_iter().map(|(u, ids)| (Some(u), ids)))
        .map(|(u, ids)| {
            let rects = match u {
                // Turned text is laid out along its own baseline.
                Some(u) => ids.iter().map(|&i| along_baseline(&glyphs[i].2.quad, u).map(|c| c as f32)).collect(),
                None => unrolled(ids.iter().map(|&i| &glyphs[i])),
            };
            arrange_flow(&glyphs, ids, rects, None)
        })
        .collect();
    // Turned text takes the place in the reading order where laying it out among the nearest
    // quarter turn's text puts its first glyph.
    let mut rank = Vec::new();
    if !extra.is_empty() {
        rank = vec![usize::MAX; glyphs.len()];
        let mut nearest: [Vec<usize>; 4] = Default::default();
        for (i, g) in glyphs.iter().enumerate() {
            if let Some(q) = nearest.get_mut(g.0) {
                q.push(i);
            }
        }
        for (r, i) in upright(nearest).into_iter().flat_map(|f| f.ids).enumerate() {
            rank[i] = r;
        }
    }
    let first = |ids: &[usize]| ids.iter().filter_map(|&i| rank.get(i).copied()).min().unwrap_or(usize::MAX);
    extra.sort_by_key(|f| first(&f.ids));
    let mut extra = extra.into_iter().peekable();
    let mut slots: Vec<Option<TextGlyph>> = glyphs.into_iter().map(|g| Some(g.2)).collect();
    let mut text = PageText::default();
    let mut emit = |text: &mut PageText, ids: &[usize], line_of: &[u32], space_before: &[bool], quarter: Option<u8>| {
        let offset = text.line_of.last().map_or(0, |line| line.saturating_add(1));
        for ((&i, &line), &space) in ids.iter().zip(line_of).zip(space_before) {
            let Some(mut g) = slots.get_mut(i).and_then(Option::take) else { continue };
            if let Some(quarter) = quarter {
                g.rect = from_upright(to_upright(g.rect, quarter, w, h), quarter, w, h);
            }
            text.glyphs.push(g);
            text.line_of.push(line.saturating_add(offset));
            text.space_before.push(space);
        }
    };
    for flow in flows {
        let mut k = 0;
        while k < flow.ids.len() {
            let line = flow.line_of[k];
            let end = flow.line_of[k..].iter().position(|l| *l != line).map_or(flow.ids.len(), |n| k + n);
            let before = first(&flow.ids[k..end]);
            while let Some(f) = extra.next_if(|f| first(&f.ids) < before) {
                emit(&mut text, &f.ids, &f.line_of, &f.space_before, None);
            }
            emit(&mut text, &flow.ids[k..end], &vec![0; end - k], &flow.space_before[k..end], flow.quarter);
            k = end;
        }
    }
    for f in extra {
        emit(&mut text, &f.ids, &f.line_of, &f.space_before, None);
    }
    Ok(text)
}

/// Glyphs in reading order, as indices into the page's glyphs, with their lines and word gaps.
struct Flow {
    /// The top of the flow's topmost glyph in view space.
    top: f32,
    /// The quarter turn whose frame the flow was laid out in.
    quarter: Option<u8>,
    ids: Vec<usize>,
    line_of: Vec<u32>,
    space_before: Vec<bool>,
}

/// Lay out the page's glyphs `ids`, giving each the box at the same position in `rects`.
fn arrange_flow(glyphs: &[Drawn], ids: Vec<usize>, rects: Vec<[f32; 4]>, quarter: Option<u8>) -> Flow {
    let top = ids.iter().filter_map(|&i| glyphs.get(i)).map(|g| g.2.rect[1]).fold(f32::INFINITY, f32::min);
    let framed: Vec<TextGlyph> = ids.iter().zip(rects).filter_map(|(&i, rect)| glyphs.get(i).map(|g| TextGlyph { rect, ..g.2.clone() })).collect();
    let (order, line_of, space_before) = arrange(&framed);
    let ids = order.into_iter().filter_map(|k| ids.get(k).copied()).collect();
    Flow { top, quarter, ids, line_of, space_before }
}

/// Boxes for a run of glyphs set along a curve: each glyph's box along its own direction, placed
/// where the pen moved in the previous glyph's direction. The path straightened out.
fn unrolled<'g>(run: impl Iterator<Item = &'g Drawn>) -> Vec<[f32; 4]> {
    let mut pen = [0.0, 0.0];
    let mut previous: Option<([f64; 2], [f64; 4])> = None;
    run.map(|(_, u, g)| {
        let b = along_baseline(&g.quad, *u);
        if let Some((pu, pb)) = previous {
            let at = along_baseline(&g.quad, pu);
            pen = [pen[0] + at[0] - pb[0], pen[1] + (at[1] + at[3] - pb[1] - pb[3]) / 2.0];
        }
        previous = Some((*u, b));
        let half = (b[3] - b[1]) / 2.0;
        [pen[0], pen[1] - half, pen[0] + b[2] - b[0], pen[1] + half].map(|c| c as f32)
    })
    .collect()
}

/// View-space box → the frame where text runs left to right, for text running `quarter` × 90°
/// clockwise from that in a `w` × `h` view (1: text runs downwards).
fn to_upright(r: [f32; 4], quarter: u8, w: f32, h: f32) -> [f32; 4] {
    let [x0, y0, x1, y1] = r;
    match quarter {
        1 => [y0, w - x1, y1, w - x0],
        2 => [w - x1, h - y1, w - x0, h - y0],
        3 => [h - y1, x0, h - y0, x1],
        _ => r,
    }
}

/// Whether two writing directions are close enough to share a frame ([`DIRECTION_TOLERANCE`]).
fn compatible(a: [f64; 2], b: [f64; 2]) -> bool {
    a[0] * b[0] + a[1] * b[1] > 0.0 && (a[0] * b[1] - a[1] * b[0]).abs() <= DIRECTION_TOLERANCE
}

/// Whether glyph `g` continues the line of glyph `p` (content order), by the test `layout` uses
/// for consecutive glyphs, taken in `p`'s own frame. The direction may turn by less than half a
/// quarter turn, as text set along a curve does.
fn continues(p: &Drawn, g: &Drawn) -> bool {
    let (pu, gu) = (p.1, g.1);
    if (pu[0] * gu[1] - pu[1] * gu[0]).abs() >= pu[0] * gu[0] + pu[1] * gu[1] {
        return false;
    }
    let (pb, gb, own) = (along_baseline(&p.2.quad, pu), along_baseline(&g.2.quad, pu), along_baseline(&g.2.quad, gu));
    let h = (pb[3] - pb[1]).min(own[3] - own[1]).max(0.1);
    let cy = |b: [f64; 4]| (b[1] + b[3]) / 2.0;
    (cy(gb) - cy(pb)).abs() < h * 0.5 && gb[0] - pb[2] < h * 3.0 && gb[0] > pb[0] - h * 2.0
}

/// A view-space quad → its box in the frame where `u` runs left to right and `u` turned a
/// quarter clockwise runs downwards. Along `u` it spans the middles of the quad's start and end
/// sides, so a skewed glyph is no wider than its advance; across, it spans every corner.
fn along_baseline(quad: &[[f32; 2]; 4], u: [f64; 2]) -> [f64; 4] {
    let p = quad.map(|[x, y]| [f64::from(x) * u[0] + f64::from(y) * u[1], f64::from(y) * u[0] - f64::from(x) * u[1]]);
    let (a0, a1) = ((p[0][0] + p[3][0]) / 2.0, (p[1][0] + p[2][0]) / 2.0);
    let (c0, c1) = p.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| (lo.min(q[1]), hi.max(q[1])));
    [a0.min(a1), c0, a0.max(a1), c1]
}

/// The inverse of [`to_upright`].
fn from_upright(r: [f32; 4], quarter: u8, w: f32, h: f32) -> [f32; 4] {
    let [u0, v0, u1, v1] = r;
    match quarter {
        1 => [w - v1, u0, w - v0, u1],
        2 => [w - u1, h - v1, w - u0, h - v0],
        3 => [v0, h - u1, v1, h - u0],
        _ => r,
    }
}

use hayro::hayro_interpret::TransformExt;

fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
}

/// Scripts written with explicit spaces whose combining signs can sit apart from their base
/// (Indic, Thai, Lao, Myanmar, Khmer): never infer a space inside a tight cluster.
fn is_complex(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x0900..=0x0DFF | 0x0E00..=0x0EFF | 0x1000..=0x109F | 0x1780..=0x17FF)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3134F | 0x1100..=0x11FF)
}

/// Glyphs `start..end` of the glyph list [`arrange`] lays out, drawn one after another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    start: usize,
    end: usize,
}

impl Run {
    fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    fn range(self) -> std::ops::Range<usize> {
        self.start..self.end
    }
}

/// Marks an absent link in a union-find or run list.
const NONE: usize = usize::MAX;

/// A segment: runs drawn on one baseline, placed together. Its glyph order is worked out when it is emitted, so a
/// segment holds run ranges and geometry only.
struct Seg {
    /// The first run. Merged segments keep their other runs in `extra`.
    first: Run,
    /// The other runs of a merged segment. Boxed, and absent for most segments.
    extra: Option<Box<SegExtra>>,
    bbox: [f32; 4],
    h: f32,
}

#[derive(Default)]
struct SegExtra {
    more: Vec<Run>,
}

impl Seg {
    fn new(first: Run, bbox: [f32; 4], h: f32) -> Self {
        Seg { first, extra: None, bbox, h }
    }

    fn cy(&self) -> f32 {
        (self.bbox[1] + self.bbox[3]) / 2.0
    }

    fn runs(&self) -> impl Iterator<Item = Run> + '_ {
        std::iter::once(self.first).chain(self.extra.iter().flat_map(|e| e.more.iter().copied()))
    }

    fn merge(&mut self, other: Seg) {
        union(&mut self.bbox, &other.bbox);
        self.h = self.h.max(other.h);
        let extra = self.extra.get_or_insert_with(Default::default);
        extra.more.push(other.first);
        if let Some(o) = other.extra {
            extra.more.extend(o.more);
        }
    }
}

/// Two glyphs of different runs that overlap by more than this fraction of the line height collide.
const RUN_COLLISION: f32 = 0.25;
/// A run that starts inside the runs before it by more than this fraction of the line height starts a new word.
const RUN_OVERLAP_SPACE: f32 = 0.5;
/// Earlier runs with the same key that a run is checked against for a redraw, newest first. A run past this limit is kept,
/// so a page costs at most this many checks per run, however many runs share a key.
const REDRAW_CANDIDATES: usize = 32;
/// Pair tests that the collision sweep may make per glyph of a segment. Ordinary text overlaps each glyph with about one
/// neighbour, so it stays far below this. A segment that needs more is dense, and [`collide`] joins its runs instead.
const COLLIDE_PAIRS_PER_GLYPH: usize = 16;
/// Glyph positions within a fifth of their height are considered the same for redraws.
const REDRAW_POSITION_TOLERANCE: f32 = 0.2;
/// A reset needs three matching glyphs at the start of the run, keeping local kerning from splitting it.
const REDRAW_PREFIX_GLYPHS: usize = 3;

/// Reusable buffers, so a page allocates them once rather than per segment. Glyph placement works in them, one
/// segment at a time, when the segment is emitted.
#[derive(Default)]
struct Scratch {
    /// The glyphs of the text runs, each with its run, in x order.
    owner: Vec<(usize, usize)>,
    /// The text runs of the segment (runs that are not all zero-width).
    text: Vec<Run>,
    /// The glyphs of the zero-width runs, in run order.
    marks: Vec<usize>,
    /// Union-find parents over `text`, and for each root the list of its runs through `next`.
    parent: Vec<usize>,
    head: Vec<usize>,
    next: Vec<usize>,
    /// The runs of one colliding component, by left edge.
    rows: Vec<Row>,
    /// The glyphs of one colliding component: each run's glyphs in x order, in run order.
    group: Vec<usize>,
    /// The units to place, and their glyphs in `flat`.
    units: Vec<Unit>,
    flat: Vec<usize>,
    /// The current segment's glyphs in visual reading order, and its forced word breaks.
    placed: Vec<usize>,
    breaks: Vec<usize>,
    /// The gaps between the current segment's glyphs, and its glyphs in logical order when it has right-to-left text.
    gaps: Vec<f32>,
    logical: Vec<usize>,
    /// `placed` with the marks merged in, built before it is swapped in.
    merged: Vec<usize>,
}

/// One run of a colliding component.
struct Row {
    left: f32,
    right: f32,
    /// The run's first glyph in content order, which is its smallest index.
    first: usize,
    /// The run's glyph with the smallest x, where a word break is recorded.
    lead: usize,
    /// The run's glyphs in `Scratch::group`.
    glyphs: std::ops::Range<usize>,
}

/// A part of a segment placed as one: a free glyph, or a colliding component.
struct Unit {
    left: f32,
    first: usize,
    /// The unit's glyphs in `Scratch::flat`.
    glyphs: std::ops::Range<usize>,
}

fn union_find(parent: &mut [usize], mut r: usize) -> usize {
    while parent[r] != r {
        parent[r] = parent[parent[r]];
        r = parent[r];
    }
    r
}

/// A 64-bit fingerprint of a run's glyph texts. Each text is hashed with its length, so ["ab", "c"] and
/// ["a", "bc"] differ. Equal fingerprints are only candidates: `redraws` decides.
fn fingerprint(glyphs: &[&TextGlyph], run: Run) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for g in run.range().filter_map(|i| glyphs.get(i)) {
        for b in (g.text.len() as u64).to_le_bytes().into_iter().chain(g.text.bytes()) {
            h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// Whether run `b` redraws run `a` glyph for glyph: the same text, each glyph within a fifth of its
/// height of the same glyph in `a`.
fn redraws(glyphs: &[&TextGlyph], a: Run, b: Run) -> bool {
    a.len() == b.len()
        && a.range().zip(b.range()).all(|(i, j)| match (glyphs.get(i), glyphs.get(j)) {
            (Some(p), Some(g)) => {
                let tol = REDRAW_POSITION_TOLERANCE * (g.rect[3] - g.rect[1]).max(0.1);
                p.text == g.text && (p.rect[0] - g.rect[0]).abs() < tol && (p.rect[1] - g.rect[1]).abs() < tol
            }
            _ => false,
        })
}

fn within_position_tolerance(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs().partial_cmp(&tolerance) == Some(std::cmp::Ordering::Less)
}

/// Whether `next` restarts `start` after the run has advanced by a glyph height. The fixed prefix
/// bounds this check and requires more than a single backward adjustment to split a run.
fn restarts_run(glyphs: &[&TextGlyph], start: usize, next: usize, progress: f32) -> bool {
    let (Some(first), Some(candidate)) = (glyphs.get(start), glyphs.get(next)) else { return false };
    let tol = REDRAW_POSITION_TOLERANCE * (candidate.rect[3] - candidate.rect[1]).max(0.1);
    if first.text != candidate.text
        || !within_position_tolerance(first.rect[0], candidate.rect[0], tol)
        || !within_position_tolerance(first.rect[1], candidate.rect[1], tol)
    {
        return false;
    }
    let h = (first.rect[3] - first.rect[1]).max(0.1);
    if !progress.is_finite() || progress <= h {
        return false;
    }
    for offset in 1..REDRAW_PREFIX_GLYPHS {
        let (Some(a), Some(b)) = (start.checked_add(offset).and_then(|i| glyphs.get(i)), next.checked_add(offset).and_then(|i| glyphs.get(i))) else {
            return false;
        };
        let tol = REDRAW_POSITION_TOLERANCE * (b.rect[3] - b.rect[1]).max(0.1);
        if a.text != b.text || !within_position_tolerance(a.rect[0], b.rect[0], tol) || !within_position_tolerance(a.rect[1], b.rect[1], tol) {
            return false;
        }
    }
    true
}

/// Puts a segment's glyphs in visual reading order into `sc.placed`, and its forced word breaks into `sc.breaks`.
///
/// Glyphs are placed by x. Runs that collide, meaning that glyphs of different runs overprint each other, stay whole
/// and are placed by their left edge. Runs that only interleave along the line, such as symbols between letters, keep
/// their glyph-by-glyph order, so the common case is one sort. A run of zero-width glyphs (a combining mark) has no
/// extent of its own, so it goes next to the glyph it starts at.
fn place_glyphs(glyphs: &[&TextGlyph], height: &dyn Fn(usize) -> f32, s: &Seg, sc: &mut Scratch) {
    let x0 = |i: usize| glyphs[i].rect[0];
    let zero_width = |i: usize| glyphs[i].rect[2] - glyphs[i].rect[0] <= 0.05 * height(i);
    sc.text.clear();
    sc.marks.clear();
    sc.placed.clear();
    sc.breaks.clear();
    for run in s.runs() {
        if run.range().all(&zero_width) {
            sc.marks.extend(run.range());
        } else {
            sc.text.push(run);
        }
    }
    if sc.text.len() <= 1 {
        // A single run cannot collide with itself: its glyphs only need sorting by x.
        sc.placed.extend(sc.text.iter().flat_map(|run| run.range()));
        sc.placed.sort_unstable_by(|a, b| x0(*a).total_cmp(&x0(*b)).then(a.cmp(b)));
    } else {
        collide(glyphs, height, s.h, sc);
    }
    // Each mark goes before the first placed glyph whose x is greater than its own. Marks are visited in x order, and
    // that position never moves back, so one pass over `placed` puts all of them in.
    sc.marks.sort_by(|a, b| x0(*a).total_cmp(&x0(*b)));
    if !sc.marks.is_empty() {
        sc.merged.clear();
        let mut next = 0;
        for &m in &sc.marks {
            while next < sc.placed.len() && x0(sc.placed[next]) <= x0(m) {
                sc.merged.push(sc.placed[next]);
                next += 1;
            }
            sc.merged.push(m);
        }
        sc.merged.extend_from_slice(&sc.placed[next..]);
        std::mem::swap(&mut sc.placed, &mut sc.merged);
    }
}

/// `place_glyphs` for a segment of two or more text runs. `h` is the segment's line height. Returns whether the segment
/// was too dense to sweep ([`COLLIDE_PAIRS_PER_GLYPH`]), in which case all its text runs form one component.
///
/// The sweep tests each glyph against the glyphs after it that start before its right edge. Its work is bounded by the
/// budget, so a page costs O(glyphs) pair tests plus the sorts. A dense segment is joined with one pass over its runs,
/// which keeps each run whole. It may keep runs whole that do not overprint, but never interleaves overprinted glyphs.
fn collide(glyphs: &[&TextGlyph], height: &dyn Fn(usize) -> f32, h: f32, sc: &mut Scratch) -> bool {
    let x0 = |i: usize| glyphs[i].rect[0];
    let runs = sc.text.len();
    sc.owner.clear();
    for (r, run) in sc.text.iter().enumerate() {
        sc.owner.extend(run.range().map(|i| (i, r)));
    }
    sc.owner.sort_unstable_by(|a, b| x0(a.0).total_cmp(&x0(b.0)).then(a.0.cmp(&b.0)));
    sc.parent.clear();
    sc.parent.extend(0..runs);
    let mut budget = COLLIDE_PAIRS_PER_GLYPH.saturating_mul(sc.owner.len());
    let mut collided = false;
    let mut dense = false;
    'sweep: for a in 0..sc.owner.len() {
        let (i, ri) = sc.owner[a];
        let (x1, hi) = (glyphs[i].rect[2], height(i));
        for b in a + 1..sc.owner.len() {
            let (j, rj) = sc.owner[b];
            if x0(j) >= x1 {
                break;
            }
            if budget == 0 {
                dense = true;
                break 'sweep;
            }
            budget -= 1;
            let (ra, rb) = (union_find(&mut sc.parent, ri), union_find(&mut sc.parent, rj));
            let overlap = x1.min(glyphs[j].rect[2]) - x0(j);
            if ra != rb && overlap > RUN_COLLISION * hi.min(height(j)) {
                sc.parent[ra] = rb;
                collided = true;
            }
        }
    }
    if dense {
        // Run 0 is its own root, so every run now has it as its root.
        sc.parent.iter_mut().for_each(|p| *p = 0);
        collided = true;
    }
    if !collided {
        sc.placed.extend(sc.owner.iter().map(|&(i, _)| i));
        return dense;
    }

    // Link each component's runs, so they can be walked without a map.
    sc.head.clear();
    sc.head.resize(runs, NONE);
    sc.next.clear();
    sc.next.resize(runs, NONE);
    for r in 0..runs {
        let root = union_find(&mut sc.parent, r);
        sc.next[r] = sc.head[root];
        sc.head[root] = r;
    }
    sc.units.clear();
    sc.flat.clear();
    for root in 0..runs {
        if union_find(&mut sc.parent, root) != root {
            continue;
        }
        let head = sc.head[root];
        if sc.next[head] == NONE {
            // A run that collides with nothing is placed glyph by glyph.
            for i in sc.text[head].range() {
                let at = sc.flat.len();
                sc.flat.push(i);
                sc.units.push(Unit { left: x0(i), first: i, glyphs: at..at + 1 });
            }
            continue;
        }
        sc.rows.clear();
        sc.group.clear();
        let mut m = head;
        while m != NONE {
            let run = sc.text[m];
            let start = sc.group.len();
            sc.group.extend(run.range());
            sc.group[start..].sort_unstable_by(|a, b| x0(*a).total_cmp(&x0(*b)).then(a.cmp(b)));
            let left = sc.group[start..].iter().map(|&i| x0(i)).fold(f32::INFINITY, f32::min);
            let right = sc.group[start..].iter().map(|&i| glyphs[i].rect[2]).fold(f32::NEG_INFINITY, f32::max);
            let lead = sc.group[start];
            sc.rows.push(Row { left, right, first: run.start, lead, glyphs: start..sc.group.len() });
            m = sc.next[m];
        }
        sc.rows.sort_unstable_by(|a, b| a.left.total_cmp(&b.left).then(a.first.cmp(&b.first)));
        let start = sc.flat.len();
        let mut reach = f32::NEG_INFINITY;
        for row in &sc.rows {
            if row.left < reach - h * RUN_OVERLAP_SPACE {
                sc.breaks.push(row.lead);
            }
            reach = reach.max(row.right);
            sc.flat.extend_from_slice(&sc.group[row.glyphs.clone()]);
        }
        let left = sc.rows.iter().map(|r| r.left).fold(f32::INFINITY, f32::min);
        let first = sc.rows.iter().map(|r| r.first).min().unwrap_or(0);
        sc.units.push(Unit { left, first, glyphs: start..sc.flat.len() });
    }
    sc.units.sort_unstable_by(|a, b| a.left.total_cmp(&b.left).then(a.first.cmp(&b.first)));
    for unit in &sc.units {
        sc.placed.extend_from_slice(&sc.flat[unit.glyphs.clone()]);
    }
    // Sorted, so that each glyph's membership is a binary search rather than a scan of the breaks.
    sc.breaks.sort_unstable();
    dense
}

/// Simplified bidi class of a glyph or word gap (UAX #9: strong L/R, numbers, neutrals).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bidi {
    L,
    R,
    Num,
    N,
}

fn bidi_class(text: &str) -> Bidi {
    if text.chars().any(|c| is_rtl(c) && !c.is_numeric()) {
        Bidi::R
    } else if text.chars().any(char::is_numeric) {
        Bidi::Num
    } else if text.chars().any(char::is_alphabetic) {
        Bidi::L
    } else {
        Bidi::N
    }
}

/// Reorders a line of `idx` from visual to logical order with a simplified bidi level pass (#2667), carrying the
/// word gaps along: `spaces[w]` is a gap left of `idx[w]` (#1062). The paragraph is right-to-left when it has
/// more right-to-left than left-to-right glyphs; numbers keep their left-to-right order, and neutrals
/// (punctuation, gaps) take the direction of their neighbours, or the paragraph's between opposite ones.
fn reverse_rtl(glyphs: &[&TextGlyph], idx: &mut [usize], spaces: &mut [bool]) {
    // Glyphs and word gaps (None) in visual order.
    let mut items: Vec<(Option<usize>, Bidi)> = Vec::with_capacity(idx.len() * 2);
    for (w, &i) in idx.iter().enumerate() {
        if w > 0 && spaces.get(w).copied().unwrap_or(false) {
            items.push((None, Bidi::N));
        }
        items.push((Some(i), bidi_class(&glyphs[i].text)));
    }
    let texts: Vec<&str> = items.iter().map(|it| it.0.map_or("", |i| glyphs[i].text.as_str())).collect();
    let text = |k: usize| texts.get(k).copied().unwrap_or("");
    let count = |b: Bidi| items.iter().filter(|it| it.1 == b).count();
    let para = if count(Bidi::R) > count(Bidi::L) { Bidi::R } else { Bidi::L };
    let n = items.len();
    // W4: a single separator between two digits belongs to the number.
    for k in 1..n.saturating_sub(1) {
        let sep = matches!(text(k), "." | "," | ":" | "/" | "+" | "-" | "\u{066B}" | "\u{066C}");
        if sep && items[k - 1].1 == Bidi::Num && items[k + 1].1 == Bidi::Num {
            items[k].1 = Bidi::Num;
        }
    }
    // W5: terminators (percent, currency) next to a number belong to it.
    let term =
        |s: &str| !s.is_empty() && s.chars().all(|c| matches!(c, '%' | '‰' | '#' | '$' | '¢'..='¥' | '°' | '\u{066A}' | '\u{20A0}'..='\u{20CF}'));
    for k in 1..n {
        if items[k].1 == Bidi::N && items[k - 1].1 == Bidi::Num && term(text(k)) {
            items[k].1 = Bidi::Num;
        }
    }
    for k in (0..n.saturating_sub(1)).rev() {
        if items[k].1 == Bidi::N && items[k + 1].1 == Bidi::Num && term(text(k)) {
            items[k].1 = Bidi::Num;
        }
    }
    // W7: a number after left-to-right text (or at the start of a left-to-right paragraph) is left-to-right.
    let mut strong = para;
    for it in &mut items {
        match it.1 {
            Bidi::L | Bidi::R => strong = it.1,
            Bidi::Num if strong == Bidi::L => it.1 = Bidi::L,
            _ => {}
        }
    }
    // N1/N2: neutrals between same-direction text take it (numbers count as right-to-left), else the paragraph's.
    let dir = |b: Bidi| if b == Bidi::L { Bidi::L } else { Bidi::R };
    let mut k = 0;
    while k < n {
        if items[k].1 != Bidi::N {
            k += 1;
            continue;
        }
        let mut e = k;
        while e < n && items[e].1 == Bidi::N {
            e += 1;
        }
        let before = if k == 0 { para } else { dir(items[k - 1].1) };
        let after = if e == n { para } else { dir(items[e].1) };
        let resolved = if before == after { before } else { para };
        for it in &mut items[k..e] {
            it.1 = resolved;
        }
        k = e;
    }
    // Levels (I1/I2), then L2: reverse every run at or above each level, highest first.
    let rtl_para = para == Bidi::R;
    let levels: Vec<u8> = items
        .iter()
        .map(|it| match it.1 {
            Bidi::R => 1,
            Bidi::Num => 2,
            _ if rtl_para => 2,
            _ => 0,
        })
        .collect();
    let mut order: Vec<usize> = (0..n).collect();
    for level in (1..=levels.iter().copied().max().unwrap_or(0)).rev() {
        let mut k = 0;
        while k < n {
            if levels[order[k]] < level {
                k += 1;
                continue;
            }
            let mut e = k;
            while e < n && levels[order[e]] >= level {
                e += 1;
            }
            order[k..e].reverse();
            k = e;
        }
    }
    let mut j = 0;
    let mut gap = false;
    for &o in &order {
        match items[o].0 {
            None => gap = true,
            Some(i) => {
                if let (Some(slot), Some(space)) = (idx.get_mut(j), spaces.get_mut(j)) {
                    *slot = i;
                    *space = gap && j > 0;
                }
                gap = false;
                j += 1;
            }
        }
    }
}

fn union(a: &mut [f32; 4], b: &[f32; 4]) {
    a[0] = a[0].min(b[0]);
    a[1] = a[1].min(b[1]);
    a[2] = a[2].max(b[2]);
    a[3] = a[3].max(b[3]);
}

/// Reading-order layout. Glyphs are reordered so that `glyphs` is in reading order, lines are
/// numbered in that order, and word gaps are inferred per line.
///
/// Heuristics (documented so they can be tuned and tested; plan/architecture.md §8):
/// 1. *Segments:* consecutive glyphs (content order) on one baseline with no column-sized gap.
///    A smaller glyph right next to the previous one (a sub- or superscript) stays in the segment.
///    Segments on the same baseline that nearly touch are merged (text drawn out of order).
/// 2. *Blocks:* segments stacked with line-sized gaps and overlapping horizontally.
/// 3. *Block order:* repeatedly take the top-most remaining block, preferring a block to its left
///    that overlaps it vertically (so columns read left, then right). A column goes on first (#3658):
///    the nearest block below the last one, overlapping it horizontally and at most 2.5 heights of
///    its last line below it, comes next, unless an unread block sits above it in its own span, sits
///    beside it on its left, or shares its first line (the first lines overlap by at least 30% of the
///    taller one: a table or form row, which reads across, even with labels and values in different
///    sizes or on shifted baselines).
///    Known limit: when a paragraph in the next column starts on the same line as the continuation
///    (text on a baseline grid), it counts as a row and the top-most rule applies, so the column
///    order of #3658 is lost there. Telling prose from a row in that case needs more evidence.
/// 4. Within a segment glyphs run left to right; runs of right-to-left script are reversed so the
///    text comes out in logical order.
pub fn layout(glyphs: Vec<TextGlyph>) -> PageText {
    let (order, line_of, space_before) = arrange(&glyphs);
    let mut slots: Vec<Option<TextGlyph>> = glyphs.into_iter().map(Some).collect();
    // Each glyph is emitted exactly once.
    let glyphs = order.iter().filter_map(|i| slots.get_mut(*i).and_then(Option::take)).collect();
    PageText { glyphs, line_of, space_before }
}

/// [`layout`] as the reading order of indices into `glyphs`, with each one's line and word gap.
fn arrange(glyphs: &[TextGlyph]) -> (Vec<usize>, Vec<u32>, Vec<bool>) {
    // Drop "fake bold" duplicates: the same character redrawn at (almost) the same place. A redraw
    // also overlaps most of its own width: a narrow letter in condensed text (80 Tz and below) sits
    // closer than a fifth of its height to its double, as the second "l" of "Alle" (#1467), and
    // OCR layers squeeze words to 20 Tz and below (#2457). Only a zero-width mark, which has no
    // width to overlap, is matched by its height alone.
    let mut keep = vec![true; glyphs.len()];
    for i in 1..glyphs.len() {
        let g = &glyphs[i];
        let h = (g.rect[3] - g.rect[1]).max(0.1);
        let w = g.rect[2] - g.rect[0];
        let dx = if w > 0.0 { (h * 0.2).min(w * 0.5) } else { h * 0.2 };
        for j in (i.saturating_sub(4)..i).rev() {
            let p = &glyphs[j];
            if keep[j] && p.text == g.text && (p.rect[0] - g.rect[0]).abs() < dx && (p.rect[1] - g.rect[1]).abs() < h * 0.2 {
                keep[i] = false;
                break;
            }
        }
    }
    let kept: Vec<usize> = (0..glyphs.len()).filter(|i| keep[*i]).collect();
    let glyphs: Vec<&TextGlyph> = kept.iter().map(|i| &glyphs[*i]).collect();
    let n = glyphs.len();
    if n == 0 {
        return Default::default();
    }
    let height = |i: usize| (glyphs[i].rect[3] - glyphs[i].rect[1]).max(0.1);
    let cy = |i: usize| (glyphs[i].rect[1] + glyphs[i].rect[3]) / 2.0;

    // 1. Segments in content order. Every run is a range of consecutive glyphs.
    let mut segs: Vec<Seg> = Vec::new();
    let mut run_start_x = 0.0;
    let mut furthest_progress = 0.0;
    for i in 0..n {
        let g = glyphs[i].rect;
        let cont = segs.last().is_some_and(|s| {
            let Some(p) = s.first.end.checked_sub(1) else { return false };
            let ph = height(p).min(height(i));
            let gap = g[0] - glyphs[p].rect[2];
            // A sub- or superscript is a smaller glyph on a shifted baseline: its em box centre sits off the line by
            // more than half its own height, but by less than half the height of the full-size glyph beside it.
            let centre_limit = if gap.abs() < ph { height(p).max(height(i)) * 0.5 } else { ph * 0.5 };
            (cy(i) - cy(p)).abs() < centre_limit && gap < ph * 3.0 && g[0] > glyphs[p].rect[0] - ph * 2.0
        });
        let reset = cont && segs.last().is_some_and(|s| restarts_run(&glyphs, s.first.start, i, furthest_progress));
        if cont
            && !reset
            && let Some(s) = segs.last_mut()
        {
            s.first.end = i + 1;
            union(&mut s.bbox, &g);
            s.h = s.h.max(height(i));
            furthest_progress = furthest_progress.max((g[0] - run_start_x).abs()).max((g[2] - run_start_x).abs());
        } else {
            segs.push(Seg::new(Run { start: i, end: i + 1 }, g, height(i)));
            run_start_x = g[0];
            furthest_progress = 0.0;
        }
    }
    // A run that redraws an earlier run at the same place (doubled or shadowed text) is dropped, as a doubled glyph is
    // above. Runs are keyed by a fingerprint of their glyph texts and a grid cell a fifth of a line high. A key leads to
    // the latest retained run with it, and `older[k]` links each retained run to the previous one with the same key, so
    // the earlier candidates are checked newest first, up to `REDRAW_CANDIDATES`. Both exist only for this pass.
    let retained = {
        let candidates = segs.iter().filter(|s| s.first.len() >= 2).count();
        let mut latest: HashMap<(u64, i32, i32), usize> = HashMap::with_capacity(candidates);
        let mut older = vec![NONE; segs.len()];
        let mut kept_runs = 0;
        for i in 0..segs.len() {
            let run = segs[i].first;
            if run.len() >= 2 {
                let tol = 0.2 * segs[i].h;
                let (cx, cy_) = ((glyphs[run.start].rect[0] / tol).floor() as i32, (glyphs[run.start].rect[1] / tol).floor() as i32);
                let fp = fingerprint(&glyphs, run);
                let mut checks = REDRAW_CANDIDATES;
                let twin = (-1..=1i32).any(|dx| {
                    (-1..=1i32).any(|dy| {
                        let mut link = latest.get(&(fp, cx.saturating_add(dx), cy_.saturating_add(dy))).copied();
                        while let Some(k) = link {
                            let Some(prev) = segs.get(k) else { break };
                            if checks == 0 {
                                return false;
                            }
                            checks -= 1;
                            if redraws(&glyphs, prev.first, run) {
                                return true;
                            }
                            link = older.get(k).copied().filter(|&o| o != NONE);
                        }
                        false
                    })
                });
                if twin {
                    continue;
                }
                older[kept_runs] = latest.insert((fp, cx, cy_), kept_runs).unwrap_or(NONE);
            }
            segs.swap(kept_runs, i);
            kept_runs += 1;
        }
        kept_runs
    };
    segs.truncate(retained);
    // Merge same-baseline segments that nearly touch.
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for a in 0..segs.len() {
            for b in 0..segs.len() {
                if a == b {
                    continue;
                }
                let (sa, sb) = (&segs[a], &segs[b]);
                let h = sa.h.min(sb.h);
                let gap = (sb.bbox[0] - sa.bbox[2]).max(sa.bbox[0] - sb.bbox[2]);
                if (sa.cy() - sb.cy()).abs() < h * 0.3 && (sa.h / sb.h - 1.0).abs() < 0.35 && gap < h * 1.2 {
                    let sb = segs.remove(b);
                    let a = if b < a { a - 1 } else { a };
                    segs[a].merge(sb);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }

    // 2. Blocks.
    let mut by_top: Vec<usize> = (0..segs.len()).collect();
    by_top.sort_by(|a, b| segs[*a].bbox[1].total_cmp(&segs[*b].bbox[1]));
    let mut blocks: Vec<(Vec<usize>, [f32; 4])> = Vec::new();
    for si in by_top {
        let s = &segs[si];
        let target = blocks.iter().position(|(members, bb)| {
            let Some(&m) = members.last() else { return false };
            let last = &segs[m];
            let vgap = s.bbox[1] - last.bbox[3];
            let overlap = s.bbox[2].min(bb[2]) - s.bbox[0].max(bb[0]);
            let minw = (s.bbox[2] - s.bbox[0]).min(bb[2] - bb[0]).max(1.0);
            vgap > -last.h * 0.5 && vgap < last.h.max(s.h) * 1.1 && overlap > minw * 0.3 && (last.h / s.h - 1.0).abs() < 0.6
        });
        match target {
            Some(b) => {
                blocks[b].0.push(si);
                let bb = s.bbox;
                union(&mut blocks[b].1, &bb);
            }
            None => blocks.push((vec![si], s.bbox)),
        }
    }

    // 3. Block order.
    let mut remaining: Vec<usize> = (0..blocks.len()).collect();
    let mut block_order: Vec<usize> = Vec::with_capacity(blocks.len());
    // A block to the left of `cb` that shares at least half the height of the shorter one: it reads first.
    let beside_left = |cb: &[f32; 4], lb: &[f32; 4]| {
        let v = lb[3].min(cb[3]) - lb[1].max(cb[1]);
        lb[2] <= cb[0] + 1.0 && v > 0.5 * (lb[3] - lb[1]).min(cb[3] - cb[1])
    };
    // The box of a block's first line. Every block is created with one segment, so the block's own box is a fallback
    // that is never used.
    let first_line = |b: usize| blocks[b].0.first().and_then(|&s| segs.get(s)).map_or(blocks[b].1, |s| s.bbox);
    while let Some(&top) = remaining.iter().min_by(|a, b| blocks[**a].1[1].total_cmp(&blocks[**b].1[1])) {
        // A column goes on past a paragraph gap (#3658): the nearest block below the one just read that overlaps it
        // horizontally comes next, if the gap above it is at most two and a half heights of the last line read (a
        // footer, a footnote or a caption under a figure is further) and no other unread block
        // - sits above it in its own horizontal span (the rest of a column above a full-width heading or footnote),
        // - is beside it on its left, or
        // - shares its first line: their first lines overlap by at least 30% of the taller one (a table or form row,
        //   which reads across, even with labels and values in different sizes or on shifted baselines; a line that
        //   only touches it, or a small superscript or footnote marker, is not a row).
        let next_in_column = block_order.last().and_then(|&prev| {
            let pb = blocks[prev].1;
            // The gap is measured in the height of the block's last line, which borders it.
            let pl = blocks[prev].0.last().and_then(|&s| segs.get(s)).map_or(pb, |s| s.bbox);
            let tol = (pl[3] - pl[1]).max(0.1) * 0.5;
            let c = remaining
                .iter()
                .copied()
                .filter(|&c| {
                    let cb = blocks[c].1;
                    let overlap = cb[2].min(pb[2]) - cb[0].max(pb[0]);
                    let minw = (cb[2] - cb[0]).min(pb[2] - pb[0]).max(1.0);
                    cb[1] >= pb[3] - tol && overlap > minw * 0.3
                })
                .min_by(|a, b| blocks[*a].1[1].total_cmp(&blocks[*b].1[1]))?;
            let cb = blocks[c].1;
            if cb[1] - pb[3] >= tol * 5.0 {
                return None;
            }
            let cl = first_line(c);
            let blocked = remaining.iter().any(|&r| {
                if r == c {
                    return false;
                }
                let rb = blocks[r].1;
                let rl = first_line(r);
                let shared = rl[3].min(cl[3]) - rl[1].max(cl[1]);
                let row = shared > 0.3 * (rl[3] - rl[1]).max(cl[3] - cl[1]);
                (rb[1] < cb[1] && rb[2].min(cb[2]) > rb[0].max(cb[0])) || beside_left(&cb, &rb) || row
            });
            (!blocked).then_some(c)
        });
        let pick = next_in_column.unwrap_or_else(|| {
            let tb = blocks[top].1;
            remaining
                .iter()
                .copied()
                .filter(|c| beside_left(&tb, &blocks[*c].1))
                .min_by(|a, b| blocks[*a].1[0].total_cmp(&blocks[*b].1[0]))
                .unwrap_or(top)
        });
        remaining.retain(|r| *r != pick);
        block_order.push(pick);
    }

    // 4. Emit each segment in block order. Its glyphs are placed here, into scratch that every segment reuses, and
    // appended straight to the output. Blocks and their order use only segment boxes, so placement can wait until now.
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut line_of: Vec<u32> = Vec::with_capacity(n);
    let mut space_before: Vec<bool> = Vec::with_capacity(n);
    let mut line = 0u32;
    let mut sc = Scratch::default();
    // `spaces[w]`: a word gap left of the w-th glyph of the segment, by position on the line, not by drawing order
    // (#1062). Reused by every segment.
    let mut spaces: Vec<bool> = Vec::new();
    for b in block_order {
        for &si in &blocks[b].0 {
            place_glyphs(&glyphs, &height, &segs[si], &mut sc);
            let h = segs[si].h;
            // Word gaps relative to this line's typical letter gap (handles tracking).
            // Long runs of repeated punctuation belong to leaders, not letter tracking: a TOC leader
            // must not raise the threshold above the word gaps in its title (#3086).
            sc.gaps.clear();
            sc.gaps.extend(sc.placed.windows(2).enumerate().filter_map(|(w, pair)| {
                let &[p, c] = pair else { return None };
                let (p, c) = (glyphs.get(p)?, glyphs.get(c)?);
                if p.text == c.text && p.text.chars().all(|ch| !ch.is_alphanumeric() && !ch.is_whitespace()) {
                    // Keep short tracked punctuation, including an ellipsis with a final period.
                    // Treat five or more matching glyphs as a leader. Three neighbours on each
                    // side suffice to classify this pair, bounding work even on long runs.
                    let same = |&&i: &&usize| glyphs.get(i).is_some_and(|g| g.text == p.text);
                    let before = sc.placed.get(..w).unwrap_or_default().iter().rev().take(3).take_while(same).count();
                    let after = sc.placed.get(w.saturating_add(2)..).unwrap_or_default().iter().take(3).take_while(same).count();
                    if before + after >= 3 {
                        return None;
                    }
                }
                Some(c.rect[0] - p.rect[2])
            }));
            sc.gaps.sort_by(f32::total_cmp);
            let typical = sc.gaps.get(sc.gaps.len() / 3).copied().unwrap_or(0.0).max(0.0);
            let threshold = (typical + h * WORD_GAP).max(h * WORD_GAP);
            spaces.clear();
            spaces.resize(sc.placed.len(), false);
            for (w, pair) in sc.placed.windows(2).enumerate() {
                let &[p, c] = pair else { continue };
                let gap = glyphs[c].rect[0] - glyphs[p].rect[2];
                let (pt, ct) = (&glyphs[p].text, &glyphs[c].text);
                let cjk = pt.chars().any(is_cjk) && ct.chars().any(is_cjk) && gap < h * 0.5;
                let tight_cluster = (pt.chars().any(is_complex) || ct.chars().any(is_complex)) && gap < h * 0.6;
                let run_start = sc.breaks.binary_search(&c).is_ok();
                if let Some(space) = spaces.get_mut(w + 1) {
                    *space = (gap > threshold || run_start) && !cjk && !tight_cluster && !pt.trim().is_empty() && !ct.trim().is_empty();
                }
            }
            // Right-to-left spans are reversed (visual → logical) with their gaps, and only then is a second copy made.
            let has_rtl = sc.placed.iter().any(|&i| glyphs[i].text.chars().any(is_rtl));
            if has_rtl {
                sc.logical.clear();
                sc.logical.extend_from_slice(&sc.placed);
                reverse_rtl(&glyphs, &mut sc.logical, &mut spaces);
            }
            let idx: &[usize] = if has_rtl { &sc.logical } else { &sc.placed };
            order.extend_from_slice(idx);
            line_of.resize(order.len(), line);
            // `spaces[0]` stays false: the first glyph of a segment never takes a space.
            space_before.extend(spaces.iter().take(idx.len()));
            space_before.resize(order.len(), false);
            line += 1;
        }
    }
    (order.into_iter().map(|i| kept[i]).collect(), line_of, space_before)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn em_box(text: &str, x0: f32, y0: f32, x1: f32, y1: f32) -> TextGlyph {
        TextGlyph { text: text.into(), rect: [x0, y0, x1, y1], quad: [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], direction: [1.0, 0.0] }
    }

    #[test]
    fn subscript_stays_in_its_word() {
        // "CO2-E": an 8.1 pt "CO" and "-E", with a 4.7 pt "2" drawn 2.7 pt lower (#3174).
        let glyphs = vec![
            em_box("C", 0.0, 92.5, 5.0, 100.6),
            em_box("O", 5.0, 92.5, 11.0, 100.6),
            em_box("2", 11.0, 97.9, 13.5, 102.6),
            em_box("-", 13.5, 92.5, 16.0, 100.6),
            em_box("E", 16.0, 92.5, 21.0, 100.6),
        ];
        assert_eq!(layout(glyphs).plain_text(), "CO2-E");
    }

    #[test]
    fn identity_v_columns_are_searchable_in_reading_order() {
        use lopdf::{Document, Object, Stream, dictionary};
        for encoding in ["Identity-V", "UniJIS-UCS2-V"] {
            let mut doc = Document::with_version("1.7");
            let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /TestUnicode def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange 5 beginbfchar <0001> <65E5> <0002> <672C> <0003> <8A9E> <0004> <6587> <0005> <7AE0> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
            let cmap = if encoding == "Identity-V" {
                cmap.to_vec()
            } else {
                let mut mapping = String::from_utf8(cmap.to_vec()).unwrap();
                for (code, unicode) in [("0001", "65E5"), ("0002", "672C"), ("0003", "8A9E"), ("0004", "6587"), ("0005", "7AE0")] {
                    mapping = mapping.replace(&format!("<{code}> <{unicode}>"), &format!("<{unicode}> <{unicode}>"));
                }
                mapping.into_bytes()
            };
            let unicode = doc.add_object(Stream::new(dictionary! {}, cmap));
            let cid = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "TestVertical",
            "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
            "DW" => 1000, "DW2" => vec![Object::Integer(880), Object::Integer(-1000)], "CIDToGIDMap" => "Identity"
        });
            let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "TestVertical", "Encoding" => encoding, "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => unicode });
            let horizontal = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "TestVertical", "Encoding" => "Identity-H", "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => unicode });
            let (first, second) = if encoding == "Identity-V" { ("000100020003", "00040005") } else { ("65E5672C8A9E", "65877AE0") };
            let operators =
                format!("BT /H1 20 Tf 1 0 0 1 40 280 Tm <{first}> Tj /F1 20 Tf 1 0 0 1 200 250 Tm <{first}> Tj 1 0 0 1 170 250 Tm <{second}> Tj ET");
            let content = doc.add_object(Stream::new(dictionary! {}, operators.into_bytes()));
            let pages = doc.new_object_id();
            let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font, "H1" => horizontal}}, "Contents" => content });
            doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
            let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
            doc.trailer.set("Root", catalog);
            let mut bytes = Vec::new();
            doc.save_to(&mut bytes).unwrap();
            let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
            let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
            assert_eq!(text.plain_text(), "日本語\n日本語\n文章");
            assert_eq!(text.find("日本語"), vec![0..3, 3..6]);
            assert_eq!(text.find("文章"), vec![6..8]);
            let rects = text.line_rects(3..6);
            assert_eq!(rects.len(), 1);
            assert!(rects[0][3] - rects[0][1] > rects[0][2] - rects[0][0], "a vertical search highlight: {rects:?}");
            assert!(text.glyphs[3].rect[0] > text.glyphs[6].rect[0], "columns read right to left in page view coordinates");
        }
    }

    /// WSCAD writes `/ToUnicode /Identity-H`, a name rather than a CMap stream, on Type 0 fonts
    /// whose two-byte codes are UTF-16 code units. The codes read as that Unicode text (as in
    /// Acrobat), so the page's text can be selected and found; before, it had none. Controls (here a
    /// DEL and a C1 control at the line ends) are not text.
    #[test]
    fn identity_to_unicode_names_give_text() {
        use lopdf::{Document, Object, Stream, dictionary};
        for name in ["Identity-H", "Identity-V"] {
            let mut doc = Document::with_version("1.4");
            let cid = doc.add_object(dictionary! {
                "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "RobotoMono-Regular",
                "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
                "DW" => 600, "CIDToGIDMap" => "Identity"
            });
            let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "RobotoMono-Regular", "Encoding" => "Identity-H", "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => name });
            let operators = "BT /F1 12 Tf 1 0 0 1 20 250 Tm <004400650063006B0062006C006100740074007F> Tj 1 0 0 1 20 200 Tm <005A00FC0072006900630068002020AC0085> Tj ET";
            let content = doc.add_object(Stream::new(dictionary! {}, operators.as_bytes().to_vec()));
            let pages = doc.new_object_id();
            let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content });
            doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
            let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
            doc.trailer.set("Root", catalog);
            let mut bytes = Vec::new();
            doc.save_to(&mut bytes).unwrap();
            let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
            let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
            assert_eq!(text.plain_text(), "Deckblatt\nZürich €", "{name}");
            assert_eq!(text.find("zürich"), vec![9..15], "{name}: glyphs 9 to 14");
        }
    }

    /// One page per text matrix, each showing `codes` in Helvetica whose `/ToUnicode` maps the
    /// codes in `unicode` (a `bfchar` body) to several characters.
    fn ligature_pages(unicode: &str, codes: &str, matrices: &[&str]) -> Pdf {
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.7");
        let cmap = format!(
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /TestLigatures def /CMapType 2 def 1 begincodespacerange <00> <FF> endcodespacerange {unicode} endcmap CMapName currentdict /CMap defineresource pop end end"
        );
        let to_unicode = doc.add_object(Stream::new(dictionary! {}, cmap.into_bytes()));
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "ToUnicode" => to_unicode });
        let pages = doc.new_object_id();
        let mut kids = Vec::new();
        for m in matrices {
            let content = doc.add_object(Stream::new(dictionary! {}, format!("BT /F1 20 Tf {m} Tm ({codes}) Tj ET").into_bytes()));
            let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content });
            kids.push(Object::Reference(page));
        }
        let count = kids.len() as i64;
        doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => count}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        Pdf::new(std::sync::Arc::new(bytes)).unwrap()
    }

    #[test]
    fn a_ligature_is_one_glyph_in_every_writing_direction() {
        // "o", one glyph for "ffi", "c", "e": "office", turned 0°, 90°, 180° and 270°. The same
        // codes without the ligature mapping ("oWce") are the control for the glyph box.
        let turns = ["1 0 0 1 100 150", "0 1 -1 0 150 100", "-1 0 0 -1 200 150", "0 -1 1 0 150 200"];
        let ligature = ligature_pages("1 beginbfchar <57> <006600660069> endbfchar", "oWce", &turns);
        let control = ligature_pages("", "oWce", &turns);
        for (page, turn) in turns.iter().enumerate() {
            let text = extract_page(&ligature, page, &InterpreterSettings::default()).unwrap();
            let plain = extract_page(&control, page, &InterpreterSettings::default()).unwrap();
            assert_eq!(plain.plain_text(), "oWce", "Tm {turn}");
            assert_eq!(text.plain_text(), "office", "Tm {turn}");
            assert_eq!(text.glyphs.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), ["o", "ffi", "c", "e"], "Tm {turn}");
            assert_eq!(text.glyphs[1].rect, plain.glyphs[1].rect, "the painted glyph's box, Tm {turn}");
            assert_eq!(text.find("ffi"), vec![1..2], "Tm {turn}");
            assert_eq!(text.find("of"), vec![0..2], "a match ending inside the ligature covers it, Tm {turn}");
            assert_eq!(text.line_rects(0..4).len(), 1, "one line, Tm {turn}");
        }
    }

    /// One page per content stream, in Helvetica (`/F1`), with `/P1` in the resources'
    /// `/Properties` naming the `/ActualText` "fi".
    fn marked_pages(contents: &[&str]) -> Pdf {
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.7");
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let pages = doc.new_object_id();
        let mut kids = Vec::new();
        for c in contents {
            let content = doc.add_object(Stream::new(dictionary! {}, c.as_bytes().to_vec()));
            let resources = dictionary! {
                "Font" => dictionary! {"F1" => font},
                "Properties" => dictionary! {"P1" => dictionary! {"ActualText" => Object::string_literal("fi")}},
            };
            let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => resources, "Contents" => content });
            kids.push(Object::Reference(page));
        }
        let count = kids.len() as i64;
        doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => count}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        Pdf::new(std::sync::Arc::new(bytes)).unwrap()
    }

    fn marked_text(pdf: &Pdf, page: usize) -> PageText {
        extract_page(pdf, page, &InterpreterSettings::default()).unwrap()
    }

    #[test]
    fn redraws_of_a_long_replacement_share_a_text_budget() {
        let long = "0041".repeat(32 * 1024);
        let redraws = 200;
        let content = format!(
            "BT /F1 20 Tf 1 0 0 1 100 150 Tm /Span <</ActualText <FEFF{long}>>> BDC (x) Tj EMC {} ET",
            "1 0 0 1 100 150 Tm (x) Tj ".repeat(redraws)
        );
        let pdf = marked_pages(&[&content]);
        REDRAW_TEXT_BYTES.with(|c| c.set(0));
        let text = marked_text(&pdf, 0);
        let given = REDRAW_TEXT_BYTES.with(|c| c.get());
        assert!(given <= MAX_REDRAW_TEXT, "{given} bytes of replacement text given to {redraws} redraws");
        assert!(text.plain_text().starts_with("AAAA"));
        // A short replacement still covers its redraw.
        let pdf = marked_pages(&["BT /F1 20 Tf 1 0 0 1 100 150 Tm /Span <</ActualText (fi)>> BDC (x) Tj EMC 1 0 0 1 100 150 Tm (x) Tj ET"]);
        REDRAW_TEXT_BYTES.with(|c| c.set(0));
        assert_eq!(marked_text(&pdf, 0).plain_text(), "fi");
        assert_eq!(REDRAW_TEXT_BYTES.with(|c| c.get()), 2);
    }

    #[test]
    fn text_after_an_inline_image_survives_a_non_ascii_byte_before_a_later_ei() {
        // Everyday content both ways: a string with a byte above 0x7F, and a later word holding
        // a capital "EI". The end-of-data look-ahead used to read the string's bytes as image
        // data, so the image swallowed the rest of the page.
        let pdf = marked_pages(&[
            "BI /W 1 /H 1 /BPC 8 /CS /DeviceGray ID \0 EI BT /F1 20 Tf 1 0 0 1 20 200 Tm (Gr\u{fc}\u{df}e) Tj 1 0 0 1 20 150 Tm (EINGANG) Tj ET",
        ]);

        assert!(marked_text(&pdf, 0).plain_text().contains("EINGANG"), "the text after the inline image was not extracted");
    }

    #[test]
    fn actual_text_replaces_the_glyphs_it_marks_in_every_writing_direction() {
        // Chrome marks each shaped Thai cluster this way: "น้ำ" (U+0E19 U+0E49 U+0E33) here stands
        // for the two glyphs "xy", between "a" and "b". The same codes unmarked are the control.
        let turns = ["1 0 0 1 100 150", "0 1 -1 0 150 100", "-1 0 0 -1 200 150", "0 -1 1 0 150 200"];
        let marked: Vec<String> =
            turns.iter().map(|m| format!("BT /F1 20 Tf {m} Tm (a) Tj /Span <</ActualText <FEFF0E190E490E33>>> BDC (xy) Tj EMC (b) Tj ET")).collect();
        let plain: Vec<String> = turns.iter().map(|m| format!("BT /F1 20 Tf {m} Tm (axyb) Tj ET")).collect();
        let marked = marked_pages(&marked.iter().map(String::as_str).collect::<Vec<_>>());
        let plain = marked_pages(&plain.iter().map(String::as_str).collect::<Vec<_>>());
        for (page, turn) in turns.iter().enumerate() {
            let text = marked_text(&marked, page);
            let control = marked_text(&plain, page);
            assert_eq!(control.plain_text(), "axyb", "Tm {turn}");
            assert_eq!(text.plain_text(), "aน้ำb", "Tm {turn}");
            assert_eq!(text.glyphs.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), ["a", "น้ำ", "b"], "Tm {turn}");
            assert_eq!(text.find("น้ำ"), vec![1..2], "Tm {turn}");
            assert_eq!(text.line_rects(0..3).len(), 1, "one line, Tm {turn}");
            // The replacement's box is the union of the boxes of the glyphs it replaces.
            let (x, y) = (&control.glyphs[1].rect, &control.glyphs[2].rect);
            let union = [x[0].min(y[0]), x[1].min(y[1]), x[2].max(y[2]), x[3].max(y[3])];
            for (got, want) in text.glyphs[1].rect.iter().zip(union) {
                assert!((got - want).abs() < 0.01, "box {:?} ≠ {union:?}, Tm {turn}", text.glyphs[1].rect);
            }
            assert_eq!(text.glyphs[1].direction, control.glyphs[1].direction, "Tm {turn}");
        }
    }

    #[test]
    fn redraw_dedupe_work_is_bounded_on_hostile_pages() {
        // 100,000 replacements and 100,000 unmarked glyphs that never match, all at one spot:
        // unbounded, every replacement would compare every glyph (10^10 comparisons).
        const N: usize = 100_000;
        let glyph = |text: &str| {
            let quad = [[100.0, 110.0], [110.0, 110.0], [110.0, 100.0], [100.0, 100.0]];
            (0, [1.0, 0.0], TextGlyph { text: text.into(), rect: [100.0, 100.0, 110.0, 110.0], quad, direction: [1.0, 0.0] })
        };
        let mut dev = TextDevice::new();
        // A genuine redraw first: it is still deduplicated within the budget.
        dev.glyphs.push(glyph("fi"));
        dev.glyphs.push(glyph("x"));
        dev.replaced.push(Replaced { at: Some(0), text: "fi".into(), originals: vec![("x".into(), [100.0, 100.0, 110.0, 110.0])] });
        dev.glyphs.extend((0..N).map(|_| glyph("y")));
        for _ in 0..N {
            dev.glyphs.push(glyph("z"));
            let at = dev.glyphs.len() - 1;
            dev.replaced.push(Replaced { at: Some(at), text: "z".into(), originals: vec![("q".into(), [100.0, 100.0, 110.0, 110.0])] });
        }
        let started = std::time::Instant::now();
        dev.finish();
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "took {:?}", started.elapsed());
        assert_eq!(dev.glyphs.len(), 2 * N + 2, "unmatched glyphs are kept");
        assert_eq!(dev.glyphs[1].2.text, "fi", "the redraw found within the budget takes the replacement");
    }

    #[test]
    fn actual_text_follows_the_pdf_rules_for_marked_content() {
        let pdf = marked_pages(&[
            // Named in the resources' /Properties.
            "BT /F1 20 Tf 1 0 0 1 100 150 Tm (o) Tj /Span /P1 BDC (W) Tj EMC (ce) Tj ET",
            // Nested: the outermost replacement covers everything inside it.
            "BT /F1 20 Tf 1 0 0 1 100 150 Tm /Span <</ActualText (AB)>> BDC /Span <</ActualText (Z)>> BDC (x) Tj EMC (y) Tj EMC ET",
            // Empty: the marked glyphs stand for no text (a soft hyphen at a line break); the gap
            // the hyphen leaves still reads as one.
            "BT /F1 20 Tf 1 0 0 1 100 150 Tm (co) Tj /Span <</ActualText ()>> BDC (-) Tj EMC (op) Tj ET",
            // Across two lines: each glyph keeps its own text, so selection and lines still work.
            "BT /F1 20 Tf 1 0 0 1 100 150 Tm /Span <</ActualText (Q)>> BDC (x) Tj 0 -40 Td (y) Tj EMC ET",
            // Unbalanced: a stray EMC first, and a replacement the page never closes.
            "EMC BT /F1 20 Tf 1 0 0 1 100 150 Tm (a) Tj /Span <</ActualText (fi)>> BDC (xy) Tj ET",
            // Marked content without /ActualText changes nothing.
            "BT /F1 20 Tf 1 0 0 1 100 150 Tm /P <</MCID 0>> BDC (ab) Tj EMC /Artifact BMC (c) Tj EMC ET",
        ]);
        let plain: Vec<String> = (0..6).map(|p| marked_text(&pdf, p).plain_text()).collect();
        assert_eq!(plain, ["ofice", "AB", "co op", "x\ny", "afi", "abc"]);
    }

    #[test]
    fn an_unmarked_redraw_of_actual_text_reads_as_the_same_text() {
        // Canva draws text twice (an outline or shadow) and marks only one drawing. The copy,
        // before or after the marked one and half a point off, takes the same replacement and is
        // dropped as a redraw; the same codes elsewhere on the page keep their own text.
        let marked = "BT /F1 20 Tf 1 0 0 1 100 150 Tm (a) Tj /Span <</ActualText <FEFF0E190E490E33>>> BDC (xy) Tj EMC (b) Tj ET";
        let copy = "BT /F1 20 Tf 1 0 0 1 100.5 150.5 Tm (axyb) Tj ET";
        let elsewhere = "BT /F1 20 Tf 1 0 0 1 100 50 Tm (axyb) Tj ET";
        let pdf = marked_pages(&[&format!("{marked} {copy} {elsewhere}"), &format!("{copy} {marked} {elsewhere}")]);
        for page in 0..2 {
            assert_eq!(marked_text(&pdf, page).plain_text(), "aน้ำb\naxyb", "page {page}");
        }
    }

    /// Issue #3109: text drawn in clipping mode (`7 Tr`) sets the clip for a subsequent fill,
    /// shading or image without painting the glyphs directly, and must still reach `TextDevice`
    /// with the same boxes and search ranges as modes 0–6.
    #[test]
    fn clipping_mode_text_is_extracted_and_searchable_like_other_rendering_modes() {
        let fill_through = " 0 0 1 rg 0 0 300 300 re f";
        let mut pages: Vec<String> =
            (0..=7).map(|mode| format!("q BT /F1 24 Tf {mode} Tr 72 150 Td () Tj [(Hello) -200 (World)] TJ ET{fill_through} Q")).collect();
        // Page 8: /ActualText wrapping 7 Tr clip-only text.
        pages.push(format!("q /Span <</ActualText (Hello World)>> BDC BT /F1 24 Tf 7 Tr 72 150 Td (Glyphs) Tj ET EMC{fill_through} Q"));
        // Page 9: 0 Tr underpainting followed by 7 Tr at the same position (outline/gradient overlay) deduplicates.
        pages.push(format!("BT /F1 24 Tf 0 Tr 72 150 Td (Hello World) Tj ET q BT /F1 24 Tf 7 Tr 72 150 Td (Hello World) Tj ET{fill_through} Q"));
        // Page 10: 90° turned 7 Tr text.
        pages.push(format!("q BT /F1 24 Tf 7 Tr 0 1 -1 0 150 100 Tm (Hello World) Tj ET{fill_through} Q"));
        // Page 11: 7 Tr without any fill-through (visually blank, still extractable).
        pages.push("BT /F1 24 Tf 7 Tr 72 150 Td (Hello World) Tj ET".into());

        let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
        let pdf = marked_pages(&refs);
        let control = marked_text(&pdf, 0);
        assert_eq!(control.plain_text(), "Hello World");
        let control_rects = control.line_rects(0..5);
        assert_eq!(control_rects.len(), 1);
        for mode in 0..=7 {
            let text = marked_text(&pdf, mode);
            assert_eq!(text.plain_text(), "Hello World", "mode {mode} Tr");
            assert_eq!(text.find("hello"), vec![0..5], "mode {mode} Tr");
            assert_eq!(text.find_opts("Hello", true, true), vec![0..5], "mode {mode} Tr");
            assert_eq!(
                text.find_with(|chars| chars.starts_with(&['H', 'e', 'l', 'l', 'o']).then_some(0..5).into_iter().collect()),
                vec![0..5],
                "mode {mode} Tr"
            );
            assert_eq!(text.line_rects(0..5), control_rects, "mode {mode} Tr");
            assert_eq!(text.glyphs, control.glyphs, "mode {mode} Tr");
        }
        let actual = marked_text(&pdf, 8);
        assert_eq!(actual.plain_text(), "Hello World");
        assert_eq!(actual.find("Hello"), vec![0..1]);

        let deduped = marked_text(&pdf, 9);
        assert_eq!(deduped.plain_text(), "Hello World");

        let turned = marked_text(&pdf, 10);
        assert_eq!(turned.plain_text(), "Hello World");
        assert_eq!(turned.find("World"), vec![6..11]);

        let unfilled = marked_text(&pdf, 11);
        assert_eq!(unfilled.plain_text(), "Hello World");

        // Visual check: page 7 (`7 Tr` + blue fill-through) paints blue only inside the clipped
        // glyph outlines, whereas page 11 (`7 Tr` alone) paints no ink at all.
        let mut renderer = crate::PageRenderer::new(std::sync::Arc::new(pdf.data().as_ref().to_vec()), crate::RenderConfig::default());
        let filled_px = renderer.render(crate::RenderRequest { page: 7, kind: crate::RequestKind::Pixels, scale: 1.0, ..Default::default() });
        assert!(filled_px.error.is_none());
        let blue_pixels = filled_px.rgba.as_chunks::<4>().0.iter().filter(|p| p[0] < 32 && p[1] < 32 && p[2] > 200).count();
        let white_pixels = filled_px.rgba.as_chunks::<4>().0.iter().filter(|p| p[..3] == [255, 255, 255]).count();
        assert!(blue_pixels > 50 && white_pixels > 80_000, "blue={blue_pixels}, white={white_pixels}");

        let unfilled_px = renderer.render(crate::RenderRequest { page: 11, kind: crate::RequestKind::Pixels, scale: 1.0, ..Default::default() });
        assert!(unfilled_px.error.is_none());
        assert!(unfilled_px.rgba.as_chunks::<4>().0.iter().all(|p| p[..3] == [255, 255, 255]));
    }

    #[test]
    fn clipping_mode_filter_keeps_titles_and_excludes_ocr() {
        let pdf = marked_pages(&[
            "q BT /F1 24 Tf 7 Tr 30 200 Td (Clipped heading) Tj ET 0 0 1 rg 0 0 300 300 re f Q BT /F1 12 Tf 3 Tr 30 100 Td (Hidden OCR) Tj ET",
            "q /Span <</ActualText (Accessible heading)>> BDC BT /F1 24 Tf 7 Tr 30 200 Td (Clipped heading) Tj ET EMC 0 0 1 rg 0 0 300 300 re f Q",
        ]);
        assert!(marked_text(&pdf, 0).plain_text().contains("Hidden OCR"));
        for (page, expected) in [(0, "Clipped heading"), (1, "Accessible heading")] {
            let text = extract_page_filtered(&pdf, page, &InterpreterSettings::default(), TextFilter { images: true, ..Default::default() }).unwrap();
            assert_eq!(text.plain_text(), expected);
            assert!(!text.find("heading").is_empty());
        }
    }

    /// ISO 32000-2 §9.3.6: Type 3 text under `7 Tr` is not rendered (vendored hayro-interpret
    /// patch 64), but it is still clipping text, not an invisible text layer: the filter that
    /// drops OCR layers keeps it, as it keeps clipping text in other fonts.
    #[test]
    fn clipping_mode_type3_text_is_kept_by_the_ocr_filter() {
        let font = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] \
                    /CharProcs << /A 6 0 R >> /Encoding << /Type /Encoding /Differences [65 /A] >> \
                    /FirstChar 65 /LastChar 65 /Widths [1000] >>";
        let glyph = "1000 0 100 0 900 800 d1 100 0 800 800 re f";
        let content = "BT /T3 24 Tf 7 Tr 30 200 Td (AAA) Tj ET BT /T3 12 Tf 3 Tr 30 100 Td (A) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /T3 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            font.to_string(),
            format!("<< /Length {} >>\nstream\n{glyph}\nendstream", glyph.len()),
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
        let all = extract_page_filtered(&pdf, 0, &InterpreterSettings::default(), TextFilter::default()).unwrap();
        assert_eq!(all.plain_text().matches('A').count(), 4, "{:?}", all.plain_text());
        let filtered = extract_page_filtered(&pdf, 0, &InterpreterSettings::default(), TextFilter { images: true, ..Default::default() }).unwrap();
        assert_eq!(filtered.plain_text().trim(), "AAA", "the 7 Tr text stays, the 3 Tr layer goes");
    }

    /// One page per `(encoding, content)`, in Helvetica (`/F1`).
    fn encoded_pages(pages: &[(lopdf::Object, &[u8])]) -> Pdf {
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let mut kids = Vec::new();
        for (encoding, content) in pages {
            let font =
                doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => encoding.clone() });
            let content = doc.add_object(Stream::new(dictionary! {}, content.to_vec()));
            let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages_id, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content });
            kids.push(Object::Reference(page));
        }
        let count = kids.len() as i64;
        doc.objects.insert(pages_id, dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => count}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        Pdf::new(std::sync::Arc::new(bytes)).unwrap()
    }

    /// The Annex D.2 alias codes extract as the ink they draw, not as formatting characters.
    ///
    /// WinAnsiEncoding gives 0xAD and 0xA0 the glyph names `hyphen` and `space`, the names it
    /// also gives 0x2D and 0x20, and MacRomanEncoding gives 0xCA `space`. Reporting the Adobe
    /// Glyph List values U+00AD and U+00A0 for them hid visible text: a soft hyphen is invisible,
    /// so "well-known" written that way matched no search and no redaction pattern.
    /// `XiaoBiaoSong.pdf` in the pdf.js corpus drew 104 of them.
    ///
    /// A `/Differences` entry naming `sfthyphen` or `nbspace` is a deliberate statement about one
    /// code and keeps its glyph list value. pdftotext draws the same line: on this fixture it
    /// reads the base-encoded codes as "a-b a b" and the named ones as U+00AD.
    #[test]
    fn the_annex_d2_alias_codes_read_as_an_ordinary_hyphen_and_space() {
        use lopdf::{Object, dictionary};
        let name = |n: &str| Object::Name(n.as_bytes().to_vec());
        let differences = Object::Dictionary(dictionary! {
            "Type" => "Encoding",
            "BaseEncoding" => "WinAnsiEncoding",
            "Differences" => vec![Object::Integer(88), name("sfthyphen"), Object::Integer(89), name("nbspace")],
        });
        let pdf = encoded_pages(&[
            (name("WinAnsiEncoding"), &b"BT /F1 20 Tf 1 0 0 1 20 150 Tm (a\xadb\xa0c) Tj ET"[..]),
            (name("MacRomanEncoding"), &b"BT /F1 20 Tf 1 0 0 1 20 150 Tm (a\xcab) Tj ET"[..]),
            (differences, &b"BT /F1 20 Tf 1 0 0 1 20 150 Tm (aXbYc) Tj ET"[..]),
        ]);
        let texts =
            |page| extract_page(&pdf, page, &InterpreterSettings::default()).unwrap().glyphs.iter().map(|g| g.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(0), ["a", "-", "b", " ", "c"], "WinAnsi 0xAD and 0xA0");
        assert_eq!(texts(1), ["a", " ", "b"], "MacRoman 0xCA");
        assert_eq!(texts(2), ["a", "\u{ad}", "b", "\u{a0}", "c"], "a named glyph keeps its glyph list value");
    }

    /// #3169, #3170: glyph names outside the Adobe Glyph List still read as text. Ligature
    /// (`f_i`) and suffixed (`hyphen.case`) names go by the glyph list specification's full
    /// mapping. A name that only numbers the glyph (`c251` small caps, `C39`, `C174`, `C39.sc`)
    /// reads as nothing, not as a guess from the code (review on #3239: with `/C1` at 233,
    /// StandardEncoding's 233 would read `Ø` for a glyph drawn as `é`), and so do a ZapfDingbats
    /// name (`a12`) and a name with no meaning left (`foo`).
    #[test]
    fn glyph_names_outside_the_glyph_list_still_read_as_text() {
        use lopdf::{Object, dictionary};
        let name = |n: &str| Object::Name(n.as_bytes().to_vec());
        let differences = Object::Dictionary(dictionary! {
            "Type" => "Encoding",
            "Differences" => vec![
                Object::Integer(30), name("f_i"), name("f_l"), Object::Integer(2), name("hyphen.case"),
                Object::Integer(39), name("C39"), Object::Integer(97), name("c251"), Object::Integer(174), name("C174"),
                Object::Integer(88), name("a12"), name("foo"), name("uni00660069"),
                Object::Integer(61), name("C39.sc"), Object::Integer(233), name("C1"),
            ],
        });
        // One glyph per line: the test font has none of these glyphs, so they have no width, and
        // on one line a later `fi` would sit on the first and be dropped as its fake-bold copy.
        let mut content = b"BT /F1 20 Tf 20 290 Td".to_vec();
        for code in *b"\x1e\x1f\x02'a\xaeXYZ=\xe9" {
            content.extend_from_slice(b" (");
            content.push(code);
            content.extend_from_slice(b") Tj 0 -25 Td");
        }
        content.extend_from_slice(b" ET");
        let pdf = encoded_pages(&[(differences, &content[..])]);
        let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap().plain_text();
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        assert_eq!(lines, ["fi", "fl", "-", "fi"], "{text:?}");
    }

    /// Review on #3239: the glyph list specification's `uni` and `u` forms take uppercase hex
    /// digits only, and map a name with lowercase ones to nothing ("uni20ac" in its examples).
    /// Names upstream's own lookup can't read (several groups, a suffix) show it: lowercase reads
    /// as nothing, uppercase as its characters. A bare `uni20ac` or `u1040c` is still read by
    /// upstream's lenient `unicode_from_name`, outside this mapping.
    #[test]
    fn glyph_names_with_lowercase_hex_read_as_nothing() {
        use lopdf::{Object, dictionary};
        let name = |n: &str| Object::Name(n.as_bytes().to_vec());
        let differences = Object::Dictionary(dictionary! {
            "Type" => "Encoding",
            "Differences" => vec![Object::Integer(65), name("uni20ac0308"), name("uni20AC0308"), name("u1040c.alt"), name("u1040C.alt"), name("f_uni20ac"), name("f_uni20AC")],
        });
        let pdf = encoded_pages(&[(
            differences,
            &b"BT /F1 20 Tf 20 180 Td (A) Tj 0 -25 Td (B) Tj 0 -25 Td (C) Tj 0 -25 Td (D) Tj 0 -25 Td (E) Tj 0 -25 Td (F) Tj ET"[..],
        )]);
        // One glyph per line, so each name's text stays its own (and a name read as nothing
        // leaves no line).
        let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap().plain_text();
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        assert_eq!(lines, ["\u{20ac}\u{308}", "\u{1040c}", "f", "f\u{20ac}"], "{text:?}");
    }

    #[test]
    fn narrow_ligatures_keep_every_letter() {
        // Split evenly, this "ffi" put its two "f"s so close that the second was dropped as a
        // fake-bold copy of the first.
        let pdf = ligature_pages("1 beginbfchar <62> <006600660069> endbfchar", "obce", &["1 0 0 1 100 150"]);
        let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
        assert_eq!(text.plain_text(), "office");
    }

    #[test]
    fn right_to_left_ligatures_keep_their_logical_order() {
        // "سلام" drawn left to right as meem, alef, lam, seen; then with lam-alef as one glyph.
        let letters = ligature_pages("4 beginbfchar <61> <0645> <62> <0644> <63> <0633> <64> <0627> endbfchar", "adbc", &["1 0 0 1 100 150"]);
        let ligature = ligature_pages("3 beginbfchar <61> <0645> <62> <06440627> <63> <0633> endbfchar", "abc", &["1 0 0 1 100 150"]);
        for pdf in [letters, ligature] {
            assert_eq!(extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap().plain_text(), "سلام");
        }
        // "بَاب" with beh and its fatha as one glyph, as Add text writes shaped Arabic: the mark
        // stays after its letter.
        let marked = ligature_pages("3 beginbfchar <61> <0628064E> <62> <0627> <63> <0628> endbfchar", "cba", &["1 0 0 1 100 150"]);
        assert_eq!(extract_page(&marked, 0, &InterpreterSettings::default()).unwrap().plain_text(), "بَاب");
    }

    #[test]
    fn vertical_ligatures_stay_in_their_column() {
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.7");
        // CID 2 is one glyph for "株式".
        let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /TestUnicode def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange 3 beginbfchar <0001> <65E5> <0002> <682A5F0F> <0003> <8A9E> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
        let unicode = doc.add_object(Stream::new(dictionary! {}, cmap.to_vec()));
        let cid = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "TestVertical",
            "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
            "DW" => 1000, "DW2" => vec![Object::Integer(880), Object::Integer(-1000)], "CIDToGIDMap" => "Identity"
        });
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "TestVertical", "Encoding" => "Identity-V", "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => unicode });
        let content = doc.add_object(Stream::new(dictionary! {}, b"BT /F1 20 Tf 1 0 0 1 200 250 Tm <000100020003> Tj ET".to_vec()));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content });
        doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
        let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
        assert_eq!(text.plain_text(), "日株式語");
        assert_eq!(text.glyphs.len(), 3);
        assert_eq!(text.line_rects(0..3).len(), 1, "one column");
    }

    #[test]
    fn upright_mapping_round_trips() {
        let r = [10.0, 20.0, 30.0, 25.0];
        for q in 0..4 {
            assert_eq!(from_upright(to_upright(r, q, 300.0, 200.0), q, 300.0, 200.0), r, "quarter {q}");
        }
        // A 90° page: the view's top-right corner is the upright top-left.
        assert_eq!(to_upright([290.0, 0.0, 300.0, 10.0], 1, 300.0, 200.0), [0.0, 0.0, 10.0, 10.0]);
    }

    #[test]
    fn turkish_dotted_and_dotless_i_match_in_any_case() {
        // "İSTANBUL IRMAK KIŞ" on one line: each word typed in lower case must find it, and the
        // reverse. `to_lowercase` alone made `İ` two characters and left `ı` unmatched.
        let mut v = Vec::new();
        word(&mut v, "İSTANBUL", 10.0, 10.0, 7.0);
        word(&mut v, "IRMAK", 80.0, 10.0, 7.0);
        word(&mut v, "KIŞ", 130.0, 10.0, 7.0);
        let upper = layout(v);
        assert_eq!(upper.plain_text(), "İSTANBUL IRMAK KIŞ");
        assert_eq!(upper.find("istanbul"), vec![0..8]);
        assert_eq!(upper.find("İstanbul"), vec![0..8]);
        assert_eq!(upper.find("ırmak"), vec![8..13]);
        assert_eq!(upper.find("kış"), vec![13..16]);
        assert_eq!(upper.find("istanbul ırmak kış"), vec![0..16], "a phrase across the words");
        assert_eq!(upper.find_opts("ırmak", false, true), vec![8..13], "whole words");
        assert!(upper.find_opts("istanbul", true, false).is_empty(), "case-sensitive search stays exact");
        assert_eq!(upper.find_opts("İSTANBUL", true, false), vec![0..8]);

        let mut v = Vec::new();
        word(&mut v, "istanbul", 10.0, 10.0, 7.0);
        word(&mut v, "ırmak", 80.0, 10.0, 7.0);
        word(&mut v, "kış", 130.0, 10.0, 7.0);
        let lower = layout(v);
        assert_eq!(lower.find("İSTANBUL"), vec![0..8]);
        assert_eq!(lower.find("IRMAK"), vec![8..13]);
        assert_eq!(lower.find("KIŞ"), vec![13..16]);
    }

    #[test]
    fn greek_sigma_forms_match_without_changing_exact_search_or_glyph_ranges() {
        let mut v = Vec::new();
        word(&mut v, "κόσμος", 10.0, 10.0, 7.0);
        word(&mut v, "ΚΌΣΜΟΣ", 80.0, 10.0, 7.0);
        word(&mut v, "σςΣ", 150.0, 10.0, 7.0);
        let text = layout(v);
        assert_eq!(text.plain_text(), "κόσμος ΚΌΣΜΟΣ σςΣ");
        for query in ["κόσμος", "ΚΌΣΜΟΣ"] {
            assert_eq!(text.find(query), vec![0..6, 6..12]);
            assert_eq!(text.find_opts(query, false, true), vec![0..6, 6..12]);
        }
        for query in ["σ", "ς", "Σ"] {
            assert_eq!(text.find(query), vec![2..3, 5..6, 8..9, 11..12, 12..13, 13..14, 14..15]);
            assert!(text.find_opts(query, false, true).is_empty());
        }
        assert_eq!(text.find_opts("κόσμος", true, false), vec![0..6]);
        assert_eq!(text.find_opts("ΚΌΣΜΟΣ", true, false), vec![6..12]);
        assert_eq!(text.find_opts("ς", true, false), vec![5..6, 13..14]);
        assert_eq!(text.find_opts_limited("ς", false, false, 2), vec![2..3, 5..6]);
        assert_eq!(text.line_rects(text.find("κόσμος")[1].clone()), text.line_rects(6..12));
    }

    #[test]
    fn case_folding_keeps_one_character_per_character() {
        for s in ["İSTANBUL", "ırmak", "Hello WORLD", "ÇAĞLAR", "Straße", "ΣΟΦΙΑ", "Σσς", "日本語"] {
            assert_eq!(fold_case(s).chars().count(), s.chars().count(), "{s}");
        }
        assert_eq!(fold_case("Hello WORLD"), "hello world");
        assert_eq!(fold_case("ÇAĞLAR Şişli"), "çağlar şişli");
        assert_eq!(fold_case("İIıi"), "iiii");
        assert_eq!(fold_case("Σσς"), "σσσ");
    }

    fn g(t: &str, x0: f32, y0: f32, x1: f32) -> TextGlyph {
        let rect = [x0, y0, x1, y0 + 10.0];
        TextGlyph { text: t.into(), rect, quad: [[x0, y0], [x1, y0], [x1, rect[3]], [x0, rect[3]]], direction: [1.0, 0.0] }
    }

    /// A glyph whose em box is `w` × `h` at `origin`, advancing along `dir` with its up edge along
    /// `up` (a unit step of each), as a sheared or turned text matrix draws it.
    fn tilted(t: &str, origin: [f32; 2], dir: [f32; 2], up: [f32; 2], w: f32, h: f32) -> TextGlyph {
        let p = |a: f32, b: f32| [origin[0] + dir[0] * a + up[0] * b, origin[1] + dir[1] * a + up[1] * b];
        let quad = [p(0.0, 0.0), p(w, 0.0), p(w, h), p(0.0, h)];
        let xs = quad.map(|q| q[0]);
        let ys = quad.map(|q| q[1]);
        let rect = [
            xs.iter().copied().fold(f32::MAX, f32::min),
            ys.iter().copied().fold(f32::MAX, f32::min),
            xs.iter().copied().fold(f32::MIN, f32::max),
            ys.iter().copied().fold(f32::MIN, f32::max),
        ];
        TextGlyph { text: t.into(), rect, quad, direction: dir }
    }

    /// #3693: pointing at the middle of a glyph in sheared or turned text finds that glyph, not
    /// the one before it whose bounding box overlaps it.
    #[test]
    fn nearest_finds_the_glyph_under_the_pointer_in_sheared_and_turned_text() {
        let s = std::f32::consts::FRAC_1_SQRT_2;
        // (writing direction, up edge) in view space (y down): upright, sheared 0.35, turned 45°.
        for (dir, up) in [([1.0, 0.0], [0.0, -1.0]), ([1.0, 0.0], [0.35, -1.0]), ([s, -s], [-s, -s])] {
            let mut advance = 0.0;
            let glyphs: Vec<TextGlyph> = "film in the list of tiny jilt flit"
                .chars()
                .map(|c| {
                    let w = if "filjt ".contains(c) { 3.0 } else { 6.0 };
                    let g = tilted(&c.to_string(), [50.0 + dir[0] * advance, 300.0 + dir[1] * advance], dir, up, w, 14.0);
                    advance += w;
                    g
                })
                .collect();
            let t = layout(glyphs.clone());
            assert_eq!(t.glyphs.len(), glyphs.len(), "layout kept every glyph");
            for (i, g) in glyphs.iter().enumerate() {
                let cx = g.quad.iter().map(|q| q[0]).sum::<f32>() / 4.0;
                let cy = g.quad.iter().map(|q| q[1]).sum::<f32>() / 4.0;
                assert_eq!(t.nearest(cx, cy), Some(i), "dir {dir:?} up {up:?}: glyph {i} ({})", g.text);
            }
            // Past either end of the line, the end glyph is still the nearest.
            let (first, last) = (&glyphs[0], &glyphs[glyphs.len() - 1]);
            assert_eq!(t.nearest(first.quad[0][0] - dir[0] * 8.0, first.quad[0][1] - dir[1] * 8.0), Some(0));
            assert_eq!(t.nearest(last.quad[1][0] + dir[0] * 8.0, last.quad[1][1] + dir[1] * 8.0), Some(glyphs.len() - 1));
        }
    }

    #[test]
    fn a_degenerate_or_nan_quad_holds_no_point() {
        assert!(!quad_contains(&[[1.0, 1.0]; 4], 1.0, 1.0));
        assert!(!quad_contains(&[[0.0, 0.0], [1.0, 0.0], [f32::NAN, 1.0], [0.0, 1.0]], 0.5, 0.5));
        assert!(quad_contains(&[[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]], 4.0, 2.0));
        // Wound the other way.
        assert!(quad_contains(&[[0.0, 0.0], [0.0, 4.0], [4.0, 4.0], [4.0, 0.0]], 2.0, 2.0));
    }

    #[test]
    fn lines_words_and_search() {
        // "Hello world" on line 1, "Next" on line 2.
        let mut v = Vec::new();
        for (i, c) in "Hello".chars().enumerate() {
            v.push(g(&c.to_string(), 10.0 + i as f32 * 6.0, 10.0, 16.0 + i as f32 * 6.0));
        }
        for (i, c) in "world".chars().enumerate() {
            v.push(g(&c.to_string(), 50.0 + i as f32 * 6.0, 10.0, 56.0 + i as f32 * 6.0));
        }
        for (i, c) in "Next".chars().enumerate() {
            v.push(g(&c.to_string(), 10.0 + i as f32 * 6.0, 30.0, 16.0 + i as f32 * 6.0));
        }
        let t = layout(v);
        assert_eq!(t.plain_text(), "Hello world\nNext");
        assert_eq!(t.find("WORLD next"), vec![5..14]);
        assert_eq!(t.find("o"), vec![4..5, 6..7]);
        assert_eq!(t.line_rects(3..12).len(), 2);
        assert_eq!(t.nearest(52.0, 14.0), Some(5));
    }

    /// #1315: a custom match that ends on a reconstructed word or line separator does not take
    /// the following glyph.
    #[test]
    fn a_match_ending_on_a_separator_stops_before_the_next_glyph() {
        let glyphs: Vec<TextGlyph> = "quickbrown"
            .chars()
            .enumerate()
            .map(|(i, c)| {
                let x = i as f32 * 6.0;
                g(&c.to_string(), x, 0.0, x + 6.0)
            })
            .collect();
        for line_break in [false, true] {
            let mut space_before = vec![false; 10];
            space_before[5] = !line_break;
            let line_of = (0..10).map(|i| u32::from(line_break && i >= 5)).collect();
            let page = PageText { glyphs: glyphs.clone(), line_of, space_before };
            let plain = if line_break { "quick\nbrown" } else { "quick brown" };
            assert_eq!(page.plain_text(), plain);
            assert_eq!(page.find("quick "), vec![0..5]);
            let span = |r: std::ops::Range<usize>| Vec::from([r]);
            assert_eq!(page.find_with(|_| span(0..5)), vec![0..5]);
            assert_eq!(page.find_with(|_| span(5..11)), vec![5..10]);
            assert_eq!(page.find_with(|_| span(6..11)), vec![5..10]);
            let hits = page.find_with(|_| span(0..6));
            assert_eq!(page.text_of(hits[0].clone()), "quick");
            assert_eq!(hits, vec![0..5]);
        }
    }

    #[test]
    fn phrase_search_ignores_repeated_spaces() {
        // "quick  brown" drawn with two space glyphs in a row. Text extraction keeps both spaces,
        // but the phrase search has always collapsed runs of whitespace in the query, so the page
        // text has to be collapsed the same way or the phrase is never found (#814).
        let mut v = Vec::new();
        word(&mut v, "quick", 10.0, 10.0, 6.0);
        v.push(g(" ", 46.0, 10.0, 52.0));
        v.push(g(" ", 52.0, 10.0, 58.0));
        word(&mut v, "brown", 58.0, 10.0, 6.0);
        let n = v.len();
        let t = PageText { glyphs: v, line_of: vec![0; n], space_before: vec![false; n] };
        assert_eq!(t.plain_text(), "quick  brown");
        assert_eq!(t.find("quick brown"), vec![0..12]);
        assert_eq!(t.find("quick  brown"), vec![0..12], "the query's own run of spaces is collapsed too");
        assert_eq!(t.find("quick\tbrown"), vec![0..12]);
        assert_eq!(t.find("quick"), vec![0..5]);
        assert_eq!(t.find("brown"), vec![7..12]);
        assert_eq!(t.find_opts_limited("o", false, false, 0), Vec::new());
        assert!(t.find("quick absent").is_empty());
    }

    #[test]
    fn limited_search_stops_after_the_requested_matches() {
        let mut glyphs = Vec::new();
        word(&mut glyphs, "oooo", 10.0, 10.0, 6.0);
        let text = layout(glyphs);
        assert_eq!(text.find_opts_limited("o", false, false, 2), vec![0..1, 1..2]);
        assert_eq!(text.find_opts_limited("o", false, false, 10), vec![0..1, 1..2, 2..3, 3..4]);
    }

    fn word(v: &mut Vec<TextGlyph>, s: &str, x: f32, y: f32, advance: f32) {
        for (i, c) in s.chars().enumerate() {
            let x0 = x + i as f32 * advance;
            v.push(g(&c.to_string(), x0, y, x0 + 6.0));
        }
    }

    #[test]
    fn fake_bold_duplicates_are_removed() {
        let mut v = Vec::new();
        for (i, c) in "Bold".chars().enumerate() {
            let x = 10.0 + i as f32 * 7.0;
            v.push(g(&c.to_string(), x, 10.0, x + 6.0));
            v.push(g(&c.to_string(), x + 0.3, 10.0, x + 6.3));
        }
        assert_eq!(layout(v).plain_text(), "Bold");
        // A zero-width combining mark redrawn on itself is dropped too.
        let v = vec![g("e", 10.0, 10.0, 16.0), g("\u{301}", 13.0, 10.0, 13.0), g("e", 10.0, 10.0, 16.0), g("\u{301}", 13.0, 10.0, 13.0)];
        assert_eq!(layout(v).plain_text(), "e\u{301}");
    }

    /// #1467: condensed text (80 Tz and below, OCR layers fitted to the scan) sets narrow letters
    /// closer than a fifth of their height apart; a doubled letter there is two letters, not a redraw.
    #[test]
    fn condensed_double_letters_are_kept() {
        for scale in [0.8, 0.65, 0.5] {
            let mut v = Vec::new();
            let mut x = 10.0;
            // Helvetica advances at 10 pt, in thousandths of an em.
            for (c, adv) in [('A', 667.0), ('l', 222.0), ('l', 222.0), ('e', 556.0)] {
                let w = adv / 100.0 * scale;
                v.push(g(&c.to_string(), x, 10.0, x + w));
                x += w;
            }
            assert_eq!(layout(v.clone()).plain_text(), "Alle", "at {scale} horizontal scaling");
            // Each letter redrawn a fifth of a point to the right is still one letter.
            let bold: Vec<TextGlyph> = v.iter().flat_map(|c| [c.clone(), g(&c.text, c.rect[0] + 0.2, 10.0, c.rect[2] + 0.2)]).collect();
            assert_eq!(layout(bold).plain_text(), "Alle", "fake bold at {scale} horizontal scaling");
        }
    }

    /// #2457: OCR text layers squeeze each word into its box down to 20 Tz and below, where a
    /// narrow letter is less than a twentieth of its height wide. Doubled letters, and letters
    /// repeated two or three places on, are still letters there, not redraws.
    #[test]
    fn ocr_condensed_double_letters_are_kept() {
        // Helvetica advances, in thousandths of an em.
        let adv = |c: char| match c {
            'M' => 833.0,
            'K' | 'B' => 667.0,
            'W' => 944.0,
            'I' | 't' | 'f' => 278.0,
            'i' | 'l' => 222.0,
            'm' => 833.0,
            'r' => 333.0,
            's' => 500.0,
            _ => 556.0,
        };
        for scale in [0.2, 0.1, 0.05] {
            for word in ["Hallo", "Mitte", "Wille", "Kommission", "Illustration", "still", "Brunnen", "Kaffee"] {
                let mut v = Vec::new();
                let mut x = 10.0;
                for c in word.chars() {
                    let w = adv(c) / 100.0 * scale;
                    v.push(g(&c.to_string(), x, 10.0, x + w));
                    x += w;
                }
                assert_eq!(layout(v.clone()).plain_text(), word, "at {scale} horizontal scaling");
                // Each letter drawn twice at the same place is still one letter.
                let bold: Vec<TextGlyph> = v.iter().flat_map(|c| [c.clone(), c.clone()]).collect();
                assert_eq!(layout(bold).plain_text(), word, "fake bold at {scale} horizontal scaling");
                // And redrawn a quarter of its own width to the right (the match reaches half of it).
                let shifted: Vec<TextGlyph> = v
                    .iter()
                    .flat_map(|c| {
                        let d = (c.rect[2] - c.rect[0]) * 0.25;
                        [c.clone(), g(&c.text, c.rect[0] + d, 10.0, c.rect[2] + d)]
                    })
                    .collect();
                assert_eq!(layout(shifted).plain_text(), word, "shifted fake bold at {scale} horizontal scaling");
            }
        }
    }

    #[test]
    fn two_columns_read_left_then_right_even_if_drawn_interleaved() {
        let mut v = Vec::new();
        // Drawn row by row across both columns, as some producers do.
        word(&mut v, "Left1", 10.0, 10.0, 6.0);
        word(&mut v, "Right1", 200.0, 10.0, 6.0);
        word(&mut v, "Left2", 10.0, 24.0, 6.0);
        word(&mut v, "Right2", 200.0, 24.0, 6.0);
        let t = layout(v);
        assert_eq!(t.plain_text(), "Left1\nLeft2\nRight1\nRight2");
    }

    /// Lays out one-word lines, each `(text, x, y)`, 10 pt high: 14 pt apart is a line in a paragraph, 24 pt a
    /// paragraph gap.
    fn lines(at: &[(&str, f32, f32)]) -> String {
        let mut v = Vec::new();
        for &(s, x, y) in at {
            word(&mut v, s, x, y, 6.0);
        }
        layout(v).plain_text()
    }

    #[test]
    fn a_column_reads_on_past_a_paragraph_gap() {
        // #3658: a heading and paragraph A at the top of the left column, a paragraph gap, then paragraph B, which
        // goes on at the top of the right column, and paragraph C below it.
        let two = [
            ("Heading", 10.0, 10.0),
            ("A1", 10.0, 30.0),
            ("A2", 10.0, 44.0),
            ("A3", 10.0, 58.0),
            ("B1", 10.0, 82.0),
            ("B2", 10.0, 96.0),
            ("B3", 200.0, 30.0),
            ("B4", 200.0, 44.0),
            ("C1", 200.0, 68.0),
            ("C2", 200.0, 82.0),
        ];
        assert_eq!(lines(&two), "Heading\nA1\nA2\nA3\nB1\nB2\nB3\nB4\nC1\nC2");
        // Three columns, each with a gap at a different height.
        let three = [
            ("A1", 10.0, 10.0),
            ("A2", 10.0, 24.0),
            ("B1", 10.0, 48.0),
            ("C1", 150.0, 10.0),
            ("D1", 150.0, 38.0),
            ("D2", 150.0, 52.0),
            ("E1", 290.0, 10.0),
            ("E2", 290.0, 24.0),
            ("E3", 290.0, 38.0),
            ("F1", 290.0, 62.0),
        ];
        assert_eq!(lines(&three), "A1\nA2\nB1\nC1\nD1\nD2\nE1\nE2\nE3\nF1");
    }

    #[test]
    fn a_column_reads_on_past_a_line_that_only_touches_it() {
        // The #3658 page with a right-column paragraph whose first line touches B1's by 1 pt or 0.1 pt, or whose
        // second line carries a 6 pt footnote marker reaching up beside B2: none of them is a row with B1.
        let page = |extra: &[(&str, f32, f32, f32)]| {
            let mut v = Vec::new();
            let left = [("H", 10.0, 10.0), ("A1", 10.0, 30.0), ("A2", 10.0, 44.0), ("A3", 10.0, 58.0), ("B1", 10.0, 82.0), ("B2", 10.0, 96.0)];
            for &(s, x, y) in left.iter().chain(&[("R1", 200.0, 30.0), ("R2", 200.0, 44.0)]) {
                word(&mut v, s, x, y, 6.0);
            }
            for &(s, x, y, h) in extra {
                for (i, c) in s.chars().enumerate() {
                    let x0 = x + i as f32 * h * 0.6;
                    v.push(em_box(&c.to_string(), x0, y, x0 + h * 0.6, y + h));
                }
            }
            layout(v).plain_text().replace('\n', " ")
        };
        assert_eq!(page(&[("S1", 200.0, 91.0, 10.0), ("S2", 200.0, 105.0, 10.0)]), "H A1 A2 A3 B1 B2 R1 R2 S1 S2", "1 pt touch");
        assert_eq!(page(&[("S1", 200.0, 72.1, 10.0), ("S2", 200.0, 86.1, 10.0)]), "H A1 A2 A3 B1 B2 R1 R2 S1 S2", "0.1 pt touch");
        let marker = page(&[("S1", 200.0, 68.0, 10.0), ("x", 200.0, 96.0, 10.0), ("9", 206.5, 90.0, 6.0), ("S2y", 214.0, 96.0, 10.0)]);
        assert!(marker.starts_with("H A1 A2 A3 B1 B2 R1 R2 S1 "), "footnote marker: {marker}");
    }

    #[test]
    fn a_full_width_line_ends_the_columns_above_it() {
        // Two columns, a full-width heading, two more columns and a full-width footnote. The left column stops at the
        // heading and the footnote: the right column above each one reads first.
        let page = [
            ("L1", 10.0, 10.0),
            ("L2", 10.0, 24.0),
            ("L3", 10.0, 48.0),
            ("R1", 200.0, 10.0),
            ("R2", 200.0, 38.0),
            ("Fullwidthheading", 10.0, 80.0),
            ("L4", 10.0, 110.0),
            ("L5", 10.0, 124.0),
            ("L6", 10.0, 148.0),
            ("R3", 200.0, 110.0),
            ("R4", 200.0, 138.0),
            ("Fullwidthfootnote", 10.0, 200.0),
        ];
        // The two full-width lines run past the right column (x = 286 and 304) at 18 pt a letter.
        let mut v = Vec::new();
        for &(s, x, y) in &page {
            word(&mut v, s, x, y, if s.len() > 2 { 18.0 } else { 6.0 });
        }
        assert_eq!(layout(v).plain_text(), "L1\nL2\nL3\nR1\nR2\nFullwidthheading\nL4\nL5\nL6\nR3\nR4\nFullwidthfootnote");
    }

    #[test]
    fn table_rows_apart_still_read_across() {
        // Rows a paragraph gap apart whose cells start level read row by row, as before #3658.
        let rows = [("Name", 10.0, 10.0), ("Qty", 100.0, 10.0), ("Apple", 10.0, 34.0), ("12", 100.0, 34.0), ("Pear", 10.0, 58.0), ("7", 100.0, 58.0)];
        assert_eq!(lines(&rows), "Name\nQty\nApple\n12\nPear\n7");
    }

    #[test]
    fn form_rows_in_mixed_sizes_still_read_across() {
        // 8 pt labels beside 12 pt values on one baseline, rows 30 pt apart: the tops differ by 4 pt, but the first
        // lines overlap, so each row reads label then value.
        let mut v = Vec::new();
        let sized = |v: &mut Vec<TextGlyph>, s: &str, x: f32, base: f32, h: f32| {
            for (i, c) in s.chars().enumerate() {
                let x0 = x + i as f32 * h * 0.6;
                v.push(em_box(&c.to_string(), x0, base - h, x0 + h * 0.6, base));
            }
        };
        for (i, (label, value)) in [("Name", "Alice"), ("Street", "Main"), ("City", "Oslo")].into_iter().enumerate() {
            let base = 20.0 + 30.0 * i as f32;
            sized(&mut v, label, 10.0, base, 8.0);
            sized(&mut v, value, 100.0, base, 12.0);
        }
        assert_eq!(layout(v).plain_text(), "Name\nAlice\nStreet\nMain\nCity\nOslo");
        // Values drawn 6 pt below their labels, rows a paragraph gap apart.
        let offset =
            [("Name", 10.0, 10.0), ("Val1", 100.0, 16.0), ("Addr", 10.0, 40.0), ("Val2", 100.0, 46.0), ("City", 10.0, 70.0), ("Val3", 100.0, 76.0)];
        assert_eq!(lines(&offset), "Name\nVal1\nAddr\nVal2\nCity\nVal3");
    }

    #[test]
    fn a_column_does_not_jump_to_a_distant_footer_footnote_or_caption() {
        // A page number far below the left column reads after the right column.
        let footer = [
            ("L1", 10.0, 10.0),
            ("L2", 10.0, 24.0),
            ("L3", 10.0, 48.0),
            ("R1", 200.0, 10.0),
            ("R2", 200.0, 24.0),
            ("R3", 200.0, 48.0),
            ("Pg7", 10.0, 400.0),
        ];
        assert_eq!(lines(&footer), "L1\nL2\nR1\nR2\nL3\nR3\nPg7");
        // So does a footnote under the left column only, with the page number under the right one.
        let footnote = [
            ("L1", 10.0, 10.0),
            ("L2", 10.0, 24.0),
            ("L3", 10.0, 48.0),
            ("R1", 200.0, 10.0),
            ("R2", 200.0, 24.0),
            ("R3", 200.0, 48.0),
            ("Note", 10.0, 380.0),
            ("Pg7", 200.0, 400.0),
        ];
        assert_eq!(lines(&footnote), "L1\nL2\nR1\nR2\nL3\nR3\nNote\nPg7");
        // A caption under a full-width figure (no text from 70 to 250), as narrow as the left column, reads after both
        // columns above the figure and before the columns below it.
        let caption = [
            ("L1", 10.0, 10.0),
            ("L2", 10.0, 24.0),
            ("L3", 10.0, 38.0),
            ("R1", 200.0, 10.0),
            ("R2", 200.0, 24.0),
            ("R3", 200.0, 38.0),
            ("Cap", 10.0, 260.0),
            ("M1", 10.0, 290.0),
            ("M2", 10.0, 304.0),
            ("N1", 200.0, 290.0),
            ("N2", 200.0, 304.0),
        ];
        assert_eq!(lines(&caption), "L1\nL2\nL3\nR1\nR2\nR3\nCap\nM1\nM2\nN1\nN2");
    }

    #[test]
    fn many_column_paragraphs_order_in_bounded_time() {
        // 2000 paragraphs in two columns, the right one offset by half a paragraph: every block is a column
        // continuation, and choosing each one scans the remaining blocks.
        let mut v = Vec::new();
        for i in 0..1000u16 {
            let y = 10.0 + 24.0 * f32::from(i);
            word(&mut v, "L", 10.0, y, 6.0);
            word(&mut v, "R", 200.0, y + 12.0, 6.0);
        }
        let start = std::time::Instant::now();
        let text = layout(v).plain_text();
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
        // Compared without printing 4000 characters on failure.
        let expected = format!("{}{}", "L\n".repeat(1000), "R\n".repeat(999) + "R");
        assert!(text == expected, "the first R reads after {:?} lines, not 1000", text.find('R').map(|i| i / 2));
    }

    #[test]
    fn tracked_text_keeps_letters_together() {
        let mut v = Vec::new();
        word(&mut v, "CHAPTER", 10.0, 10.0, 9.0); // 3pt tracking between 6pt-wide letters
        word(&mut v, "FOUR", 10.0 + 7.0 * 9.0 + 5.0, 10.0, 9.0);
        assert_eq!(layout(v).plain_text(), "CHAPTER FOUR");
    }

    #[test]
    fn toc_leaders_do_not_hide_word_gaps() {
        use lopdf::{Document, Object, Stream, dictionary};
        for dots in [0, 5, 10, 20, 40, 60] {
            for (words, expected) in [
                ("(Was)-333(ist)-333(OTRS?)", "Was ist OTRS?"),
                ("(Das)-333(OTRS)-333(Web-Interface)", "Das OTRS Web-Interface"),
                ("(Neuen)-333(Zugang)-333(beantragen)", "Neuen Zugang beantragen"),
            ] {
                // The issue's LaTeX TJ spacing, without embedding the corpus PDF or a font asset.
                let mut doc = Document::with_version("1.7");
                let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Times-Roman" });
                let operators = format!("BT /F1 10 Tf 10 100 Td [{words}-441{}-1663(7)] TJ ET", "-500(.)".repeat(dots));
                let content = doc.add_object(Stream::new(dictionary! {}, operators.into_bytes()));
                let pages = doc.new_object_id();
                let page = doc.add_object(dictionary! {
                    "Type" => "Page", "Parent" => pages,
                    "MediaBox" => vec![0.into(), 0.into(), 800.into(), Object::Integer(200)],
                    "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } }, "Contents" => content
                });
                doc.objects.insert(pages, dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1 }.into());
                let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
                doc.trailer.set("Root", catalog);
                let mut bytes = Vec::new();
                doc.save_to(&mut bytes).unwrap();
                let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
                let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
                assert_eq!(text.plain_text(), format!("{expected} {}7", ". ".repeat(dots)), "{dots} leader dots");
                let hits = text.find_opts(expected, true, true);
                assert_eq!(hits.len(), 1, "{expected:?}, {dots} leader dots");
                assert_eq!(text.text_of(hits[0].clone()), expected);
                assert_eq!(text.line_rects(hits[0].clone()).len(), 1);
                assert_eq!(text.find(&expected.to_lowercase()), hits);
                assert!(text.find(&expected.replace(' ', "")).is_empty());
            }
        }
    }

    #[test]
    fn short_tracked_ellipses_remain_searchable() {
        for dots in [3, 4] {
            let mut v = Vec::new();
            for i in 0..dots {
                let x = 10.0 + i as f32 * 4.0;
                v.push(g(".", x, 10.0, x + 2.0));
            }
            let ellipsis = ".".repeat(dots);
            let text = layout(v.clone());
            assert_eq!(text.plain_text(), ellipsis);
            assert_eq!(text.find(&ellipsis), vec![0..dots]);
            v.insert(0, g("A", 0.0, 10.0, 6.0));
            let x = 12.0 + dots as f32 * 4.0;
            v.push(g("B", x, 10.0, x + 6.0));
            let text = layout(v);
            let expected = format!("A {ellipsis} B");
            assert_eq!(text.plain_text(), expected);
            assert_eq!(text.find(&expected), vec![0..dots + 2]);
        }
    }

    #[test]
    fn leaders_preserve_tracking_in_letters_numbers_and_punctuation() {
        for advance in [9.0, 12.0] {
            for leader in [".", "-", "…"] {
                let mut v = Vec::new();
                word(&mut v, "CHAPTER-1", 10.0, 10.0, advance);
                let x = 10.0 + 9.0 * advance + 5.0;
                word(&mut v, "1.4", x, 10.0, advance);
                let mut x = x + 3.0 * advance + 5.0;
                for _ in 0..60 {
                    v.push(g(leader, x, 10.0, x + 2.5));
                    x += 11.0;
                }
                let text = layout(v);
                assert_eq!(text.plain_text(), format!("CHAPTER-1 1.4 {}", vec![leader; 60].join(" ")));
                assert_eq!(text.find("chapter-1 1.4"), vec![0..12]);
            }
        }
        // Decimal punctuation and repeated letters/digits still use their original tracking estimate.
        for value in ["1.4", "1111", "llll"] {
            let mut v = Vec::new();
            word(&mut v, value, 10.0, 10.0, 12.0);
            assert_eq!(layout(v).plain_text(), value);
        }
    }

    #[test]
    fn rtl_runs_come_out_in_logical_order_and_cjk_has_no_spaces() {
        let mut v = Vec::new();
        // Hebrew "שלום" drawn visually right-to-left: ם ו ל ש from left to right.
        word(&mut v, "םולש", 10.0, 10.0, 6.0);
        assert_eq!(layout(v).plain_text(), "שלום");
        let mut v = Vec::new();
        word(&mut v, "每个字", 10.0, 10.0, 9.0);
        assert_eq!(layout(v).plain_text(), "每个字");
    }

    /// #1062: the word gap stays between the runs when the right-hand run is drawn first, and
    /// between the words of a right-to-left line.
    #[test]
    fn word_gaps_follow_position_not_drawing_order() {
        let mut v = Vec::new();
        word(&mut v, "19.4", 40.0, 10.0, 6.0);
        word(&mut v, "20.0", 10.0, 10.0, 6.0);
        assert_eq!(layout(v).plain_text(), "20.0 19.4");
        let mut v = Vec::new();
        // "שלום עולם" drawn visually: עולם reversed on the left, שלום reversed on the right.
        word(&mut v, "םלוע", 10.0, 10.0, 6.0);
        word(&mut v, "םולש", 40.0, 10.0, 6.0);
        assert_eq!(layout(v).plain_text(), "שלום עולם");
    }

    /// #2667: a number or punctuation on a right-to-left line must not split it into separately reversed words.
    #[test]
    fn rtl_line_with_number_and_punctuation_reads_in_logical_order() {
        let line = |words: &[&str]| {
            let mut v = Vec::new();
            let mut x = 10.0;
            for w in words {
                word(&mut v, w, x, 10.0, 6.0);
                x += w.chars().count() as f32 * 6.0 + 6.0;
            }
            layout(v).plain_text()
        };
        // Each line as drawn, left to right.
        assert_eq!(line(&["םילקש", "120", "ריחמ"]), "מחיר 120 שקלים");
        assert_eq!(line(&[".הקידב", "ךמסמ", "והז", ":תירבע"]), "עברית: זהו מסמך בדיקה.");
        assert_eq!(line(&["باتك", "12.5", "ددع"]), "عدد 12.5 كتاب");
        assert_eq!(line(&["םלוע", "םולש"]), "שלום עולם");
        assert_eq!(line(&["Total", "םולש", "price"]), "Total שלום price");
    }

    /// Two lines, "ab cd" and "e fg": words stop at blank glyphs, word gaps and line ends.
    #[test]
    fn words_and_lines_around_a_glyph() {
        let t = PageText {
            glyphs: Vec::from(["a", "b", " ", "c", "d", "e", "f", "g"].map(|s| TextGlyph {
                text: s.into(),
                rect: [0.0; 4],
                quad: [[0.0; 2]; 4],
                direction: [1.0, 0.0],
            })),
            line_of: vec![0, 0, 0, 0, 0, 1, 1, 1],
            space_before: vec![false, false, false, false, false, false, true, false],
        };
        assert_eq!(t.word_at(1), Some((0, 1)));
        assert_eq!(t.word_at(3), Some((3, 4)));
        assert_eq!(t.word_at(4), Some((3, 4)));
        assert_eq!(t.word_at(5), Some((5, 5)));
        assert_eq!(t.word_at(7), Some((6, 7)));
        assert_eq!(t.line_at(2), Some((0, 4)));
        assert_eq!(t.line_at(6), Some((5, 7)));
        assert_eq!(t.word_at(8), None);
        assert_eq!(t.line_at(8), None);
        assert_eq!(PageText::default().line_at(0), None);
    }

    /// A 400 × 400 page drawing `ops` with Helvetica as /F1 and Courier as /F2.
    fn text_page(ops: &str) -> Pdf {
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.7");
        let helvetica = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let courier = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier" });
        let content = doc.add_object(Stream::new(dictionary! {}, ops.as_bytes().to_vec()));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(400), Object::Integer(400)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => helvetica, "F2" => courier}}, "Contents" => content });
        doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        Pdf::new(std::sync::Arc::new(bytes)).unwrap()
    }

    /// `body` shown in /F`font` at `size` with its baseline turned `degrees` counter-clockwise.
    fn turned(degrees: f64, font: u8, size: f64, x: f64, y: f64, body: &str) -> String {
        let (s, c) = degrees.to_radians().sin_cos();
        format!("BT /F{font} {size} Tf {c:.6} {s:.6} {:.6} {c:.6} {x:.4} {y:.4} Tm {body} ET\n", -s)
    }

    fn page_text(ops: &str) -> String {
        extract_page(&text_page(ops), 0, &InterpreterSettings::default()).unwrap().plain_text()
    }

    #[test]
    fn short_doubled_word_at_same_origin_does_not_interleave() {
        let ops = "BT /F1 12 Tf 50 Tz 1 0 0 1 100 200 Tm (world) Tj ET\n".repeat(2);
        assert_single_word(page_glyphs(&ops));
    }

    #[test]
    fn short_near_shadow_does_not_interleave() {
        let ops = "BT /F1 12 Tf 50 Tz 1 0 0 1 100 200 Tm (world) Tj ET\nBT /F1 12 Tf 50 Tz 1 0 0 1 101 200 Tm (world) Tj ET\n";
        assert_single_word(page_glyphs(ops));
    }

    #[test]
    fn turned_short_doubled_word_does_not_interleave() {
        let one = turned(37.0, 1, 12.0, 100.0, 200.0, "50 Tz (world) Tj");
        assert_single_word(page_glyphs(&(one.clone() + &one)));
    }

    #[test]
    fn large_backward_tj_adjustment_stays_in_one_run() {
        let text = page_glyphs("BT /F1 12 Tf 50 Tz 1 0 0 1 100 200 Tm [(wor) 3000 (ld)] TJ ET");
        assert_eq!(text.plain_text(), "ld wor");
        assert_eq!(text.glyphs.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), ["l", "d", "w", "o", "r"]);
        assert!(text.line_of.iter().all(|line| *line == 0));
        assert_eq!(text.space_before, [false, false, true, false, false]);
    }

    #[test]
    fn reset_prefix_requires_one_glyph_height_of_progress() {
        let mut glyphs = Vec::new();
        word(&mut glyphs, "abc", 10.0, 10.0, 6.0);
        word(&mut glyphs, "abc", 10.0, 10.0, 6.0);
        let glyphs: Vec<&TextGlyph> = glyphs.iter().collect();
        assert!(!restarts_run(&glyphs, 0, 3, 9.99));
        assert!(!restarts_run(&glyphs, 0, 3, 10.0));
        assert!(restarts_run(&glyphs, 0, 3, 10.01));
    }

    #[test]
    fn reset_prefix_uses_the_existing_redraw_position_tolerance() {
        let mut near = Vec::new();
        word(&mut near, "abc", 10.0, 10.0, 6.0);
        word(&mut near, "abc", 11.99, 10.0, 6.0);
        let near: Vec<&TextGlyph> = near.iter().collect();
        assert!(restarts_run(&near, 0, 3, 10.01));

        let mut far = Vec::new();
        word(&mut far, "abc", 10.0, 10.0, 6.0);
        word(&mut far, "abc", 12.01, 10.0, 6.0);
        let far: Vec<&TextGlyph> = far.iter().collect();
        assert!(!restarts_run(&far, 0, 3, 10.01));
    }

    #[test]
    fn reset_prefix_rejects_non_finite_positions() {
        let mut glyphs = Vec::new();
        word(&mut glyphs, "abc", 10.0, 10.0, 6.0);
        word(&mut glyphs, "abc", 10.0, 10.0, 6.0);
        glyphs[3].rect[0] = f32::NAN;
        let glyphs: Vec<&TextGlyph> = glyphs.iter().collect();
        assert!(!restarts_run(&glyphs, 0, 3, 10.01));
    }

    #[test]
    fn turned_lines_group_along_their_baseline() {
        // Two lines 13 pt apart, words separated by TJ offsets (no space glyphs), the second
        // line drawn first. Turned away from the quarter turns, the lines' boxes overlap.
        for degrees in [12.0, 30.0, 37.0, 45.0, 60.0, 135.0, 200.0, 300.0, -45.0] {
            let (s, c) = f64::to_radians(degrees).sin_cos();
            let second = turned(degrees, 1, 12.0, 200.0 + s * 13.0, 200.0 - c * 13.0, "[(Second) -400 (line) -400 (here)] TJ");
            let first = turned(degrees, 1, 12.0, 200.0, 200.0, "[(First) -400 (line) -400 (text)] TJ");
            assert_eq!(page_text(&(second + &first)), "First line text\nSecond line here", "{degrees}°");
        }
    }

    #[test]
    fn crossing_turned_lines_stay_apart() {
        // Lines at 30° and −30° crossing each other, their glyphs drawn alternately.
        let (up, down) = ("Upward words here", "Downward words now");
        let mut ops = String::new();
        let (s, c) = f64::to_radians(30.0).sin_cos();
        for (k, (a, b)) in up.chars().zip(down.chars()).enumerate() {
            let along = 7.2 * k as f64;
            ops += &turned(30.0, 2, 12.0, 100.0 + c * along, 150.0 + s * along, &format!("({a}) Tj"));
            ops += &turned(-30.0, 2, 12.0, 100.0 + c * along, 250.0 - s * along, &format!("({b}) Tj"));
        }
        ops += &turned(-30.0, 2, 12.0, 100.0 + c * 7.2 * 17.0, 250.0 - s * 7.2 * 17.0, "(w) Tj");
        assert_eq!(page_text(&ops), "Downward words now\nUpward words here");
    }

    #[test]
    fn text_along_a_curve_is_one_line() {
        // Courier glyphs (7.2 pt advance) set on circles of radius 90 around (200, 200): over
        // the top reading clockwise, under the bottom reading counter-clockwise, and a full ring.
        let arc = |text: &str, from: f64, clockwise: bool, radius: f64| {
            let step = (7.2 / radius).to_degrees() * if clockwise { -1.0 } else { 1.0 };
            let mut ops = String::new();
            for (k, ch) in text.chars().enumerate() {
                let a = from + step * k as f64;
                let (s, c) = a.to_radians().sin_cos();
                ops += &turned(if clockwise { a - 90.0 } else { a + 90.0 }, 2, 12.0, 200.0 + radius * c, 200.0 + radius * s, &format!("({ch}) Tj"));
            }
            ops
        };
        assert_eq!(page_text(&arc("CURVED STAMP TEXT", 150.0, true, 90.0)), "CURVED STAMP TEXT");
        assert_eq!(page_text(&arc("BOTTOM ARC WORDS", 220.0, false, 90.0)), "BOTTOM ARC WORDS");
        assert_eq!(page_text(&arc("ROUND SEAL OF THE COMPANY LIMITED", 180.0, true, 50.0)), "ROUND SEAL OF THE COMPANY LIMITED");
    }

    #[test]
    fn sheared_text_keeps_its_word_gaps() {
        // The baseline climbs at about 31° while the glyphs stay upright.
        let ops = "BT /F1 12 Tf 1 0.6 0 1 100 200 Tm [(Sheared) -400 (first)] TJ 1 0.6 0 1 100 187 Tm [(Sheared) -400 (second)] TJ ET";
        assert_eq!(page_text(ops), "Sheared first\nSheared second");
    }

    #[test]
    fn turned_text_keeps_its_place_in_the_reading_order() {
        // A turned label between a heading and its caption reads between them, as it did when
        // it was laid out with the horizontal text; a vertical body still follows its heading.
        let ops = "BT /F1 14 Tf 1 0 0 1 50 360 Tm (Heading above) Tj ET\n".to_string()
            + &turned(30.0, 1, 12.0, 80.0, 250.0, "(Turned label) Tj")
            + "BT /F1 12 Tf 1 0 0 1 50 150 Tm (Caption below) Tj ET\n";
        assert_eq!(page_text(&ops), "Heading above\nTurned label\nCaption below");
    }

    #[test]
    fn turned_glyphs_keep_their_shape_and_direction() {
        // 30° counter-clockwise on the page is 30° up from the x axis in view space (y down).
        let pdf = text_page(&(turned(30.0, 1, 20.0, 100.0, 100.0, "(Ab) Tj") + "BT /F1 20 Tf 1 0 0.5 1 100 300 Tm (Ab) Tj ET"));
        let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
        assert_eq!(text.plain_text(), "Ab\nAb");
        for g in &text.glyphs {
            assert!(g.quad.iter().flatten().chain(&g.direction).chain(&g.rect).all(|c| c.is_finite()), "{g:?}");
            let bounds = g.quad.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |r, p| {
                [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]
            });
            assert_eq!(g.rect, bounds, "the box bounds the quad");
            let [x, y] = [g.quad[1][0] - g.quad[0][0], g.quad[1][1] - g.quad[0][1]];
            assert!(
                (x * g.direction[1] - y * g.direction[0]).abs() < 1e-3 && x * g.direction[0] + y * g.direction[1] > 0.0,
                "quad[0] → quad[1] runs along the direction: {g:?}"
            );
        }
        let (skewed, turned) = (&text.glyphs[0], &text.glyphs[2]);
        assert!((turned.direction[0] - 30f32.to_radians().cos()).abs() < 1e-6 && (turned.direction[1] + 0.5).abs() < 1e-6, "{turned:?}");
        assert_eq!(skewed.direction, [1.0, 0.0]);
        let slant = skewed.quad[3][0] - skewed.quad[0][0];
        assert!((slant - 10.0).abs() < 1e-3, "the skewed glyph's upright edge leans by half its 20 pt height: {skewed:?}");
    }

    /// The operators drawing each `(text, text matrix)` run in Helvetica 12 pt, one text object per run.
    fn run_ops(runs: &[(&str, &str)]) -> String {
        runs.iter().map(|(text, tm)| format!("BT /F1 12 Tf {tm} Tm ({text}) Tj ET\n")).collect()
    }

    fn page_glyphs(ops: &str) -> PageText {
        extract_page(&text_page(ops), 0, &InterpreterSettings::default()).unwrap()
    }

    fn assert_single_word(text: PageText) {
        assert_eq!(text.plain_text(), "world");
        assert_eq!(text.glyphs.iter().map(|g| g.text.as_str()).collect::<String>(), "world");
        assert_eq!(text.line_of, [0; 5]);
        assert_eq!(text.space_before, [false; 5]);
    }

    #[test]
    fn runs_printed_over_each_other_stay_whole_at_any_angle() {
        // Two runs that start at one point (exact overprint), that start 40 pt along and 3 pt down
        // (shifted), and that are 14 pt apart (clear): at 0° and at 37°, each run comes out whole.
        let tm = |angle: f32, along: f32, down: f32| {
            let (s, c) = angle.to_radians().sin_cos();
            let (x, y) = (100.0 + along * c - down * s, 200.0 + along * s + down * c);
            format!("{c:.6} {s:.6} {:.6} {c:.6} {x:.4} {y:.4}", -s)
        };
        let (alpha, delta) = ("Alpha beta gamma", "Delta epsilon zeta");
        for angle in [0.0, 37.0] {
            for (along, down) in [(0.0, 0.0), (40.0, -3.0), (0.0, -14.0)] {
                let text = page_glyphs(&run_ops(&[(alpha, &tm(angle, 0.0, 0.0)), (delta, &tm(angle, along, down))]));
                for run in [alpha, delta] {
                    assert!(
                        !text.find(run).is_empty(),
                        "{run:?} whole at {angle}° with the second run {along} pt along and {down} pt across: {:?}",
                        text.plain_text()
                    );
                }
            }
        }
    }

    #[test]
    fn a_run_drawn_over_a_line_leaves_the_line_whole() {
        // "XX" is drawn over the middle of "beta" after the line, so it is a run of its own that
        // overlaps the line. The line must still come out as one whole line.
        let text = page_glyphs(&run_ops(&[("Alpha beta gamma", "1 0 0 1 100 200"), ("XX", "1 0 0 1 136 200")]));
        let flat = text.plain_text();
        assert!(flat.contains("Alpha beta gamma"), "{flat:?}");
        assert!(flat.contains("XX") && !flat.contains('\n'), "{flat:?}");
    }

    #[test]
    fn a_combining_mark_drawn_last_stays_with_its_letter() {
        // "ab", then a far-off "Z", then a zero-width combining mark at the start of "b". The mark
        // is a run of its own, but it has no extent, so it goes back next to "b".
        let mut v = Vec::new();
        word(&mut v, "ab", 10.0, 10.0, 6.0);
        v.push(g("Z", 200.0, 10.0, 206.0));
        v.push(g("\u{301}", 16.0, 10.0, 16.0));
        let t = layout(v);
        assert_eq!(t.glyphs.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), ["a", "b", "\u{301}", "Z"]);
    }

    #[test]
    fn runs_differing_only_in_glyph_boundaries_are_not_redraws() {
        // "ab" + "c" and "a" + "bc" have the same letters over the same places, but the glyph
        // boundaries differ, so the second is not a redraw of the first and both stay.
        let mut v = vec![g("ab", 10.0, 10.0, 16.0), g("c", 16.0, 10.0, 22.0)];
        v.push(g("Z", 200.0, 10.0, 206.0));
        v.push(g("a", 10.0, 10.0, 16.0));
        v.push(g("bc", 16.0, 10.0, 22.0));
        assert_eq!(layout(v).glyphs.len(), 5);
    }

    #[test]
    fn many_combining_marks_stay_with_their_letters() {
        // 400 letters on one line, then a zero-width mark at the start of each one, drawn after the whole line:
        // the marks form one run of their own, and each goes back next to its letter, in order.
        let letters: Vec<String> = (0..400).map(|i| char::from(b'a' + (i % 26) as u8).to_string()).collect();
        let mut v = Vec::new();
        for (i, c) in letters.iter().enumerate() {
            let x = 10.0 + i as f32 * 6.0;
            v.push(g(c, x, 10.0, x + 6.0));
        }
        for i in 0..letters.len() {
            let x = 10.0 + i as f32 * 6.0;
            v.push(g("\u{301}", x, 10.0, x));
        }
        let t = layout(v);
        let expected: Vec<&str> = letters.iter().flat_map(|c| [c.as_str(), "\u{301}"]).collect();
        assert_eq!(t.glyphs.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), expected);
        assert!(t.line_of.iter().all(|line| *line == 0));
    }

    /// `count` runs of three glyphs, each starting within 3 pt of x = 10 on one baseline, so they all overprint one
    /// another. Every glyph has a text of its own, so no glyph is taken for a fake-bold copy. With `separated`, each run
    /// follows a far-off glyph, which makes it a run of its own.
    fn overprinted_runs(count: usize, separated: bool) -> Vec<TextGlyph> {
        let mut v = Vec::new();
        for k in 0..count {
            let text: String = (0..3).map(|i| char::from_u32(0x4E00 + (3 * k + i) as u32).unwrap()).collect();
            word(&mut v, &text, 10.0 + (k * 7 % 31) as f32 * 0.1, 10.0, 6.0);
            if separated {
                let far = char::from_u32(0x2500 + k as u32).unwrap().to_string();
                v.push(g(&far, 500.0, 10.0, 506.0));
            }
        }
        v
    }

    #[test]
    fn a_dense_overprint_joins_its_runs_and_keeps_each_whole() {
        // 150 runs of three glyphs: about 100 000 overlapping pairs, far more than the sweep's budget. The segment is
        // joined as one component, so the sweep is skipped, and each run still comes out whole.
        let v = overprinted_runs(150, false);
        let refs: Vec<&TextGlyph> = v.iter().collect();
        let height = |i: usize| (refs[i].rect[3] - refs[i].rect[1]).max(0.1);
        let mut sc = Scratch { text: (0..150).map(|k| Run { start: 3 * k, end: 3 * k + 3 }).collect(), ..Default::default() };
        assert!(collide(&refs, &height, 10.0, &mut sc), "more pairs than the budget");
        assert_eq!(sc.units.len(), 1, "one component");

        let v = overprinted_runs(150, true);
        let runs: Vec<String> = (0..150).map(|k| v[4 * k..4 * k + 3].iter().map(|g| g.text.as_str()).collect()).collect();
        let t = layout(v);
        assert_eq!(t.glyphs.len(), 600);
        for run in &runs {
            assert!(!t.find(run).is_empty(), "{run:?} whole in {:?}", t.plain_text());
        }
    }

    #[test]
    fn overlapping_rows_each_start_a_word() {
        // 60 rows of three glyphs, each row starting 2 pt right of the one before and overprinting it, so every row
        // is a forced word break. The rows are separated by far-off glyphs, which makes each one a run of its own.
        // Every glyph has a text of its own, so no glyph is taken for a fake-bold copy.
        let mut v = Vec::new();
        let mut rows = Vec::new();
        for k in 0..60 {
            let text: String = (0..3).map(|i| char::from_u32(0x0400 + (3 * k + i) as u32).unwrap()).collect();
            word(&mut v, &text, 10.0 + 2.0 * k as f32, 10.0, 6.0);
            v.push(g(&char::from_u32(0x0500 + k as u32).unwrap().to_string(), 500.0, 10.0, 506.0));
            rows.push(text);
        }
        let t = layout(v);
        let expected: String = rows.join(" ");
        assert!(t.plain_text().starts_with(&expected), "{:?}", t.plain_text());
    }

    #[test]
    fn a_long_line_keeps_every_word_gap() {
        // 1500 five-letter words on one line, a word gap after each: every gap is found, and only those.
        let mut v = Vec::new();
        let mut words = Vec::new();
        for w in 0..1500 {
            let text: String = (0..5).map(|i| char::from(b'a' + ((w * 7 + i * 3) % 26) as u8)).collect();
            word(&mut v, &text, 10.0 + w as f32 * 36.0, 10.0, 6.0);
            words.push(text);
        }
        assert_eq!(layout(v).plain_text(), words.join(" "));
    }

    #[test]
    fn a_redraw_is_found_among_look_alike_runs_up_to_the_candidate_limit() {
        // 40 runs of "abc" share a text and a grid cell, but each differs from every other by more than a fifth of a
        // line in its second or third glyph, so none redraws another. A run of "abc" at the place of the newest run is
        // found, as the newest candidates are checked first. One at the place of the oldest run is 40 candidates back,
        // past the limit, so it is kept.
        let abc = |v: &mut Vec<TextGlyph>, k: usize| {
            let (b, c) = (16.0 + 2.5 * (k % 4) as f32, 22.0 + 2.5 * (k / 4) as f32);
            v.push(g("a", 10.0, 10.0, 16.0));
            v.push(g("b", b, 10.0, b + 6.0));
            v.push(g("c", c, 10.0, c + 6.0));
        };
        let far = |v: &mut Vec<TextGlyph>| {
            for _ in 0..4 {
                let x = 500.0 + 10.0 * v.len() as f32;
                v.push(g("|", x, 10.0, x + 6.0));
            }
        };
        let mut v = Vec::new();
        for k in 0..40 {
            abc(&mut v, k);
            far(&mut v);
        }
        abc(&mut v, 39);
        far(&mut v);
        assert_eq!(layout(v.clone()).glyphs.iter().filter(|g| g.text == "a").count(), 40);
        abc(&mut v, 0);
        assert_eq!(layout(v).glyphs.iter().filter(|g| g.text == "a").count(), 41);
    }

    #[test]
    fn a_run_redrawn_at_the_same_place_is_printed_once() {
        // "Hello world" drawn again 0.1 pt away (doubled text) prints once. The same words far
        // below are a separate line, as they are not a redraw.
        let ops = run_ops(&[("Hello world", "1 0 0 1 100 200"), ("Hello world", "1 0 0 1 100.1 200.1"), ("Hello world", "1 0 0 1 100 100")]);
        assert_eq!(page_text(&ops), "Hello world\nHello world");
    }

    #[test]
    fn a_redraw_is_found_past_an_earlier_run_with_the_same_key() {
        // Two "Hello" runs share a text and a grid cell but do not redraw each other: the second is
        // tracked wider. A third run redraws the second and is dropped. A far-off "Z" keeps each run
        // a segment of its own.
        let mut v = Vec::new();
        word(&mut v, "Hello", 10.0, 10.0, 6.0);
        v.push(g("Z", 200.0, 10.0, 206.0));
        word(&mut v, "Hello", 10.5, 10.0, 9.0);
        v.push(g("Z", 300.0, 10.0, 306.0));
        word(&mut v, "Hello", 10.6, 10.0, 9.0);
        let t = layout(v);
        assert_eq!(t.glyphs.iter().filter(|g| g.text == "H").count(), 2, "{:?}", t.plain_text());
        assert_eq!(t.glyphs.len(), 12);
    }

    #[test]
    fn runs_that_only_touch_or_overlap_slightly_still_join() {
        // "beta" is drawn first, then "Alpha" to its left: 2 pt apart (touching) or 2 pt overlapping
        // (negative spacing). Both come out as one line in reading order. Where the word space goes
        // for reversed fragments is not asserted here.
        let mut v = Vec::new();
        word(&mut v, "beta", 42.0, 10.0, 6.0);
        word(&mut v, "Alpha", 10.0, 10.0, 6.0);
        let t = layout(v);
        assert_eq!(t.plain_text().replace(' ', ""), "Alphabeta");
        assert!(t.line_of.iter().all(|line| *line == 0));
        let mut v = Vec::new();
        word(&mut v, "beta", 38.0, 10.0, 6.0);
        word(&mut v, "Alpha", 10.0, 10.0, 6.0);
        let t = layout(v);
        assert_eq!(t.plain_text(), "Alphabeta");
    }

    /// A three-row table, "Name Qty / Apple 12 / Pear 7": the columns sit far enough apart that
    /// reading order takes one column, then the other.
    fn table() -> PageText {
        let mut v = Vec::new();
        for (row, (name, qty)) in [("Name", "Qty"), ("Apple", "12"), ("Pear", "7")].into_iter().enumerate() {
            let y = 10.0 + row as f32 * 14.0;
            word(&mut v, name, 10.0, y, 6.0);
            word(&mut v, qty, 100.0, y, 6.0);
        }
        layout(v)
    }

    /// Issue #740: a column selection takes only what is inside the rectangle.
    #[test]
    fn a_column_selection_takes_one_table_column() {
        let t = table();
        assert_eq!(t.plain_text(), "Name\nApple\nPear\nQty\n12\n7", "reading order is column by column");
        let qty = t.glyphs_in([95.0, 0.0, 130.0, 60.0]);
        assert_eq!(t.column_text(&qty), "Qty\n12\n7");
        assert_eq!(t.glyph_rects(&qty).len(), 3, "one highlight per row");
        // Corners in either order.
        assert_eq!(t.glyphs_in([130.0, 60.0, 95.0, 0.0]), qty);
    }

    /// Across columns the text comes out row by row, cells separated by tabs.
    #[test]
    fn a_column_selection_across_columns_reads_row_by_row() {
        let t = table();
        let all = t.glyphs_in([0.0, 0.0, 200.0, 60.0]);
        assert_eq!(t.column_text(&all), "Name\tQty\nApple\t12\nPear\t7");
        // The middle row only, and only the start of "Apple": centres decide.
        let part = t.glyphs_in([0.0, 25.0, 31.0, 33.0]);
        assert_eq!(t.column_text(&part), "Appl");
        assert_eq!(t.glyph_rects(&part), vec![[10.0, 24.0, 34.0, 34.0]]);
    }

    /// A rectangle that misses every glyph, or is not a number, selects nothing.
    #[test]
    fn an_empty_or_degenerate_column_selection_is_empty() {
        let t = table();
        for r in [[300.0, 300.0, 400.0, 400.0], [f32::NAN, 0.0, 200.0, 60.0], [50.0, 50.0, 50.0, 50.0]] {
            let none = t.glyphs_in(r);
            assert!(none.is_empty(), "{r:?}");
            assert_eq!(t.column_text(&none), "");
            assert!(t.glyph_rects(&none).is_empty());
        }
        assert_eq!(PageText::default().column_text(&[0, 7, usize::MAX]), "", "indices past the text are ignored");
    }

    /// Right-to-left rows put their cells right to left.
    #[test]
    fn right_to_left_rows_read_right_to_left() {
        let mut v = Vec::new();
        // "שלום" (right) and "עולם" (left), each drawn visually.
        word(&mut v, "םולש", 100.0, 10.0, 6.0);
        word(&mut v, "םלוע", 10.0, 10.0, 6.0);
        let t = layout(v);
        let all = t.glyphs_in([0.0, 0.0, 200.0, 30.0]);
        assert_eq!(t.column_text(&all), "שלום\tעולם");
    }

    /// Vertical lines (CJK vertical writing) come out one per row, in reading order.
    #[test]
    fn vertical_lines_keep_reading_order() {
        // Written downwards: the em box's edge towards the next line is its left one.
        let cell = |t: &str, x: f32, y: f32| TextGlyph {
            text: t.into(),
            rect: [x, y, x + 10.0, y + 10.0],
            quad: [[x, y], [x, y + 10.0], [x + 10.0, y + 10.0], [x + 10.0, y]],
            direction: [0.0, 1.0],
        };
        let t = PageText {
            // Right column 日本 first, then the left column 語文, each top to bottom.
            glyphs: vec![cell("日", 40.0, 10.0), cell("本", 40.0, 20.0), cell("語", 20.0, 10.0), cell("文", 20.0, 20.0)],
            line_of: vec![0, 0, 1, 1],
            space_before: vec![false; 4],
        };
        let all = t.glyphs_in([0.0, 0.0, 60.0, 40.0]);
        assert_eq!(t.column_text(&all), "日本\n語文");
    }

    /// An embedded TrueType font with no `/Encoding`, flagged `flags`, its `/ToUnicode` (if
    /// any) mapping only printable ASCII, as OCR engines write the text layer of a scan.
    fn ocr_layer_text(flags: i64, ascii_to_unicode: bool) -> String {
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.7");
        let program = doc.add_object(Stream::new(dictionary! {}, include_bytes!("../../../assets/fonts/Inter-Regular.ttf").to_vec()));
        let descriptor =
            doc.add_object(dictionary! { "Type" => "FontDescriptor", "FontName" => "Inter-Regular", "Flags" => flags, "FontFile2" => program });
        let mut font = dictionary! { "Type" => "Font", "Subtype" => "TrueType", "BaseFont" => "Inter-Regular", "FontDescriptor" => descriptor };
        if ascii_to_unicode {
            let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Ascii def /CMapType 2 def 1 begincodespacerange <00> <FF> endcodespacerange 1 beginbfrange <20> <7E> <0020> endbfrange endcmap CMapName currentdict /CMap defineresource pop end end";
            font.set("ToUnicode", doc.add_object(Stream::new(dictionary! {}, cmap.to_vec())));
        }
        let font = doc.add_object(font);
        // WinAnsi bytes: "Rückfragen für Überhitzungsschutz: können, weiß, Café".
        let mut ops = b"BT /F1 14 Tf 1 0 0 1 20 200 Tm (".to_vec();
        ops.extend_from_slice(b"R\xfcckfragen f\xfcr \xdcberhitzungsschutz: k\xf6nnen, wei\xdf, Caf\xe9");
        ops.extend_from_slice(b") Tj ET");
        let content = doc.add_object(Stream::new(dictionary! {}, ops));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(400), Object::Integer(400)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content });
        doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
        extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap().plain_text()
    }

    #[test]
    fn truetype_without_an_encoding_reads_its_codes_as_win_ansi() {
        // Café: 0xE9 is /Oslash in StandardEncoding, so reading the codes as Standard first and
        // WinAnsi only for the gaps would still give "CafØ".
        let expected = "Rückfragen für Überhitzungsschutz: können, weiß, Café";
        assert_eq!(ocr_layer_text(4, false), expected, "symbolic, no /ToUnicode");
        assert_eq!(ocr_layer_text(32, true), expected, "nonsymbolic, /ToUnicode for ASCII only");
    }
}
