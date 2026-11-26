//! Edit text on a scanned page: an OCR word is invisible text over the scan, so rewriting it must
//! also hide the scanned word. The paper colour around the word becomes the cover drawn over it
//! ([`pdfcraft_edit::BlockStyle::cover`]) and the colour of its ink becomes the new text's. The
//! ruled lines that cross the word's surroundings, a table cell's borders, are read too, so the
//! cover leaves them standing ([`pdfcraft_edit::BlockStyle::cover_keeps`]).

use std::sync::Arc;

use pdfcraft_edit::TextBlock;
use pdfcraft_render::{PageInfo, PageRenderer, RenderConfig, RenderRequest, Tile};

/// Device pixels per point the word's surroundings are read at.
const SCALE: f32 = 3.0;
/// The most pixels read for one paragraph; a larger area is read at a lower resolution.
const MAX_PIXELS: f32 = 4_000_000.0;
/// How far around the word (points) the paper is read.
const RING: f32 = 4.0;
/// How far a rule may lean across the area, one pixel in this many of its length, and still be
/// read row by row: a scan is rarely off square by more than half a degree.
const LEAN: usize = 120;
/// The least of a row (or column) that must be inked over its rule's length for it to be part of
/// the rule's strip: the rows a leaning rule runs through, not the blank row beside it that the
/// lean allowance reads. A rule one pixel thick leaning the most [`LEAN`] allows puts `LEAN`
/// pixels on each row, whatever its length, so no row is asked for more than half of that.
const ROW_INK: f32 = 0.2;
const ROW_INK_LEANING: f32 = LEAN as f32 / 2.0;
/// How far (pixels) a rule may stop short of the edge, or of the rule it meets, and still be read
/// as reaching it: the halo a scanned junction leaves.
const JUNCTION_GAP: usize = 2;
/// The least of the area's length a rule must run on ink of its own, outside the rules it crosses:
/// where a rule across crosses a blank column, that column is no rule down.
const MIN_RUN: f32 = 0.1;
/// How far (pixels) a strip reaches past the ink read as the rule, on every side: the grey halo a
/// scanned line has, which the cover would otherwise paint, leaving a thinner line than the scan's.
const HALO: usize = 1;
/// The thickest a rule may be, as a fraction of the area's other side: thicker, the "rule" is a
/// filled cell, or lettering on a dark ground, and the cover must paint it.
const RULE_THICKNESS: f32 = 0.3;

/// What the scan shows around an invisible paragraph.
pub(crate) struct Sampled {
    /// The paper's colour (RGB 0–1): the cover's.
    pub paper: [f64; 3],
    /// The ink's colour (RGB 0–1): the new text's.
    pub ink: [f64; 3],
    /// The ruled lines that cross the paragraph's surroundings, as boxes in user space, for the
    /// cover to leave be.
    pub rules: Vec<[f64; 4]>,
}

