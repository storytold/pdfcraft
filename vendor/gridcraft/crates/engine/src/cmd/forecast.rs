//! Data › Forecast › Forecast Sheet: a new sheet with the history, a FORECAST.ETS column, the
//! FORECAST.ETS.CONFINT confidence bounds, optional FORECAST.ETS.STAT statistics and a chart.
//!
//! Like Excel, the sheet holds a copy of the timeline and values (sorted by time) and live
//! formulas over that copy, so the forecast follows edits to the copied history.

use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_functions::Timeline;
use gridcraft_model::{Anchor, Cell, Chart, ChartKind, Formula, LegendPos, Series, Sheet, StyleId, Table, TableColumn, TotalsFn};
use serde_json::{Value as Json, json};

use super::{CommandSpec, EngineError, Result, Session, bad, bool_param, cmd, edit, has_doc, split_sheet, str_param};
use crate::selection::current_region;

const CMD: &str = "data.forecastSheet";
/// Most history rows a forecast sheet copies.
const MAX_HISTORY: u64 = 10_000;
/// Most forecast rows.
const MAX_PERIODS: u32 = 10_000;
/// Most history rows × forecast rows. Every forecast row holds three ETS formulas that each refit
/// the model over the whole history (about 3 µs per history point in a release build), so this
/// bounds a sheet's recalculation to a few seconds: 10,000 rows of history get 50 forecast rows,
/// 50 rows of history the full 10,000.
const MAX_WORK: u64 = 500_000;

/// Forecast rows by default: a third of the history, within [`max_periods`].
fn default_periods(history: usize) -> u32 {
    u32::try_from(history.div_ceil(3)).unwrap_or(u32::MAX).clamp(1, max_periods(history))
}

/// Most forecast rows for `history` rows of history.
fn max_periods(history: usize) -> u32 {
    let h = u64::try_from(history).unwrap_or(u64::MAX).max(1);
    u32::try_from(MAX_WORK / h).unwrap_or(u32::MAX).clamp(1, MAX_PERIODS)
}

/// Most forecast rows when the model is fit to the first `fit` of `history` rows, which span
/// `slots` timeline slots once their gaps are filled. With a Forecast Start inside the history,
/// every row after it is forecast too (a hindcast row holds the same ETS formulas, each refitting
/// the model over the `slots` slots), so those rows (counted from the raw rows) count against
/// [`MAX_WORK`] before any forecast row does. `None` when the hindcast alone exceeds it.
fn max_periods_from(history: usize, fit: usize, slots: usize) -> Option<u32> {
    if fit >= history {
        return Some(max_periods(history));
    }
    let f = u64::try_from(slots.max(fit)).unwrap_or(u64::MAX).max(1);
    let hindcast = u64::try_from(history - fit).unwrap_or(u64::MAX);
    let rest = (MAX_WORK / f).checked_sub(hindcast).filter(|r| *r >= 1)?;
    Some(u32::try_from(rest).unwrap_or(u32::MAX).min(MAX_PERIODS))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "data.forecastSheet",
        "Forecast Sheet",
        ["Data", "Forecast"],
        None,
        "{timeline?: \"A2:A25\", values?: \"B2:B25\" (default: the first two columns of `range`, the selection or the current region; a text first cell is the header), end?: timeline value or date text | periods?: n (default: a third of the history, at least 1; ≤10,000 and history × periods ≤ 500,000, or with a forecastStart inside the history (hindcast rows are forecast too) (the greater of the fitted rows and fitted slots × (periods + rows from forecastStart on, that row included)) ≤ 500,000; history ≤ 10,000 rows), forecastStart?: a timeline value or date text on the history, before its last point (hindcast: the model is fit to the points before it, the forecast columns start there and run over the actuals; default and the last point: forecast after the history), confidenceInterval?: bool (default true), confidence?: 0.95 or 95, seasonality?: \"auto\" | 0 (none) | 2–8,760, fillMissing?: interpolate|zeros, aggregate?: average|count|counta|max|median|min|sum, statistics?: bool, chart?: line|column} → {sheet, table, chart, history, periods}. Inserts a sheet before the active one: timeline, values, Forecast (FORECAST.ETS), Lower/Upper Confidence Bound (FORECAST.ETS.CONFINT), optional FORECAST.ETS.STAT table, and a chart.",
        has_doc,
        forecast_sheet
    )]
}

/// The cells of a one-row or one-column range, at most `MAX_HISTORY + 1`.
fn line_cells(r: RangeRef) -> Result<Vec<CellRef>> {
    if r.count() > MAX_HISTORY + 1 {
        return Err(bad(CMD, format!("at most {MAX_HISTORY} rows of history")));
    }
    if r.width() == 1 {
        Ok((r.start.row..=r.end.row).map(|row| CellRef::new(row, r.start.col)).collect())
    } else if r.height() == 1 {
        Ok((r.start.col..=r.end.col).map(|col| CellRef::new(r.start.row, col)).collect())
    } else {
        Err(bad(CMD, "the timeline and the values are each one row or one column"))
    }
}

