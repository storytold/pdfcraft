use super::host::host;
use super::instances::instance_manager;
use super::model::{MAX_DEPTH, MAX_NODES, XfaDoc, XfaEffect, XfaEvent, XfaKind, clip, reset_names};
use super::nodes::{node_index, node_object, noop, resolve_som, resolver, sink};
use super::{
    Attribute, Context, HashSet, JsResult, JsString, JsValue, NativeFunction, ObjectInitializer, arg, data_object, error, js_string, platform,
    printd, printf, printx, s, scand, text,
};

// ── xfa.host, xfa.layout, xfa.event, app, console ───────────────────────────────────────────

/// Records effect `e` on the host, against the run's limits, as [`XHost::push`] does.
fn effect(ctx: &mut Context, e: XfaEffect) -> JsResult<()> {
    host(ctx)?.borrow_mut().push(e);
    Ok(())
}

/// `event.target.exportDataObject(…)`: the document asks to open or save an attached file.
fn export_data_object(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let (name, open) = data_object(args, ctx)?;
    effect(ctx, XfaEffect::OpenAttachment { name, open })?;
    Ok(JsValue::undefined())
}

/// `event.target.getField(name)`: the document's field whose fully qualified name is `name`.
/// A field PdfCraft laid out is named by the SOM expression of its XFA field with every index
/// written (`form1[0].page1[0].total[0]`), as form tools name the PDF fields of an XFA form
/// (XFA 3.3, ch. 2, "Field Names", pp. 72-74), and only that spelling finds it: no other
/// spelling, short name or missing index does. A subform's SOM expression gives an object that
/// names the subform. A signature field isn't found by an initialize script (it is from later
/// events). Anything else gives `null`; no name at all is an error.
fn get_field(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let Some(wanted) = args.first() else { return Err(error("getField: a field name is required")) };
    let name = clip(text(wanted, ctx)?);
    if name.is_empty() {
        return Ok(JsValue::null());
    }
    let kind = crate::host::host(ctx)?.borrow().fields.iter().find(|f| f.name == name).map(|f| f.kind);
    match kind {
        Some(crate::FieldType::Signature) if host(ctx)?.borrow().activity == "initialize" => Ok(JsValue::null()),
        Some(_) => Ok(crate::document::field_object(ctx, &name).into()),
        None => {
            let subform = host(ctx)?.borrow().nodes.iter().any(|n| !n.gone && n.som == name && n.kind == XfaKind::Subform);
            Ok(if subform {
                ObjectInitializer::new(ctx).property(js_string!("name"), s(&name), Attribute::READONLY).build().into()
            } else {
                JsValue::null()
            })
        }
    }
}

/// A SOM path without its indexes (`form.page1.qty`), for matching the names scripts pass.
fn plain_som(som: &str) -> String {
    let mut out = String::with_capacity(som.len());
    let mut skip = false;
    for c in som.chars() {
        match c {
            '[' => skip = true,
            ']' => skip = false,
            _ if !skip => out.push(c),
            _ => {}
        }
    }
    out
}

/// Does `name` from a script (`qty`, `page1.qty`, `xfa.form.form.page1.qty`, a SOM expression with
/// its indexes, or a container such as `table`) name the object at `som` or one of its
/// ancestors? What `xfa.host.resetData` and `event.target.resetForm` reset by.
pub fn som_names(som: &str, name: &str) -> bool {
    let plain = plain_som(som);
    let n = plain_som(name.trim());
    let n = n.strip_prefix("xfa.form.").or_else(|| n.strip_prefix("$form.")).unwrap_or(&n);
    let want: Vec<&str> = n.split('.').filter(|s| !s.is_empty()).collect();
    let have: Vec<&str> = plain.split('.').filter(|s| !s.is_empty()).collect();
    !want.is_empty() && have.windows(want.len()).any(|w| w == want.as_slice())
}

