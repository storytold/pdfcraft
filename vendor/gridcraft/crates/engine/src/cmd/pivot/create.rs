use super::fields::{AreaPlacement, add_to_area, in_axes};
use super::params::{func_name, func_param, group_name, group_param, layout_name, show_as_name, sort_name};
use super::{
    Arc, Cell, CellRef, Ctx, EngineError, Json, NumFmt, PivotDateGroup, PivotTable, RangeRef, Result, Session, Sheet, StyleId, Workbook, bad,
    bool_param, cell_param, current_region, edit, find_pivot, json, other, pv, str_param, with,
};

// ---------------------------------------------------------------- insert

/// The default source: the table at the selection, else the selected block / current region.
fn default_source(s: &Session) -> Result<String> {
    let d = s.doc()?;
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    if let Some(t) = sh.table_at(d.selection.active) {
        return Ok(t.name.clone());
    }
    let sel = d.selection.current();
    let r = if sel.is_single() {
        current_region(sh, d.selection.active)
    } else {
        let used = sh.used_range().unwrap_or(sel);
        sel.intersection(&used).unwrap_or(sel)
    };
    if r.is_single() && sh.value(r.start).is_empty() {
        return Err(other("The reference isn't valid. Select a cell in a range with labeled columns, or pass `source`."));
    }
    Ok(pv::range_text(&sh.name, r))
}

pub(super) fn source_param(s: &Session, p: &Json) -> Result<String> {
    match str_param(p, "source").or_else(|| str_param(p, "range")) {
        Some(t) if !t.trim().is_empty() => {
            // Bare ranges are on the active sheet; store them with the sheet name.
            let d = s.doc()?;
            let t = t.trim().trim_start_matches('=');
            if !t.contains('!')
                && RangeRef::parse(t).is_some()
                && let Some(sh) = d.wb.active()
                && let Some(r) = RangeRef::parse(t)
            {
                return Ok(pv::range_text(&sh.name, r));
            }
            Ok(t.to_string())
        }
        _ => default_source(s),
    }
}

fn next_pivot_name(wb: &Workbook) -> String {
    (1..)
        .map(|n| format!("PivotTable{n}"))
        .find(|n| !wb.sheets.iter().any(|s| s.pivots.iter().any(|p| p.name.eq_ignore_ascii_case(n))))
        .unwrap_or_default()
}

fn next_pivot_id(wb: &Workbook) -> u32 {
    let m = wb.sheets.iter().flat_map(|s| s.pivots.iter().map(|p| p.id)).max().unwrap_or(0);
    wb.next_object_id().max(m.saturating_add(1))
}

fn field_list(v: Option<&Json>) -> Vec<Json> {
    match v {
        Some(Json::Array(a)) => a.iter().take(512).cloned().collect(),
        Some(Json::String(s)) => vec![Json::String(s.clone())],
        _ => vec![],
    }
}

