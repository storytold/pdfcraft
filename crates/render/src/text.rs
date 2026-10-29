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
#[cfg(test)]
use order::{Run, Scratch, collide, restarts_run};

#[cfg(test)]
mod tests;
