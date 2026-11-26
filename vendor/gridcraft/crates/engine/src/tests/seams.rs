//! Engine building blocks for multi-command actions such as macros: undo groups, the recording
//! buffer, document trust and chunked storage of workbook data in hidden names.

use serde_json::json;

use super::{s, v};
use crate::{EngineError, Session, Trust};

fn undo_labels(s: &Session) -> Vec<String> {
    s.doc().unwrap().undo.iter().map(|e| e.label.clone()).collect()
}

fn set(s: &mut Session, cell: &str, input: &str) {
    s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
}

// ---------------------------------------------------------------- undo groups

#[test]
fn grouped_commands_are_one_undo_step() {
    let mut s = s();
    set(&mut s, "A1", "before");
    let r = s.grouped("Fill Three", |s| {
        set(s, "A1", "1");
        set(s, "A2", "2");
        s.execute("home.bold", json!({"range": "A1:A2"}))?;
        Ok(7)
    });
    assert_eq!(r.unwrap(), 7);
    assert_eq!(undo_labels(&s), ["Enter Cell", "Fill Three"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "A1").display(), "before");
    assert!(v(&s, "A2").is_empty());
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(v(&s, "A2").display(), "2");
    assert!(!s.in_group(s.doc().unwrap().uid));
}

#[test]
fn a_failing_group_rolls_everything_back() {
    let mut s = s();
    set(&mut s, "B1", "base");
    set(&mut s, "A1", "undone");
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("selection.set", json!({"range": "C3"})).unwrap();
    let redo_before = s.doc().unwrap().redo.len();
    assert_eq!(redo_before, 1);
    let r: crate::Result<()> = s.grouped("Doomed", |s| {
        set(s, "B1", "changed");
        s.execute("selection.set", json!({"range": "D4"}))?;
        set(s, "B2", "new");
        Err(EngineError::Other("step 3 failed".into()))
    });
    assert!(r.unwrap_err().to_string().contains("step 3 failed"));
    assert_eq!(v(&s, "B1").display(), "base");
    assert!(v(&s, "B2").is_empty());
    assert_eq!(s.doc().unwrap().selection.a1(), "C3");
    assert_eq!(undo_labels(&s), ["Enter Cell"]);
    assert_eq!(s.doc().unwrap().redo.len(), redo_before, "the redo stack comes back too");
    // The recalculation follows the restored workbook.
    set(&mut s, "C1", "=B1&\"!\"");
    assert_eq!(v(&s, "C1").display(), "base!");
}

#[test]
fn groups_nest_and_the_outermost_makes_the_step() {
    let mut s = s();
    let r = s.grouped("Outer", |s| {
        set(s, "A1", "1");
        s.grouped("Inner", |s| {
            set(s, "A2", "2");
            Ok(())
        })?;
        // A failing inner group rolls back only its own changes; the outer one carries on.
        let inner: crate::Result<()> = s.grouped("Inner fails", |s| {
            set(s, "A3", "3");
            Err(EngineError::Other("no".into()))
        });
        assert!(inner.is_err());
        assert!(v(s, "A3").is_empty());
        set(s, "A4", "4");
        Ok(())
    });
    r.unwrap();
    assert_eq!(undo_labels(&s), ["Outer"]);
    assert_eq!(v(&s, "A2").display(), "2");
    s.execute("edit.undo", json!({})).unwrap();
    for c in ["A1", "A2", "A3", "A4"] {
        assert!(v(&s, c).is_empty(), "{c}");
    }
}

#[test]
fn an_unchanged_group_adds_no_step() {
    let mut s = s();
    set(&mut s, "A1", "x");
    s.grouped("Look Around", |s| {
        s.execute("selection.set", json!({"range": "B2"}))?;
        s.execute("cell.get", json!({"cell": "A1"}))?;
        Ok(())
    })
    .unwrap();
    assert_eq!(undo_labels(&s), ["Enter Cell"]);
}

#[test]
fn a_group_spans_calls_and_blocks_undo_until_it_ends() {
    let mut s = s();
    set(&mut s, "A1", "start");
    let token = s.group_begin("Debug Macro").unwrap();
    // Each step is its own call, as a debugger makes them.
    set(&mut s, "A1", "step 1");
    set(&mut s, "A2", "step 2");
    let undo = s.execute("edit.undo", json!({}));
    assert!(matches!(undo, Err(EngineError::Disabled(..))), "{undo:?}");
    assert!(matches!(s.execute("edit.redo", json!({})), Err(EngineError::Disabled(..))));
    assert_eq!(s.execute("history.list", json!({})).unwrap()["grouping"], json!(true));
    assert!(s.group_end(token).unwrap());
    assert_eq!(s.execute("history.list", json!({})).unwrap()["grouping"], json!(false));
    assert_eq!(undo_labels(&s), ["Enter Cell", "Debug Macro"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "A1").display(), "start");
    assert!(v(&s, "A2").is_empty());
}

#[test]
fn rolling_back_a_spanning_group_restores_the_start() {
    let mut s = s();
    set(&mut s, "A1", "start");
    let token = s.group_begin("Debug Macro").unwrap();
    set(&mut s, "A1", "step 1");
    s.execute("home.insertSheet", json!({})).unwrap();
    s.group_rollback(token).unwrap();
    assert_eq!(v(&s, "A1").display(), "start");
    assert_eq!(s.doc().unwrap().wb.sheets.len(), 1);
    assert_eq!(undo_labels(&s), ["Enter Cell"]);
}

#[test]
fn ending_an_outer_group_ends_the_inner_ones() {
    let mut s = s();
    let outer = s.group_begin("Outer").unwrap();
    let inner = s.group_begin("Inner").unwrap();
    set(&mut s, "A1", "1");
    s.group_end(outer).unwrap();
    assert_eq!(undo_labels(&s), ["Outer"]);
    assert!(s.group_end(inner).is_err(), "the inner group ended with its outer group");
    assert!(!s.in_group(s.doc().unwrap().uid));
}

#[test]
fn a_long_group_keeps_the_history_before_it() {
    let mut s = s();
    for i in 0..100 {
        set(&mut s, "A1", &i.to_string());
    }
    let first = s.doc().unwrap().undo.first().map(|e| e.wb.clone()).unwrap();
    s.grouped("Many Steps", |s| {
        for i in 0..250 {
            set(s, "B1", &i.to_string());
        }
        Ok(())
    })
    .unwrap();
    let d = s.doc().unwrap();
    assert_eq!(d.undo.len(), 100, "the undo limit still holds");
    assert_eq!(d.undo.last().map(|e| e.label.as_str()), Some("Many Steps"));
    // Only the oldest entry made room for the group's one step.
    assert!(!d.undo.iter().any(|e| std::sync::Arc::ptr_eq(&e.wb, &first)));
    assert_eq!(d.undo.iter().filter(|e| e.label == "Enter Cell").count(), 99);
}