/// The paper and ink colours of invisible paragraph `b` on `page`, and the ruled lines around it,
/// or `None` when the paragraph is visible text or the page can't be read.
pub(crate) fn sample(cos: &pdfcraft_cos::Document, info: &PageInfo, page: usize, b: &TextBlock) -> Option<Sampled> {
    if !b.invisible {
        return None;
    }
    // The word's box in view space (points, y down).
    let corners = [[b.rect[0], b.rect[1]], [b.rect[2], b.rect[1]], [b.rect[0], b.rect[3]], [b.rect[2], b.rect[3]]]
        .map(|[x, y]| info.user_to_view(x as f32, y as f32));
    let (x0, x1) = corners.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, z), c| (a.min(c[0]), z.max(c[0])));
    let (y0, y1) = corners.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, z), c| (a.min(c[1]), z.max(c[1])));
    let word = [x0, y0, x1, y1];
    let area = [x0 - RING, y0 - RING, x1 + RING, y1 + RING];
    if word.iter().any(|v| !v.is_finite()) || x1 <= x0 || y1 <= y0 {
        return None;
    }
    let pixels = (area[2] - area[0]) * (area[3] - area[1]) * SCALE * SCALE;
    let scale = if pixels > MAX_PIXELS { SCALE * (MAX_PIXELS / pixels).sqrt() } else { SCALE };
    let (page_w, page_h) = (info.width * scale, info.height * scale);
    let clamp = |v: f32, max: f32| v.clamp(0.0, max.max(0.0)).floor() as u32;
    let (tx0, ty0) = (clamp(area[0] * scale, page_w), clamp(area[1] * scale, page_h));
    let (tx1, ty1) = (clamp(area[2] * scale, page_w), clamp(area[3] * scale, page_h));
    if tx1 <= tx0 || ty1 <= ty0 {
        return None;
    }
    let tile = Tile { x: tx0, y: ty0, w: tx1 - tx0, h: ty1 - ty0 };
    // The working file as it is: written out only if the document holds unsaved changes, which
    // it doesn't between edits (each one reopens the file it wrote).
    let bytes = if cos.is_modified() {
        Arc::new(pdfcraft_cos::write_incremental(cos, &pdfcraft_cos::SaveOptions::default()).ok()?)
    } else {
        cos.bytes().clone()
    };
    // The document's true colours, whatever the reading colours are set to.
    let shot = PageRenderer::new(bytes, RenderConfig::default()).render(RenderRequest { page, scale, tile: Some(tile), ..Default::default() });
    if shot.error.is_some() {
        return None;
    }
    let pad = ((y1 - y0) * 0.1).clamp(0.5, 2.0);
    let (mut paper, mut ink) = (Vec::new(), Vec::new());
    let rgb_of = |px: &[u8; 4]| [px[0], px[1], px[2]].map(|c| f64::from(c) / 255.0);
    for (i, px) in shot.rgba.as_chunks::<4>().0.iter().enumerate() {
        let (col, row) = (i % shot.width.max(1) as usize, i / shot.width.max(1) as usize);
        let x = (tx0 as f32 + col as f32 + 0.5) / scale;
        let y = (ty0 as f32 + row as f32 + 0.5) / scale;
        let rgb = rgb_of(px);
        if x >= x0 && x <= x1 && y >= y0 && y <= y1 {
            ink.push(rgb);
        } else if x < x0 - pad || x > x1 + pad || y < y0 - pad || y > y1 + pad {
            paper.push(rgb);
        }
    }
    let by_luma = |a: &[f64; 3], b: &[f64; 3]| luma(a).total_cmp(&luma(b));
    paper.sort_by(by_luma);
    ink.sort_by(by_luma);
    // The paper is the ring's median, so a table rule or a neighbouring letter in it doesn't count.
    let paper = paper.get(paper.len() / 2).copied().unwrap_or([1.0; 3]);
    // The ink is the average of the word's darkest pixels; a box without any is printed in black.
    let darkest = ink.get(..ink.len().div_ceil(8)).unwrap_or_default();
    let ink = if darkest.is_empty() {
        [0.0; 3]
    } else {
        let sum = darkest.iter().fold([0.0; 3], |s, c| [s[0] + c[0], s[1] + c[1], s[2] + c[2]]);
        sum.map(|v| v / darkest.len() as f64)
    };
    let ink = if luma(&paper) - luma(&ink) < 0.25 { [0.0; 3] } else { ink };
    // Ink is what lies on the ink's side of the midpoint between the two.
    let mid = (luma(&paper) + luma(&ink)) / 2.0;
    let dark: Vec<bool> = shot.rgba.as_chunks::<4>().0.iter().map(|px| luma(&rgb_of(px)) < mid).collect();
    let (w, h) = (shot.width as usize, shot.height as usize);
    let rules = rule_strips(&dark, w, h)
        .into_iter()
        .map(|[sx0, sy0, sx1, sy1]| {
            // Tile pixels to view space, then back to the space the paragraph's box is in.
            let view = |x: usize, y: usize| {
                info.view_to_user_f64(f64::from(tx0 + x as u32) / f64::from(scale), f64::from(ty0 + y as u32) / f64::from(scale))
            };
            let corners = [view(sx0, sy0), view(sx1, sy0), view(sx0, sy1), view(sx1, sy1)];
            corners.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |r, [x, y]| {
                [r[0].min(*x), r[1].min(*y), r[2].max(*x), r[3].max(*y)]
            })
        })
        .filter(|r| r.iter().all(|v| v.is_finite()))
        .collect();
    Some(Sampled { paper, ink, rules })
}

