//! Ruled lines erased from the image the models read, once the detector has found the words.
//!
//! When a table rule touches a word, the detector boxes the two together and the recogniser
//! reads the rule's stroke as the nearest letter shape: `|DATE`, `IDATE`, `DDATE` (#8502).
//! Nothing in the models knows that a stroke running far past a word's height is a rule and
//! not a glyph, so the models are not shown one: measured against the median height of the
//! words the detector found, a run of ink along a row or down a column at least [`MIN_RUN`]
//! word heights long, no thicker than [`MAX_THICK`] of one, [`ASPECT`] times longer than
//! thick, and of that thickness along [`STEADY`] of its length, is a rule, and its pixels take
//! the paper grey beside it; the words are then read again from the cleaned image, in the
//! boxes already found. Where
//! another stroke crosses the rule (a letter's stem over a cell's rule, a descender through an
//! underline) the rule is left under the stroke, so the letter keeps all its ink and carries
//! only a stub of the rule as wide as the stroke; where two rules cross, the junction goes
//! with both. Filled areas (a logo, a black header bar, a dark scan) are thicker than a rule
//! and stay; dashes, underscores and the strokes of lettering are shorter and stay. The word
//! height is the scale, not the page's resolution, which a page made from an image does not
//! know. Only the models' copy changes: the page, the word boxes and the editing cover see the
//! rules as they are.

use std::ops::Range;

/// The shortest run of ink that is a rule, in word heights: the rule of a small title-block
/// cell is longer, a stroke of lettering or an em dash is shorter.
const MIN_RUN: f32 = 3.0;
/// The thickest stroke that is a rule, as a share of the word height (a drawing's 3 pt border
/// beside 8 pt lettering).
const MAX_THICK: f32 = 0.6;
/// A rule is at least this many times longer than it is thick.
const ASPECT: usize = 8;
/// The share of a run's length (two thirds) that must show the rule's thickness, to within
/// [`RAGGED`] pixels (a scan's edges are ragged), for the run to be a rule and not a row of
/// lettering whose letters touch.
const STEADY: (usize, usize) = (2, 3);
const RAGGED: usize = 2;
/// Ink is darker than this percentage of the paper's grey (the page's median).
const INK_PERCENT: u32 = 60;
/// How far beyond a rule's edge the paper is sampled, in pixels: past a scan's halo.
const PAPER_GAP: usize = 3;
/// How many columns of a run are measured first, to refuse a filled area cheaply.
const PROBES: usize = 16;

/// Per-pixel flags: ink, and for each pass (0 along rows, 1 down columns) the band a rule
/// covers and the part of it that is the rule's own (no stroke crosses there).
const INK: u8 = 1;
const BAND: [u8; 2] = [2, 8];
const OWN: [u8; 2] = [4, 16];

/// Erase the ruled lines of `pixels` (`width` × `height`, `channels` 1 or 3 bytes a pixel,
/// its words `word_height` pixels tall), or `None` when it has none or the arguments do not
/// describe an image: the common page is read as it is, without a copy.
pub(crate) fn erase_rules(pixels: &[u8], width: u32, height: u32, channels: usize, word_height: f32) -> Option<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    let area = w.checked_mul(h).filter(|&a| a > 0)?;
    if !matches!(channels, 1 | 3) || pixels.len() != area * channels || !word_height.is_finite() || word_height < 1.0 {
        return None;
    }
    let limits = Limits {
        min_run: ((MIN_RUN * word_height).round() as usize).max(2 * ASPECT),
        max_thick: ((MAX_THICK * word_height).round() as usize).max(1),
    };
    let gray: Vec<u8> = if channels == 1 {
        pixels.to_vec()
    } else {
        pixels
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| ((299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2])) / 1000).min(255) as u8)
            .collect()
    };
    let mut sheet = Sheet::new(gray);
    for (pass, rows) in [true, false].into_iter().enumerate() {
        sheet.mark_rules(Lanes { w, h, rows }, pass, &limits);
    }
    sheet.paint(pixels, channels)
}