#[test]
fn a_grouped_action_that_closes_its_document_succeeds() {
    // The group goes with the document; the action did what it was asked.
    let mut s = s();
    let uid = s.doc().unwrap().uid;
    s.grouped("Close", |s| s.execute("file.close", json!({}))).unwrap();
    assert!(s.docs.iter().all(|d| d.uid != uid) && !s.in_group(uid));
    // Opening a file into an untouched new workbook takes its place the same way.
    s.new_workbook();
    let uid = s.doc().unwrap().uid;
    let bytes = serde_json::to_vec(&gridcraft_model::Workbook::new()).unwrap();
    s.grouped("Open", |s| s.execute("file.open", json!({"name": "theirs.json", "base64": crate::io::base64_encode(&bytes)}))).unwrap();
    assert!(s.docs.iter().all(|d| d.uid != uid) && !s.in_group(uid) && s.groups.is_empty());
    // A group ended by someone else while its document is still open is still an error.
    let err = s.grouped("Ended", |s| {
        let token = s.groups.last().map(|g| crate::GroupToken { id: g.id, doc: g.doc });
        s.group_end(token.ok_or_else(|| EngineError::Other("no group".into()))?).map(|_| ())
    });
    assert!(err.unwrap_err().to_string().contains("already ended"));
}

#[test]
fn closing_a_document_drops_its_groups() {
    let mut s = s();
    let token = s.group_begin("Debug Macro").unwrap();
    let uid = token.doc();
    s.close_document(s.active_index());
    assert!(!s.in_group(uid));
    assert!(s.group_end(token).is_err());
    assert!(matches!(s.group_begin("x"), Err(EngineError::NoDocument)));
}

#[test]
fn a_group_owns_the_steps_of_commands_that_run_commands() {
    // Insert Copied Cells runs nested steps and folds them into its own undo step; inside a group
    // it must leave the history from before the group alone.
    let mut s = s();
    set(&mut s, "A1", "kept");
    set(&mut s, "A5", "copied");
    s.execute("edit.copy", json!({"range": "5:5"})).unwrap();
    s.grouped("Insert Inside", |s| {
        s.execute("selection.set", json!({"range": "A2"}))?;
        s.execute("home.insertCopiedCells", json!({}))?;
        Ok(())
    })
    .unwrap();
    assert_eq!(v(&s, "A2").display(), "copied");
    assert_eq!(undo_labels(&s), ["Enter Cell", "Enter Cell", "Insert Inside"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "A2").is_empty());
    assert_eq!(v(&s, "A1").display(), "kept");
}

fn put(s: &mut Session, cell: &str, text: &str) -> crate::Result<()> {
    s.execute("cell.set", json!({"cell": cell, "input": text})).map(|_| ())
}

fn register_group_commands() {
    use crate::cmd::register_undoable_test_command as register;
    // Edits before its group are part of the step.
    register("test.editThenGroup", |s, _| {
        put(s, "A1", "outside")?;
        s.grouped("Inner", |s| put(s, "A2", "inside"))?;
        Ok(json!({}))
    });
    // A group that fails and rolls back, then an edit outside it.
    register("test.failThenEdit", |s, _| {
        let failed: crate::Result<()> = s.grouped("Doomed", |s| {
            put(s, "B1", "gone")?;
            Err(EngineError::Other("no".into()))
        });
        assert!(failed.is_err());
        put(s, "B2", "kept")?;
        Ok(json!({}))
    });
    // A command that is one group keeps the group's name.
    register("test.onlyGroup", |s, _| s.grouped("Named Step", |s| put(s, "D1", "x")).map(|_| json!({})));
    // A group, then an edit the command makes itself (no nested command, so no undo entry).
    register("test.groupThenEdit", |s, _| {
        s.grouped("Named Step", |s| put(s, "D1", "x"))?;
        crate::cmd::edit(s, |cx| {
            let at = gridcraft_core::CellRef::parse("D2").ok_or_else(|| EngineError::Other("D2".into()))?;
            cx.sheet_mut(0)?.set_value(at, gridcraft_core::Value::Text("y".into()));
            cx.touch(0, at);
            Ok(())
        })?;
        Ok(json!({}))
    });
    // A group with a change, then one that is rolled back: the step is still the first group's.
    register("test.groupThenRolledBack", |s, _| {
        s.grouped("Named Step", |s| put(s, "D1", "x"))?;
        let failed: crate::Result<()> = s.grouped("Doomed", |s| {
            put(s, "D3", "gone")?;
            Err(EngineError::Other("no".into()))
        });
        assert!(failed.is_err());
        Ok(json!({}))
    });
    // An edit, then the rollback of the innermost group that was open before the command.
    register("test.rollbackOpenGroup", |s, _| {
        put(s, "E1", "gone")?;
        let token = s.groups.last().map(|g| crate::GroupToken { id: g.id, doc: g.doc });
        s.group_rollback(token.ok_or_else(|| EngineError::Other("no group".into()))?)?;
        Ok(json!({}))
    });
    // An edit, then the end of the innermost group that was open before the command.
    register("test.endOpenGroup", |s, _| {
        put(s, "E1", "end")?;
        let token = s.groups.last().map(|g| crate::GroupToken { id: g.id, doc: g.doc });
        s.group_end(token.ok_or_else(|| EngineError::Other("no group".into()))?)?;
        Ok(json!({}))
    });
    // The end of the innermost group that was open before the command, then an edit.
    register("test.endOpenGroupThenEdit", |s, _| {
        let token = s.groups.last().map(|g| crate::GroupToken { id: g.id, doc: g.doc });
        s.group_end(token.ok_or_else(|| EngineError::Other("no group".into()))?)?;
        put(s, "E1", "after")?;
        Ok(json!({}))
    });
    // Many nested edits: the steps they make are bounded while the command runs. (Not undoable
    // itself: like a script, its nested steps are the undo steps.)
    crate::cmd::register_test_command("test.manyNestedEdits", |s, _| {
        let start = s.doc()?.undo.len();
        for i in 0..crate::UNDO_LIMIT + 50 {
            put(s, "A1", &i.to_string())?;
        }
        let len = s.doc()?.undo.len();
        if len > start + crate::UNDO_LIMIT {
            return Err(EngineError::Other(format!("{} steps kept inside the command", len - start)));
        }
        Ok(json!({}))
    });
    // An edit, then a group the command leaves open (a debugger's start).
    register("test.editThenOpenGroup", |s, _| {
        put(s, "A1", "first")?;
        let _open = s.group_begin("Debug")?;
        put(s, "A2", "second")?;
        Ok(json!({}))
    });
}

#[test]
fn a_command_that_runs_its_own_group_makes_one_step() {
    register_group_commands();
    let mut s = s();
    s.execute("test.onlyGroup", json!({})).unwrap();
    assert_eq!(undo_labels(&s), ["Named Step"]);
    s.execute("edit.undo", json!({})).unwrap();
    set(&mut s, "C1", "base");
    s.execute("test.editThenGroup", json!({})).unwrap();
    assert_eq!(undo_labels(&s), ["Enter Cell", "test.editThenGroup"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "A1").is_empty() && v(&s, "A2").is_empty());
    assert_eq!(v(&s, "C1").display(), "base");

    s.execute("test.failThenEdit", json!({})).unwrap();
    assert!(v(&s, "B1").is_empty());
    assert_eq!(v(&s, "B2").display(), "kept");
    assert_eq!(undo_labels(&s), ["Enter Cell", "test.failThenEdit"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "B2").is_empty());
}

