use super::{
    Anchor, CellRef, Chart, ChartKind, EngineError, Json, LegendPos, MAX_COLS, MAX_ROWS, RangeRef, Result, Series, Session, Sheet, anchor_mode_param,
    bad, block, bool_param, cell_param_on, edit, f64_param, json, size_cap, split_sheet, str_param, target_ranges, target_sheet, u32_param,
};

#[cfg(test)]
pub(super) fn compiled() -> bool {
    true
}

fn chart_areas(s: &Session, p: &Json) -> Result<Vec<RangeRef>> {
    let mut areas = if p.get("range").is_some() {
        target_ranges(s, p)?
    } else if s.doc()?.selection.ranges.len() > 1 {
        s.doc()?.selection.ranges.clone()
    } else {
        vec![block(s, p)?]
    };
    // Whole columns/rows span a million cells: clip their far edges to the cells in use, so only
    // a block that really is too large gets refused. Bounded areas keep their extent as given.
    let sheet = target_sheet(s, p)?;
    if let Some(used) = s.doc()?.wb.sheet(sheet).and_then(|sh| sh.used_range()) {
        // Areas that start outside the used range keep their extent as given.
        for r in &mut areas {
            let whole_cols = r.start.row == 0 && r.end.row >= MAX_ROWS - 1;
            let whole_rows = r.start.col == 0 && r.end.col >= MAX_COLS - 1;
            if whole_cols && r.start.row <= used.end.row {
                r.end.row = r.end.row.min(used.end.row);
            }
            if whole_rows && r.start.col <= used.end.col {
                r.end.col = r.end.col.min(used.end.col);
            }
        }
    }
    Ok(areas)
}

// ---------------------------------------------------------------- charts

pub fn chart_kind(t: &str, sub: &str) -> ChartKind {
    match (t, sub) {
        ("column", "stacked") => ChartKind::ColumnStacked,
        ("column", "stacked100") => ChartKind::ColumnStacked100,
        ("bar", "stacked") => ChartKind::BarStacked,
        ("bar", "stacked100") => ChartKind::BarStacked100,
        ("bar", _) => ChartKind::BarClustered,
        ("line", "markers") => ChartKind::LineMarkers,
        ("line", "stacked") => ChartKind::LineStacked,
        ("line", _) => ChartKind::Line,
        ("pie", _) => ChartKind::Pie,
        ("doughnut", _) => ChartKind::Doughnut,
        ("area", "stacked") => ChartKind::AreaStacked,
        ("area", "stacked100") => ChartKind::AreaStacked100,
        ("area", _) => ChartKind::Area,
        ("scatter", "lines") => ChartKind::ScatterLines,
        ("scatter", _) => ChartKind::Scatter,
        ("bubble", _) => ChartKind::Bubble,
        ("radar", _) => ChartKind::Radar,
        ("histogram", _) => ChartKind::Histogram,
        ("waterfall", _) => ChartKind::Waterfall,
        ("funnel", _) => ChartKind::Funnel,
        ("treemap", _) => ChartKind::Treemap,
        ("sunburst", _) => ChartKind::Sunburst,
        ("boxWhisker", _) => ChartKind::BoxWhisker,
        ("stock", _) => ChartKind::Stock,
        ("combo", _) => ChartKind::Combo,
        _ => ChartKind::ColumnClustered,
    }
}

