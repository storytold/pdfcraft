//! Load/Save Model and Reset All: the model written on cells of its sheet, read back from them,
//! and removed from the sheet.

use super::model::{Rhs, is_model_name, model_formulas, parse_model, parse_model_formulas, save_model, saved_model};
use super::{
    Cell, CellRef, DefinedName, EngineError, Json, LOAD_CMD, MAX_CONSTRAINTS, RangeRef, Result, SAVE_CMD, Session, Sheet, Value, bad, bool_param,
    edit, json, split_sheet, str_param,
};
use crate::cmd::analysis::OVERWRITE_ERROR;
use crate::cmd::cell_param_on;

/// Most cells a model written by Save Model takes: the objective, the variables, the options
/// and one per constraint.
const MAX_MODEL_ROWS: usize = MAX_CONSTRAINTS + 3;

fn is_options_row(formula: &str) -> bool {
    formula.trim().trim_start_matches('=').trim_start().starts_with('{')
}

/// The formulas from `start` down: to the first cell without a formula, or through the options
/// row (`{…}`) that ends a model written by Save Model; at most [`MAX_MODEL_ROWS`] cells.
fn formulas_down(sheet: &Sheet, start: CellRef) -> Vec<(CellRef, String)> {
    let mut out = Vec::new();
    for k in 0..MAX_MODEL_ROWS as u32 {
        let Some(c) = start.row.checked_add(k).map(|row| CellRef::new(row, start.col)) else { break };
        let Some(f) = sheet.cell(c).and_then(|x| x.formula.as_ref()).map(|f| f.text.clone()) else { break };
        let last = is_options_row(&f);
        out.push((c, f));
        if last {
            break;
        }
    }
    out
}

/// The cells of a model written by Save Model that starts at `start` (none if the cells there
/// don't hold one).
fn saved_model_cells(sheet: &Sheet, start: CellRef) -> Vec<CellRef> {
    let old = formulas_down(sheet, start);
    let rows: Vec<String> = old.iter().map(|(_, f)| f.clone()).collect();
    if rows.last().is_some_and(|f| is_options_row(f)) && parse_model_formulas(&rows).is_ok() {
        old.into_iter().map(|(c, _)| c).collect()
    } else {
        Vec::new()
    }
}

/// How many cells the options row of Save Model (`={…}`, one row) spans.
fn options_width(formula: &str) -> u32 {
    u32::try_from(formula.split(',').count()).unwrap_or(u32::MAX)
}

/// The options row is an array constant that spills `width - 1` cells to the right of `anchor`.
/// Refused, before anything is written, when the spill would run off the sheet or be blocked
/// (the anchor would show `#SPILL!`): a cell there holds content or another formula's spilled
/// values, or a merged area or a table meets the row. `ours` are the cells Save Model
/// writes or clears (a model saved at the same place before, whose options spill may be
/// replaced); `used` are the model's own cells, which the spill must not cover.
fn check_options_spill(sheet: &Sheet, anchor: CellRef, width: u32, ours: &[CellRef], used: &[CellRef]) -> Result<()> {
    if width <= 1 {
        return Ok(());
    }
    let end = anchor.col.checked_add(width - 1).filter(|c| *c < gridcraft_core::MAX_COLS).ok_or_else(|| {
        EngineError::Other(format!(
            "The saved model's options need {width} cells to the right from {}; choose another place to save it.",
            anchor.a1()
        ))
    })?;
    let full = RangeRef::new(anchor, CellRef::new(anchor.row, end));
    let spill = RangeRef::new(CellRef::new(anchor.row, anchor.col.saturating_add(1)), CellRef::new(anchor.row, end));
    let blocked =
        |why: String| EngineError::Other(format!("The saved model's options fill {}, but {why}; choose another place to save it.", spill.a1()));
    if let Some(c) = used.iter().find(|c| spill.contains(**c)) {
        return Err(blocked(format!("{} is a cell of the model", c.a1())));
    }
    for c in spill.iter() {
        if sheet.cells.has(c) {
            return Err(blocked(format!("{} isn't empty", c.a1())));
        }
        if sheet.spill.contains_key(&c) && sheet.spill_anchor(c).is_none_or(|a| !ours.contains(&a)) {
            return Err(blocked(format!("{} holds the values of another formula", c.a1())));
        }
    }
    if sheet.merges.iter().any(|m| m.intersects(&full)) {
        return Err(blocked("merged cells are in the way".into()));
    }
    // A dynamic array can't spill into a table.
    if sheet.tables.iter().any(|t| t.range.intersects(&full)) {
        return Err(blocked("a table is in the way".into()));
    }
    Ok(())
}