/// A range parameter on any sheet (whole columns trimmed to the used range).
fn range_on(s: &Session, t: &str) -> Result<(usize, RangeRef)> {
    let d = s.doc()?;
    let (sh, body) = split_sheet(t);
    let si = match sh {
        Some(n) => d.wb.sheet_index(&n).ok_or_else(|| bad(CMD, format!("no sheet named `{n}`")))?,
        None => d.wb.active_sheet(),
    };
    let r = RangeRef::parse(body.trim()).ok_or_else(|| bad(CMD, format!("not a range: `{t}`")))?;
    let r = if r.is_full_cols() || r.is_full_rows() {
        match d.wb.sheet(si).and_then(|s| s.used_range()) {
            Some(u) => r.intersection(&RangeRef::new(CellRef::new(0, 0), u.end)).unwrap_or(RangeRef::cell(r.start)),
            None => RangeRef::cell(r.start),
        }
    } else {
        r
    };
    Ok((si, r))
}

/// (sheet, timeline cells, value cells).
fn source(s: &Session, p: &Json) -> Result<(usize, Vec<CellRef>, Vec<CellRef>)> {
    let text = |k: &str| str_param(p, k).map(str::trim).filter(|t| !t.is_empty());
    match (text("timeline"), text("values")) {
        (Some(t), Some(v)) => {
            let (ts, tr) = range_on(s, t)?;
            let (vs, vr) = range_on(s, v)?;
            if ts != vs {
                return Err(bad(CMD, "the timeline and the values must be on one sheet"));
            }
            Ok((ts, line_cells(tr)?, line_cells(vr)?))
        }
        (None, None) => {
            let d = s.doc()?;
            let (si, r) = match text("range") {
                Some(t) => range_on(s, t)?,
                None => {
                    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
                    let sel = d.selection.current();
                    (d.wb.active_sheet(), if sel.is_single() { current_region(sh, d.selection.active) } else { sel })
                }
            };
            if r.width() < 2 {
                return Err(EngineError::Other("Select a timeline column and a values column for the forecast.".into()));
            }
            let col = |c: u32| RangeRef::new(CellRef::new(r.start.row, c), CellRef::new(r.end.row, c));
            Ok((si, line_cells(col(r.start.col))?, line_cells(col(r.start.col + 1))?))
        }
        _ => Err(bad(CMD, "give both `timeline` and `values`, or neither")),
    }
}

/// A number parameter given as a number or as text.
fn number(p: &Json, key: &str) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Number(n)) => Ok(n.as_f64()),
        Some(Json::String(t)) if t.trim().is_empty() => Ok(None),
        Some(Json::String(t)) => {
            t.trim().trim_end_matches('%').trim().parse::<f64>().map(Some).map_err(|_| bad(CMD, format!("`{key}` is not a number")))
        }
        Some(_) => Err(bad(CMD, format!("`{key}` is not a number"))),
    }
}

struct Options {
    seasonality: u32,
    fill: u8,
    aggregate: Option<u8>,
    confidence: Option<f64>,
    statistics: bool,
    column_chart: bool,
}

fn options(p: &Json) -> Result<Options> {
    let seasonality = match p.get("seasonality") {
        Some(Json::String(t)) if t.trim().is_empty() || t.trim().eq_ignore_ascii_case("auto") => 1,
        None | Some(Json::Null) => 1,
        _ => match number(p, "seasonality")? {
            Some(n) if n == n.trunc() && (0.0..=8760.0).contains(&n) => n as u32,
            _ => return Err(bad(CMD, "`seasonality` is \"auto\", 0 (none) or a season length from 2 to 8,760")),
        },
    };
    let fill = match str_param(p, "fillMissing").map(|t| t.trim().to_ascii_lowercase()) {
        None => 1,
        Some(t) if t.is_empty() || t == "interpolate" || t == "interpolation" => 1,
        Some(t) if t == "zeros" || t == "zero" => 0,
        Some(t) => return Err(bad(CMD, format!("`fillMissing` is interpolate or zeros, not `{t}`"))),
    };
    let aggregate = match str_param(p, "aggregate").map(|t| t.trim().to_ascii_lowercase()) {
        None => None,
        Some(t) => match t.as_str() {
            "" | "average" => None,
            "count" => Some(2),
            "counta" => Some(3),
            "max" => Some(4),
            "median" => Some(5),
            "min" => Some(6),
            "sum" => Some(7),
            _ => return Err(bad(CMD, format!("unknown `aggregate`: `{t}`"))),
        },
    };
    let confidence = if bool_param(p, "confidenceInterval").unwrap_or(true) {
        let c = number(p, "confidence")?.unwrap_or(0.95);
        let c = if c >= 1.0 { c / 100.0 } else { c };
        if !(c > 0.0 && c < 1.0) {
            return Err(bad(CMD, "`confidence` is between 0 and 1 (or 0 and 100%)"));
        }
        Some(c)
    } else {
        None
    };
    let column_chart = match str_param(p, "chart").map(|t| t.trim().to_ascii_lowercase()) {
        None => false,
        Some(t) if t.is_empty() || t == "line" => false,
        Some(t) if t == "column" => true,
        Some(t) => return Err(bad(CMD, format!("`chart` is line or column, not `{t}`"))),
    };
    Ok(Options { seasonality, fill, aggregate, confidence, statistics: bool_param(p, "statistics").unwrap_or(false), column_chart })
}

