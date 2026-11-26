//! Review tab: protection, spelling (basic), workbook statistics, comment navigation and the
//! Comments pane, Show All Notes and Convert to Comments.

use gridcraft_core::CellRef;
use gridcraft_model::{EditRange, PasswordHash, SheetProtection, Visibility, password_hash};
use serde_json::{Value as Json, json};

use super::{CommandSpec, DocState, EngineError, Result, Session, bad, bool_param, cmd, edit, has_doc, str_param, target_ranges, target_sheet};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "review.protectSheet",
            "Protect Sheet…",
            ["Review", "Protect"],
            None,
            "{password?, formatCells?, formatColumns?, formatRows?, insertRows?, insertColumns?, deleteRows?, deleteColumns?, sort?, autofilter?}",
            has_doc,
            protect_sheet
        ),
        cmd!("review.unprotectSheet", "Unprotect Sheet", ["Review", "Protect"], None, "{password?}", has_doc, unprotect_sheet),
        cmd!(
            "review.protectWorkbook",
            "Protect Workbook",
            ["Review", "Protect"],
            None,
            "{on?: bool (default: toggle), password?} (structure protection; unprotecting needs the password when one was set)",
            has_doc,
            protect_workbook
        ),
        cmd!(query "review.workbookStatistics", "Workbook Statistics", ["Review", "Proofing"], None, "{}", has_doc, stats),
        // Not journaled: the params can hold a password.
        CommandSpec {
            id: "review.allowEditRanges",
            label: "Allow Edit Ranges…",
            menu: &["Review", "Protect"],
            shortcut: None,
            shortcut_mac: None,
            params: "{action?: list (default) | add | modify | delete | unlock, title?, range?, password?, newTitle?, sheet?} → ranges",
            enabled: has_doc,
            run: allow_edit_ranges,
            journal: false,
            undoable: true,
        },
        cmd!(query "review.checkAccessibility", "Check Accessibility", ["Review", "Accessibility"], None, "{} → issues", has_doc, accessibility),
        cmd!(noundo "review.previousComment", "Previous Comment", ["Review", "Comments"], None, "{} → {sheet, cell}", has_comments, |s, _| {
            step_comment(s, false)
        }),
        cmd!(noundo "review.nextComment", "Next Comment", ["Review", "Comments"], None, "{} → {sheet, cell}", has_comments, |s, _| step_comment(
            s, true
        )),
        cmd!(query "review.showComments", "Show Comments", ["Review", "Comments"], None, "{on?: bool} (toggles the Comments pane)", has_doc, |s, p| {
            s.comments_pane = bool_param(p, "on").unwrap_or(!s.comments_pane);
            Ok(json!({"on": s.comments_pane}))
        }),
        cmd!(query "review.showAllNotes", "Show All Notes", ["Review", "Notes"], None, "{on?: bool}", has_doc, |s, p| {
            s.show_all_notes = bool_param(p, "on").unwrap_or(!s.show_all_notes);
            Ok(json!({"on": s.show_all_notes}))
        }),
        cmd!(
            "review.convertToComments",
            "Convert to Comments",
            ["Review", "Notes"],
            None,
            "{} (every note in the workbook) → {converted}",
            has_doc,
            convert_notes
        ),
    ]
}

/// Threaded comments in navigation order: visible sheets in tab order, then row by row.
fn comment_cells(s: &Session) -> Vec<(usize, CellRef)> {
    let Some(d) = s.active() else { return vec![] };
    d.wb.sheets
        .iter()
        .enumerate()
        .filter(|(_, sh)| sh.visibility == Visibility::Visible)
        // `CellRef` orders by row, then column: reading order.
        .flat_map(|(i, sh)| sh.comments.iter().filter(|(_, c)| c.threaded).map(move |(at, _)| (i, *at)))
        .collect()
}

