//! Python cells under edits: what keeps, moves, rewrites or removes their code.

use std::sync::Arc;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Cell, PyCode};
use serde_json::json;

use super::s;
use crate::Session;

/// Makes `cell` on sheet `sheet` a Python cell with `code` and a cached value.
fn plant(s: &mut Session, sheet: usize, cell: &str, code: &str) {
    let d = s.doc_mut().unwrap();
    let mut wb = (*d.wb).clone();
    let sh = wb.sheet_mut(sheet).unwrap();
    let at = CellRef::parse(cell).unwrap();
    let style = sh.style_id(at);
    sh.set_cell(at, Cell { value: Value::Number(1.0), formula: None, style, py: Some(Arc::new(PyCode::new(code))) });
    d.wb = Arc::new(wb);
}

fn code(s: &Session, sheet: usize, cell: &str) -> Option<String> {
    let d = s.doc().unwrap();
    d.wb.sheet(sheet).unwrap().cell(CellRef::parse(cell).unwrap()).and_then(|c| c.py.as_ref()).map(|p| p.code.clone())
}

#[test]
fn typing_py_formulas_points_to_python_set() {
    let mut s = s();
    let e = s.execute("cell.set", json!({"cell": "A1", "input": "=PY(\"1+1\",0)"})).unwrap_err();
    assert!(e.to_string().contains("python.set"), "{e}");
    assert!(s.execute("cell.set", json!({"cell": "A1", "input": "=py"})).is_err());
    // Other functions starting with PY are not affected.
    s.execute("cell.set", json!({"cell": "A2", "input": "=PI()"})).unwrap();
}

#[test]
fn entering_or_clearing_a_python_cell_removes_its_code() {
    let mut s = s();
    plant(&mut s, 0, "A1", "1 + 1");
    plant(&mut s, 0, "A2", "2 + 2");
    plant(&mut s, 0, "A3", "3 + 3");
    let got = s.execute("cell.get", json!({"cell": "A1"})).unwrap();
    assert_eq!(got["input"], "1 + 1");
    assert_eq!(got["python"]["code"], "1 + 1");
    assert_eq!(got["python"]["output"], "value");
    s.execute("cell.set", json!({"cell": "A1", "input": "hello"})).unwrap();
    assert_eq!(code(&s, 0, "A1"), None);
    s.execute("edit.clearFormats", json!({"range": "A2"})).unwrap();
    assert_eq!(code(&s, 0, "A2").as_deref(), Some("2 + 2"), "clearing formats keeps the code");
    s.execute("edit.clearContents", json!({"range": "A2"})).unwrap();
    assert_eq!(code(&s, 0, "A2"), None);
    s.execute("edit.clearAll", json!({"range": "A3"})).unwrap();
    assert_eq!(code(&s, 0, "A3"), None);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(code(&s, 0, "A3").as_deref(), Some("3 + 3"), "undo brings the code back");
}

#[test]
fn copy_fill_and_sort_carry_the_code_and_paste_values_drops_it() {
    let mut s = s();
    plant(&mut s, 0, "A1", "xl('B1') * 2");
    s.execute("edit.copy", json!({"range": "A1"})).unwrap();
    s.execute("edit.paste", json!({"at": "C1"})).unwrap();
    assert_eq!(code(&s, 0, "C1").as_deref(), Some("xl('B1') * 2"), "the code is copied as it is");
    s.execute("edit.pasteSpecial", json!({"at": "D1", "what": "values"})).unwrap();
    assert_eq!(code(&s, 0, "D1"), None);
    assert_eq!(super::v(&s, "D1"), Value::Number(1.0), "the cached value is pasted");
    s.execute("edit.fillDown", json!({"range": "A1:A3"})).unwrap();
    assert_eq!(code(&s, 0, "A3").as_deref(), Some("xl('B1') * 2"));
    // Sorting moves Python cells with their rows.
    s.execute("range.setValues", json!({"range": "E1", "values": [[3], [1], [2]]})).unwrap();
    plant(&mut s, 0, "F1", "'three'");
    plant(&mut s, 0, "F2", "'one'");
    s.execute("data.sort", json!({"range": "E1:F3", "keys": [{"column": "E"}]})).unwrap();
    assert_eq!(code(&s, 0, "F1").as_deref(), Some("'one'"));
    assert_eq!(code(&s, 0, "F3").as_deref(), Some("'three'"));
}

