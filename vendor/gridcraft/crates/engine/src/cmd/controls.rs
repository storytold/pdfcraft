//! Form controls (Insert › Controls): buttons that run a macro or a command, and sliders and spin
//! buttons that hold a whole number from 0 to 30000 and write it to a linked cell.
//!
//! A slider or spin button's value lives on the control and in its linked cell. Setting it goes
//! through `cell.set`, so the cell's dependents recalculate and Undo takes both back as one step;
//! editing the linked cell moves the control (see [`sync_linked_values`]). Pressing a button runs
//! its action through the macro policy with the document's trust, so a button from a file someone
//! else made asks before it runs anything.

use gridcraft_core::CellRef;
use gridcraft_model::{
    Anchor, AnchorMode, CONTROL_MAX, ControlAction, ControlKind, FormControl, MAX_MACRO_NAME, MacroInput, ObjKind, Sheet, Workbook,
};
use serde_json::{Value as Json, json};

use super::{
    CommandSpec, EngineError, PROTECTED, Result, Session, anchor_mode_param, bad, bool_param, cell_param, cmd, edit, f64_param, find_command,
    has_doc, split_sheet, stash, str_param, u32_param,
};
use crate::macros::policy::{Policy, macro_policy};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "insert.button",
            "Button",
            ["Insert", "Controls"],
            None,
            "{at?: \"B2\" (or cell), dx?, dy?, width?, height?, text?, macro?: name, input?: \"selection\" | \"B2:D9\", output?: \"F2\", command?: id, params?: {..}} places a button; pressing it runs the macro or the command → {control: id}",
            has_doc,
            insert_button
        ),
        cmd!(
            "insert.slider",
            "Slider",
            ["Insert", "Controls"],
            None,
            "{at?: \"B2\" (or cell), dx?, dy?, width?, height?, link?: \"$B$1\" | \"Sheet2!$B$1\", min?: 0, max?: 100, step?: 1, page?: 10, value?, horizontal?: true} whole numbers, 0 ≤ min < max ≤ 30000, 1 ≤ step ≤ max − min, page ≥ step → {control: id}",
            has_doc,
            |s, p| insert_number_control(s, p, ControlKind::Slider)
        ),
        cmd!(
            "insert.spinButton",
            "Spin Button",
            ["Insert", "Controls"],
            None,
            "{at?: \"B2\" (or cell), dx?, dy?, width?, height?, link?: \"$B$1\", min?: 0, max?: 100, step?: 1, value?} whole numbers, 0 ≤ min < max ≤ 30000, 1 ≤ step ≤ max − min → {control: id}",
            has_doc,
            |s, p| insert_number_control(s, p, ControlKind::Spin)
        ),
        cmd!(
            "control.format",
            "Format Control",
            [],
            None,
            "{id, text?, name?, link?: \"$B$1\" | \"\" (no link), min?, max?, step?, page?, value?, horizontal?, print?, locked?} → the control (as control.list)",
            has_doc,
            format
        ),
        cmd!(
            "control.assign",
            "Assign Action",
            [],
            None,
            "{id, macro?: name, input?: \"selection\" | \"B2:D9\", output?: \"F2\" | command?: id, params?: {..} | clear?: true} sets what a button runs → the control",
            has_doc,
            assign
        ),
        cmd!(
            "control.setValue",
            "Change Control Value",
            [],
            None,
            "{id, value} clamps the value to min..max, snaps it to the step, and writes it to the linked cell → {value}",
            has_doc,
            set_value
        ),
        cmd!(
            "control.press",
            "Press Button",
            [],
            None,
            "{id, approve?: bool, onConflict?, overwrite?} runs a button's macro or command (through the macro policy, with the workbook's trust; a workbook from a file asks first) → what the macro or command returned",
            has_doc,
            press
        ),
        cmd!(query "control.list", "Form Controls", [], None, "{} → the active sheet's controls: [{id, kind, name, text, at, link?, value, min, max, step, page, horizontal, action?}]", has_doc, list),
    ]
}

/// The longest caption or name a control takes.
const MAX_TEXT: usize = 1_024;

