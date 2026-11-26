use super::create::pivot_json;
use super::params::{area_param, func_param, position_param, show_as_param};
use super::{
    Json, PivotCalcField, PivotDateGroup, PivotField, PivotFilter, PivotFunc, PivotShowAs, PivotTable, PivotValue, Result, Session, Workbook, bad,
    bool_param, edit, find_pivot, json, ok, other, pv, str_param, u32_param,
};

// ---------------------------------------------------------------- layout editing (pure)

pub(super) fn in_axes(pt: &PivotTable, field: &str) -> bool {
    pt.rows.iter().chain(pt.columns.iter()).any(|f| f.source_col.eq_ignore_ascii_case(field))
}

/// Name of the Σ Values pseudo-field (where the value fields sit when there are two or more).
const VALUES_FIELD: &str = "Σ Values";

/// Whether `field` names Σ Values: `Σ Values`, or `Values` unless a source field has that name.
fn is_values_field(src: &pv::Source, field: &str) -> bool {
    let f = field.trim();
    f == VALUES_FIELD || (f.eq_ignore_ascii_case("values") && src.col(f).is_none())
}

/// Keeps Σ Values in place after a field left (`inserted` false) or arrived at index `i` of
/// Rows (`rows`) or Columns.
pub(super) fn shift_values(pt: &mut PivotTable, rows: bool, i: usize, inserted: bool) {
    if pt.values_on_rows != rows {
        return;
    }
    let Some(p) = pt.values_position else { return };
    // Σ Values stays just ahead of the field it preceded.
    let p = match inserted {
        true if i <= p => p + 1,
        false if i < p => p - 1,
        _ => p,
    };
    let n = if rows { pt.rows.len() } else { pt.columns.len() };
    pt.values_position = (p < n).then_some(p);
}

/// Takes `field` out of Rows and Columns (Σ Values keeps its place); returns its first entry.
fn take_from_axes(pt: &mut PivotTable, field: &str) -> Option<PivotField> {
    let mut first = None;
    for rows in [true, false] {
        loop {
            let list = if rows { &mut pt.rows } else { &mut pt.columns };
            let Some(i) = list.iter().position(|f| f.source_col.eq_ignore_ascii_case(field)) else { break };
            let f = list.remove(i);
            shift_values(pt, rows, i, false);
            first.get_or_insert(f);
        }
    }
    first
}

/// Inserts `f` into Rows (`rows`) or Columns at `position` (default: the end).
fn insert_axis_field(pt: &mut PivotTable, rows: bool, f: PivotField, position: Option<usize>) {
    let list = if rows { &mut pt.rows } else { &mut pt.columns };
    let at = position.unwrap_or(list.len()).min(list.len());
    list.insert(at, f);
    shift_values(pt, rows, at, true);
}

/// Moves Σ Values between Rows and Columns, or within Rows, to `position` (default: the end).
/// In Columns it always comes after the last column field: the column layout doesn't place it
/// anywhere else yet.
fn move_values_field(cmd: &str, pt: &mut PivotTable, from: &str, to: &str, position: Option<usize>) -> Result<Json> {
    let current = if pt.values_on_rows { "rows" } else { "columns" };
    if from != current {
        return Err(bad(cmd, format!("`{VALUES_FIELD}` isn't in the {from} area")));
    }
    if !matches!(to, "rows" | "columns") {
        return Err(other("The field you are moving cannot be placed in that PivotTable area."));
    }
    pt.values_on_rows = to == "rows";
    pt.values_position = position.filter(|p| pt.values_on_rows && *p < pt.rows.len());
    Ok(json!({"field": VALUES_FIELD, "to": to}))
}

/// The canonical source field name for `field`.
pub(super) fn canonical(cmd: &str, src: &pv::Source, field: &str) -> Result<String> {
    src.col(field).and_then(|c| src.headers.get(c).cloned()).ok_or_else(|| bad(cmd, format!("no field `{field}` in the source data")))
}

/// The calculated field named `field`, if any.
fn calc_field<'a>(pt: &'a PivotTable, field: &str) -> Option<&'a PivotCalcField> {
    pt.calculated_fields.iter().find(|c| c.name.eq_ignore_ascii_case(field.trim()))
}

