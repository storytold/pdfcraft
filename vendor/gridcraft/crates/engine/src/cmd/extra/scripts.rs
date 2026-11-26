use super::{EngineError, Json, Result, SCRIPTS_NAME, Session, Workbook, bad, bool_param, edit, json, stash_get, stash_map, stash_set, str_param};

pub(super) fn record(s: &mut Session, p: &Json) -> Result<Json> {
    if bool_param(p, "stop").unwrap_or(false) || s.recording.is_some() {
        let Some(start) = s.recording.take() else { return Ok(json!({"recording": false})) };
        let cmds: Vec<Json> = s
            .journal
            .iter()
            .skip(start)
            .filter(|(id, _)| !id.starts_with("automate.") && !crate::record::NOT_RECORDED.contains(&id.as_str()))
            .map(|(id, p)| json!({"command": id, "params": p}))
            .collect();
        let name = str_param(p, "name").map(str::to_string).unwrap_or_else(|| format!("Script {}", start));
        s.execute("automate.newScript", json!({"name": name, "commands": cmds}))?;
        return Ok(json!({"recording": false, "saved": name, "steps": cmds.len()}));
    }
    s.recording = Some(s.journal.len());
    Ok(json!({"recording": true}))
}

pub(super) fn new_script(s: &mut Session, p: &Json) -> Result<Json> {
    let name = str_param(p, "name").ok_or_else(|| bad("automate.newScript", "missing `name`"))?.to_string();
    let cmds = p.get("commands").cloned().unwrap_or(json!([]));
    if !cmds.is_array() {
        return Err(bad("automate.newScript", "`commands` must be an array"));
    }
    edit(s, |cx| {
        let mut all = stash_map(&cx.wb, SCRIPTS_NAME);
        all[name.as_str()] = cmds.clone();
        stash_set(&mut cx.wb, SCRIPTS_NAME, &all);
        Ok(json!({"name": name}))
    })
}

pub(super) fn all_scripts(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc()?;
    let (wb, trusted) = (&d.wb, d.scripts_trusted);
    let all = stash_get(wb, SCRIPTS_NAME);
    let mut list: Vec<Json> = all
        .as_object()
        .map(|o| o.iter().map(|(k, v)| json!({"name": k, "steps": v.as_array().map_or(0, Vec::len), "trusted": trusted})).collect())
        .unwrap_or_default();
    // A preserved VBA project is reported alongside GridCraft's own scripts; it is carried on
    // save but never executed here, so it has no step count.
    if let Some(bytes) = &wb.vba_project {
        list.push(json!({"name": "(VBA)", "steps": 0, "kind": "vba", "size": bytes.len(), "executed": false}));
    }
    Ok(Json::Array(list))
}

/// Whether `wb` carries GridCraft scripts (a workbook opened with them starts untrusted).
pub(crate) fn has_scripts(wb: &Workbook) -> bool {
    stash_map(wb, SCRIPTS_NAME).as_object().is_some_and(|o| !o.is_empty())
}

/// Why a script step is refused, whatever the workbook's trust: scripts stay inside the
/// workbook. Anything that reads or writes files (`file.*`, or any `path` parameter: Get Data,
/// From Text/CSV, Insert Picture) or leaves the app (`link.*` opens URLs) is the caller's to run
/// directly, not a stored script's. (Nested `automate.*` steps are skipped.) This check goes by
/// command id and parameter name, so it is only the up-front refusal: the steps also run inside
/// an [`crate::io::ScriptSandbox`] (file access refused whatever the parameter is called) and
/// [`run_script`] drops any `OpenUrl` request a step raises.
fn denied_script_step(id: &str, params: &Json) -> Option<String> {
    if id.starts_with("file.") || id.starts_with("link.") {
        return Some(format!("scripts can't run `{id}`: it reaches outside the workbook"));
    }
    // Scripts are scoped to their workbook: a step that switches the active window would
    // point every later step at another open document, so those are refused too.
    if id.starts_with("window.") || matches!(id, "view.switchWindows" | "view.newWindow" | "view.hideWindow" | "view.unhideWindow") {
        return Some(format!("scripts can't run `{id}`: it reaches outside the workbook"));
    }
    if params.get("path").is_some() {
        return Some(format!("scripts can't pass a file `path` to `{id}`"));
    }
    None
}

pub(super) fn trust_scripts(s: &mut Session, p: &Json) -> Result<Json> {
    // Only a missing `trusted` means "trust"; `"false"`, `0` or `null` are mistakes, not consent.
    let trusted = match p.get("trusted") {
        None => true,
        Some(Json::Bool(b)) => *b,
        Some(_) => return Err(bad("automate.trustScripts", "`trusted` must be true or false")),
    };
    s.doc_mut()?.scripts_trusted = trusted;
    Ok(json!({"trusted": trusted}))
}

