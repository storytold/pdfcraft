//! The macro debugger: a run that stops between steps.
//!
//! `macro.debug` starts a run that pauses before its first step, then steps, continues to the next
//! breakpoint, runs to a chosen step, or stops. The run lives on the session between calls (one
//! action per call) inside an undo group that spans them; Stop keeps the changes as one undo step
//! or rolls them back. While a macro is being debugged its workbook is locked: only the
//! debugger, queries and the commands in [`LOOKS_AROUND`] run there, and everything else, saving
//! included, is refused ("Finish or stop the macro first"). Other workbooks stay editable. The
//! debugger runs on its own, never as a step of a script or macro, and is never recorded.

use std::collections::BTreeSet;
use std::sync::Arc;

use gridcraft_model::Workbook;
use serde_json::{Value as Json, json};

use super::run::{Machine, StepError, rolled_back};
use crate::{EngineError, GroupToken, Result, Selection, Session};

/// The most breakpoints one call sets.
pub const MAX_BREAKPOINTS: usize = 10_000;
/// The most entries `changed` lists (ranges and sheet notes), as the footprint lists ranges.
const MAX_LISTED: usize = 500;

/// Commands that run on the debugged workbook between steps, besides `macro.debug` and queries.
/// They only look around (the selection, the active sheet, Go To, Find, zoom, copying), switch to
/// another window, close the workbook (which ends the run) or stop the run (Stop Action). The
/// debugger puts the run's active sheet and selection back before its next step runs.
pub const LOOKS_AROUND: &[&str] = &[
    "selection.set",
    "selection.move",
    "selection.next",
    "selection.row",
    "selection.column",
    "selection.currentRegion",
    "edit.selectAll",
    "sheet.activate",
    "sheet.next",
    "sheet.previous",
    "edit.goTo",
    "edit.goToSpecial",
    "edit.find",
    "edit.copy",
    "view.zoom",
    "view.zoom100",
    "view.zoomToSelection",
    "window.activate",
    "file.close",
    "automate.stopAction",
];

/// Whether command `id` (a query when `query`) runs on a workbook the debugger locks.
pub fn runs_while_locked(id: &str, query: bool) -> bool {
    // `macro.check` is a query, but its dry run runs the macro's steps, which the lock refuses: it
    // waits too, rather than report the lock as the failure of a step.
    (query && id != "macro.check") || id == "macro.debug" || LOOKS_AROUND.contains(&id)
}

/// A macro being debugged.
pub struct Debugger {
    pub name: String,
    pub doc: u64,
    /// Out of the session only while a step runs.
    machine: Option<Machine>,
    token: Option<GroupToken>,
    /// Breakpoints set for this run (step paths), on top of the steps' own.
    pub breakpoints: BTreeSet<String>,
    /// Only `breakpoints` stop the run; the steps' own flags are ignored (an editor that shows
    /// its own breakpoints sends them all, so clearing one takes effect at once).
    pub only_listed: bool,
    /// The workbook when the run started, for "changed so far".
    start: Arc<Workbook>,
    /// Every step path in the macro, for checking `runTo` targets and breakpoints.
    paths: BTreeSet<String>,
    /// The workbooks the run has touched (by uid). Steps run on the macro's own workbook and the
    /// policy refuses window and file commands, so this is the macro's workbook unless a command
    /// reaches another one some other way; edits of these wait until the run ends.
    touched: BTreeSet<u64>,
    /// Steps the footprint's dry runs executed before the run (they count towards the run's
    /// budget, not towards `executed`).
    base: u64,
    /// The conflict rule that shifted the data at the start (`shiftDown`, `shiftRight`).
    shifted: Option<&'static str>,
    /// Where the run left its workbook after the last action.
    view: Option<View>,
}

/// The active sheet and the selections of the debugged workbook where the run left them.
#[derive(Clone)]
struct View {
    sheet: usize,
    selection: Selection,
    sheet_selections: Vec<Selection>,
}

impl Debugger {
    /// Whether edits of the workbook with this uid wait for the run to end.
    pub fn locks(&self, doc: u64) -> bool {
        self.touched.contains(&doc)
    }

    /// Whether one of the run's steps is running (its machine is out of the session).
    pub fn stepping(&self) -> bool {
        self.machine.is_none()
    }

