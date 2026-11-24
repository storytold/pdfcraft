//! Comments the Search panel finds when Include comments is on.
//!
//! A row is a comment or one of its replies. A reply match finds the thread it belongs to, the
//! same card the Comments panel shows. Page text stays with the find bar's matches.

use std::collections::HashMap;

use pdfcraft_render::{Annotation, DocInfo};

use crate::panels::comments_panel::kind_label;

/// A comment thread whose comment or reply matches the query.
#[derive(Clone, Debug, PartialEq)]
pub struct CommentHit {
    /// The thread's own comment, as the Comments panel selects it.
    pub page: usize,
    pub index: usize,
    /// The comment's rectangle as the page shows it, where the view goes.
    pub rect: [f32; 4],
    /// The thread's type, as the panel names it (an English key).
    pub kind: String,
    /// The matched field, at most [`EXCERPT_BYTES`] of it.
    pub excerpt: String,
}

impl CommentHit {
    pub fn key(&self) -> (usize, usize) {
        (self.page, self.index)
    }
}

/// The threads matching `query`, in page and annotation order. A thread is found by its comment's
/// text, then its author, then its type, the fields the Comments panel searches; a reply that
/// matches a better field than the thread's comment supplies the excerpt.
pub fn comment_hits(info: &DocInfo, query: &str, case_sensitive: bool, whole_words: bool) -> Vec<CommentHit> {
    let needle = normalize(query, case_sensitive);
    if needle.is_empty() {
        return Vec::new();
    }
    let tops = pdfcraft_render::thread_tops(&info.annotations);
    let roots: HashMap<(usize, usize), &Annotation> = info.annotations.iter().filter(|a| a.is_comment()).map(|a| ((a.page, a.index), a)).collect();
    let mut best: HashMap<(usize, usize), (u8, CommentHit)> = HashMap::new();
    for annotation in &info.annotations {
        // Group subordinates (`/RT /Group`) are part of the primary: the Comments panel skips them,
        // so their own text, author or type must not find a card that does not show it.
        if annotation.subtype == "Popup" || annotation.is_group_member() {
            continue;
        }
        let key = if annotation.is_comment() {
            (annotation.page, annotation.index)
        } else {
            let Some(top) = tops.get(&(annotation.page, annotation.index)).copied().or(annotation.parent) else { continue };
            top
        };
        let Some(root) = roots.get(&key) else { continue };
        let Some(page) = info.pages.get(root.page) else { continue };
        let rect = root.shown_rect(page.rotation);
        if !rect.iter().all(|value| value.is_finite()) {
            continue;
        }
        let Some((rank, excerpt)) = matched_excerpt(annotation, &needle, case_sensitive, whole_words) else { continue };
        if best.get(&key).is_some_and(|(old, _)| *old >= rank) {
            continue;
        }
        let hit = CommentHit { page: root.page, index: root.index, rect, kind: kind_label(root).to_string(), excerpt };
        best.insert(key, (rank, hit));
    }
    let mut hits: Vec<CommentHit> = best.into_values().map(|(_, hit)| hit).collect();
    hits.sort_by_key(CommentHit::key);
    hits
}

/// The best matching field of one comment or reply: its text (rank 3), author (2) or type (1).
fn matched_excerpt(annotation: &Annotation, needle: &str, case_sensitive: bool, whole_words: bool) -> Option<(u8, String)> {
    let body = annotation.contents.as_deref().unwrap_or("");
    let author = annotation.author.as_deref().unwrap_or("");
    for (rank, field) in [(3u8, body), (2, author), (1, kind_label(annotation))] {
        if !field.is_empty() && text_has(&normalize(field, case_sensitive), needle, whole_words) {
            return Some((rank, snippet(field, needle, case_sensitive, whole_words)));
        }
    }
    None
}