fn luma(c: &[f64; 3]) -> f64 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

/// The ruled lines in a `w` × `h` grid of ink, as strips `[x0, y0, x1, y1)`: a row, or a column,
/// of ink that runs from one edge of the grid to the other, or from an edge to a rule it meets
/// (a table cell's divider starts at the cell's top rule), or between two. Lettering stops short
/// of the edges and the rules. A strip thicker than [`RULE_THICKNESS`] of the grid is no rule
/// either, but a dark ground.
fn rule_strips(dark: &[bool], w: usize, h: usize) -> Vec<[usize; 4]> {
    if w < 2 || h < 2 || dark.len() < w * h {
        return Vec::new();
    }
    let at = |x: usize, y: usize| dark.get(y * w + x).copied().unwrap_or(false);
    // A rule may lean a little: a row is read together with the rows within `lean` of it.
    let lean = |len: usize| len.div_ceil(2 * LEAN).max(1);
    let (lean_y, lean_x) = (lean(w), lean(h));
    let row_inked = |x: usize, y: usize| (y.saturating_sub(lean_y)..=(y + lean_y).min(h - 1)).any(|yy| at(x, yy));
    let column_inked = |x: usize, y: usize| (x.saturating_sub(lean_x)..=(x + lean_x).min(w - 1)).any(|xx| at(xx, y));
    // Rules across first, from edge to edge; then rules down, from an edge or a rule across to
    // the other; then across again, now reaching a rule down too.
    let across = |anchors: &[[usize; 4]]| lines(h, w, |y, x| row_inked(x, y), |y, x| at(x, y), anchors);
    let down = lines(w, h, column_inked, at, &across(&[]));
    let across = across(&down);
    // `lines` gives [position0, along0, position1, along1]: rows run along x, columns along y.
    across.iter().map(|[y0, x0, y1, x1]| [*x0, *y0, *x1, *y1]).chain(down.iter().copied()).collect()
}

/// The strips of ruled line among `count` positions (rows, say), each `len` long, as
/// `[position0, along0, position1, along1)`. `inked(i, j)` is ink at position `i`, `j` along,
/// read with its lean; `own(i, j)` the pixel itself. `anchors` are the strips found the other
/// way, which lie across `[a[0], a[2])` of this one.
fn lines(
    count: usize,
    len: usize,
    inked: impl Fn(usize, usize) -> bool,
    own: impl Fn(usize, usize) -> bool,
    anchors: &[[usize; 4]],
) -> Vec<[usize; 4]> {
    let anchors: Vec<[usize; 2]> = anchors.iter().map(|a| [a[0], a[2]]).collect();
    let lines: Vec<Option<[usize; 2]>> = (0..count).map(|i| line(len, |j| inked(i, j), |j| own(i, j), &anchors)).collect();
    let mut strips = Vec::new();
    for [i0, i1] in runs(count, |i| lines.get(i).is_some_and(Option::is_some)) {
        if (i1 - i0) as f32 > RULE_THICKNESS * count as f32 {
            continue;
        }
        let along = lines.get(i0..i1).unwrap_or_default().iter().flatten().fold([usize::MAX, 0], |r, [s, e]| [r[0].min(*s), r[1].max(*e)]);
        // With its halo, within the grid.
        strips.push([i0.saturating_sub(HALO), along[0].saturating_sub(HALO), (i1 + HALO).min(count), (along[1] + HALO).min(len)]);
    }
    strips
}

