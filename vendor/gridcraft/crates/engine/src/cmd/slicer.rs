//! Slicers: on-sheet widgets that filter one column of a table by clicking value tiles.
//!
//! A slicer is a thin front-end over the existing filter machinery: its selection is written as a
//! [`FilterCriterion::Values`] into the sheet's [`AutoFilter`] (over the table's range) and
//! [`super::data::apply_filter`] hides the rows. Creating a slicer (`insert.slicer` /
//! `table.insertSlicer`) selects everything, i.e. applies no filter.

use gridcraft_core::CellRef;
use gridcraft_model::{Anchor, AnchorMode, AutoFilter, FilterCriterion, Slicer, Workbook};
use serde_json::{Value as Json, json};

use super::{
    CommandSpec, EngineError, Result, Session, bad, bool_param, cell_param_on, cmd, edit, f64_param, has_doc, str_param, target_sheet, u32_param,
};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "insert.slicer",
            "Slicer",
            ["Insert", "Filters"],
            None,
            "{table?, column?: \"Region\", columns?: 1, at?: \"H2\", width?, height?}",
            has_doc,
            insert_slicer
        ),
        cmd!(
            "table.insertSlicer",
            "Insert Slicer",
            ["Table", "Tools"],
            None,
            "{table?, column?: \"Region\", columns?: 1, at?, width?, height?}",
            has_doc,
            insert_slicer
        ),
        cmd!("slicer.toggle", "Toggle Slicer Value", [], None, "{slicer?: id, value: \"East\", multi?: false}", has_doc, slicer_toggle),
        cmd!("slicer.selectAll", "Clear Slicer Filter", [], None, "{slicer?: id}", has_doc, slicer_select_all),
        cmd!("slicer.clear", "Clear Slicer Filter", [], None, "{slicer?: id}", has_doc, slicer_select_all),
        cmd!("slicer.delete", "Delete Slicer", [], None, "{slicer?: id}", has_doc, slicer_delete),
    ]
}

/// Distinct, non-blank display values of a table column, in first-seen order. Used to populate a
/// slicer's tiles and to detect "everything selected".
pub fn column_values(wb: &Workbook, sheet: usize, table: &str, column: &str) -> Vec<String> {
    let Some(sh) = wb.sheet(sheet) else { return vec![] };
    let Some(t) = sh.tables.iter().find(|t| t.name.eq_ignore_ascii_case(table)) else { return vec![] };
    let Some(ci) = t.column_index(column) else { return vec![] };
    let Some(dr) = t.data_range() else { return vec![] };
    let col = dr.start.col + ci as u32;
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for row in dr.start.row..=dr.end.row {
        let text = crate::display::cell_text(wb, sh, CellRef::new(row, col));
        if text.is_empty() {
            continue;
        }
        if seen.insert(text.to_lowercase()) {
            out.push(text);
        }
    }
    out
}

/// A slicer's tiles as `(value, selected)`, for the UI. `selected` is true for every tile when the
/// slicer has no filter (`selected == None`).
pub fn slicer_items(wb: &Workbook, sheet: usize, sl: &Slicer) -> Vec<(String, bool)> {
    column_values(wb, sheet, &sl.table, &sl.column)
        .into_iter()
        .map(|v| {
            let on = match &sl.selected {
                None => true,
                Some(set) => set.iter().any(|x| x.eq_ignore_ascii_case(&v)),
            };
            (v, on)
        })
        .collect()
}

/// Writes the slicer's current selection into the sheet filter and re-applies it.
fn reapply_slicer(wb: &mut Workbook, sheet: usize, id: u32) {
    let info = wb.sheet(sheet).and_then(|sh| {
        let sl = sh.slicers.iter().find(|s| s.id == id)?;
        let t = sh.tables.iter().find(|t| t.name.eq_ignore_ascii_case(&sl.table))?;
        let ci = t.column_index(&sl.column)?;
        Some((t.range, t.range.start.col + ci as u32, sl.selected.clone()))
    });
    let Some((table_range, abs_col, selected)) = info else { return };
    {
        let Some(sh) = wb.sheet_mut(sheet) else { return };
        let af = sh.autofilter.get_or_insert_with(|| AutoFilter { range: table_range, criteria: vec![] });
        let off = abs_col.saturating_sub(af.range.start.col);
        af.criteria.retain(|(o, _)| *o != off);
        if let Some(values) = selected {
            af.criteria.push((off, FilterCriterion::Values { values, blanks: false }));
        }
    }
    super::data::apply_filter(wb, sheet);
}