fn control_json(c: &FormControl) -> Json {
    let mut o = json!({
        "id": c.id,
        "kind": match c.kind { ControlKind::Button => "button", ControlKind::Slider => "slider", ControlKind::Spin => "spinButton" },
        "name": c.display_name(),
        "at": c.anchor.cell.a1(),
        "width": c.anchor.width,
        "height": c.anchor.height,
        "print": c.print,
        "locked": c.locked,
    });
    if c.kind == ControlKind::Button {
        o["text"] = json!(c.text);
        if let Some(a) = &c.action {
            o["action"] = serde_json::to_value(a).unwrap_or(Json::Null);
        }
    } else {
        o["value"] = json!(c.value);
        o["min"] = json!(c.min);
        o["max"] = json!(c.max);
        o["step"] = json!(c.step);
        if c.kind == ControlKind::Slider {
            o["page"] = json!(c.page);
            o["horizontal"] = json!(c.horizontal);
        }
    }
    if let Some(l) = &c.link {
        o["link"] = json!(l);
    }
    o
}

fn list(s: &mut Session, _: &Json) -> Result<Json> {
    let sh = s.doc()?.wb.active().ok_or(EngineError::NoDocument)?;
    Ok(Json::Array(sh.controls.iter().map(control_json).collect()))
}

/// A whole number from 0 to 30000 named `key`, if given. Numbers in text (`"12"`) count; anything
/// else is refused with a message that names the parameter.
fn whole(cmd: &str, p: &Json, key: &str) -> Result<Option<u32>> {
    let v = match p.get(key) {
        None | Some(Json::Null) => return Ok(None),
        Some(Json::Number(n)) => n.as_f64(),
        Some(Json::String(t)) => t.trim().parse::<f64>().ok(),
        Some(_) => None,
    };
    match v {
        Some(x) if x.is_finite() && x.fract() == 0.0 && (0.0..=CONTROL_MAX as f64).contains(&x) => Ok(Some(x as u32)),
        _ => Err(bad(cmd, format!("`{key}` must be a whole number from 0 to {CONTROL_MAX}"))),
    }
}

fn text_param(cmd: &str, p: &Json, key: &str) -> Result<Option<String>> {
    match p.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(t)) if t.chars().count() <= MAX_TEXT => Ok(Some(t.clone())),
        Some(Json::String(_)) => Err(bad(cmd, format!("`{key}` is longer than {MAX_TEXT} characters"))),
        Some(_) => Err(bad(cmd, format!("`{key}` must be text"))),
    }
}

/// The cell a link names, from the control's sheet `host`: `(sheet index, cell)`. `None` when it
/// doesn't name one cell of an existing sheet (a link into a deleted sheet reads `#REF!`).
pub fn link_target(wb: &Workbook, host: usize, link: &str) -> Option<(usize, CellRef)> {
    let text = link.trim().trim_start_matches('=');
    let (sheet, body) = split_sheet(text);
    let si = match sheet {
        Some(n) => wb.sheet_index(&n)?,
        None => host,
    };
    wb.sheet(si)?;
    Some((si, CellRef::parse(body.trim())?))
}

/// A link parameter in its stored form: `$B$2` on the host sheet, `'My Sheet'!$B$2` elsewhere.
/// An empty text removes the link.
fn link_param(cmd: &str, wb: &Workbook, host: usize, p: &Json) -> Result<Option<Option<String>>> {
    let text = match p.get("link") {
        None => return Ok(None),
        Some(Json::Null) => return Ok(Some(None)),
        Some(Json::String(t)) => t.trim(),
        Some(_) => return Err(bad(cmd, "`link` must be a cell reference such as $B$1")),
    };
    if text.is_empty() {
        return Ok(Some(None));
    }
    let (si, c) = link_target(wb, host, text)
        .ok_or_else(|| bad(cmd, format!("`{text}` isn't a cell of this workbook; the link must be one cell, such as $B$1")))?;
    let abs = format!("${}${}", gridcraft_core::col_to_letters(c.col), c.row as u64 + 1);
    Ok(Some(Some(if si == host {
        abs
    } else {
        let name = wb.sheet(si).map(|sh| sh.name.clone()).unwrap_or_default();
        format!("{}!{abs}", gridcraft_formula::quote_sheet(&name))
    })))
}

