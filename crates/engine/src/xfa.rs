//! XFA scripting in the engine: a form's scripted events (initialize and calculate on open,
//! exit, validate and calculate after a field changes, click for buttons) run through
//! [`pdfcraft_js::xfa`] over the live form tree [`pdfcraft_xfa::form_tree`] builds, and what
//! they change or ask for is applied here: values go to the fields (and so to the datasets),
//! presence and access changes become overrides kept in the PDF, added or removed rows change
//! the datasets, and any of those lays the form out again.

use std::collections::HashMap;
use std::sync::Arc;

use pdfcraft_js::xfa::{XfaDoc, XfaEffect, XfaEvent, XfaKind, XfaNode, run_xfa};
use pdfcraft_xfa::model::Template;
use pdfcraft_xfa::{FormNode, NodeKind, ScriptEvent};

use crate::js::JsOutput;
use crate::{FieldValue, js::Request};

/// Most scripts run for one event (calculates of a whole form, say).
const MAX_SCRIPTS_PER_EVENT: usize = 2_000;
/// Most times the form is laid out again for one event (a script adding rows in a loop).
const MAX_RELAYOUTS_PER_EVENT: usize = 50;

fn to_js(n: &FormNode) -> XfaNode {
    XfaNode {
        name: n.name.clone(),
        som: n.som.clone(),
        kind: n.kind.map(|k| match k {
            NodeKind::Subform => XfaKind::Subform,
            NodeKind::Field => XfaKind::Field,
            NodeKind::ExclGroup => XfaKind::ExclGroup,
            NodeKind::Draw => XfaKind::Draw,
            NodeKind::Area => XfaKind::Area,
        }),
        value: n.value.clone(),
        numeric: n.numeric,
        presence: n.presence.clone(),
        access: n.access.clone(),
        repeatable: n.repeatable,
        occur_min: n.occur_min,
        occur_max: n.occur_max,
        index: n.index,
        children: n.children.iter().map(to_js).collect(),
    }
}

/// The live form and its events, from the document as it is now.
fn live(doc: &pdfcraft_cos::Document, tpl: &Template) -> (FormNode, Vec<ScriptEvent>) {
    let data = pdfcraft_xfa::data_of(doc);
    let ov = pdfcraft_xfa::overrides(doc);
    pdfcraft_xfa::form_tree(tpl, data.as_ref(), &ov)
}

fn find_container<'a>(n: &'a FormNode, som: &str, depth: usize) -> Option<&'a FormNode> {
    if depth > 64 {
        return None;
    }
    if n.som == som && n.kind != Some(NodeKind::Field) {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_container(c, som, depth + 1))
}

fn count_instances(root: &FormNode, parent_som: &str, name: &str) -> usize {
    let parent = if parent_som.is_empty() { Some(root) } else { find_container(root, parent_som, 0) };
    parent.map_or(0, |p| p.children.iter().filter(|c| c.name == name).count())
}

/// An instance SOM (`…table[0].row[2]`) split into its parent's data path and SOM, its name
/// and its index.
fn split_instance(som: &str) -> Option<(pdfcraft_xfa::DataPath, String, String, usize)> {
    let mut path = pdfcraft_xfa::som_to_path(som);
    let (name, index) = path.pop()?;
    let parent_som = som.rsplit_once('.').map(|(p, _)| p).unwrap_or("").to_string();
    Some((path, parent_som, name, index))
}

/// A SOM path without its indexes (`form.page1.qty`), for matching names scripts pass.
fn plain_som(som: &str) -> String {
    let mut out = String::with_capacity(som.len());
    let mut skip = false;
    for c in som.chars() {
        match c {
            '[' => skip = true,
            ']' => skip = false,
            _ if !skip => out.push(c),
            _ => {}
        }
    }
    out
}