fn insert_slicer(s: &mut Session, p: &Json) -> Result<Json> {
    // A table named by `table` is found on any sheet, as the other table commands find it, and the slicer goes on that
    // table's sheet. Looked up on the active sheet only, a named table on another sheet gave "Select a cell inside a
    // table first, or pass `table`" although `table` was passed.
    let named = match str_param(p, "table") {
        Some(name) => Some(s.doc()?.wb.table(name).ok_or_else(|| bad("insert.slicer", format!("no table named `{name}`")))?),
        None => None,
    };
    let sheet = match named {
        Some((si, _)) => si,
        None => target_sheet(s, p)?,
    };
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let t = match named {
        Some((_, ti)) => sh.tables.get(ti),
        None => sh.table_at(d.selection.active),
    }
    .ok_or_else(|| EngineError::Other("Select a cell inside a table first, or pass `table`.".into()))?;
    let column = match str_param(p, "column") {
        Some(c) => t
            .columns
            .iter()
            .find(|tc| tc.name.eq_ignore_ascii_case(c))
            .map(|tc| tc.name.clone())
            .ok_or_else(|| bad("insert.slicer", format!("table has no column `{c}`")))?,
        None => {
            // Default to the active cell's column within the table, else the first column.
            let idx = d.selection.active.col.checked_sub(t.range.start.col).map(|o| o as usize).filter(|o| *o < t.columns.len()).unwrap_or(0);
            t.columns.get(idx).map(|tc| tc.name.clone()).ok_or_else(|| bad("insert.slicer", "the table has no columns"))?
        }
    };
    let table = t.name.clone();
    let at = cell_param_on(s, p, "at", sheet)?
        .unwrap_or_else(|| CellRef::new(t.range.start.row, t.range.end.col.saturating_add(2).min(gridcraft_core::MAX_COLS - 1)));
    let columns = u32_param(p, "columns").unwrap_or(1).clamp(1, 10);
    let width = f64_param(p, "width").unwrap_or(168.0).clamp(60.0, 10000.0) as f32;
    let height = f64_param(p, "height").unwrap_or(200.0).clamp(60.0, 10000.0) as f32;
    let id = d.wb.next_object_id();
    let slicer = Slicer {
        id,
        table,
        column: column.clone(),
        caption: column,
        anchor: Anchor { cell: at, dx: 4.0, dy: 4.0, width, height, mode: AnchorMode::MoveOnly },
        selected: None,
        columns,
    };
    edit(s, |cx| {
        cx.sheet_mut(sheet)?.slicers.push(slicer);
        Ok(json!({"slicer": id}))
    })
}

/// The target slicer: by `slicer` id, else the last one on the active sheet.
fn find_slicer(s: &Session, p: &Json) -> Result<(usize, u32)> {
    let d = s.doc()?;
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    let idx = match u32_param(p, "slicer") {
        Some(id) => sh.slicers.iter().position(|x| x.id == id),
        None => sh.slicers.len().checked_sub(1),
    };
    idx.and_then(|i| sh.slicers.get(i).map(|x| (i, x.id))).ok_or_else(|| bad("slicer", "no slicer on this sheet"))
}

fn slicer_toggle(s: &mut Session, p: &Json) -> Result<Json> {
    let (idx, id) = find_slicer(s, p)?;
    let value = str_param(p, "value").ok_or_else(|| bad("slicer.toggle", "missing `value`"))?.to_string();
    let multi = bool_param(p, "multi").unwrap_or(false);
    let sheet = s.doc()?.wb.active_sheet();
    let (table, column, current) = {
        let d = s.doc()?;
        let sl = d.wb.active().and_then(|sh| sh.slicers.get(idx)).ok_or_else(|| bad("slicer.toggle", "no slicer"))?;
        (sl.table.clone(), sl.column.clone(), sl.selected.clone())
    };
    let all = column_values(&s.doc()?.wb, sheet, &table, &column);
    if !all.iter().any(|v| v.eq_ignore_ascii_case(&value)) {
        return Err(bad("slicer.toggle", "value is not in the column"));
    }
    let new_selected: Option<Vec<String>> = if multi {
        let mut set: Vec<String> = current.unwrap_or_else(|| all.clone());
        match set.iter().position(|x| x.eq_ignore_ascii_case(&value)) {
            Some(pos) => {
                set.remove(pos);
            }
            None => set.push(value.clone()),
        }
        // Everything selected is the same as no filter.
        if set.len() == all.len() && all.iter().all(|a| set.iter().any(|x| x.eq_ignore_ascii_case(a))) { None } else { Some(set) }
    } else {
        // Single click: show only this value; clicking the sole selected value clears the filter.
        match &current {
            Some(set) if set.len() == 1 && set.iter().any(|x| x.eq_ignore_ascii_case(&value)) => None,
            _ => Some(vec![value.clone()]),
        }
    };
    edit(s, |cx| {
        if let Some(sl) = cx.sheet_mut(sheet)?.slicers.get_mut(idx) {
            sl.selected = new_selected;
        }
        reapply_slicer(&mut cx.wb, sheet, id);
        cx.structural = true;
        Ok(Json::Null)
    })
}

fn slicer_select_all(s: &mut Session, p: &Json) -> Result<Json> {
    let (idx, id) = find_slicer(s, p)?;
    let sheet = s.doc()?.wb.active_sheet();
    edit(s, |cx| {
        if let Some(sl) = cx.sheet_mut(sheet)?.slicers.get_mut(idx) {
            sl.selected = None;
        }
        reapply_slicer(&mut cx.wb, sheet, id);
        cx.structural = true;
        Ok(Json::Null)
    })
}

fn slicer_delete(s: &mut Session, p: &Json) -> Result<Json> {
    let (idx, id) = find_slicer(s, p)?;
    let sheet = s.doc()?.wb.active_sheet();
    // Absolute column of the slicer, so its filter criterion can be removed too.
    let abs_col = s.doc().ok().and_then(|d| {
        let sh = d.wb.active()?;
        let sl = sh.slicers.get(idx)?;
        let t = sh.tables.iter().find(|t| t.name.eq_ignore_ascii_case(&sl.table))?;
        let ci = t.column_index(&sl.column)?;
        Some(t.range.start.col + ci as u32)
    });
    edit(s, |cx| {
        let sh = cx.sheet_mut(sheet)?;
        sh.slicers.retain(|x| x.id != id);
        if let (Some(abs), Some(af)) = (abs_col, sh.autofilter.as_mut()) {
            let off = abs.saturating_sub(af.range.start.col);
            af.criteria.retain(|(o, _)| *o != off);
        }
        super::data::apply_filter(&mut cx.wb, sheet);
        cx.structural = true;
        Ok(Json::Null)
    })
}
