//! Captions ("Figure 3. Typical application", "Table 2-1 Pin descriptions") and the region of the
//! page each figure caption names: the drawings, images and labels next to it.

use std::collections::HashSet;

use crate::{Page, Table};

/// What a caption names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CaptionKind {
    Figure,
    Table,
}

/// A paragraph that starts with a figure's or a table's label and number.
#[derive(Clone, Debug, PartialEq)]
pub struct Caption {
    /// Its block in [`Page::blocks`].
    pub block: usize,
    pub kind: CaptionKind,
    /// Its number as printed: "3", "2-1", "A.4", "3b".
    pub number: String,
    /// Whether an earlier caption of the document has the same kind and number, as a table or
    /// figure continued from an earlier page has.
    pub continued: bool,
}

/// A figure: what a figure caption names on its page.
#[derive(Clone, Debug, PartialEq)]
pub struct Figure {
    /// Its caption in [`PageStructure::captions`](super::PageStructure::captions).
    pub caption: usize,
    /// The box of what it is made of, [x0, y0, x1, y1] on the displayed page in points (y up):
    /// its drawings, images and labels, not its caption.
    pub rect: [f64; 4],
    /// Its paths in [`Page::drawings`].
    pub drawings: Vec<usize>,
    /// Its pictures in [`Page::images`].
    pub images: Vec<usize>,
    /// The short blocks of [`Page::blocks`] it holds: axis labels, pin names, a legend.
    pub labels: Vec<usize>,
}

/// The labels a caption starts with, in the order they are tried.
const LABELS: [(&str, CaptionKind); 4] =
    [("figure", CaptionKind::Figure), ("fig.", CaptionKind::Figure), ("fig", CaptionKind::Figure), ("table", CaptionKind::Table)];

/// A caption's number is at most this long ("A.12-3b").
const MAX_NUMBER_CHARS: usize = 16;

/// The label a caption's text starts with and its number: "Figure 3." or "Table 2-1 Pin
/// descriptions", not "Figure 3 shows". After the number comes nothing, punctuation, or a space
/// and text that does not start in lowercase.
pub(super) fn caption_label(text: &str) -> Option<(CaptionKind, &str)> {
    let (kind, number, after) = label_number(text.trim_start())?;
    let ends_label = match after.chars().next() {
        None => true,
        Some(c) if c.is_whitespace() => !after.trim_start().starts_with(char::is_lowercase),
        Some(c) => ".:|)\u{2013}\u{2014}-".contains(c),
    };
    ends_label.then_some((kind, number))
}

/// A mention of a figure or table that `text` starts with ("Figure 3", "table 2-1", "Fig. 4b"):
/// its kind, its length in bytes and its number; the number may not run on into a word.
pub(super) fn mention(text: &str) -> Option<(CaptionKind, usize, &str)> {
    let (kind, number, after) = label_number(text)?;
    if after.starts_with(char::is_alphanumeric) {
        return None;
    }
    Some((kind, text.len() - after.len(), number))
}

/// The label `text` starts with (any case; a space after it, or after "Fig." none needed), the
/// number after it, and the text after that.
fn label_number(text: &str) -> Option<(CaptionKind, &str, &str)> {
    LABELS.iter().find_map(|&(label, kind)| {
        let head = text.get(..label.len())?;
        let rest = text.get(label.len()..)?;
        let after_label = rest.trim_start();
        if !head.eq_ignore_ascii_case(label) || (after_label.len() == rest.len() && !label.ends_with('.')) {
            return None;
        }
        let end = number_len(after_label)?;
        Some((kind, after_label.get(..end)?, after_label.get(end..)?))
    })
}

/// The length in bytes of the caption number `s` starts with: an optional capital letter and a
/// `.` or `-` ("A.4"), digits, more groups of digits after `.` or `-` ("2-1", "3.4"), and an
/// optional lowercase letter ("3b").
fn number_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let at = |i: usize| b.get(i).copied().unwrap_or(0);
    let mut end = 0;
    if at(0).is_ascii_uppercase() && matches!(at(1), b'.' | b'-') && at(2).is_ascii_digit() {
        end = 2;
    }
    if !at(end).is_ascii_digit() {
        return None;
    }
    loop {
        while at(end).is_ascii_digit() {
            end += 1;
        }
        if matches!(at(end), b'.' | b'-') && at(end + 1).is_ascii_digit() {
            end += 1;
        } else {
            break;
        }
    }
    if at(end).is_ascii_lowercase() && !at(end + 1).is_ascii_alphabetic() {
        end += 1;
    }
    (end <= MAX_NUMBER_CHARS).then_some(end)
}

