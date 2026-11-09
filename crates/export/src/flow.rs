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
    let t = text.trim_start();
    let Some(first) = t.chars().next() else { return false };
    if BULLETS.contains(&first) || DASHES.contains(&first) || first == '*' {
        return true;
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    // ASCII digits are one byte each and `first` is one whole character, so both cuts fall on
    // character boundaries.
    let (rest, marks): (&str, &[char]) = if (1..=3).contains(&digits) {
        (t.get(digits..).unwrap_or_default(), &['.', ')'])
    } else if digits == 0 && first.is_alphabetic() {
        (t.get(first.len_utf8()..).unwrap_or_default(), &[')'])
    } else {
        return false;
    };
    let mut after = rest.chars();
    after.next().is_some_and(|c| marks.contains(&c)) && after.next().is_none_or(char::is_whitespace)
}

/// Whether a line's text ends a sentence (closing quotes and brackets aside). Text set without
/// paragraph spacing gives no other sign that a paragraph ends there, so such a line is left
/// apart from the next rather than joined to it wrongly.
fn ends_sentence(text: &str) -> bool {
    text.trim_end()
        .trim_end_matches(['"', '\'', '”', '’', '“', '»', ')', ']', '」', '』'])
        .ends_with(['.', '!', '?', ':', ';', '…', '。', '！', '？'])
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
    if right <= left {
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
                let unspaced_break = |end: &str| {
                    end.chars().next_back().is_some_and(unspaced) && para.first().and_then(|r| r.text.chars().next()).is_some_and(unspaced)
                };
                if let Some(end) = prev.last_mut()
                    && !end.text.ends_with(char::is_whitespace)
                    && !end.text.ends_with('-')
                    && !unspaced_break(&end.text)
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
    if !alike || a.rect[3] - a.rect[1] > ONE_LINE * size || list_item(&first.text) || ends_sentence(&last.text) {
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
