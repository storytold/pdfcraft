//! Form controls: buttons, sliders and spin buttons (insert, format, assign, setValue, press).

use gridcraft_core::Value;
use gridcraft_model::{ControlAction, ControlKind, FormControl, MacroInput};
use serde_json::{Value as Json, json};

use super::{s, v};
use crate::{EngineError, Session, Trust};

fn id(r: &Json) -> u64 {
    r["control"].as_u64().unwrap()
}

fn control(s: &Session, id: u64) -> FormControl {
    s.doc().unwrap().wb.active().unwrap().controls.iter().find(|c| c.id as u64 == id).cloned().unwrap()
}

fn undo_len(s: &Session) -> usize {
    s.doc().unwrap().undo.len()
}

fn err(r: crate::Result<Json>) -> String {
    r.unwrap_err().to_string()
}

#[test]
fn numbers_are_whole_and_each_rule_has_its_own_error() {
    let mut s = s();
    let ok = s.execute("insert.slider", json!({"at": "D2", "min": 10, "max": 50, "step": 5, "page": 10})).unwrap();
    let c = control(&s, id(&ok));
    assert_eq!((c.kind, c.min, c.max, c.step, c.page, c.value), (ControlKind::Slider, 10, 50, 5, 10, 10));
    for (p, needle) in [
        (json!({"min": 2.5}), "`min` must be a whole number from 0 to 30000"),
        (json!({"min": -1}), "`min` must be a whole number"),
        (json!({"max": 30001}), "`max` must be a whole number from 0 to 30000"),
        (json!({"step": "two"}), "`step` must be a whole number"),
        (json!({"max": 1e300}), "`max` must be a whole number"),
        (json!({"min": 50, "max": 50}), "Maximum value (50) must be greater than Minimum value (50)"),
        (json!({"min": 60, "max": 50}), "must be greater than Minimum"),
        (json!({"step": 0}), "Incremental change must be at least 1"),
        (json!({"min": 0, "max": 10, "step": 11}), "Incremental change (11) can be at most Maximum minus Minimum (10)"),
        (json!({"step": 5, "page": 2}), "Page change (2) must be at least the incremental change (5)"),
        (json!({"link": "not a cell"}), "isn't a cell of this workbook"),
        (json!({"link": "Nowhere!A1"}), "isn't a cell of this workbook"),
        (json!({"link": "A1:B2"}), "isn't a cell"),
    ] {
        let e = err(s.execute("insert.slider", p.clone()));
        assert!(e.contains(needle), "{p}: {e}");
    }
    // A spin button has no page change, so a page below the step is fine.
    s.execute("insert.spinButton", json!({"step": 5, "page": 2})).unwrap();
    // Numbers in text are numbers.
    let n = id(&s.execute("insert.spinButton", json!({"min": "3", "max": "9"})).unwrap());
    let c = control(&s, n);
    assert_eq!((c.min, c.max, c.value), (3, 9, 3));
    // A larger step pulls a slider's default page up with it.
    let n = id(&s.execute("insert.slider", json!({"max": 1000, "step": 50})).unwrap());
    let c = control(&s, n);
    assert_eq!(c.page, 50);
    // The model checks the same rules.
    let bad = FormControl { kind: ControlKind::Spin, min: 0, max: 30_001, ..Default::default() };
    assert!(bad.check_numbers().is_err());
    assert!(FormControl { kind: ControlKind::Button, max: 0, ..Default::default() }.check_numbers().is_ok());
}

#[test]
fn set_value_clamps_snaps_and_writes_the_link_as_one_undo_step() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "C1", "input": "=B1*2"})).unwrap();
    let c = id(&s.execute("insert.slider", json!({"link": "B1", "min": 10, "max": 100, "step": 5})).unwrap());
    assert_eq!(control(&s, c).link.as_deref(), Some("$B$1"));
    let before = undo_len(&s);
    for (given, want) in [(37.0, 35.0), (38.0, 40.0), (-5.0, 10.0), (1e9, 100.0), (99.0, 100.0), (12.4, 10.0)] {
        let r = s.execute("control.setValue", json!({"id": c, "value": given})).unwrap();
        assert_eq!(r["value"], json!(want as u32), "{given}");
        assert_eq!(v(&s, "B1"), Value::Number(want), "{given}");
        assert_eq!(v(&s, "C1"), Value::Number(want * 2.0), "dependents recalculate");
        assert_eq!(control(&s, c).value, want as u32);
    }
    assert_eq!(undo_len(&s), before + 5, "one undo step per change (99 snaps to the 100 it holds)");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "B1"), Value::Number(100.0));
    assert_eq!(control(&s, c).value, 100);
    // Setting the value it already has adds no step.
    let n = undo_len(&s);
    s.execute("control.setValue", json!({"id": c, "value": 100})).unwrap();
    assert_eq!(undo_len(&s), n);
    for p in [json!({"id": c}), json!({"id": c, "value": "x"}), json!({"id": 999, "value": 1}), json!({"value": 1})] {
        assert!(s.execute("control.setValue", p).is_err());
    }
    // A button has no value.
    let b = id(&s.execute("insert.button", json!({})).unwrap());
    assert!(err(s.execute("control.setValue", json!({"id": b, "value": 1}))).contains("control.press"));
}

#[test]
fn editing_the_linked_cell_moves_the_control() {
    let mut s = s();
    let c = id(&s.execute("insert.spinButton", json!({"link": "$B$2", "max": 20, "step": 2})).unwrap());
    s.execute("cell.set", json!({"cell": "B2", "input": "7"})).unwrap();
    assert_eq!(control(&s, c).value, 8, "snapped to the step");
    s.execute("cell.set", json!({"cell": "B2", "input": "500"})).unwrap();
    assert_eq!(control(&s, c).value, 20, "clamped");
    s.execute("cell.set", json!({"cell": "B2", "input": "text"})).unwrap();
    assert_eq!(control(&s, c).value, 20, "text leaves it where it was");
    // A new control starts at what its cell holds.
    s.execute("cell.set", json!({"cell": "D4", "input": "12"})).unwrap();
    let d = id(&s.execute("insert.slider", json!({"link": "D4", "max": 20})).unwrap());
    assert_eq!(control(&s, d).value, 12);
    // A linked cell on another sheet.
    s.execute("home.insertSheet", json!({"name": "Data Sheet"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    let e = id(&s.execute("insert.slider", json!({"link": "'Data Sheet'!a1", "value": 30})).unwrap());
    assert_eq!(control(&s, e).link.as_deref(), Some("'Data Sheet'!$A$1"));
    let wb = &s.doc().unwrap().wb;
    let other = wb.sheet(wb.sheet_index("Data Sheet").unwrap()).unwrap();
    assert_eq!(other.cell(gridcraft_core::CellRef::new(0, 0)).unwrap().value, Value::Number(30.0));
}

#[test]
fn format_replies_with_the_value_a_new_link_gives() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "C5", "input": "30"})).unwrap();
    let c = id(&s.execute("insert.slider", json!({"link": "B1", "max": 100, "value": 10})).unwrap());
    // Relinked to a cell that holds another number, the control takes that number, and the reply
    // says so, as control.list does.
    let r = s.execute("control.format", json!({"id": c, "link": "C5"})).unwrap();
    assert_eq!(control(&s, c).value, 30);
    assert_eq!(r["value"], json!(30), "{r}");
    assert_eq!(r["link"], json!("$C$5"), "{r}");
    // A given value is written to the new link and replied as given.
    let r = s.execute("control.format", json!({"id": c, "link": "D6", "value": 44})).unwrap();
    assert_eq!((r["value"].clone(), v(&s, "D6")), (json!(44), Value::Number(44.0)));
}