/// The anchor of a new control: `at` or `cell` (default the active cell), with its size.
fn new_anchor(s: &Session, p: &Json, size: (f32, f32)) -> Result<Anchor> {
    let cell = match cell_param(s, p, "at")? {
        Some(c) => c,
        None => cell_param(s, p, "cell")?.unwrap_or(s.doc()?.selection.active),
    };
    let dim = |k: &str, v: f32| f64_param(p, k).filter(|x| x.is_finite()).map_or(v, |x| x.clamp(4.0, 10_000.0) as f32);
    let off = |k: &str| f64_param(p, k).filter(|x| x.is_finite()).map_or(0.0, |x| x.clamp(-10_000.0, 10_000.0) as f32);
    Ok(Anchor {
        cell,
        dx: off("dx"),
        dy: off("dy"),
        width: dim("width", size.0),
        height: dim("height", size.1),
        mode: anchor_mode_param(p).unwrap_or(AnchorMode::MoveOnly),
    })
}

fn insert_button(s: &mut Session, p: &Json) -> Result<Json> {
    const CMD: &str = "insert.button";
    let anchor = new_anchor(s, p, (96.0, 28.0))?;
    let action = action_param(s, CMD, p)?;
    let d = s.doc()?;
    let id = d.wb.next_object_id();
    let text = text_param(CMD, p, "text")?.unwrap_or_else(|| format!("Button {id}"));
    let sheet = d.wb.active_sheet();
    let control = FormControl { id, kind: ControlKind::Button, anchor, text, action, ..FormControl::default() };
    edit(s, |cx| {
        cx.sheet_mut(sheet)?.controls.push(control);
        Ok(json!({"control": id}))
    })
}

fn insert_number_control(s: &mut Session, p: &Json, kind: ControlKind) -> Result<Json> {
    let cmd = if kind == ControlKind::Slider { "insert.slider" } else { "insert.spinButton" };
    let horizontal = bool_param(p, "horizontal").unwrap_or(true);
    let size = match kind {
        ControlKind::Slider if horizontal => (144.0, 18.0),
        ControlKind::Slider => (18.0, 144.0),
        _ => (18.0, 36.0),
    };
    let anchor = new_anchor(s, p, size)?;
    let d = s.doc()?;
    let sheet = d.wb.active_sheet();
    let id = d.wb.next_object_id();
    let mut c = FormControl { id, kind, anchor, horizontal, ..FormControl::default() };
    numbers(cmd, p, &mut c)?;
    if let Some(link) = link_param(cmd, &d.wb, sheet, p)? {
        c.link = link;
    }
    let given = whole(cmd, p, "value")?;
    // Without a value, a control starts at the number its linked cell holds (or its minimum).
    let start = given.map(f64::from).or_else(|| linked_number(&d.wb, sheet, &c)).unwrap_or(c.min as f64);
    c.value = c.snap(start);
    let value = c.value;
    let link = c.link.clone();
    s.grouped(&format!("Insert {}", kind.label()), |s| {
        edit(s, |cx| {
            cx.sheet_mut(sheet)?.controls.push(c);
            Ok(())
        })?;
        if given.is_some() {
            write_link(s, sheet, link.as_deref(), value)?;
        }
        Ok(json!({"control": id}))
    })
}

/// Applies `min`, `max`, `step`, `page` and `value` from `p` to `c` and checks the result.
fn numbers(cmd: &str, p: &Json, c: &mut FormControl) -> Result<()> {
    for (key, slot) in [("min", &mut c.min), ("max", &mut c.max), ("step", &mut c.step), ("page", &mut c.page)] {
        if let Some(v) = whole(cmd, p, key)? {
            *slot = v;
        }
    }
    // A new slider's page follows a larger step unless it was given.
    if p.get("page").is_none() && c.page < c.step {
        c.page = c.step;
    }
    c.check_numbers().map_err(|e| bad(cmd, e))
}

