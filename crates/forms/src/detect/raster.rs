//! Field detection on scanned pages: the [`Shapes`] that [`detect`](super::detect) needs, read
//! from a picture of the page instead of its content stream.
//!
//! Steps:
//! - Ink is a pixel clearly darker than the paper around it (a local mean, so the uneven paper
//!   of a scan or photo still reads as paper).
//! - Straight lines are runs of ink at least [`MIN_LINE`] long, horizontal or vertical, with
//!   short gaps (noise, a faint print) bridged.
//! - A box is a patch of paper that the lines enclose and that is close to a rectangle. Its
//!   writing area is the whole box when it is empty, or else the largest band of empty rows or
//!   columns in it (many official forms print a small label in the corner of each box).
//! - The remaining horizontal lines, the ones that are not the edge of a box, are rules.
//!
//! Everything is bounded: pictures larger than [`MAX_PIXELS`] are refused (the caller renders
//! at a resolution that fits), and the results are capped.

use super::Shapes;

/// The largest picture analysed (about A3 at 200 dpi).
pub const MAX_PIXELS: usize = 12_000_000;
/// The shortest run of ink that counts as a line, in points (a check box side is at least 5).
const MIN_LINE: f64 = 4.0;
/// Gaps up to this long (points) inside a line are bridged.
const GAP: f64 = 1.0;
/// Lines thicker than this (points, on average) are dark areas, not rules.
const MAX_THICKNESS: f64 = 3.0;
/// The shortest rule reported, in points ([`detect`](super::detect) wants 40 for a field).
const MIN_RULE: f64 = 20.0;
/// A box is at least this share of its bounding rectangle (a skewed scan loses a little).
const RECTANGULAR: f64 = 0.75;
/// Boxes taller than this (points) are frames around parts of the form, not places to write.
const MAX_BOX_HEIGHT: f64 = 120.0;
/// At most this many boxes and rules are reported.
const MAX_SHAPES: usize = 4000;

/// A grey picture (0 black, 255 white) and its size.
struct Grey {
    w: usize,
    h: usize,
    v: Vec<u8>,
}

/// The shapes in `rgba` (premultiplied RGBA8, `width` × `height`, the whole page as displayed),
/// where `px_per_pt` pixels make a point. Coordinates are points with the origin at the
/// picture's bottom-left and y up, the frame [`detect`](super::detect) expects. Empty when the
/// picture is malformed or larger than [`MAX_PIXELS`].
pub fn shapes(rgba: &[u8], width: u32, height: u32, px_per_pt: f64) -> Shapes {
    let (w, h) = (width as usize, height as usize);
    let valid = w >= 8
        && h >= 8
        && px_per_pt.is_finite()
        && (0.25..=20.0).contains(&px_per_pt)
        && w.checked_mul(h).is_some_and(|n| n <= MAX_PIXELS)
        && w.checked_mul(h).and_then(|n| n.checked_mul(4)) == Some(rgba.len());
    if !valid {
        return Shapes::default();
    }
    let grey = grey(rgba, w, h);
    let ink = ink(&grey, px_per_pt);
    let px = |pt: f64| (pt * px_per_pt).round().max(1.0) as usize;
    let (min_line, gap) = (px(MIN_LINE), px(GAP));
    let horizontal = runs(&ink, w, h, min_line, gap, true);
    let vertical = runs(&ink, w, h, min_line, gap, false);
    // The lines, a pixel thicker so that a hairline crack in a corner still encloses.
    let mut lines: Vec<bool> = horizontal.iter().zip(&vertical).map(|(a, b)| *a || *b).collect();
    lines = dilate(&lines, w, h);
    let areas = enclosed(&lines, &ink, w, h, px_per_pt);
    let to_pt = |x: usize, y: usize| [x as f64 / px_per_pt, (h as f64 - y as f64) / px_per_pt];
    let mut out = Shapes::default();
    for [x0, y0, x1, y1] in areas.into_iter().take(MAX_SHAPES) {
        let (a, b) = (to_pt(x0, y1), to_pt(x1, y0));
        out.boxes.push([a[0], a[1], b[0], b[1]]);
    }
    let tol = px(2.0);
    // Whether a vertical line passes the end of a horizontal one at column `x`.
    let attached = |x: usize, l: &Component| {
        (l.y0.saturating_sub(tol)..l.y1.saturating_add(tol).min(h))
            .any(|y| (x.saturating_sub(tol)..x.saturating_add(tol + 1).min(w)).any(|x| vertical.get(y * w + x).copied().unwrap_or(false)))
    };
    for l in components(&horizontal, w, h) {
        let len = l.x1 - l.x0;
        let thick = l.count as f64 / len.max(1) as f64;
        if (len as f64) < MIN_RULE * px_per_pt || thick > MAX_THICKNESS * px_per_pt {
            continue;
        }
        let y = l.y_sum as f64 / l.count.max(1) as f64;
        // The scan's own border is not a line on the form.
        if y < tol as f64 || y > h.saturating_sub(tol) as f64 {
            continue;
        }
        // Closed at both ends: the edge of a box, a table or a frame, not a line to write on.
        if attached(l.x0, &l) && attached(l.x1 - 1, &l) {
            continue;
        }
        let y = (h as f64 - y) / px_per_pt;
        if out.rules.len() < MAX_SHAPES {
            out.rules.push([l.x0 as f64 / px_per_pt, y, l.x1 as f64 / px_per_pt, y]);
        }
    }
    out
}

