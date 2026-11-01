//! Page filters: even or odd page numbers, and orientation as displayed.

/// Page filters of Acrobat's Rotate Pages and page selection: even/odd page numbers and
/// orientation (as displayed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageParity {
    #[default]
    Both,
    Even,
    Odd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageOrientation {
    #[default]
    Both,
    Landscape,
    Portrait,
}

/// The pages of `pages` (0-based) that pass the filters.
pub fn filter_pages(info: &pdfcraft_render::DocInfo, pages: &[usize], parity: PageParity, orientation: PageOrientation) -> Vec<usize> {
    pages
        .iter()
        .copied()
        .filter(|p| match parity {
            PageParity::Both => true,
            // Page numbers are 1-based: index 1 is page 2, an even page.
            PageParity::Even => p % 2 == 1,
            PageParity::Odd => p % 2 == 0,
        })
        .filter(|p| {
            let Some(pg) = info.pages.get(*p) else { return false };
            match orientation {
                PageOrientation::Both => true,
                PageOrientation::Landscape => pg.width > pg.height,
                PageOrientation::Portrait => pg.width <= pg.height,
            }
        })
        .collect()
}
