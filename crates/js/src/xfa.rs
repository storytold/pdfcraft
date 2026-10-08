//! The XFA scripting object model (XFA 3.3 part 6, "Scripting"): `xfa.form`, `this`, `xfa.host`,
//! `xfa.layout`, `xfa.event`, `xfa.datasets`, instance managers and SOM resolution, as native
//! objects over a snapshot of the form tree the caller hands in. Scripts work on the snapshot
//! and everything they change or ask for comes back as [`XfaEffect`]s for the engine to apply.
//!
//! Form nodes are Proxies, because XFA scripts address children by name (`page1.familyName`)
//! and instance managers by `_name`. Unqualified names resolve the XFA way: the current node's
//! children, then its siblings, then each ancestor's children.

use std::cell::RefCell;
use std::collections::HashMap;

use boa_engine::object::ObjectInitializer;
use boa_engine::object::builtins::{JsArray, JsProxy};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsObject, JsResult, JsString, JsValue, NativeFunction, Source, js_string};

use crate::{Limits, arg, as_number, error, function, platform, printd, printf, printx, refuse, s, text};

/// What kind of template object a node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XfaKind {
    /// `xfa.form` itself.
    Form,
    Subform,
    Field,
    ExclGroup,
    Draw,
    Area,
}

impl XfaKind {
    fn class_name(self) -> &'static str {
        match self {
            XfaKind::Form => "form",
            XfaKind::Subform => "subform",
            XfaKind::Field => "field",
            XfaKind::ExclGroup => "exclGroup",
            XfaKind::Draw => "draw",
            XfaKind::Area => "area",
        }
    }
}

/// One object of the form, as the caller snapshots it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XfaNode {
    pub name: String,
    /// The SOM expression naming this instance (`form[0].head[0].familyName[0]`).
    pub som: String,
    pub kind: Option<XfaKind>,
    /// Fields: the raw value (ISO dates, on values for buttons); empty when null.
    pub value: String,
    /// Numeric fields give numbers to scripts.
    pub numeric: bool,
    /// `visible`, `invisible`, `hidden` or `inactive`.
    pub presence: String,
    /// `open`, `readOnly`, `protected` or `nonInteractive`.
    pub access: String,
    /// Subforms: may have several instances (`occur max` other than 1).
    pub repeatable: bool,
    pub occur_min: usize,
    pub occur_max: Option<usize>,
    /// This instance's index among its same-named siblings.
    pub index: usize,
    pub children: Vec<XfaNode>,
}

/// The event a script runs for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XfaEvent {
    /// `click`, `calculate`, `validate`, `initialize`, `change`, `exit`, …
    pub activity: String,
    /// The SOM of `this`.
    pub target: String,
    pub new_text: String,
    pub prev_text: String,
}

/// About the document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XfaDoc {
    pub file_name: String,
    /// The current page, 0-based.
    pub page: usize,
    pub page_count: usize,
}

/// Something a script changed or asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum XfaEffect {
    SetValue {
        som: String,
        value: String,
    },
    SetPresence {
        som: String,
        presence: String,
    },
    SetAccess {
        som: String,
        access: String,
    },
    /// A new instance of a repeating subform, named by its own SOM (`…row[3]`).
    AddInstance {
        som: String,
    },
    /// The instance at this SOM is removed.
    RemoveInstance {
        som: String,
    },
    MessageBox(String),
    /// `xfa.host.resetData()`: the named fields, or every field when empty.
    ResetData(Vec<String>),
    Print,
    SaveAs,
    LaunchUrl(String),
    SetFocus(String),
    Beep,
    /// `xfa.form.recalculate(true)`.
    Recalculate,
    /// `xfa.form.remerge()`, `xfa.layout.relayout()`.
    Relayout,
}

/// What running a script did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XfaOutcome {
    pub effects: Vec<XfaEffect>,
    pub console: Vec<String>,
    /// The script's error, if it failed.
    pub error: Option<String>,
    /// The script's completion value (a calculate script's result; a validate script's verdict).
    pub result: Option<String>,
    /// The completion value as a boolean, when it was one.
    pub result_bool: Option<bool>,
    /// Limits the script ran into (too many effects, message boxes or console lines): what
    /// was left out, for the caller to report.
    pub notes: Vec<String>,
    /// The script ran past its time limit and was left running on its own thread (the engine
    /// can't interrupt it): the caller should run no more of this form's scripts.
    pub abandoned: bool,
}

/// Most nodes a snapshot may hold.
const MAX_NODES: usize = 200_000;
/// Most instances a script may add in one run.
const MAX_ADDED: usize = 1_000;
/// Deepest tree followed.
const MAX_DEPTH: usize = 64;
/// Most effects one run may produce (after values set again on the same object are merged).
pub const MAX_EFFECTS: usize = 10_000;
/// Most message boxes one run may show.
pub const MAX_ALERTS: usize = 100;
/// Most console lines one run may print.
pub const MAX_CONSOLE: usize = 1_000;
/// Longest message or console line kept, in characters.
const MAX_LINE: usize = 4_096;

/// `m` cut to [`MAX_LINE`] characters.
fn clip(m: String) -> String {
    match m.char_indices().nth(MAX_LINE) {
        Some((at, _)) => format!("{}…", m.get(..at).unwrap_or_default()),
        None => m,
    }
}

struct HNode {
    name: String,
    som: String,
    kind: XfaKind,
    value: String,
    numeric: bool,
    presence: String,
    access: String,
    repeatable: bool,
    occur_min: usize,
    occur_max: Option<usize>,
    index: usize,
    parent: Option<usize>,
    children: Vec<usize>,
    /// Removed by `removeInstance`: skipped everywhere.
    gone: bool,
}

struct XHost {
    nodes: Vec<HNode>,
    effects: Vec<XfaEffect>,
    console: Vec<String>,
    doc: XfaDoc,
    added: usize,
    /// Index of `this`.
    current: usize,
    /// Where the effect setting a property of an object is in `effects`, by (property, SOM):
    /// setting it again replaces that effect (the last value wins) unless something that
    /// depends on order (rows added or removed, a reset) came after it.
    set_at: HashMap<(u8, String), usize>,
    /// Effects before this index are behind an order-dependent effect.
    barrier: usize,
    alerts: usize,
    dropped_effects: usize,
    dropped_alerts: usize,
    dropped_console: usize,
}

