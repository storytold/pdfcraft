use super::document::{field_object, get_field, get_nth_field_name, noop, num_fields, page_num_get, page_num_set, print, reset_form, submit_form};
use super::format::{printd, printf, printx, scand};
use super::host::{arg, function, host, s, text, text_line};
use super::model::{DISPLAY_HIDDEN, DISPLAY_NO_PRINT, DISPLAY_NO_VIEW, DISPLAY_VISIBLE, Event, Panel, Request};
use super::{Attribute, Context, JsArray, JsObject, JsResult, JsString, JsValue, NativeFunction, ObjectInitializer, js_string};

// ── app, util, console ──────────────────────────────────────────────────────────────────────

/// A message argument: a string, or an object with `cMsg`, cut to a message's length.
fn message(v: &JsValue, key: &str, ctx: &mut Context) -> JsResult<String> {
    match v.as_object() {
        Some(o) if !o.is_array() && !o.is_callable() => {
            let m = o.get(JsString::from(key), ctx)?;
            text_line(&m, ctx)
        }
        _ => text_line(v, ctx),
    }
}

/// `app.alert(cMsg)`: records the message, given as a string or as an object with `cMsg`, and
/// returns 1 (the OK button).
fn alert(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = message(&arg(args, 0), "cMsg", ctx)?;
    host(ctx)?.borrow_mut().alert(m);
    // The OK button.
    Ok(JsValue::from(1))
}

/// `app.beep()`: queues a request to beep.
fn beep(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    host(ctx)?.borrow_mut().request(Request::Beep);
    Ok(JsValue::undefined())
}

/// `app.launchURL(cURL)`: queues a request to open the URL, given as a string or as an object
/// with `cURL`.
fn launch_url(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let u = message(&arg(args, 0), "cURL", ctx)?;
    host(ctx)?.borrow_mut().request(Request::LaunchUrl(u));
    Ok(JsValue::undefined())
}

/// What `app.execMenuItem(name)` asks for, for the menu items forms use: the side panels, Print
/// and Save As. Other items do nothing.
pub(crate) fn menu_item(name: &str) -> Option<Request> {
    Some(match name {
        "ShowHideFileAttachment" => Request::ShowPanel(Panel::Attachments),
        "ShowHideBookmarks" => Request::ShowPanel(Panel::Bookmarks),
        "ShowHideThumbnails" => Request::ShowPanel(Panel::Pages),
        "ShowHideSignatures" => Request::ShowPanel(Panel::Signatures),
        "ShowHideOptCont" => Request::ShowPanel(Panel::Layers),
        "Print" => Request::Print,
        // Save opens Save As too, as it always has for XFA forms' buttons: a click on a form
        // never overwrites the file unasked.
        "SaveAs" | "Save" => Request::SaveAs,
        _ => return None,
    })
}

/// `app.execMenuItem(name)`: queues the request [`menu_item`] gives for the name (Save opens
/// Save As too); an unknown name does nothing.
fn exec_menu_item(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let item = text(&arg(args, 0), ctx)?;
    if let Some(r) = menu_item(&item) {
        host(ctx)?.borrow_mut().request(r);
    }
    Ok(JsValue::undefined())
}

/// `app.response(…)`: always `null`, the answer for a canceled dialog, since no one is there to
/// type one.
fn response(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    // No one to answer: as if the dialog was cancelled.
    Ok(JsValue::null())
}

fn println(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = text_line(&arg(args, 0), ctx)?;
    host(ctx)?.borrow_mut().print(m);
    Ok(JsValue::undefined())
}

fn console_clear(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    host(ctx)?.borrow_mut().console.clear();
    Ok(JsValue::undefined())
}

fn timer(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    // Timers never fire in the sandbox; scripts get an object they can pass to clear*.
    Ok(ObjectInitializer::new(ctx).build().into())
}