fn has_comments(s: &Session) -> std::result::Result<(), String> {
    has_doc(s)?;
    if comment_cells(s).is_empty() { Err("there are no comments in this workbook".into()) } else { Ok(()) }
}

/// Selects the next (or previous) threaded comment after the active cell, moving on to the next
/// sheet and wrapping around at the end of the workbook.
fn step_comment(s: &mut Session, forward: bool) -> Result<Json> {
    let list = comment_cells(s);
    let d = s.doc()?;
    let here = (d.wb.active_sheet(), d.selection.active);
    let target = if forward {
        list.iter().find(|c| **c > here).or_else(|| list.first())
    } else {
        list.iter().rev().find(|c| **c < here).or_else(|| list.last())
    };
    let Some(&(sheet, at)) = target else {
        return Err(EngineError::Other("There are no comments in this workbook.".into()));
    };
    let d = s.doc_mut()?;
    d.wb_switch_sheet(sheet);
    d.selection = crate::Selection::at(at);
    if let Some(sh) = d.wb.active() {
        d.selection.expand_merges(sh);
    }
    let name = d.wb.active().map(|sh| sh.name.clone()).unwrap_or_default();
    Ok(json!({"sheet": name, "cell": at.a1()}))
}

/// Review › Notes › Convert to Comments: every note in the workbook becomes a threaded comment
/// with the same author and text.
fn convert_notes(s: &mut Session, _: &Json) -> Result<Json> {
    let sheets: Vec<usize> =
        s.doc()?.wb.sheets.iter().enumerate().filter(|(_, sh)| sh.comments.values().any(|c| !c.threaded)).map(|(i, _)| i).collect();
    if sheets.is_empty() {
        return Ok(json!({"converted": 0}));
    }
    edit(s, |cx| {
        let mut n = 0usize;
        for i in &sheets {
            for c in cx.sheet_mut(*i)?.comments.values_mut().filter(|c| !c.threaded) {
                c.threaded = true;
                c.visible = false;
                n += 1;
            }
        }
        Ok(json!({"converted": n}))
    })
}

fn protect_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    // Re-protecting a password-protected sheet would replace the password without knowing it:
    // unprotect it first, as in Excel. One without a password can take new options directly.
    let has_password = |pr: &SheetProtection| pr.password.is_some() || pr.legacy_password_hash.is_some();
    if s.doc()?.wb.sheet(sheet).and_then(|sh| sh.protection.as_ref()).is_some_and(has_password) {
        return Err(EngineError::Other("The sheet is protected with a password. Unprotect it first (Review › Unprotect Sheet).".into()));
    }
    let b = |k: &str| bool_param(p, k).unwrap_or(false);
    // Selection rights default to what the sheet has now (a file may forbid selecting locked cells).
    let current = s.doc()?.wb.sheet(sheet).and_then(|sh| sh.protection.clone());
    let select_locked = bool_param(p, "selectLockedCells").unwrap_or_else(|| current.as_ref().is_none_or(|c| c.select_locked));
    let select_unlocked = bool_param(p, "selectUnlockedCells").unwrap_or_else(|| current.as_ref().is_none_or(|c| c.select_unlocked));
    let prot = SheetProtection {
        password: None,
        legacy_password_hash: None,
        select_locked,
        select_unlocked,
        format_cells: b("formatCells"),
        format_columns: b("formatColumns"),
        format_rows: b("formatRows"),
        insert_columns: b("insertColumns"),
        insert_rows: b("insertRows"),
        delete_columns: b("deleteColumns"),
        delete_rows: b("deleteRows"),
        sort: b("sort"),
        autofilter: b("autofilter"),
    };
    let d = s.doc()?;
    let what = d.wb.sheet(sheet).map(|sh| sh.name.clone()).unwrap_or_default();
    let prot = SheetProtection { password: str_param(p, "password").filter(|x| !x.is_empty()).map(|pw| new_password_hash(d, &what, pw)), ..prot };
    edit(s, |cx| {
        // FORMULATEXT reads a sheet's protection: its results are recalculated.
        cx.recalc_all = true;
        cx.sheet_mut(sheet)?.protection = Some(prot.clone());
        Ok(Json::Null)
    })
}