#[test]
fn xl_references_follow_inserts_moves_and_renames() {
    let mut s = s();
    s.execute("home.insertSheet", json!({})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    let other = s.doc().unwrap().wb.sheet(1).unwrap().name.clone();
    plant(&mut s, 0, "C5", &format!("a = xl(\"A1:A3\")\nb = xl('{other}!B2', headers=True)\nc = xl(dynamic)\n# xl(\"A1\") stays in comments"));
    s.execute("home.insertRows", json!({"rows": "2:2"})).unwrap();
    let c = code(&s, 0, "C6").unwrap();
    assert!(c.starts_with("a = xl(\"A1:A4\")\n"), "{c}");
    assert!(c.contains(&format!("xl('{other}!B2', headers=True)")), "{c}");
    assert!(c.contains("# xl(\"A1\") stays"), "{c}");
    s.execute("sheet.rename", json!({"sheet": 1, "name": "Data"})).unwrap();
    assert!(code(&s, 0, "C6").unwrap().contains("xl('Data!B2', headers=True)"));
    // Cutting the cells a reference names moves the reference with them.
    s.execute("edit.cut", json!({"range": "A1:A4"})).unwrap();
    s.execute("edit.paste", json!({"at": "H1"})).unwrap();
    assert!(code(&s, 0, "C6").unwrap().starts_with("a = xl(\"H1:H4\")"), "{:?}", code(&s, 0, "C6"));
    // Deleting the referenced cells leaves #REF!, which fails when the code runs.
    // The selection is elsewhere, so only the named column goes.
    s.execute("selection.set", json!({"range": "A1"})).unwrap();
    s.execute("home.deleteColumns", json!({"cols": "H:H"})).unwrap();
    assert!(code(&s, 0, "C6").unwrap().starts_with("a = xl(\"#REF!\")"), "{:?}", code(&s, 0, "C6"));
}

#[test]
fn a_rename_to_a_name_that_needs_quotes_rewrites_single_quoted_references() {
    let mut s = s();
    s.execute("home.insertSheet", json!({})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    let other = s.doc().unwrap().wb.sheet(1).unwrap().name.clone();
    plant(&mut s, 0, "A1", &format!("a = xl('{other}!B2')\nb = xl(\"{other}!C3\")"));
    s.execute("sheet.rename", json!({"sheet": 1, "name": "Q4 2026"})).unwrap();
    assert_eq!(code(&s, 0, "A1").unwrap(), "a = xl(\"'Q4 2026'!B2\")\nb = xl(\"'Q4 2026'!C3\")");
    // A name with both quotes in it: the literal's own quote is escaped.
    s.execute("sheet.rename", json!({"sheet": 1, "name": "Bob's \"Q4\""})).unwrap();
    let c = code(&s, 0, "A1").unwrap();
    assert_eq!(c, "a = xl(\"'Bob''s \\\"Q4\\\"'!B2\")\nb = xl(\"'Bob''s \\\"Q4\\\"'!C3\")");
    let refs: Vec<Option<String>> = gridcraft_model::python::xl_calls(&c).into_iter().map(|x| x.reference).collect();
    assert_eq!(refs, [Some("'Bob''s \"Q4\"'!B2".to_string()), Some("'Bob''s \"Q4\"'!C3".to_string())]);
    // And back to a plain name.
    s.execute("sheet.rename", json!({"sheet": 1, "name": "Data"})).unwrap();
    assert_eq!(code(&s, 0, "A1").unwrap(), "a = xl(\"Data!B2\")\nb = xl(\"Data!C3\")");
}

#[test]
fn a_cut_to_another_sheet_keeps_references_on_the_source_sheet() {
    let mut s = s();
    s.execute("home.insertSheet", json!({})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    let (src, dest) = {
        let wb = &s.doc().unwrap().wb;
        (wb.sheet(0).unwrap().name.clone(), wb.sheet(1).unwrap().name.clone())
    };
    plant(&mut s, 0, "A1", "a = xl(\"B5\")\nb = xl('A2')");
    plant(&mut s, 0, "A2", "1");
    s.execute("edit.cut", json!({"range": "A1:A2"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 1})).unwrap();
    s.execute("edit.paste", json!({"at": "C1"})).unwrap();
    // B5 stays on the source sheet; A2 moved with the cut, so it follows to the destination.
    assert_eq!(code(&s, 1, "C1").unwrap(), format!("a = xl(\"{src}!B5\")\nb = xl('{dest}!C2')"));
    assert_eq!(code(&s, 0, "A1"), None);
    // Within one sheet nothing is qualified.
    s.execute("edit.cut", json!({"range": "C1"})).unwrap();
    s.execute("edit.paste", json!({"at": "E1"})).unwrap();
    assert_eq!(code(&s, 1, "E1").unwrap(), format!("a = xl(\"{src}!B5\")\nb = xl('{dest}!C2')"));
    // A copy to another sheet still carries the code as it is.
    plant(&mut s, 0, "A9", "xl(\"B5\")");
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    s.execute("edit.copy", json!({"range": "A9"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 1})).unwrap();
    s.execute("edit.paste", json!({"at": "A9"})).unwrap();
    assert_eq!(code(&s, 1, "A9").as_deref(), Some("xl(\"B5\")"));
}

#[test]
fn goal_seek_and_solver_refuse_to_change_a_python_cell() {
    let mut s = s();
    plant(&mut s, 0, "A1", "2");
    s.execute("cell.set", json!({"cell": "B1", "input": "=A1*2"})).unwrap();
    let e = s.execute("data.goalSeek", json!({"set": "B1", "to": 10, "changing": "A1"})).unwrap_err();
    assert!(e.to_string().contains("must contain a value"), "{e}");
    assert_eq!(code(&s, 0, "A1").as_deref(), Some("2"), "the code is kept");
    // The set cell must be a formula: a Python cell isn't one this build can recalculate.
    s.execute("cell.set", json!({"cell": "C1", "input": "3"})).unwrap();
    assert!(s.execute("data.goalSeek", json!({"set": "A1", "to": 10, "changing": "C1"})).is_err());
    let e = s.execute("data.solver", json!({"objective": "B1", "goal": "max", "variables": "A1"})).unwrap_err();
    assert!(e.to_string().contains("constants"), "{e}");
    assert_eq!(code(&s, 0, "A1").as_deref(), Some("2"));
}

#[test]
fn text_to_columns_keeps_a_py_piece_as_text() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "'=PY(\"x\");7"})).unwrap();
    assert_eq!(super::v(&s, "A1"), Value::text("=PY(\"x\");7"));
    s.execute("data.textToColumns", json!({"range": "A1", "delimiters": [";"]})).unwrap();
    // A quote in the middle of a field is kept, as in Excel.
    assert_eq!(super::v(&s, "A1"), Value::text("=PY(\"x\")"), "the piece is kept, not cleared");
    assert_eq!(code(&s, 0, "A1"), None, "and it is not a Python cell");
    assert_eq!(super::v(&s, "B1"), Value::Number(7.0));
}