/// The run length and thickness that make a rule, in pixels.
struct Limits {
    min_run: usize,
    max_thick: usize,
}

/// The grid read lane by lane: along rows (`rows`: a lane is a row and `pos` runs along x)
/// or down columns.
#[derive(Clone, Copy)]
struct Lanes {
    w: usize,
    h: usize,
    rows: bool,
}

impl Lanes {
    /// A lane's length.
    fn len(&self) -> usize {
        if self.rows { self.w } else { self.h }
    }

    /// How many lanes there are.
    fn count(&self) -> usize {
        if self.rows { self.h } else { self.w }
    }

    /// The pixel index at `pos` along `lane`; `None` off the grid.
    fn at(&self, lane: isize, pos: usize) -> Option<usize> {
        let lane = usize::try_from(lane).ok().filter(|&l| l < self.count())?;
        if pos >= self.len() {
            return None;
        }
        Some(if self.rows { lane * self.w + pos } else { pos * self.w + lane })
    }
}

/// The page's grey, flags and the paper grey painted where a rule is erased.
struct Sheet {
    gray: Vec<u8>,
    flags: Vec<u8>,
    paper: Vec<u8>,
}

impl Sheet {
    fn new(gray: Vec<u8>) -> Sheet {
        let threshold = ink_threshold(&gray);
        let flags = gray.iter().map(|&g| if u32::from(g) < threshold { INK } else { 0 }).collect();
        let paper = vec![0; gray.len()];
        Sheet { gray, flags, paper }
    }

    fn has(&self, i: Option<usize>, bits: u8) -> bool {
        i.and_then(|i| self.flags.get(i)).is_some_and(|f| f & bits != 0)
    }

    /// Find the runs of ink along every lane and mark those that are rules.
    fn mark_rules(&mut self, lanes: Lanes, pass: usize, limits: &Limits) {
        for lane in 0..lanes.count() {
            let mut runs = Vec::new();
            let mut pos = 0;
            while pos < lanes.len() {
                if !self.has(lanes.at(lane as isize, pos), INK) {
                    pos += 1;
                    continue;
                }
                let start = pos;
                while self.has(lanes.at(lane as isize, pos), INK) {
                    pos += 1;
                }
                if pos - start >= limits.min_run {
                    runs.push(start..pos);
                }
            }
            for run in runs {
                self.mark_run(lanes, pass, limits, lane, run);
            }
        }
    }

