//! The bookmark list.

/// A bookmark as the bookmark list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct BookmarkRow {
    /// Child indices from the top level, 0-based, as the edit functions take them.
    pub path: Vec<usize>,
    pub title: String,
    /// Shown expanded (a positive `/Count`).
    pub open: bool,
    /// The 0-based page it goes to.
    pub page: Option<usize>,
    /// Where on that page it goes ([`pdfcraft_render::DestView::Top`] when it doesn't say).
    pub view: pdfcraft_render::DestView,
    /// The title's colour (`/C`), `None` for the default black, and its style (`/F`).
    pub color: Option<[f32; 3]>,
    pub italic: bool,
    pub bold: bool,
}

/// One page of the bookmark list, in outline order.
#[derive(Clone, Debug, PartialEq)]
pub struct BookmarkPage {
    pub rows: Vec<BookmarkRow>,
    /// Where the next page starts, or `None` at the end.
    pub next: Option<usize>,
    /// Set only when the document's structure could not be read for editing: its listing then
    /// comes from the viewer's copy of the outline, which is shorter than the file's.
    pub truncated: bool,
}

/// The rows of a viewer outline, in outline order, with their paths.
pub(crate) fn outline_rows(items: &[pdfcraft_render::OutlineItem]) -> Vec<BookmarkRow> {
    let mut rows = Vec::new();
    // One entry per open level: its items, and how many of them have been listed.
    let mut levels: Vec<(&[pdfcraft_render::OutlineItem], usize)> = vec![(items, 0)];
    while let Some(&(list, next)) = levels.last() {
        let Some(item) = list.get(next) else {
            levels.pop();
            continue;
        };
        if let Some(top) = levels.last_mut() {
            top.1 += 1;
        }
        let path: Vec<usize> = levels.iter().map(|(_, listed)| listed.saturating_sub(1)).collect();
        rows.push(BookmarkRow {
            path,
            title: item.title.clone(),
            open: item.open,
            page: item.page,
            view: item.view,
            color: item.color,
            italic: item.italic,
            bold: item.bold,
        });
        if !item.children.is_empty() {
            levels.push((&item.children, 0));
        }
    }
    rows
}

/// Vertical fraction `0.0..=1.0` (`0` = displayed top, `1` = displayed bottom) of an outline
/// item's destination on `page` at clockwise `view_rotation`.
pub fn view_top_fraction(info: &pdfcraft_render::DocInfo, page: usize, view: &pdfcraft_render::DestView, view_rotation: u16) -> f32 {
    use pdfcraft_render::DestView;
    let Some(p) = info.pages.get(page) else { return 0.0 };
    let (x, y) = match *view {
        DestView::Top | DestView::Fit => return 0.0,
        DestView::Xyz { left, top, .. } => (left, top),
        DestView::FitH { top } => (None, top),
        DestView::FitV { left } => (left, None),
        DestView::FitR { rect } => (Some(rect[0]), Some(rect[3])),
    };
    p.dest_fraction(x, y, view_rotation)[1].unwrap_or(0.0)
}
