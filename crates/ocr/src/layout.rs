//! Columns and reading order of recognised lines.
//!
//! The line finder joins words that sit at one height into one line, also across the gutter
//! between two columns, and returns lines top to bottom, so a newspaper's lines run from one
//! column into the next and the text (copied, searched or exported) comes out interleaved. [`reading_order`] splits each line at gaps far wider than a word space,
//! then orders the pieces by recursive XY cuts: the page is cut at its widest empty band,
//! across or down, and each part in turn, so a heading over the columns comes first, then each
//! column from top to bottom.

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
}