/// The number the control's linked cell holds, if it holds one.
fn linked_number(wb: &Workbook, host: usize, c: &FormControl) -> Option<f64> {
    let (si, cell) = link_target(wb, host, c.link.as_deref()?)?;
    wb.sheet(si)?.cell(cell)?.value.as_f64()
}

/// Writes `value` to the linked cell through `cell.set` (nothing without a link, or when the cell
/// already shows `value`, formula or not: Format Control's OK sends the value back every time, and
/// a write that changes nothing must not turn a formula into a constant or fail on a locked cell
/// of a protected sheet; a real change replaces a formula with the constant, as in Excel).
fn write_link(s: &mut Session, host: usize, link: Option<&str>, value: u32) -> Result<()> {
    let Some(link) = link else { return Ok(()) };
    let Some((si, cell)) = link_target(&s.doc()?.wb, host, link) else { return Ok(()) };
    let holds = s.doc()?.wb.sheet(si).and_then(|sh| sh.cell(cell)).and_then(|c| c.value.as_f64());
    if holds == Some(value as f64) {
        return Ok(());
    }
    s.execute("cell.set", json!({"sheet": si, "cell": cell.a1(), "input": value.to_string()}))?;
    Ok(())
}

/// The control `id` names on the active sheet: (sheet, its index in `controls`, a copy).
fn find(s: &Session, cmd: &str, p: &Json) -> Result<(usize, usize, FormControl)> {
    let id = u32_param(p, "id").ok_or_else(|| bad(cmd, "missing `id` (a control id from control.list)"))?;
    let d = s.doc()?;
    let sheet = d.wb.active_sheet();
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let i = sh.controls.iter().position(|c| c.id == id).ok_or_else(|| bad(cmd, format!("there is no control {id} on this sheet")))?;
    let c = sh.controls.get(i).cloned().ok_or_else(|| bad(cmd, "no such control"))?;
    Ok((sheet, i, c))
}

/// Drawing objects on a protected sheet can't be changed while they are locked.
/// For the object commands (`object.move`, `object.delete`, `object.setAnchorMode`, `arrange.*`,
/// `arrange.align`): refuses to change form controls `ids` on `sheet` when one is locked and the
/// sheet is protected, as `control.format` and `control.assign` do. Other kinds pass.
pub(crate) fn check_objects_unlocked(s: &Session, sheet: usize, kind: ObjKind, ids: &[u32]) -> Result<()> {
    if kind != ObjKind::Control {
        return Ok(());
    }
    let Some(sh) = s.doc()?.wb.sheet(sheet) else { return Ok(()) };
    if sh.is_protected() && sh.controls.iter().any(|c| c.locked && ids.contains(&c.id)) {
        return Err(EngineError::Other(PROTECTED.into()));
    }
    Ok(())
}

fn check_unlocked(s: &Session, sheet: usize, c: &FormControl) -> Result<()> {
    if c.locked && s.doc()?.wb.sheet(sheet).is_some_and(Sheet::is_protected) {
        return Err(EngineError::Other(PROTECTED.into()));
    }
    Ok(())
}

