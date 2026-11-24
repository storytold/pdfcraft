//! Where a chart's data sits on a sheet, as the blocks a selected chart outlines: series names in
//! red, category labels in purple and values in blue (#4390). Dragging a corner of the values
//! block points the chart at other cells, and the names and categories follow it (#4389).

use gridcraft_core::{CellRef, RangeRef, col_to_letters};
use gridcraft_formula::{Expr, RefKind, SheetSel, parse, quote_sheet};
use gridcraft_model::{Chart, Series, Workbook};

/// Most series a chart takes from a dragged block (Excel's limit per chart).
pub const MAX_SERIES: u32 = 255;
/// Most cells a dragged values block may span, as for `chart.selectData`.
pub const MAX_CELLS: u64 = 1_000_000;

/// A chart's data as blocks of cells on one sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChartData {
    /// The sheet the data is on.
    pub sheet: usize,
    /// Every series' values: one column per series (one row with `by_rows`), in series order.
    pub values: RangeRef,
    /// Series run across rows rather than down columns.
    pub by_rows: bool,
    /// The category labels every series shares, when they line up beside `values`.
    pub categories: Option<RangeRef>,
    /// One name cell per series, in the line beside `values`, when every series has one there.
    pub names: Option<RangeRef>,
}

impl ChartData {
    /// The blocks after the values move to `to`: category labels keep their column (row) and
    /// span the new points, and names keep their row (column) and span the new series.
    pub fn moved(&self, to: RangeRef) -> ChartData {
        let (categories, names) = if self.by_rows {
            (
                self.categories.map(|c| RangeRef::new(CellRef::new(c.start.row, to.start.col), CellRef::new(c.start.row, to.end.col))),
                self.names.map(|n| RangeRef::new(CellRef::new(to.start.row, n.start.col), CellRef::new(to.end.row, n.start.col))),
            )
        } else {
            (
                self.categories.map(|c| RangeRef::new(CellRef::new(to.start.row, c.start.col), CellRef::new(to.end.row, c.start.col))),
                self.names.map(|n| RangeRef::new(CellRef::new(n.start.row, to.start.col), CellRef::new(n.start.row, to.end.col))),
            )
        };
        ChartData { values: to, categories, names, ..*self }
    }

    /// The cells of series `i` within `values` (`None` past the last series).
    fn line(&self, values: RangeRef, i: u32) -> Option<RangeRef> {
        if self.by_rows {
            let row = values.start.row.checked_add(i).filter(|r| *r <= values.end.row)?;
            Some(RangeRef::new(CellRef::new(row, values.start.col), CellRef::new(row, values.end.col)))
        } else {
            let col = values.start.col.checked_add(i).filter(|c| *c <= values.end.col)?;
            Some(RangeRef::new(CellRef::new(values.start.row, col), CellRef::new(values.end.row, col)))
        }
    }

    /// How many series `values` holds.
    fn count(&self, values: RangeRef) -> u32 {
        if self.by_rows { values.height() } else { values.width() }
    }
}

/// The sheet and cells a series reference names. `home` is the chart's own sheet, for an
/// unqualified reference. Literals, names, 3-D spans, other workbooks and whole rows or columns
/// are not blocks.
fn block(wb: &Workbook, home: usize, text: &str) -> Option<(usize, RangeRef)> {
    let Ok(Expr::Ref(r)) = parse(text.trim().trim_start_matches('=')) else { return None };
    let sheet = match &r.sheet {
        SheetSel::Current => home,
        SheetSel::Named(n) => wb.sheet_index(n)?,
        SheetSel::Span(..) => return None,
    };
    matches!(r.kind, RefKind::Cell(_) | RefKind::Range(..)).then(|| (sheet, r.range()))
}

