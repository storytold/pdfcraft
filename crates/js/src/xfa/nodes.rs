use super::globals::{recalculate, relayout};
use super::host::{HNode, XHost, host};
use super::instances::{instance_manager, list_object};
use super::model::{MAX_DEPTH, XfaEffect, XfaKind};
use super::props;
use super::{
    Attribute, Context, JsObject, JsProxy, JsResult, JsString, JsValue, NativeFunction, ObjectInitializer, arg, as_number, clip, error, function,
    js_string, s, text,
};

// ── node objects ────────────────────────────────────────────────────────────────────────────

const IDX: &str = "__xfaNode";

pub(super) fn node_index(target: &JsValue, ctx: &mut Context) -> JsResult<usize> {
    let o = target.as_object().ok_or_else(|| error("not an XFA node"))?;
    let v = o.get(JsString::from(IDX), ctx)?;
    let n = v.to_number(ctx)?;
    if !n.is_finite() || n < 0.0 {
        return Err(error("not an XFA node"));
    }
    Ok(n as usize)
}

/// A Proxy standing for node `idx`.
pub(super) fn node_object(ctx: &mut Context, idx: usize) -> JsResult<JsValue> {
    let target = ObjectInitializer::new(ctx).property(JsString::from(IDX), JsValue::from(idx as f64), Attribute::READONLY).build();
    let proxy = JsProxy::builder(target).get(node_get).set(node_set).has(node_has).build(ctx)?;
    Ok(proxy.into())
}

