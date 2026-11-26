//! Page Layout view geometry: a sheet shown as printed pages side by side.
//!
//! The view uses the print pagination ([`crate::cmd::print::split_bands`]) over the sheet from
//! A1, so its page edges are the printed ones whenever the print area starts at A1. Pages keep
//! going right and down past the data ("Click to add data" pages), as far as the view needs.
//!
//! Coordinates are *layout units*: sheet pixels (96 dpi) at the print scale, so a cell keeps its
//! sheet size and a page is `paper / scale` big. The UI multiplies by its zoom. Pages sit in a
//! grid with a gap around each: page `(row band, column band)` starts at
//! `(gap + j·(page_w + gap), gap + i·(page_h + gap))`, and its cells start at its top-left margin.

use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef};
use gridcraft_model::{LineIndex, PageOrder, Sheet, Workbook};

use crate::cmd::print::{self, PX_TO_PT, PrintOptions};

/// Gap between pages on screen, in pixels at 100% zoom.
pub const PAGE_GAP: f32 = 14.0;

/// One sheet paginated for the Page Layout view.
#[derive(Clone, Debug)]
pub struct PageGrid {
    /// Page size in layout units.
    pub page_w: f64,
    pub page_h: f64,
    /// Left, right, top, bottom, header and footer margins in layout units.
    pub margins: [f64; 6],
    /// Gap between pages in layout units.
    pub gap: f64,
    /// Print scale (1.0 = 100%): screen sizes of cells are `zoom × scale`.
    pub scale: f32,
    /// Left and right margins of headers and footers in layout units (the side margins, or a
    /// fixed 0.7 inch, at most a quarter of the page width, with Align with Page Margins off).
    pub hf_sides: [f64; 2],
    /// Layout units per sheet pixel of header and footer text: 1 with Scale with Document on,
    /// otherwise `1 / scale`, so the text keeps its point size on paper.
    pub hf_text_k: f32,
    /// Paper size in points (after orientation), for the rulers.
    pub paper_pt: (f32, f32),
    pub row_bands: Vec<(u32, u32)>,
    pub col_bands: Vec<(u32, u32)>,
    /// The bands reach the sheet's last row / column.
    pub rows_complete: bool,
    pub cols_complete: bool,
    /// Bands holding printed data: `[first, last]` row bands and column bands.
    pub data_rows: (usize, usize),
    pub data_cols: (usize, usize),
    /// The sheet's page order, which numbers the printed pages.
    pub page_order: PageOrder,
    ri: LineIndex,
    ci: LineIndex,
}

/// A header or footer zone of one page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HfZone {
    pub footer: bool,
    /// 0 left, 1 centre, 2 right.
    pub section: usize,
}

impl PageGrid {
    /// Paginates sheet `si` far enough to cover row `need_row` and column `need_col` (and at
    /// least the printed area). `None` when the sheet doesn't exist.
    pub fn build(wb: &Workbook, si: usize, need_row: u32, need_col: u32) -> Option<PageGrid> {
        let sh = wb.sheet(si)?;
        let printed = print::layout_sheet(wb, si, &PrintOptions::default(), None)?;
        let mut lay = printed.clone();
        let end_row = need_row.max(printed.area.end.row).min(MAX_ROWS - 1);
        let end_col = need_col.max(printed.area.end.col).min(MAX_COLS - 1);
        lay.area = RangeRef::new(CellRef::new(0, 0), CellRef::new(end_row, end_col));
        let (row_bands, col_bands) = print::split_bands(sh, &lay, MAX_ROWS as usize);
        let s = f64::from(lay.scale.clamp(0.1, 4.0));
        let to_layout = |pt: f32| f64::from(pt) / f64::from(PX_TO_PT) / s;
        let margins = lay.margins.map(to_layout);
        let (hf_left, hf_right) = lay.hf_sides();
        let hf_text_k = lay.hf_text_scale() / s as f32;
        let band_range = |bands: &[(u32, u32)], lo: u32, hi: u32| {
            let first = bands.iter().position(|b| b.1 >= lo).unwrap_or(0);
            let last = bands.iter().rposition(|b| b.0 <= hi).unwrap_or(first).max(first);
            (first, last)
        };
        let area = printed.area;
        let data_rows = band_range(&row_bands, area.start.row, area.end.row);
        let data_cols = band_range(&col_bands, area.start.col, area.end.col);
        Some(PageGrid {
            page_w: to_layout(lay.page_w),
            page_h: to_layout(lay.page_h),
            margins,
            gap: f64::from(PAGE_GAP) / s,
            scale: lay.scale,
            hf_sides: [to_layout(hf_left), to_layout(hf_right)],
            hf_text_k,
            paper_pt: (lay.page_w, lay.page_h),
            rows_complete: row_bands.last().is_some_and(|b| b.1 >= MAX_ROWS - 1),
            cols_complete: col_bands.last().is_some_and(|b| b.1 >= MAX_COLS - 1),
            row_bands,
            col_bands,
            data_rows,
            data_cols,
            page_order: printed.settings.page_order,
            ri: LineIndex::new(&sh.rows, sh.default_row_height, MAX_ROWS),
            ci: LineIndex::new(&sh.cols, sh.default_col_width, MAX_COLS),
        })
    }

