//! Document-local viewer controls registered by form scripts.
use crate::host::{arg, error, host, text_with_budget};
use crate::{Context, JsObject, JsResult, JsValue, NativeFunction, ObjectInitializer, js_string};
#[derive(Clone, Debug, PartialEq)]
pub struct ToolButton {
    pub name: String,
    pub label: String,
    pub tooltip: String,
    pub execute: String,
    pub enable: String,
    pub marked: String,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ButtonChange {
    Add(ToolButton),
    Remove(String),
}
pub(crate) fn add_button(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let o = arg(args, 0).as_object().ok_or_else(|| error("addToolButton expects an object"))?;
    let name = text_with_budget(&o.get(js_string!("cName"), ctx)?, ctx, 256, "tool button name is too long")?;
    if name.is_empty() {
        return Err(error("tool button needs cName"));
    }
    let mut label = text_with_budget(&o.get(js_string!("cLabel"), ctx)?, ctx, 4096, "tool button label is too long")?;
    if label.is_empty() {
        label.clone_from(&name);
    }
    let tooltip = text_with_budget(&o.get(js_string!("cTooltext"), ctx)?, ctx, 4096, "tool button tooltip is too long")?;
    let execute = text_with_budget(&o.get(js_string!("cExec"), ctx)?, ctx, crate::run::MAX_SCRIPT_BYTES, "tool button script is too long")?;
    let enable = text_with_budget(&o.get(js_string!("cEnable"), ctx)?, ctx, 4096, "tool button enable script is too long")?;
    let marked = text_with_budget(&o.get(js_string!("cMarked"), ctx)?, ctx, 4096, "tool button mark script is too long")?;
    if let Some(why) = crate::refuse(&execute) {
        return Err(error(why));
    }
    let mut h = host(ctx)?.borrow_mut();
    if h.tool_buttons.len() >= 64 {
        return Err(error("too many tool button changes"));
    }
    h.tool_buttons.push(ButtonChange::Add(ToolButton { name, label, tooltip, execute, enable, marked }));
    Ok(JsValue::undefined())
}
pub(crate) fn remove_button(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let name = text_with_budget(&arg(args, 0), ctx, 256, "tool button name is too long")?;
    let mut h = host(ctx)?.borrow_mut();
    if h.tool_buttons.len() >= 64 {
        return Err(error("too many tool button changes"));
    }
    h.tool_buttons.push(ButtonChange::Remove(name));
    Ok(JsValue::undefined())
}

fn emit(data: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let text = text_with_budget(&data.get(js_string!("text"), ctx)?, ctx, 4096, "progress text is too long")?;
    let value = data.get(js_string!("value"), ctx)?.as_number().unwrap_or(0.0);
    let duration = data.get(js_string!("duration"), ctx)?.as_number().unwrap_or(100.0);
    let active = data.get(js_string!("active"), ctx)?.to_boolean();
    crate::interaction::progress(text, value, duration, active);
    Ok(JsValue::undefined())
}
pub(crate) fn thermometer(ctx: &mut Context) -> JsResult<JsObject> {
    let data = ObjectInitializer::new(ctx)
        .property(js_string!("text"), crate::host::s(""), boa_engine::property::Attribute::all())
        .property(js_string!("value"), 0, boa_engine::property::Attribute::all())
        .property(js_string!("duration"), 100, boa_engine::property::Attribute::all())
        .property(js_string!("active"), false, boa_engine::property::Attribute::all())
        .build();
    let meter = ObjectInitializer::new(ctx).build();
    fn begin(_: &JsValue, _: &[JsValue], data: &JsObject, c: &mut Context) -> JsResult<JsValue> {
        data.set(js_string!("active"), true, false, c)?;
        emit(data, c)
    }
    fn end(_: &JsValue, _: &[JsValue], data: &JsObject, c: &mut Context) -> JsResult<JsValue> {
        data.set(js_string!("active"), false, false, c)?;
        emit(data, c)
    }
    for (name, native) in [("begin", begin as fn(&JsValue, &[JsValue], &JsObject, &mut Context) -> JsResult<JsValue>), ("end", end)] {
        let f = crate::host::function(ctx, NativeFunction::from_copy_closure_with_captures(native, data.clone()));
        meter.create_data_property_or_throw(crate::JsString::from(name), f, ctx)?;
    }
    fn get(_: &JsValue, _: &[JsValue], key: &(JsObject, crate::JsString), ctx: &mut Context) -> JsResult<JsValue> {
        key.0.get(key.1.clone(), ctx)
    }
    fn set(_: &JsValue, args: &[JsValue], key: &(JsObject, crate::JsString), ctx: &mut Context) -> JsResult<JsValue> {
        let v = if key.1 == js_string!("text") {
            crate::host::s(&text_with_budget(&arg(args, 0), ctx, 4096, "progress text is too long")?)
        } else {
            let n = arg(args, 0).to_number(ctx)?;
            if !n.is_finite() || !(0.0..=1e9).contains(&n) {
                return Err(error("progress values must be finite and between 0 and 1000000000"));
            }
            n.into()
        };
        key.0.set(key.1.clone(), v, false, ctx)?;
        emit(&key.0, ctx)
    }
    for name in ["text", "value", "duration"] {
        let capture = (data.clone(), crate::JsString::from(name));
        let g = crate::host::function(ctx, NativeFunction::from_copy_closure_with_captures(get, capture.clone()));
        let s = crate::host::function(ctx, NativeFunction::from_copy_closure_with_captures(set, capture));
        meter.define_property_or_throw(
            crate::JsString::from(name),
            boa_engine::property::PropertyDescriptor::builder().get(g).set(s).configurable(true),
            ctx,
        )?;
    }
    let cancelled = crate::host::function(ctx, NativeFunction::from_fn_ptr(|_, _, _| Ok(crate::interaction::cancelled().into())));
    meter.define_property_or_throw(
        js_string!("cancelled"),
        boa_engine::property::PropertyDescriptor::builder().get(cancelled).configurable(true),
        ctx,
    )?;
    Ok(meter)
}