/// The canonical name of a source or calculated field.
fn canonical_any(cmd: &str, pt: &PivotTable, src: &pv::Source, field: &str) -> Result<String> {
    match calc_field(pt, field) {
        Some(c) if src.col(field).is_none() => Ok(c.name.clone()),
        _ => canonical(cmd, src, field),
    }
}

fn unique_value_name(pt: &PivotTable, base: &str, except: Option<usize>) -> String {
    let taken = |n: &str| pt.values.iter().enumerate().any(|(i, v)| Some(i) != except && v.name.eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.to_string();
    }
    (2..10_000).map(|k| format!("{base}{k}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
}

fn auto_name(f: PivotFunc, field: &str) -> String {
    format!("{} of {field}", pv::func_caption(f))
}

/// A calculated field is evaluated on the report's totals, not per source record, so there is
/// no set of values to count the distinct members of.
const NO_CALC_DISTINCT: &str = "A calculated field can't use Distinct Count.";

/// Adds `field` to an area (moving it out of the other axis areas).
pub(super) struct AreaPlacement<'a> {
    pub cmd: &'a str,
    pub field: &'a str,
    pub area: &'a str,
    pub func: Option<PivotFunc>,
    pub name: Option<&'a str>,
    pub position: Option<usize>,
}

pub(super) fn add_to_area(pt: &mut PivotTable, src: &pv::Source, placement: AreaPlacement<'_>) -> Result<()> {
    let AreaPlacement { cmd, field, area, func, name, position } = placement;
    let is_calc = src.col(field).is_none() && calc_field(pt, field).is_some();
    if is_calc && area != "values" {
        return Err(other("A calculated field can only be placed in the Values area."));
    }
    let field = canonical_any(cmd, pt, src, field)?;
    match area {
        "rows" | "columns" => {
            if pt.rows.len() + pt.columns.len() >= pv::MAX_AXIS_FIELDS && !in_axes(pt, &field) {
                return Err(other(format!("A PivotTable can have at most {} row and column fields.", pv::MAX_AXIS_FIELDS)));
            }
            let existing = take_from_axes(pt, &field).unwrap_or_else(|| PivotField { source_col: field.clone(), ..Default::default() });
            // A report filter that selects nothing in particular goes away; a selection stays
            // as the field's hidden items.
            pt.filters.retain(|f| !(f.source_col.eq_ignore_ascii_case(&field) && f.selected.is_none()));
            insert_axis_field(pt, area == "rows", existing, position);
        }
        "values" => {
            if pt.values.len() >= pv::MAX_VALUE_FIELDS {
                return Err(other(format!("A PivotTable can have at most {} value fields.", pv::MAX_VALUE_FIELDS)));
            }
            let numeric = is_calc || src.col(&field).and_then(|c| src.numeric.get(c).copied()).unwrap_or(false);
            let f = func.unwrap_or(if numeric { PivotFunc::Sum } else { PivotFunc::Count });
            if is_calc && f == PivotFunc::DistinctCount {
                return Err(other(NO_CALC_DISTINCT));
            }
            let base = match name {
                Some(n) if !n.trim().is_empty() => {
                    if src.col(n).is_some() {
                        return Err(other("PivotTable field name already exists."));
                    }
                    n.to_string()
                }
                _ => auto_name(f, &field),
            };
            let nm = unique_value_name(pt, &base, None);
            let v = PivotValue { source_col: field, func: f, name: nm, show_as: PivotShowAs::Normal, number_format: None };
            let at = position.unwrap_or(pt.values.len()).min(pt.values.len());
            pt.values.insert(at, v);
        }
        _ => {
            take_from_axes(pt, &field);
            let existing = pt.filters.iter().position(|f| f.source_col.eq_ignore_ascii_case(&field));
            let entry = match existing {
                Some(i) => pt.filters.remove(i),
                None => PivotFilter { source_col: field, selected: None },
            };
            let at = position.unwrap_or(pt.filters.len()).min(pt.filters.len());
            pt.filters.insert(at, entry);
        }
    }
    Ok(())
}

/// Removes `field` from an area; returns the source field it referred to.
fn remove_from_area(cmd: &str, pt: &mut PivotTable, field: &str, area: &str) -> Result<String> {
    match area {
        "rows" | "columns" => {
            let rows = area == "rows";
            let list = if rows { &mut pt.rows } else { &mut pt.columns };
            let i = list
                .iter()
                .position(|f| f.source_col.eq_ignore_ascii_case(field))
                .ok_or_else(|| bad(cmd, format!("`{field}` isn't in the {area} area")))?;
            let f = list.remove(i);
            shift_values(pt, rows, i, false);
            pt.filters.retain(|x| !x.source_col.eq_ignore_ascii_case(&f.source_col));
            Ok(f.source_col)
        }
        "values" => {
            let i = pt
                .values
                .iter()
                .position(|v| v.name.eq_ignore_ascii_case(field))
                .or_else(|| pt.values.iter().position(|v| v.source_col.eq_ignore_ascii_case(field)))
                .ok_or_else(|| bad(cmd, format!("`{field}` isn't in the values area")))?;
            Ok(pt.values.remove(i).source_col)
        }
        _ => {
            let i = pt
                .filters
                .iter()
                .position(|f| f.source_col.eq_ignore_ascii_case(field) && !in_axes(pt, &f.source_col))
                .ok_or_else(|| bad(cmd, format!("`{field}` isn't in the filters area")))?;
            Ok(pt.filters.remove(i).source_col)
        }
    }
}

// ---------------------------------------------------------------- shared update

/// Applies `f` to the targeted pivot and re-renders it, as one undo step.
pub(super) fn update(s: &mut Session, p: &Json, f: impl FnOnce(&mut PivotTable, &pv::Source, &Workbook) -> Result<Json>) -> Result<Json> {
    let (si, pi) = find_pivot(s, p)?;
    let replace = bool_param(p, "replace").unwrap_or(false);
    edit(s, |cx| {
        let pt = cx.wb.sheet(si).and_then(|sh| sh.pivots.get(pi)).cloned().ok_or_else(|| other("There's no such PivotTable."))?;
        let src = pv::read_source(&cx.wb, &pt.source, si).map_err(other)?;
        let mut new = pt.clone();
        let mut out = f(&mut new, &src, &cx.wb)?;
        if let Some(slot) = cx.sheet_mut(si)?.pivots.get_mut(pi) {
            *slot = new;
        }
        let area = pv::refresh(&mut cx.wb, si, pi, replace).map_err(other)?;
        cx.structural = true;
        if out.is_null() {
            out = json!({});
        }
        if let Some(m) = out.as_object_mut() {
            m.insert("range".into(), json!(area.a1()));
        }
        Ok(out)
    })
}

pub(super) fn fields(s: &mut Session, p: &Json) -> Result<Json> {
    let (si, pi) = find_pivot(s, p)?;
    let d = s.doc()?;
    let pt = d.wb.sheet(si).and_then(|sh| sh.pivots.get(pi)).ok_or_else(|| other("There's no such PivotTable."))?;
    let mut out = pivot_json(&d.wb, si, pt);
    let src = pv::read_source(&d.wb, &pt.source, si);
    let list: Vec<Json> = match &src {
        Ok(src) => src
            .headers
            .iter()
            .enumerate()
            .map(|(c, h)| {
                let mut areas = vec![];
                if pt.rows.iter().any(|f| f.source_col.eq_ignore_ascii_case(h)) {
                    areas.push("rows");
                }
                if pt.columns.iter().any(|f| f.source_col.eq_ignore_ascii_case(h)) {
                    areas.push("columns");
                }
                if pt.values.iter().any(|f| f.source_col.eq_ignore_ascii_case(h)) {
                    areas.push("values");
                }
                if pt.filters.iter().any(|f| f.source_col.eq_ignore_ascii_case(h)) && !in_axes(pt, h) {
                    areas.push("filters");
                }
                json!({
                    "name": h,
                    "numeric": src.numeric.get(c).copied().unwrap_or(false),
                    "date": src.dates.get(c).copied().unwrap_or(false),
                    "samples": pv::distinct_labels(&d.wb, src, c, PivotDateGroup::None, 5),
                    "areas": areas,
                })
            })
            .collect(),
        Err(_) => vec![],
    };
    let mut list = list;
    for c in &pt.calculated_fields {
        let areas: Vec<&str> = if pt.values.iter().any(|v| v.source_col.eq_ignore_ascii_case(&c.name)) { vec!["values"] } else { vec![] };
        list.push(json!({"name": c.name, "numeric": true, "date": false, "samples": [], "areas": areas, "calculated": true, "formula": c.formula}));
    }
    if let Some(m) = out.as_object_mut() {
        m.insert("fields".into(), Json::Array(list));
        if let Ok(src) = &src {
            m.insert("records".into(), json!(src.rows.len()));
        }
        if let Err(e) = src {
            m.insert("error".into(), json!(e));
        }
    }
    Ok(out)
}

pub(super) fn list(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc()?;
    let mut v = vec![];
    for (si, sh) in d.wb.sheets.iter().enumerate() {
        for pt in &sh.pivots {
            v.push(pivot_json(&d.wb, si, pt));
        }
    }
    Ok(Json::Array(v))
}

pub(super) fn add_field(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.addField";
    let field = str_param(p, "field").ok_or_else(|| bad(C, "missing `field`"))?.to_string();
    let area = area_param(C, str_param(p, "area"))?;
    let func = match str_param(p, "func") {
        Some(f) => Some(func_param(Some(f)).ok_or_else(|| bad(C, format!("unknown function `{f}`")))?),
        None => None,
    };
    let name = str_param(p, "name").map(str::to_string);
    let pos = position_param(p);
    update(s, p, |pt, src, _| {
        add_to_area(pt, src, AreaPlacement { cmd: C, field: &field, area, func, name: name.as_deref(), position: pos })?;
        Ok(json!({"field": canonical_any(C, pt, src, &field)?, "area": area}))
    })
}

pub(super) fn calculated_field(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.calculatedField";
    let Some(name) = str_param(p, "name").map(|n| n.trim().to_string()) else {
        // Menu-style call: the Insert Calculated Field dialog.
        let (si, pi) = find_pivot(s, p)?;
        let d = s.doc()?;
        let pt = d.wb.sheet(si).and_then(|sh| sh.pivots.get(pi)).ok_or_else(|| other("There's no such PivotTable."))?;
        let n = (1..).map(|k| format!("Field{k}")).find(|n| calc_field(pt, n).is_none()).unwrap_or_else(|| "Field1".into());
        let pivot = pt.name.clone();
        s.ui_requests.push(crate::UiRequest::Dialog("pivotCalculatedField".into(), json!({"pivot": pivot, "name": n, "formula": "= 0"})));
        return ok();
    };
    if name.is_empty() || name.chars().count() > 255 {
        return Err(bad(C, "`name` must be 1 to 255 characters"));
    }
    let delete = bool_param(p, "delete").unwrap_or(false);
    let formula = str_param(p, "formula").map(|f| {
        let f = f.trim();
        if f.starts_with('=') { f.to_string() } else { format!("={f}") }
    });
    let to_values = bool_param(p, "values").unwrap_or(true);
    if !delete && formula.is_none() {
        return Err(bad(C, "missing `formula`"));
    }
    update(s, p, |pt, src, _| {
        let existing = pt.calculated_fields.iter().position(|c| c.name.eq_ignore_ascii_case(&name));
        if delete {
            let i = existing.ok_or_else(|| bad(C, format!("no calculated field `{name}`")))?;
            let gone = pt.calculated_fields.remove(i);
            pt.values.retain(|v| !v.source_col.eq_ignore_ascii_case(&gone.name));
            return Ok(json!({"name": gone.name, "deleted": true}));
        }
        let formula = formula.clone().unwrap_or_default();
        if src.col(&name).is_some() {
            return Err(other("PivotTable field name already exists."));
        }
        let canon = match existing {
            Some(i) => {
                let c = pt.calculated_fields.get_mut(i).ok_or_else(|| other("There's no such calculated field."))?;
                c.formula = formula.clone();
                c.name.clone()
            }
            None => {
                if pt.calculated_fields.len() >= crate::pivot_calc::MAX_CALC_FIELDS {
                    return Err(other(format!("A PivotTable can have at most {} calculated fields.", crate::pivot_calc::MAX_CALC_FIELDS)));
                }
                pt.calculated_fields.push(PivotCalcField { name: name.clone(), formula: formula.clone() });
                name.clone()
            }
        };
        let set = crate::pivot_calc::CalcSet::new(pt, &src.headers);
        let i = set.index(&canon).ok_or_else(|| other("There's no such calculated field."))?;
        if let Some(e) = set.error(i) {
            return Err(bad(C, e.to_string()));
        }
        if set.is_cyclic(i) {
            return Err(bad(C, format!("`{canon}` refers to itself through its formula")));
        }
        if existing.is_none() && to_values && !pt.values.iter().any(|v| v.source_col.eq_ignore_ascii_case(&canon)) {
            add_to_area(pt, src, AreaPlacement { cmd: C, field: &canon, area: "values", func: Some(PivotFunc::Sum), name: None, position: None })?;
        }
        Ok(json!({
            "name": canon,
            "formula": formula,
            "calculatedFields": pt.calculated_fields.iter().map(|c| json!({"name": c.name, "formula": c.formula})).collect::<Vec<_>>(),
        }))
    })
}

pub(super) fn remove_field(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.removeField";
    let field = str_param(p, "field").ok_or_else(|| bad(C, "missing `field`"))?.to_string();
    let area = match str_param(p, "area") {
        Some(a) => Some(area_param(C, Some(a))?),
        None => None,
    };
    update(s, p, |pt, _, _| {
        match area {
            Some(a) => {
                remove_from_area(C, pt, &field, a)?;
            }
            None => {
                let mut any = false;
                for a in ["rows", "columns", "values", "filters"] {
                    any |= remove_from_area(C, pt, &field, a).is_ok();
                }
                if !any {
                    return Err(bad(C, format!("`{field}` isn't in the PivotTable")));
                }
            }
        }
        Ok(json!({}))
    })
}

pub(super) fn move_field(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.moveField";
    let field = str_param(p, "field").ok_or_else(|| bad(C, "missing `field`"))?.to_string();
    let from = area_param(C, str_param(p, "from"))?;
    let to = area_param(C, str_param(p, "to"))?;
    let pos = position_param(p);
    update(s, p, |pt, src, _| {
        if is_values_field(src, &field) {
            return move_values_field(C, pt, from, to, pos);
        }
        if from == to {
            // Reorder within the area.
            match from {
                "rows" | "columns" => {
                    let rows = from == "rows";
                    let list = if rows { &mut pt.rows } else { &mut pt.columns };
                    let i = list
                        .iter()
                        .position(|f| f.source_col.eq_ignore_ascii_case(&field))
                        .ok_or_else(|| bad(C, format!("`{field}` isn't in the {from} area")))?;
                    let f = list.remove(i);
                    shift_values(pt, rows, i, false);
                    insert_axis_field(pt, rows, f, pos);
                }
                "values" => {
                    let i = pt
                        .values
                        .iter()
                        .position(|v| v.name.eq_ignore_ascii_case(&field))
                        .or_else(|| pt.values.iter().position(|v| v.source_col.eq_ignore_ascii_case(&field)))
                        .ok_or_else(|| bad(C, format!("`{field}` isn't in the values area")))?;
                    let v = pt.values.remove(i);
                    let at = pos.unwrap_or(pt.values.len()).min(pt.values.len());
                    pt.values.insert(at, v);
                }
                _ => {
                    let i = pt
                        .filters
                        .iter()
                        .position(|f| f.source_col.eq_ignore_ascii_case(&field))
                        .ok_or_else(|| bad(C, format!("`{field}` isn't in the filters area")))?;
                    let f = pt.filters.remove(i);
                    let at = pos.unwrap_or(pt.filters.len()).min(pt.filters.len());
                    pt.filters.insert(at, f);
                }
            }
            return Ok(json!({}));
        }
        // Keep axis settings (sort, grouping) when moving between rows and columns.
        let kept = pt.rows.iter().chain(pt.columns.iter()).find(|f| f.source_col.eq_ignore_ascii_case(&field)).cloned();
        let hidden = pt.filters.iter().find(|f| f.source_col.eq_ignore_ascii_case(&field)).cloned();
        let src_field = remove_from_area(C, pt, &field, from)?;
        add_to_area(pt, src, AreaPlacement { cmd: C, field: &src_field, area: to, func: None, name: None, position: pos })?;
        if matches!(to, "rows" | "columns") {
            let list = if to == "rows" { &mut pt.rows } else { &mut pt.columns };
            if let (Some(k), Some(slot)) = (kept, list.iter_mut().find(|f| f.source_col.eq_ignore_ascii_case(&src_field))) {
                *slot = k;
            }
            if let Some(h) = hidden.filter(|h| h.selected.is_some())
                && !pt.filters.iter().any(|f| f.source_col.eq_ignore_ascii_case(&src_field))
            {
                pt.filters.push(h);
            }
        }
        Ok(json!({"field": src_field, "to": to}))
    })
}

pub(super) fn value_settings(s: &mut Session, p: &Json) -> Result<Json> {
    const C: &str = "pivot.valueSettings";
    let field = str_param(p, "field").map(str::to_string);
    let index = u32_param(p, "index").map(|v| v as usize);
    let func = match str_param(p, "func") {
        Some(f) => Some(func_param(Some(f)).ok_or_else(|| bad(C, format!("unknown function `{f}`")))?),
        None => None,
    };
    let show = match str_param(p, "showAs") {
        Some(v) => Some(show_as_param(v).ok_or_else(|| bad(C, format!("unknown showAs `{v}`")))?),
        None => None,
    };
    let name = str_param(p, "name").map(str::to_string);
    let fmt = p.get("numberFormat").cloned();
    update(s, p, |pt, src, _| {
        let i = match (index, &field) {
            (Some(i), _) => i,
            (None, Some(f)) => pt
                .values
                .iter()
                .position(|v| v.name.eq_ignore_ascii_case(f))
                .or_else(|| pt.values.iter().position(|v| v.source_col.eq_ignore_ascii_case(f)))
                .ok_or_else(|| bad(C, format!("`{f}` isn't in the values area")))?,
            (None, None) => {
                if pt.values.len() == 1 {
                    0
                } else {
                    return Err(bad(C, "pass `field` or `index`"));
                }
            }
        };
        let cur = pt.values.get(i).cloned().ok_or_else(|| bad(C, "no value field at that index"))?;
        let mut v = cur.clone();
        if let Some(f) = func {
            if f == PivotFunc::DistinctCount && src.col(&cur.source_col).is_none() && calc_field(pt, &cur.source_col).is_some() {
                return Err(other(NO_CALC_DISTINCT));
            }
            v.func = f;
            // An automatic caption follows the function. Count Numbers was captioned "Count
            // Numbers of …" before it took Excel's "Count of …", and saved workbooks still carry it.
            let is_auto = |base: &str| {
                cur.name.eq_ignore_ascii_case(base) || cur.name.strip_prefix(base).is_some_and(|r| r.chars().all(|c| c.is_ascii_digit()))
            };
            let legacy = cur.func == PivotFunc::CountNumbers && is_auto(&format!("Count Numbers of {}", cur.source_col));
            if is_auto(&auto_name(cur.func, &cur.source_col)) || legacy {
                v.name = unique_value_name(pt, &auto_name(f, &cur.source_col), Some(i));
            }
        }
        if let Some(n) = &name {
            let n = n.trim();
            if n.is_empty() {
                return Err(other("The name isn't valid."));
            }
            if src.col(n).is_some() {
                return Err(other("PivotTable field name already exists."));
            }
            if pt.values.iter().enumerate().any(|(j, x)| j != i && x.name.eq_ignore_ascii_case(n)) {
                return Err(other("PivotTable field name already exists."));
            }
            v.name = n.to_string();
        }
        if let Some(sa) = show {
            v.show_as = sa;
        }
        match &fmt {
            Some(Json::String(f)) if !f.is_empty() && f.len() <= 255 => v.number_format = Some(f.clone()),
            Some(Json::Null) => v.number_format = None,
            Some(Json::String(f)) if f.is_empty() => v.number_format = None,
            Some(_) => return Err(bad(C, "`numberFormat` is a format code or null")),
            None => {}
        }
        let name = v.name.clone();
        if let Some(slot) = pt.values.get_mut(i) {
            *slot = v;
        }
        Ok(json!({"name": name}))
    })
}
