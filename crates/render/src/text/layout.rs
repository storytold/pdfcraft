use super::device::{DIRECTION_TOLERANCE, Drawn, WORD_GAP};
use super::order::{
    COLUMN_EMS, COLUMN_LINES, COLUMN_ROWS, GUTTER_MIN, Gap, Gutters, NONE, Piece, REDRAW_CANDIDATES, Run, SAME_LINE_GAP, Scratch, Seg, fingerprint,
    ink_of, is_script_on, mark_barcode_rows, measure_cluster_gaps, parted_by_gutter, place_glyphs, redraws, restarts_run, reverse_rtl, same_line,
};
use super::{HashMap, HashSet, PageText, TextGlyph};

/// Glyphs in reading order, as indices into the page's glyphs, with their lines and word gaps.
pub(super) struct Flow {
    /// The top of the flow's topmost glyph in view space.
    pub(super) top: f32,
    /// The quarter turn whose frame the flow was laid out in.
    pub(super) quarter: Option<u8>,
    pub(super) ids: Vec<usize>,
    pub(super) line_of: Vec<u32>,
    pub(super) space_before: Vec<bool>,
}

/// Lay out the page's glyphs `ids`, giving each the box at the same position in `rects`.
pub(super) fn arrange_flow(glyphs: &[Drawn], ids: Vec<usize>, rects: Vec<[f32; 4]>, quarter: Option<u8>) -> Flow {
    let top = ids.iter().filter_map(|&i| glyphs.get(i)).map(|g| g.2.rect[1]).fold(f32::INFINITY, f32::min);
    let framed: Vec<TextGlyph> = ids.iter().zip(rects).filter_map(|(&i, rect)| glyphs.get(i).map(|g| TextGlyph { rect, ..g.2.clone() })).collect();
    let (order, line_of, space_before) = arrange(&framed);
    let ids = order.into_iter().filter_map(|k| ids.get(k).copied()).collect();
    Flow { top, quarter, ids, line_of, space_before }
}

/// Boxes for a run of glyphs set along a curve: each glyph's box along its own direction, placed
/// where the pen moved in the previous glyph's direction. The path straightened out.
pub(super) fn unrolled<'g>(run: impl Iterator<Item = &'g Drawn>) -> Vec<[f32; 4]> {
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
pub(super) fn to_upright(r: [f32; 4], quarter: u8, w: f32, h: f32) -> [f32; 4] {
    let [x0, y0, x1, y1] = r;
    match quarter {
        1 => [y0, w - x1, y1, w - x0],
        2 => [w - x1, h - y1, w - x0, h - y0],
        3 => [h - y1, x0, h - y0, x1],
        _ => r,
    }
}

/// Whether two writing directions are close enough to share a frame ([`DIRECTION_TOLERANCE`]).
pub(super) fn compatible(a: [f64; 2], b: [f64; 2]) -> bool {
    a[0] * b[0] + a[1] * b[1] > 0.0 && (a[0] * b[1] - a[1] * b[0]).abs() <= DIRECTION_TOLERANCE
}

/// Whether glyph `g` continues the line of glyph `p` (content order), by the test `layout` uses
/// for consecutive glyphs, taken in `p`'s own frame. The direction may turn by less than half a
/// quarter turn, as text set along a curve does.
pub(super) fn continues(p: &Drawn, g: &Drawn) -> bool {
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
pub(super) fn along_baseline(quad: &[[f32; 2]; 4], u: [f64; 2]) -> [f64; 4] {
    let p = quad.map(|[x, y]| [f64::from(x) * u[0] + f64::from(y) * u[1], f64::from(y) * u[0] - f64::from(x) * u[1]]);
    let (a0, a1) = ((p[0][0] + p[3][0]) / 2.0, (p[1][0] + p[2][0]) / 2.0);
    let (c0, c1) = p.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| (lo.min(q[1]), hi.max(q[1])));
    [a0.min(a1), c0, a0.max(a1), c1]
}

/// The inverse of [`to_upright`].
pub(super) fn from_upright(r: [f32; 4], quarter: u8, w: f32, h: f32) -> [f32; 4] {
    let [u0, v0, u1, v1] = r;
    match quarter {
        1 => [w - v1, u0, w - v0, u1],
        2 => [w - u1, h - v1, w - u0, h - v0],
        3 => [v0, h - u1, v1, h - u0],
        _ => r,
    }
}

pub(super) fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
}

/// Scripts written with explicit spaces whose combining signs can sit apart from their base
/// (Indic, Thai, Lao, Myanmar, Khmer): never infer a space next to such a sign in a tight cluster.
fn is_complex(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x0900..=0x0DFF | 0x0E00..=0x0EFF | 0x1000..=0x109F | 0x1780..=0x17FF)
}

/// Whether `text` is only combining marks (Unicode Mn and Me, which take bidi class NSM) of a
/// left-to-right script: Thai tone marks and above or below vowels, Indic signs such as virama,
/// Latin combining accents. Right-to-left marks are ordered with their script by [`reverse_rtl`].
fn is_mark(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM && !is_rtl(c))
}

/// How far from the glyph before it, as a fraction of its height, a combining mark may start and
/// still ride on it. A zero-advance mark starts at the pen after its base, give or take a small
/// mark-positioning offset, and its ink is drawn back over the base.
const MARK_REACH: f32 = 0.3;

/// Whether glyph `m`, drawn right after `p` in one run, is a combining mark on `p`: ISO 32000-2
/// §9.10 leaves extraction to the processor, and Unicode stores a mark after its base, as the
/// content stream draws it here. The mark then stays after `p` in reading order however far its
/// own box sits along the line; sorting by x alone moved a Thai tone mark that starts a hair past
/// its base's advance after the next letter (#6517).
fn rides_on(p: &TextGlyph, m: &TextGlyph) -> bool {
    let reach = MARK_REACH * (m.rect[3] - m.rect[1]).max(0.1);
    is_mark(&m.text)
        && p.text.chars().any(|c| !c.is_whitespace())
        && !p.text.chars().any(is_rtl)
        && m.rect[0] >= p.rect[0] - reach
        && m.rect[0] <= p.rect[2] + reach
}

/// How far, as a fraction of its letter's height, a combining mark's em box may sit above or below the letter's and
/// still ride on it. A tone mark lifted over an upper vowel can be drawn more than half an em higher, past where a
/// glyph of its own would start a new line.
const MARK_RISE: f32 = 1.0;

