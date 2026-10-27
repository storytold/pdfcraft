use super::globals::{recalculate, relayout};
use super::host::{HNode, XHost, host};
use super::instances::{instance_manager, list_object};
use super::model::{MAX_DEPTH, XfaEffect};
use super::*;

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

/// `(idx, name)` of the child of `parent` named `name`, first instance.
pub(crate) fn child_named(h: &XHost, parent: usize, name: &str) -> Option<usize> {
    let p = h.nodes.get(parent)?;
    p.children.iter().copied().find(|&c| h.nodes.get(c).is_some_and(|n| !n.gone && n.name == name))
}

/// Every live instance named `name` under `parent`, in order.
pub(crate) fn instances(h: &XHost, parent: usize, name: &str) -> Vec<usize> {
    h.nodes
        .get(parent)
        .map(|p| p.children.iter().copied().filter(|&c| h.nodes.get(c).is_some_and(|n| !n.gone && n.name == name)).collect())
        .unwrap_or_default()
}

/// Resolve a SOM expression from node `from`: `$`, `$form`, `$record`, `$host`, `xfa.form…`,
/// `parent`, `..name` (descendant search), `name[n]`, `name[*]`, `_name`.
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
    Ok(match key.as_str() {
        IDX => JsValue::from(idx as f64),
        "rawValue" | "value" => host(ctx)?.borrow().nodes.get(idx).map_or(JsValue::null(), value_js),
        "formattedValue" => s(&host(ctx)?.borrow().nodes.get(idx).and_then(|n| n.formatted.clone()).unwrap_or(value)),
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
        _ => {}
    }
    Ok(JsValue::from(true))
}

fn node_to_string(this: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let idx = node_index(this, ctx)?;
    Ok(s(&host(ctx)?.borrow().nodes.get(idx).map(|n| n.value.clone()).unwrap_or_default()))
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
