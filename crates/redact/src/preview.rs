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
    /// Its `/Subtype` is given by reference: the renderer draws it as a Redact mark, and applying
    /// one is not supported yet (#5849 follows such a `/Subtype`), so it is refused, never skipped.
    #[error("its type is given by reference, which redaction can't apply yet")]
    IndirectSubtype,
    /// Its type can't be known: its `/Subtype`, or the reference listing it, can't be followed (a
    /// cycle, or a chain longer than references are followed). It may be a mark the page shows.
    #[error("its type can't be read (a reference that can't be followed), so it may be a redaction mark")]
    UnresolvedType,
    /// Its page's marks have more `/QuadPoints` entries together than one page may.
    #[error("the marks on its page have more than {MAX_PAGE_QUADPOINT_VALUES} QuadPoints entries together")]
    PageLimit,
}

/// Why the marks could not be listed at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MarkScanError {
    /// A page's `/Annots` is a reference that can't be followed, so its marks can't be listed.
    #[error("a page's list of annotations is a reference that can't be followed, so its redaction marks can't be known")]
    UnresolvedAnnotations,
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

/// An annotation's `/Subtype`, followed through a reference when it is given by one (the
/// renderer does so), and whether it was. Read here for every decision redaction makes about
/// what an annotation is, so the page and redaction never classify one differently. #5849 also
/// follows it; keeping the reading in one helper keeps that change local.
/// Whether `d`'s `/Subtype` is a reference that can't be followed to a value (a cycle, or a chain past
/// [`Document::resolve`]'s bound): its type is unknown, which redaction never reads as "not a mark".
pub(crate) fn subtype_unresolved(doc: &Document, d: &Dict) -> bool {
    matches!(d.get(b"Subtype"), Some(r @ Object::Ref(_)) if matches!(&*doc.resolve(r), Object::Ref(_)))
}

pub(crate) fn subtype_of(doc: &Document, d: &Dict) -> (Option<Vec<u8>>, bool) {
    match d.get(b"Subtype") {
        Some(Object::Name(n)) => (Some(n.clone()), false),
        Some(r @ Object::Ref(_)) => (doc.resolve(r).as_name().map(<[u8]>::to_vec), true),
        _ => (None, false),
    }
}

/// A listed entry's dictionary: borrowed when written in place, held as the document holds it
/// when listed by reference. Never a copy.
enum Entry<'a> {
    Inline(&'a Dict),
    Held(Arc<Object>),
}

impl Entry<'_> {
    fn dict(&self) -> Option<&Dict> {
        match self {
            Entry::Inline(d) => Some(d),
            // A stream object is an annotation dictionary all the same (the renderer reads it so).
            Entry::Held(o) => match &**o {
                Object::Dict(d) => Some(d),
                Object::Stream(s) => Some(&s.dict),
                _ => None,
            },
        }
    }
}

/// One listed entry that is a Redact annotation.
struct Candidate<'a> {
    index: usize,
    object: Option<ObjRef>,
    /// The listed object is the dictionary itself (not a reference to a reference).
    direct_object: bool,
    indirect_subtype: bool,
    /// Its type can't be known: refused.
    unresolved: bool,
    entry: Entry<'a>,
}

/// The one collector behind [`mark_previews`] and [`crate::marks_checked`].
pub(crate) fn collect(doc: &Document, pages: Option<&[usize]>) -> Result<Vec<Collected>, MarkScanError> {
    collect_within(doc, pages, MAX_PREVIEW_BYTES)
}