#[test]
fn links_follow_inserted_rows() {
    let mut s = s();
    let c = id(&s.execute("insert.slider", json!({"at": "E2", "link": "B3", "value": 40})).unwrap());
    s.execute("selection.set", json!({"range": "1:1"})).unwrap();
    s.execute("home.insertRows", json!({})).unwrap();
    let ctl = control(&s, c);
    assert_eq!(ctl.link.as_deref(), Some("$B$4"));
    assert_eq!(ctl.anchor.cell.a1(), "E3");
    assert_eq!(v(&s, "B4"), Value::Number(40.0));
    s.execute("control.setValue", json!({"id": c, "value": 41})).unwrap();
    assert_eq!(v(&s, "B4"), Value::Number(41.0));
}

#[test]
fn format_and_assign() {
    let mut s = s();
    let c = id(&s.execute("insert.slider", json!({"link": "B1", "max": 100, "value": 80})).unwrap());
    let r = s.execute("control.format", json!({"id": c, "max": 50, "name": "Rate", "horizontal": false})).unwrap();
    assert_eq!(
        (r["max"].clone(), r["value"].clone(), r["name"].clone(), r["horizontal"].clone()),
        (json!(50), json!(50), json!("Rate"), json!(false))
    );
    assert_eq!(v(&s, "B1"), Value::Number(50.0), "the new maximum pulls the linked value in");
    assert!(err(s.execute("control.format", json!({"id": c, "min": 50}))).contains("greater than Minimum"));
    assert_eq!(control(&s, c).min, 0, "a refused change leaves the control as it was");
    s.execute("control.format", json!({"id": c, "link": ""})).unwrap();
    assert_eq!(control(&s, c).link, None);
    s.execute("control.format", json!({"id": c, "value": 25})).unwrap();
    assert_eq!(v(&s, "B1"), Value::Number(50.0), "without a link nothing is written");
    // Buttons: text and action.
    // A fixed input needs a macro that takes one.
    s.execute("macro.save", json!({"name": "Fill", "steps": [{"command": "cell.set", "params": {"cell": "Input.cell(1,1)", "input": "x"}}], "inputs": [{"name": "Input", "kind": "range"}]})).unwrap();
    let b = id(&s.execute("insert.button", json!({"text": "Go", "macro": "Fill", "input": "C2:D4", "output": "F2"})).unwrap());
    let action = control(&s, b).action;
    assert_eq!(action, Some(ControlAction::Macro { name: "Fill".into(), input: MacroInput::Fixed("C2:D4".into()), output: Some("F2".into()) }));
    s.execute("control.assign", json!({"id": b, "command": "home.bold", "params": {"range": "A1"}})).unwrap();
    assert!(matches!(control(&s, b).action, Some(ControlAction::Command { ref id, .. }) if id == "home.bold"));
    assert!(err(s.execute("control.assign", json!({"id": b, "command": "file.saveAs"}))).contains("can't run file.saveAs"));
    assert!(err(s.execute("control.assign", json!({"id": b, "command": "control.press", "params": {"id": b}}))).contains("can't run control.press"));
    assert!(err(s.execute("control.assign", json!({"id": b, "command": "no.such"}))).contains("no command"));
    assert!(err(s.execute("control.assign", json!({"id": b, "macro": "Fill", "input": "nonsense!!"}))).contains("`input`"));
    assert!(err(s.execute("control.assign", json!({"id": b}))).contains("clear"));
    assert!(err(s.execute("control.assign", json!({"id": c, "macro": "Fill"}))).contains("only buttons"));
    s.execute("control.assign", json!({"id": b, "clear": true})).unwrap();
    assert_eq!(control(&s, b).action, None);
    // Renaming the macro keeps the button pointing at it.
    s.execute("control.assign", json!({"id": b, "macro": "fill"})).unwrap();
    s.execute("macro.rename", json!({"name": "Fill", "to": "Fill Block"})).unwrap();
    assert!(matches!(control(&s, b).action, Some(ControlAction::Macro { ref name, .. }) if name == "Fill Block"));
    let list = s.execute("control.list", json!({})).unwrap();
    assert_eq!(list.as_array().map(Vec::len), Some(2));
    assert_eq!(list[1]["text"], json!("Go"));
}

#[test]
fn pressing_a_button_runs_its_macro_as_one_undo_step() {
    let mut s = s();
    s.execute(
        "macro.save",
        json!({"name": "Mark", "inputs": [{"name": "Input", "kind": "range"}], "steps": [{"command": "cell.set", "params": {"cell": "Input.cell(1,1)", "input": "done"}}], "output": {"onConflict": "overwrite"}}),
    )
    .unwrap();
    let b = id(&s.execute("insert.button", json!({"macro": "Mark"})).unwrap());
    s.execute("selection.set", json!({"range": "C3:D4"})).unwrap();
    let before = undo_len(&s);
    let r = s.execute("control.press", json!({"id": b})).unwrap();
    assert_eq!(r["macro"], json!("Mark"));
    assert_eq!(v(&s, "C3"), Value::text("done"));
    assert_eq!(undo_len(&s), before + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "C3"), Value::Empty);
    // A fixed input ignores the selection.
    s.execute("control.assign", json!({"id": b, "macro": "Mark", "input": "F6:G7"})).unwrap();
    s.execute("control.press", json!({"id": b})).unwrap();
    assert_eq!((v(&s, "F6"), v(&s, "C3")), (Value::text("done"), Value::Empty));
    // No action, a missing macro, not a button.
    let empty = id(&s.execute("insert.button", json!({"text": "Nothing"})).unwrap());
    assert!(err(s.execute("control.press", json!({"id": empty}))).contains("has no action"));
    s.execute("control.assign", json!({"id": empty, "macro": "Gone"})).unwrap();
    assert!(err(s.execute("control.press", json!({"id": empty}))).contains("no macro called"));
    let spin = id(&s.execute("insert.spinButton", json!({})).unwrap());
    assert!(err(s.execute("control.press", json!({"id": spin}))).contains("only buttons"));
}

