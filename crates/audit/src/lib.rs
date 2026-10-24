//! Verification tooling: audit a document for faux redactions.
//!
//! [`audit_redactions`] finds *faux redactions* — opaque filled rectangles drawn over text
//! that is still extractable underneath — and redaction annotations that were marked but
//! never applied. A properly applied redaction removes the text, so it never flags.
//!
//! A rectangle counts as a cover only when it was painted strictly *after* the text it
//! overlaps, and only when it is dark enough against the text's color to make the text
//! unreadable (WCAG contrast below 3.0). Page backgrounds, table shading and highlight
//! marks are therefore not flagged. A white (or nearly white) opaque rectangle painted after
//! dark text hides it just as well, the way forms are "whited out"; that is reported as its
//! own kind, [`FindingKind::HiddenUnderWhite`], so callers can decide (#2385).
//!
//! Covers come from the page content — `re` rectangles and single closed `m`/`l` subpaths whose
//! four corners form an axis-aligned rectangle in user space (#1510), filled opaquely, and a
//! thick opaque stroke along one axis (a "marker" line, its round or square caps included) — and
//! from the annotations: an opaque `/Highlight` dark enough to hide the text under it (#2385), and
//! an opaque filled `/Square` likewise, or white (#2854). A `/Redact` mark is reported whatever its
//! flags: Hidden or NoView hides the mark, not the text.
//!
//! Detection limits (documented, not hidden): other filled paths, slanted or bent strokes,
//! shadings and patterns are not considered; a cover drawn with a translucent ExtGState
//! (`/ca`, `/CA < 1`) or a highlight or square with `/CA < 1` is not flagged; annotation rectangles are
//! compared in unrotated user space, so pages with `/Rotate` may misalign; covers inside form
//! XObjects are found, but only text drawn directly in the page content is checked under them,
//! so text inside a form XObject is not audited; a `/Highlight` is judged by its own
//! `/QuadPoints`, `/C` and `/CA`, never by its appearance stream (`/AP`), so a finding can differ
//! from what a viewer paints.
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
    /// An opaque dark cover (a filled rectangle, a thick stroke, a highlight annotation) hides
    /// text that is still extractable.
    CoveredText,
    /// A `/Redact` annotation was placed but never applied: the text is untouched.
    UnappliedMark,
    /// A white (or nearly white) opaque rectangle painted after text, or a white filled square
    /// annotation over it, hides it: the text is invisible on the page and still extractable (a
    /// "whited out" form).
    HiddenUnderWhite,
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
            FindingKind::HiddenUnderWhite => "hidden-under-white",
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
    /// Empty when the document's permissions withhold copying (see [`audit_redactions`]).
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
    #[error("content walk entered more form XObjects than the audit follows")]
    TooManyForms,
    #[error("page has a malformed /Annots entry")]
    BadAnnots,
    #[error("page has more covers and text lines than the audit compares")]
    TooManyComparisons,
    #[error("covered text exceeds the audit's report budget")]
    TooMuchOutput,
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
/// Relative luminance from which an opaque cover counts as white (white is 1.0; `0.9 g`, the
/// kind of shading a table has, is 0.79): dark text under it is gone, not shaded.
const WHITE_LUMINANCE: f64 = 0.9;

/// Audit every page for faux redactions and unapplied redaction marks. Fails closed: any
/// content that cannot be decoded, or that needs more work or report than the audit's bounds, is
/// returned as an error, never as an empty finding list.
///
/// When the document's permissions withhold copying (ISO 32000-2 Table 22, bit 5), findings are
/// still reported but their `covered_text` is empty: the audit hands out no text that copying
/// would refuse.
pub fn audit_redactions(doc: &Document) -> Result<Vec<AuditFinding>, AuditError> {
    audit(doc, Limits::DEFAULT)
}

/// Work and output bounds for one audit.
#[derive(Clone, Copy)]
struct Limits {
    /// Cover-against-line comparisons per page, counting a line once per word box it has (at
    /// least once), since [`covers`] walks them.
    comparisons: usize,
    /// Bytes of covered text the findings report.
    covered_text: usize,
}

impl Limits {
    /// Every cover is compared with every line of its page, and each finding repeats the text
    /// it covers, so both grow with the square of the content: a few megabytes of covers over
    /// one long line would otherwise take hours and gigabytes.
    const DEFAULT: Limits = Limits { comparisons: 50_000_000, covered_text: 64 << 20 };
}

/// The work and report the audit has spent against its [`Limits`].
struct Budget {
    limits: Limits,
    comparisons: usize,
    /// What comparing one cover with every line of the current page costs.
    page_cost: usize,
    covered_text: usize,
}

