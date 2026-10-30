//! Comments the Find bar searches when Include comments is on.
//!
//! A row is a comment or one of its replies. A reply match selects the thread it belongs to,
//! the same card the Comments panel would show. Text hits stay in [`super::find`].

use std::collections::HashMap;
use std::ops::Range;

use pdfcraft_render::{Annotation, DocInfo, PageInfo};

use crate::panels::comments_panel::kind_label;

/// One find hit. Text is a glyph run; a comment hit is the thread it belongs to.
#[derive(Clone, Debug, PartialEq)]
pub enum FindHit {
    /// Glyphs on a page.
    Text { page: usize, range: Range<usize> },
    /// A comment thread. `top` is its view-space top, used to scroll the way a text hit does.
    Comment { page: usize, index: usize, top: f32, rect: [f32; 4], kind: String, excerpt: String },
}

impl FindHit {
    /// A page-text hit, for tests and callers that used to store `(page, range)`.
    pub fn text(page: usize, range: Range<usize>) -> Self {
        Self::Text { page, range }
    }

    pub fn page(&self) -> usize {
        match self {
            Self::Text { page, .. } | Self::Comment { page, .. } => *page,
        }
    }

    pub fn is_comment(&self) -> bool {
        matches!(self, Self::Comment { .. })
    }

    pub(super) fn text_on(&self, page: usize) -> bool {
        matches!(self, Self::Text { page: p, .. } if *p == page)
    }

    /// Text hits on a page come before that page's comments, in glyph and annotation order.
    pub(super) fn sort_key(&self) -> (usize, u8, usize, usize) {
        match self {
            Self::Text { page, range } => (*page, 0, range.start, range.end),
            Self::Comment { page, index, .. } => (*page, 1, *index, 0),
        }
    }
}

/// A comment (or one reply) the Find bar can search. Several rows may name the same thread.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FindComment {
    page: usize,
    index: usize,
    author: String,
    kind: String,
    root_kind: String,
    body: String,
    top: f32,
    rect: [f32; 4],
}

/// Comments and replies, each row pointing at the thread's own comment so a reply match selects it.
pub(super) fn comment_find_index(info: &DocInfo) -> Vec<FindComment> {
    let tops = pdfcraft_render::thread_tops(&info.annotations);
    let roots: HashMap<(usize, usize), &Annotation> = info.annotations.iter().filter(|a| a.is_comment()).map(|a| ((a.page, a.index), a)).collect();
    let mut out = Vec::new();
    for annotation in &info.annotations {
        if annotation.subtype == "Popup" {
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
        let Some(top) = view_top(page, rect) else { continue };
        out.push(FindComment {
            page: root.page,
            index: root.index,
            author: annotation.author.clone().unwrap_or_default(),
            kind: kind_label(annotation).to_string(),
            root_kind: kind_label(root).to_string(),
            body: annotation.contents.clone().unwrap_or_default(),
            top,
            rect,
        });
    }
    out
}

/// The view-space top of `rect` on `page` (y down), the same edge a text hit scrolls toward.
fn view_top(page: &PageInfo, rect: [f32; 4]) -> Option<f32> {
    let [x0, y0, x1, y1] = rect;
    let top = [page.user_to_view(x0, y0)[1], page.user_to_view(x1, y0)[1], page.user_to_view(x0, y1)[1], page.user_to_view(x1, y1)[1]]
        .into_iter()
        .fold(f32::INFINITY, f32::min);
    top.is_finite().then_some(top)
}

pub(super) fn comment_hits(comments: &[FindComment], query: &str, case_sensitive: bool, whole_words: bool) -> Vec<FindHit> {
    let needle = normalize(query, case_sensitive);
    if needle.is_empty() {
        return Vec::new();
    }
    // Best field wins: the comment's own text, then its author, then its type. A reply that
    // matches better than the thread replaces the row, still selecting the thread.
    let mut best: HashMap<(usize, usize), (u8, FindHit)> = HashMap::new();
    for comment in comments {
        let Some((rank, field)) = matched_field(comment, &needle, case_sensitive, whole_words) else { continue };
        let key = (comment.page, comment.index);
        if best.get(&key).is_some_and(|(old, _)| *old >= rank) {
            continue;
        }
        best.insert(
            key,
            (
                rank,
                FindHit::Comment {
                    page: comment.page,
                    index: comment.index,
                    top: comment.top,
                    rect: comment.rect,
                    kind: comment.root_kind.clone(),
                    excerpt: snippet(field),
                },
            ),
        );
    }
    best.into_values().map(|(_, hit)| hit).collect()
}

/// Author, text and type, the same fields the Comments panel searches. Rank 3 is the text.
fn matched_field<'a>(comment: &'a FindComment, needle: &str, case_sensitive: bool, whole_words: bool) -> Option<(u8, &'a str)> {
    for (rank, field) in [(3u8, comment.body.as_str()), (2, comment.author.as_str()), (1, comment.kind.as_str())] {
        if field.is_empty() {
            continue;
        }
        if text_has(&normalize(field, case_sensitive), needle, whole_words) {
            return Some((rank, field));
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
    if needle.is_empty() {
        return false;
    }
    let hay: Vec<char> = haystack.chars().collect();
    let ned: Vec<char> = needle.chars().collect();
    let Some(last) = hay.len().checked_sub(ned.len()) else { return false };
    for start in 0..=last {
        let end = start.saturating_add(ned.len());
        if hay.get(start..end) != Some(ned.as_slice()) {
            continue;
        }
        if !whole_words {
            return true;
        }
        let before = start.checked_sub(1).and_then(|i| hay.get(i));
        let after = hay.get(end);
        if !before.is_some_and(|c| c.is_alphanumeric()) && !after.is_some_and(|c| c.is_alphanumeric()) {
            return true;
        }
    }
    false
}

/// The start of a long comment, on a character boundary, for the result list.
fn snippet(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for c in flat.chars() {
        if out.len().saturating_add(c.len_utf8()) > 160 {
            break;
        }
        out.push(c);
    }
    out
}
