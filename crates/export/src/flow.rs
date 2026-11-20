//! Text that runs on from one line to the next.
//!
//! The reading blocks keep style boundaries (#4158), so a sentence with a bold name or an italic
//! word in it reaches the exporter as several blocks, and `layout::paragraphs` joins only the ones
//! that run on along one line. A sentence that wraps onto the next line after
//! such a run came out as one paragraph per line. [`join_wrapped`] joins a paragraph whose last
//! line reaches the right edge of the page's text with the one that starts just below it at the
//! same left edge, in the same size: the text wrapped there. Headings, list items, a line that
//! ends a sentence and a line with paragraph space after it stay apart.
//!
//! A list item's bullet is often a block of its own: drawn in a symbol font, a tab stop before
//! the item's text. Table detection read such a list as a two-column table, bullets in one
//! column and the items in the other. [`bullet_items`] pairs each bullet with its text, so the
//! text runs on from its bullet and the bullets stay out of table detection.

use super::layout::{Block, Run};

/// Bullets that mark a list item wherever they stand, including the Symbol and Wingdings bullets
/// Word writes into its PDFs (U+F0B7, U+F0A7).
const BULLETS: [char; 11] = ['•', '◦', '▪', '‣', '⁃', '●', '○', '■', '□', '\u{F0B7}', '\u{F0A7}'];

/// Dashes and a middle dot: list markers at the start of a line, punctuation inside one.
const DASHES: [char; 4] = ['-', '\u{2013}', '\u{2014}', '·'];

/// A run taller than this many times its font size is more than one line: a block of several
/// rows ends where its widest row does, not where its last one does.
const ONE_LINE: f64 = 1.6;

/// How far (as a multiple of the font size) the space between two lines may be from none, and
/// they still follow each other within a paragraph: more is paragraph spacing.
const LINE_GAP: f64 = 0.5;

/// How close to the right edge of the page's text a full line ends: within this many times the
/// font size, or this fraction of the text's width, whichever is more (a ragged line ends short
/// of the edge by up to a long word).
const FULL_LINE: (f64, f64) = (3.0, 0.12);

/// How far (points) the first line of a paragraph may start from the left edge of the line above
/// it and still be at the same left edge.
const SAME_LEFT: f64 = 2.0;

/// How far after its bullet (as a multiple of the bullet's size) a list item's text may start: a
/// tab stop away.
const BULLET_GAP: f64 = 3.0;

/// How many blocks [`bullet_items`] may look at on one page, so that a page of many bullets stays
/// quick; past it, the bullets left are not taken for bullets.
const BULLET_BUDGET: usize = 1_000_000;

/// Takes a step from `budget`: whether there was one left.
fn step(budget: &mut usize) -> bool {
    budget.checked_sub(1).map(|left| *budget = left).is_some()
}

/// Whether two boxes ([x0, y0, x1, y1]) share a line: they overlap vertically by more than half
/// the shorter one, as `layout::continued_from` has it.
fn on_one_line(a: [f64; 4], b: [f64; 4]) -> bool {
    a[3].min(b[3]) - a[1].max(b[1]) > 0.5 * (a[3] - a[1]).min(b[3] - b[1])
}

/// Whether a block is a bullet drawn on its own: one bullet, dash or middle dot, no wider than one
/// and a half times its size.
fn is_bullet(b: &Block) -> bool {
    let mut chars = b.text.trim().chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return false };
    (BULLETS.contains(&c) || DASHES.contains(&c)) && b.rect[2] - b.rect[0] <= 1.5 * b.size
}

/// Whether no other block on `blocks[i]`'s line starts left of it. Each block looked at takes a
/// step from `budget`; when the budget runs out the answer is no.
fn opens_line(i: usize, blocks: &[Block], budget: &mut usize) -> bool {
    let Some(a) = blocks.get(i) else { return false };
    for (j, b) in blocks.iter().enumerate() {
        if !step(budget) {
            return false;
        }
        if j != i && b.rect[0] < a.rect[0] && on_one_line(a.rect, b.rect) {
            return false;
        }
    }
    true
}