#[test]
fn python_cells_survive_native_json() {
    let mut s = s();
    plant(&mut s, 0, "B2", "x = 1\nx");
    let wb = s.doc().unwrap().wb.clone();
    let json = serde_json::to_string(&*wb).unwrap();
    let back: gridcraft_model::Workbook = serde_json::from_str(&json).unwrap();
    let cell = back.sheet(0).unwrap().cell(CellRef::parse("B2").unwrap()).unwrap();
    assert_eq!(cell.py.as_ref().unwrap().code, "x = 1\nx");
    // Cells written before Python existed have no `py` field.
    let plain: Cell = serde_json::from_value(json!({"value": {"t": "Number", "v": 2.0}, "formula": null, "style": 0})).unwrap();
    assert!(plain.py.is_none());
    assert!(!serde_json::to_string(&plain).unwrap().contains("py"), "no field is written for ordinary cells");
}

#[test]
fn new_errors_parse_and_have_error_type_numbers() {
    use gridcraft_core::CellError;
    for (text, e) in
        [("#PYTHON!", CellError::Python), ("#BUSY!", CellError::Busy), ("#TIMEOUT!", CellError::Timeout), ("#BLOCKED!", CellError::Blocked)]
    {
        assert_eq!(CellError::parse(text), Some(e));
        assert_eq!(e.as_str(), text);
    }
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "=ERROR.TYPE(#BLOCKED!)"})).unwrap();
    assert_eq!(super::v(&s, "A1"), Value::Number(11.0));
    s.execute("cell.set", json!({"cell": "A2", "input": "=ISERROR(#PYTHON!)"})).unwrap();
    assert_eq!(super::v(&s, "A2"), Value::Bool(true));
}

