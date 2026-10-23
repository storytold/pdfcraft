//! How big a page is drawn: the render scale and the cells the image covers. Pure arithmetic,
//! total over any input (zero, negative, NaN and infinite sizes are errors, never panics).

/// What a page is fitted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fit {
    /// The whole page in the window.
    Page,
    /// The window's width; a page taller than the window runs past it.
    Width,
}

impl Fit {
    pub(crate) fn parse(s: &str) -> Result<Self, String> {
        match s {
            "page" => Ok(Self::Page),
            "width" => Ok(Self::Width),
            other => Err(format!("view: --fit {other:?}: use page or width")),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Width => "width",
        }
    }

    /// The other one: the pager's `f` key.
    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Page => Self::Width,
            Self::Width => Self::Page,
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
pub(crate) const TWO_UP_GAP: u16 = 2;

/// How much of its size a page may give up to share the window with the next one.
pub(crate) const TWO_UP_SHARE: f32 = 0.8;

/// The columns each of two pages side by side gets in a window `cols` wide.
pub(crate) fn two_up_cols(cols: u16) -> u16 {
    cols.saturating_sub(TWO_UP_GAP) / 2
}

/// Whether two `page_w` × `page_h` pages go side by side in `cols` × `rows` cells: each, fitted
/// to its half, keeps at least [`TWO_UP_SHARE`] of the size it would have alone. Portrait pages
/// in a wide window do; anything in a tall window, and landscape pages, mostly don't.
pub(crate) fn two_up_fits(page_w: f32, page_h: f32, cols: u16, rows: u16, cell: Cell, max_rows: u16) -> bool {
    match (layout(page_w, page_h, cols, rows, cell, Fit::Page, max_rows), layout(page_w, page_h, two_up_cols(cols), rows, cell, Fit::Page, max_rows))
    {
        (Ok(alone), Ok(beside)) => beside.scale >= alone.scale * TWO_UP_SHARE,
        _ => false,
    }
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
        assert_eq!((two_up_cols(200), two_up_cols(1), two_up_cols(0)), (99, 0, 0));
    }

    fn fit() -> impl Strategy<Value = Fit> {
        prop_oneof![Just(Fit::Page), Just(Fit::Width)]
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
