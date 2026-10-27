use super::host::host;
use super::instances::instance_manager;
use super::model::{MAX_DEPTH, XfaDoc, XfaEffect, XfaEvent, clip, reset_names};
use super::nodes::{node_index, node_object, noop, resolve_som, resolver, sink};
use super::*;

// ── xfa.host, xfa.layout, xfa.event, app, console ───────────────────────────────────────────

/// Records effect `e` on the host, as [`XHost::push`] does.
fn effect(ctx: &mut Context, e: XfaEffect) -> JsResult<()> {
    host(ctx)?.borrow_mut().push(e);
    Ok(())
}

/// `xfa.host.messageBox(message)`: records the message, cut to [`MAX_LINE`] characters, and
/// returns 1.
fn message_box(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = clip(text(&arg(args, 0), ctx)?);
    effect(ctx, XfaEffect::MessageBox(m))?;
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

/// `xfa.host.beep()`: asks to beep.
fn beep(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    effect(ctx, XfaEffect::Beep)?;
    Ok(JsValue::undefined())
}

/// `xfa.host.gotoURL(url)`: asks to open the URL.
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

pub(super) fn recalculate(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    effect(ctx, XfaEffect::Recalculate)?;
    Ok(JsValue::undefined())
}

pub(super) fn relayout(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    effect(ctx, XfaEffect::Relayout)?;
    Ok(JsValue::undefined())
}

fn console_println(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = clip(text(&arg(args, 0), ctx)?);
    host(ctx)?.borrow_mut().print(m);
    Ok(JsValue::undefined())
}

fn response(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::null())
}

fn layout_page(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().doc.page as f64 + 1.0))
}

fn layout_page0(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().doc.page as f64))
}

fn layout_page_count(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().doc.page_count as f64))
}

fn zero(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(0.0))
}

fn xfa_resolve(ctx: &mut Context, many: bool) -> JsResult<JsValue> {
    let current = host(ctx)?.borrow().current;
    resolver(ctx, current, many)
}

/// Defines the globals for one script on node `current`: `xfa` (`form`, `host`, `layout`, `event`,
/// `resolveNode` and more), `event` as the same object as `xfa.event`, `app`, `console`, `util`,
/// and the nodes in scope by name, with `_name` for each repeatable one's instance manager.
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
    let ev = ObjectInitializer::new(ctx)
        .property(js_string!("name"), s(&event.activity), Attribute::all())
        .property(js_string!("newText"), s(&event.new_text), Attribute::all())
        .property(js_string!("prevText"), s(&event.prev_text), Attribute::all())
        .property(js_string!("fullText"), s(&event.new_text), Attribute::all())
        .property(js_string!("change"), s(""), Attribute::all())
        .property(js_string!("target"), this_node.clone(), Attribute::all())
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
        .build();
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
    ctx.register_global_property(js_string!("event"), ev, rw)?;
    ctx.register_global_property(js_string!("__xfa_this"), this_node, rw)?;
    let app = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(message_box), js_string!("alert"), 4)
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
