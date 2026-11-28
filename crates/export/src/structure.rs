//! What a document's pages hold beyond their paragraphs, for the semantic HTML export (#7196):
//! the running headers and footers that repeat in their margins, and the runs set as subscripts
//! or superscripts. It only reads [`Page`]s.

use std::collections::{HashMap, HashSet};

use crate::layout::{INLINE_GAP, overhang};
use crate::{Page, TextRun};

mod figures;
mod sections;
mod tables;

pub use figures::{Caption, CaptionKind, Figure};
pub use sections::{Heading, OutlineEntry, Reference};
pub use tables::TableStructure;

/// Where a run sits on its line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Script {
    #[default]
    Baseline,
    /// Smaller and lowered: the "2" of H₂O, the "CC" of V_CC.
    Sub,
    /// Smaller and raised: the "2" of mc², a footnote mark.
    Super,
}

/// What [`structure`] finds on one page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageStructure {
    /// Per block of [`Page::blocks`]: whether it is a running header or footer, which a
    /// semantic export leaves out of the document's text.
    pub running: Vec<bool>,
    /// Per run of [`Page::runs`]: where it sits on its line.
    pub scripts: Vec<Script>,
    /// The page's figure and table captions, in block order.
    pub captions: Vec<Caption>,
    /// The page's figures, in the order of their captions.
    pub figures: Vec<Figure>,
    /// The page's tables, as the other exports detect them, top to bottom.
    pub tables: Vec<TableStructure>,
    /// The headings of the outline's bookmarks that go to this page, in outline order.
    pub headings: Vec<Heading>,
    /// The mentions of figures and tables in the page's text, in block and text order.
    pub references: Vec<Reference>,
}

/// The structure of each page of `pages`, in order, for a document without an outline: see
/// [`structure_with_outline`].
pub fn structure(pages: &[Page]) -> Vec<PageStructure> {
    structure_with_outline(pages, &[])
}

/// The structure of each page of `pages`, in order, with the headings of the document's
/// `outline`.
///
/// A block is a running header or footer when it is upright, short and lies entirely within a
/// tenth of the page's height from its top or bottom edge, and the same text is in that margin on
/// at least two pages and at least two fifths of them. Numbers in it may change from page to page
/// only as the pages do ("Page 3 of 12", then "Page 4 of 12"; roman numerals too), so a "Table 3"
/// and a "Table 5" are different text. A caption ("Figure 2", "Table 3 (continued)") is never
/// one.
///
/// A caption is a paragraph that starts with "Figure", "Fig." or "Table" and a number ("3",
/// "2-1", "A.4", "3b"), followed by nothing, punctuation, or text that does not start in
/// lowercase, so "Figure 3 shows" is not one. A figure caption names the drawings, pictures and
/// short labels grouped closest to it, above or below it and overlapping it across; the paths
/// of a table's grid, and a background or frame covering half the page, are not part of a figure.
///
/// A table's header is its first rows set all in bold (while a row after them is not), or else a
/// first row with a cell over some of the columns and the row of sub-columns under it. Below
/// the header of a table with no rules between its rows, an empty cell is spanned by the cell
/// above it when the cells before it on its row are spanned too and a cell after it has text.
/// A table caption names the table nearest it, under it first. A table's notes are the blocks
/// right under it that start with a note mark ("Note 1:", "(2)", "*"). A table continues the last
/// table of the page before when their columns line up and its caption continues that table's,
/// or its first rows repeat that table's header, or it has no caption and nothing but running
/// headers and footers lies between the two.
///
/// Each bookmark of the outline names the block on its page whose text is its title (letter
/// case, spacing, a section number before either and the punctuation ending them aside); of
/// several, the one nearest the height it goes to. A mention of a figure or table ("see Figure
/// 3", "in table 2-1") anywhere in a block's text points to the first caption with its kind and
/// number.
///
/// A run is a subscript or superscript when it is upright and short, and a run it follows or
/// precedes on its line, at most four runs away in content order, sits on a baseline below or
/// above its own and is set in a size at least a tenth larger (but not as much larger as a drop
/// cap).
pub fn structure_with_outline(pages: &[Page], outline: &[OutlineEntry]) -> Vec<PageStructure> {
    let mut seen = HashSet::new();
    let mut ends = Vec::with_capacity(pages.len());
    let mut out: Vec<PageStructure> = running_blocks(pages)
        .into_iter()
        .zip(pages)
        .map(|(running, p)| {
            let captions = figures::captions(p, &running, &mut seen);
            let (found, consumed) = crate::tables(&p.blocks, &p.rules);
            let figures = figures::figures(p, &captions, &running, &found, &consumed);
            let tables = tables::page_tables(p, found, &captions, &running, &consumed);
            ends.push(tables::page_ends(p, &tables, &running, &consumed));
            PageStructure { scripts: scripts(&p.runs), running, captions, figures, tables, headings: Vec::new(), references: Vec::new() }
        })
        .collect();
    for (pi, pair) in ends.windows(2).enumerate() {
        let ([_, Some((last, ends_page))], [Some((first, starts_page)), _]) = (pair[0], pair[1]) else { continue };
        let (before, after) = out.split_at_mut(pi + 1);
        let (Some(prev_page), Some(page)) = (before.last(), after.first_mut()) else { continue };
        let Some(prev) = prev_page.tables.get(last) else { continue };
        let number = prev.caption.and_then(|c| prev_page.captions.get(c)).map(|c| c.number.as_str());
        let open = tables::Open { page: pi, table: prev, number, ends_page };
        let found = page.tables.get(first).and_then(|t| tables::continues(&open, pi + 1, t, &page.captions, starts_page));
        if let (Some(repeats), Some(t)) = (found, page.tables.get_mut(first)) {
            t.continues = Some((pi, last));
            t.repeats_header = repeats;
            if repeats {
                t.header_rows = prev.header_rows;
            }
        }
    }
    let headings = sections::headings(pages, &out, outline);
    let references = sections::references(pages, &out);
    for ((s, h), r) in out.iter_mut().zip(headings).zip(references) {
        s.headings = h;
        s.references = r;
    }
    out
}

