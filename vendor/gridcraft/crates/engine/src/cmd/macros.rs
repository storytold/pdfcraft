//! Macros: typed command lists that run as one undo step. See [`crate::macros`] for the model,
//! tokens, runner, debugger, recorder and policy. Automate scripts (`automate.*`) are a separate,
//! older feature with their own storage; these commands neither read nor change it. A recording
//! keeps the steps a script run ran while it recorded, as the run reported them.

use serde_json::{Value as Json, json};

use super::{CommandSpec, EngineError, Result, Session, bad, bool_param, cmd, edit, has_doc, str_param};
use crate::macros::recorder::RecordingStart;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "macro.list", "List Macros", [], None, "{} → [{name, description, shortcut?, steps, inputs: [names], onConflict, permissions}]", has_doc, list),
        cmd!(query "macro.get", "Get Macro", [], None, "{name} → the macro: {v, name, description, shortcut?, inputs, steps, output, permissions}", has_doc, get),
        cmd!(
            "macro.save",
            "Save Macro",
            [],
            None,
            "{macro: {name, description?, shortcut?, inputs?: [{name, kind: cell|range, rows: [min, max], cols: [min, max], header, colTypes: [number|text|date|bool|any], nonEmpty, defaultRef?}], steps: [{type: command, id, params, enabled?, breakpoint?, note?} | {type: repeat, over, of: rows|columns, max, body} | {type: if, cell, op: eq|ne|gt|ge|lt|le|empty|notEmpty|contains, value, then, else} | {type: stop, message, rollback?}], output?: {anchor?, onConflict: ask|fail|overwrite|shiftDown|shiftRight}, permissions?: [command ids]} (or those fields at the top level), replace?: bool (default false: saving over a macro with the name is refused)} → {name, steps}",
            has_doc,
            save
        ),
        cmd!("macro.delete", "Delete Macro", [], None, "{name}", has_doc, delete),
        cmd!("macro.rename", "Rename Macro", [], None, "{name, to}", has_doc, rename),
        cmd!(
            "macro.run",
            "Run Macro",
            [],
            None,
            "{name, inputs?: {Input: \"B2:D9\"} | \"B2:D9\", output?: \"F2\" (instead of the macro's output anchor), onConflict?: ask|fail|overwrite|shiftDown|shiftRight, overwrite?: bool, approve?: bool, timeLimit?: seconds} runs as one undo step; any error rolls it all back → {macro, ran, inputs, stopped?, shifted?}",
            has_doc,
            run
        ),
        cmd!(
            query "macro.check",
            "Check Macro",
            [],
            None,
            "{name, inputs?, output?: \"F2\" (as macro.run), onConflict?, overwrite?, timeLimit?: seconds} validates the inputs and dry-runs the macro on the workbook (then puts everything back) → {ok (macro.run with these parameters would go ahead without a question), errors?, needsApproval?, onConflict, inputs, output?, conflicts, footprint}",
            has_doc,
            check
        ),
        cmd!(
            noundo "macro.record",
            "Record Macro",
            ["View", "Macros"],
            None,
            "{stop?: bool, cancel?: bool (ends a recording without saving it), relative?: bool (default true; when starting), name?, shortcut?: \"Ctrl+Shift+M\" | \"M\", description?} toggles recording; when starting, the name (new and valid) and the shortcut (free) are checked and kept for the save, and without a name the macro gets the next MacroN; when stopping, a name or description given then wins (a taken name gets a suffix); with relative references the selection when it starts is the example input → {recording, name?, relative?, input?} or {recording: false, saved, steps, dropped, skipped, inputs, shortcutDropped? (the shortcut another macro took while recording)}",
            has_doc,
            record
        ),
        cmd!(
            "macro.debug",
            "Step Into",
            [],
            None,
            "{action: start|step|continue|runTo|stop|state, name, inputs?, output?, overwrite?, approve?, timeLimit?: seconds per action (start, as macro.run), step: path (runTo), keep?: bool (stop), breakpoints?: [\"3\", \"2.1\"] (steps of the macro), breakpointsOnly?: bool (the list replaces the steps' own breakpoints)} debugs a macro one step at a time; stop keeps the changes as one undo step or rolls them back. Runs on its own, never as a step of a script or macro. Until it finishes or stops, its workbook runs only macro.debug, queries, selection and sheet changes, Go To, Find, Copy, zoom, Switch Window, Close and Stop Action (other workbooks stay editable) → {state: paused|finished|stopped|failed, current: {step, kind, command?, params?, iteration?}, lastParams, executed, changed (at most 500), changedCells, atBreakpoint?, shifted?, wroteFiles?, message?, error?}",
            has_doc,
            |s, p| crate::macros::debug::debug(s, p)
        ),
    ]
}