impl Budget {
    /// Resets the comparison count for a page with these `lines`.
    fn start_page(&mut self, lines: &[TextLine]) {
        self.comparisons = 0;
        self.page_cost = lines.iter().fold(0, |n, line| n.saturating_add(line.ink.len().max(1)));
    }

    /// The text of the `lines` that `hides`, joined with single spaces, or `None` when they show
    /// no text. Without `copying` the text is empty and never assembled: one long line under many
    /// covers would otherwise be copied once per cover, outside both bounds.
    fn hidden_text(&mut self, lines: &[TextLine], copying: bool, hides: impl Fn(&TextLine) -> bool) -> Result<Option<String>, AuditError> {
        self.comparisons = self.comparisons.saturating_add(self.page_cost);
        if self.comparisons > self.limits.comparisons {
            return Err(AuditError::TooManyComparisons);
        }
        if !copying {
            return Ok(lines.iter().any(|line| hides(line) && !line.text.trim().is_empty()).then(String::new));
        }
        let mut covered = String::new();
        for line in lines.iter().filter(|l| hides(l)) {
            if !covered.is_empty() {
                covered.push(' ');
            }
            covered.push_str(line.text.trim());
        }
        Ok((!covered.trim().is_empty()).then_some(covered))
    }

    /// The text a finding reports for `covered`: none when copying is withheld. Charges what it
    /// reports against the output bound.
    fn report_text(&mut self, covered: String, copying: bool) -> Result<String, AuditError> {
        let reported = if copying { covered } else { String::new() };
        self.covered_text = self.covered_text.saturating_add(reported.len());
        if self.covered_text > self.limits.covered_text {
            return Err(AuditError::TooMuchOutput);
        }
        Ok(reported)
    }
}

fn audit(doc: &Document, limits: Limits) -> Result<Vec<AuditFinding>, AuditError> {
    let copying = doc.permissions().is_none_or(|p| p.copy());
    let mut budget = Budget { limits, comparisons: 0, page_cost: 0, covered_text: 0 };
    let mut out = Vec::new();
    for (n, page) in pdfcraft_model::pages(doc).iter().enumerate() {
        // The glyphs themselves: `/ActualText` is the file's own claim about them, and an empty
        // or substitute one must not hide text a cover sits on.
        let lines = pdfcraft_edit::glyph_text_lines(doc, page).map_err(|e| AuditError::Text(n, e.to_string()))?;
        budget.start_page(&lines);
        // 1. Opaque rectangles (filled, or painted by a thick stroke) over extractable text.
        for r in cover::filled_rects(doc, &page.dict)? {
            if let Some(covered) = budget.hidden_text(&lines, copying, |line| hides(&r, line))? {
                let covered_text = budget.report_text(covered, copying)?;
                out.push(AuditFinding { page: n, kind: rect_kind(&r), rect: r.rect, covered_text });
            }
        }
        // 2. Redact annotations that were never applied, and highlights and filled squares that
        // hide the text under them (annotations are drawn over the page's content): dark enough
        // against it, or, for a square, white, like a white rectangle in the content.
        for mark in annots(doc, &page.dict)? {
            let (kind, hides) = match mark.kind {
                Mark::Redact => (FindingKind::UnappliedMark, None),
                Mark::Square(rgb) if luminance(rgb) >= WHITE_LUMINANCE => (FindingKind::HiddenUnderWhite, None),
                Mark::Highlight(rgb) | Mark::Square(rgb) => (FindingKind::CoveredText, Some(rgb)),
            };
            if let Some(covered) = budget.hidden_text(&lines, copying, |line| {
                mark.areas.iter().any(|a| covers(a, line)) && hides.is_none_or(|rgb| contrast(line.color, rgb) < COVER_CONTRAST)
            })? {
                let covered_text = budget.report_text(covered, copying)?;
                out.push(AuditFinding { page: n, kind, rect: mark.rect, covered_text });
            }
        }
    }
    Ok(out)
}

/// A filled rectangle is a faux-redaction cover for a text line iff it was painted strictly
/// after the start of the line, overlaps it substantially, and is dark enough against the line's
/// color to make the text unreadable. Light covers (table shading, highlights) over dark text fail
/// the contrast check and are not flagged. A line can join runs painted before and after the
/// cover; the later ones are drawn on it in the same low-contrast color, so they are unreadable
/// too.
fn is_cover(rect: &cover::FilledRect, line: &TextLine) -> bool {
    rect.pos > line.first_paint_pos() && covers(&rect.rect, line) && contrast(line.color, rect.rgb) < COVER_CONTRAST
}

/// What a painted rectangle does to the text under it: white or nearly so, it whites the text out
/// whatever its color; otherwise it is a cover, and only hides text it is dark enough against.
fn rect_kind(rect: &cover::FilledRect) -> FindingKind {
    if luminance(rect.rgb) >= WHITE_LUMINANCE { FindingKind::HiddenUnderWhite } else { FindingKind::CoveredText }
}