/// Does `name` from a script (`qty`, `page1.qty`, `xfa.form.form.page1.qty`, or a container
/// such as `table`) name this SOM or one of its ancestors?
fn som_matches(som: &str, name: &str) -> bool {
    let plain = plain_som(som);
    let n = plain_som(name.trim());
    let n = n.strip_prefix("xfa.form.").or_else(|| n.strip_prefix("$form.")).unwrap_or(&n);
    let want: Vec<&str> = n.split('.').filter(|s| !s.is_empty()).collect();
    let have: Vec<&str> = plain.split('.').filter(|s| !s.is_empty()).collect();
    !want.is_empty() && have.windows(want.len()).any(|w| w == want.as_slice())
}

/// Give field `name` (SOM `som`) the value a script set, in the AcroForm and in the data.
/// Returns whether it changed, with any notes about the data.
fn set_field(doc: &mut pdfcraft_cos::Document, som: &str, name: &str, value: &str) -> Result<(bool, Vec<String>), String> {
    let fields = pdfcraft_forms::fields(doc);
    let Some(f) = fields.iter().find(|f| f.name == name) else { return Ok((false, Vec::new())) };
    let on_state = || f.widgets.first().and_then(|w| w.on_state.clone()).unwrap_or_else(|| "1".into());
    let new = match f.kind {
        pdfcraft_forms::FieldKind::CheckBox => FieldValue::Check(pdfcraft_xfa::data::is_on(value, &on_state()) || value == on_state()),
        pdfcraft_forms::FieldKind::Radio => FieldValue::Radio((!value.trim().is_empty()).then(|| value.trim().to_string())),
        pdfcraft_forms::FieldKind::PushButton | pdfcraft_forms::FieldKind::Signature => return Ok((false, Vec::new())),
        _ => FieldValue::Text(value.to_string()),
    };
    let same = match &new {
        FieldValue::Text(t) => f.value.first().map_or(t.is_empty(), |v| v == t),
        FieldValue::Check(on) => !f.value.is_empty() == *on,
        FieldValue::Radio(sel) => f.value.first() == sel.as_ref(),
        _ => false,
    };
    if same {
        return Ok((false, Vec::new()));
    }
    // The data holds it too, so the next script sees it.
    let (data_text, field_value) = match &new {
        FieldValue::Check(on) => {
            if *on {
                (on_state(), vec![on_state()])
            } else {
                (String::new(), Vec::new())
            }
        }
        FieldValue::Radio(sel) => (sel.clone().unwrap_or_default(), sel.iter().cloned().collect()),
        FieldValue::Text(t) => (t.clone(), vec![t.clone()]),
        _ => (String::new(), Vec::new()),
    };
    let r = pdfcraft_xfa::write_data_value(doc, &pdfcraft_xfa::som_to_path(som), &data_text).map_err(|e| e.to_string())?;
    // Read-only fields still take values from scripts (calculated totals are read-only).
    pdfcraft_forms::apply_script_changes(
        doc,
        &[pdfcraft_forms::FieldChange { name: name.to_string(), value: Some(field_value), ..Default::default() }],
        &mut pdfcraft_forms::NoScripts,
    )
    .map_err(|e| format!("{name}: {e}"))?;
    Ok((true, r.warnings))
}

