//! Columns and reading order of recognised lines.
//!
//! The line finder joins words that sit at one height into one line, also across the gutter
//! between two columns, and returns lines top to bottom, so a newspaper's lines run from one
//! column into the next and the text (copied, searched or exported) comes out interleaved. [`reading_order`] splits each line at gaps far wider than a word space,
//! then orders the pieces by recursive XY cuts: the page is cut at its widest empty band,
//! across or down, and each part in turn, so a heading over the columns comes first, then each
//! column from top to bottom.
//!
//! With the layout model installed, [`paragraphs`] then puts the lines into the page's regions
//! (paragraphs, headings, captions, table cells) and reads the regions in the model's order, each
//! region one paragraph.

use crate::regions::Region;
use crate::{Line, Word};

/// A gap between two words wider than this many line heights separates columns. A word space is
/// about a third of a line height; on MDPBench 0.8 read best among 0.8, 1.2 and 2.0.
const COLUMN_GAP: f32 = 0.8;
/// Cuts narrower than this many median line heights are not made: a down cut must be at least a
/// narrow gutter, a cut across at least a little more than line spacing.
const MIN_CUT_DOWN: f32 = 0.6;
const MIN_CUT_ACROSS: f32 = 0.3;
/// Pages with more pieces than this keep the line finder's order.
const MAX_PIECES: usize = 20_000;
/// A line belongs to the region that covers at least this much of its box (the one covering the
/// most, if several do).
const IN_REGION: f32 = 0.5;

fn height(r: [f32; 4]) -> f32 {
    (r[3] - r[1]).max(1.0)
}

fn bounds(words: &[Word]) -> Option<[f32; 4]> {
    words.iter().map(|w| w.rect).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
}

/// `line` split wherever the gap between two neighbouring words is wider than [`COLUMN_GAP`]
/// times the line's height.
fn split_columns(line: Line) -> Vec<Line> {
    let Some(b) = bounds(&line.words) else { return Vec::new() };
    let limit = COLUMN_GAP * height(b);
    let mut out: Vec<Line> = Vec::new();
    let mut current: Vec<Word> = Vec::new();
    for w in line.words {
        if let Some(prev) = current.last()
            && w.rect[0] - prev.rect[2] > limit
        {
            out.push(Line { words: std::mem::take(&mut current) });
        }
        current.push(w);
    }
    if !current.is_empty() {
        out.push(Line { words: current });
    }
    out
}

/// The widest empty band across `spans` (start, end along one axis): its middle and width.
fn widest_gap(spans: &mut [(f32, f32)]) -> Option<(f32, f32)> {
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut reach = spans.first()?.1;
    let mut best: Option<(f32, f32)> = None;
    for &(start, end) in spans.iter().skip(1) {
        let gap = start - reach;
        if gap > 0.0 && best.is_none_or(|(_, w)| gap > w) {
            best = Some((reach + gap / 2.0, gap));
        }
        reach = reach.max(end);
    }
    best
}

/// The indices of `boxes` in reading order (recursive XY cuts; an explicit stack, so no
/// recursion depth to bound).
fn xy_cut(boxes: &[[f32; 4]], unit: f32) -> Vec<usize> {
    let mut order = Vec::with_capacity(boxes.len());
    // Groups still to order, the next one on top.
    let mut stack: Vec<Vec<usize>> = vec![(0..boxes.len()).collect()];
    while let Some(group) = stack.pop() {
        if group.len() <= 1 {
            order.extend(group);
            continue;
        }
        let at = |i: &usize| boxes.get(*i).copied().unwrap_or([0.0; 4]);
        let mut xs: Vec<(f32, f32)> = group.iter().map(|i| (at(i)[0], at(i)[2])).collect();
        let mut ys: Vec<(f32, f32)> = group.iter().map(|i| (at(i)[1], at(i)[3])).collect();
        let down = widest_gap(&mut xs).filter(|g| g.1 >= MIN_CUT_DOWN * unit);
        let across = widest_gap(&mut ys).filter(|g| g.1 >= MIN_CUT_ACROSS * unit);
        // The wider cut first; widths compared in line heights, so a gutter (narrow, but
        // across the whole group) still wins over the space between two paragraphs.
        let cut = match (down, across) {
            (Some(d), Some(a)) if d.1 / MIN_CUT_DOWN >= a.1 / MIN_CUT_ACROSS => Some((true, d.0)),
            (Some(_), Some(a)) => Some((false, a.0)),
            (Some(d), None) => Some((true, d.0)),
            (None, Some(a)) => Some((false, a.0)),
            (None, None) => None,
        };
        // Every cut must leave boxes on both sides, or the same group would come back forever
        // (a box whose right edge lies left of its left edge can sit on the wrong side of it).
        let parts = cut.map(|(vertical, mid)| -> (Vec<usize>, Vec<usize>) {
            group.iter().partition(|i| if vertical { at(i)[2] <= mid } else { at(i)[3] <= mid })
        });
        match parts {
            Some((first, second)) if !first.is_empty() && !second.is_empty() => {
                // Pushed in reverse, so `first` is taken next.
                stack.push(second);
                stack.push(first);
            }
            _ => {
                // No empty band left: rows top to bottom, each left to right. Sorting with a
                // "same row within half a height" comparison is not a total order (sort may panic
                // on it), so sort by height on the page, then gather neighbours into rows.
                let mid = |i: &usize| (at(i)[1] + at(i)[3]) / 2.0;
                let mut rest = group;
                rest.sort_by(|a, b| mid(a).total_cmp(&mid(b)));
                let mut row: Vec<usize> = Vec::new();
                for i in rest {
                    if row.first().is_some_and(|f| (mid(&i) - mid(f)).abs() > unit / 2.0) {
                        row.sort_by(|a, b| at(a)[0].total_cmp(&at(b)[0]));
                        order.append(&mut row);
                    }
                    row.push(i);
                }
                row.sort_by(|a, b| at(a)[0].total_cmp(&at(b)[0]));
                order.extend(row);
            }
        }
    }
    order
}