/// Runs a stored script. Scripts in a workbook opened from a file run only once the workbook is
/// trusted for this session (`automate.trustScripts`, or `trusted: true` on this call); steps that
/// reach outside the workbook ([`denied_script_step`]) are refused before any step runs.
pub(super) fn run_script(s: &mut Session, p: &Json) -> Result<Json> {
    let name = str_param(p, "name").ok_or_else(|| bad("automate.runScript", "missing `name`"))?;
    let d = s.doc()?;
    let all = stash_get(&d.wb, SCRIPTS_NAME);
    let steps = all.get(name).and_then(Json::as_array).cloned().ok_or_else(|| bad("automate.runScript", "no such script"))?;
    if !d.scripts_trusted && !bool_param(p, "trusted").unwrap_or(false) {
        return Err(bad(
            "automate.runScript",
            "scripts in this workbook came from a file and are disabled; trust the workbook first (Trust Scripts in View › Macros, Alt+F8; `automate.trustScripts`; or `trusted: true`)",
        ));
    }
    let steps: Vec<Json> = steps.into_iter().take(100_000).collect();
    for st in &steps {
        let id = st["command"].as_str().unwrap_or("");
        if let Some(why) = denied_script_step(id, &st["params"]) {
            return Err(bad("automate.runScript", why));
        }
    }
    let _sandbox = crate::io::ScriptSandbox::enter();
    let mut n = 0;
    // What ran, for a recorder in progress: the recording keeps these steps with the run, so
    // editing or deleting the script before the recording stops does not change what it saved.
    let mut ran_steps: Vec<(String, Json)> = Vec::new();
    for st in &steps {
        let id = st["command"].as_str().unwrap_or("");
        if id.starts_with("automate.") {
            continue;
        }
        let before = s.ui_requests.len();
        let wb_before = s.doc().ok().map(|d| (d.uid, d.wb.clone()));
        let ran = s.execute(id, st["params"].clone());
        // A step that asks the host to open a URL leaves the app: drop the request and stop.
        if s.ui_requests.get(before..).is_some_and(|new| new.iter().any(|r| matches!(r, crate::UiRequest::OpenUrl(_)))) {
            let mut i = 0;
            s.ui_requests.retain(|r| {
                i += 1;
                i <= before || !matches!(r, crate::UiRequest::OpenUrl(_))
            });
            return Err(bad("automate.runScript", format!("scripts can't open links (`{id}`)")));
        }
        let result = ran?;
        n += 1;
        if s.recorder.is_some() {
            // The recorder's rule for each step, as for a command run on its own: a menu-style
            // step that only opened a dialog is left out (playing it back would stop at the
            // dialog), and a toggle keeps the state it set.
            let opened_dialog = s.ui_requests.get(before..).is_some_and(|new| new.iter().any(|r| matches!(r, crate::UiRequest::Dialog(..))));
            let changed = wb_before.as_ref().is_some_and(|(uid, wb)| s.docs.iter().any(|d| d.uid == *uid && !std::sync::Arc::ptr_eq(&d.wb, wb)));
            if !opened_dialog || changed {
                ran_steps.push((id.to_string(), crate::record::with_toggle_state(id, st["params"].clone(), &result)));
            }
        }
    }
    if s.recorder.is_some() {
        s.script_ran = Some(ran_steps);
    }
    Ok(json!({"ran": n}))
}

/// Redo work there again. A macro being debugged on that workbook is stopped with them.
pub(super) fn stop_action(s: &mut Session, _: &Json) -> Result<Json> {
    // Inside another command the open groups are that command's own (an action in progress):
    // a step of it that stopped them would pull the workbook from under the rest of its run.
    if s.depth() > 1 {
        return Err(EngineError::Disabled("automate.stopAction".into(), "Stop Action runs on its own, not as a step of a script or macro.".into()));
    }
    let uid = s.doc()?.uid;
    let mut out = json!({"stopped": s.abort_groups(uid)});
    // A macro being debugged here had its group among them: its run is over.
    if let Some((name, wrote_files)) = crate::macros::debug::end_stopped(s, uid) {
        out["macro"] = json!(name);
        out["message"] = json!(format!(
            "Macro '{name}', which was being debugged, was stopped and rolled back. {}",
            crate::macros::run::rolled_back(&wrote_files)
        ));
    }
    Ok(out)
}

pub(super) fn delete_script(s: &mut Session, p: &Json) -> Result<Json> {
    let name = str_param(p, "name").ok_or_else(|| bad("automate.deleteScript", "missing `name`"))?.to_string();
    edit(s, |cx| {
        let mut all = stash_map(&cx.wb, SCRIPTS_NAME);
        if let Some(o) = all.as_object_mut() {
            o.remove(&name);
        }
        stash_set(&mut cx.wb, SCRIPTS_NAME, &all);
        Ok(Json::Null)
    })
}