#[test]
fn go_to_special_counts_a_python_cell_as_a_formula_not_a_constant() {
    let mut s = s();
    plant(&mut s, 0, "A1", "1 + 1");
    s.execute("cell.set", json!({"cell": "A2", "input": "=1+1"})).unwrap();
    s.execute("cell.set", json!({"cell": "A3", "input": "7"})).unwrap();
    s.execute("selection.set", json!({"range": "A1"})).unwrap();
    let r = s.execute("edit.goToSpecial", json!({"kind": "formulas"})).unwrap();
    assert_eq!(r["count"], 2, "{r}");
    assert_eq!(r["selection"], "A1:A2", "{r}");
    s.execute("selection.set", json!({"range": "A1"})).unwrap();
    let r = s.execute("edit.goToSpecial", json!({"kind": "constants"})).unwrap();
    assert_eq!(r["count"], 1, "{r}");
    assert_eq!(r["selection"], "A3", "{r}");
}

#[test]
fn a_macro_counts_python_code_as_data_and_a_blocked_spill_as_no_write() {
    let mut s = s();
    // F2 shows 1, its cached value: a step that enters 1 there still replaces the code.
    plant(&mut s, 0, "F2", "1");
    // H2's code shows #BLOCKED!, as any other new code there would.
    s.execute("python.set", json!({"cell": "H2", "code": "'a'"})).unwrap();
    // J2's result spills over J2:J3. A step that writes J3 blocks it, which doesn't write J2.
    s.execute("python.set", json!({"cell": "J2", "code": "[5, 6]"})).unwrap();
    {
        let d = s.doc_mut().unwrap();
        let wb = Arc::make_mut(&mut d.wb);
        let cell = wb.sheet_mut(0).unwrap().cells.get_mut(CellRef::parse("J2").unwrap()).unwrap();
        let data = vec![Value::Number(5.0), Value::Number(6.0)];
        let value = Value::Array(Arc::new(gridcraft_core::Array::new(2, 1, data).unwrap()));
        Arc::make_mut(cell.py.as_mut().unwrap()).output =
            Some(gridcraft_model::PyOutput { value, type_name: "list".into(), preview: String::new(), convertible: true });
    }
    s.execute("formulas.calculateNow", json!({})).unwrap();
    assert_eq!(super::v(&s, "J3"), Value::Number(6.0));
    super::macros::put(
        &mut s,
        json!({"name": "Py", "steps": [
            {"command": "cell.set", "params": {"cell": "F2", "input": "1"}},
            {"command": "python.set", "params": {"cell": "H2", "code": "'b'"}},
            {"command": "cell.set", "params": {"cell": "J3", "input": "x"}}
        ]}),
    );
    let c = s.execute("macro.check", json!({"name": "Py"})).unwrap();
    assert_eq!(c["footprint"]["sheets"][0]["cells"], json!(["F2", "H2", "J3"]), "{c}");
    assert_eq!(c["footprint"]["sheets"][0]["conflicts"], json!(["F2", "H2"]), "{c}");
    let e = s.execute("macro.run", json!({"name": "Py", "onConflict": "fail"})).unwrap_err().to_string();
    assert!(e.contains("F2") && e.contains("H2") && !e.contains("J2"), "{e}");
    assert_eq!(code(&s, 0, "F2").as_deref(), Some("1"), "nothing was changed");
}

#[test]
fn replace_edits_a_python_cells_code_and_keeps_it_a_python_cell() {
    let mut s = s();
    plant(&mut s, 0, "A1", "x = 1\nx");
    {
        let d = s.doc_mut().unwrap();
        let mut wb = (*d.wb).clone();
        let cell = wb.sheet_mut(0).unwrap().cells.get_mut(CellRef::parse("A1").unwrap()).unwrap();
        Arc::make_mut(cell.py.as_mut().unwrap()).object = true;
        d.wb = Arc::new(wb);
    }
    let r = s.execute("edit.replace", json!({"what": "x", "with": "y"})).unwrap();
    assert_eq!(r["replaced"], 1);
    assert_eq!(code(&s, 0, "A1").as_deref(), Some("y = 1\ny"));
    let d = s.doc().unwrap();
    let cell = d.wb.sheet(0).unwrap().cell(CellRef::parse("A1").unwrap()).unwrap();
    assert!(cell.py.as_ref().unwrap().object, "the return type is kept");
    assert_eq!(cell.value, Value::Error(gridcraft_core::CellError::Blocked));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(code(&s, 0, "A1").as_deref(), Some("x = 1\nx"));
}