    /// Judge one long run of ink and, when it is a rule, mark its band and its own pixels.
    fn mark_run(&mut self, lanes: Lanes, pass: usize, limits: &Limits, lane: usize, run: Range<usize>) {
        // A thick rule is a run on each of its lanes; the first marks it whole.
        if run.clone().all(|p| self.has(lanes.at(lane as isize, p), OWN[pass])) {
            return;
        }
        let cap = limits.max_thick + RAGGED + 1;
        let thickness = |(before, after): &(usize, usize)| before + after + 1;
        // A filled area (a photo, a box) is a long run on each of its many lanes: a few
        // columns show its thickness, and refuse it before every column is measured.
        let probe = (0..PROBES).map(|i| run.start + i * run.len() / PROBES).map(|p| thickness(&self.extent(lanes, lane, p, cap)));
        if median(probe).is_none_or(|t| t > limits.max_thick) {
            return;
        }
        let extents: Vec<(usize, usize)> = run.clone().map(|p| self.extent(lanes, lane, p, cap)).collect();
        let Some(t) = median(extents.iter().map(thickness)) else { return };
        if t > limits.max_thick || run.len() < ASPECT * t {
            return;
        }
        let steady = |e: &(usize, usize)| thickness(e).abs_diff(t) <= RAGGED;
        if extents.iter().filter(|e| steady(e)).count() * STEADY.1 < run.len() * STEADY.0 {
            return;
        }
        let before = median(extents.iter().filter(|e| steady(e)).map(|e| e.0)).unwrap_or(0);
        let after = median(extents.iter().filter(|e| steady(e)).map(|e| e.1)).unwrap_or(0);
        let paper = self.paper_beside(lanes, lane, &run, before, after);
        // A column is the rule's own when its ink is no more than a pixel thicker than the
        // rule (a scan's edges are ragged); a stroke reaching further crosses it. So does a
        // column a pixel out that runs on from a crossing, up to a letter's height: the stem
        // of a letter as narrow as the rule, over it but for a pixel, stands on its foot and
        // is kept with it, where a ragged pixel on its own goes with the rule. The band
        // erased at a crossing is the one of the nearest own column (a scanner's tilt shifts
        // the band along the run).
        let ragged = |e: &(usize, usize)| thickness(e) <= t + 1;
        let mut own: Vec<bool> = extents.iter().map(ragged).collect();
        let letter = limits.min_run / 2;
        for i in (0..extents.len()).filter(|&i| !ragged(&extents[i])) {
            for step in [-1isize, 1] {
                let mut j = i;
                for _ in 0..letter {
                    j = match j.checked_add_signed(step) {
                        Some(j) if j < extents.len() && thickness(&extents[j]) > t => j,
                        _ => break,
                    };
                    own[j] = false;
                }
            }
        }
        let bands = nearest_own(&extents, &own, (before, after));
        let lane = lane as isize;
        for ((pos, own), (b, a)) in run.zip(own).zip(bands) {
            let bits = if own { OWN[pass] | BAND[pass] } else { BAND[pass] };
            self.mark(lanes, lane - b as isize - 1..=lane + a as isize + 1, pos, bits, paper);
        }
    }

    /// Set `bits` and the `paper` grey on the pixels at `pos` of `lanes_from_to`.
    fn mark(&mut self, lanes: Lanes, lanes_from_to: std::ops::RangeInclusive<isize>, pos: usize, bits: u8, paper: u8) {
        for i in lanes_from_to.filter_map(|l| lanes.at(l, pos)) {
            if let Some(f) = self.flags.get_mut(i) {
                *f |= bits;
            }
            if let Some(p) = self.paper.get_mut(i) {
                *p = paper;
            }
        }
    }

    /// How far the ink at `pos` on `lane` continues across the lanes before and after it,
    /// each counted up to `cap`.
    fn extent(&self, lanes: Lanes, lane: usize, pos: usize, cap: usize) -> (usize, usize) {
        let lane = lane as isize;
        let reach = |step: isize| (1..=cap).take_while(|&k| self.has(lanes.at(lane + step * k as isize, pos), INK)).count();
        (reach(-1), reach(1))
    }

    /// The paper's grey beside a rule: the median of the pixels that are not ink on the two
    /// lanes [`PAPER_GAP`] beyond each edge of its band, over the run; white when there are none.
    fn paper_beside(&self, lanes: Lanes, lane: usize, run: &Range<usize>, before: usize, after: usize) -> u8 {
        let lane = lane as isize;
        let sides = [(lane - (before + PAPER_GAP) as isize, -1isize), (lane + (after + PAPER_GAP) as isize, 1)];
        let samples = sides.into_iter().flat_map(|(edge, step)| {
            [edge, edge + step].into_iter().flat_map(move |l| {
                run.clone().filter_map(move |pos| lanes.at(l, pos)).filter(|&i| !self.has(Some(i), INK)).filter_map(|i| self.gray.get(i).copied())
            })
        });
        median(samples.map(usize::from)).map_or(255, |g| g as u8)
    }

