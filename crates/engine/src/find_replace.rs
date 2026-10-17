//! Find and replace in the page text (Edit text ▸ Replace, the `text_replace` tool).
//!
//! Each match is rewritten inside its own paragraph ([`crate::Document::text_blocks`]) through
//! [`Edit::EditTextBlock`], so the paragraph keeps its fonts, sizes and colours and rewraps in its
//! box. The paragraph's `editor_text` is matched with the find bar's own rules
//! ([`pdfcraft_render::text::find_in_text`]) and the replacement takes the style of the run it
//! lands in ([`pdfcraft_edit::TextBlock::replaced`]); every other run keeps its own.
//!
//! A match is left alone, and counted, when it runs from one paragraph into the next, lies in
//! invisible text over a scan, would need another font (the paragraph's own can't draw the new
//! text, see [`pdfcraft_edit::LineEdit::substituted`]) or is not page text the paragraphs hold
//! (a form field, a comment, text drawn inside a form XObject). Each paragraph is rewritten on a
//! copy first, so the edit returned is exactly what will apply: [`Edit::Batch`] for Replace all,
//! one undo step.

use std::collections::BTreeMap;
use std::ops::Range;

use crate::{Document, Edit, EditError, check_permission, guard};

/// What to look for and what to put in its place, with the find bar's options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Replace {
    pub find: String,
    /// May be empty: the match is deleted.
    pub with: String,
    pub case_sensitive: bool,
    pub whole_words: bool,
}

/// Which matches to replace.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Which {
    /// Every match on the pages.
    All,
    /// The first match in document order (a match that can't be replaced is reported, not passed).
    First,
    /// The match the find bar highlights on `page`: the centres of its first and last glyphs, in
    /// view space (points from the top-left of the displayed page, y down: the text layer's).
    At { page: usize, start: [f32; 2], end: [f32; 2] },
}

/// Why a match was left as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Skip {
    /// It runs from one paragraph into the next.
    SpansParagraphs,
    /// It is invisible text over a scanned image (recognized text).
    ScannedText,
    /// The paragraph's font can't draw the new text; another font would have stepped in.
    MissingGlyphs,
    /// The paragraph couldn't be rewritten (see [`Replacement::refusals`]).
    Refused,
    /// It is not in the page's editable text: a form field, a comment, text inside a form XObject.
    NotEditable,
}

impl Skip {
    /// How the reason is said after a count ("2 skipped: …"); an interface translates it.
    pub fn wording(self) -> &'static str {
        match self {
            Skip::SpansParagraphs => "spans two paragraphs",
            Skip::ScannedText => "scanned text",
            Skip::MissingGlyphs => "the font can't draw it",
            Skip::Refused => "the paragraph can't be rewritten",
            Skip::NotEditable => "not editable text",
        }
    }

    /// A short name for tools.
    pub fn key(self) -> &'static str {
        match self {
            Skip::SpansParagraphs => "spans_paragraphs",
            Skip::ScannedText => "scanned_text",
            Skip::MissingGlyphs => "missing_glyphs",
            Skip::Refused => "refused",
            Skip::NotEditable => "not_editable",
        }
    }
}

/// Every [`Skip::wording`], for an interface to translate.
pub const SKIP_WORDINGS: &[&str] =
    &["spans two paragraphs", "scanned text", "the font can't draw it", "the paragraph can't be rewritten", "not editable text"];

/// What a replacement would do: the edit (apply it with [`crate::Session::apply`]) and the counts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Replacement {
    /// `None` when nothing can be replaced.
    pub edit: Option<Edit>,
    /// Matches replaced.
    pub replaced: usize,
    /// The pages (0-based, ascending) where something was replaced.
    pub pages: Vec<usize>,
    /// Matches left alone, by reason.
    pub skipped: BTreeMap<Skip, usize>,
    /// What the engine said about the paragraphs it refused (at most [`MAX_REFUSALS`]).
    pub refusals: Vec<String>,
}

/// Refusal messages kept in a [`Replacement`].
pub const MAX_REFUSALS: usize = 10;

impl Replacement {
    pub fn skipped_total(&self) -> usize {
        self.skipped.values().fold(0, |a, b| a.saturating_add(*b))
    }

    fn skip(&mut self, why: Skip, n: usize) {
        if n > 0 {
            let count = self.skipped.entry(why).or_insert(0);
            *count = count.saturating_add(n);
        }
    }
}

/// The label of Replace all in the history (one of [`crate::BATCH_LABELS`]).
pub const REPLACE_ALL: &str = "Replace all";