#[test]
fn replace_leaves_python_code_that_would_no_longer_fit() {
    let mut s = s();
    let long = "a".repeat(gridcraft_model::python::MAX_FORMULA_CHARS / 2);
    plant(&mut s, 0, "A1", &long);
    let r = s.execute("edit.replace", json!({"what": "a", "with": "bb"})).unwrap();
    assert_eq!(r["replaced"], 0);
    assert_eq!(r["skipped"], 1);
    assert_eq!(code(&s, 0, "A1").as_deref(), Some(long.as_str()));
}

#[test]
fn a_hidden_python_cell_on_a_protected_sheet_hides_its_code() {
    let mut s = s();
    s.execute("python.set", json!({"cell": "A1", "code": "secret = 1\nsecret"})).unwrap();
    s.execute("python.set", json!({"cell": "B1", "code": "'shown'"})).unwrap();
    s.execute("home.formatCells", json!({"range": "A1", "style": {"protection": {"locked": true, "hidden": true}}})).unwrap();
    let hidden = |s: &Session, a: &str| {
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        crate::display::formula_hidden(&d.wb, sh, CellRef::parse(a).unwrap())
    };
    assert!(!hidden(&s, "A1"), "an unprotected sheet hides nothing");
    s.execute("review.protectSheet", json!({})).unwrap();
    assert!(hidden(&s, "A1"), "Hidden on a protected sheet hides the code, as it hides a formula");
    assert!(!hidden(&s, "B1"));
    // Editing it is refused where a formula's would be: the cell is locked.
    assert!(s.execute("python.set", json!({"cell": "A1", "code": "2"})).is_err());
    assert_eq!(code(&s, 0, "A1").as_deref(), Some("secret = 1\nsecret"));
}

#[test]
fn every_merge_keeps_only_the_upper_left_cells_python_code() {
    let mut s = s();
    for (cmd, row) in [("home.mergeAcross", 1), ("home.mergeCells", 2), ("home.mergeCenter", 3)] {
        plant(&mut s, 0, &format!("A{row}"), "'kept'");
        plant(&mut s, 0, &format!("B{row}"), "'merged away'");
        s.execute(cmd, json!({"range": format!("A{row}:B{row}")})).unwrap();
        assert_eq!(code(&s, 0, &format!("A{row}")).as_deref(), Some("'kept'"), "{cmd}");
        assert_eq!(code(&s, 0, &format!("B{row}")), None, "{cmd}: the hidden cell is no Python cell");
        assert_eq!(super::v(&s, &format!("B{row}")), Value::Empty, "{cmd}");
    }
}

#[test]
fn unmerging_lets_a_blocked_python_result_spill_again() {
    let mut s = s();
    s.execute("python.set", json!({"cell": "A1", "code": "[5, 6]"})).unwrap();
    {
        let d = s.doc_mut().unwrap();
        let wb = Arc::make_mut(&mut d.wb);
        let cell = wb.sheet_mut(0).unwrap().cells.get_mut(CellRef::parse("A1").unwrap()).unwrap();
        let value = Value::Array(Arc::new(gridcraft_core::Array::new(2, 1, vec![Value::Number(5.0), Value::Number(6.0)]).unwrap()));
        Arc::make_mut(cell.py.as_mut().unwrap()).output =
            Some(gridcraft_model::PyOutput { value, type_name: "list".into(), preview: String::new(), convertible: true });
    }
    s.execute("formulas.calculateNow", json!({})).unwrap();
    assert_eq!(super::v(&s, "A2"), Value::Number(6.0));
    s.execute("home.mergeCells", json!({"range": "A2:B2"})).unwrap();
    assert_eq!(super::v(&s, "A1"), Value::Error(gridcraft_core::CellError::Spill), "a merged cell blocks the spill");
    s.execute("home.unmergeCells", json!({"range": "A2:B2"})).unwrap();
    assert_eq!((super::v(&s, "A1"), super::v(&s, "A2")), (Value::Number(5.0), Value::Number(6.0)), "it spills again");
}