    /// The run's own steps so far.
    fn executed(&self) -> u64 {
        self.machine.as_ref().map_or(0, |m| m.executed.saturating_sub(self.base))
    }
}

/// Each open workbook's uid and its current version, to see which ones a step changes.
fn versions(s: &Session) -> Vec<(u64, Arc<Workbook>)> {
    s.docs.iter().map(|d| (d.uid, d.wb.clone())).collect()
}

/// Adds to the run's touched log the workbooks that changed since `before`.
fn note_touched(s: &mut Session, before: &[(u64, Arc<Workbook>)]) {
    let changed: Vec<u64> =
        s.docs.iter().filter(|d| before.iter().any(|(uid, wb)| *uid == d.uid && !Arc::ptr_eq(wb, &d.wb))).map(|d| d.uid).collect();
    if let Some(dbg) = s.macro_debug.as_mut() {
        dbg.touched.extend(changed);
    }
}

/// Remembers where the run left its workbook: the active sheet and the selections.
fn remember_view(s: &mut Session) {
    let Some(uid) = s.macro_debug.as_ref().map(|d| d.doc) else { return };
    let view = s.docs.iter().find(|d| d.uid == uid).map(|d| View {
        sheet: d.wb.active_sheet(),
        selection: d.selection.clone(),
        sheet_selections: d.sheet_selections.clone(),
    });
    if let Some(dbg) = s.macro_debug.as_mut() {
        dbg.view = view;
    }
}

/// Puts back the active sheet and the selections where the run left them, so the next step runs
/// where `macro.run` would run it, however the person looked around meanwhile.
fn put_back_view(s: &mut Session) {
    let Some((uid, Some(view))) = s.macro_debug.as_ref().map(|d| (d.doc, d.view.clone())) else { return };
    let Some(d) = s.docs.iter_mut().find(|d| d.uid == uid) else { return };
    if view.sheet < d.wb.sheets.len() {
        // The active sheet is view state: setting it doesn't make the workbook dirty.
        d.wb.set_active_sheet(view.sheet);
    }
    d.selection = view.selection;
    d.sheet_selections = view.sheet_selections;
}

/// The message for edits refused while a macro is being debugged.
pub const BUSY: &str = "Finish or stop the macro first";

/// Ends a debugger whose undo group is no longer open (something other than the debugger ended
/// or rolled it back): its workbook is not locked any more. Returns the macro's name when it did.
pub(crate) fn drop_if_ended(s: &mut Session) -> Option<String> {
    let d = s.macro_debug.as_ref()?;
    if d.stepping() || d.token.as_ref().is_some_and(|t| s.group_is_open(t)) {
        return None;
    }
    s.macro_debug.take().map(|d| d.name)
}

/// After Stop Action rolled back the undo groups of the workbook with uid `doc`: ends the
/// debugger if its run was there. Returns the macro's name and the files the run wrote, which stay.
pub(crate) fn end_stopped(s: &mut Session, doc: u64) -> Option<(String, Vec<String>)> {
    if !s.macro_debug.as_ref().is_some_and(|d| d.doc == doc && !d.stepping()) {
        return None;
    }
    let d = s.macro_debug.take()?;
    Some((d.name, d.machine.map(|m| m.wrote_files).unwrap_or_default()))
}

/// Why an action finds the run already ended: its undo group was closed outside the debugger.
fn already_ended(name: &str) -> String {
    format!("Macro '{name}' was no longer being debugged: its undo group had been closed outside the debugger, so nothing was kept or rolled back.")
}

enum Mode {
    Step,
    Continue,
    RunTo(String),
}

const ACTIONS: [&str; 6] = ["start", "step", "continue", "runTo", "stop", "state"];