/// Refused when a cell of the model's column holds another formula's spilled values: the
/// formula written there would block that spill (the other formula would show `#SPILL!`).
/// `ours` are as in [`check_options_spill`]: a model saved at the same place before may be
/// replaced.
fn check_column_spills(sheet: &Sheet, column: RangeRef, ours: &[CellRef]) -> Result<()> {
    for c in column.iter() {
        if sheet.spill.contains_key(&c) && sheet.spill_anchor(c).is_none_or(|a| !ours.contains(&a)) {
            return Err(EngineError::Other(format!(
                "The saved model would cover {}, which holds the values of another formula; choose another place to save it.",
                c.a1()
            )));
        }
    }
    Ok(())
}

/// Refused, unless `overwrite`, when a cell of the model's column holds a value, a formula or
/// Python code (a model saved there before included): Save Model then asks before it replaces
/// data, as Excel does. The message carries [`OVERWRITE_ERROR`], which the dialog turns into its
/// question.
fn check_column_empty(sheet: &Sheet, column: RangeRef, overwrite: bool) -> Result<()> {
    let taken = column.iter().find(|c| sheet.cell(*c).is_some_and(Cell::has_content));
    match taken {
        Some(c) if !overwrite => Err(EngineError::Other(format!(
            "The range {} {OVERWRITE_ERROR} ({} isn't empty); pass overwrite: true to replace it.",
            column.a1(),
            c.a1()
        ))),
        _ => Ok(()),
    }
}

/// `data.solverSaveModel`: writes the model on cells of its sheet, in one step. Saving over a
/// longer model saved at the same cell clears the rest of it, so Load Model reads the new one.
pub(super) fn save_model_cells(s: &mut Session, p: &Json) -> Result<Json> {
    let d = s.doc()?;
    let params = match p.get("model") {
        Some(m @ Json::Object(_)) => m.clone(),
        Some(Json::Null) | None => {
            saved_model(&d.wb, d.wb.active_sheet()).ok_or_else(|| EngineError::Other("This sheet has no saved Solver model.".into()))?
        }
        Some(_) => return Err(bad(SAVE_CMD, "`model` must be data.solver params")),
    };
    let model = parse_model(s, &params)?;
    let to = cell_param_on(s, p, "to", model.sheet)?.ok_or_else(|| bad(SAVE_CMD, "missing `to` (the first cell of the saved model)"))?;
    let rows = model_formulas(&model)?;
    let count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    let last = to
        .row
        .checked_add(count.saturating_sub(1))
        .filter(|r| *r < gridcraft_core::MAX_ROWS)
        .ok_or_else(|| EngineError::Other("The saved model doesn't fit below that cell.".into()))?;
    let range = RangeRef::new(to, CellRef::new(last, to.col));
    // The model's own cells are never overwritten.
    let mut used = vec![model.objective];
    used.extend(&model.vars);
    for c in &model.cons {
        used.extend(&c.lhs);
        if let Rhs::Cells(r) = &c.rhs {
            used.extend(r);
        }
    }
    if used.iter().any(|c| range.contains(*c)) {
        return Err(EngineError::Other(format!("{} overlaps the model's cells; choose another place to save it.", range.a1())));
    }
    let sheet = model.sheet;
    let sh = s.doc()?.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    // What is left of a model saved here before; the new model's own cells stay.
    let stale: Vec<CellRef> = saved_model_cells(sh, to).into_iter().filter(|c| !range.contains(*c) && !used.contains(c)).collect();
    let ours: Vec<CellRef> = range.iter().chain(stale.iter().copied()).collect();
    let width = rows.last().map_or(1, |f| options_width(f));
    check_column_spills(sh, range, &ours)?;
    check_options_spill(sh, CellRef::new(last, to.col), width, &ours, &used)?;
    // Last, so the question is asked only when replacing the data would succeed.
    check_column_empty(sh, range, bool_param(p, "overwrite").unwrap_or(false))?;
    edit(s, |cx| {
        let sh = cx.sheet_mut(sheet)?;
        let cells: Vec<CellRef> = range.iter().collect();
        for (c, f) in cells.iter().zip(&rows) {
            let mut cell = Cell::formula(gridcraft_model::Formula::new(f));
            cell.style = sh.style_id(*c);
            sh.set_cell(*c, cell);
        }
        for c in &stale {
            let style = sh.style_id(*c);
            sh.set_cell(*c, Cell { value: Value::Empty, formula: None, style, py: None });
        }
        for c in cells.into_iter().chain(stale) {
            cx.touch(sheet, c);
        }
        Ok(())
    })?;
    Ok(json!({"range": range.a1()}))
}

/// The formulas of a model range, in order. Every cell up to the last formula must hold one: a
/// cell without a formula there (a constraint overwritten by a constant, say) would shift the
/// rows below it and drop a constraint or the options. Empty cells after the last formula are
/// allowed.
fn cells_with_formulas(r: RangeRef, formula: impl Fn(CellRef) -> Option<String>) -> Result<Vec<String>> {
    let cells: Vec<(CellRef, Option<String>)> = r.iter().map(|c| (c, formula(c))).collect();
    let end = cells.iter().rposition(|(_, f)| f.is_some()).map_or(0, |i| i + 1);
    let mut rows = Vec::with_capacity(end);
    for (c, f) in cells.into_iter().take(end) {
        let f = f.ok_or_else(|| EngineError::Other(format!("{} holds no formula, so the range isn't a model saved with Save Model.", c.a1())))?;
        rows.push(f);
    }
    Ok(rows)
}

