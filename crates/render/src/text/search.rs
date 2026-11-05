//! Literal search over normalized UTF-8 page text, retaining original glyph owners.
use super::{PageText, fold_case, push_folded};
use std::ops::Range;

struct GlyphSpan {
    end: usize,
    glyph: usize,
}

pub(super) fn find(page: &PageText, query: &str, sensitive: bool, whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let normalized_query = if sensitive { query.to_owned() } else { fold_case(query) };
    let needle = normalized_query.split_whitespace().collect::<Vec<_>>().join(" ");
    if needle.is_empty() {
        return Vec::new();
    }

    let (haystack, owners) = normalized_text(page, sensitive);
    if needle.len() > haystack.len() {
        return Vec::new();
    }
    // memmem uses SIMD byte scans for literal needles. UTF-8 needles can only match at scalar
    // boundaries, while this compact span table maps byte hits back to PDF glyphs.
    let finder = memchr::memmem::Finder::new(needle.as_bytes());
    let mut cursor = 0;
    let mut first_span = 0;
    let mut last_span = 0;
    let mut hits = Vec::new();
    while cursor <= haystack.len().saturating_sub(needle.len()) {
        let Some(relative) = finder.find(&haystack.as_bytes()[cursor..]) else { break };
        let start = cursor + relative;
        let end = start + needle.len();
        let bounded = !whole_words || (!word_before(&haystack, start) && !word_after(&haystack, end));
        if bounded {
            if let (Some(first), Some(last)) = (owner_at(&owners, start, &mut first_span), owner_at(&owners, end.saturating_sub(1), &mut last_span)) {
                hits.push(first..last.saturating_add(1));
                if hits.len() == limit {
                    break;
                }
            }
            // As before, accepted literal hits consume their full span.
            cursor = end;
        } else {
            // Whole-word rejection must still allow an overlapping later candidate.
            cursor = start.saturating_add(haystack[start..].chars().next().map_or(1, char::len_utf8));
        }
    }
    hits
}

fn normalized_text(page: &PageText, sensitive: bool) -> (String, Vec<GlyphSpan>) {
    let mut text = String::new();
    let mut owners = Vec::with_capacity(page.glyphs.len());
    let mut folded = String::with_capacity(4);
    for (i, glyph) in page.glyphs.iter().enumerate() {
        let line_break = i.checked_sub(1).and_then(|previous| page.line_of.get(previous)).zip(page.line_of.get(i)).is_some_and(|(a, b)| a != b);
        let gap = page.space_before.get(i).copied().unwrap_or(false) || line_break;
        let start = text.len();
        if i > 0 && gap && !text.ends_with(' ') {
            text.push(' ');
        }
        for original in glyph.text.chars() {
            folded.clear();
            if sensitive {
                folded.push(original);
            } else {
                push_folded(original, &mut folded);
            }
            for c in folded.chars() {
                let c = if c.is_whitespace() { ' ' } else { c };
                if c != ' ' || !text.ends_with(' ') {
                    text.push(c);
                }
            }
        }
        if text.len() > start {
            owners.push(GlyphSpan { end: text.len(), glyph: i });
        }
    }
    (text, owners)
}

fn owner_at(owners: &[GlyphSpan], byte: usize, cursor: &mut usize) -> Option<usize> {
    while owners.get(*cursor).is_some_and(|span| span.end <= byte) {
        *cursor += 1;
    }
    let span = owners.get(*cursor)?;
    (byte < span.end).then_some(span.glyph)
}

fn word_before(text: &str, byte: usize) -> bool {
    text.get(..byte).and_then(|prefix| prefix.chars().next_back()).is_some_and(char::is_alphanumeric)
}

fn word_after(text: &str, byte: usize) -> bool {
    text.get(byte..).and_then(|suffix| suffix.chars().next()).is_some_and(char::is_alphanumeric)
}
