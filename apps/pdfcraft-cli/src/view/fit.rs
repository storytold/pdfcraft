//! How big a page is drawn: the render scale and the cells the image covers, and how many pages
//! share the window. Pure arithmetic, total over any input (zero, negative, NaN and infinite
//! sizes are errors, never panics).

/// What a page is fitted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fit {
    /// The whole page in the window.
    Page,
    /// The window's width; a page taller than the window runs past it.
    Width,
}

/// How far in pages are zoomed (`--zoom`, and the pager's `+` and `-`): out, more pages to a row,
/// in as many rows as fit; in from one page to a row, the page as wide as the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Zoom {
    /// Two pages to a row when each keeps most of its size ([`two_up_fits`]), otherwise one:
    /// decided again at every draw, until the reader zooms.
    Auto,
    /// One page, as wide as the window.
    Width,
    /// This many pages to a row, 1 to [`MAX_PER_ROW`].
    PerRow(u16),
}

impl Zoom {
    pub(crate) fn parse(s: &str) -> Result<Self, String> {
        match (s, s.parse::<u16>()) {
            ("auto", _) => Ok(Self::Auto),
            ("width", _) => Ok(Self::Width),
            (_, Ok(n @ 1..=MAX_PER_ROW)) => Ok(Self::PerRow(n)),
            (other, _) => Err(format!("view: --zoom {other:?}: use auto, width or a number of pages per row (1–{MAX_PER_ROW})")),
        }
    }

    /// One step in from `per_row` pages to a row (as last drawn): one fewer, and from one, the
    /// width. `None` at the width, as far in as it goes.
    pub(crate) fn zoomed_in(self, per_row: u16) -> Option<Self> {
        match self {
            Self::Width => None,
            _ if per_row <= 1 => Some(Self::Width),
            _ => Some(Self::PerRow(per_row.saturating_sub(1))),
        }
    }

    /// One step out from `per_row` pages to a row (as last drawn): one more, up to `most`.
    /// `None` at `most`, as far out as it goes.
    pub(crate) fn zoomed_out(self, per_row: u16, most: u16) -> Option<Self> {
        match self {
            Self::Width => Some(Self::PerRow(1)),
            _ if per_row >= most => None,
            _ => Some(Self::PerRow(per_row.saturating_add(1))),
        }
    }
}

/// A terminal cell's size in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cell {
    pub(crate) w: f32,
    pub(crate) h: f32,
}

/// The cell assumed when the terminal doesn't report its pixel size.
pub(crate) const FALLBACK_CELL: Cell = Cell { w: 10.0, h: 20.0 };

impl Cell {
    /// From a window's size in cells and in pixels; `None` when the terminal reports no pixels.
    pub(crate) fn from_window(cols: u16, rows: u16, xpixel: u16, ypixel: u16) -> Option<Self> {
        (cols > 0 && rows > 0 && xpixel > 0 && ypixel > 0)
            .then(|| Self { w: f32::from(xpixel) / f32::from(cols), h: f32::from(ypixel) / f32::from(rows) })
    }
}

/// The largest rendered side in pixels: a page fitted to a very large window stays a sane size.
pub(crate) const MAX_PIXELS: f32 = 8192.0;

/// The most pixels a page is rendered with, 16 MP: 64 MB of RGBA, which bounds what one page
/// costs however tall a document makes it (more than a 4K screen shows at once).
pub(crate) const MAX_AREA: f64 = 16.0e6;

/// Where a page goes and how big it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Layout {
    /// Render scale, in pixels per point.
    pub(crate) scale: f32,
    /// The cells the image covers, rounded up.
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}

