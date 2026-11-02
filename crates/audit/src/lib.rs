//! Verification tooling: audit a document for faux redactions.
//!
//! [`audit_redactions`] finds *faux redactions* — opaque filled rectangles drawn over text
//! that is still extractable underneath — and redaction annotations that were marked but
//! never applied. A properly applied redaction removes the text, so it never flags.
//!
//! A rectangle counts as a cover only when it was painted strictly *after* the text it
//! overlaps, and only when it is dark enough against the text's color to make the text
//! unreadable (WCAG contrast below 3.0). Page backgrounds, table shading and highlight
//! marks are therefore not flagged.
//!
//! Detection limits (documented, not hidden): `re` rectangles and single closed `m`/`l`
//! subpaths whose four corners form an axis-aligned rectangle in user space (#1510) are
//! considered when closed by a fill operator — other filled paths, shadings and patterns are
//! not; a rectangle drawn with a translucent ExtGState (`/ca < 1`) is not flagged; annotation
//! rectangles are compared in unrotated user space, so pages with `/Rotate` may misalign;
//! covers inside form XObjects are found, but only text drawn directly in the page content
//! is checked under them, so text inside a form XObject is not audited.
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
/// WCAG contrast ratio below which the text is unreadable against the cover.
const COVER_CONTRAST: f64 = 3.0;

/// Audit every page for faux redactions and unapplied redaction marks. Fails closed: any
/// content that cannot be decoded is returned as an error, never as an empty finding list.
pub fn audit_redactions(doc: &Document) -> Result<Vec<AuditFinding>, AuditError> {
    let mut out = Vec::new();
    for (n, page) in pdfcraft_model::pages(doc).iter().enumerate() {
        let lines = pdfcraft_edit::text_lines(doc, n).map_err(|e| AuditError::Text(n, e.to_string()))?;
        // 1. Opaque filled rectangles over extractable text.
        for r in cover::filled_rects(doc, &page.dict)? {
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
        for annot in annots(doc, &page.dict)? {
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
/// after the line, overlaps it substantially, and is dark enough against the line's color to
/// make the text unreadable. Light covers (table shading, highlights) over dark text fail the
/// contrast check and are not flagged.
fn is_cover(rect: &cover::FilledRect, line: &TextLine) -> bool {
    rect.pos > line.paint_pos() && covers(&rect.rect, line) && contrast(line.color, rect.rgb) < COVER_CONTRAST
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

/// WCAG 2.x relative luminance of an sRGB triple.
fn luminance(rgb: [f64; 3]) -> f64 {
    let lin = rgb.map(|c| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    });
    0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2]
}

/// WCAG contrast ratio between two sRGB colors: 1.0 (identical) to 21.0 (black on white).
fn contrast(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (l1, l2) = (luminance(a), luminance(b));
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// Rectangles of `/Redact` annotations on a page (unrotated user space). A missing `/Annots`
/// means no marks; a malformed one fails the audit rather than silently passing.
fn annots(doc: &Document, page: &Dict) -> Result<Vec<[f64; 4]>, AuditError> {
    let Some(a) = page.get(b"Annots") else { return Ok(Vec::new()) };
    let resolved = doc.resolve(a);
    let items: Vec<pdfcraft_cos::Object> = match &*resolved {
        pdfcraft_cos::Object::Array(items) => items.clone(),
        // A missing /Annots means no marks; a malformed one fails the audit.
        _ => return Err(AuditError::BadAnnots),
    };
    let mut out = Vec::new();
    for a in &items {
        let dict = match &*doc.resolve(a) {
            pdfcraft_cos::Object::Dict(d) => d.clone(),
            _ => continue,
        };
        if dict.name(b"Subtype") != Some(b"Redact") {
            continue;
        }
        let v: Vec<f64> = match dict.get(b"Rect") {
            None => continue,
            Some(r) => doc.resolve(r).as_array().map(|arr| arr.iter().filter_map(|o| doc.resolve(o).as_f64()).collect()).unwrap_or_default(),
        };
        if v.len() == 4 && v.iter().all(|x| x.is_finite()) {
            out.push([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])]);
        }
    }
    Ok(out)
}