fn format(s: &mut Session, p: &Json) -> Result<Json> {
    const CMD: &str = "control.format";
    let (sheet, i, mut c) = find(s, CMD, p)?;
    check_unlocked(s, sheet, &c)?;
    if let Some(t) = text_param(CMD, p, "text")? {
        c.text = t;
    }
    if let Some(n) = text_param(CMD, p, "name")? {
        c.name = n.trim().to_string();
    }
    for (key, slot) in [("print", &mut c.print), ("locked", &mut c.locked), ("horizontal", &mut c.horizontal)] {
        if let Some(v) = bool_param(p, key) {
            *slot = v;
        }
    }
    let mut given = None;
    if c.kind != ControlKind::Button {
        for (key, slot) in [("min", &mut c.min), ("max", &mut c.max), ("step", &mut c.step), ("page", &mut c.page)] {
            if let Some(v) = whole(CMD, p, key)? {
                *slot = v;
            }
        }
        c.check_numbers().map_err(|e| bad(CMD, e))?;
        given = whole(CMD, p, "value")?;
    }
    if let Some(link) = link_param(CMD, &s.doc()?.wb, sheet, p)? {
        c.link = link;
    }
    // New bounds pull the value in; a given value is snapped to them.
    c.value = c.snap(given.map_or(c.value as f64, f64::from));
    let number = c.kind != ControlKind::Button;
    let write = number && (given.is_some() || p.get("min").is_some() || p.get("max").is_some() || p.get("step").is_some());
    let (value, link) = (c.value, c.link.clone());
    s.grouped("Format Control", |s| {
        edit(s, |cx| {
            if let Some(slot) = cx.sheet_mut(sheet)?.controls.get_mut(i) {
                *slot = c;
            }
            Ok(())
        })?;
        if write {
            write_link(s, sheet, link.as_deref(), value)?;
        }
        Ok(())
    })?;
    // The reply is the control as committed: a new link to a cell holding another number has
    // moved its value there.
    let (_, _, c) = find(s, CMD, p)?;
    Ok(control_json(&c))
}

/// Commands a button never runs, beyond the macro policy's lists: a button has no way to show
/// what its command would open, so a web page is refused when the action is assigned or comes in
/// from a file. A command that turns out to open a page or a dialog when it runs is refused
/// then, by [`press`].
const BUTTON_DENIED: &[(&str, &str)] = &[("link.open", "buttons don't open web pages")];

/// The macro policy for a button's command, with the button's own refusals first.
fn button_policy(id: &str, params: &Json, trust: crate::Trust) -> Policy {
    match BUTTON_DENIED.iter().find(|(d, _)| *d == id) {
        Some((_, why)) => Policy::Deny((*why).into()),
        None => macro_policy(id, params, trust),
    }
}

/// Drops the UI requests a button's command queued from `queued` on, keeping messages for the
/// person, and says why the press is refused when one would open a dialog or a web page, which
/// buttons don't do (`name` is the button's). A cell edit the command asked for is dropped too.
pub(crate) fn take_requests(s: &mut Session, queued: usize, name: &str) -> Option<String> {
    use crate::UiRequest;
    let mut blocked = None;
    let asked: Vec<UiRequest> = s.ui_requests.drain(queued.min(s.ui_requests.len())..).collect();
    for q in asked {
        match q {
            UiRequest::Dialog(d, _) => blocked = Some(format!("'{name}' would open the {d} dialog; give its command the parameters it needs.")),
            UiRequest::OpenUrl(_) => blocked = Some(format!("'{name}' would open a web page, which buttons don't do.")),
            UiRequest::EditCell(_) | UiRequest::InsertName(_) => {}
            m @ UiRequest::Message(_) => s.ui_requests.push(m),
        }
    }
    blocked
}

/// The stored name of the workbook's macro `name` when it exists and declares no inputs: a fixed
/// input range would make every press fail (`macro.run` refuses inputs it doesn't take), so such
/// an assignment is refused when it is made and dropped to the selection when a file brings one
/// in. A macro the workbook doesn't have (yet) is not checked: the assignment is kept as given.
fn takes_no_inputs(wb: &Workbook, name: &str) -> Option<String> {
    crate::macros::find(wb, name).and_then(|i| wb.macros.get(i)).filter(|m| m.inputs.is_empty()).map(|m| m.name.clone())
}