/// Fit a `page_w` × `page_h` point page into `cols` × `rows` cells of `cell` pixels, never more
/// than `max_rows` cells tall or [`MAX_PIXELS`] on a side.
pub(crate) fn layout(page_w: f32, page_h: f32, cols: u16, rows: u16, cell: Cell, fit: Fit, max_rows: u16) -> Result<Layout, String> {
    let positive = |v: f32| v.is_finite() && v > 0.0;
    if !(positive(page_w) && positive(page_h)) {
        return Err("the page has no size".into());
    }
    if cols == 0 || rows == 0 || max_rows == 0 {
        return Err("the terminal reports no size".into());
    }
    if !(positive(cell.w) && positive(cell.h)) {
        return Err("the terminal reports no cell size".into());
    }
    let (area_w, area_h) = (f32::from(cols) * cell.w, f32::from(rows) * cell.h);
    let fitted = match fit {
        Fit::Page => (area_w / page_w).min(area_h / page_h),
        Fit::Width => area_w / page_w,
    };
    // In f64, where the area of any f32 page is finite.
    let by_area = (MAX_AREA / (f64::from(page_w) * f64::from(page_h))).sqrt() as f32;
    let scale = fitted.min(f32::from(max_rows) * cell.h / page_h).min(MAX_PIXELS / page_w).min(MAX_PIXELS / page_h).min(by_area);
    if !positive(scale) {
        return Err("the page is too large or too small to show".into());
    }
    // Rounding can put an exact fit one cell over; the clamps take it back. Both bounds are at
    // least 1 (checked above), so `clamp` can't panic, and the results are whole numbers in
    // u16's range, so the casts are exact.
    let row_limit = match fit {
        Fit::Page => rows.min(max_rows),
        Fit::Width => max_rows,
    };
    let image_cols = (page_w * scale / cell.w).ceil().clamp(1.0, f32::from(cols));
    let image_rows = (page_h * scale / cell.h).ceil().clamp(1.0, f32::from(row_limit));
    Ok(Layout { scale, cols: image_cols as u16, rows: image_rows as u16 })
}

/// The cells between two pages side by side.
pub(crate) const GAP: u16 = 2;

/// The cell rows between two rows of pages.
pub(crate) const ROW_GAP: u16 = 1;

/// The most pages to a row.
pub(crate) const MAX_PER_ROW: u16 = 8;

/// The fewest columns a page gets in a row: zooming out stops before pages get narrower.
pub(crate) const MIN_PAGE_COLS: u16 = 8;

/// The most pages on screen at once, however small they are (a strip of wide pages in a tall
/// window would make many rows).
pub(crate) const MAX_ON_SCREEN: u16 = 48;

/// How much of its size a page may give up to share the window with the next one.
pub(crate) const TWO_UP_SHARE: f32 = 0.8;

/// The columns each of `per_row` pages side by side gets in a window `cols` wide.
pub(crate) fn per_row_cols(cols: u16, per_row: u16) -> u16 {
    let per_row = per_row.max(1);
    cols.saturating_sub(GAP.saturating_mul(per_row.saturating_sub(1))) / per_row
}

/// The most pages to a row in a window `cols` wide: each at least [`MIN_PAGE_COLS`] wide, and
/// never more than [`MAX_PER_ROW`]. At least 1, however narrow the window.
pub(crate) fn max_per_row(cols: u16) -> u16 {
    // n pages and their gaps fit when n × (MIN_PAGE_COLS + GAP) ≤ cols + GAP.
    let n = (u32::from(cols) + u32::from(GAP)) / u32::from(MIN_PAGE_COLS + GAP);
    u16::try_from(n).unwrap_or(u16::MAX).clamp(1, MAX_PER_ROW)
}

/// How many rows of pages `page_rows` tall fit in `area_rows`, [`ROW_GAP`] between them: at
/// least 1, and few enough that `per_row` to a row stay within [`MAX_ON_SCREEN`].
pub(crate) fn grid_rows(page_rows: u16, area_rows: u16, per_row: u16) -> u16 {
    let n = (u32::from(area_rows) + u32::from(ROW_GAP)) / (u32::from(page_rows) + u32::from(ROW_GAP)).max(1);
    u16::try_from(n).unwrap_or(u16::MAX).min(MAX_ON_SCREEN / per_row.max(1)).max(1)
}

/// Whether two `page_w` × `page_h` pages go side by side in `cols` × `rows` cells: each, fitted
/// to its half, keeps at least [`TWO_UP_SHARE`] of the size it would have alone. Portrait pages
/// in a wide window do; anything in a tall window, and landscape pages, mostly don't.
pub(crate) fn two_up_fits(page_w: f32, page_h: f32, cols: u16, rows: u16, cell: Cell, max_rows: u16) -> bool {
    match (
        layout(page_w, page_h, cols, rows, cell, Fit::Page, max_rows),
        layout(page_w, page_h, per_row_cols(cols, 2), rows, cell, Fit::Page, max_rows),
    ) {
        (Ok(alone), Ok(beside)) => beside.scale >= alone.scale * TWO_UP_SHARE,
        _ => false,
    }
}

/// The cells pages go into: `cols` × `rows` of `cell` pixels, none of them more than `max_rows`
/// tall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Area {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) cell: Cell,
    pub(crate) max_rows: u16,
}