impl XHost {
    /// Record an effect: values, presence and access set again on the same object replace the
    /// earlier one; past [`MAX_EFFECTS`] (or [`MAX_ALERTS`] message boxes) effects are counted
    /// and dropped.
    fn push(&mut self, e: XfaEffect) {
        let key = match &e {
            XfaEffect::SetValue { som, .. } => Some((0u8, som.clone())),
            XfaEffect::SetPresence { som, .. } => Some((1, som.clone())),
            XfaEffect::SetAccess { som, .. } => Some((2, som.clone())),
            _ => None,
        };
        if let Some(key) = key {
            if let Some(&at) = self.set_at.get(&key)
                && at >= self.barrier
                && let Some(slot) = self.effects.get_mut(at)
            {
                *slot = e;
                return;
            }
            if self.effects.len() >= MAX_EFFECTS {
                self.dropped_effects += 1;
                return;
            }
            self.set_at.insert(key, self.effects.len());
            self.effects.push(e);
            return;
        }
        if matches!(e, XfaEffect::MessageBox(_)) {
            if self.alerts >= MAX_ALERTS {
                self.dropped_alerts += 1;
                return;
            }
            self.alerts += 1;
        }
        if self.effects.len() >= MAX_EFFECTS {
            self.dropped_effects += 1;
            return;
        }
        if matches!(e, XfaEffect::AddInstance { .. } | XfaEffect::RemoveInstance { .. } | XfaEffect::ResetData(_)) {
            self.barrier = self.effects.len() + 1;
        }
        self.effects.push(e);
    }

    fn print(&mut self, line: String) {
        if self.console.len() >= MAX_CONSOLE {
            self.dropped_console += 1;
        } else {
            self.console.push(line);
        }
    }

    /// What the limits left out.
    fn notes(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.dropped_effects > 0 {
            out.push(format!("the script made more than {MAX_EFFECTS} changes; {} more were left out", self.dropped_effects));
        }
        if self.dropped_alerts > 0 {
            out.push(format!("the script showed more than {MAX_ALERTS} messages; {} more were left out", self.dropped_alerts));
        }
        if self.dropped_console > 0 {
            out.push(format!("the script printed more than {MAX_CONSOLE} console lines; {} more were left out", self.dropped_console));
        }
        out
    }
}

type Shared = RefCell<XHost>;

fn host(ctx: &Context) -> JsResult<&Shared> {
    ctx.get_data::<Shared>().ok_or_else(|| error("the XFA script host is not set up"))
}

fn flatten(n: &XfaNode, parent: Option<usize>, out: &mut Vec<HNode>, depth: usize) -> Option<usize> {
    if depth > MAX_DEPTH || out.len() >= MAX_NODES {
        return None;
    }
    let idx = out.len();
    out.push(HNode {
        name: n.name.clone(),
        som: n.som.clone(),
        kind: n.kind.unwrap_or(XfaKind::Subform),
        value: n.value.clone(),
        numeric: n.numeric,
        presence: if n.presence.is_empty() { "visible".into() } else { n.presence.clone() },
        access: if n.access.is_empty() { "open".into() } else { n.access.clone() },
        repeatable: n.repeatable,
        occur_min: n.occur_min,
        occur_max: n.occur_max,
        index: n.index,
        parent,
        children: Vec::new(),
        gone: false,
    });
    let mut kids = Vec::new();
    for c in &n.children {
        if let Some(ci) = flatten(c, Some(idx), out, depth + 1) {
            kids.push(ci);
        }
    }
    if let Some(h) = out.get_mut(idx) {
        h.children = kids;
    }
    Some(idx)
}

// ── node objects ────────────────────────────────────────────────────────────────────────────

const IDX: &str = "__xfaNode";

fn node_index(target: &JsValue, ctx: &mut Context) -> JsResult<usize> {
    let o = target.as_object().ok_or_else(|| error("not an XFA node"))?;
    let v = o.get(JsString::from(IDX), ctx)?;
    let n = v.to_number(ctx)?;
    if !n.is_finite() || n < 0.0 {
        return Err(error("not an XFA node"));
    }
    Ok(n as usize)
}

/// A Proxy standing for node `idx`.
fn node_object(ctx: &mut Context, idx: usize) -> JsResult<JsValue> {
    let target = ObjectInitializer::new(ctx).property(JsString::from(IDX), JsValue::from(idx as f64), Attribute::READONLY).build();
    let proxy = JsProxy::builder(target).get(node_get).set(node_set).has(node_has).build(ctx)?;
    Ok(proxy.into())
}

/// An object that takes any property read or write without complaint (`this.border.fill…`,
/// `this.fillColor = …`): what a script sets there changes nothing here.
fn sink(ctx: &mut Context) -> JsResult<JsValue> {
    let target = ObjectInitializer::new(ctx).build();
    let proxy = JsProxy::builder(target).get(sink_get).set(sink_set).build(ctx)?;
    Ok(proxy.into())
}

fn sink_get(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let key = arg(args, 1);
    if key.as_symbol().is_some() {
        return Ok(JsValue::undefined());
    }
    match text(&key, ctx)?.as_str() {
        "value" | "toString" | "valueOf" => Ok(s("")),
        _ => sink(ctx),
    }
}

fn sink_set(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(true))
}

fn node_has(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(true))
}

fn value_js(h: &HNode) -> JsValue {
    if h.value.is_empty() {
        return JsValue::null();
    }
    if h.numeric
        && let Some(n) = as_number(&h.value)
    {
        return JsValue::from(n);
    }
    s(&h.value)
}

/// `(idx, name)` of the child of `parent` named `name`, first instance.
fn child_named(h: &XHost, parent: usize, name: &str) -> Option<usize> {
    let p = h.nodes.get(parent)?;
    p.children.iter().copied().find(|&c| h.nodes.get(c).is_some_and(|n| !n.gone && n.name == name))
}

/// Every live instance named `name` under `parent`, in order.
fn instances(h: &XHost, parent: usize, name: &str) -> Vec<usize> {
    h.nodes
        .get(parent)
        .map(|p| p.children.iter().copied().filter(|&c| h.nodes.get(c).is_some_and(|n| !n.gone && n.name == name)).collect())
        .unwrap_or_default()
}