/// The `color` object: each named color (`transparent`, `black`, `red`, `cyan`, `gray`, …) as
/// an array of its color space (`T`, `G`, `RGB` or `CMYK`) followed by its components.
fn color_object(ctx: &mut Context) -> JsObject {
    let mut entries: Vec<(&str, Vec<JsValue>)> = vec![
        ("transparent", vec![s("T")]),
        ("black", vec![s("G"), 0.into()]),
        ("white", vec![s("G"), 1.into()]),
        ("red", vec![s("RGB"), 1.into(), 0.into(), 0.into()]),
        ("green", vec![s("RGB"), 0.into(), 1.into(), 0.into()]),
        ("blue", vec![s("RGB"), 0.into(), 0.into(), 1.into()]),
        ("cyan", vec![s("CMYK"), 1.into(), 0.into(), 0.into(), 0.into()]),
        ("magenta", vec![s("CMYK"), 0.into(), 1.into(), 0.into(), 0.into()]),
        ("yellow", vec![s("CMYK"), 0.into(), 0.into(), 1.into(), 0.into()]),
        ("dkGray", vec![s("G"), JsValue::from(0.25)]),
        ("gray", vec![s("G"), JsValue::from(0.5)]),
        ("ltGray", vec![s("G"), JsValue::from(0.75)]),
    ];
    let arrays: Vec<(&str, JsValue)> = entries.drain(..).map(|(k, v)| (k, JsArray::from_iter(v, ctx).into())).collect();
    let mut o = ObjectInitializer::new(ctx);
    for (k, v) in arrays {
        o.property(JsString::from(k), v, Attribute::all());
    }
    o.build()
}

/// The XFA version scripts see as `xfa_version`.
const XFA_VERSION: f64 = 3.5;