/// How pages share an [`Area`]: `per_row` to a row in `rows` rows, each fitted into a place
/// `cols` × `page_rows` cells and centred in it (at the width, a taller page scrolls), and the
/// most pages to a row the area takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Grid {
    pub(crate) fit: Fit,
    pub(crate) per_row: u16,
    pub(crate) rows: u16,
    pub(crate) cols: u16,
    pub(crate) page_rows: u16,
    pub(crate) most: u16,
}

/// How `count` pages share `area` at `zoom`, for a reader at `at` among them (`size` is the size
/// of the page at a place among them): as zoomed, as far as the area takes. Out from the width,
/// the page at the start of the reader's row sets the place every page is fitted into, and as
/// many rows of those as fit go one under another: pages never shrink to make room for another
/// row.
pub(crate) fn grid(zoom: Zoom, count: usize, at: usize, size: impl Fn(usize) -> Option<(f32, f32)>, area: Area) -> Grid {
    let Area { cols: max_cols, rows: area_rows, cell, max_rows } = area;
    let most = max_per_row(max_cols).min(u16::try_from(count).unwrap_or(u16::MAX)).max(1);
    let per_row = match zoom {
        Zoom::Width => return Grid { fit: Fit::Width, per_row: 1, rows: 1, cols: max_cols, page_rows: area_rows, most },
        // `most` is at least 1, so the clamp can't panic.
        Zoom::PerRow(n) => n.clamp(1, most),
        // Two side by side when each keeps most of its size alone: the spread's first page
        // decides.
        Zoom::Auto => {
            let spread_first = size(at.saturating_sub(at % 2));
            let two = most >= 2 && spread_first.is_some_and(|(w, h)| two_up_fits(w, h, max_cols, area_rows, cell, max_rows));
            if two { 2 } else { 1 }
        }
    };
    let cols = per_row_cols(max_cols, per_row);
    // `per_row` is at least 1 either way.
    let row_start = size(at.saturating_sub(at % usize::from(per_row)));
    let place = row_start.and_then(|(w, h)| layout(w, h, cols, area_rows, cell, Fit::Page, max_rows).ok());
    let (cols, page_rows) = place.map_or((cols, area_rows), |l| (l.cols, l.rows));
    Grid { fit: Fit::Page, per_row, rows: grid_rows(page_rows, area_rows, per_row), cols, page_rows, most }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const LETTER: (f32, f32) = (612.0, 792.0);
    const CELL: Cell = Cell { w: 10.0, h: 20.0 };

    #[test]
    fn a_portrait_page_fits_the_height_of_a_wide_window() {
        let l = layout(LETTER.0, LETTER.1, 200, 50, CELL, Fit::Page, u16::MAX).unwrap();
        assert_eq!(l.scale, 1000.0 / 792.0);
        assert_eq!((l.cols, l.rows), (78, 50));
    }

    #[test]
    fn fit_width_fills_the_width_and_runs_past_the_height() {
        let l = layout(LETTER.0, LETTER.1, 100, 30, CELL, Fit::Width, u16::MAX).unwrap();
        assert_eq!(l.scale, 1000.0 / 612.0);
        assert_eq!((l.cols, l.rows), (100, 65));
    }

    #[test]
    fn max_rows_and_max_pixels_cap_the_size() {
        let l = layout(LETTER.0, LETTER.1, 100, 30, CELL, Fit::Width, 40).unwrap();
        assert_eq!(l.rows, 40);
        assert!(l.cols < 100);
        let huge = layout(LETTER.0, LETTER.1, 4000, 4000, CELL, Fit::Page, u16::MAX).unwrap();
        assert!(LETTER.1 * huge.scale <= MAX_PIXELS * 1.0001);
        // A 1000 × 4000 pt page fitted to a 6000 px wide window: 8000 px tall is within the side
        // cap, but 6000 × 24000 px is not within the area cap, which brings it to 2000 × 8000.
        let strip = layout(1000.0, 4000.0, 600, 50, CELL, Fit::Width, u16::MAX).unwrap();
        let area = f64::from(1000.0 * strip.scale) * f64::from(4000.0 * strip.scale);
        assert!(area <= MAX_AREA * 1.0001 && area > MAX_AREA * 0.99, "{area}");
    }

    #[test]
    fn the_cell_comes_from_the_window() {
        assert_eq!(Cell::from_window(100, 50, 1000, 1000), Some(Cell { w: 10.0, h: 20.0 }));
        assert_eq!(Cell::from_window(100, 50, 0, 0), None);
        assert_eq!(Cell::from_window(0, 0, 1000, 1000), None);
    }

    #[test]
    fn sizes_that_cannot_be_shown_are_errors() {
        for (w, h) in [(0.0, 792.0), (-1.0, 792.0), (f32::NAN, 792.0), (612.0, f32::INFINITY)] {
            assert!(layout(w, h, 80, 24, CELL, Fit::Page, u16::MAX).is_err(), "{w}×{h}");
        }
        assert!(layout(612.0, 792.0, 0, 24, CELL, Fit::Page, u16::MAX).is_err());
        assert!(layout(612.0, 792.0, 80, 0, CELL, Fit::Page, u16::MAX).is_err());
        assert!(layout(612.0, 792.0, 80, 24, CELL, Fit::Page, 0).is_err());
        assert!(layout(612.0, 792.0, 80, 24, Cell { w: 0.0, h: 20.0 }, Fit::Page, u16::MAX).is_err());
        // A vanishingly small page needs an infinite scale.
        assert!(layout(1e-38, 1e-38, 80, 24, CELL, Fit::Page, u16::MAX).is_err());
        // A huge one gets a tiny scale: the square page fills the 480 px height.
        let l = layout(f32::MAX, f32::MAX, 80, 24, CELL, Fit::Page, u16::MAX).unwrap();
        assert_eq!(l.rows, 24);
        assert!((48..=49).contains(&l.cols), "{l:?}");
    }

    #[test]
    fn two_portrait_pages_share_a_wide_window() {
        // 200 × 50 cells of 10 × 20 px: 2000 × 1000 px. A Letter page is as tall alone as beside
        // another.
        assert!(two_up_fits(LETTER.0, LETTER.1, 200, 50, CELL, u16::MAX));
        // 80 × 24 (800 × 480 px) is wider than tall, too.
        assert!(two_up_fits(LETTER.0, LETTER.1, 80, 23, CELL, u16::MAX));
        // A tall window: beside another, a page would be half the size.
        assert!(!two_up_fits(LETTER.0, LETTER.1, 80, 60, CELL, u16::MAX));
        // Landscape pages fill a wide window's width on their own.
        assert!(!two_up_fits(LETTER.1, LETTER.0, 200, 50, CELL, u16::MAX));
        // Nothing to split, or nothing to show.
        assert!(!two_up_fits(LETTER.0, LETTER.1, 2, 50, CELL, u16::MAX));
        assert!(!two_up_fits(0.0, LETTER.1, 200, 50, CELL, u16::MAX));
    }

    #[test]
    fn zoom_names() {
        assert_eq!(Zoom::parse("auto"), Ok(Zoom::Auto));
        assert_eq!(Zoom::parse("width"), Ok(Zoom::Width));
        assert_eq!(Zoom::parse("1"), Ok(Zoom::PerRow(1)));
        assert_eq!(Zoom::parse("8"), Ok(Zoom::PerRow(MAX_PER_ROW)));
        for wrong in ["0", "9", "65536", "-1", "page", ""] {
            assert!(Zoom::parse(wrong).is_err_and(|e| e.contains("use auto, width or a number of pages per row (1–8)")), "{wrong:?}");
        }
    }

    #[test]
    fn zooming_steps_between_the_width_and_the_most_pages_to_a_row() {
        // In: one page fewer to a row, from one page the width, and no further.
        assert_eq!(Zoom::PerRow(3).zoomed_in(3), Some(Zoom::PerRow(2)));
        assert_eq!(Zoom::PerRow(1).zoomed_in(1), Some(Zoom::Width));
        assert_eq!(Zoom::Width.zoomed_in(1), None);
        // Out: one more, up to the most the window takes, and from the width one page.
        assert_eq!(Zoom::PerRow(2).zoomed_out(2, 8), Some(Zoom::PerRow(3)));
        assert_eq!(Zoom::PerRow(8).zoomed_out(8, 8), None);
        assert_eq!(Zoom::Width.zoomed_out(1, 8), Some(Zoom::PerRow(1)));
        // From what was drawn: Auto showed two, and a window too narrow for the four asked
        // showed three.
        assert_eq!(Zoom::Auto.zoomed_in(2), Some(Zoom::PerRow(1)));
        assert_eq!(Zoom::Auto.zoomed_out(2, 8), Some(Zoom::PerRow(3)));
        assert_eq!(Zoom::PerRow(4).zoomed_in(3), Some(Zoom::PerRow(2)));
        assert_eq!(Zoom::PerRow(4).zoomed_out(3, 3), None);
    }

    #[test]
    fn pages_to_a_row_share_the_width_and_rows_of_them_the_height() {
        assert_eq!((per_row_cols(200, 1), per_row_cols(200, 2), per_row_cols(200, 3)), (200, 99, 65));
        assert_eq!((per_row_cols(1, 2), per_row_cols(0, 3), per_row_cols(80, 0)), (0, 0, 80));
        // 8 columns a page and 2 between: 3 pages take 28 columns.
        assert_eq!((max_per_row(27), max_per_row(28), max_per_row(1000), max_per_row(0)), (2, 3, MAX_PER_ROW, 1));
        // Pages 10 rows tall, a row between: 4 rows of them in 43 rows, and 3 in 42.
        assert_eq!((grid_rows(10, 43, 1), grid_rows(10, 42, 1)), (4, 3));
        // A page taller than the area still gets a row; tiny ones stop at MAX_ON_SCREEN.
        assert_eq!((grid_rows(50, 20, 1), grid_rows(0, 0, 0)), (1, 1));
        assert_eq!((grid_rows(1, 1000, 1), grid_rows(1, 1000, 8), grid_rows(1, 1000, 100)), (48, 6, 1));
    }

    #[test]
    fn a_grid_is_the_zoom_as_far_as_the_area_takes() {
        let wide = Area { cols: 200, rows: 50, cell: CELL, max_rows: u16::MAX };
        let letter = |_| Some(LETTER);
        // Two portrait pages side by side in a wide window, each the size it is alone; one in a
        // tall window, where two would be half the size.
        let two = grid(Zoom::Auto, 10, 0, letter, wide);
        assert_eq!(two, Grid { fit: Fit::Page, per_row: 2, rows: 1, cols: 78, page_rows: 50, most: MAX_PER_ROW });
        assert_eq!(grid(Zoom::Auto, 10, 0, letter, Area { cols: 80, rows: 100, ..wide }).per_row, 1);
        // Or one when there is only one.
        assert_eq!(grid(Zoom::Auto, 1, 0, letter, wide).per_row, 1);
        // N to a row as far as the window takes them, and never more than there are pages.
        assert_eq!(grid(Zoom::PerRow(8), 10, 0, letter, Area { cols: 40, ..wide }).per_row, 4);
        assert_eq!(grid(Zoom::PerRow(8), 3, 0, letter, wide).per_row, 3);
        // The width: one page, as wide as the window, and the window's rows to scroll it in.
        assert_eq!(grid(Zoom::Width, 10, 4, letter, wide), Grid { fit: Fit::Width, per_row: 1, rows: 1, cols: 200, page_rows: 50, most: 8 });
        // The page at the start of the reader's row sets the place: landscape page 7 starts the
        // row of pages 7–9, at three to a row.
        let mixed = |i: usize| Some(if i == 6 { (LETTER.1, LETTER.0) } else { LETTER });
        let (landscape, portrait) = (grid(Zoom::PerRow(3), 10, 7, mixed, wide), grid(Zoom::PerRow(3), 10, 4, mixed, wide));
        assert_eq!((landscape.cols, portrait.cols), (65, 65));
        assert!(landscape.page_rows < portrait.page_rows, "{landscape:?} {portrait:?}");
        // No page to size the place by: the whole of each page's share.
        assert_eq!(grid(Zoom::PerRow(2), 0, 0, |_| None, wide), Grid { fit: Fit::Page, per_row: 1, rows: 1, cols: 200, page_rows: 50, most: 1 });
    }

    fn fit() -> impl Strategy<Value = Fit> {
        prop_oneof![Just(Fit::Page), Just(Fit::Width)]
    }

    fn zoom() -> impl Strategy<Value = Zoom> {
        prop_oneof![Just(Zoom::Auto), Just(Zoom::Width), (1..=MAX_PER_ROW).prop_map(Zoom::PerRow)]
    }

    proptest! {
        /// Within one cell of the page's aspect ratio, inside the window (or its width, for
        /// `width`), never past `max_rows` or `MAX_PIXELS`.
        #[test]
        fn a_layout_fits(w in 1f32..20_000.0, h in 1f32..20_000.0, cols in 1u16..500, rows in 1u16..300,
                         cw in 2f32..40.0, ch in 4f32..80.0, max_rows in 1u16..400, fit in fit()) {
            let cell = Cell { w: cw, h: ch };
            let l = layout(w, h, cols, rows, cell, fit, max_rows).unwrap();
            let (px_w, px_h) = (w * l.scale, h * l.scale);
            prop_assert!(px_w <= MAX_PIXELS * 1.001 && px_h <= MAX_PIXELS * 1.001);
            prop_assert!(f64::from(px_w) * f64::from(px_h) <= MAX_AREA * 1.001);
            prop_assert!(l.cols >= 1 && l.cols <= cols && l.rows >= 1 && l.rows <= max_rows);
            if fit == Fit::Page {
                prop_assert!(l.rows <= rows);
            }
            // The image covers its cells, and at most one more cell than it needs each way.
            let slack = 1.001;
            prop_assert!(px_w <= f32::from(l.cols) * cw * slack, "{px_w} > {} cols", l.cols);
            prop_assert!(px_h <= f32::from(l.rows) * ch * slack, "{px_h} > {} rows", l.rows);
            prop_assert!(l.cols == 1 || px_w * slack > f32::from(l.cols - 1) * cw);
            prop_assert!(l.rows == 1 || px_h * slack > f32::from(l.rows - 1) * ch);
        }

        /// Pages to a row and the gaps between them fit the window's width, and rows of them its
        /// height, whenever a page fits at all; never more than the limits.
        #[test]
        fn rows_and_columns_of_pages_fit(cols in any::<u16>(), per_row in any::<u16>(), page_rows in any::<u16>(), area_rows in any::<u16>()) {
            let n = per_row.max(1);
            let page_cols = per_row_cols(cols, per_row);
            let width = u32::from(page_cols) * u32::from(n) + u32::from(GAP) * u32::from(n - 1);
            prop_assert!(page_cols == 0 || width <= u32::from(cols));
            let most = max_per_row(cols);
            prop_assert!((1..=MAX_PER_ROW).contains(&most));
            prop_assert!(most == 1 || per_row_cols(cols, most) >= MIN_PAGE_COLS);
            let rows = grid_rows(page_rows, area_rows, per_row);
            prop_assert!(rows >= 1 && u32::from(rows) * u32::from(n) <= u32::from(MAX_ON_SCREEN).max(u32::from(n)));
            let height = u32::from(rows) * u32::from(page_rows) + u32::from(ROW_GAP) * u32::from(rows - 1);
            prop_assert!(rows == 1 || height <= u32::from(area_rows));
        }

        /// However zoomed, wherever the reader is and whatever the pages' sizes, a grid's places
        /// fit their share of the area, and its pages to a row are within the most it takes.
        #[test]
        fn a_grid_fits_its_area(zoom in zoom(), count in 0usize..100, at in 0usize..100, sizes in proptest::collection::vec((any::<f32>(), any::<f32>()), 0..100),
                                cols in any::<u16>(), rows in any::<u16>(), max_rows in any::<u16>()) {
            let area = Area { cols, rows, cell: CELL, max_rows };
            let g = grid(zoom, count, at, |i| sizes.get(i).copied(), area);
            prop_assert!(g.per_row >= 1 && g.per_row <= g.most && g.most <= MAX_PER_ROW && g.rows >= 1, "{g:?}");
            prop_assert!(usize::from(g.most) <= count.max(1));
            match g.fit {
                Fit::Width => prop_assert_eq!((g.per_row, g.cols, g.page_rows), (1, cols, rows)),
                Fit::Page => prop_assert!(g.cols <= per_row_cols(cols, g.per_row) && g.page_rows <= rows, "{g:?}"),
            }
        }

        #[test]
        fn any_numbers_are_an_answer(w in any::<f32>(), h in any::<f32>(), cols in any::<u16>(), rows in any::<u16>(),
                                     cw in any::<f32>(), ch in any::<f32>(), max_rows in any::<u16>(), fit in fit()) {
            if let Ok(l) = layout(w, h, cols, rows, Cell { w: cw, h: ch }, fit, max_rows) {
                prop_assert!(l.scale.is_finite() && l.scale > 0.0);
                prop_assert!(l.cols >= 1 && l.cols <= cols && l.rows >= 1 && l.rows <= max_rows);
            }
        }
    }
}
