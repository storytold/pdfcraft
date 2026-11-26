use super::device::WORD_GAP;
use super::layout::is_rtl;
use super::{HashMap, TextGlyph};

/// Glyphs `start..end` of the glyph list [`arrange`] lays out, drawn one after another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Run {
    pub(super) start: usize,
    pub(super) end: usize,
}

impl Run {
    pub(super) fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    fn range(self) -> std::ops::Range<usize> {
        self.start..self.end
    }
}

/// Marks an absent link in a union-find or run list.
pub(super) const NONE: usize = usize::MAX;

/// A segment: runs drawn on one baseline, placed together. Its glyph order is worked out when it is emitted, so a
/// segment holds run ranges and geometry only.
pub(super) struct Seg {
    /// The first run. Merged segments keep their other runs in `extra`.
    pub(super) first: Run,
    /// The other runs of a merged segment. Boxed, and absent for most segments.
    extra: Option<Box<SegExtra>>,
    pub(super) bbox: [f32; 4],
    pub(super) h: f32,
    /// The right edge of its visible glyphs; the box may reach further with trailing spaces.
    /// Negative infinity when every glyph is blank.
    pub(super) ink: f32,
    /// A row of a barcode drawn with a font (see [`mark_barcode_rows`]): it joins no other text.
    pub(super) barcode: bool,
}

#[derive(Default)]
struct SegExtra {
    more: Vec<Run>,
}

/// The right edge of `g`'s ink, for [`Seg::ink`]: negative infinity for a blank glyph.
pub(super) fn ink_of(g: &TextGlyph) -> f32 {
    if g.text.trim().is_empty() { f32::NEG_INFINITY } else { g.rect[2] }
}

/// Whether `(script_bbox, script_h)` is a superscript or subscript on `(host_bbox, host_h)`.
pub(super) fn is_script_on(host_bbox: &[f32; 4], host_h: f32, script_bbox: &[f32; 4], script_h: f32) -> bool {
    let host_cy = (host_bbox[1] + host_bbox[3]) / 2.0;
    let script_cy = (script_bbox[1] + script_bbox[3]) / 2.0;
    let script_w = script_bbox[2] - script_bbox[0];
    let host_w = host_bbox[2] - host_bbox[0];
    let smaller = script_h >= host_h * 0.45 && script_w < host_h * 3.0 && (script_h < host_h * 0.9 || (script_h <= host_h && host_w > script_w));
    if !smaller {
        return false;
    }
    if script_cy < host_cy {
        // Superscript: raised above the host baseline, with its bottom inside the host line.
        host_cy - script_cy >= host_h * 0.12
            && host_cy - script_cy < host_h * 0.7
            && host_bbox[3] - script_bbox[3] >= host_h * 0.15
            && script_bbox[3] > host_bbox[1] + host_h * 0.25
    } else {
        // Subscript: lowered below the host baseline, with its centre still inside the host line.
        script_cy - host_cy >= host_h * 0.12
            && script_cy - host_cy < host_h * 0.5
            && script_bbox[3] > host_bbox[3]
            && host_bbox[3] > script_bbox[1] + script_h * 0.5
            && script_bbox[1] >= host_bbox[1] - script_h * 0.2
    }
}

impl Seg {
    pub(super) fn new(first: Run, bbox: [f32; 4], h: f32, ink: f32) -> Self {
        Seg { first, extra: None, bbox, h, ink, barcode: false }
    }

    /// Whether this segment is a table cell padded with spaces towards `next`, the next cell on
    /// its baseline: a report generator lays rows out in fixed-width fields this way (see
    /// [`PADDED_FIELD`]).
    pub(super) fn padded_towards(&self, next: &Seg, h: f32) -> bool {
        let gap = next.bbox[0] - self.bbox[2];
        self.ink.is_finite() && self.bbox[2] - self.ink >= h * PADDED_FIELD && (0.0..h * PADDED_REACH).contains(&gap)
    }

    pub(super) fn cy(&self) -> f32 {
        (self.bbox[1] + self.bbox[3]) / 2.0
    }