#[test]
fn a_command_button_goes_through_the_policy_with_the_documents_trust() {
    let mut s = s();
    let b = id(&s.execute("insert.button", json!({"command": "cell.set", "params": {"cell": "A1", "input": "7"}})).unwrap());
    // A workbook made here runs it straight away, as one undo step.
    let before = undo_len(&s);
    s.execute("control.press", json!({"id": b})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(7.0));
    assert_eq!(undo_len(&s), before + 1);
    s.execute("edit.undo", json!({})).unwrap();
    // From a file: it asks first, once.
    s.doc_mut().unwrap().trust = Trust::FromFile;
    match s.execute("control.press", json!({"id": b})) {
        Err(EngineError::Approve(q)) => assert!(q.contains("came with a workbook you opened"), "{q}"),
        other => panic!("expected a question, got {other:?}"),
    }
    assert_eq!(v(&s, "A1"), Value::Empty, "nothing ran");
    s.execute("control.press", json!({"id": b, "approve": true})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(7.0));
    s.execute("cell.set", json!({"cell": "A1", "input": ""})).unwrap();
    s.execute("control.press", json!({"id": b})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(7.0), "approved for the session");
    // A command that reads a file always asks.
    s.doc_mut().unwrap().trust = Trust::Local;
    let f = id(&s.execute("insert.button", json!({"command": "data.fromTextCsv", "params": {"path": "/nonexistent/x.csv"}})).unwrap());
    assert!(matches!(s.execute("control.press", json!({"id": f})), Err(EngineError::Approve(q)) if q.contains("reads a file")));
    // A denied command that a file brought in never runs.
    let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
    if let Some(c) = sh.controls.iter_mut().find(|c| c.id as u64 == f) {
        c.action = Some(ControlAction::Command { id: "file.saveAs".into(), params: json!({"path": "/tmp/x.xlsx"}) });
    }
    assert!(err(s.execute("control.press", json!({"id": f, "approve": true}))).contains("can't run file.saveAs"));
    // A macro button in a file's workbook asks too (the runner's gate).
    s.execute(
        "macro.save",
        json!({"name": "M", "steps": [{"command": "cell.set", "params": {"cell": "B1", "input": "1"}}], "output": {"onConflict": "overwrite"}}),
    )
    .unwrap();
    let m = id(&s.execute("insert.button", json!({"macro": "M"})).unwrap());
    s.doc_mut().unwrap().trust = Trust::FromFile;
    assert!(matches!(s.execute("control.press", json!({"id": m})), Err(EngineError::Approve(_))));
    s.execute("control.press", json!({"id": m, "approve": true})).unwrap();
    assert_eq!(v(&s, "B1"), Value::Number(1.0));
}

#[test]
fn a_file_commands_approval_question_names_the_file() {
    let mut s = s();
    let b = id(&s.execute("insert.button", json!({"command": "file.exportCsv", "params": {"path": "/tmp/reports/q3 totals.csv"}})).unwrap());
    match s.execute("control.press", json!({"id": b})) {
        Err(EngineError::Approve(q)) => assert!(q.contains("writes a file (/tmp/reports/q3 totals.csv)"), "{q}"),
        other => panic!("expected a question, got {other:?}"),
    }
    // A long path is shortened in the middle, and line breaks can't hide part of it.
    let long = format!("/tmp/{}\n{}.csv", "a".repeat(300), "b".repeat(300));
    let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
    if let Some(c) = sh.controls.iter_mut().find(|c| c.id as u64 == b) {
        c.action = Some(ControlAction::Command { id: "file.exportCsv".into(), params: json!({"path": long}) });
    }
    match s.execute("control.press", json!({"id": b})) {
        Err(EngineError::Approve(q)) => {
            assert!(q.contains("(/tmp/aaa") && q.contains("…") && q.contains("bbb.csv)") && !q.contains('\n') && q.len() < 600, "{q}")
        }
        other => panic!("expected a question, got {other:?}"),
    }
    // Without a path the question is unchanged.
    let plain = id(&s.execute("insert.button", json!({"command": "cell.set", "params": {"cell": "A1", "input": "1"}})).unwrap());
    s.doc_mut().unwrap().trust = Trust::FromFile;
    match s.execute("control.press", json!({"id": plain})) {
        Err(EngineError::Approve(q)) => assert!(q.ends_with("came with a workbook you opened."), "{q}"),
        other => panic!("expected a question, got {other:?}"),
    }
}

#[test]
fn hostile_params_are_refused() {
    let mut s = s();
    let c = id(&s.execute("insert.slider", json!({"width": 1e30, "height": -5, "at": "XFD1048576"})).unwrap());
    let a = control(&s, c).anchor;
    assert_eq!((a.width, a.height), (10_000.0, 4.0));
    let long = "x".repeat(2_000);
    assert!(s.execute("insert.button", json!({"text": long})).is_err());
    assert!(s.execute("insert.button", json!({"text": 5})).is_err());
    assert!(s.execute("insert.button", json!({"command": "cell.set", "params": [1]})).is_err());
    assert!(s.execute("control.format", json!({"id": c, "link": 5})).is_err());
    assert!(s.execute("control.format", json!({"id": c, "value": f64::MAX})).is_err());
    assert!(s.execute("control.setValue", json!({"id": c, "value": f64::MIN})).is_ok());
    assert_eq!(control(&s, c).value, 0);
    // Snapping keeps the maximum reachable when the step doesn't divide the range.
    let odd = FormControl { kind: ControlKind::Spin, min: 0, max: 10, step: 3, ..Default::default() };
    assert_eq!((odd.snap(9.4), odd.snap(9.6), odd.snap(f64::NAN)), (9, 10, 0));
}

fn link(s: &Session, sheet_i: usize, id: u64) -> String {
    let sh = s.doc().unwrap().wb.sheet(sheet_i).unwrap();
    sh.controls.iter().find(|c| c.id as u64 == id).and_then(|c| c.link.clone()).unwrap_or_default()
}