#[test]
fn a_stale_group_step_does_not_name_a_later_command() {
    // The record of the last group step survives the Undo of that step. A later command that
    // ends a group of its own at the same history length, after rolling another back, must not
    // take the name of the nested edit it made.
    register_group_commands();
    let mut s = s();
    s.execute("test.onlyGroup", json!({})).unwrap();
    assert_eq!(undo_labels(&s), ["Named Step"]);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("test.failThenEdit", json!({})).unwrap();
    assert_eq!(undo_labels(&s), ["test.failThenEdit"]);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("test.failThenEdit", json!({})).unwrap();
    assert_eq!(undo_labels(&s), ["test.failThenEdit"]);
    assert!(v(&s, "B1").is_empty());
    assert_eq!(v(&s, "B2").display(), "kept");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "B2").is_empty() && v(&s, "D1").is_empty());
}

#[test]
fn a_command_keeps_its_groups_name_only_for_exactly_that_step() {
    // The name comes from the group only when the group's step is the command's whole step: an
    // edit after the group (no nested command, so the history length matches) makes it the
    // command's own, while a later group that rolled back changes nothing.
    register_group_commands();
    let mut s = s();
    s.execute("test.groupThenEdit", json!({})).unwrap();
    assert_eq!(undo_labels(&s), ["test.groupThenEdit"]);
    assert_eq!((v(&s, "D1").display(), v(&s, "D2").display()), ("x".into(), "y".into()));
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "D1").is_empty() && v(&s, "D2").is_empty());
    assert_eq!(undo_labels(&s), Vec::<String>::new());
    s.execute("test.groupThenRolledBack", json!({})).unwrap();
    assert_eq!(undo_labels(&s), ["Named Step"]);
    assert_eq!(v(&s, "D1").display(), "x");
    assert!(v(&s, "D3").is_empty());
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "D1").is_empty());
}

#[test]
fn what_a_command_changes_after_ending_a_group_is_its_own_step() {
    // Ending a group that was open before the command leaves the group its step (if it made one)
    // and makes a step of the command's own for what it changed after the end.
    register_group_commands();
    let mut s = s();
    // The group made no step: the command's edit must still be undoable, under its own name.
    let _unchanged = s.group_begin("Debug").unwrap();
    s.execute("test.endOpenGroupThenEdit", json!({})).unwrap();
    assert_eq!(undo_labels(&s), ["test.endOpenGroupThenEdit"]);
    assert_eq!(v(&s, "E1").display(), "after");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "E1").is_empty());
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(v(&s, "E1").display(), "after");
    // The group made a step: two steps, the group's and the command's.
    s.execute("edit.undo", json!({})).unwrap();
    let _changed = s.group_begin("Debug").unwrap();
    set(&mut s, "B1", "inside");
    s.execute("test.endOpenGroupThenEdit", json!({})).unwrap();
    assert_eq!(undo_labels(&s), ["Debug", "test.endOpenGroupThenEdit"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "E1").is_empty());
    assert_eq!(v(&s, "B1").display(), "inside");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "B1").is_empty());
}

#[test]
fn a_group_a_command_leaves_open_starts_where_the_command_did() {
    // A command that edits and then opens a group it leaves open has no undo step of its own:
    // the group's step, made when it ends, covers the command's edit too, so no state from
    // inside the command can come back.
    register_group_commands();
    let mut s = s();
    set(&mut s, "C1", "base");
    s.execute("test.editThenOpenGroup", json!({})).unwrap();
    let d = s.doc().unwrap();
    assert!(s.in_group(d.uid));
    let token = s.groups.last().map(|g| crate::GroupToken { id: g.id, doc: g.doc }).unwrap();
    assert!(s.group_end(token).unwrap());
    assert_eq!(undo_labels(&s), ["Enter Cell", "Debug"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "A1").is_empty() && v(&s, "A2").is_empty());
    assert_eq!(v(&s, "C1").display(), "base");
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!((v(&s, "A1").display(), v(&s, "A2").display()), ("first".into(), "second".into()));
}

#[test]
fn steps_made_inside_a_command_are_bounded_while_it_runs() {
    // The history limit applies when the outermost command finishes, so a command's bookkeeping
    // holds; but a command that runs thousands of nested commands (a script, a subtotal) must not
    // keep thousands of steps alive until then. Each nested step bounds the steps made since the
    // command began, and leaves the history from before it alone.
    register_group_commands();
    let mut s = s();
    let before = fill_history(&mut s);
    s.execute("test.manyNestedEdits", json!({})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!(d.undo.len(), crate::UNDO_LIMIT);
    assert!(d.undo.iter().all(|e| e.label == "Enter Cell"));
    assert!(!d.undo.iter().any(|e| before.iter().any(|w| std::sync::Arc::ptr_eq(w, &e.wb))), "the older history gave way to the newest steps");
    assert_eq!(v(&s, "A1").display(), "149");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "A1").display(), "148");
}

#[test]
fn a_group_rolled_back_inside_a_command_keeps_the_redo_history() {
    // The debugger's Stop with Undo Changes rolls its group back inside a command. The rollback
    // put the redo history back, and the command's own bookkeeping then cleared it again.
    register_group_commands();
    let mut s = s();
    set(&mut s, "A1", "first");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().redo.len(), 1);
    let token = s.group_begin("Debug").unwrap();
    set(&mut s, "B1", "inside");
    s.execute("test.rollbackOpenGroup", json!({})).unwrap();
    assert!(!s.in_group(token.doc));
    assert!(v(&s, "B1").is_empty() && v(&s, "E1").is_empty());
    assert_eq!(s.doc().unwrap().redo.len(), 1, "the redo history survives the rolled-back group");
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(v(&s, "A1").display(), "first");
}

/// Fills the undo history to the limit with edits of Z1 ("v0" to "v99"), and returns its snapshots.
fn fill_history(s: &mut Session) -> Vec<std::sync::Arc<gridcraft_model::Workbook>> {
    for i in 0..crate::UNDO_LIMIT {
        set(s, "Z1", &format!("v{i}"));
    }
    let d = s.doc().unwrap();
    assert_eq!(d.undo.len(), crate::UNDO_LIMIT);
    d.undo.iter().map(|e| e.wb.clone()).collect()
}

