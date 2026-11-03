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
use std::collections::{BTreeMap, HashMap, HashSet};

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
    /// otherwise be found first (#3693). Failing that, the nearest bounding box wins, vertical
    /// distance weighing more, and a tie goes to the glyph whose centre is closer. A point level
    /// with some text prefers the nearest glyph of that row, so past the end of a short table cell
    /// a longer cell in the next row does not win; but a point in the leading right beside another
    /// line, parted by a gutter (see [`Self::gutter_between`]) from the row level with it, takes
    /// that line: between two lines of one column, it is not taken by another column's line that
    /// different baselines put level. Nor is a point beside a barcode drawn with a font taken by
    /// the code's row level with it.
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
        // (glyph, distance, centre distance). `total_cmp` ranks NaN last, so a glyph with a broken
        // box never wins over a real one.
        let mut on_row: Option<(usize, f32, f32)> = None;
        let mut any: Option<(usize, f32, f32)> = None;
        let closer = |best: Option<(usize, f32, f32)>, d: f32, c: f32| best.is_none_or(|(_, b, bc)| d.total_cmp(&b).then(c.total_cmp(&bc)).is_lt());
        for (i, g) in self.glyphs.iter().enumerate() {
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
            let c = centre_distance(g);
            if dy == 0.0 && closer(on_row, dx, c) {
                on_row = Some((i, dx, c));
            }
            // Prefer the same line: vertical distance weighs more.
            let d = dx + dy * 3.0;
            if closer(any, d, c) {
                any = Some((i, d, c));
            }
        }
        match (on_row, any) {
            // The point sits in the leading right beside another line (within half its height), and
            // a gutter parts it from the row level with it: that row is another column's.
            (Some((r, _, _)), Some((i, _, _))) if r != i => {
                let (Some(g), Some(n)) = (self.glyphs.get(r), self.glyphs.get(i)) else { return Some(r) };
                let beside = (n.rect[1] - y).max(y - n.rect[3]) <= (n.rect[3] - n.rect[1]) * 0.5;
                let h = g.rect[3] - g.rect[1];
                // From the far end of the row's line, so its column's shorter lines count too.
                let toward_right = x > g.rect[2];
                let from = self.line_span(r).map_or(if toward_right { g.rect[2] } else { g.rect[0] }, |(l0, l1)| if toward_right { l0 } else { l1 });
                let reach = h * GUTTER_ROWS;
                // A barcode drawn with a font has a row at every height beside it; the point is only
                // on one of them when it is over the code.
                let off_barcode = self.line_span(r).is_some_and(|(r0, r1)| !(r0..=r1).contains(&x))
                    && self.line_of.get(r).is_some_and(|l| self.barcode_lines().contains(l));
                Some(if off_barcode || (beside && self.gutter_between(from, x, y - reach, y + reach, h)) { i } else { r })
            }
            (on_row, any) => on_row.or(any).map(|(i, _, _)| i),
        }
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

    /// What a drag from glyph `anchor` to glyph `head` selects by visual rows, as Preview selects
    /// it, where reading order between them would not follow the rows: on one row, the row's
    /// glyphs from one end to the other, as [`Self::glyphs_in`] takes them; over several, the rest
    /// of the top row from the upper end, the rows between and the bottom row up to the lower end.
    ///
    /// A table reads column by column, so a drag across its columns would take the first column
    /// down to the drag's end and the next ones from the top, and a row whose cells came apart
    /// reads its last cells after the table. Rows are used when reading order strays above or
    /// below the rows the drag covers, or when the drag ends in another column than it started
    /// in: the two glyphs' lines don't overlap side by side, or the pointer, at `head_x` (view
    /// space) when known, ended past the end of both of them. `None` otherwise
    /// (running text, a drag down one article or table column): the reading-order range is the
    /// answer.
    pub fn visual_between(&self, anchor: usize, head: usize, head_x: Option<f32>) -> Option<Vec<usize>> {
        let (a, h) = (self.glyphs.get(anchor)?, self.glyphs.get(head)?);
        if self.line_of.get(anchor)? == self.line_of.get(head)? {
            return None;
        }
        // Rows mean nothing in vertical writing or turned text: two vertical columns at one
        // height are not a row. Right-to-left rows are still rows.
        let horizontal = |g: &TextGlyph| g.direction[1].abs() <= WORD_GAP;
        if !horizontal(a) || !horizontal(h) {
            return None;
        }
        let centre = |g: &TextGlyph| (g.rect[1] + g.rect[3]) / 2.0;
        let (upper, lower) = if centre(a) <= centre(h) { (a, h) } else { (h, a) };
        let (top, bottom) = (upper.rect[1].min(lower.rect[1]), upper.rect[3].max(lower.rect[3]));
        let range = self.glyphs.get(anchor.min(head)..=anchor.max(head))?;
        let strays = !range.iter().all(|g| (top..=bottom).contains(&centre(g)));
        // One row: each one's centre lies within the other's height.
        let within = |y: f32, g: &TextGlyph| (g.rect[1]..=g.rect[3]).contains(&y);
        let one_row = within(centre(upper), lower) && within(centre(lower), upper);
        let spans = self.line_span(anchor).zip(self.line_span(head));
        // From the anchor to where the pointer ended (or the head glyph), across a gutter no text
        // of these rows crosses: another column. Word gaps in a column, however wide a justified
        // line spreads them, are crossed by the other rows' text.
        // From the far end of the anchor's line, so its column's other lines count wherever along
        // the line the drag started.
        let to_x = head_x.unwrap_or((h.rect[0] + h.rect[2]) / 2.0);
        let a_x = (a.rect[0] + a.rect[2]) / 2.0;
        let from = spans.map_or(a_x, |((a0, a1), _)| if to_x >= a_x { a0 } else { a1 });
        let strip = self.gutter_strip(from, to_x, top, bottom, a.rect[3] - a.rect[1]);
        let other_column = strip.is_some();
        if !strays && (one_row || !other_column) {
            return None;
        }
        // From one column of running text into the next, reading order carries the text on (the
        // rest of one column, then the start of the next); rows would interleave the columns. One
        // row shows too little of the columns, so a drag along one looks at the rows around it.
        let lh = a.rect[3] - a.rect[1];
        let (near_top, near_bottom) = if one_row { (top - lh * GUTTER_ROWS, bottom + lh * GUTTER_ROWS) } else { (top, bottom) };
        let strip = if one_row { self.gutter_strip(from, to_x, near_top, near_bottom, lh) } else { strip };
        if let Some(strip) = strip
            && self.running_text_beside(strip, from.min(to_x), from.max(to_x), near_top, near_bottom)
        {
            return None;
        }
        let picked = if one_row {
            self.glyphs_in([a.rect[0].min(h.rect[0]), top, a.rect[2].max(h.rect[2]), bottom])
        } else {
            // Rows run left to right from the upper end; other writing keeps reading order.
            let ltr = |g: &TextGlyph| g.direction[0] > 0.0 && g.direction[1].abs() <= WORD_GAP && !g.text.chars().any(is_rtl);
            if !ltr(upper) || !ltr(lower) {
                return None;
            }
            // The rows take only lines reaching into the columns the drag went through: none left
            // of everywhere it has been (the column beside a right-hand one), and, when it stayed
            // in one column (reading order only strayed, as when a justified line's last word is
            // read late), none right of it either. Across columns, rows run on to their end.
            let ((a0, a1), (b0, b1)) = spans?;
            let pointer = head_x.filter(|x| x.is_finite());
            let left = a0.min(b0).min(pointer.unwrap_or(f32::INFINITY));
            let right = if other_column { f32::INFINITY } else { a1.max(b1).max(pointer.unwrap_or(f32::NEG_INFINITY)) };
            let mut lines: BTreeMap<u32, (f32, f32)> = BTreeMap::new();
            for (g, &l) in self.glyphs.iter().zip(&self.line_of) {
                if (top..=bottom).contains(&centre(g)) && !g.text.trim().is_empty() {
                    let span = lines.entry(l).or_insert((f32::INFINITY, f32::NEG_INFINITY));
                    *span = (span.0.min(g.rect[0]), span.1.max(g.rect[2]));
                }
            }
            let in_column = |l: u32| lines.get(&l).is_some_and(|&(l0, l1)| l0.max(left) < l1.min(right));
            let wanted = |(g, &l): (&TextGlyph, &u32)| {
                let (cx, cy) = ((g.rect[0] + g.rect[2]) / 2.0, centre(g));
                let on_top_row = cy <= upper.rect[3];
                let on_bottom_row = cy >= lower.rect[1];
                (top..=bottom).contains(&cy) && (!on_top_row || cx >= upper.rect[0]) && (!on_bottom_row || cx <= lower.rect[2]) && in_column(l)
            };
            self.glyphs.iter().zip(&self.line_of).enumerate().filter(|(_, gl)| wanted(*gl)).map(|(i, _)| i).collect()
        };
        // The same glyphs as reading order, in running text: its text is the better copy. In a
        // table (pieces side by side on a row) the rows give the copy its row-by-row order.
        let (lo, hi) = (anchor.min(head), anchor.max(head));
        let same = picked.first() == Some(&lo) && picked.last() == Some(&hi) && picked.len() == hi - lo + 1;
        if same && !self.side_by_side(&picked) {
            return None;
        }
        // Across columns, nearly every row of a table has cells side by side; a column of running
        // text beside a sidebar or a figure caption has many rows with nothing beside them. Rows
        // would interleave the two, so reading order is kept.
        if other_column && !one_row {
            // The whole rows of the band, not just the glyphs picked: the bottom row is cut off at
            // the pointer and may show a single cell.
            let band: Vec<usize> =
                (0..self.glyphs.len()).filter(|&i| self.glyphs.get(i).is_some_and(|g| (top..=bottom).contains(&centre(g)))).collect();
            let (multi, rows) = self.cell_rows(&band);
            if (multi as f32) < rows as f32 * ROW_CELLS {
                return None;
            }
        }
        (!picked.is_empty()).then_some(picked)
    }

    /// Whether two of the lines `glyphs` fall on sit side by side on one row, as table cells do.
    /// Only the first [`SIDE_BY_SIDE_LINES`] lines are compared.
    fn side_by_side(&self, glyphs: &[usize]) -> bool {
        let mut lines: Vec<(u32, [f32; 4])> = Vec::new();
        for &i in glyphs {
            let (Some(g), Some(&l)) = (self.glyphs.get(i), self.line_of.get(i)) else { continue };
            if let Some((last, bb)) = lines.last_mut()
                && *last == l
            {
                union(bb, &g.rect);
            } else if lines.len() >= SIDE_BY_SIDE_LINES {
                break;
            } else {
                lines.push((l, g.rect));
            }
        }
        lines.iter().enumerate().any(|(k, (_, a))| {
            lines.iter().skip(k + 1).any(|(_, b)| {
                let (ca, cb) = ((a[1] + a[3]) / 2.0, (b[1] + b[3]) / 2.0);
                (b[1]..=b[3]).contains(&ca) && (a[1]..=a[3]).contains(&cb)
            })
        })
    }

    /// The visual rows of the visible glyphs `glyphs` (by vertical position, whichever lines they
    /// belong to), as how many have two or more cells (a gap of [`CELL_GAP`] line heights between
    /// glyphs) and how many there are. A statement row written as one long line with wide gaps
    /// counts as cells, as a table row of separate lines does.
    fn cell_rows(&self, glyphs: &[usize]) -> (usize, usize) {
        let mut ink: Vec<(f32, f32, f32, f32)> = glyphs
            .iter()
            .filter_map(|&i| self.glyphs.get(i))
            .filter(|g| !g.text.trim().is_empty())
            .map(|g| ((g.rect[1] + g.rect[3]) / 2.0, g.rect[0], g.rect[2], g.rect[3] - g.rect[1]))
            .collect();
        ink.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut rows, mut multi) = (0usize, 0usize);
        let mut start = 0;
        while start < ink.len() {
            // A row runs while the centres stay within 0.6 line heights of the row's first glyph.
            let (cy0, h0) = (ink[start].0, ink[start].3.max(0.1));
            let end = start + ink[start..].iter().take_while(|g| (g.0 - cy0).abs() <= h0 * 0.6).count().max(1);
            let mut row: Vec<(f32, f32)> = ink[start..end].iter().map(|g| (g.1, g.2)).collect();
            row.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut reach = f32::NEG_INFINITY;
            let gap = row.iter().any(|&(x0, x1)| {
                let wide = reach.is_finite() && x0 - reach > h0 * CELL_GAP;
                reach = reach.max(x1);
                wide
            });
            rows += 1;
            multi += usize::from(gap);
            start = end;
        }
        (multi, rows)
    }

    /// Whether a column gutter lies between `x0` and `x1` (either order) in the rows whose
    /// centres lie between `top` and `bottom`: a vertical strip at least `width` wide that no
    /// glyph of those rows crosses, with at least two lines of text on each side of it between
    /// `x0` and `x1`, or beyond either when the strip reaches it (columns' baselines need not
    /// line up). The gap a justified line leaves between two words has only that line beyond it.
    fn gutter_between(&self, x0: f32, x1: f32, top: f32, bottom: f32, width: f32) -> bool {
        self.gutter_strip(x0, x1, top, bottom, width).is_some()
    }

    /// [`Self::gutter_between`]'s gutter, as its left and right edges.
    fn gutter_strip(&self, x0: f32, x1: f32, top: f32, bottom: f32, width: f32) -> Option<(f32, f32)> {
        let (lo, hi) = (x0.min(x1), x0.max(x1));
        if !(hi - lo >= width && width > 0.0) {
            return None;
        }
        let centre = |g: &TextGlyph| (g.rect[1] + g.rect[3]) / 2.0;
        // The rows' glyphs as (centre, left, right), top to bottom.
        let mut rows: Vec<(f32, f32, f32)> =
            self.glyphs.iter().filter(|g| (top..=bottom).contains(&centre(g))).map(|g| (centre(g), g.rect[0], g.rect[2])).collect();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Two lines of text on each side of the strip g0..g1: glyphs whose centres lie within half
        // a width of the previous one's (rows is sorted by centre) are on its line.
        let lines_beside = |side: &dyn Fn(f32, f32) -> bool| {
            let (mut lines, mut last) = (0usize, f32::NEG_INFINITY);
            for &(cy, _, _) in rows.iter().filter(|&&(_, a, b)| side(a, b)) {
                if cy - last > width * 0.5 {
                    lines += 1;
                }
                last = cy;
            }
            lines
        };
        // Text within lo..hi counts, so a column beyond the span parts nothing inside it; but a
        // strip running on to either end, where the point is out in a gap, is a gutter if a
        // column lies beyond it.
        let parted = |g0: f32, g1: f32| {
            let left = if g0 <= lo { f32::NEG_INFINITY } else { lo };
            let right = if g1 >= hi { f32::INFINITY } else { hi };
            lines_beside(&|_, b| (left..=g0).contains(&b)) >= 2 && lines_beside(&|a, _| (g1..=right).contains(&a)) >= 2
        };
        let mut spans: Vec<(f32, f32)> = rows.iter().filter(|&&(_, a, b)| b > lo && a < hi).map(|&(_, a, b)| (a, b)).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut covered = lo;
        for (s0, s1) in spans {
            if s0 - covered >= width && parted(covered, s0) {
                return Some((covered, s0));
            }
            covered = covered.max(s1);
        }
        (hi - covered >= width && parted(covered, hi)).then_some((covered, hi))
    }

    /// Whether the lines between `top` and `bottom` on each side of the gutter `g0..g1` are
    /// running text: one column a side (no two lines side by side), most of its lines (at least
    /// half: paragraphs' first and last lines fall short) starting and ending at about the same
    /// place, at least half as wide as the gutter and [`RUNNING_TEXT_EMS`] line heights wide.
    /// Table cells, terms and form labels are short or ragged beside a wide gap, or sit several
    /// to a row. Only the lines ending after `lo` on the left, and starting before `hi`
    /// (or right after the gutter) on the right, count.
    fn running_text_beside(&self, (g0, g1): (f32, f32), lo: f32, hi: f32, top: f32, bottom: f32) -> bool {
        let mut lines: BTreeMap<u32, [f32; 4]> = BTreeMap::new();
        for (g, &l) in self.glyphs.iter().zip(&self.line_of) {
            if (top..=bottom).contains(&((g.rect[1] + g.rect[3]) / 2.0)) && !g.text.trim().is_empty() {
                lines.entry(l).and_modify(|r| union(r, &g.rect)).or_insert(g.rect);
            }
        }
        let gutter = g1 - g0;
        let column = |side: Vec<[f32; 4]>| {
            let n = side.len();
            if n < 2 {
                return false;
            }
            let (x0, x1) = side.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), r| (a.min(r[0]), b.max(r[2])));
            let w = x1 - x0;
            let one_a_row = side.iter().enumerate().all(|(k, a)| {
                side.iter().skip(k + 1).all(|b| {
                    let (ca, cb) = ((a[1] + a[3]) / 2.0, (b[1] + b[3]) / 2.0);
                    !((b[1]..=b[3]).contains(&ca) || (a[1]..=a[3]).contains(&cb))
                })
            });
            let starts = side.iter().filter(|r| r[0] <= x0 + w * RUNNING_TEXT_RAG).count();
            let ends = side.iter().filter(|r| r[2] >= x1 - w * RUNNING_TEXT_RAG).count();
            let tallest = side.iter().map(|r| r[3] - r[1]).fold(0.0f32, f32::max);
            one_a_row && w >= gutter * 0.5 && w >= tallest * RUNNING_TEXT_EMS && starts * 2 >= n && ends * 2 >= n
        };
        let reach = hi.max(g1 + gutter);
        let left: Vec<[f32; 4]> = lines.values().filter(|r| r[2] <= g0 && r[2] >= lo).copied().collect();
        let right: Vec<[f32; 4]> = lines.values().filter(|r| r[0] >= g1 && r[0] <= reach).copied().collect();
        left.len() <= SIDE_BY_SIDE_LINES && right.len() <= SIDE_BY_SIDE_LINES && column(left) && column(right)
    }

    /// The lines that are rows of a barcode drawn with a font (a QR or Aztec code), as the layout
    /// finds them: such rows read after all other text.
    fn barcode_lines(&self) -> HashSet<u32> {
        let mut lines: Vec<(u32, usize, [f32; 4])> = Vec::new();
        for (g, &l) in self.glyphs.iter().zip(&self.line_of) {
            match lines.last_mut() {
                Some((id, n, bb)) if *id == l => {
                    *n += 1;
                    union(bb, &g.rect);
                }
                _ => lines.push((l, 1, g.rect)),
            }
        }
        let rows: Vec<(usize, [f32; 4])> = lines.iter().map(|&(_, n, bb)| (n, bb)).collect();
        lines.iter().zip(barcode_rows(&rows)).filter(|(_, barcode)| *barcode).map(|((l, _, _), _)| *l).collect()
    }

    /// The left and right edges of the ink of the line glyph `i` is on: spaces padding it out (a
    /// heading indented with spaces past a barcode beside it) are not where its text is. A line of
    /// spaces only spans its spaces.
    fn line_span(&self, i: usize) -> Option<(f32, f32)> {
        let (first, last) = self.line_at(i)?;
        let line = self.glyphs.get(first..=last)?;
        let span = |inked: bool| {
            line.iter()
                .filter(|g| !inked || !g.text.trim().is_empty())
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(x0, x1), g| (x0.min(g.rect[0]), x1.max(g.rect[2])))
        };
        Some(Some(span(true)).filter(|(x0, x1)| x0 <= x1).unwrap_or_else(|| span(false)))
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