/// Whether glyph `m`, drawn right after glyph `p` (a letter, or a mark riding on letter `a`), rides on that cluster:
/// on `p` itself, or, for a mark stacked on another one, on its letter `a`.
fn rides_on_cluster(glyphs: &[&TextGlyph], p: usize, a: usize, m: &TextGlyph) -> bool {
    glyphs.get(p).is_some_and(|pg| rides_on(pg, m)) || (a != p && glyphs.get(a).is_some_and(|ag| rides_on(ag, m)))
}

/// Glyph `i` continues the run of glyph `i - 1`: when it is a combining mark on that glyph's cluster, it joins it.
fn attach_mark(glyphs: &[&TextGlyph], anchor: &mut [usize], i: usize) {
    let Some(p) = i.checked_sub(1) else { return };
    let Some(&a) = anchor.get(p) else { return };
    if let Some(mg) = glyphs.get(i)
        && rides_on_cluster(glyphs, p, a, mg)
        && let Some(slot) = anchor.get_mut(i)
    {
        *slot = a;
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3134F | 0x1100..=0x11FF)
}

pub(super) fn union(a: &mut [f32; 4], b: &[f32; 4]) {
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
///    Segments on the same baseline that nearly touch are merged (text drawn out of order), and so
///    is a table cell padded with spaces with the next cell of its row (`PADDED_FIELD`).
///    Nothing is joined across the gutter between two columns of running text (#2387), down to 0.6
///    line heights wide with three lines of running text on each side (`gutter_in`): a segment that
///    runs across one is cut there, and neither a line nor a sub- or superscript is merged across one,
///    but for a table cell padded with spaces up to the next cell of its row.
/// 2. *Blocks:* segments stacked with line-sized gaps and overlapping horizontally.
/// 3. *Block order:* repeatedly take the top-most remaining block, preferring a block to its left
///    that overlaps it vertically (so columns read left, then right). A column goes on first (#3658):
///    the nearest block below the last one, overlapping it horizontally and at most 2.5 heights of
///    its last line below it, comes next, unless an unread block sits above it in its own span, sits
///    beside it on its left, or shares its first line (the first lines overlap by at least 30% of the
///    taller one: a table or form row, which reads across, even with labels and values in different
///    sizes or on shifted baselines).
///    Between two columns of running text a gutter says more (#3682). First lines that sit level
///    across it are two paragraphs, not a row. A gap of more than 2.5 heights is a figure in the
///    column, which goes on below it, when the column beside has text all the way down the gap. An
///    unread block above the next one, on the column's side of the gutter, is the column's own (the
///    words of a justified line set far apart, the parts of a formula) and comes first. A block of
///    blanks holds up no column.
///    Known limit: without running text on both sides of a gutter (short lines, or fewer than three),
///    a paragraph in the next column that starts on the same line as the continuation (text on a
///    baseline grid) counts as a row and the top-most rule applies, so the column order of #3658 is
///    lost there.
///    The rows of a barcode drawn with a font (a QR or Aztec code) join no other text and are read
///    after everything else, top to bottom.
/// 4. Within a segment glyphs run left to right; runs of right-to-left script are reversed so the
///    text comes out in logical order. A block whose first line sits on the baseline of the line read
///    just before it, to its right and less than [`SAME_LINE_GAP`] heights away, continues that line
///    after a space (a label and its value, words drawn out of order or in another size). A block of
///    several lines does so only after a one-line block narrower than [`SAME_LINE_GAP`] heights (a
///    label): beside a wider one, or the last line of a column, it is the next column. No block
///    continues a line across a gutter.
pub fn layout(glyphs: Vec<TextGlyph>) -> PageText {
    let (order, line_of, space_before) = arrange(&glyphs);
    let mut slots: Vec<Option<TextGlyph>> = glyphs.into_iter().map(Some).collect();
    // Each glyph is emitted exactly once.
    let glyphs = order.iter().filter_map(|i| slots.get_mut(*i).and_then(Option::take)).collect();
    PageText { glyphs, line_of, space_before, truncated: false }
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
    // `anchor[i]`: the glyph a combining mark rides on (the first glyph of its cluster), or `i`.
    let mut anchor: Vec<usize> = (0..n).collect();
    let mut run_start_x = 0.0;
    let mut furthest_progress = 0.0;
    for i in 0..n {
        let g = glyphs[i].rect;
        let hi = height(i);
        let cont = segs.last().is_some_and(|s| {
            let Some(last) = s.first.end.checked_sub(1) else { return false };
            // A glyph after a combining mark continues from the mark's letter, and a mark on that letter's cluster
            // stays on its line even when drawn raised or lowered by up to an em (`MARK_RISE`).
            let p = anchor.get(last).copied().unwrap_or(last);
            if rides_on_cluster(&glyphs, last, p, glyphs[i]) && (cy(i) - cy(p)).abs() < height(p) * MARK_RISE {
                return true;
            }
            let ph = height(p).min(hi);
            let sh = s.h.min(hi);
            let gap = g[0] - glyphs[p].rect[2];
            // A sub- or superscript is a smaller glyph on a shifted baseline: its em box centre sits off the line by
            // more than half its own height, but by less than half the height of the full-size glyph beside it.
            let centre_limit = if gap.abs() < ph { height(p).max(hi) * 0.5 } else { ph * 0.5 };
            let same_line = ((cy(i) - cy(p)).abs() < centre_limit && gap < ph * 3.0)
                || ((cy(i) - s.cy()).abs() < sh * 0.3 && (s.h / hi - 1.0).abs() < 0.35 && gap < sh * 3.0);
            let on_seg = gap < s.h * 1.2 && (is_script_on(&s.bbox, s.h, &g, hi) || is_script_on(&g, hi, &s.bbox, s.h));
            (same_line || on_seg) && g[0] > glyphs[p].rect[0] - sh * 2.0
        });
        let reset = cont && segs.last().is_some_and(|s| restarts_run(&glyphs, s.first.start, i, furthest_progress));
        if cont
            && !reset
            && let Some(s) = segs.last_mut()
        {
            attach_mark(&glyphs, &mut anchor, i);
            s.first.end = i + 1;
            s.extend_box(&g, hi);
            s.add_ink(glyphs[i]);
            furthest_progress = furthest_progress.max((g[0] - run_start_x).abs()).max((g[2] - run_start_x).abs());
        } else {
            segs.push(Seg::new(Run { start: i, end: i + 1 }, g, hi, ink_of(glyphs[i])));
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
    mark_barcode_rows(&mut segs);
    // Nothing below joins text across the gutter between two columns of running text (#2387).
    let mut gutters = Gutters::new();
    split_at_gutters(&glyphs, &height, &mut segs, &mut gutters);
    // Merge same-baseline segments that nearly touch, and a padded cell with the next of its row, then attach any remaining superscript or
    // subscript segments to the closest host line segment they sit on.
    // The pairs of segments found parted by a gutter, each by its first glyph: every merge starts the pass over, and
    // a pair is looked at once.
    let mut parted: HashSet<(usize, usize)> = HashSet::new();
    find_gutters(&segs, &mut parted, &mut gutters);
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for a in 0..segs.len() {
            for b in 0..segs.len() {
                if a == b {
                    continue;
                }
                let (sa, sb) = (&segs[a], &segs[b]);
                if nearly_touch(sa, sb) && (padded_cells(sa, sb) || !gutter_parts(&segs, sa, sb, &mut parted, &mut gutters)) {
                    let sb = segs.remove(b);
                    let a = if b < a { a - 1 } else { a };
                    segs[a].merge(sb);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            let mut b = 0;
            while b < segs.len() {
                let sb = &segs[b];
                let best_host = (0..segs.len())
                    .filter(|&a| {
                        if a == b {
                            return false;
                        }
                        let sa = &segs[a];
                        let gap = (sb.bbox[0] - sa.bbox[2]).max(sa.bbox[0] - sb.bbox[2]);
                        sa.bbox[2] - sa.bbox[0] > sb.bbox[2] - sb.bbox[0]
                            && gap < sa.h * 1.2
                            && sa.barcode == sb.barcode
                            && is_script_on(&sa.bbox, sa.h, &sb.bbox, sb.h)
                            && !parted_by_gutter(&segs, sa, sb, &mut gutters)
                    })
                    .min_by(|&a1, &a2| (segs[a1].cy() - sb.cy()).abs().total_cmp(&(segs[a2].cy() - sb.cy()).abs()));
                if let Some(a) = best_host {
                    let sb = segs.remove(b);
                    let a = if b < a { a - 1 } else { a };
                    segs[a].merge(sb);
                    merged = true;
                } else {
                    b += 1;
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
            vgap > -last.h * 0.5
                && vgap < last.h.max(s.h) * 1.1
                && overlap > minw * 0.3
                && (last.h / s.h - 1.0).abs() < 0.6
                && last.barcode == s.barcode
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
    // A barcode drawn with a font is a picture, not text to read: it comes after the page's text
    // and does not hold up the column beside it.
    let (mut remaining, barcodes): (Vec<usize>, Vec<usize>) =
        (0..blocks.len()).partition(|&b| blocks[b].0.first().and_then(|&s| segs.get(s)).is_none_or(|s| !s.barcode));
    let mut block_order: Vec<usize> = Vec::with_capacity(blocks.len());
    // A block to the left of `cb` that shares at least half the height of the shorter one: it reads first.
    let beside_left = |cb: &[f32; 4], lb: &[f32; 4]| {
        let v = lb[3].min(cb[3]) - lb[1].max(cb[1]);
        lb[2] <= cb[0] + 1.0 && v > 0.5 * (lb[3] - lb[1]).min(cb[3] - cb[1])
    };
    // The box of a block's first line. Every block is created with one segment, so the block's own box is a fallback
    // that is never used.
    let first_line = |b: usize| blocks[b].0.first().and_then(|&s| segs.get(s)).map_or(blocks[b].1, |s| s.bbox);
    let first_seg = |b: usize| blocks.get(b).and_then(|(members, _)| members.first()).and_then(|&s| segs.get(s));
    // Whether two blocks whose first lines sit level are parted by a gutter, found once for each pair.
    let mut level_parted: HashMap<(usize, usize), bool> = HashMap::new();
    // Whether a block has any visible glyph.
    let inked: Vec<bool> = blocks.iter().map(|(members, _)| members.iter().any(|&s| segs.get(s).is_some_and(|s| s.ink.is_finite()))).collect();
    let ink_of_block = block_inks(&blocks, &segs);
    // The columns the gutters found so far part, worked out again when another gutter is found.
    let mut columns: Bands = (usize::MAX, Vec::new());
    while let Some(&top) = remaining.iter().min_by(|a, b| blocks[**a].1[1].total_cmp(&blocks[**b].1[1])) {
        // A column goes on past a paragraph gap (#3658): the nearest block below the one just read that overlaps it
        // horizontally comes next, if the gap above it is at most two and a half heights of the last line read (a
        // footer, a footnote or a caption under a figure is further) and no other unread block
        // - sits above it in its own horizontal span (the rest of a column above a full-width heading or footnote),
        // - is beside it on its left, or
        // - shares its first line: their first lines overlap by at least 30% of the taller one (a table or form row,
        //   which reads across, even with labels and values in different sizes or on shifted baselines; a line that
        //   only touches it, or a small superscript or footnote marker, is not a row).
        // Where a gutter parts two columns of running text (#3682), the column goes on in three more cases: past
        // first lines level across the gutter, past a figure the column beside runs on past, and to a block of its
        // own above the next one.
        let next_in_column = block_order.last().and_then(|&prev| {
            let pb = blocks[prev].1;
            // The gap is measured in the height of the block's last line, which borders it.
            let pl = blocks[prev].0.last().and_then(|&s| segs.get(s)).map_or(pb, |s| s.bbox);
            // Its glyphs' height: a formula's line is taller than its text.
            let ph = blocks[prev].0.last().and_then(|&s| segs.get(s)).map_or(pl[3] - pl[1], |s| s.h);
            let tol = (pl[3] - pl[1]).max(0.1) * 0.5;
            let c = remaining
                .iter()
                .copied()
                .filter(|&c| {
                    let cb = blocks[c].1;
                    let overlap = cb[2].min(pb[2]) - cb[0].max(pb[0]);
                    let minw = (cb[2] - cb[0]).min(pb[2] - pb[0]).max(1.0);
                    // A block of blanks is not where a column goes on: the space a word processor draws for an
                    // empty paragraph beside a figure would lead it away from the caption below.
                    cb[1] >= pb[3] - tol && overlap > minw * 0.3 && inked.get(c).copied().unwrap_or(true)
                })
                .min_by(|a, b| blocks[*a].1[1].total_cmp(&blocks[*b].1[1]))?;
            let cb = blocks[c].1;
            let column = (pb[0].min(cb[0]), pb[2].max(cb[2]));
            // A gap wider than that is a figure in the column, and the column goes on below it, when the column
            // beside it has running text all the way down the gap, across a gutter.
            let gutter = if cb[1] - pb[3] >= tol * 5.0 { Some(gutter_beside(&segs, column, (pb[3], cb[1]), (&pl, ph), &mut gutters)?) } else { None };
            let across = |a: &[f32; 4], b: &[f32; 4]| {
                gutter.is_some_and(|(lo, hi)| {
                    let mid = (lo + hi) / 2.0;
                    (a[2] <= mid && b[0] >= mid) || (b[2] <= mid && a[0] >= mid)
                })
            };
            let cl = first_line(c);
            // The top-most unread block above the candidate in its span, if nothing else holds the candidate up.
            let mut above: Option<usize> = None;
            for &r in &remaining {
                // A block of blanks (the spaces a word processor draws for empty paragraphs) holds up no column.
                if r == c || !inked.get(r).copied().unwrap_or(true) {
                    continue;
                }
                let rb = blocks[r].1;
                let rl = first_line(r);
                let shared = rl[3].min(cl[3]) - rl[1].max(cl[1]);
                // Two paragraphs or headings that start level across a gutter, one in each column, are no row.
                let row = shared > 0.3 * (rl[3] - rl[1]).max(cl[3] - cl[1])
                    && !across(&cl, &rl)
                    && !*level_parted
                        .entry((c.min(r), c.max(r)))
                        .or_insert_with(|| first_seg(c).zip(first_seg(r)).is_some_and(|(a, b)| parted_by_gutter(&segs, a, b, &mut gutters)));
                if beside_left(&cb, &rb) || row {
                    return None;
                }
                // The top-most, and of two level with each other the one on the left: the words of one line.
                let before = |ab: &[f32; 4]| rb[1] < ab[1] || (rb[1] == ab[1] && rb[0] < ab[0]);
                if rb[1] < cb[1] && rb[2].min(cb[2]) > rb[0].max(cb[0]) && above.is_none_or(|a| blocks.get(a).is_none_or(|(_, ab)| before(ab))) {
                    above = Some(r);
                }
            }
            let Some(above) = above else { return Some(c) };
            // A block above the candidate on the column's side of the gutter is the column's own (the words of a
            // justified line set far apart, the parts of a formula), and comes first. One across the gutter or
            // on the other side of it is another column's.
            let (lo, hi) = gutter.or_else(|| gutter_beside(&segs, column, (pl[1], cb[3]), (&pl, ph), &mut gutters))?;
            let (mid, ab) = ((lo + hi) / 2.0, blocks.get(above)?.1);
            ((pb[2] <= mid && ab[2] <= mid) || (pb[0] >= mid && ab[0] >= mid)).then_some(above)
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
        // A column is read to its end before the next one (#3682).
        let pick =
            column_first((&blocks, &segs), (&ink_of_block, &inked), &remaining, (&mut columns, &mut gutters), block_order.last().copied(), pick);
        remaining.retain(|r| *r != pick);
        block_order.push(pick);
    }
    block_order.extend(barcodes);

    // 4. Emit each segment in block order. Its glyphs are placed here, into scratch that every segment reuses, and
    // appended straight to the output. Blocks and their order use only segment boxes, so placement can wait until now.
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut line_of: Vec<u32> = Vec::with_capacity(n);
    let mut space_before: Vec<bool> = Vec::with_capacity(n);
    let mut line = 0u32;
    let mut sc = Scratch { anchor, ..Default::default() };
    // `spaces[w]`: a word gap left of the w-th glyph of the segment, by position on the line, not by drawing order
    // (#1062). Reused by every segment.
    let mut spaces: Vec<bool> = Vec::new();
    // The segment emitted last, whether it had right-to-left text, and whether its block is that one line.
    let mut last: Option<(usize, bool, bool)> = None;
    for b in block_order {
        for (k, &si) in blocks[b].0.iter().enumerate() {
            place_glyphs(&glyphs, &height, &segs[si], &mut sc);
            let h = segs[si].h;
            measure_cluster_gaps(&glyphs, &mut sc);
            // The text of the cluster that glyph `i` belongs to: its base's.
            let cluster_text = |i: usize| sc.anchor.get(i).and_then(|&a| glyphs.get(a)).map_or("", |g| g.text.as_str());
            // Word gaps relative to this line's typical letter gap (handles tracking).
            // Long runs of repeated punctuation belong to leaders, not letter tracking: a TOC leader
            // must not raise the threshold above the word gaps in its title (#3086).
            sc.gaps.clear();
            sc.gaps.extend(sc.placed.windows(2).enumerate().filter_map(|(w, pair)| {
                let &[p, c] = pair else { return None };
                let gap = (*sc.cluster_gap.get(w + 1)?)?;
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
                Some(gap)
            }));
            sc.gaps.sort_by(f32::total_cmp);
            let typical = sc.gaps.get(sc.gaps.len() / 3).copied().unwrap_or(0.0).max(0.0);
            let threshold = (typical + h * WORD_GAP).max(h * WORD_GAP);
            spaces.clear();
            spaces.resize(sc.placed.len(), false);
            for (w, pair) in sc.placed.windows(2).enumerate() {
                let &[p, c] = pair else { continue };
                let Some(&Some(gap)) = sc.cluster_gap.get(w + 1) else { continue };
                let (pt, ct) = (cluster_text(p), cluster_text(c));
                let cjk = pt.chars().any(is_cjk) && ct.chars().any(is_cjk) && gap < h * 0.5;
                // A combining sign that rides on no glyph before it (drawn apart from its base, or in a run of its own)
                // may sit anywhere over its base, so it never takes a space next to it. Two base letters of these
                // scripts are measured like any others: a Thai or Indic word gap is narrower than half an em.
                let loose_sign = |t: &str| is_mark(t) && t.chars().any(is_complex);
                let tight_cluster = (loose_sign(pt) || loose_sign(ct)) && gap < h * 0.6;
                let run_start = sc.breaks.binary_search(&c).is_ok();
                if let Some(space) = spaces.get_mut(w + 1) {
                    *space = (gap > threshold || run_start)
                        && !cjk
                        && !tight_cluster
                        && !pt.trim().is_empty()
                        && !ct.trim().is_empty()
                        && !pt.ends_with(char::is_whitespace)
                        && !ct.starts_with(char::is_whitespace);
                }
            }
            // Right-to-left spans are reversed (visual → logical) with their gaps, and only then is a second copy made.
            let has_rtl = sc.placed.iter().any(|&i| glyphs[i].text.chars().any(is_rtl));
            if has_rtl {
                sc.logical.clear();
                sc.logical.extend_from_slice(&sc.placed);
                reverse_rtl(&glyphs, &sc.anchor, &mut sc.logical, &mut spaces);
            }
            let idx: &[usize] = if has_rtl { &sc.logical } else { &sc.placed };
            let joined = joins_line(&segs, last, si, k == 0, blocks[b].0.len() > 1, has_rtl, &mut gutters);
            let start = order.len();
            order.extend_from_slice(idx);
            // `spaces[0]` stays false: the first glyph of a segment never takes a space, unless it joins a line.
            space_before.extend(spaces.iter().take(idx.len()));
            space_before.resize(order.len(), false);
            line = put_on_line(&mut line_of, &mut space_before, start..order.len(), line, &segs[si], joined);
            last = Some((si, has_rtl, blocks[b].0.len() == 1));
        }
    }
    (order.into_iter().map(|i| kept[i]).collect(), line_of, space_before)
}

/// Puts the reading-order glyphs `glyphs` of segment `seg` on their line: `line`, or the line
/// before it when `seg` continues the line of `joined` (see [`joins_line`]), with a space before
/// its first glyph when the two are a word gap apart. Returns the line the next segment starts on.
fn put_on_line(line_of: &mut Vec<u32>, space_before: &mut [bool], glyphs: std::ops::Range<usize>, line: u32, seg: &Seg, joined: Option<&Seg>) -> u32 {
    let Some(p) = joined else {
        line_of.resize(glyphs.end, line);
        return line.saturating_add(1);
    };
    line_of.resize(glyphs.end, line.saturating_sub(1));
    if let Some(space) = space_before.get_mut(glyphs.start) {
        *space = seg.bbox[0] - p.bbox[2] > WORD_GAP * p.h.min(seg.h);
    }
    line
}

/// The segment read just before (`last`, with whether it had right-to-left text) whose line segment
/// `si` goes on, when `si` is the first line of its block (`first`) and sits on that line's baseline
/// to its right (a label and its value, a word drawn out of order or set in another size).
/// Same-baseline segments are always in different blocks. A column of several lines is read after
/// the column to its left ends, so their lines don't meet. A paragraph of several lines that starts
/// beside a line as wide as [`SAME_LINE_GAP`] heights or more is the next column, not that line's
/// value, so it starts a line of its own; only a narrower line that is a block of its own (a label)
/// takes it: the short last line of a column of several lines is no label (review on #4468). One-line
/// paragraphs of two columns that share a baseline cannot be told from a table row, and are further
/// apart than [`same_line`] allows unless the left one fills its column. Right-to-left lines are
/// left alone: their blocks still read left first. A row of a barcode drawn with a font joins no
/// line of text, nor text a code row.
fn joins_line<'s>(
    segs: &'s [Seg],
    last: Option<(usize, bool, bool)>,
    si: usize,
    first: bool,
    paragraph: bool,
    has_rtl: bool,
    gutters: &mut Gutters,
) -> Option<&'s Seg> {
    let (p, p_rtl, p_alone) = last?;
    if !first || p_rtl || has_rtl {
        return None;
    }
    let (p, s) = (segs.get(p)?, segs.get(si)?);
    let label = p_alone && p.bbox[2] - p.bbox[0] < SAME_LINE_GAP * p.h;
    (same_line(p, s) && (!paragraph || label) && p.barcode == s.barcode && !parted_by_gutter(segs, p, s, gutters)).then_some(p)
}

/// Whether segments `a` and `b` sit on one baseline, in sizes alike, and nearly touch, or `a` is a cell padded
/// with spaces towards `b` ([`Seg::padded_towards`]): the two are merged into one line, unless a gutter parts them.
fn nearly_touch(a: &Seg, b: &Seg) -> bool {
    let h = a.h.min(b.h);
    let gap = (b.bbox[0] - a.bbox[2]).max(a.bbox[0] - b.bbox[2]);
    let joined = (gap < h * 1.2 || a.padded_towards(b, h)) && a.barcode == b.barcode;
    (a.cy() - b.cy()).abs() < h * 0.3 && (a.h / b.h - 1.0).abs() < 0.35 && joined
}

/// Whether one of segments `a` and `b` is a table cell padded with spaces towards the other ([`Seg::padded_towards`]):
/// the two are a row, merged whatever gap the padding leaves after the cell's text.
fn padded_cells(a: &Seg, b: &Seg) -> bool {
    let h = a.h.min(b.h);
    a.padded_towards(b, h) || b.padded_towards(a, h)
}

/// Looks for the page's gutters between the segments that would be merged, before any is: a gutter found between
/// two lines also parts the lines that have too little text around them to show it, whichever of them the merging
/// comes to first.
fn find_gutters(segs: &[Seg], parted: &mut HashSet<(usize, usize)>, gutters: &mut Gutters) {
    for (k, a) in segs.iter().enumerate() {
        for b in segs.iter().skip(k + 1) {
            if (nearly_touch(a, b) || nearly_touch(b, a)) && !padded_cells(a, b) {
                gutter_parts(segs, a, b, parted, gutters);
            }
        }
    }
}

/// Whether segments `a` and `b`, on one baseline, are parted by a gutter ([`parted_by_gutter`]), as `parted`
/// remembers it by the first glyph of each: a pair found parted is not looked at again.
fn gutter_parts(segs: &[Seg], a: &Seg, b: &Seg, parted: &mut HashSet<(usize, usize)>, gutters: &mut Gutters) -> bool {
    let pair = (a.first.start.min(b.first.start), a.first.start.max(b.first.start));
    if parted.contains(&pair) {
        return true;
    }
    let found = parted_by_gutter(segs, a, b, gutters);
    if found {
        parted.insert(pair);
    }
    found
}

/// Cuts each segment where it runs across the gutter between two columns of running text ([`Gutters::between`]).
/// A run continues across less than three heights, so the end of a line in one column and the start of a line
/// drawn right after it on the same baseline in the next column, less than that apart, are one segment: a page
/// drawn row by row has every line of both columns in one.
fn split_at_gutters(glyphs: &[&TextGlyph], height: &dyn Fn(usize) -> f32, segs: &mut Vec<Seg>, gutters: &mut Gutters) {
    // A forward jump as wide as a gutter between two visible glyphs drawn one after the other: the segment, the
    // glyph after the jump, and the pieces before and after it.
    struct Jump {
        seg: usize,
        at: usize,
        before: usize,
    }
    // Every segment in pieces: its glyphs up to each jump. The piece after a jump is the next one of its segment.
    let mut pieces: Vec<Piece> = Vec::new();
    let mut jumps: Vec<Jump> = Vec::new();
    for (si, s) in segs.iter().enumerate() {
        let Some(whole) = s.piece() else { continue };
        let first = pieces.len();
        // The glyph drawn last, and the piece its visible glyphs are in.
        let mut last: Option<&TextGlyph> = None;
        let mut piece: Option<Piece> = None;
        for i in s.first.start..s.first.end {
            let Some(g) = glyphs.get(i).copied() else { continue };
            let before = last.replace(g);
            if g.text.trim().is_empty() {
                continue;
            }
            // From the glyph drawn before, a space too: the space between two words is no jump (a typewriter
            // face's space is as wide as the narrowest gutter, and a justified line's extra spacing is not part of
            // a space's box). So a space that ends a line and reaches into the gutter of columns drawn row by row
            // in one run narrows it. Measured in the segment's height: a superscript or a bullet is smaller than
            // the line it is on.
            if let Some(before) = before
                && g.rect[0] - before.rect[2] >= s.h * GUTTER_MIN
                && let Some(done) = piece.take()
            {
                jumps.push(Jump { seg: si, at: i, before: pieces.len() });
                pieces.push(done);
            }
            piece = Some(match piece {
                Some(p) => Piece { x0: p.x0.min(g.rect[0]), ink: p.ink.max(g.rect[2]), cy: whole.cy },
                None => Piece { x0: g.rect[0], ink: g.rect[2], cy: whole.cy },
            });
        }
        pieces.extend(piece);
        if pieces.len() == first {
            pieces.push(whole);
        }
    }
    // Twice over: a gutter found at one jump also parts the jumps before it that have too little text around them.
    let mut cut = vec![false; jumps.len()];
    for _ in 0..2 {
        for (j, cut) in jumps.iter().zip(&mut cut) {
            let (Some(s), Some(before), Some(after)) = (segs.get(j.seg), pieces.get(j.before), pieces.get(j.before + 1)) else { continue };
            let gap = Gap { start: before.x0, from: before.ink, to: after.x0, end: after.ink };
            *cut = *cut || gutters.between(pieces.iter().copied(), gap, s.cy(), s.h).is_some();
        }
    }
    if !cut.contains(&true) {
        return;
    }
    // The jumps are in segment order, and in glyph order within a segment.
    let mut cuts = jumps.iter().zip(&cut).filter(|(_, cut)| **cut).map(|(j, _)| j).peekable();
    let mut out: Vec<Seg> = Vec::with_capacity(segs.len() + jumps.len());
    for (si, s) in segs.drain(..).enumerate() {
        let mut start = s.first.start;
        let mut parts = 0;
        while let Some(j) = cuts.next_if(|j| j.seg == si) {
            out.extend(seg_of(glyphs, height, Run { start, end: j.at }));
            start = j.at;
            parts += 1;
        }
        if parts == 0 {
            out.push(s);
        } else {
            out.extend(seg_of(glyphs, height, Run { start, end: s.first.end }));
        }
    }
    *segs = out;
}

/// The segment of the glyphs `run`, drawn one after another on one line, as [`arrange`] builds it glyph by glyph.
fn seg_of(glyphs: &[&TextGlyph], height: &dyn Fn(usize) -> f32, run: Run) -> Option<Seg> {
    let mut seg: Option<Seg> = None;
    for i in run.start..run.end {
        let Some(g) = glyphs.get(i) else { continue };
        match seg.as_mut() {
            Some(s) => {
                s.first.end = i + 1;
                s.extend_box(&g.rect, height(i));
                s.add_ink(g);
            }
            None => seg = Some(Seg::new(Run { start: i, end: i + 1 }, g.rect, height(i), ink_of(g))),
        }
    }
    seg
}

/// A block: its segments, and the box around them.
type Block = (Vec<usize>, [f32; 4]);

/// [`column_bands`], with how many gutters they were worked out from.
type Bands = (usize, Vec<[f32; 3]>);

/// Each block's visible text, from its left edge to its right: the blanks that end a line reach into the gutter.
/// A block of blanks spans its box.
fn block_inks(blocks: &[Block], segs: &[Seg]) -> Vec<(f32, f32)> {
    blocks
        .iter()
        .map(|(members, bb)| {
            let ink = members.iter().filter_map(|&s| segs.get(s)).filter(|s| s.ink.is_finite());
            let (x0, x1) = ink.fold((f32::INFINITY, f32::NEG_INFINITY), |(x0, x1), s| (x0.min(s.ink_start), x1.max(s.ink)));
            if x0 <= x1 { (x0, x1) } else { (bb[0], bb[2]) }
        })
        .collect()
}

/// [`column_bands`] of the gutters `found`, kept in `columns` with how many gutters they were worked out from:
/// worked out again when another gutter has been found.
fn column_bands_again(columns: &mut Bands, segs: &[Seg], found: &[[f32; 4]]) {
    if columns.0 != found.len() {
        *columns = (found.len(), column_bands(segs, found));
    }
}

/// The block to read next where `pick` is the block order's choice after block `last`: `pick`, or the block that
/// [`before_next_column`] puts before it. A block whose first line is level with the one just read is the next
/// cell of its row, and stays `pick`, when no gutter is found between the two, or when the rows go on
/// ([`rows_go_on`]): the one just read is at most [`COLUMN_LINES`] lines, and the next block below it has a block
/// level with it beside `pick` too (a table of cells several lines long). A column's paragraph followed by a
/// formula is no row, however short: the formula has nothing level with it in the next column.
fn column_first(
    (blocks, segs): (&[Block], &[Seg]),
    inks: (&[(f32, f32)], &[bool]),
    remaining: &[usize],
    (columns, gutters): (&mut Bands, &mut Gutters),
    last: Option<usize>,
    pick: usize,
) -> usize {
    let first_seg = |b: usize| blocks.get(b).and_then(|(members, _)| members.first()).and_then(|&s| segs.get(s));
    if let Some((last, (a, b))) = last.and_then(|l| Some((l, first_seg(l)?))).and_then(|(l, a)| Some((l, (a, first_seg(pick)?))))
        && level(&a.bbox, &b.bbox)
        && (!parted_by_gutter(segs, a, b, gutters) || rows_go_on((blocks, segs), inks.1, remaining, last, pick))
    {
        return pick;
    }
    column_bands_again(columns, segs, gutters.found());
    before_next_column(blocks, inks, remaining, &columns.1, pick)
}

/// Whether two lines' boxes are level: they share at least 30% of the taller one's height.
fn level(a: &[f32; 4], b: &[f32; 4]) -> bool {
    a[3].min(b[3]) - a[1].max(b[1]) > 0.3 * (a[3] - a[1]).max(b[3] - b[1])
}

/// Whether block `last`, just read, and block `pick`, level with it beside it, are cells of rows that go on: `last`
/// is at most [`COLUMN_LINES`] lines, and the nearest unread block below it in its own span has an unread block
/// level with its first line in the span of `pick`. Blocks of blanks (`inked` false: the empty paragraphs a word
/// processor draws) are no cells, as they hold up no column in the block order.
fn rows_go_on((blocks, segs): (&[Block], &[Seg]), inked: &[bool], remaining: &[usize], last: usize, pick: usize) -> bool {
    let (Some((members, lb)), Some((_, pb))) = (blocks.get(last), blocks.get(pick)) else { return false };
    if members.len() > COLUMN_LINES {
        return false;
    }
    let first_line = |b: usize| blocks.get(b).and_then(|(m, _)| m.first()).and_then(|&s| segs.get(s)).map(|s| s.bbox);
    let across = |a: &[f32; 4], b: &[f32; 4]| a[2].min(b[2]) > a[0].max(b[0]);
    // Half the height of the last line read: a block starting less than that above its bottom is not below it.
    let tol = members.last().and_then(|&s| segs.get(s)).map_or(0.0, |s| s.h * 0.5);
    let inked = |r: usize| inked.get(r).copied().unwrap_or(true);
    let below = remaining
        .iter()
        .copied()
        .filter(|&r| r != pick && inked(r) && blocks.get(r).is_some_and(|(_, rb)| rb[1] >= lb[3] - tol && across(rb, lb)))
        .min_by(|a, b| blocks[*a].1[1].total_cmp(&blocks[*b].1[1]));
    let Some((below, next)) = below.and_then(|b| Some((b, first_line(b)?))) else { return false };
    remaining.iter().any(|&r| {
        r != pick && r != below && inked(r) && blocks.get(r).is_some_and(|(_, rb)| across(rb, pb)) && first_line(r).is_some_and(|f| level(&f, &next))
    })
}

/// The block to read before block `pick`, or `pick` itself: a block of the column right of the gutter between two
/// columns of running text (`bands`, as [`column_bands`] gives them) waits for the unread blocks left of that
/// gutter, top-most first. Each formula set between a column's paragraphs otherwise ends its block there, and the
/// page reads in bands, a stretch of the left column and then of the right one. `inks` are each block's visible
/// text from its left edge to its right, and whether it has any.
fn before_next_column(blocks: &[Block], (inks, inked): (&[(f32, f32)], &[bool]), remaining: &[usize], bands: &[[f32; 3]], pick: usize) -> usize {
    let level = |b: usize, y0: f32, y1: f32| blocks.get(b).is_some_and(|(_, bb)| (y0..=y1).contains(&((bb[1] + bb[3]) / 2.0)));
    let has_ink = |b: usize| inked.get(b).copied().unwrap_or(false);
    let top_left = |b: usize| blocks.get(b).map_or((f32::INFINITY, f32::INFINITY), |(_, bb)| (bb[1], bb[0]));
    bands
        .iter()
        .filter(|&&[mid, y0, y1]| inks.get(pick).is_some_and(|ink| ink.0 >= mid) && level(pick, y0, y1))
        .find_map(|&[mid, y0, y1]| {
            // A block stacked across the gutter beside the columns (a heading over both and the lines set with it,
            // or text the blocks could not tell apart by column) leaves no column to finish: the order stays.
            let stacked_across = blocks
                .iter()
                .enumerate()
                .any(|(r, (_, rb))| has_ink(r) && inks.get(r).is_some_and(|ink| ink.0 < mid && ink.1 > mid) && rb[1] < y1 && rb[3] > y0);
            if stacked_across {
                return None;
            }
            let left = |r: &usize| *r != pick && has_ink(*r) && inks.get(*r).is_some_and(|ink| ink.1 <= mid) && level(*r, y0, y1);
            remaining.iter().copied().filter(left).min_by(|a, b| {
                let ((ay, ax), (by, bx)) = (top_left(*a), top_left(*b));
                ay.total_cmp(&by).then(ax.total_cmp(&bx))
            })
        })
        .unwrap_or(pick)
}

/// Where each gutter of `found` (its edges and its stretch of the page, as [`Gutters::found`] gives them) parts
/// two columns of running text: its middle, and the top and bottom of the stretch where the columns stand side
/// by side. From the leftmost gutter to the rightmost.
///
/// The columns' lines are those at least [`COLUMN_EMS`] heights wide that lie wholly on one side of the gutter
/// within its stretch, [`COLUMN_LINES`] or more a side, as long as the next line of either column follows within
/// [`COLUMN_ROWS`] heights, but for a line that stands apart at either end ([`without_stray_ends`]). They stand
/// side by side from where the second of the two columns starts, down to the last line of either. A header above
/// the columns and a footer below them are outside that stretch; so are the text beside a figure at the top of
/// one column, a panel above the columns, and a table of narrow cells below them.
fn column_bands(segs: &[Seg], found: &[[f32; 4]]) -> Vec<[f32; 3]> {
    let mut bands: Vec<[f32; 3]> = Vec::new();
    for &[lo, hi, top, bottom] in found {
        let mid = (lo + hi) / 2.0;
        // The columns' lines as (top, bottom, height, whether left of the gutter), top to bottom.
        let mut lines: Vec<(f32, f32, f32, bool)> = segs
            .iter()
            .filter(|s| !s.barcode && s.ink.is_finite() && s.ink - s.ink_start >= s.h * COLUMN_EMS && top < s.cy() && s.cy() < bottom)
            // Beside the gutter: it lies closer to the line's end (or start) than half the line is wide. Text
            // further off, under or over another column of a page set in panels, is not this column's.
            .filter(|s| {
                let half = (s.ink - s.ink_start) / 2.0;
                (s.ink <= mid && lo - s.ink <= half) || (s.ink_start >= mid && s.ink_start - hi <= half)
            })
            .map(|s| (s.bbox[1], s.bbox[3], s.h, s.ink <= mid))
            .collect();
        lines.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Each run of lines with no gap of more than `COLUMN_ROWS` heights in it.
        let mut start = 0;
        while start < lines.len() {
            let mut end = start + 1;
            let mut reach = lines[start].1;
            while let Some(&(y0, y1, _, _)) = lines.get(end) {
                let h = lines.get(end - 1).map_or(0.0, |l| l.2);
                if y0 - reach > h * COLUMN_ROWS {
                    break;
                }
                reach = reach.max(y1);
                end += 1;
            }
            let run = without_stray_ends(lines.get(start..end).unwrap_or_default());
            let first = |left: bool| run.iter().find(|l| l.3 == left).map(|l| l.0);
            let count = |left: bool| run.iter().filter(|l| l.3 == left).count();
            if let (Some(a), Some(b)) = (first(true), first(false))
                && count(true) >= COLUMN_LINES
                && count(false) >= COLUMN_LINES
            {
                bands.push([mid, a.max(b), run.iter().fold(f32::NEG_INFINITY, |y, l| y.max(l.1))]);
            }
            start = end;
        }
    }
    bands.sort_by(|a, b| a[0].total_cmp(&b[0]));
    bands
}

/// A line that stands this many of its heights clear of the columns' other lines, above them or below, is not
/// one of theirs: a running head over one column, a footer under one.
const STRAY_LINE_GAP: f32 = 2.0;

/// `lines` (each a line's top, bottom, height and whether it is left of the gutter, top to bottom) without the
/// lines at its start and end that stand apart: a first line more than [`STRAY_LINE_GAP`] heights above the next
/// line of its own column, with no line of the other column that high, and a last line as far below. A running
/// head at the top of the right column would otherwise be read between the two columns, and so would a footer at
/// the bottom of the left one.
fn without_stray_ends(mut lines: &[(f32, f32, f32, bool)]) -> &[(f32, f32, f32, bool)] {
    while let Some((&(_, y1, h, left), rest)) = lines.split_first() {
        let clear = y1 + h * STRAY_LINE_GAP;
        let own = rest.iter().filter(|l| l.3 == left).map(|l| l.0).fold(f32::INFINITY, f32::min);
        let other = rest.iter().filter(|l| l.3 != left).map(|l| l.0).fold(f32::INFINITY, f32::min);
        if own > clear && other > clear && !rest.is_empty() {
            lines = rest;
        } else {
            break;
        }
    }
    while let Some((&(y0, _, h, left), rest)) = lines.split_last() {
        let clear = y0 - h * STRAY_LINE_GAP;
        let own = rest.iter().filter(|l| l.3 == left).map(|l| l.1).fold(f32::NEG_INFINITY, f32::max);
        let other = rest.iter().filter(|l| l.3 != left).map(|l| l.1).fold(f32::NEG_INFINITY, f32::max);
        if own < clear && other < clear && !rest.is_empty() {
            lines = rest;
        } else {
            break;
        }
    }
    lines
}

/// The gutter between a column, from `x0` to `x1`, and the column beside it, where that one has at least
/// [`COLUMN_LINES`] lines of text level with the stretch from `top` to `bottom`: the gutter at the height of
/// `last`, the line the stretch starts from, whose text is `h` high ([`Gutters::between`]). The lines level with
/// the stretch are at least half as wide as running text: a paragraph's last line and a heading count, a figure's
/// labels don't, and that column's text reaches down to within [`COLUMN_ROWS`] heights of the stretch's bottom, or
/// further. Beside a figure as wide as the page, or above a footer, the column beside has nothing level with the
/// stretch, or ends near its top.
fn gutter_beside(
    segs: &[Seg],
    (x0, x1): (f32, f32),
    (top, bottom): (f32, f32),
    (last, h): (&[f32; 4], f32),
    gutters: &mut Gutters,
) -> Option<(f32, f32)> {
    if gutters.spent() {
        return None;
    }
    let pieces = || segs.iter().filter_map(Seg::piece);
    // The text level with the stretch right of the column, then left of it: its near edge, its far edge, and its lines.
    let (mut right, mut after) = ((f32::INFINITY, f32::NEG_INFINITY), 0usize);
    let (mut left, mut before) = ((f32::NEG_INFINITY, f32::INFINITY), 0usize);
    // How far down each side's text reaches, below the stretch too: a column with a figure of its own beside the
    // stretch goes on below it.
    let (mut right_low, mut left_low) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for p in pieces().filter(|p| p.cy > top && p.ink - p.x0 >= h * COLUMN_EMS / 2.0) {
        let level = p.cy < bottom;
        if p.x0 >= x1 {
            right_low = right_low.max(p.cy);
            if level {
                (right, after) = ((right.0.min(p.x0), right.1.max(p.ink)), after + 1);
            }
        } else if p.ink <= x0 {
            left_low = left_low.max(p.cy);
            if level {
                (left, before) = ((left.0.max(p.ink), left.1.min(p.x0)), before + 1);
            }
        }
    }
    let reaches_down = |low: f32| low >= bottom - h * COLUMN_ROWS;
    let (after, before) = (if reaches_down(right_low) { after } else { 0 }, if reaches_down(left_low) { before } else { 0 });
    gutters.spend(segs.len());
    let cy = (last[1] + last[3]) / 2.0;
    if after >= COLUMN_LINES
        && let Some(gutter) = gutters.between(pieces(), Gap { start: x0, from: x1, to: right.0, end: right.1 }, cy, h)
    {
        return Some(gutter);
    }
    if before < COLUMN_LINES {
        return None;
    }
    // The text on the left is itself what the gap follows.
    gutters.between(pieces(), Gap { start: left.1, from: left.0, to: x0, end: x1 }, cy, h)
}
