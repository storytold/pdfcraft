//! XFA scripting in the engine: a form's scripted events (initialize and calculate on open,
//! exit, validate and calculate after a field changes, click for buttons) run through
//! [`pdfcraft_js::xfa`] over the live form tree [`pdfcraft_xfa::form_tree`] builds, and what
//! they change or ask for is applied here: values go to the fields (and so to the datasets),
//! presence and access changes become overrides kept in the PDF, added or removed rows change
//! the datasets, and any of those lays the form out again.

use std::collections::{HashMap, HashSet};
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
/// Most runs of one calculation in one event: calculations that keep changing what the other
/// reads stop there.
const MAX_RUNS_PER_CALCULATION: usize = 16;
/// Most calculations whose reads a document keeps: a form that keeps adding rows starts over
/// past it, and every calculation runs once more, as on opening.
pub(crate) const MAX_RECORDED_CALCULATIONS: usize = 50_000;
/// Most field reads a document keeps for all its calculations together: past it, a
/// calculation's reads are not kept and any change concerns it, so a form whose calculations
/// each read thousands of fields stays within memory.
const MAX_RECORDED_READS: usize = 100_000;

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
    /// What each calculation read when it last ran, kept for the document's life.
    pub deps: &'a mut CalcDeps,
}

/// What each calculation read when it last ran (XFA dependency tracking), kept for the
/// document's life: after a change, only the calculations that read what changed run again.
#[derive(Clone, Debug, Default)]
pub(crate) struct CalcDeps {
    /// By calculation (the SOM of its field): the fields it read.
    reads: HashMap<String, Vec<String>>,
    /// By field: the calculations that read it.
    readers: HashMap<String, HashSet<String>>,
    /// Calculations that read more values than one run's log holds, or than the document
    /// keeps: any change concerns them.
    wide: HashSet<String>,
    /// Reads kept in `reads`, against [`MAX_RECORDED_READS`].
    kept: usize,
}

impl CalcDeps {
    /// Record what the calculation of field `calc` read when it ran (`cut`: more than the log
    /// of one run holds).
    fn record(&mut self, calc: &str, reads: Vec<String>, cut: bool) {
        if let Some(old) = self.reads.remove(calc) {
            self.kept = self.kept.saturating_sub(old.len());
            for field in old {
                if let Some(set) = self.readers.get_mut(&field) {
                    set.remove(calc);
                    if set.is_empty() {
                        self.readers.remove(&field);
                    }
                }
            }
        }
        if self.reads.len() >= MAX_RECORDED_CALCULATIONS {
            *self = CalcDeps::default();
        }
        // Too many to keep: the calculation is woken by any change instead.
        let reads = if cut || self.kept.saturating_add(reads.len()) > MAX_RECORDED_READS {
            self.wide.insert(calc.to_string());
            Vec::new()
        } else {
            self.wide.remove(calc);
            reads
        };
        self.kept = self.kept.saturating_add(reads.len());
        for field in &reads {
            self.readers.entry(field.clone()).or_default().insert(calc.to_string());
        }
        self.reads.insert(calc.to_string(), reads);
    }

    /// Whether the calculation of field `calc` has run, so that what it reads is known.
    fn knows(&self, calc: &str) -> bool {
        self.reads.contains_key(calc)
    }

    /// The calculations that read `field`, and those that read too much to tell.
    fn readers_of(&self, field: &str) -> Vec<String> {
        self.readers.get(field).into_iter().flatten().chain(self.wide.iter()).cloned().collect()
    }

    /// A record holding `n` made-up calculations, each having read one field of its own: for
    /// tests that bring a document's record to [`MAX_RECORDED_CALCULATIONS`].
    #[cfg(test)]
    pub(crate) fn filled(n: usize) -> CalcDeps {
        let mut deps = CalcDeps::default();
        for i in 0..n {
            deps.record(&format!("made.up[{i}]"), vec![format!("made.up.read[{i}]")], false);
        }
        deps
    }

    /// The number of calculations recorded (tests).
    #[cfg(test)]
    pub(crate) fn recorded(&self) -> usize {
        self.reads.len()
    }
}

/// The live form node `n`, with its descendants, as the script engine's snapshot of it.
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
        formatted: n.display_picture.as_deref().and_then(|p| pdfcraft_xfa::formatted_value(p, &n.value)),
    }
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
    /// The SOM paths of the fields bound to each data node (built with `by_som`).
    soms_by_path: Option<HashMap<pdfcraft_xfa::DataPath, Vec<String>>>,
    fields: Option<HashMap<String, FieldInfo>>,
    /// The live form as last built; dropped when a script changed the form.
    cached: Option<LiveForm>,
    /// The script engine for this event: one thread for all its scripts, each run in a fresh
    /// context on the form as it is then (`cached`, kept up to date in place for values and
    /// rebuilt when the structure changed).
    session: Option<XfaSession>,
    changes: Changes,
    /// Calculations to run, by the SOM of their field: their inputs changed.
    due: HashSet<String>,
    /// Every calculation is due (on opening, after rows were added or removed, a reset).
    all_due: bool,
    /// Runs of each calculation in this event, against [`MAX_RUNS_PER_CALCULATION`].
    calc_runs: HashMap<String, usize>,
    /// A calculation reached [`MAX_RUNS_PER_CALCULATION`] (reported once).
    calc_capped: bool,
}

