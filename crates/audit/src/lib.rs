//! Verification tooling for PDF documents.
//!
//! Two checks that upstream PrintCraft does not do:
//!
//! - [`audit_redactions`]: find *faux redactions* — opaque filled rectangles drawn over text
//!   that is still extractable underneath — and redaction annotations that were marked but
//!   never applied. A properly applied redaction removes the text, so it never flags.
//! - [`sanitize_for_sharing`]: run the full hidden-information removal and return a
//!   human-readable before/after diff proving what was removed.
//!
//! Detection limits (documented, not hidden): only `re` rectangles closed by a fill operator
//! are considered — arbitrary filled paths, shadings and patterns are not; a rectangle drawn
//! with a translucent ExtGState (`/ca < 1`) is not flagged; annotation rectangles are compared
//! in unrotated user space, so pages with `/Rotate` may misalign.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use printcraft_cos::{Dict, Document};
use printcraft_redact::sanitize::{HIDDEN, Hidden, sanitize, scan};

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
    /// The covering rectangle (or the mark's rectangle), in page user space.
    pub rect: [f64; 4],
    /// The extractable text the cover hides (possibly empty when nothing decodable remains).
    pub covered_text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("page {0}: {1}")]
    Text(usize, String),
    #[error(transparent)]
    Cos(#[from] printcraft_cos::CosError),
    #[error(transparent)]
    Edit(#[from] printcraft_edit::EditError),
    #[error(transparent)]
    Redact(#[from] printcraft_redact::RedactError),
}

/// Fraction of a text line's area that must lie under a cover to count as hidden.
const COVER_FRACTION: f64 = 0.4;

/// Audit every page for faux redactions and unapplied redaction marks.
pub fn audit_redactions(doc: &Document) -> Result<Vec<AuditFinding>, AuditError> {
    let mut out = Vec::new();
    for (n, page) in printcraft_model::pages(doc).iter().enumerate() {
        let lines = printcraft_edit::text_lines(doc, n).map_err(|e| AuditError::Text(n, e.to_string()))?;
        // 1. Opaque filled rectangles over extractable text.
        for r in cover::filled_rects(doc, &page.dict) {
            let mut covered = String::new();
            for line in &lines {
                if covers(&r.rect, &line.rect) {
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
        for annot in annots(doc, &page.dict) {
            let mut covered = String::new();
            for line in &lines {
                if covers(&annot, &line.rect) {
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

/// Does `cover` hide a substantial part of `line`?
fn covers(cover: &[f64; 4], line: &[f64; 4]) -> bool {
    let ix0 = cover[0].max(line[0]);
    let iy0 = cover[1].max(line[1]);
    let ix1 = cover[2].min(line[2]);
    let iy1 = cover[3].min(line[3]);
    if ix1 <= ix0 || iy1 <= iy0 {
        return false;
    }
    let line_area = (line[2] - line[0]).max(0.0) * (line[3] - line[1]).max(0.0);
    if line_area <= 0.0 {
        return false;
    }
    (ix1 - ix0) * (iy1 - iy0) / line_area >= COVER_FRACTION
}

/// Rectangles of `/Redact` annotations on a page (unrotated user space).
fn annots(doc: &Document, page: &Dict) -> Vec<[f64; 4]> {
    let mut out = Vec::new();
    let annots: Vec<printcraft_cos::Object> = match page.get(b"Annots") {
        None => return out,
        Some(a) => match &*doc.resolve(a) {
            printcraft_cos::Object::Array(items) => items.clone(),
            _ => return out,
        },
    };
    for a in &annots {
        let dict = match &*doc.resolve(a) {
            printcraft_cos::Object::Dict(d) => d.clone(),
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
    out
}

/// The `/Info` string entries of a document, for before/after comparison.
pub fn metadata_of(doc: &Document) -> Vec<(String, String)> {
    const KEYS: [&[u8]; 8] = [b"Title", b"Author", b"Subject", b"Keywords", b"Creator", b"Producer", b"CreationDate", b"ModDate"];
    let info = doc.trailer().get(b"Info").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let mut out = Vec::new();
    for key in KEYS {
        let value = info
            .get(key)
            .map(|o| doc.resolve(o))
            .and_then(|o| match &*o {
                printcraft_cos::Object::String(s) => Some(s.to_text()),
                _ => None,
            })
            .unwrap_or_default();
        if !value.is_empty() {
            let name = String::from_utf8_lossy(key).into_owned();
            out.push((name, value));
        }
    }
    out
}

/// One hidden-information category's before/after counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryDiff {
    pub id: &'static str,
    pub label: &'static str,
    pub before: usize,
    pub after: usize,
}

/// The verifiable result of sanitizing a document for sharing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SanitizeReport {
    pub categories: Vec<CategoryDiff>,
    /// Total items removed across all categories.
    pub removed_total: usize,
    pub metadata_before: Vec<(String, String)>,
    pub metadata_after: Vec<(String, String)>,
}

impl SanitizeReport {
    /// One human-readable line per removed item, e.g. `removed metadata: Author=Erick Barraza`.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (name, value) in &self.metadata_before {
            if !self.metadata_after.iter().any(|(n, v)| n == name && v == value) {
                out.push(format!("removed metadata: {name}={value}"));
            }
        }
        for c in &self.categories {
            let removed = c.before.saturating_sub(c.after);
            if removed > 0 {
                out.push(format!("removed {}: {removed} item(s)", c.label.to_lowercase()));
            }
        }
        if out.is_empty() {
            out.push("nothing to remove: the document was already clean".to_string());
        }
        out
    }
}

/// Remove every hidden-information category and report exactly what changed.
///
/// This is the "sanitize for sharing" flow: the caller saves the document afterwards (a full
/// rewrite drops the earlier revisions that still hold the removed data).
pub fn sanitize_for_sharing(doc: &mut Document) -> Result<SanitizeReport, AuditError> {
    let counts_before = scan(doc);
    let metadata_before = metadata_of(doc);
    sanitize(doc)?;
    let counts_after = scan(doc);
    let metadata_after = metadata_of(doc);
    let categories: Vec<CategoryDiff> = HIDDEN
        .iter()
        .map(|h: &Hidden| {
            let before = counts_before.iter().find(|(k, _)| k == h).map(|(_, n)| *n).unwrap_or(0);
            let after = counts_after.iter().find(|(k, _)| k == h).map(|(_, n)| *n).unwrap_or(0);
            CategoryDiff { id: h.id(), label: h.label(), before, after }
        })
        .collect();
    let removed_total = categories.iter().map(|c| c.before.saturating_sub(c.after)).sum();
    Ok(SanitizeReport { categories, removed_total, metadata_before, metadata_after })
}
