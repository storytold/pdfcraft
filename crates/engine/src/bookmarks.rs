//! The bookmark list.

/// A bookmark as the bookmark list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct BookmarkRow {
    /// Child indices from the top level, 0-based, as the edit functions take them.
    pub path: Vec<usize>,
    /// The item's object reference: its identity in the document. A path moves
    /// when a bookmark is added or removed above it, a page operation that keeps
    /// the pages keeps every item where it is; the reference is what a
    /// collaborative operation addresses (`docs/collab/RFC.md` §3.1.7).
    pub object: Option<(u32, u16)>,
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
            // The viewer's copy carries no object reference: an outline read
            // for display only cannot be addressed.
            object: None,
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

/// A bookmark of the document's own outline as one row: its path, the object it
/// is, its title and where it goes.
pub(crate) fn row_of(bookmark: pdfcraft_organize::Bookmark) -> BookmarkRow {
    let view = bookmark.view.map_or_else(Default::default, |(kind, numbers)| {
        let numbers: Vec<Option<f32>> = numbers.iter().map(|n| n.map(|n| n as f32)).collect();
        pdfcraft_render::DestView::from_operands(Some(&kind), &numbers)
    });
    BookmarkRow {
        path: bookmark.path,
        object: Some((bookmark.obj.num, bookmark.obj.generation)),
        title: bookmark.title,
        open: bookmark.open,
        page: bookmark.page,
        view,
        color: bookmark.color,
        italic: bookmark.italic,
        bold: bookmark.bold,
    }
}

/// Every bookmark of the document's own outline, in listing order. Empty when the
/// document's structure cannot be read for editing (a viewer's copy is not
/// addressable, so it is not offered here).
pub(crate) fn document_rows(cos: &pdfcraft_cos::Document) -> Vec<BookmarkRow> {
    pdfcraft_organize::bookmarks(cos).into_iter().map(row_of).collect()
}
