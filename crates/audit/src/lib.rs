//! Verification tooling: audit a document for faux redactions.
//!
//! [`audit_redactions`] finds *faux redactions* — opaque filled rectangles drawn over text
//! that is still extractable underneath — and redaction annotations that were marked but
//! never applied. A properly applied redaction removes the text, so it never flags.
//!
//! A rectangle counts as a cover only when it was painted strictly *after* the text it
//! overlaps and hides what is under it: opaque (`/ca` of 1) and in Normal or Compatible blend
//! mode. Its colour does not matter: a white-out box over black text, or a black box over grey
//! or coloured text, hides it as surely as black over black. Page backgrounds and table shading
//! painted before the text are therefore not flagged, nor are highlights drawn translucent or
//! with a Multiply blend mode, which leave the text visible. It hides a line when it lies over
//! half of one of the line's words, 40% of the area of its words, or 40% of its box over a word
//! still drawn (see [`covers`]): a box over one word of a sentence counts, an underline, a
//! strikethrough or a table rule does not. A finding reports the whole text of every line the
//! cover hides, not only the hidden glyphs.
//!
//! Covers are collected from the page content, the form XObjects it draws, and the normal
//! appearance stream of every annotation a viewer paints (not Hidden or NoView, not a popup),
//! placed by its `/Rect`, `/BBox` and `/Matrix` and painted after the page content: a box
//! drawn over text with a shape or highlight tool is audited like one in the content.
//!
//! Detection limits (documented, not hidden): `re` rectangles and single closed `m`/`l`
//! subpaths whose four corners form an axis-aligned rectangle in user space (#1510) are
//! considered when closed by a fill operator — other filled paths, shadings and patterns are
//! not; a rectangle drawn with a translucent ExtGState (`/ca < 1`) or a blend mode other than
//! Normal or Compatible is not flagged; an opaque light rectangle painted over text for another
//! reason (shading added after the text) is flagged, since it does hide the text; a cover over
//! less than half of every word it touches and under 40% of the line is not flagged; annotation
//! rectangles are compared in unrotated user space, so pages with `/Rotate` may misalign; covers
//! inside form XObjects and annotation appearances are found, but only text drawn directly in
//! the page content is checked under them, so text inside a form XObject or an appearance is not
//! audited; an annotation whose `/AP /N` holds several states and no `/AS` has no appearance to
//! show and contributes nothing.
//!
//! The audit fails closed: any content that cannot be read (an undecodable content stream,
//! malformed annotations, form XObjects nested deeper than 32 or in a cycle) is an error, never
//! an empty finding list.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Document};
use pdfcraft_edit::TextLine;

mod cover;

#[cfg(test)]
mod tests;

/// What the audit found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindingKind {
    /// An opaque filled rectangle covers text that is still extractable.
    CoveredText,
    /// A `/Redact` annotation was placed but never applied: the text is untouched.
    UnappliedMark,
}

impl FindingKind {
    /// The stable machine-readable ID for this finding class.
    ///
    /// These strings are the crate's public vocabulary: they appear in CLI output and are
    /// the IDs a future machine-readable format would serialize. Do not rename them;
    /// add a new variant instead.
    pub fn id(self) -> &'static str {
        match self {
            FindingKind::CoveredText => "covered-text",
            FindingKind::UnappliedMark => "unapplied-mark",
        }
    }
}

/// One audit finding.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditFinding {
    /// 0-based page index.
    pub page: usize,
    pub kind: FindingKind,
    /// The covering rectangle (or the mark's rectangle), in page user space, normalized so
    /// `rect[0] <= rect[2]` and `rect[1] <= rect[3]`.
    pub rect: [f64; 4],
    /// The extractable text the cover hides, lines joined with single spaces and trimmed.
    /// Possibly empty when nothing decodable remains under the cover.
    pub covered_text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("page {0}: {1}")]
    Text(usize, String),
    #[error("content walk exceeded the operator bound")]
    TooManyOps,
    #[error("form XObjects nest deeper than the audit follows")]
    TooDeep,
    #[error("page has a malformed /Annots entry")]
    BadAnnots,
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
    #[error(transparent)]
    Edit(#[from] pdfcraft_edit::EditError),
}

/// Fraction of a text line's ink (the area of its words), or of its box, that must lie under a
/// cover to count as hidden (see [`covers`]).
const COVER_FRACTION: f64 = 0.4;
/// Fraction of one word's area that must lie under a cover to count as hidden on its own, however
/// long the line around it (see [`covers`]).
const WORD_FRACTION: f64 = 0.5;