    /// `pixels` with every rule painted paper, or `None` when nothing was marked.
    fn paint(&self, pixels: &[u8], channels: usize) -> Option<Vec<u8>> {
        let mut out: Option<Vec<u8>> = None;
        for (i, &f) in self.flags.iter().enumerate() {
            let erase = f & (OWN[0] | OWN[1]) != 0 || (f & BAND[0] != 0 && f & BAND[1] != 0);
            if !erase {
                continue;
            }
            let g = self.paper.get(i).copied().unwrap_or(255);
            if let Some(px) = out.get_or_insert_with(|| pixels.to_vec()).get_mut(i * channels..(i + 1) * channels) {
                px.fill(g);
            }
        }
        out
    }
}

/// For each column of a run, the band of the nearest column that is the rule's own (its own
/// band for an own column), or `fallback` where none is.
fn nearest_own(extents: &[(usize, usize)], own: &[bool], fallback: (usize, usize)) -> Vec<(usize, usize)> {
    let n = extents.len();
    let own = |i: usize| own.get(i).copied().unwrap_or(false);
    // The nearest own column before (or at) each column, and after it, with its distance.
    let mut forward: Vec<Option<(usize, (usize, usize))>> = vec![None; n];
    let mut last = None;
    for (i, e) in extents.iter().enumerate() {
        if own(i) {
            last = Some((i, *e));
        }
        if let (Some((at, band)), Some(slot)) = (last, forward.get_mut(i)) {
            *slot = Some((i - at, band));
        }
    }
    let mut next = None;
    let mut bands = vec![fallback; n];
    for (i, e) in extents.iter().enumerate().rev() {
        if own(i) {
            next = Some((i, *e));
        }
        let backward = next.map(|(at, band)| (at - i, band));
        let nearest = match (forward.get(i).copied().flatten(), backward) {
            (Some(f), Some(b)) => Some(if f.0 <= b.0 { f } else { b }),
            (f, b) => f.or(b),
        };
        if let (Some((_, band)), Some(slot)) = (nearest, bands.get_mut(i)) {
            *slot = band;
        }
    }
    bands
}

/// The grey under which a pixel is ink: [`INK_PERCENT`] of the page's median grey, which is
/// the paper on any page that is mostly paper.
fn ink_threshold(gray: &[u8]) -> u32 {
    let mut histogram = [0usize; 256];
    for &g in gray {
        if let Some(n) = histogram.get_mut(usize::from(g)) {
            *n += 1;
        }
    }
    let half = gray.len() / 2;
    let mut seen = 0;
    let paper = histogram.iter().position(|&n| {
        seen += n;
        seen > half
    });
    paper.map_or(0, |p| p as u32) * INK_PERCENT / 100
}

