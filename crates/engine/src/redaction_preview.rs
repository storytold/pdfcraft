//! What the interface shows of redaction marks, read from the editable graph by the same
//! collector applying reads ([`pdfcraft_redact::mark_previews`]), never from the renderer's
//! annotation list (whose quads read a bad number as 0).
//!
//! The view is rebuilt with the display copy, on every change to the working graph; anything that
//! replaces the graph without rebuilding it leaves the view [`RedactionPreviews::Unavailable`], and
//! applying is refused while it is.

pub use pdfcraft_redact::{MarkAreaError, MarkAreas, MarkPreview, MarkRefusal, MarkScanError, QuadIssues};
use pdfcraft_render::PageInfo;

/// The checked redaction marks of the current graph, or why there are none to trust.
#[derive(Clone, Debug, PartialEq)]
pub enum RedactionPreviews {
    /// Every Redact annotation the pages list, in page and `/Annots` order, ready or refused.
    Available(Vec<MarkPreview>),
    Unavailable(PreviewUnavailable),
}

/// Why the checked marks can't be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PreviewUnavailable {
    /// The document has no editable graph (opened read-only or not parsed).
    #[error("the document can't be read for editing")]
    NoEditableGraph,
    /// The view is being rebuilt, or a rebuild failed.
    #[error("the document's view is being rebuilt")]
    Refreshing,
    /// The marks could not all be checked.
    #[error("{0}")]
    Scan(MarkScanError),
    /// The editable graph and the displayed pages don't list the same pages.
    #[error("the pages read for editing don't match the pages shown")]
    PageMapping,
    /// The display copy could not be written, so the page shows the file's own appearances.
    #[error("the marks could not be drawn as they would be applied")]
    Display,
}

impl RedactionPreviews {
    /// The checked mark at `(page, index)` (`index` its `/Annots` slot), if the view is
    /// available and lists one there.
    pub fn get(&self, page: usize, index: usize) -> Option<&MarkPreview> {
        let Self::Available(list) = self else { return None };
        list.binary_search_by_key(&(page, index), |p| (p.page, p.index)).ok().and_then(|i| list.get(i))
    }

    /// The checked marks of `page`, in `/Annots` order (none while unavailable).
    pub fn on_page(&self, page: usize) -> &[MarkPreview] {
        let Self::Available(list) = self else { return &[] };
        let start = list.partition_point(|p| p.page < page);
        let end = list.partition_point(|p| p.page <= page);
        list.get(start..end).unwrap_or(&[])
    }

    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    /// Marks that can't be applied (0 while unavailable).
    pub fn refused(&self) -> usize {
        match self {
            Self::Available(list) => list.iter().filter(|p| p.areas.is_err()).count(),
            Self::Unavailable(_) => 0,
        }
    }
}

/// The checked marks of `cos`, provided its pages are the pages shown (`pages`): same count, and
/// each page with the same rotation and visible box. Equal counts alone don't show the same order.
pub(crate) fn previews_for(cos: &pdfcraft_cos::Document, pages: &[PageInfo]) -> RedactionPreviews {
    let model = pdfcraft_model::pages(cos);
    let same = model.len() == pages.len()
        && model.iter().zip(pages).all(|(m, p)| {
            let crop = m.crop(cos);
            let close = crop.iter().zip(p.crop.iter()).all(|(a, b)| (a - f64::from(*b)).abs() <= 0.5);
            close && m.rotation(cos) == i64::from(p.rotation)
        });
    if !same {
        return RedactionPreviews::Unavailable(PreviewUnavailable::PageMapping);
    }
    match pdfcraft_redact::mark_previews(cos, None) {
        Ok(list) => RedactionPreviews::Available(list),
        Err(e) => RedactionPreviews::Unavailable(PreviewUnavailable::Scan(e)),
    }
}