/// The list items of a page whose bullet is a block of its own, as (bullet, text): the text is the
/// block that starts nearest after the bullet on its line, within [`BULLET_GAP`] times its size.
/// A dash or a middle dot is a bullet only when it opens its line; inside a line it is
/// punctuation, or a table cell's "none". The items' texts of a list all start at one tab stop,
/// so the search looks through every block that starts there, within [`BULLET_BUDGET`].
pub(crate) fn bullet_items(blocks: &[Block]) -> Vec<(usize, usize)> {
    let mut by_start: Vec<(f64, usize)> = blocks.iter().enumerate().map(|(i, b)| (b.rect[0], i)).collect();
    by_start.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut budget = BULLET_BUDGET;
    let mut out = Vec::new();
    for (ai, a) in blocks.iter().enumerate().filter(|(_, b)| is_bullet(b)) {
        // The text may start a point inside the bullet's box, as a run that continues another can.
        let k = by_start.partition_point(|(x, _)| *x < a.rect[2] - 1.0);
        let text = by_start
            .get(k..)
            .unwrap_or_default()
            .iter()
            .take_while(|(x, _)| *x <= a.rect[2] + BULLET_GAP * a.size)
            .take_while(|_| step(&mut budget))
            .filter(|(_, bi)| *bi != ai)
            .filter_map(|(_, bi)| blocks.get(*bi).map(|b| (*bi, b)))
            .find(|(_, b)| on_one_line(a.rect, b.rect) && (a.rotation - b.rotation).abs() <= 0.1 && !is_bullet(b))
            .map(|(bi, _)| bi);
        let Some(text) = text else { continue };
        let dash = a.text.trim().chars().next().is_some_and(|c| DASHES.contains(&c));
        if !dash || opens_line(ai, blocks, &mut budget) {
            out.push((ai, text));
        }
    }
    out
}

/// Whether `text` opens like a list item: a bullet, a dash or `*`; one to three digits followed
/// by `.` or `)`; or one letter followed by `)` ("a)", "а)"); then a space or nothing. "1.5 kg",
/// "i.e." and "2026. Year" are text.
pub(crate) fn list_item(text: &str) -> bool {
    list_item_chars(text.chars().map(|c| (0usize, c)))
}

fn list_item_runs(runs: &[Run]) -> bool {
    list_item_chars(runs.iter().enumerate().flat_map(|(ri, r)| r.text.chars().map(move |c| (ri, c))))
}

fn list_item_chars(mut chars: impl Iterator<Item = (usize, char)>) -> bool {
    let Some((_, first)) = chars.find(|(_, c)| !c.is_whitespace()) else { return false };
    if BULLETS.contains(&first) || DASHES.contains(&first) || first == '*' {
        return true;
    }
    let mark_run = if first.is_ascii_digit() {
        let mut digits = 1usize;
        loop {
            let Some((ri, c)) = chars.next() else { return false };
            if c.is_ascii_digit() {
                digits += 1;
                if digits > 3 {
                    return false;
                }
            } else if c == '.' || c == ')' {
                break ri;
            } else {
                return false;
            }
        }
    } else if first.is_alphabetic() {
        let Some((ri, ')')) = chars.next() else { return false };
        ri
    } else {
        return false;
    };
    chars.next().is_none_or(|(ri, c)| c.is_whitespace() || (ri != mark_run && !c.is_ascii_digit()))
}

/// Whether a paragraph's trailing text ends a sentence (closing quotes and brackets aside). Text
/// set without paragraph spacing gives no other sign that a paragraph ends there, so such a line
/// is left apart from the next rather than joined to it wrongly.
fn ends_sentence(runs: &[Run]) -> bool {
    let mut chars = runs.iter().rev().flat_map(|r| r.text.chars().rev()).skip_while(|c| c.is_whitespace());
    chars
        .find(|c| !['"', '\'', '”', '’', '“', '»', ')', ']', '」', '』'].contains(c))
        .is_some_and(|c| ['.', '!', '?', ':', ';', '…', '。', '！', '？'].contains(&c))
}

/// Whether `c` is of the writing that puts no space between words: Chinese and Japanese
/// characters, their punctuation and fullwidth forms. A line that wraps between two of them
/// joins without a word space.
fn unspaced(c: char) -> bool {
    matches!(
        c,
        '\u{3000}'..='\u{30FF}' // CJK punctuation, Hiragana, Katakana
            | '\u{31F0}'..='\u{31FF}' // Katakana extension
            | '\u{3400}'..='\u{4DBF}' // CJK extension A
            | '\u{4E00}'..='\u{9FFF}' // CJK unified ideographs
            | '\u{F900}'..='\u{FAFF}' // CJK compatibility ideographs
            | '\u{FF01}'..='\u{FF9F}' // fullwidth forms, halfwidth Katakana
            | '\u{20000}'..='\u{3FFFF}' // CJK extension B onwards
    )
}