/// `chart`'s data on `home`'s workbook as blocks, when its series form one: every series' values
/// on one sheet, in adjacent columns (or rows) of the same extent, in series order. `None` for
/// any other layout, and for a PivotChart, whose series follow its PivotTable.
pub fn chart_data(wb: &Workbook, home: usize, chart: &Chart) -> Option<ChartData> {
    if chart.pivot.is_some() {
        return None;
    }
    let first = chart.series.first()?;
    let (sheet, r0) = block(wb, home, &first.values)?;
    let by_rows = match (r0.width(), r0.height()) {
        (1, 1) => chart.by_rows,
        (1, _) => false,
        (_, 1) => true,
        _ => return None,
    };
    let n = u32::try_from(chart.series.len()).ok()?;
    let last = n.checked_sub(1)?;
    let values = if by_rows {
        RangeRef::new(r0.start, CellRef::new(r0.end.row.checked_add(last)?, r0.end.col))
    } else {
        RangeRef::new(r0.start, CellRef::new(r0.end.row, r0.end.col.checked_add(last)?))
    };
    let mut data = ChartData { sheet, values, by_rows, categories: None, names: None };
    for (i, se) in (0..n).zip(&chart.series) {
        if block(wb, home, &se.values) != Some((sheet, data.line(values, i)?)) {
            return None;
        }
    }
    // Category labels shared by every series, in one line beside the points. Compared as blocks,
    // not text: `=Sheet1!$A$2:$A$4` and `Sheet1!$A$2:$A$4` are the same labels.
    let cats = |se: &Series| se.categories.as_deref().and_then(|t| block(wb, home, t));
    if let Some((s, c)) = cats(first)
        && s == sheet
        && chart.series.iter().all(|se| cats(se) == Some((s, c)))
    {
        let lines_up = if by_rows {
            c.height() == 1
                && (c.start.col, c.end.col) == (values.start.col, values.end.col)
                && !(values.start.row..=values.end.row).contains(&c.start.row)
        } else {
            c.width() == 1
                && (c.start.row, c.end.row) == (values.start.row, values.end.row)
                && !(values.start.col..=values.end.col).contains(&c.start.col)
        };
        data.categories = lines_up.then_some(c);
    }
    // A name cell for every series, all in one line beside the values.
    let names: Option<Vec<CellRef>> = chart
        .series
        .iter()
        .map(|se| se.name.as_deref().and_then(|t| block(wb, home, t)).filter(|(s, r)| *s == sheet && r.is_single()).map(|(_, r)| r.start))
        .collect();
    if let Some(cells) = names
        && let (Some(a), Some(b)) = (cells.first(), cells.last())
    {
        let lines_up = (0..n).zip(&cells).all(|(i, c)| {
            if by_rows {
                c.col == a.col && Some(c.row) == values.start.row.checked_add(i) && !(values.start.col..=values.end.col).contains(&c.col)
            } else {
                c.row == a.row && Some(c.col) == values.start.col.checked_add(i) && !(values.start.row..=values.end.row).contains(&c.row)
            }
        });
        data.names = lines_up.then(|| RangeRef::new(*a, *b));
    }
    Some(data)
}

/// Why [`with_values`] refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValuesError {
    /// The chart's series are not one block of cells ([`chart_data`] is `None`).
    NotABlock,
    /// The block is over [`MAX_CELLS`] cells or [`MAX_SERIES`] series.
    TooLarge,
}

/// `chart` with its values moved to `to` on the data's own sheet, as dragging a corner of the
/// values outline does. Series `i` takes the `i`-th column (row) of `to` and keeps everything
/// else it had (colour, kind, axis, trendlines); a wider block adds series and a narrower one
/// drops the last ones. Category labels and names that line up follow the block; ones that don't
/// stay as they are. A data range kept for Switch Row/Column becomes the moved blocks.
pub fn with_values(wb: &Workbook, home: usize, chart: &Chart, to: RangeRef) -> Result<Chart, ValuesError> {
    let data = chart_data(wb, home, chart).ok_or(ValuesError::NotABlock)?;
    if to.count() > MAX_CELLS || data.count(to) > MAX_SERIES {
        return Err(ValuesError::TooLarge);
    }
    let sheet = wb.sheet(data.sheet).ok_or(ValuesError::NotABlock)?;
    let q = quote_sheet(&sheet.name);
    let row = |r: u32| u64::from(r) + 1;
    let range = |r: RangeRef| format!("{q}!${}${}:${}${}", col_to_letters(r.start.col), row(r.start.row), col_to_letters(r.end.col), row(r.end.row));
    let cell = |c: CellRef| format!("{q}!${}${}", col_to_letters(c.col), row(c.row));
    let moved = data.moved(to);
    let shared_cats = chart.series.first().and_then(|se| se.categories.clone());
    let mut out = chart.clone();
    // Keep the orientation the series were read in: a block of one cell per series is read by
    // `by_rows`, so a stale flag would make the moved series unreadable as a block.
    out.by_rows = data.by_rows;
    out.series = (0..data.count(to))
        .filter_map(|i| {
            let line = moved.line(to, i)?;
            let old = chart.series.get(i as usize);
            let mut se = old.cloned().unwrap_or_else(|| Series { categories: shared_cats.clone(), ..Series::default() });
            se.values = range(line);
            if let Some(c) = moved.categories {
                se.categories = Some(range(c));
            }
            if let Some(n) = moved.names {
                se.name =
                    Some(cell(if data.by_rows { CellRef::new(line.start.row, n.start.col) } else { CellRef::new(n.start.row, line.start.col) }));
            }
            // Bubble sizes in a line of their own beside the old points span the new ones.
            if let (Some(old_line), Some(text)) = (data.line(data.values, i), se.bubble_sizes.clone())
                && let Some((s, b)) = block(wb, home, &text)
                && s == data.sheet
            {
                let follows = if data.by_rows {
                    b.height() == 1 && (b.start.col, b.end.col) == (old_line.start.col, old_line.end.col)
                } else {
                    b.width() == 1 && (b.start.row, b.end.row) == (old_line.start.row, old_line.end.row)
                };
                if follows {
                    se.bubble_sizes = Some(range(if data.by_rows {
                        RangeRef::new(CellRef::new(b.start.row, to.start.col), CellRef::new(b.start.row, to.end.col))
                    } else {
                        RangeRef::new(CellRef::new(to.start.row, b.start.col), CellRef::new(to.end.row, b.start.col))
                    }));
                }
            }
            Some(se)
        })
        .collect();
    if out.source.is_some() {
        let all = [moved.categories, moved.names].into_iter().flatten().fold(to, |a, r| a.union(&r));
        out.source = Some(format!("{q}!{}", all.a1()));
    }
    Ok(out)
}
