//! XFA scripting in the engine: a form's scripted events (initialize and calculate on open,
//! exit, validate and calculate after a field changes, click for buttons) run through
//! [`pdfcraft_js::xfa`] over the live form tree [`pdfcraft_xfa::form_tree`] builds, and what
//! they change or ask for is applied here: values go to the fields (and so to the datasets),
//! presence and access changes become overrides kept in the PDF, added or removed rows change
//! the datasets, and any of those lays the form out again.

use std::collections::HashMap;
use std::sync::Arc;

use pdfcraft_js::xfa::{XfaDoc, XfaEffect, XfaEvent, XfaKind, XfaNode};
use pdfcraft_js::xfa_session::{Script, XfaSession};
use pdfcraft_xfa::model::Template;
use pdfcraft_xfa::{FormNode, LiveForm, NodeKind, ScriptEvent};

use crate::js::JsOutput;
use crate::js::Request;

/// Most scripts run for one event (calculates of a whole form, say).
const MAX_SCRIPTS_PER_EVENT: usize = 2_000;
/// Most times the form is laid out again for one event (a script adding rows in a loop).
const MAX_RELAYOUTS_PER_EVENT: usize = 50;
/// Most message boxes, console lines and errors kept for one event.
const MAX_ALERTS_PER_EVENT: usize = 100;
const MAX_CONSOLE_PER_EVENT: usize = 1_000;
const MAX_ERRORS_PER_EVENT: usize = 100;
/// Time the scripts of one event may take in all before the rest are skipped: opening (and
/// the calculations every change runs) is held tighter than a click.
const OPEN_BUDGET_MS: u64 = 3_000;
const EVENT_BUDGET_MS: u64 = 5_000;
/// Cumulative host-side replay work per event, on native and WASM. Charges full
/// snapshot scans, cache invalidation scans, and emitted descendant data operations.
const MAX_EFFECT_WORK: usize = pdfcraft_xfa::MAX_FORM_NODES * 10;

/// Engine limits for one script: initialize, calculate and validate scripts run without the
/// user asking (on open, on every change), so they get fewer loop iterations than a click.
fn limits_for(activity: &str) -> pdfcraft_js::Limits {
    match activity {
        "click" | "change" | "exit" | "enter" => pdfcraft_js::Limits::default(),
        _ => pdfcraft_js::Limits { loop_iterations: 100_000, recursion: 64 },
    }
}

/// Longest one script may run before it is abandoned (it can't be interrupted, only left
/// behind): scripts that run unasked are held tighter than a click's.
fn timeout_for(activity: &str) -> std::time::Duration {
    std::time::Duration::from_millis(match activity {
        "click" | "change" | "exit" | "enter" => 3_000,
        _ => 1_000,
    })
}

/// Requests only a click may make (anything else asking is noted and ignored).
fn click_only(e: &XfaEffect) -> Option<&'static str> {
    match e {
        XfaEffect::Print => Some("print"),
        XfaEffect::SaveAs => Some("save"),
        XfaEffect::LaunchUrl(_) => Some("open a link"),
        XfaEffect::ShowPanel(_) => Some("show a side panel"),
        XfaEffect::SetFocus(_) => Some("move the focus"),
        _ => None,
    }
}

/// A wall clock where there is one (not in the browser build, where the script and relayout
/// caps alone bound an event).
#[derive(Clone, Copy)]
struct Clock {
    #[cfg(not(target_arch = "wasm32"))]
    started: std::time::Instant,
    budget_ms: u64,
}

impl Clock {
    fn start(budget_ms: u64) -> Self {
        Clock {
            #[cfg(not(target_arch = "wasm32"))]
            started: std::time::Instant::now(),
            budget_ms,
        }
    }

    fn spent(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.started.elapsed() >= std::time::Duration::from_millis(self.budget_ms)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = self.budget_ms;
            false
        }
    }
}

/// What the scripts of an event changed in the form (for the note opening leaves).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Changes {
    pub values: usize,
    pub rows: usize,
    pub visibility: usize,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.values == 0 && self.rows == 0 && self.visibility == 0
    }

    /// "3 values, 1 row" — what changed, for a note.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        let n = |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
        if self.values > 0 {
            parts.push(n(self.values, "field value", "field values"));
        }
        if self.rows > 0 {
            parts.push(n(self.rows, "added or removed row", "added or removed rows"));
        }
        if self.visibility > 0 {
            parts.push(n(self.visibility, "shown, hidden or locked object", "shown, hidden or locked objects"));
        }
        parts.join(", ")
    }
}

/// What a run of the form's scripts shares with the edit around it.
pub(crate) struct XfaRun<'a> {
    pub page_count: usize,
    pub out: &'a mut JsOutput,
    /// The datasets stream this edit already wrote, replaced in place by later writes.
    pub datasets: Option<pdfcraft_cos::ObjRef>,
    /// A script ran past its time limit and was abandoned: the form's scripts should stay off.
    pub ran_away: bool,
    /// What every time limit of the scripts is multiplied by (the event's budget, the engine's
    /// start and each script's own): 1, unless the session was given another
    /// ([`crate::Session::with_xfa_time_scale`]).
    pub time_scale: u32,
}

/// Copy the live tree and detached definitions into the script-host snapshot, including
/// each field's formatted display value.
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
        template_order: n.template_order,
        children: n.children.iter().map(to_js).collect(),
        instance_templates: n.instance_templates.iter().map(to_js).collect(),
        formatted: n.display_picture.as_deref().and_then(|p| pdfcraft_xfa::formatted_value(p, &n.value)),
    }
}

/// Find the live owner of a row, not just the first container with its parent's SOM.
/// Unnamed wrappers share that SOM. Inspect retained definitions for ownership, but
/// never descend into them; all inspected candidates spend the replay work budget.
fn find_container<'a>(n: &'a FormNode, som: &str, name: &str, depth: usize, visited: &mut usize) -> Option<&'a FormNode> {
    *visited = visited.saturating_add(1);
    if depth > 64 {
        return None;
    }
    if n.som == som
        && n.kind != Some(NodeKind::Field)
        && n.children.iter().chain(&n.instance_templates).any(|child| {
            *visited = visited.saturating_add(1);
            child.name == name && child.kind == Some(NodeKind::Subform)
        })
    {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_container(c, som, name, depth + 1, visited))
}

