//! Script-owned dialog descriptions, evaluated on the document worker. Frontends exchange
//! bounded data and control events; they never evaluate the document's code.
use crate::host::{arg, error, function, s, text_with_budget};
use crate::{Context, JsArray, JsObject, JsResult, JsValue, NativeFunction, ObjectInitializer, js_string};
use boa_engine::property::PropertyKey;
use serde_json::{Map, Value};
use std::cell::Cell;

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub alignment: String,
    pub multiline: bool,
    pub password: bool,
    pub readonly: bool,
    pub group: String,
    pub other_name: String,
    pub multiselect: bool,
    pub red: bool,
    pub enabled: bool,
    pub visible: bool,
    pub value: Value,
    pub children: Vec<Element>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Dialog {
    pub title: String,
    pub elements: Vec<Element>,
    pub focus: String,
}

thread_local! { static DEPTH: Cell<usize> = const { Cell::new(0) }; }
struct Depth;
impl Drop for Depth {
    fn drop(&mut self) {
        DEPTH.with(|n| n.set(n.get().saturating_sub(1)));
    }
}

struct Budget {
    nodes: usize,
    bytes: usize,
}
impl Budget {
    fn string(&mut self, value: &JsValue, ctx: &mut Context) -> JsResult<String> {
        if value.is_null_or_undefined() {
            return Ok(String::new());
        }
        let value = text_with_budget(value, ctx, 16_384.min(self.bytes), "dialog text exceeds its limit")?;
        self.bytes = self.bytes.saturating_sub(value.len());
        Ok(value)
    }
    fn node(&mut self, depth: usize) -> JsResult<()> {
        if depth > 16 || self.nodes == 0 {
            return Err(error("dialog exceeds its size or depth limit"));
        }
        self.nodes -= 1;
        Ok(())
    }
}
fn object(ctx: &mut Context) -> JsObject {
    ObjectInitializer::new(ctx).build()
}
fn part(data: &JsObject, name: &str, ctx: &mut Context) -> JsResult<JsObject> {
    data.get(crate::JsString::from(name), ctx)?.as_object().ok_or_else(|| error("dialog state is unavailable"))
}
fn key_string(key: &PropertyKey) -> Option<String> {
    match key {
        PropertyKey::String(s) => Some(s.to_std_string_escaped()),
        PropertyKey::Index(n) => Some(n.get().to_string()),
        _ => None,
    }
}
/// Convert only bounded plain control data, before using any recursive JSON converter.
fn json(value: &JsValue, ctx: &mut Context, depth: usize, budget: &mut Budget) -> JsResult<Value> {
    budget.node(depth)?;
    if value.is_null_or_undefined() {
        return Ok(Value::Null);
    }
    if let Some(v) = value.as_boolean() {
        return Ok(Value::Bool(v));
    }
    if let Some(v) = value.as_number() {
        return Ok(serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number));
    }
    if value.as_string().is_some() {
        let value = text_with_budget(value, ctx, (8 << 20).min(budget.bytes), "dialog value exceeds its limit")?;
        budget.bytes = budget.bytes.saturating_sub(value.len());
        return Ok(Value::String(value));
    }
    let o = value.as_object().ok_or_else(|| error("invalid dialog value"))?;
    let read = o.get(js_string!("read"), ctx)?.as_callable();
    let encoded = o.get(js_string!("argb"), ctx)?;
    if read.is_some() || encoded.is_string() {
        let dimension = |v: JsValue, ctx: &mut Context| -> JsResult<usize> {
            let n = v.to_number(ctx)?;
            if !n.is_finite() || n < 1.0 || n > 1024.0 || n.fract() != 0.0 {
                return Err(error("invalid dialog image dimensions"));
            }
            Ok(n as usize)
        };
        let width = dimension(o.get(js_string!("width"), ctx)?, ctx)?;
        let height = dimension(o.get(js_string!("height"), ctx)?, ctx)?;
        let bytes = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n <= 1 << 20 && n.saturating_mul(2) <= budget.bytes)
            .ok_or_else(|| error("dialog image exceeds its size limit"))?;
        let expected = bytes.saturating_mul(2);
        let hex = if let Some(read) = read {
            let mut hex = String::new();
            for _ in 0..64 {
                // Icon streams are read in bounded blocks. A short block may be the entire
                // image; callbacks that expose hex characters instead of bytes work too.
                let data = read.call(value, &[JsValue::from(131072)], ctx)?;
                let block = text_with_budget(&data, ctx, expected.saturating_sub(hex.len()).min(262144), "dialog image exceeds its size limit")?;
                if block.is_empty() {
                    break;
                }
                hex.push_str(&block);
                if hex.len() == expected {
                    break;
                }
            }
            hex
        } else {
            text_with_budget(&encoded, ctx, expected, "dialog image exceeds its size limit")?
        };
        if hex.len() != bytes.saturating_mul(2) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(error("dialog image must contain ARGB hexadecimal pixels"));
        }
        budget.bytes = budget.bytes.saturating_sub(hex.len());
        return Ok(serde_json::json!({"width":width,"height":height,"argb":hex}));
    }
    if o.is_array() {
        let a = JsArray::from_object(o)?;
        let count = a.length(ctx)?;
        if count > budget.nodes as u64 {
            return Err(error("dialog list has too many entries"));
        }
        let mut out = Vec::new();
        for n in 0..count {
            out.push(json(&a.get(n, ctx)?, ctx, depth + 1, budget)?);
        }
        return Ok(Value::Array(out));
    }
    if !o.is_ordinary() {
        return Err(error("dialog values must be plain data"));
    }
    let mut out = Map::new();
    for key in o.own_property_keys(ctx)? {
        let Some(name) = key_string(&key) else { continue };
        if name.len() > 4096 || name.len() > budget.bytes {
            return Err(error("dialog list label exceeds its limit"));
        }
        budget.bytes -= name.len();
        out.insert(name, json(&o.get(key, ctx)?, ctx, depth + 1, budget)?);
    }
    Ok(Value::Object(out))
}
fn patch(args: &[JsValue], data: &JsObject, bucket: &str, ctx: &mut Context) -> JsResult<JsValue> {
    let source = arg(args, 0).as_object().ok_or_else(|| error("dialog.load/enable/visible expects an object"))?;
    let target = part(data, bucket, ctx)?;
    let ids = part(data, "ids", ctx)?;
    let mut budget = Budget { nodes: 4096, bytes: 16 << 20 };
    let keys = source.own_property_keys(ctx)?;
    // Retained values share the conversion budget across calls. Exclude replacements
    // first so writing the same large value again does not consume the limit twice.
    for key in target.own_property_keys(ctx)? {
        if !keys.contains(&key) {
            if let Some(name) = key_string(&key) {
                budget.bytes = budget.bytes.checked_sub(name.len()).ok_or_else(|| error("dialog value exceeds its limit"))?;
            }
            json(&target.get(key, ctx)?, ctx, 0, &mut budget)?;
        }
    }
    let mut updates = Vec::new();
    for key in keys {
        let Some(name) = key_string(&key) else { continue };
        if !ids.get(crate::JsString::from(name.as_str()), ctx)?.to_boolean() {
            continue;
        }
        let value = source.get(key.clone(), ctx)?;
        budget.bytes = budget.bytes.checked_sub(name.len()).ok_or_else(|| error("dialog value exceeds its limit"))?;
        let value = json(&value, ctx, 0, &mut budget)?;
        updates.push((key, JsValue::from_json(&value, ctx)?));
    }
    // Validate the whole patch before retaining any of it.
    for (key, value) in updates {
        target.set(key, value, false, ctx)?;
    }
    Ok(JsValue::undefined())
}
fn load(_: &JsValue, args: &[JsValue], data: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    patch(args, data, "values", ctx)
}
fn enable(_: &JsValue, args: &[JsValue], data: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    patch(args, data, "enabled", ctx)
}
fn visible(_: &JsValue, args: &[JsValue], data: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    patch(args, data, "visible", ctx)
}
fn store(_: &JsValue, _: &[JsValue], data: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let values = part(data, "values", ctx)?;
    let value = json(&values.into(), ctx, 0, &mut Budget { nodes: 4096, bytes: 16 << 20 })?;
    JsValue::from_json(&value, ctx)
}
fn end(_: &JsValue, args: &[JsValue], data: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let value = text_with_budget(&arg(args, 0), ctx, 128, "dialog result is too long")?;
    data.set(js_string!("result"), s(&value), false, ctx)?;
    Ok(JsValue::undefined())
}
fn focus(_: &JsValue, args: &[JsValue], data: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let id = text_with_budget(&arg(args, 0), ctx, 128, "dialog item id is too long")?;
    data.set(js_string!("focus"), s(&id), false, ctx)?;
    Ok(JsValue::undefined())
}
fn remove_entries(_: &JsValue, args: &[JsValue], data: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let id = text_with_budget(&arg(args, 0), ctx, 128, "dialog item id is too long")?;
    let empty = object(ctx);
    part(data, "values", ctx)?.set(crate::JsString::from(id), empty, false, ctx)?;
    Ok(JsValue::undefined())
}
fn insert_entry(_: &JsValue, args: &[JsValue], data: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let id = text_with_budget(&arg(args, 0), ctx, 128, "dialog item id is too long")?;
    let label = text_with_budget(&arg(args, 1), ctx, 4096, "dialog list entry is too long")?;
    let values = part(data, "values", ctx)?;
    let current = values.get(crate::JsString::from(id.as_str()), ctx)?;
    let entries = current.as_object().unwrap_or_else(|| object(ctx));
    if entries.own_property_keys(ctx)?.len() >= 1024 {
        return Err(error("dialog list has too many entries"));
    }
    let rank = arg(args, 2).as_number().unwrap_or(0.0).abs().clamp(1.0, 1024.0);
    entries.set(crate::JsString::from(label), JsValue::from(-rank), false, ctx)?;
    values.set(crate::JsString::from(id), entries, false, ctx)?;
    Ok(JsValue::undefined())
}
fn red(_: &JsValue, args: &[JsValue], data: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let id = text_with_budget(&arg(args, 0), ctx, 128, "dialog item id is too long")?;
    part(data, "red", ctx)?.set(crate::JsString::from(id), true, false, ctx)?;
    Ok(JsValue::undefined())
}
fn context(ctx: &mut Context) -> JsResult<(JsObject, JsObject)> {
    let data = object(ctx);
    for key in ["values", "enabled", "visible", "ids", "red"] {
        let value = object(ctx);
        data.set(crate::JsString::from(key), value, false, ctx)?;
    }
    let dialog = object(ctx);
    for (name, native) in [
        ("load", load as fn(&JsValue, &[JsValue], &JsObject, &mut Context) -> JsResult<JsValue>),
        ("store", store),
        ("enable", enable),
        ("visible", visible),
        ("end", end),
        ("focus", focus),
        ("insertEntryInList", insert_entry),
        ("removeAllEntriesFromList", remove_entries),
        ("setForeColorRed", red),
    ] {
        let callable = function(ctx, NativeFunction::from_copy_closure_with_captures(native, data.clone()));
        dialog.define_property_or_throw(
            crate::JsString::from(name),
            boa_engine::property::PropertyDescriptor::builder().value(callable).writable(true).configurable(true),
            ctx,
        )?;
    }
    Ok((data, dialog))
}
fn read_elements(value: JsValue, data: &JsObject, ctx: &mut Context, depth: usize, budget: &mut Budget) -> JsResult<Vec<Element>> {
    if value.is_null_or_undefined() {
        return Ok(vec![]);
    }
    let array = JsArray::from_object(value.as_object().ok_or_else(|| error("dialog elements must be an array"))?)?;
    let len = array.length(ctx)?;
    if len > budget.nodes as u64 {
        return Err(error("dialog has too many elements"));
    }
    let mut elements = Vec::new();
    for i in 0..len {
        budget.node(depth)?;
        let o = array.get(i, ctx)?.as_object().ok_or_else(|| error("dialog element must be an object"))?;
        let mut string = |key: &str, ctx: &mut Context| -> JsResult<String> { budget.string(&o.get(crate::JsString::from(key), ctx)?, ctx) };
        let id = string("item_id", ctx)?;
        if id.len() > 128 {
            return Err(error("dialog item id is too long"));
        }
        let kind = string("type", ctx)?;
        let name = string("name", ctx)?;
        let alignment = string("align_children", ctx)?;
        let group = string("group_id", ctx)?;
        let other_name = string("other_name", ctx)?;
        let mut value = JsValue::undefined();
        let mut enabled = !o.get(js_string!("enabled"), ctx)?.as_boolean().is_some_and(|v| !v);
        let mut visible = !o.get(js_string!("visible"), ctx)?.as_boolean().is_some_and(|v| !v);
        if !id.is_empty() {
            part(data, "ids", ctx)?.set(crate::JsString::from(id.as_str()), true, false, ctx)?;
            value = part(data, "values", ctx)?.get(crate::JsString::from(id.as_str()), ctx)?;
            if value.is_undefined() {
                let default = match kind.as_str() {
                    "edit_text" => Some(s("")),
                    "check_box" | "radio" => Some(JsValue::from(false)),
                    "popup" | "list_box" | "hier_list_box" => Some(object(ctx).into()),
                    _ => None,
                };
                if let Some(default) = default {
                    part(data, "values", ctx)?.set(crate::JsString::from(id.as_str()), default.clone(), false, ctx)?;
                    value = default;
                }
            }
            for (key, target) in [("enabled", &mut enabled), ("visible", &mut visible)] {
                let set = part(data, key, ctx)?.get(crate::JsString::from(id.as_str()), ctx)?;
                if !set.is_undefined() {
                    *target = set.to_boolean();
                }
            }
        }
        let children = read_elements(o.get(js_string!("elements"), ctx)?, data, ctx, depth + 1, budget)?;
        let value = json(&value, ctx, 0, budget)?;
        let red = part(data, "red", ctx)?.get(crate::JsString::from(id.as_str()), ctx)?.to_boolean();
        elements.push(Element {
            id,
            kind,
            name,
            alignment,
            group,
            other_name,
            red,
            value,
            enabled,
            visible,
            children,
            multiselect: o.get(js_string!("multiple_selection"), ctx)?.to_boolean(),
            multiline: o.get(js_string!("multiline"), ctx)?.to_boolean(),
            password: o.get(js_string!("password"), ctx)?.to_boolean(),
            readonly: o.get(js_string!("readonly"), ctx)?.to_boolean(),
        });
    }
    Ok(elements)
}
fn snapshot(descriptor: &JsObject, data: &JsObject, ctx: &mut Context) -> JsResult<Dialog> {
    let description = descriptor.get(js_string!("description"), ctx)?.as_object().ok_or_else(|| error("execDialog needs a description"))?;
    let mut budget = Budget { nodes: 4096, bytes: 16 << 20 };
    let title = budget.string(&description.get(js_string!("name"), ctx)?, ctx)?;
    let elements = read_elements(description.get(js_string!("elements"), ctx)?, data, ctx, 0, &mut budget)?;
    let focus = budget.string(&data.get(js_string!("focus"), ctx)?, ctx)?;
    Ok(Dialog { title, elements, focus })
}
fn callback(descriptor: &JsObject, name: &str, dialog: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    match descriptor.get(crate::JsString::from(name), ctx)?.as_callable() {
        Some(callable) => callable.call(&descriptor.clone().into(), &[dialog.clone().into()], ctx),
        None => Ok(JsValue::undefined()),
    }
}
fn allowed(elements: &[Element], id: &str) -> bool {
    elements.iter().any(|e| e.enabled && e.visible && (e.id == id || (id == "other" && e.kind == "ok_cancel_other") || allowed(&e.children, id)))
}
fn validate_json(value: &Value, depth: usize, count: &mut usize) -> JsResult<()> {
    if depth > 16 || *count == 0 {
        return Err(error("dialog reply exceeds its size or depth limit"));
    }
    *count -= 1;
    match value {
        Value::Array(a) => {
            for v in a {
                validate_json(v, depth + 1, count)?;
            }
        }
        Value::Object(o) => {
            for v in o.values() {
                validate_json(v, depth + 1, count)?;
            }
        }
        _ => {}
    }
    Ok(())
}
/// Accepting an enabled radio also clears disabled peers. Direct disabled writes
/// are validated separately, before these runtime-generated changes are added.
fn radio_changes(elements: &[Element], values: &mut Map<String, Value>) {
    fn selected(elements: &[Element], values: &Map<String, Value>, groups: &mut std::collections::BTreeMap<String, String>) {
        for e in elements {
            if e.kind == "radio" && values.get(&e.id).and_then(Value::as_bool) == Some(true) {
                groups.insert(e.group.clone(), e.id.clone());
            }
            selected(&e.children, values, groups);
        }
    }
    fn clear(elements: &[Element], groups: &std::collections::BTreeMap<String, String>, values: &mut Map<String, Value>) {
        for e in elements {
            if e.kind == "radio"
                && let Some(id) = groups.get(&e.group)
            {
                values.insert(e.id.clone(), Value::Bool(&e.id == id));
            }
            clear(&e.children, groups, values);
        }
    }
    let mut groups = std::collections::BTreeMap::new();
    selected(elements, values, &mut groups);
    clear(elements, &groups, values);
}
/// Synchronously run callbacks while each prompt suspends only the script worker.
pub(crate) fn execute(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let depth = DEPTH.with(|n| {
        let depth = n.get();
        if depth < 4 {
            n.set(depth + 1);
        }
        depth
    });
    if depth >= 4 {
        return Err(error("dialogs are nested too deeply"));
    }
    let _depth = Depth;
    let descriptor = arg(args, 0).as_object().ok_or_else(|| error("execDialog expects a dialog object"))?;
    let (data, dialog) = context(ctx)?;
    snapshot(&descriptor, &data, ctx)?;
    let result = (|| {
        callback(&descriptor, "initialize", &dialog, ctx)?;
        for _ in 0..128 {
            let ended = data.get(js_string!("result"), ctx)?;
            if !ended.is_undefined() {
                if ended.as_string().is_some_and(|v| v.to_std_string_escaped() == "ok") {
                    if callback(&descriptor, "validate", &dialog, ctx)?.as_boolean() == Some(false) {
                        data.set(js_string!("result"), JsValue::undefined(), false, ctx)?;
                        continue;
                    }
                    callback(&descriptor, "commit", &dialog, ctx)?;
                }
                return Ok(ended);
            }
            let model = snapshot(&descriptor, &data, ctx)?;
            let Some(reply) = crate::interaction::ask(crate::interaction::Prompt::Dialog(model.clone()))? else {
                return Ok(s("cancel"));
            };
            let reply: Value = serde_json::from_str(&reply).unwrap_or(Value::String(reply));
            validate_json(&reply, 0, &mut 4096)?;
            let event = reply.as_str().or_else(|| reply.get("event").and_then(Value::as_str)).ok_or_else(|| error("dialog reply needs an event"))?;
            if event.is_empty() {
                return Err(error("dialog reply needs a nonempty event"));
            }
            if event == "cancel" {
                return Ok(s("cancel"));
            }
            if event != "ok" && !allowed(&model.elements, event) {
                return Err(error("dialog event names a missing or disabled control"));
            }
            if let Some(values) = reply.get("values") {
                let values = values.as_object().ok_or_else(|| error("dialog reply values must be an object"))?;
                for id in values.keys() {
                    if !allowed(&model.elements, id) {
                        return Err(error("dialog reply changes a missing or disabled control"));
                    }
                }
                let mut values = values.clone();
                radio_changes(&model.elements, &mut values);
                load(&JsValue::undefined(), &[JsValue::from_json(&Value::Object(values), ctx)?], &data, ctx)?;
            }
            if event == "ok" {
                if callback(&descriptor, "validate", &dialog, ctx)?.as_boolean() == Some(false) {
                    continue;
                }
                callback(&descriptor, "commit", &dialog, ctx)?;
                return Ok(s("ok"));
            }
            callback(&descriptor, event, &dialog, ctx)?;
        }
        Err(error("dialog interaction limit exceeded"))
    })();
    let destroyed = callback(&descriptor, "destroy", &dialog, ctx);
    match result {
        Ok(value) => {
            destroyed?;
            Ok(value)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        DocInfo, Event,
        session::{Job, ScriptSession},
    };
    use std::sync::Arc;
    fn job(script: &str) -> Job {
        Job {
            script: script.into(),
            event: Event::doc("Console"),
            doc: DocInfo::default(),
            fields: Arc::new(vec![]),
            doc_scripts: vec![].into(),
            calculations: vec![],
            timer: None,
        }
    }
    #[test]
    fn radio_selection_clears_disabled_peers_but_direct_writes_are_rejected() {
        let script = "var chosen;app.execDialog({initialize:function(d){d.load({old:true});d.enable({old:false});},commit:function(d){chosen=d.store();},description:{elements:[{type:'radio',item_id:'old',group_id:'g'},{type:'radio',item_id:'new',group_id:'g'}]}});chosen.old+':'+chosen.new";
        let out = ScriptSession::default().run(job(script), &[Some(r#"{"event":"ok","values":{"new":true}}"#.into())]);
        assert_eq!(out.error, None);
        assert_eq!(out.result.as_deref(), Some("false:true"));
        let out = ScriptSession::default().run(job(script), &[Some(r#"{"event":"ok","values":{"old":false}}"#.into())]);
        assert!(out.error.as_deref().is_some_and(|e| e.contains("disabled")));
    }
    #[test]
    fn repeated_loads_bound_retained_values_and_reject_whole_patches() {
        let script = "var kept;app.execDialog({initialize:function(d){var big='xxx';for(var i=0;i<21;i++)big+=big;d.load({one:big});d.load({two:big});d.load({one:big});try{d.load({small:'changed',three:big});}catch(e){kept=d.store();}d.end('cancel');},description:{elements:['one','two','three','small'].map(function(id){return {type:'edit_text',item_id:id};})}});kept.one.length+':'+kept.two.length+':'+kept.three+':'+kept.small";
        let out = ScriptSession::default().run(job(script), &[]);
        assert_eq!(out.error, None);
        assert_eq!(out.result.as_deref(), Some("6291456:6291456::"));
    }
    #[test]
    fn dialog_callbacks_load_validate_commit_and_destroy_in_order() {
        let script = "var trace=[],chosen;var d={initialize:function(d){trace.push('init');d.load({name:'Fighter',level:'0'});},validate:function(d){trace.push('validate');return +d.store().level>0;},commit:function(d){trace.push('commit');chosen=d.store();},destroy:function(){trace.push('destroy');},description:{name:'Class',elements:[{type:'edit_text',item_id:'name'},{type:'edit_text',item_id:'level'}]}};app.execDialog(d);trace.join(',')+':'+chosen.name+':'+chosen.level";
        let out = ScriptSession::default().run(job(script), &[Some("ok".into()), Some(r#"{"event":"ok","values":{"level":"2"}}"#.into())]);
        assert_eq!(out.error, None);
        assert_eq!(out.result.as_deref(), Some("init,validate,validate,commit,destroy:Fighter:2"));
    }
    #[test]
    fn icon_streams_are_read_once_and_dialog_end_commits() {
        let script = "var reads=0,commits=0;app.execDialog({initialize:function(d){d.load({icon:{width:1,height:1,read:function(n){reads++;return 'ff102030'}}});},pick:function(d){d.end('ok');},commit:function(){commits++;},description:{elements:[{type:'image',item_id:'icon'},{type:'button',item_id:'pick'}]}});reads+':'+commits";
        let out = ScriptSession::default().run(job(script), &[Some("pick".into())]);
        assert_eq!(out.error, None);
        assert_eq!(out.result.as_deref(), Some("1:1"));
        let out=ScriptSession::default().run(job("app.execDialog({initialize:function(d){d.load({icon:{width:1e20,height:1e20,read:function(){throw 'must not read'}}})},description:{elements:[{type:'image',item_id:'icon'}]}})"),&[]);
        assert!(out.error.as_deref().is_some_and(|e| e.contains("dimensions")));
    }
    #[test]
    fn callbacks_refresh_values_and_disabled_controls_reject_forged_events() {
        let script = "var chosen;var d={initialize:function(d){d.enable({level:false});},pick:function(d){d.enable({level:true});d.load({level:'3'});},commit:function(d){chosen=d.store().level;},description:{elements:[{type:'button',item_id:'pick'},{type:'edit_text',item_id:'level'}]}};app.execDialog(d);chosen";
        let out = ScriptSession::default().run(job(script), &[Some("pick".into()), Some("ok".into())]);
        assert_eq!(out.error, None);
        assert_eq!(out.result.as_deref(), Some("3"));
        let out = ScriptSession::default().run(job(script), &[Some(r#"{"event":"ok","values":{"level":"99"}}"#.into())]);
        assert!(out.error.as_deref().is_some_and(|e| e.contains("disabled")));
    }
    #[test]
    fn cancellation_destroys_without_commit_and_cyclic_descriptions_are_bounded() {
        let out = ScriptSession::default().run(
            job("var c=0,d=0;var r=app.execDialog({commit:function(){c++},destroy:function(){d++},description:{elements:[]}});r+':'+c+':'+d"),
            &[],
        );
        assert_eq!(out.result.as_deref(), Some("cancel:0:1"));
        let out = ScriptSession::default().run(job("var e={type:'view'};e.elements=[e];app.execDialog({description:{elements:[e]}})"), &[]);
        assert!(out.error.as_deref().is_some_and(|e| e.contains("depth limit")));
    }
}
