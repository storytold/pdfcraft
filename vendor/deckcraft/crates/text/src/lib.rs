//! Text layout for slide text bodies.
//!
//! [`layout`] resolves every run's formatting through the placeholder/style chain and lays each
//! paragraph out in the order bidirectional text needs:
//! 1. embedding levels for the whole logical paragraph (UAX #9, base level from `a:pPr/@rtl`);
//! 2. itemization by level, script, face (per-cluster fallback through the `a:latin`/`a:ea`/`a:cs`
//!    slots and the theme's script fonts) and shaping style, each item shaped with the whole
//!    paragraph as context; paint-only attributes (colour, highlight, underline) never split a
//!    word, so Arabic joining and ligatures survive them;
//! 3. line breaking in logical order (UAX #14 opportunities at cluster boundaries, greedy fill);
//! 4. per line, rules L1/L2 reorder the characters visually (L4 mirroring is done by the shaper).
//!
//! It then places bullets and numbers from the paragraph's start edge (the right edge for RTL
//! paragraphs), applies indents, spacing and alignment (kashida for `justLow`), anchors the block
//! in the shape's text box, and shrinks text on overflow when the body asks for it. The result is
//! positioned glyph runs in shape-local coordinates plus per-line caret geometry for editing:
//! bidirectional carets with affinity, logical and visual movement, and selection as the union of
//! visual boxes.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod bidi;
pub mod datetime;

use std::collections::HashMap;
use std::sync::Arc;

use deckcraft_color::Rgba;
use deckcraft_fonts::script::{self, joins_following, joins_preceding};
use deckcraft_fonts::{FontDb, FontFace, FontSlot, ScriptTag, ShapeParams};
use deckcraft_geom::{Point, Rect};
use deckcraft_model::resolve::{self, Ctx};
use deckcraft_model::text::{Align, Anchor, AutoFit, BodyProps, Bullet, Caps, ParaProps, RunKind, RunProps, Spacing, Strike, TextBody, TextDir};
use deckcraft_model::{Fill, Shape};
use unicode_segmentation::UnicodeSegmentation;

mod autonum;
mod breaking;
mod layout;
mod run;
mod shape;
mod style;
mod symbol;

pub use autonum::autonum;
#[cfg(test)]
use autonum::chinese_num;
#[cfg(test)]
use breaking::{LINE_PITCH, break_opportunities, measure_str};
#[cfg(test)]
pub(crate) use layout::LAYOUT_PASSES;
pub use layout::layout;
#[cfg(test)]
use layout::layout_scaled;
pub use run::{Dash, Deco, DecoLine, GlyphRun, RunEffects, TextShadow, UPRIGHT_CENTRE};
#[cfg(test)]
use shape::{SUB_SUPER, caps_char, is_upright, shaping_features};
#[cfg(test)]
use style::Cell;
pub use style::{Fields, NoFields, Opts};
#[cfg(test)]
use symbol::{covers_symbol, is_symbol_font, symbol_font_char, symbol_font_draw, symbol_unicode, symbol_unicode_alts};
pub use symbol::{map_symbol_bullet, symbol_char};

#[derive(Clone, Debug, Default)]
pub struct LineInfo {
    pub para: usize,
    /// Character range [start, end) in the paragraph (end excludes the line's break/newline).
    pub start: usize,
    pub end: usize,
    pub top: f64,
    pub baseline: f64,
    pub bottom: f64,
    /// Caret x for each character boundary start..=end (len = end - start + 1), downstream: the
    /// leading edge of the character after the boundary (its right edge if it is right-to-left);
    /// the last entry is the trailing edge of the line's last character.
    pub caret_x: Vec<f64>,
    /// Column index (multi-column bodies).
    pub column: usize,
    /// Right-to-left paragraph.
    pub rtl: bool,
    /// Bidi level of each character start..end after rule L1 (odd = right-to-left).
    pub levels: Vec<u8>,
    /// Visual box (left, right) of each character start..end. Characters inside a grapheme have
    /// empty boxes; a ligature's characters share its width (GDEF ligature carets).
    pub edges: Vec<(f64, f64)>,
    /// Whether a caret may stand before each character start..end (grapheme boundaries).
    pub boundary: Vec<bool>,
    /// Visual extent (left, right) of the line's text.
    pub extent: (f64, f64),
    /// Distance from the paragraph's start edge to the line's text (indent, margin, bullet).
    pub start_offset: f64,
}