/// Counts and default descendant SOM suffixes for one manager during an effect batch.
/// Suffixes are rebased for each new row, so the cached definition holds no user values.
struct InstanceState {
    count: usize,
    seeds: Vec<String>,
}

impl InstanceState {
    /// Read a manager once from the current tree. Only live descendants of its retained
    /// definition seed data; dormant children must stay absent. Traversal is bounded.
    fn read(root: &FormNode, parent_som: &str, name: &str, work: &mut usize) -> Result<Self, String> {
        let parent = find_container(root, parent_som, name, 0, work);
        let count = parent.map_or(0, |p| {
            *work = work.saturating_add(p.children.len());
            p.children.iter().filter(|c| c.name == name).count()
        });
        let mut state = Self { count, seeds: Vec::new() };
        let Some(source) = parent.and_then(|p| {
            p.instance_templates.iter().find(|n| {
                *work = work.saturating_add(1);
                n.name == name
            })
        }) else {
            return Ok(state);
        };
        let mut pending = source.children.iter().map(|n| (n, 1usize)).collect::<Vec<_>>();
        let mut visited = 0;
        while let Some((node, depth)) = pending.pop() {
            visited += 1;
            *work = work.saturating_add(1);
            if depth > 64 || visited > pdfcraft_xfa::MAX_FORM_NODES {
                return Err("The added XFA instance exceeds the supported structure limits".into());
            }
            if node.kind == Some(NodeKind::Subform)
                && !node.name.is_empty()
                && node.name != "#subform"
                && let Some(suffix) = node.som.strip_prefix(&source.som)
            {
                state.seeds.push(suffix.to_string());
            }
            pending.extend(node.children.iter().map(|n| (n, depth + 1)));
        }
        Ok(state)
    }
}

/// Size of a bounded form snapshot, including definitions retained for managers. This
/// accounts for rebuilding even when unrelated subtrees contain no fields. Manager
/// lookups count only the nodes they actually visit.
fn form_nodes(node: &FormNode, depth: usize) -> usize {
    if depth > 64 {
        return pdfcraft_xfa::MAX_FORM_NODES;
    }
    node.children.iter().chain(&node.instance_templates).fold(1usize, |n, c| n.saturating_add(form_nodes(c, depth + 1)))
}