/// Apply what a script did. Returns whether the form has to be laid out again.
fn apply_effects(doc: &mut pdfcraft_cos::Document, root: &FormNode, effects: &[XfaEffect], out: &mut JsOutput) -> Result<bool, String> {
    let by_som = pdfcraft_xfa::fields_by_som(doc);
    let mut relayout = false;
    let mut ov = pdfcraft_xfa::overrides(doc);
    let mut ov_changed = false;
    // Instances of each repeating subform as the script's effects leave them, from what the
    // form showed before the script ran.
    let mut counts: HashMap<(String, String), usize> = HashMap::new();
    for e in effects {
        match e {
            XfaEffect::SetValue { som, value } => match by_som.get(som) {
                Some(name) => {
                    let (_, notes) = set_field(doc, som, name, value)?;
                    out.console.extend(notes);
                }
                None => {
                    // No widget (a hidden field, a draw, a row not laid out yet): the value
                    // goes to the data alone.
                    let r = pdfcraft_xfa::write_data_value(doc, &pdfcraft_xfa::som_to_path(som), value).map_err(|e| e.to_string())?;
                    out.console.extend(r.warnings);
                }
            },
            XfaEffect::SetPresence { som, presence } => {
                ov.presence.insert(som.clone(), presence.clone());
                ov_changed = true;
                relayout = true;
            }
            XfaEffect::SetAccess { som, access } => {
                ov.access.insert(som.clone(), access.clone());
                ov_changed = true;
                relayout = true;
            }
            XfaEffect::AddInstance { som } | XfaEffect::RemoveInstance { som } => {
                let Some((parent, parent_som, name, index)) = split_instance(som) else { continue };
                let have = *counts.entry((parent_som.clone(), name.clone())).or_insert_with(|| count_instances(root, &parent_som, &name));
                let r = if matches!(e, XfaEffect::AddInstance { .. }) {
                    counts.insert((parent_som, name.clone()), have + 1);
                    pdfcraft_xfa::add_data_instances(doc, &parent, &name, (index + 1).max(have + 1)).map_err(|e| e.to_string())?
                } else {
                    // The data holds every shown instance before one is taken out.
                    let r = pdfcraft_xfa::add_data_instances(doc, &parent, &name, have).map_err(|e| e.to_string())?;
                    out.console.extend(r.warnings);
                    counts.insert((parent_som, name.clone()), have.saturating_sub(1));
                    let mut path = parent;
                    path.push((name, index));
                    pdfcraft_xfa::remove_data_instance(doc, &path).map_err(|e| e.to_string())?
                };
                out.console.extend(r.warnings);
                relayout = true;
            }
            XfaEffect::MessageBox(m) => out.alerts.push(m.clone()),
            XfaEffect::ResetData(names) => {
                let listed: Option<Vec<String>> = if names.is_empty() {
                    None
                } else {
                    Some(by_som.iter().filter(|(som, _)| names.iter().any(|n| som_matches(som, n))).map(|(_, f)| f.clone()).collect())
                };
                if listed.as_ref().is_none_or(|l| !l.is_empty()) {
                    pdfcraft_forms::reset(doc, listed.as_deref()).map_err(|e| e.to_string())?;
                }
            }
            XfaEffect::Print => out.requests.push(Request::Print),
            XfaEffect::SaveAs => out.requests.push(Request::SaveAs),
            XfaEffect::LaunchUrl(u) => out.requests.push(Request::LaunchUrl(u.clone())),
            XfaEffect::SetFocus(som) => {
                if let Some(name) = by_som.get(som) {
                    out.requests.push(Request::Focus(name.clone()));
                }
            }
            XfaEffect::Beep => out.requests.push(Request::Beep),
            XfaEffect::Recalculate => {}
            XfaEffect::Relayout => relayout = true,
        }
    }
    if ov_changed {
        pdfcraft_xfa::set_overrides(doc, &ov).map_err(|e| e.to_string())?;
    }
    Ok(relayout)
}

/// Lay the form out again and give the new widgets their appearances.
fn relayout(doc: &mut pdfcraft_cos::Document, tpl: &Template, out: &mut JsOutput) -> Result<(), String> {
    let report = pdfcraft_xfa::rerender(doc, tpl).map_err(|e| e.to_string())?;
    out.console.extend(report.warnings);
    for f in pdfcraft_forms::fields(doc) {
        pdfcraft_forms::redraw_field(doc, &f.name).map_err(|e| format!("{}: {e}", f.name))?;
    }
    Ok(())
}

struct Runner<'a> {
    doc: &'a mut pdfcraft_cos::Document,
    tpl: &'a Template,
    out: &'a mut JsOutput,
    page_count: usize,
    formcalc_warned: bool,
    runs: usize,
    relayouts: usize,
    /// The live form as last built; dropped when a script changed the document.
    cached: Option<(FormNode, Vec<ScriptEvent>)>,
}