/// Builds series from a block: first row/column as names/categories when they're text.
pub fn series_from_range(sh: &Sheet, r: RangeRef, by_rows: bool) -> Vec<Series> {
    let q = gridcraft_formula::quote_sheet(&sh.name);
    let abs = |r: RangeRef| {
        format!(
            "{q}!${}${}:${}${}",
            gridcraft_core::col_to_letters(r.start.col),
            r.start.row + 1,
            gridcraft_core::col_to_letters(r.end.col),
            r.end.row + 1
        )
    };
    let abs1 = |c: CellRef| format!("{q}!${}${}", gridcraft_core::col_to_letters(c.col), c.row + 1);
    let is_text = |c: CellRef| sh.value(c).is_text() || sh.value(c).is_empty();
    let header_row = r.height() > 1
        && (r.start.col..=r.end.col).any(|c| sh.value(CellRef::new(r.start.row, c)).is_text())
        && (r.start.col..=r.end.col).filter(|c| *c != r.start.col).all(|c| is_text(CellRef::new(r.start.row, c)));
    // A blank top-left cell above a header row marks the first column as the category labels even
    // when it holds numbers (years, X values), the usual spreadsheet convention, so it isn't
    // plotted as a series of its own (#287).
    let blank_corner = header_row && sh.value(r.start).is_empty();
    let label_col =
        r.width() > 1 && (blank_corner || (r.start.row + header_row as u32..=r.end.row).all(|row| is_text(CellRef::new(row, r.start.col))));
    let data = RangeRef::new(CellRef::new(r.start.row + header_row as u32, r.start.col + label_col as u32), r.end);
    let mut out = Vec::new();
    if !by_rows {
        for c in data.start.col..=data.end.col {
            out.push(Series {
                name: header_row.then(|| abs1(CellRef::new(r.start.row, c))),
                categories: label_col.then(|| abs(RangeRef::new(CellRef::new(data.start.row, r.start.col), CellRef::new(data.end.row, r.start.col)))),
                values: abs(RangeRef::new(CellRef::new(data.start.row, c), CellRef::new(data.end.row, c))),
                bubble_sizes: None,
                color: None,
                secondary: false,
                kind: None,
                quartiles: Default::default(),
                smooth: false,
                trendlines: Vec::new(),
            });
        }
    } else {
        for row in data.start.row..=data.end.row {
            out.push(Series {
                name: label_col.then(|| abs1(CellRef::new(row, r.start.col))),
                categories: header_row
                    .then(|| abs(RangeRef::new(CellRef::new(r.start.row, data.start.col), CellRef::new(r.start.row, data.end.col)))),
                values: abs(RangeRef::new(CellRef::new(row, data.start.col), CellRef::new(row, data.end.col))),
                bubble_sizes: None,
                color: None,
                secondary: false,
                kind: None,
                quartiles: Default::default(),
                smooth: false,
                trendlines: Vec::new(),
            });
        }
    }
    out
}

fn series_from_areas(sh: &Sheet, areas: &[RangeRef]) -> Result<(RangeRef, bool, Vec<Series>)> {
    let Some(first) = areas.first().copied() else { return Err(EngineError::Other("Select data for the chart first.".into())) };
    let bounds = areas.iter().skip(1).fold(first, |r, area| r.union(area));
    if areas.len() == 1 {
        let by_rows = first.width() > first.height() + 1;
        return Ok((bounds, by_rows, series_from_range(sh, first, by_rows)));
    }
    let valid_shape = first.width() == 1
        && first.height() > 1
        && areas.iter().skip(1).all(|area| area.width() == 1 && area.height() == first.height())
        && first.iter().all(|cell| sh.value(cell).is_text() || sh.value(cell).is_empty());
    if !valid_shape {
        return Err(bad("insert.chart", "multi-area charts require a text category column followed by equal-height series columns"));
    }
    let category = series_from_range(sh, first, false).into_iter().next().map(|series| series.values);
    let mut series = Vec::with_capacity(areas.len() - 1);
    for area in &areas[1..] {
        let mut area_series = series_from_range(sh, *area, false);
        let Some(mut item) = area_series.pop() else {
            return Err(EngineError::Other("Select data for the chart first.".into()));
        };
        if item.name.is_none() {
            return Err(bad("insert.chart", "multi-area series columns require headers"));
        }
        item.categories = category.clone();
        series.push(item);
    }
    Ok((bounds, false, series))
}

/// A scatter plot of a block without category labels takes its first column (or row) as the X
/// values of the others, as Excel does: two plain columns make one series of points, not two (#428).
pub fn scatter_x_from_first(kind: ChartKind, series: &mut Vec<Series>) {
    if matches!(kind, ChartKind::Scatter | ChartKind::ScatterLines) && series.len() > 1 && series[0].categories.is_none() {
        let x = series.remove(0).values;
        for se in series.iter_mut() {
            se.categories = Some(x.clone());
        }
    }
}

/// Most cells a chart's source block may span (bounds the series it builds).
const MAX_CHART_CELLS: u64 = 1_000_000;