fn name_param<'a>(cmd: &str, p: &'a Json) -> Result<&'a str> {
    str_param(p, "name").filter(|n| !n.trim().is_empty()).ok_or_else(|| bad(cmd, "missing `name`"))
}

/// The index of macro `name` in the active workbook, or an error that says there is none.
fn index_of(s: &Session, name: &str) -> Result<usize> {
    crate::macros::find(&s.doc()?.wb, name).ok_or_else(|| EngineError::Other(format!("There is no macro called '{name}'.")))
}

fn list(s: &mut Session, _: &Json) -> Result<Json> {
    let wb = &s.doc()?.wb;
    Ok(Json::Array(
        wb.macros
            .iter()
            .map(|m| {
                let mut o = json!({
                    "name": m.name,
                    "description": m.description,
                    "steps": m.step_count(),
                    "inputs": m.inputs.iter().map(|i| i.name.clone()).collect::<Vec<_>>(),
                    "onConflict": m.output.on_conflict.name(),
                    "permissions": m.permissions,
                });
                if let Some(k) = &m.shortcut {
                    o["shortcut"] = json!(k);
                }
                o
            })
            .collect(),
    ))
}

fn get(s: &mut Session, p: &Json) -> Result<Json> {
    let i = index_of(s, name_param("macro.get", p)?)?;
    let wb = &s.doc()?.wb;
    Ok(wb.macros.get(i).map(crate::macros::macro_json).unwrap_or(Json::Null))
}

fn save(s: &mut Session, p: &Json) -> Result<Json> {
    let v = p.get("macro").filter(|m| m.is_object()).unwrap_or(p);
    let m = crate::macros::parse_macro(v).map_err(|e| bad("macro.save", e))?;
    // Replacing is asked for, never assumed: a save with a name in use is refused.
    let replace = match p.get("replace") {
        None | Some(Json::Null) => false,
        Some(Json::Bool(b)) => *b,
        Some(_) => return Err(bad("macro.save", "`replace` must be true or false")),
    };
    let (name, steps) = (m.name.clone(), m.step_count());
    edit(s, |cx| {
        if let Some(kept) = crate::macros::kept_name(&cx.wb, &m.name) {
            return Err(EngineError::Other(taken_by_kept(&kept)));
        }
        match crate::macros::find(&cx.wb, &m.name) {
            Some(i) if !replace => {
                let existing = cx.wb.macros.get(i).map_or(m.name.as_str(), |o| o.name.as_str());
                return Err(EngineError::Other(format!("A macro called '{existing}' already exists. Pass `replace: true` to replace it.")));
            }
            Some(i) => {
                if let Some(old) = cx.wb.macros.get_mut(i) {
                    *old = m;
                }
            }
            None => {
                crate::macros::check_room(&cx.wb).map_err(EngineError::Other)?;
                cx.wb.macros.push(m);
            }
        }
        Ok(())
    })?;
    Ok(json!({"name": name, "steps": steps}))
}

/// The refusal of a name that a macro kept in a newer format has.
fn taken_by_kept(name: &str) -> String {
    format!(
        "A macro called '{name}' made by a newer version of GridCraft is kept in this workbook; this version can't change it. Choose another name."
    )
}