/// `event.target.resetForm(names)`: resets the named fields (an array of names, or one name; a
/// name is matched as [`som_names`] matches it), or every field when no names are given; a list
/// that names nothing (an empty array, or only missing or empty entries) resets nothing. The
/// script reads the fields' default values back at once; the form itself is reset by the
/// effect when the script is done, as for `xfa.host.resetData`.
fn reset_form(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let given = arg(args, 0);
    let all = given.is_undefined() || given.is_null();
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for name in crate::host::strings(&given, ctx)? {
        let name = clip(name.trim().to_string());
        if !name.is_empty() && names.len() < MAX_NODES && seen.insert(name.clone()) {
            names.push(name);
        }
    }
    if !all && names.is_empty() {
        return Ok(JsValue::undefined());
    }
    effect(ctx, XfaEffect::ResetData(names.clone()))?;
    // What the script reads from now on: each reset field's default value.
    let reset: Vec<(String, Vec<String>)> = {
        let mut fields = crate::host::host(ctx)?.borrow_mut();
        let fields = std::sync::Arc::make_mut(&mut fields.fields);
        let mut out = Vec::new();
        for f in fields.iter_mut() {
            let named = names.is_empty() || names.iter().any(|n| som_names(&f.name, n));
            if named && !matches!(f.kind, crate::FieldType::Button | crate::FieldType::Signature) {
                f.value = f.default.clone();
                out.push((f.name.clone(), f.default.clone()));
            }
        }
        out
    };
    let mut h = host(ctx)?.borrow_mut();
    for (som, value) in reset {
        if let Some(n) = h.nodes.iter_mut().find(|n| !n.gone && n.som == som && n.kind == XfaKind::Field) {
            n.value = value.join("\n");
            n.formatted = None;
        }
    }
    Ok(JsValue::undefined())
}

/// A script changed `before` into `after` through `getField`: a new value is set on the XFA
/// field the same way `rawValue` sets it (so the script reads it back there too); properties
/// XFA fields have no counterpart for here are noted and left as they were.
pub(crate) fn field_written(ctx: &mut Context, before: &crate::FieldState, after: &crate::FieldState) -> JsResult<()> {
    let mut h = host(ctx)?.borrow_mut();
    if before.value != after.value {
        let at = h.nodes.iter().position(|n| !n.gone && n.som == after.name);
        if !at.is_some_and(|i| h.set_value(i, after.value.join("\n"))) {
            h.unapplied(format!("{}: value", after.name));
        }
    }
    let others = [
        ("readonly", before.readonly != after.readonly),
        ("required", before.required != after.required),
        ("display", before.display != after.display),
        ("textColor", before.text_color != after.text_color),
        ("fillColor", before.fill_color != after.fill_color),
    ];
    for (property, changed) in others {
        if changed {
            h.unapplied(format!("{}: {property}", after.name));
        }
    }
    Ok(())
}

/// `xfa.host.messageBox(message, title, icon, buttons)`: records the message with its title,
/// icon and buttons, its text and title cut to [`MAX_LINE`] characters, and returns 1.
fn message_box(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = crate::globals::message_box_message(args, ctx)?;
    effect(ctx, XfaEffect::MessageBox(m))?;
    // Always OK for now: the viewer can't ask the user and run the script on with the answer
    // yet (#4291).
    Ok(JsValue::from(1.0))
}

/// `app.alert(cMsg, nIcon, nType, cTitle)` in an XFA form's script: the same message box, its
/// arguments in `app.alert`'s order (or one object with those keys).
fn app_alert(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = crate::globals::alert_message(args, ctx)?;
    effect(ctx, XfaEffect::MessageBox(m))?;
    // The OK button, as for `messageBox` (#4291).
    Ok(JsValue::from(1.0))
}

/// `xfa.host.resetData(names)`: asks to reset the comma-separated fields, or every field when
/// the list is empty.
fn reset_data(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let list = text(&arg(args, 0), ctx)?;
    effect(ctx, XfaEffect::ResetData(reset_names(&list)))?;
    Ok(JsValue::undefined())
}

/// `xfa.host.print(…)`: asks to print; its arguments are ignored.
fn print(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    effect(ctx, XfaEffect::Print)?;
    Ok(JsValue::undefined())
}

/// `xfa.host.beep()` and `app.beep()`: ask to beep.
fn beep(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    effect(ctx, XfaEffect::Beep)?;
    Ok(JsValue::undefined())
}

/// `xfa.host.gotoURL(url)` and `app.launchURL(url)`: ask to open the URL.
fn goto_url(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let u = text(&arg(args, 0), ctx)?;
    effect(ctx, XfaEffect::LaunchUrl(u))?;
    Ok(JsValue::undefined())
}

/// `xfa.host.setFocus(target)`: asks to focus the node, given as a node object or as a SOM
/// expression resolved from the current node; an expression that names no node is passed on
/// as written.
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