#[derive(Clone, Debug, Default)]
pub struct TextLayout {
    pub runs: Vec<GlyphRun>,
    /// Empty list markers for the editor only; ordinary rendering and export omit them.
    /// Their space is still reserved so the caret starts where the first character will.
    pub empty_bullets: Vec<GlyphRun>,
    pub decos: Vec<Deco>,
    pub lines: Vec<LineInfo>,
    /// Inner text box (insets applied), shape-local.
    pub inner: Rect,
    /// Height of the laid-out text (before anchoring).
    pub content_height: f64,
    /// Widest line.
    pub content_width: f64,
    /// Autofit font scale used (1 = none).
    pub font_scale: f64,
    pub line_reduction: f64,
    pub overflow: bool,
    /// Text rotation about the shape centre, degrees (vertical text = 90/270).
    pub rotation: f64,
    /// Per paragraph: (first line index, line count).
    pub para_lines: Vec<(usize, usize)>,
    /// Per paragraph: whether a caret may stand at each character offset 0..=len (grapheme
    /// boundaries), for logical caret movement.
    pub boundaries: Vec<Vec<bool>>,
}

/// Which side of a boundary a caret belongs to: `Downstream` sticks to the character after it
/// (the default), `Upstream` to the character before it, which differs at direction changes and
/// at the end of a wrapped line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Affinity {
    Upstream,
    #[default]
    Downstream,
}

/// Caret movement: through the text order, or across the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MoveMode {
    /// Next/previous grapheme in logical order (`forward` = towards the end of the text).
    #[default]
    Logical,
    /// Next grapheme boundary to the right/left on screen (`forward` = right).
    Visual,
}

impl LineInfo {
    fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
    fn odd(&self, k: usize) -> bool {
        self.levels.get(k).is_some_and(|l| l % 2 == 1)
    }
    /// Leading edge (where its direction begins) of character `k` of the line.
    fn leading(&self, k: usize) -> Option<f64> {
        self.edges.get(k).map(|e| if self.odd(k) { e.1 } else { e.0 })
    }
    /// Trailing edge (where its direction ends) of character `k` of the line.
    fn trailing(&self, k: usize) -> Option<f64> {
        self.edges.get(k).map(|e| if self.odd(k) { e.0 } else { e.1 })
    }
    fn shift_x(&mut self, dx: f64) {
        for x in &mut self.caret_x {
            *x += dx;
        }
        for e in &mut self.edges {
            e.0 += dx;
            e.1 += dx;
        }
        self.extent.0 += dx;
        self.extent.1 += dx;
    }
    /// Graphemes on the line as (first, end) character indices from the line start.
    fn graphemes(&self) -> Vec<(usize, usize)> {
        let n = self.len();
        let mut out = Vec::new();
        let mut k = 0;
        while k < n {
            let mut e = k + 1;
            while e < n && !self.boundary.get(e).copied().unwrap_or(true) {
                e += 1;
            }
            out.push((k, e));
            k = e;
        }
        out
    }
    /// Visual extent of characters `from..to` (k from the line start).
    fn grapheme_box(&self, from: usize, to: usize) -> Option<(f64, f64)> {
        let es = self.edges.get(from..to)?;
        let l = es.iter().map(|e| e.0).fold(f64::INFINITY, f64::min);
        let r = es.iter().map(|e| e.1).fold(f64::NEG_INFINITY, f64::max);
        (l <= r).then_some((l, r))
    }
    /// Visual x intervals covering paragraph characters `from..to` on this line, left to right
    /// and merged where they touch. Bidi text can make a logical range several boxes.
    pub fn spans(&self, from: usize, to: usize) -> Vec<(f64, f64)> {
        let a = from.max(self.start) - self.start;
        let b = to.min(self.end).saturating_sub(self.start);
        merge_spans(self.edges.get(a..b.max(a)).unwrap_or(&[]).to_vec())
    }
    /// Horizontal extent of paragraph characters `from..to` on this line.
    pub fn span(&self, from: usize, to: usize) -> Option<(f64, f64)> {
        let s = self.spans(from, to);
        Some((s.first()?.0, s.last()?.1))
    }
    /// Caret stops left to right: (x, character offset in the paragraph, affinity).
    fn visual_stops(&self) -> Vec<(f64, usize, Affinity)> {
        let mut gs: Vec<(f64, f64, usize, usize, bool)> =
            self.graphemes().into_iter().filter_map(|(a, b)| self.grapheme_box(a, b).map(|(l, r)| (l, r, a, b, self.odd(a)))).collect();
        gs.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut out: Vec<(f64, usize, Affinity)> = Vec::with_capacity(gs.len() * 2 + 1);
        let mut push = |s: (f64, usize, Affinity)| {
            if !out.last().is_some_and(|l: &(f64, usize, Affinity)| (l.0 - s.0).abs() < 0.01 && l.1 == s.1) {
                out.push(s);
            }
        };
        for (l, r, a, b, rtl) in gs {
            let (a, b) = (self.start + a, self.start + b);
            if rtl {
                push((l, b, Affinity::Upstream));
                push((r, a, Affinity::Downstream));
            } else {
                push((l, a, Affinity::Downstream));
                push((r, b, Affinity::Upstream));
            }
        }
        if out.is_empty() {
            out.push((self.caret_x.first().copied().unwrap_or(0.0), self.start, Affinity::Downstream));
        }
        out
    }
}

