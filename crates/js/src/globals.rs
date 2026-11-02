use super::document::{field_object, get_field, get_nth_field_name, noop, num_fields, page_num_get, page_num_set, print, reset_form, submit_form};
use super::format::{printd, printf, printx, scand};
use super::host::{arg, error, function, host, read, s, strings, text, text_line};
use super::model::{DISPLAY_HIDDEN, DISPLAY_NO_PRINT, DISPLAY_NO_VIEW, DISPLAY_VISIBLE, Event, Panel, Request};
use super::{
    Arc, Attribute, Context, JsArray, JsObject, JsResult, JsString, JsValue, Message, MessageButtons, MessageIcon, NativeFunction, ObjectInitializer,
    Source, js_string,
};

// ── app, util, console ──────────────────────────────────────────────────────────────────────

/// A message argument: a string, or an object with `cMsg`, cut to a message's length.
fn message(v: &JsValue, key: &str, ctx: &mut Context) -> JsResult<String> {
    match named_params(v, key, ctx)? {
        Some(o) => {
            let m = o.get(JsString::from(key), ctx)?;
            text_line(&m, ctx)
        }
        None => text_line(v, ctx),
    }
}

/// `v` as a method's single object of named parameters: an object (not an array or function)
/// that has `key`, the parameter every such call names. Any other value, a `new String(…)` or a
/// caught `Error` included, is the first positional argument, read as text.
fn named_params(v: &JsValue, key: &str, ctx: &mut Context) -> JsResult<Option<JsObject>> {
    match v.as_object() {
        Some(o) if !o.is_array() && !o.is_callable() && o.has_property(JsString::from(key), ctx)? => Ok(Some(o.clone())),
        _ => Ok(None),
    }
}

/// A message's title: `v` as text cut to a message's length, `None` when it is missing or empty.
fn title(v: &JsValue, ctx: &mut Context) -> JsResult<Option<String>> {
    Ok(Some(text_line(v, ctx)?).filter(|t| !t.is_empty()))
}

/// An icon or buttons number: `v` as a number, `None` when it is missing or isn't one (a value
/// that can't be converted counts as missing, so a bad argument never stops the message).
fn code(v: &JsValue, ctx: &mut Context) -> Option<f64> {
    if v.is_undefined() || v.is_null() {
        return None;
    }
    v.to_number(ctx).ok()
}

/// `app.alert`'s message: `(cMsg, nIcon, nType, cTitle)`, or one object with those keys (as
/// the JavaScript for Acrobat API Reference allows for every method with named parameters).
pub(crate) fn alert_message(args: &[JsValue], ctx: &mut Context) -> JsResult<Message> {
    let first = arg(args, 0);
    let (text, icon, buttons, t) = match named_params(&first, "cMsg", ctx)? {
        Some(o) => {
            let get = |k: &str, ctx: &mut Context| o.get(JsString::from(k), ctx);
            (get("cMsg", ctx)?, get("nIcon", ctx)?, get("nType", ctx)?, get("cTitle", ctx)?)
        }
        None => (first, arg(args, 1), arg(args, 2), arg(args, 3)),
    };
    Ok(Message {
        text: text_line(&text, ctx)?,
        title: title(&t, ctx)?,
        icon: MessageIcon::from_code(code(&icon, ctx)),
        buttons: MessageButtons::from_code(code(&buttons, ctx)),
    })
}

/// `xfa.host.messageBox`'s message: `(message, title, icon, buttons)`, as the XFA
/// specification orders them.
pub(crate) fn message_box_message(args: &[JsValue], ctx: &mut Context) -> JsResult<Message> {
    Ok(Message {
        text: text_line(&arg(args, 0), ctx)?,
        title: title(&arg(args, 1), ctx)?,
        icon: MessageIcon::from_code(code(&arg(args, 2), ctx)),
        buttons: MessageButtons::from_code(code(&arg(args, 3), ctx)),
    })
}