pub(super) fn insert_pivot(s: &mut Session, p: &Json) -> Result<Json> {
    let source = source_param(s, p)?;
    let active = s.doc()?.wb.active_sheet();
    // Validate the source now (header names, size).
    let src = pv::read_source(&s.doc()?.wb, &source, active).map_err(other)?;
    let name = match str_param(p, "name") {
        Some(n) if !n.trim().is_empty() => {
            let n = n.trim().to_string();
            if s.doc()?.wb.sheets.iter().any(|sh| sh.pivots.iter().any(|x| x.name.eq_ignore_ascii_case(&n))) {
                return Err(other("A PivotTable with that name already exists."));
            }
            n
        }
        _ => next_pivot_name(&s.doc()?.wb),
    };
    let dest = str_param(p, "destination").or_else(|| str_param(p, "location")).unwrap_or("new").trim().to_string();
    // Existing sheet destination: (sheet index, anchor).
    let target: Option<(usize, CellRef)> = if dest.eq_ignore_ascii_case("new") || dest.is_empty() {
        None
    } else {
        let d = s.doc()?;
        let (sh, body) = crate::cmd::split_sheet(&dest);
        let si = match sh {
            Some(n) => d.wb.sheet_index(&n).ok_or_else(|| bad("insert.pivotTable", format!("no sheet named `{n}`")))?,
            None => active,
        };
        let at = RangeRef::parse(body).map(|r| r.start).ok_or_else(|| bad("insert.pivotTable", format!("not a cell: `{dest}`")))?;
        Some((si, at))
    };
    // Decided on the resolved destination: `New`, ` new ` and an empty one all make a sheet.
    if target.is_none() && s.doc()?.wb.protected_structure {
        return Err(other("The workbook is protected and cannot be changed."));
    }
    let replace = bool_param(p, "replace").unwrap_or(false);
    let rows = field_list(p.get("rows"));
    let columns = field_list(p.get("columns"));
    let values = field_list(p.get("values"));
    let filters = field_list(p.get("filters"));
    let mut new_sheet_at: Option<usize> = None;
    let res = edit(s, |cx| {
        let id = next_pivot_id(&cx.wb);
        let (si, anchor) = match target {
            Some(t) => t,
            None => {
                let at = insert_sheet_before_active(cx);
                new_sheet_at = Some(at);
                (at, CellRef::new(2, 0))
            }
        };
        let mut pt = PivotTable { id, name: name.clone(), source: source.clone(), anchor, ..Default::default() };
        let add = |pt: &mut PivotTable, list: &[Json], area: &str| -> Result<()> {
            for f in list {
                match f {
                    Json::String(n) => {
                        add_to_area(pt, &src, AreaPlacement { cmd: "insert.pivotTable", field: n, area, func: None, name: None, position: None })?
                    }
                    Json::Object(_) => {
                        let n = str_param(f, "field").ok_or_else(|| bad("insert.pivotTable", "a field object needs `field`"))?;
                        let func = match str_param(f, "func") {
                            Some(fname) => {
                                Some(func_param(Some(fname)).ok_or_else(|| bad("insert.pivotTable", format!("unknown function `{fname}`")))?)
                            }
                            None => None,
                        };
                        add_to_area(
                            pt,
                            &src,
                            AreaPlacement { cmd: "insert.pivotTable", field: n, area, func, name: str_param(f, "name"), position: None },
                        )?;
                        let by = group_param(str_param(f, "group"));
                        if let Some(by) = by.filter(|b| *b != PivotDateGroup::None)
                            && matches!(area, "rows" | "columns")
                            && src.col(n).and_then(|c| src.numeric.get(c).copied()).unwrap_or(false)
                            && let Some(af) = pt.rows.iter_mut().chain(pt.columns.iter_mut()).find(|x| x.source_col.eq_ignore_ascii_case(n))
                        {
                            af.date_group = by;
                        }
                    }
                    _ => return Err(bad("insert.pivotTable", "fields are names or {field, func?, name?}")),
                }
            }
            Ok(())
        };
        add(&mut pt, &rows, "rows")?;
        add(&mut pt, &columns, "columns")?;
        add(&mut pt, &values, "values")?;
        add(&mut pt, &filters, "filters")?;
        let sh = cx.sheet_mut(si)?;
        sh.pivots.push(pt);
        let pi = sh.pivots.len() - 1;
        let area = pv::refresh(&mut cx.wb, si, pi, replace).map_err(other)?;
        cx.wb.set_active_sheet(si);
        *cx.sel = crate::Selection::at(anchor);
        cx.structural = true;
        let sheet_name = cx.wb.sheet(si).map(|s| s.name.clone()).unwrap_or_default();
        Ok(json!({
            "id": id,
            "name": name,
            "sheet": sheet_name,
            "anchor": anchor.a1(),
            "range": area.a1(),
            "source": source,
            "fields": src.headers,
        }))
    })?;
    if let Some(at) = new_sheet_at
        && let Ok(d) = s.doc_mut()
    {
        d.sheet_selections.insert(at.min(d.sheet_selections.len()), crate::Selection::default());
    }
    Ok(res)
}

/// Inserts a new, empty sheet before the active one (Excel's place for a new PivotTable or a
/// Show Details sheet); returns its index.
fn insert_sheet_before_active(cx: &mut Ctx) -> usize {
    let at = cx.wb.active_sheet().min(cx.wb.sheets.len());
    let nm = cx.wb.next_sheet_name();
    cx.wb.sheets.insert(at, Arc::new(Sheet::new(nm)));
    for n in cx.wb.names.iter_mut() {
        if let Some(sc) = n.scope.as_mut()
            && *sc >= at
        {
            *sc += 1;
        }
    }
    at
}