/// The history is still full, ends with one step labelled `label`, and kept every older step but
/// the oldest.
fn assert_one_step_replaced_the_oldest(s: &Session, before: &[std::sync::Arc<gridcraft_model::Workbook>], label: &str) {
    let d = s.doc().unwrap();
    assert_eq!(d.undo.len(), crate::UNDO_LIMIT);
    assert_eq!(d.undo.last().map(|e| e.label.as_str()), Some(label));
    assert_eq!(d.undo.iter().filter(|e| e.label == label).count(), 1);
    let kept = d.undo.iter().take(crate::UNDO_LIMIT - 1);
    assert!(kept.zip(before.iter().skip(1)).all(|(e, wb)| std::sync::Arc::ptr_eq(&e.wb, wb)));
}

/// Undo goes back to the state before the step, then to the edit before it; Redo comes back.
fn assert_undo_redo(s: &mut Session, cells: &[(&str, &str)]) {
    let after = |s: &Session| cells.iter().all(|(c, t)| v(s, c).display() == *t);
    let gone = |s: &Session| cells.iter().all(|(c, _)| v(s, c).is_empty());
    assert!(after(s));
    s.execute("edit.undo", json!({})).unwrap();
    assert!(gone(s) && v(s, "Z1").display() == "v99", "the first Undo restores the state before the command");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(gone(s) && v(s, "Z1").display() == "v98", "the second Undo restores the edit before it");
    s.execute("edit.redo", json!({})).unwrap();
    assert!(gone(s) && v(s, "Z1").display() == "v99");
    s.execute("edit.redo", json!({})).unwrap();
    assert!(after(s) && v(s, "Z1").display() == "v99");
}

#[test]
fn a_group_only_command_makes_one_step_when_the_history_is_full() {
    register_group_commands();
    let mut s = s();
    let before = fill_history(&mut s);
    s.execute("test.onlyGroup", json!({})).unwrap();
    assert_one_step_replaced_the_oldest(&s, &before, "Named Step");
    assert_undo_redo(&mut s, &[("D1", "x")]);
}

#[test]
fn an_edit_then_a_group_makes_one_step_when_the_history_is_full() {
    register_group_commands();
    let mut s = s();
    let before = fill_history(&mut s);
    s.execute("test.editThenGroup", json!({})).unwrap();
    assert_one_step_replaced_the_oldest(&s, &before, "test.editThenGroup");
    assert_undo_redo(&mut s, &[("A1", "outside"), ("A2", "inside")]);
}

#[test]
fn a_group_ended_at_the_history_limit_replaces_only_the_oldest_step() {
    register_group_commands();
    // A command ends a group that was open before it.
    let mut s = s();
    let before = fill_history(&mut s);
    let _open = s.group_begin("Debug Macro").unwrap();
    set(&mut s, "E2", "step");
    assert_eq!(s.doc().unwrap().undo.len(), crate::UNDO_LIMIT);
    s.execute("test.endOpenGroup", json!({})).unwrap();
    assert!(!s.in_group(s.doc().unwrap().uid));
    assert_one_step_replaced_the_oldest(&s, &before, "Debug Macro");
    assert_undo_redo(&mut s, &[("E1", "end"), ("E2", "step")]);

    // A group ended directly, outside any command, across calls.
    let mut s = super::s();
    let before = fill_history(&mut s);
    let token = s.group_begin("Spanning").unwrap();
    set(&mut s, "F1", "one");
    set(&mut s, "F2", "two");
    assert!(s.group_end(token).unwrap());
    assert_one_step_replaced_the_oldest(&s, &before, "Spanning");
    assert_undo_redo(&mut s, &[("F1", "one"), ("F2", "two")]);
}

#[test]
fn a_command_that_panics_leaves_the_history_as_it_found_it() {
    // A command that edits through a nested command and then panics: the nested step is gone
    // with the workbook it changed, and the redo history the nested step cleared comes back.
    crate::cmd::register_undoable_test_command("test.editThenPanic", |s, _| {
        put(s, "A1", "half done")?;
        panic!("after an edit");
    });
    let mut s = s();
    set(&mut s, "A1", "1");
    set(&mut s, "A1", "2");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!((undo_labels(&s).len(), s.doc().unwrap().redo.len()), (1, 1));
    let err = s.execute("test.editThenPanic", json!({})).unwrap_err().to_string();
    assert!(err.contains("after an edit"), "{err}");
    assert_eq!(v(&s, "A1").display(), "1", "the workbook is restored");
    assert_eq!(undo_labels(&s), ["Enter Cell"], "no step for the nested edit");
    assert_eq!(s.doc().unwrap().redo.len(), 1, "the redo history is back");
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(v(&s, "A1").display(), "2");

    // A command that undoes through a nested command and then panics: the entry Undo removed
    // comes back, and the redo entry it made goes.
    crate::cmd::register_undoable_test_command("test.undoThenPanic", |s, _| {
        s.execute("edit.undo", json!({}))?;
        panic!("after an undo");
    });
    assert_eq!((undo_labels(&s).len(), s.doc().unwrap().redo.len()), (2, 0));
    assert!(s.execute("test.undoThenPanic", json!({})).is_err());
    assert_eq!(v(&s, "A1").display(), "2", "the workbook is restored");
    assert_eq!(undo_labels(&s), ["Enter Cell", "Enter Cell"], "the undone step is back");
    assert_eq!(s.doc().unwrap().redo.len(), 0);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "A1").display(), "1");
}

#[test]
fn a_grouped_action_that_panics_is_rolled_back_before_the_panic_goes_on() {
    // Outside a command, a caller that catches the panic finds the group rolled back and no
    // group left open, so Undo is enabled and the next grouped action does not nest.
    let mut s = s();
    set(&mut s, "A1", "before");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        s.grouped::<()>("Doomed", |s| {
            set(s, "A1", "half done");
            panic!("inside the group");
        })
    }));
    assert!(caught.is_err(), "the panic goes on to the caller");
    let uid = s.doc().unwrap().uid;
    assert!(!s.in_group(uid));
    assert_eq!(v(&s, "A1").display(), "before");
    assert_eq!(undo_labels(&s), ["Enter Cell"]);
    s.grouped("Next", |s| {
        set(s, "A1", "after");
        Ok(())
    })
    .unwrap();
    assert_eq!(undo_labels(&s), ["Enter Cell", "Next"]);
}

#[test]
fn a_group_ended_by_a_command_that_then_panics_keeps_its_step() {
    // The command takes a token the test left for it, ends or rolls back that group, and panics.
    // The group's token went with the command, so the group stays ended; its step runs from the
    // group's start to the restored workbook, so Undo still returns to the state before the group.
    static TOKEN: std::sync::Mutex<Option<crate::GroupToken>> = std::sync::Mutex::new(None);
    crate::cmd::register_undoable_test_command("test.endGroupThenPanic", |s, _| {
        let token = TOKEN.lock().map_err(|_| EngineError::Other("lock".into()))?.take();
        if let Some(token) = token {
            s.group_end(token)?;
        }
        panic!("after ending a group");
    });
    crate::cmd::register_undoable_test_command("test.rollBackGroupThenPanic", |s, _| {
        let token = TOKEN.lock().map_err(|_| EngineError::Other("lock".into()))?.take();
        if let Some(token) = token {
            s.group_rollback(token)?;
        }
        panic!("after rolling back a group");
    });
    for cmd in ["test.endGroupThenPanic", "test.rollBackGroupThenPanic"] {
        let mut s = s();
        set(&mut s, "A1", "before");
        let token = s.group_begin("Outer").unwrap();
        set(&mut s, "A1", "inside");
        set(&mut s, "A2", "inside too");
        *TOKEN.lock().unwrap() = Some(token);
        assert!(s.execute(cmd, json!({})).is_err());
        let uid = s.doc().unwrap().uid;
        assert!(!s.in_group(uid), "{cmd}: the group stays ended");
        assert_eq!(undo_labels(&s), ["Enter Cell", "Outer"], "{cmd}");
        assert_eq!(v(&s, "A1").display(), "inside", "{cmd}: the workbook is as the command found it");
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!((v(&s, "A1").display(), v(&s, "A2").display()), ("before".into(), String::new()), "{cmd}");
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(v(&s, "A2").display(), "inside too", "{cmd}");
    }
}