fn list_object(ctx: &mut Context, items: &[usize]) -> JsResult<JsValue> {
    let vals: Vec<JsValue> = items.iter().map(|i| JsValue::from(*i as f64)).collect();
    let arr = JsArray::from_iter(vals, ctx);
    let arr_obj: JsObject = arr.into();
    fn item(_: &JsValue, args: &[JsValue], arr: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
        let i = arg(args, 0).to_number(ctx)?;
        if !i.is_finite() || i < 0.0 {
            return Ok(JsValue::null());
        }
        let v = JsArray::from_object(arr.clone())?.get(i as u64, ctx)?;
        if v.is_undefined() {
            return Ok(JsValue::null());
        }
        let idx = v.to_number(ctx)? as usize;
        node_object(ctx, idx)
    }
    let item_fn = function(ctx, NativeFunction::from_copy_closure_with_captures(item, arr_obj));
    Ok(ObjectInitializer::new(ctx)
        .property(js_string!("length"), JsValue::from(items.len() as f64), Attribute::READONLY)
        .property(js_string!("item"), item_fn, Attribute::all())
        .build()
        .into())
}

/// The instance manager of the subform named `name` under `parent`.
fn instance_manager(ctx: &mut Context, parent: usize, name: &str) -> JsResult<JsValue> {
    let (count, min, max) = {
        let h = host(ctx)?.borrow();
        let live = instances(&h, parent, name);
        let first = live.first().and_then(|i| h.nodes.get(*i));
        (live.len(), first.map_or(0, |n| n.occur_min), first.and_then(|n| n.occur_max))
    };
    let key = ObjectInitializer::new(ctx)
        .property(js_string!("parent"), JsValue::from(parent as f64), Attribute::READONLY)
        .property(js_string!("name"), s(name), Attribute::READONLY)
        .build();
    fn target_of(key: &JsObject, ctx: &mut Context) -> JsResult<(usize, String)> {
        let p = key.get(js_string!("parent"), ctx)?.to_number(ctx)? as usize;
        let n = text(&key.get(js_string!("name"), ctx)?, ctx)?;
        Ok((p, n))
    }
    fn add_instance(_: &JsValue, _: &[JsValue], key: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
        let (p, n) = target_of(key, ctx)?;
        match add_instance_of(ctx, p, &n)? {
            Some(i) => node_object(ctx, i),
            None => Ok(JsValue::null()),
        }
    }
    fn insert_instance(this: &JsValue, args: &[JsValue], key: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
        add_instance(this, args, key, ctx)
    }
    fn remove_instance(_: &JsValue, args: &[JsValue], key: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
        let (p, n) = target_of(key, ctx)?;
        let i = arg(args, 0).to_number(ctx)?;
        if i.is_finite() && i >= 0.0 {
            remove_instance_of(ctx, p, &n, i as usize)?;
        }
        Ok(JsValue::undefined())
    }
    fn set_instances(_: &JsValue, args: &[JsValue], key: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
        let (p, n) = target_of(key, ctx)?;
        let want = arg(args, 0).to_number(ctx)?;
        if !want.is_finite() || want < 0.0 {
            return Ok(JsValue::undefined());
        }
        let want = (want as usize).min(MAX_ADDED);
        // Bounded: adding stops at the cap or the maximum, removing at the floor, and a step
        // that changes nothing ends the loop.
        for _ in 0..=MAX_ADDED {
            let have = instances(&host(ctx)?.borrow(), p, &n).len();
            if have < want {
                if add_instance_of(ctx, p, &n)?.is_none() {
                    break;
                }
            } else if have > want && have > 0 {
                remove_instance_of(ctx, p, &n, have - 1)?;
                if instances(&host(ctx)?.borrow(), p, &n).len() == have {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(JsValue::undefined())
    }
    fn move_instance(_: &JsValue, _: &[JsValue], _: &JsObject, _: &mut Context) -> JsResult<JsValue> {
        Ok(JsValue::undefined())
    }
    fn count_get(_: &JsValue, _: &[JsValue], key: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
        let (p, n) = target_of(key, ctx)?;
        let c = instances(&host(ctx)?.borrow(), p, &n).len();
        Ok(JsValue::from(c as f64))
    }
    fn count_set(_: &JsValue, args: &[JsValue], key: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
        set_instances(&JsValue::undefined(), args, key, ctx)
    }
    let count_g = function(ctx, NativeFunction::from_copy_closure_with_captures(count_get, key.clone()));
    let count_s = function(ctx, NativeFunction::from_copy_closure_with_captures(count_set, key.clone()));
    let add = function(ctx, NativeFunction::from_copy_closure_with_captures(add_instance, key.clone()));
    let insert = function(ctx, NativeFunction::from_copy_closure_with_captures(insert_instance, key.clone()));
    let remove = function(ctx, NativeFunction::from_copy_closure_with_captures(remove_instance, key.clone()));
    let set = function(ctx, NativeFunction::from_copy_closure_with_captures(set_instances, key.clone()));
    let mv = function(ctx, NativeFunction::from_copy_closure_with_captures(move_instance, key));
    let _ = count;
    let o = ObjectInitializer::new(ctx)
        .property(js_string!("min"), JsValue::from(min as f64), Attribute::READONLY)
        .property(js_string!("max"), max.map_or(JsValue::from(-1.0), |m| JsValue::from(m as f64)), Attribute::READONLY)
        .property(js_string!("addInstance"), add, Attribute::all())
        .property(js_string!("insertInstance"), insert, Attribute::all())
        .property(js_string!("removeInstance"), remove, Attribute::all())
        .property(js_string!("setInstances"), set, Attribute::all())
        .property(js_string!("moveInstance"), mv, Attribute::all())
        .property(js_string!("className"), s("instanceManager"), Attribute::READONLY)
        .build();
    use boa_engine::property::PropertyDescriptor;
    o.define_property_or_throw(js_string!("count"), PropertyDescriptor::builder().get(count_g).set(count_s).configurable(true), ctx)?;
    Ok(o.into())
}

/// Re-derive the SOM of `idx` and everything under it from its parent and index.
fn renumber(h: &mut XHost, idx: usize, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let (parent_som, name, index, kids) = {
        let Some(n) = h.nodes.get(idx) else { return };
        let parent_som = n.parent.and_then(|p| h.nodes.get(p)).map(|p| p.som.clone()).unwrap_or_default();
        (parent_som, n.name.clone(), n.index, n.children.clone())
    };
    let som = if name.starts_with('#') || name.is_empty() {
        parent_som
    } else if parent_som.is_empty() {
        format!("{name}[{index}]")
    } else {
        format!("{parent_som}.{name}[{index}]")
    };
    if let Some(n) = h.nodes.get_mut(idx) {
        n.som = som;
    }
    for k in kids {
        renumber(h, k, depth + 1);
    }
}

/// A copy of `src`'s subtree (values cleared) under `parent`, returning the copy's index.
fn clone_subtree(h: &mut XHost, src: usize, parent: Option<usize>, depth: usize) -> Option<usize> {
    if depth > MAX_DEPTH || h.nodes.len() >= MAX_NODES {
        return None;
    }
    let (template, kids) = {
        let n = h.nodes.get(src)?;
        (
            HNode {
                name: n.name.clone(),
                som: String::new(),
                kind: n.kind,
                value: String::new(),
                numeric: n.numeric,
                presence: n.presence.clone(),
                access: n.access.clone(),
                repeatable: n.repeatable,
                occur_min: n.occur_min,
                occur_max: n.occur_max,
                index: n.index,
                parent,
                children: Vec::new(),
                gone: false,
            },
            n.children.clone(),
        )
    };
    let idx = h.nodes.len();
    h.nodes.push(template);
    let mut copied = Vec::new();
    for k in kids {
        // Nested repeating subforms copy their first instance only.
        let skip = h.nodes.get(k).is_some_and(|c| c.gone || c.repeatable && c.index > 0);
        if skip {
            continue;
        }
        if let Some(ci) = clone_subtree(h, k, Some(idx), depth + 1) {
            copied.push(ci);
        }
    }
    if let Some(n) = h.nodes.get_mut(idx) {
        n.children = copied;
    }
    Some(idx)
}

fn add_instance_of(ctx: &mut Context, parent: usize, name: &str) -> JsResult<Option<usize>> {
    let mut h = host(ctx)?.borrow_mut();
    let live = instances(&h, parent, name);
    let Some(&first) = live.first() else { return Ok(None) };
    let (repeatable, max) = h.nodes.get(first).map(|n| (n.repeatable, n.occur_max)).unwrap_or((false, Some(1)));
    if !repeatable || max.is_some_and(|m| live.len() >= m) || h.added >= MAX_ADDED {
        return Ok(None);
    }
    let Some(new) = clone_subtree(&mut h, first, Some(parent), 0) else { return Ok(None) };
    let last = live.last().copied().unwrap_or(first);
    if let Some(n) = h.nodes.get_mut(new) {
        n.index = live.len();
    }
    // Keep instances together: right after the last one.
    if let Some(p) = h.nodes.get_mut(parent) {
        let pos = p.children.iter().position(|&c| c == last).map_or(p.children.len(), |i| i + 1);
        p.children.insert(pos, new);
    }
    renumber(&mut h, new, 0);
    h.added += 1;
    let som = h.nodes.get(new).map(|n| n.som.clone()).unwrap_or_default();
    h.push(XfaEffect::AddInstance { som });
    Ok(Some(new))
}

fn remove_instance_of(ctx: &mut Context, parent: usize, name: &str, i: usize) -> JsResult<()> {
    let mut h = host(ctx)?.borrow_mut();
    let live = instances(&h, parent, name);
    let Some(&gone) = live.get(i) else { return Ok(()) };
    let min = h.nodes.get(gone).map_or(0, |n| n.occur_min).max(1);
    if live.len() <= min {
        return Ok(());
    }
    let som = h.nodes.get(gone).map(|n| n.som.clone()).unwrap_or_default();
    if let Some(n) = h.nodes.get_mut(gone) {
        n.gone = true;
    }
    // Later instances move down one.
    for (k, &inst) in live.iter().enumerate().skip(i + 1) {
        if let Some(n) = h.nodes.get_mut(inst) {
            n.index = k - 1;
        }
        renumber(&mut h, inst, 0);
    }
    h.push(XfaEffect::RemoveInstance { som });
    Ok(())
}

/// Resolve a SOM expression from node `from`: `$`, `$form`, `$record`, `$host`, `xfa.form…`,
/// `parent`, `..name` (descendant search), `name[n]`, `name[*]`, `_name`.
fn resolve_som(h: &XHost, from: usize, expr: &str) -> Vec<usize> {
    let expr = expr.trim();
    if expr.is_empty() {
        return vec![from];
    }
    let root = 0usize;
    let mut cur: Vec<usize> = vec![from];
    let mut rest = expr;
    let starts =
        [("$form", root), ("xfa.form", root), ("$record", root), ("xfa.datasets.data", root), ("xfa.record", root), ("$", from), ("this", from)];
    for (prefix, at) in starts {
        if let Some(r) = rest.strip_prefix(prefix)
            && (r.is_empty() || r.starts_with('.') || r.starts_with('['))
        {
            cur = vec![at];
            rest = r;
            break;
        }
    }
    let mut depth = 0;
    while !rest.is_empty() && depth < MAX_DEPTH {
        depth += 1;
        let descend = rest.starts_with("..");
        rest = rest.trim_start_matches('.');
        let end = rest.find(['.', '[']).unwrap_or(rest.len());
        let (name, after) = rest.split_at(end);
        let name = name.trim();
        let (index, after) = match after.strip_prefix('[') {
            Some(a) => {
                let close = a.find(']').unwrap_or(a.len());
                let idx = a.get(..close).map(str::trim).unwrap_or("");
                (Some(idx.to_string()), a.get(close + 1..).unwrap_or(""))
            }
            None => (None, after),
        };
        rest = after;
        let mut next = Vec::new();
        for &c in &cur {
            match name {
                "" => {}
                "parent" => {
                    if let Some(p) = h.nodes.get(c).and_then(|n| n.parent) {
                        next.push(p);
                    }
                }
                _ => {
                    let mut found: Vec<usize> = if descend { descendants_named(h, c, name, 0) } else { instances(h, c, name) };
                    // A relative name a node has no child for is looked for among its siblings
                    // and in each enclosing container, nearest first (so a field's script can
                    // say `resolveNode("otherField")`).
                    if found.is_empty() && depth == 1 && !descend {
                        let mut at = h.nodes.get(c).and_then(|n| n.parent);
                        let mut guard = 0;
                        while let Some(p) = at
                            && guard < MAX_DEPTH
                        {
                            found = instances(h, p, name);
                            if !found.is_empty() {
                                break;
                            }
                            at = h.nodes.get(p).and_then(|n| n.parent);
                            guard += 1;
                        }
                    }
                    match index.as_deref() {
                        Some("*") => next.extend(found),
                        Some(i) => {
                            if let Some(k) = i.parse::<usize>().ok().and_then(|i| found.get(i)) {
                                next.push(*k);
                            }
                        }
                        None => next.extend(found.first()),
                    }
                }
            }
        }
        cur = next;
        if cur.is_empty() {
            break;
        }
    }
    cur
}

fn descendants_named(h: &XHost, from: usize, name: &str, depth: usize) -> Vec<usize> {
    let mut out = Vec::new();
    if depth > MAX_DEPTH {
        return out;
    }
    let Some(n) = h.nodes.get(from) else { return out };
    for &c in &n.children {
        if let Some(k) = h.nodes.get(c)
            && !k.gone
        {
            if k.name == name {
                out.push(c);
            } else {
                out.extend(descendants_named(h, c, name, depth + 1));
            }
        }
    }
    out
}

fn node_get(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let idx = node_index(&arg(args, 0), ctx)?;
    let key = arg(args, 1);
    if key.as_symbol().is_some() {
        return Ok(JsValue::undefined());
    }
    let key = text(&key, ctx)?;
    let snapshot = {
        let h = host(ctx)?.borrow();
        let Some(n) = h.nodes.get(idx) else { return Ok(JsValue::undefined()) };
        (n.name.clone(), n.som.clone(), n.kind, n.value.clone(), n.presence.clone(), n.access.clone(), n.index, n.parent, n.children.clone())
    };
    let (name, som, kind, value, presence, access, index, parent, children) = snapshot;
    Ok(match key.as_str() {
        IDX => JsValue::from(idx as f64),
        "rawValue" | "value" => host(ctx)?.borrow().nodes.get(idx).map_or(JsValue::null(), value_js),
        "formattedValue" => s(&value),
        "name" => s(&name),
        "somExpression" => s(&som),
        "className" => s(kind.class_name()),
        "index" | "instanceIndex" => JsValue::from(index as f64),
        "presence" => s(&presence),
        "access" => s(&access),
        "isNull" => JsValue::from(value.is_empty()),
        "mandatory" => s("disabled"),
        "parent" => match parent {
            Some(p) => node_object(ctx, p)?,
            None => JsValue::null(),
        },
        "nodes" => {
            let live: Vec<usize> = {
                let h = host(ctx)?.borrow();
                children.iter().copied().filter(|&c| h.nodes.get(c).is_some_and(|n| !n.gone)).collect()
            };
            list_object(ctx, &live)?
        }
        "all" => {
            let live = match parent {
                Some(p) => instances(&host(ctx)?.borrow(), p, &name),
                None => vec![idx],
            };
            list_object(ctx, &live)?
        }
        "instanceManager" => match parent {
            Some(p) => instance_manager(ctx, p, &name)?,
            None => JsValue::undefined(),
        },
        "resolveNode" => resolver(ctx, idx, false)?,
        "resolveNodes" => resolver(ctx, idx, true)?,
        "recalculate" => function(ctx, NativeFunction::from_fn_ptr(recalculate)).into(),
        "remerge" => function(ctx, NativeFunction::from_fn_ptr(relayout)).into(),
        "getAttribute"
        | "setAttribute"
        | "execEvent"
        | "execInitialize"
        | "execCalculate"
        | "execValidate"
        | "getElement"
        | "setElement"
        | "isPropertySpecified"
        | "loadXML"
        | "saveXML"
        | "clone" => function(ctx, NativeFunction::from_fn_ptr(noop)).into(),
        "toString" | "valueOf" | "toJSON" => function(ctx, NativeFunction::from_fn_ptr(node_to_string)).into(),
        "length" => JsValue::undefined(),
        "border" | "ui" | "font" | "caption" | "fillColor" | "borderColor" | "fontColor" | "margin" | "para" | "assist" | "validate" | "format"
        | "items" | "bind" | "keep" | "h" | "w" | "x" | "y" | "minH" | "maxH" | "minW" | "maxW" | "layout" | "model" | "oneOfChild" | "locale"
        | "relevant" | "colSpan" | "dataNode" | "defaultValue" | "editValue" | "selectedIndex" | "event" | "calculate" | "traversal" | "extras"
        | "desc" => sink(ctx)?,
        other => {
            if let Some(child) = other.strip_prefix('_') {
                let exists = child_named(&host(ctx)?.borrow(), idx, child).is_some();
                return if exists { instance_manager(ctx, idx, child) } else { Ok(JsValue::undefined()) };
            }
            let found = child_named(&host(ctx)?.borrow(), idx, other);
            match found {
                Some(c) => node_object(ctx, c)?,
                None => JsValue::undefined(),
            }
        }
    })
}

fn node_set(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let idx = node_index(&arg(args, 0), ctx)?;
    let key = arg(args, 1);
    if key.as_symbol().is_some() {
        return Ok(JsValue::from(true));
    }
    let key = text(&key, ctx)?;
    let value = arg(args, 2);
    let value = if value.is_null() || value.is_undefined() { String::new() } else { text(&value, ctx)? };
    let mut h = host(ctx)?.borrow_mut();
    let Some(n) = h.nodes.get(idx) else { return Ok(JsValue::from(true)) };
    let som = n.som.clone();
    match key.as_str() {
        "rawValue" | "value" | "formattedValue" => {
            if let Some(n) = h.nodes.get_mut(idx) {
                n.value = value.clone();
            }
            h.push(XfaEffect::SetValue { som, value });
        }
        "presence" => {
            let p = match value.as_str() {
                "hidden" | "invisible" | "inactive" | "visible" => value.clone(),
                _ => return Ok(JsValue::from(true)),
            };
            if let Some(n) = h.nodes.get_mut(idx) {
                n.presence = p.clone();
            }
            h.push(XfaEffect::SetPresence { som, presence: p });
        }
        "access" => {
            let a = match value.as_str() {
                "open" | "readOnly" | "protected" | "nonInteractive" => value.clone(),
                _ => return Ok(JsValue::from(true)),
            };
            if let Some(n) = h.nodes.get_mut(idx) {
                n.access = a.clone();
            }
            h.push(XfaEffect::SetAccess { som, access: a });
        }
        _ => {}
    }
    Ok(JsValue::from(true))
}

fn node_to_string(this: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let idx = node_index(this, ctx)?;
    Ok(s(&host(ctx)?.borrow().nodes.get(idx).map(|n| n.value.clone()).unwrap_or_default()))
}

fn noop(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

/// `resolveNode(som)` / `resolveNodes(som)` bound to node `from`.
fn resolver(ctx: &mut Context, from: usize, many: bool) -> JsResult<JsValue> {
    let key = ObjectInitializer::new(ctx)
        .property(js_string!("from"), JsValue::from(from as f64), Attribute::READONLY)
        .property(js_string!("many"), JsValue::from(many), Attribute::READONLY)
        .build();
    fn resolve(_: &JsValue, args: &[JsValue], key: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
        let from = key.get(js_string!("from"), ctx)?.to_number(ctx)? as usize;
        let many = key.get(js_string!("many"), ctx)?.to_boolean();
        let expr = text(&arg(args, 0), ctx)?;
        let found = resolve_som(&host(ctx)?.borrow(), from, &expr);
        if many {
            return list_object(ctx, &found);
        }
        match found.first() {
            Some(i) => node_object(ctx, *i),
            None => Ok(JsValue::null()),
        }
    }
    Ok(function(ctx, NativeFunction::from_copy_closure_with_captures(resolve, key)).into())
}

// ── xfa.host, xfa.layout, xfa.event, app, console ───────────────────────────────────────────

fn effect(ctx: &mut Context, e: XfaEffect) -> JsResult<()> {
    host(ctx)?.borrow_mut().push(e);
    Ok(())
}

fn message_box(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = clip(text(&arg(args, 0), ctx)?);
    effect(ctx, XfaEffect::MessageBox(m))?;
    Ok(JsValue::from(1.0))
}

fn reset_data(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let list = text(&arg(args, 0), ctx)?;
    let names: Vec<String> = list.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
    effect(ctx, XfaEffect::ResetData(names))?;
    Ok(JsValue::undefined())
}

fn print(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    effect(ctx, XfaEffect::Print)?;
    Ok(JsValue::undefined())
}

fn beep(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    effect(ctx, XfaEffect::Beep)?;
    Ok(JsValue::undefined())
}

fn goto_url(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let u = text(&arg(args, 0), ctx)?;
    effect(ctx, XfaEffect::LaunchUrl(u))?;
    Ok(JsValue::undefined())
}

fn set_focus(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let target = arg(args, 0);
    let som = match target.as_object() {
        Some(_) if node_index(&target, ctx).is_ok() => {
            let i = node_index(&target, ctx)?;
            host(ctx)?.borrow().nodes.get(i).map(|n| n.som.clone()).unwrap_or_default()
        }
        _ => {
            let expr = text(&target, ctx)?;
            let h = host(ctx)?.borrow();
            let found = resolve_som(&h, h.current, &expr);
            found.first().and_then(|i| h.nodes.get(*i)).map(|n| n.som.clone()).unwrap_or(expr)
        }
    };
    effect(ctx, XfaEffect::SetFocus(som))?;
    Ok(JsValue::undefined())
}

fn exec_menu_item(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let item = text(&arg(args, 0), ctx)?;
    match item.as_str() {
        "SaveAs" | "Save" => effect(ctx, XfaEffect::SaveAs)?,
        "Print" => effect(ctx, XfaEffect::Print)?,
        _ => {}
    }
    Ok(JsValue::undefined())
}

fn recalculate(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    effect(ctx, XfaEffect::Recalculate)?;
    Ok(JsValue::undefined())
}

fn relayout(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    effect(ctx, XfaEffect::Relayout)?;
    Ok(JsValue::undefined())
}

fn console_println(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = clip(text(&arg(args, 0), ctx)?);
    host(ctx)?.borrow_mut().print(m);
    Ok(JsValue::undefined())
}

fn response(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::null())
}

fn layout_page(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().doc.page as f64 + 1.0))
}

fn layout_page0(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().doc.page as f64))
}