#[test]
fn links_follow_inserts_deletes_renames_and_cut_moves() {
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Data"})).unwrap();
    // A control on Data pointing back at Sheet1.
    let back = id(&s.execute("insert.spinButton", json!({"at": "C3", "link": "Sheet1!$B$2"})).unwrap());
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    let local = id(&s.execute("insert.slider", json!({"at": "E2", "link": "$B$2"})).unwrap());
    let remote = id(&s.execute("insert.spinButton", json!({"at": "E6", "link": "Data!$A$1"})).unwrap());

    s.execute("home.insertRows", json!({"rows": "1:1"})).unwrap();
    s.execute("home.insertColumns", json!({"cols": "A:A"})).unwrap();
    assert_eq!(link(&s, 0, local), "$C$3");
    assert_eq!(link(&s, 1, back), "Sheet1!$C$3");
    assert_eq!(link(&s, 0, remote), "Data!$A$1", "edits on Sheet1 leave links into Data alone");

    s.execute("cell.set", json!({"cell": "C3", "input": "7"})).unwrap();
    s.execute("edit.cut", json!({"range": "C3"})).unwrap();
    s.execute("edit.paste", json!({"at": "G9"})).unwrap();
    assert_eq!(link(&s, 0, local), "$G$9", "a cut-move takes the link with the cell");
    assert_eq!(link(&s, 1, back), "Sheet1!$G$9");

    s.execute("sheet.rename", json!({"sheet": 1, "name": "Inputs"})).unwrap();
    assert_eq!(link(&s, 0, remote), "Inputs!$A$1");
    s.execute("home.deleteRows", json!({"rows": "9:9"})).unwrap();
    assert!(link(&s, 0, local).contains("#REF!"), "{}", link(&s, 0, local));
    s.execute("home.deleteSheet", json!({"sheet": 1})).unwrap();
    assert!(link(&s, 0, remote).contains("#REF!"), "{}", link(&s, 0, remote));
    // A link to a deleted cell writes nothing.
    s.execute("control.setValue", json!({"id": local, "value": 5})).unwrap();
    for _ in 0..3 {
        s.execute("edit.undo", json!({})).unwrap();
    }
    assert_eq!((link(&s, 0, local), link(&s, 0, remote)), ("$G$9".to_string(), "Inputs!$A$1".to_string()));
}