/// `macro.debug {name?, inputs?, action: start|step|continue|runTo|stop|state, step?, keep?,
/// breakpoints?, breakpointsOnly?}`.
pub fn debug(s: &mut Session, p: &Json) -> Result<Json> {
    // A step of a script or macro that started the debugger would leave it behind when the
    // script fails and rolls back its groups, the debugger's among them.
    if s.depth() > 1 {
        return Err(EngineError::Disabled("macro.debug".into(), "The macro debugger runs on its own, not as a step of a script or macro.".into()));
    }
    let ended = drop_if_ended(s);
    let action = match p.get("action") {
        None | Some(Json::Null) if s.macro_debug.is_some() || ended.is_some() => "state".to_string(),
        None | Some(Json::Null) => "start".to_string(),
        Some(Json::String(a)) => a.clone(),
        // `{"action": {"runTo": {"step": "3"}}}`
        Some(Json::Object(o)) if o.len() == 1 => o.keys().next().cloned().unwrap_or_default(),
        Some(_) => return Err(bad("`action` must be start, step, continue, runTo, stop or state")),
    };
    if !ACTIONS.contains(&action.as_str()) {
        return Err(bad(format!("action must be start, step, continue, runTo, stop or state, not '{action}'")));
    }
    if let Some(name) = ended
        && action != "start"
    {
        return Err(EngineError::Other(already_ended(&name)));
    }
    let only = bool_param(p, "breakpointsOnly")?;
    if action == "start" {
        return start(s, p, only.unwrap_or(false));
    }
    let keep = bool_param(p, "keep")?;
    let d = s.macro_debug.as_ref().ok_or_else(not_debugging)?;
    let target = if action == "runTo" {
        let target = p
            .get("step")
            .or_else(|| p.pointer("/action/runTo/step"))
            .and_then(path_of)
            .ok_or_else(|| bad("`runTo` needs `step`: the path of the step to stop at"))?;
        if !d.paths.contains(&target) {
            return Err(bad(format!("macro '{}' has no step {target}", d.name)));
        }
        Some(target)
    } else {
        None
    };
    let breakpoints = p.get("breakpoints").filter(|v| !v.is_null()).map(|v| paths_param(v, &d.paths, &d.name)).transpose()?;
    if let Some(d) = s.macro_debug.as_mut() {
        if let Some(bps) = breakpoints {
            d.breakpoints = bps;
        }
        if let Some(only) = only {
            d.only_listed = only;
        }
    }
    match (action.as_str(), target) {
        ("step", _) => advance(s, Mode::Step),
        ("continue", _) => advance(s, Mode::Continue),
        ("runTo", Some(target)) => advance(s, Mode::RunTo(target)),
        ("stop", _) => stop(s, keep.unwrap_or(false)),
        _ => state(s),
    }
}

fn bad(msg: impl Into<String>) -> EngineError {
    crate::cmd::bad("macro.debug", msg)
}

fn not_debugging() -> EngineError {
    EngineError::Other("No macro is being debugged. Start one with action: start.".into())
}

/// `true`, `false`, or not given (`null` counts as not given); anything else is refused.
fn bool_param(p: &Json, key: &str) -> Result<Option<bool>> {
    match p.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad(format!("`{key}` must be true or false"))),
    }
}

/// A step path from JSON: `"2.1"` or `3`.
fn path_of(v: &Json) -> Option<String> {
    match v {
        Json::String(t) if !t.trim().is_empty() => Some(t.trim().to_string()),
        Json::Number(n) => n.as_u64().map(|n| n.to_string()),
        _ => None,
    }
}

/// The `breakpoints` list: step paths of macro `name` (one of `paths`), at most
/// [`MAX_BREAKPOINTS`].
fn paths_param(v: &Json, paths: &BTreeSet<String>, name: &str) -> Result<BTreeSet<String>> {
    let list = v.as_array().ok_or_else(|| bad("`breakpoints` must be a list of step paths"))?;
    if list.len() > MAX_BREAKPOINTS {
        return Err(bad(format!("`breakpoints` lists {} steps; at most {MAX_BREAKPOINTS} can be set", list.len())));
    }
    let mut out = BTreeSet::new();
    for x in list {
        let path = path_of(x).ok_or_else(|| bad("a breakpoint is a step path such as \"3\" or \"2.1\""))?;
        if !paths.contains(&path) {
            return Err(bad(format!("macro '{name}' has no step {path} for a breakpoint")));
        }
        out.insert(path);
    }
    Ok(out)
}