/// `paras` (a page's paragraphs, in any order) top to bottom, with each paragraph that wraps onto
/// the next line joined to it. `level` is a paragraph's heading level (0 for body text).
pub(crate) fn join_wrapped<'a>(mut paras: Vec<Vec<Run<'a>>>, level: impl Fn(&[Run<'a>]) -> u8) -> Vec<Vec<Run<'a>>> {
    // The left and right edges of the page's text: where a full line starts and ends.
    let (left, right) = paras
        .iter()
        .flatten()
        .map(|r| r.block.rect)
        .filter(|r| r[0].is_finite() && r[2].is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, r), x| (l.min(x[0]), r.max(x[2])));
    if right <= left || !(right - left).is_finite() {
        return paras;
    }
    // Top to bottom, then left to right, by each paragraph's first run.
    paras.sort_by(|a, b| {
        let top_left = |p: &[Run]| p.first().map_or([0.0; 2], |r| [r.block.rect[3], r.block.rect[0]]);
        let (a, b) = (top_left(a), top_left(b));
        b[0].total_cmp(&a[0]).then(a[1].total_cmp(&b[1]))
    });
    // Each paragraph with its heading level. A joined paragraph is body text: only body text joins.
    let mut out: Vec<(Vec<Run<'a>>, u8)> = Vec::with_capacity(paras.len());
    for para in paras {
        let lvl = level(&para);
        match out.last_mut() {
            Some((prev, 0)) if lvl == 0 && wraps_into(prev, &para, [left, right]) => {
                // The line break was a word space, unless the line ends in a hyphen; the hyphen
                // stays, as it may belong to the word ("well-known"). Chinese and Japanese text
                // wraps between characters, with no space.
                let last_char = prev.iter().rev().flat_map(|r| r.text.chars().rev()).next();
                let first_char = para.iter().flat_map(|r| r.text.chars()).next();
                if let (Some(end_ch), Some(start_ch)) = (last_char, first_char)
                    && !end_ch.is_whitespace()
                    && !start_ch.is_whitespace()
                    && end_ch != '-'
                    && !(unspaced(end_ch) && unspaced(start_ch))
                    && let Some(end) = prev.iter_mut().rev().find(|r| !r.text.is_empty())
                {
                    end.text.push(' ');
                }
                prev.extend(para);
            }
            _ => out.push((para, lvl)),
        }
    }
    out.into_iter().map(|(para, _)| para).collect()
}

/// Whether the body text paragraph `prev` ends in a full line whose text runs on in `next`, the
/// body text paragraph just below it. `[left, right]` are the edges of the page's text.
fn wraps_into(prev: &[Run], next: &[Run], [left, right]: [f64; 2]) -> bool {
    let (Some(last), Some(first)) = (prev.last(), next.first()) else { return false };
    let (a, b) = (last.block, first.block);
    let size = a.size;
    let alike = size.is_finite() && size > 0.0 && (a.size - b.size).abs() <= 0.5 && a.rotation.abs() <= 0.1 && b.rotation.abs() <= 0.1;
    let starts_list = if next.len() == 1 { list_item(&first.text) } else { list_item_runs(next) };
    if !alike || a.rect[3] - a.rect[1] > ONE_LINE * size || starts_list || ends_sentence(prev) {
        return false;
    }
    // The last line: the runs at the end of `prev` that share the last run's line.
    let (line_left, line_right) = prev
        .iter()
        .rev()
        .take_while(|r| on_one_line(r.block.rect, a.rect))
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, r), x| (l.min(x.block.rect[0]), r.max(x.block.rect[2])));
    let full = line_right >= right - (FULL_LINE.0 * size).max(FULL_LINE.1 * (right - left));
    let same_left = (b.rect[0] - line_left).abs() <= SAME_LEFT;
    // Just below: no paragraph space between the two lines.
    let gap = a.rect[1] - b.rect[3];
    full && same_left && (-LINE_GAP * size..=LINE_GAP * size).contains(&gap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str, rect: [f64; 4], bold: bool, italic: bool) -> Block {
        Block { text: text.to_string(), rect, size: 12.0, bold, italic, color: [0.0, 0.0, 0.0], rotation: 0.0, space_around: [false, false] }
    }

    fn run<'a>(b: &'a Block) -> Run<'a> {
        Run { block: b, text: b.text.clone() }
    }

    #[test]
    fn sentence_end_and_list_marker_split_across_runs_do_not_join_wrapped() {
        // 1. A sentence whose period is in an italic run followed by a closing parenthesis run
        // on a full-width line must not merge with the paragraph below it.
        let b1 = block("The parties agreed (", [50.0, 700.0, 200.0, 712.0], false, false);
        let b2 = block("for one year.", [200.0, 700.0, 445.0, 712.0], false, true);
        let b3 = block(")", [445.0, 700.0, 450.0, 712.0], false, false);
        let b4 = block("The agreement begins in May", [50.0, 686.0, 300.0, 698.0], false, false);

        let paras = vec![vec![run(&b1), run(&b2), run(&b3)], vec![run(&b4)]];
        let joined = join_wrapped(paras, |_| 0);
        assert_eq!(joined.len(), 2, "sentence ending before closing paren run should not wrap into next paragraph");

        // 2. A numbered or lettered list item whose marker is split across styled runs (e.g. bold "2" + ". Item"
        // or italic "a" + ") Item") must not merge into the full-width line above it.
        let lead = block("The tenant shall comply with the following rules and conditions", [50.0, 700.0, 450.0, 712.0], false, false);
        let num_bold = block("2", [50.0, 686.0, 58.0, 698.0], true, false);
        let num_rest = block(". Keep the premises clean", [58.0, 686.0, 250.0, 698.0], false, false);
        let paras_num = vec![vec![run(&lead)], vec![run(&num_bold), run(&num_rest)]];
        let joined_num = join_wrapped(paras_num, |_| 0);
        assert_eq!(joined_num.len(), 2, "split numbered list marker should stay a separate paragraph");

        let let_italic = block("a", [50.0, 686.0, 58.0, 698.0], false, true);
        let let_rest = block(") Keep the premises clean", [58.0, 686.0, 250.0, 698.0], false, false);
        let paras_let = vec![vec![run(&lead)], vec![run(&let_italic), run(&let_rest)]];
        let joined_let = join_wrapped(paras_let, |_| 0);
        assert_eq!(joined_let.len(), 2, "split lettered list marker should stay a separate paragraph");

        // 3. A styled marker run ("1." or "2" + ".") followed by an item run without a leading space
        // must still be recognized as a list item, whereas a split decimal ("1." + "5 kg") wraps as text.
        let num_marker = block("1.", [50.0, 686.0, 62.0, 698.0], true, false);
        let item_no_space = block("Pay the rent on time", [70.0, 686.0, 250.0, 698.0], false, false);
        let joined_marker = join_wrapped(vec![vec![run(&lead)], vec![run(&num_marker), run(&item_no_space)]], |_| 0);
        assert_eq!(joined_marker.len(), 2, "styled '1.' run followed by unspaced item run must stay apart");

        let dot_run = block(".", [58.0, 686.0, 62.0, 698.0], false, false);
        let joined_three = join_wrapped(vec![vec![run(&lead)], vec![run(&num_bold), run(&dot_run), run(&item_no_space)]], |_| 0);
        assert_eq!(joined_three.len(), 2, "three-run split list marker must stay apart");

        let dec_frac = block("5 kg of flour", [62.0, 686.0, 200.0, 698.0], false, false);
        let joined_dec = join_wrapped(vec![vec![run(&lead)], vec![run(&num_marker), run(&dec_frac)]], |_| 0);
        assert_eq!(joined_dec.len(), 1, "split decimal '1.' + '5 kg' should wrap as ordinary text");
    }

    #[test]
    fn join_wrapped_handles_overflowing_page_span_and_trailing_empty_runs() {
        // Extreme coordinates whose span (right - left) overflows f64 to +inf must not treat short lines as full-width.
        let b_far_left = block("Short line", [-1.0e308, 700.0, -1.0e308 + 50.0, 712.0], false, false);
        let b_below = block("Next paragraph", [-1.0e308, 686.0, -1.0e308 + 60.0, 698.0], false, false);
        let b_far_right = block("Far right", [1.0e308 - 50.0, 500.0, 1.0e308, 512.0], false, false);
        let paras = vec![vec![run(&b_far_left)], vec![run(&b_below)], vec![run(&b_far_right)]];
        let joined = join_wrapped(paras, |_| 0);
        assert_eq!(joined.len(), 3);

        // Hyphen before a trailing empty run must not insert a spurious space when wrapping.
        let b_hyph = block("well-", [50.0, 700.0, 449.0, 712.0], false, false);
        let b_empty = block("", [449.0, 700.0, 450.0, 712.0], false, true);
        let b_cont = block("known fact.", [50.0, 686.0, 200.0, 698.0], false, false);
        let joined_hyph = join_wrapped(vec![vec![run(&b_hyph), run(&b_empty)], vec![run(&b_cont)]], |_| 0);
        assert_eq!(joined_hyph.len(), 1);
        let text: String = joined_hyph.iter().flatten().map(|r| r.text.as_str()).collect();
        assert_eq!(text, "well-known fact.");
    }
}
