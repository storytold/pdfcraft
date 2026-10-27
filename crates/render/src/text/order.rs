use super::layout::{is_rtl, union};
use super::*;

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
}

#[derive(Default)]
struct SegExtra {
    more: Vec<Run>,
}

impl Seg {
    pub(super) fn new(first: Run, bbox: [f32; 4], h: f32) -> Self {
        Seg { first, extra: None, bbox, h }
    }

    pub(super) fn cy(&self) -> f32 {
        (self.bbox[1] + self.bbox[3]) / 2.0
    }

    fn runs(&self) -> impl Iterator<Item = Run> + '_ {
        std::iter::once(self.first).chain(self.extra.iter().flat_map(|e| e.more.iter().copied()))
    }

    pub(super) fn merge(&mut self, other: Seg) {
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
/// extent of its own, so it goes next to the glyph it starts at.
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
pub(super) fn collide(glyphs: &[&TextGlyph], height: &dyn Fn(usize) -> f32, h: f32, sc: &mut Scratch) -> bool {
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
pub(super) fn reverse_rtl(glyphs: &[&TextGlyph], idx: &mut [usize], spaces: &mut [bool]) {
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