/// Does `rect` hide `line`: painted after the start of the line and over it, and white or a cover
/// ([`is_cover`]). Light covers in between (table shading) are not flagged.
fn hides(rect: &cover::FilledRect, line: &TextLine) -> bool {
    match rect_kind(rect) {
        FindingKind::HiddenUnderWhite => rect.pos > line.first_paint_pos() && covers(&rect.rect, line),
        _ => is_cover(rect, line),
    }
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

/// An annotation the audit looks at.
enum Mark {
    Redact,
    /// An opaque highlight, with its color.
    Highlight([f64; 3]),
    /// An opaque filled square, with its interior color (`/IC`).
    Square([f64; 3]),
}

struct Annot {
    /// `/Rect`, normalized, in unrotated user space.
    rect: [f64; 4],
    /// What the annotation paints: a highlight's `/QuadPoints`, each as its bounding box, or else
    /// the `/Rect`.
    areas: Vec<[f64; 4]>,
    kind: Mark,
}

/// The `/Redact` annotations on a page, and the `/Highlight` and filled `/Square` ones shown
/// opaque with a color (unrotated user space). A missing `/Annots` means no marks; a malformed
/// one fails the audit rather than silently passing.
fn annots(doc: &Document, page: &Dict) -> Result<Vec<Annot>, AuditError> {
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
        let nums = |key: &[u8]| -> Vec<f64> {
            dict.get(key)
                .map(|r| doc.resolve(r))
                .and_then(|r| r.as_array().map(|arr| arr.iter().filter_map(|o| doc.resolve(o).as_f64()).collect()))
                .unwrap_or_default()
        };
        // Hidden or NoView: not drawn, so a cover hides nothing. A `/Redact` mark is audited
        // whatever its flags: hiding the mark does not apply the redaction.
        let flags = dict.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
        let drawn = flags & (2 | 32) == 0;
        // The color the annotation paints opaquely: a highlight's `/C`, a square's interior `/IC`.
        // `None`: no color (drawn transparent), or `/CA` below 1.
        let opaque = |key: &[u8]| -> Option<[f64; 3]> {
            let alpha = dict.get(b"CA").and_then(|a| doc.resolve(a).as_f64()).unwrap_or(1.0);
            let rgb = match nums(key).as_slice() {
                [g] => [*g, *g, *g],
                [r, g, b] => [*r, *g, *b],
                [c, m, y, k] => [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)],
                _ => return None,
            };
            if alpha.is_nan() || alpha < 1.0 || rgb.iter().any(|v| !v.is_finite()) {
                return None;
            }
            Some(rgb.map(|v| v.clamp(0.0, 1.0)))
        };
        let kind = match subtype(doc, &dict)?.as_deref() {
            Some(b"Redact") => Mark::Redact,
            Some(b"Highlight") => match opaque(b"C") {
                Some(rgb) if drawn => Mark::Highlight(rgb),
                _ => continue,
            },
            Some(b"Square") => match opaque(b"IC") {
                Some(rgb) if drawn => Mark::Square(rgb),
                _ => continue,
            },
            _ => continue,
        };
        let v = nums(b"Rect");
        if v.len() == 4 && v.iter().all(|x| x.is_finite()) {
            let rect = [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])];
            let q = nums(b"QuadPoints");
            let areas = if matches!(kind, Mark::Highlight(_)) && !q.is_empty() && q.len() % 8 == 0 && q.iter().all(|x| x.is_finite()) {
                q.chunks(8)
                    .map(|c| {
                        [
                            c[0].min(c[2]).min(c[4]).min(c[6]),
                            c[1].min(c[3]).min(c[5]).min(c[7]),
                            c[0].max(c[2]).max(c[4]).max(c[6]),
                            c[1].max(c[3]).max(c[5]).max(c[7]),
                        ]
                    })
                    .collect()
            } else {
                vec![rect]
            };
            out.push(Annot { rect, areas, kind });
        }
    }
    Ok(out)
}

/// The `/Subtype` name of `dict`, following an indirect reference. A reference that cannot be
/// loaded fails the audit: the subtype decides whether the object is checked at all.
fn subtype(doc: &Document, dict: &Dict) -> Result<Option<Vec<u8>>, AuditError> {
    let name = match dict.get(b"Subtype") {
        Some(pdfcraft_cos::Object::Ref(r)) => doc.try_get(r.num)?.as_name().map(<[u8]>::to_vec),
        other => other.and_then(pdfcraft_cos::Object::as_name).map(<[u8]>::to_vec),
    };
    Ok(name)
}