/// A button action from `macro` (with `input` and `output`) or `command` (with `params`).
fn action_param(s: &Session, cmd: &str, p: &Json) -> Result<Option<ControlAction>> {
    if let Some(name) = str_param(p, "macro").map(str::trim).filter(|n| !n.is_empty()) {
        if name.chars().count() > MAX_MACRO_NAME {
            return Err(bad(cmd, "`macro` is not a macro name"));
        }
        let input = match str_param(p, "input").map(str::trim) {
            None | Some("") => MacroInput::Selection,
            Some(t) if t.eq_ignore_ascii_case("selection") => MacroInput::Selection,
            Some(t) => {
                let (_, body) = split_sheet(t);
                if gridcraft_core::RangeRef::parse(body).is_none() {
                    return Err(bad(cmd, format!("`input` must be \"selection\" or a range such as B2:D9, not `{t}`")));
                }
                if let Some(stored) = takes_no_inputs(&s.doc()?.wb, name) {
                    return Err(bad(cmd, format!("macro '{stored}' takes no inputs, so `input` must be \"selection\"")));
                }
                MacroInput::Fixed(t.to_string())
            }
        };
        let output = str_param(p, "output").map(str::trim).filter(|o| !o.is_empty()).map(str::to_string);
        if let Some(o) = &output
            && gridcraft_core::RangeRef::parse(split_sheet(o).1).is_none()
        {
            return Err(bad(cmd, format!("`output` must be a cell such as F2, not `{o}`")));
        }
        return Ok(Some(ControlAction::Macro { name: name.to_string(), input, output }));
    }
    if let Some(id) = str_param(p, "command").map(str::trim).filter(|n| !n.is_empty()) {
        if find_command(id).is_none() {
            return Err(bad(cmd, format!("there is no command `{id}`")));
        }
        let params = p.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Err(bad(cmd, "`params` must be an object"));
        }
        let trust = s.doc()?.trust;
        if let Policy::Deny(why) = button_policy(id, &params, trust) {
            return Err(EngineError::Other(format!("A button can't run {id}: {why}.")));
        }
        return Ok(Some(ControlAction::Command { id: id.to_string(), params }));
    }
    Ok(None)
}

fn assign(s: &mut Session, p: &Json) -> Result<Json> {
    const CMD: &str = "control.assign";
    let (sheet, i, mut c) = find(s, CMD, p)?;
    check_unlocked(s, sheet, &c)?;
    if c.kind != ControlKind::Button {
        return Err(bad(CMD, "only buttons run actions; a slider or spin button writes its value to its linked cell"));
    }
    let action = action_param(s, CMD, p)?;
    if action.is_none() && bool_param(p, "clear") != Some(true) {
        return Err(bad(CMD, "pass `macro` or `command` (or `clear: true` to remove the action)"));
    }
    c.action = action;
    let out = control_json(&c);
    edit(s, |cx| {
        if let Some(slot) = cx.sheet_mut(sheet)?.controls.get_mut(i) {
            *slot = c;
        }
        Ok(out)
    })
}

fn set_value(s: &mut Session, p: &Json) -> Result<Json> {
    const CMD: &str = "control.setValue";
    let (sheet, i, c) = find(s, CMD, p)?;
    if c.kind == ControlKind::Button {
        return Err(bad(CMD, "a button has no value; use control.press"));
    }
    let v = match p.get("value") {
        Some(Json::Number(n)) => n.as_f64(),
        Some(Json::String(t)) => t.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|v| v.is_finite())
    .ok_or_else(|| bad(CMD, "`value` must be a number"))?;
    let value = c.snap(v);
    let label = format!("Change {}", c.kind.label());
    s.grouped(&label, |s| {
        if c.value != value {
            edit(s, |cx| {
                if let Some(slot) = cx.sheet_mut(sheet)?.controls.get_mut(i) {
                    slot.value = value;
                }
                Ok(())
            })?;
        }
        write_link(s, sheet, c.link.as_deref(), value)?;
        Ok(json!({"value": value}))
    })
}