/// Chart types that suit the shape of the data, best first, as `(type, subtype, label)` for
/// `insert.chart`. Our own heuristic from the shape of the block (series count, points per
/// series, text categories, negative values), not a copy of Excel's ranking: two numeric columns
/// without labels → scatter; one short series → column, pie, bar; long series → line; several
/// series → clustered and stacked columns and bars, line.
pub fn recommend_charts(s: &Session, p: &Json) -> Result<Vec<(&'static str, &'static str, &'static str)>> {
    let areas = chart_areas(s, p)?;
    if areas.iter().fold(0_u64, |count, area| count.saturating_add(area.count())) > MAX_CHART_CELLS {
        return Err(bad("insert.chart", "range too large for a chart"));
    }
    let sheet = target_sheet(s, p)?;
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let (_, by_rows, series) = series_from_areas(sh, &areas)?;
    let ranges: Vec<RangeRef> = series
        .iter()
        .filter_map(|x| match gridcraft_formula::parse(&x.values) {
            Ok(gridcraft_formula::Expr::Ref(rf)) => Some(rf.range()),
            _ => None,
        })
        .collect();
    let points = ranges.first().map_or(0, RangeRef::count);
    let (mut any_number, mut non_negative) = (false, true);
    for n in ranges.iter().flat_map(|rr| rr.iter()).filter_map(|c| sh.value(c).as_f64()) {
        any_number = true;
        non_negative &= n >= 0.0;
    }
    if series.is_empty() || !any_number {
        return Err(EngineError::Other("Select data for the chart first.".into()));
    }
    let labelled = series.first().is_some_and(|x| x.categories.is_some());
    const COLUMN: (&str, &str, &str) = ("column", "", "Clustered Column");
    const BAR: (&str, &str, &str) = ("bar", "", "Clustered Bar");
    const LINE: (&str, &str, &str) = ("line", "", "Line");
    const LINE_MARKERS: (&str, &str, &str) = ("line", "markers", "Line with Markers");
    const PIE: (&str, &str, &str) = ("pie", "", "Pie");
    const DOUGHNUT: (&str, &str, &str) = ("doughnut", "", "Doughnut");
    const AREA: (&str, &str, &str) = ("area", "", "Area");
    const SCATTER: (&str, &str, &str) = ("scatter", "", "Scatter");
    const STACKED_COLUMN: (&str, &str, &str) = ("column", "stacked", "Stacked Column");
    const STACKED_BAR: (&str, &str, &str) = ("bar", "stacked", "Stacked Bar");
    let mut out = Vec::new();
    if series.len() == 2 && !labelled && !by_rows {
        out.extend([SCATTER, LINE_MARKERS, COLUMN]);
    } else if series.len() == 1 {
        if points > 12 {
            out.extend([LINE, COLUMN, AREA]);
        } else {
            out.extend([COLUMN, BAR]);
            if non_negative && points >= 2 {
                out.extend([PIE, DOUGHNUT]);
            }
            out.push(LINE_MARKERS);
        }
    } else if points > 12 {
        out.extend([LINE, COLUMN, AREA, STACKED_COLUMN]);
    } else {
        out.extend([COLUMN, BAR, STACKED_COLUMN, LINE_MARKERS, STACKED_BAR]);
    }
    Ok(out)
}

/// The chart `insert.chart` would insert for these params, without inserting it: the sheet it
/// goes on and the chart (previews in the Recommended Charts dialog).
pub fn build_chart(s: &Session, p: &Json) -> Result<(usize, Chart)> {
    let areas = chart_areas(s, p)?;
    if areas.iter().fold(0_u64, |count, area| count.saturating_add(area.count())) > MAX_CHART_CELLS {
        return Err(bad("insert.chart", "range too large for a chart"));
    }
    let mut t = str_param(p, "type").unwrap_or("column").to_string();
    let mut sub = str_param(p, "subtype").unwrap_or("").to_string();
    if t == "auto" {
        let first = recommend_charts(s, p)?.into_iter().next().unwrap_or(("column", "", ""));
        t = first.0.into();
        sub = first.1.into();
    }
    let sheet = target_sheet(s, p)?;
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let kind = chart_kind(&t, &sub);
    let (bounds, by_rows, mut series) = series_from_areas(sh, &areas)?;
    if series.is_empty() {
        return Err(EngineError::Other("Select data for the chart first.".into()));
    }
    scatter_x_from_first(kind, &mut series);
    let id = d.wb.next_object_id();
    let at = cell_param_on(s, p, "at", sheet)?.unwrap_or_else(|| CellRef::new(bounds.start.row, bounds.end.col + 2));
    let title = str_param(p, "title").map(str::to_string).or_else(|| {
        if series.len() == 1 {
            series[0].name.as_ref().map(|n| gridcraft_calc::evaluate(&d.wb, sheet, at, n).display())
        } else {
            Some("Chart Title".into())
        }
    });
    let chart = Chart {
        id,
        kind,
        anchor: Anchor {
            cell: at,
            dx: 0.0,
            dy: 0.0,
            width: f64_param(p, "width").unwrap_or(480.0) as f32,
            height: f64_param(p, "height").unwrap_or(288.0) as f32,
            mode: anchor_mode_param(p).unwrap_or_default(),
        },
        title,
        series,
        legend: if matches!(kind, ChartKind::Pie | ChartKind::Doughnut) { LegendPos::Right } else { LegendPos::Bottom },
        data_labels: false,
        data_table: None,
        gridlines: !matches!(kind, ChartKind::Pie | ChartKind::Doughnut | ChartKind::Treemap | ChartKind::Sunburst | ChartKind::Funnel),
        style: 1,
        hole_size: 75,
        first_slice_angle: 0,
        x_title: None,
        y_title: None,
        source: (areas.len() == 1).then(|| format!("{}!{}", gridcraft_formula::quote_sheet(&sh.name), bounds.a1())),
        by_rows,
        pivot: None,
    };
    Ok((sheet, chart))
}