/// Show Details (double-click a value cell): the source records behind it, as values with their
/// column's number format, under the source headers on a new sheet.
pub(super) fn show_details(s: &mut Session, p: &Json) -> Result<Json> {
    let d = s.doc()?;
    let at = cell_param(s, p, "cell")?.unwrap_or(d.selection.active);
    let active = d.wb.active_sheet();
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    let pt = sh
        .pivots
        .iter()
        .find(|pt| pt.last_range.is_some_and(|r| r.contains(at)))
        .ok_or_else(|| EngineError::Disabled("pivot.showDetails".into(), pv::NOT_A_VALUE_CELL.into()))?;
    let origin = pt.last_range.map(|r| r.start).unwrap_or_default();
    let found = pv::detail_records(&d.wb, pt, active, at.row.saturating_sub(origin.row), at.col.saturating_sub(origin.col));
    // With the option off, a report that is out of date or a source that cannot be read is
    // reported as the option; only a cell that is not a value cell keeps its own message.
    if !pt.enable_show_details && !matches!(found, Ok(None)) {
        return Err(other(SHOW_DETAILS_OFF));
    }
    let (src, records) = found.map_err(other)?.ok_or_else(|| EngineError::Disabled("pivot.showDetails".into(), pv::NOT_A_VALUE_CELL.into()))?;
    if d.wb.protected_structure {
        return Err(other("The workbook is protected and cannot be changed."));
    }
    let count = records.len();
    let mut new_sheet_at: Option<usize> = None;
    let res = edit(s, |cx| {
        let si = insert_sheet_before_active(cx);
        new_sheet_at = Some(si);
        let base = cx.wb.styles.get(StyleId::default()).clone();
        let fmts: Vec<StyleId> = src
            .formats
            .iter()
            .map(|f| {
                let mut st = base.clone();
                st.num_fmt = NumFmt::new(f);
                cx.wb.styles.intern(st)
            })
            .collect();
        // Columns wide enough for their text, as the PivotTable report's are (dates would show
        // as ####); the first rows are enough to judge a column.
        let font = base.font.clone();
        let widths: Vec<f32> = src
            .headers
            .iter()
            .enumerate()
            .map(|(c, h)| {
                let fmt = src.formats.get(c).map(String::as_str).unwrap_or("General");
                let texts = records.iter().take(1_000).filter_map(|i| src.rows.get(*i).and_then(|r| r.get(c)));
                let widest = texts
                    .map(|v| crate::cmd::format::approx_text_width(&crate::display::format(v, fmt, &cx.wb).text, font.size, false))
                    .fold(crate::cmd::format::approx_text_width(h, font.size, false), f32::max);
                (widest + 14.0).min(320.0)
            })
            .collect();
        let sh = cx.sheet_mut(si)?;
        for (c, w) in widths.into_iter().enumerate() {
            if w > sh.col_width(c as u32) {
                sh.cols.entry(c as u32).or_default().size = Some(w);
            }
        }
        for (c, h) in src.headers.iter().enumerate() {
            sh.set_cell(
                CellRef::new(0, c as u32),
                Cell { value: gridcraft_core::Value::text(h.as_str()), formula: None, style: StyleId::default(), py: None },
            );
        }
        for (r, i) in records.iter().enumerate() {
            let Some(rec) = src.rows.get(*i) else { continue };
            for (c, v) in rec.iter().enumerate() {
                let style = fmts.get(c).copied().unwrap_or_default();
                if v.is_empty() && style == StyleId::default() {
                    continue;
                }
                sh.set_cell(CellRef::new(r as u32 + 1, c as u32), Cell { value: v.clone(), formula: None, style, py: None });
            }
        }
        let name = sh.name.clone();
        cx.wb.set_active_sheet(si);
        *cx.sel = crate::Selection::at(CellRef::new(0, 0));
        cx.structural = true;
        Ok(json!({"sheet": name, "records": count}))
    })?;
    if let Some(at) = new_sheet_at
        && let Ok(d) = s.doc_mut()
    {
        d.sheet_selections.insert(at.min(d.sheet_selections.len()), crate::Selection::default());
    }
    Ok(res)
}