fn press(s: &mut Session, p: &Json) -> Result<Json> {
    const CMD: &str = "control.press";
    let (_, _, c) = find(s, CMD, p)?;
    if c.kind != ControlKind::Button {
        return Err(bad(CMD, "only buttons can be pressed; use control.setValue for a slider or spin button"));
    }
    let name = c.display_name();
    match c.action {
        None => Err(EngineError::Other(format!("'{name}' has no action. Right-click it and choose Assign Action."))),
        Some(ControlAction::Macro { name: mac, input, output }) => {
            let mut params = json!({"name": mac});
            if let MacroInput::Fixed(r) = input {
                params["inputs"] = json!(r);
            }
            if let Some(o) = output {
                params["output"] = json!(o);
            }
            for key in ["approve", "onConflict", "overwrite", "timeLimit"] {
                if let Some(v) = p.get(key) {
                    params[key] = v.clone();
                }
            }
            // The macro runner gates every step with the document's trust.
            crate::macros::run::run(s, &mac, &params)
        }
        Some(ControlAction::Command { id, params }) => {
            let d = s.doc()?;
            let (doc, trust) = (d.uid, d.trust);
            let policy = button_policy(&id, &params, trust);
            let file_command = matches!(policy, Policy::NeedsPermission(_));
            let why = match policy {
                Policy::Allow => None,
                Policy::Ask(why) | Policy::NeedsPermission(why) => Some(why),
                Policy::Deny(why) => return Err(EngineError::Other(format!("'{name}' can't run {id}: {why}."))),
            };
            if let Some(why) = why {
                let key = (doc, approval_key(&id, &params));
                if !s.macro_approvals.contains(&key) {
                    if bool_param(p, "approve") != Some(true) {
                        return Err(EngineError::Approve(format!("Run {id} from '{name}'? It {why}{}.", path_note(&params))));
                    }
                    s.macro_approvals.insert(key);
                }
            }
            // As a macro step: only an approved file command reaches the file system, so a
            // command that reads or writes a file under another parameter name can't.
            let _sandbox = (!file_command).then(crate::io::ScriptSandbox::enter);
            s.grouped(&name, |s| {
                let queued = s.ui_requests.len();
                let r = s.execute(&id, params.clone());
                // As a macro step: a command that would open a dialog or a web page is refused
                // and its group rolled back; messages for the person stay queued.
                let blocked = take_requests(s, queued, &name);
                let r = r?;
                match blocked {
                    Some(why) => Err(EngineError::Other(why)),
                    None => Ok(r),
                }
            })
        }
    }
}

/// The file a button's command names in its `path`, as ` (path)` for its approval question, so
/// the person sees what it reads or writes before saying yes. Empty without a `path`; a long one
/// is shortened in the middle.
fn path_note(params: &Json) -> String {
    let text = match params.get("path") {
        None | Some(Json::Null) => return String::new(),
        Some(Json::String(t)) => t.clone(),
        Some(other) => other.to_string(),
    };
    let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let n = text.chars().count();
    let shown = if n > 200 {
        let head: String = text.chars().take(100).collect();
        let tail: String = text.chars().skip(n - 99).collect();
        format!("{head}…{tail}")
    } else {
        text
    };
    format!(" ({shown})")
}

/// The approval key of a button's command: its id and parameters, with a prefix that keeps it
/// apart from a macro's fingerprint (canonical JSON, which starts with `{`).
fn approval_key(id: &str, params: &Json) -> String {
    format!("control-command\u{0}{id}\u{0}{params}")
}

/// After an edit: each slider and spin button whose linked cell holds a number takes that number
/// (clamped and snapped), so editing the cell moves the control. Sheets without such a control
/// are left untouched.
pub(crate) fn sync_linked_values(wb: &mut Workbook) {
    for si in 0..wb.sheets.len() {
        let updates: Vec<(usize, u32)> = match wb.sheet(si) {
            Some(sh) if sh.controls.iter().any(|c| c.link.is_some()) => sh
                .controls
                .iter()
                .enumerate()
                .filter(|(_, c)| c.kind != ControlKind::Button)
                .filter_map(|(i, c)| linked_number(wb, si, c).map(|n| (i, c.snap(n))).filter(|(_, v)| *v != c.value))
                .collect(),
            _ => continue,
        };
        if updates.is_empty() {
            continue;
        }
        if let Some(sh) = wb.sheet_mut(si) {
            for (i, v) in updates {
                if let Some(c) = sh.controls.get_mut(i) {
                    c.value = v;
                }
            }
        }
    }
}

// ---------------------------------------------------------------- xlsx

/// The hidden-data key of the button actions xlsx has no place for.
pub(crate) const ACTIONS_KEY: &str = "control_actions";