#[test]
fn a_command_that_fails_rolls_back_the_groups_it_opened() {
    // A `?` between `group_begin` and `group_end` used to leave the group open for good: Undo
    // and Redo refused, and later edits made no steps.
    crate::cmd::register_undoable_test_command("test.groupThenFail", |s, _| {
        let _token = s.group_begin("Half")?;
        put(s, "A1", "half done")?;
        Err(EngineError::Other("stopped halfway".into()))
    });
    crate::cmd::register_undoable_test_command("test.editGroupThenFail", |s, _| {
        put(s, "B1", "outside")?;
        let _token = s.group_begin("Half")?;
        put(s, "B2", "inside")?;
        Err(EngineError::Other("stopped halfway".into()))
    });
    let mut s = s();
    set(&mut s, "A1", "before");
    set(&mut s, "A2", "undone");
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("selection.set", json!({"range": "C3"})).unwrap();
    let uid = s.doc().unwrap().uid;
    let err = s.execute("test.groupThenFail", json!({})).unwrap_err().to_string();
    assert!(err.contains("stopped halfway"), "{err}");
    assert!(!s.in_group(uid), "no group is left open");
    assert_eq!(v(&s, "A1").display(), "before");
    assert_eq!(s.doc().unwrap().selection.a1(), "C3");
    assert_eq!(undo_labels(&s), ["Enter Cell"]);
    assert_eq!(s.doc().unwrap().redo.len(), 1, "the redo history comes back");
    assert_eq!(s.execute("history.list", json!({})).unwrap()["grouping"], json!(false));
    set(&mut s, "A3", "next");
    assert_eq!(undo_labels(&s), ["Enter Cell", "Enter Cell"], "later edits make steps again");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "A3").is_empty());

    // What the command changed before opening the group stays, as for any failed command, and is
    // a step of its own; what it changed inside the group goes.
    assert!(s.execute("test.editGroupThenFail", json!({})).is_err());
    assert!(!s.in_group(uid));
    assert_eq!((v(&s, "B1").display(), v(&s, "B2").display()), ("outside".into(), String::new()));
    assert_eq!(undo_labels(&s), ["Enter Cell", "test.editGroupThenFail"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "B1").is_empty());

    // A group open before the command stays open: only the command's own group goes.
    let token = s.group_begin("Outer").unwrap();
    set(&mut s, "D1", "outer");
    assert!(s.execute("test.groupThenFail", json!({})).is_err());
    assert!(s.in_group(uid), "the outer group is still running");
    assert_eq!((v(&s, "A1").display(), v(&s, "D1").display()), ("before".into(), "outer".into()));
    assert!(s.group_end(token).unwrap());
    assert_eq!(undo_labels(&s), ["Enter Cell", "Outer"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "D1").is_empty());
}

#[test]
fn stop_action_rolls_back_a_group_left_open() {
    // A host that dropped its token, or a debugger the person wants to abandon: the message on
    // Undo names the command that stops it, and the command puts the start back.
    let mut s = s();
    set(&mut s, "A1", "start");
    set(&mut s, "A2", "undone");
    s.execute("edit.undo", json!({})).unwrap();
    let uid = s.doc().unwrap().uid;
    let token = s.group_begin("Debug").unwrap();
    set(&mut s, "A1", "step 1");
    drop(s.group_begin("Inner").unwrap());
    set(&mut s, "A3", "step 2");
    let err = s.execute("edit.undo", json!({})).unwrap_err().to_string();
    assert!(err.contains("automate.stopAction"), "{err}");
    assert_eq!(s.execute("automate.stopAction", json!({})).unwrap(), json!({"stopped": true}));
    assert!(!s.in_group(uid));
    assert_eq!(v(&s, "A1").display(), "start");
    assert!(v(&s, "A3").is_empty());
    assert_eq!(undo_labels(&s), ["Enter Cell"], "stopping is not a step of its own");
    assert_eq!(s.doc().unwrap().redo.len(), 1, "the redo history comes back");
    assert!(s.group_end(token).is_err(), "the token is spent");
    assert_eq!(s.execute("automate.stopAction", json!({})).unwrap(), json!({"stopped": false}));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(v(&s, "A2").display(), "undone");
    // Other documents keep their groups.
    let other = s.new_workbook();
    let other_uid = s.documents()[other].uid;
    let _kept = s.group_begin("Theirs").unwrap();
    s.set_active(0);
    assert_eq!(s.execute("automate.stopAction", json!({})).unwrap(), json!({"stopped": false}));
    assert!(s.in_group(other_uid));
}

fn history_of(s: &Session) -> Vec<(String, *const gridcraft_model::Workbook)> {
    let d = s.doc().unwrap();
    d.undo.iter().chain(d.redo.iter()).map(|e| (e.label.clone(), std::sync::Arc::as_ptr(&e.wb))).collect()
}

#[test]
fn queries_take_no_copy_of_the_history() {
    // `execute` copies the history so a panic can put it back; a query leaves it alone, so the
    // copy is skipped for the queries front ends run all the time.
    let mut s = s();
    for i in 0..50 {
        set(&mut s, "A1", &i.to_string());
    }
    s.execute("edit.undo", json!({})).unwrap();
    super::HISTORY_COPIES.with(|n| n.set(0));
    for _ in 0..100 {
        s.execute("history.list", json!({})).unwrap();
        s.execute("cell.get", json!({"cell": "A1"})).unwrap();
        s.execute("document.inspect", json!({})).unwrap();
    }
    assert_eq!(super::HISTORY_COPIES.with(|n| n.get()), 0);
    set(&mut s, "A2", "x");
    assert_eq!(super::HISTORY_COPIES.with(|n| n.get()), 1, "a command that can change it still copies it");
}