/// [`collect`] within `budget` bytes of what it holds, counted before anything is kept.
pub(crate) fn collect_within(doc: &Document, pages: Option<&[usize]>, budget: usize) -> Result<Vec<Collected>, MarkScanError> {
    let mut out: Vec<Collected> = Vec::new();
    let mut held_bytes = 0usize;
    let mut charge = |bytes: usize| {
        held_bytes = held_bytes.saturating_add(bytes);
        if held_bytes > budget { Err(MarkScanError::PreviewLimit) } else { Ok(()) }
    };
    for (pi, p) in pdfcraft_model::pages(doc).iter().enumerate() {
        if pages.is_some_and(|sel| !sel.contains(&pi)) {
            continue;
        }
        // The page's list, borrowed from the page or held as the document holds it.
        let held_list;
        let list: &[Object] = match p.dict.get(b"Annots") {
            Some(Object::Array(a)) => a,
            Some(r @ Object::Ref(_)) => {
                held_list = doc.resolve(r);
                // A list that can't be followed may hold marks the page shows: no partial answer.
                if matches!(&*held_list, Object::Ref(_)) {
                    return Err(MarkScanError::UnresolvedAnnotations);
                }
                held_list.as_array().map_or(&[], Vec::as_slice)
            }
            _ => &[],
        };
        let mut candidates: Vec<Candidate<'_>> = Vec::new();
        let mut page_values = 0usize;
        for (index, listed) in list.iter().enumerate() {
            let entry = match listed {
                Object::Dict(d) => Entry::Inline(d),
                Object::Ref(_) => Entry::Held(doc.resolve(listed)),
                _ => continue,
            };
            // An entry whose type can't be known (a reference or a `/Subtype` that can't be followed) may
            // be a mark the renderer draws: it is listed, refused, never skipped.
            let unresolved = match entry.dict() {
                None => matches!(&entry, Entry::Held(o) if matches!(**o, Object::Ref(_))),
                Some(d) => subtype_unresolved(doc, d),
            };
            let (redact, indirect_subtype) = match entry.dict() {
                Some(d) if !unresolved => {
                    let (subtype, indirect) = subtype_of(doc, d);
                    (subtype.as_deref() == Some(b"Redact"), indirect)
                }
                _ => (false, false),
            };
            if !redact && !unresolved {
                continue;
            }
            let (object, direct_object) = match listed.as_ref() {
                Some(r) => (Some(r), doc.get(r).as_dict().is_some()),
                None => (None, false),
            };
            // The page's total is counted from the arrays' lengths, before any entry is read: a
            // direct array is measured in place, only a reference is followed (held, not copied).
            page_values = page_values.saturating_add(match entry.dict().and_then(|d| d.get(b"QuadPoints")) {
                Some(Object::Array(a)) => a.len(),
                Some(q @ Object::Ref(_)) => doc.resolve(q).as_array().map_or(0, Vec::len),
                _ => 0,
            });
            charge(std::mem::size_of::<Candidate<'_>>().saturating_add(std::mem::size_of::<Collected>()))?;
            candidates.try_reserve(1).map_err(|_| MarkScanError::Allocation)?;
            #[cfg(test)]
            tests::count_candidate();
            candidates.push(Candidate { index, object, direct_object, indirect_subtype, unresolved, entry });
        }
        let over_page = page_values > MAX_PAGE_QUADPOINT_VALUES;
        for c in candidates {
            let dict = c.entry.dict();
            let areas = if c.unresolved {
                Err(MarkRefusal::UnresolvedType)
            } else if c.indirect_subtype {
                // The renderer draws it as a Redact mark; applying it is not supported yet, so it is
                // refused, never skipped.
                Err(MarkRefusal::IndirectSubtype)
            } else if over_page {
                Err(MarkRefusal::PageLimit)
            } else {
                match dict.map(|d| mark_areas(doc, d)).unwrap_or(Err(MarkAreaError::NoUsableArea)) {
                    Err(e) => Err(MarkRefusal::Geometry(e)),
                    // Valid numbers, but a mark applying can't remove is refused all the same.
                    Ok(_) if c.object.is_none() => Err(MarkRefusal::InlineAnnotation),
                    Ok(_) if !c.direct_object => Err(MarkRefusal::UnsupportedAnnotationReference),
                    Ok(a) => Ok(a),
                }
            };
            let locator = if areas.is_err() { dict.and_then(|d| mark_locator(doc, d)) } else { None };
            charge(areas.as_ref().map_or(0, |a| a.rects.len()).saturating_mul(std::mem::size_of::<[f64; 4]>()))?;
            let held = match (&areas, c.entry) {
                (Ok(_), Entry::Held(o)) => Some(o),
                _ => None,
            };
            out.try_reserve(1).map_err(|_| MarkScanError::Allocation)?;
            out.push(Collected { preview: MarkPreview { page: pi, index: c.index, object: c.object, areas, locator }, held });
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::Cell;

    thread_local! {
        static CANDIDATES: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn count_candidate() {
        CANDIDATES.with(|c| c.set(c.get().saturating_add(1)));
    }

    /// Candidates `f` kept.
    pub(crate) fn candidates<T>(f: impl FnOnce() -> T) -> (T, usize) {
        CANDIDATES.with(|c| c.set(0));
        let out = f();
        (out, CANDIDATES.with(Cell::get))
    }
}
