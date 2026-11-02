use super::host::{arg, colour, error, function, host, read, s, set_value, strings, text, value_of, write};
use super::model::{DISPLAY_HIDDEN, DISPLAY_VISIBLE, FieldType, Request};
use super::{Attribute, Context, JsArray, JsObject, JsResult, JsString, JsValue, NativeFunction, ObjectInitializer, js_string};

macro_rules! getter {
    ($ctx:expr, $name:expr, |$f:ident, $c:ident| $body:expr) => {{
        fn get(_: &JsValue, _: &[JsValue], name: &JsString, $c: &mut Context) -> JsResult<JsValue> {
            let $f = read($c, name)?;
            Ok($body)
        }
        function($ctx, NativeFunction::from_copy_closure_with_captures(get, $name.clone()))
    }};
}

pub(crate) fn field_object(ctx: &mut Context, name: &str) -> JsObject {
    let n = JsString::from(name);
    let value_get = getter!(ctx, n, |f, c| value_of(&f, c));
    let value_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let vals = strings(&arg(args, 0), c)?;
            write(c, name, |f| set_value(f, vals))?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };
    let as_string = getter!(ctx, n, |f, _c| s(&match f.kind {
        FieldType::CheckBox | FieldType::RadioButton => f.value.first().cloned().unwrap_or_else(|| "Off".into()),
        _ => f.value.join(","),
    }));
    let default = getter!(ctx, n, |f, _c| s(&f.default.first().cloned().unwrap_or_default()));
    let kind = getter!(ctx, n, |f, _c| s(f.kind.name()));
    let fname = getter!(ctx, n, |f, _c| s(&f.name));
    let readonly_get = getter!(ctx, n, |f, _c| JsValue::from(f.readonly));
    let readonly_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let on = arg(args, 0).to_boolean();
            write(c, name, |f| f.readonly = on)?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };
    let required_get = getter!(ctx, n, |f, _c| JsValue::from(f.required));
    let required_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let on = arg(args, 0).to_boolean();
            write(c, name, |f| f.required = on)?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };
    let display_get = getter!(ctx, n, |f, _c| JsValue::from(f.display));
    let display_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let d = arg(args, 0).to_number(c)? as i32;
            write(c, name, |f| f.display = d.clamp(0, 3))?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };
    let hidden_get = getter!(ctx, n, |f, _c| JsValue::from(f.display == DISPLAY_HIDDEN));
    let hidden_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let on = arg(args, 0).to_boolean();
            write(c, name, |f| f.display = if on { DISPLAY_HIDDEN } else { DISPLAY_VISIBLE })?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };
    let num_items = getter!(ctx, n, |f, _c| JsValue::from(f.options.len() as f64));
    let char_limit = getter!(ctx, n, |f, _c| JsValue::from(f.char_limit.unwrap_or(0) as f64));
    let export_values = getter!(ctx, n, |f, c| {
        let vals: Vec<JsValue> = f.options.iter().map(|o| s(&o.0)).collect();
        JsArray::from_iter(vals, c).into()
    });
    let indices = getter!(ctx, n, |f, c| {
        let idx: Vec<JsValue> = f.options.iter().enumerate().filter(|(_, o)| f.value.contains(&o.0)).map(|(i, _)| JsValue::from(i as f64)).collect();
        if idx.len() == 1 { idx[0].clone() } else { JsArray::from_iter(idx, c).into() }
    });
    let text_color_get = getter!(ctx, n, |f, c| colour(&f.text_color, c));
    let text_color_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let v = strings(&arg(args, 0), c)?;
            write(c, name, |f| f.text_color = Some(v))?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };
    let fill_color_get = getter!(ctx, n, |f, c| colour(&f.fill_color, c));
    let fill_color_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let v = strings(&arg(args, 0), c)?;
            write(c, name, |f| f.fill_color = Some(v))?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };

    fn get_item_at(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
        let f = read(c, name)?;
        let i = arg(args, 0).to_number(c)?;
        // bExportValue defaults to true.
        let export = args.get(1).is_none_or(JsValue::to_boolean);
        let i = if i < 0.0 { f.options.len().saturating_sub(1) } else { i as usize };
        let o = f.options.get(i).ok_or_else(|| error(format!("{} has no item {i}", f.name)))?;
        Ok(s(if export { &o.0 } else { &o.1 }))
    }
    fn is_box_checked(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
        let f = read(c, name)?;
        let i = arg(args, 0).to_number(c)? as usize;
        let on = f.options.get(i).is_some_and(|o| f.value.first() == Some(&o.0));
        Ok(JsValue::from(on))
    }
    fn check_this_box(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
        let f = read(c, name)?;
        let i = arg(args, 0).to_number(c)? as usize;
        let on = args.get(1).is_none_or(JsValue::to_boolean);
        let export = f.options.get(i).map(|o| o.0.clone()).ok_or_else(|| error(format!("{} has no widget {i}", f.name)))?;
        write(c, name, |f| {
            if on {
                f.value = vec![export];
            } else if f.value.first() == Some(&export) {
                f.value.clear();
            }
        })?;
        Ok(JsValue::undefined())
    }
    fn set_focus(_: &JsValue, _: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
        host(c)?.borrow_mut().request(Request::Focus(name.to_std_string_escaped()));
        Ok(JsValue::undefined())
    }

    let rw = Attribute::CONFIGURABLE | Attribute::ENUMERABLE;
    let get_item_at = NativeFunction::from_copy_closure_with_captures(get_item_at, n.clone());
    let is_box_checked = NativeFunction::from_copy_closure_with_captures(is_box_checked, n.clone());
    let check_this_box = NativeFunction::from_copy_closure_with_captures(check_this_box, n.clone());
    let set_focus = NativeFunction::from_copy_closure_with_captures(set_focus, n.clone());
    ObjectInitializer::new(ctx)
        .accessor(js_string!("value"), Some(value_get), Some(value_set), rw)
        .accessor(js_string!("valueAsString"), Some(as_string), None, rw)
        .accessor(js_string!("defaultValue"), Some(default), None, rw)
        .accessor(js_string!("type"), Some(kind), None, rw)
        .accessor(js_string!("name"), Some(fname), None, rw)
        .accessor(js_string!("readonly"), Some(readonly_get), Some(readonly_set), rw)
        .accessor(js_string!("required"), Some(required_get), Some(required_set), rw)
        .accessor(js_string!("display"), Some(display_get), Some(display_set), rw)
        .accessor(js_string!("hidden"), Some(hidden_get), Some(hidden_set), rw)
        .accessor(js_string!("numItems"), Some(num_items), None, rw)
        .accessor(js_string!("charLimit"), Some(char_limit), None, rw)
        .accessor(js_string!("exportValues"), Some(export_values), None, rw)
        .accessor(js_string!("currentValueIndices"), Some(indices), None, rw)
        .accessor(js_string!("textColor"), Some(text_color_get), Some(text_color_set), rw)
        .accessor(js_string!("fillColor"), Some(fill_color_get), Some(fill_color_set), rw)
        .function(get_item_at, js_string!("getItemAt"), 2)
        .function(is_box_checked, js_string!("isBoxChecked"), 1)
        .function(check_this_box, js_string!("checkThisBox"), 2)
        .function(set_focus, js_string!("setFocus"), 0)
        .build()
}