/// A running header or footer lies within this fraction of the page's height from its top or
/// bottom edge.
const MARGIN_BAND: f64 = 0.1;

/// Text longer than this (characters) is not a running header or footer.
const MAX_RUNNING_CHARS: usize = 160;

/// How many other occurrences of the same text, nearest in page order, one is compared with; a
/// document whose every page repeats a header stays quick.
const RUNNING_PROBE: usize = 4;

/// A digit run longer than this is kept as text, not read as a number that may change.
const MAX_NUMBER_DIGITS: usize = 9;

/// A roman numeral longer than this is not read as a page number.
const MAX_ROMAN_CHARS: usize = 8;

/// A block in a page's top or bottom margin, and the numbers in its text.
struct InMargin {
    page: usize,
    block: usize,
    numbers: Vec<u64>,
}

/// Per page, per block: whether it is a running header or footer (see [`structure`]).
fn running_blocks(pages: &[Page]) -> Vec<Vec<bool>> {
    let mut out: Vec<Vec<bool>> = pages.iter().map(|p| vec![false; p.blocks.len()]).collect();
    // Per margin (top or bottom) and text with its numbers taken out: where it is, in page order.
    let mut groups: HashMap<(bool, String), Vec<InMargin>> = HashMap::new();
    for (pi, p) in pages.iter().enumerate() {
        if !(p.height.is_finite() && p.height > 0.0) {
            continue;
        }
        let band = p.height * MARGIN_BAND;
        for (bi, b) in p.blocks.iter().enumerate() {
            let top = b.rect[1] >= p.height - band;
            if !(b.rotation.abs() <= 0.1 && (top || b.rect[3] <= band)) || figures::caption_label(&b.text).is_some() {
                continue;
            }
            if let Some((text, numbers)) = template(&b.text) {
                groups.entry((top, text)).or_default().push(InMargin { page: pi, block: bi, numbers });
            }
        }
    }
    for found in groups.values() {
        let mut on_pages = found.iter().map(|f| f.page).collect::<Vec<_>>();
        on_pages.dedup();
        if on_pages.len() < 2 || on_pages.len().saturating_mul(5) < pages.len().saturating_mul(2) {
            continue;
        }
        for (k, f) in found.iter().enumerate() {
            let near = found
                .get(k.saturating_sub(RUNNING_PROBE)..k)
                .unwrap_or_default()
                .iter()
                .chain(found.get(k + 1..).unwrap_or_default().iter().take(RUNNING_PROBE));
            if near.filter(|o| o.page != f.page).any(|o| follows_pages(&f.numbers, f.page, &o.numbers, o.page))
                && let Some(slot) = out.get_mut(f.page).and_then(|p| p.get_mut(f.block))
            {
                *slot = true;
            }
        }
    }
    out
}

/// Whether two occurrences of a text on pages `pa` and `pb` have numbers that are equal or apart
/// by as many as the pages are.
fn follows_pages(a: &[u64], pa: usize, b: &[u64], pb: usize) -> bool {
    let step = pa.abs_diff(pb) as u64;
    a.len() == b.len()
        && a.iter().zip(b).all(|(&x, &y)| {
            let (first, last) = if pa < pb { (x, y) } else { (y, x) };
            x == y || last.checked_sub(first) == Some(step)
        })
}

