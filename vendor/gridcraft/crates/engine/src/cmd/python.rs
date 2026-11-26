//! Python in cells: making and editing Python cells, and their return type. This build stores
//! the code and the last result a file cached; it doesn't run Python, so a cell with new or
//! changed code shows `#BLOCKED!`.

use std::sync::Arc;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Cell, PyCode, python::MAX_FORMULA_CHARS};
use serde_json::{Value as Json, json};

use super::{CommandSpec, bad, cell_param_on, cmd, edit, has_doc, str_param, target_sheet};
use crate::{EngineError, Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "python.set",
            "Set Python Code",
            [],
            None,
            "{cell?, code, output?: object|value} makes the cell (default: the active cell) a Python cell with this code; the code must fit in Excel's 8,192-character formula once written as PY(…). `output` is the return type: `value` (an Excel value, the default) or `object` (a Python object)",
            has_doc,
            set
        ),
        cmd!(
            "python.output",
            "Python Output",
            [],
            Some("Ctrl+Alt+Shift+M"),
            "{cell?, mode?: object|value} sets the return type of a Python cell (default: the active cell): `object` (a Python object) or `value` (an Excel value). Without `mode` it switches to the other one. The cell keeps its code; a cached result of the old return type goes, as with `python.set`, so the cell shows #BLOCKED! until it runs",
            has_doc,
            output
        ),
    ]
}

fn target_cell(s: &Session, p: &Json) -> Result<(usize, CellRef)> {
    let sheet = target_sheet(s, p)?;
    let at = match cell_param_on(s, p, "cell", sheet)? {
        Some(c) => c,
        None => s.doc()?.selection.active,
    };
    Ok((sheet, at))
}

fn existing(s: &Session, sheet: usize, at: CellRef) -> Result<Option<PyCode>> {
    Ok(s.doc()?.wb.sheet(sheet).and_then(|sh| sh.cell(at)).and_then(|c| c.py.as_deref()).cloned())
}

fn output_mode(cmd: &str, p: &Json, key: &str) -> Result<Option<bool>> {
    match p.get(key) {
        None | Some(Json::Null) => return Ok(None),
        Some(Json::String(_)) => {}
        Some(_) => return Err(bad(cmd, format!("`{key}` is `object` or `value`"))),
    }
    match str_param(p, key) {
        None => Ok(None),
        Some(m) if m.eq_ignore_ascii_case("object") => Ok(Some(true)),
        Some(m) if m.eq_ignore_ascii_case("value") => Ok(Some(false)),
        Some(m) => Err(bad(cmd, format!("`{key}` is `object` or `value`, not `{m}`"))),
    }
}

fn set(s: &mut Session, p: &Json) -> Result<Json> {
    let (sheet, at) = target_cell(s, p)?;
    let code = match p.get("code") {
        Some(Json::String(c)) => c.as_str(),
        Some(_) => return Err(bad("python.set", "`code` must be text")),
        None => return Err(bad("python.set", "missing `code`")),
    };
    let mode = output_mode("python.set", p, "output")?;
    let old = existing(s, sheet, at)?;
    let object = mode.or(old.as_ref().map(|o| o.object)).unwrap_or(false);
    store(s, "python.set", sheet, at, PyCode { object, ..PyCode::new(code) }, old)?;
    let d = s.doc()?;
    let shown = d.wb.sheet(sheet).map(|sh| sh.value(at)).unwrap_or_default();
    Ok(json!({"cell": at.a1(), "value": crate::cmd::inspect::value_json(&shown)}))
}

/// Writes `py` into the cell, which held `old`: what `python.set` and `python.output` share. The
/// code must fit in one cell and a table header takes none; a locked cell on a protected sheet is
/// refused by the edit itself.
fn store(s: &mut Session, cmd: &str, sheet: usize, at: CellRef, mut py: PyCode, old: Option<PyCode>) -> Result<()> {
    if !py.fits() {
        return Err(bad(
            cmd,
            format!("the code doesn't fit in one cell: written as PY(…) it would be longer than Excel's {MAX_FORMULA_CHARS}-character formula limit"),
        ));
    }
    // A table header is the column's name, never a formula or code (as in Excel).
    let header =
        s.doc()?.wb.sheet(sheet).is_some_and(|sh| sh.tables.iter().any(|t| t.header_row && t.range.start.row == at.row && t.range.contains(at)));
    if header {
        return Err(EngineError::Other(format!("{} is a table header; Python cells can't go in a table's header row", at.a1())));
    }
    // Unchanged code with the same return type keeps its result (and its value); new code, or a
    // new return type, has none until it runs: a result of the other type would show a value and
    // preview that don't belong to the cell.
    let keep = old.filter(|o| o.code == py.code && o.object == py.object);
    let value = match &keep {
        Some(o) => {
            py.output = o.output.clone();
            py.stale = o.stale;
            None
        }
        None => Some(gridcraft_model::python::BLOCKED),
    };
    edit(s, |cx| {
        let sh = cx.sheet_mut(sheet)?;
        let style = sh.style_id(at);
        let value = value.unwrap_or_else(|| sh.cell(at).map(|c| c.value.clone()).unwrap_or(Value::Empty));
        // A spill the cell had (a formula's, or the old result's) is left to the recalculation,
        // which removes it and recalculates what read it.
        sh.set_cell(at, Cell { value, formula: None, style, py: Some(Arc::new(py)) });
        cx.touch(sheet, at);
        Ok(())
    })
}

/// `python.output`: the cell's return type. The code stays; a cached result belongs to the old
/// return type, so it goes, as with `python.set` and a new return type: the cell shows
/// `#BLOCKED!` until it runs, and its old spill goes with its readers recalculated.
fn output(s: &mut Session, p: &Json) -> Result<Json> {
    let (sheet, at) = target_cell(s, p)?;
    let Some(old) = existing(s, sheet, at)? else {
        return Err(EngineError::Other(format!("{} isn't a Python cell, so it has no Python output to change", at.a1())));
    };
    let object = output_mode("python.output", p, "mode")?.unwrap_or(!old.object);
    let mode = |object: bool| if object { "object" } else { "value" };
    if object != old.object {
        let py = PyCode { code: old.code.clone(), object, output: None, stale: false };
        store(s, "python.output", sheet, at, py, Some(old))?;
    }
    let d = s.doc()?;
    let shown = d.wb.sheet(sheet).map(|sh| sh.value(at)).unwrap_or_default();
    Ok(json!({"cell": at.a1(), "output": mode(object), "value": crate::cmd::inspect::value_json(&shown)}))
}