#[test]
fn every_query_leaves_the_history_alone() {
    // What lets a query skip the copy: none of them changes the undo or redo history.
    let mut s = Session::new();
    s.execute("file.new", json!({"sample": "sales"})).unwrap();
    set(&mut s, "A1", "1");
    set(&mut s, "A1", "2");
    s.execute("edit.undo", json!({})).unwrap();
    let before = history_of(&s);
    assert_eq!(before.len(), 2);
    let mut queries = 0;
    for spec in crate::command_specs().iter().filter(|c| !c.journal && !c.undoable) {
        queries += 1;
        for params in [json!({}), json!({"range": "A1:B3", "cell": "A1", "sheet": 0}), json!([1, 2])] {
            let _ = s.execute(spec.id, params.clone());
            s.set_active(0);
            assert_eq!(history_of(&s), before, "{} {params}", spec.id);
        }
    }
    assert!(queries > 40, "{queries}");
}

// ---------------------------------------------------------------- recording buffer

fn recorded_ids(s: &Session) -> Vec<String> {
    s.recorder.as_ref().unwrap().steps.iter().map(|st| st.id.clone()).collect()
}

#[test]
fn a_headless_call_that_fails_for_want_of_a_file_is_not_recorded() {
    // `file.open` with no file queues a dialog and succeeds; a headless caller gets an error for
    // it instead, so the recording keeps no step for it either.
    let mut s = s();
    s.start_recording();
    set(&mut s, "A1", "1");
    assert!(s.execute_headless("file.open", json!({})).is_err());
    assert_eq!(recorded_ids(&s), ["cell.set"]);
    let rec = s.stop_recording().unwrap();
    assert_eq!(rec.dropped, 0);
}

#[test]
fn a_call_that_only_opens_a_dialog_is_not_recorded() {
    // Menu-style calls with no parameters only queue their dialog; the dialog's confirming call
    // is the step, so playing the recording back runs each command once, with its parameters.
    let mut s = s();
    s.start_recording();
    for id in ["file.open", "data.fromTextCsv", "home.formatCells"] {
        s.execute(id, json!({})).unwrap_or_else(|e| panic!("{id}: {e}"));
        let dialogs = s.take_ui_requests();
        assert!(dialogs.iter().any(|q| matches!(q, crate::UiRequest::Dialog(..))), "{id} opens a dialog: {dialogs:?}");
    }
    assert!(recorded_ids(&s).is_empty(), "{:?}", recorded_ids(&s));
    set(&mut s, "A1", "1");
    s.execute("home.formatCells", json!({"range": "A1", "style": {"bold": true}})).unwrap();
    assert_eq!(recorded_ids(&s), ["cell.set", "home.formatCells"]);
    // A dialog queued before the call, and not taken yet, does not hide the call.
    s.execute("file.open", json!({})).unwrap();
    set(&mut s, "A2", "2");
    assert_eq!(recorded_ids(&s), ["cell.set", "home.formatCells", "cell.set"]);
    assert_eq!(s.stop_recording().unwrap().dropped, 0);
}

#[test]
fn stop_action_runs_only_on_its_own_and_is_not_recorded() {
    // A step of an action in progress may not stop the action: nested, the command is refused
    // and the action's group stays. Nor is Stop Action a step of the person's work: a recording
    // leaves it out, so playing the recording back never stops the run it is part of.
    let mut s = s();
    s.start_recording();
    let r: crate::Result<()> = s.grouped("Outer", |s| {
        set(s, "A1", "one");
        let err = s.execute_nested("automate.stopAction", json!({})).unwrap_err().to_string();
        assert!(err.contains("on its own"), "{err}");
        set(s, "A2", "two");
        Ok(())
    });
    r.unwrap();
    assert_eq!(v(&s, "A2").display(), "two");
    assert_eq!(undo_labels(&s), ["Outer"], "the action kept its group");
    // On its own it stops a group left open; the recording keeps neither the stop nor the edits
    // the stop rolled back, so playing it back makes only the edits the workbook has.
    let _left = s.group_begin("Left open").unwrap();
    set(&mut s, "A3", "three");
    assert_eq!(s.execute("automate.stopAction", json!({})).unwrap(), json!({"stopped": true}));
    assert!(v(&s, "A3").is_empty());
    assert_eq!(recorded_ids(&s), ["cell.set", "cell.set"]);
    // An edit recorded in another workbook while the group was open stays, as its edit does.
    let first = s.active_index();
    let _left = s.group_begin("Left open").unwrap();
    set(&mut s, "A4", "four");
    s.new_workbook();
    set(&mut s, "B1", "other");
    s.set_active(first);
    assert_eq!(s.execute("automate.stopAction", json!({})).unwrap(), json!({"stopped": true}));
    assert_eq!(recorded_ids(&s), ["cell.set", "cell.set", "cell.set"], "the other workbook's edit stays");
    assert_eq!(s.recorder.as_ref().unwrap().steps.last().map(|st| st.params["cell"].clone()), Some(json!("B1")));
    assert_eq!(s.stop_recording().unwrap().dropped, 0);
}

#[test]
fn rollbacks_take_back_their_own_recorded_steps_in_any_order() {
    // Groups open on two workbooks, each with a recorded edit: rolling back the first takes out
    // only its edit, and the second's mark still finds its own after that.
    let mut s = s();
    s.start_recording();
    let a = s.active_index();
    let _in_a = s.group_begin("A").unwrap();
    set(&mut s, "A1", "a");
    s.new_workbook();
    let b = s.active_index();
    let _in_b = s.group_begin("B").unwrap();
    set(&mut s, "B1", "b");
    s.set_active(a);
    s.execute("automate.stopAction", json!({})).unwrap();
    let cells = |s: &Session| s.recorder.as_ref().unwrap().steps.iter().map(|st| st.params["cell"].clone()).collect::<Vec<_>>();
    assert_eq!(cells(&s), [json!("B1")]);
    s.set_active(b);
    s.execute("automate.stopAction", json!({})).unwrap();
    assert!(cells(&s).is_empty());
    // A recording started again inside a group: every step of it came after the group began.
    let _again = s.group_begin("Again").unwrap();
    s.start_recording();
    set(&mut s, "C1", "c");
    s.execute("automate.stopAction", json!({})).unwrap();
    assert!(cells(&s).is_empty(), "{:?}", cells(&s));
}

#[test]
fn a_run_that_edits_and_opens_a_dialog_is_recorded() {
    // A script with an edit before a menu-style step: the edit happened, so the run is a step of
    // the recording (the person's confirming call is another). Only a call that opened a dialog
    // and changed nothing is left out.
    let mut s = s();
    let edit_then_dialog = json!([{"command": "cell.set", "params": {"cell": "B1", "input": "1"}}, {"command": "home.formatCells", "params": {}}]);
    let dialog_only = json!([{"command": "home.formatCells", "params": {}}]);
    s.execute("automate.newScript", json!({"name": "Edit", "commands": edit_then_dialog})).unwrap();
    s.execute("automate.newScript", json!({"name": "Ask", "commands": dialog_only})).unwrap();
    s.start_recording();
    s.execute("automate.runScript", json!({"name": "Ask"})).unwrap();
    assert!(s.take_ui_requests().iter().any(|q| matches!(q, crate::UiRequest::Dialog(..))));
    assert!(recorded_ids(&s).is_empty(), "{:?}", recorded_ids(&s));
    s.execute("automate.runScript", json!({"name": "Edit"})).unwrap();
    assert!(s.take_ui_requests().iter().any(|q| matches!(q, crate::UiRequest::Dialog(..))));
    assert_eq!(recorded_ids(&s), ["automate.runScript"], "the edit happened, so the run is a step");
    assert_eq!(s.stop_recording().unwrap().dropped, 0);
}