fn layout_page_count(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().doc.page_count as f64))
}

fn zero(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(0.0))
}

fn xfa_resolve(ctx: &mut Context, many: bool) -> JsResult<JsValue> {
    let current = host(ctx)?.borrow().current;
    resolver(ctx, current, many)
}

fn install(ctx: &mut Context, event: &XfaEvent, doc: &XfaDoc, current: usize) -> JsResult<()> {
    let rw = Attribute::CONFIGURABLE;
    let form = node_object(ctx, 0)?;
    let this_node = node_object(ctx, current)?;
    let host_obj = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(message_box), js_string!("messageBox"), 4)
        .function(NativeFunction::from_fn_ptr(reset_data), js_string!("resetData"), 1)
        .function(NativeFunction::from_fn_ptr(print), js_string!("print"), 8)
        .function(NativeFunction::from_fn_ptr(beep), js_string!("beep"), 1)
        .function(NativeFunction::from_fn_ptr(goto_url), js_string!("gotoURL"), 2)
        .function(NativeFunction::from_fn_ptr(set_focus), js_string!("setFocus"), 1)
        .function(NativeFunction::from_fn_ptr(response), js_string!("response"), 4)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("openList"), 1)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("exportData"), 2)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("importData"), 1)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("pageDown"), 0)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("pageUp"), 0)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("getFocus"), 0)
        .property(js_string!("currentPage"), JsValue::from(doc.page as f64), rw)
        .property(js_string!("numPages"), JsValue::from(doc.page_count as f64), rw)
        .property(js_string!("name"), s("Acrobat"), Attribute::READONLY)
        .property(js_string!("appType"), s("Exchange-Pro"), Attribute::READONLY)
        .property(js_string!("version"), s("11"), Attribute::READONLY)
        .property(js_string!("variation"), s("Full"), Attribute::READONLY)
        .property(js_string!("language"), s("en"), Attribute::READONLY)
        .property(js_string!("platform"), s(platform()), Attribute::READONLY)
        .property(js_string!("calculationsEnabled"), JsValue::from(true), rw)
        .property(js_string!("validationsEnabled"), JsValue::from(true), rw)
        .property(js_string!("title"), s(&doc.file_name), rw)
        .build();
    let layout = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(layout_page), js_string!("page"), 1)
        .function(NativeFunction::from_fn_ptr(layout_page0), js_string!("absPage"), 1)
        .function(NativeFunction::from_fn_ptr(layout_page_count), js_string!("pageCount"), 0)
        .function(NativeFunction::from_fn_ptr(layout_page_count), js_string!("absPageCount"), 0)
        .function(NativeFunction::from_fn_ptr(relayout), js_string!("relayout"), 0)
        .function(NativeFunction::from_fn_ptr(relayout), js_string!("relayoutPageArea"), 0)
        .function(NativeFunction::from_fn_ptr(zero), js_string!("h"), 2)
        .function(NativeFunction::from_fn_ptr(zero), js_string!("w"), 2)
        .function(NativeFunction::from_fn_ptr(zero), js_string!("x"), 2)
        .function(NativeFunction::from_fn_ptr(zero), js_string!("y"), 2)
        .function(NativeFunction::from_fn_ptr(zero), js_string!("pageSpan"), 1)
        .property(js_string!("ready"), JsValue::from(true), Attribute::READONLY)
        .build();
    let ev = ObjectInitializer::new(ctx)
        .property(js_string!("name"), s(&event.activity), Attribute::all())
        .property(js_string!("newText"), s(&event.new_text), Attribute::all())
        .property(js_string!("prevText"), s(&event.prev_text), Attribute::all())
        .property(js_string!("fullText"), s(&event.new_text), Attribute::all())
        .property(js_string!("change"), s(""), Attribute::all())
        .property(js_string!("target"), this_node.clone(), Attribute::all())
        .property(js_string!("selStart"), JsValue::from(0.0), Attribute::all())
        .property(js_string!("selEnd"), JsValue::from(0.0), Attribute::all())
        .property(js_string!("cancelAction"), JsValue::from(false), Attribute::all())
        .property(js_string!("reenter"), JsValue::from(false), Attribute::all())
        .property(js_string!("shift"), JsValue::from(false), Attribute::all())
        .property(js_string!("modifier"), JsValue::from(false), Attribute::all())
        .property(js_string!("keyDown"), JsValue::from(false), Attribute::all())
        .property(js_string!("commitKey"), JsValue::from(0.0), Attribute::all())
        .function(NativeFunction::from_fn_ptr(noop), js_string!("emit"), 0)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("reset"), 0)
        .build();
    let datasets = ObjectInitializer::new(ctx).property(js_string!("data"), form.clone(), rw).build();
    let template = sink(ctx)?;
    let resolve_one = xfa_resolve(ctx, false)?;
    let resolve_many = xfa_resolve(ctx, true)?;
    let xfa = ObjectInitializer::new(ctx)
        .property(js_string!("form"), form.clone(), rw)
        .property(js_string!("host"), host_obj, rw)
        .property(js_string!("layout"), layout, rw)
        .property(js_string!("event"), ev.clone(), rw)
        .property(js_string!("datasets"), datasets, rw)
        .property(js_string!("record"), form.clone(), rw)
        .property(js_string!("template"), template, rw)
        .property(js_string!("resolveNode"), resolve_one, rw)
        .property(js_string!("resolveNodes"), resolve_many, rw)
        .function(NativeFunction::from_fn_ptr(recalculate), js_string!("recalculate"), 1)
        .function(NativeFunction::from_fn_ptr(relayout), js_string!("remerge"), 0)
        .build();
    ctx.register_global_property(js_string!("xfa"), xfa, rw)?;
    ctx.register_global_property(js_string!("event"), ev, rw)?;
    ctx.register_global_property(js_string!("__xfa_this"), this_node, rw)?;
    let app = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(message_box), js_string!("alert"), 4)
        .function(NativeFunction::from_fn_ptr(beep), js_string!("beep"), 1)
        .function(NativeFunction::from_fn_ptr(response), js_string!("response"), 1)
        .function(NativeFunction::from_fn_ptr(goto_url), js_string!("launchURL"), 2)
        .function(NativeFunction::from_fn_ptr(exec_menu_item), js_string!("execMenuItem"), 1)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("setTimeOut"), 2)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("setInterval"), 2)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("clearTimeOut"), 1)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("clearInterval"), 1)
        .property(js_string!("viewerType"), s("Exchange-Pro"), Attribute::READONLY)
        .property(js_string!("viewerVariation"), s("Full"), Attribute::READONLY)
        .property(js_string!("viewerVersion"), JsValue::from(24.0), Attribute::READONLY)
        .property(js_string!("platform"), s(platform()), Attribute::READONLY)
        .property(js_string!("language"), s("ENU"), Attribute::READONLY)
        .build();
    ctx.register_global_property(js_string!("app"), app, rw)?;
    let console = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(console_println), js_string!("println"), 1)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("show"), 0)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("hide"), 0)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("clear"), 0)
        .build();
    ctx.register_global_property(js_string!("console"), console, rw)?;
    let util = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(printf), js_string!("printf"), 1)
        .function(NativeFunction::from_fn_ptr(printd), js_string!("printd"), 2)
        .function(NativeFunction::from_fn_ptr(printx), js_string!("printx"), 2)
        .build();
    ctx.register_global_property(js_string!("util"), util, rw)?;
    // Unqualified names: the current node's children, its siblings, then each ancestor's
    // children, the nearest first. Instance managers as `_name`.
    let scopes: Vec<usize> = {
        let h = host(ctx)?.borrow();
        let mut out = vec![current];
        let mut at = h.nodes.get(current).and_then(|n| n.parent);
        let mut guard = 0;
        while let Some(p) = at
            && guard < MAX_DEPTH
        {
            out.push(p);
            at = h.nodes.get(p).and_then(|n| n.parent);
            guard += 1;
        }
        out
    };
    // Names that stay what JavaScript (or this host) means by them: a field called `Date`
    // is reached as `this.parent.Date` or by resolveNode.
    const RESERVED: &[&str] = &[
        "xfa",
        "event",
        "app",
        "console",
        "util",
        "this",
        "__xfa_this",
        "__xfa_src",
        "eval",
        "arguments",
        "globalThis",
        "window",
        "undefined",
        "NaN",
        "Infinity",
        "Object",
        "Function",
        "Array",
        "Number",
        "String",
        "Boolean",
        "Symbol",
        "Date",
        "RegExp",
        "Error",
        "TypeError",
        "RangeError",
        "SyntaxError",
        "Math",
        "JSON",
        "Map",
        "Set",
        "WeakMap",
        "WeakSet",
        "Promise",
        "Proxy",
        "Reflect",
        "parseInt",
        "parseFloat",
        "isNaN",
        "isFinite",
        "encodeURIComponent",
        "decodeURIComponent",
        "encodeURI",
        "decodeURI",
        "escape",
        "unescape",
        "BigInt",
        "ArrayBuffer",
        "DataView",
        "Intl",
    ];
    let mut defined: Vec<String> = RESERVED.iter().map(|s| s.to_string()).collect();
    for scope in scopes {
        let kids: Vec<(usize, String, bool)> = {
            let h = host(ctx)?.borrow();
            h.nodes
                .get(scope)
                .map(|n| n.children.iter().filter_map(|&c| h.nodes.get(c).map(|k| (c, k.name.clone(), k.repeatable && !k.gone))).collect())
                .unwrap_or_default()
        };
        for (c, name, repeatable) in kids {
            if !is_identifier(&name) || defined.contains(&name) {
                continue;
            }
            let obj = node_object(ctx, c)?;
            ctx.register_global_property(JsString::from(name.as_str()), obj, rw)?;
            if repeatable {
                let im = instance_manager(ctx, scope, &name)?;
                ctx.register_global_property(JsString::from(format!("_{name}").as_str()), im, rw)?;
            }
            defined.push(name);
        }
    }
    Ok(())
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        && !matches!(
            name,
            "var"
                | "let"
                | "const"
                | "function"
                | "return"
                | "if"
                | "else"
                | "for"
                | "while"
                | "new"
                | "null"
                | "true"
                | "false"
                | "undefined"
                | "typeof"
        )
}