/// `app.execMenuItem(name)`: the effect for the request the shared menu table gives the name
/// (Save opens Save As too); an unknown name does nothing.
fn exec_menu_item(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let item = text(&arg(args, 0), ctx)?;
    match crate::menu_item(&item) {
        Some(crate::Request::SaveAs) => effect(ctx, XfaEffect::SaveAs)?,
        Some(crate::Request::Print) => effect(ctx, XfaEffect::Print)?,
        Some(crate::Request::ShowPanel(p)) => effect(ctx, XfaEffect::ShowPanel(p))?,
        _ => {}
    }
    Ok(JsValue::undefined())
}

/// `recalculate(…)` on a node or on `xfa`: records a request to recalculate the form.
pub(super) fn recalculate(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    effect(ctx, XfaEffect::Recalculate)?;
    Ok(JsValue::undefined())
}

/// `xfa.layout.relayout()`, `relayoutPageArea()` and `remerge()`: record a request to lay the
/// form out again.
pub(super) fn relayout(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    effect(ctx, XfaEffect::Relayout)?;
    Ok(JsValue::undefined())
}

/// `console.println(…)`: adds the line, cut to [`MAX_LINE`] characters, to the run's console
/// output.
fn console_println(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = clip(text(&arg(args, 0), ctx)?);
    host(ctx)?.borrow_mut().print(m);
    Ok(JsValue::undefined())
}

/// `xfa.host.response(…)` and `app.response(…)`: always return `null`, as for a canceled dialog.
fn response(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::null())
}

/// `xfa.layout.page(…)`: the current page, 1-based, whatever the argument.
fn layout_page(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().doc.page as f64 + 1.0))
}

/// `xfa.layout.absPage(…)`: the current page, 0-based, whatever the argument.
fn layout_page0(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().doc.page as f64))
}

/// `xfa.layout.pageCount()` and `absPageCount()`: the document's page count.
fn layout_page_count(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().doc.page_count as f64))
}

/// The `xfa.layout` measures `h`, `w`, `x`, `y` and `pageSpan`: always 0.
fn zero(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(0.0))
}

/// `xfa.resolveNode(som)`, or `xfa.resolveNodes(som)` when `many`, resolving from the node the
/// script runs on.
fn xfa_resolve(ctx: &mut Context, many: bool) -> JsResult<JsValue> {
    let current = host(ctx)?.borrow().current;
    resolver(ctx, current, many)
}

/// Defines the globals for one script on node `current`: `xfa` (`form`, `host`, `layout`, `event`,
/// `resolveNode` and more; `xfa.event`'s target is that node), a separate global `event` whose
/// target is a document offering `exportDataObject`, `app`, `console`, `util`, and the nodes in
/// scope by name, with `_name` for each repeatable one's instance manager.
pub(super) fn install(ctx: &mut Context, event: &XfaEvent, doc: &XfaDoc, current: usize) -> JsResult<()> {
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
    // Two event objects, as in Acrobat's XFA scripting: `xfa.event`, whose target is the form
    // node the event happened on, and Acrobat's own `event`, whose target is the document
    // (`event.target.exportDataObject(…)` is how forms open their attached files).
    let acrobat_doc = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(export_data_object), js_string!("exportDataObject"), 4)
        .function(NativeFunction::from_fn_ptr(get_field), js_string!("getField"), 1)
        .function(NativeFunction::from_fn_ptr(reset_form), js_string!("resetForm"), 1)
        .build();
    let event_object = |ctx: &mut Context, target: JsValue| {
        ObjectInitializer::new(ctx)
            .property(js_string!("name"), s(&event.activity), Attribute::all())
            .property(js_string!("newText"), s(&event.new_text), Attribute::all())
            .property(js_string!("prevText"), s(&event.prev_text), Attribute::all())
            .property(js_string!("fullText"), s(&event.new_text), Attribute::all())
            .property(js_string!("change"), s(""), Attribute::all())
            .property(js_string!("target"), target, Attribute::all())
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
            .build()
    };
    let ev = event_object(ctx, this_node.clone());
    let acrobat_ev = event_object(ctx, acrobat_doc.into());
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
    ctx.register_global_property(js_string!("event"), acrobat_ev, rw)?;
    ctx.register_global_property(js_string!("__xfa_this"), this_node, rw)?;
    let app = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(app_alert), js_string!("alert"), 4)
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
        .function(NativeFunction::from_fn_ptr(scand), js_string!("scand"), 2)
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
    let mut defined: Vec<String> = RESERVED.iter().map(ToString::to_string).collect();
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