/// Where a rule runs at one position: the longest stretch of ink (the rules it crosses counting
/// as ink) that starts at the edge or at a rule it meets, within [`JUNCTION_GAP`], and ends at
/// the other, with [`MIN_RUN`] of ink of its own and [`ROW_INK`] of it on this very row. `None`
/// when there is no such stretch.
fn line(len: usize, inked: impl Fn(usize) -> bool, own: impl Fn(usize) -> bool, anchors: &[[usize; 2]]) -> Option<[usize; 2]> {
    let in_anchor = |j: usize| anchors.iter().any(|[a0, a1]| (*a0..*a1).contains(&j));
    let starts_anchored = |s: usize| s <= JUNCTION_GAP || in_anchor(s) || anchors.iter().any(|[_, a1]| *a1 <= s && s - a1 <= JUNCTION_GAP);
    let ends_anchored = |e: usize| e + JUNCTION_GAP >= len || in_anchor(e - 1) || anchors.iter().any(|[a0, _]| *a0 >= e && a0 - e <= JUNCTION_GAP);
    let own_run = ((MIN_RUN * len as f32).ceil() as usize).max(3);
    let row_ink = |run: usize| (ROW_INK * run as f32).min(ROW_INK_LEANING);
    runs(len, |j| inked(j) || in_anchor(j))
        .into_iter()
        .filter(|[s, e]| starts_anchored(*s) && ends_anchored(*e))
        .filter(|[s, e]| (*s..*e).filter(|&j| inked(j) && !in_anchor(j)).count() >= own_run)
        .filter(|[s, e]| (*s..*e).filter(|&j| own(j)).count() as f32 >= row_ink(e - s))
        .max_by_key(|[s, e]| e - s)
}