impl Document {
    /// The edit that replaces `query`'s matches on `pages` (0-based; others are ignored), and
    /// what it replaces and leaves. `found` is how many matches the find bar has on each page:
    /// those no paragraph holds are counted as [`Skip::NotEditable`] (`None`: not counted). The
    /// document is not changed; the same permission check as Edit text applies.
    pub fn plan_replace(
        &self,
        pages: &[usize],
        query: &Replace,
        which: Which,
        found: Option<&BTreeMap<usize, usize>>,
    ) -> Result<Replacement, EditError> {
        let editor = self.editor.as_ref().ok_or_else(|| EditError::ReadOnly(self.read_only_reason.clone().unwrap_or_default()))?;
        // What Edit text checks: the document's permissions.
        let probe = Edit::EditTextBlock { page: 0, block: 0, text: String::new(), style: Default::default() };
        if let Some(p) = editor.cos.permissions() {
            check_permission(&probe, &p)?;
        }
        if query.find.split_whitespace().next().is_none() {
            return Err(EditError::Invalid("there is nothing to find".into()));
        }
        let n = self.info.pages.len();
        let mut pages: Vec<usize> = pages.iter().copied().filter(|p| *p < n).collect();
        pages.sort_unstable();
        pages.dedup();
        if let Which::At { page, .. } = which {
            pages.retain(|p| *p == page);
        }
        let mut out = Replacement::default();
        let mut work = editor.cos.clone();
        let mut edits = Vec::new();
        for page in pages {
            // A page whose text can't be read is reported, and the others still replaced.
            let blocks = match pdfcraft_edit::text_blocks(&work, page) {
                Ok(blocks) => blocks,
                Err(e) => {
                    refused(&mut out, found.and_then(|f| f.get(&page)).copied().unwrap_or(0), page, e.to_string());
                    continue;
                }
            };
            let matches: Vec<Vec<Range<usize>>> = blocks.iter().map(|b| find(&b.editor_text, query, usize::MAX)).collect();
            // Paragraph `k`'s matches to replace now.
            let mut chosen: BTreeMap<usize, Vec<Range<usize>>> = BTreeMap::new();
            match which {
                Which::All => {
                    let held = matches.iter().fold(0usize, |a, m| a.saturating_add(m.len()));
                    let spanning = spanning(&blocks, query);
                    out.skip(Skip::SpansParagraphs, spanning);
                    if let Some(f) = found.and_then(|f| f.get(&page)) {
                        out.skip(Skip::NotEditable, f.saturating_sub(held.saturating_add(spanning)));
                    }
                    chosen.extend(matches.into_iter().enumerate().filter(|(_, m)| !m.is_empty()));
                }
                Which::First => {
                    if let Some((k, m)) = matches.into_iter().enumerate().find_map(|(k, m)| m.into_iter().next().map(|m| (k, m))) {
                        chosen.insert(k, vec![m]);
                    }
                }
                Which::At { start, end, .. } => match at(self, &work, page, &blocks, &matches, start, end) {
                    Ok((k, m)) => {
                        chosen.insert(k, vec![m]);
                    }
                    Err(why) => out.skip(why, 1),
                },
            }
            // Last paragraph first: rewriting one never moves those before it in the page's order.
            for (k, ranges) in chosen.into_iter().rev() {
                let Some(block) = blocks.get(k) else { continue };
                let count = ranges.len();
                if block.invisible {
                    out.skip(Skip::ScannedText, count);
                    continue;
                }
                let Some((text, sources)) = block.replaced(&ranges, &query.with) else {
                    out.skip(Skip::Refused, count);
                    continue;
                };
                if text == block.editor_text {
                    // The replacement is what was there.
                    out.replaced = out.replaced.saturating_add(count);
                    continue;
                }
                let style = pdfcraft_edit::BlockStyle { sources, ..Default::default() };
                let mut trial = work.clone();
                match guard(|| pdfcraft_edit::rewrite_block(&mut trial, page, k, Some(&text), &style)) {
                    Ok(Ok(pdfcraft_edit::LineEdit { substituted: None })) => {
                        work = trial;
                        edits.push(Edit::EditTextBlock { page, block: k, text, style });
                        out.replaced = out.replaced.saturating_add(count);
                        if !out.pages.contains(&page) {
                            out.pages.push(page);
                        }
                    }
                    Ok(Ok(pdfcraft_edit::LineEdit { substituted: Some(_) })) => out.skip(Skip::MissingGlyphs, count),
                    Ok(Err(e)) => refused(&mut out, count, page, e.to_string()),
                    Err(panic) => refused(&mut out, count, page, panic),
                }
            }
            if which == Which::First && (out.replaced > 0 || out.skipped_total() > 0) {
                break;
            }
        }
        out.pages.sort_unstable();
        out.edit = match (which, edits.len()) {
            (_, 0) => None,
            (Which::All, _) => Some(Edit::Batch { label: REPLACE_ALL.into(), edits }),
            _ => edits.pop(),
        };
        Ok(out)
    }
}