fn unprotect_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    if let Some(pr) = s.doc()?.wb.sheet(sheet).and_then(|sh| sh.protection.as_ref()) {
        let pw = str_param(p, "password").unwrap_or("");
        let ok = match (&pr.password, pr.legacy_password_hash) {
            (Some(h), _) => h.verify(pw).map_err(EngineError::Other)?,
            (None, Some(h)) => password_hash(pw) == h,
            (None, None) => true,
        };
        if !ok {
            return Err(EngineError::Other("The password you supplied is not correct.".into()));
        }
    }
    edit(s, |cx| {
        // Only removing protection changes what FORMULATEXT reads; a no-op unprotect skips the full recalculation.
        if cx.sheet_mut(sheet)?.protection.take().is_some() {
            cx.recalc_all = true;
        }
        Ok(Json::Null)
    })
}

/// A new Excel-compatible (SHA-512, salted) hash of `pw`. The salt mixes `what` is protected
/// with the document, a counter and the clock, so equal passwords get different hashes.
fn new_password_hash(d: &DocState, what: &str, pw: &str) -> PasswordHash {
    static SALT_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SALT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let seed = format!("{what}\u{0}{n}\u{0}{}\u{0}{}", d.uid, gridcraft_calc::now_serial().to_bits());
    PasswordHash::new_sha512(pw, &gridcraft_model::password::salt_from(seed.as_bytes()))
}

fn range_text(r: &EditRange) -> String {
    r.ranges.iter().map(|x| x.a1()).collect::<Vec<_>>().join(",")
}