/// `text` with its spaces collapsed and each number replaced by `#`, and those numbers; `None`
/// for text too long or empty to be a running header or footer. Text that is only a roman
/// numeral is one number.
fn template(text: &str) -> Option<(String, Vec<u64>)> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() || text.chars().count() > MAX_RUNNING_CHARS {
        return None;
    }
    if let Some(n) = roman(&text) {
        return Some(("#".into(), vec![n]));
    }
    let mut out = String::with_capacity(text.len());
    let mut numbers = Vec::new();
    let mut digits = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            take_number(&mut digits, &mut out, &mut numbers);
            out.push(c);
        }
    }
    take_number(&mut digits, &mut out, &mut numbers);
    Some((out, numbers))
}

/// Moves a run of `digits` into the template `out`: as `#` and its value in `numbers`, or as
/// itself when it is too long to be a number that changes.
fn take_number(digits: &mut String, out: &mut String, numbers: &mut Vec<u64>) {
    if digits.is_empty() {
        return;
    }
    match digits.parse::<u64>() {
        Ok(n) if digits.len() <= MAX_NUMBER_DIGITS => {
            out.push('#');
            numbers.push(n);
        }
        _ => out.push_str(digits),
    }
    digits.clear();
}

/// The value of a lowercase or uppercase roman numeral made of i, v and x (front matter page
/// numbers), or `None`.
fn roman(word: &str) -> Option<u64> {
    let lower = word.chars().all(|c| matches!(c, 'i' | 'v' | 'x'));
    let upper = word.chars().all(|c| matches!(c, 'I' | 'V' | 'X'));
    if !(lower || upper) || word.is_empty() || word.len() > MAX_ROMAN_CHARS {
        return None;
    }
    let values: Vec<i64> = word
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            'x' => 10,
            'v' => 5,
            _ => 1,
        })
        .collect();
    // A numeral smaller than the one after it is taken away from it ("ix" is 9).
    let total: i64 = values.iter().enumerate().map(|(k, &v)| if values.get(k + 1).is_some_and(|&next| next > v) { -v } else { v }).sum();
    u64::try_from(total).ok()
}

/// How many runs before and after a run, in content order, are looked at to find the one on its
/// line it is a script of.
const SCRIPT_PROBE: usize = 4;

/// A script is set at most this fraction of the size of the run it is a script of.
const SCRIPT_SIZE: f64 = 0.9;

/// A run more than this many times a smaller one's size is a drop cap or a heading beside it,
/// not the line it is a script of.
const MAX_SCRIPT_RATIO: f64 = 2.2;

/// How far a superscript's baseline rises above its line's, as a fraction of the line's size.
const RISE: [f64; 2] = [0.1, 0.6];

/// How far a subscript's baseline drops below its line's, as a fraction of the line's size.
const DROP: [f64; 2] = [0.08, 0.45];

/// Text longer than this (characters) is not a script.
const MAX_SCRIPT_CHARS: usize = 12;

/// Per run: where it sits on its line (see [`structure`]).
fn scripts(runs: &[TextRun]) -> Vec<Script> {
    (0..runs.len())
        .map(|i| {
            let Some(r) = runs.get(i).filter(|r| could_be_script(r)) else { return Script::Baseline };
            (1..=SCRIPT_PROBE)
                .flat_map(|d| [i.checked_sub(d), i.checked_add(d)])
                .filter_map(|j| runs.get(j?))
                .find_map(|line| script_of(r, line))
                .unwrap_or_default()
        })
        .collect()
}

/// Whether `r` is upright, short text in a usable size.
fn could_be_script(r: &TextRun) -> bool {
    let chars = r.text.trim().chars().count();
    r.rotation.abs() <= 0.1 && r.size.is_finite() && r.size > 0.0 && (1..=MAX_SCRIPT_CHARS).contains(&chars)
}

/// Where `r` sits as a script of `line`, a run beside it, or `None` when it is not one.
fn script_of(r: &TextRun, line: &TextRun) -> Option<Script> {
    if !(line.rotation.abs() <= 0.1 && line.size.is_finite() && r.size <= line.size * SCRIPT_SIZE && line.size <= r.size * MAX_SCRIPT_RATIO) {
        return None;
    }
    let beside = |gap: f64| gap >= -overhang(line.size) && gap <= INLINE_GAP * line.size;
    if !(beside(r.rect[0] - line.rect[2]) || beside(line.rect[0] - r.rect[2])) {
        return None;
    }
    let shift = (r.baseline[1] - line.baseline[1]) / line.size;
    if (RISE[0]..=RISE[1]).contains(&shift) {
        Some(Script::Super)
    } else if (DROP[0]..=DROP[1]).contains(&-shift) {
        Some(Script::Sub)
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