fn find(text: &str, query: &Replace, limit: usize) -> Vec<Range<usize>> {
    pdfcraft_render::text::find_in_text(text, &query.find, query.case_sensitive, query.whole_words, limit)
}

fn refused(out: &mut Replacement, count: usize, page: usize, why: String) {
    out.skip(Skip::Refused, count);
    if out.refusals.len() < MAX_REFUSALS {
        out.refusals.push(format!("page {}: {why}", page.saturating_add(1)));
    }
}

/// Matches that start in one paragraph and end in a later one: the page's paragraphs read one
/// after another (a line break between two), as the find bar reads the page.
fn spanning(blocks: &[pdfcraft_edit::TextBlock], query: &Replace) -> usize {
    let mut joined = String::new();
    let mut ends = Vec::with_capacity(blocks.len());
    for b in blocks {
        if !joined.is_empty() {
            joined.push('\n');
        }
        joined.push_str(&b.editor_text);
        ends.push(joined.len());
    }
    // Which paragraph a byte of `joined` is in (a separator counts with the paragraph after it).
    let paragraph = |byte: usize| ends.partition_point(|end| *end <= byte);
    find(&joined, query, usize::MAX).into_iter().filter(|r| paragraph(r.start) != paragraph(r.end.saturating_sub(1))).count()
}

/// The paragraph match the find bar's match at `start` … `end` (view space) is: the paragraph
/// whose box holds both, and its match nearest `start`.
fn at(
    doc: &Document,
    cos: &pdfcraft_cos::Document,
    page: usize,
    blocks: &[pdfcraft_edit::TextBlock],
    matches: &[Vec<Range<usize>>],
    start: [f32; 2],
    end: [f32; 2],
) -> Result<(usize, Range<usize>), Skip> {
    let info = doc.info.pages.get(page).ok_or(Skip::NotEditable)?;
    let view = |x: f64, y: f64| info.user_to_view(x as f32, y as f32);
    // A paragraph's box in view space, a little larger so a glyph's centre on its edge is inside.
    let holds = |b: &pdfcraft_edit::TextBlock, p: [f32; 2]| {
        let corners = [view(b.rect[0], b.rect[1]), view(b.rect[2], b.rect[3]), view(b.rect[0], b.rect[3]), view(b.rect[2], b.rect[1])];
        let pad = (b.size as f32 * 0.3).clamp(1.0, 12.0);
        let (x0, x1) = corners.iter().fold((f32::MAX, f32::MIN), |(a, b), c| (a.min(c[0]), b.max(c[0])));
        let (y0, y1) = corners.iter().fold((f32::MAX, f32::MIN), |(a, b), c| (a.min(c[1]), b.max(c[1])));
        p[0] >= x0 - pad && p[0] <= x1 + pad && p[1] >= y0 - pad && p[1] <= y1 + pad
    };
    let lines = pdfcraft_edit::text_lines(cos, page).unwrap_or_default();
    // Where a match's first character is drawn, estimated along its run.
    let place = |b: &pdfcraft_edit::TextBlock, r: &Range<usize>| -> Option<[f32; 2]> {
        let (span, line) = b.spans.iter().find(|(s, _)| s.contains(&r.start)).or_else(|| b.spans.iter().find(|(s, _)| s.start >= r.start))?;
        let line = lines.get(*line)?;
        let run = b.editor_text.get(span.clone())?;
        let before = b.editor_text.get(span.start..r.start.max(span.start))?.chars().count() as f64;
        let total = run.chars().count().max(1) as f64;
        let along = ((before + 0.5) / total).clamp(0.0, 1.0);
        let x = line.rect[0] + along * (line.rect[2] - line.rect[0]);
        Some(view(x, (line.rect[1] + line.rect[3]) / 2.0))
    };
    let distance = |p: [f32; 2]| (p[0] - start[0]).hypot(p[1] - start[1]);
    let mut best: Option<(f32, usize, Range<usize>)> = None;
    for (k, b) in blocks.iter().enumerate().filter(|(_, b)| holds(b, start)) {
        for r in matches.get(k).into_iter().flatten() {
            let d = place(b, r).map_or(f32::MAX, distance);
            if best.as_ref().is_none_or(|(e, ..)| d < *e) {
                best = Some((d, k, r.clone()));
            }
        }
    }
    match best {
        Some((_, k, r)) if blocks.get(k).is_some_and(|b| holds(b, end)) => Ok((k, r)),
        Some(_) => Err(Skip::SpansParagraphs),
        None if blocks.iter().any(|b| holds(b, start)) && blocks.iter().any(|b| holds(b, end) && !holds(b, start)) => Err(Skip::SpansParagraphs),
        None => Err(Skip::NotEditable),
    }
}

#[cfg(test)]
mod tests;
