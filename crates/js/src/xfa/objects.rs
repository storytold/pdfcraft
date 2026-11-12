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
    /// No event script names it: loaded once an object its code names has loaded, as a module extending it.
    pub unnamed: bool,
}

/// The most script object code one script may have compiled: each object is at most 2 MiB, and a script reaching many large
/// ones is stopped here rather than compiling them all.
pub const MAX_LOADED_BYTES: usize = 8 << 20;

/// The most names one object may declare at its top level: each becomes an accessor, which boa compiles in time growing with
/// the square of their count. Real objects declare a few hundred at most.
pub const MAX_MEMBERS: usize = 2048;

/// Names a script object does not take over as a global: what the host's scripts are run with.
const KEPT: &[&str] = &[
    "xfa",
    "event",
    "__xfa_this",
    "__xfa_src",
    "__xfa_object",
    "__xfa_load",
    "__xfa_extend",
    "__xfa_built",
    "__xfa_prepare",
    "globalThis",
    "undefined",
    "eval",
    "arguments",
];

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

/// The script objects a script reaches, set up in the realm they are built in, with `expose`, which gives another realm the
/// getters that reach them. Scripts reaching the same objects share it, and an object one of them built is that same
/// object for the others: it is built once, and keeps its state.
pub(super) struct Defined {
    /// See [`key`].
    pub(super) key: u64,
    expose: JsValue,
    /// The bare names it defined in the objects' realm.
    pub(super) names: Vec<String>,
}

/// What tells one script's reachable objects from another's: each one's identity and how the script reaches it, in order.
pub(super) fn key(objects: &[ScriptObjectCode]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for o in objects {
        identity(o).hash(&mut h);
        (o.bare, o.unnamed).hash(&mut h);
    }
    h.finish()
}

/// Sets `objects` up in the current realm (the one they are built in): an object named like a host global (`util`, say)
/// replaces it there and in each realm [`expose`] is given.
pub(super) fn setup(ctx: &mut Context, objects: &[ScriptObjectCode], key: u64) -> JsResult<Defined> {
    let globals: Vec<(usize, String)> = BUILT_INS.with(|built_ins| {
        objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.bare && !KEPT.contains(&o.name.as_str()) && !built_ins.contains(&o.name))
            .map(|(k, o)| (k, o.name.clone()))
            .collect()
    });
    let load = function(ctx, NativeFunction::from_fn_ptr(load));
    ctx.register_global_property(js_string!("__xfa_load"), load, Attribute::CONFIGURABLE)?;
    let extend = function(ctx, NativeFunction::from_fn_ptr(extend));
    ctx.register_global_property(js_string!("__xfa_extend"), extend, Attribute::CONFIGURABLE)?;
    let prepare = function(ctx, NativeFunction::from_fn_ptr(prepare));
    ctx.register_global_property(js_string!("__xfa_prepare"), prepare, Attribute::CONFIGURABLE)?;
    let ids: Vec<String> = objects.iter().map(|o| format!("{:?}", identity(o))).collect();
    let names: Vec<String> = globals.iter().map(|(k, n)| format!("[{k}, {n:?}]")).collect();
    let code = format!(
        r#"(function (g, ids, names) {{
  if (!g.__xfa_built) Object.defineProperty(g, "__xfa_built", {{ configurable: true, value: Object.create(null) }});
  var built = g.__xfa_built, state = [];
  function loader(k) {{
    var id = ids[k];
    if (id === undefined) return undefined;
    __xfa_prepare();
    if (id in built) return built[id];
    if (state[k] === 1) return undefined;
    state[k] = 1;
    var o;
    try {{ o = __xfa_load(k); }} catch (e) {{ state[k] = 0; throw e; }}
    state[k] = 0;
    built[id] = o;
    try {{ __xfa_extend(k); }} catch (e) {{}}
    return o;
  }}
  function expose(t) {{
    Object.defineProperty(t, "__xfa_object", {{ configurable: true, value: loader }});
    names.forEach(function (n) {{
      Object.defineProperty(t, n[1], {{ configurable: true,
        get: function () {{ return loader(n[0]); }},
        set: function (v) {{ Object.defineProperty(t, n[1], {{ value: v, writable: true, configurable: true }}); }} }});
    }});
  }}
  expose(g);
  return [loader, expose];
}})(globalThis, [{}], [{}])"#,
        ids.join(", "),
        names.join(", ")
    );
    let pair = ctx.eval(Source::from_bytes(code.as_bytes()))?;
    let pair = pair.as_object().ok_or_else(|| JsNativeError::typ().with_message("the script objects were not set up"))?;
    let expose = pair.get(1, ctx)?;
    Ok(Defined { key, expose, names: globals.into_iter().map(|(_, n)| n).collect() })
}