fn start(s: &mut Session, p: &Json, only_listed: bool) -> Result<Json> {
    if let Some(d) = &s.macro_debug {
        return Err(EngineError::Other(format!("Macro '{}' is being debugged. Stop it first.", d.name)));
    }
    let name = p.get("name").and_then(Json::as_str).filter(|n| !n.trim().is_empty()).ok_or_else(|| bad("missing `name`"))?;
    let b = super::run::begin(s, name, p)?;
    let paths = all_paths(&b.mac.steps, "");
    let breakpoints = match p.get("breakpoints").filter(|v| !v.is_null()) {
        Some(v) => paths_param(v, &paths, &b.mac.name)?,
        None => BTreeSet::new(),
    };
    let start_wb = s.doc()?.wb.clone();
    let token = s.group_begin(&format!("Run Macro: {}", b.mac.name))?;
    // The run's machine: unless the rule is overwrite, it checks each step for writes over data.
    let (mut machine, shifted) = match super::run::make_way(s, &b) {
        Ok(made) => made,
        Err(e) => {
            let _ = s.group_rollback(token);
            return Err(e);
        }
    };
    // Each action has the time limit to itself; the dry runs just had theirs.
    machine.restart_clock();
    let base = machine.executed;
    s.macro_debug = Some(Debugger {
        name: b.mac.name.clone(),
        doc: b.prepared.doc,
        machine: Some(machine),
        token: Some(token),
        breakpoints,
        only_listed,
        start: start_wb,
        paths,
        touched: BTreeSet::from([b.prepared.doc]),
        base,
        shifted: shifted.then(|| b.rule.name()),
        view: None,
    });
    if s.macro_debug.as_ref().and_then(|d| d.machine.as_ref()).is_some_and(Machine::is_done) {
        return finish(s, None);
    }
    remember_view(s);
    state(s)
}

fn all_paths(steps: &[gridcraft_model::Step], prefix: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (i, st) in steps.iter().enumerate() {
        let path = if prefix.is_empty() { (i + 1).to_string() } else { format!("{prefix}.{}", i + 1) };
        match &st.kind {
            gridcraft_model::StepKind::Repeat { body, .. } => out.extend(all_paths(body, &path)),
            gridcraft_model::StepKind::If { then, otherwise, .. } => {
                out.extend(all_paths(then, &format!("{path}.then")));
                out.extend(all_paths(otherwise, &format!("{path}.else")));
            }
            _ => {}
        }
        out.insert(path);
    }
    out
}

/// Runs one step (`Step`), or steps until a breakpoint, the target step or the end.
fn advance(s: &mut Session, mode: Mode) -> Result<Json> {
    put_back_view(s);
    let d = s.macro_debug.as_mut().ok_or_else(not_debugging)?;
    let (name, base) = (d.name.clone(), d.base);
    let breakpoints = d.breakpoints.clone();
    let only_listed = d.only_listed;
    let mut machine = d.machine.take().ok_or_else(not_debugging)?;
    machine.restart_clock();
    let before = versions(s);
    let mut first = true;
    let mut hit = false;
    let mut outcome = Ok(());
    while !machine.is_done() {
        if !first {
            let Some((path, st)) = machine.current() else { break };
            if matches!(mode, Mode::Step) {
                break;
            }
            if (st.breakpoint && !only_listed) || breakpoints.contains(&path) {
                hit = true;
                break;
            }
            if let Mode::RunTo(target) = &mode
                && *target == path
            {
                break;
            }
        }
        first = false;
        outcome = machine.step(s);
        if outcome.is_err() {
            break;
        }
    }
    let done = machine.is_done();
    match s.macro_debug.as_mut() {
        Some(d) => d.machine = Some(machine),
        // The macro.debug call panicked and `recover` has already rolled the run back.
        None => {
            return match outcome {
                Err(e) => Ok(failed(&name, machine.executed.saturating_sub(base), &e)),
                Ok(()) => Err(not_debugging()),
            };
        }
    }
    note_touched(s, &before);
    if let Err(e) = outcome {
        return finish(s, Some(e));
    }
    if done {
        return finish(s, None);
    }
    remember_view(s);
    let mut out = state(s)?;
    out["atBreakpoint"] = json!(hit);
    Ok(out)
}