/// A model read down from `start` must end with its options row, which Save Model always
/// writes last: a cell without a formula before it (a constraint cleared, say) would otherwise
/// end the read early and load the rows above it as the whole model, with default options.
fn complete_from_cell(start: CellRef, down: &[(CellRef, String)]) -> Result<()> {
    match down.last() {
        Some((_, f)) if is_options_row(f) => Ok(()),
        _ if down.len() >= MAX_MODEL_ROWS => Err(EngineError::Other(format!(
            "No options row ends the model within {MAX_MODEL_ROWS} cells of {}, so it isn't a model saved with Save Model.",
            start.a1()
        ))),
        last => {
            let gap = last.map_or(Some(start), |(c, _)| c.row.checked_add(1).map(|row| CellRef::new(row, c.col)));
            Err(EngineError::Other(match gap {
                Some(gap) => format!(
                    "{} holds no formula before the model's options row, so {} doesn't start a complete model saved with Save Model.",
                    gap.a1(),
                    start.a1()
                ),
                None => format!("{} doesn't start a complete model saved with Save Model.", start.a1()),
            }))
        }
    }
}

/// `data.solverLoadModel`: makes a model written by Save Model the active sheet's model.
pub(super) fn load_model_cells(s: &mut Session, p: &Json) -> Result<Json> {
    let d = s.doc()?;
    let active = d.wb.active_sheet();
    let text = str_param(p, "from").ok_or_else(|| bad(LOAD_CMD, "missing `from` (the saved model's range)"))?;
    let (sh, body) = split_sheet(text);
    if sh.is_some_and(|n| d.wb.sheet_index(&n) != Some(active)) {
        return Err(EngineError::Other("Load Model reads a model saved on the active sheet.".into()));
    }
    let r = RangeRef::parse(body.trim()).ok_or_else(|| bad(LOAD_CMD, format!("not a range: `{text}`")))?;
    let cap = MAX_MODEL_ROWS;
    let sheet = d.wb.sheet(active).ok_or(EngineError::NoDocument)?;
    let formula = |c: CellRef| sheet.cell(c).and_then(|x| x.formula.as_ref()).map(|f| f.text.clone());
    let rows: Vec<String> = if r.is_single() {
        let down = formulas_down(sheet, r.start);
        complete_from_cell(r.start, &down)?;
        down.into_iter().map(|(_, f)| f).collect()
    } else {
        if r.count() > cap as u64 {
            return Err(EngineError::Other(format!("The range is too large for a saved model (at most {cap} cells).")));
        }
        let rows = cells_with_formulas(r, formula)?;
        if !rows.last().is_some_and(|f| is_options_row(f)) {
            return Err(EngineError::Other(format!(
                "{} doesn't end with the model's options row, so it isn't a complete model saved with Save Model.",
                r.a1()
            )));
        }
        rows
    };
    let params = parse_model_formulas(&rows)?;
    let model = parse_model(s, &params)?;
    if model.sheet != active {
        return Err(EngineError::Other("Load Model reads a model of the active sheet.".into()));
    }
    edit(s, |cx| {
        save_model(&mut cx.wb, &model);
        Ok(())
    })?;
    Ok(json!({"model": saved_model(&s.doc()?.wb, active)}))
}

/// Whether a defined name belongs to the saved model of `sheet`.
fn is_sheet_model_name(n: &DefinedName, sheet: usize) -> bool {
    n.scope == Some(sheet) && is_model_name(&n.name)
}

/// `data.solverReset`: Reset All removes the active sheet's model. A model the sheet reads from
/// workbook-level names (which other sheets may read too) is left in place and masked by an
/// empty sheet-level `solver_opt`, so no other sheet's model changes.
pub(super) fn reset_model(s: &mut Session, _p: &Json) -> Result<Json> {
    let d = s.doc()?;
    let active = d.wb.active_sheet();
    let removed = d.wb.names.iter().filter(|n| is_sheet_model_name(n, active)).count();
    let mut after = d.wb.names.clone();
    after.retain(|n| !is_sheet_model_name(n, active));
    let masked = {
        let mut probe = (*d.wb).clone();
        probe.names = after;
        saved_model(&probe, active).is_some()
    };
    if removed > 0 || masked {
        edit(s, |cx| {
            cx.wb.names.retain(|n| !is_sheet_model_name(n, active));
            if masked {
                cx.wb.names.push(DefinedName {
                    name: "solver_opt".into(),
                    scope: Some(active),
                    formula: "\"\"".into(),
                    comment: String::new(),
                    hidden: true,
                });
            }
            Ok(())
        })?;
    }
    Ok(json!({"removed": removed, "masked": masked}))
}