#[test]
fn the_recorder_keeps_top_level_calls_with_where_they_ran() {
    let mut s = s();
    s.execute("home.insertSheet", json!({})).unwrap();
    s.execute("selection.set", json!({"range": "B2:C3"})).unwrap();
    s.start_recording();
    assert!(s.is_recording());
    assert_eq!(s.depth(), 0);
    set(&mut s, "B2", "10");
    s.execute("cell.get", json!({"cell": "B2"})).unwrap(); // a query: not recorded
    assert!(s.execute("sheet.move", json!({})).is_err()); // failed: not recorded
    s.execute("view.customViews", json!({"save": "Mine"})).unwrap();
    // Show runs sheet.activate, selection.set and view.zoom inside it: one step.
    s.execute("view.customViews", json!({"show": "Mine"})).unwrap();
    assert_eq!(recorded_ids(&s), ["cell.set", "view.customViews", "view.customViews"]);
    let first = &s.recorder.as_ref().unwrap().steps[0];
    let d = s.doc().unwrap();
    assert_eq!((first.doc, first.sheet), (d.uid, d.wb.active_sheet()));
    assert_eq!(first.sheet_name, d.wb.active().unwrap().name);
    assert_eq!(first.selection.a1(), "B2:C3", "the selection before the command ran");
    assert_eq!(first.params, json!({"cell": "B2", "input": "10"}));
    assert_eq!(s.depth(), 0);
    let rec = s.stop_recording().unwrap();
    assert_eq!(rec.dropped, 0);
    set(&mut s, "A1", "after");
    assert!(s.recorder.is_none() && !s.is_recording());
}

#[test]
fn the_recorder_survives_journal_trimming() {
    let mut s = s();
    s.start_recording();
    for i in 0..10_050u32 {
        s.execute("selection.set", json!({"range": format!("A{}", i % 50 + 1)})).unwrap();
    }
    set(&mut s, "C1", "last");
    assert!(s.journal.len() < 10_051, "the journal was trimmed");
    let steps = &s.recorder.as_ref().unwrap().steps;
    assert_eq!(steps.len(), 10_051);
    assert_eq!(steps[0].params, json!({"range": "A1"}));
    assert_eq!(steps[10_050].id, "cell.set");
}

#[test]
fn the_recorder_has_a_cap() {
    let mut rec = crate::record::Recorder::default();
    let step = crate::record::RecordedStep {
        doc: 1,
        sheet: 0,
        sheet_name: "Sheet1".into(),
        selection: crate::Selection::default(),
        id: "cell.set".into(),
        params: json!({}),
    };
    for _ in 0..crate::record::RECORD_LIMIT + 3 {
        rec.push(step.clone());
    }
    assert_eq!(rec.steps.len(), crate::record::RECORD_LIMIT);
    assert_eq!(rec.dropped, 3);
}

#[test]
fn the_recorder_has_a_byte_cap_and_keeps_a_prefix() {
    // A `file.open` step holds the whole file as base64: the buffer is bounded in bytes too, and
    // once a step is dropped every later one is, so what is kept is always how the recording began.
    let mut rec = crate::record::Recorder::default();
    let step = |n: usize| crate::record::RecordedStep {
        doc: 1,
        sheet: 0,
        sheet_name: "Sheet1".into(),
        selection: crate::Selection::default(),
        id: "file.open".into(),
        params: json!({"base64": "A".repeat(n)}),
    };
    let big = crate::record::RECORD_MAX_BYTES / 2;
    rec.push(step(big));
    assert_eq!((rec.steps.len(), rec.dropped), (1, 0));
    rec.push(step(big));
    assert_eq!((rec.steps.len(), rec.dropped), (1, 1), "the second does not fit beside the first");
    rec.push(step(1));
    assert_eq!((rec.steps.len(), rec.dropped), (1, 2), "a small step after a dropped one is dropped too");
    let mut rec = crate::record::Recorder::default();
    rec.push(step(crate::record::RECORD_MAX_BYTES + 1));
    assert_eq!((rec.steps.len(), rec.dropped), (0, 1), "a step larger than the whole budget is not kept");
}

#[test]
fn passwords_stay_out_of_the_recording_buffer() {
    const PW: &str = "synthetic-pw-5521";
    let mut s = s();
    s.start_recording();
    s.execute("review.protectSheet", json!({"password": PW, "formatCells": true})).unwrap();
    s.execute("review.unprotectSheet", json!({"password": PW})).unwrap();
    let rec = s.stop_recording().unwrap();
    let text = serde_json::to_string(&rec.steps).unwrap();
    assert!(!text.contains(PW), "{text}");
    assert_eq!(rec.steps[0].params, json!({"formatCells": true}));
}

#[test]
fn record_actions_is_unchanged_by_the_buffer() {
    // Automate › Record Actions keeps recording through the journal; the buffer is separate.
    let mut s = s();
    s.execute("automate.recordActions", json!({})).unwrap();
    assert!(!s.is_recording());
    set(&mut s, "A1", "typed");
    let r = s.execute("automate.recordActions", json!({"stop": true, "name": "Typed"})).unwrap();
    assert_eq!(r["steps"], 1);
    assert!(s.recorder.is_none());
}

// ---------------------------------------------------------------- trust

fn open_json(s: &mut Session, wb: &gridcraft_model::Workbook) {
    let bytes = serde_json::to_vec(wb).unwrap();
    s.execute("file.open", json!({"name": "theirs.json", "base64": crate::io::base64_encode(&bytes)})).unwrap();
}

fn trust(s: &Session) -> Trust {
    s.doc().unwrap().trust
}

#[test]
fn documents_know_where_they_came_from() {
    let mut s = s();
    assert_eq!(trust(&s), Trust::Local);
    assert!(Trust::Local.allows_code() && !Trust::FromFile.allows_code());
    s.execute("file.new", json!({"sample": "sales"})).unwrap();
    assert_eq!(trust(&s), Trust::Local);
    assert_eq!(s.execute("document.inspect", json!({})).unwrap()["trust"], "local");
    open_json(&mut s, &gridcraft_model::Workbook::new());
    assert_eq!(trust(&s), Trust::FromFile);
    assert_eq!(s.execute("document.inspect", json!({})).unwrap()["trust"], "fromFile");
    // A second window on the same workbook is the same workbook.
    s.execute("view.newWindow", json!({})).unwrap();
    assert_eq!(trust(&s), Trust::FromFile);
    // Moving and copying sheets inside it changes nothing.
    s.execute("sheet.move", json!({"to": 0, "copy": true})).unwrap();
    assert_eq!(trust(&s), Trust::FromFile);
}