fn delete(s: &mut Session, p: &Json) -> Result<Json> {
    let i = index_of(s, name_param("macro.delete", p)?)?;
    edit(s, |cx| {
        if i < cx.wb.macros.len() {
            cx.wb.macros.remove(i);
        }
        Ok(Json::Null)
    })
}

fn rename(s: &mut Session, p: &Json) -> Result<Json> {
    let i = index_of(s, name_param("macro.rename", p)?)?;
    let to = str_param(p, "to").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("macro.rename", "missing `to`"))?.to_string();
    if to.chars().any(char::is_control) {
        return Err(bad("macro.rename", "a macro name can't hold line breaks or other control characters"));
    }
    let wb = &s.doc()?.wb;
    if crate::macros::find(wb, &to).is_some_and(|j| j != i) {
        return Err(EngineError::Other(format!("A macro called '{to}' already exists.")));
    }
    if let Some(kept) = crate::macros::kept_name(wb, &to) {
        return Err(EngineError::Other(taken_by_kept(&kept)));
    }
    edit(s, |cx| {
        let Some(m) = cx.wb.macros.get_mut(i) else { return Ok(()) };
        let mut renamed = m.clone();
        renamed.name = to.clone();
        // The whole check, size included: a longer name can take a macro past what a file holds.
        crate::macros::check(&renamed).map_err(EngineError::Other)?;
        let from = std::mem::replace(m, renamed).name;
        // Buttons that run the macro follow the new name.
        let runs = |c: &gridcraft_model::FormControl| matches!(&c.action, Some(gridcraft_model::ControlAction::Macro { name, .. }) if name.eq_ignore_ascii_case(&from));
        for si in 0..cx.wb.sheets.len() {
            if !cx.wb.sheet(si).is_some_and(|sh| sh.controls.iter().any(runs)) {
                continue;
            }
            for c in cx.sheet_mut(si)?.controls.iter_mut() {
                if let Some(gridcraft_model::ControlAction::Macro { name, .. }) = &mut c.action
                    && name.eq_ignore_ascii_case(&from)
                {
                    *name = to.clone();
                }
            }
        }
        Ok(())
    })?;
    Ok(json!({"name": to}))
}

fn run(s: &mut Session, p: &Json) -> Result<Json> {
    let name = name_param("macro.run", p)?;
    // While recording: the steps a recording keeps for this run, taken from the macro as it runs,
    // so saving or deleting the macro before the recording stops doesn't change what it saved.
    let inline = if s.is_recording() {
        let wb = &s.doc()?.wb;
        crate::macros::find(wb, name).and_then(|i| wb.macros.get(i)).and_then(crate::macros::recorder::inline_steps)
    } else {
        None
    };
    let out = crate::macros::run::run(s, name, p)?;
    if s.is_recording() {
        // `None` when the macro's steps don't stand on their own: the recording leaves the run out.
        s.script_ran = inline;
    }
    Ok(out)
}

fn check(s: &mut Session, p: &Json) -> Result<Json> {
    let name = name_param("macro.check", p)?;
    crate::macros::run::check(s, name, p)
}

fn record(s: &mut Session, p: &Json) -> Result<Json> {
    check_record_params(s.is_recording(), p)?;
    // `cancel` ends a recording without saving it: the way out of a recording whose macro
    // cannot be saved (a name the check refuses, a full workbook).
    if bool_param(p, "cancel").unwrap_or(false) {
        let was = s.stop_recording().is_some();
        s.macro_recording = None;
        return Ok(json!({"recording": false, "cancelled": was}));
    }
    if bool_param(p, "stop").unwrap_or(false) || s.is_recording() {
        return stop_recording(s, p);
    }
    start_recording(s, p, bool_param(p, "relative").unwrap_or(true))
}