impl Runner<'_> {
    fn new<'a>(doc: &'a mut pdfcraft_cos::Document, tpl: &'a Template, out: &'a mut JsOutput, page_count: usize) -> Runner<'a> {
        Runner { doc, tpl, out, page_count, formcalc_warned: false, runs: 0, relayouts: 0, cached: None }
    }

    fn live(&mut self) -> &(FormNode, Vec<ScriptEvent>) {
        if self.cached.is_none() {
            self.cached = Some(live(self.doc, self.tpl));
        }
        // Just filled in above.
        self.cached.get_or_insert_with(Default::default)
    }

    /// Run one event script against the form as it is now. Returns the script's outcome.
    fn run(&mut self, ev: &ScriptEvent, new_text: &str) -> Result<pdfcraft_js::xfa::XfaOutcome, String> {
        self.runs += 1;
        if self.runs > MAX_SCRIPTS_PER_EVENT {
            return Err("too many scripts ran for one event".into());
        }
        if ev.formcalc {
            if !self.formcalc_warned {
                self.out.errors.push(format!("{}: FormCalc scripts don't run yet (only JavaScript ones)", ev.som));
                self.formcalc_warned = true;
            }
            return Ok(Default::default());
        }
        let event = XfaEvent { activity: ev.activity.clone(), target: ev.som.clone(), new_text: new_text.to_string(), prev_text: String::new() };
        let doc_info = XfaDoc { file_name: String::new(), page: 0, page_count: self.page_count };
        let root_js = to_js(&self.live().0);
        let o = run_xfa(&ev.script, &event, &doc_info, &root_js, pdfcraft_js::Limits::default());
        self.out.console.extend(o.console.iter().cloned());
        if let Some(e) = &o.error {
            self.out.errors.push(format!("{} ({}): {e}", ev.som, ev.activity));
            return Ok(o);
        }
        let changes_doc = o.effects.iter().any(|e| {
            matches!(
                e,
                XfaEffect::SetValue { .. }
                    | XfaEffect::SetPresence { .. }
                    | XfaEffect::SetAccess { .. }
                    | XfaEffect::AddInstance { .. }
                    | XfaEffect::RemoveInstance { .. }
                    | XfaEffect::ResetData(_)
                    | XfaEffect::Relayout
            )
        });
        let relayout_wanted = {
            let root = &self.live().0;
            // The tree is borrowed for the effects; the document is borrowed mutably, so take
            // a clone of the (small) root for this one call.
            let root = root.clone();
            apply_effects(self.doc, &root, &o.effects, self.out)?
        };
        if changes_doc {
            self.cached = None;
        }
        if relayout_wanted {
            self.relayouts += 1;
            if self.relayouts > MAX_RELAYOUTS_PER_EVENT {
                return Err("the form's scripts laid it out again too many times for one event".into());
            }
            relayout(self.doc, self.tpl, self.out)?;
        }
        Ok(o)
    }

    /// Every calculate script, in document order: the field takes the script's result unless
    /// the script set a value itself.
    fn calculates(&mut self) -> Result<(), String> {
        let events: Vec<ScriptEvent> = self.live().1.iter().filter(|e| e.activity == "calculate").cloned().collect();
        for ev in &events {
            let o = self.run(ev, "")?;
            if o.error.is_some() || o.effects.iter().any(|e| matches!(e, XfaEffect::SetValue { som, .. } if *som == ev.som)) {
                continue;
            }
            if let Some(result) = o.result {
                let by_som = pdfcraft_xfa::fields_by_som(self.doc);
                let changed = match by_som.get(&ev.som) {
                    Some(name) => {
                        let (changed, notes) = set_field(self.doc, &ev.som, name, &result)?;
                        self.out.console.extend(notes);
                        changed
                    }
                    None => {
                        let r = pdfcraft_xfa::data::write_data_value(self.doc, &pdfcraft_xfa::som_to_path(&ev.som), &result)
                            .map_err(|e| e.to_string())?;
                        self.out.console.extend(r.warnings);
                        r.written
                    }
                };
                if changed {
                    self.cached = None;
                }
            }
        }
        Ok(())
    }

    fn events_of(&mut self, som: &str, activity: &str) -> Vec<ScriptEvent> {
        self.live().1.iter().filter(|e| e.som == som && e.activity == activity).cloned().collect()
    }
}