/// Luminance over white paper (a transparent pixel is paper).
fn grey(rgba: &[u8], w: usize, h: usize) -> Grey {
    let v = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[r, g, b, a]| {
            let (r, g, b, a) = (u32::from(r), u32::from(g), u32::from(b), u32::from(a));
            // Premultiplied over white: c + (255 - a).
            let l = (r * 77 + g * 150 + b * 29) >> 8;
            (l + (255 - a)).min(255) as u8
        })
        .collect();
    Grey { w, h, v }
}

/// Which pixels are ink: darker than 80 % of the mean around them (a window of about 20 points),
/// or dark outright.
fn ink(g: &Grey, px_per_pt: f64) -> Vec<bool> {
    let (w, h) = (g.w, g.h);
    let r = ((10.0 * px_per_pt).round() as usize).clamp(2, 200);
    // A separable box mean: rows first, then columns. Sums stay below 255 × 401 × 401 < 2^32.
    let mut rows = vec![0u32; w * h];
    for y in 0..h {
        let line = g.v.get(y * w..(y + 1) * w).unwrap_or_default();
        let mut sum: u32 = line.iter().take(r + 1).map(|v| u32::from(*v)).sum();
        let mut n = (r + 1).min(w) as u32;
        for x in 0..w {
            if let Some(slot) = rows.get_mut(y * w + x) {
                *slot = sum / n.max(1);
            }
            if let Some(v) = line.get(x + r + 1) {
                sum += u32::from(*v);
                n += 1;
            }
            if x >= r
                && let Some(v) = line.get(x - r)
            {
                sum = sum.saturating_sub(u32::from(*v));
                n = n.saturating_sub(1);
            }
        }
    }
    let mut out = vec![false; w * h];
    for x in 0..w {
        let at = |y: usize| rows.get(y * w + x).copied().unwrap_or(255);
        let mut sum: u32 = (0..=r.min(h - 1)).map(at).sum();
        let mut n = (r + 1).min(h) as u32;
        for y in 0..h {
            let mean = sum / n.max(1);
            let i = y * w + x;
            let v = u32::from(g.v.get(i).copied().unwrap_or(255));
            if let Some(slot) = out.get_mut(i) {
                *slot = v < 96 || (v * 5 < mean * 4 && v < 224);
            }
            if y + r + 1 < h {
                sum += at(y + r + 1);
                n += 1;
            }
            if y >= r {
                sum = sum.saturating_sub(at(y - r));
                n = n.saturating_sub(1);
            }
        }
    }
    out
}

/// The pixels on straight runs of ink at least `min` long, along rows (`across`) or columns,
/// with gaps up to `gap` bridged. A run is nearly solid: a word in small print, its letters
/// run together by noise, has more gaps than a line.
fn runs(ink: &[bool], w: usize, h: usize, min: usize, gap: usize, across: bool) -> Vec<bool> {
    let mut out = vec![false; w * h];
    let (outer, inner) = if across { (h, w) } else { (w, h) };
    let index = |o: usize, i: usize| if across { o * w + i } else { i * w + o };
    for o in 0..outer {
        // The current run: its first and last inked position and how many are inked.
        let mut run: Option<(usize, usize, usize)> = None;
        let close = |run: Option<(usize, usize, usize)>, out: &mut Vec<bool>| {
            if let Some((a, b, n)) = run
                && b + 1 - a >= min
                && n * 4 >= (b + 1 - a) * 3
            {
                for i in a..=b {
                    if let Some(slot) = out.get_mut(index(o, i)) {
                        *slot = true;
                    }
                }
            }
        };
        for i in 0..inner {
            if !ink.get(index(o, i)).copied().unwrap_or(false) {
                continue;
            }
            run = match run {
                Some((a, b, n)) if i - b <= gap + 1 => Some((a, i, n + 1)),
                other => {
                    close(other, &mut out);
                    Some((i, i, 1))
                }
            };
        }
        close(run, &mut out);
    }
    out
}