/// Refuses parameters `macro.record` would ignore (`null` counts as not given): `relative` and
/// `shortcut` when stopping (they are chosen when recording starts), `stop: false` while
/// recording (the call stops it), anything beside `cancel: true`, and parameters of the wrong
/// kind. `name` and `description` count when starting and when stopping (stopping wins).
fn check_record_params(recording: bool, p: &Json) -> Result<()> {
    let given = |k: &str| p.get(k).filter(|v| !v.is_null());
    for k in ["stop", "cancel", "relative"] {
        if given(k).is_some_and(|v| !v.is_boolean()) {
            return Err(bad("macro.record", format!("`{k}` must be true or false")));
        }
    }
    for k in ["name", "description", "shortcut"] {
        if given(k).is_some_and(|v| !v.is_string()) {
            return Err(bad("macro.record", format!("`{k}` must be text")));
        }
    }
    if given("cancel").and_then(Json::as_bool) == Some(true) {
        if let Some(k) = ["stop", "relative", "name", "description", "shortcut"].into_iter().find(|k| given(k).is_some()) {
            return Err(bad("macro.record", format!("`cancel: true` ends the recording without saving it, so `{k}` would be ignored")));
        }
        return Ok(());
    }
    let stop = given("stop").and_then(Json::as_bool);
    if recording && stop == Some(false) {
        return Err(bad(
            "macro.record",
            "a recording is in progress, and this call stops it: pass `stop: true`, or `cancel: true` to end it unsaved",
        ));
    }
    if (recording || stop == Some(true))
        && let Some(k) = ["relative", "shortcut"].into_iter().find(|k| given(k).is_some())
    {
        return Err(bad("macro.record", format!("`{k}` is chosen when recording starts")));
    }
    Ok(())
}

/// Starts recording. `name`, `shortcut` and `description` (from the Record Macro dialog) are
/// checked now, so a bad name or a taken shortcut is refused before anything is recorded.
fn start_recording(s: &mut Session, p: &Json, relative: bool) -> Result<Json> {
    let d = s.doc()?;
    let name = match str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => Some(crate::macros::check_new_name(&d.wb, n).map_err(|e| bad("macro.record", e))?),
        None => None,
    };
    let shortcut = match str_param(p, "shortcut").map(str::trim).filter(|k| !k.is_empty()) {
        Some(k) => {
            let letter = crate::macros::parse_shortcut(k)
                .ok_or_else(|| bad("macro.record", "`shortcut` must be one letter from A to Z (used as Ctrl+Shift+letter)"))?;
            if let Some(why) = crate::macros::shortcut_conflict(&d.wb, name.as_deref().unwrap_or(""), letter) {
                return Err(bad("macro.record", why));
            }
            Some(crate::macros::shortcut_text(letter))
        }
        None => None,
    };
    let description = match str_param(p, "description") {
        Some(t) if t.chars().count() > gridcraft_model::MAX_MACRO_TEXT => {
            return Err(bad("macro.record", format!("`description` can be at most {} characters long", gridcraft_model::MAX_MACRO_TEXT)));
        }
        Some(t) => Some(t.to_string()),
        None => None,
    };
    let mut start = RecordingStart::new(d, relative);
    (start.name, start.shortcut, start.description) = (name.clone(), shortcut, description);
    let input = start.selection.current().a1();
    s.start_recording();
    s.macro_recording = Some(start);
    let mut out = if relative { json!({"recording": true, "relative": true, "input": input}) } else { json!({"recording": true, "relative": false}) };
    if let Some(n) = name {
        out["name"] = json!(n);
    }
    Ok(out)
}