#[test]
fn the_object_commands_leave_a_locked_control_on_a_protected_sheet_alone() {
    let mut s = s();
    let locked = id(&s.execute("insert.slider", json!({"at": "D2", "link": "B1"})).unwrap());
    let other = id(&s.execute("insert.spinButton", json!({"at": "D8"})).unwrap());
    let free = id(&s.execute("insert.button", json!({"at": "D12"})).unwrap());
    s.execute("control.format", json!({"id": free, "locked": false})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();
    let before = s.doc().unwrap().wb.clone();
    for (cmd, p) in [
        ("object.move", json!({"kind": "control", "id": locked, "at": "H9"})),
        ("object.delete", json!({"kind": "control", "id": locked})),
        ("object.setAnchorMode", json!({"kind": "control", "id": locked, "mode": "absolute"})),
        ("arrange.bringForward", json!({"kind": "control", "id": locked, "toFront": true})),
        ("arrange.sendBackward", json!({"kind": "control", "id": locked})),
        ("arrange.align", json!({"kind": "control", "ids": [locked, other], "align": "left"})),
    ] {
        assert!(err(s.execute(cmd, p)).contains("protected"), "{cmd}");
    }
    assert!(std::sync::Arc::ptr_eq(&s.doc().unwrap().wb, &before), "nothing changed");
    // An unlocked control still moves, as in Excel.
    s.execute("object.move", json!({"kind": "control", "id": free, "at": "H12"})).unwrap();
    assert_eq!(control(&s, free).anchor.cell.a1(), "H12");
}

#[test]
fn a_locked_control_on_a_protected_sheet_cannot_be_changed() {
    let mut s = s();
    let c = id(&s.execute("insert.slider", json!({"link": "B1"})).unwrap());
    let b = id(&s.execute("insert.button", json!({})).unwrap());
    s.execute("review.protectSheet", json!({})).unwrap();
    assert!(err(s.execute("control.format", json!({"id": c, "max": 50}))).contains("protected"));
    assert!(err(s.execute("control.assign", json!({"id": b, "command": "home.bold"}))).contains("protected"));
}

fn reopen(s: &mut Session) {
    let r = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let b64 = r["base64"].as_str().unwrap().to_string();
    s.execute("file.open", json!({"name": "x.xlsx", "base64": b64})).unwrap();
}

#[test]
fn a_kept_button_action_comes_back_only_on_the_same_button() {
    use crate::cmd::controls::{load_action_names, with_action_names};
    let mut s = s();
    s.execute("macro.save", json!({"name": "Mark", "steps": [{"command": "cell.set", "params": {"cell": "Input.cell(1,1)", "input": "m"}}], "inputs": [{"name": "Input", "kind": "range"}]})).unwrap();
    s.execute("insert.button", json!({"at": "D2", "text": "Run", "command": "cell.set", "params": {"cell": "A9", "input": "cmd"}})).unwrap();
    s.execute("insert.button", json!({"at": "D6", "text": "Other"})).unwrap();
    s.execute("insert.button", json!({"at": "D9", "text": "Fixed", "macro": "Mark", "input": "F1:G2"})).unwrap();
    let wb = (*s.doc().unwrap().wb).clone();
    let stored = with_action_names(&wb).unwrap().unwrap();
    let actions = |wb: &gridcraft_model::Workbook| wb.sheet(0).unwrap().controls.iter().map(|c| c.action.clone()).collect::<Vec<_>>();
    let kept = |wb: &gridcraft_model::Workbook| {
        let mut wb = wb.clone();
        for c in &mut wb.sheet_mut(0).unwrap().controls {
            // As xlsx carries them: a macro on the selection, or nothing for a command.
            c.action = match c.action.take() {
                Some(ControlAction::Macro { name, .. }) => Some(ControlAction::Macro { name, input: MacroInput::Selection, output: None }),
                _ => None,
            };
        }
        wb
    };
    // The same buttons in the same places get their actions back.
    let mut same = kept(&stored);
    load_action_names(&mut same);
    assert_eq!(actions(&same), actions(&wb));
    // Another application removed the first button: the command must not move to "Other".
    let mut shifted = kept(&stored);
    shifted.sheet_mut(0).unwrap().controls.remove(0);
    load_action_names(&mut shifted);
    assert_eq!(shifted.sheets[0].controls[0].action, None, "the command stays with the removed button");
    // A button given another macro there keeps it; a command doesn't replace a macro either.
    let mut reassigned = kept(&stored);
    for i in [0, 2] {
        reassigned.sheet_mut(0).unwrap().controls[i].action =
            Some(ControlAction::Macro { name: "Other".into(), input: MacroInput::Selection, output: None });
    }
    load_action_names(&mut reassigned);
    for i in [0, 2] {
        assert!(
            matches!(&reassigned.sheets[0].controls[i].action, Some(ControlAction::Macro { name, input: MacroInput::Selection, .. }) if name == "Other"),
            "{i}"
        );
    }
    // Another application cleared the macro of "Fixed": the stored action (with its fixed input)
    // does not bring the assignment back. The command on a button given no macro still does.
    let mut cleared = kept(&stored);
    cleared.sheet_mut(0).unwrap().controls[2].action = None;
    load_action_names(&mut cleared);
    assert_eq!(cleared.sheets[0].controls[2].action, None, "the removed assignment stays removed");
    assert_eq!(cleared.sheets[0].controls[0].action, actions(&wb)[0], "the command comes back");
}

/// An entry of the hidden name must carry the button's name and anchor cell (the writer always
/// stores both): a hand-edited entry without either, or with something else there, is dropped
/// like an unparsable one, so it cannot pin its action onto whatever button sits at that index.
#[test]
fn a_kept_action_without_the_buttons_name_or_anchor_is_dropped() {
    use crate::cmd::controls::{ACTIONS_KEY, load_action_names, with_action_names};
    use crate::cmd::stash;
    let mut s = s();
    s.execute("insert.button", json!({"at": "D2", "text": "Run", "command": "cell.set", "params": {"cell": "A9", "input": "cmd"}})).unwrap();
    let stored = with_action_names(&s.doc().unwrap().wb).unwrap().unwrap();
    let items = stash::get_json(&stored, ACTIONS_KEY).unwrap();
    assert!(items[0]["name"].is_string() && items[0]["at"].is_string(), "the writer stores both keys: {items}");
    // As the file brings a command button back: with no action.
    let kept = |items: &Json| {
        let mut wb = stored.clone();
        wb.sheet_mut(0).unwrap().controls[0].action = None;
        stash::remove(&mut wb, ACTIONS_KEY);
        stash::set_json(&mut wb, ACTIONS_KEY, items).unwrap();
        load_action_names(&mut wb);
        assert!(stash::get(&wb, ACTIONS_KEY).is_none(), "the hidden name is removed");
        wb.sheets[0].controls[0].action.clone()
    };
    let edited = |key: &str, value: Option<Json>| {
        let mut items = items.clone();
        let entry = items[0].as_object_mut().unwrap();
        match value {
            Some(v) => entry.insert(key.to_string(), v),
            None => entry.remove(key),
        };
        items
    };
    assert_eq!(kept(&edited("name", None)), None, "no `name`");
    assert_eq!(kept(&edited("at", None)), None, "no `at`");
    assert_eq!(kept(&edited("name", Some(json!(1)))), None, "`name` is not text");
    assert_eq!(kept(&edited("at", Some(Json::Null))), None, "`at` is null");
    assert!(matches!(kept(&items), Some(ControlAction::Command { .. })), "the writer's own entry still comes back");
}

/// A fixed input range for a macro that declares no inputs would fail every press (`macro.run`
/// refuses inputs the macro doesn't take), so the assignment is refused when it is made, naming
/// the macro; a macro the workbook doesn't have yet is not checked, and a macro with an input
/// takes the range. A file that brings such a stored input in gets the macro on the selection.
#[test]
fn a_fixed_input_for_a_macro_without_inputs_is_refused_when_assigned() {
    use crate::cmd::controls::{load_action_names, with_action_names};
    let mut s = s();
    s.execute("macro.save", json!({"name": "Fill", "steps": [{"command": "cell.set", "params": {"cell": "A1", "input": "x"}}]})).unwrap();
    s.execute("macro.save", json!({"name": "Mark", "steps": [{"command": "cell.set", "params": {"cell": "Input.cell(1,1)", "input": "m"}}], "inputs": [{"name": "Input", "kind": "range"}]})).unwrap();
    let want = "macro 'Fill' takes no inputs";
    assert!(err(s.execute("insert.button", json!({"at": "B2", "macro": "Fill", "input": "C2:D4"}))).contains(want));
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().controls.len(), 0, "no button was placed");
    let b = id(&s.execute("insert.button", json!({"at": "B2", "macro": "Fill", "output": "F2"})).unwrap());
    let e = err(s.execute("control.assign", json!({"id": b, "macro": "fill", "input": "C2:D4"})));
    assert!(e.contains(want), "any letter case: {e}");
    assert_eq!(control(&s, b).action, Some(ControlAction::Macro { name: "Fill".into(), input: MacroInput::Selection, output: Some("F2".into()) }));
    s.execute("control.assign", json!({"id": b, "macro": "Mark", "input": "C2:D4"})).unwrap();
    s.execute("control.assign", json!({"id": b, "macro": "Later", "input": "C2:D4"})).unwrap();
    assert_eq!(control(&s, b).action, Some(ControlAction::Macro { name: "Later".into(), input: MacroInput::Fixed("C2:D4".into()), output: None }));
    // A file's stored fixed input for Fill (another version assigned it) comes back as the selection.
    s.execute("control.assign", json!({"id": b, "macro": "Fill", "output": "F2"})).unwrap();
    let mut wb = (*s.doc().unwrap().wb).clone();
    wb.sheet_mut(0).unwrap().controls[0].action =
        Some(ControlAction::Macro { name: "Fill".into(), input: MacroInput::Fixed("C2:D4".into()), output: Some("F2".into()) });
    let mut stored = with_action_names(&wb).unwrap().unwrap();
    load_action_names(&mut stored);
    assert_eq!(
        stored.sheets[0].controls[0].action,
        Some(ControlAction::Macro { name: "Fill".into(), input: MacroInput::Selection, output: Some("F2".into()) })
    );
}

/// The saved xlsx bytes of `s`'s workbook.
fn saved_xlsx(s: &mut Session) -> Vec<u8> {
    let r = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    crate::io::base64_decode(r["base64"].as_str().unwrap()).unwrap()
}