/// Review › Allow Edit Ranges: list, add, modify or delete the ranges that stay editable on a
/// protected sheet, or unlock a password range for this session.
fn allow_edit_ranges(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let action = str_param(p, "action").unwrap_or("list");
    let title = str_param(p, "title").map(str::trim).filter(|t| !t.is_empty());
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let sheet_name = sh.name.clone();
    let find = |t: Option<&str>| -> Result<usize> {
        let t = t.ok_or_else(|| bad("review.allowEditRanges", "missing `title`"))?;
        sh.edit_ranges
            .iter()
            .position(|r| r.title.eq_ignore_ascii_case(t))
            .ok_or_else(|| bad("review.allowEditRanges", format!("no range titled `{t}`")))
    };
    let hash = |pw: &str, title: &str| new_password_hash(d, &format!("{sheet_name}\u{0}{title}"), pw);
    if matches!(action, "add" | "modify" | "delete") && sh.is_protected() {
        return Err(EngineError::Other("Unprotect the sheet to change its edit ranges.".into()));
    }
    match action {
        "list" => {}
        "unlock" => {
            let i = find(title)?;
            let Some(r) = sh.edit_ranges.get(i) else { return Err(bad("review.allowEditRanges", "no such range")) };
            if let Some(h) = &r.password {
                let pw = str_param(p, "password").unwrap_or("");
                if !h.verify(pw).map_err(EngineError::Other)? {
                    return Err(EngineError::Other("The password you supplied is not correct.".into()));
                }
                let key = (sheet_name.clone(), r.title.clone());
                let d = s.doc_mut()?;
                if !d.unlocked_ranges.contains(&key) {
                    d.unlocked_ranges.push(key);
                }
            }
        }
        "add" => {
            let t = match title {
                Some(t) => t.to_string(),
                None => {
                    (1..).map(|n| format!("Range{n}")).find(|t| !sh.edit_ranges.iter().any(|r| r.title.eq_ignore_ascii_case(t))).unwrap_or_default()
                }
            };
            if sh.edit_ranges.iter().any(|r| r.title.eq_ignore_ascii_case(&t)) {
                return Err(bad("review.allowEditRanges", format!("a range titled `{t}` already exists")));
            }
            let ranges = target_ranges(s, p)?;
            let password = str_param(p, "password").filter(|pw| !pw.is_empty()).map(|pw| hash(pw, &t));
            let r = EditRange { title: t, ranges, password, security_descriptor: None };
            edit(s, |cx| {
                cx.sheet_mut(sheet)?.edit_ranges.push(r.clone());
                Ok(())
            })?;
        }
        "modify" => {
            let i = find(title)?;
            let mut r = sh.edit_ranges.get(i).cloned().ok_or_else(|| bad("review.allowEditRanges", "no such range"))?;
            if let Some(n) = str_param(p, "newTitle").map(str::trim).filter(|n| !n.is_empty()) {
                if sh.edit_ranges.iter().enumerate().any(|(j, x)| j != i && x.title.eq_ignore_ascii_case(n)) {
                    return Err(bad("review.allowEditRanges", format!("a range titled `{n}` already exists")));
                }
                r.title = n.to_string();
            }
            if let Some(pw) = str_param(p, "password") {
                r.password = (!pw.is_empty()).then(|| hash(pw, &r.title));
            }
            if p.get("range").is_some() {
                r.ranges = target_ranges(s, p)?;
            }
            edit(s, |cx| {
                if let Some(x) = cx.sheet_mut(sheet)?.edit_ranges.get_mut(i) {
                    *x = r.clone();
                }
                Ok(())
            })?;
        }
        "delete" => {
            let i = find(title)?;
            edit(s, |cx| {
                let list = &mut cx.sheet_mut(sheet)?.edit_ranges;
                if i < list.len() {
                    list.remove(i);
                }
                Ok(())
            })?;
        }
        other => return Err(bad("review.allowEditRanges", format!("unknown action `{other}`"))),
    }
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    Ok(Json::Array(
        sh.edit_ranges
            .iter()
            .map(|r| {
                let unlocked = r.password.is_none() || d.unlocked_ranges.iter().any(|(s, t)| *s == sh.name && *t == r.title);
                json!({"title": r.title, "range": range_text(r), "password": r.password.is_some(), "unlocked": unlocked})
            })
            .collect(),
    ))
}

/// Review › Protect Workbook: lock the workbook structure (sheets can't be added, moved,
/// renamed, hidden or deleted), optionally with a password that unprotecting must match.
fn protect_workbook(s: &mut Session, p: &Json) -> Result<Json> {
    let d = s.doc()?;
    let cur = d.wb.protected_structure;
    let on = bool_param(p, "on").unwrap_or(!cur);
    if on == cur {
        return Ok(json!({"on": on}));
    }
    let password = if on {
        str_param(p, "password").filter(|pw| !pw.is_empty()).map(|pw| new_password_hash(d, "workbook", pw))
    } else {
        if let Some(h) = &d.wb.structure_password
            && !h.verify(str_param(p, "password").unwrap_or("")).map_err(EngineError::Other)?
        {
            return Err(EngineError::Other("The password you supplied is not correct.".into()));
        }
        None
    };
    edit(s, |cx| {
        cx.wb.protected_structure = on;
        cx.wb.structure_password = password.clone();
        Ok(json!({"on": on}))
    })
}

fn stats(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc()?;
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    let formulas = sh.cells.iter().filter(|(_, c)| c.formula.is_some()).count();
    Ok(json!({
        "sheet": {"name": sh.name, "endOfSheet": sh.used_range().map(|r| r.end.a1()), "cellsWithData": sh.cells.iter().filter(|(_, c)| !c.value.is_empty()).count(), "tables": sh.tables.len(), "formulas": formulas, "charts": sh.charts.len(), "comments": sh.comments.len()},
        "workbook": {"sheets": d.wb.sheets.len(), "cellsWithData": d.wb.sheets.iter().map(|s| s.cells.len()).sum::<usize>(), "tables": d.wb.sheets.iter().map(|s| s.tables.len()).sum::<usize>(), "charts": d.wb.sheets.iter().map(|s| s.charts.len()).sum::<usize>(), "names": d.wb.names.len()}
    }))
}