fn median(values: impl Iterator<Item = usize>) -> Option<usize> {
    let mut v: Vec<usize> = values.collect();
    v.sort_unstable();
    v.get(v.len() / 2).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The word height the fixtures are drawn for: a rule is 90 px or more, 18 px or less.
    const WORD: f32 = 30.0;

    /// A grey page, one byte a pixel.
    fn page(w: u32, h: u32, grey: u8) -> (Vec<u8>, u32, u32) {
        (vec![grey; (w * h) as usize], w, h)
    }

    fn fill(img: &mut (Vec<u8>, u32, u32), x0: u32, y0: u32, w: u32, h: u32, grey: u8) {
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                img.0[(y * img.1 + x) as usize] = grey;
            }
        }
    }

    fn erased(img: &(Vec<u8>, u32, u32), word: f32) -> Option<(Vec<u8>, u32, u32)> {
        erase_rules(&img.0, img.1, img.2, 1, word).map(|px| (px, img.1, img.2))
    }

    /// How many pixels of `[x0, y0, x1, y1)` are ink (grey under 128).
    fn ink_in(img: &(Vec<u8>, u32, u32), x0: u32, y0: u32, x1: u32, y1: u32) -> usize {
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| img.0[(y * img.1 + x) as usize] < 128).count()
    }

    /// Rules across and down a page (5 px beside 30 px words) are erased, their junction
    /// too; a block letter, a filled box and a dash keep every pixel.
    #[test]
    fn rules_are_erased_and_lettering_boxes_and_dashes_are_kept() {
        let mut img = page(600, 400, 255);
        fill(&mut img, 20, 200, 560, 5, 0); // across
        fill(&mut img, 300, 20, 5, 360, 0); // down, crossing it at (300, 200)
        fill(&mut img, 40, 40, 12, 40, 0); // an L
        fill(&mut img, 40, 68, 36, 12, 0);
        fill(&mut img, 400, 40, 120, 60, 0); // a filled box
        fill(&mut img, 60, 300, 60, 5, 0); // a dash, two word heights
        let out = erased(&img, WORD).expect("the rules are found");
        assert_eq!(ink_in(&out, 20, 198, 580, 207), 0, "the rule across is gone, its junction with it");
        assert_eq!(ink_in(&out, 298, 20, 307, 380), 0, "the rule down is gone");
        assert_eq!(ink_in(&out, 40, 40, 76, 80), ink_in(&img, 40, 40, 76, 80), "the letter keeps its ink");
        assert_eq!(ink_in(&out, 400, 40, 520, 100), 120 * 60, "the box keeps its ink");
        assert_eq!(ink_in(&out, 60, 300, 120, 305), 60 * 5, "the dash keeps its ink");
        // Three channels paint the same.
        let rgb: Vec<u8> = img.0.iter().flat_map(|&g| [g, g, g]).collect();
        let out3 = erase_rules(&rgb, 600, 400, 3, WORD).unwrap();
        assert!(out3.iter().zip(out.0.iter().flat_map(|&g| [g, g, g])).all(|(a, b)| *a == b));
    }

    /// A letter whose stem overlaps a cell's rule keeps all of its ink; the rule above and
    /// below it goes, and only a stub of it as wide as the stem stays beside the letter.
    #[test]
    fn a_stroke_crossing_a_rule_keeps_its_ink() {
        let mut img = page(400, 400, 255);
        fill(&mut img, 100, 20, 5, 360, 0); // the rule down, x 100..105
        fill(&mut img, 102, 150, 12, 40, 0); // the stem, 3 px over the rule
        fill(&mut img, 102, 178, 36, 12, 0); // the foot
        let out = erased(&img, WORD).unwrap();
        assert_eq!(ink_in(&out, 98, 20, 107, 148), 0, "the rule above the letter is gone");
        assert_eq!(ink_in(&out, 98, 192, 107, 380), 0, "and below it");
        assert_eq!(ink_in(&out, 105, 150, 138, 190), ink_in(&img, 105, 150, 138, 190), "the letter's ink beyond the rule stays");
        assert!(ink_in(&out, 100, 150, 105, 190) > 0, "the rule stays under the stem rather than cut the letter");
    }

    /// A stem as narrow as the rule, over it but for one pixel, keeps that pixel: the ink there
    /// is thicker than the rule, so the column is a crossing and the rule stays under the stem,
    /// while the rule beyond the letter goes.
    #[test]
    fn a_narrow_stem_a_pixel_past_a_rule_keeps_its_ink() {
        let mut img = page(400, 400, 255);
        fill(&mut img, 100, 20, 5, 360, 0); // the rule down, x 100..105
        fill(&mut img, 101, 150, 5, 40, 0); // the stem, x 101..106: one pixel past the rule
        fill(&mut img, 101, 178, 36, 12, 0); // the foot
        let out = erased(&img, WORD).unwrap();
        assert_eq!(ink_in(&out, 98, 20, 107, 148), 0, "the rule above the letter is gone");
        assert_eq!(ink_in(&out, 98, 192, 107, 380), 0, "and below it");
        assert_eq!(ink_in(&out, 105, 150, 106, 190), 40, "the stem keeps its pixel past the rule");
        assert_eq!(ink_in(&out, 106, 178, 137, 190), ink_in(&img, 106, 178, 137, 190), "and the foot its ink");
        // The same pixels with no foot are a ragged edge of the rule (a scan's edges are), and
        // go with it; so does a lone pixel out.
        let mut ragged = page(400, 400, 255);
        fill(&mut ragged, 100, 20, 5, 360, 0);
        fill(&mut ragged, 105, 150, 1, 40, 0);
        fill(&mut ragged, 99, 250, 1, 1, 0);
        let out = erased(&ragged, WORD).unwrap();
        assert_eq!(ink_in(&out, 98, 20, 107, 380), 0, "a ragged edge is the rule's");
    }

    /// A rule a scanner tilted by a degree is still a rule; a steep diagonal is not.
    #[test]
    fn a_slightly_tilted_rule_is_a_rule_and_a_diagonal_is_not() {
        let mut img = page(400, 200, 255);
        let slope = 1.5f64.to_radians().tan();
        for x in 20..380u32 {
            let y = (60.0 + (x as f64 - 200.0) * slope).round() as u32;
            fill(&mut img, x, y, 1, 5, 0);
        }
        for x in 20..180u32 {
            fill(&mut img, x, 100 + (x - 20) / 2, 1, 5, 0); // 27°
        }
        let out = erased(&img, WORD).unwrap();
        assert!(ink_in(&out, 20, 50, 380, 72) < 20, "the tilted rule is gone: {}", ink_in(&out, 20, 50, 380, 72));
        assert_eq!(ink_in(&out, 20, 100, 180, 190), ink_in(&img, 20, 100, 180, 190), "the diagonal stays");
    }

    /// On a grey scan the erased rule takes the paper's grey beside it, not white, and a halo
    /// around the rule goes with it.
    #[test]
    fn an_erased_rule_takes_the_paper_grey_beside_it() {
        let mut img = page(400, 200, 200);
        fill(&mut img, 20, 99, 360, 7, 160); // the halo
        fill(&mut img, 20, 100, 360, 5, 40);
        let out = erased(&img, WORD).unwrap();
        for x in [20u32, 200, 379] {
            for y in 98..=107 {
                let g = out.0[(y * 400 + x) as usize];
                assert!(g.abs_diff(200) <= 2, "({x}, {y}) is {g}, not the paper's 200");
            }
        }
    }

    /// Filled areas and a dark page are no rules: nothing is touched and no copy is made; nor
    /// on a page with no ink at all. The word height is the scale: a stroke three word heights
    /// long is a rule beside small words and lettering beside large ones.
    #[test]
    fn filled_areas_dark_pages_and_blank_pages_are_left_alone() {
        assert!(erased(&page(300, 300, 0), WORD).is_none(), "a dark page");
        assert!(erased(&page(300, 300, 255), WORD).is_none(), "a blank page");
        let mut boxed = page(400, 300, 255);
        fill(&mut boxed, 20, 20, 360, 100, 0);
        assert!(erased(&boxed, WORD).is_none(), "a filled box");
        let mut stroke = page(100, 60, 255);
        fill(&mut stroke, 10, 30, 80, 2, 0);
        assert_eq!(ink_in(&erased(&stroke, 10.0).unwrap(), 10, 28, 90, 34), 0, "a rule beside 10 px words");
        assert!(erased(&stroke, 40.0).is_none(), "a stroke of 40 px lettering");
        assert!(erase_rules(&stroke.0, 100, 60, 3, WORD).is_none(), "a buffer that is not the image described");
        assert!(erase_rules(&stroke.0, 100, 60, 1, f32::NAN).is_none());
    }

    /// Lettering whose letters run together along a row is not a rule: its thickness is not
    /// steady along the run.
    #[test]
    fn merged_lettering_is_not_a_rule() {
        let mut img = page(400, 100, 255);
        fill(&mut img, 20, 60, 200, 3, 0); // a baseline the letters share
        for x in (20..220).step_by(10) {
            fill(&mut img, x, 40 + (x % 30) / 3, 4, 20, 0); // stems of varying height
        }
        assert!(erased(&img, WORD).is_none());
    }
}