    /// The grid covers row `r` and column `c` (else it needs a rebuild with more pages).
    pub fn covers(&self, r: u32, c: u32) -> bool {
        (self.rows_complete || self.row_bands.last().is_some_and(|b| b.1 >= r))
            && (self.cols_complete || self.col_bands.last().is_some_and(|b| b.1 >= c))
    }

    fn band_of(bands: &[(u32, u32)], line: u32) -> usize {
        bands.partition_point(|b| b.1 < line).min(bands.len().saturating_sub(1))
    }

    /// Left edge of page column `j` / top edge of page row `i`.
    pub fn page_x(&self, j: usize) -> f64 {
        self.gap + j as f64 * (self.page_w + self.gap)
    }
    pub fn page_y(&self, i: usize) -> f64 {
        self.gap + i as f64 * (self.page_h + self.gap)
    }
    /// The page rectangle `(x, y, w, h)` of page `(i, j)`.
    pub fn page_rect(&self, i: usize, j: usize) -> (f64, f64, f64, f64) {
        (self.page_x(j), self.page_y(i), self.page_w, self.page_h)
    }
    /// Where the cells of page `(i, j)` go: inside the margins.
    pub fn content_rect(&self, i: usize, j: usize) -> (f64, f64, f64, f64) {
        let [l, r, t, b, ..] = self.margins;
        (self.page_x(j) + l, self.page_y(i) + t, (self.page_w - l - r).max(1.0), (self.page_h - t - b).max(1.0))
    }

    /// Left edge of column `col`.
    pub fn col_x(&self, col: u32) -> f64 {
        let j = Self::band_of(&self.col_bands, col);
        let start = self.col_bands.get(j).map_or(0, |b| b.0);
        self.page_x(j) + self.margins[0] + (self.ci.start(col) - self.ci.start(start))
    }
    /// Top edge of row `row`.
    pub fn row_y(&self, row: u32) -> f64 {
        let i = Self::band_of(&self.row_bands, row);
        let start = self.row_bands.get(i).map_or(0, |b| b.0);
        self.page_y(i) + self.margins[2] + (self.ri.start(row) - self.ri.start(start))
    }

    /// Page column under `x` (the nearest one in gaps).
    pub fn page_col_at(&self, x: f64) -> usize {
        let j = ((x - self.gap / 2.0) / (self.page_w + self.gap)).floor();
        (j.max(0.0) as usize).min(self.col_bands.len().saturating_sub(1))
    }
    pub fn page_row_at(&self, y: f64) -> usize {
        let i = ((y - self.gap / 2.0) / (self.page_h + self.gap)).floor();
        (i.max(0.0) as usize).min(self.row_bands.len().saturating_sub(1))
    }