/// A page's captions, in block order. `seen` holds the kinds and numbers of the captions of the
/// pages before, so a caption that repeats one is `continued`.
pub(super) fn captions(page: &Page, running: &[bool], seen: &mut HashSet<(CaptionKind, String)>) -> Vec<Caption> {
    page.blocks
        .iter()
        .enumerate()
        .filter(|(bi, _)| !running.get(*bi).copied().unwrap_or(false))
        .filter_map(|(block, b)| {
            let (kind, number) = caption_label(&b.text)?;
            let continued = !seen.insert((kind, number.to_owned()));
            Some(Caption { block, kind, number: number.to_owned(), continued })
        })
        .collect()
}

/// Paths and pictures this close (points) are parts of one drawing.
const CLUSTER_GAP: f64 = 12.0;

/// A short block this close (points) to a drawing is one of its labels.
const LABEL_GAP: f64 = 6.0;

/// Labels are found this many times, each time next to the drawing and the labels found before:
/// tick labels beside an axis, then the axis title beside them.
const LABEL_ROUNDS: usize = 2;

/// A block longer than this (characters) is text, not a label.
const MAX_LABEL_CHARS: usize = 60;

/// A path or picture covering this share of the page is its background or frame, not a figure.
const BACKGROUND_SHARE: f64 = 0.5;

/// How far a figure may be from its caption, in caption font sizes, and in points at least and
/// at most.
const CAPTION_GAP: (f64, f64, f64) = (3.0, 12.0, 48.0);

/// How far (points) a table's grid may reach past the text of its cells.
const TABLE_PADDING: f64 = 8.0;

/// Comparisons one page may spend finding its figures; past it the page has none, and its
/// drawings and labels stay where the export puts them otherwise.
const MAX_FIGURE_WORK: usize = 1 << 22;

/// A part a figure is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Drawing(usize),
    Image(usize),
}

/// Parts that lie close together, the labels next to them, and the box of all of them.
struct Cluster {
    rect: [f64; 4],
    parts: Vec<Part>,
    labels: Vec<usize>,
}

/// The figures of a page with these `captions`: each figure caption takes the group of drawings,
/// images and labels closest to it, above or below it within [`CAPTION_GAP`] and overlapping it
/// across, nearest first. Paths that reach into one of the page's `tables`, backgrounds and
/// frames are not part of any, nor are the blocks running or `consumed` by a table.
pub(super) fn figures(page: &Page, captions: &[Caption], running: &[bool], tables: &[Table], consumed: &[bool]) -> Vec<Figure> {
    if !captions.iter().any(|c| c.kind == CaptionKind::Figure) {
        return Vec::new();
    }
    let mut budget = MAX_FIGURE_WORK;
    // Paths that reach into a table are its grid, not a figure.
    let table_rects: Vec<[f64; 4]> = tables.iter().map(|t| t.rect).collect();
    let Some(mut clusters) = clusters(page, &table_rects, &mut budget) else { return Vec::new() };
    let caption_blocks: HashSet<usize> = captions.iter().map(|c| c.block).collect();
    let flag = |v: &[bool], i: usize| v.get(i).copied().unwrap_or(false);
    let label_like: Vec<usize> = (0..page.blocks.len())
        .filter(|&bi| {
            page.blocks.get(bi).is_some_and(|b| {
                let chars = b.text.trim().chars().count();
                b.rotation.abs() <= 0.1
                    && (1..=MAX_LABEL_CHARS).contains(&chars)
                    && !flag(running, bi)
                    && !flag(consumed, bi)
                    && !caption_blocks.contains(&bi)
            })
        })
        .collect();
    let mut taken = vec![false; page.blocks.len()];
    for c in &mut clusters {
        for _ in 0..LABEL_ROUNDS {
            let around = c.rect;
            for &bi in &label_like {
                let Some(spent) = budget.checked_sub(1) else { return Vec::new() };
                budget = spent;
                let Some(b) = page.blocks.get(bi) else { continue };
                if !flag(&taken, bi) && touches(&around, &b.rect, LABEL_GAP) {
                    c.rect = union(c.rect, b.rect);
                    c.labels.push(bi);
                    if let Some(t) = taken.get_mut(bi) {
                        *t = true;
                    }
                }
            }
        }
    }

    let boxes: Vec<[f64; 4]> = clusters.iter().map(|c| c.rect).collect();
    let Some(pairs) = pair_captions(page, captions, |_, c| c.kind == CaptionKind::Figure, &boxes, true, &mut budget) else { return Vec::new() };
    let mut out = Vec::new();
    for (ci, k) in pairs {
        let Some(c) = clusters.get_mut(k) else { continue };
        let (mut drawings, mut images) = (Vec::new(), Vec::new());
        c.parts.sort_unstable();
        for p in &c.parts {
            match *p {
                Part::Drawing(i) => drawings.push(i),
                Part::Image(i) => images.push(i),
            }
        }
        c.labels.sort_unstable();
        out.push(Figure { caption: ci, rect: c.rect, drawings, images, labels: std::mem::take(&mut c.labels) });
    }
    out
}