pub(super) fn insert_chart(s: &mut Session, p: &Json) -> Result<Json> {
    let (sheet, chart) = build_chart(s, p)?;
    let id = chart.id;
    edit(s, |cx| {
        cx.sheet_mut(sheet)?.charts.push(chart);
        Ok(json!({"chart": id}))
    })
}

fn find_chart(s: &Session, p: &Json) -> Result<(usize, usize)> {
    let d = s.doc()?;
    let sheet = d.wb.active_sheet();
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let idx = match u32_param(p, "chart") {
        Some(id) => sh.charts.iter().position(|c| c.id == id),
        None => sh.charts.len().checked_sub(1),
    };
    idx.map(|i| (sheet, i)).ok_or_else(|| bad("chart", "no such chart"))
}

pub(super) fn chart_set(s: &mut Session, p: &Json) -> Result<Json> {
    let (si, ci) = find_chart(s, p)?;
    let at = cell_param_on(s, p, "at", si)?;
    let quartiles = p
        .get("quartiles")
        .map(|v| match v.as_str() {
            Some("exclusive") => Ok(gridcraft_model::QuartileMethod::Exclusive),
            Some("inclusive") => Ok(gridcraft_model::QuartileMethod::Inclusive),
            _ => Err(bad("quartiles", "expected exclusive or inclusive")),
        })
        .transpose()?;
    edit(s, |cx| {
        let sh = cx.sheet_mut(si)?;
        let Some(c) = sh.charts.get_mut(ci) else { return Ok(Json::Null) };
        if let Some(t) = str_param(p, "type") {
            let kind = chart_kind(t, str_param(p, "subtype").unwrap_or(""));
            // A new type (other than a combo) applies to every series, as choosing it did before
            // series kept their own: their markers or lines from a file go with the old type.
            if kind != c.kind && kind != ChartKind::Combo {
                for series in &mut c.series {
                    series.kind = None;
                }
            }
            c.kind = kind;
            if !c.supports_data_table() {
                c.data_table = None;
            }
        }
        update_data_table(c, p, bool_param(p, "dataTable"))?;
        if let Some(method) = quartiles {
            for series in &mut c.series {
                series.quartiles = method;
            }
        }
        if let Some(t) = p.get("title") {
            c.title = t.as_str().map(str::to_string);
        }
        if let Some(l) = str_param(p, "legend") {
            c.legend = match l {
                "none" => LegendPos::None,
                "top" => LegendPos::Top,
                "left" => LegendPos::Left,
                "right" => LegendPos::Right,
                _ => LegendPos::Bottom,
            };
        }
        if let Some(v) = bool_param(p, "dataLabels") {
            c.data_labels = v;
        }
        if let Some(v) = bool_param(p, "gridlines") {
            c.gridlines = v;
        }
        if let Some(v) = u32_param(p, "style") {
            c.style = v.clamp(1, 16);
        }
        if let Some(v) = u32_param(p, "holeSize") {
            c.hole_size = v.clamp(10, 90) as u8;
        }
        if let Some(v) = u32_param(p, "firstSliceAngle") {
            c.first_slice_angle = (v % 360) as u16;
        }
        if let Some(t) = p.get("xTitle") {
            c.x_title = t.as_str().map(str::to_string);
        }
        if let Some(t) = p.get("yTitle") {
            c.y_title = t.as_str().map(str::to_string);
        }
        if let Some(at) = at {
            c.anchor.cell = at;
        }
        if let Some(w) = f64_param(p, "width") {
            c.anchor.width = w.clamp(20.0, size_cap(c.anchor.width)) as f32;
        }
        if let Some(h) = f64_param(p, "height") {
            c.anchor.height = h.clamp(20.0, size_cap(c.anchor.height)) as f32;
        }
        if let Some(v) = f64_param(p, "dx") {
            c.anchor.dx = v as f32;
        }
        if let Some(v) = f64_param(p, "dy") {
            c.anchor.dy = v as f32;
        }
        Ok(Json::Null)
    })
}