#[test]
fn save_model_asks_before_it_writes_over_a_python_cell_that_shows_nothing() {
    let mut s = s();
    for (cell, input) in [("B1", "=5*A1+4*A2"), ("B2", "=6*A1+4*A2")] {
        s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
    }
    // H2 is a Python cell whose cached value is empty: its code is still content.
    {
        let d = s.doc_mut().unwrap();
        let wb = Arc::make_mut(&mut d.wb);
        let sh = wb.sheet_mut(0).unwrap();
        sh.set_cell(
            CellRef::parse("H2").unwrap(),
            Cell { value: Value::Empty, formula: None, style: Default::default(), py: Some(Arc::new(PyCode::new("None"))) },
        );
    }
    let model = json!({"objective": "B1", "variables": "A1:A2", "constraints": ["B2 <= 24"]});
    let e = s.execute("data.solverSaveModel", json!({"to": "H1", "model": model})).unwrap_err().to_string();
    assert!(e.contains(crate::cmd::analysis::OVERWRITE_ERROR) && e.contains("H2"), "{e}");
    assert_eq!(code(&s, 0, "H2").as_deref(), Some("None"), "nothing was written");
}

/// A1 is a Python cell whose kept result `[5, 6]` spills over A1:A2, and B1 reads A2.
pub(super) fn spill_with_a_reader() -> crate::Session {
    let mut s = s();
    s.execute("python.set", json!({"cell": "A1", "code": "[5, 6]"})).unwrap();
    {
        let d = s.doc_mut().unwrap();
        let wb = Arc::make_mut(&mut d.wb);
        let cell = wb.sheet_mut(0).unwrap().cells.get_mut(CellRef::parse("A1").unwrap()).unwrap();
        let value = Value::Array(Arc::new(gridcraft_core::Array::new(2, 1, vec![Value::Number(5.0), Value::Number(6.0)]).unwrap()));
        Arc::make_mut(cell.py.as_mut().unwrap()).output =
            Some(gridcraft_model::PyOutput { value, type_name: "list".into(), preview: String::new(), convertible: true });
    }
    s.execute("formulas.calculateNow", json!({})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=SUM(A2)"})).unwrap();
    assert_eq!(super::v(&s, "B1"), Value::Number(6.0));
    s
}

#[test]
fn new_python_code_recalculates_the_readers_of_its_old_spill() {
    let mut s = spill_with_a_reader();
    s.execute("python.set", json!({"cell": "A1", "code": "[7, 8]"})).unwrap();
    assert_eq!(super::v(&s, "A2"), Value::Empty, "new code has no result yet");
    assert_eq!(super::v(&s, "B1"), Value::Number(0.0), "B1 read the old spill");
    // The same code again keeps its result, and its spill.
    let mut s = spill_with_a_reader();
    s.execute("python.set", json!({"cell": "A1", "code": "[5, 6]"})).unwrap();
    assert_eq!((super::v(&s, "A2"), super::v(&s, "B1")), (Value::Number(6.0), Value::Number(6.0)));
}

#[test]
fn replace_in_python_code_recalculates_the_readers_of_its_old_spill() {
    let mut s = spill_with_a_reader();
    let r = s.execute("edit.replace", json!({"what": "[5,", "with": "[7,"})).unwrap();
    assert_eq!(r["replaced"], 1);
    assert_eq!(super::v(&s, "A2"), Value::Empty);
    assert_eq!(super::v(&s, "B1"), Value::Number(0.0), "B1 read the old spill");
}

#[test]
fn clearing_pasting_over_or_blocking_a_python_spill_recalculates_its_readers() {
    let mut s = spill_with_a_reader();
    s.execute("edit.clearContents", json!({"range": "A1"})).unwrap();
    assert_eq!(super::v(&s, "B1"), Value::Number(0.0), "clear");
    let mut s = spill_with_a_reader();
    s.execute("cell.set", json!({"cell": "D1", "input": "9"})).unwrap();
    s.execute("edit.copy", json!({"range": "D1"})).unwrap();
    s.execute("edit.paste", json!({"at": "A1"})).unwrap();
    assert_eq!(super::v(&s, "B1"), Value::Number(0.0), "paste");
    let mut s = spill_with_a_reader();
    s.execute("home.mergeCells", json!({"range": "A2:A3"})).unwrap();
    assert_eq!(super::v(&s, "A1"), Value::Error(gridcraft_core::CellError::Spill));
    assert_eq!(super::v(&s, "B1"), Value::Number(0.0), "merge");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(super::v(&s, "B1"), Value::Number(6.0), "undo");
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(super::v(&s, "B1"), Value::Number(0.0), "redo");
}

/// `prefix` and a comment padded to the longest code that still fits in one cell.
fn at_the_limit(prefix: &str) -> String {
    let with = |n: usize| format!("{prefix}\n#{}", "x".repeat(n));
    let (mut lo, mut hi) = (0, gridcraft_model::python::MAX_FORMULA_CHARS);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if PyCode::new(&with(mid)).fits() { lo = mid } else { hi = mid - 1 }
    }
    with(lo)
}