/// Run `script` for `event` on the form `root` (the root subform), from node `event.target`.
pub fn run_xfa(script: &str, event: &XfaEvent, doc: &XfaDoc, root: &XfaNode, limits: Limits) -> XfaOutcome {
    let failed = |why: String| XfaOutcome { error: Some(why), ..Default::default() };
    if let Some(why) = refuse(script) {
        return failed(why);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::scope(|scope| {
            let spawned = std::thread::Builder::new()
                .name("pdfcraft-xfa-js".into())
                .stack_size(crate::SCRIPT_STACK)
                .spawn_scoped(scope, || run_here(script, event, doc, root, limits));
            match spawned {
                Ok(thread) => thread.join().unwrap_or_else(|_| failed("the script stopped with an internal error".into())),
                Err(e) => failed(format!("the script engine could not start: {e}")),
            }
        })
    }
    #[cfg(target_arch = "wasm32")]
    run_here(script, event, doc, root, limits)
}

/// [`run_xfa`] with a time limit: a script still running after `timeout` (loops inside
/// nested function calls can run for hours within the engine's per-frame loop limit) is left
/// on its thread, which ends when the engine's own limits stop it, and an outcome with
/// `abandoned` set comes back at once. In the browser build there are no threads: the script
/// runs to its limits.
pub fn run_xfa_within(script: &str, event: &XfaEvent, doc: &XfaDoc, root: XfaNode, limits: Limits, timeout: std::time::Duration) -> XfaOutcome {
    let failed = |why: String| XfaOutcome { error: Some(why), ..Default::default() };
    if let Some(why) = refuse(script) {
        return failed(why);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let (script, event, doc) = (script.to_string(), event.clone(), doc.clone());
        let spawned = std::thread::Builder::new().name("pdfcraft-xfa-js".into()).stack_size(crate::SCRIPT_STACK).spawn(move || {
            // The receiver is gone when the caller stopped waiting: nothing to report then.
            let _ = tx.send(run_here(&script, &event, &doc, &root, limits));
        });
        if let Err(e) = spawned {
            return failed(format!("the script engine could not start: {e}"));
        }
        match rx.recv_timeout(timeout) {
            Ok(o) => o,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => XfaOutcome {
                error: Some(format!("the script ran longer than {:.1} s and was abandoned", timeout.as_secs_f64())),
                abandoned: true,
                ..Default::default()
            },
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => failed("the script stopped with an internal error".into()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = timeout;
        run_here(script, event, doc, &root, limits)
    }
}

fn run_here(script: &str, event: &XfaEvent, doc: &XfaDoc, root: &XfaNode, limits: Limits) -> XfaOutcome {
    let mut ctx = Context::default();
    ctx.runtime_limits_mut().set_loop_iteration_limit(limits.loop_iterations);
    ctx.runtime_limits_mut().set_recursion_limit(limits.recursion);
    // `xfa.form` is a synthetic root whose child is the template's root subform.
    let form = XfaNode { name: "form".into(), kind: Some(XfaKind::Form), children: vec![root.clone()], ..Default::default() };
    let mut nodes = Vec::new();
    flatten(&form, None, &mut nodes, 0);
    let current = nodes.iter().position(|n| n.som == event.target && n.kind != XfaKind::Form).unwrap_or(1.min(nodes.len().saturating_sub(1)));
    ctx.insert_data(RefCell::new(XHost {
        nodes,
        effects: Vec::new(),
        console: Vec::new(),
        doc: doc.clone(),
        added: 0,
        current,
        set_at: HashMap::new(),
        barrier: 0,
        alerts: 0,
        dropped_effects: 0,
        dropped_alerts: 0,
        dropped_console: 0,
    }));
    let mut out = XfaOutcome::default();
    let result = (|| -> JsResult<()> {
        install(&mut ctx, event, doc, current)?;
        ctx.register_global_property(js_string!("__xfa_src"), s(script), Attribute::CONFIGURABLE)?;
        // Direct eval inside a function called on the node: `this` is the node and the
        // completion value is the script's last expression (what a calculate script yields).
        let r = ctx.eval(Source::from_bytes(b"(function () { return eval(__xfa_src); }).call(__xfa_this)"))?;
        if !r.is_undefined() {
            // Best effort: a value that can't be made text (a symbol, say) is no failure.
            out.result = Some(text(&r, &mut ctx).unwrap_or_default());
            if r.is_boolean() {
                out.result_bool = Some(r.to_boolean());
            } else if r.is_null() {
                out.result = Some(String::new());
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        out.error = Some(e.to_string());
    }
    if let Some(h) = ctx.remove_data::<Shared>() {
        let h = h.into_inner();
        out.notes = h.notes();
        out.effects = h.effects;
        out.console = h.console;
    }
    out
}
