//! The headings the document's outline names, and the "Figure 3" and "Table 2-1" mentions in its
//! text with the captions they point to.

use std::collections::HashMap;

use super::PageStructure;
use super::figures::{CaptionKind, mention};
use crate::Page;

/// A bookmark of the document's outline, where it goes on the exported pages.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineEntry {
    pub title: String,
    /// How deep it is: 0 for a top-level bookmark, 1 for its children, and so on.
    pub level: usize,
    /// The 0-based page it goes to.
    pub page: usize,
    /// The height on that page (displayed page, y up) it puts at the top of the window, when it
    /// says.
    pub top: Option<f64>,
}

/// A heading the outline names: the bookmark, and the block on its page set as its title.
#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    /// The bookmark in the outline passed to [`structure_with_outline`](super::structure_with_outline).
    pub entry: usize,
    /// Its depth in the outline ([`OutlineEntry::level`]).
    pub level: usize,
    /// Its block in [`Page::blocks`]; `None` when no block on its page has its title.
    pub block: Option<usize>,
}

/// A mention of a figure or table in a block's text: "see Figure 3".
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    /// Its block in [`Page::blocks`].
    pub block: usize,
    /// Where it is in the block's text, in bytes: the label and the number.
    pub range: std::ops::Range<usize>,
    pub kind: CaptionKind,
    /// The number as printed.
    pub number: String,
    /// The first caption of the document with that kind and number: its page and its index in
    /// that page's [`PageStructure::captions`]; `None` when there is none.
    pub target: Option<(usize, usize)>,
}

/// Text longer than this (characters) is not a heading.
const MAX_HEADING_CHARS: usize = 200;

/// A page has at most this many references.
const MAX_REFERENCES: usize = 4096;

/// A heading's text compared with an outline title: lowercase, its spaces collapsed, without the
/// punctuation that ends it.
fn key(text: &str) -> Option<String> {
    let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    let joined = words.join(" ");
    let trimmed = joined.trim_end_matches(['.', ':', ';']).trim_end();
    (!trimmed.is_empty() && trimmed.chars().count() <= MAX_HEADING_CHARS).then(|| trimmed.to_owned())
}

/// `key` without the section number it starts with ("3.2", "4.", "a.1"), when it has one and
/// text after it.
fn without_number(key: &str) -> Option<&str> {
    let (first, rest) = key.split_once(' ')?;
    let mut chars = first.chars();
    let numeric = |s: &str| s.chars().all(|c| c.is_ascii_digit() || c == '.') && s.chars().any(|c| c.is_ascii_digit());
    let lettered = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.as_str().starts_with('.')
        && (chars.as_str().len() == 1 || numeric(chars.as_str()));
    (numeric(first) || lettered).then_some(rest).filter(|r| !r.is_empty())
}

/// Each page's headings ([`PageStructure::headings`]), in outline order: every bookmark goes to
/// the block on its page whose text is its title (letter case, spacing, a section number before
/// either and the punctuation ending them aside), not running and not taken by an earlier
/// bookmark; of several, the one nearest the height it goes to, or else the topmost.
pub(super) fn headings(pages: &[Page], structures: &[PageStructure], outline: &[OutlineEntry]) -> Vec<Vec<Heading>> {
    let mut out: Vec<Vec<Heading>> = vec![Vec::new(); pages.len()];
    // Per page with a bookmark, its blocks by key (with and without a section number).
    let mut keys: HashMap<usize, HashMap<String, Vec<usize>>> = HashMap::new();
    let mut taken: HashMap<usize, Vec<bool>> = HashMap::new();
    for (entry, e) in outline.iter().enumerate() {
        let (Some(page), Some(s), Some(slot)) = (pages.get(e.page), structures.get(e.page), out.get_mut(e.page)) else { continue };
        let by_key = keys.entry(e.page).or_insert_with(|| {
            let mut m: HashMap<String, Vec<usize>> = HashMap::new();
            for (bi, b) in page.blocks.iter().enumerate() {
                if s.running.get(bi).copied().unwrap_or(false) {
                    continue;
                }
                let Some(k) = key(&b.text) else { continue };
                if let Some(bare) = without_number(&k) {
                    m.entry(bare.to_owned()).or_default().push(bi);
                }
                m.entry(k).or_default().push(bi);
            }
            m
        });
        let used = taken.entry(e.page).or_insert_with(|| vec![false; page.blocks.len()]);
        let title = key(&e.title);
        let candidates = title.iter().flat_map(|t| [Some(t.as_str()), without_number(t)]).flatten().filter_map(|t| by_key.get(t)).flatten();
        let height = |bi: &usize| page.blocks.get(*bi).map_or(f64::NEG_INFINITY, |b| b.rect[3]);
        let block = candidates.copied().filter(|bi| !used.get(*bi).copied().unwrap_or(true)).min_by(|a, b| match e.top {
            Some(top) => (height(a) - top).abs().total_cmp(&(height(b) - top).abs()).then(a.cmp(b)),
            None => height(b).total_cmp(&height(a)).then(a.cmp(b)),
        });
        if let Some(u) = block.and_then(|bi| used.get_mut(bi)) {
            *u = true;
        }
        slot.push(Heading { entry, level: e.level, block });
    }
    out
}

/// Each page's references ([`PageStructure::references`]): the mentions of a figure or table
/// in its blocks that are not running, each pointing to the first caption with its kind and
/// number. A caption's own label is not a mention.
pub(super) fn references(pages: &[Page], structures: &[PageStructure]) -> Vec<Vec<Reference>> {
    let mut first: HashMap<(CaptionKind, &str), (usize, usize)> = HashMap::new();
    for (pi, s) in structures.iter().enumerate() {
        for (ci, c) in s.captions.iter().enumerate() {
            first.entry((c.kind, c.number.as_str())).or_insert((pi, ci));
        }
    }
    pages
        .iter()
        .zip(structures)
        .map(|(page, s)| {
            let mut out = Vec::new();
            for (bi, b) in page.blocks.iter().enumerate() {
                if s.running.get(bi).copied().unwrap_or(false) {
                    continue;
                }
                let label_end = s.captions.iter().any(|c| c.block == bi).then(|| b.text.len() - b.text.trim_start().len());
                let mut after_word = true;
                let mut skip_to = 0;
                for (i, c) in b.text.char_indices() {
                    if out.len() >= MAX_REFERENCES {
                        break;
                    }
                    let starts = after_word && i >= skip_to;
                    after_word = !c.is_alphanumeric();
                    if !starts || Some(i) == label_end {
                        continue;
                    }
                    let Some((kind, len, number)) = b.text.get(i..).and_then(mention) else { continue };
                    let target = first.get(&(kind, number)).copied();
                    out.push(Reference { block: bi, range: i..i + len, kind, number: number.to_owned(), target });
                    skip_to = i + len;
                }
            }
            out
        })
        .collect()
}