/// `bytes` with `edit` applied to the text of every part, as another application that rewrote
/// those parts would leave the file.
fn edit_parts(bytes: &[u8], mut edit: impl FnMut(&str, String) -> String) -> Vec<u8> {
    use std::io::{Cursor, Read, Write};
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..zip.len() {
        let mut part = zip.by_index(i).unwrap();
        let name = part.name().to_string();
        let mut data = Vec::new();
        part.read_to_end(&mut data).unwrap();
        let data = match String::from_utf8(data) {
            Ok(text) => edit(&name, text).into_bytes(),
            Err(e) => e.into_bytes(),
        };
        out.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        out.write_all(&data).unwrap();
    }
    out.finish().unwrap().into_inner()
}

/// Opens `bytes` as `name`, checks the hidden name of the button actions is taken back out, and
/// lists the controls.
fn open_and_list(s: &mut Session, name: &str, bytes: &[u8]) -> Json {
    s.execute("file.open", json!({"name": name, "base64": crate::io::base64_encode(bytes)})).unwrap();
    assert!(s.doc().unwrap().wb.names.iter().all(|n| !n.name.contains("control_actions")), "{name}: the hidden name is taken back out");
    s.execute("control.list", json!({})).unwrap()
}

/// The macro reference xlsx writes for a button that runs `Mark`.
const MARK: &str = "[0]!Mark";

/// Another application removed a button's macro assignment and kept the button with its name and
/// place: the hidden name's copy of the action (with the fixed input and the output cell) does not
/// bring the assignment back. The same file, unedited, reopens with the whole action.
#[test]
fn a_macro_assignment_removed_in_another_application_stays_removed() {
    let mut s = s();
    s.execute("macro.save", json!({"name": "Mark", "steps": [{"command": "cell.set", "params": {"cell": "Input.cell(1,1)", "input": "m"}}], "inputs": [{"name": "Input", "kind": "range"}], "output": {"onConflict": "overwrite"}})).unwrap();
    s.execute("insert.button", json!({"at": "D2", "text": "Run", "macro": "Mark", "input": "F1:G2", "output": "H1"})).unwrap();
    let before = s.execute("control.list", json!({})).unwrap()[0].clone();
    let full = ControlAction::Macro { name: "Mark".into(), input: MacroInput::Fixed("F1:G2".into()), output: Some("H1".into()) };
    assert_eq!(before["action"], serde_json::to_value(&full).unwrap());
    let bytes = saved_xlsx(&mut s);
    // xlsx assigns the macro in the sheet's `<controlPr macro>` and in the VML shape's
    // `<x:FmlaMacro>` (the parts the reader takes it from); clearing the assignment drops both.
    let (attr, fmla) = (format!(" macro=\"{MARK}\""), format!("<x:FmlaMacro>{MARK}</x:FmlaMacro>"));
    let mut edited = Vec::new();
    let cleared = edit_parts(&bytes, |name, text| {
        if text.contains(&attr) || text.contains(&fmla) {
            edited.push(name.to_string());
        }
        text.replace(&attr, "").replace(&fmla, "")
    });
    edited.sort();
    assert_eq!(edited, ["xl/drawings/vmlDrawing1.vml", "xl/worksheets/sheet1.xml"]);
    let list = open_and_list(&mut s, "cleared.xlsx", &cleared);
    assert_eq!(list.as_array().unwrap().len(), 1);
    for key in ["kind", "name", "at", "text"] {
        assert_eq!(list[0][key], before[key], "{key}: the same button");
    }
    assert_eq!(list[0].get("action"), None, "the removed assignment stays removed");
    // Unedited, the file still assigns the macro, and the action comes back whole.
    let list = open_and_list(&mut s, "kept.xlsx", &bytes);
    assert_eq!(list[0]["action"], before["action"]);
}

/// Another application assigned a macro to a button that ran a command (kept in the hidden name):
/// the file's macro wins, as it does when a macro button is given another macro.
#[test]
fn a_macro_assigned_in_another_application_replaces_a_stored_command() {
    let mut s = s();
    s.execute("macro.save", json!({"name": "Mark", "steps": [{"command": "cell.set", "params": {"cell": "A1", "input": "m"}}]})).unwrap();
    s.execute("insert.button", json!({"at": "D2", "text": "Run", "command": "cell.set", "params": {"cell": "A9", "input": "cmd"}})).unwrap();
    let bytes = saved_xlsx(&mut s);
    let (pr, data) = ("<controlPr", "<x:ClientData ObjectType=\"Button\">");
    let mut edited = Vec::new();
    let assigned = edit_parts(&bytes, |name, text| {
        if text.contains(pr) || text.contains(data) {
            edited.push(name.to_string());
        }
        text.replace(pr, &format!("<controlPr macro=\"{MARK}\"")).replace(data, &format!("{data}<x:FmlaMacro>{MARK}</x:FmlaMacro>"))
    });
    edited.sort();
    assert_eq!(edited, ["xl/drawings/vmlDrawing1.vml", "xl/worksheets/sheet1.xml"]);
    let list = open_and_list(&mut s, "assigned.xlsx", &assigned);
    let plain = ControlAction::Macro { name: "Mark".into(), input: MacroInput::Selection, output: None };
    assert_eq!(list[0]["action"], serde_json::to_value(&plain).unwrap(), "the file's macro, not the stored command");
}