    /// Column under `x`, clamped to the page's columns (margins and gaps hit the nearest one).
    pub fn col_at(&self, x: f64) -> u32 {
        let j = self.page_col_at(x);
        let Some(&(a, b)) = self.col_bands.get(j) else { return 0 };
        let local = x - self.page_x(j) - self.margins[0];
        if local <= 0.0 { a } else { self.ci.at(self.ci.start(a) + local).clamp(a, b) }
    }
    pub fn row_at(&self, y: f64) -> u32 {
        let i = self.page_row_at(y);
        let Some(&(a, b)) = self.row_bands.get(i) else { return 0 };
        let local = y - self.page_y(i) - self.margins[2];
        if local <= 0.0 { a } else { self.ri.at(self.ri.start(a) + local).clamp(a, b) }
    }

    /// The page `(i, j)` whose paper contains the point, if any.
    pub fn page_at(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        let (i, j) = (self.page_row_at(y), self.page_col_at(x));
        let (px, py, w, h) = self.page_rect(i, j);
        (x >= px && x < px + w && y >= py && y < py + h).then_some((i, j))
    }

    /// The point lies on cells (inside a page's margins, on a cell of that page).
    pub fn on_cells(&self, x: f64, y: f64) -> bool {
        let Some((i, j)) = self.page_at(x, y) else { return false };
        let (cx, cy, w, h) = self.content_rect(i, j);
        let (Some(&(_, c1)), Some(&(_, r1))) = (self.col_bands.get(j), self.row_bands.get(i)) else { return false };
        // Cells end where the band ends (the rest of the printable area stays blank).
        let right = (self.col_x(c1) + self.ci.start(c1 + 1) - self.ci.start(c1)).min(cx + w);
        let bottom = (self.row_y(r1) + self.ri.start(r1 + 1) - self.ri.start(r1)).min(cy + h);
        x >= cx && x < right && y >= cy && y < bottom
    }

    /// The header or footer section under the point: the top or bottom margin of a page, split
    /// into thirds between the header and footer side margins.
    pub fn hf_zone_at(&self, x: f64, y: f64) -> Option<((usize, usize), HfZone)> {
        let (i, j) = self.page_at(x, y)?;
        let (px, py, _, _) = self.page_rect(i, j);
        let [_, _, t, b, ..] = self.margins;
        let [l, r] = self.hf_sides;
        let footer = if y < py + t {
            false
        } else if y >= py + self.page_h - b {
            true
        } else {
            return None;
        };
        let inner = (self.page_w - l - r).max(3.0);
        let section = (((x - px - l) / (inner / 3.0)).floor().max(0.0) as usize).min(2);
        Some(((i, j), HfZone { footer, section }))
    }

    /// Page `(i, j)` holds printed data (others are "Click to add data" pages).
    pub fn is_data_page(&self, i: usize, j: usize) -> bool {
        (self.data_rows.0..=self.data_rows.1).contains(&i) && (self.data_cols.0..=self.data_cols.1).contains(&j)
    }
    /// Printed page number of page `(i, j)` (in the sheet's page order) and the printed page count.
    pub fn page_number(&self, i: usize, j: usize) -> Option<(usize, usize)> {
        if !self.is_data_page(i, j) {
            return None;
        }
        let rows = self.data_rows.1 - self.data_rows.0 + 1;
        let cols = self.data_cols.1 - self.data_cols.0 + 1;
        Some((self.page_order.index(i - self.data_rows.0, j - self.data_cols.0, rows, cols) + 1, rows * cols))
    }

    /// Page rows and columns overlapping the layout rectangle `[x0, x1] × [y0, y1]`.
    pub fn pages_in(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> ((usize, usize), (usize, usize)) {
        ((self.page_row_at(y0), self.page_row_at(y1)), (self.page_col_at(x0), self.page_col_at(x1)))
    }

    /// Width / height of everything laid out so far.
    pub fn extent(&self) -> (f64, f64) {
        (self.page_x(self.col_bands.len()), self.page_y(self.row_bands.len()))
    }

    /// Rows `(first, last)` of a page row and columns of a page column.
    pub fn row_band(&self, i: usize) -> Option<(u32, u32)> {
        self.row_bands.get(i).copied()
    }
    pub fn col_band(&self, j: usize) -> Option<(u32, u32)> {
        self.col_bands.get(j).copied()
    }
}

/// Sheet-wide helper: the bottom-right cell a page grid for this sheet must reach so that the
/// layout rectangle up to `(x, y)` is covered. Layout coordinates never run ahead of sheet
/// coordinates (pages only add margins and gaps), so the sheet position is a safe bound.
pub fn need_for(sh: &Sheet, x: f64, y: f64) -> (u32, u32) {
    (sh.row_at(y.max(0.0)).saturating_add(1).min(MAX_ROWS - 1), sh.col_at(x.max(0.0)).saturating_add(1).min(MAX_COLS - 1))
}

#[cfg(test)]
mod tests {
    use gridcraft_model::{Orientation, Sheet};