/// Line heights above and below a point (or a one-row drag) looked over for a gutter.
const GUTTER_ROWS: f32 = 4.0;
/// The share of a row selection's visual rows that must have cells side by side for the rows to be
/// a table's: a column of running text beside a sidebar has such cells on only some of its rows.
const ROW_CELLS: f32 = 0.75;
/// A gap of this many line heights between two glyphs of a row parts two cells (a word gap is
/// about a third of a line height).
const CELL_GAP: f32 = 2.0;
/// Running text is at least this many line heights wide; terms, labels and narrow cells are not.
const RUNNING_TEXT_EMS: f32 = 8.0;
/// The share of a column's width its running text's lines may fall short of its edges by.
const RUNNING_TEXT_RAG: f32 = 0.2;
/// Lines a selection compares pairwise to tell a table from running text.
const SIDE_BY_SIDE_LINES: usize = 256;

use hayro::hayro_interpret::TransformExt;

mod device;
mod layout;
mod order;

use device::WORD_GAP;
#[cfg(test)]
use device::{MAX_REDRAW_TEXT, REDRAW_TEXT_BYTES, Replaced, TextDevice};
pub(crate) use device::{extract_page, extract_page_filtered};
pub use layout::layout;
#[cfg(test)]
use layout::{from_upright, to_upright};
use layout::{is_rtl, union};
use order::barcode_rows;
#[cfg(test)]
use order::{Run, Scratch, Seg, collide, restarts_run};

#[cfg(test)]
mod tests;