/// What every answer about the run's end or its state adds: the data shift at the start and the
/// files the run wrote (they stay, whatever happens to the workbook).
fn add_notes(out: &mut Json, d: &Debugger, wrote_files: &[String]) {
    if let Some(rule) = d.shifted {
        out["shifted"] = json!(rule);
    }
    if !wrote_files.is_empty() {
        out["wroteFiles"] = json!(wrote_files);
    }
}

/// Ends the run after its last step (kept as one undo step, unless a Stop step asked for a
/// rollback) or a failed step (rolled back).
fn finish(s: &mut Session, error: Option<StepError>) -> Result<Json> {
    let Some(mut d) = s.macro_debug.take() else { return Err(not_debugging()) };
    let (changed, cells) = changed(s, &d);
    let executed = d.executed();
    let machine = d.machine.take();
    let stopped = machine.as_ref().and_then(|m| m.stopped.clone());
    let wrote = machine.map(|m| m.wrote_files).unwrap_or_default();
    let rollback = error.is_some() || matches!(stopped, Some((_, true)));
    let ended = match d.token.take() {
        Some(token) if rollback => s.group_rollback(token).is_ok(),
        Some(token) => s.group_end(token).is_ok(),
        None => false,
    };
    if !ended {
        let step = error.map(|e| format!("{} ", e.describe())).unwrap_or_default();
        return Err(EngineError::Other(format!("{step}{}", already_ended(&d.name))));
    }
    if let Some(e) = &error {
        return Ok(failed(&d.name, executed, e));
    }
    let mut out = json!({"macro": d.name, "executed": executed});
    match &stopped {
        Some((msg, rolled)) => {
            out["state"] = json!("stopped");
            out["stopped"] = json!(msg);
            out["kept"] = json!(!rolled);
            if *rolled {
                out["message"] = json!(rolled_back(&wrote));
            }
        }
        None => out["state"] = json!("finished"),
    }
    if !rollback {
        out["changed"] = json!(changed);
        out["changedCells"] = json!(cells);
    }
    add_notes(&mut out, &d, &wrote);
    Ok(out)
}

/// The result of a run that a failed step rolled back.
fn failed(name: &str, executed: u64, e: &StepError) -> Json {
    let mut out = json!({
        "macro": name,
        "executed": executed,
        "state": "failed",
        "error": serde_json::to_value(e).unwrap_or(Json::Null),
        "message": format!("{} {}", e.describe(), rolled_back(&e.wrote_files)),
    });
    if !e.wrote_files.is_empty() {
        out["wroteFiles"] = json!(e.wrote_files);
    }
    out
}

/// Stops the run: keeps what it did as one undo step, or rolls it back.
fn stop(s: &mut Session, keep: bool) -> Result<Json> {
    let Some(mut d) = s.macro_debug.take() else { return Err(not_debugging()) };
    let (changed, cells) = changed(s, &d);
    let ended = match d.token.take() {
        Some(token) if keep => s.group_end(token).is_ok(),
        Some(token) => s.group_rollback(token).is_ok(),
        None => false,
    };
    if !ended {
        return Err(EngineError::Other(already_ended(&d.name)));
    }
    let wrote = d.machine.as_ref().map(|m| m.wrote_files.clone()).unwrap_or_default();
    let mut out = json!({"state": "stopped", "macro": d.name, "kept": keep, "executed": d.executed()});
    if keep {
        out["changed"] = json!(changed);
        out["changedCells"] = json!(cells);
    } else {
        out["message"] = json!(rolled_back(&wrote));
    }
    add_notes(&mut out, &d, &wrote);
    Ok(out)
}

/// Where the run is: the next step with its parameters as they would run, the step that just
/// ran, and what the run has changed in its workbook so far.
fn state(s: &mut Session) -> Result<Json> {
    let d = s.macro_debug.as_ref().ok_or_else(not_debugging)?;
    let machine = d.machine.as_ref().ok_or_else(not_debugging)?;
    let wb = s.docs.iter().find(|x| x.uid == d.doc).map(|x| x.wb.clone()).ok_or(EngineError::NoDocument)?;
    let (changed, cells) = changed(s, d);
    let mut out = json!({
        "state": "paused",
        "macro": d.name,
        "current": machine.current_json(&wb),
        "lastParams": machine.last_params,
        "executed": d.executed(),
        "breakpoints": d.breakpoints,
        "changed": changed,
        "changedCells": cells,
    });
    add_notes(&mut out, d, &machine.wrote_files);
    Ok(out)
}