fn accessibility(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc()?;
    let mut issues = Vec::new();
    for sh in &d.wb.sheets {
        for im in &sh.images {
            if im.alt.is_empty() {
                issues.push(json!({"sheet": sh.name, "issue": "Missing alternative text", "object": format!("Picture {}", im.id)}));
            }
        }
        if !sh.merges.is_empty() {
            issues.push(json!({"sheet": sh.name, "issue": "Merged cells", "count": sh.merges.len()}));
        }
        if sh.name.starts_with("Sheet") {
            issues.push(json!({"sheet": sh.name, "issue": "Default sheet name"}));
        }
    }
    Ok(Json::Array(issues))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Session {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("home.insertSheet", json!({"name": "Two"})).unwrap();
        s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
        for (sheet, cell) in [(0, "C1"), (0, "A3"), (1, "B2")] {
            s.execute("review.newComment", json!({"sheet": sheet, "cell": cell, "text": "c", "author": "Ann"})).unwrap();
        }
        s.execute("review.newNote", json!({"sheet": 0, "cell": "A2", "text": "note", "author": "Bob"})).unwrap();
        s
    }

    fn at(s: &Session) -> (usize, String) {
        let d = s.doc().unwrap();
        (d.wb.active_sheet(), d.selection.active.a1())
    }

    #[test]
    fn next_and_previous_comment_go_in_reading_order_across_sheets_and_wrap() {
        let mut s = book();
        s.execute("selection.set", json!({"cell": "B1"})).unwrap();
        // Notes are skipped: A2 holds a note, not a comment.
        let order = [(0, "C1"), (0, "A3"), (1, "B2"), (0, "C1")];
        for (sheet, cell) in order {
            s.execute("review.nextComment", json!({})).unwrap();
            assert_eq!(at(&s), (sheet, cell.to_string()));
        }
        for (sheet, cell) in [(1, "B2"), (0, "A3"), (0, "C1")] {
            s.execute("review.previousComment", json!({})).unwrap();
            assert_eq!(at(&s), (sheet, cell.to_string()));
        }
    }

    #[test]
    fn comment_navigation_is_disabled_without_comments() {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("review.newNote", json!({"cell": "A1", "text": "note"})).unwrap();
        assert!(s.execute("review.nextComment", json!({})).is_err());
        assert!(s.execute("review.previousComment", json!({})).is_err());
        assert_eq!(at(&s), (0, "A1".to_string()));
    }

    #[test]
    fn protecting_and_unprotecting_leave_table_headers_alone() {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [["a", "b"], [1, 2]]})).unwrap();
        s.execute("insert.table", json!({"range": "A1:B2", "header": true})).unwrap();
        // An imported header can hold a number: only a structural edit would turn it back into text.
        let d = s.doc_mut().unwrap();
        std::sync::Arc::make_mut(&mut d.wb).sheet_mut(0).unwrap().set_value(gridcraft_core::CellRef::new(0, 0), gridcraft_core::Value::Number(7.0));
        s.execute("review.protectSheet", json!({})).unwrap();
        s.execute("review.unprotectSheet", json!({})).unwrap();
        let v = s.doc().unwrap().wb.sheet(0).unwrap().value(gridcraft_core::CellRef::new(0, 0));
        assert_eq!(v, gridcraft_core::Value::Number(7.0));
    }

    #[test]
    fn protection_that_forbids_selecting_locked_cells_is_enforced_and_kept() {
        let mut s = book();
        s.execute("home.lockCell", json!({"range": "B2", "on": false})).unwrap();
        s.execute("review.protectSheet", json!({"selectLockedCells": false})).unwrap();
        // Locked cells can't be selected; unlocked ones can.
        assert!(s.execute("selection.set", json!({"cell": "A1"})).is_err());
        s.execute("selection.set", json!({"cell": "B2"})).unwrap();
        // Arrow keys skip locked cells, and stay put when nothing selectable is left.
        s.execute("selection.move", json!({"dr": 0, "dc": 1})).unwrap();
        assert_eq!(s.doc().unwrap().selection.active.a1(), "B2");
        // Re-protecting without the flag keeps the sheet's selection rights.
        s.execute("review.unprotectSheet", json!({})).unwrap();
        s.execute("review.protectSheet", json!({"selectLockedCells": false})).unwrap();
        s.execute("review.protectSheet", json!({"sort": true})).unwrap();
        assert!(!s.doc().unwrap().wb.active().unwrap().protection.clone().unwrap().select_locked);
        s.execute("review.unprotectSheet", json!({})).unwrap();
        s.execute("selection.set", json!({"cell": "A1"})).unwrap();
    }

    #[test]
    fn sheet_passwords_use_excel_hashes_and_old_json_hashes_still_verify() {
        let mut s = book();
        s.execute("review.protectSheet", json!({"password": "pa55"})).unwrap();
        let prot = s.doc().unwrap().wb.active().unwrap().protection.clone().unwrap();
        assert!(matches!(&prot.password, Some(PasswordHash::Iterated { algorithm, .. }) if algorithm == "SHA-512"));
        assert!(s.execute("review.unprotectSheet", json!({})).is_err());
        assert!(s.execute("review.unprotectSheet", json!({"password": "pass"})).is_err());
        // Protecting again can't swap the password out from under it.
        assert!(s.execute("review.protectSheet", json!({})).is_err());
        assert_eq!(s.doc().unwrap().wb.active().unwrap().protection.as_ref(), Some(&prot));
        s.execute("review.unprotectSheet", json!({"password": "pa55"})).unwrap();
        assert!(!s.doc().unwrap().wb.active().unwrap().is_protected());
        // A GridCraft JSON workbook saved before carried the internal hash under `password_hash`.
        let old = format!(
            r#"{{"password_hash": {}, "select_locked": true, "select_unlocked": true, "format_cells": false, "format_columns": false, "format_rows": false, "insert_columns": false, "insert_rows": false, "delete_columns": false, "delete_rows": false, "sort": false, "autofilter": false}}"#,
            password_hash("old")
        );
        let prot: SheetProtection = serde_json::from_str(&old).unwrap();
        assert_eq!((prot.legacy_password_hash, prot.password.is_none()), (Some(password_hash("old")), true));
        let i = s.doc().unwrap().wb.active_sheet();
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        std::sync::Arc::make_mut(&mut wb.sheets[i]).protection = Some(prot);
        assert!(s.execute("review.unprotectSheet", json!({"password": "new"})).is_err());
        s.execute("review.unprotectSheet", json!({"password": "old"})).unwrap();
    }

    #[test]
    fn protected_structure_blocks_rename_move_copy_hide_and_unhide_sheet() {
        let mut s = book();
        let names = |s: &Session| s.doc().unwrap().wb.sheets.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
        let before = names(&s);
        s.execute("review.protectWorkbook", json!({})).unwrap();
        assert!(s.execute("sheet.rename", json!({"sheet": 0, "name": "Renamed"})).is_err());
        assert!(s.execute("sheet.move", json!({"sheet": 0, "to": 1})).is_err());
        assert!(s.execute("sheet.move", json!({"sheet": 0, "to": 1, "copy": true})).is_err());
        assert!(s.execute("sheet.hide", json!({"sheet": 1})).is_err());
        assert_eq!(names(&s), before);
        assert!(s.doc().unwrap().wb.sheets.iter().all(|x| x.visibility == Visibility::Visible));
        s.execute("review.protectWorkbook", json!({})).unwrap();
        s.execute("sheet.rename", json!({"sheet": 0, "name": "Renamed"})).unwrap();
        s.execute("sheet.move", json!({"sheet": 0, "to": 1})).unwrap();
        s.execute("sheet.move", json!({"sheet": 0, "to": 1, "copy": true})).unwrap();
        assert_eq!(s.doc().unwrap().wb.sheets.len(), 3);
        s.execute("sheet.hide", json!({"sheet": 1})).unwrap();
        s.execute("review.protectWorkbook", json!({})).unwrap();
        assert!(s.execute("sheet.unhide", json!({"sheet": 1})).is_err());
        assert!(s.execute("sheet.unhide", json!({})).is_err());
        assert_eq!(s.doc().unwrap().wb.sheets[1].visibility, Visibility::Hidden);
        s.execute("review.protectWorkbook", json!({})).unwrap();
        s.execute("sheet.unhide", json!({"sheet": 1})).unwrap();
    }

    #[test]
    fn protect_workbook_with_a_password_really_verifies() {
        let mut s = book();
        s.execute("review.protectWorkbook", json!({"password": "pa55"})).unwrap();
        assert!(s.doc().unwrap().wb.protected_structure);
        assert!(s.execute("home.insertSheet", json!({"name": "Three"})).is_err(), "the structure is locked");
        // A second Protect keeps it protected; unprotecting needs the right password.
        assert_eq!(s.execute("review.protectWorkbook", json!({"on": true})).unwrap()["on"], true);
        assert!(s.execute("review.protectWorkbook", json!({})).is_err());
        assert!(s.execute("review.protectWorkbook", json!({"on": false, "password": "pass"})).is_err());
        assert!(s.doc().unwrap().wb.protected_structure);
        s.execute("review.protectWorkbook", json!({"on": false, "password": "pa55"})).unwrap();
        let wb = &s.doc().unwrap().wb;
        assert!(!wb.protected_structure && wb.structure_password.is_none());
        // Without a password it toggles freely.
        s.execute("review.protectWorkbook", json!({})).unwrap();
        s.execute("review.protectWorkbook", json!({})).unwrap();
        assert!(!s.doc().unwrap().wb.protected_structure);
    }

    #[test]
    fn convert_to_comments_turns_every_note_into_a_comment_and_undoes() {
        let mut s = book();
        s.execute("review.newNote", json!({"sheet": 1, "cell": "D4", "text": "second", "author": "Cy"})).unwrap();
        s.execute("review.showNote", json!({"sheet": 1, "cell": "D4"})).unwrap();
        let r = s.execute("review.convertToComments", json!({})).unwrap();
        assert_eq!(r["converted"], 2);
        let wb = &s.doc().unwrap().wb;
        let a2 = &wb.sheets[0].comments[&CellRef::parse("A2").unwrap()];
        let d4 = &wb.sheets[1].comments[&CellRef::parse("D4").unwrap()];
        assert!(a2.threaded && d4.threaded && !d4.visible);
        assert_eq!((a2.author.as_str(), a2.text.as_str(), d4.author.as_str(), d4.text.as_str()), ("Bob", "note", "Cy", "second"));
        // Nothing left to convert; one undo restores the notes.
        assert_eq!(s.execute("review.convertToComments", json!({})).unwrap()["converted"], 0);
        s.execute("edit.undo", json!({})).unwrap();
        assert!(!s.doc().unwrap().wb.sheets[0].comments[&CellRef::parse("A2").unwrap()].threaded);
    }

    #[test]
    fn show_comments_and_show_all_notes_toggle() {
        let mut s = book();
        assert_eq!(s.execute("review.showComments", json!({})).unwrap()["on"], true);
        assert_eq!(s.execute("review.showComments", json!({})).unwrap()["on"], false);
        assert_eq!(s.execute("review.showComments", json!({"on": true})).unwrap()["on"], true);
        assert!(s.comments_pane);
        assert_eq!(s.execute("review.showAllNotes", json!({})).unwrap()["on"], true);
        assert_eq!(s.execute("review.showAllNotes", json!({"on": false})).unwrap()["on"], false);
        assert!(!s.show_all_notes);
    }
}

