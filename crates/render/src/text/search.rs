//! Literal search over normalized UTF-8 page text, retaining original glyph owners.
use super::{PageText, fold_case};
use std::ops::Range;

struct GlyphSpan {
    end: usize,
    glyph: usize,
}

pub struct PreparedSearchText {
    haystack: String,
    owners: Vec<GlyphSpan>,
    sensitive: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub glyphs: Range<usize>,
    pub byte_start: usize,
}

#[derive(Clone)]
pub struct SearchPattern {
    needle: String,
    finder: memchr::memmem::Finder<'static>,
    sensitive: bool,
    whole_words: bool,
}

impl SearchPattern {
    pub fn new(query: &str, sensitive: bool, whole_words: bool) -> Self {
        let normalized_query = if sensitive { query.to_owned() } else { fold_case(query) };
        let needle = normalized_query.split_whitespace().collect::<Vec<_>>().join(" ");
        let finder = memchr::memmem::Finder::new(needle.as_bytes()).into_owned();
        Self { needle, finder, sensitive, whole_words }
    }

    pub fn find(&self, page: &PageText) -> Vec<Range<usize>> {
        self.find_limited(page, usize::MAX)
    }

    pub fn is_case_sensitive(&self) -> bool {
        self.sensitive
    }

    pub fn normalized_needle(&self) -> &str {
        &self.needle
    }

    pub fn find_limited(&self, page: &PageText, limit: usize) -> Vec<Range<usize>> {
        if limit == 0 || self.needle.is_empty() {
            return Vec::new();
        }
        let prepared = self.prepare_page(page);
        self.find_prepared_limited(&prepared, limit)
    }

    /// Prepare one page once when many query edits will scan it.
    pub fn prepare_page(&self, page: &PageText) -> PreparedSearchText {
        let (haystack, owners) = normalized_text(page, self.sensitive);
        PreparedSearchText { haystack, owners, sensitive: self.sensitive }
    }

    /// Match a page prepared by a pattern with the same case-sensitivity setting.
    pub fn find_prepared(&self, prepared: &PreparedSearchText) -> Vec<Range<usize>> {
        self.find_prepared_limited(prepared, usize::MAX)
    }

    pub fn find_prepared_limited(&self, prepared: &PreparedSearchText, limit: usize) -> Vec<Range<usize>> {
        if limit == 0 || self.needle.is_empty() || prepared.sensitive != self.sensitive {
            return Vec::new();
        }
        find_ranges_in_prepared(prepared, &self.needle, &self.finder, self.whole_words, limit)
    }

    pub fn find_prepared_hits(&self, prepared: &PreparedSearchText) -> Vec<SearchHit> {
        if self.needle.is_empty() || prepared.sensitive != self.sensitive {
            return Vec::new();
        }
        find_in_prepared(prepared, &self.needle, &self.finder, self.whole_words, usize::MAX)
    }

    /// Refine exact hit starts from a completed shorter literal query. Whole-word searches
    /// intentionally use a full scan because the shorter query's end-boundary rule can hide
    /// prefixes that become valid when extended.
    pub fn refine_prepared_hits(&self, prepared: &PreparedSearchText, starts: &[usize]) -> Vec<SearchHit> {
        if self.needle.is_empty() || prepared.sensitive != self.sensitive || self.whole_words {
            return Vec::new();
        }
        let mut owners = 0;
        starts
            .iter()
            .copied()
            .filter_map(|start| {
                let end = start.checked_add(self.needle.len())?;
                (prepared.haystack.get(start..end)? == self.needle).then_some(())?;
                let first = owner_at(&prepared.owners, start, &mut owners)?;
                let last = owner_at(&prepared.owners, end.saturating_sub(1), &mut owners)?;
                Some(SearchHit { glyphs: first..last.saturating_add(1), byte_start: start })
            })
            .collect()
    }
}

pub(super) fn find(page: &PageText, query: &str, sensitive: bool, whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    SearchPattern::new(query, sensitive, whole_words).find_limited(page, limit)
}

fn find_in_prepared(
    prepared: &PreparedSearchText,
    needle: &str,
    finder: &memchr::memmem::Finder<'_>,
    whole_words: bool,
    limit: usize,
) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    let mut count = 0;
    visit_prepared(prepared, needle, finder, whole_words, limit, |glyphs, byte_start| {
        hits.push(SearchHit { glyphs, byte_start });
        count += 1;
        count < limit
    });
    hits
}

fn find_ranges_in_prepared(
    prepared: &PreparedSearchText,
    needle: &str,
    finder: &memchr::memmem::Finder<'_>,
    whole_words: bool,
    limit: usize,
) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut count = 0;
    visit_prepared(prepared, needle, finder, whole_words, limit, |glyphs, _| {
        ranges.push(glyphs);
        count += 1;
        count < limit
    });
    ranges
}

fn visit_prepared(
    prepared: &PreparedSearchText,
    needle: &str,
    finder: &memchr::memmem::Finder<'_>,
    whole_words: bool,
    limit: usize,
    mut visit: impl FnMut(Range<usize>, usize) -> bool,
) {
    if limit == 0 {
        return;
    }
    let (haystack, owners) = (prepared.haystack.as_str(), prepared.owners.as_slice());
    if needle.len() > haystack.len() {
        return;
    }
    // memmem uses SIMD byte scans for literal needles. UTF-8 needles can only match at scalar
    // boundaries, while this compact span table maps byte hits back to PDF glyphs.
    let mut cursor = 0;
    let mut first_span = 0;
    let mut last_span = 0;
    while cursor <= haystack.len().saturating_sub(needle.len()) {
        let Some(relative) = finder.find(&haystack.as_bytes()[cursor..]) else { break };
        let start = cursor + relative;
        let end = start + needle.len();
        let bounded = !whole_words || (!word_before(haystack, start) && !word_after(haystack, end));
        if bounded {
            if let (Some(first), Some(last)) = (owner_at(owners, start, &mut first_span), owner_at(owners, end.saturating_sub(1), &mut last_span)) {
                if !visit(first..last.saturating_add(1), start) {
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
}

fn normalized_text(page: &PageText, sensitive: bool) -> (String, Vec<GlyphSpan>) {
    let mut text = String::new();
    let mut owners = Vec::with_capacity(page.glyphs.len());
    for (i, glyph) in page.glyphs.iter().enumerate() {
        let line_break = i.checked_sub(1).and_then(|previous| page.line_of.get(previous)).zip(page.line_of.get(i)).is_some_and(|(a, b)| a != b);
        let gap = page.space_before.get(i).copied().unwrap_or(false) || line_break;
        let start = text.len();
        if i > 0 && gap && !text.ends_with(' ') {
            text.push(' ');
        }
        for original in glyph.text.chars() {
            if sensitive {
                push_search_char(original, &mut text);
            } else {
                match original {
                    'I' | 'i' | '\u{130}' | '\u{131}' => push_search_char('i', &mut text),
                    'Σ' | 'σ' | 'ς' => push_search_char('σ', &mut text),
                    _ => {
                        for c in original.to_lowercase() {
                            push_search_char(c, &mut text);
                        }
                    }
                }
            }
        }
        if text.len() > start {
            owners.push(GlyphSpan { end: text.len(), glyph: i });
        }
    }
    (text, owners)
}

fn push_search_char(c: char, text: &mut String) {
    let c = if c.is_whitespace() { ' ' } else { c };
    if c != ' ' || !text.ends_with(' ') {
        text.push(c);
    }
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