/// `app.alert(cMsg, nIcon, nType, cTitle)`: records the message with its title, icon and buttons,
/// given in that order or as one object with those keys, and returns 1 (the OK button).
fn alert(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = alert_message(args, ctx)?;
    host(ctx)?.borrow_mut().alert(m.clone());
    if !crate::interaction::has_host() {
        return Ok(JsValue::from(1));
    }
    let answer = crate::interaction::ask(crate::interaction::Prompt::Alert(m.clone()))?;
    let fallback = match m.buttons {
        MessageButtons::Ok => "1",
        MessageButtons::YesNo => "3",
        _ => "2",
    };
    let answer = answer.as_deref().unwrap_or(fallback);
    let accepted = match m.buttons {
        MessageButtons::Ok => ["1"].as_slice(),
        MessageButtons::OkCancel => ["1", "2"].as_slice(),
        MessageButtons::YesNo => ["3", "4"].as_slice(),
        MessageButtons::YesNoCancel => ["2", "3", "4"].as_slice(),
    };
    if answer == "1" && !accepted.contains(&"1") {
        return Ok(JsValue::from(2));
    }
    if !accepted.contains(&answer) {
        return Err(error("invalid alert response"));
    }
    Ok(JsValue::from(answer.parse::<i32>().unwrap_or(2)))
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

/// Return a page box in Acrobat's rotated-user-space order: left, top, right, bottom.
fn get_page_box(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let name = arg(args, 0);
    let name = if name.is_undefined() {
        "Crop".to_string()
    } else {
        name.as_string().ok_or_else(|| error("getPageBox box name must be a string"))?.to_std_string_escaped()
    };
    let which = match name.as_str() {
        "Media" | "MediaBox" => 0,
        "Crop" | "CropBox" => 1,
        "Bleed" | "BleedBox" => 2,
        "Trim" | "TrimBox" => 3,
        "Art" | "ArtBox" => 4,
        _ => return Err(error("getPageBox box name must be Media, Crop, Trim, Bleed, or Art")),
    };
    let page_value = arg(args, 1);
    let page = if page_value.is_undefined() {
        0
    } else {
        let page = page_value.to_number(ctx)?;
        if !page.is_finite() || page < 0.0 || page.fract() != 0.0 || page >= usize::MAX as f64 {
            return Err(error("getPageBox page must be a non-negative in-range integer"));
        }
        page as usize
    };
    let (rect, rotation, crop) = {
        let h = host(ctx)?.borrow();
        let boxes = h.doc.page_boxes.get(page).ok_or_else(|| error("getPageBox page is outside the document"))?;
        let rotation = h.doc.page_rotations.get(page).copied().unwrap_or(0);
        (boxes[which], rotation, boxes[1])
    };
    let [x0, y0, x1, y1] = rect;
    let [cx0, cy0, cx1, cy1] = crop;
    let rotated = match rotation {
        90 => [y0 - cy0, cx1 - x0, y1 - cy0, cx1 - x1],
        180 => [cx1 - x1, cy1 - y0, cx1 - x0, cy1 - y1],
        270 => [cy1 - y1, x1 - cx0, cy1 - y0, x0 - cx0],
        _ => [x0, y1, x1, y0],
    };
    if !rotated.iter().all(|value| value.is_finite()) {
        return Err(error("getPageBox returned non-finite coordinates"));
    }
    Ok(JsArray::from_iter(rotated.into_iter().map(JsValue::from), ctx).into())
}

/// Write a bounded line to the script console.
fn println(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = text_line(&arg(args, 0), ctx)?;
    host(ctx)?.borrow_mut().print(m);
    Ok(JsValue::undefined())
}

fn console_clear(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    host(ctx)?.borrow_mut().console.clear();
    Ok(JsValue::undefined())
}

/// Recalculate fields in the document's `/CO` order against the writes already made by this
/// script. A small depth cap stops calculate scripts from recursing forever.
fn calculate_now(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    const MAX_CALCULATE_DEPTH: usize = 8;
    let calculations = {
        let shared = host(ctx)?;
        let mut h = shared.borrow_mut();
        if h.calculate_depth >= MAX_CALCULATE_DEPTH {
            return Err(error("calculateNow recursion limit exceeded"));
        }
        h.calculate_depth += 1;
        h.calculations.clone()
    };
    let previous_event = match ctx.global_object().get(js_string!("event"), ctx) {
        Ok(event) => event,
        Err(error) => {
            if let Ok(shared) = host(ctx) {
                let mut h = shared.borrow_mut();
                h.calculate_depth = h.calculate_depth.saturating_sub(1);
            }
            return Err(error);
        }
    };
    let result = (|| {
        for (name, script) in calculations {
            let Some(field) = host(ctx)?.borrow().fields.iter().find(|f| f.name == name).cloned() else { continue };
            if let Some(why) = super::run::refuse(&script) {
                return Err(error(why));
            }
            let event = Event::field("Calculate", &name, &field.value.first().cloned().unwrap_or_default());
            let object = event_object(ctx, &event);
            ctx.global_object().set(js_string!("event"), object.clone(), true, ctx)?;
            ctx.eval(Source::from_bytes(&script))?;
            let rc = object.get(js_string!("rc"), ctx)?;
            if !rc.to_boolean() {
                continue;
            }
            let value = object.get(js_string!("value"), ctx)?;
            let value = text(&value, ctx)?;
            let shared = host(ctx)?;
            let mut h = shared.borrow_mut();
            if let Some((index, _)) = h.fields.iter().enumerate().find(|(_, f)| f.name == name) {
                let target = Arc::make_mut(&mut h.fields).get_mut(index).ok_or_else(|| error("the field is no longer available"))?;
                target.value = vec![value];
                h.changed.insert(index);
            }
        }
        Ok(JsValue::undefined())
    })();
    let restored = ctx.global_object().set(js_string!("event"), previous_event, true, ctx).map(|_| JsValue::undefined());
    if let Ok(shared) = host(ctx) {
        let mut h = shared.borrow_mut();
        h.calculate_depth = h.calculate_depth.saturating_sub(1);
    }
    result.and(restored)
}

/// Acrobat's built-in calculation helper, used by the scripts written by its Calculate tab.
fn af_simple_calculate(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let op = text(&arg(args, 0), ctx)?;
    let names = strings(&arg(args, 1), ctx)?;
    let mut values = Vec::new();
    for name in names {
        let field = read(ctx, &JsString::from(name))?;
        values.extend(field.value.iter().map(|v| super::host::as_number(v).unwrap_or(0.0)));
    }
    let result = match op.as_str() {
        "SUM" => values.iter().sum(),
        "PRD" => values.iter().product(),
        "AVG" if values.is_empty() => 0.0,
        "AVG" => values.iter().sum::<f64>() / values.len() as f64,
        "MIN" => values.iter().copied().reduce(f64::min).unwrap_or(0.0),
        "MAX" => values.iter().copied().reduce(f64::max).unwrap_or(0.0),
        _ => 0.0,
    };
    Ok(s(&if result.is_finite() { result.to_string() } else { "0".into() }))
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
fn zoom_get(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let z = host(ctx)?.borrow().doc.zoom;
    Ok(JsValue::from(if z > 0.0 { z } else { 100.0 }))
}
fn zoom_set(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let z = arg(args, 0).to_number(ctx)?;
    if !z.is_finite() || !(1.0..=6400.0).contains(&z) {
        return Err(error("zoom must be between 1 and 6400"));
    }
    host(ctx)?.borrow_mut().doc.zoom = z;
    host(ctx)?.borrow_mut().request(Request::Zoom(z));
    Ok(JsValue::undefined())
}
fn highlight_get(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().doc.runtime_highlight))
}
fn highlight_set(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let h = arg(args, 0).to_boolean();
    host(ctx)?.borrow_mut().doc.runtime_highlight = h;
    host(ctx)?.borrow_mut().request(Request::Highlight(h));
    Ok(JsValue::undefined())
}
pub(crate) fn install(ctx: &mut Context) -> JsResult<()> {
    let globals: [(&str, usize, NativeFunction); 11] = [
        ("getField", 1, NativeFunction::from_fn_ptr(get_field)),
        ("getNthFieldName", 1, NativeFunction::from_fn_ptr(get_nth_field_name)),
        ("resetForm", 1, NativeFunction::from_fn_ptr(reset_form)),
        ("print", 1, NativeFunction::from_fn_ptr(print)),
        ("submitForm", 1, NativeFunction::from_fn_ptr(submit_form)),
        ("mailDoc", 1, NativeFunction::from_fn_ptr(noop)),
        ("calculateNow", 0, NativeFunction::from_fn_ptr(calculate_now)),
        ("getPageBox", 2, NativeFunction::from_fn_ptr(get_page_box)),
        ("AFSimple_Calculate", 2, NativeFunction::from_fn_ptr(af_simple_calculate)),
        ("syncAnnotScan", 0, NativeFunction::from_fn_ptr(noop)),
        ("dirty", 0, NativeFunction::from_fn_ptr(noop)),
    ];
    for (name, len, f) in globals {
        ctx.register_global_callable(JsString::from(name), len, f)?;
    }
    let zg = function(ctx, NativeFunction::from_fn_ptr(zoom_get));
    let zs = function(ctx, NativeFunction::from_fn_ptr(zoom_set));

    let border = ObjectInitializer::new(ctx)
        .property(js_string!("s"), s("solid"), Attribute::all())
        .property(js_string!("d"), s("dashed"), Attribute::all())
        .property(js_string!("b"), s("beveled"), Attribute::all())
        .property(js_string!("i"), s("inset"), Attribute::all())
        .property(js_string!("u"), s("underline"), Attribute::all())
        .build();
    ctx.register_global_property(js_string!("border"), border, Attribute::CONFIGURABLE)?;
    let global = ctx.global_object();
    global.define_property_or_throw(
        js_string!("zoom"),
        boa_engine::property::PropertyDescriptor::builder().get(zg).set(zs).configurable(true).build(),
        ctx,
    )?;
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

    let hg = function(ctx, NativeFunction::from_fn_ptr(highlight_get));
    let hs = function(ctx, NativeFunction::from_fn_ptr(highlight_set));
    let app = ObjectInitializer::new(ctx)
        .accessor(js_string!("runtimeHighlight"), Some(hg), Some(hs), Attribute::CONFIGURABLE)
        .function(NativeFunction::from_fn_ptr(alert), js_string!("alert"), 1)
        .function(NativeFunction::from_fn_ptr(beep), js_string!("beep"), 0)
        .function(NativeFunction::from_fn_ptr(crate::interaction::response), js_string!("response"), 1)
        .function(NativeFunction::from_fn_ptr(launch_url), js_string!("launchURL"), 2)
        .function(NativeFunction::from_fn_ptr(crate::interaction::popup_menu), js_string!("popUpMenuEx"), 0)
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