impl<'a, 'b> Runner<'a, 'b> {
    /// A runner over `doc` for one event, with `budget_ms` for its scripts; the data and
    /// overrides are read from the document now.
    fn new(doc: &'a mut pdfcraft_cos::Document, tpl: &'a Template, run: &'a mut XfaRun<'b>, budget_ms: u64) -> Self {
        let data = pdfcraft_xfa::data_of(doc);
        let ov = pdfcraft_xfa::overrides(doc);
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
            soms_by_path: None,
            fields: None,
            cached: None,
            session: None,
            changes: Changes::default(),
            due: HashSet::new(),
            all_due: false,
            calc_runs: HashMap::new(),
            calc_capped: false,
        }
    }

    /// The field at `som` was given a value by the script of `by` (`None`: by the user), and
    /// with it every field bound to the same data node. The calculations that read any of
    /// them are due, and so is each one's own calculation when anything else wrote it (another
    /// viewer runs it again then). A calculation's write wakes nobody when it changed nothing,
    /// so calculations that agree don't keep waking each other.
    fn wrote(&mut self, som: &str, by: Option<&ScriptEvent>, changed: bool) {
        if by.is_some_and(|e| e.activity == "calculate") && !changed {
            return;
        }
        let writer = by.map(|e| e.som.clone());
        for field in self.bound_with(som) {
            if writer.as_deref() != Some(field.as_str()) {
                self.due.insert(field.clone());
            }
            for calc in self.run.deps.readers_of(&field) {
                if writer.as_deref() != Some(calc.as_str()) {
                    self.due.insert(calc);
                }
            }
        }
    }

    /// The SOM paths of the fields bound to the same data node as `som`, `som` among them: a
    /// value written to one is every one's ([`Self::set_value`]).
    fn bound_with(&mut self, som: &str) -> Vec<String> {
        let path = pdfcraft_xfa::data_path(self.tpl, som);
        self.by_som();
        let mut fields = self.soms_by_path.as_ref().and_then(|m| m.get(&path)).cloned().unwrap_or_default();
        if !fields.iter().any(|f| f == som) {
            fields.push(som.to_string());
        }
        fields
    }

    /// The live form no longer matches the data (its structure changed): rebuilt for the next
    /// script.
    fn invalidate(&mut self) {
        self.cached = None;
    }

    /// The script engine for this event.
    fn session(&mut self) -> &mut XfaSession {
        let doc_info = XfaDoc { file_name: String::new(), page: 0, page_count: self.run.page_count };
        self.session.get_or_insert_with(|| XfaSession::new(doc_info))
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

    fn live(&mut self) -> &LiveForm {
        if self.cached.is_none() {
            let live = pdfcraft_xfa::form_tree(self.tpl, self.data.as_ref(), &self.ov);
            if live.truncated && !self.truncated_warned {
                self.truncated_warned = true;
                self.error(format!(
                    "The form has more than {} objects or 20000 scripted events; its scripts see only the first ones",
                    pdfcraft_xfa::MAX_FORM_NODES
                ));
            }
            self.cached = Some(live);
        }
        self.cached.get_or_insert_with(Default::default)
    }

    /// The form's field names by SOM path, built once per layout with the indexes by data node.
    fn by_som(&mut self) -> &HashMap<String, String> {
        if self.by_som.is_none() {
            let by_som = pdfcraft_xfa::fields_by_som(self.doc);
            let mut by_path: HashMap<pdfcraft_xfa::DataPath, Vec<String>> = HashMap::new();
            let mut soms_by_path: HashMap<pdfcraft_xfa::DataPath, Vec<String>> = HashMap::new();
            for (som, name) in &by_som {
                let path = pdfcraft_xfa::data_path(self.tpl, som);
                by_path.entry(path.clone()).or_default().push(name.clone());
                soms_by_path.entry(path).or_default().push(som.clone());
            }
            for names in by_path.values_mut() {
                names.sort();
            }
            self.by_path = Some(by_path);
            self.soms_by_path = Some(soms_by_path);
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

    fn console(&mut self, lines: impl IntoIterator<Item = String>) {
        for l in lines {
            if self.run.out.console.len() < MAX_CONSOLE_PER_EVENT {
                self.run.out.console.push(l);
            } else {
                self.dropped_output += 1;
            }
        }
    }

    fn alert(&mut self, m: String) {
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
        self.console(report.warnings);
        crate::xfa_redraw(self.doc)?;
        self.console(crate::undrawn_field_warnings(self.doc));
        self.by_som = None;
        self.by_path = None;
        self.soms_by_path = None;
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

    /// Apply what a script of `ev` did. Returns whether the form has to be laid out again.
    fn apply_effects(&mut self, ev: &ScriptEvent, effects: &[XfaEffect]) -> Result<bool, String> {
        let mut relayout = false;
        // Instances of each repeating subform before the script ran, as the effects leave them.
        let mut counts: HashMap<(String, String), usize> = HashMap::new();
        {
            // A copy of the shared reference: `live()` borrows `self` mutably.
            let tpl = self.tpl;
            let live = self.live();
            for e in effects {
                if let XfaEffect::AddInstance { som } | XfaEffect::RemoveInstance { som } = e
                    && let Some((_, parent_som, name, _)) = split_instance(tpl, som)
                {
                    let have = count_instances(&live.root, &parent_som, &name);
                    counts.entry((parent_som, name)).or_insert(have);
                }
            }
        }
        for e in effects {
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
                    let changed = self.set_value(som, value);
                    self.wrote(som, Some(ev), changed);
                }
                XfaEffect::SetPresence { som, presence } => {
                    if self.ov.presence.insert(som.clone(), presence.clone()).as_ref() != Some(presence) {
                        self.ov_dirty = true;
                        self.changes.visibility += 1;
                        self.invalidate();
                        relayout = true;
                    }
                }
                XfaEffect::SetAccess { som, access } => {
                    if self.ov.access.insert(som.clone(), access.clone()).as_ref() != Some(access) {
                        self.ov_dirty = true;
                        self.changes.visibility += 1;
                        self.invalidate();
                        relayout = true;
                    }
                }
                XfaEffect::AddInstance { som } | XfaEffect::RemoveInstance { som } => {
                    let Some((parent, parent_som, name, index)) = split_instance(self.tpl, som) else { continue };
                    let key = (parent_som, name.clone());
                    let have = counts.get(&key).copied().unwrap_or(0);
                    if matches!(e, XfaEffect::AddInstance { .. }) {
                        counts.insert(key, have + 1);
                        self.data_op(pdfcraft_xfa::DataOp::Instances { parent, name, count: (index + 1).max(have + 1) });
                    } else {
                        // The data holds every shown instance before one is taken out.
                        counts.insert(key, have.saturating_sub(1));
                        self.data_op(pdfcraft_xfa::DataOp::Instances { parent: parent.clone(), name: name.clone(), count: have });
                        let mut path = parent;
                        path.push((name, index));
                        self.data_op(pdfcraft_xfa::DataOp::Remove(path));
                    }
                    self.changes.rows += 1;
                    self.all_due = true;
                    relayout = true;
                }
                XfaEffect::MessageBox(m) => self.alert(m.clone()),
                XfaEffect::ResetData(names) => {
                    self.reset(names)?;
                    self.all_due = true;
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
                // A calculation's own request is already served by the rounds it runs in; taking
                // it would make every calculation, itself included, due again until the cap.
                XfaEffect::Recalculate if ev.activity != "calculate" => self.all_due = true,
                XfaEffect::Recalculate => {}
                XfaEffect::Relayout => relayout = true,
            }
        }
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
            timeout: timeout_for(&ev.activity),
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

    /// The calculations that are due, in document order, round after round until none is:
    /// every one when all are due, else those whose inputs changed and any that never ran. One
    /// that keeps being woken stops after [`MAX_RUNS_PER_CALCULATION`] runs in the event. The
    /// field takes a script's result unless the script set a value itself.
    fn calculates(&mut self) -> Result<(), String> {
        loop {
            let calcs: Vec<ScriptEvent> = self.live().events.iter().filter(|e| e.activity == "calculate").cloned().collect();
            let all = std::mem::take(&mut self.all_due);
            let mut due = Vec::new();
            for ev in calcs {
                if !(all || self.due.contains(&ev.som) || !self.run.deps.knows(&ev.som)) {
                    continue;
                }
                if self.calc_runs.get(&ev.som).copied().unwrap_or(0) >= MAX_RUNS_PER_CALCULATION {
                    self.due.remove(&ev.som);
                    if !self.calc_capped {
                        self.calc_capped = true;
                        self.error(format!(
                            "{} (calculate): the form's calculations kept changing each other; it stopped after {MAX_RUNS_PER_CALCULATION} runs",
                            ev.som
                        ));
                    }
                    continue;
                }
                due.push(ev);
            }
            if due.is_empty() {
                return Ok(());
            }
            let mut spent = false;
            for ev in &due {
                // Woken again while it waited in this round: it runs now, once.
                self.due.remove(&ev.som);
                *self.calc_runs.entry(ev.som.clone()).or_insert(0) += 1;
                let Some(mut o) = self.run(ev, "")? else {
                    spent = true;
                    continue;
                };
                self.run.deps.record(&ev.som, std::mem::take(&mut o.reads), o.reads_cut);
                if o.error.is_some() || o.effects.iter().any(|e| matches!(e, XfaEffect::SetValue { som, .. } if *som == ev.som)) {
                    continue;
                }
                if let Some(result) = o.result {
                    let changed = self.set_value(&ev.som, &result);
                    self.wrote(&ev.som, Some(ev), changed);
                }
            }
            if spent {
                return Ok(());
            }
        }
    }

    /// The `activity` scripts of the object at `som`, in document order.
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

/// On open: initialize scripts, then every calculation (and those that what they wrote made
/// due). Returns what they changed.
pub(crate) fn on_open(doc: &mut pdfcraft_cos::Document, tpl: &Template, run: &mut XfaRun) -> Result<Changes, String> {
    let mut r = Runner::new(doc, tpl, run, OPEN_BUDGET_MS);
    let events: Vec<ScriptEvent> = r.live().events.iter().filter(|e| e.activity == "initialize" || e.activity == "docReady").cloned().collect();
    for ev in &events {
        r.run(ev, "")?;
    }
    r.all_due = true;
    r.calculates()?;
    r.finish()
}

/// After fields `names` took new values: each one's change, exit and validate scripts, then
/// the calculations that read what changed (every one after a `reset`). A validate script
/// that answers false, or fails, shows its message; the value stays, as Acrobat marks the
/// field invalid rather than reverting it.
pub(crate) fn on_changes(doc: &mut pdfcraft_cos::Document, tpl: &Template, names: &[String], reset: bool, run: &mut XfaRun) -> Result<(), String> {
    let mut r = Runner::new(doc, tpl, run, OPEN_BUDGET_MS);
    r.all_due = reset;
    for name in names {
        let som = r.by_som().iter().find(|(_, f)| *f == name).map(|(s, _)| s.clone());
        let Some(som) = som else { continue };
        r.wrote(&som, None, true);
        let value = r.fields().get(name).map(|f| f.value.join("\n")).unwrap_or_default();
        for ev in r.events_of(&som, "change").into_iter().chain(r.events_of(&som, "exit")) {
            r.run(&ev, &value)?;
        }
        for ev in r.events_of(&som, "validate") {
            let Some(o) = r.run(&ev, &value)? else { continue };
            if (o.result_bool == Some(false) || o.error.is_some()) && !value.trim().is_empty() {
                r.alert(ev.message.clone().unwrap_or_else(|| format!("{name}: the value is not valid")));
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
        r.all_due = true;
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

/// A button's click script (or any event, by SOM path and activity), then the calculations
/// that read what it changed.
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
    r.clock = Clock::start(OPEN_BUDGET_MS);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Names scripts pass (`qty`, `page1.qty`, `xfa.form.form.page1.qty`) match the indexed SOM
    /// paths of the objects they name, and nothing else.
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

    /// The record of what calculations read stays bounded however many fields they read: past
    /// [`MAX_RECORDED_READS`] a calculation keeps no reads and any change wakes it; recording a
    /// calculation again replaces what it read; and its old fields stop naming it.
    #[test]
    fn calculation_reads_are_kept_within_a_bound() {
        let fields = |from: usize, n: usize| (from..from + n).map(|i| format!("f{i}")).collect::<Vec<_>>();
        let mut deps = CalcDeps::default();
        let per = 4_000;
        let fit = MAX_RECORDED_READS / per;
        for c in 0..fit {
            deps.record(&format!("c{c}"), fields(c * per, per), false);
        }
        assert_eq!(deps.kept, fit * per);
        assert!(deps.wide.is_empty());
        // One more doesn't fit: it is kept as reading everything, with nothing stored.
        deps.record("over", fields(0, per), false);
        assert_eq!(deps.kept, fit * per, "nothing more was kept");
        assert!(deps.knows("over") && deps.readers_of("unrelated").contains(&"over".to_string()));
        // Recording a calculation again replaces its reads, and frees room.
        deps.record("c0", fields(0, 1), false);
        assert_eq!(deps.kept, (fit - 1) * per + 1);
        assert_eq!(deps.readers_of("f1"), vec!["over".to_string()], "c0 no longer reads f1");
        assert!(deps.readers_of("f0").contains(&"c0".to_string()));
        // A run whose own log was cut reads everything too.
        deps.record("cut", Vec::new(), true);
        assert!(deps.readers_of("anything").contains(&"cut".to_string()));
    }
}