#[test]
fn every_kind_and_action_survives_xlsx() {
    let mut s = s();
    s.execute("macro.save", json!({"name": "Mark", "steps": [{"command": "cell.set", "params": {"cell": "Input.cell(1,1)", "input": "m"}}], "inputs": [{"name": "Input", "kind": "range"}], "output": {"onConflict": "overwrite"}})).unwrap();
    s.execute("insert.slider", json!({"at": "D2", "link": "B1", "min": 10, "max": 200, "step": 10, "page": 50, "value": 70})).unwrap();
    s.execute("insert.spinButton", json!({"at": "D6", "link": "B2", "max": 9, "value": 4})).unwrap();
    s.execute("insert.button", json!({"at": "D9", "text": "Plain", "macro": "Mark"})).unwrap();
    s.execute("insert.button", json!({"at": "D11", "text": "Fixed", "macro": "Mark", "input": "F1:G2", "output": "H1"})).unwrap();
    s.execute("insert.button", json!({"at": "D13", "text": "Command", "command": "cell.set", "params": {"cell": "A9", "input": "cmd"}})).unwrap();
    s.execute("review.newNote", json!({"cell": "A1", "text": "a note"})).unwrap();
    let before: Vec<Json> = s
        .execute("control.list", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let mut c = c.clone();
            c.as_object_mut().unwrap().remove("id");
            c
        })
        .collect();
    reopen(&mut s);
    let after: Vec<Json> = s
        .execute("control.list", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let mut c = c.clone();
            c.as_object_mut().unwrap().remove("id");
            c
        })
        .collect();
    assert_eq!(before.len(), after.len());
    for (b, a) in before.iter().zip(&after) {
        for key in ["kind", "name", "at", "text", "link", "value", "min", "max", "step", "page", "horizontal", "action", "print", "locked"] {
            assert_eq!(b.get(key), a.get(key), "{key}: {b} vs {a}");
        }
    }
    assert!(s.doc().unwrap().wb.names.iter().all(|n| !n.name.contains("control_actions")), "the hidden name is taken back out");
    assert_eq!(s.doc().unwrap().trust, Trust::FromFile);
    // A workbook from a file: its command button asks before it runs.
    let cmd = after.iter().position(|c| c["text"] == json!("Command")).unwrap();
    let id = s.execute("control.list", json!({})).unwrap()[cmd]["id"].as_u64().unwrap();
    assert!(matches!(s.execute("control.press", json!({"id": id})), Err(EngineError::Approve(_))));
    s.execute("control.press", json!({"id": id, "approve": true})).unwrap();
    assert_eq!(v(&s, "A9"), Value::text("cmd"));
    // The linked cells still move the controls.
    s.execute("cell.set", json!({"cell": "B1", "input": "120"})).unwrap();
    assert_eq!(s.execute("control.list", json!({})).unwrap()[0]["value"], json!(120));
}

/// A button's command goes through the macro policy when it is assigned and again when it is
/// pressed, so a file whose hidden name carries a denied command (written by another application,
/// or edited by hand) can't use a button as a way around the policy.
#[test]
fn a_denied_command_is_refused_when_assigned_and_when_a_file_brings_it_in() {
    let dir = std::env::temp_dir().join(format!("gridcraft-control-deny-{}", std::process::id()));
    let leak = dir.join("leak.xlsx").to_string_lossy().to_string();
    let denied = [
        ("link.open", json!({"url": "https://example.com/?d=secret"})),
        ("file.saveAs", json!({"path": leak})),
        ("file.save", json!({})),
        ("file.open", json!({"path": leak})),
        ("app.commands", json!({})),
        ("macro.run", json!({"name": "Mark"})),
        ("automate.runScript", json!({"name": "Leak", "trusted": true})),
        ("control.press", json!({"id": 1})),
    ];
    let mut s = s();
    s.execute("macro.save", json!({"name": "Mark", "steps": [{"command": "cell.set", "params": {"cell": "A1", "input": "m"}}]})).unwrap();
    // Neither Insert › Button nor Assign Action takes one.
    let b = id(&s.execute("insert.button", json!({"at": "B2", "text": "Run"})).unwrap());
    for (cmd, params) in &denied {
        let want = format!("can't run {cmd}");
        assert!(err(s.execute("insert.button", json!({"at": "B9", "command": cmd, "params": params}))).contains(&want), "{cmd}");
        assert!(err(s.execute("control.assign", json!({"id": b, "command": cmd, "params": params}))).contains(&want), "{cmd}");
        assert_eq!(control(&s, b).action, None, "{cmd}");
    }
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().controls.len(), 1, "no button was placed");
    // A file can still carry them: one button per command, given its action as a file would.
    for i in 1..denied.len() {
        s.execute("insert.button", json!({"at": format!("B{}", 2 + 3 * i), "text": format!("Run {i}")})).unwrap();
    }
    let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
    for (c, (cmd, params)) in sh.controls.iter_mut().zip(&denied) {
        c.action = Some(ControlAction::Command { id: (*cmd).to_string(), params: params.clone() });
    }
    reopen(&mut s);
    assert_eq!(s.doc().unwrap().trust, Trust::FromFile);
    let list = s.execute("control.list", json!({})).unwrap();
    for (i, (cmd, params)) in denied.iter().enumerate() {
        // The action came back through the hidden name, so the check at press time is what holds.
        assert_eq!(list[i]["action"], json!({"type": "command", "id": cmd, "params": params}), "{cmd}");
        let pressed = list[i]["id"].as_u64().unwrap();
        let before = undo_len(&s);
        assert!(err(s.execute("control.press", json!({"id": pressed, "approve": true}))).contains(&format!("can't run {cmd}")), "{cmd}");
        assert_eq!(undo_len(&s), before, "{cmd}: nothing ran");
    }
    assert!(!std::path::Path::new(&leak).exists());
    assert_eq!(v(&s, "A1"), Value::Empty, "the macro never ran");
    assert!(!s.ui_requests.iter().any(|r| matches!(r, crate::UiRequest::OpenUrl(_))), "no page was opened: {:?}", s.ui_requests);
}

/// A command the policy allows may still turn out to open a dialog or a web page when it runs
/// (a command given no parameters opens its dialog): the press is refused with the reason, as a
/// macro step is, and the request is dropped, so nothing opens. Messages for the person pass.
#[test]
fn a_button_command_that_would_open_a_dialog_or_a_page_is_refused_after_it_runs() {
    use crate::UiRequest;
    use crate::cmd::controls::take_requests;
    let mut s = s();
    let b = id(&s.execute("insert.button", json!({"at": "B2", "text": "Link", "command": "insert.link", "params": {}})).unwrap());
    let before = undo_len(&s);
    let name = control(&s, b).display_name();
    assert_eq!(
        err(s.execute("control.press", json!({"id": b}))),
        format!("'{name}' would open the insertLink dialog; give its command the parameters it needs.")
    );
    assert!(!s.ui_requests.iter().any(|r| matches!(r, UiRequest::Dialog(..))), "{:?}", s.ui_requests);
    assert_eq!(undo_len(&s), before, "nothing stays done");
    // The filter itself, with every kind of request a command can queue: the page is refused,
    // the cell edit is dropped, the message stays, and requests queued before are left alone.
    s.ui_requests.push(UiRequest::Message("earlier".into()));
    let queued = s.ui_requests.len();
    s.ui_requests.extend([UiRequest::EditCell(None), UiRequest::Message("saved".into()), UiRequest::OpenUrl("https://example.com/".into())]);
    assert_eq!(take_requests(&mut s, queued, "Go").as_deref(), Some("'Go' would open a web page, which buttons don't do."));
    assert_eq!(s.ui_requests, vec![UiRequest::Message("earlier".into()), UiRequest::Message("saved".into())]);
    assert_eq!(take_requests(&mut s, 5, "Go"), None, "a mark past the queue's end drops nothing");
    assert_eq!(s.ui_requests.len(), 2);
}

