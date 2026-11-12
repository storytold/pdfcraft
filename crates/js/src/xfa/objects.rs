//! Script objects: the named JavaScripts a form keeps under a subform's `<variables>` (XFA 3.3 chapter 10). Each one a
//! script can reach is a global whose getter compiles the object the first time it is read, so an object is built only
//! when used, one that uses another declared after it finds it built on demand, and one that does not load fails only
//! the scripts that reach it. Its subform reaches it as a child too (`form1.calcs`, `resolveNode("form1.calcs")`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use boa_engine::interner::Interner;
use boa_engine::parser::Parser;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsResult, JsValue, NativeFunction, Source, js_string};

use super::host::host;
use super::model::XfaKind;
use super::nodes::node_object;
use crate::{arg, function};

/// A script object a script can reach: its name, the SOM of the subform that declares it (`None` for one the form doesn't
/// name, or a page set's) and its code.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptObjectCode {
    pub name: String,
    pub som: Option<String>,
    pub text: Arc<str>,
    /// Reached by its bare name: `false` for an outer object an inner one of the same name hides, which its subform still
    /// reaches (`form1.Calc`).
    pub bare: bool,
}

/// The most script object code one script may have compiled: each object is at most 2 MiB, and a script reaching many large
/// ones is stopped here rather than compiling them all.
pub const MAX_LOADED_BYTES: usize = 8 << 20;

/// Names a script object does not take over as a global: what the host's scripts are run with.
const KEPT: &[&str] = &["xfa", "event", "__xfa_this", "__xfa_src", "__xfa_object", "__xfa_load", "globalThis", "undefined", "eval", "arguments"];

thread_local! {
    /// The ECMAScript built-ins (`Number`, `Date`, …): a script object of such a name would replace the constructor for every
    /// script that reaches it, so it is reached through its subform only.
    static BUILT_INS: Vec<String> = {
        let mut ctx = Context::default();
        let global = ctx.global_object();
        global.own_property_keys(&mut ctx).unwrap_or_default().iter().map(ToString::to_string).collect()
    };
    /// The members read from each object's code, by a hash of the code: a form's scripts reach the same objects again and again.
    static MEMBERS: RefCell<HashMap<u64, Result<Arc<str>, String>>> = RefCell::new(HashMap::new());
}

/// Defines the script objects of the running script as globals (an object named like a host global, `util` say, replaces it)
/// and the loader their getters call.
pub(super) fn define(ctx: &mut Context, objects: &[ScriptObjectCode]) -> JsResult<()> {
    if objects.is_empty() {
        return Ok(());
    }
    let loader = function(ctx, NativeFunction::from_fn_ptr(load));
    ctx.register_global_property(js_string!("__xfa_load"), loader, Attribute::CONFIGURABLE)?;
    let globals: Vec<String> = BUILT_INS.with(|built_ins| {
        objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.bare && !KEPT.contains(&o.name.as_str()) && !built_ins.contains(&o.name))
            .map(|(k, o)| format!("[{k}, {:?}]", o.name))
            .collect()
    });
    let code = format!(
        r#"(function (g, names) {{
  var cache = [], state = [];
  Object.defineProperty(g, "__xfa_object", {{ configurable: true, value: function (k) {{
    if (state[k] === 2) return cache[k];
    if (state[k] === 1) return undefined;
    state[k] = 1;
    try {{ cache[k] = __xfa_load(k); }} catch (e) {{ state[k] = 0; throw e; }}
    state[k] = 2;
    return cache[k];
  }} }});
  names.forEach(function (n) {{
    Object.defineProperty(g, n[1], {{ configurable: true,
      get: function () {{ return g.__xfa_object(n[0]); }},
      set: function (v) {{ Object.defineProperty(g, n[1], {{ value: v, writable: true, configurable: true }}); }} }});
  }});
}})(globalThis, [{}]);"#,
        globals.join(", ")
    );
    ctx.eval(Source::from_bytes(code.as_bytes()))?;
    Ok(())
}

/// The script object `name` that the subform at node `idx` declares, if it declares one: `undefined` otherwise.
pub(super) fn child_object(ctx: &mut Context, idx: usize, name: &str) -> JsResult<JsValue> {
    let k = {
        let h = host(ctx)?.borrow();
        let Some(som) = h.nodes.get(idx).map(|n| Arc::clone(&n.som)) else { return Ok(JsValue::undefined()) };
        h.objects.iter().position(|o| o.name == name && o.som.as_deref() == Some(&*som))
    };
    k.map_or(Ok(JsValue::undefined()), |k| object_at(ctx, k))
}