/// Pairs the page's `captions` that are `wanted` with `boxes`: every caption in reach of a box (above or
/// below it within [`CAPTION_GAP`], overlapping it across) takes the nearest free one, and on a tie
/// the box above it when `above_first` (a figure over its caption), below it otherwise (a table
/// under its caption). Each pair is (caption, box), in caption order; `None` when that takes more
/// than `budget` comparisons.
pub(super) fn pair_captions(
    page: &Page,
    captions: &[Caption],
    wanted: impl Fn(usize, &Caption) -> bool,
    boxes: &[[f64; 4]],
    above_first: bool,
    budget: &mut usize,
) -> Option<Vec<(usize, usize)>> {
    let mut pairs: Vec<(f64, bool, usize, usize)> = Vec::new();
    for (ci, cap) in captions.iter().enumerate().filter(|(ci, c)| wanted(*ci, c)) {
        let Some(b) = page.blocks.get(cap.block).filter(|b| b.rotation.abs() <= 0.1) else { continue };
        let reach = (b.size * CAPTION_GAP.0).clamp(CAPTION_GAP.1, CAPTION_GAP.2);
        for (k, r) in boxes.iter().enumerate() {
            *budget = budget.checked_sub(1)?;
            if r[2].min(b.rect[2]) <= r[0].max(b.rect[0]) {
                continue;
            }
            let (above, below) = (r[1] - b.rect[3], b.rect[1] - r[3]);
            if (-2.0..=reach).contains(&above) {
                pairs.push((above.max(0.0), !above_first, ci, k));
            } else if (-2.0..=reach).contains(&below) {
                pairs.push((below.max(0.0), above_first, ci, k));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let (mut caption_done, mut box_done) = (vec![false; captions.len()], vec![false; boxes.len()]);
    let mut out = Vec::new();
    for (_, _, ci, k) in pairs {
        let (Some(c), Some(b)) = (caption_done.get(ci), box_done.get(k)) else { continue };
        if *c || *b {
            continue;
        }
        for (v, i) in [(&mut caption_done, ci), (&mut box_done, k)] {
            if let Some(d) = v.get_mut(i) {
                *d = true;
            }
        }
        out.push((ci, k));
    }
    out.sort_unstable();
    Some(out)
}

/// The page's paths and pictures, but for backgrounds, frames and what reaches into `tables`, in
/// groups that lie within [`CLUSTER_GAP`] of each other, or `None` when that takes more than
/// `budget` comparisons.
fn clusters(page: &Page, tables: &[[f64; 4]], budget: &mut usize) -> Option<Vec<Cluster>> {
    let page_area = page.width * page.height;
    let small = |r: &[f64; 4]| r.iter().all(|v| v.is_finite()) && (r[2] - r[0]) * (r[3] - r[1]) < page_area * BACKGROUND_SHARE;
    let parts = page
        .drawings
        .iter()
        .enumerate()
        .map(|(i, d)| (Part::Drawing(i), d.rect))
        .chain(page.images.iter().enumerate().map(|(i, im)| (Part::Image(i), im.rect)))
        .filter(|(_, r)| small(r));
    let mut out: Vec<Cluster> = Vec::new();
    for (part, rect) in parts {
        *budget = budget.checked_sub(tables.len())?;
        if tables.iter().any(|t| touches(&rect, t, TABLE_PADDING)) {
            continue;
        }
        let mut c = Cluster { rect, parts: vec![part], labels: Vec::new() };
        // Merging can bring the group near another, so look again until none is.
        loop {
            *budget = budget.checked_sub(out.len().max(1))?;
            let Some(k) = out.iter().position(|o| touches(&o.rect, &c.rect, CLUSTER_GAP)) else { break };
            let o = out.swap_remove(k);
            c.rect = union(c.rect, o.rect);
            c.parts.extend(o.parts);
        }
        out.push(c);
    }
    Some(out)
}

/// Whether boxes `a` and `b` overlap or are at most `gap` apart.
fn touches(a: &[f64; 4], b: &[f64; 4], gap: f64) -> bool {
    a[0] - gap <= b[2] && b[0] <= a[2] + gap && a[1] - gap <= b[3] && b[1] <= a[3] + gap
}

fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}