/// Shared by Chart Properties and Add Chart Element; no separate UI state.
pub(crate) fn update_data_table(c: &mut Chart, p: &Json, on: Option<bool>) -> Result<()> {
    if p.get("dataTable").is_some_and(|v| !v.is_boolean()) {
        return Err(bad("chart", "`dataTable` must be a boolean"));
    }
    if on == Some(false) {
        c.data_table = None;
        return Ok(());
    }
    let fields = ["dataTableKeys", "dataTableHorizontalBorder", "dataTableVerticalBorder", "dataTableOutline"];
    if on == Some(true) || fields.iter().any(|k| p.get(k).is_some()) {
        if !c.supports_data_table() {
            return Err(bad("chart", "data tables need a column, bar, line, area or compatible combo chart"));
        }
        let table = c.data_table.get_or_insert_with(Default::default);
        for (key, value) in
            fields.into_iter().zip([&mut table.show_keys, &mut table.horizontal_border, &mut table.vertical_border, &mut table.outline])
        {
            if let Some(v) = p.get(key) {
                *value = v.as_bool().ok_or_else(|| bad("chart", format!("`{key}` must be a boolean")))?;
            }
        }
    }
    Ok(())
}

pub(super) fn chart_switch(s: &mut Session, p: &Json) -> Result<Json> {
    let (si, ci) = find_chart(s, p)?;
    edit(s, |cx| {
        let Some(sh) = cx.wb.sheet(si) else { return Ok(Json::Null) };
        let Some(c) = sh.charts.get(ci) else { return Ok(Json::Null) };
        // Charts read from a file have no source range: take the block their series span.
        let Some((src, by_rows)) = c.source.clone().map(|s| (s, c.by_rows)).or_else(|| series_block(&c.series)) else { return Ok(Json::Null) };
        let (sheet_name, body) = split_sheet(&src);
        let src_sheet = sheet_name.and_then(|n| cx.wb.sheet_index(&n)).unwrap_or(si);
        let Some(mut r) = RangeRef::parse(body) else { return Ok(Json::Null) };
        let Some(ssh) = cx.wb.sheet(src_sheet) else { return Ok(Json::Null) };
        // Whole-column/row references (common in files) span a million cells: clip the block's
        // far edges to the cells in use, then refuse anything still too large to chart.
        if let Some(used) = ssh.used_range() {
            r.end.row = r.end.row.min(used.end.row).max(r.start.row);
            r.end.col = r.end.col.min(used.end.col).max(r.start.col);
        }
        if r.count() > MAX_CHART_CELLS {
            return Err(bad("chart.switchRowColumn", "range too large for a chart"));
        }
        let by_rows = !by_rows;
        let series = series_from_range(ssh, r, by_rows);
        if let Some(c) = cx.sheet_mut(si)?.charts.get_mut(ci) {
            c.source.get_or_insert(src);
            c.by_rows = by_rows;
            c.series = series;
            scatter_x_from_first(c.kind, &mut c.series);
        }
        Ok(Json::Null)
    })
}

/// The block a chart's series come from, and whether each series is a row of it: the bounding
/// range of the series names, categories and values, when they're all references to one sheet.
fn series_block(series: &[Series]) -> Option<(String, bool)> {
    let mut sheet: Option<String> = None;
    let mut bounds: Option<RangeRef> = None;
    let mut rows = 0;
    for s in series {
        for (i, f) in [Some(&s.values), s.categories.as_ref(), s.name.as_ref()].into_iter().flatten().enumerate() {
            let Ok(gridcraft_formula::Expr::Ref(r)) = gridcraft_formula::parse(f) else { return None };
            let gridcraft_formula::SheetSel::Named(n) = &r.sheet else { return None };
            if !sheet.get_or_insert_with(|| n.clone()).eq_ignore_ascii_case(n) {
                return None;
            }
            let range = r.range();
            if i == 0 && range.height() == 1 && range.width() > 1 {
                rows += 1;
            }
            bounds = Some(bounds.map_or(range, |b| b.union(&range)));
        }
    }
    Some((format!("{}!{}", gridcraft_formula::quote_sheet(&sheet?), bounds?.a1()), rows > 0 && rows == series.len()))
}

pub(super) fn chart_delete(s: &mut Session, p: &Json) -> Result<Json> {
    let (si, ci) = find_chart(s, p)?;
    edit(s, |cx| {
        cx.sheet_mut(si)?.charts.remove(ci);
        Ok(Json::Null)
    })
}