/// Audit every page for faux redactions and unapplied redaction marks. Fails closed: any
/// content that cannot be decoded is returned as an error, never as an empty finding list.
pub fn audit_redactions(doc: &Document) -> Result<Vec<AuditFinding>, AuditError> {
    let mut out = Vec::new();
    for (n, page) in pdfcraft_model::pages(doc).iter().enumerate() {
        let lines = pdfcraft_edit::text_lines(doc, n).map_err(|e| AuditError::Text(n, e.to_string()))?;
        let annotations = annotations(doc, &page.dict)?;
        // 1. Opaque filled rectangles over extractable text, in the page content and in the
        //    appearances of its annotations.
        for r in cover::filled_rects(doc, &page.dict, &annotations)? {
            let mut covered = String::new();
            for line in &lines {
                if is_cover(&r, line) {
                    if !covered.is_empty() {
                        covered.push(' ');
                    }
                    covered.push_str(line.text.trim());
                }
            }
            if !covered.trim().is_empty() {
                out.push(AuditFinding { page: n, kind: FindingKind::CoveredText, rect: r.rect, covered_text: covered });
            }
        }
        // 2. Redact annotations that were never applied.
        for annot in redact_marks(doc, &annotations) {
            let mut covered = String::new();
            for line in &lines {
                if covers(&annot, line) {
                    if !covered.is_empty() {
                        covered.push(' ');
                    }
                    covered.push_str(line.text.trim());
                }
            }
            if !covered.trim().is_empty() {
                out.push(AuditFinding { page: n, kind: FindingKind::UnappliedMark, rect: annot, covered_text: covered });
            }
        }
    }
    Ok(out)
}

/// A filled rectangle is a faux-redaction cover for a text line iff it was painted strictly
/// after the line and hides glyphs of it. Its colour does not matter: an opaque fill hides the
/// text whatever the two colours are. Fills that cannot hide it (translucent, or in a blend
/// mode such as Multiply, as highlights are) are left out by the cover walker.
fn is_cover(rect: &cover::FilledRect, line: &TextLine) -> bool {
    rect.pos > line.paint_pos() && covers(&rect.rect, line)
}

/// Does `cover` hide a substantial part of what `line` draws? Three ways, any is enough:
///
/// - Half of one word lies under the cover. A box over one number in a long line hides a few
///   percent of the line, and the number all the same (#2662).
///
/// - 40% of the area of the line's words (its `ink`) lies under the cover. Measured on the words,
///   not the box: an applied redaction leaves a gap that draws nothing (a `TJ` adjustment where
///   the text was), and the box still spans it, so a redaction box at the end of a line used to
///   cover most of the box and flag the words left beside it.
/// - 40% of the line's box lies under the cover (the measure used before `ink`) and intersects the ink
///   of a word still drawn. This keeps a cover over a short value after a long label
///   flagged, while a cover over the gap alone, with no word under it, stops counting.
///
/// A line without word boxes is measured on its box alone.
fn covers(cover: &[f64; 4], line: &TextLine) -> bool {
    let hidden = |b: &[f64; 4]| area(&[cover[0].max(b[0]), cover[1].max(b[1]), cover[2].min(b[2]), cover[3].min(b[3])]);
    let fraction = |boxes: &[[f64; 4]]| {
        let total: f64 = boxes.iter().map(area).sum();
        if total > 0.0 { boxes.iter().map(hidden).sum::<f64>() / total } else { 0.0 }
    };
    if line.ink.iter().any(|w| area(w) > 0.0 && hidden(w) / area(w) >= WORD_FRACTION) {
        return true;
    }
    let inked = fraction(&line.ink) >= COVER_FRACTION;
    let boxed = fraction(std::slice::from_ref(&line.rect)) >= COVER_FRACTION;
    let word_intersects = || line.ink.iter().any(|w| hidden(w) > 0.0);
    inked || (boxed && (line.ink.is_empty() || word_intersects()))
}

/// The area of a rectangle, 0 when it is empty or inverted.
fn area(r: &[f64; 4]) -> f64 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

/// The annotation dictionaries of a page, in `/Annots` order. A missing `/Annots` means no
/// annotations; a malformed one fails the audit rather than silently passing.
fn annotations(doc: &Document, page: &Dict) -> Result<Vec<Dict>, AuditError> {
    let Some(a) = page.get(b"Annots") else { return Ok(Vec::new()) };
    let resolved = doc.resolve(a);
    let items: Vec<pdfcraft_cos::Object> = match &*resolved {
        pdfcraft_cos::Object::Array(items) => items.clone(),
        _ => return Err(AuditError::BadAnnots),
    };
    Ok(items
        .iter()
        .filter_map(|a| match &*doc.resolve(a) {
            pdfcraft_cos::Object::Dict(d) => Some(d.clone()),
            _ => None,
        })
        .collect())
}

/// Rectangles of the `/Redact` annotations among `annotations` (unrotated user space).
fn redact_marks(doc: &Document, annotations: &[Dict]) -> Vec<[f64; 4]> {
    annotations.iter().filter(|a| a.name(b"Subtype") == Some(b"Redact")).filter_map(|a| cover::rect(doc, a.get(b"Rect"))).collect()
}