/// What the run changed in its workbook since it started: cells as `Sheet!A1:B2` ranges and the
/// sheets it added, deleted or renamed (at most [`MAX_LISTED`] entries), and how many cells
/// changed on the sheets the workbook had at the start.
fn changed(s: &Session, d: &Debugger) -> (Vec<String>, usize) {
    let Some(now) = s.docs.iter().find(|x| x.uid == d.doc).map(|x| x.wb.clone()) else { return (vec![], 0) };
    let (pairs, added) = super::run::pair_sheets(&d.start, &now);
    let mut out = Vec::new();
    let mut count = 0usize;
    for (bi, ai) in pairs {
        let Some(before) = d.start.sheets.get(bi) else { continue };
        let Some(after) = ai.and_then(|a| now.sheets.get(a)) else {
            out.push(format!("{} (deleted sheet)", before.name));
            continue;
        };
        if before.name != after.name {
            out.push(format!("{} (renamed from {})", after.name, before.name));
        }
        if Arc::ptr_eq(before, after) {
            continue;
        }
        let mut cells = before.cells.diff(&after.cells);
        count = count.saturating_add(cells.len());
        if out.len() < MAX_LISTED {
            let q = super::resolve::quote_sheet(&after.name);
            out.extend(super::run::ranges(&mut cells).into_iter().map(|r| format!("{q}!{r}")));
        }
    }
    out.extend(added.into_iter().filter_map(|a| now.sheets.get(a)).map(|sh| format!("{} (new sheet)", sh.name)));
    out.truncate(MAX_LISTED);
    (out, count)
}