fn col_letters(c: u32) -> String {
    gridcraft_core::col_to_letters(c)
}

fn forecast_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    let opts = options(p)?;
    let (si, tcells, vcells) = source(s, p)?;
    if tcells.len() != vcells.len() {
        return Err(bad(CMD, "the timeline and the values need the same number of cells"));
    }
    let d = s.doc()?;
    if d.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    let sys = d.wb.date_system;
    let sh = d.wb.sheet(si).ok_or(EngineError::NoDocument)?;
    // A text first cell is the header.
    let header = tcells.first().is_some_and(|c| sh.value(*c).is_text()) || vcells.first().is_some_and(|c| sh.value(*c).is_text());
    let name_of = |c: Option<&CellRef>, fallback: &str| match c.map(|c| sh.value(*c)) {
        Some(v) if header && !v.is_empty() => v.display(),
        _ => fallback.to_string(),
    };
    let (tname, vname) = (name_of(tcells.first(), "Timeline"), name_of(vcells.first(), "Values"));
    let skip = usize::from(header);
    let mut rows: Vec<(f64, Value)> = Vec::new();
    for (t, v) in tcells.iter().zip(&vcells).skip(skip) {
        let (tv, vv) = (sh.value(*t), sh.value(*v));
        match tv {
            Value::Number(x) => rows.push((x, if matches!(vv, Value::Number(_)) { vv } else { Value::Empty })),
            Value::Empty if vv.is_empty() => {}
            _ => return Err(EngineError::Other(format!("The timeline must hold numbers or dates ({} isn't one).", t.a1()))),
        }
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let times: Vec<f64> = rows.iter().map(|r| r.0).collect();
    let timeline =
        Timeline::detect(&times, sys).ok_or_else(|| EngineError::Other("The timeline needs at least two points a constant step apart.".into()))?;
    if !rows.iter().any(|r| matches!(r.1, Value::Number(_))) {
        return Err(EngineError::Other("There are no values to forecast.".into()));
    }
    let time_param = |key: &str| -> Result<Option<f64>> {
        match p.get(key) {
            Some(Json::Number(n)) => Ok(n.as_f64()),
            Some(Json::String(t)) if !t.trim().is_empty() => match gridcraft_core::parse::parse_input(t.trim(), sys).value {
                Value::Number(x) => Ok(Some(x)),
                _ => Err(bad(CMD, format!("`{key}` is not a number or a date: `{t}`"))),
            },
            None | Some(Json::Null) => Ok(None),
            Some(Json::String(_)) => Ok(None),
            Some(_) => Err(bad(CMD, format!("`{key}` is not a number or a date"))),
        }
    };
    let end = time_param("end")?;
    // Forecast Start: the index (into `rows`) of the first forecast point, or `rows.len()` for the
    // usual forecast after the history.
    let fit = match time_param("forecastStart")? {
        None => rows.len(),
        Some(start) => {
            let last_t = times.last().copied().unwrap_or(f64::NAN);
            let tol = |a: f64, b: f64| (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0);
            if tol(start, last_t) || timeline.after_last(1).is_some_and(|next| tol(start, next)) {
                rows.len()
            } else {
                let i = times
                    .iter()
                    .position(|t| tol(*t, start))
                    .ok_or_else(|| bad(CMD, "`forecastStart` is not on the timeline (it is a point of the history, before its last point)"))?;
                if rows.get(..i).is_none_or(|h| h.iter().filter(|r| matches!(r.1, Value::Number(_))).count() < 2) {
                    return Err(EngineError::Other("The forecast start needs at least two values before it to fit the forecast.".into()));
                }
                i
            }
        }
    };
    // The slots the model is fit over: the fit rows' span of the timeline, gaps included.
    let slots = match (fit < rows.len(), times.get(fit.wrapping_sub(1)), times.first()) {
        (true, Some(last_fit), Some(first)) => match (timeline.steps_after_last(*last_fit), timeline.steps_after_last(*first)) {
            (Some(b), Some(a)) => ((b - a).round() + 1.0).clamp(0.0, 1e9) as usize,
            _ => fit,
        },
        _ => fit,
    };
    let cap = max_periods_from(rows.len(), fit, slots).ok_or_else(|| {
        EngineError::Other("The forecast start is too early for this much history: choose a later forecast start or less history.".into())
    })?;
    let periods = match end {
        Some(end) => steps_to(&timeline, end, cap)?,
        None => match number(p, "periods")? {
            Some(n) if n >= 1.0 && n.ceil() <= f64::from(cap) => n.ceil() as u32,
            Some(_) => return Err(bad(CMD, format!("`periods` is from 1 to {cap} for {} rows of history", rows.len()))),
            None => default_periods(rows.len()).min(cap),
        },
    };
    let future: Vec<f64> =
        (1..=periods).map(|k| timeline.after_last(k)).collect::<Option<_>>().ok_or_else(|| bad(CMD, "the forecast runs past the last date"))?;
    let fmt_of = |c: Option<&CellRef>| c.map(|c| d.wb.styles.get(sh.style_id(*c)).num_fmt.clone()).unwrap_or_default();
    let (tfmt, vfmt) = (fmt_of(tcells.get(skip)), fmt_of(vcells.get(skip)));
    let sheet_name = d.wb.next_sheet_name();
    let table_name = (1..).map(|n| format!("Table{n}")).find(|n| d.wb.table(n).is_none()).unwrap_or_else(|| "Table".into());
    let chart_id = d.wb.next_object_id();
    let table_id = chart_id.saturating_add(1);

    // Columns: A timeline, B values, C forecast, D/E bounds.
    let n = rows.len() as u32;
    let last = n + 1; // 1-based row of the last history point
    let fit_last = fit as u32 + 1; // 1-based row of the last point the model is fit to
    let end_row = last + periods; // 1-based row of the last forecast
    let hist = |col: u32| format!("${0}$2:${0}${fit_last}", col_letters(col));
    let mut tail = format!("{},{}", opts.seasonality, opts.fill);
    if let Some(a) = opts.aggregate {
        tail.push_str(&format!(",{a}"));
    }
    let mut headers = vec![tname, vname.clone(), format!("Forecast({vname})")];
    if opts.confidence.is_some() {
        headers.push(format!("Lower Confidence Bound({vname})"));
        headers.push(format!("Upper Confidence Bound({vname})"));
    }
    // Table column names are unique.
    for i in 1..headers.len() {
        let base = headers.get(i).cloned().unwrap_or_default();
        let mut name = base.clone();
        let mut k = 2;
        while headers.iter().take(i).any(|h| h.eq_ignore_ascii_case(&name)) {
            name = format!("{base}{k}");
            k += 1;
        }
        if let Some(h) = headers.get_mut(i) {
            *h = name;
        }
    }
    let width = headers.len() as u32;
    let stats_col = width + 1;
    let chart_col = if opts.statistics { stats_col + 3 } else { width + 1 };

    let at = s.doc()?.wb.active_sheet();
    let index = edit(s, |cx| {
        let at = at.min(cx.wb.sheets.len());
        let bold = cx.wb.styles.derive(StyleId::DEFAULT, |st| st.font.bold = true);
        let tstyle = cx.wb.styles.derive(StyleId::DEFAULT, |st| st.num_fmt = tfmt.clone());
        let vstyle = cx.wb.styles.derive(StyleId::DEFAULT, |st| st.num_fmt = vfmt.clone());
        let mut sheet = Sheet::new(sheet_name.clone());
        let put = |sheet: &mut Sheet, row: u32, col: u32, v: Value, style: StyleId| {
            sheet.set_cell(CellRef::new(row, col), Cell { value: v, formula: None, style, py: None });
        };
        let formula = |sheet: &mut Sheet, row: u32, col: u32, text: String, style: StyleId| {
            let mut cell = Cell::formula(Formula::new(&text));
            cell.style = style;
            sheet.set_cell(CellRef::new(row, col), cell);
        };
        for (c, h) in headers.iter().enumerate() {
            put(&mut sheet, 0, c as u32, Value::text(h.as_str()), StyleId::DEFAULT);
        }
        for (i, (t, v)) in rows.iter().enumerate() {
            let r = i as u32 + 1;
            put(&mut sheet, r, 0, Value::Number(*t), tstyle);
            if !v.is_empty() {
                put(&mut sheet, r, 1, v.clone(), vstyle);
            }
        }
        // The last point the model is fit to starts the forecast lines, so they join the values.
        let join = fit_last - 1; // 0-based row
        formula(&mut sheet, join, 2, format!("=B{fit_last}"), vstyle);
        if opts.confidence.is_some() {
            formula(&mut sheet, join, 3, format!("=C{fit_last}"), vstyle);
            formula(&mut sheet, join, 4, format!("=C{fit_last}"), vstyle);
        }
        let (tl, vl) = (hist(0), hist(1));
        let forecast_row = |sheet: &mut Sheet, r: u32| {
            formula(sheet, r - 1, 2, format!("=FORECAST.ETS(A{r},{vl},{tl},{tail})"), vstyle);
            if let Some(c) = opts.confidence {
                let ci = format!("FORECAST.ETS.CONFINT(A{r},{vl},{tl},{c},{tail})");
                formula(sheet, r - 1, 3, format!("=C{r}-{ci}"), vstyle);
                formula(sheet, r - 1, 4, format!("=C{r}+{ci}"), vstyle);
            }
        };
        // Hindcast: forecasts over the actuals from the start to the last history point.
        for r in (fit_last + 1)..=last {
            forecast_row(&mut sheet, r);
        }
        for (k, t) in future.iter().enumerate() {
            let r = last + k as u32 + 1; // 1-based
            put(&mut sheet, r - 1, 0, Value::Number(*t), tstyle);
            forecast_row(&mut sheet, r);
        }
        if opts.statistics {
            put(&mut sheet, 0, stats_col, Value::text("Statistic"), bold);
            put(&mut sheet, 0, stats_col + 1, Value::text("Value"), bold);
            for (k, label) in ["Alpha", "Beta", "Gamma", "MASE", "SMAPE", "MAE", "RMSE"].iter().enumerate() {
                let r = k as u32 + 1;
                put(&mut sheet, r, stats_col, Value::text(*label), StyleId::DEFAULT);
                formula(&mut sheet, r, stats_col + 1, format!("=FORECAST.ETS.STAT({vl},{tl},{},{tail})", k + 1), StyleId::DEFAULT);
            }
        }
        for c in 0..width {
            sheet.cols.insert(
                c,
                gridcraft_model::LineInfo {
                    size: Some(match c {
                        0 | 1 => 90.0,
                        2 => 130.0,
                        _ => 230.0,
                    }),
                    ..Default::default()
                },
            );
        }
        let range = RangeRef::new(CellRef::new(0, 0), CellRef::new(end_row - 1, width - 1));
        sheet.tables.push(Table {
            id: table_id,
            name: table_name.clone(),
            range,
            header_row: true,
            totals_row: false,
            columns: headers
                .iter()
                .map(|h| TableColumn { name: h.clone(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None })
                .collect(),
            style: "TableStyleMedium2".into(),
            banded_rows: true,
            banded_cols: false,
            first_col: false,
            last_col: false,
            filter_button: true,
            filter: vec![],
            table_type: None,
            connection_id: None,
        });
        let q = gridcraft_formula::quote_sheet(&sheet_name);
        let col_ref = |c: u32| format!("{q}!${0}$2:${0}${end_row}", col_letters(c));
        let series = (1..width)
            .map(|c| Series {
                name: Some(format!("{q}!${}$1", col_letters(c))),
                categories: Some(col_ref(0)),
                values: col_ref(c),
                bubble_sizes: None,
                color: None,
                secondary: false,
                kind: (opts.column_chart && c >= 3).then_some(ChartKind::Line),
                quartiles: Default::default(),
                smooth: false,
                trendlines: Vec::new(),
            })
            .collect();
        sheet.charts.push(Chart {
            id: chart_id,
            kind: if opts.column_chart { ChartKind::ColumnClustered } else { ChartKind::Line },
            anchor: Anchor { cell: CellRef::new(1, chart_col), dx: 0.0, dy: 0.0, width: 480.0, height: 288.0, mode: Default::default() },
            title: None,
            series,
            legend: LegendPos::Bottom,
            data_labels: false,
            data_table: None,
            gridlines: true,
            style: 1,
            hole_size: 75,
            first_slice_angle: 0,
            x_title: None,
            y_title: None,
            source: None,
            by_rows: false,
            pivot: None,
        });
        cx.wb.sheets.insert(at, Arc::new(sheet));
        for nm in cx.wb.names.iter_mut() {
            if let Some(sc) = nm.scope.as_mut()
                && *sc >= at
            {
                *sc += 1;
            }
        }
        cx.wb.set_active_sheet(at);
        *cx.sel = crate::Selection::at(CellRef::new(0, 0));
        cx.structural = true;
        Ok(at)
    })?;
    if let Ok(d) = s.doc_mut() {
        d.sheet_selections.insert(index.min(d.sheet_selections.len()), crate::Selection::default());
    }
    Ok(json!({"sheet": sheet_name, "table": table_name, "chart": chart_id, "history": n, "periods": periods, "fitted": fit}))
}

/// Forecast rows needed to reach `end` (rounded up to whole steps).
fn steps_to(timeline: &Timeline, end: f64, cap: u32) -> Result<u32> {
    let steps = timeline.steps_after_last(end).ok_or_else(|| bad(CMD, "`end` is not on the timeline"))?;
    if steps < 1e-9 {
        return Err(EngineError::Other("The forecast end must be after the last point of the timeline.".into()));
    }
    let steps = steps.ceil();
    if steps > f64::from(cap) {
        return Err(bad(CMD, format!("at most {cap} forecast steps for this much history")));
    }
    Ok(steps as u32)
}

#[cfg(test)]
mod tests {
    use gridcraft_core::{CellRef, Value};
    use serde_json::json;

    use crate::Session;

    fn session_with_history() -> Session {
        let mut s = Session::new();
        s.new_workbook();
        let mut rows = vec![json!(["Month", "Sales"])];
        // y = 10 + 2t with a four-step season.
        let season = [3.0, -1.0, 4.0, -6.0];
        for t in 1..=16 {
            rows.push(json!([t, 10.0 + 2.0 * t as f64 + season[(t - 1) % 4]]));
        }
        s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
        s
    }

    fn value(s: &Session, sheet: usize, a1: &str) -> Value {
        s.doc().unwrap().wb.sheet(sheet).unwrap().value(CellRef::parse(a1).unwrap())
    }

    fn formula(s: &Session, sheet: usize, a1: &str) -> String {
        let wb = &s.doc().unwrap().wb;
        wb.sheet(sheet).unwrap().cell(CellRef::parse(a1).unwrap()).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone()).unwrap_or_default()
    }

    #[test]
    fn forecast_sheet_builds_table_formulas_and_chart() {
        let mut s = session_with_history();
        s.execute("selection.set", json!({"range": "A1:B17"})).unwrap();
        let r = s.execute("data.forecastSheet", json!({"periods": 4, "statistics": true})).unwrap();
        assert_eq!(r["history"], 16);
        assert_eq!(r["periods"], 4);
        let wb = &s.doc().unwrap().wb;
        // Inserted before the data sheet and activated.
        assert_eq!(wb.active_sheet(), 0);
        let sh = wb.sheet(0).unwrap();
        assert_eq!(sh.name, r["sheet"].as_str().unwrap());
        assert_eq!(value(&s, 0, "C1"), Value::text("Forecast(Sales)"));
        assert_eq!(value(&s, 0, "D1"), Value::text("Lower Confidence Bound(Sales)"));
        assert_eq!(value(&s, 0, "E1"), Value::text("Upper Confidence Bound(Sales)"));
        assert_eq!(formula(&s, 0, "C17"), "B17");
        assert_eq!(formula(&s, 0, "C18"), "FORECAST.ETS(A18,$B$2:$B$17,$A$2:$A$17,1,1)");
        assert_eq!(formula(&s, 0, "D18"), "C18-FORECAST.ETS.CONFINT(A18,$B$2:$B$17,$A$2:$A$17,0.95,1,1)");
        assert_eq!(formula(&s, 0, "H2"), "FORECAST.ETS.STAT($B$2:$B$17,$A$2:$A$17,1,1,1)");
        // Future timeline and the exact continuation of the seasonal series (t = 17 → 10 + 34 + 3).
        assert_eq!(value(&s, 0, "A18"), Value::Number(17.0));
        assert_eq!(value(&s, 0, "A21"), Value::Number(20.0));
        let c18 = value(&s, 0, "C18").as_f64().unwrap();
        assert!((c18 - 47.0).abs() < 1e-6, "{c18}");
        let (lo, hi) = (value(&s, 0, "D19").as_f64().unwrap(), value(&s, 0, "E19").as_f64().unwrap());
        assert!(lo <= hi);
        assert_eq!(sh.tables.first().map(|t| t.range.a1()), Some("A1:E21".into()));
        let chart = sh.charts.first().unwrap();
        assert_eq!(chart.series.len(), 4);
        assert_eq!(chart.series[1].values, format!("{}!$C$2:$C$21", r["sheet"].as_str().unwrap()));
        // One undo step removes the sheet again.
        let sheets = s.doc().unwrap().wb.sheets.len();
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc().unwrap().wb.sheets.len(), sheets - 1);
    }

    #[test]
    fn hindcast_rows_count_against_the_work_limit() {
        use super::{MAX_WORK, max_periods, max_periods_from};
        assert_eq!(max_periods_from(1_300, 1_300, 1_300), Some(max_periods(1_300)));
        // 2,000 rows fit to the first 1,899: 101 hindcast rows, then 162 forecast rows.
        assert_eq!(max_periods_from(2_000, 1_899, 1_899), Some(162));
        // Fit to the first 1,000: the 1,000 hindcast rows alone are twice the limit.
        assert_eq!(max_periods_from(2_000, 1_000, 1_000), None);
        // 800 rows fit to the first 400, which span 571 slots (gaps filled): each formula refits
        // 571 slots, so 475 forecast rows after the 400 hindcast rows, not 850.
        assert_eq!(max_periods_from(800, 400, 571), Some(475));
        assert!((475 + 400) * 571 <= MAX_WORK as usize);
        for (h, f) in [(2_000usize, 1_899usize), (10_000, 9_990), (100, 2), (60, 50)] {
            if let Some(p) = max_periods_from(h, f, f) {
                assert!((u64::from(p) + (h - f) as u64) * f as u64 <= MAX_WORK, "{h} {f}");
            }
        }
        let mut s = Session::new();
        s.new_workbook();
        let mut rows = vec![json!(["t", "y"])];
        rows.extend((1..=2_000).map(|t| json!([t, (t % 7) as f64 + t as f64 * 0.5])));
        s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
        let sheets = s.doc().unwrap().wb.sheets.len();
        let err = s.execute("data.forecastSheet", json!({"range": "A1:B2001", "forecastStart": 1_001})).expect_err("hindcast too long");
        assert!(err.to_string().contains("forecast start is too early"), "{err}");
        let err = s.execute("data.forecastSheet", json!({"range": "A1:B2001", "forecastStart": 1_900, "periods": 163})).expect_err("past the cap");
        assert!(err.to_string().contains("from 1 to 162"), "{err}");
        assert_eq!(s.doc().unwrap().wb.sheets.len(), sheets);
    }

    #[test]
    fn the_hindcast_cap_follows_the_slots_the_fit_spans_not_its_rows() {
        // 800 rows, 20% of the slots missing: the first 400 rows span 500 slots, so each ETS
        // formula refits 500 slots and the cap is 1,000 - 400 = 600 (it was 850 by rows).
        let mut s = Session::new();
        s.new_workbook();
        let times: Vec<u32> = (1u32..).filter(|t| !matches!(t % 10, 3 | 6)).take(800).collect();
        let mut rows = vec![json!(["t", "y"])];
        rows.extend(times.iter().map(|t| json!([t, (t % 7) as f64 + f64::from(*t) * 0.5])));
        s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
        let start = times[400];
        let err = s.execute("data.forecastSheet", json!({"range": "A1:B801", "forecastStart": start, "periods": 601})).expect_err("past the cap");
        assert!(err.to_string().contains("from 1 to 600"), "{err}");
    }

    #[test]
    fn history_and_periods_are_capped() {
        use super::{MAX_HISTORY, MAX_PERIODS, MAX_WORK, default_periods, max_periods};
        // The default (a third of the history) never exceeds the caps.
        assert_eq!(default_periods(16), 6);
        assert_eq!(default_periods(1_200), 400);
        assert_eq!(default_periods(3_000), 166);
        assert_eq!(default_periods(100_000), 5);
        assert_eq!(max_periods(10), MAX_PERIODS);
        assert_eq!(max_periods(0), MAX_PERIODS);
        for h in [1usize, 2, 3, 50, 51, 1_000, 1_300, 3_000, 10_000, 100_000, usize::MAX] {
            let (d, m) = (default_periods(h), max_periods(h));
            assert!(d >= 1 && d <= m && m <= MAX_PERIODS, "{h}: {d} {m}");
            assert!(u64::try_from(h).unwrap_or(u64::MAX).saturating_mul(u64::from(m)) <= MAX_WORK.max(u64::try_from(h).unwrap_or(u64::MAX)), "{h}");
        }
        // Explicit periods or an end past the cap are refused before anything is built.
        let mut s = Session::new();
        s.new_workbook();
        let mut rows = vec![json!(["t", "y"])];
        rows.extend((1..=1_300).map(|t| json!([t, (t % 7) as f64 + t as f64 * 0.5])));
        s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
        let sheets = s.doc().unwrap().wb.sheets.len();
        let cap = max_periods(1_300); // 384
        let err = |r: crate::Result<serde_json::Value>| r.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err(s.execute("data.forecastSheet", json!({"range": "A1:B1301", "periods": cap + 1}))).contains("from 1 to 384"));
        assert!(err(s.execute("data.forecastSheet", json!({"range": "A1:B1301", "end": 1_300 + cap + 1}))).contains("at most 384"));
        assert!(s.execute("data.forecastSheet", json!({"range": "A1:B1301", "periods": 10_000})).is_err());
        assert_eq!(s.doc().unwrap().wb.sheets.len(), sheets);
        // More history rows than MAX_HISTORY are refused.
        let big = MAX_HISTORY + 2;
        let r = s.execute("data.forecastSheet", json!({"timeline": format!("A1:A{big}"), "values": format!("B1:B{big}")}));
        assert!(r.is_err());
    }

    #[test]
    fn forecast_sheet_parameters_and_errors() {
        let mut s = session_with_history();
        let r = s
            .execute(
                "data.forecastSheet",
                json!({"timeline": "A2:A17", "values": "B2:B17", "end": "22", "confidenceInterval": false, "seasonality": 4, "fillMissing": "zeros", "aggregate": "sum", "chart": "column"}),
            )
            .unwrap();
        assert_eq!(r["periods"], 6);
        assert_eq!(value(&s, 0, "B1"), Value::text("Values"));
        assert_eq!(formula(&s, 0, "C18"), "FORECAST.ETS(A18,$B$2:$B$17,$A$2:$A$17,4,0,7)");
        assert_eq!(value(&s, 0, "D1"), Value::Empty);
        let sh = s.doc().unwrap().wb.sheet(0).unwrap();
        assert_eq!(sh.charts.first().map(|c| c.kind), Some(gridcraft_model::ChartKind::ColumnClustered));
        // Errors: end before the data, bad seasonality, uneven timeline, one column only.
        s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
        assert!(s.execute("data.forecastSheet", json!({"range": "Sheet1!A1:B17", "end": 3})).is_err());
        assert!(s.execute("data.forecastSheet", json!({"range": "Sheet1!A1:B17", "seasonality": 9000})).is_err());
        assert!(s.execute("data.forecastSheet", json!({"range": "Sheet1!B1:B17"})).is_err());
        s.execute("cell.set", json!({"cell": "A5", "input": "4.5"})).unwrap();
        assert!(s.execute("data.forecastSheet", json!({"range": "Sheet1!A1:B17"})).is_err());
    }

    #[test]
    fn forecast_start_hindcasts_over_the_actuals() {
        let mut s = session_with_history(); // t = 1..16 in A2:A17
        let r = s.execute("data.forecastSheet", json!({"range": "A1:B17", "forecastStart": 13, "periods": 2, "confidenceInterval": false})).unwrap();
        assert_eq!(r["fitted"], 12);
        // Fit to t = 1..12 (rows 2..13); the line joins at row 13 and forecasts rows 14..19.
        assert_eq!(formula(&s, 0, "C13"), "B13");
        assert_eq!(value(&s, 0, "C13"), value(&s, 0, "B13"));
        assert_eq!(formula(&s, 0, "C14"), "FORECAST.ETS(A14,$B$2:$B$13,$A$2:$A$13,1,1)");
        assert_eq!(formula(&s, 0, "C19"), "FORECAST.ETS(A19,$B$2:$B$13,$A$2:$A$13,1,1)");
        // The exact seasonal series is recovered, and the actual is still shown next to it.
        let c14 = value(&s, 0, "C14").as_f64().unwrap();
        assert!((c14 - 39.0).abs() < 1e-6, "{c14}"); // t = 13: 10 + 26 + 3
        assert!(value(&s, 0, "B14").as_f64().is_some());
        // Start at the last point or one step later: as without forecastStart.
        for start in [json!(16), json!(17)] {
            let r = s.execute("data.forecastSheet", json!({"range": "Sheet1!A1:B17", "forecastStart": start, "periods": 2})).unwrap();
            assert_eq!(r["fitted"], 16);
        }
        assert_eq!(formula(&s, 0, "C18"), "FORECAST.ETS(A18,$B$2:$B$17,$A$2:$A$17,1,1)");
    }

    #[test]
    fn forecast_start_invalid_values_are_errors() {
        let mut s = session_with_history();
        let sheets = s.doc().unwrap().wb.sheets.len();
        for bad in [json!(0), json!(2), json!(3.5), json!(18), json!(1000), json!("abc"), json!([1]), json!(1e308)] {
            assert!(s.execute("data.forecastSheet", json!({"range": "A1:B17", "forecastStart": bad})).is_err(), "{bad}");
        }
        assert_eq!(s.doc().unwrap().wb.sheets.len(), sheets);
        // Blank or null behaves like no start; empty params unchanged.
        let r = s.execute("data.forecastSheet", json!({"range": "A1:B17", "forecastStart": ""})).unwrap();
        assert_eq!(r["fitted"], 16);
    }
}