/// On open: initialize scripts, then calculations. Returns whether anything changed.
pub(crate) fn on_open(doc: &mut pdfcraft_cos::Document, tpl: &Template, page_count: usize, out: &mut JsOutput) -> Result<bool, String> {
    let before = doc.is_modified();
    let mut r = Runner::new(doc, tpl, out, page_count);
    let events: Vec<ScriptEvent> = r.live().1.iter().filter(|e| e.activity == "initialize" || e.activity == "docReady").cloned().collect();
    for ev in &events {
        r.run(ev, "")?;
    }
    r.calculates()?;
    Ok(!before && r.doc.is_modified() || before)
}

/// After field `name` took a new value: its exit and validate scripts, then calculations. A
/// validate script that answers false, or fails, shows its message; the value stays, as Acrobat
/// marks the field invalid rather than reverting it.
pub(crate) fn on_change(doc: &mut pdfcraft_cos::Document, tpl: &Template, name: &str, page_count: usize, out: &mut JsOutput) -> Result<(), String> {
    let som = pdfcraft_xfa::fields_by_som(doc).into_iter().find(|(_, f)| f == name).map(|(s, _)| s);
    let mut r = Runner::new(doc, tpl, out, page_count);
    if let Some(som) = som {
        let value = pdfcraft_forms::fields(r.doc).into_iter().find(|f| f.name == name).map(|f| f.value.join("\n")).unwrap_or_default();
        for ev in r.events_of(&som, "change").into_iter().chain(r.events_of(&som, "exit")) {
            r.run(&ev, &value)?;
        }
        for ev in r.events_of(&som, "validate") {
            let o = r.run(&ev, &value)?;
            if (o.result_bool == Some(false) || o.error.is_some()) && !value.trim().is_empty() {
                r.out.alerts.push(ev.message.clone().unwrap_or_else(|| format!("{name}: the value is not valid")));
            }
        }
    }
    r.calculates()
}

/// Every calculation (after a reset).
pub(crate) fn recalculate(doc: &mut pdfcraft_cos::Document, tpl: &Template, page_count: usize, out: &mut JsOutput) -> Result<(), String> {
    Runner::new(doc, tpl, out, page_count).calculates()
}

/// A button's click script (or any event, by SOM path and activity), then calculations.
pub(crate) fn on_event(
    doc: &mut pdfcraft_cos::Document,
    tpl: &Template,
    som: &str,
    activity: &str,
    page_count: usize,
    out: &mut JsOutput,
) -> Result<(), String> {
    let mut r = Runner::new(doc, tpl, out, page_count);
    let events = r.events_of(som, activity);
    if events.is_empty() {
        return Err(format!("{som} has no {activity} script"));
    }
    for ev in events {
        r.run(&ev, "")?;
    }
    r.calculates()
}

/// The SOM path of field `name` when PdfCraft generated it from an XFA template and the
/// template has a click script for it (a button with a native action has none).
pub(crate) fn clickable_som(doc: &pdfcraft_cos::Document, tpl: &Template, name: &str) -> Option<String> {
    let som = pdfcraft_xfa::fields_by_som(doc).into_iter().find(|(_, f)| f == name).map(|(s, _)| s)?;
    let (_, events) = live(doc, tpl);
    events.iter().any(|e| e.som == som && e.activity == "click").then_some(som)
}

/// The parsed template of a laid-out XFA form, cached for the document's life.
pub(crate) fn template(doc: &pdfcraft_cos::Document) -> Option<Arc<Template>> {
    pdfcraft_xfa::template_of(doc).ok().map(Arc::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_names_match_indexed_som_paths() {
        assert!(som_matches("form[0].page1[0].qty[0]", "qty"));
        assert!(som_matches("form[0].page1[0].qty[0]", "page1.qty"));
        assert!(som_matches("form[0].page1[0].qty[0]", "xfa.form.form.page1.qty"));
        assert!(som_matches("form[0].page1[0].table[0].row[2].amount[0]", "table"));
        assert!(!som_matches("form[0].page1[0].qty[0]", "qt"));
        assert!(!som_matches("form[0].page1[0].price[0]", "qty"));
        assert_eq!(
            split_instance("form[0].table[0].row[2]").map(|(p, ps, n, i)| (p.len(), ps, n, i)),
            Some((2, "form[0].table[0]".into(), "row".into(), 2))
        );
    }
}
