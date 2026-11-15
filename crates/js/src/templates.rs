//! A bounded shadow of page structure: page edits and new fields are visible immediately to
//! the running script, then the engine replays the edits atomically on the PDF object graph.
use crate::host::{arg, error, function, host, s, text_with_budget};
use crate::{Context, JsArray, JsObject, JsResult, JsValue, NativeFunction, ObjectInitializer, js_string};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub name: String,
    pub page: Option<usize>,
    pub field_names: Arc<Vec<String>>,
    pub crop: [f64; 4],
    pub rotation: i64,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Spawn { name: String, page: usize, rename: bool, overlay: bool },
    Hidden { name: String, hidden: bool },
    Create { name: String, page: usize },
    Remove { name: String },
    Delete { start: usize, end: usize },
}

fn guard(h: &crate::host::Host) -> JsResult<()> {
    if h.page_changes.len() >= 100 {
        return Err(error("a script may change at most 100 template pages"));
    }
    if h.calculate_depth > 0 || matches!(h.event_name.as_str(), "Calculate" | "Format" | "Validate" | "Keystroke") {
        return Err(error("field calculations cannot change pages"));
    }
    Ok(())
}
fn template(ctx: &Context, name: &str) -> JsResult<Template> {
    host(ctx)?.borrow().doc.templates.iter().find(|t| t.name == name).cloned().ok_or_else(|| error("that page template no longer exists"))
}
/// Replace positions without invalidating held field objects, and remap calculation targets.
fn fields(h: &mut crate::host::Host, next: Vec<crate::FieldState>, new_names: &HashSet<String>) {
    h.calculations_current = false;
    let key = |f: &crate::FieldState| (f.object_id, if f.object_id.is_some() { String::new() } else { f.name.clone() });
    let changed: HashSet<_> = h.changed.iter().filter_map(|i| h.fields.get(*i)).map(&key).collect();
    let calculations: Vec<_> =
        h.calculations.iter().filter_map(|(i, s)| h.fields.get(*i).map(|f| (f.object_id, f.name.clone(), s.clone()))).collect();
    h.changed = next.iter().enumerate().filter(|(_, f)| new_names.contains(&f.name) || changed.contains(&key(f))).map(|(i, _)| i).collect();
    let by_key: HashMap<_, _> = next.iter().enumerate().map(|(i, f)| (key(f), i)).collect();
    h.calculations = calculations
        .into_iter()
        .filter_map(|(id, name, s)| by_key.get(&(id, if id.is_some() { String::new() } else { name })).map(|i| (*i, s)))
        .collect();
    h.field_keys = crate::host::field_indices(&next);
    h.objects.retain(|key, _| h.field_keys.contains_key(key));
    h.fields = Arc::new(next);
}
fn field_bytes(f: &crate::FieldState) -> usize {
    f.value
        .iter()
        .chain(&f.default)
        .map(String::len)
        .chain(f.options.iter().map(|(a, b)| a.len().saturating_add(b.len())))
        .chain(f.actions.iter().map(|(a, b)| a.len().saturating_add(b.len())))
        .chain([f.name.len(), f.submit_name.len(), f.user_name.len(), std::mem::size_of::<crate::FieldState>()])
        .fold(0usize, usize::saturating_add)
}
fn insert(h: &mut crate::host::Host, template: &Template, page: usize, rename: bool) -> JsResult<()> {
    let prefix = format!("P{page}.{}.", template.name);
    if rename && h.fields.iter().any(|f| f.name.starts_with(&prefix)) {
        return Err(error("template field prefix is already in use; choose another insertion position"));
    }
    let names: HashSet<_> = template.field_names.iter().map(String::as_str).collect();
    let source: Vec<_> = h.fields.iter().enumerate().filter(|(_, f)| names.contains(f.name.as_str())).collect();
    let bytes = h.fields.iter().map(field_bytes).fold(0usize, usize::saturating_add);
    let extra = if rename { source.iter().map(|(_, f)| field_bytes(f).saturating_add(prefix.len())).fold(0usize, usize::saturating_add) } else { 0 };
    if bytes.saturating_add(extra) > 64 << 20 {
        return Err(error("template expansion exceeds its field data budget"));
    }
    if h.fields.len().saturating_add(if rename { source.len() } else { 0 }) > 100_000 || h.doc.num_pages >= 100_000 {
        return Err(error("template expansion exceeds its page or field limit"));
    }
    let mut next = h.fields.as_ref().clone();
    for f in &mut next {
        for p in &mut f.pages {
            if *p >= 0 && *p as usize >= page {
                *p = p.saturating_add(1);
            }
        }
    }
    let mut added = HashSet::new();
    let mut calculations = Vec::new();
    for (old, f) in source {
        if rename {
            let mut f = f.clone();
            let index = next.len();
            f.name = format!("{prefix}{}", f.name);
            f.object_id = None;
            f.instance_id = 0;
            f.pages = vec![i32::try_from(page).map_err(|_| error("page index is too large"))?];
            f.page_crop = template.crop;
            f.page_rotation = template.rotation;
            added.insert(f.name.clone());
            for (_, script) in h.calculations.iter().filter(|(i, _)| *i == old) {
                calculations.push((index, script.clone()));
            }
            next.push(f);
        } else if let Some(field) = next.get_mut(old) {
            field.pages.retain(|p| *p >= 0);
            field.pages.push(i32::try_from(page).map_err(|_| error("page index is too large"))?);
            field.pages.sort_unstable();
            field.pages.dedup();
        }
    }
    fields(h, next, &added);
    h.calculations.extend(calculations);
    for t in &mut h.doc.templates {
        if let Some(p) = &mut t.page
            && *p >= page
        {
            *p = p.saturating_add(1);
        }
    }
    h.doc.num_pages = h.doc.num_pages.saturating_add(1);
    h.doc.page_boxes.insert(page.min(h.doc.page_boxes.len()), [template.crop; 5]);
    h.doc.page_rotations.insert(page.min(h.doc.page_rotations.len()), template.rotation);
    Ok(())
}
fn delete(h: &mut crate::host::Host, start: usize, end: usize) -> JsResult<()> {
    if start > end || end >= h.doc.num_pages {
        return Err(error("deletePages range is outside this document"));
    }
    let count = end.saturating_sub(start).saturating_add(1);
    if count >= h.doc.num_pages {
        return Err(error("a document must keep at least one page"));
    }
    let originals: HashSet<_> = h.doc.templates.iter().flat_map(|t| t.field_names.iter().cloned()).collect();
    let mut next = Vec::new();
    for mut f in h.fields.iter().cloned() {
        let hidden = f.pages.iter().all(|p| *p < 0);
        f.pages.retain(|p| *p >= 0 && ((*p as usize) < start || (*p as usize) > end));
        for p in &mut f.pages {
            if *p >= 0 && *p as usize > end {
                *p = p.saturating_sub(i32::try_from(count).unwrap_or(i32::MAX));
            }
        }
        if f.pages.is_empty() {
            if hidden || originals.contains(&f.name) {
                f.pages.push(-1);
            } else {
                continue;
            }
        }
        next.push(f);
    }
    fields(h, next, &HashSet::new());
    for t in &mut h.doc.templates {
        if let Some(p) = t.page {
            t.page = if (start..=end).contains(&p) { None } else { Some(if p > end { p - count } else { p }) };
        }
    }
    h.doc.num_pages -= count;
    h.doc.page = h.doc.page.min(h.doc.num_pages.saturating_sub(1));
    for n in (start..=end).rev() {
        if n < h.doc.page_boxes.len() {
            h.doc.page_boxes.remove(n);
        }
        if n < h.doc.page_rotations.len() {
            h.doc.page_rotations.remove(n);
        }
    }
    Ok(())
}
fn page(value: &JsValue, ctx: &mut Context, max: usize, default: usize) -> JsResult<usize> {
    if value.is_undefined() {
        return Ok(default.min(max));
    }
    let n = value.to_number(ctx)?;
    if !n.is_finite() {
        return Err(error("page index must be finite"));
    }
    Ok(n.max(0.0).min(max as f64) as usize)
}
fn spawn(_: &JsValue, args: &[JsValue], name: &crate::JsString, ctx: &mut Context) -> JsResult<JsValue> {
    let t = template(ctx, &name.to_std_string_escaped())?;
    let first = arg(args, 0);
    let (p, r, o) = if let Some(obj) = first.as_object() {
        (obj.get(js_string!("nPage"), ctx)?, obj.get(js_string!("bRename"), ctx)?, obj.get(js_string!("bOverlay"), ctx)?)
    } else {
        (first, arg(args, 1), arg(args, 2))
    };
    let max = host(ctx)?.borrow().doc.num_pages;
    let p = page(&p, ctx, max, 0)?;
    let rename = r.is_undefined() || r.to_boolean();
    let overlay = o.is_undefined() || o.to_boolean();
    if overlay {
        return Err(error("overlay template spawning is not supported; use bOverlay=false"));
    }
    let mut h = host(ctx)?.borrow_mut();
    guard(&h)?;
    insert(&mut h, &t, p, rename)?;
    h.page_changes.push(Change::Spawn { name: t.name, page: p, rename, overlay });
    Ok(JsValue::undefined())
}
fn hidden_get(_: &JsValue, _: &[JsValue], name: &crate::JsString, ctx: &mut Context) -> JsResult<JsValue> {
    Ok(template(ctx, &name.to_std_string_escaped())?.page.is_none().into())
}
fn hidden_set(_: &JsValue, args: &[JsValue], name: &crate::JsString, ctx: &mut Context) -> JsResult<JsValue> {
    let t = template(ctx, &name.to_std_string_escaped())?;
    let hidden = arg(args, 0).to_boolean();
    let mut h = host(ctx)?.borrow_mut();
    guard(&h)?;
    if hidden == t.page.is_none() {
        return Ok(JsValue::undefined());
    }
    if hidden {
        if let Some(page) = t.page {
            delete(&mut h, page, page)?;
        }
    } else {
        let page = h.doc.num_pages;
        insert(&mut h, &t, page, false)?;
        if let Some(t) = h.doc.templates.iter_mut().find(|s| s.name == t.name) {
            t.page = Some(page);
        }
    }
    h.page_changes.push(Change::Hidden { name: t.name, hidden });
    Ok(JsValue::undefined())
}
fn object(ctx: &mut Context, name: &str) -> JsObject {
    let key = crate::JsString::from(name);
    let spawn = function(ctx, NativeFunction::from_copy_closure_with_captures(spawn, key.clone()));
    let get = function(ctx, NativeFunction::from_copy_closure_with_captures(hidden_get, key.clone()));
    let set = function(ctx, NativeFunction::from_copy_closure_with_captures(hidden_set, key));
    ObjectInitializer::new(ctx)
        .property(js_string!("name"), s(name), boa_engine::property::Attribute::READONLY)
        .property(js_string!("spawn"), spawn, boa_engine::property::Attribute::all())
        .accessor(js_string!("hidden"), Some(get), Some(set), boa_engine::property::Attribute::all())
        .build()
}
fn get(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let first = arg(args, 0);
    let first = if let Some(o) = first.as_object() { o.get(js_string!("cName"), ctx)? } else { first };
    let name = text_with_budget(&first, ctx, 4096, "template name is too long")?;
    Ok(if host(ctx)?.borrow().doc.templates.iter().any(|t| t.name == name) { object(ctx, &name).into() } else { JsValue::null() })
}
fn all(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let names: Vec<_> = host(ctx)?.borrow().doc.templates.iter().map(|t| t.name.clone()).collect();
    let values: Vec<JsValue> = names.iter().map(|n| object(ctx, n).into()).collect();
    Ok(JsArray::from_iter(values, ctx).into())
}
fn create(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let first = arg(args, 0);
    let (name, p) =
        if let Some(o) = first.as_object() { (o.get(js_string!("cName"), ctx)?, o.get(js_string!("nPage"), ctx)?) } else { (first, arg(args, 1)) };
    let name = text_with_budget(&name, ctx, 4096, "template name is too long")?;
    if name.is_empty() {
        return Err(error("template name must not be empty"));
    }
    let max = host(ctx)?.borrow().doc.num_pages.saturating_sub(1);
    let p = page(&p, ctx, max, 0)?;
    {
        let mut h = host(ctx)?.borrow_mut();
        guard(&h)?;
        if h.doc.templates.len() >= 4096 {
            return Err(error("too many templates"));
        }
        let field_names = Arc::new(h.fields.iter().filter(|f| f.pages.contains(&(i32::try_from(p).unwrap_or(-1)))).map(|f| f.name.clone()).collect());
        let crop = h.doc.page_boxes.get(p).map_or([0.0; 4], |b| b[1]);
        let rotation = h.doc.page_rotations.get(p).copied().unwrap_or(0);
        h.doc.templates.retain(|t| t.name != name);
        h.doc.templates.push(Template { name: name.clone(), page: Some(p), field_names, crop, rotation });
        h.page_changes.push(Change::Create { name: name.clone(), page: p });
    }
    Ok(object(ctx, &name).into())
}
fn remove(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let name = text_with_budget(&arg(args, 0), ctx, 4096, "template name is too long")?;
    template(ctx, &name)?;
    let mut h = host(ctx)?.borrow_mut();
    guard(&h)?;
    h.doc.templates.retain(|t| t.name != name);
    h.page_changes.push(Change::Remove { name });
    Ok(JsValue::undefined())
}
fn delete_pages(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let first = arg(args, 0);
    let (start, end) =
        if let Some(o) = first.as_object() { (o.get(js_string!("nStart"), ctx)?, o.get(js_string!("nEnd"), ctx)?) } else { (first, arg(args, 1)) };
    let max = host(ctx)?.borrow().doc.num_pages;
    let start = page(&start, ctx, max, 0)?;
    let end = page(&end, ctx, max, start)?;
    let mut h = host(ctx)?.borrow_mut();
    guard(&h)?;
    delete(&mut h, start, end)?;
    h.page_changes.push(Change::Delete { start, end });
    Ok(JsValue::undefined())
}
fn spawn_named(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let first = arg(args, 0);
    let (name, params) = if let Some(o) = first.as_object() {
        (o.get(js_string!("cTemplate"), ctx)?, vec![first])
    } else {
        (first, vec![arg(args, 1), arg(args, 2), arg(args, 3)])
    };
    let name = text_with_budget(&name, ctx, 4096, "template name is too long")?;
    spawn(&JsValue::undefined(), &params, &crate::JsString::from(name), ctx)
}
pub(crate) fn install(ctx: &mut Context) -> JsResult<()> {
    for (name, len, callback) in [
        ("getTemplate", 1, get as fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>),
        ("createTemplate", 2, create),
        ("removeTemplate", 1, remove),
        ("deletePages", 2, delete_pages),
        ("spawnPageFromTemplate", 4, spawn_named),
    ] {
        ctx.register_global_callable(crate::JsString::from(name), len, NativeFunction::from_fn_ptr(callback))?;
    }
    let get = function(ctx, NativeFunction::from_fn_ptr(all));
    ctx.global_object().define_property_or_throw(
        js_string!("templates"),
        boa_engine::property::PropertyDescriptor::builder().get(get).configurable(true),
        ctx,
    )?;
    Ok(())
}