    fn runs(&self) -> impl Iterator<Item = Run> + '_ {
        std::iter::once(self.first).chain(self.extra.iter().flat_map(|e| e.more.iter().copied()))
    }

    /// Extend `self`'s box and height with `(bbox, h)`. A smaller script (superscript or
    /// subscript) widens the line horizontally without pulling its vertical baseline band up or
    /// down; when a line begins with a script and continues with taller body text, the body text's
    /// vertical band becomes the line's.
    pub(super) fn extend_box(&mut self, bbox: &[f32; 4], h: f32) {
        if h > self.h * 1.15 || is_script_on(bbox, h, &self.bbox, self.h) {
            self.bbox[1] = bbox[1];
            self.bbox[3] = bbox[3];
        } else if !(self.h > h * 1.15 || is_script_on(&self.bbox, self.h, bbox, h)) {
            self.bbox[1] = self.bbox[1].min(bbox[1]);
            self.bbox[3] = self.bbox[3].max(bbox[3]);
        }
        self.bbox[0] = self.bbox[0].min(bbox[0]);
        self.bbox[2] = self.bbox[2].max(bbox[2]);
        self.h = self.h.max(h);
    }

    pub(super) fn merge(&mut self, other: Seg) {
        self.extend_box(&other.bbox, other.h);
        self.ink = self.ink.max(other.ink);
        let extra = self.extra.get_or_insert_with(Default::default);
        extra.more.push(other.first);
        if let Some(o) = other.extra {
            extra.more.extend(o.more);
        }
    }
}

/// A barcode font (a QR or Aztec code on an invoice, as on every Mexican CFDI) draws its symbol as
/// rows of glyphs stacked flush with no leading: at least this many rows ...
const BARCODE_ROWS: usize = 4;
/// ... of at least this many glyphs each.
const BARCODE_ROW_GLYPHS: usize = 8;

/// Mark the segments that are rows of a barcode drawn with a font (see [`barcode_rows`]). Text
/// beside the code sits on the same baselines and would otherwise join its rows, so the code's
/// rows and the text would interleave in reading order.
pub(super) fn mark_barcode_rows(segs: &mut [Seg]) {
    let rows: Vec<(usize, [f32; 4])> = segs.iter().map(|s| (if s.extra.is_none() { s.first.len() } else { 0 }, s.bbox)).collect();
    for (s, barcode) in segs.iter_mut().zip(barcode_rows(&rows)) {
        s.barcode = barcode;
    }
}

/// Which of the rows of text `rows`, each as its glyph count and box, are rows of a barcode drawn
/// with a font: runs of at least [`BARCODE_ROWS`] rows of one glyph count, height, left and right
/// edge, each starting where the one above ends. Text set solid still has lines of different
/// lengths and glyph counts.
pub(super) fn barcode_rows(rows: &[(usize, [f32; 4])]) -> Vec<bool> {
    // Positions to an eighth of a point: a barcode's rows are placed exactly.
    let q = |v: f32| (v * 8.0).round() as i64;
    let height = |bb: &[f32; 4]| bb[3] - bb[1];
    let mut stacks: HashMap<(usize, i64, i64, i64), Vec<usize>> = HashMap::new();
    for (k, (n, bb)) in rows.iter().enumerate() {
        if *n >= BARCODE_ROW_GLYPHS && bb.iter().all(|v| v.is_finite()) {
            stacks.entry((*n, q(bb[0]), q(bb[2]), q(height(bb)))).or_default().push(k);
        }
    }
    let mut barcode = vec![false; rows.len()];
    for stack in stacks.values_mut().filter(|stack| stack.len() >= BARCODE_ROWS) {
        stack.sort_by(|&a, &b| rows[a].1[1].total_cmp(&rows[b].1[1]));
        let mut start = 0;
        for k in 1..=stack.len() {
            let flush = stack.get(k).is_some_and(|&r| {
                let (above, below) = (&rows[stack[k - 1]].1, &rows[r].1);
                (below[1] - above[3]).abs() <= height(above) * 0.05
            });
            if !flush {
                if k - start >= BARCODE_ROWS {
                    for &r in &stack[start..k] {
                        barcode[r] = true;
                    }
                }
                start = k;
            }
        }
    }
    barcode
}

/// A segment whose box runs on past its last visible glyph by this many line heights of spaces is
/// a padded table cell, and the next cell on its baseline belongs to the same row if it starts
/// within [`PADDED_REACH`] line heights of the padding. Without this, a row whose padding stops
/// short of the next cell reads that cell after the whole table, and a drag along the row selects
/// everything between.
const PADDED_FIELD: f32 = 2.0;
const PADDED_REACH: f32 = 8.0;

/// Two glyphs of different runs that overlap by more than this fraction of the line height collide.
const RUN_COLLISION: f32 = 0.25;