#[cfg(test)]
mod edit_range_tests {
    use super::*;

    fn protected_book() -> Session {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("review.allowEditRanges", json!({"action": "add", "title": "Open", "range": "A1:A3"})).unwrap();
        s.execute("review.allowEditRanges", json!({"action": "add", "title": "Budget", "range": "B1:B3,D1", "password": "pa55"})).unwrap();
        s.execute("review.protectSheet", json!({})).unwrap();
        s
    }

    #[test]
    fn edit_ranges_open_locked_cells_and_passwords_really_verify() {
        let mut s = protected_book();
        s.execute("cell.set", json!({"cell": "A2", "input": "1"})).unwrap();
        assert!(s.execute("cell.set", json!({"cell": "C1", "input": "1"})).is_err());
        let err = s.execute("cell.set", json!({"cell": "D1", "input": "1"})).unwrap_err().to_string();
        assert!(err.contains("Budget"), "{err}");
        // A paste or fill is checked the same way.
        assert!(s.execute("range.setValues", json!({"range": "B2", "values": [[5]]})).is_err());
        assert!(s.execute("review.allowEditRanges", json!({"action": "unlock", "title": "Budget", "password": "pass"})).is_err());
        assert!(s.execute("review.allowEditRanges", json!({"action": "unlock", "title": "Budget"})).is_err());
        let list = s.execute("review.allowEditRanges", json!({"action": "unlock", "title": "budget", "password": "pa55"})).unwrap();
        assert_eq!(list[1]["unlocked"], true);
        s.execute("range.setValues", json!({"range": "B2", "values": [[5]]})).unwrap();
        s.execute("cell.set", json!({"cell": "D1", "input": "2"})).unwrap();
        // Ranges can't change while the sheet is protected.
        assert!(s.execute("review.allowEditRanges", json!({"action": "delete", "title": "Open"})).is_err());
    }

