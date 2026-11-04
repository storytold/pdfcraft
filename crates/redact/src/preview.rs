//! The redaction marks a document holds, each with the areas applying it would remove or the
//! reason it can't be applied: one collector that previews ([`mark_previews`]) and [`crate::apply`]
//! both read, so what is shown and what is removed come from the same decisions.
//!
//! Every Redact annotation a page lists is reported at its original `/Annots` slot (`index`, as
//! `pdfcraft_render::Annotation::index` gives it), refused ones included: a mark that can't be
//! applied is never left out, so absence from this list never means "no mark".

use pdfcraft_annot::redaction::{MarkAreaError, MarkAreas, mark_areas, mark_locator};
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// The most `/QuadPoints` entries all the marks of one page may have together; past it the
/// page's marks are refused ([`MarkRefusal::PageLimit`]), other pages unaffected.
pub const MAX_PAGE_QUADPOINT_VALUES: usize = 1 << 20;

/// The most memory (approximate bytes) a preview list may hold; past it the scan fails
/// ([`MarkScanError::PreviewLimit`]) rather than return part of the marks.
pub const MAX_PREVIEW_BYTES: usize = 64 << 20;

/// One Redact annotation as a page lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkPreview {
    /// 0-based page.
    pub page: usize,
    /// Its slot in the page's `/Annots` array, before any filtering or sorting.
    pub index: usize,
    /// The annotation object (`None` for one written in place).
    pub object: Option<ObjRef>,
    /// The areas applying it removes, or why it can't be applied.
    pub areas: Result<MarkAreas, MarkRefusal>,
    /// For a refused mark, where to point at it: its `/Rect` when that is safe to draw (never an
    /// area that would be removed); `None` for a ready mark.
    pub locator: Option<[f64; 4]>,
}

impl MarkPreview {
    /// The areas a preview may fill: the ready areas, or none for a refused mark.
    pub fn removal_rects(&self) -> &[[f64; 4]] {
        self.areas.as_ref().map_or(&[], |a| a.rects.as_slice())
    }
}

/// Why a mark can't be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MarkRefusal {
    #[error("{0}")]
    Geometry(MarkAreaError),
    /// Written in place in `/Annots`: there is no object to apply and remove.
    #[error("it is written in place in the page's annotation list, so it can't be applied and removed")]
    InlineAnnotation,
    /// Listed through a reference to a reference, which applying can't remove safely.
    #[error("it is listed through a chain of references, so it can't be applied and removed safely")]
    UnsupportedAnnotationReference,
    /// Its page's marks have more `/QuadPoints` entries together than one page may.
    #[error("the marks on its page have more than {MAX_PAGE_QUADPOINT_VALUES} QuadPoints entries together")]
    PageLimit,
}

/// Why the marks could not be listed at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MarkScanError {
    #[error("there was not enough memory to check the redaction marks")]
    Allocation,
    #[error("the redaction marks take more than {} MiB to check at once", MAX_PREVIEW_BYTES >> 20)]
    PreviewLimit,
}

/// Every Redact annotation on the pages (all, or the 0-based `pages`), in page order and each
/// page in `/Annots` order, with its areas or refusal.
///
/// # Errors
///
/// [`MarkScanError`] when the list can't be held: then nothing is returned, never a part.
pub fn mark_previews(doc: &Document, pages: Option<&[usize]>) -> Result<Vec<MarkPreview>, MarkScanError> {
    Ok(collect(doc, pages)?.into_iter().map(|c| c.preview).collect())
}

/// A collected mark: its preview, and (when ready) its dictionary, held, for building the `Mark`.
pub(crate) struct Collected {
    pub(crate) preview: MarkPreview,
    pub(crate) held: Option<Arc<Object>>,
}

/// Whether `d` is a Redact annotation. Read directly as the collector always has; #5849 changes
/// this to follow an indirect `/Subtype`, and keeping it apart here keeps that change local.
fn is_redact(d: &Dict) -> bool {
    d.name(b"Subtype") == Some(b"Redact")
}

/// One listed entry that is a Redact annotation: its object (if it has one), whether that object
/// is the dictionary itself, and the dictionary (held as the document holds it).
struct Candidate {
    index: usize,
    object: Option<ObjRef>,
    direct_object: bool,
    held: Arc<Object>,
}

/// The one collector behind [`mark_previews`] and [`crate::marks_checked`].
pub(crate) fn collect(doc: &Document, pages: Option<&[usize]>) -> Result<Vec<Collected>, MarkScanError> {
    let mut out: Vec<Collected> = Vec::new();
    let mut held_bytes = 0usize;
    for (pi, p) in pdfcraft_model::pages(doc).iter().enumerate() {
        if pages.is_some_and(|sel| !sel.contains(&pi)) {
            continue;
        }
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut page_values = 0usize;
        for (index, entry) in crate::annots_of(doc, &p.dict).into_iter().enumerate() {
            // Held, not copied: a listed object is the document's own; only an entry written in place
            // is a copy (of what `annots_of` already copied).
            let held = doc.resolve(&entry);
            let Some(d) = held.as_dict().filter(|d| is_redact(d)) else { continue };
            let (object, direct_object) = match entry.as_ref() {
                Some(r) => (Some(r), doc.get(r).as_dict().is_some()),
                None => (None, false),
            };
            // The page's total is counted from the arrays' lengths, before any entry is read.
            // A direct array is measured in place; only a reference is followed (and held, not copied).
            let values = match d.get(b"QuadPoints") {
                Some(Object::Array(a)) => a.len(),
                Some(q @ Object::Ref(_)) => doc.resolve(q).as_array().map_or(0, Vec::len),
                _ => 0,
            };
            page_values = page_values.saturating_add(values);
            candidates.try_reserve(1).map_err(|_| MarkScanError::Allocation)?;
            candidates.push(Candidate { index, object, direct_object, held: held.clone() });
        }
        let over_page = page_values > MAX_PAGE_QUADPOINT_VALUES;
        for c in candidates {
            let Some(dict) = c.held.as_dict() else { continue };
            let areas = if over_page {
                Err(MarkRefusal::PageLimit)
            } else {
                match mark_areas(doc, dict) {
                    Err(e) => Err(MarkRefusal::Geometry(e)),
                    // Valid numbers, but a mark applying can't remove is refused all the same.
                    Ok(_) if c.object.is_none() => Err(MarkRefusal::InlineAnnotation),
                    Ok(_) if !c.direct_object => Err(MarkRefusal::UnsupportedAnnotationReference),
                    Ok(a) => Ok(a),
                }
            };
            let locator = if areas.is_err() { mark_locator(doc, dict) } else { None };
            let rects = areas.as_ref().map_or(0, |a| a.rects.len());
            held_bytes =
                held_bytes.saturating_add(std::mem::size_of::<Collected>()).saturating_add(rects.saturating_mul(std::mem::size_of::<[f64; 4]>()));
            if held_bytes > MAX_PREVIEW_BYTES {
                return Err(MarkScanError::PreviewLimit);
            }
            let held = areas.is_ok().then(|| c.held.clone());
            out.try_reserve(1).map_err(|_| MarkScanError::Allocation)?;
            out.push(Collected { preview: MarkPreview { page: pi, index: c.index, object: c.object, areas, locator }, held });
        }
    }
    Ok(out)
}