/// `mask` grown by one pixel in each of the four directions.
fn dilate(mask: &[bool], w: usize, h: usize) -> Vec<bool> {
    let at = |x: usize, y: usize| mask.get(y * w + x).copied().unwrap_or(false);
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let on = at(x, y) || (x > 0 && at(x - 1, y)) || (x + 1 < w && at(x + 1, y)) || (y > 0 && at(x, y - 1)) || (y + 1 < h && at(x, y + 1));
            if let Some(slot) = out.get_mut(y * w + x) {
                *slot = on;
            }
        }
    }
    out
}

/// Where to write in the rectangular patches of paper that `lines` enclose (none that touch
/// the picture's edge), in pixels: [x0, y0, x1, y1) with y down.
fn enclosed(lines: &[bool], ink: &[bool], w: usize, h: usize, px_per_pt: f64) -> Vec<[usize; 4]> {
    let mut label = vec![0u32; w * h];
    let mut out = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut next: u32 = 0;
    let max_h = (MAX_BOX_HEIGHT * px_per_pt) as usize;
    for start in 0..w * h {
        if lines.get(start).copied().unwrap_or(true) || label.get(start).copied().unwrap_or(1) != 0 {
            continue;
        }
        next = next.saturating_add(1);
        if next == u32::MAX {
            break;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        let mut count = 0usize;
        let mut border = false;
        stack.clear();
        stack.push(start);
        if let Some(l) = label.get_mut(start) {
            *l = next;
        }
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            count += 1;
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            border |= x == 0 || y == 0 || x + 1 == w || y + 1 == h;
            let mut visit = |j: usize| {
                if !lines.get(j).copied().unwrap_or(true)
                    && let Some(l) = label.get_mut(j)
                    && *l == 0
                {
                    *l = next;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        let (bw, bh) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        let min = (MIN_LINE * px_per_pt) as usize;
        if border || bw < min || bh < min || bh > max_h || (count as f64) < RECTANGULAR * (bw * bh) as f64 {
            continue;
        }
        if let Some(area) = writing_area(&label, ink, [w, h], [x0, y0, x1, y1], next, px_per_pt) {
            out.push(area);
        }
        if out.len() >= MAX_SHAPES {
            break;
        }
    }
    out
}

/// Where to write in the region `id` with bounding rectangle `rect`, in pixels: all of it when it is empty,
/// else the largest band of empty rows big enough for a line of handwriting (below or above a
/// label), else the largest such band of columns (beside one). A small square (a check box)
/// with ink in it is already ticked or is a letter, not a blank. Ink counts anywhere in the
/// rectangle except inside other regions: the strokes of a label are lines of their own.
///
/// A side of the area that is a side of the region reaches out to the middle of the line around
/// it (the region stops short of it by the dilation and half a line); a side that faces a label
/// keeps a point away from it.
fn writing_area(label: &[u32], ink: &[bool], [w, h]: [usize; 2], rect: [usize; 4], id: u32, px_per_pt: f64) -> Option<[usize; 4]> {
    let [x0, y0, x1, y1] = rect;
    let px = |pt: f64| (pt * px_per_pt).round().max(1.0) as usize;
    let (pad, gap) = (1 + px(0.5), px(1.0));
    // `area` within `rect`: its sides on the region's sides padded, the others moved in.
    let place = |a: [usize; 4]| {
        let lo = |v: usize, edge: usize| if v == edge { v.saturating_sub(pad) } else { v + gap };
        let hi = |v: usize, edge: usize, max: usize| if v == edge { v.saturating_add(pad).min(max) } else { v.saturating_sub(gap) };
        let out = [lo(a[0], x0), lo(a[1], y0), hi(a[2], x1, w), hi(a[3], y1, h)];
        (out[0] < out[2] && out[1] < out[3]).then_some(out)
    };
    let (bw, bh) = (x1 - x0, y1 - y0);
    let mut rows = vec![0usize; bh];
    let mut cols = vec![0usize; bw];
    for y in y0..y1 {
        for x in x0..x1 {
            let i = y * w + x;
            let mine = matches!(label.get(i), Some(&l) if l == id || l == 0);
            if mine && ink.get(i).copied().unwrap_or(false) {
                if let Some(r) = rows.get_mut(y - y0) {
                    *r += 1;
                }
                if let Some(c) = cols.get_mut(x - x0) {
                    *c += 1;
                }
            }
        }
    }
    // A few specks per row are noise, not print.
    let (row_noise, col_noise) = ((bw / 200).max(1), (bh / 200).max(1));
    if rows.iter().all(|n| *n <= row_noise) {
        return place(rect);
    }
    if bw as f64 <= 24.0 * px_per_pt && bh as f64 <= 24.0 * px_per_pt {
        return None;
    }
    let longest = |counts: &[usize], allowed: usize| {
        let (mut best, mut start) = ((0, 0), None);
        for (i, c) in counts.iter().chain(std::iter::once(&usize::MAX)).enumerate() {
            match (start, *c <= allowed) {
                (None, true) => start = Some(i),
                (Some(s), false) => {
                    if i - s > best.1 - best.0 {
                        best = (s, i);
                    }
                    start = None;
                }
                _ => {}
            }
        }
        best
    };
    let (r0, r1) = longest(&rows, row_noise);
    let (c0, c1) = longest(&cols, col_noise);
    let tall = |n: usize| n as f64 >= 12.0 * px_per_pt;
    let wide = |n: usize| n as f64 >= 40.0 * px_per_pt;
    let band = (tall(r1 - r0) && wide(bw)).then_some([x0, y0 + r0, x1, y0 + r1]);
    let column = (wide(c1 - c0) && tall(bh)).then_some([x0 + c0, y0, x0 + c1, y1]);
    band.or(column).and_then(place)
}

/// A connected piece of a line mask.
struct Component {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    count: usize,
    /// The sum of the rows of its pixels (u64: on 32-bit targets it outgrows `usize`).
    y_sum: u64,
}

/// The 8-connected pieces of `mask`.
fn components(mask: &[bool], w: usize, h: usize) -> Vec<Component> {
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if !mask.get(start).copied().unwrap_or(false) || seen.get(start).copied().unwrap_or(true) {
            continue;
        }
        let mut c = Component { x0: usize::MAX, x1: 0, y0: usize::MAX, y1: 0, count: 0, y_sum: 0 };
        stack.clear();
        stack.push(start);
        if let Some(s) = seen.get_mut(start) {
            *s = true;
        }
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            c.x0 = c.x0.min(x);
            c.x1 = c.x1.max(x + 1);
            c.y0 = c.y0.min(y);
            c.y1 = c.y1.max(y + 1);
            c.count += 1;
            c.y_sum = c.y_sum.saturating_add(y as u64);
            for dy in [-1isize, 0, 1] {
                for dx in [-1isize, 0, 1] {
                    let (Some(nx), Some(ny)) = (x.checked_add_signed(dx), y.checked_add_signed(dy)) else { continue };
                    if nx >= w || ny >= h {
                        continue;
                    }
                    let j = ny * w + nx;
                    if mask.get(j).copied().unwrap_or(false)
                        && let Some(s) = seen.get_mut(j)
                        && !*s
                    {
                        *s = true;
                        stack.push(j);
                    }
                }
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A white page `w` × `h` pixels to draw a form on, two pixels to the point.
    struct Page {
        w: usize,
        h: usize,
        px: Vec<u8>,
    }

    const S: f64 = 2.0;

    impl Page {
        fn new(w: usize, h: usize) -> Self {
            Page { w, h, px: vec![255; w * h * 4] }
        }
        fn fill(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, v: u8) {
            for y in y0..y1.min(self.h) {
                for x in x0..x1.min(self.w) {
                    let i = (y * self.w + x) * 4;
                    self.px[i..i + 3].copy_from_slice(&[v, v, v]);
                }
            }
        }
        /// An outlined box, `t` pixels thick.
        fn frame(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, t: usize) {
            self.fill(x0, y0, x1, y0 + t, 0);
            self.fill(x0, y1 - t, x1, y1, 0);
            self.fill(x0, y0, x0 + t, y1, 0);
            self.fill(x1 - t, y0, x1, y1, 0);
        }
        /// Something like a word: short strokes, none a line.
        fn word(&mut self, x: usize, y: usize, letters: usize) {
            for i in 0..letters {
                self.fill(x + i * 12, y, x + i * 12 + 2, y + 14, 30);
                self.fill(x + i * 12 + 4, y + 6, x + i * 12 + 9, y + 8, 30);
            }
        }
        fn shapes(&self) -> Shapes {
            shapes(&self.px, self.w as u32, self.h as u32, S)
        }
    }

    /// Points (y up) of a pixel rectangle on a page `h` pixels high.
    fn pt(h: usize, [x0, y0, x1, y1]: [usize; 4]) -> [f64; 4] {
        [x0 as f64 / S, (h - y1) as f64 / S, x1 as f64 / S, (h - y0) as f64 / S]
    }

    fn near(a: [f64; 4], b: [f64; 4], tol: f64) -> bool {
        a.iter().zip(b).all(|(p, q)| (p - q).abs() <= tol)
    }

    #[test]
    fn a_scanned_form_gives_its_rules_boxes_and_check_boxes() {
        let mut p = Page::new(600, 400);
        // A label and the rule to write on after it.
        p.word(20, 40, 4);
        p.fill(80, 56, 400, 58, 0);
        // An empty box, and a box with a small label in its top-left corner.
        p.frame(20, 100, 420, 150, 2);
        p.frame(20, 180, 420, 260, 2);
        p.word(26, 186, 3);
        // A check box (10 pt) with a label to its right.
        p.frame(20, 300, 40, 320, 2);
        p.word(50, 303, 5);
        let s = p.shapes();
        assert_eq!(s.rules.len(), 1, "{s:?}");
        let r = s.rules[0];
        assert!((r[0] - 40.0).abs() < 1.5 && (r[2] - 200.0).abs() < 1.5 && (r[1] - (400.0 - 57.0) / S).abs() < 1.0, "{r:?}");
        assert_eq!(s.boxes.len(), 3, "{s:?}");
        // The empty box is the inside of its frame (give or take the line).
        assert!(s.boxes.iter().any(|b| near(*b, pt(400, [20, 100, 420, 150]), 2.0)), "{s:?}");
        // The labelled box: only the part below the label.
        let lower = s.boxes.iter().find(|b| (b[1] - (400.0 - 260.0) / S).abs() < 2.0).expect("labelled box");
        assert!(lower[3] <= (400.0 - 200.0) / S + 2.0 && lower[3] - lower[1] >= 12.0, "{lower:?}");
        // The check box is square and about 10 pt.
        assert!(s.boxes.iter().any(|b| near(*b, pt(400, [20, 300, 40, 320]), 1.5)), "{s:?}");
    }

    #[test]
    fn text_and_a_frame_around_the_page_are_not_shapes() {
        let mut p = Page::new(400, 300);
        for line in 0..6 {
            p.word(30, 30 + line * 30, 20);
        }
        // A frame around the whole page, and a solid dark bar (a logo, a censored part).
        p.frame(4, 4, 396, 296, 2);
        p.fill(40, 240, 360, 270, 0);
        let s = p.shapes();
        assert!(s.boxes.is_empty() && s.rules.is_empty(), "{s:?}");
    }

    #[test]
    fn noise_and_uneven_paper_do_not_hide_the_lines() {
        let mut p = Page::new(600, 300);
        p.fill(80, 56, 400, 58, 0);
        p.frame(20, 100, 420, 150, 2);
        p.frame(20, 200, 40, 220, 2);
        // Paper that darkens to the right (a shadow) and speckled with deterministic noise; the
        // lines lose some pixels.
        let mut seed: u32 = 12345;
        for y in 0..p.h {
            for x in 0..p.w {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = (seed >> 16) % 41;
                let i = (y * p.w + x) * 4;
                let v = p.px[i];
                let shade = (x * 60 / p.w) as u8;
                let v = if v == 0 { if noise < 3 { 200 } else { 40 + noise as u8 } } else { 255u8.saturating_sub(shade).saturating_sub(noise as u8) };
                p.px[i..i + 3].copy_from_slice(&[v, v, v]);
            }
        }
        let s = p.shapes();
        assert_eq!(s.rules.len(), 1, "{s:?}");
        assert_eq!(s.boxes.len(), 2, "{s:?}");
    }

    #[test]
    fn malformed_pictures_give_nothing() {
        let white = vec![255u8; 64 * 64 * 4];
        assert_eq!(shapes(&white, 64, 64, 2.0), Shapes::default());
        // Size and data disagree, sizes overflow, or the scale is nonsense.
        assert_eq!(shapes(&white, 64, 65, 2.0), Shapes::default());
        assert_eq!(shapes(&white, u32::MAX, u32::MAX, 2.0), Shapes::default());
        assert_eq!(shapes(&[], 0, 0, 2.0), Shapes::default());
        for s in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e9] {
            assert_eq!(shapes(&white, 64, 64, s), Shapes::default(), "{s}");
        }
        // All black: one dark area, no lines.
        let black: Vec<u8> = [0u8, 0, 0, 255].repeat(64 * 64);
        assert_eq!(shapes(&black, 64, 64, 2.0), Shapes::default());
    }
}