/// After the `macro.debug` call itself panicked: if the run lost its place, roll it back so its
/// undo group doesn't stay open.
pub(crate) fn recover(s: &mut Session) {
    if s.macro_debug.as_ref().is_some_and(Debugger::stepping)
        && let Some(mut d) = s.macro_debug.take()
        && let Some(token) = d.token.take()
    {
        let _ = s.group_rollback(token);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    #[test]
    fn a_run_that_lost_its_place_is_rolled_back() {
        let mut s = Session::new();
        s.new_workbook();
        let m = crate::macros::parse_macro(&json!({"name": "M", "output": {"onConflict": "overwrite"}, "steps": [
            {"command": "cell.set", "params": {"cell": "A1", "input": "x"}},
            {"command": "cell.set", "params": {"cell": "A2", "input": "y"}}
        ]}))
        .unwrap();
        let d = s.doc_mut().unwrap();
        let mut wb = (*d.wb).clone();
        wb.macros.push(m);
        d.wb = std::sync::Arc::new(wb);
        let start = s.doc().unwrap().wb.clone();
        s.execute("macro.debug", json!({"name": "M"})).unwrap();
        s.execute("macro.debug", json!({"action": "step"})).unwrap();
        // As if a step had panicked with the machine out of the session.
        s.macro_debug.as_mut().unwrap().machine = None;
        super::recover(&mut s);
        assert!(s.macro_debug.is_none());
        assert!(std::sync::Arc::ptr_eq(&s.doc().unwrap().wb, &start));
        assert!(!s.in_group(s.doc().unwrap().uid));
        s.execute("cell.set", json!({"cell": "B1", "input": "free"})).unwrap();
    }

    #[test]
    fn a_step_that_panics_reports_the_step_failure() {
        crate::register_test_command("test.debugStepPanics", |_, _| panic!("boom"));
        let mut s = Session::new();
        s.new_workbook();
        let m = crate::macros::parse_macro(&json!({"name": "M", "output": {"onConflict": "overwrite"}, "steps": [
            {"command": "cell.set", "params": {"cell": "A1", "input": "x"}},
            {"command": "test.debugStepPanics", "params": {}}
        ]}))
        .unwrap();
        let d = s.doc_mut().unwrap();
        let mut wb = (*d.wb).clone();
        wb.macros.push(m);
        d.wb = std::sync::Arc::new(wb);
        let start = s.doc().unwrap().wb.clone();
        s.execute("macro.debug", json!({"name": "M"})).unwrap();
        s.execute("macro.debug", json!({"action": "step"})).unwrap();
        let r = s.execute("macro.debug", json!({"action": "step"})).unwrap();
        assert_eq!(r["state"], "failed", "{r}");
        assert_eq!(r["macro"], "M");
        assert_eq!(r["error"]["step"], "2");
        assert!(r["message"].as_str().unwrap().contains("boom"), "{r}");
        assert!(s.macro_debug.is_none());
        assert!(std::sync::Arc::ptr_eq(&s.doc().unwrap().wb, &start));
        assert!(!s.in_group(s.doc().unwrap().uid));
    }

    #[test]
    fn a_workbook_in_the_touched_log_waits_until_the_run_ends() {
        let mut s = Session::new();
        s.new_workbook();
        let m = crate::macros::parse_macro(&json!({"name": "M", "output": {"onConflict": "overwrite"}, "steps": [
            {"command": "cell.set", "params": {"cell": "A1", "input": "x"}},
            {"command": "cell.set", "params": {"cell": "A2", "input": "y"}}
        ]}))
        .unwrap();
        let d = s.doc_mut().unwrap();
        let mut wb = (*d.wb).clone();
        wb.macros.push(m);
        d.wb = std::sync::Arc::new(wb);
        s.new_workbook();
        let other = s.doc().unwrap().uid;
        s.set_active(0);
        s.execute("macro.debug", json!({"name": "M"})).unwrap();
        assert!(!s.macro_debug.as_ref().unwrap().locks(other));
        // As if a step had changed the other workbook too.
        let before = super::versions(&s);
        let d = s.docs.iter_mut().find(|d| d.uid == other).unwrap();
        let mut wb = (*d.wb).clone();
        wb.props.title = "changed by the macro".into();
        d.wb = std::sync::Arc::new(wb);
        super::note_touched(&mut s, &before);
        assert!(s.macro_debug.as_ref().unwrap().locks(other));
        s.set_active(1);
        assert!(s.execute("cell.set", json!({"cell": "B1", "input": "no"})).is_err());
        s.execute("macro.debug", json!({"action": "stop", "keep": true})).unwrap();
        s.execute("cell.set", json!({"cell": "B1", "input": "free"})).unwrap();
    }

    #[test]
    fn edits_run_by_another_command_wait_too() {
        let mut s = Session::new();
        s.new_workbook();
        let m = crate::macros::parse_macro(&json!({"name": "M", "output": {"onConflict": "overwrite"}, "steps": [
            {"command": "cell.set", "params": {"cell": "A1", "input": "x"}},
            {"command": "cell.set", "params": {"cell": "A2", "input": "y"}}
        ]}))
        .unwrap();
        let d = s.doc_mut().unwrap();
        let mut wb = (*d.wb).clone();
        wb.macros.push(m);
        d.wb = std::sync::Arc::new(wb);
        s.execute("automate.recordActions", json!({})).unwrap();
        s.execute("cell.set", json!({"cell": "C1", "input": "1"})).unwrap();
        s.execute("macro.debug", json!({"name": "M"})).unwrap();
        s.execute("macro.debug", json!({"action": "step"})).unwrap();
        // Record Actions changes nothing itself; stopping it saves the script with a nested
        // command, which would land in the run's undo group.
        let e = s.execute("automate.recordActions", json!({"stop": true, "name": "S"})).unwrap_err().to_string();
        assert!(e.contains(super::BUSY), "{e}");
        assert!(s.execute("automate.allScripts", json!({})).unwrap().as_array().unwrap().is_empty());
        // The run's own steps still go ahead.
        let r = s.execute("macro.debug", json!({"action": "step"})).unwrap();
        assert_eq!(r["executed"], 2, "{r}");
        assert!(s.macro_debug.is_none());
        // The recording was kept: stopping now saves it.
        let saved = s.execute("automate.recordActions", json!({"stop": true, "name": "S"})).unwrap();
        assert_eq!(saved["saved"], "S", "{saved}");
        let scripts = s.execute("automate.allScripts", json!({})).unwrap();
        assert_eq!(scripts[0]["name"], "S", "{scripts}");
    }
}
