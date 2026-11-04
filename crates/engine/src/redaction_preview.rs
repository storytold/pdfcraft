//! What the interface shows of redaction marks, read from the editable graph by the same
//! collector applying reads ([`pdfcraft_redact::mark_previews`]), never from the renderer's
//! annotation list (whose quads read a bad number as 0).
//!
//! The view is rebuilt with the display copy, on every change to the working graph; anything that
//! replaces the graph without rebuilding it leaves the view [`RedactionPreviews::Unavailable`], and
//! applying is refused while it is.

pub use pdfcraft_redact::{MarkAreaError, MarkAreas, MarkPreview, MarkRefusal, MarkScanError, QuadIssues};
use pdfcraft_render::DocInfo;

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
    /// The page shows a redaction mark that applying wouldn't read.
    #[error("a redaction mark is shown that the marks applying reads don't include")]
    UnlistedMark,
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

/// The checked marks of `cos`, provided its pages are the pages shown (`info`): the same page
/// objects in the same order as the renderer walks them (equal sizes don't prove that: a tree can
/// list one page twice, or a node the renderer takes for a page), each with the same rotation and
/// visible box; and provided every Redact annotation the renderer lists is among them.
pub(crate) fn previews_for(cos: &pdfcraft_cos::Document, info: &DocInfo) -> RedactionPreviews {
    let model = pdfcraft_model::pages(cos);
    let pages = &info.pages;
    let same_objects = model.len() == info.page_objects.len()
        && model.iter().zip(&info.page_objects).all(|(m, shown)| *shown == Some((m.obj.num, m.obj.generation)));
    let same = same_objects
        && model.len() == pages.len()
        && model.iter().zip(pages).all(|(m, p)| {
            let crop = m.crop(cos);
            let close = crop.iter().zip(p.crop.iter()).all(|(a, b)| (a - f64::from(*b)).abs() <= 0.5);
            close && m.rotation(cos) == i64::from(p.rotation)
        });
    if !same {
        return RedactionPreviews::Unavailable(PreviewUnavailable::PageMapping);
    }
    let list = match pdfcraft_redact::mark_previews(cos, None) {
        Ok(list) => list,
        Err(e) => return RedactionPreviews::Unavailable(PreviewUnavailable::Scan(e)),
    };
    // A mark the page shows that the collector doesn't know would be shown and not applied.
    let listed = |page: usize, index: usize| list.binary_search_by_key(&(page, index), |p| (p.page, p.index)).is_ok();
    if info.annotations.iter().any(|a| a.subtype == "Redact" && !listed(a.page, a.index)) {
        return RedactionPreviews::Unavailable(PreviewUnavailable::UnlistedMark);
    }
    RedactionPreviews::Available(list)
}

/// Draw every checked mark in `display` (a display-only copy of the working graph) exactly as
/// the view shows it: ready marks as the outline of their areas, refused ones as a warning
/// locator or not at all ([`pdfcraft_annot::redaction::show_in_display`]). Whatever appearance a
/// mark carries in the file is not what the page shows. Nothing happens while the view is
/// unavailable (applying is refused then).
pub(crate) fn show_redactions(display: &mut pdfcraft_cos::Document, previews: &RedactionPreviews) -> Result<(), String> {
    #[cfg(test)]
    if FAIL_DISPLAY.with(std::cell::Cell::get) {
        return Err("injected display failure".into());
    }
    let RedactionPreviews::Available(list) = previews else { return Ok(()) };
    if list.is_empty() {
        return Ok(());
    }
    // The page tree is walked once, and each page with marks is drawn once.
    let pages = pdfcraft_model::pages(display);
    for marks in list.chunk_by(|a, b| a.page == b.page) {
        let Some(page) = marks.first().map(|p| p.page) else { continue };
        let page_obj = pages.get(page).map(|p| p.obj).ok_or_else(|| format!("page {} is missing from the display copy", page + 1))?;
        let shown: Vec<(usize, pdfcraft_annot::redaction::Shown<'_>)> = marks
            .iter()
            .map(|p| {
                let shown = match &p.areas {
                    Ok(a) => pdfcraft_annot::redaction::Shown::Ready(&a.rects),
                    Err(_) => pdfcraft_annot::redaction::Shown::Refused(p.locator),
                };
                (p.index, shown)
            })
            .collect();
        pdfcraft_annot::redaction::show_page_in_display(display, page, page_obj, &shown).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Makes [`show_redactions`] fail, as a display copy that can't be drawn would.
    pub(crate) static FAIL_DISPLAY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl crate::Document {
    /// Draw the checked redaction marks into `copy`, a page-bearing copy of the working graph made
    /// for drawing (a drag's background, a text edit's backdrop, a print), as the page view draws
    /// them, so no picture of the page shows a mark otherwise. Call it before any edit to the copy
    /// that moves `/Annots` slots. While the view is unavailable and the page lists Redact marks,
    /// no checked drawing exists to give: an error. Any failure also refuses applying until the
    /// next refresh draws the display again.
    pub(crate) fn draw_checked_marks(&self, copy: &mut pdfcraft_cos::Document) -> Result<(), String> {
        let drawn = match &self.redaction_previews {
            RedactionPreviews::Available(list) if list.is_empty() => return Ok(()),
            view @ RedactionPreviews::Available(_) => show_redactions(copy, view),
            RedactionPreviews::Unavailable(why) if self.info.annotations.iter().any(|a| a.subtype == "Redact") => {
                Err(format!("the redaction marks can't be drawn as they would be applied ({why})"))
            }
            RedactionPreviews::Unavailable(_) => return Ok(()),
        };
        if drawn.is_err() {
            self.preview_copy_failed.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        drawn
    }
}