/// Non-empty x intervals sorted left to right, merged where they touch.
fn merge_spans(mut v: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    v.retain(|e| e.1 > e.0);
    v.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(v.len());
    for e in v {
        match out.last_mut() {
            Some(last) if e.0 <= last.1 + 0.01 => last.1 = last.1.max(e.1),
            _ => out.push(e),
        }
    }
    out
}

/// A caret position: paragraph and character offset within it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub para: usize,
    pub ch: usize,
}

impl TextLayout {
    /// The line containing a caret position (the later line at a soft wrap boundary).
    pub fn line_of(&self, pos: Pos) -> Option<usize> {
        self.line_of_affine(pos, Affinity::Downstream)
    }
    /// The line containing a caret position: at a soft wrap boundary, the earlier line for an
    /// upstream caret and the later one for a downstream caret.
    pub fn line_of_affine(&self, pos: Pos, affinity: Affinity) -> Option<usize> {
        let mut found = None;
        for (i, l) in self.lines.iter().enumerate() {
            if l.para == pos.para && pos.ch >= l.start && pos.ch <= l.end {
                if affinity == Affinity::Upstream && pos.ch == l.end && pos.ch > l.start {
                    return Some(i);
                }
                found = Some(i);
                if pos.ch < l.end {
                    break;
                }
            }
        }
        found
            .or_else(|| self.lines.iter().rposition(|l| l.para == pos.para && l.start <= pos.ch))
            .or_else(|| self.lines.iter().rposition(|l| l.para == pos.para))
    }
    /// Caret line segment (x, top, bottom) in shape-local, unrotated coordinates (downstream).
    pub fn caret(&self, pos: Pos) -> Option<(f64, f64, f64)> {
        self.caret_at(pos, Affinity::Downstream)
    }
    /// Caret line segment (x, top, bottom) for a position and affinity: downstream at the leading
    /// edge of the character after the boundary, upstream at the trailing edge of the one before.
    pub fn caret_at(&self, pos: Pos, affinity: Affinity) -> Option<(f64, f64, f64)> {
        let li = self.line_of_affine(pos, affinity)?;
        let l = self.lines.get(li)?;
        let k = pos.ch.clamp(l.start, l.end) - l.start;
        let x = match affinity {
            Affinity::Downstream if k < l.len() => l.leading(k),
            Affinity::Upstream if k > 0 => l.trailing(k - 1),
            _ => None,
        };
        let x = x.or_else(|| l.caret_x.get(k).copied()).or(l.caret_x.last().copied())?;
        Some((x, l.top, l.bottom))
    }
    /// Nearest caret position for a point.
    pub fn hit(&self, p: Point) -> Pos {
        self.hit_affinity(p).0
    }
    /// Nearest caret position for a point, with the affinity that keeps the caret on the side of
    /// the character that was hit (bidi text has two carets at a direction change).
    pub fn hit_affinity(&self, p: Point) -> (Pos, Affinity) {
        let mut best: Option<&LineInfo> = None;
        for l in &self.lines {
            if p.y >= l.top && p.y < l.bottom {
                // In multi-column bodies pick the line whose column contains x.
                if best.is_none() || p.x >= l.extent.0.min(l.caret_x.first().copied().unwrap_or(l.extent.0)) - 4.0 {
                    best = Some(l);
                }
            }
        }
        let l = match best {
            Some(l) => l,
            None => {
                let first = self.lines.first();
                let last = self.lines.last();
                match (first, last) {
                    (Some(f), _) if p.y < f.top => f,
                    (_, Some(l)) => l,
                    _ => return (Pos::default(), Affinity::Downstream),
                }
            }
        };
        let mut gs: Vec<(f64, f64, usize, usize, bool)> =
            l.graphemes().into_iter().filter_map(|(a, b)| l.grapheme_box(a, b).map(|(x0, x1)| (x0, x1, a, b, l.odd(a)))).collect();
        if gs.is_empty() {
            return (Pos { para: l.para, ch: l.start }, Affinity::Downstream);
        }
        gs.sort_by(|x, y| x.0.total_cmp(&y.0));
        let hit = gs.iter().find(|g| p.x >= g.0 && p.x <= g.1 && g.1 > g.0).copied();
        let (g, left_half) = match hit {
            Some(g) => (g, p.x < (g.0 + g.1) / 2.0),
            None => match (gs.first().copied(), gs.last().copied()) {
                (Some(f), _) if p.x < f.0 => (f, true),
                (_, Some(z)) if p.x > z.1 => (z, false),
                // Between zero-width graphemes: the nearest one.
                _ => match gs.iter().min_by(|a, b| ((a.0 + a.1) / 2.0 - p.x).abs().total_cmp(&((b.0 + b.1) / 2.0 - p.x).abs())).copied() {
                    Some(g) => (g, p.x < (g.0 + g.1) / 2.0),
                    None => return (Pos { para: l.para, ch: l.start }, Affinity::Downstream),
                },
            },
        };
        let (_, _, a, b, rtl) = g;
        let (before, after) = (Pos { para: l.para, ch: l.start + a }, Pos { para: l.para, ch: l.start + b });
        // The leading half of a character puts the caret before it, the trailing half after it.
        if left_half != rtl { (before, Affinity::Downstream) } else { (after, Affinity::Upstream) }
    }
    /// Move a caret one grapheme: logically (`forward` = towards the end of the text, crossing
    /// paragraphs) or visually (`forward` = to the right on screen, continuing on the next or
    /// previous line in the paragraph's direction at the line's edge).
    pub fn move_caret(&self, pos: Pos, affinity: Affinity, forward: bool, mode: MoveMode) -> (Pos, Affinity) {
        if mode == MoveMode::Visual
            && let Some(li) = self.line_of_affine(pos, affinity)
            && let Some(l) = self.lines.get(li)
        {
            let stops = l.visual_stops();
            let cur_x = self.caret_at(pos, affinity).map(|c| c.0).unwrap_or(0.0);
            let idx = stops
                .iter()
                .position(|s| s.1 == pos.ch && s.2 == affinity)
                .or_else(|| stops.iter().position(|s| s.1 == pos.ch))
                .or_else(|| stops.iter().enumerate().min_by(|a, b| (a.1.0 - cur_x).abs().total_cmp(&(b.1.0 - cur_x).abs())).map(|(i, _)| i));
            if let Some(idx) = idx {
                let mut j = idx;
                loop {
                    let next = if forward { j.checked_add(1) } else { j.checked_sub(1) };
                    let Some(nj) = next else { break };
                    let Some(s) = stops.get(nj) else { break };
                    // Skip stops that wouldn't move the caret on screen or in the text.
                    if s.1 != pos.ch && (s.0 - cur_x).abs() > 0.01 {
                        return (Pos { para: l.para, ch: s.1 }, s.2);
                    }
                    j = nj;
                }
            }
            // Off the line's edge: on to the neighbouring line in reading order.
            let logical_forward = forward != l.rtl;
            let target = if logical_forward { li.checked_add(1) } else { li.checked_sub(1) };
            return match target.and_then(|t| self.lines.get(t)) {
                Some(t) if logical_forward => (Pos { para: t.para, ch: t.start }, Affinity::Downstream),
                Some(t) => (Pos { para: t.para, ch: t.end }, if t.para == l.para { Affinity::Upstream } else { Affinity::Downstream }),
                None => (pos, affinity),
            };
        }
        let Some(b) = self.boundaries.get(pos.para) else { return (pos, affinity) };
        let len = b.len().saturating_sub(1);
        let ch = pos.ch.min(len);
        let next = if forward {
            (ch + 1..=len).find(|&k| b.get(k).copied().unwrap_or(true))
        } else {
            (0..ch).rev().find(|&k| b.get(k).copied().unwrap_or(true))
        };
        match next {
            Some(k) => (Pos { para: pos.para, ch: k }, Affinity::Downstream),
            None if forward && pos.para + 1 < self.boundaries.len() => (Pos { para: pos.para + 1, ch: 0 }, Affinity::Downstream),
            None if !forward && pos.para > 0 => {
                let prev = pos.para - 1;
                (Pos { para: prev, ch: self.boundaries.get(prev).map(|b| b.len().saturating_sub(1)).unwrap_or(0) }, Affinity::Downstream)
            }
            None => (Pos { para: pos.para, ch }, Affinity::Downstream),
        }
    }
    /// Selection highlight rectangles between two positions: per line, the union of the visual
    /// boxes of the selected characters (one logical range can be several boxes in bidi text).
    pub fn selection_rects(&self, a: Pos, b: Pos) -> Vec<Rect> {
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        let mut out = vec![];
        for l in &self.lines {
            let lp = Pos { para: l.para, ch: l.start };
            let le = Pos { para: l.para, ch: l.end };
            if le < s || lp > e {
                continue;
            }
            let from = if s > lp { s.ch } else { l.start };
            let to = if e < le { e.ch } else { l.end };
            let mut spans = l.spans(from, to);
            // Selecting across a paragraph end shows a small extra box for the newline, at the
            // paragraph's end side.
            if e.para > l.para && to == l.end {
                let end_x = l.caret_x.last().copied().unwrap_or(l.extent.1);
                spans.push(if l.rtl { (end_x - 6.0, end_x) } else { (end_x, end_x + 6.0) });
                spans = merge_spans(spans);
            }
            for (x0, x1) in spans {
                if x1 > x0 {
                    out.push(Rect::new(x0, l.top, x1, l.bottom));
                }
            }
        }
        out
    }
    /// Underlines for text an input method is composing, from `a` to `b` (#101): `(x0, x1, y)` per
    /// visual span, a little below each line's baseline (PowerPoint underlines the composition
    /// in place). A composition that wraps gets one underline per line.
    pub fn composition_underlines(&self, a: Pos, b: Pos) -> Vec<(f64, f64, f64)> {
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        let mut out = vec![];
        if s == e {
            return out;
        }
        for l in &self.lines {
            let lp = Pos { para: l.para, ch: l.start };
            let le = Pos { para: l.para, ch: l.end };
            if le <= s || lp >= e {
                continue;
            }
            let from = if s > lp { s.ch } else { l.start };
            let to = if e < le { e.ch } else { l.end };
            // Under the baseline, in the descent, never below the line.
            let y = (l.baseline + (l.bottom - l.baseline) * 0.35).min(l.bottom);
            for (x0, x1) in l.spans(from, to) {
                if x1 > x0 {
                    out.push((x0, x1, y));
                }
            }
        }
        out
    }
    /// Vertical caret movement: the position on the line above/below closest to `x`.
    pub fn vertical(&self, pos: Pos, down: bool, x: f64) -> Pos {
        self.vertical_affine(pos, Affinity::Downstream, down, x).0
    }
    /// The line a caret is on. An upstream caret at the start of a soft-wrapped line (where End
    /// leaves it, after the previous line's trailing spaces) belongs to the line before.
    pub fn line_index(&self, pos: Pos, affinity: Affinity) -> Option<usize> {
        if affinity == Affinity::Upstream && pos.ch > 0 {
            for (i, w) in self.lines.windows(2).enumerate() {
                if let [a, b] = w
                    && b.para == pos.para
                    && a.para == pos.para
                    && b.start == pos.ch
                {
                    return Some(i);
                }
            }
        }
        self.line_of_affine(pos, affinity)
    }
    /// Up/Down from a caret with its affinity: the position on the line above/below nearest `x`.
    pub fn vertical_affine(&self, pos: Pos, affinity: Affinity, down: bool, x: f64) -> (Pos, Affinity) {
        let Some(li) = self.line_index(pos, affinity) else { return (pos, affinity) };
        let target = if down { li.checked_add(1) } else { li.checked_sub(1) };
        let Some(l) = target.and_then(|t| self.lines.get(t)) else { return (pos, affinity) };
        let p = Point::new(x, (l.top + l.bottom) / 2.0);
        let mut h = self.hit(p);
        if h.para != l.para {
            h.para = l.para;
        }
        h.ch = h.ch.clamp(l.start, l.end);
        (h, Affinity::Downstream)
    }
    /// Start/end of the visual line containing `pos` (logical: Home/End).
    pub fn line_bounds(&self, pos: Pos) -> (Pos, Pos) {
        self.line_bounds_affine(pos, Affinity::Downstream)
    }
    /// Start/end of the visual line a caret with this affinity is on.
    pub fn line_bounds_affine(&self, pos: Pos, affinity: Affinity) -> (Pos, Pos) {
        match self.line_index(pos, affinity).and_then(|i| self.lines.get(i)) {
            Some(l) => (Pos { para: l.para, ch: l.start }, Pos { para: l.para, ch: l.end }),
            None => (pos, pos),
        }
    }
    /// Where End puts the caret: after a soft wrap, past the line's trailing spaces at the start
    /// of the next line with upstream affinity (so it is still drawn on this line), else the end
    /// of the paragraph.
    pub fn line_end_caret(&self, pos: Pos, affinity: Affinity) -> (Pos, Affinity) {
        let Some(li) = self.line_index(pos, affinity) else { return (pos, Affinity::Downstream) };
        let Some(l) = self.lines.get(li) else { return (pos, Affinity::Downstream) };
        match self.lines.get(li + 1) {
            Some(n) if n.para == l.para && n.start > l.start => (Pos { para: l.para, ch: n.start }, Affinity::Upstream),
            _ => (Pos { para: l.para, ch: l.end }, Affinity::Downstream),
        }
    }
    /// The caret to draw (x, top, bottom): an upstream caret after a soft wrap is drawn at the end
    /// of the line it belongs to.
    pub fn caret_drawn(&self, pos: Pos, affinity: Affinity) -> Option<(f64, f64, f64)> {
        let li = self.line_index(pos, affinity)?;
        let l = self.lines.get(li)?;
        if pos.ch > l.end {
            let x = l.caret_x.last().copied()?;
            return Some((x, l.top, l.bottom));
        }
        self.caret_at(pos, affinity)
    }
    /// Bounding box of all glyphs/lines (shape-local).
    pub fn bounds(&self) -> Rect {
        let mut r: Option<Rect> = None;
        for l in &self.lines {
            let lr = Rect::new(l.extent.0, l.top, l.extent.1.max(l.extent.0), l.bottom);
            r = Some(r.map(|a| a.union(lr)).unwrap_or(lr));
        }
        r.unwrap_or(self.inner)
    }
}