    use super::*;

    fn wb_with(rows: u32, cols: u32) -> Workbook {
        let mut wb = Workbook::new();
        if let Some(sh) = wb.sheet_mut(0) {
            for r in 0..rows {
                for c in 0..cols {
                    sh.set_value(CellRef::new(r, c), gridcraft_core::Value::Number(1.0));
                }
            }
        }
        wb
    }

    #[test]
    fn pages_match_print_pagination_and_cells_map_both_ways() {
        let wb = wb_with(120, 14);
        let g = PageGrid::build(&wb, 0, 400, 40).unwrap();
        let printed = print::layout_sheet(&wb, 0, &PrintOptions::default(), None).unwrap();
        // The printed area starts at A1, so the view's page edges are the printed ones (the
        // last printed page simply ends with the data).
        let starts = |b: &[(u32, u32)], n: usize| b.iter().take(n).map(|x| x.0).collect::<Vec<_>>();
        assert!(printed.row_bands.len() > 1 && printed.col_bands.len() > 1);
        assert_eq!(starts(&g.row_bands, printed.row_bands.len()), starts(&printed.row_bands, usize::MAX));
        assert_eq!(starts(&g.col_bands, printed.col_bands.len()), starts(&printed.col_bands, usize::MAX));
        assert_eq!(g.data_rows, (0, printed.row_bands.len() - 1));
        assert!(g.covers(400, 40));
        // Every cell maps to a point inside its own page's margins and back.
        for (r, c) in [(0, 0), (5, 3), (60, 9), (119, 13), (300, 30)] {
            let (x, y) = (g.col_x(c) + 1.0, g.row_y(r) + 1.0);
            assert_eq!((g.row_at(y), g.col_at(x)), (r, c), "cell ({r},{c})");
            assert!(g.on_cells(x, y));
            let (i, j) = g.page_at(x, y).unwrap();
            let (cx, cy, _, _) = g.content_rect(i, j);
            assert!(x >= cx && y >= cy);
        }
        // The first row of the second page starts below the first page plus a gap.
        let (a, _) = g.row_bands[1];
        assert!((g.row_y(a) - (g.page_y(1) + g.margins[2])).abs() < 1e-6);
        assert!(g.row_y(a) > g.page_h + g.gap);
    }

    #[test]
    fn margins_gaps_and_header_zones() {
        let mut wb = wb_with(10, 3);
        if let Some(sh) = wb.sheet_mut(0) {
            sh.print.orientation = Orientation::Landscape;
        }
        let g = PageGrid::build(&wb, 0, 50, 20).unwrap();
        assert!(g.page_w > g.page_h, "landscape pages are wider than tall");
        // A point in the top margin hits the header, thirds pick the section.
        let (px, py, w, h) = g.page_rect(0, 0);
        let mid_top = (px + w / 2.0, py + g.margins[2] / 2.0);
        assert_eq!(g.hf_zone_at(mid_top.0, mid_top.1), Some(((0, 0), HfZone { footer: false, section: 1 })));
        assert_eq!(g.hf_zone_at(px + g.margins[0] + 2.0, py + h - 2.0).map(|z| z.1), Some(HfZone { footer: true, section: 0 }));
        assert_eq!(g.hf_zone_at(px + w - g.margins[1] - 2.0, py + 3.0).map(|z| z.1.section), Some(2));
        assert!(!g.on_cells(mid_top.0, mid_top.1), "margins aren't cells");
        // The gap between pages is no page at all.
        assert_eq!(g.page_at(px + w + g.gap / 2.0, py + 10.0), None);
        assert_eq!(g.hf_zone_at(px - 1.0, py + 3.0), None);
        // Page numbers count printed pages only.
        assert_eq!(g.page_number(0, 0), Some((1, 1)));
        assert_eq!(g.page_number(0, 1), None);
        assert!(g.is_data_page(0, 0) && !g.is_data_page(1, 0));
    }