    #[test]
    fn edit_ranges_add_modify_delete_list_and_undo() {
        let mut s = Session::new();
        s.new_workbook();
        // Empty params list; nothing to list yet.
        assert_eq!(s.execute("review.allowEditRanges", json!({})).unwrap(), json!([]));
        s.execute("selection.set", json!({"range": "C3:D4"})).unwrap();
        let list = s.execute("review.allowEditRanges", json!({"action": "add"})).unwrap();
        assert_eq!(list, json!([{"title": "Range1", "range": "C3:D4", "password": false, "unlocked": true}]));
        assert!(s.execute("review.allowEditRanges", json!({"action": "add", "title": "range1", "range": "A1"})).is_err());
        let list = s
            .execute("review.allowEditRanges", json!({"action": "modify", "title": "Range1", "newTitle": "Mine", "range": "E5", "password": "x"}))
            .unwrap();
        assert_eq!((list[0]["title"].as_str(), list[0]["range"].as_str(), list[0]["password"].as_bool()), (Some("Mine"), Some("E5"), Some(true)));
        // Inserting a row above moves the range with its cells.
        s.execute("home.insertRows", json!({"rows": "1:1"})).unwrap();
        assert_eq!(s.execute("review.allowEditRanges", json!({})).unwrap()[0]["range"], "E6");
        s.execute("review.allowEditRanges", json!({"action": "delete", "title": "Mine"})).unwrap();
        assert_eq!(s.execute("review.allowEditRanges", json!({})).unwrap(), json!([]));
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.execute("review.allowEditRanges", json!({})).unwrap()[0]["title"], "Mine");
        assert!(s.execute("review.allowEditRanges", json!({"action": "nope"})).is_err());
        assert!(s.execute("review.allowEditRanges", json!({"action": "delete"})).is_err());
    }
}
