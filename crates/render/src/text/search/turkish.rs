//! Explicit Turkish matching. Normalized scalars own spans of the original glyphs;
//! page text, selection text and geometry are never normalized in place.
use std::collections::{HashMap, VecDeque};
use std::ops::Range;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::{canonical_combining_class, decompose_canonical, is_combining_mark};

use super::super::PageText;
use super::advance;

type Owned = (char, Range<usize>);

fn merge(into: &mut Range<usize>, span: &Range<usize>) {
    if span.start < span.end {
        if into.start >= into.end {
            *into = span.clone();
        } else {
            into.start = into.start.min(span.start);
            into.end = into.end.max(span.end);
        }
    }
}

/// Canonical normalization preserves the order of equal decomposed scalars. Queues
/// associate those scalars with their original owners even when unequal marks reorder.
/// Consuming the input releases it before building the output; storage stays linear
/// in the already bounded extracted text and its canonical decomposition.
fn canonical(input: Vec<Owned>, compose: bool) -> Vec<Owned> {
    let mut text = String::new();
    let mut owners: HashMap<char, VecDeque<Range<usize>>> = HashMap::new();
    for (c, span) in input {
        text.push(c);
        decompose_canonical(c, |part| owners.entry(part).or_default().push_back(span.clone()));
    }
    let normalized: Box<dyn Iterator<Item = char> + '_> = if compose { Box::new(text.nfc()) } else { Box::new(text.nfd()) };
    normalized
        .map(|c| {
            let mut span = 0..0;
            decompose_canonical(c, |part| {
                if let Some(owner) = owners.get_mut(&part).and_then(VecDeque::pop_front) {
                    merge(&mut span, &owner);
                }
            });
            (c, span)
        })
        .collect()
}

fn normalize(input: Vec<Owned>, sensitive: bool) -> Vec<Owned> {
    if sensitive {
        return canonical(input, true);
    }
    let decomposed = canonical(input, false);
    let mut folded: Vec<Owned> = Vec::with_capacity(decomposed.len());
    let mut after_i = None;
    for (c, span) in decomposed {
        if c == 'I' {
            after_i = Some(folded.len());
            folded.push(('ı', span));
            continue;
        }
        // SpecialCasing tr Before_Dot/After_I: an intervening starter or class-230
        // mark blocks the context. Retain the removed dot's glyph in the result span.
        if c == '\u{307}'
            && let Some(i) = after_i.take()
            && let Some(base) = folded.get_mut(i)
        {
            base.0 = 'i';
            merge(&mut base.1, &span);
            continue;
        }
        if matches!(canonical_combining_class(c), 0 | 230) {
            after_i = None;
        }
        if matches!(c, 'Σ' | 'σ' | 'ς') {
            folded.push(('σ', span));
        } else {
            folded.extend(c.to_lowercase().map(|lower| (lower, span.clone())));
        }
    }
    canonical(folded, true)
}

fn page_chars(page: &PageText) -> Vec<Owned> {
    let mut chars: Vec<Owned> = Vec::new();
    for (i, glyph) in page.glyphs.iter().enumerate() {
        let line_break = i.checked_sub(1).and_then(|p| page.line_of.get(p)).zip(page.line_of.get(i)).is_some_and(|(a, b)| a != b);
        if (line_break || page.space_before.get(i).copied().unwrap_or(false)) && chars.last().is_some_and(|c| c.0 != ' ') {
            chars.push((' ', 0..0));
        }
        let Some(end) = i.checked_add(1) else { break };
        for c in glyph.text.chars() {
            let c = if c.is_whitespace() { ' ' } else { c };
            if c != ' ' || chars.last().is_none_or(|last| last.0 != ' ') {
                chars.push((c, i..end));
            }
        }
    }
    chars
}

pub(super) fn find(page: &PageText, query: &str, sensitive: bool, whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
    let needle: Vec<char> = normalize(query.chars().map(|c| (c, 0..0)).collect(), sensitive).into_iter().map(|c| c.0).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let chars = normalize(page_chars(page), sensitive);
    if needle.len() > chars.len() {
        return Vec::new();
    }
    let mut prefix = vec![0; needle.len()];
    let mut matched = 0;
    for (i, &c) in needle.iter().enumerate().skip(1) {
        matched = advance(&needle, &prefix, matched, c);
        if let Some(slot) = prefix.get_mut(i) {
            *slot = matched;
        }
    }
    let word = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|c| c.0.is_alphanumeric() || is_combining_mark(c.0));
    let mut hits = Vec::new();
    matched = 0;
    for (i, (c, _)) in chars.iter().enumerate() {
        matched = advance(&needle, &prefix, matched, *c);
        if matched != needle.len() {
            continue;
        }
        let end = i.saturating_add(1);
        let start = end.saturating_sub(needle.len());
        if !whole_words || (!word(start.checked_sub(1)) && !word(Some(end))) {
            // Canonical reordering can make owners non-monotonic. Accepted literal hits
            // do not overlap, so scanning their spans is linear over the whole search.
            let mut span = 0..0;
            for (_, owner) in chars.get(start..end).unwrap_or_default() {
                merge(&mut span, owner);
            }
            if span.start < span.end {
                hits.push(span);
                if hits.len() == limit {
                    break;
                }
            }
            matched = 0;
        } else {
            matched = prefix.last().copied().unwrap_or_default();
        }
    }
    hits
}