/// A linked cell that holds a formula: a write of the value the formula already shows is skipped,
/// so Format Control's OK (which sends the value back every time) keeps the formula, also on a
/// protected sheet with the cell locked; a changed value replaces the formula with the constant.
#[test]
fn a_formula_in_the_linked_cell_survives_a_write_of_the_value_it_shows() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "10"})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=A1*2"})).unwrap();
    let formula =
        |s: &Session| s.doc().unwrap().wb.sheet(0).unwrap().cell(gridcraft_core::CellRef::parse("B1").unwrap()).is_some_and(|c| c.formula.is_some());
    assert!(formula(&s));
    // Inserting with the value the formula shows (10 * 2 = 20) writes nothing.
    let c = id(&s.execute("insert.slider", json!({"at": "D2", "link": "B1", "value": 20})).unwrap());
    assert!(formula(&s), "the formula stays");
    assert_eq!(v(&s, "B1"), Value::Number(20.0));
    // The dialog's OK: every number sent back, the value unchanged, a new name.
    s.execute("control.format", json!({"id": c, "name": "Speed", "value": 20, "min": 0, "max": 100, "step": 1, "page": 10, "locked": false}))
        .unwrap();
    assert!(formula(&s), "a rename keeps the formula");
    // Editing A1 moves the control through the formula (15 * 2 = 30); OK with that value keeps it.
    s.execute("cell.set", json!({"cell": "A1", "input": "15"})).unwrap();
    assert_eq!(control(&s, c).value, 30);
    s.execute("control.format", json!({"id": c, "value": 30, "min": 0, "max": 100, "step": 1})).unwrap();
    assert!(formula(&s));
    // On a protected sheet with B1 locked, the same OK is not refused.
    s.execute("review.protectSheet", json!({})).unwrap();
    s.execute("control.format", json!({"id": c, "name": "Rate", "value": 30, "min": 0, "max": 100, "step": 1})).unwrap();
    assert_eq!(control(&s, c).name, "Rate");
    assert!(formula(&s));
    s.execute("review.unprotectSheet", json!({})).unwrap();
    // A changed value writes the constant over the formula.
    s.execute("control.format", json!({"id": c, "value": 40, "min": 0, "max": 100, "step": 1})).unwrap();
    assert!(!formula(&s), "the formula is replaced");
    assert_eq!(v(&s, "B1"), Value::Number(40.0));
}

/// Using a slider or spin button writes its linked cell through `cell.set`, so the cell's
/// protection governs it: a locked cell on a protected sheet refuses the change, and the control
/// keeps its value (the control's change and the cell write are one group, rolled back together).
/// A control's own Locked flag keeps it from being moved or edited, not from being used, as in
/// Excel, so an unlocked linked cell takes the value on a protected sheet.
#[test]
fn a_locked_linked_cell_on_a_protected_sheet_refuses_the_change_and_the_control_keeps_its_value() {
    let mut s = s();
    let c = id(&s.execute("insert.slider", json!({"at": "D2", "link": "B1", "value": 40})).unwrap());
    s.execute("control.format", json!({"id": c, "locked": false})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();
    let before = undo_len(&s);
    assert!(err(s.execute("control.setValue", json!({"id": c, "value": 60}))).contains("protected"));
    assert_eq!((control(&s, c).value, v(&s, "B1")), (40, Value::Number(40.0)), "nothing is half-applied");
    assert_eq!(undo_len(&s), before, "no undo step is left behind");
    // Format Control: a new maximum that would pull the value into the cell is refused whole.
    assert!(err(s.execute("control.format", json!({"id": c, "max": 30}))).contains("protected"));
    assert_eq!((control(&s, c).max, control(&s, c).value), (100, 40));
    // Inserting a control with a value for a locked cell is refused and leaves no control behind.
    assert!(err(s.execute("insert.spinButton", json!({"at": "D8", "link": "B2", "value": 5}))).contains("protected"));
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().controls.len(), 1);
    // The value the cell already holds writes nothing, so nothing is refused.
    s.execute("control.setValue", json!({"id": c, "value": 40})).unwrap();
    assert_eq!(undo_len(&s), before);
    // An unlocked linked cell takes the value on the protected sheet.
    s.execute("review.unprotectSheet", json!({})).unwrap();
    s.execute("home.lockCell", json!({"range": "B1", "on": false})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();
    s.execute("control.setValue", json!({"id": c, "value": 60})).unwrap();
    assert_eq!((control(&s, c).value, v(&s, "B1")), (60, Value::Number(60.0)));
}

/// A link to a cell on another sheet obeys that sheet's protection, not the control's sheet's.
#[test]
fn a_link_to_another_sheet_obeys_that_sheets_protection() {
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Data"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    let c = id(&s.execute("insert.slider", json!({"at": "D2", "link": "Data!B1", "value": 10})).unwrap());
    assert_eq!(control(&s, c).link.as_deref(), Some("Data!$B$1"));
    let data = |s: &Session| {
        let wb = &s.doc().unwrap().wb;
        wb.sheet(wb.sheet_index("Data").unwrap()).unwrap().value(gridcraft_core::CellRef::new(0, 1))
    };
    assert_eq!(data(&s), Value::Number(10.0));
    s.execute("review.protectSheet", json!({"sheet": "Data"})).unwrap();
    let before = undo_len(&s);
    assert!(err(s.execute("control.setValue", json!({"id": c, "value": 20}))).contains("protected"));
    assert_eq!((control(&s, c).value, data(&s)), (10, Value::Number(10.0)));
    assert_eq!(undo_len(&s), before);
    assert!(err(s.execute("insert.spinButton", json!({"at": "D8", "link": "Data!B2", "value": 5}))).contains("protected"));
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().controls.len(), 1);
    // Protecting the control's own sheet instead leaves the write to the other sheet alone.
    s.execute("review.unprotectSheet", json!({"sheet": "Data"})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();
    s.execute("control.setValue", json!({"id": c, "value": 20})).unwrap();
    assert_eq!((control(&s, c).value, data(&s)), (20, Value::Number(20.0)));
    // A link to a sheet the workbook doesn't have is refused with a clear message.
    s.execute("review.unprotectSheet", json!({})).unwrap();
    assert!(err(s.execute("control.format", json!({"id": c, "link": "Nowhere!B1"}))).contains("isn't a cell of this workbook"));
    assert_eq!(control(&s, c).link.as_deref(), Some("Data!$B$1"));
}