// ── the document (global object) ────────────────────────────────────────────────────────────

pub(crate) fn get_field(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let name = text(&arg(args, 0), ctx)?;
    let exists = host(ctx)?.borrow().fields.iter().any(|f| f.name == name);
    if exists {
        return Ok(field_object(ctx, &name).into());
    }
    // A parent name (e.g. "total" for "total.0"): its first kid stands in, as for groups.
    let prefix = format!("{name}.");
    let kid = host(ctx)?.borrow().fields.iter().find(|f| f.name.starts_with(&prefix)).map(|f| f.name.clone());
    Ok(kid.map_or(JsValue::null(), |k| field_object(ctx, &k).into()))
}

pub(crate) fn get_nth_field_name(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let i = arg(args, 0).to_number(ctx)?;
    let name = host(ctx)?.borrow().fields.get(i.max(0.0) as usize).map(|f| f.name.clone());
    Ok(name.map_or(JsValue::null(), |n| s(&n)))
}

pub(crate) fn reset_form(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let names = strings(&arg(args, 0), ctx)?;
    host(ctx)?.borrow_mut().request(Request::Reset(names));
    Ok(JsValue::undefined())
}

pub(crate) fn print(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    host(ctx)?.borrow_mut().request(Request::Print);
    Ok(JsValue::undefined())
}

pub(crate) fn submit_form(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let a = arg(args, 0);
    let url = match a.as_object() {
        Some(o) if !o.is_array() => {
            let u = o.get(js_string!("cURL"), ctx)?;
            text(&u, ctx)?
        }
        _ => text(&a, ctx)?,
    };
    host(ctx)?.borrow_mut().request(Request::Submit(url));
    Ok(JsValue::undefined())
}

pub(crate) fn noop(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

pub(crate) fn num_fields(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().fields.len() as f64))
}

pub(crate) fn page_num_get(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().doc.page as f64))
}

pub(crate) fn page_num_set(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let n = arg(args, 0).to_number(ctx)?;
    let mut h = host(ctx)?.borrow_mut();
    let last = h.doc.num_pages.saturating_sub(1);
    let p = (n.max(0.0) as usize).min(last);
    h.doc.page = p;
    h.request(Request::GoToPage(p));
    Ok(JsValue::undefined())
}