/// A run that starts inside the runs before it by more than this fraction of the line height starts a new word.
const RUN_OVERLAP_SPACE: f32 = 0.5;

/// Earlier runs with the same key that a run is checked against for a redraw, newest first. A run past this limit is kept,
/// so a page costs at most this many checks per run, however many runs share a key.
pub(super) const REDRAW_CANDIDATES: usize = 32;

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
pub(super) struct Scratch {
    /// The glyphs of the text runs, each with its run, in x order.
    pub(super) owner: Vec<(usize, usize)>,
    /// The text runs of the segment (runs that are not all zero-width).
    pub(super) text: Vec<Run>,
    /// The glyphs of the zero-width runs, in run order.
    pub(super) marks: Vec<usize>,
    /// Union-find parents over `text`, and for each root the list of its runs through `next`.
    pub(super) parent: Vec<usize>,
    pub(super) head: Vec<usize>,
    pub(super) next: Vec<usize>,
    /// The runs of one colliding component, by left edge.
    pub(super) rows: Vec<Row>,
    /// The glyphs of one colliding component: each run's glyphs in x order, in run order.
    pub(super) group: Vec<usize>,
    /// The units to place, and their glyphs in `flat`.
    pub(super) units: Vec<Unit>,
    pub(super) flat: Vec<usize>,
    /// The current segment's glyphs in visual reading order, and its forced word breaks.
    pub(super) placed: Vec<usize>,
    pub(super) breaks: Vec<usize>,
    /// The gaps between the current segment's glyphs, and its glyphs in logical order when it has right-to-left text.
    pub(super) gaps: Vec<f32>,
    pub(super) logical: Vec<usize>,
    /// `placed` with the marks merged in, built before it is swapped in.
    pub(super) merged: Vec<usize>,
    /// The merged ink intervals of the current segment, each with the ink length before it, for `drop_covered_blanks`.
    pub(super) ink: Vec<[f32; 3]>,
    /// The ink glyphs of the current segment as (left, right, run), by left edge, for `drop_covered_blanks`.
    pub(super) ink_runs: Vec<(f32, f32, usize)>,
    /// For each glyph of the page, the glyph a combining mark drawn right after it rides on (the start of its cluster),
    /// or the glyph itself. Placement sorts clusters, so a mark keeps its content-stream place after its base (#6517).
    /// A glyph past its end is its own anchor.
    pub(super) anchor: Vec<usize>,
    /// The gap left of each glyph of `placed`, between clusters; `None` for the first glyph and for a riding mark.
    pub(super) cluster_gap: Vec<Option<f32>>,
}

/// How close, as a fraction of its height, the glyph drawn after a combining mark must start to the mark's right edge
/// for the mark to have moved the pen by its own width ([`measure_cluster_gaps`]).
const MARK_ADVANCE_TOLERANCE: f32 = 0.05;

/// Fills `sc.cluster_gap` with the gap left of each glyph of `sc.placed`, between clusters: a combining mark riding on a
/// glyph has none (no word ever starts at it), and its cluster ends at the base's right edge. A zero-advance mark's box
/// is no measure of where the pen went (`draw_glyph` widens it to its outline or half an em), and its start is only
/// the base's advance moved by a mark-positioning offset. A mark whose font gives it an advance of its own does move
/// the pen, and the glyph drawn after it starts right at its box's right edge: the cluster then ends there, or every
/// letter after such a mark would start a word.
pub(super) fn measure_cluster_gaps(glyphs: &[&TextGlyph], sc: &mut Scratch) {
    sc.cluster_gap.clear();
    let mut right = f32::NEG_INFINITY;
    for (w, &c) in sc.placed.iter().enumerate() {
        let Some(g) = glyphs.get(c) else {
            sc.cluster_gap.push(None);
            continue;
        };
        if anchor_of(&sc.anchor, c) != c {
            sc.cluster_gap.push(None);
            let tolerance = MARK_ADVANCE_TOLERANCE * (g.rect[3] - g.rect[1]).max(0.1);
            let pen_moved = c.checked_add(1).and_then(|n| glyphs.get(n)).is_some_and(|n| (n.rect[0] - g.rect[2]).abs() <= tolerance);
            if pen_moved && g.rect[2] > right {
                right = g.rect[2];
            }
        } else {
            sc.cluster_gap.push((w > 0).then_some(g.rect[0] - right));
            right = g.rect[2];
        }
    }
}