/// An instance SOM (`…table[0].row[2]`) split into its parent's data path (areas left out,
/// see [`pdfcraft_xfa::data_path`]) and SOM, its name and its index.
fn split_instance(tpl: &Template, som: &str) -> Option<(pdfcraft_xfa::DataPath, String, String, usize)> {
    let mut path = pdfcraft_xfa::data_path(tpl, som);
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

/// A generated field as the runner tracks it: kind, value and on state.
#[derive(Clone)]
struct FieldInfo {
    kind: pdfcraft_forms::FieldKind,
    value: Vec<String>,
    on_state: String,
    /// List boxes that take several values (one per line in the data).
    multi: bool,
}

fn field_infos(doc: &pdfcraft_cos::Document) -> HashMap<String, FieldInfo> {
    pdfcraft_forms::fields(doc)
        .into_iter()
        .map(|f| {
            let on_state = f.widgets.first().and_then(|w| w.on_state.clone()).unwrap_or_else(|| "1".into());
            let multi = f.kind == pdfcraft_forms::FieldKind::List && f.has(pdfcraft_forms::flags::MULTI_SELECT);
            (f.name, FieldInfo { kind: f.kind, value: f.value, on_state, multi })
        })
        .collect()
}

/// The data text of a field's value.
fn data_text(info: &FieldInfo) -> String {
    match info.kind {
        pdfcraft_forms::FieldKind::CheckBox => {
            if info.value.is_empty() {
                String::new()
            } else {
                info.on_state.clone()
            }
        }
        _ => info.value.join("\n"),
    }
}

/// Runs the scripts of one event (opening, a change, a click) against one document: what
/// they change is kept in memory (data, field values, overrides) and written once, when the
/// event ends or the form has to be laid out again.
struct Runner<'a, 'b> {
    doc: &'a mut pdfcraft_cos::Document,
    tpl: &'a Template,
    run: &'a mut XfaRun<'b>,
    clock: Clock,
    runs: usize,
    skipped: usize,
    relayouts: usize,
    relayouts_skipped: bool,
    dropped_output: usize,
    truncated_warned: bool,
    data: Option<pdfcraft_xfa::DataNode>,
    ov: pdfcraft_xfa::Overrides,
    ov_dirty: bool,
    pending: Vec<pdfcraft_xfa::DataOp>,
    /// Field values to write, by name (the last wins).
    field_changes: std::collections::BTreeMap<String, Vec<String>>,
    by_som: Option<HashMap<String, String>>,
    /// Widget names by the data node they bind to (built with `by_som`).
    by_path: Option<HashMap<pdfcraft_xfa::DataPath, Vec<String>>>,
    fields: Option<HashMap<String, FieldInfo>>,
    /// The live form as last built; dropped when a script changed the form.
    cached: Option<LiveForm>,
    /// Rebuilt nodes not yet charged to structural replay. Accumulate across invalidations
    /// so a snapshot is charged once even if several scripts rebuild before a manager lookup.
    pending_snapshot_work: usize,
    /// Remaining replay work across all scripts of this event (not reset per batch).
    effect_work: usize,
    #[cfg(test)]
    live_builds: usize,
    /// The script engine for this event: one thread for all its scripts, each run in a fresh
    /// context on the form as it is then (`cached`, kept up to date in place for values and
    /// rebuilt when the structure changed).
    session: Option<XfaSession>,
    changes: Changes,
}

impl<'a, 'b> Runner<'a, 'b> {
    /// Begin an event with private data/override state and cumulative script and replay budgets.
    fn new(doc: &'a mut pdfcraft_cos::Document, tpl: &'a Template, run: &'a mut XfaRun<'b>, budget_ms: u64) -> Self {
        let data = pdfcraft_xfa::data_of(doc);
        let ov = pdfcraft_xfa::overrides(doc);
        let budget_ms = budget_ms.saturating_mul(u64::from(run.time_scale));
        Runner {
            doc,
            tpl,
            run,
            clock: Clock::start(budget_ms),
            runs: 0,
            skipped: 0,
            relayouts: 0,
            relayouts_skipped: false,
            dropped_output: 0,
            truncated_warned: false,
            data,
            ov,
            ov_dirty: false,
            pending: Vec::new(),
            field_changes: Default::default(),
            by_som: None,
            by_path: None,
            fields: None,
            cached: None,
            pending_snapshot_work: 0,
            #[cfg(not(test))]
            effect_work: MAX_EFFECT_WORK,
            #[cfg(test)]
            effect_work: test_faults::effect_work_limit(),
            #[cfg(test)]
            live_builds: 0,
            session: None,
            changes: Changes::default(),
        }
    }

    /// The live form no longer matches the data (its structure changed): rebuilt for the next
    /// script.
    fn invalidate(&mut self) {
        self.cached = None;
    }

    /// The script engine for this event.
    fn session(&mut self) -> &mut XfaSession {
        let doc_info = XfaDoc { file_name: String::new(), page: 0, page_count: self.run.page_count };
        let scale = self.run.time_scale;
        self.session.get_or_insert_with(|| XfaSession::new(doc_info).with_time_scale(scale))
    }

    /// Give the object at `som` the value `text` in the data and in the live form (refreshed
    /// in place, as the tree builder would set it; a value changes nothing else, so the tree
    /// is not rebuilt for it).
    fn value_op(&mut self, som: &str, path: pdfcraft_xfa::DataPath, text: String) {
        let op = pdfcraft_xfa::DataOp::Value { path, text };
        self.data.get_or_insert_with(Default::default).apply(&op);
        self.pending.push(op);
        let refreshed = match self.cached.as_mut() {
            Some(live) => pdfcraft_xfa::refresh_value(&mut live.root, self.tpl, self.data.as_ref(), som).is_some(),
            None => true,
        };
        if !refreshed {
            self.invalidate();
        }
    }

    /// Rebuild an invalidated snapshot lazily and record its size for replay-work accounting.
    fn live(&mut self) -> &LiveForm {
        if self.cached.is_none() {
            #[cfg(test)]
            {
                self.live_builds += 1;
            }
            let live = pdfcraft_xfa::form_tree(self.tpl, self.data.as_ref(), &self.ov);
            if live.truncated && !self.truncated_warned {
                self.truncated_warned = true;
                self.error(format!(
                    "The form has more than {} objects or 20000 scripted events; its scripts see only the first ones",
                    pdfcraft_xfa::MAX_FORM_NODES
                ));
            }
            self.pending_snapshot_work = self.pending_snapshot_work.saturating_add(form_nodes(&live.root, 0));
            self.cached = Some(live);
        }
        self.cached.get_or_insert_with(Default::default)
    }

    fn by_som(&mut self) -> &HashMap<String, String> {
        if self.by_som.is_none() {
            let by_som = pdfcraft_xfa::fields_by_som(self.doc);
            let mut by_path: HashMap<pdfcraft_xfa::DataPath, Vec<String>> = HashMap::new();
            for (som, name) in &by_som {
                by_path.entry(pdfcraft_xfa::data_path(self.tpl, som)).or_default().push(name.clone());
            }
            for names in by_path.values_mut() {
                names.sort();
            }
            self.by_path = Some(by_path);
            self.by_som = Some(by_som);
        }
        self.by_som.get_or_insert_with(Default::default)
    }

    /// The widgets bound to the data node at `path`, in name order.
    fn widgets_at(&mut self, path: &pdfcraft_xfa::DataPath) -> Vec<String> {
        self.by_som();
        self.by_path.as_ref().and_then(|m| m.get(path)).cloned().unwrap_or_default()
    }

    fn fields(&mut self) -> &mut HashMap<String, FieldInfo> {
        if self.fields.is_none() {
            self.fields = Some(field_infos(self.doc));
        }
        self.fields.get_or_insert_with(Default::default)
    }

    fn error(&mut self, e: String) {
        if self.run.out.errors.len() < MAX_ERRORS_PER_EVENT {
            self.run.out.errors.push(e);
        } else {
            self.dropped_output += 1;
        }
    }

    /// Keep console `lines`, up to [`MAX_CONSOLE_PER_EVENT`] for one event; the rest are counted.
    fn console(&mut self, lines: impl IntoIterator<Item = String>) {
        for l in lines {
            if self.run.out.console.len() < MAX_CONSOLE_PER_EVENT {
                self.run.out.console.push(l);
            } else {
                self.dropped_output += 1;
            }
        }
    }

    /// Show message `m`, up to [`MAX_ALERTS_PER_EVENT`] for one event; the rest are counted.
    fn alert(&mut self, m: crate::js::Message) {
        if self.run.out.alerts.len() < MAX_ALERTS_PER_EVENT {
            self.run.out.alerts.push(m);
        } else {
            self.dropped_output += 1;
        }
    }

    /// Change the data's structure (and the in-memory copy scripts read): the form is rebuilt
    /// for the next script.
    fn data_op(&mut self, op: pdfcraft_xfa::DataOp) {
        self.data.get_or_insert_with(Default::default).apply(&op);
        self.pending.push(op);
        self.invalidate();
    }

    /// Give the object at `som` the value `value`: in the data, and in every widget bound to
    /// that data node (each valued by its own kind: a check box by its on state, a list box by
    /// lines). Returns whether anything changed.
    fn set_value(&mut self, som: &str, value: &str) -> bool {
        use pdfcraft_forms::FieldKind as K;
        let path = pdfcraft_xfa::data_path(self.tpl, som);
        let names = self.widgets_at(&path);
        // The data text: the widget at `som` says how its kind writes the value; without one,
        // the first widget bound to the node, else the value itself.
        let primary = self.by_som().get(som).cloned().or_else(|| names.first().cloned());
        let mut text = value.to_string();
        let mut changed = false;
        for name in &names {
            let Some(info) = self.fields().get(name).cloned() else { continue };
            let new_value: Vec<String> = match info.kind {
                K::CheckBox => {
                    if pdfcraft_xfa::data::is_on(value, &info.on_state) || value == info.on_state {
                        vec![info.on_state.clone()]
                    } else {
                        Vec::new()
                    }
                }
                K::Radio => {
                    let v = value.trim();
                    if v.is_empty() { Vec::new() } else { vec![v.to_string()] }
                }
                // Read-only fields still take values from scripts (calculated totals are
                // read-only); buttons and signatures hold none.
                K::PushButton | K::Signature => continue,
                K::List if info.multi => value.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect(),
                _ => vec![value.to_string()],
            };
            let same = match info.kind {
                K::CheckBox => info.value.is_empty() == new_value.is_empty(),
                K::Radio => info.value.first() == new_value.first(),
                K::List if info.multi => info.value == new_value,
                _ => info.value.first().map_or(value.is_empty(), |v| v == value),
            };
            let updated = FieldInfo { value: new_value.clone(), ..info };
            if primary.as_deref() == Some(name.as_str()) {
                text = data_text(&updated);
            }
            if same {
                continue;
            }
            self.fields().insert(name.clone(), updated);
            self.field_changes.insert(name.clone(), new_value);
            changed = true;
        }
        if self.data.as_ref().and_then(|d| d.text_at(&path)).unwrap_or("") != text {
            self.value_op(som, path, text);
            changed = true;
        }
        if changed {
            self.changes.values += 1;
        }
        changed
    }

    /// Write what the scripts changed so far: the data as one rewrite, the field values, the
    /// overrides.
    fn flush(&mut self) -> Result<(), String> {
        let mut wrote = false;
        if !self.pending.is_empty() {
            let ops = std::mem::take(&mut self.pending);
            let r = pdfcraft_xfa::write_data_ops(self.doc, &ops, self.run.datasets).map_err(|e| e.to_string())?;
            if r.stream.is_some() {
                self.run.datasets = r.stream;
            }
            self.console(r.warnings);
            wrote = true;
        }
        if !self.field_changes.is_empty() {
            let changes: Vec<pdfcraft_forms::FieldChange> = std::mem::take(&mut self.field_changes)
                .into_iter()
                .map(|(name, value)| pdfcraft_forms::FieldChange { name, value: Some(value), ..Default::default() })
                .collect();
            pdfcraft_forms::apply_script_changes(self.doc, &changes, &mut pdfcraft_forms::NoScripts).map_err(|e| e.to_string())?;
            self.fields = None;
        }
        if self.ov_dirty {
            pdfcraft_xfa::set_overrides(self.doc, &self.ov).map_err(|e| e.to_string())?;
            self.ov_dirty = false;
        }
        if wrote {
            // What the packet now holds (a value it could not take is reported above).
            self.data = pdfcraft_xfa::data_of(self.doc);
            self.invalidate();
        }
        Ok(())
    }

    /// Lay the form out again and give the new widgets their appearances.
    fn relayout(&mut self) -> Result<(), String> {
        if self.relayouts >= MAX_RELAYOUTS_PER_EVENT {
            if !self.relayouts_skipped {
                self.relayouts_skipped = true;
                self.error(format!(
                    "The form's scripts asked to lay it out again more than {MAX_RELAYOUTS_PER_EVENT} times; the form is laid out once more when they finish"
                ));
            }
            return Ok(());
        }
        self.relayouts += 1;
        self.flush()?;
        let report = pdfcraft_xfa::rerender(self.doc, self.tpl).map_err(|e| e.to_string())?;
        // An empty import can materialize initial groups during layout. Subsequent scripts
        // must see those groups, not the pre-binding empty tree.
        if self.data.as_ref().is_none_or(|data| data.children.is_empty()) {
            self.data = pdfcraft_xfa::data_of(self.doc);
        }
        self.console(report.warnings);
        crate::xfa_redraw(self.doc)?;
        self.console(crate::undrawn_field_warnings(self.doc));
        self.by_som = None;
        self.by_path = None;
        self.fields = None;
        self.invalidate();
        Ok(())
    }

    /// `xfa.host.resetData`: the fields back to their defaults, in the form and the data.
    fn reset(&mut self, names: &[String]) -> Result<(), String> {
        self.flush()?;
        let by_som = self.by_som().clone();
        let listed: Option<Vec<String>> = if names.is_empty() {
            None
        } else {
            Some(by_som.iter().filter(|(som, _)| names.iter().any(|n| som_matches(som, n))).map(|(_, f)| f.clone()).collect())
        };
        if listed.as_ref().is_some_and(Vec::is_empty) {
            return Ok(());
        }
        pdfcraft_forms::reset(self.doc, listed.as_deref()).map_err(|e| e.to_string())?;
        self.fields = None;
        let infos = self.fields().clone();
        for (som, name) in &by_som {
            if listed.as_ref().is_some_and(|l| !l.contains(name)) {
                continue;
            }
            let Some(info) = infos.get(name) else { continue };
            if matches!(info.kind, pdfcraft_forms::FieldKind::PushButton | pdfcraft_forms::FieldKind::Signature) {
                continue;
            }
            let path = pdfcraft_xfa::data_path(self.tpl, som);
            let text = data_text(info);
            if self.data.as_ref().and_then(|d| d.text_at(&path)).unwrap_or("") != text {
                self.data_op(pdfcraft_xfa::DataOp::Value { path, text });
                self.changes.values += 1;
            }
        }
        self.invalidate();
        Ok(())
    }

    /// Charge cumulative host work and the event deadline. Returning an error aborts
    /// the enclosing engine edit/open transaction; callers must not publish partial state.
    fn effect_charge(&mut self, work: usize) -> Result<(), String> {
        if self.clock.spent() {
            return Err("The XFA event exceeded its time limit while applying script changes".into());
        }
        self.effect_work = self.effect_work.checked_sub(work).ok_or("The XFA event exceeded its instance-processing work limit")?;
        Ok(())
    }

    /// Apply what a script of `ev` did. Returns whether the form has to be laid out again.
    /// Counts/defaults are cached per manager until a change can invalidate them. A
    /// removed parent invalidates all descendant managers, including shifted siblings;
    /// their next use reads the updated tree. Unrelated managers retain their counts.
    fn apply_effects(&mut self, ev: &ScriptEvent, effects: &[XfaEffect]) -> Result<bool, String> {
        let mut relayout = false;
        let mut instances: HashMap<(String, String), InstanceState> = HashMap::new();
        for e in effects {
            self.effect_charge(1)?;
            // Only a click script may print, save, open links or move the focus (not the
            // calculations a click sets off, nor anything run on open or on a change).
            if let Some(what) = click_only(e)
                && ev.activity != "click"
            {
                let note = format!("{} ({}): a script may only {what} when a button is clicked; ignored", ev.som, ev.activity);
                self.console([note]);
                continue;
            }
            match e {
                XfaEffect::SetValue { som, value } => {
                    // Updating a field in an existing data parent cannot change manager
                    // counts. Keep the cache for the common add-row-then-fill sequence;
                    // creating missing ancestors requires a fresh structural snapshot.
                    let path = pdfcraft_xfa::data_path(self.tpl, som);
                    let parent_exists = path.split_last().is_some_and(|(_, parent)| self.data.as_ref().and_then(|d| d.get(parent)).is_some());
                    if !parent_exists {
                        instances.clear();
                    }
                    self.set_value(som, value);
                }
                XfaEffect::SetPresence { som, presence } => {
                    instances.clear();
                    if self.ov.presence.insert(som.clone(), presence.clone()).as_ref() != Some(presence) {
                        self.ov_dirty = true;
                        self.changes.visibility += 1;
                        self.invalidate();
                        relayout = true;
                    }
                }
                XfaEffect::SetAccess { som, access } => {
                    instances.clear();
                    if self.ov.access.insert(som.clone(), access.clone()).as_ref() != Some(access) {
                        self.ov_dirty = true;
                        self.changes.visibility += 1;
                        self.invalidate();
                        relayout = true;
                    }
                }
                XfaEffect::AddInstance { som } | XfaEffect::RemoveInstance { som } => {
                    let Some((parent, parent_som, name, index)) = split_instance(self.tpl, som) else { continue };
                    let key = (parent_som.clone(), name.clone());
                    if !instances.contains_key(&key) {
                        self.live();
                        // Charge rebuilt snapshots once, then only the nodes visited by
                        // this lookup. Each snapshot and lookup is independently bounded.
                        let rebuilt = std::mem::take(&mut self.pending_snapshot_work);
                        self.effect_charge(rebuilt)?;
                        let mut lookup_work = 0;
                        let state = InstanceState::read(&self.live().root, &parent_som, &name, &mut lookup_work)?;
                        self.effect_charge(lookup_work)?;
                        instances.insert(key.clone(), state);
                    }
                    let Some(state) = instances.get_mut(&key) else { continue };
                    let have = state.count;
                    if matches!(e, XfaEffect::AddInstance { .. }) {
                        self.effect_charge(state.seeds.len())?;
                        let count = index.saturating_add(1).max(have.saturating_add(1));
                        self.data_op(pdfcraft_xfa::DataOp::Instances { parent, name, count });
                        for suffix in &state.seeds {
                            let mut path = pdfcraft_xfa::data_path(self.tpl, &format!("{som}{suffix}"));
                            if let Some((name, index)) = path.pop() {
                                self.data_op(pdfcraft_xfa::DataOp::Instances { parent: path, name, count: index.saturating_add(1) });
                            }
                        }
                        state.count = count;
                    } else {
                        // Materialize initial rows before removing one; a blank dataset
                        // does not yet contain every row shown by the template.
                        self.data_op(pdfcraft_xfa::DataOp::Instances { parent: parent.clone(), name: name.clone(), count: have });
                        let mut path = parent;
                        path.push((name.clone(), index));
                        self.data_op(pdfcraft_xfa::DataOp::Remove(path));
                        state.count = have.saturating_sub(usize::from(index < have));
                        self.effect_charge(instances.len())?;
                        let prefix = if parent_som.is_empty() { format!("{name}[") } else { format!("{parent_som}.{name}[") };
                        instances.retain(|(p, _), _| !p.starts_with(&prefix));
                    }
                    self.changes.rows += 1;
                    relayout = true;
                }
                XfaEffect::MessageBox(m) => self.alert(m.clone()),
                XfaEffect::ResetData(names) => {
                    instances.clear();
                    self.reset(names)?;
                }
                XfaEffect::Print => self.run.out.requests.push(Request::Print),
                XfaEffect::SaveAs => self.run.out.requests.push(Request::SaveAs),
                XfaEffect::LaunchUrl(u) => self.run.out.requests.push(Request::LaunchUrl(u.clone())),
                XfaEffect::ShowPanel(p) => self.run.out.requests.push(Request::ShowPanel(*p)),
                XfaEffect::SetFocus(som) => {
                    if let Some(name) = self.by_som().get(som).cloned() {
                        self.run.out.requests.push(Request::Focus(name));
                    }
                }
                XfaEffect::Beep => self.run.out.requests.push(Request::Beep),
                XfaEffect::Recalculate => {}
                XfaEffect::Relayout => relayout = true,
            }
        }
        self.effect_charge(0)?;
        Ok(relayout)
    }

    /// Run one event script against the form as it is now. `None`: it was skipped (the
    /// event's budget is spent).
    fn run(&mut self, ev: &ScriptEvent, new_text: &str) -> Result<Option<pdfcraft_js::xfa::XfaOutcome>, String> {
        if self.runs >= MAX_SCRIPTS_PER_EVENT || self.clock.spent() || self.run.ran_away {
            self.skipped += 1;
            return Ok(None);
        }
        self.runs += 1;
        #[cfg(test)]
        self.check_live();
        let event = XfaEvent { activity: ev.activity.clone(), target: ev.som.clone(), new_text: new_text.to_string(), prev_text: String::new() };
        let root = to_js(&self.live().root);
        let script = Script {
            source: ev.script.clone(),
            formcalc: ev.formcalc,
            event,
            root,
            limits: limits_for(&ev.activity),
            timeout: timeout_for(&ev.activity).saturating_mul(self.run.time_scale),
        };
        let o = self.session().run(script);
        if o.abandoned {
            self.run.ran_away = true;
            // The thread is left to the runaway script: nothing more is sent to it.
            self.session = None;
        }
        self.console(o.console.iter().cloned());
        for n in &o.notes {
            self.error(format!("{} ({}): {n}", ev.som, ev.activity));
        }
        // A script that fails keeps what it changed before the error, as JavaScript does: forms
        // often trip over an object an earlier version had, late in a script whose first lines
        // did the work (opening the next fields, showing a subform). An abandoned script, or
        // one the engine failed, reports no changes.
        if let Some(e) = &o.error {
            self.error(format!("{} ({}): {e}", ev.som, ev.activity));
        }
        if self.apply_effects(ev, &o.effects)? {
            self.relayout()?;
        }
        Ok(Some(o))
    }

    /// Every calculate script, in document order: the field takes the script's result unless
    /// the script set a value itself.
    fn calculates(&mut self) -> Result<(), String> {
        let events: Vec<ScriptEvent> = self.live().events.iter().filter(|e| e.activity == "calculate").cloned().collect();
        for ev in &events {
            let Some(o) = self.run(ev, "")? else { continue };
            if o.error.is_some() || o.effects.iter().any(|e| matches!(e, XfaEffect::SetValue { som, .. } if *som == ev.som)) {
                continue;
            }
            if let Some(result) = o.result {
                self.set_value(&ev.som, &result);
            }
        }
        Ok(())
    }

    fn events_of(&mut self, som: &str, activity: &str) -> Vec<ScriptEvent> {
        self.live().events.iter().filter(|e| e.som == som && e.activity == activity).cloned().collect()
    }

    /// End the event: write what is left, lay the form out if a capped relayout is owed, and
    /// report what the limits cut.
    fn finish(mut self) -> Result<Changes, String> {
        #[cfg(test)]
        self.check_live();
        if self.relayouts_skipped {
            self.relayouts = 0;
            self.relayouts_skipped = false;
            self.relayout()?;
        }
        #[cfg(test)]
        if let Some(abandoned) = test_faults::take_finish_failure() {
            self.run.ran_away |= abandoned;
            if abandoned {
                self.error("Injected script abandonment before finalization".into());
            }
            return Err("Injected XFA final flush failure".into());
        }
        self.flush()?;
        self.session = None;
        if self.skipped > 0 {
            let why = if self.run.ran_away {
                "one ran too long".to_string()
            } else if self.runs >= MAX_SCRIPTS_PER_EVENT {
                format!("more than {MAX_SCRIPTS_PER_EVENT} scripts ran")
            } else {
                format!("the scripts took more than {} seconds", self.clock.budget_ms / 1000)
            };
            self.run.out.errors.push(format!("The form's scripts were stopped: {why}; {} more were skipped", self.skipped));
        }
        if self.dropped_output > 0 {
            self.run.out.errors.push(format!("The form's scripts produced too much output; {} more messages were left out", self.dropped_output));
        }
        Ok(self.changes)
    }
}

impl Runner<'_, '_> {
    /// The live form kept up to date in place must be the form a fresh build gives: every
    /// field's SOM, value, presence and access (tests run this before every script and at the
    /// end of every event). A difference is a bug in the in-place refresh, so the test binary
    /// is ended at once: a panic here would be caught by the engine's guard and turned into a
    /// script error the test might not look at.
    #[cfg(test)]
    fn check_live(&mut self) {
        let Some(live) = self.cached.as_ref() else { return };
        let fresh = pdfcraft_xfa::form_tree(self.tpl, self.data.as_ref(), &self.ov);
        let (mut have, mut want) = (Vec::new(), Vec::new());
        collect_fields(&live.root, &mut have, 0);
        collect_fields(&fresh.root, &mut want, 0);
        if have != want {
            let differ: Vec<String> =
                have.iter().zip(want.iter()).filter(|(a, b)| a != b).take(5).map(|(a, b)| format!("live {a:?} vs fresh {b:?}")).collect();
            eprintln!("the live form diverged from a fresh build ({} vs {} fields): {}", have.len(), want.len(), differ.join("; "));
            std::process::abort();
        }
    }
}

/// Every field's (SOM, value, presence, access), in document order.
#[cfg(test)]
fn collect_fields(n: &FormNode, out: &mut Vec<(String, String, String, String)>, depth: usize) {
    if depth > 64 {
        return;
    }
    if n.kind == Some(NodeKind::Field) {
        out.push((n.som.clone(), n.value.clone(), n.presence.clone(), n.access.clone()));
    }
    for c in &n.children {
        collect_fields(c, out, depth + 1);
    }
}

/// Reflow a laid-out XFA form when an edited multiline field can grow. The datasets must
/// already hold the final values, including calculation results. Fixed-height fields keep
/// their existing geometry; forms without scripts parse their template only when needed.
/// `Some` means pages/widgets were rebuilt and the caller must fully refresh its snapshot.
pub(crate) fn relayout_changed_text(
    doc: &mut pdfcraft_cos::Document,
    cached: Option<&Template>,
    before: &[pdfcraft_forms::Field],
) -> Result<Option<Vec<String>>, String> {
    if pdfcraft_xfa::existing_layout(doc).is_none() {
        return Ok(None);
    }
    let current = &*doc;
    let mut changed = before
        .iter()
        .filter(|f| f.kind == pdfcraft_forms::FieldKind::Text && f.has(pdfcraft_forms::flags::MULTILINE))
        .filter_map(|f| {
            let object = current.get(f.obj);
            let dict = object.as_dict()?;
            let value = dict.get(b"V").and_then(|v| current.resolve(v).as_string().map(|s| s.to_text())).unwrap_or_default();
            if f.value.first().map_or("", String::as_str) == value {
                return None;
            }
            dict.get(pdfcraft_xfa::SOM_KEY).and_then(|s| current.resolve(s).as_string().map(|s| s.to_text()))
        })
        .peekable();
    if changed.peek().is_none() {
        return Ok(None);
    }
    let template = match cached {
        Some(tpl) => std::borrow::Cow::Borrowed(tpl),
        None => std::borrow::Cow::Owned(pdfcraft_xfa::template_of(current).map_err(|e| e.to_string())?),
    };
    let reflow = changed.any(|som| pdfcraft_xfa::field_grows_with_text(&template, &som));
    drop(changed);
    if !reflow {
        return Ok(None);
    }
    let report = pdfcraft_xfa::rerender(doc, &template).map_err(|e| e.to_string())?;
    crate::xfa_redraw(doc)?;
    Ok(Some(report.warnings))
}

/// On open: initialize scripts, then calculations. Returns what they changed.
pub(crate) fn on_open(doc: &mut pdfcraft_cos::Document, tpl: &Template, run: &mut XfaRun) -> Result<Changes, String> {
    let mut r = Runner::new(doc, tpl, run, OPEN_BUDGET_MS);
    let events: Vec<ScriptEvent> = r.live().events.iter().filter(|e| e.activity == "initialize" || e.activity == "docReady").cloned().collect();
    for ev in &events {
        r.run(ev, "")?;
    }
    r.calculates()?;
    r.finish()
}

/// After fields `names` took new values: each one's change, exit and validate scripts, then
/// the calculations once (or only those, after a reset). A validate script that answers false,
/// or fails, shows its message; the value stays, as Acrobat marks the field invalid rather
/// than reverting it.
pub(crate) fn on_changes(doc: &mut pdfcraft_cos::Document, tpl: &Template, names: &[String], run: &mut XfaRun) -> Result<(), String> {
    let mut r = Runner::new(doc, tpl, run, OPEN_BUDGET_MS);
    for name in names {
        let som = r.by_som().iter().find(|(_, f)| *f == name).map(|(s, _)| s.clone());
        let Some(som) = som else { continue };
        let value = r.fields().get(name).map(|f| f.value.join("\n")).unwrap_or_default();
        for ev in r.events_of(&som, "change").into_iter().chain(r.events_of(&som, "exit")) {
            r.run(&ev, &value)?;
        }
        for ev in r.events_of(&som, "validate") {
            let Some(o) = r.run(&ev, &value)? else { continue };
            if (o.result_bool == Some(false) || o.error.is_some()) && !value.trim().is_empty() {
                r.alert(crate::js::Message::plain(ev.message.clone().unwrap_or_else(|| format!("{name}: the value is not valid"))));
            }
        }
    }
    r.calculates()?;
    r.finish().map(|_| ())
}

/// The form's data changed outside its scripts (a data file imported, a picture chosen): lay
/// the form out again from the data, then, when `calculate`, run every calculation.
pub(crate) fn on_data_replaced(doc: &mut pdfcraft_cos::Document, tpl: &Template, calculate: bool, run: &mut XfaRun) -> Result<(), String> {
    let mut r = Runner::new(doc, tpl, run, OPEN_BUDGET_MS);
    r.relayout()?;
    if calculate {
        r.calculates()?;
    }
    r.finish().map(|_| ())
}

/// A text value typed into the laid-out XFA date field `name` by another of its edit picture's
/// patterns, written in the first ([`pdfcraft_xfa::typed_date`]); `None` for anything else.
/// `tpl` is the template the scripts use, when they run; otherwise it is read from `doc`.
pub(crate) fn typed_date(
    doc: &pdfcraft_cos::Document,
    tpl: Option<&Template>,
    name: &str,
    value: &pdfcraft_forms::FieldValue,
) -> Option<pdfcraft_forms::FieldValue> {
    let pdfcraft_forms::FieldValue::Text(text) = value else { return None };
    let som = pdfcraft_xfa::fields_by_som(doc).into_iter().find_map(|(som, n)| (n == name).then_some(som))?;
    let read;
    let tpl = match tpl {
        Some(t) => t,
        None => {
            read = pdfcraft_xfa::template_of(doc).ok()?;
            &read
        }
    };
    pdfcraft_xfa::typed_date(tpl, &som, text).map(pdfcraft_forms::FieldValue::Text)
}

/// A button's click script (or any event, by SOM path and activity), then calculations.
pub(crate) fn on_event(doc: &mut pdfcraft_cos::Document, tpl: &Template, som: &str, activity: &str, run: &mut XfaRun) -> Result<(), String> {
    let mut r = Runner::new(doc, tpl, run, EVENT_BUDGET_MS);
    let events = r.events_of(som, activity);
    if events.is_empty() {
        return Err(format!("{som} has no {activity} script"));
    }
    for ev in events {
        r.run(&ev, "")?;
    }
    // The calculations that follow get a budget of their own.
    r.clock = Clock::start(OPEN_BUDGET_MS.saturating_mul(u64::from(r.run.time_scale)));
    r.calculates()?;
    r.finish().map(|_| ())
}

/// The SOM path of a generated field whose template has a click script.
pub(crate) fn clickable_som(doc: &pdfcraft_cos::Document, tpl: &Template, field: &pdfcraft_forms::Field) -> Option<String> {
    // Page-master copies have distinct PDF field names but share a SOM path. Reversing
    // fields_by_som would lose every copy except the last: use the clicked field itself.
    let object = doc.get(field.obj);
    let som = object.as_dict()?.get(pdfcraft_xfa::SOM_KEY).map(|s| doc.resolve(s))?.as_string()?.to_text();
    let live = pdfcraft_xfa::form_tree(tpl, pdfcraft_xfa::data_of(doc).as_ref(), &pdfcraft_xfa::overrides(doc));
    live.events.iter().any(|e| e.som == som && e.activity == "click").then_some(som)
}

/// The parsed template of a laid-out XFA form, kept for the document's life, when it has
/// scripts to run (`None` otherwise: nothing about the form is scripted).
pub(crate) fn template(doc: &pdfcraft_cos::Document) -> Option<Arc<Template>> {
    pdfcraft_xfa::template_of(doc).ok().filter(pdfcraft_xfa::has_scripts).map(Arc::new)
}

/// Push buttons PdfCraft generated for XFA click scripts carry no PDF action (the script is
/// XFA's, not Acrobat JavaScript): give them one that runs the script by the field's SOM path.
pub(crate) fn mark_script_buttons(doc: &pdfcraft_cos::Document, fields: &mut [pdfcraft_forms::Field]) {
    for f in fields.iter_mut() {
        if f.kind == pdfcraft_forms::FieldKind::PushButton
            && f.button.is_none()
            && std::iter::once(f.obj)
                .chain(f.widgets.iter().map(|w| w.obj))
                .any(|r| doc.get(r).as_dict().is_some_and(|d| d.contains(pdfcraft_xfa::CLICK_KEY)))
        {
            f.button = Some(pdfcraft_forms::af::ButtonAction::Script(String::new()));
        }
    }
}

/// Scoped finalization faults avoid timing-dependent tests and abandoned worker threads.
#[cfg(test)]
pub(crate) mod test_faults {
    use std::cell::Cell;

    thread_local! {
        static FINISH_FAILURE: Cell<Option<bool>> = const { Cell::new(None) };
        static EFFECT_WORK: Cell<usize> = const { Cell::new(super::MAX_EFFECT_WORK) };
    }

    /// Keep budget-exhaustion fixtures small enough that wall-clock limits do not race
    /// their work limit when the full suite runs concurrently. Production limits are unchanged.
    pub(crate) struct WorkLimit(usize);

    impl Drop for WorkLimit {
        fn drop(&mut self) {
            EFFECT_WORK.with(|v| v.set(self.0));
        }
    }

    pub(crate) fn limit_effect_work(limit: usize) -> WorkLimit {
        WorkLimit(EFFECT_WORK.with(|v| v.replace(limit)))
    }

    pub(super) fn effect_work_limit() -> usize {
        EFFECT_WORK.with(Cell::get)
    }

    /// Inject one finalization failure on this test thread; optionally model an abandoned
    /// script. Restore the previous setting even if the test panics.
    pub(crate) fn with_finish_failure<T>(abandoned: bool, action: impl FnOnce() -> T) -> T {
        struct Restore(Option<bool>);
        impl Drop for Restore {
            fn drop(&mut self) {
                FINISH_FAILURE.with(|v| v.set(self.0));
            }
        }
        let _restore = Restore(FINISH_FAILURE.with(|v| v.replace(Some(abandoned))));
        let result = action();
        assert!(FINISH_FAILURE.with(|v| v.get().is_none()), "the test must reach finalization");
        result
    }

    /// Consume the fault once, without affecting other tests or subsequent edits.
    pub(super) fn take_finish_failure() -> Option<bool> {
        FINISH_FAILURE.with(Cell::take)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic rebuild count catches amplification without a flaky timing limit.
    #[test]
    fn instance_effect_batch_reuses_live_form() {
        let xml = pdfcraft_xfa::fixtures::optional_rows(0).replace(r#"max="2""#, r#"max="-1""#).replace(
            "<field name=\"other\"",
            &format!("<subform name=\"unrelated\">{}</subform><field name=\"other\"", "<field name=\"unused\"/>".repeat(3000)),
        );
        for fill in [false, true] {
            let (tpl, _) = pdfcraft_xfa::parse(&xml).unwrap();
            let mut doc = pdfcraft_cos::Document::open(Arc::new(pdfcraft_xfa::fixtures::shell(&xml))).unwrap();
            let mut out = JsOutput::default();
            let mut run = XfaRun { page_count: 1, out: &mut out, datasets: None, ran_away: false, time_scale: 1 };
            let mut runner = Runner::new(&mut doc, &tpl, &mut run, 60_000);
            let effects: Vec<_> = (0..200)
                .flat_map(|i| {
                    let som = format!("form[0].row[{i}]");
                    let mut effects = vec![XfaEffect::AddInstance { som: som.clone() }];
                    if fill {
                        effects.push(XfaEffect::SetValue { som: format!("{som}.entry[0]"), value: "filled".into() });
                    }
                    effects
                })
                .collect();
            let ev = ScriptEvent { som: "form[0].add[0]".into(), activity: "click".into(), script: String::new(), formcalc: false, message: None };
            let start = std::time::Instant::now();
            runner.apply_effects(&ev, &effects).unwrap();
            eprintln!("200 additions (fill={fill}) beside 3000 unrelated fields: {:?}; {} live builds", start.elapsed(), runner.live_builds);
            assert_eq!(runner.data.as_ref().unwrap().count(&pdfcraft_xfa::som_to_path("form[0]"), "row"), 200);
            assert_eq!(runner.live_builds, 1, "one batch must not rebuild the unrelated subtree for every row");
            if fill {
                assert_eq!(runner.data.as_ref().unwrap().text_at(&pdfcraft_xfa::som_to_path("form[0].row[199].entry[0]")), Some("filled"));
            }
        }
    }

    /// Many small scripts can consult the same manager in a large document without
    /// paying twice for every unrelated node. A reduced budget keeps the fixture small.
    #[test]
    fn instance_work_counts_rebuilds_once_across_script_batches() {
        let xml = pdfcraft_xfa::fixtures::optional_rows(0).replace(r#"max="2""#, r#"max="-1""#).replace(
            "<field name=\"other\"",
            &format!("<subform name=\"unrelated\">{}</subform><field name=\"other\"", "<field name=\"unused\"/>".repeat(3000)),
        );
        let (tpl, _) = pdfcraft_xfa::parse(&xml).unwrap();
        let mut doc = pdfcraft_cos::Document::open(Arc::new(pdfcraft_xfa::fixtures::shell(&xml))).unwrap();
        let mut out = JsOutput::default();
        let mut run = XfaRun { page_count: 1, out: &mut out, datasets: None, ran_away: false, time_scale: 1 };
        let mut runner = Runner::new(&mut doc, &tpl, &mut run, 60_000);
        runner.effect_work = 40_000;
        runner.live(); // A script obtains its snapshot before replaying its effects.
        let ev = ScriptEvent { som: "form[0].add[0]".into(), activity: "click".into(), script: String::new(), formcalc: false, message: None };
        for i in 0..12 {
            runner.apply_effects(&ev, &[XfaEffect::AddInstance { som: format!("form[0].row[{i}]") }]).unwrap();
        }
        assert_eq!(runner.data.as_ref().unwrap().count(&pdfcraft_xfa::som_to_path("form[0]"), "row"), 12);
        assert_eq!(runner.live_builds, 12);
        assert!(runner.effect_work < 4000, "rebuilding must still consume the cumulative budget");
    }

    /// Host replay checks the native event deadline even when no new script starts.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn effect_replay_checks_the_deadline_before_mutating_data() {
        let xml = pdfcraft_xfa::fixtures::optional_rows(0);
        let (tpl, _) = pdfcraft_xfa::parse(&xml).unwrap();
        let mut doc = pdfcraft_cos::Document::open(Arc::new(pdfcraft_xfa::fixtures::shell(&xml))).unwrap();
        let mut out = JsOutput::default();
        let mut run = XfaRun { page_count: 1, out: &mut out, datasets: None, ran_away: false, time_scale: 1 };
        let mut runner = Runner::new(&mut doc, &tpl, &mut run, 0);
        let ev = ScriptEvent { som: "form[0].add[0]".into(), activity: "click".into(), script: String::new(), formcalc: false, message: None };
        let error = runner.apply_effects(&ev, &[XfaEffect::AddInstance { som: "form[0].row[0]".into() }]).unwrap_err();
        assert!(error.contains("time limit"));
        assert!(runner.pending.is_empty());
        assert_eq!(runner.changes.rows, 0);
    }

    #[test]
    fn script_names_match_indexed_som_paths() {
        assert!(som_matches("form[0].page1[0].qty[0]", "qty"));
        assert!(som_matches("form[0].page1[0].qty[0]", "page1.qty"));
        assert!(som_matches("form[0].page1[0].qty[0]", "xfa.form.form.page1.qty"));
        assert!(som_matches("form[0].page1[0].table[0].row[2].amount[0]", "table"));
        assert!(!som_matches("form[0].page1[0].qty[0]", "qt"));
        assert!(!som_matches("form[0].page1[0].price[0]", "qty"));
        assert_eq!(
            split_instance(&Template::default(), "form[0].table[0].row[2]").map(|(p, ps, n, i)| (p.len(), ps, n, i)),
            Some((2, "form[0].table[0]".into(), "row".into(), 2))
        );
    }
}