/// `lines` split at column gutters and put in reading order.
pub(crate) fn reading_order(lines: Vec<Line>) -> Vec<Line> {
    let pieces: Vec<Line> = lines.into_iter().flat_map(split_columns).collect();
    if pieces.len() > MAX_PIECES {
        return pieces;
    }
    let boxes: Vec<[f32; 4]> = pieces.iter().filter_map(|l| bounds(&l.words)).collect();
    if boxes.len() != pieces.len() {
        return pieces;
    }
    let mut heights: Vec<f32> = boxes.iter().map(|b| height(*b)).collect();
    heights.sort_by(f32::total_cmp);
    let unit = heights.get(heights.len() / 2).copied().unwrap_or(1.0).max(1.0);
    let order = xy_cut(&boxes, unit);
    let mut slots: Vec<Option<Line>> = pieces.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots.get_mut(i).and_then(Option::take)).collect()
}

/// The part of `a`'s area that `b` covers, from 0 to 1 (0 for an empty or broken `a`).
fn covered(a: [f32; 4], b: [f32; 4]) -> f32 {
    // `max(0.0)` also turns a NaN side into 0.
    let area = |r: [f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let whole = area(a);
    if !whole.is_finite() || whole <= 0.0 {
        return 0.0;
    }
    let inside = area([a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]);
    if inside.is_finite() { (inside / whole).min(1.0) } else { 0.0 }
}

/// `lines` (in reading order) as paragraphs: each line goes to the region covering at least half
/// of it, the regions are read in their rank order, and each region's lines, in the order they
/// came, make one paragraph. A line in no region (a label in a figure, a line the model missed)
/// is a paragraph of its own, right after the paragraph of the line before it, so it stays among
/// its neighbours. Without regions the page is one paragraph: nothing says where one ends.
pub(crate) fn paragraphs(lines: Vec<Line>, regions: &[Region]) -> Vec<Vec<Line>> {
    if lines.is_empty() {
        return Vec::new();
    }
    if regions.is_empty() || lines.len() > MAX_PIECES {
        return vec![lines];
    }
    let mut ranked: Vec<&Region> = regions.iter().collect();
    // Stable, so regions of equal rank keep the network's order.
    ranked.sort_by(|a, b| a.rank.total_cmp(&b.rank));
    let mut members: Vec<Vec<Line>> = vec![Vec::new(); ranked.len()];
    // Lines in no region, by the region of the last line before them that has one: `loose[0]`
    // before every region, `loose[i + 1]` after region `i`.
    let mut loose: Vec<Vec<Line>> = vec![Vec::new(); ranked.len() + 1];
    let mut slot = 0;
    for line in lines {
        let best = bounds(&line.words).and_then(|b| {
            ranked
                .iter()
                .enumerate()
                .map(|(i, r)| (i, covered(b, r.rect)))
                // The first of equally good regions wins: `max_by` would take the last.
                .fold(None, |best: Option<(usize, f32)>, (i, v)| if best.is_none_or(|(_, w)| v > w) { Some((i, v)) } else { best })
                .filter(|&(_, v)| v >= IN_REGION)
        });
        match best.and_then(|(i, _)| Some((i, members.get_mut(i)?))) {
            Some((i, region)) => {
                region.push(line);
                slot = i.saturating_add(1);
            }
            None => {
                if let Some(after) = loose.get_mut(slot) {
                    after.push(line);
                }
            }
        }
    }
    let mut out: Vec<Vec<Line>> = Vec::new();
    let mut loose = loose.into_iter();
    out.extend(loose.next().into_iter().flatten().map(|l| vec![l]));
    for (region, after) in members.into_iter().zip(loose) {
        if !region.is_empty() {
            out.push(region);
        }
        out.extend(after.into_iter().map(|l| vec![l]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, l: f32, t: f32, r: f32, b: f32) -> Word {
        Word { text: text.into(), rect: [l, t, r, b] }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    /// Two columns whose lines the line finder joined across the gutter come out column by
    /// column, under a heading that spans both.
    #[test]
    fn two_columns_read_one_after_the_other() {
        let mut lines = vec![Line { words: vec![word("Heading", 0.0, 0.0, 400.0, 40.0)] }];
        for row in 0..3 {
            let t = 80.0 + row as f32 * 30.0;
            lines.push(Line {
                words: vec![word(&format!("left{row}"), 0.0, t, 180.0, t + 20.0), word(&format!("right{row}"), 220.0, t, 400.0, t + 20.0)],
            });
        }
        let got = texts(&reading_order(lines));
        assert_eq!(got, ["Heading", "left0", "left1", "left2", "right0", "right1", "right2"]);
    }

    /// A word space is not a gutter, and one column stays in line order.
    #[test]
    fn a_single_column_keeps_its_lines() {
        let lines: Vec<Line> = (0..4)
            .map(|row| {
                let t = row as f32 * 30.0;
                Line { words: vec![word("a", 0.0, t, 50.0, t + 20.0), word("b", 60.0, t, 120.0, t + 20.0)] }
            })
            .collect();
        let got = texts(&reading_order(lines));
        assert_eq!(got, ["a b", "a b", "a b", "a b"]);
    }

    /// Overlapping boxes at jittered heights leave no band to cut at; ordering them must not
    /// panic (a tolerance comparison is not a total order) and keeps every piece. Found on
    /// MDPBench, where the tolerance sort panicked.
    #[test]
    fn crowded_boxes_without_a_gap_are_ordered_without_panicking() {
        let lines: Vec<Line> = (0..400)
            .map(|i| {
                let x = (i * 37 % 300) as f32;
                let t = (i * 13 % 97) as f32 * 0.4;
                Line { words: vec![word(&i.to_string(), x, t, x + 80.0, t + 20.0)] }
            })
            .collect();
        let got = reading_order(lines);
        assert_eq!(got.len(), 400);
        let mut seen: Vec<String> = texts(&got);
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 400);
    }

    #[test]
    fn nothing_and_degenerate_boxes_do_not_panic() {
        assert!(reading_order(Vec::new()).is_empty());
        let odd = vec![
            Line { words: vec![word("x", f32::NAN, 0.0, 1.0, 1.0)] },
            Line { words: vec![] },
            Line { words: vec![word("y", 5.0, 5.0, 5.0, 5.0), word("z", 5.0, 5.0, 5.0, 5.0)] },
        ];
        let got = reading_order(odd);
        assert_eq!(got.iter().map(|l| l.words.len()).sum::<usize>(), 3);
    }

    /// Boxes turned inside out (right of left edge < left) must not stall the cuts: a cut that
    /// leaves one side empty would hand the same group back forever.
    #[test]
    fn inverted_boxes_end() {
        let lines = vec![
            Line { words: vec![word("a", 0.0, 0.0, 10.0, 10.0)] },
            Line { words: vec![word("b", 100.0, 0.0, 5.0, 10.0)] },
            Line { words: vec![word("c", 0.0, 100.0, 10.0, 5.0)] },
        ];
        let got = reading_order(lines);
        assert_eq!(got.len(), 3);
    }

    fn region(rect: [f32; 4], rank: f32) -> Region {
        Region { label: "text", rect, rank }
    }

    fn paragraph_texts(paragraphs: &[Vec<Line>]) -> Vec<Vec<String>> {
        paragraphs.iter().map(|p| texts(p)).collect()
    }

    fn one_word_line(text: &str, rect: [f32; 4]) -> Line {
        Line { words: vec![word(text, rect[0], rect[1], rect[2], rect[3])] }
    }

    /// Lines are read region by region, in the regions' rank: a sidebar the model ranks after the
    /// main text comes after it, though the XY cuts put it first, and each region is one paragraph
    /// whose lines keep their order.
    #[test]
    fn lines_are_read_region_by_region_in_rank_order() {
        let lines = vec![
            one_word_line("aside1", [0.0, 0.0, 90.0, 20.0]),
            one_word_line("aside2", [0.0, 30.0, 90.0, 50.0]),
            one_word_line("main1", [120.0, 0.0, 400.0, 20.0]),
            one_word_line("main2", [120.0, 30.0, 400.0, 50.0]),
            one_word_line("main3", [120.0, 60.0, 400.0, 80.0]),
        ];
        // The network lists the aside first but ranks it after the main text.
        let regions = [region([-5.0, -5.0, 95.0, 55.0], 7.0), region([110.0, -5.0, 410.0, 85.0], 2.0)];
        assert_eq!(paragraph_texts(&paragraphs(lines, &regions)), [vec!["main1", "main2", "main3"], vec!["aside1", "aside2"]]);
    }

    /// A line goes to the region that covers most of it, and only if that is at least half of it;
    /// a line in no region is a paragraph of its own. Empty regions make no paragraph, and a
    /// region without a rank reads last.
    #[test]
    fn a_line_joins_the_region_covering_most_of_it() {
        let lines = vec![
            // 60 % in region B, 40 % in region A.
            one_word_line("mostly-b", [0.0, 0.0, 100.0, 10.0]),
            // 40 % in region A: loose.
            one_word_line("loose", [100.0, 62.0, 200.0, 82.0]),
            // Exactly half in region A: in.
            one_word_line("half", [10.0, 50.0, 110.0, 60.0]),
            // Equally in C and D: the first in rank order, C.
            one_word_line("tie", [10.0, 200.0, 110.0, 210.0]),
            one_word_line("in-d", [100.0, 200.0, 190.0, 210.0]),
            one_word_line("unranked", [10.0, 100.0, 90.0, 110.0]),
        ];
        let regions = [
            region([60.0, -10.0, 200.0, 70.0], 1.0),      // A
            region([-10.0, -10.0, 60.0, 20.0], 0.0),      // B
            region([-10.0, 195.0, 60.0, 215.0], 3.0),     // C
            region([60.0, 195.0, 200.0, 215.0], 4.0),     // D
            region([0.0, 900.0, 10.0, 910.0], 5.0),       // empty
            region([0.0, 100.0, 100.0, 110.0], f32::NAN), // no rank
        ];
        let got = paragraph_texts(&paragraphs(lines, &regions));
        assert_eq!(got, [vec!["mostly-b"], vec!["loose"], vec!["half"], vec!["tie"], vec!["in-d"], vec!["unranked"]]);
    }

    /// A line in no region stays among its neighbours: right after the paragraph of the line before
    /// it, or first when no line before it is in a region; each is a paragraph of its own.
    #[test]
    fn a_line_in_no_region_stays_among_its_neighbours() {
        let lines = vec![
            one_word_line("caption", [0.0, 0.0, 50.0, 10.0]),
            one_word_line("first", [0.0, 100.0, 50.0, 110.0]),
            one_word_line("first-again", [0.0, 120.0, 50.0, 130.0]),
            one_word_line("label", [0.0, 150.0, 50.0, 160.0]),
            one_word_line("label2", [0.0, 170.0, 50.0, 180.0]),
            one_word_line("second", [0.0, 200.0, 50.0, 210.0]),
            one_word_line("footer", [0.0, 300.0, 50.0, 310.0]),
        ];
        let regions = [region([0.0, 195.0, 60.0, 215.0], 2.0), region([0.0, 95.0, 60.0, 135.0], 1.0)];
        let got = paragraph_texts(&paragraphs(lines, &regions));
        assert_eq!(got, [vec!["caption"], vec!["first", "first-again"], vec!["label"], vec!["label2"], vec!["second"], vec!["footer"]]);
    }

    /// Without regions nothing says where a paragraph ends: the page is one, in line order. No
    /// lines make no paragraph, and broken boxes neither panic nor lose a line.
    #[test]
    fn without_regions_the_page_is_one_paragraph() {
        let lines = || vec![one_word_line("a", [0.0, 0.0, 10.0, 10.0]), one_word_line("b", [0.0, 20.0, 10.0, 30.0])];
        assert_eq!(paragraph_texts(&paragraphs(lines(), &[])), [vec!["a", "b"]]);
        assert!(paragraphs(Vec::new(), &[region([0.0, 0.0, 10.0, 10.0], 0.0)]).is_empty());
        let odd = vec![
            one_word_line("nan", [f32::NAN, 0.0, 10.0, 10.0]),
            Line { words: vec![] },
            one_word_line("flat", [0.0, 5.0, 10.0, 5.0]),
            one_word_line("inf", [f32::NEG_INFINITY, 0.0, f32::INFINITY, 10.0]),
        ];
        let regions = [region([0.0, 0.0, 10.0, 10.0], 0.0), region([f32::NAN, 0.0, f32::INFINITY, 10.0], 1.0)];
        let got = paragraphs(odd, &regions);
        assert_eq!(got.iter().map(Vec::len).sum::<usize>(), 4);
    }
}