/// The glyph that glyph `i`'s cluster starts at ([`Scratch::anchor`]).
fn anchor_of(anchor: &[usize], i: usize) -> usize {
    anchor.get(i).copied().unwrap_or(i)
}

/// Orders glyphs by the x of their cluster's first glyph, keeping each cluster together and in content order.
fn by_cluster(glyphs: &[&TextGlyph], anchor: &[usize], a: usize, b: usize) -> std::cmp::Ordering {
    let (ka, kb) = (anchor_of(anchor, a), anchor_of(anchor, b));
    let x = |i: usize| glyphs.get(i).map_or(0.0, |g| g.rect[0]);
    x(ka).total_cmp(&x(kb)).then(ka.cmp(&kb)).then(a.cmp(&b))
}

/// One run of a colliding component.
pub(super) struct Row {
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
pub(super) struct Unit {
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
pub(super) fn fingerprint(glyphs: &[&TextGlyph], run: Run) -> u64 {
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
pub(super) fn redraws(glyphs: &[&TextGlyph], a: Run, b: Run) -> bool {
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
pub(super) fn restarts_run(glyphs: &[&TextGlyph], start: usize, next: usize, progress: f32) -> bool {
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
/// extent of its own, so it goes next to the glyph it starts at. A space whose advance the next glyph takes back stays
/// before that glyph ([`keep_taken_back_blanks`]).
pub(super) fn place_glyphs(glyphs: &[&TextGlyph], height: &dyn Fn(usize) -> f32, s: &Seg, sc: &mut Scratch) {
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
        sc.placed.sort_unstable_by(|a, b| by_cluster(glyphs, &sc.anchor, *a, *b));
    } else {
        collide(glyphs, height, s.h, sc);
        // Only another run can draw over a run's blanks: a single run is left as it is.
        drop_covered_blanks(glyphs, sc);
    }
    // Each mark goes before the first placed glyph whose x is greater than its own, or its letter's when it is a
    // combining mark riding on one. Marks are visited in that order, and that position never moves back, so one pass
    // over `placed` puts all of them in.
    sc.marks.sort_by(|a, b| by_cluster(glyphs, &sc.anchor, *a, *b));
    if !sc.marks.is_empty() {
        sc.merged.clear();
        let mut next = 0;
        for &m in &sc.marks {
            while next < sc.placed.len() && x0(anchor_of(&sc.anchor, sc.placed[next])) <= x0(anchor_of(&sc.anchor, m)) {
                sc.merged.push(sc.placed[next]);
                next += 1;
            }
            sc.merged.push(m);
        }
        sc.merged.extend_from_slice(&sc.placed[next..]);
        std::mem::swap(&mut sc.placed, &mut sc.merged);
    }
    // After the marks: their merge relies on `placed` being in x order, which this breaks on purpose.
    keep_taken_back_blanks(glyphs, &mut sc.placed);
}

/// How many glyphs [`keep_taken_back_blanks`] looks at past a blank: marks between it and its letter, in drawing order
/// and in x order, and glyphs without a strong direction on each side of a taken-back neutral. Each scan stops there,
/// so the pass stays linear in the line's glyphs.
const TAKEN_BACK_SCAN: usize = 8;

/// Puts a blank glyph (a space) back before the glyph drawn right after it on a line with right-to-left text, when that
/// glyph's adjustment took the blank's advance back: it starts at or left of the blank's own start and covers at least
/// half of it. Right-to-left text drawn in reverse order starts a show string with the space that ends its word
/// (ISO 32000-2 §14.8.2.5.3, whose NOTE 2 has such a space realign the typography without visible effect), and a word
/// processor pulls the next letter back over it. A blank has no ink, so its box says nothing about which side of that
/// letter it belongs on, and sorting by x split "המדינה" as "המדינ ה". The space is a word break in the character
/// stream (§14.8.2.6.2), and drawing order is the only evidence of its place.
///
/// Combining marks (bidi class NSM) drawn between the blank and its letter are passed over, and the blank, those marks
/// and the letter keep drawing order, which puts each mark after its letter once the line is reversed, as Unicode
/// stores it. Only a right-to-left letter or a neutral (punctuation, a symbol) after the blank counts: left-to-right
/// text has no reverse-order show strings, and in scanned pages' invisible text layers its spaces overlap letters at
/// random. A neutral counts only inside right-to-left text: the nearest letter or number before it, and the nearest
/// after the blank, is right-to-left (numbers count as such beside neutrals), or the line's edge on a right-to-left
/// line. A parenthesis that opens Latin text then stays
/// where x order puts it, as on a line without right-to-left text. A glyph that only touches the blank's start was
/// drawn beside it, as text set letter by letter leftwards is, and x order already reads it right. A blank that a
/// glyph placed before them also reaches over sits inside a cluster (a letter and the marks drawn on it), where its
/// place is no clearer, so it stays where x order put it.
fn keep_taken_back_blanks(glyphs: &[&TextGlyph], placed: &mut [usize]) {
    let glyph = |placed: &[usize], p: usize| placed.get(p).and_then(|&i| glyphs.get(i)).copied();
    let blank = |g: &TextGlyph| !g.text.is_empty() && g.text.chars().all(char::is_whitespace);
    let mark = |g: &TextGlyph| !g.text.is_empty() && g.text.chars().all(|c| unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM);
    // The direction a glyph gives the text around it: Some(false) for left-to-right letters, Some(true) for
    // right-to-left letters and for numbers, which read as right-to-left beside neutrals (UAX #9 N1, as in
    // `reverse_rtl`), None for a glyph without one. Nonspacing marks give none, alphabetic ones (U+0345) included: a
    // mark has the direction of the letter it sits on.
    let strong = |g: &TextGlyph| {
        let spacing = || g.text.chars().filter(|&c| unicode_bidi::bidi_class(c) != unicode_bidi::BidiClass::NSM);
        if spacing().any(|c| c.is_alphabetic() && !is_rtl(c)) {
            Some(false)
        } else if spacing().any(char::is_alphanumeric) {
            Some(true)
        } else {
            None
        }
    };
    if !placed.iter().any(|&i| glyphs.get(i).is_some_and(|g| g.text.chars().any(is_rtl))) {
        return;
    }
    // The line's direction, as `reverse_rtl` takes it. A line's edge has that direction (UAX #9 X10: sos and eos).
    let line_rtl = is_rtl_line(glyphs, placed.iter().copied());
    // Whether the nearest glyph with a strong direction from position `from` on, stepping by `step`, is right-to-left,
    // or the line is when there is none before its edge.
    let rtl_side = |placed: &[usize], from: Option<usize>, step: fn(usize) -> Option<usize>| {
        let mut p = from;
        for _ in 0..TAKEN_BACK_SCAN {
            let Some(g) = p.and_then(|p| glyph(placed, p)) else { return line_rtl };
            if let Some(rtl) = strong(g) {
                return rtl;
            }
            p = p.and_then(step);
        }
        false
    };
    for p in 1..placed.len() {
        let Some(&space) = placed.get(p) else { continue };
        let Some(s) = glyphs.get(space).copied() else { continue };
        if !blank(s) {
            continue;
        }
        // The glyph drawn after the blank, past any marks drawn between them.
        let mut next = space.checked_add(1);
        for _ in 0..TAKEN_BACK_SCAN {
            match next.and_then(|i| glyphs.get(i)) {
                Some(g) if mark(g) => next = next.and_then(|i| i.checked_add(1)),
                _ => break,
            }
        }
        let Some(ink) = next.filter(|&i| glyphs.get(i).is_some_and(|g| !mark(g))) else { continue };
        // The glyphs x order put next to the blank that were drawn after it, up to and including the letter: the
        // letter and the marks drawn between them, on either side of the blank.
        let drawn_with = |i: usize| i > space && i <= ink;
        let mut q = p;
        for _ in 0..=TAKEN_BACK_SCAN {
            match q.checked_sub(1).and_then(|k| placed.get(k)) {
                Some(&i) if drawn_with(i) => q -= 1,
                _ => break,
            }
        }
        let mut r = p;
        for _ in 0..=TAKEN_BACK_SCAN {
            match r.checked_add(1).and_then(|k| placed.get(k)) {
                Some(&i) if drawn_with(i) => r += 1,
                _ => break,
            }
        }
        if q == p || !placed.get(q..=r).is_some_and(|group| group.contains(&ink)) {
            continue;
        }
        let Some(g) = glyphs.get(ink).copied() else { continue };
        let width = s.rect[2] - s.rect[0];
        let covered = g.rect[2].min(s.rect[2]) - s.rect[0];
        let taken_back = g.rect[0] <= s.rect[0] && width > 0.0 && covered >= 0.5 * width;
        let alone = q.checked_sub(1).and_then(|b| glyph(placed, b)).is_none_or(|before| before.rect[2] <= s.rect[0]);
        // A left-to-right letter or a European digit after the blank never moves; a neutral moves inside right-to-left text.
        let direction = match strong(g) {
            _ if g.text.chars().any(|c| c.is_alphanumeric() && !is_rtl(c)) => false,
            Some(rtl) => rtl,
            None => rtl_side(placed, q.checked_sub(1), |p| p.checked_sub(1)) && rtl_side(placed, r.checked_add(1), |p| p.checked_add(1)),
        };
        if !blank(g) && direction && taken_back && alone {
            // The blank, its marks and its letter in drawing order.
            if let Some(group) = placed.get_mut(q..=r) {
                group.sort_unstable();
            }
        }
    }
}

/// Removes blank glyphs from `sc.placed` that are mostly covered by the ink of other glyphs on the line. A run of blanks
/// drawn over the digits of an earlier run (a cover headline that overprints a number with spaces) would otherwise sort
/// between the digits and show as spaces inside the number (#3172). Only other runs' ink counts: a blank its own run's
/// letters overlap (kerned into a tight word gap) is that run's word break. A blank that stands in a gap is kept. A
/// forced word break on a removed blank moves to the next glyph of its run that is kept. Each blank costs two binary
/// searches over the line's ink, and a blank that ink mostly covers then a scan of the glyphs that can reach it.
fn drop_covered_blanks(glyphs: &[&TextGlyph], sc: &mut Scratch) {
    let blank = |i: usize| glyphs[i].text.trim().is_empty();
    if !sc.placed.iter().any(|&i| blank(i)) {
        return;
    }
    let ink = &mut sc.ink;
    ink.clear();
    ink.extend(sc.placed.iter().filter(|&&i| !blank(i)).map(|&i| [glyphs[i].rect[0], glyphs[i].rect[2], 0.0]));
    ink.sort_by(|a, b| a[0].total_cmp(&b[0]));
    // Merge into disjoint intervals.
    ink.dedup_by(|cur, prev| {
        let overlaps = cur[0] <= prev[1];
        if overlaps {
            prev[1] = prev[1].max(cur[1]);
        }
        overlaps
    });
    // `iv[2]` is the ink length of all intervals before this one.
    let mut total = 0.0;
    for iv in ink.iter_mut() {
        iv[2] = total;
        total += iv[1] - iv[0];
    }
    let ink = &sc.ink;
    let covered = |l: f32, r: f32| {
        let first = ink.partition_point(|iv| iv[1] <= l);
        let end = ink.partition_point(|iv| iv[0] < r);
        let (Some(head), Some(last)) = (ink.get(first), end.checked_sub(1).and_then(|k| ink.get(k))) else { return 0.0 };
        if end <= first {
            return 0.0;
        }
        let after = ink.get(end).map_or(total, |iv| iv[2]);
        after - head[2] - (l - head[0]).max(0.0) - (last[1] - r).max(0.0)
    };
    // The ink glyphs with their runs, by left edge, and the widest of them: only a glyph starting less than that left
    // of a blank can reach it.
    let by_x = &mut sc.ink_runs;
    by_x.clear();
    by_x.extend(sc.owner.iter().filter(|&&(i, _)| !blank(i)).map(|&(i, r)| (glyphs[i].rect[0], glyphs[i].rect[2], r)));
    by_x.sort_by(|a, b| a.0.total_cmp(&b.0));
    let widest = by_x.iter().map(|g| g.1 - g.0).fold(0.0f32, f32::max);
    let by_x = &sc.ink_runs;
    // How much of [l, r] the ink of runs other than `run` covers. The intervals come in order of their left edges.
    let covered_by_others = |l: f32, r: f32, run: usize| {
        let (from, to) = (by_x.partition_point(|g| g.0 < l - widest), by_x.partition_point(|g| g.0 < r));
        let (mut sum, mut reach) = (0.0, l);
        for &(a, b, _) in by_x.get(from..to).unwrap_or_default().iter().filter(|g| g.2 != run) {
            let (a, b) = (a.max(reach), b.min(r));
            if b > a {
                sum += b - a;
                reach = b;
            }
        }
        sum
    };
    sc.merged.clear();
    for &(i, run) in &sc.owner {
        let (l, r) = (glyphs[i].rect[0], glyphs[i].rect[2]);
        if blank(i) && r - l > 0.0 && covered(l, r) > 0.5 * (r - l) && covered_by_others(l, r, run) > 0.5 * (r - l) {
            sc.merged.push(i);
        }
    }
    if sc.merged.is_empty() {
        return;
    }
    // `placed` is not sorted by index, so the dropped glyphs are sorted for the lookups.
    sc.merged.sort_unstable();
    let dropped = &sc.merged;
    sc.placed.retain(|i| dropped.binary_search(i).is_err());
    for b in sc.breaks.iter_mut() {
        if dropped.binary_search(b).is_err() {
            continue;
        }
        let x0 = |i: usize| glyphs[i].rect[0];
        let run = sc.text.iter().find(|run| run.range().contains(b));
        let kept = run.and_then(|run| {
            let left = run.range().filter(|j| dropped.binary_search(j).is_err());
            left.min_by(|a, c| x0(*a).total_cmp(&x0(*c)).then(a.cmp(c)))
        });
        *b = kept.unwrap_or(NONE);
    }
    sc.breaks.retain(|&b| b != NONE);
    sc.breaks.sort_unstable();
    sc.breaks.dedup();
    sc.merged.clear();
}

/// `place_glyphs` for a segment of two or more text runs. `h` is the segment's line height. Returns whether the segment
/// was too dense to sweep ([`COLLIDE_PAIRS_PER_GLYPH`]), in which case all its text runs form one component.
///
/// The sweep tests each glyph against the glyphs after it that start before its right edge. Its work is bounded by the
/// budget, so a page costs O(glyphs) pair tests plus the sorts. A dense segment is joined with one pass over its runs,
/// which keeps each run whole. It may keep runs whole that do not overprint, but never interleaves overprinted glyphs.
pub(super) fn collide(glyphs: &[&TextGlyph], height: &dyn Fn(usize) -> f32, h: f32, sc: &mut Scratch) -> bool {
    let x0 = |i: usize| glyphs[i].rect[0];
    let runs = sc.text.len();
    sc.owner.clear();
    for (r, run) in sc.text.iter().enumerate() {
        sc.owner.extend(run.range().map(|i| (i, r)));
    }
    sc.owner.sort_unstable_by(|a, b| by_cluster(glyphs, &sc.anchor, a.0, b.0));
    sc.parent.clear();
    sc.parent.extend(0..runs);
    let mut budget = COLLIDE_PAIRS_PER_GLYPH.saturating_mul(sc.owner.len());
    let mut collided = false;
    let mut dense = false;
    // A mark riding on a glyph goes with it, so only cluster starts are swept. They are in x order in `owner`.
    let rides = |i: usize| anchor_of(&sc.anchor, i) != i;
    'sweep: for a in 0..sc.owner.len() {
        let (i, ri) = sc.owner[a];
        if rides(i) {
            continue;
        }
        let (x1, hi) = (glyphs[i].rect[2], height(i));
        for b in a + 1..sc.owner.len() {
            let (j, rj) = sc.owner[b];
            if rides(j) {
                // Skipping counts against the budget, so a long string of marks cannot make the sweep quadratic.
                if budget == 0 {
                    dense = true;
                    break 'sweep;
                }
                budget -= 1;
                continue;
            }
            if x0(j) >= x1 {
                break;
            }
            if budget == 0 {
                dense = true;
                break 'sweep;
            }
            budget -= 1;
            let (ra, rb) = (union_find(&mut sc.parent, ri), union_find(&mut sc.parent, rj));
            let hj = height(j);
            let script = is_script_on(&glyphs[i].rect, hi, &glyphs[j].rect, hj) || is_script_on(&glyphs[j].rect, hj, &glyphs[i].rect, hi);
            let overlap = x1.min(glyphs[j].rect[2]) - x0(j);
            if ra != rb && !script && overlap > RUN_COLLISION * hi.min(hj) {
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
                sc.units.push(Unit { left: x0(anchor_of(&sc.anchor, i)), first: i, glyphs: at..at + 1 });
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
            sc.group[start..].sort_unstable_by(|a, b| by_cluster(glyphs, &sc.anchor, *a, *b));
            // Ordered by where its clusters start: a mark riding on a glyph may reach left of it, but it has no place of
            // its own, so it cannot put its run before one whose first letter stands further left.
            let left = sc.group[start..].iter().map(|&i| x0(anchor_of(&sc.anchor, i))).fold(f32::INFINITY, f32::min);
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
    } else if is_nonspacing_mark(text) {
        // A mark of a left-to-right script that rides on no letter (`resolve_marks`) has no direction of its own: NSM
        // is a weak class (UAX #9 Table 4), so it reads with the glyphs around its drawn position, as a neutral does,
        // even when it is alphabetic (U+0345, Other_Alphabetic).
        Bidi::N
    } else if text.chars().any(char::is_alphabetic) {
        Bidi::L
    } else {
        Bidi::N
    }
}

/// Whether a line of the glyphs `line` is right-to-left: it has more right-to-left letters than left-to-right ones.
/// Letters are counted one by one, so a ligature glyph ("ffi") counts each of its letters. Punctuation of a
/// right-to-left script (a maqaf, an Arabic comma) is not a letter and counts toward neither direction, nor do
/// nonspacing marks: a mark has the direction of the letter it sits on.
fn is_rtl_line(glyphs: &[&TextGlyph], line: impl Iterator<Item = usize>) -> bool {
    let (mut rtl, mut ltr) = (0usize, 0usize);
    for g in line.filter_map(|i| glyphs.get(i)) {
        for c in g.text.chars().filter(|&c| unicode_bidi::bidi_class(c) != unicode_bidi::BidiClass::NSM) {
            if is_rtl(c) && c.is_alphabetic() {
                rtl = rtl.saturating_add(1);
            } else if c.is_alphabetic() && !is_rtl(c) {
                ltr = ltr.saturating_add(1);
            }
        }
    }
    rtl > ltr
}

/// Whether `text` is only nonspacing marks (bidi class NSM). Such a glyph counts toward neither direction of a line:
/// it has the direction of the letter it sits on.
fn is_nonspacing_mark(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM)
}

/// UAX #9 W1: a nonspacing mark takes the class of the letter it rides on ([`Scratch::anchor`]), which placement keeps
/// right in front of it. A mark that rides on nothing (one after a word gap, or a right-to-left mark, which placement
/// puts by its own position) keeps its own class.
fn resolve_marks(glyphs: &[&TextGlyph], anchor: &[usize], items: &mut [(Option<usize>, Bidi)]) {
    let nsm = |i: usize| glyphs.get(i).is_some_and(|g| is_nonspacing_mark(&g.text));
    for k in 1..items.len() {
        let (prev, base) = match items.get(k - 1) {
            Some(&(Some(prev), base)) => (prev, base),
            _ => continue,
        };
        if let Some(it) = items.get_mut(k)
            && let Some(mark) = it.0
            && nsm(mark)
            && anchor_of(anchor, mark) != mark
            && anchor_of(anchor, mark) == anchor_of(anchor, prev)
        {
            it.1 = base;
        }
    }
}

/// Reorders a line of `idx` from visual to logical order with a simplified bidi level pass (#2667), carrying the
/// word gaps along: `spaces[w]` is a gap left of `idx[w]` (#1062). The paragraph is right-to-left when it has
/// more right-to-left than left-to-right letters ([`is_rtl_line`]); numbers keep their left-to-right order, and neutrals
/// (punctuation, gaps) take the direction of their neighbours, or the paragraph's between opposite ones.
pub(super) fn reverse_rtl(glyphs: &[&TextGlyph], anchor: &[usize], idx: &mut [usize], spaces: &mut [bool]) {
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
    // The paragraph's direction, as `keep_taken_back_blanks` takes it.
    let para = if is_rtl_line(glyphs, idx.iter().copied()) { Bidi::R } else { Bidi::L };
    resolve_marks(glyphs, anchor, &mut items);
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

/// Most space between two runs on one baseline that still reads as one line, in heights of the
/// taller run: a label and its value or table cells, and not two column lines that happen to share
/// a baseline (a single-line paragraph in each column), whose gap spans most of a column.
pub(super) const SAME_LINE_GAP: f32 = 8.0;

/// Whether segment `s` continues the line of segment `p` to its right: their baselines (a fifth of
/// the em box above its bottom, as `draw_glyph` builds it) lie within a fifth of the smaller
/// height, whatever their sizes, and `s` starts no earlier than `p` ends and less than
/// [`SAME_LINE_GAP`] heights after it.
pub(super) fn same_line(p: &Seg, s: &Seg) -> bool {
    let h = p.h.min(s.h);
    let baseline = |seg: &Seg| seg.bbox[3] - 0.2 * seg.h;
    let gap = s.bbox[0] - p.bbox[2];
    (baseline(p) - baseline(s)).abs() < 0.2 * h && gap > -WORD_GAP * h && gap < SAME_LINE_GAP * p.h.max(s.h)
}