/// Stops recording and saves what was recorded as a macro, in the workbook where recording
/// started. When saving fails (a bad name, a full workbook, a macro being debugged there) the
/// recording goes on, so stopping again keeps what was recorded; when that workbook was closed,
/// the recording ends unsaved.
fn stop_recording(s: &mut Session, p: &Json) -> Result<Json> {
    let Some(rec) = s.stop_recording() else { return Ok(json!({"recording": false})) };
    let start = s.macro_recording.take();
    let at = match &start {
        Some(st) => s
            .docs
            .iter()
            .position(|d| d.uid == st.doc)
            .ok_or_else(|| EngineError::Other("The workbook where recording started was closed, so the recording was not saved.".into()))?,
        None => s.active_index(),
    };
    let saved = save_recording(s, p, &rec, start.clone(), at);
    if saved.is_err() {
        s.recorder = Some(rec);
        s.macro_recording = start;
    }
    saved
}

/// Saves the recording into the workbook at index `at`.
fn save_recording(s: &mut Session, p: &Json, rec: &crate::record::Recorder, start: Option<RecordingStart>, at: usize) -> Result<Json> {
    let d = s.docs.get(at).ok_or(EngineError::NoDocument)?;
    // A macro being debugged there could still roll the saved macro back with its run.
    if s.macro_debug.as_ref().is_some_and(|m| m.locks(d.uid)) {
        return Err(EngineError::Disabled("macro.record".into(), crate::macros::debug::BUSY.into()));
    }
    // A recording started without `macro.record` (`Session::start_recording`) takes its example
    // input from the workbook as it is now.
    let start = start.unwrap_or_else(|| RecordingStart::new(d, true));
    let (doc, wb) = (d.uid, d.wb.clone());
    // A name given when stopping wins over the one given when starting; either one, if another
    // macro has it, gets a suffix, so the save never replaces a macro.
    let name = match str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).or(start.name.as_deref()) {
        Some(n) => crate::macros::unique_name(&wb, n),
        None => crate::macros::next_name(&wb),
    };
    let mut converted = crate::macros::recorder::convert(&name, &start, &rec.steps, &rec.ran);
    // Steps past the macro limit or its size, and ones recorded in another workbook, are left
    // out and counted with the ones the recorder dropped.
    let mut cut = converted.cut.saturating_add(converted.mac.steps.len().saturating_sub(gridcraft_model::MAX_MACRO_STEPS));
    converted.mac.steps.truncate(gridcraft_model::MAX_MACRO_STEPS);
    if let Some(desc) = str_param(p, "description").or(start.description.as_deref()) {
        converted.mac.description = desc.chars().take(gridcraft_model::MAX_MACRO_TEXT).collect();
    }
    cut = cut.saturating_add(crate::macros::fit_size(&mut converted.mac));
    // The shortcut, unless another macro took it while recording; then it is left off and the
    // result says so.
    let mut shortcut_dropped = None;
    if let Some(k) = &start.shortcut {
        if crate::macros::parse_shortcut(k).is_some_and(|c| crate::macros::shortcut_conflict(&wb, &name, c).is_none()) {
            converted.mac.shortcut = Some(k.clone());
        } else {
            shortcut_dropped = Some(k.clone());
        }
    }
    let steps = converted.mac.steps.len();
    let inputs: Vec<String> = converted.mac.inputs.iter().map(|i| i.name.clone()).collect();
    // Saved into that workbook even when another one is active now.
    let active = s.active_index();
    s.set_active(at);
    let saved = s.execute("macro.save", json!({"macro": crate::macros::macro_json(&converted.mac), "replace": false}));
    s.set_active(active);
    saved?;
    // The person recorded it, so it runs without the approval a macro that came with a file
    // needs, until it changes.
    if let Some(d) = s.docs.iter().find(|d| d.uid == doc)
        && let Some(m) = crate::macros::find(&d.wb, &name).and_then(|i| d.wb.macros.get(i))
    {
        let key = (doc, crate::macros::fingerprint(m));
        s.macro_approvals.insert(key);
    }
    let mut out = json!({"recording": false, "saved": name, "steps": steps, "dropped": rec.dropped.saturating_add(cut), "skipped": converted.skipped, "inputs": inputs});
    if let Some(k) = shortcut_dropped {
        out["shortcutDropped"] = json!(k);
    }
    Ok(out)
}