/// The reachable script object named `name`, wherever it is declared: what `resolveNode("calcs")` finds.
pub(super) fn named_object(ctx: &mut Context, name: &str) -> JsResult<JsValue> {
    let k = host(ctx)?.borrow().objects.iter().position(|o| o.name == name && o.bare);
    k.map_or(Ok(JsValue::undefined()), |k| object_at(ctx, k))
}

fn object_at(ctx: &mut Context, k: usize) -> JsResult<JsValue> {
    let loader = ctx.global_object().get(js_string!("__xfa_object"), ctx)?;
    match loader.as_callable() {
        Some(f) => f.call(&JsValue::undefined(), &[JsValue::from(k as f64)], ctx),
        None => Ok(JsValue::undefined()),
    }
}

/// `__xfa_load(k)`: compiles script object `k` and runs its code once, `this` its subform, returning an object with each
/// function and variable it declares at its top level.
fn load(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let k = arg(args, 0).to_number(ctx)? as usize;
    let (object, this) = {
        let mut h = host(ctx)?.borrow_mut();
        let Some(o) = h.objects.get(k).cloned() else { return Ok(JsValue::undefined()) };
        h.loaded_bytes = h.loaded_bytes.saturating_add(o.text.len());
        if h.loaded_bytes > MAX_LOADED_BYTES {
            let why = format!("the script object {} was not loaded: the script's objects pass {} MiB", o.name, MAX_LOADED_BYTES >> 20);
            return Err(JsNativeError::range().with_message(why).into());
        }
        let this = o.som.as_deref().and_then(|som| h.nodes.iter().position(|n| &*n.som == som && n.kind != XfaKind::Form));
        (o, this)
    };
    let failed = |why: String| JsNativeError::error().with_message(format!("the script object {} did not load: {why}", object.name));
    if let Some(why) = crate::run::refuse_nesting(&object.text) {
        return Err(failed(why).into());
    }
    let members = members_of(&object.text).map_err(failed)?;
    let code = format!("(function () {{\n{}\n;return (function (o) {{ {members} return o; }})({{}});\n}})", object.text);
    let build = ctx.eval(Source::from_bytes(code.as_bytes())).map_err(|e| failed(e.to_string()))?;
    let this = match this {
        Some(i) => node_object(ctx, i)?,
        None => JsValue::undefined(),
    };
    let Some(build) = build.as_callable() else { return Ok(JsValue::undefined()) };
    build.call(&this, &[], ctx).map_err(|e| failed(e.to_string()).into())
}

/// A live member on `o` for every function, variable and lexical name `text` declares at its top level (`var`s in its blocks
/// too): reading it reads the binding, writing it writes the binding, so `Calc.step = 10` changes what `Calc.twice` uses.
/// The names are read by the parser, so regular expressions, templates and strings are read as they run.
fn members_of(text: &str) -> Result<Arc<str>, String> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let key = hasher.finish();
    if let Some(known) = MEMBERS.with(|m| m.borrow().get(&key).cloned()) {
        return known;
    }
    let read = read_members(text);
    MEMBERS.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= 256 {
            m.clear();
        }
        m.insert(key, read.clone());
    });
    read
}

fn read_members(text: &str) -> Result<Arc<str>, String> {
    use boa_engine::ast::StatementListItem;
    use boa_engine::ast::operations::{bound_names, var_declared_names};
    let mut interner = Interner::default();
    let body = Parser::new(Source::from_bytes(text.as_bytes())).parse_function_body(&mut interner, false, false).map_err(|e| e.to_string())?;
    let mut names: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in body.statements() {
        let syms: Vec<_> = match item {
            StatementListItem::Statement(s) => var_declared_names(&**s).into_iter().collect(),
            StatementListItem::Declaration(d) => bound_names(&**d),
        };
        for sym in syms {
            let name = interner.resolve_expect(sym).to_string();
            if seen.insert(name.clone()) {
                names.push(name);
            }
        }
    }
    names.sort();
    let member = |n: &String| {
        format!(
            "Object.defineProperty(o, {n:?}, {{ enumerable: true, configurable: true, get: function () {{ return {n}; }}, set: function (v) {{ {n} = v; }} }});"
        )
    };
    Ok(names.iter().map(member).collect::<Vec<_>>().join(" ").into())
}