/// The stretches `[start, end)` of `0..n` where `is` holds.
fn runs(n: usize, is: impl Fn(usize) -> bool) -> Vec<[usize; 2]> {
    let mut out: Vec<[usize; 2]> = Vec::new();
    for i in (0..n).filter(|&i| is(i)) {
        match out.last_mut() {
            Some(last) if last[1] == i => last[1] = i + 1,
            _ => out.push([i, i + 1]),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid of ink, drawn in code.
    struct Grid {
        w: usize,
        h: usize,
        dark: Vec<bool>,
    }

    impl Grid {
        fn new(w: usize, h: usize) -> Grid {
            Grid { w, h, dark: vec![false; w * h] }
        }

        fn rect(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
            for y in y0..y1.min(self.h) {
                for x in x0..x1.min(self.w) {
                    self.dark[y * self.w + x] = true;
                }
            }
        }

        /// Lettering: a row of upright strokes with gaps between them.
        fn letters(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
            for x in (x0..x1).step_by(6) {
                self.rect(x, y0, x + 4, y1);
            }
        }

        fn strips(&self) -> Vec<[usize; 4]> {
            rule_strips(&self.dark, self.w, self.h)
        }
    }

    #[test]
    fn a_line_across_the_area_is_a_rule_and_lettering_is_not() {
        let mut g = Grid::new(200, 80);
        g.rect(0, 10, 200, 13);
        g.letters(30, 30, 170, 60);
        // The strip takes in the pixel around the rule: a scanned line's halo.
        assert_eq!(g.strips(), [[0, 9, 200, 14]]);

        let mut g = Grid::new(200, 80);
        g.rect(50, 0, 53, 80);
        g.letters(60, 30, 190, 60);
        assert_eq!(g.strips(), [[49, 0, 54, 80]], "a divider down the area");

        let mut g = Grid::new(200, 80);
        g.rect(0, 10, 200, 13);
        g.rect(50, 0, 53, 80);
        assert_eq!(g.strips(), [[0, 9, 200, 14], [49, 0, 54, 80]], "both at once");
    }

    #[test]
    fn a_rule_may_lean_a_little() {
        let mut g = Grid::new(240, 80);
        for x in 0..240 {
            let y = 10 + x / 120;
            g.rect(x, y, x + 1, y + 3);
        }
        assert_eq!(g.strips(), [[0, 9, 240, 15]], "one strip over the rows it runs through");

        // A rule one pixel thick, leaning the most that is allowed, across a long area: each row
        // holds only 1 / LEAN of its pixels, however long the area.
        let mut g = Grid::new(720, 40);
        for x in 0..720 {
            g.rect(x, 10 + x / 120, x + 1, 11 + x / 120);
        }
        assert_eq!(g.strips(), [[0, 11, 720, 15]], "the rows that see all of it, over its whole length");
    }

    #[test]
    fn what_stops_short_of_an_edge_or_fills_the_area_is_not_a_rule() {
        let mut g = Grid::new(200, 80);
        g.rect(0, 10, 150, 13);
        assert!(g.strips().is_empty(), "a line that ends inside the area: {:?}", g.strips());

        let mut g = Grid::new(200, 80);
        g.rect(60, 0, 63, 70);
        g.rect(0, 30, 200, 60);
        assert!(g.strips().is_empty(), "a tall letter, and a solid block that reaches the edges but is no line: {:?}", g.strips());

        let mut g = Grid::new(200, 80);
        g.rect(0, 0, 200, 80);
        assert!(g.strips().is_empty(), "a dark ground: {:?}", g.strips());
    }

    #[test]
    fn lettering_that_touches_a_rule_leaves_the_rule_its_own_rows() {
        let mut g = Grid::new(200, 80);
        g.rect(0, 40, 200, 43);
        g.letters(20, 10, 180, 40);
        assert_eq!(g.strips(), [[0, 38, 200, 44]], "the row of lettering touching it goes with it, then the halo on either side");
    }

    #[test]
    fn a_divider_runs_from_the_rule_it_meets_to_the_edge() {
        let mut g = Grid::new(200, 80);
        g.rect(0, 20, 200, 23);
        // Down from the rule to the bottom edge; up from the top edge to the rule; and down from
        // the rule after the halo of a scanned junction. A divider that touches the rule runs
        // through the rule's rows and halo too: they are its ink as much as the rule's.
        g.rect(50, 23, 53, 80);
        g.rect(120, 0, 123, 20);
        g.rect(150, 25, 153, 80);
        g.letters(60, 30, 110, 60);
        assert_eq!(g.strips(), [[0, 19, 200, 24], [49, 18, 54, 80], [119, 0, 124, 25], [149, 24, 154, 80]]);

        // A stroke of lettering that touches the rule, and a stub that meets nothing, are not dividers.
        let mut g = Grid::new(200, 80);
        g.rect(0, 20, 200, 23);
        g.rect(60, 23, 63, 60);
        g.rect(120, 30, 123, 80);
        assert_eq!(g.strips(), [[0, 19, 200, 24]]);
    }

    #[test]
    fn the_scan_of_the_end_to_end_test_is_read_right() {
        // The tile `editing_a_word_in_a_table_cell_keeps_the_cells_rules` reads: a 1 pt rule and
        // divider at 3 px/pt, and the solid block of the scanned word.
        let mut g = Grid::new(204, 84);
        g.rect(0, 6, 204, 9);
        g.rect(6, 0, 9, 84);
        g.rect(12, 12, 192, 72);
        assert_eq!(g.strips(), [[0, 5, 204, 10], [5, 0, 10, 84]]);
    }

    #[test]
    fn runs_group_what_is_true() {
        assert_eq!(runs(6, |i| i == 1 || i == 2 || i == 5), [[1, 3], [5, 6]]);
        assert!(runs(0, |_| true).is_empty());
    }
}
