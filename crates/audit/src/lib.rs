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
//! not; a rectangle drawn with a translucent ExtGState (`/ca < 1`) is not flagged; opaque
//! filled `/Square` annotations count as covers, but annotation rectangles are compared in
//! unrotated user space, so pages with `/Rotate` may misalign;
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
    /// Resets the comparison count for a page with these `lines`. A line costs its word boxes
    /// or its runs, whichever are more (at least one): a cover check walks the boxes, and the
    /// covered text walks the runs, which can draw no words at all (`( ) Tj`).
    fn start_page(&mut self, lines: &[TextLine]) {
        self.comparisons = 0;
        self.page_cost = lines.iter().fold(0, |n, line| n.saturating_add(line.ink.len().max(line.runs().count()).max(1)));
    }

    /// The text `cover` hides in the `lines` that `hides` ([`covered_words`]), joined with single
    /// spaces, or `None` when they show no text. Without `copying` the text is empty and never
    /// assembled: one long line under many covers would otherwise be copied once per cover,
    /// outside both bounds.
    fn hidden_text(
        &mut self,
        lines: &[TextLine],
        copying: bool,
        cover: &[f64; 4],
        hides: impl Fn(&TextLine) -> bool,
    ) -> Result<Option<String>, AuditError> {
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
            covered_words(cover, line, &mut covered);
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
        // 1. Opaque filled rectangles over extractable text.
        for r in cover::filled_rects(doc, &page.dict)? {
            if let Some(covered) = budget.hidden_text(&lines, copying, &r.rect, |line| is_cover(&r, line))? {
                let covered_text = budget.report_text(covered, copying)?;
                out.push(AuditFinding { page: n, kind: FindingKind::CoveredText, rect: r.rect, covered_text });
            }
        }
        // 2. Annotation covers over extractable text.
        for annot in annots(doc, &page.dict)? {
            let cover = match &annot {
                AnnotationCover::Redact(rect) | AnnotationCover::Square { rect, .. } => *rect,
            };
            if let Some(covered) = budget.hidden_text(&lines, copying, &cover, |line| match &annot {
                AnnotationCover::Redact(rect) => covers(rect, line),
                AnnotationCover::Square { rect, rgb } => covers(rect, line) && contrast(line.color, *rgb) < COVER_CONTRAST,
            })? {
                let (kind, rect) = match annot {
                    AnnotationCover::Redact(rect) => (FindingKind::UnappliedMark, rect),
                    AnnotationCover::Square { rect, .. } => (FindingKind::CoveredText, rect),
                };
                let covered_text = budget.report_text(covered, copying)?;
                out.push(AuditFinding { page: n, kind, rect, covered_text });
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

/// Appends the text of `line` that `cover` lies on to `out`: the runs whose words it overlaps
/// (#4164), so the words of the line painted beside the cover, which stay visible, are not
/// reported as hidden. Neighbouring runs are joined as the line joins them, so a word split
/// across runs stays one word; runs with others left out between them are spaced. The whole
/// line when its runs can't say: a single run, a line whose text was reordered for reading, or
/// a cover that overlaps the line's box but none of its words.
fn covered_words(cover: &[f64; 4], line: &TextLine, out: &mut String) {
    let overlaps = |w: &[f64; 4]| area(&[cover[0].max(w[0]), cover[1].max(w[1]), cover[2].min(w[2]), cover[3].min(w[3])]) > 0.0;
    let mut words = String::new();
    let mut last = None;
    for (i, (text, ink, spaced)) in line.runs().enumerate() {
        if text.trim().is_empty() || !ink.iter().any(overlaps) {
            continue;
        }
        let apart = last.is_some_and(|l: usize| l.saturating_add(1) != i || spaced);
        if apart && !words.ends_with(char::is_whitespace) && !text.starts_with(char::is_whitespace) {
            words.push(' ');
        }
        words.push_str(text);
        last = Some(i);
    }
    if line.runs().nth(1).is_none() || last.is_none() {
        out.push_str(line.text.trim());
    } else {
        out.push_str(words.trim());
    }
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

enum AnnotationCover {
    Redact([f64; 4]),
    Square { rect: [f64; 4], rgb: [f64; 3] },
}

/// Opaque annotation covers on a page (unrotated user space). A missing `/Annots` means no
/// covers; a malformed one fails the audit rather than silently passing.
fn annots(doc: &Document, page: &Dict) -> Result<Vec<AnnotationCover>, AuditError> {
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
        let v: Vec<f64> = match dict.get(b"Rect") {
            None => continue,
            Some(r) => doc.resolve(r).as_array().map(|arr| arr.iter().filter_map(|o| doc.resolve(o).as_f64()).collect()).unwrap_or_default(),
        };
        if v.len() != 4 || !v.iter().all(|x| x.is_finite()) {
            continue;
        }
        let rect = [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])];
        match subtype(doc, &dict)?.as_deref() {
            Some(b"Redact") => out.push(AnnotationCover::Redact(rect)),
            Some(b"Square") => {
                let flags = dict.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
                if flags & 2 != 0 || flags & 32 != 0 {
                    continue;
                }
                let alpha = dict.get(b"CA").map(|o| doc.resolve(o)).and_then(|o| o.as_f64()).unwrap_or(1.0);
                if alpha >= 1.0
                    && let Some(rgb) = annotation_color(doc, &dict, b"IC")
                {
                    out.push(AnnotationCover::Square { rect, rgb });
                }
            }
            _ => {}
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

fn annotation_color(doc: &Document, dict: &Dict, key: &[u8]) -> Option<[f64; 3]> {
    let values: Vec<f64> = dict.get(key).map(|o| doc.resolve(o))?.as_array()?.iter().map(|o| doc.resolve(o).as_f64()).collect::<Option<_>>()?;
    if !values.iter().all(|v| v.is_finite()) {
        return None;
    }
    match values.as_slice() {
        [g] => Some([g.clamp(0.0, 1.0); 3]),
        [r, g, b] => Some([r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]),
        [c, m, y, k] => {
            let (c, m, y, k) = (c.clamp(0.0, 1.0), m.clamp(0.0, 1.0), y.clamp(0.0, 1.0), k.clamp(0.0, 1.0));
            Some([(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)])
        }
        _ => None,
    }
}