#[test]
fn an_edit_that_would_make_python_code_too_long_is_refused() {
    let mut s = s();
    s.execute("home.insertSheet", json!({})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    let other = s.doc().unwrap().wb.sheet(1).unwrap().name.clone();
    let refused = |e: crate::EngineError, cell: &str| {
        let e = e.to_string();
        assert!(e.contains(&format!("!{cell} longer than Excel's 8192-character")) && e.contains("nothing was changed"), "{e}");
    };
    // A sheet rename that lengthens a reference.
    let a1 = at_the_limit(&format!("a = xl(\"{other}!B2\")"));
    plant(&mut s, 0, "A1", &a1);
    refused(s.execute("sheet.rename", json!({"sheet": 1, "name": "A much longer sheet name"})).unwrap_err(), "A1");
    assert_eq!(code(&s, 0, "A1").unwrap(), a1);
    assert_eq!(s.doc().unwrap().wb.sheet(1).unwrap().name, other);
    // One of the same length is made.
    let same = "Z".repeat(other.chars().count());
    s.execute("sheet.rename", json!({"sheet": 1, "name": same})).unwrap();
    assert_eq!(code(&s, 0, "A1").unwrap(), a1.replace(&other, &same));
    // Code short of the limit still follows each edit.
    plant(&mut s, 0, "A7", "d = xl(\"Z1\")");
    s.execute("selection.set", json!({"range": "B10"})).unwrap();
    s.execute("home.insertColumns", json!({"cols": "Y:Y"})).unwrap();
    assert_eq!(code(&s, 0, "A7").unwrap(), "d = xl(\"AA1\")");
    // An inserted column that turns Z1 into AA1.
    let a3 = at_the_limit("b = xl(\"Z1\")");
    plant(&mut s, 0, "A3", &a3);
    s.execute("cell.set", json!({"cell": "D1", "input": "kept"})).unwrap();
    s.execute("selection.set", json!({"range": "C1"})).unwrap();
    refused(s.execute("home.insertColumns", json!({"cols": "C:C"})).unwrap_err(), "A3");
    assert_eq!(code(&s, 0, "A3").unwrap(), a3);
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().value(CellRef::parse("D1").unwrap()), Value::text("kept"));
    // A cut to another sheet, which qualifies B9 with the source sheet.
    let a5 = at_the_limit("c = xl(\"B9\")");
    plant(&mut s, 0, "A5", &a5);
    s.execute("edit.cut", json!({"range": "A5"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 1})).unwrap();
    refused(s.execute("edit.paste", json!({"at": "C1"})).unwrap_err(), "C1");
    assert_eq!(code(&s, 0, "A5").unwrap(), a5);
    assert_eq!(code(&s, 1, "C1"), None);
}

/// Makes `cell` a Python cell whose cached value is empty, as a file's `PY` formula with no cached
/// value reads.
fn plant_empty(s: &mut Session, cell: &str) {
    plant(s, 0, cell, "x = 1");
    let d = s.doc_mut().unwrap();
    let mut wb = (*d.wb).clone();
    if let Some(c) = wb.sheet_mut(0).unwrap().cells.get_mut(CellRef::parse(cell).unwrap()) {
        c.value = Value::Empty;
    }
    d.wb = Arc::new(wb);
}