/// An object that takes any property read or write without complaint (`this.border.fill…`,
/// `this.fillColor = …`): what a script sets there changes nothing here.
pub(super) fn sink(ctx: &mut Context) -> JsResult<JsValue> {
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

/// Whether node `i` adds no segment to SOM expressions: an unnamed subform or area, whose
/// content is its parent's there ("Transparent Nodes", XFA 3.3 pp. 94-95).
pub(crate) fn transparent(h: &XHost, i: usize) -> bool {
    h.nodes.get(i).is_some_and(|n| matches!(n.kind, XfaKind::Subform | XfaKind::Area)) && !h.owns_som(i)
}

/// The container whose SOM scope node `i` is in when `i` is a container itself: `i`, or its
/// nearest ancestor that is not an unnamed subform or area.
pub(crate) fn scope_of(h: &XHost, i: usize) -> usize {
    let mut at = i;
    for _ in 0..MAX_DEPTH {
        match h.nodes.get(at).and_then(|n| n.parent) {
            Some(p) if transparent(h, at) => at = p,
            _ => break,
        }
    }
    at
}

/// Visit the live objects named `name` in the SOM scope of `parent`, in document order, until
/// `visit` says to stop (`true`, which is then returned): the children of `parent`, and those
/// of its unnamed subforms and areas, which a SOM expression sees through (XFA 3.3 pp. 94-95,
/// Example 3.12: `Receipt.Description[1]` is in the second unnamed subform of `Receipt`).
fn scope_walk(h: &XHost, parent: usize, name: &str, depth: usize, visit: &mut dyn FnMut(usize) -> bool) -> bool {
    let Some(p) = h.nodes.get(parent).filter(|_| depth <= MAX_DEPTH) else { return false };
    for &c in p.children.iter() {
        let Some(n) = h.nodes.get(c).filter(|n| !n.gone) else { continue };
        if &*n.name == name {
            if visit(c) {
                return true;
            }
        } else if transparent(h, c) && scope_walk(h, c, name, depth + 1, visit) {
            return true;
        }
    }
    false
}

/// The first live object named `name` in the SOM scope of `parent` (see [`scoped`]).
pub(crate) fn child_named(h: &XHost, parent: usize, name: &str) -> Option<usize> {
    let mut first = None;
    scope_walk(h, parent, name, 0, &mut |c| {
        first = Some(c);
        true
    });
    first
}

/// Every live object named `name` in the SOM scope of `parent`, in document order: what
/// `parent.name[*]` names, and `parent.name[n]` the one at place `n`.
pub(crate) fn scoped(h: &XHost, parent: usize, name: &str) -> Vec<usize> {
    let mut out = Vec::new();
    scope_walk(h, parent, name, 0, &mut |c| {
        out.push(c);
        false
    });
    out
}

/// The children of `parent` as SOM expressions see them: its own, with the content of each
/// unnamed subform or area in its place (gone ones included, as in its child list).
pub(crate) fn scope_members(h: &XHost, parent: usize) -> Vec<usize> {
    fn walk(h: &XHost, parent: usize, depth: usize, out: &mut Vec<usize>) {
        let Some(p) = h.nodes.get(parent).filter(|_| depth <= MAX_DEPTH) else { return };
        for &c in p.children.iter() {
            if transparent(h, c) {
                walk(h, c, depth + 1, out);
            } else {
                out.push(c);
            }
        }
    }
    let mut out = Vec::new();
    walk(h, parent, 0, &mut out);
    out
}

/// Every live instance named `name` among the children of `parent` itself, in order: what an
/// instance manager counts, adds to and removes from.
pub(crate) fn instances(h: &XHost, parent: usize, name: &str) -> Vec<usize> {
    h.nodes
        .get(parent)
        .map(|p| p.children.iter().copied().filter(|&c| h.nodes.get(c).is_some_and(|n| !n.gone && &*n.name == name)).collect())
        .unwrap_or_default()
}

/// The place of node `i` among the instances its instance manager counts (`instanceIndex`). The
/// number in its SOM path (`index`) is that only when no same-named object comes before the
/// first instance in its SOM scope.
pub(crate) fn instance_place(h: &XHost, i: usize) -> usize {
    let Some(n) = h.nodes.get(i) else { return 0 };
    n.parent.and_then(|p| instances(h, p, &n.name).iter().position(|&k| k == i)).unwrap_or(n.index)
}

/// Resolve a SOM expression from node `from`: `$`, `$form`, `$record`, `$host`, `xfa.form…`,
/// `parent`, `..name` (descendant search), `name[n]`, `name[*]`, `_name`. A name is looked for
/// in the SOM scope of the node before it ([`scoped`]), so through unnamed subforms and areas.
pub(crate) fn resolve_som(h: &XHost, from: usize, expr: &str) -> Vec<usize> {
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
                    let mut found: Vec<usize> = if descend { descendants_named(h, c, name, 0) } else { scoped(h, c, name) };
                    // A relative name a node has no child for is looked for among its siblings
                    // and in each enclosing container, nearest first (so a field's script can
                    // say `resolveNode("otherField")`).
                    if found.is_empty() && depth == 1 && !descend {
                        let mut at = h.nodes.get(c).and_then(|n| n.parent);
                        let mut guard = 0;
                        while let Some(p) = at
                            && guard < MAX_DEPTH
                        {
                            found = scoped(h, p, name);
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
        // Each node once: `row[*].parent.row[*]` would otherwise multiply the list by the
        // number of rows at every step.
        let mut seen = std::collections::HashSet::with_capacity(next.len());
        next.retain(|i| seen.insert(*i));
        cur = next;
        if cur.is_empty() {
            break;
        }
    }
    cur
}

/// The live descendants of node `from` named `name` (`from..name` in a SOM path), in document
/// order, to [`MAX_DEPTH`].
pub(crate) fn descendants_named(h: &XHost, from: usize, name: &str, depth: usize) -> Vec<usize> {
    let mut out = Vec::new();
    if depth > MAX_DEPTH {
        return out;
    }
    let Some(n) = h.nodes.get(from) else { return out };
    for &c in n.children.iter() {
        if let Some(k) = h.nodes.get(c)
            && !k.gone
        {
            if &*k.name == name {
                out.push(c);
            } else {
                out.extend(descendants_named(h, c, name, depth + 1));
            }
        }
    }
    out
}

/// A property read on a form node's Proxy: its value (and `formattedValue` as its picture shows
/// it), attributes, relatives, methods, instance managers and children by name.
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
    // A child named like one of the members read from the properties comes first, as for the
    // properties themselves (XFA 3.3 p. 99).
    let shadowing = if props::NODE_MEMBERS.contains(&key.as_str()) { child_named(&host(ctx)?.borrow(), idx, &key) } else { None };
    if let Some(c) = shadowing {
        return node_object(ctx, c);
    }
    Ok(match key.as_str() {
        IDX => JsValue::from(idx as f64),
        "rawValue" | "value" => host(ctx)?.borrow().nodes.get(idx).map_or(JsValue::null(), value_js),
        "formattedValue" => s(&host(ctx)?.borrow().nodes.get(idx).and_then(|n| n.formatted.as_deref().map(str::to_string)).unwrap_or(value)),
        "name" => s(&name),
        "somExpression" => s(&som),
        "className" => s(kind.class_name()),
        "index" => JsValue::from(index as f64),
        "instanceIndex" => JsValue::from(instance_place(&host(ctx)?.borrow(), idx) as f64),
        "presence" => s(&presence),
        "access" => s(&access),
        "isNull" => JsValue::from(value.is_empty()),
        "mandatory" => s(&props::node_mandatory(&host(ctx)?.borrow(), idx)),
        "fillColor" | "fontColor" | "borderColor" => {
            let path = props::shorthand_color(&key).unwrap_or_default();
            s(&props::node_color(&host(ctx)?.borrow(), idx, path))
        }
        "validationMessage" | "formatMessage" | "mandatoryMessage" => {
            let test = props::message_test(&key).unwrap_or_default();
            s(&props::node_message(&host(ctx)?.borrow(), idx, test))
        }
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
                Some(p) => {
                    let h = host(ctx)?.borrow();
                    scoped(&h, scope_of(&h, p), &name)
                }
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
        "getAttribute" => node_method(ctx, idx, props::get_attribute),
        "getElement" => node_method(ctx, idx, props::get_element),
        "isPropertySpecified" => node_method(ctx, idx, props::is_property_specified),
        "execValidate" => node_method(ctx, idx, props::exec_validate),
        "setAttribute" | "execEvent" | "execInitialize" | "execCalculate" | "setElement" | "loadXML" | "saveXML" | "clone" => {
            function(ctx, NativeFunction::from_fn_ptr(noop)).into()
        }
        "toString" | "valueOf" | "toJSON" => function(ctx, NativeFunction::from_fn_ptr(node_to_string)).into(),
        "length" => JsValue::undefined(),
        other => {
            if let Some(child) = other.strip_prefix('_') {
                // The manager is that of the container the subform is a child of, which is an
                // unnamed subform of this one when the name was found through it.
                let owner = {
                    let h = host(ctx)?.borrow();
                    child_named(&h, idx, child).map(|c| h.nodes.get(c).and_then(|n| n.parent).unwrap_or(idx))
                };
                return match owner {
                    Some(p) => instance_manager(ctx, p, child),
                    None => Ok(JsValue::undefined()),
                };
            }
            // `#border`: the property, whatever the children are called.
            if let Some(prop) = other.strip_prefix('#').filter(|p| props::NODE_PROPS.contains(p)) {
                return props::prop_object(ctx, idx, &props::node_prop_path(prop));
            }
            let found = child_named(&host(ctx)?.borrow(), idx, other);
            match found {
                Some(c) => node_object(ctx, c)?,
                // A child of the name comes first, for every name below: "XFA SOM resolves the
                // name clash in favor of the explicit naming" (XFA 3.3 p. 99).
                // Property objects (p. 78): read with the spec's defaults, written for this run
                // only.
                None if props::NODE_PROPS.contains(&other) => props::prop_object(ctx, idx, &props::node_prop_path(other))?,
                // Members not modelled yet take any read or write without complaint.
                None if SINK_MEMBERS.contains(&other) => sink(ctx)?,
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
    let som = n.som.to_string();
    match key.as_str() {
        "rawValue" | "value" | "formattedValue" => {
            // Not a field: the property doesn't exist, as in Acrobat, and nothing is recorded.
            h.set_value(idx, value);
        }
        "presence" => {
            let p = match value.as_str() {
                "hidden" | "invisible" | "inactive" | "visible" => value.clone(),
                _ => return Ok(JsValue::from(true)),
            };
            h.set_state(idx, XfaEffect::SetPresence { som, presence: p });
        }
        "access" => {
            let a = match value.as_str() {
                "open" | "readOnly" | "protected" | "nonInteractive" => value.clone(),
                _ => return Ok(JsValue::from(true)),
            };
            h.set_state(idx, XfaEffect::SetAccess { som, access: a });
        }
        // A child of the same name is what a read of the member gives (see `node_get`), so a
        // write names that child too, and doesn't reach the property; writes to a child by name
        // are not modelled, as for every other child name.
        k if props::NODE_MEMBERS.contains(&k) && child_named(&h, idx, k).is_some() => {}
        "mandatory" | "validationMessage" | "formatMessage" | "mandatoryMessage" => h.set_validation(idx, &key, clip(value)),
        "fillColor" | "fontColor" | "borderColor" => h.set_color(idx, &key, clip(value)),
        _ => {}
    }
    Ok(JsValue::from(true))
}

fn node_to_string(this: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let idx = node_index(this, ctx)?;
    Ok(s(&host(ctx)?.borrow().nodes.get(idx).map(|n| n.value.clone()).unwrap_or_default()))
}

/// Node members not modelled yet: a read gives an object that takes any read or write
/// ([`sink`]) unless a child has the name.
const SINK_MEMBERS: &[&str] = &[
    "h",
    "w",
    "x",
    "y",
    "minH",
    "maxH",
    "minW",
    "maxW",
    "layout",
    "model",
    "oneOfChild",
    "locale",
    "relevant",
    "colSpan",
    "dataNode",
    "defaultValue",
    "editValue",
    "selectedIndex",
    "event",
    "traversal",
];

/// The node method `f` bound to node `idx`.
fn node_method(ctx: &mut Context, idx: usize, f: fn(&JsValue, &[JsValue], &JsObject, &mut Context) -> JsResult<JsValue>) -> JsValue {
    let r = props::node_ref(ctx, idx);
    function(ctx, NativeFunction::from_copy_closure_with_captures(f, r)).into()
}

pub(super) fn noop(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

/// `resolveNode(som)` / `resolveNodes(som)` bound to node `from`.
pub(super) fn resolver(ctx: &mut Context, from: usize, many: bool) -> JsResult<JsValue> {
    let key = ObjectInitializer::new(ctx)
        .property(js_string!("from"), JsValue::from(from as f64), Attribute::READONLY)
        .property(js_string!("many"), JsValue::from(many), Attribute::READONLY)
        .build();
    fn resolve(_: &JsValue, args: &[JsValue], key: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
        let from = key.get(js_string!("from"), ctx)?.to_number(ctx)? as usize;
        let many = key.get(js_string!("many"), ctx)?.to_boolean();
        let expr = text(&arg(args, 0), ctx)?;
        let found = resolve_som(&host(ctx)?.borrow(), from, &expr);
        // An expression that runs into an object's properties (`ui.#textEdit`, `border.edge[1]`),
        // for some of the objects it names at least: a child named as the property comes first
        // for each object on its own, in place ((child, "")).
        let props = props::resolve_into_props(&host(ctx)?.borrow(), from, &expr);
        if let Some((node, path)) = props.first().cloned() {
            {
                // Edges and corners reached past those a border lists come into being.
                let mut h = host(ctx)?.borrow_mut();
                let reached = if many { props.as_slice() } else { props.get(..1).unwrap_or_default() };
                reached.iter().filter(|(_, p)| !p.is_empty()).for_each(|(n, p)| h.instantiate(*n, p));
            }
            return match (many, path.is_empty()) {
                (true, _) => props::list_object(ctx, props),
                (false, true) => node_object(ctx, node),
                (false, false) => props::prop_object(ctx, node, &path),
            };
        }
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

#[cfg(test)]
#[path = "scope_tests.rs"]
mod scope_tests;
