use super::output::Output;
use super::{CellRef, Chart, PivotLookup, Series, Workbook, quote_sheet};

// ---------------------------------------------------------------- PivotCharts

/// The link a PivotChart stores to its PivotTable: `Sheet2!PivotTable1`.
pub fn chart_link(sheet: &str, pivot: &str) -> String {
    format!("{}!{pivot}", quote_sheet(sheet))
}

/// (sheet, PivotTable name) of a PivotChart link; a leading `[Book.xlsx]` is ignored.
pub fn parse_chart_link(link: &str) -> Option<(String, String)> {
    let t = link.trim();
    let t = match t.strip_prefix('[') {
        Some(rest) => rest.split_once(']').map(|(_, r)| r).unwrap_or(rest),
        None => t,
    };
    let (sheet, name) = t.rsplit_once('!')?;
    let sheet = sheet.trim();
    let sheet = gridcraft_core::unquote_sheet(sheet);
    let name = name.trim();
    if sheet.is_empty() || name.is_empty() {
        return None;
    }
    Some((sheet, name.to_string()))
}

/// Whether a PivotChart link points at `pivot` on `sheet`.
pub fn links_to(link: &str, sheet: &str, pivot: &str) -> bool {
    parse_chart_link(link).is_some_and(|(s, n)| s.eq_ignore_ascii_case(sheet) && n.eq_ignore_ascii_case(pivot))
}

/// An absolute reference to `col` over grid `rows` (contiguous runs joined into a union).
fn rows_ref(sheet: &str, origin: CellRef, col: u32, rows: &[u32]) -> Option<String> {
    let q = quote_sheet(sheet);
    let c = origin.col.checked_add(col)?;
    let letters = gridcraft_core::col_to_letters(c);
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(&start) = rows.get(i) {
        let mut end = start;
        while rows.get(i + 1).is_some_and(|n| Some(*n) == end.checked_add(1)) {
            end += 1;
            i += 1;
        }
        i += 1;
        let (a, b) = (origin.row.checked_add(start)? + 1, origin.row.checked_add(end)? + 1);
        parts.push(if a == b { format!("{q}!${letters}${a}") } else { format!("{q}!${letters}${a}:${letters}${b}") });
    }
    match parts.len() {
        0 => None,
        1 => parts.pop(),
        _ => Some(format!("({})", parts.join(","))),
    }
}

/// Most category rows a PivotChart plots.
const MAX_CHART_ROWS: usize = 10_000;

/// Most series a PivotChart gets: the chart renderer reads at most 255 series, and each one owns
/// reference text as long as its category rows, copied into every linked chart.
const MAX_CHART_SERIES: usize = 255;

/// A PivotChart's series for a report written at `origin`: categories are the innermost row
/// items, one series per item column (column items × value fields); totals are left out.
pub fn chart_series(sheet: &str, origin: CellRef, out: &Output) -> Vec<Series> {
    let rows: Vec<u32> = out.chart_rows.iter().copied().take(MAX_CHART_ROWS).collect();
    let categories = out.label_col.and_then(|c| rows_ref(sheet, origin, c, &rows));
    out.chart_cols
        .iter()
        .take(MAX_CHART_SERIES)
        .filter_map(|(c, name)| {
            Some(Series {
                name: Some(format!("\"{}\"", name.replace('"', "\"\""))),
                categories: categories.clone(),
                values: rows_ref(sheet, origin, *c, &rows)?,
                bubble_sizes: None,
                color: None,
                secondary: false,
                kind: None,
                quartiles: Default::default(),
                smooth: false,
                trendlines: Vec::new(),
            })
        })
        .collect()
}

/// Rebuilds the series of every PivotChart of a PivotTable after its report was written.
pub(super) fn rebind_charts(wb: &mut Workbook, sheet: &str, pivot: &str, origin: CellRef, out: &Output) {
    let linked = |ch: &Chart| ch.pivot.as_deref().is_some_and(|l| links_to(l, sheet, pivot));
    let hits: Vec<usize> = (0..wb.sheets.len()).filter(|i| wb.sheet(*i).is_some_and(|sh| sh.charts.iter().any(linked))).collect();
    if hits.is_empty() {
        return;
    }
    let series = chart_series(sheet, origin, out);
    for i in hits {
        let Some(sh) = wb.sheet_mut(i) else { continue };
        for ch in sh.charts.iter_mut().filter(|c| linked(c)) {
            ch.series = series.clone();
        }
    }
}

/// Clears a pivot's last output area.
pub fn clear_output(wb: &mut Workbook, sheet: usize, idx: usize) {
    let Some(shm) = wb.sheet_mut(sheet) else { return };
    let Some(r) = shm.pivots.get(idx).and_then(|p| p.last_range) else { return };
    shm.take_cells(r);
    if let Some(p) = shm.pivots.get_mut(idx) {
        p.last_range = None;
        p.data_index = PivotLookup::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pivot_chart_gets_at_most_the_series_the_renderer_reads() {
        let out = Output {
            grid: vec![],
            row_kinds: vec![],
            body_offset: 0,
            width: 1000,
            chart_rows: vec![1, 2, 3],
            chart_cols: (1..1000).map(|c| (c, format!("S{c}"))).collect(),
            label_col: Some(0),
            data: vec![],
        };
        assert_eq!(chart_series("Sheet1", CellRef::new(0, 0), &out).len(), MAX_CHART_SERIES);
    }
}