#[test]
fn python_code_with_an_empty_value_counts_as_content() {
    // A report or a PivotTable asks before replacing it.
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Sales"], ["East", 1], ["West", 2], ["East", 3]]})).unwrap();
    plant_empty(&mut s, "D2");
    let report = json!({"tool": "Descriptive Statistics", "range": "B1:B4", "labels": true, "output": "D1"});
    assert!(s.execute("data.analysisToolPak", report).is_err(), "the report asks for overwrite");
    plant_empty(&mut s, "H4");
    let pivot = s.execute("insert.pivotTable", json!({"destination": "Sheet1!H3", "rows": ["Region"], "values": ["Sales"]}));
    assert!(pivot.is_err_and(|e| e.to_string().contains("already data")));
    assert_eq!((code(&s, 0, "D2"), code(&s, 0, "H4")), (Some("x = 1".into()), Some("x = 1".into())), "the code is kept");
    // Cells aren't pushed off the sheet over it.
    plant_empty(&mut s, "K1048576");
    assert!(s.execute("home.insertCells", json!({"range": "K1", "shift": "down"})).is_err());
    assert_eq!(code(&s, 0, "K1048576"), Some("x = 1".into()));
    // A table without a header row doesn't push it off the sheet either.
    s.execute("range.setValues", json!({"range": "X1", "values": [[1], [2]]})).unwrap();
    plant_empty(&mut s, "X1048576");
    assert!(s.execute("insert.table", json!({"range": "X1:X2", "header": false})).is_err());
    assert_eq!(code(&s, 0, "X1048576"), Some("x = 1".into()));
    // It is part of the data region.
    s.execute("range.setValues", json!({"range": "P1", "values": [[1], [""], [3]]})).unwrap();
    plant_empty(&mut s, "P2");
    let sh = s.doc().unwrap().wb.sheet(0).unwrap().clone();
    assert_eq!(crate::selection::current_region(&sh, CellRef::parse("P1").unwrap()).a1(), "P1:P3");
    // Pasted into part of a legacy array, it is refused like any content.
    s.execute("selection.set", json!({"range": "R1:R3"})).unwrap();
    s.execute("cell.set", json!({"input": "=5", "array": true})).unwrap();
    plant_empty(&mut s, "T1");
    s.execute("edit.copy", json!({"range": "T1"})).unwrap();
    assert!(s.execute("edit.paste", json!({"at": "R2"})).is_err_and(|e| e.to_string().contains("part of an array")));
}

/// Renaming a defined name or a table renames it in Python cells' `xl()` references too.
#[test]
fn xl_references_follow_name_and_table_renames() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1:B3", "values": [["k", "v"], ["a", 1], ["b", 2]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B3"})).unwrap();
    let table = s.doc().unwrap().wb.sheets[0].tables[0].name.clone();
    s.execute("formulas.defineName", json!({"name": "Opts", "refersTo": "=Sheet1!$A$2:$A$3"})).unwrap();
    plant(&mut s, 0, "E1", &format!("a = xl(\"Opts\")\nb = xl(\"{table}[v]\")"));
    s.execute("formulas.defineName", json!({"name": "Choices", "oldName": "Opts", "refersTo": "=Sheet1!$A$2:$A$3"})).unwrap();
    assert_eq!(code(&s, 0, "E1").as_deref(), Some(format!("a = xl(\"Choices\")\nb = xl(\"{table}[v]\")").as_str()));
    s.execute("table.rename", json!({"table": table, "name": "Data"})).unwrap();
    assert_eq!(code(&s, 0, "E1").as_deref(), Some("a = xl(\"Choices\")\nb = xl(\"Data[v]\")"));
}

#[test]
fn inserting_a_link_without_text_preserves_an_empty_python_cell() {
    let mut s = s();
    plant_empty(&mut s, "A1");
    s.execute("insert.link", json!({"cell": "A1", "target": "https://example.org"})).unwrap();
    assert_eq!(code(&s, 0, "A1"), Some("x = 1".into()));
    let d = s.doc().unwrap();
    let sh = d.wb.sheet(0).unwrap();
    let at = CellRef::parse("A1").unwrap();
    assert_eq!(sh.value(at), Value::Empty);
    assert_eq!(sh.hyperlinks.get(&at).map(|link| link.target.as_str()), Some("https://example.org"));
}
