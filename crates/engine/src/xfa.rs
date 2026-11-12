//! XFA scripting in the engine: a form's scripted events (initialize and calculate on open,
//! exit, validate and calculate after a field changes, click for buttons) run through
//! [`pdfcraft_js::xfa`] over the live form tree [`pdfcraft_xfa::form_tree`] builds, and what
//! they change or ask for is applied here: values go to the fields (and so to the datasets),
//! presence and access changes become overrides kept in the PDF, added or removed rows change
//! the datasets, and any of those lays the form out again.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_js::xfa::{XfaDoc, XfaEffect, XfaEvent, XfaKind, XfaNode, XfaProp, XfaProps};
use pdfcraft_js::xfa_session::{Script, XfaSession};
use pdfcraft_xfa::model::{Prop, Props, Template};
use pdfcraft_xfa::validation_rules as rules;
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
        XfaEffect::OpenAttachment { .. } => Some("open an attached file"),
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
    /// Properties scripts set (colours, borders, fonts, tooltips, validations, occurrences).
    pub properties: usize,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.values == 0 && self.rows == 0 && self.visibility == 0 && self.properties == 0
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
        if self.properties > 0 {
            parts.push(n(self.properties, "changed property", "changed properties"));
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
    /// Whether the viewer highlights form fields, for `app.runtimeHighlight`.
    pub highlight: bool,
    /// What every time limit of the scripts is multiplied by (the event's budget, the engine's
    /// start and each script's own): 1, unless the session was given another
    /// ([`crate::Session::with_xfa_time_scale`]).
    pub time_scale: u32,
}

/// The script engine's copies of a form's property trees, by the address of the tree each was
/// made from: made the first time a script of the form needs one, then shared by every later
/// script and event of that form. They are kept in the form's template
/// ([`pdfcraft_xfa::model::TemplateCache`]), so they go with it when the form is closed, and
/// the lock is the form's own. Each entry holds a weak reference to its tree, which keeps the
/// address from being reused while the entry exists; entries whose tree is gone (a tree of a
/// copy of the template, such as one with a script's overrides applied) are dropped each time
/// the map is used.
type PropsCopies = HashMap<usize, (std::sync::Weak<Props>, Arc<XfaProps>)>;
type PropsCache = std::sync::Mutex<PropsCopies>;

/// The form's map of copies, with the entries of trees that are gone dropped; `None` only when
/// the template's cache holds something else. A poisoned lock only means a panic elsewhere
/// while it was held: the map is still a valid memo.
fn props_copies(tpl: &Template) -> Option<std::sync::MutexGuard<'_, PropsCopies>> {
    let mut copies = tpl.cache.get_or_init(PropsCache::default)?.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    copies.retain(|_, (tree, _)| tree.strong_count() > 0);
    Some(copies)
}

/// The copy made of `p`, if there is one.
fn copy_of(copies: &PropsCopies, p: &Arc<Props>) -> Option<Arc<XfaProps>> {
    let (tree, copy) = copies.get(&(Arc::as_ptr(p) as usize))?;
    (std::sync::Weak::as_ptr(tree) == Arc::as_ptr(p) && tree.strong_count() > 0).then(|| Arc::clone(copy))
}

/// The script engine's copy of the template's property tree `p`.
fn convert_props(p: &Props) -> XfaProps {
    fn prop(p: &Prop, depth: usize) -> XfaProp {
        XfaProp {
            tag: p.tag.clone(),
            attrs: p.attrs.clone(),
            // Shared, not copied: a validation's script can be long.
            text: p.text.clone(),
            // The parser keeps 8 levels; this bounds the walk whatever builds the tree.
            children: if depth > 64 { Vec::new() } else { p.children.iter().map(|c| prop(c, depth + 1)).collect() },
        }
    }
    XfaProps { attrs: p.attrs.clone(), elements: p.elements.iter().map(|e| prop(e, 0)).collect() }
}

/// The live form node `n` of the form whose template is `tpl`, with its descendants, as the
/// script engine's snapshot of it; the property trees are the form's shared copies
/// ([`PropsCopies`]), not copied again. The map is locked only to look copies up and to add the
/// ones made (outside the lock) for trees met for the first time.
fn to_js(n: &FormNode, tpl: &Template) -> XfaNode {
    fn trees<'a>(n: &'a FormNode, out: &mut HashMap<usize, &'a Arc<Props>>, depth: usize) {
        out.entry(Arc::as_ptr(&n.props) as usize).or_insert(&n.props);
        if depth <= 256 {
            n.children.iter().for_each(|c| trees(c, out, depth + 1));
        }
    }
    let mut needed = HashMap::new();
    trees(n, &mut needed, 0);
    let mut found: HashMap<usize, Arc<XfaProps>> = HashMap::with_capacity(needed.len());
    let missing: Vec<&Arc<Props>> = {
        let copies = props_copies(tpl);
        needed
            .iter()
            .filter_map(|(&key, p)| match copies.as_ref().and_then(|c| copy_of(c, p)) {
                Some(copy) => {
                    found.insert(key, copy);
                    None
                }
                None => Some(*p),
            })
            .collect()
    };
    if !missing.is_empty() {
        let made: Vec<(&Arc<Props>, Arc<XfaProps>)> = missing.into_iter().map(|p| (p, Arc::new(convert_props(p)))).collect();
        let mut copies = props_copies(tpl);
        for (p, copy) in made {
            let key = Arc::as_ptr(p) as usize;
            // Another thread may have made one meanwhile: keep the first.
            let copy = match copies.as_mut() {
                Some(c) => copy_of(c, p).unwrap_or_else(|| {
                    c.insert(key, (Arc::downgrade(p), Arc::clone(&copy)));
                    copy
                }),
                None => copy,
            };
            found.insert(key, copy);
        }
    }
    to_js_with(n, &found, 0)
}

fn to_js_with(n: &FormNode, copies: &HashMap<usize, Arc<XfaProps>>, depth: usize) -> XfaNode {
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
        children: if depth > 256 { Vec::new() } else { n.children.iter().map(|c| to_js_with(c, copies, depth + 1)).collect() },
        props: copies.get(&(Arc::as_ptr(&n.props) as usize)).cloned().unwrap_or_else(|| Arc::new(convert_props(&n.props))),
        formatted: n.display_picture.as_deref().and_then(|p| pdfcraft_xfa::formatted_value(p, &n.value)),
        changed: Arc::from([]),
    }
}

/// Give each node of the snapshot `n` the keys of the property changes `ov` keeps for it
/// ([`XfaNode::changed`]), so the script engine leaves out a write the form would refuse.
fn mark_changed(n: &mut XfaNode, ov: &pdfcraft_xfa::Overrides, depth: usize) {
    if ov.props().is_empty() {
        return;
    }
    if let Some(sets) = ov.props().get(&n.som) {
        n.changed = sets.keys().cloned().collect();
    }
    if depth <= 256 {
        n.children.iter_mut().for_each(|c| mark_changed(c, ov, depth + 1));
    }
}

/// The names of a run of `resetData` calls, applied together: each name once, in the order
/// first given. A call without names resets the whole form, which takes in every other.
#[derive(Default)]
struct ResetRun {
    names: Vec<String>,
    /// `names` as a set, so adding a name costs the same however many there are (a script
    /// may pass hundreds of thousands).
    seen: HashSet<String>,
    whole: bool,
}

impl ResetRun {
    fn add(&mut self, names: &[String]) {
        if self.whole {
            return;
        }
        if names.is_empty() {
            self.whole = true;
            self.names.clear();
            self.seen.clear();
            return;
        }
        for n in names {
            if self.seen.insert(n.clone()) {
                self.names.push(n.clone());
            }
        }
    }

    /// The names to reset; none for the whole form.
    fn names(&self) -> &[String] {
        if self.whole { &[] } else { &self.names }
    }
}

/// The SOM paths of the live form's fields under `n`, at most 64 levels deep, into `out`.
fn field_soms(n: &FormNode, out: &mut HashSet<String>, depth: usize) {
    if depth > 64 {
        return;
    }
    if n.kind == Some(NodeKind::Field) {
        out.insert(n.som.clone());
    }
    for c in &n.children {
        field_soms(c, out, depth + 1);
    }
}

/// The subform or other container at SOM path `som` in the live form under `n` (fields are
/// skipped), searched at most 64 levels deep.
fn find_container<'a>(n: &'a FormNode, som: &str, depth: usize) -> Option<&'a FormNode> {
    if depth > 64 {
        return None;
    }
    if n.som == som && n.kind != Some(NodeKind::Field) {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_container(c, som, depth + 1))
}