/// Defines the form object model's globals: the document's functions (`getField`, `resetForm`,
/// `print`, `submitForm` and others) and properties (`numFields`, `pageNum`, `numPages`,
/// `documentFileName`, `info`, the XFA version), and the `app`, `util`, `console`, `display` and
/// `color` objects.
pub(crate) fn install(ctx: &mut Context) -> JsResult<()> {
    let globals: [(&str, usize, NativeFunction); 9] = [
        ("getField", 1, NativeFunction::from_fn_ptr(get_field)),
        ("getNthFieldName", 1, NativeFunction::from_fn_ptr(get_nth_field_name)),
        ("resetForm", 1, NativeFunction::from_fn_ptr(reset_form)),
        ("print", 1, NativeFunction::from_fn_ptr(print)),
        ("submitForm", 1, NativeFunction::from_fn_ptr(submit_form)),
        ("mailDoc", 1, NativeFunction::from_fn_ptr(noop)),
        ("calculateNow", 0, NativeFunction::from_fn_ptr(noop)),
        ("syncAnnotScan", 0, NativeFunction::from_fn_ptr(noop)),
        ("dirty", 0, NativeFunction::from_fn_ptr(noop)),
    ];
    for (name, len, f) in globals {
        ctx.register_global_callable(JsString::from(name), len, f)?;
    }
    let global = ctx.global_object();
    let rw = Attribute::CONFIGURABLE;
    let nf = function(ctx, NativeFunction::from_fn_ptr(num_fields));
    let pg = function(ctx, NativeFunction::from_fn_ptr(page_num_get));
    let ps = function(ctx, NativeFunction::from_fn_ptr(page_num_set));
    let (file, pages, info, xfa) = {
        let h = host(ctx)?.borrow();
        (h.doc.file_name.clone(), h.doc.num_pages, h.doc.info.clone(), h.doc.xfa)
    };
    let mut io = ObjectInitializer::new(ctx);
    for (k, v) in &info {
        io.property(JsString::from(k.as_str()), s(v), Attribute::all());
    }
    let info = io.build();
    use boa_engine::property::PropertyDescriptor;
    global.define_property_or_throw(js_string!("numFields"), PropertyDescriptor::builder().get(nf).configurable(true), ctx)?;
    global.define_property_or_throw(js_string!("pageNum"), PropertyDescriptor::builder().get(pg).set(ps).configurable(true), ctx)?;
    ctx.register_global_property(js_string!("numPages"), JsValue::from(pages as f64), rw)?;
    ctx.register_global_property(js_string!("documentFileName"), s(&file), rw)?;
    ctx.register_global_property(js_string!("info"), info, rw)?;
    // XFA forms check both before they run: every document sees the XFA version, and only an
    // XFA form sees `xfa_installed`.
    ctx.register_global_property(js_string!("xfa_version"), JsValue::from(XFA_VERSION), rw)?;
    if xfa {
        ctx.register_global_property(js_string!("xfa_installed"), JsValue::from(true), rw)?;
    }

    let app = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(alert), js_string!("alert"), 1)
        .function(NativeFunction::from_fn_ptr(beep), js_string!("beep"), 0)
        .function(NativeFunction::from_fn_ptr(response), js_string!("response"), 1)
        .function(NativeFunction::from_fn_ptr(launch_url), js_string!("launchURL"), 2)
        .function(NativeFunction::from_fn_ptr(exec_menu_item), js_string!("execMenuItem"), 1)
        .function(NativeFunction::from_fn_ptr(timer), js_string!("setTimeOut"), 2)
        .function(NativeFunction::from_fn_ptr(timer), js_string!("setInterval"), 2)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("clearTimeOut"), 1)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("clearInterval"), 1)
        .property(js_string!("viewerType"), s("Exchange-Pro"), Attribute::READONLY)
        .property(js_string!("viewerVariation"), s("Full"), Attribute::READONLY)
        .property(js_string!("viewerVersion"), JsValue::from(24.0), Attribute::READONLY)
        .property(js_string!("formsVersion"), JsValue::from(24.0), Attribute::READONLY)
        .property(js_string!("platform"), s(platform()), Attribute::READONLY)
        .property(js_string!("language"), s("ENU"), Attribute::READONLY)
        .build();
    ctx.register_global_property(js_string!("app"), app, rw)?;
    let util = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(printf), js_string!("printf"), 1)
        .function(NativeFunction::from_fn_ptr(printd), js_string!("printd"), 2)
        .function(NativeFunction::from_fn_ptr(printx), js_string!("printx"), 2)
        .function(NativeFunction::from_fn_ptr(scand), js_string!("scand"), 2)
        .build();
    ctx.register_global_property(js_string!("util"), util, rw)?;
    let console = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(println), js_string!("println"), 1)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("show"), 0)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("hide"), 0)
        .function(NativeFunction::from_fn_ptr(console_clear), js_string!("clear"), 0)
        .build();
    ctx.register_global_property(js_string!("console"), console, rw)?;
    let display = ObjectInitializer::new(ctx)
        .property(js_string!("visible"), DISPLAY_VISIBLE, Attribute::READONLY)
        .property(js_string!("hidden"), DISPLAY_HIDDEN, Attribute::READONLY)
        .property(js_string!("noPrint"), DISPLAY_NO_PRINT, Attribute::READONLY)
        .property(js_string!("noView"), DISPLAY_NO_VIEW, Attribute::READONLY)
        .build();
    ctx.register_global_property(js_string!("display"), display, rw)?;
    let color = color_object(ctx);
    ctx.register_global_property(js_string!("color"), color, rw)?;
    Ok(())
}

pub(crate) fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "MAC"
    } else if cfg!(target_os = "windows") {
        "WIN"
    } else {
        "UNIX"
    }
}

pub(crate) fn event_object(ctx: &mut Context, e: &Event) -> JsObject {
    let target = e.target.as_ref().map_or(JsValue::null(), |t| field_object(ctx, t).into());
    ObjectInitializer::new(ctx)
        .property(js_string!("name"), s(&e.name), Attribute::all())
        .property(js_string!("type"), s(&e.kind), Attribute::all())
        .property(js_string!("value"), s(&e.value), Attribute::all())
        .property(js_string!("change"), s(&e.change), Attribute::all())
        .property(js_string!("changeEx"), s(&e.change), Attribute::all())
        .property(js_string!("willCommit"), e.will_commit, Attribute::all())
        .property(js_string!("rc"), true, Attribute::all())
        .property(js_string!("target"), target, Attribute::all())
        .property(js_string!("targetName"), s(e.target.as_deref().unwrap_or("")), Attribute::all())
        .property(js_string!("selStart"), 0, Attribute::all())
        .property(js_string!("selEnd"), 0, Attribute::all())
        .property(js_string!("commitKey"), 0, Attribute::all())
        .property(js_string!("modifier"), false, Attribute::all())
        .property(js_string!("shift"), false, Attribute::all())
        .build()
}