#[test]
fn whoever_makes_a_document_says_where_it_came_from() {
    // No default trust: a document built from bytes outside `file.open` (a recovery copy, a
    // template) is untrusted because its maker says so, not trusted because it forgot to.
    let mut s = Session::new();
    let bytes = gridcraft_xlsx::write_xlsx(&gridcraft_model::Workbook::new()).unwrap();
    let (wb, _) = crate::io::open_bytes("recovered.xlsx", &bytes).unwrap();
    s.add_document(crate::DocState::new(wb, None, "Recovered".into(), Trust::FromFile));
    assert_eq!(trust(&s), Trust::FromFile);
    assert_eq!(s.execute("document.inspect", json!({})).unwrap()["trust"], "fromFile");
    s.add_document(crate::DocState::new(gridcraft_model::Workbook::new(), None, "Made here".into(), Trust::Local));
    assert_eq!(trust(&s), Trust::Local);
    s.new_workbook();
    assert_eq!(trust(&s), Trust::Local);
}

#[test]
fn pasting_from_a_file_carries_its_trust() {
    let mut s = s(); // document 0: Local
    open_json(&mut s, &gridcraft_model::Workbook::new()); // document 1: FromFile
    let theirs = s.active_index();
    set(&mut s, "A1", "=1+1");
    s.execute("edit.copy", json!({"range": "A1"})).unwrap();
    s.new_workbook();
    let mine = s.active_index();
    // Values carry nothing that runs.
    s.execute("edit.pasteSpecial", json!({"what": "values", "at": "B1"})).unwrap();
    assert_eq!(trust(&s), Trust::Local);
    // Formats carry cell styles and merged areas, nothing that runs.
    s.execute("edit.pasteSpecial", json!({"what": "formats", "at": "B3"})).unwrap();
    assert_eq!(trust(&s), Trust::Local);
    // A linked paste of values still writes formulas.
    s.execute("edit.pasteSpecial", json!({"what": "values", "link": true, "at": "B2"})).unwrap();
    assert_eq!(trust(&s), Trust::FromFile);
    s.new_workbook();
    s.execute("edit.paste", json!({"at": "C1"})).unwrap();
    assert_eq!(trust(&s), Trust::FromFile);
    assert_eq!(v(&s, "C1").display(), "2");

    // Pasting within a workbook changes nothing, and content from a local workbook leaves the
    // destination as it was.
    s.set_active(theirs);
    s.execute("edit.paste", json!({"at": "A3"})).unwrap();
    assert_eq!(trust(&s), Trust::FromFile, "pasting within a workbook");
    s.new_workbook();
    set(&mut s, "A1", "=3");
    s.execute("edit.copy", json!({"range": "A1"})).unwrap();
    s.set_active(mine);
    s.execute("edit.paste", json!({"at": "D1"})).unwrap();
    assert_eq!(trust(&s), Trust::FromFile, "still untrusted from the earlier paste");
    s.new_workbook();
    s.execute("edit.paste", json!({"at": "D1"})).unwrap();
    assert_eq!(trust(&s), Trust::Local);
    assert_eq!(Trust::Local.receive(Trust::FromFile), Trust::FromFile);
    assert_eq!(Trust::FromFile.receive(Trust::Local), Trust::FromFile);
    assert_eq!(Trust::Local.receive(Trust::Local), Trust::Local);
}

#[test]
fn a_linked_paste_carries_the_files_trust_whatever_it_pastes() {
    // Paste Link writes a formula into every cell whatever `what` says, so a linked paste of
    // formats from a file carries the file's trust like one of formulas. Column widths and notes
    // write no cells, linked or not.
    let mut s = s();
    open_json(&mut s, &gridcraft_model::Workbook::new());
    set(&mut s, "A1", "=1+1");
    s.execute("review.newComment", json!({"cell": "A1", "text": "theirs"})).unwrap();
    let wide = gridcraft_model::LineInfo { size: Some(123.0), ..Default::default() };
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap().cols.insert(0, wide);
    s.execute("edit.copy", json!({"range": "A1"})).unwrap();
    let b1 = gridcraft_core::CellRef::parse("B1").unwrap();
    let sheet = |s: &Session| s.doc().unwrap().wb.sheet(0).unwrap().clone();
    for what in ["columnWidths", "comments"] {
        s.new_workbook();
        s.execute("edit.pasteSpecial", json!({"what": what, "link": true, "at": "B1"})).unwrap();
        assert_eq!(trust(&s), Trust::Local, "{what}");
        let sh = sheet(&s);
        assert!(sh.cell(b1).is_none(), "a linked paste of {what} writes no cell");
        if what == "comments" {
            assert_eq!(sh.comments.get(&b1).map(|c| c.text.as_str()), Some("theirs"));
        } else {
            assert_eq!(sh.cols.get(&1).and_then(|c| c.size), Some(123.0));
        }
    }
    s.new_workbook();
    s.execute("edit.pasteSpecial", json!({"what": "formats", "link": true, "at": "B1"})).unwrap();
    assert!(sheet(&s).cell(b1).is_some_and(|c| c.formula.is_some()), "a linked paste of formats writes a formula");
    assert_eq!(trust(&s), Trust::FromFile);
}

#[test]
fn a_sheet_keeps_its_identity_until_it_is_copied_or_read_from_a_file() {
    let mut s = s();
    let uids = |s: &Session| s.doc().unwrap().wb.sheets.iter().map(|sh| sh.uid.get()).collect::<Vec<_>>();
    let first = uids(&s)[0];
    // Renamed, moved, deleted and brought back by undo: the same sheet.
    s.execute("home.insertSheet", json!({"name": "Other"})).unwrap();
    s.execute("sheet.rename", json!({"sheet": "Sheet1", "name": "Data"})).unwrap();
    s.execute("sheet.move", json!({"sheet": "Data", "to": 1})).unwrap();
    let data = s.doc().unwrap().wb.sheet_index("Data").unwrap();
    assert_eq!(uids(&s)[data], first);
    s.execute("home.deleteSheet", json!({"sheet": "Data"})).unwrap();
    assert!(!uids(&s).contains(&first));
    s.execute("edit.undo", json!({})).unwrap();
    assert!(uids(&s).contains(&first), "undo brings the same sheet back");
    // A copy, or a new sheet of the same name, is another sheet.
    s.execute("sheet.move", json!({"sheet": "Data", "to": 0, "copy": true})).unwrap();
    let all = uids(&s);
    assert_eq!(all.len(), 3);
    assert_eq!(all.iter().collect::<std::collections::BTreeSet<_>>().len(), 3, "{all:?}");
    // Read from a file: new sheets.
    let bytes = crate::io::save_bytes(&s.doc().unwrap().wb, "book.xlsx").unwrap();
    let (read, _) = crate::io::open_bytes("book.xlsx", &bytes).unwrap();
    assert!(read.sheets.iter().all(|sh| !all.contains(&sh.uid.get())));
}
