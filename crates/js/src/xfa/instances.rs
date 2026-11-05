use super::host::{HNode, XHost, host};
use super::model::{MAX_ADDED, MAX_DEPTH, MAX_NODES, XfaEffect};
use super::nodes::{instances, node_object};
use super::{Attribute, Context, JsArray, JsObject, JsResult, JsValue, NativeFunction, ObjectInitializer, arg, function, js_string, s, text};

pub(super) fn list_object(ctx: &mut Context, items: &[usize]) -> JsResult<JsValue> {
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
pub(super) fn instance_manager(ctx: &mut Context, parent: usize, name: &str) -> JsResult<JsValue> {
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
        host(ctx)?.borrow_mut().set_instances(p, &n, want as usize);
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
    let som = child_som(parent_som, &name, index);
    if let Some(n) = h.nodes.get_mut(idx) {
        n.som = som;
    }
    for k in kids {
        renumber(h, k, depth + 1);
    }
}

/// The SOM of instance `index` of the node `name` under a parent at `parent_som`.
fn child_som(parent_som: String, name: &str, index: usize) -> String {
    if name.starts_with('#') || name.is_empty() {
        parent_som
    } else if parent_som.is_empty() {
        format!("{name}[{index}]")
    } else {
        format!("{parent_som}.{name}[{index}]")
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
                // The copy holds no value, so nothing is shown in its picture either.
                formatted: None,
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
    Ok(h.add_instance(parent, name))
}

fn remove_instance_of(ctx: &mut Context, parent: usize, name: &str, i: usize) -> JsResult<()> {
    let mut h = host(ctx)?.borrow_mut();
    h.remove_instance(parent, name, i);
    Ok(())
}

impl XHost {
    /// A new instance of the subform `name` under `parent`, after the last one; `None` when the
    /// subform doesn't repeat, is at its maximum, or the run added its limit.
    pub(crate) fn add_instance(&mut self, parent: usize, name: &str) -> Option<usize> {
        let h = self;
        let live = instances(h, parent, name);
        let &first = live.first()?;
        let (repeatable, max) = h.nodes.get(first).map(|n| (n.repeatable, n.occur_max)).unwrap_or((false, Some(1)));
        if !repeatable || max.is_some_and(|m| live.len() >= m) || h.added >= MAX_ADDED || h.nodes.len() >= MAX_NODES {
            return None;
        }
        // The copy's SOM, as `renumber` will give it, so its effect is reserved before the copy.
        let name = h.nodes.get(first).map(|n| n.name.clone()).unwrap_or_default();
        let parent_som = h.nodes.get(parent).map(|p| p.som.clone()).unwrap_or_default();
        let som = child_som(parent_som, &name, live.len());
        if !h.reserve(&XfaEffect::AddInstance { som: String::new() }, som.len()) {
            return None;
        }
        let new = clone_subtree(h, first, Some(parent), 0)?;
        let last = live.last().copied().unwrap_or(first);
        if let Some(n) = h.nodes.get_mut(new) {
            n.index = live.len();
        }
        // Keep instances together: right after the last one.
        if let Some(p) = h.nodes.get_mut(parent) {
            let pos = p.children.iter().position(|&c| c == last).map_or(p.children.len(), |i| i + 1);
            p.children.insert(pos, new);
        }
        renumber(h, new, 0);
        h.added += 1;
        h.record(XfaEffect::AddInstance { som });
        Some(new)
    }

    /// Add or remove instances of the subform `name` under `parent` until there are `want`.
    /// Bounded: adding stops at the cap or the maximum, removing at the floor, and a step that
    /// changes nothing ends the loop.
    pub(crate) fn set_instances(&mut self, parent: usize, name: &str, want: usize) {
        let want = want.min(MAX_ADDED);
        for _ in 0..=MAX_ADDED {
            let have = instances(self, parent, name).len();
            if have < want {
                if self.add_instance(parent, name).is_none() {
                    break;
                }
            } else if have > want && have > 0 {
                self.remove_instance(parent, name, have - 1);
                if instances(self, parent, name).len() == have {
                    break;
                }
            } else {
                break;
            }
        }
    }

    /// Remove instance `i` of the subform `name` under `parent` (never below the minimum, or
    /// below one).
    pub(crate) fn remove_instance(&mut self, parent: usize, name: &str, i: usize) {
        let h = self;
        let live = instances(h, parent, name);
        let Some(&gone) = live.get(i) else { return };
        let min = h.nodes.get(gone).map_or(0, |n| n.occur_min).max(1);
        if live.len() <= min {
            return;
        }
        let som = h.nodes.get(gone).map(|n| n.som.clone()).unwrap_or_default();
        if !h.reserve(&XfaEffect::RemoveInstance { som: String::new() }, som.len()) {
            return;
        }
        if let Some(n) = h.nodes.get_mut(gone) {
            n.gone = true;
        }
        // Later instances move down one.
        for (k, &inst) in live.iter().enumerate().skip(i + 1) {
            if let Some(n) = h.nodes.get_mut(inst) {
                n.index = k - 1;
            }
            renumber(h, inst, 0);
        }
        h.record(XfaEffect::RemoveInstance { som });
    }
}