/// Height the shape needs so its text fits (Resize shape to fit text): text height + insets.
pub fn fit_height(ctx: &Ctx, shape: &Shape, body: &TextBody, rect: Rect, fields: &dyn Fields) -> f64 {
    let bp = resolve::body(ctx, shape);
    let l = layout(ctx, shape, body, &Opts { rect, fields, prompt_color: None, no_shrink: true });
    l.content_height + bp.inset_t.unwrap_or(3.6) + bp.inset_b.unwrap_or(3.6)
}

/// Width a non-wrapping text box needs.
pub fn fit_width(ctx: &Ctx, shape: &Shape, body: &TextBody, rect: Rect, fields: &dyn Fields) -> f64 {
    let bp = resolve::body(ctx, shape);
    let wide = Rect::new(rect.x0, rect.y0, rect.x0 + 100_000.0, rect.y1);
    let mut s2 = shape.clone();
    if let Some(t) = s2.text.as_mut() {
        t.body.wrap = Some(false);
    }
    let l = layout(ctx, &s2, body, &Opts { rect: wide, fields, prompt_color: None, no_shrink: true });
    let w = l.lines.iter().map(|li| li.start_offset + (li.extent.1 - li.extent.0)).fold(0.0, f64::max);
    w + bp.inset_l.unwrap_or(7.2) + bp.inset_r.unwrap_or(7.2) + 1.0
}

#[cfg(test)]
mod tests;