fn has_node(n: &FormNode, som: &str, depth: usize) -> bool {
    depth <= 64 && (n.som == som || n.children.iter().any(|c| has_node(c, som, depth + 1)))
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

/// The final layout index, which can differ from the index of the bound data node.
fn indexed_som_tail(som: &str) -> Option<(&str, usize)> {
    let (prefix, index) = som.strip_suffix(']')?.rsplit_once('[')?;
    Some((prefix, index.parse().ok()?))
}

/// Adjust a pending focus when a sibling is deleted and its later instances are renumbered.
fn shift_focus_after_removal(target: &mut Option<String>, prefix: &str, removed: usize) {
    let Some((index, suffix)) = target
        .as_deref()
        .and_then(|som| som.strip_prefix(prefix)?.strip_prefix('[')?.split_once(']'))
        .and_then(|(i, tail)| Some((i.parse::<usize>().ok()?, tail)))
    else {
        return;
    };
    if index == removed {
        *target = None;
    } else if index > removed {
        *target = Some(format!("{prefix}[{}]{suffix}", index - 1));
    }
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

/// The names a script passed to a reset (`qty`, `page1.qty`, `xfa.form.form.page1.qty`, or a
/// container such as `table`), indexed, so matching an object against them costs the same
/// however many names there are.
struct ResetTargets {
    /// Each name's steps without indexes, joined by `.`.
    wants: HashSet<String>,
    /// The most steps of a name.
    longest: usize,
}

impl ResetTargets {
    fn new(names: &[String]) -> Self {
        let mut wants = HashSet::new();
        let mut longest = 0;
        for name in names {
            let n = plain_som(name.trim());
            let n = n.strip_prefix("xfa.form.").or_else(|| n.strip_prefix("$form.")).unwrap_or(&n);
            let steps: Vec<&str> = n.split('.').filter(|s| !s.is_empty()).collect();
            if !steps.is_empty() {
                longest = longest.max(steps.len());
                wants.insert(steps.join("."));
            }
        }
        ResetTargets { wants, longest }
    }

    /// Does a name name this SOM or one of its ancestors: is it a run of the SOM's steps? The
    /// runs are looked up, at most [`Self::longest`] steps each, so the work depends on the
    /// SOM's depth and not on how many names there are.
    fn matches(&self, som: &str) -> bool {
        let plain = plain_som(som);
        let have: Vec<&str> = plain.split('.').filter(|s| !s.is_empty()).collect();
        for start in 0..have.len() {
            let mut run = String::new();
            for (k, step) in have.iter().skip(start).take(self.longest).enumerate() {
                if k > 0 {
                    run.push('.');
                }
                run.push_str(step);
                if self.wants.contains(&run) {
                    return true;
                }
            }
        }
        false
    }
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
    /// Lay the form out once, when the event ends, rather than after every script that changed
    /// its structure (opening: see [`on_open`]).
    defer_relayout: bool,
    /// A deferred layout is owed.
    relayout_owed: bool,
    dropped_output: usize,
    truncated_warned: bool,
    data: Option<pdfcraft_xfa::DataNode>,
    ov: pdfcraft_xfa::Overrides,
    ov_dirty: bool,
    /// Scripts changed properties since the form was last laid out: it is laid out once more
    /// when the event ends (properties change how the form looks, which later scripts don't
    /// read, so one layout serves every change of the event).
    restyle_pending: bool,
    /// A property change was refused because the form holds as many as it may (noted once).
    props_full: bool,
    pending: Vec<pdfcraft_xfa::DataOp>,
    /// Field values to write, by name (the last wins).
    field_changes: std::collections::BTreeMap<String, Vec<String>>,
    by_som: Option<HashMap<String, String>>,
    objects_by_som: HashMap<String, pdfcraft_cos::ObjRef>,
    focus_requests: Vec<(usize, Option<String>)>,
    /// Widget names by the data node they bind to (built with `by_som`).
    by_path: Option<HashMap<pdfcraft_xfa::DataPath, Vec<String>>>,
    fields: Option<HashMap<String, FieldInfo>>,
    /// The live form as last built; dropped when a script changed the form.
    cached: Option<LiveForm>,
    /// `cached` as the scripts get it, made once and sent to each script until the live form
    /// changes in any way (a value too): the script engine reuses what it built from it.
    js_root: Option<Arc<XfaNode>>,
    /// Property changes made since `js_root` was built, handed to each later script with it, so
    /// a write needs no new copy of the form (at most [`MAX_PENDING_PROPS`]; past them the copy
    /// is made again).
    pending_props: Vec<pdfcraft_js::xfa::XfaPropSet>,
    /// The script engine for this event: one thread for all its scripts, each run in a fresh
    /// context on the form as it is then (`cached`, kept up to date in place for values and
    /// rebuilt when the structure changed).
    session: Option<XfaSession>,
    changes: Changes,
    /// Fields the layout numbers (a field whose own layout:ready script puts a page number in
    /// it): while layout:ready scripts run, a value written to one is dropped, so each page keeps
    /// the number the layout gave it.
    numbered: HashSet<String>,
    /// A field embedded in rich text (a floating field) took a new value since the form was
    /// last laid out: the text that shows it is laid out again when the event ends.
    embedded_changed: bool,
}

impl<'a, 'b> Runner<'a, 'b> {
    /// A runner for one event on `doc` and its template, its scripts sharing `budget_ms`.
    fn new(doc: &'a mut pdfcraft_cos::Document, tpl: &'a Template, run: &'a mut XfaRun<'b>, budget_ms: u64) -> Self {
        let data = pdfcraft_xfa::data_of(doc);
        let ov = pdfcraft_xfa::overrides_of(doc, tpl);
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
            defer_relayout: false,
            relayout_owed: false,
            dropped_output: 0,
            truncated_warned: false,
            data,
            ov,
            ov_dirty: false,
            restyle_pending: false,
            props_full: false,
            pending: Vec::new(),
            field_changes: Default::default(),
            by_som: None,
            objects_by_som: HashMap::new(),
            focus_requests: Vec::new(),
            by_path: None,
            fields: None,
            cached: None,
            js_root: None,
            pending_props: Vec::new(),
            session: None,
            changes: Changes::default(),
            numbered: HashSet::new(),
            embedded_changed: false,
        }
    }

    /// The live form no longer matches the data (its structure changed): rebuilt for the next
    /// script.
    fn invalidate(&mut self) {
        self.cached = None;
        self.js_root = None;
    }

    /// A property change for the scripts that follow in this event: with the copy of the form
    /// they are given, rather than in a new copy of it, up to [`MAX_PENDING_PROPS`].
    fn pend_prop(&mut self, som: &str, path: &str, attr: Option<&str>, value: &str, copied: bool) {
        if self.js_root.is_none() {
            return;
        }
        if self.pending_props.len() >= MAX_PENDING_PROPS {
            self.js_root = None;
            return;
        }
        self.pending_props.push(pdfcraft_js::xfa::XfaPropSet {
            som: som.to_string(),
            path: path.to_string(),
            attr: attr.map(str::to_string),
            value: value.to_string(),
            copied,
        });
    }

    /// The script engine for this event.
    fn session(&mut self) -> &mut XfaSession {
        let doc_info = XfaDoc { file_name: String::new(), page: 0, page_count: self.run.page_count, highlight: self.run.highlight };
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
        // The value changes in the live form below: the scripts' copy is made again.
        self.js_root = None;
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
            let uses = pdfcraft_xfa::page_area_uses(self.doc, self.tpl);
            let live = pdfcraft_xfa::form_tree_paged(self.tpl, self.data.as_ref(), &self.ov, &uses);
            #[cfg(test)]
            counts::bump(&counts::TREES);
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

    fn by_som(&mut self) -> &HashMap<String, String> {
        if self.by_som.is_none() {
            let mut by_som = HashMap::new();
            self.objects_by_som.clear();
            for (som, (name, object)) in pdfcraft_xfa::field_objects_by_som(self.doc) {
                self.objects_by_som.insert(som.clone(), object);
                by_som.insert(som, name);
            }
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
        // A floating field shows the value of its data node in the text that embeds it (XFA 3.3
        // pp. 221–222): a page area's, or one another field shares, as well.
        let embedded = pdfcraft_xfa::path_is_embedded(self.tpl, &path);
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
            self.embedded_changed |= embedded;
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
        self.restyle_pending = false;
        self.flush()?;
        self.embedded_changed = false;
        #[cfg(test)]
        counts::bump(&counts::LAYOUTS);
        let report = pdfcraft_xfa::rerender(self.doc, self.tpl).map_err(|e| e.to_string())?;
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
        let targets = (!names.is_empty()).then(|| ResetTargets::new(names));
        let wanted = |som: &str| targets.as_ref().is_none_or(|t| t.matches(som));
        let listed: Option<Vec<String>> = targets.as_ref().map(|_| by_som.iter().filter(|(som, _)| wanted(som)).map(|(_, f)| f.clone()).collect());
        // Fields and exclusion groups of the live form with no widget yet (a row an opening
        // script added, before the form is laid out again): their data goes back to the
        // template's default too, or the value would stay and be laid out and saved.
        let unshown: Vec<String> = {
            let live = self.live();
            let mut out = Vec::new();
            let mut stack: Vec<&pdfcraft_xfa::FormNode> = vec![&live.root];
            while let Some(n) = stack.pop() {
                if matches!(n.kind, Some(pdfcraft_xfa::NodeKind::Field | pdfcraft_xfa::NodeKind::ExclGroup))
                    && !by_som.contains_key(&n.som)
                    && wanted(&n.som)
                {
                    out.push(n.som.clone());
                }
                stack.extend(n.children.iter());
            }
            out
        };
        for som in unshown {
            // A floating field rich text embeds goes back to its default with every row that
            // holds it (below).
            if pdfcraft_xfa::field_is_embedded(self.tpl, &som) {
                continue;
            }
            let Some(text) = pdfcraft_xfa::reset_default(self.tpl, &som) else { continue };
            let path = pdfcraft_xfa::data_path(self.tpl, &som);
            self.reset_data(path, text);
        }
        // Names no widget answers to may still name a floating field (below).
        let widgets = !listed.as_ref().is_some_and(Vec::is_empty);
        if widgets {
            pdfcraft_forms::reset(self.doc, listed.as_deref()).map_err(|e| e.to_string())?;
        }
        self.fields = None;
        let infos = self.fields().clone();
        let listed: Option<HashSet<&String>> = listed.as_ref().map(|l| l.iter().collect());
        for (som, name) in by_som.iter().filter(|_| widgets) {
            if listed.as_ref().is_some_and(|l| !l.contains(name)) {
                continue;
            }
            let Some(info) = infos.get(name) else { continue };
            if matches!(info.kind, pdfcraft_forms::FieldKind::PushButton | pdfcraft_forms::FieldKind::Signature) {
                continue;
            }
            let path = pdfcraft_xfa::data_path(self.tpl, som);
            self.reset_data(path, data_text(info));
        }
        // A floating field without a widget (a hidden one, as forms place them) goes back to
        // its default too, and the text that embeds it shows that.
        for (som, default) in pdfcraft_xfa::embedded_fields(self.tpl) {
            if by_som.contains_key(&som) || !wanted(&som) {
                continue;
            }
            // Every row that holds it, not only the first.
            let names: Vec<String> = pdfcraft_xfa::data_path(self.tpl, &som).into_iter().map(|(name, _)| name).collect();
            let paths = self.data.as_ref().map(|d| d.paths_named(&names, pdfcraft_xfa::MAX_FORM_NODES)).unwrap_or_default();
            for path in paths {
                if self.data.as_ref().and_then(|d| d.text_at(&path)).unwrap_or("") != default {
                    self.embedded_changed = true;
                    self.data_op(pdfcraft_xfa::DataOp::Value { path, text: default.clone() });
                    self.changes.values += 1;
                }
            }
        }
        self.invalidate();
        Ok(())
    }

    /// Put a reset's `text` in the data at `path`, unless it already holds it: a multi-select
    /// list's picks (`value` elements) are compared as its lines (the write replaces them).
    fn reset_data(&mut self, path: pdfcraft_xfa::DataPath, text: String) {
        if self.data.as_ref().and_then(|d| d.value_at(&path)).unwrap_or_default() != text {
            self.embedded_changed |= pdfcraft_xfa::path_is_embedded(self.tpl, &path);
            self.data_op(pdfcraft_xfa::DataOp::Value { path, text });
            self.changes.values += 1;
        }
    }

    /// A property of the object at `som` set by a script of `ev`: kept in the overrides (and so
    /// in the file, and in the undo step), the live form refreshed for the next script, and
    /// the form laid out again when the event ends. A value the property can't take (a colour
    /// that isn't one, a measurement that doesn't read) is refused with a console note, and the
    /// property keeps what it had; past the form's limit on changes, one note says so.
    fn set_property(&mut self, ev: &ScriptEvent, som: &str, path: &str, attr: Option<&str>, value: &str, copied: bool) {
        let kept = if copied { self.ov.set_prop_copy(som, path, attr, value) } else { self.ov.set_prop(som, path, attr, value) };
        match kept {
            Ok(false) => {}
            // A value the host copied from a drawn edge is no change of the script's: not counted,
            // and one the form can't take (the template's own odd value) is left out quietly.
            Err(pdfcraft_xfa::PropRefused::Invalid(_)) if copied => {}
            Ok(true) => {
                self.ov_dirty = true;
                // Metadata, named values and screen-reader text are kept for scripts; the
                // pages don't show them, so they need no layout.
                self.restyle_pending |= pdfcraft_xfa::restyle::draws(path);
                if !copied {
                    self.changes.properties += 1;
                }
                // How often a subform occurs, or a validation script turned off, changes the
                // instance managers and events, and a field's items, pictures or value element
                // change the value scripts read (a radio button's on value): the tree is rebuilt
                // for those.
                let structural = path.starts_with("occur#")
                    || (path.starts_with("validate#") && attr == Some("scriptTest"))
                    || pdfcraft_xfa::restyle::changes_live_value(path);
                // A later script in this event reads the change with the copy of the form it is
                // given, which needs no copying again.
                self.pend_prop(som, path, attr, value, copied);
                let refreshed = !structural
                    && match self.cached.as_mut() {
                        Some(live) => pdfcraft_xfa::refresh_props(&mut live.root, &self.ov, som),
                        None => true,
                    };
                if !refreshed {
                    self.invalidate();
                }
            }
            Err(pdfcraft_xfa::PropRefused::Invalid(why)) => {
                let what = attr.map_or_else(|| path.to_string(), |a| format!("{path} {a}"));
                self.console([format!("{} ({}): {som}: {what} not set: {why}", ev.som, ev.activity)]);
            }
            Err(pdfcraft_xfa::PropRefused::Full) => {
                if !self.props_full {
                    self.props_full = true;
                    self.error(format!(
                        "The form's scripts changed more than {} properties; later changes are left out",
                        pdfcraft_xfa::restyle::MAX_PROP_OVERRIDES
                    ));
                }
            }
        }
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
        // Resets in a row, applied together when the run of them ends.
        let mut pending_reset: Option<ResetRun> = None;
        for (at, e) in effects.iter().enumerate() {
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
                    if !self.numbered.contains(som) {
                        self.set_value(som, value);
                    }
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
                XfaEffect::SetProperty { som, path, attr, value, copied } => self.set_property(ev, som, path, attr.as_deref(), value, *copied),
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
                        // SOM indexes after the removed instance shift down. A focus on
                        // the deleted instance itself must never reach its replacement.
                        if let Some((prefix, removed)) = indexed_som_tail(som) {
                            for (_, target) in &mut self.focus_requests {
                                shift_focus_after_removal(target, prefix, removed);
                            }
                        }
                        let mut path = parent;
                        path.push((name, index));
                        self.data_op(pdfcraft_xfa::DataOp::Remove(path));
                        // What scripts changed on the removed instance goes with it; the later
                        // instances' changes move down with them.
                        if self.ov.remove_instance(som) {
                            self.ov_dirty = true;
                        }
                    }
                    self.changes.rows += 1;
                    relayout = true;
                }
                XfaEffect::MessageBox(m) => self.alert(m.clone()),
                XfaEffect::ResetData(names) => {
                    // A script resetting several subforms in a row (`resetData` once for each)
                    // resets them together: the same fields end up at their defaults.
                    pending_reset.get_or_insert_with(ResetRun::default).add(names);
                    let next_is_reset = effects.get(at.saturating_add(1)).is_some_and(|e| matches!(e, XfaEffect::ResetData(_)));
                    if !next_is_reset && let Some(run) = pending_reset.take() {
                        self.reset(run.names())?;
                    }
                }
                XfaEffect::Print => self.run.out.requests.push(Request::Print),
                XfaEffect::SaveAs => self.run.out.requests.push(Request::SaveAs),
                XfaEffect::LaunchUrl(u) => self.run.out.requests.push(Request::LaunchUrl(u.clone())),
                XfaEffect::OpenAttachment { name, open } => self.run.out.requests.push(Request::OpenAttachment { name: name.clone(), open: *open }),
                XfaEffect::ShowPanel(p) => self.run.out.requests.push(Request::ShowPanel(*p)),
                XfaEffect::SetFocus(som) => {
                    self.focus_requests.push((self.run.out.requests.len(), Some(som.clone())));
                    self.run.out.requests.push(Request::Focus(String::new()));
                }
                XfaEffect::Beep => self.run.out.requests.push(Request::Beep),
                XfaEffect::Highlight(on) => {
                    self.run.highlight = *on;
                    // The session's scripts read the setting it started with: the next script
                    // gets a session that reads the new one.
                    self.session = None;
                    self.run.out.requests.push(Request::Highlight(*on));
                }
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
        // An event listed before a script relaid the form out can name an object that is gone
        // (a page the form no longer fills, a row removed): it isn't run, rather than run with
        // `this` at some other object.
        if !has_node(&self.live().root, &ev.som, 0) {
            return Ok(None);
        }
        self.runs += 1;
        #[cfg(test)]
        self.check_live();
        let event = XfaEvent { activity: ev.activity.clone(), target: ev.som.clone(), new_text: new_text.to_string(), prev_text: String::new() };
        let tpl = self.tpl;
        let root = match &self.js_root {
            Some(r) => Arc::clone(r),
            None => {
                self.pending_props.clear();
                #[cfg(test)]
                counts::bump(&counts::SNAPSHOTS);
                let mut r = to_js(&self.live().root, tpl);
                mark_changed(&mut r, &self.ov, 0);
                let r = Arc::new(r);
                self.js_root = Some(Arc::clone(&r));
                r
            }
        };
        let script = Script {
            source: ev.script.clone(),
            formcalc: ev.formcalc,
            event,
            root,
            limits: limits_for(&ev.activity),
            timeout: timeout_for(&ev.activity).saturating_mul(self.run.time_scale),
            props: self.pending_props.clone(),
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
            if self.defer_relayout {
                self.relayout_owed = true;
            } else {
                self.relayout()?;
            }
        }
        Ok(Some(o))
    }

    /// Every calculate script, in document order: the field takes the script's result unless
    /// the script set a value itself.
    fn calculates(&mut self) -> Result<(), String> {
        let events: Vec<ScriptEvent> = self.live().events.iter().filter(|e| e.activity == "calculate").cloned().collect();
        for ev in &events {
            // A field whose calculation the user overrode keeps the user's value.
            if pdfcraft_xfa::value_overridden(self.tpl, &self.ov, &ev.som) {
                continue;
            }
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
        if self.relayouts_skipped || self.relayout_owed || self.embedded_changed || self.restyle_pending {
            self.relayouts = 0;
            self.relayouts_skipped = false;
            self.relayout_owed = false;
            self.relayout()?;
        }
        self.flush()?;
        if !self.focus_requests.is_empty() {
            self.by_som();
            for (index, som) in self.focus_requests.into_iter().rev() {
                if let Some((name, object)) = som.as_ref().and_then(|som| self.by_som.as_ref()?.get(som).zip(self.objects_by_som.get(som))) {
                    self.run.out.requests[index] = Request::FocusField { name: name.clone(), object_id: crate::js::field_object_id(*object) };
                } else {
                    self.run.out.requests.remove(index);
                }
            }
        }
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
        let uses = pdfcraft_xfa::page_area_uses(self.doc, self.tpl);
        let fresh = pdfcraft_xfa::form_tree_paged(self.tpl, self.data.as_ref(), &self.ov, &uses);
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

/// Reflow a laid-out XFA form when an edited multiline field can grow, or an edited text field,
/// choice list or check box is embedded in rich text (a floating field, whose value that text
/// shows). The datasets must
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
        .filter(|f| {
            use pdfcraft_forms::FieldKind as K;
            matches!(f.kind, K::Text | K::Combo | K::List | K::CheckBox)
        })
        .filter_map(|f| {
            let object = current.get(f.obj);
            let dict = object.as_dict()?;
            if value_unchanged(current, f, dict.get(b"V")) {
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
    let reflow = changed.any(|som| pdfcraft_xfa::field_grows_with_text(&template, &som) || pdfcraft_xfa::field_is_embedded(&template, &som));
    drop(changed);
    if !reflow {
        return Ok(None);
    }
    let report = pdfcraft_xfa::rerender(doc, &template).map_err(|e| e.to_string())?;
    crate::xfa_redraw(doc)?;
    Ok(Some(report.warnings))
}

/// Whether field `f` still holds the value it had (`f.value`), its `/V` being `v`: a list box's
/// picks are an array, a check box's state a name (Off for none, as the field's value has
/// none), and of a text field only the first value counts. Compared in place, so an edit costs
/// no copy of every field's value.
fn value_unchanged(doc: &pdfcraft_cos::Document, f: &pdfcraft_forms::Field, v: Option<&pdfcraft_cos::Object>) -> bool {
    use pdfcraft_cos::Object as O;
    let text = f.kind == pdfcraft_forms::FieldKind::Text;
    let first = f.value.first().map_or("", String::as_str);
    // The value is the one string `is` accepts.
    let one = |is: &dyn Fn(&str) -> bool| if text { is(first) } else { matches!(f.value.as_slice(), [old] if is(old)) };
    let none = if text { first.is_empty() } else { f.value.is_empty() };
    let held;
    let v = match v {
        Some(r @ O::Ref(_)) => {
            held = doc.resolve(r);
            Some(&*held)
        }
        v => v,
    };
    match v {
        Some(O::Array(items)) => {
            let mut old = f.value.iter();
            for item in items {
                let held_item;
                let item = match item {
                    O::Ref(_) => {
                        held_item = doc.resolve(item);
                        &*held_item
                    }
                    i => i,
                };
                let Some(s) = item.as_string() else { continue };
                if text {
                    return s.text_eq(first);
                }
                if !old.next().is_some_and(|o| s.text_eq(o)) {
                    return false;
                }
            }
            if text { none } else { old.next().is_none() }
        }
        Some(O::Name(n)) if n.as_slice() == b"Off" => none,
        Some(O::Name(n)) => {
            let n = String::from_utf8_lossy(n);
            one(&|old| old == n)
        }
        Some(v) => match v.as_string() {
            Some(s) => one(&|old| s.text_eq(old)),
            None => none,
        },
        None => none,
    }
}

/// On open: initialize scripts, then layout:ready scripts (the form is laid out by now), then
/// calculations. Returns what they changed.
///
/// layout:ready runs once here, where forms set themselves up (turning field highlighting on,
/// say); a viewer runs it after every layout. Every layout:ready script runs, but a field that
/// numbers pages keeps the number the layout gave each page: the value a script writes to it is
/// dropped (one value would put the same number on every page).
/// Work an event does, counted for tests: the live form built, the copies of it made for the
/// scripts, the layouts. Timing-independent, so tests can say the work doesn't grow with the
/// number of writes.
#[cfg(test)]
pub(crate) mod counts {
    use std::cell::Cell;
    thread_local! {
        pub(crate) static TREES: Cell<usize> = const { Cell::new(0) };
        pub(crate) static SNAPSHOTS: Cell<usize> = const { Cell::new(0) };
        pub(crate) static LAYOUTS: Cell<usize> = const { Cell::new(0) };
    }
    pub(crate) fn bump(c: &'static std::thread::LocalKey<Cell<usize>>) {
        c.with(|c| c.set(c.get().saturating_add(1)));
    }
    /// (trees, snapshots, layouts) since the last call, and start again.
    pub(crate) fn take() -> (usize, usize, usize) {
        (TREES.with(|c| c.replace(0)), SNAPSHOTS.with(|c| c.replace(0)), LAYOUTS.with(|c| c.replace(0)))
    }
}

/// Most property changes handed to a script with the copy of the form it is given; past them
/// the copy is made again. A script's start copies the form once and goes through these once,
/// so this keeps a start's work within a copy's.
const MAX_PENDING_PROPS: usize = 4_096;

pub(crate) fn on_open(doc: &mut pdfcraft_cos::Document, tpl: &Template, run: &mut XfaRun) -> Result<Changes, String> {
    let mut r = Runner::new(doc, tpl, run, OPEN_BUDGET_MS);
    // The scripts read the live form (template, data and overrides), not the pages, so the
    // pages are laid out once, after the last of them. Laying a large form out after every
    // script that hides a subform spent the time limit on the first few, and the form was kept
    // half initialized: a page its initialize scripts hide stayed open to typing until the
    // next opening hid it and reset its data.
    r.defer_relayout = true;
    let events: Vec<ScriptEvent> = r.live().events.iter().filter(|e| e.activity == "initialize" || e.activity == "docReady").cloned().collect();
    for ev in &events {
        r.run(ev, "")?;
    }
    let (ready, numbered) = {
        let live = r.live();
        let ready: Vec<ScriptEvent> = live.events.iter().filter(|e| e.activity == "layoutReady").cloned().collect();
        // The fields, gathered in one walk rather than one per script.
        let mut fields = HashSet::new();
        field_soms(&live.root, &mut fields, 0);
        let numbered: HashSet<String> =
            ready.iter().filter(|e| fields.contains(&e.som) && pdfcraft_xfa::numbers_pages(&e.script, e.formcalc)).map(|e| e.som.clone()).collect();
        (ready, numbered)
    };
    r.numbered = numbered;
    for ev in &ready {
        r.run(ev, "")?;
    }
    r.numbered.clear();
    r.calculates()?;
    r.finish()
}

/// After fields `names` took new values: each one's change, exit and validate scripts, then
/// the calculations once (or only those, after a reset). A validate script that answers false,
/// or fails, shows its message; the value stays, as Acrobat marks the field invalid rather
/// than reverting it. The SOM paths whose exit scripts ran go into `exited`.
pub(crate) fn on_changes(
    doc: &mut pdfcraft_cos::Document,
    tpl: &Template,
    names: &[String],
    run: &mut XfaRun,
    exited: &mut Vec<String>,
) -> Result<(), String> {
    let mut r = Runner::new(doc, tpl, run, OPEN_BUDGET_MS);
    for name in names {
        let som = r.by_som().iter().find(|(_, f)| *f == name).map(|(s, _)| s.clone());
        let Some(som) = som else { continue };
        let value = r.fields().get(name).map(|f| f.value.join("\n")).unwrap_or_default();
        let field = pdfcraft_xfa::field_now(r.tpl, &r.ov, &som);
        // A value typed into a calculated field: kept in place of the calculation, and the user
        // asked, when its override says so (see `validation_rules::user_value_overrides`).
        if let Some(f) = field.as_ref().filter(|f| f.calculate.is_some())
            && !pdfcraft_xfa::value_overridden(r.tpl, &r.ov, &som)
            && let Some(m) = rules::user_value_overrides(f.calc_override, f.calc_message.as_deref(), name)
        {
            r.run.out.requests.push(Request::CalculationOverridden(m));
            if pdfcraft_xfa::mark_value_override(&mut r.ov, &som) {
                r.ov_dirty = true;
            }
        }
        for ev in r.events_of(&som, "change") {
            r.run(&ev, &value)?;
        }
        // Noted only when an exit script ran, whether it succeeded or failed: it has run for this
        // focus loss, and a failed script keeps what it changed before its error, so running it
        // again would repeat that and the error. One the event's budget skipped, or one never
        // reached because an earlier script stopped the run, is still to run when the field
        // loses focus.
        let mut ran_exit = false;
        for ev in r.events_of(&som, "exit") {
            ran_exit |= r.run(&ev, &value)?.is_some();
        }
        if ran_exit {
            exited.push(som.clone());
        }
        // The format test, then the script test (XFA 3.3 p. 382), when the user leaves the
        // field (p. 396); the value stays as typed. The field as its change and exit scripts
        // left it: one may have set the test, its picture or its message.
        let value = r.fields().get(name).map(|f| f.value.join("\n")).unwrap_or_default();
        let field = pdfcraft_xfa::field_now(r.tpl, &r.ov, &som);
        let format_failed = field.as_ref().filter(|f| rules::format_fails(f.format_test, f.format_picture.as_deref(), &value));
        if let Some(f) = format_failed {
            r.alert(crate::js::Message::plain(rules::format_message(f.format_message.as_deref(), name)));
        }
        let skip_script_message = format_failed.is_some_and(|f| rules::later_tests_skipped(f.format_test));
        for ev in r.events_of(&som, "validate") {
            let Some(o) = r.run(&ev, &value)? else { continue };
            if (o.result_bool == Some(false) || o.error.is_some()) && !value.trim().is_empty() && !skip_script_message {
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

/// A field gaining (`enter`) or losing (`exit`) focus: its scripts for that activity, then, when
/// they changed something, the calculations. Returns what changed; nothing (and no error) when
/// the field has no such script, so a focus change with nothing to run leaves the form alone.
pub(crate) fn on_focus(doc: &mut pdfcraft_cos::Document, tpl: &Template, som: &str, activity: &str, run: &mut XfaRun) -> Result<Changes, String> {
    let mut r = Runner::new(doc, tpl, run, EVENT_BUDGET_MS);
    let events = r.events_of(som, activity);
    if events.is_empty() {
        return Ok(Changes::default());
    }
    // The field's value, as the script's event sees it (`xfa.event.prevText`, `newText`).
    let value = r.by_som().get(som).cloned().and_then(|name| r.fields().get(&name).map(|f| f.value.join("\n"))).unwrap_or_default();
    for ev in events {
        r.run(&ev, &value)?;
    }
    if !r.changes.is_empty() {
        // The calculations that follow get a budget of their own.
        r.clock = Clock::start(OPEN_BUDGET_MS.saturating_mul(u64::from(r.run.time_scale)));
        r.calculates()?;
    }
    r.finish()
}

/// The SOM path of widget `widget` of field `field`: each radio button of a group has its own,
/// and a field's copies on several pages share one. `None` when the field has no widget
/// `widget`: the field's own path is only for a widget that records none, so a widget index
/// out of range never reaches the field (for a radio field, the group).
pub(crate) fn widget_som(doc: &pdfcraft_cos::Document, field: &pdfcraft_forms::Field, widget: usize) -> Option<String> {
    let som_of = |obj: pdfcraft_cos::ObjRef| {
        let object = doc.get(obj);
        object.as_dict()?.get(pdfcraft_xfa::SOM_KEY).map(|s| doc.resolve(s))?.as_string().map(|s| s.to_text())
    };
    let w = field.widgets.get(widget)?;
    som_of(w.obj).or_else(|| som_of(field.obj))
}

/// Does the live form give SOM path `som` an `activity` script?
pub(crate) fn has_event(doc: &pdfcraft_cos::Document, tpl: &Template, som: &str, activity: &str) -> bool {
    // Page-master copies have a SOM path of their own on each page (`master[1]…`).
    let uses = pdfcraft_xfa::page_area_uses(doc, tpl);
    let live = pdfcraft_xfa::form_tree_paged(tpl, pdfcraft_xfa::data_of(doc).as_ref(), &pdfcraft_xfa::overrides_of(doc, tpl), &uses);
    live.events.iter().any(|e| e.som == som && e.activity == activity)
}

/// The SOM path of a generated field whose template has a click script.
pub(crate) fn clickable_som(doc: &pdfcraft_cos::Document, tpl: &Template, field: &pdfcraft_forms::Field) -> Option<String> {
    scripted_som(doc, tpl, field, "click")
}

/// The SOM path of a generated field whose template has an `activity` script.
pub(crate) fn scripted_som(doc: &pdfcraft_cos::Document, tpl: &Template, field: &pdfcraft_forms::Field, activity: &str) -> Option<String> {
    // The field's own SOM path: each page-master copy has one of its own (`master[1]…`).
    let object = doc.get(field.obj);
    let som = object.as_dict()?.get(pdfcraft_xfa::SOM_KEY).map(|s| doc.resolve(s))?.as_string()?.to_text();
    let uses = pdfcraft_xfa::page_area_uses(doc, tpl);
    let live = pdfcraft_xfa::form_tree_paged(tpl, pdfcraft_xfa::data_of(doc).as_ref(), &pdfcraft_xfa::overrides_of(doc, tpl), &uses);
    live.events.iter().any(|e| e.som == som && e.activity == activity).then_some(som)
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

    /// A run of resets keeps each name once, in the order first given, and a reset of the whole
    /// form takes in the rest, before or after it.
    #[test]
    fn resets_in_a_row_keep_each_name_once_and_a_whole_form_reset_wins() {
        let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let mut run = ResetRun::default();
        run.add(&v(&["a", "b", "a"]));
        run.add(&v(&["c", "b"]));
        assert_eq!(run.names(), v(&["a", "b", "c"]).as_slice());
        run.add(&[]);
        assert!(run.names().is_empty());
        run.add(&v(&["d"]));
        assert!(run.names().is_empty(), "still the whole form");
        let mut first = ResetRun::default();
        first.add(&[]);
        first.add(&v(&["a"]));
        assert!(first.names().is_empty());
    }

    /// Adding a name to a run costs the same however many it holds: two runs of 60,000
    /// distinct names merge at once (comparing each name with every kept one is 3.6 billion
    /// comparisons).
    #[test]
    fn a_long_run_of_reset_names_merges_in_linear_time() {
        let a: Vec<String> = (0..60_000).map(|i| format!("form1[0].a{i}")).collect();
        let b: Vec<String> = (0..60_000).map(|i| format!("form1[0].b{i}")).collect();
        let started = std::time::Instant::now();
        let mut run = ResetRun::default();
        run.add(&a);
        run.add(&b);
        assert_eq!(run.names().len(), 120_000);
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "took {:?}", started.elapsed());
    }

    /// Matching objects against a reset's names costs the same however many names there are:
    /// 5,000 objects against 50,000 names that miss them all is 250 million comparisons when
    /// each object is compared with every name.
    #[test]
    fn matching_many_objects_against_many_reset_names_is_bounded() {
        let names: Vec<String> = (0..50_000).map(|i| format!("form1.page1.other{i}")).collect();
        let started = std::time::Instant::now();
        let targets = ResetTargets::new(&names);
        let hits = (0..5_000).filter(|i| targets.matches(&format!("form1[0].page1[0].table[0].row[{i}].item[0]"))).count();
        assert_eq!(hits, 0);
        assert!(targets.matches("form1[0].page1[0].other49999[0]"));
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "took {:?}", started.elapsed());
    }

    /// Verify that a shared layout name cannot cause a data-binding index to redirect pending focus.
    #[test]
    fn focus_shift_uses_the_som_index_when_data_binding_omits_the_segment() {
        let (tpl, _) = pdfcraft_xfa::parse(
            r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
                <subform name="form"><area name="row"/><subform name="row"><occur min="1" max="4"/></subform></subform>
            </template>"#,
        )
        .unwrap();
        let removed = "form[0].row[1]";
        assert_eq!(split_instance(&tpl, removed).unwrap().3, 0);
        let (prefix, index) = indexed_som_tail(removed).unwrap();
        for (before, after) in [
            ("form[0].row[0].amount[0]", Some("form[0].row[0].amount[0]")),
            ("form[0].row[1].amount[0]", None),
            ("form[0].row[2].amount[0]", Some("form[0].row[1].amount[0]")),
        ] {
            let mut target = Some(before.to_owned());
            shift_focus_after_removal(&mut target, prefix, index);
            assert_eq!(target.as_deref(), after);
        }
        assert!(indexed_som_tail("form[0].row[bad]").is_none());
    }

    #[test]
    fn script_names_match_indexed_som_paths() {
        let som_matches = |som: &str, name: &str| ResetTargets::new(&[name.to_string()]).matches(som);
        assert!(som_matches("form[0].page1[0].qty[0]", "qty"));
        assert!(som_matches("form[0].page1[0].qty[0]", "page1.qty"));
        assert!(som_matches("form[0].page1[0].qty[0]", "xfa.form.form.page1.qty"));
        assert!(som_matches("form[0].page1[0].qty[0]", "$form.form.page1.qty"));
        assert!(som_matches("form[0].page1[0].table[0].row[2].amount[0]", "table"));
        assert!(som_matches("form[0].page1[0].table[0].row[2].amount[0]", "table.row[0].amount"));
        assert!(!som_matches("form[0].page1[0].qty[0]", "qt"));
        assert!(!som_matches("form[0].page1[0].price[0]", "qty"));
        assert!(!som_matches("form[0].page1[0].qty[0]", "page1.page1.qty"));
        assert!(!som_matches("form[0].page1[0].qty[0]", " . "));
        assert!(!ResetTargets::new(&[]).matches("form[0].qty[0]"));
        let many = ResetTargets::new(&["price".into(), "page1.qty".into()]);
        assert!(many.matches("form[0].page1[0].qty[0]") && many.matches("form[0].page2[0].price[0]"));
        assert_eq!(
            split_instance(&Template::default(), "form[0].table[0].row[2]").map(|(p, ps, n, i)| (p.len(), ps, n, i)),
            Some((2, "form[0].table[0]".into(), "row".into(), 2))
        );
    }

    /// The scripts' snapshot shares the template's property trees: the next script's, and the
    /// next event's (a live form built again, from a copy of the template as overrides make
    /// one), point at the trees the first converted rather than converting them again.
    #[test]
    fn snapshots_share_property_trees_across_runs_and_events() {
        let fields: String = (0..2_000)
            .map(|i| format!(r#"<field name="f{i}"><ui><choiceList/></ui><border><edge/><fill><color value="1,2,3"/></fill></border></field>"#))
            .collect();
        let xml = format!(
            r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">{fields}</subform></template>"#
        );
        let (tpl, _) = pdfcraft_xfa::parse(&xml).unwrap();
        let live = pdfcraft_xfa::form_tree(&tpl, None, &Default::default());
        let a = to_js(&live.root, &tpl);
        let b = to_js(&live.root, &tpl);
        // A tree built again (with a script's overrides) is still the form's: same copies.
        let copy = pdfcraft_xfa::apply_overrides(&tpl, &Default::default());
        let c = to_js(&pdfcraft_xfa::form_tree(&copy, None, &Default::default()).root, &tpl);
        assert_eq!(a.children.len(), 2_000);
        for ((x, y), z) in a.children.iter().zip(&b.children).zip(&c.children) {
            assert!(Arc::ptr_eq(&x.props, &y.props) && Arc::ptr_eq(&x.props, &z.props));
        }
        assert_eq!(a.children[0].props.elements[1].children[1].children[0].attrs, [("value".to_string(), "1,2,3".to_string())]);
    }

    /// A form's copies of its property trees are its own: kept for its later snapshots, and
    /// dropped with its template when the form is closed, whatever other forms do.
    #[test]
    fn a_closed_forms_property_copies_go_with_its_template() {
        let xml = |name: &str| {
            format!(
                r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb"><field name="{name}"><border><edge/></border></field></subform></template>"#
            )
        };
        let (closed, _) = pdfcraft_xfa::parse(&xml("a")).unwrap();
        let (open, _) = pdfcraft_xfa::parse(&xml("b")).unwrap();
        let live = pdfcraft_xfa::form_tree(&closed, None, &Default::default());
        let snapshot = to_js(&live.root, &closed);
        let copy = Arc::downgrade(&snapshot.children[0].props);
        drop(snapshot);
        assert!(copy.upgrade().is_some(), "kept for the form's next snapshot");
        // Another form's snapshot neither keeps nor needs this form's copies.
        let other = to_js(&pdfcraft_xfa::form_tree(&open, None, &Default::default()).root, &open);
        assert!(!Arc::ptr_eq(&other.children[0].props, &copy.upgrade().unwrap_or_default()));
        drop(live);
        drop(closed);
        assert!(copy.upgrade().is_none(), "closing the form drops its copies at once");
    }

    /// A live form node whose property tree is told apart by an attribute.
    fn marked(mark: &str) -> FormNode {
        FormNode {
            props: Arc::new(Props { attrs: vec![("mark".into(), mark.into())], elements: Vec::new(), ..Default::default() }),
            ..Default::default()
        }
    }

    /// The copy made of a tree is the one every later snapshot of the form gets, and the map
    /// holds one entry for it.
    #[test]
    fn later_snapshots_of_a_form_get_the_same_copy() {
        let tpl = Template::default();
        let node = marked("a");
        let first = to_js(&node, &tpl);
        let second = to_js(&node, &tpl);
        assert!(Arc::ptr_eq(&first.props, &second.props));
        let copies = props_copies(&tpl).expect("the engine's map");
        assert_eq!(copies.len(), 1);
        assert!(copy_of(&copies, &node.props).is_some_and(|c| Arc::ptr_eq(&c, &first.props)));
    }

    /// The map is keyed by a tree's address, so a tree freed and another made at the same
    /// address must never get the first one's copy (the ABA case). An entry's weak reference
    /// keeps the freed tree's allocation, so its address is not reused while the entry exists;
    /// the next use of the map drops the entry, and only then can a new tree take the address,
    /// with no entry left to find. Each new tree below is dropped at the end of its turn, so the
    /// allocator is free to put the next one where it was.
    #[test]
    fn a_freed_trees_copy_never_goes_to_a_tree_at_its_address() {
        let tpl = Template::default();
        let old = marked("old");
        let addr = Arc::as_ptr(&old.props) as usize;
        drop(to_js(&old, &tpl));
        drop(old);
        let made: Vec<Arc<Props>> = (0..4_096).map(|_| Arc::new(Props::default())).collect();
        assert!(made.iter().all(|p| Arc::as_ptr(p) as usize != addr), "the address is held while the entry exists");
        drop(made);
        assert!(props_copies(&tpl).is_some_and(|c| c.is_empty()), "the next use drops the entry");
        for i in 0..4_096 {
            let node = marked(&format!("new {i}"));
            assert_eq!(to_js(&node, &tpl).props.attrs, node.props.attrs);
        }
    }

    /// An entry under a tree's address that was made from another tree is not that tree's copy:
    /// a lookup passes it by, and the snapshot's own lookup replaces it with the tree's copy.
    #[test]
    fn an_entry_made_from_another_tree_is_not_returned() {
        let tpl = Template::default();
        let node = marked("this");
        let other = Arc::new(Props { attrs: vec![("mark".into(), "other".into())], elements: Vec::new(), ..Default::default() });
        let gone = Arc::new(Props { attrs: vec![("mark".into(), "gone".into())], elements: Vec::new(), ..Default::default() });
        let key = Arc::as_ptr(&node.props) as usize;
        let mut copies = PropsCopies::new();
        // Its tree is alive but elsewhere.
        copies.insert(key, (Arc::downgrade(&other), Arc::new(convert_props(&other))));
        assert!(copy_of(&copies, &node.props).is_none());
        // Its tree is gone.
        copies.insert(key, (Arc::downgrade(&gone), Arc::new(convert_props(&gone))));
        drop(gone);
        assert!(copy_of(&copies, &node.props).is_none());
        // Through a snapshot: the stray entry is passed by and replaced.
        props_copies(&tpl).expect("the engine's map").insert(key, (Arc::downgrade(&other), Arc::new(convert_props(&other))));
        assert_eq!(to_js(&node, &tpl).props.attrs, node.props.attrs);
        assert!(props_copies(&tpl).and_then(|c| copy_of(&c, &node.props)).is_some_and(|c| c.attrs == node.props.attrs));
    }

    /// Trees of other templates (the template read again) that a snapshot met leave the map once
    /// they are gone: however many come and go, it holds the form's own trees and no more.
    #[test]
    fn the_map_does_not_grow_with_trees_that_are_gone() {
        let xml = r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb"><field name="a"><border><edge/></border></field><field name="b"/></subform></template>"#;
        let (tpl, _) = pdfcraft_xfa::parse(xml).unwrap();
        let live = pdfcraft_xfa::form_tree(&tpl, None, &Default::default());
        drop(to_js(&live.root, &tpl));
        let own = props_copies(&tpl).map_or(0, |c| c.len());
        assert!(own >= 3, "{own}");
        for _ in 0..500 {
            let (again, _) = pdfcraft_xfa::parse(xml).unwrap();
            drop(to_js(&pdfcraft_xfa::form_tree(&again, None, &Default::default()).root, &tpl));
        }
        assert_eq!(props_copies(&tpl).map_or(0, |c| c.len()), own);
    }

    /// The decoded content of every page of `bytes`, as text.
    fn page_ops(bytes: &[u8]) -> String {
        let doc = pdfcraft_cos::Document::open(Arc::new(bytes.to_vec())).unwrap();
        let mut out = String::new();
        for r in pdfcraft_annot::page_refs(&doc).unwrap() {
            let page = doc.get(r);
            let Some(contents) = page.as_dict().and_then(|d| d.get(b"Contents")).cloned() else { continue };
            for c in pdfcraft_cos::DecodeBudget::page().decode_contents(&doc, &contents).into_iter().flatten() {
                out.push_str(&String::from_utf8_lossy(&c));
            }
        }
        out
    }

    /// The scripted test form with the `hello` button's click script set to `click`, and the
    /// `legacy` button's (FormCalc) to `legacy`.
    fn with_scripts(click: &str, legacy: &str) -> Vec<u8> {
        let template = pdfcraft_xfa::fixtures::scripted_template()
            .replace(r#"xfa.host.messageBox("Hello " + qty.rawValue);"#, click)
            .replace(r#"$host.messageBox("FormCalc")"#, legacy);
        pdfcraft_xfa::fixtures::shell(&template)
    }

    /// The colour a click script gives a field's fill, as page content (0.2 0.4 0.6 is
    /// 51,102,153).
    const FILL: &str = "0.2 0.4 0.6 rg";

    /// A click script's property changes take effect: the field it fills is drawn filled, the
    /// field it makes mandatory is Required, a tooltip is set; a later event's script reads them
    /// back; a colour that isn't one is refused with a note. Undo takes the whole event back,
    /// redo brings it again, and the saved file keeps it.
    #[test]
    fn property_changes_from_a_click_take_effect_and_are_kept() {
        let bytes = with_scripts(
            r#"qty.border.fill.color.value = "51,102,153"; qty.mandatory = "error"; price.assist.toolTip.value = "Unit price"; price.fontColor = "blue";"#,
            r#"$host.messageBox(concat(qty.border.fill.color.value, "|", qty.mandatory, "|", price.assist.toolTip.value))"#,
        );
        let mut s = crate::Session::new();
        let id = s.open("props.pdf", None, Arc::new(bytes), None).unwrap();
        let _ = s.take_js_output(id);
        let state = |s: &crate::Session| {
            let d = s.get(id).unwrap();
            let field = |n: &str| d.form.iter().find(|f| f.name.ends_with(n)).cloned().unwrap();
            (
                page_ops(&d.bytes).contains(FILL),
                field("qty").has(pdfcraft_forms::flags::REQUIRED),
                field("price").tooltip.clone(),
                pdfcraft_xfa::overrides(&pdfcraft_cos::Document::open(d.bytes.clone()).unwrap()).props().len(),
            )
        };
        let before = state(&s);
        assert_eq!(before, (false, false, None, 0));
        let out = s.run_javascript(id, "", Some("hello")).unwrap();
        assert!(out.error.is_none(), "{out:?}");
        let after = state(&s);
        assert_eq!(after, (true, true, Some("Unit price".to_string()), 2));
        assert!(out.console.iter().any(|l| l.contains("\"blue\" is not a colour")), "{out:?}");
        // One undo step.
        s.undo(id).unwrap();
        assert_eq!(state(&s), before);
        s.redo(id).unwrap();
        assert_eq!(state(&s), after);
        // Read back by a later event's script, in FormCalc.
        let out = s.run_javascript(id, "", Some("legacy")).unwrap();
        assert_eq!(out.alerts, [pdfcraft_js::Message::plain("51,102,153|error|Unit price")]);
        // Saved and opened again.
        let saved = s.save_bytes(id).unwrap();
        let mut again = crate::Session::new();
        let reopened = again.open("saved.pdf", None, saved, None).unwrap();
        let d = again.get(reopened).unwrap();
        assert!(page_ops(&d.bytes).contains(FILL));
        assert!(d.form.iter().find(|f| f.name.ends_with("qty")).unwrap().has(pdfcraft_forms::flags::REQUIRED));
        let out = again.run_javascript(reopened, "", Some("legacy")).unwrap();
        assert_eq!(out.alerts, [pdfcraft_js::Message::plain("51,102,153|error|Unit price")]);
    }

    /// Scripts that colour fields when the form opens (as Ontario's forms do in FormCalc,
    /// `$.font.fill.color.value = "0,0,255"`, on hundreds of fields) lay the form out once, and
    /// the note opening leaves counts the changes.
    #[test]
    fn opening_scripts_that_change_properties_take_effect() {
        let template = pdfcraft_xfa::fixtures::scripted_template().replace(
            r#"<field name="price" w="2in" h="0.3in"><ui><numericEdit/></ui>"#,
            r#"<field name="price" w="2in" h="0.3in"><ui><numericEdit/></ui><event activity="initialize"><script>$.border.fill.color.value = "51,102,153"</script></event>"#,
        );
        let bytes = pdfcraft_xfa::fixtures::shell(&template);
        let mut doc = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
        pdfcraft_xfa::render_into(&mut doc).unwrap();
        assert!(!page_ops(&pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()).contains(FILL));
        let tpl = template_of_doc(&doc);
        let mut out = JsOutput::default();
        let mut run = XfaRun { page_count: 1, out: &mut out, datasets: None, ran_away: false, time_scale: 1, highlight: false };
        let changes = on_open(&mut doc, &tpl, &mut run).unwrap();
        assert_eq!(changes.properties, 1, "{changes:?}");
        assert!(changes.describe().contains("1 changed property"));
        assert!(page_ops(&pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()).contains(FILL));
    }

    fn template_of_doc(doc: &pdfcraft_cos::Document) -> Template {
        pdfcraft_xfa::template_of(doc).unwrap()
    }

    /// A row's changes stay with the row: the third row made mandatory, then the first removed,
    /// leaves the (now) second row Required and reading `error`, and no other; undo and redo
    /// of the removal move the change back and forth with the row.
    #[test]
    fn a_rows_property_changes_follow_it_when_an_earlier_row_is_removed() {
        let bytes = with_scripts(
            r#"table._row.addInstance(1); table._row.addInstance(1); table.resolveNode("row[2]").amount.mandatory = "error"; table.resolveNode("row[0]").amount.fillColor = "9,9,9";"#,
            r#"table._row.removeInstance(0)
$host.messageBox(concat(table.row[0].amount.mandatory, "|", table.row[1].amount.mandatory))"#,
        );
        let mut s = crate::Session::new();
        let id = s.open("rows.pdf", None, Arc::new(bytes), None).unwrap();
        let _ = s.take_js_output(id);
        // The SOM paths of the rows' amount fields that are Required, and the property changes kept.
        let state = |s: &crate::Session| {
            let d = s.get(id).unwrap();
            let doc = pdfcraft_cos::Document::open(d.bytes.clone()).unwrap();
            let by_som = pdfcraft_xfa::fields_by_som(&doc);
            let mut required: Vec<String> = by_som
                .iter()
                .filter(|(som, name)| som.contains(".amount[") && d.form.iter().any(|f| &f.name == *name && f.has(pdfcraft_forms::flags::REQUIRED)))
                .map(|(som, _)| som.clone())
                .collect();
            required.sort();
            let kept: Vec<String> = pdfcraft_xfa::overrides(&doc).props().keys().cloned().collect();
            (required, kept)
        };
        let out = s.run_javascript(id, "", Some("hello")).unwrap();
        assert!(out.error.is_none(), "{out:?}");
        let added = state(&s);
        assert_eq!(added.0, ["form[0].page1[0].table[0].row[2].amount[0]"]);
        let out = s.run_javascript(id, "", Some("legacy")).unwrap();
        assert_eq!(out.alerts, [pdfcraft_js::Message::plain("disabled|error")], "{out:?}");
        let removed = state(&s);
        assert_eq!(removed.0, ["form[0].page1[0].table[0].row[1].amount[0]"]);
        assert_eq!(removed.1, ["form[0].page1[0].table[0].row[1].amount[0]"], "the first row's fill went with it");
        s.undo(id).unwrap();
        assert_eq!(state(&s), added);
        s.redo(id).unwrap();
        assert_eq!(state(&s), removed);
    }

    /// `occur.max` raised by a click lets the next click add more rows than the template
    /// allowed; the limit is kept with the form.
    #[test]
    fn occur_changed_by_a_script_changes_what_the_instance_manager_allows() {
        let template = pdfcraft_xfa::fixtures::scripted_template()
            .replace(r#"<occur min="1" max="-1"/>"#, r#"<occur min="1" max="2"/>"#)
            .replace(r#"xfa.host.messageBox("Hello " + qty.rawValue);"#, r#"table.row.occur.max = "4";"#);
        let mut s = crate::Session::new();
        let id = s.open("rows.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&template)), None).unwrap();
        let rows = |s: &crate::Session| s.get(id).unwrap().form.iter().filter(|f| f.name.contains("amount")).count();
        for _ in 0..3 {
            s.run_javascript(id, "", Some("addRow")).unwrap();
        }
        assert_eq!(rows(&s), 2, "the template's maximum");
        s.run_javascript(id, "", Some("hello")).unwrap();
        for _ in 0..3 {
            s.run_javascript(id, "", Some("addRow")).unwrap();
        }
        assert_eq!(rows(&s), 4, "the maximum the script set");
    }

    /// The script refuses exactly the values the form refuses, so a script never reads back a
    /// change the form drops (the two checks live on either side of the layering line).
    #[test]
    fn the_script_and_the_form_refuse_the_same_values() {
        let cases: &[(&str, Option<&str>, &str)] = &[
            ("border#0/fill#0/color#0", Some("value"), "255,0,0"),
            ("border#0/fill#0/color#0", Some("value"), " 1 , 2 , 3 "),
            ("border#0/fill#0/color#0", Some("value"), "1,2,3,4"),
            ("border#0/fill#0/color#0", Some("value"), "red"),
            ("border#0/fill#0/color#0", Some("value"), "1,2"),
            ("border#0/fill#0/color#0", Some("value"), ""),
            ("border#0/edge#0", Some("thickness"), "0.5pt"),
            ("border#0/edge#0", Some("thickness"), "2"),
            ("border#0/edge#0", Some("thickness"), "2em"),
            ("border#0/edge#0", Some("thickness"), "thick"),
            ("border#0/edge#0", Some("thickness"), "NaNpt"),
            ("font#0", Some("size"), "12pt"),
            ("font#0", Some("size"), "large"),
            ("margin#0", Some("leftInset"), "3mm"),
            ("para#0", Some("tabDefault"), "0.25in"),
            ("occur#0", Some("max"), "-1"),
            ("occur#0", Some("max"), "1.5"),
            ("value#0/text#0", Some("maxChars"), "12"),
            ("value#0/text#0", Some("maxChars"), "x"),
            ("validate#0", Some("nullTest"), "error"),
            ("break#0", Some("before"), ""),
            ("assist#0/toolTip#0", None, "Help"),
            ("validate#0/message#0/text#0", None, "Bad"),
            ("validate#0/script#0", None, "true"),
            ("value#0/text#0", None, "x"),
            ("border#0/edge#3/color#0", Some("value"), "1,2,3"),
            ("border#0/edge#4/color#0", Some("value"), "1,2,3"),
            ("border#0/corner#7", Some("presence"), "hidden"),
            ("ui#0/textEdit#0/border#0/edge#5", Some("thickness"), "1pt"),
            // Paths: a property root, `tag#n` steps, occurrences and depth within what forms use.
            ("extras#0/foo#7", Some("x"), "1"),
            ("extras#0/foo#8", Some("x"), "1"),
            ("extras#0/a#0/b#0/c#0/d#0/e#0/f#0/g#0", Some("x"), "1"),
            ("extras#0/a#0/b#0/c#0/d#0/e#0/f#0/g#0/h#0", Some("x"), "1"),
            ("caption#0/font#0", Some("weight"), "bold"),
            ("border/edge#0", Some("presence"), "hidden"),
            ("border#0", Some("bad name"), "x"),
            // Attributes the form doesn't read again once open.
            ("validate#0/script#0", Some("contentType"), "application/x-javascript"),
            ("bind#0", Some("ref"), "$.other"),
            ("bind#0", Some("match"), "none"),
            ("bind#0/picture#0", None, "date{YYYYMMDD}"),
        ];
        let long = "x".repeat(pdfcraft_xfa::restyle::MAX_PROP_VALUE + 1);
        for (path, attr, value) in cases.iter().copied().chain([("assist#0/toolTip#0", None, long.as_str())]) {
            let script = pdfcraft_js::xfa::property_value_ok(path, attr, value).is_ok();
            let form = pdfcraft_xfa::restyle::check(path, attr, value).is_ok();
            assert_eq!(script, form, "{path} {attr:?} {value:?}");
        }
    }

    /// A field's click script reads its own and other objects' properties through a laid-out
    /// dynamic form: template values, the spec's defaults, and a write read back.
    #[test]
    fn a_click_script_reads_field_properties() {
        let template = pdfcraft_xfa::fixtures::scripted_template()
            .replace(
                r#"<field name="hello" w="1in" h="0.3in"><ui><button/></ui>"#,
                r#"<field name="hello" w="1in" h="0.3in"><ui><button/></ui><border><fill><color value="200,200,255"/></fill></border>"#,
            )
            .replace(
                r#"xfa.host.messageBox("Hello " + qty.rawValue);"#,
                r#"qty.validate.nullTest = "error"; xfa.host.messageBox([this.ui.oneOfChild.className, this.border.fill.color.value, this.border.presence, qty.ui.oneOfChild.className, qty.validate.message.text.value, qty.validate.nullTest, table.row.occur.max, more.ui.oneOfChild.className, this.isPropertySpecified("border"), qty.isPropertySpecified("border")].join("|"));"#,
            );
        let mut s = crate::Session::new();
        let id = s.open("props.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&template)), None).unwrap();
        let _ = s.take_js_output(id);
        let out = s.run_javascript(id, "", Some("hello")).unwrap();
        assert!(out.error.is_none(), "{out:?}");
        assert_eq!(
            out.alerts,
            [pdfcraft_js::Message::plain("button|200,200,255|visible|numericEdit|Quantity must be 100 or less|error|-1|checkButton|true|false")]
        );
        assert!(s.take_js_output(id).errors.is_empty());
    }

    fn field(obj: pdfcraft_cos::ObjRef, kind: pdfcraft_forms::FieldKind, value: &[&str]) -> pdfcraft_forms::Field {
        pdfcraft_forms::Field {
            name: "f".into(),
            obj,
            kind,
            value: value.iter().map(|v| (*v).to_owned()).collect(),
            default: Vec::new(),
            flags: 0,
            max_len: None,
            options: Vec::new(),
            selected: Vec::new(),
            single_value_is_array: false,
            da: String::new(),
            quadding: 0,
            tooltip: None,
            widgets: Vec::new(),
            actions: pdfcraft_forms::af::Actions::default(),
            button: None,
        }
    }

    /// A field dictionary whose `/V` is `v`, with a SOM path.
    fn field_dict(doc: &mut pdfcraft_cos::Document, v: Option<pdfcraft_cos::Object>) -> pdfcraft_cos::ObjRef {
        let mut d = pdfcraft_cos::Dict::new();
        if let Some(v) = v {
            d.set(b"V".to_vec(), v);
        }
        d.set(pdfcraft_xfa::SOM_KEY.to_vec(), pdfcraft_cos::Object::String(pdfcraft_cos::PdfString::text("form[0].f[0]")));
        doc.add(d)
    }

    #[test]
    fn changed_values_are_found_in_place() {
        use pdfcraft_cos::{Object as O, PdfString};
        use pdfcraft_forms::FieldKind as K;
        let s = |t: &str| O::String(PdfString::text(t));
        let mut doc = pdfcraft_cos::Document::new_empty();
        let same = |doc: &mut pdfcraft_cos::Document, kind, old: &[&str], v: Option<O>| {
            let r = field_dict(doc, v);
            let dict = doc.get(r);
            value_unchanged(doc, &field(r, kind, old), dict.as_dict().and_then(|d| d.get(b"V")))
        };
        // Text: the first value only, an absent or non-text value being empty.
        assert!(same(&mut doc, K::Text, &["Café"], Some(s("Café"))));
        assert!(!same(&mut doc, K::Text, &["Café"], Some(s("Cafe"))));
        assert!(same(&mut doc, K::Text, &[], None));
        assert!(same(&mut doc, K::Text, &[], Some(s(""))));
        assert!(!same(&mut doc, K::Text, &["a"], None));
        assert!(same(&mut doc, K::Text, &["a", "b"], Some(O::Array(vec![s("a"), s("c")]))));
        // A value given by reference is followed.
        let inner = doc.add(s("日本"));
        assert!(same(&mut doc, K::Text, &["日本"], Some(O::Ref(inner))));
        assert!(!same(&mut doc, K::Combo, &["日"], Some(O::Ref(inner))));
        // Combo box: one value.
        assert!(same(&mut doc, K::Combo, &["Two"], Some(s("Two"))));
        assert!(!same(&mut doc, K::Combo, &["Two"], Some(s("Three"))));
        assert!(!same(&mut doc, K::Combo, &[], Some(s("Two"))));
        assert!(!same(&mut doc, K::Combo, &["Two"], None));
        // List box: every pick, in order, by count; non-strings in the array are skipped.
        let picks = |v: &[&str]| O::Array(v.iter().map(|t| s(t)).collect());
        assert!(same(&mut doc, K::List, &["a", "b"], Some(picks(&["a", "b"]))));
        assert!(!same(&mut doc, K::List, &["a", "b"], Some(picks(&["b", "a"]))));
        assert!(!same(&mut doc, K::List, &["a", "b"], Some(picks(&["a"]))));
        assert!(!same(&mut doc, K::List, &["a"], Some(picks(&["a", "b"]))));
        assert!(!same(&mut doc, K::List, &["a", "b"], Some(picks(&["a", "c"]))));
        assert!(same(&mut doc, K::List, &[], Some(picks(&[]))));
        let item = doc.add(s("b"));
        assert!(same(&mut doc, K::List, &["a", "b"], Some(O::Array(vec![s("a"), O::Int(3), O::Ref(item)]))));
        // Check box: a state name, Off for none.
        assert!(same(&mut doc, K::CheckBox, &["Yes"], Some(O::name("Yes"))));
        assert!(!same(&mut doc, K::CheckBox, &["Yes"], Some(O::name("Off"))));
        assert!(same(&mut doc, K::CheckBox, &[], Some(O::name("Off"))));
        assert!(!same(&mut doc, K::CheckBox, &[], Some(O::name("Yes"))));
        assert!(!same(&mut doc, K::CheckBox, &["Yes"], Some(O::name("On"))));
    }

    #[test]
    fn unchanged_values_return_before_the_template_is_read() {
        use pdfcraft_cos::{Object as O, PdfString};
        use pdfcraft_forms::FieldKind as K;
        // Laid out, but with no XFA packets: reading the template fails.
        let mut doc = pdfcraft_cos::Document::new_empty();
        let mut acro = pdfcraft_cos::Dict::new();
        acro.set(pdfcraft_xfa::LAYOUT_KEY.to_vec(), pdfcraft_cos::Dict::new());
        let acro = doc.add(acro);
        let root = doc.root().unwrap_or_else(|| panic!("no catalog"));
        let mut catalog = doc.get(root).as_dict().cloned().unwrap_or_default();
        catalog.set(b"AcroForm".to_vec(), O::Ref(acro));
        doc.set(root, catalog);
        assert!(pdfcraft_xfa::existing_layout(&doc).is_some());
        let text = field_dict(&mut doc, Some(O::String(PdfString::text("same"))));
        let list = field_dict(&mut doc, Some(O::Array(vec![O::String(PdfString::text("a")), O::String(PdfString::text("b"))])));
        let check = field_dict(&mut doc, Some(O::name("Yes")));
        let before = [field(text, K::Text, &["same"]), field(list, K::List, &["a", "b"]), field(check, K::CheckBox, &["Yes"])];
        assert_eq!(relayout_changed_text(&mut doc, None, &before), Ok(None));
        // A changed pick reaches the template, which this document doesn't have.
        let before = [field(text, K::Text, &["same"]), field(list, K::List, &["b", "a"]), field(check, K::CheckBox, &["Yes"])];
        assert!(relayout_changed_text(&mut doc, None, &before).is_err());
    }

    /// A form whose opening script gives a field a format test (picture, `error`, message), and
    /// three calculated fields: one whose calculation the user may override with a warning,
    /// one that says `disabled` and one that keeps the default.
    fn checks_form() -> Vec<u8> {
        let template = r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="s" layout="tb">
  <event activity="initialize"><script contentType="application/x-javascript">V2.validate.picture.value = "text{9999}"; V2.validate.formatTest = "error"; V2.formatMessage = "Custom format message";</script></event>
  <field name="V2" w="2in" h="0.3in"><ui><textEdit/></ui></field>
  <field name="V3" w="2in" h="0.3in"><ui><textEdit/></ui><validate><picture>text{99}</picture><message><text name="formatTest">Two digits</text></message></validate></field>
  <field name="V4" w="2in" h="0.3in"><ui><textEdit/></ui><validate><picture>text{99}</picture><message><text name="formatTest">Two digits</text></message></validate><event activity="exit"><script contentType="application/x-javascript">this.validate.formatTest = "disabled";</script></event></field>
  <field name="V5" w="2in" h="0.3in"><ui><textEdit/></ui><event activity="change"><script contentType="application/x-javascript">this.validate.picture.value = "text{99}"; this.validate.formatTest = "error"; this.formatMessage = "Set on change";</script></event></field>
  <field name="K1" w="2in" h="0.3in"><ui><numericEdit/></ui><value><decimal>1</decimal></value></field>
  <field name="K2" w="2in" h="0.3in"><ui><numericEdit/></ui><calculate override="warning"><script contentType="application/x-javascript">K1.rawValue * 10</script></calculate></field>
  <field name="K3" w="2in" h="0.3in"><ui><numericEdit/></ui><calculate override="disabled"><script contentType="application/x-javascript">K1.rawValue * 100</script></calculate></field>
  <field name="K4" w="2in" h="0.3in"><ui><numericEdit/></ui><calculate><script contentType="application/x-javascript">K1.rawValue * 1000</script></calculate></field>
 </subform>
</subform>
</template>
</xdp:xdp>
"##;
        pdfcraft_xfa::fixtures::shell(template)
    }

    fn type_into(s: &mut crate::Session, id: crate::DocId, field: &str, value: &str) {
        let name = s.get(id).unwrap().form.iter().find(|f| f.name.ends_with(field)).unwrap().name.clone();
        s.apply(id, crate::Edit::SetFieldValue { name, value: pdfcraft_forms::FieldValue::Text(value.into()) }).unwrap();
    }

    fn value_of(s: &crate::Session, id: crate::DocId, field: &str) -> String {
        s.get(id).unwrap().form.iter().find(|f| f.name.ends_with(field)).unwrap().value.join("\n")
    }

    /// A value that fails its validation's format test says the validation's message when the
    /// user leaves the field, and stays as typed (XFA 3.3 p. 880, p. 396); one that passes, or
    /// an empty one, says nothing. The opening script's test and message count as the
    /// template's do.
    #[test]
    fn a_value_failing_its_format_test_says_the_message_and_stays() {
        let mut s = crate::Session::new();
        let id = s.open("checks.pdf", None, Arc::new(checks_form()), None).unwrap();
        let _ = s.take_js_output(id);
        type_into(&mut s, id, "V2", "abc");
        assert_eq!(s.take_js_output(id).alerts.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["Custom format message"]);
        assert_eq!(value_of(&s, id, "V2"), "abc");
        type_into(&mut s, id, "V2", "1234");
        assert!(s.take_js_output(id).alerts.is_empty());
        type_into(&mut s, id, "V3", "123");
        assert_eq!(s.take_js_output(id).alerts.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["Two digits"]);
        type_into(&mut s, id, "V3", "");
        assert!(s.take_js_output(id).alerts.is_empty());
    }

    /// The format test the user's entry is checked by is the one the field's change and exit
    /// scripts leave: one turned off as the field is left says nothing, and one set as the value
    /// changes says its message.
    #[test]
    fn the_format_test_is_the_one_the_fields_scripts_leave() {
        let mut s = crate::Session::new();
        let id = s.open("checks.pdf", None, Arc::new(checks_form()), None).unwrap();
        let _ = s.take_js_output(id);
        type_into(&mut s, id, "V4", "123");
        assert!(s.take_js_output(id).alerts.is_empty(), "the exit script turned the test off");
        type_into(&mut s, id, "V5", "abc");
        assert_eq!(s.take_js_output(id).alerts.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["Set on change"]);
    }

    /// A value typed into a field whose calculation the form only recommends is kept, the
    /// user is asked whether to keep it, and later calculations leave it alone (saved with the
    /// form); undoing the entry goes back to the calculation. Fields the user didn't override
    /// keep being calculated, `disabled` among them, as readers do.
    #[test]
    fn a_value_typed_over_a_recommended_calculation_stays_until_undone() {
        let mut s = crate::Session::new();
        let id = s.open("checks.pdf", None, Arc::new(checks_form()), None).unwrap();
        let _ = s.take_js_output(id);
        assert_eq!(["K2", "K3", "K4"].map(|f| value_of(&s, id, f)), ["10", "100", "1000"]);
        type_into(&mut s, id, "K2", "5");
        let out = s.take_js_output(id);
        assert!(matches!(out.requests.as_slice(), [Request::CalculationOverridden(m)] if m.contains("K2")), "{out:?}");
        assert_eq!(value_of(&s, id, "K2"), "5");
        type_into(&mut s, id, "K1", "2");
        assert_eq!(["K2", "K3", "K4"].map(|f| value_of(&s, id, f)), ["5", "200", "2000"]);
        // Kept with the form.
        let saved = s.save_bytes(id).unwrap();
        let mut again = crate::Session::new();
        let reopened = again.open("saved.pdf", None, saved, None).unwrap();
        type_into(&mut again, reopened, "K1", "3");
        assert_eq!(["K2", "K3"].map(|f| value_of(&again, reopened, f)), ["5", "300"]);
        // Asked again only for a new entry into a field not yet overridden.
        let _ = again.take_js_output(reopened);
        type_into(&mut again, reopened, "K2", "6");
        assert!(again.take_js_output(reopened).requests.is_empty());
        // Going back: the K1 change, then the entry, undone; the next calculation reaches K2.
        s.undo(id).unwrap();
        s.undo(id).unwrap();
        assert_eq!(value_of(&s, id, "K2"), "10");
        type_into(&mut s, id, "K1", "4");
        assert_eq!(value_of(&s, id, "K2"), "40");
    }

    /// A caption's colour, a check box's on value and a choice list that stops taking typed text,
    /// set by a click (round two of the property writes): drawn and on the fields, one undo
    /// step, and kept through save and reopen; a later FormCalc script reads the caption's back.
    #[test]
    fn caption_and_item_changes_from_a_click_take_effect_and_are_kept() {
        let template = pdfcraft_xfa::fixtures::scripted_template().replace(
            r#"<field name="note" w="4in" h="0.3in"><ui><textEdit/></ui>"#,
            r#"<field name="note" w="4in" h="0.3in"><ui><choiceList textEntry="1"/></ui><items><text>A</text><text>B</text></items>"#,
        );
        let template = template
            .replace(r#"<subform name="details" layout="tb" presence="hidden">"#, r#"<subform name="details" layout="tb">"#)
            .replace(
                r#"xfa.host.messageBox("Hello " + qty.rawValue);"#,
                r#"price.caption.font.fill.color.value = "244,15,55"; price.caption.font.posture = "italic"; if (more.items.nodes.item(0).value == "1") more.items.nodes.item(0).value = "Yes"; details.note.ui.choiceList.textEntry = "0"; details.presence = "visible";"#,
            )
            .replace(
                r#"$host.messageBox("FormCalc")"#,
                r#"$host.messageBox(concat(price.caption.font.fill.color.value, "|", price.caption.font.posture))"#,
            );
        let mut s = crate::Session::new();
        let id = s.open("captions.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&template)), None).unwrap();
        let _ = s.take_js_output(id);
        // The caption's red text, the check box's on state, whether the list takes typed text.
        let state = |s: &crate::Session| {
            let d = s.get(id).unwrap();
            let field = |n: &str| d.form.iter().find(|f| f.name.ends_with(n)).cloned().unwrap();
            let on = field("more").widgets.iter().find_map(|w| w.on_state.clone());
            (page_ops(&d.bytes).contains("0.957 0.059 0.216 rg"), on, field("note").has(pdfcraft_forms::flags::EDIT))
        };
        let before = state(&s);
        assert_eq!(before, (false, Some("1".to_string()), true));
        let out = s.run_javascript(id, "", Some("hello")).unwrap();
        assert!(out.error.is_none(), "{out:?}");
        let after = state(&s);
        assert_eq!(after, (true, Some("Yes".to_string()), false));
        s.undo(id).unwrap();
        assert_eq!(state(&s), before);
        s.redo(id).unwrap();
        assert_eq!(state(&s), after);
        let out = s.run_javascript(id, "", Some("legacy")).unwrap();
        assert_eq!(out.alerts, [pdfcraft_js::Message::plain("244,15,55|italic")]);
        let saved = s.save_bytes(id).unwrap();
        let mut again = crate::Session::new();
        let reopened = again.open("saved.pdf", None, saved, None).unwrap();
        let d = again.get(reopened).unwrap();
        assert!(page_ops(&d.bytes).contains("0.957 0.059 0.216 rg"));
        assert_eq!(d.form.iter().find(|f| f.name.ends_with("more")).unwrap().widgets.iter().find_map(|w| w.on_state.clone()), Some("Yes".into()));
        let out = again.run_javascript(reopened, "", Some("legacy")).unwrap();
        assert_eq!(out.alerts, [pdfcraft_js::Message::plain("244,15,55|italic")]);
    }

    /// A script that hides every object and colours a field, saved and opened again, keeps both:
    /// the property change waits under its hidden object and shows again with it. Fields bound to
    /// one data value (`bind match="global"`) share the value, not the property change.
    #[test]
    fn property_changes_survive_hide_all_and_stay_off_globally_bound_fields() {
        let template = pdfcraft_xfa::fixtures::scripted_template()
            .replace(
                r#"<field name="note" w="4in" h="0.3in"><ui><textEdit/></ui>"#,
                r#"<field name="note" w="4in" h="0.3in"><ui><textEdit/></ui><bind match="global"/>"#,
            )
            .replace(
                r#"<subform name="table" layout="table" columnWidths="3in 2in">"#,
                r#"<field name="note" w="4in" h="0.3in"><ui><textEdit/></ui><bind match="global"/></field><subform name="table" layout="table" columnWidths="3in 2in">"#,
            )
            .replace(
                r#"xfa.host.messageBox("Hello " + qty.rawValue);"#,
                r#"details.note.fillColor = "51,102,153"; details.presence = "visible"; var all = xfa.form.form.page1.nodes; for (var i = 0; i &lt; all.length; i++) { if (all.item(i).className != "field" || all.item(i).name != "legacy") all.item(i).presence = "hidden"; }"#,
            )
            .replace(r#"$host.messageBox("FormCalc")"#, r#"page1.qty.presence = "visible"
page1.details.presence = "visible"
page1.note.presence = "visible""#);
        let mut s = crate::Session::new();
        let id = s.open("hidden.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&template)), None).unwrap();
        let _ = s.take_js_output(id);
        let out = s.run_javascript(id, "", Some("hello")).unwrap();
        assert!(out.error.is_none(), "{out:?}");
        let d = s.get(id).unwrap();
        assert!(!page_ops(&d.bytes).contains(FILL), "hidden: nothing drawn");
        let kept = pdfcraft_xfa::overrides(&pdfcraft_cos::Document::open(d.bytes.clone()).unwrap());
        assert_eq!(kept.props().keys().cloned().collect::<Vec<_>>(), ["form[0].page1[0].details[0].note[0]"], "{out:?}");
        let saved = s.save_bytes(id).unwrap();
        let mut again = crate::Session::new();
        let reopened = again.open("saved.pdf", None, saved, None).unwrap();
        assert!(!page_ops(&again.get(reopened).unwrap().bytes).contains(FILL));
        let out = again.run_javascript(reopened, "", Some("legacy")).unwrap();
        assert!(out.error.is_none(), "{out:?}");
        let d = again.get(reopened).unwrap();
        let ops = page_ops(&d.bytes);
        assert_eq!(ops.matches(FILL).count(), 1, "shown again, the one field filled; its globally bound twin is not");
        let saved = again.save_bytes(reopened).unwrap();
        let doc = pdfcraft_cos::Document::open(saved).unwrap();
        assert_eq!(pdfcraft_xfa::overrides(&doc).props().len(), 1);
    }

    /// A later script of the same event reads a radio button's value as the form gives it once
    /// an earlier one changed the button's on value (its items): the form is built again for
    /// such a change, as it is for the next event, not only refreshed.
    #[test]
    fn a_later_script_reads_a_value_an_items_change_moved() {
        let template = r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="s" layout="tb">
  <exclGroup name="g" layout="lr-tb">
   <field name="a" w="1in" h="0.3in"><ui><checkButton shape="round"/></ui><items><text>1</text></items></field>
   <field name="b" w="1in" h="0.3in"><ui><checkButton shape="round"/></ui><items><text>2</text></items></field>
  </exclGroup>
  <field name="pick" w="1in" h="0.3in"><ui><button/></ui><event activity="click"><script contentType="application/x-javascript">g.rawValue = "1";</script></event></field>
  <field name="go" w="1in" h="0.3in"><ui><button/></ui>
   <event activity="click"><script contentType="application/x-javascript">g.a.items.nodes.item(0).value = "9";</script></event>
   <event activity="click"><script contentType="application/x-javascript">xfa.host.messageBox("[" + g.a.rawValue + "]");</script></event>
  </field>
 </subform>
</subform>
</template>
</xdp:xdp>
"##;
        let mut s = crate::Session::new();
        let id = s.open("radio.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(template)), None).unwrap();
        s.apply(id, crate::Edit::XfaEvent { som: "form[0].s[0].pick[0]".into(), activity: "click".into() }).unwrap();
        let _ = s.take_js_output(id);
        s.apply(id, crate::Edit::XfaEvent { som: "form[0].s[0].go[0]".into(), activity: "click".into() }).unwrap();
        let alerts: Vec<String> = s.take_js_output(id).alerts.iter().map(|m| m.text.clone()).collect();
        // The group still holds "1", which is no longer the button's on value: not chosen.
        assert_eq!(alerts, ["[null]"]);
    }

    /// A script never reads back a property change the form refuses: on a field already holding
    /// the 64 changes the form keeps for one object (made by its opening script), a later
    /// script's write to another property is left out, and it reads the property as it was.
    #[test]
    fn a_write_past_the_objects_limit_is_not_read_back() {
        assert_eq!(pdfcraft_js::xfa::MAX_CHANGES_PER_OBJECT, pdfcraft_xfa::restyle::MAX_PROP_CHANGES_PER_OBJECT);
        let template = r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="s" layout="tb">
  <field name="full" w="2in" h="0.3in"><ui><textEdit/></ui><font typeface="Arial" size="10pt"/>
   <event activity="initialize"><script contentType="application/x-javascript">for (var i = 0; i &lt; 64; i++) { this.font["x" + i] = "1"; }</script></event>
   <event activity="click"><script contentType="application/x-javascript">this.font.size = "20pt"; this.font.x3 = "2"; xfa.host.messageBox(this.font.size + "|" + this.font.x3);</script></event>
  </field>
 </subform>
</subform>
</template>
</xdp:xdp>
"##;
        let mut s = crate::Session::new();
        let id = s.open("full.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(template)), None).unwrap();
        let _ = s.take_js_output(id);
        s.apply(id, crate::Edit::XfaEvent { som: "form[0].s[0].full[0]".into(), activity: "click".into() }).unwrap();
        let alerts: Vec<String> = s.take_js_output(id).alerts.iter().map(|m| m.text.clone()).collect();
        assert_eq!(alerts, ["10pt|2"], "the size is left out, the change to a kept property is not");
        let doc = pdfcraft_cos::Document::open(s.get(id).unwrap().bytes.clone()).unwrap();
        let kept = pdfcraft_xfa::overrides(&doc).props().get("form[0].s[0].full[0]").cloned().unwrap_or_default();
        assert_eq!(kept.len(), 64);
        assert_eq!(kept.get("font#0@x3").map(String::as_str), Some("2"));
        assert!(!kept.contains_key("font#0@size"));
    }

    /// Hostile property writes leave the form whole: absurd sizes and reserves, NaN and infinite
    /// measurements, an item past the reader's list, deep paths and a write to every field many
    /// times over are refused or clamped, with nothing past the form's limits kept.
    #[test]
    fn hostile_property_writes_from_scripts_are_bounded() {
        let script = r#"
            price.caption.reserve = "1e308in"; price.caption.reserve = "NaNpt"; price.caption.reserve = "-5in";
            price.font.size = "Infinity"; price.caption.font.size = "100000in";
            more.items.nodes.item(0).value = new Array(70000).join("x");
            for (var i = 0; i &lt; 20000; i++) { qty.border.fill.color.value = i % 255 + ",0,0"; price.assist.toolTip.value = "t" + i; }
            try { var p = qty.extras; for (var d = 0; d &lt; 40; d++) { p = p.getElement("extras", 0); } p.x = "1"; } catch (e) {}
            var q = qty.resolveNode("extras.extras.extras.extras.extras.extras.extras.extras.extras.extras"); if (q) q.x = "1";
        "#;
        let template = pdfcraft_xfa::fixtures::scripted_template().replace(r#"xfa.host.messageBox("Hello " + qty.rawValue);"#, script);
        let mut s = crate::Session::new();
        let id = s.open("hostile.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&template)), None).unwrap();
        let _ = s.take_js_output(id);
        let out = s.run_javascript(id, "", Some("hello")).unwrap();
        assert!(out.error.is_none(), "{out:?}");
        let d = s.get(id).unwrap();
        assert!(!d.form.is_empty(), "the form is still laid out: {out:?} {:?}", s.take_js_output(id));
        let doc = pdfcraft_cos::Document::open(d.bytes.clone()).unwrap();
        let ov = pdfcraft_xfa::overrides(&doc);
        assert!(ov.props().values().map(|m| m.len()).sum::<usize>() <= pdfcraft_xfa::restyle::MAX_PROP_OVERRIDES);
        assert!(ov.props().values().flat_map(|m| m.values()).all(|v| v.len() <= pdfcraft_xfa::restyle::MAX_PROP_VALUE));
        // The last tooltip wins; the 70,000-byte item text was cut to what a value keeps.
        let price = ov.props().get("form[0].page1[0].price[0]").cloned().unwrap_or_default();
        assert_eq!(price.get("assist#0/toolTip#0").map(String::as_str), Some("t19999"));
        let item = ov.props().get("form[0].page1[0].more[0]").and_then(|m| m.get("items#0/integer#0")).map_or(0, String::len);
        assert!(item < 70_000, "{item}");
        let saved = s.save_bytes(id).unwrap();
        let mut again = crate::Session::new();
        assert!(again.open("saved.pdf", None, saved, None).is_ok());
    }

    /// A form of `n` fields, each with an initialize script that colours it (a name holding a
    /// `.`, as CFIA 5800's `CheckList20.10` does, when `dotted`), opened: the live form built,
    /// the copies of it made for the scripts and the layouts, counted.
    fn opening_work(n: usize, dotted: bool) -> (usize, usize, usize) {
        let fields: String = (0..n)
            .map(|i| {
                let name = if dotted { format!("f{i}.x") } else { format!("f{i}") };
                format!(
                    r#"<field name="{name}" w="2in" h="0.3in"><ui><textEdit/></ui><event activity="initialize"><script contentType="application/x-javascript">this.border.fill.color.value = "51,102,153"; this.assist.toolTip.value = "Tip {i}";</script></event></field>"#
                )
            })
            .collect();
        let template = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
<pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
{fields}</subform></template></xdp:xdp>"#
        );
        let mut doc = pdfcraft_cos::Document::open(Arc::new(pdfcraft_xfa::fixtures::shell(&template))).unwrap();
        pdfcraft_xfa::render_into(&mut doc).unwrap();
        let tpl = template_of_doc(&doc);
        let mut out = JsOutput::default();
        let mut run = XfaRun { page_count: 1, out: &mut out, datasets: None, ran_away: false, time_scale: 1000, highlight: false };
        let _ = counts::take();
        let changes = on_open(&mut doc, &tpl, &mut run).unwrap();
        assert_eq!(changes.properties, 2 * n, "{changes:?}");
        counts::take()
    }

    /// The work an event does doesn't grow with the property changes its scripts make: one live
    /// form, one copy of it for the scripts and one layout, for 1 field's scripts or 200 (names
    /// holding a `.` too). Timing-independent: counts, not seconds.
    #[test]
    fn property_writes_dont_multiply_the_work_of_an_event() {
        for dotted in [false, true] {
            for n in [1, 200] {
                assert_eq!(opening_work(n, dotted), (1, 1, 1), "{n} fields, dotted names {dotted}");
            }
        }
    }

    /// A later script of the same event reads what an earlier one wrote to a property: the
    /// change goes with the copy of the form the later script is given.
    #[test]
    fn a_later_script_in_the_event_reads_an_earlier_scripts_property_change() {
        let template = r#"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
<pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
<field name="a" w="2in" h="0.3in"><ui><textEdit/></ui><event activity="initialize"><script contentType="application/x-javascript">b.border.fill.color.value = "51,102,153"; b.assist.toolTip.value = "Set by a";</script></event></field>
<field name="b" w="4in" h="0.3in"><ui><textEdit/></ui><event activity="initialize"><script contentType="application/x-javascript">this.rawValue = this.border.fill.color.value + "|" + this.assist.toolTip.value;</script></event></field>
</subform></template></xdp:xdp>"#;
        let mut s = crate::Session::new();
        let id = s.open("later.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(template)), None).unwrap();
        let b = s.get(id).unwrap().form.iter().find(|f| f.name.ends_with('b')).map(|f| f.value.clone()).unwrap_or_default();
        assert_eq!(b, vec!["51,102,153|Set by a".to_string()]);
    }
}