/// The message Show Details gives while Enable show details is off.
const SHOW_DETAILS_OFF: &str = "Show Details is turned off for this PivotTable.";

/// PivotTable Options: Enable show details (Data). Changes only the option; the report is not
/// refreshed. Without params it returns the current setting.
pub(super) fn options(s: &mut Session, p: &Json) -> Result<Json> {
    let (si, pi) = find_pivot(s, p)?;
    let enable = bool_param(p, "enableShowDetails");
    if p.get("enableShowDetails").is_some_and(|v| !v.is_null()) && enable.is_none() {
        return Err(bad("pivot.options", "`enableShowDetails` must be true or false"));
    }
    let current = |s: &Session| -> Result<Json> {
        let pt = s.doc()?.wb.sheet(si).and_then(|sh| sh.pivots.get(pi)).ok_or_else(|| other("There's no such PivotTable."))?;
        Ok(json!({"enableShowDetails": pt.enable_show_details}))
    };
    let Some(on) = enable else { return current(s) };
    if current(s)?["enableShowDetails"] == json!(on) {
        return current(s);
    }
    edit(s, |cx| {
        let pt = cx.sheet_mut(si)?.pivots.get_mut(pi).ok_or_else(|| other("There's no such PivotTable."))?;
        pt.enable_show_details = on;
        Ok(json!({"enableShowDetails": on}))
    })
}

pub(super) fn summarize_table(s: &mut Session, p: &Json) -> Result<Json> {
    let d = s.doc()?;
    let name = match str_param(p, "table") {
        Some(n) => d.wb.table(n).and_then(|(si, ti)| d.wb.sheet(si).and_then(|sh| sh.tables.get(ti))).map(|t| t.name.clone()),
        None => d.wb.active().and_then(|sh| sh.table_at(d.selection.active)).map(|t| t.name.clone()),
    }
    .ok_or_else(|| EngineError::Disabled("table.summarizePivot".into(), "select a cell in a table".into()))?;
    let mut q = with(p, "source", json!(name));
    if let Some(m) = q.as_object_mut() {
        m.remove("table");
    }
    insert_pivot(s, &q)
}

// ---------------------------------------------------------------- queries

pub(super) fn pivot_json(wb: &Workbook, si: usize, pt: &PivotTable) -> Json {
    json!({
        "id": pt.id,
        "name": pt.name,
        "sheet": wb.sheet(si).map(|s| s.name.clone()).unwrap_or_default(),
        "anchor": pt.anchor.a1(),
        "range": pt.last_range.map(|r| r.a1()),
        "source": pt.source,
        "rows": pt.rows.iter().map(|f| json!({"field": f.source_col, "sort": sort_name(f.sort), "group": group_name(f.date_group), "collapsed": f.collapsed_items})).collect::<Vec<_>>(),
        "columns": pt.columns.iter().map(|f| json!({"field": f.source_col, "sort": sort_name(f.sort), "group": group_name(f.date_group), "collapsed": f.collapsed_items})).collect::<Vec<_>>(),
        "values": pt.values.iter().map(|v| json!({"field": v.source_col, "func": func_name(v.func), "name": v.name, "showAs": show_as_name(v.show_as), "numberFormat": v.number_format})).collect::<Vec<_>>(),
        "filters": pt.filters.iter().map(|f| json!({"field": f.source_col, "selected": f.selected, "report": !in_axes(pt, &f.source_col)})).collect::<Vec<_>>(),
        "layout": layout_name(pt.layout),
        "grandTotalsRows": pt.grand_totals_rows,
        "grandTotalsCols": pt.grand_totals_cols,
        "subtotals": if !pt.subtotals { "none" } else if pt.subtotals_top { "top" } else { "bottom" },
        "style": pt.style,
        "showHeaders": pt.show_headers,
        "calculatedFields": pt.calculated_fields.iter().map(|c| json!({"name": c.name, "formula": c.formula})).collect::<Vec<_>>(),
        "valuesOnRows": pt.values_on_rows,
        "valuesPosition": pt.values_position,
        "enableShowDetails": pt.enable_show_details,
    })
}