/// Gives the current realm (a script's own) the objects of `d`: `__xfa_object` and a getter for each bare name.
pub(super) fn expose(ctx: &mut Context, d: &Defined) -> JsResult<()> {
    let global: JsValue = ctx.global_object().into();
    match d.expose.as_callable() {
        Some(f) => f.call(&JsValue::undefined(), &[global], ctx).map(|_| ()),
        None => Ok(()),
    }
}

thread_local! {
    /// The hash of each object's code, by where the code is: a session's scripts reach the same objects again and again.
    static CODE_HASHES: RefCell<HashMap<(usize, usize), u64>> = RefCell::new(HashMap::new());
}

/// Forget the code hashes: the objects they were taken from may be gone.
pub(super) fn forget_hashes() {
    CODE_HASHES.with(|c| c.borrow_mut().clear());
}

/// What tells one script object from another across a session's scripts: its subform, name and code.
fn identity(o: &ScriptObjectCode) -> String {
    use std::hash::{Hash, Hasher};
    let at = (o.text.as_ptr() as usize, o.text.len());
    let hash = CODE_HASHES.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(h) = c.get(&at) {
            return *h;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        o.text.hash(&mut h);
        let h = h.finish();
        if c.len() >= 4096 {
            c.clear();
        }
        c.insert(at, h);
        h
    });
    format!("{}|{}|{:x}", o.som.as_deref().unwrap_or(""), o.name, hash)
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

/// `__xfa_prepare()`: sets the realm the objects live in up for the running script (its form, event and `this`), the first
/// time the script reaches an object: their code reads `xfa` there.
fn prepare(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let (event, doc, current) = {
        let mut h = host(ctx)?.borrow_mut();
        if h.objects_ready {
            return Ok(JsValue::undefined());
        }
        h.objects_ready = true;
        (h.event.clone(), h.doc.clone(), h.current)
    };
    super::globals::install(ctx, &event, &doc, current)?;
    Ok(JsValue::undefined())
}

/// `__xfa_extend(k)`: loads the never-named objects whose code names object `k`, now that `k` is built (a failing one is left
/// out, as it was never run before).
fn extend(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let k = arg(args, 0).to_number(ctx)? as usize;
    let modules: Vec<usize> = {
        let h = host(ctx)?.borrow();
        let Some(name) = h.objects.get(k).map(|o| o.name.clone()) else { return Ok(JsValue::undefined()) };
        h.objects.iter().enumerate().filter(|(j, o)| *j != k && o.unnamed && words(&o.text).any(|w| w == name)).map(|(j, _)| j).collect()
    };
    for j in modules {
        let _ = object_at(ctx, j);
    }
    Ok(JsValue::undefined())
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
    // the members it exports are code compiled too: a short object declaring many names comes to more than its text
    {
        let mut h = host(ctx)?.borrow_mut();
        h.loaded_bytes = h.loaded_bytes.saturating_add(members.len());
        if h.loaded_bytes > MAX_LOADED_BYTES {
            let why = format!("the script object {} was not loaded: the script's objects pass {} MiB", object.name, MAX_LOADED_BYTES >> 20);
            return Err(JsNativeError::range().with_message(why).into());
        }
    }
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
    if names.len() > MAX_MEMBERS {
        return Err(format!("it declares {} names at its top level, more than {MAX_MEMBERS}", names.len()));
    }
    names.sort();
    let member = |n: &String| {
        format!(
            "Object.defineProperty(o, {n:?}, {{ enumerable: true, configurable: true, get: function () {{ return {n}; }}, set: function (v) {{ {n} = v; }} }});"
        )
    };
    Ok(names.iter().map(member).collect::<Vec<_>>().join(" ").into())
}

/// The identifier-like words of `text`, strings and comments not told apart.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).filter(|w| !w.is_empty())
}