/// A button's action that xlsx carries itself: a macro on the selection (`macro="[0]!Name"`).
fn plain(a: &ControlAction) -> bool {
    matches!(a, ControlAction::Macro { input: MacroInput::Selection, output: None, .. })
}

/// A copy of `wb` for xlsx with the button actions xlsx can't express (commands, fixed inputs,
/// output cells) in a hidden name, keyed by sheet name and the control's place on the sheet, with
/// the button's name and anchor cell to check it is still the same button when the file comes
/// back. `None` when there is nothing to store.
pub(crate) fn with_action_names(wb: &Workbook) -> Result<Option<Workbook>> {
    let mut items = Vec::new();
    for sh in &wb.sheets {
        for (i, c) in sh.controls.iter().enumerate() {
            if let Some(a) = c.action.as_ref().filter(|a| !plain(a)) {
                items.push(json!({"sheet": sh.name, "index": i, "name": c.display_name(), "at": c.anchor.cell.a1(), "action": a}));
            }
        }
    }
    let stale = stash::get(wb, ACTIONS_KEY).is_some();
    if items.is_empty() && !stale {
        return Ok(None);
    }
    let mut out = wb.clone();
    stash::remove(&mut out, ACTIONS_KEY);
    if !items.is_empty() {
        stash::set_json(&mut out, ACTIONS_KEY, &Json::Array(items))?;
    }
    Ok(Some(out))
}

/// After a file is opened: gives buttons back the actions kept in the hidden name, and removes
/// it. Entries that don't parse, or don't match a button, are dropped: an entry must carry the
/// button's name and anchor cell, the button at the stored place must have that name and anchor
/// cell (another application may have added, removed or reordered controls), and it must not
/// have been given another macro since (a stored macro action comes back only on a button that
/// still runs that macro, a stored command only on one the file gives no macro). A stored fixed
/// input for a macro that takes no inputs becomes the selection (see [`takes_no_inputs`]).
pub(crate) fn load_action_names(wb: &mut Workbook) {
    let Some(items) = stash::get_json(wb, ACTIONS_KEY) else { return };
    stash::remove(wb, ACTIONS_KEY);
    for item in items.as_array().into_iter().flatten().take(100_000) {
        let (Some(sheet), Some(index)) = (item.get("sheet").and_then(Json::as_str), item.get("index").and_then(Json::as_u64)) else { continue };
        let Ok(action) = serde_json::from_value::<ControlAction>(item.get("action").cloned().unwrap_or(Json::Null)) else { continue };
        let valid = match &action {
            ControlAction::Macro { name, .. } => !name.trim().is_empty() && name.chars().count() <= MAX_MACRO_NAME,
            ControlAction::Command { id, params } => !id.is_empty() && id.len() <= 200 && params.is_object(),
        };
        let Some(si) = wb.sheet_index(sheet) else { continue };
        let Ok(index) = usize::try_from(index) else { continue };
        let text = |k: &str| item.get(k).and_then(Json::as_str);
        let same = |c: &FormControl| {
            c.kind == ControlKind::Button
                && text("name").is_some_and(|n| n == c.display_name())
                && text("at").is_some_and(|a| a == c.anchor.cell.a1())
                && match (&c.action, &action) {
                    (None, ControlAction::Command { .. }) => true,
                    (Some(ControlAction::Macro { name: now, .. }), ControlAction::Macro { name: kept, .. }) => now.eq_ignore_ascii_case(kept),
                    _ => false,
                }
        };
        if !valid || !wb.sheet(si).is_some_and(|sh| sh.controls.get(index).is_some_and(same)) {
            continue;
        }
        let action = match action {
            // The macro takes no inputs now: the stored range would fail every press, so the
            // button runs the macro on the selection, as the file itself assigns it.
            ControlAction::Macro { name, input: MacroInput::Fixed(_), output } if takes_no_inputs(wb, &name).is_some() => {
                ControlAction::Macro { name, input: MacroInput::Selection, output }
            }
            other => other,
        };
        if let Some(c) = wb.sheet_mut(si).and_then(|sh| sh.controls.get_mut(index)) {
            c.action = Some(action);
        }
    }
}