    #[test]
    fn page_numbers_follow_the_page_order() {
        // Two pages tall and two wide, as printed; the pages beyond are "Click to add data" pages.
        let mut wb = wb_with(60, 15);
        let numbers = |wb: &Workbook| {
            let g = PageGrid::build(wb, 0, 200, 60).unwrap();
            assert_eq!((g.data_rows, g.data_cols), ((0, 1), (0, 1)));
            assert_eq!((g.page_number(2, 0), g.page_number(0, 2)), (None, None));
            [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(i, j)| g.page_number(i, j).unwrap())
        };
        assert_eq!(numbers(&wb), [(1, 4), (2, 4), (3, 4), (4, 4)]);
        wb.sheet_mut(0).unwrap().print.page_order = PageOrder::OverThenDown;
        assert_eq!(numbers(&wb), [(1, 4), (3, 4), (2, 4), (4, 4)]);
    }

    #[test]
    fn header_zones_and_text_follow_scale_with_document_and_align_with_page_margins() {
        let mut wb = wb_with(10, 3);
        if let Some(sh) = wb.sheet_mut(0) {
            sh.print.scale = 50;
            sh.print.margins = [2.0, 2.0, 0.75, 0.75, 0.3, 0.3];
        }
        let g = PageGrid::build(&wb, 0, 50, 20).unwrap();
        let (px, py, _, _) = g.page_rect(0, 0);
        // Both on: the sections sit between the 2-inch side margins and the text scales with the cells.
        assert_eq!(g.hf_sides, [g.margins[0], g.margins[1]]);
        assert!((g.hf_text_k - 1.0).abs() < 1e-6);
        assert_eq!(g.hf_zone_at(px + 1000.0, py + 3.0).map(|z| z.1.section), Some(2));
        if let Some(sh) = wb.sheet_mut(0) {
            sh.print.hf_scale_with_doc = false;
            sh.print.hf_align_with_margins = false;
        }
        let g = PageGrid::build(&wb, 0, 50, 20).unwrap();
        // Both off: 0.7 inch (67.2 sheet pixels, 134.4 layout units at 50%) on each side, and the
        // text keeps its point size on paper.
        assert!(g.hf_sides.iter().all(|s| (s - 134.4).abs() < 1e-3), "{:?}", g.hf_sides);
        assert!((g.hf_text_k - 2.0).abs() < 1e-6);
        assert_eq!(g.hf_zone_at(px + 1000.0, py + 3.0).map(|z| z.1.section), Some(1));
    }

    #[test]
    fn print_scale_and_hostile_settings_stay_bounded() {
        let mut wb = wb_with(3, 3);
        if let Some(sh) = wb.sheet_mut(0) {
            sh.print.scale = 50;
            sh.print.margins = [f32::NAN, 1e9, -4.0, f32::INFINITY, 0.0, 0.0];
            sh.print.row_breaks = vec![0, 1, 2, u32::MAX];
        }
        let g = PageGrid::build(&wb, 0, MAX_ROWS, MAX_COLS).unwrap();
        assert!(g.rows_complete && g.cols_complete);
        assert!(g.page_w.is_finite() && g.page_h.is_finite() && g.page_w > 0.0);
        let (ex, ey) = g.extent();
        assert!(ex.is_finite() && ey.is_finite());
        assert_eq!(g.row_at(ey * 2.0), MAX_ROWS - 1);
        assert_eq!(g.col_at(-5.0), 0);
        assert_eq!(need_for(&Sheet::new("S"), f64::MAX, f64::MAX), (MAX_ROWS - 1, MAX_COLS - 1));
        assert!(PageGrid::build(&wb, 7, 0, 0).is_none());
    }
}