fn normalize(text: &str, case_sensitive: bool) -> String {
    let folded = if case_sensitive { text.to_owned() } else { pdfcraft_render::text::fold_case(text) };
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `needle` occurs in `haystack`. Both are already normalised. Whole words require a
/// boundary on each side, the same alphanumeric test the page search uses.
fn text_has(haystack: &str, needle: &str, whole_words: bool) -> bool {
    let hay: Vec<char> = haystack.chars().collect();
    let needle: Vec<char> = needle.chars().collect();
    match_at(&hay, &needle, whole_words).is_some()
}

/// Where the first match of `needle` starts in `hay`, in characters.
fn match_at(hay: &[char], needle: &[char], whole_words: bool) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let last = hay.len().checked_sub(needle.len())?;
    (0..=last).find(|&start| {
        let end = start.saturating_add(needle.len());
        if hay.get(start..end) != Some(needle) {
            return false;
        }
        let before = start.checked_sub(1).and_then(|i| hay.get(i));
        let after = hay.get(end);
        !whole_words || (!before.is_some_and(|c| c.is_alphanumeric()) && !after.is_some_and(|c| c.is_alphanumeric()))
    })
}

/// At most this many bytes of a comment go into the results.
pub const EXCERPT_BYTES: usize = 160;
/// Characters kept before a match that lies past the start of a long comment.
const EXCERPT_LEAD: usize = 20;

/// The part of a comment the results show, on character boundaries: its start, or, when the match
/// ends past the first [`EXCERPT_BYTES`], a window that opens shortly before it. The match keeps its
/// room: lead context is only what still fits beside the whole match.
fn snippet(text: &str, needle: &str, case_sensitive: bool, whole_words: bool) -> String {
    let flat: Vec<char> = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    let bytes = |range: std::ops::Range<usize>| flat.get(range).map_or(0, |chars| chars.iter().map(|c| c.len_utf8()).sum::<usize>());
    let begin = match_span(&flat, needle, case_sensitive, whole_words).filter(|&(_, end)| bytes(0..end) > EXCERPT_BYTES).map_or(0, |(start, end)| {
        let room = EXCERPT_BYTES.saturating_sub("…".len());
        (0..=EXCERPT_LEAD.min(start)).rev().map(|lead| start.saturating_sub(lead)).find(|&from| bytes(from..end) <= room).unwrap_or(start)
    });
    let mut out = String::from(if begin > 0 { "…" } else { "" });
    for &c in flat.iter().skip(begin) {
        if out.len().saturating_add(c.len_utf8()) > EXCERPT_BYTES {
            break;
        }
        out.push(c);
    }
    out
}

/// Where `query` matches in `excerpt`, as byte offsets, so the result row can highlight it.
pub fn excerpt_match(excerpt: &str, query: &str, case_sensitive: bool, whole_words: bool) -> Option<std::ops::Range<usize>> {
    let needle = normalize(query, case_sensitive);
    let chars: Vec<(usize, char)> = excerpt.char_indices().collect();
    let flat: Vec<char> = chars.iter().map(|&(_, c)| c).collect();
    let (start, end) = match_span(&flat, &needle, case_sensitive, whole_words)?;
    let from = chars.get(start)?.0;
    let to = chars.get(end).map_or(excerpt.len(), |&(at, _)| at);
    Some(from..to)
}

/// The first match in `flat` as a range of its characters. `needle` is normalised, so each
/// character is folded on its own and mapped back to the one it came from.
fn match_span(flat: &[char], needle: &str, case_sensitive: bool, whole_words: bool) -> Option<(usize, usize)> {
    let mut folded = Vec::with_capacity(flat.len());
    let mut origin = Vec::with_capacity(flat.len());
    for (at, &c) in flat.iter().enumerate() {
        let one = if case_sensitive { c.to_string() } else { pdfcraft_render::text::fold_case(&c.to_string()) };
        for f in one.chars() {
            folded.push(f);
            origin.push(at);
        }
    }
    let needle: Vec<char> = needle.chars().collect();
    let start = match_at(&folded, &needle, whole_words)?;
    let last = start.saturating_add(needle.len()).checked_sub(1)?;
    Some((*origin.get(start)?, origin.get(last)?.saturating_add(1)))
}
