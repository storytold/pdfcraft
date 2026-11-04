use super::globals::{event_object, install};
use super::host::{Host, Shared, text};
use super::model::{DocInfo, Event, FieldState, Limits, Outcome};
use super::{Arc, Attribute, BTreeSet, Context, HostHooks, JsNativeError, JsResult, JsString, Realm, RefCell, Source, js_string};

/// Longest script, in bytes.
pub(crate) const MAX_SCRIPT_BYTES: usize = 256 * 1024;

/// Largest width or precision accepted by `util.printf`.
pub(crate) const MAX_PRINTF_WIDTH_PRECISION: usize = 4096;

/// Longest `util.printf` output, in bytes.
pub(crate) const MAX_PRINTF_OUTPUT_BYTES: usize = 1 << 20;

/// Longest array read item by item (`resetForm`, a field's `value` or colours). A sparse array's
/// length costs the script nothing but the host one string per item; real lists are tiny.
pub(crate) const MAX_ARRAY_LENGTH: u64 = 1 << 20;

/// Deepest nesting of `(`, `[` and `{`.
const MAX_BRACKET_DEPTH: usize = 64;

/// Longest run of prefix operators (`!`, `~`, `+`, `-`).
const MAX_PREFIX_RUN: usize = 64;

/// Most `?` (conditional expressions nest without brackets).
const MAX_CONDITIONALS: usize = 4096;

/// Longest active arrow-function chain across the currently open bracket levels.
const MAX_ARROW_FUNCTIONS: usize = 256;

/// Assignment and exponentiation associate right-to-left without increasing bracket depth.
/// Bound active chains, rather than the total operators in independent statements.
const MAX_RIGHT_ASSOCIATIVE_OPERATORS: usize = 4096;

/// The script thread's stack.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const SCRIPT_STACK: usize = 1 << 30;

/// Persistent document-worker stack, bounded separately from transient legacy runners.
/// Parser/compiler boundary and hostile-input regressions run through the real worker.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const DOCUMENT_WORKER_STACK: usize = 64 << 20;

mod preflight;

/// Refuse unsafe source nesting before Boa parses it; independent statements do not share a chain budget.
pub(crate) fn refuse(source: &str) -> Option<String> {
    preflight::refuse(source)
}

/// Run `script` for `event`, after the document-level scripts (`doc_scripts`, which define the
/// functions field scripts call), with the form's `fields`.
pub fn run(script: &str, event: &Event, doc: &DocInfo, fields: &[FieldState], doc_scripts: &[String], limits: Limits) -> Outcome {
    run_shared(script, event, doc, Arc::new(fields.to_vec()), doc_scripts, limits)
}

/// Like [`run`], sharing an immutable initial field snapshot. Scripts still get a fresh
/// context; the first field write copies the snapshot, so neither the caller nor another
/// script can observe uncommitted writes. Read-only calculations avoid copying the whole form.
pub fn run_shared(script: &str, event: &Event, doc: &DocInfo, fields: Arc<Vec<FieldState>>, doc_scripts: &[String], limits: Limits) -> Outcome {
    run_shared_calculations(script, event, doc, fields, doc_scripts, &[], limits)
}

/// Like [`run_shared`], with the document's ordered calculated fields available to
/// `calculateNow()` while this context is running. Each tuple contains the target
/// position in `fields` and its source; the engine resolves positions by PDF object identity.
pub fn run_shared_calculations(
    script: &str,
    event: &Event,
    doc: &DocInfo,
    fields: Arc<Vec<FieldState>>,
    doc_scripts: &[String],
    calculations: &[(usize, String)],
    limits: Limits,
) -> Outcome {
    let failed = |why: String| Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), error: Some(why), ..Default::default() };
    if let Some(why) = refuse(script) {
        return failed(why);
    }
    // A document script too long or too deeply nested to run safely is skipped and reported,
    // as one that throws is; the others and the script still run (#3940).
    let mut skipped = Vec::new();
    let kept: Vec<(usize, &str)> = doc_scripts
        .iter()
        .enumerate()
        .filter_map(|(i, d)| match refuse(d) {
            Some(why) => {
                skipped.push((i, why));
                None
            }
            None => Some((i, d.as_str())),
        })
        .collect();
    let (kept, skipped) = (&kept, &skipped);
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::scope(|scope| {
            let spawned = std::thread::Builder::new().name("pdfcraft-js".into()).stack_size(SCRIPT_STACK).spawn_scoped(scope, || {
                run_here(
                    script,
                    event,
                    doc,
                    fields,
                    ScriptSetup { doc_scripts: kept, skipped, calculations, initialize: true, reset_calls: 0 },
                    limits,
                )
            });
            match spawned {
                Ok(thread) => thread.join().unwrap_or_else(|_| failed("the script stopped with an internal error".into())),
                Err(e) => failed(format!("the script engine could not start: {e}")),
            }
        })
    }
    #[cfg(target_arch = "wasm32")]
    run_here(script, event, doc, fields, ScriptSetup { doc_scripts: kept, skipped, calculations, initialize: true, reset_calls: 0 }, limits)
}

/// Host hooks that refuse to compile code from strings (`eval`, `Function` and its generator
/// and async kin). Such code is built at run time, so it never passed [`refuse`], and boa's
/// recursive parser would overflow the stack on a deeply nested one.
pub(crate) struct NoCodeFromStrings;

impl HostHooks for NoCodeFromStrings {
    fn ensure_can_compile_strings(&self, _: Realm, _: &[JsString], _: &JsString, _: bool, _: &mut Context) -> JsResult<()> {
        Err(JsNativeError::typ().with_message("scripts can't compile code from strings (eval, Function)").into())
    }
}

/// Host hooks that hold code compiled from strings to the checks a script passes ([`refuse`]),
/// for runners that start the script itself through `eval`.
pub(crate) struct CheckedCodeFromStrings;

impl HostHooks for CheckedCodeFromStrings {
    fn ensure_can_compile_strings(&self, _: Realm, parameters: &[JsString], body: &JsString, _: bool, _: &mut Context) -> JsResult<()> {
        let units = parameters.iter().chain(std::iter::once(body)).fold(0usize, |n, s| n.saturating_add(s.len()));
        if units > MAX_SCRIPT_BYTES {
            return Err(JsNativeError::range().with_message("the code is too long to compile").into());
        }
        // As one text: brackets left open in one parameter carry into the next.
        let mut source = parameters.iter().map(JsString::to_std_string_lossy).collect::<Vec<_>>().join(",");
        source.push('\n');
        source.push_str(&body.to_std_string_lossy());
        match refuse(&source) {
            Some(why) => Err(JsNativeError::range().with_message(why).into()),
            None => Ok(()),
        }
    }
}

/// How [`Outcome::doc_errors`] names a failing document script: by its 1-based place.
fn doc_error(index: usize, error: &dyn std::fmt::Display) -> String {
    format!("document script {}: {error}", index.saturating_add(1))
}

/// `doc_scripts` are the document scripts to run with their places among all of them;
/// `skipped` the ones refused before running, with the reason.
pub(crate) struct ScriptSetup<'a> {
    pub(crate) doc_scripts: &'a [(usize, &'a str)],
    pub(crate) skipped: &'a [(usize, String)],
    pub(crate) calculations: &'a [(usize, String)],
    pub(crate) initialize: bool,
    pub(crate) reset_calls: usize,
}

/// Create a restricted temporary Boa context and convert initialization failures into script outcomes.
fn run_here(script: &str, event: &Event, doc: &DocInfo, fields: Arc<Vec<FieldState>>, setup: ScriptSetup<'_>, limits: Limits) -> Outcome {
    match Context::builder().host_hooks(std::rc::Rc::new(NoCodeFromStrings)).build() {
        Ok(mut ctx) => run_in_context(&mut ctx, script, event, doc, fields, setup, limits),
        Err(e) => Outcome { rc: true, error: Some(format!("the script engine could not start: {e}")), ..Default::default() },
    }
}

/// Replace per-event host state, optionally initialize document scripts, and collect results within runtime limits.
pub(crate) fn run_in_context(
    ctx: &mut Context,
    script: &str,
    event: &Event,
    doc: &DocInfo,
    fields: Arc<Vec<FieldState>>,
    setup: ScriptSetup<'_>,
    limits: Limits,
) -> Outcome {
    ctx.runtime_limits_mut().set_loop_iteration_limit(limits.loop_iterations);
    ctx.runtime_limits_mut().set_recursion_limit(limits.recursion);
    let (icons, objects) = ctx
        .remove_data::<Shared>()
        .map(|h| {
            let h = h.into_inner();
            (h.icons, h.objects)
        })
        .unwrap_or_default();
    ctx.insert_data(RefCell::new(Host {
        fields,
        icons,
        objects,
        changed: BTreeSet::new(),
        alerts: Vec::new(),
        console: Vec::new(),
        requests: Vec::new(),
        doc: doc.clone(),
        calculations: setup.calculations.to_vec(),
        calculate_depth: 0,
        reset_calls: setup.reset_calls,
        reset_limit_exceeded: false,
        kept: 0,
        dropped: 0,
    }));
    let mut out = Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), ..Default::default() };
    let mut doc_errors: Vec<(usize, String)> = setup.skipped.iter().map(|(i, why)| (*i, doc_error(*i, why))).collect();
    let result = (|| -> JsResult<()> {
        if setup.initialize {
            install(ctx)?;
        }
        let ev = event_object(ctx, event);
        ctx.register_global_property(js_string!("event"), ev.clone(), Attribute::all())?;
        // One document script that throws (calling a method PdfCraft lacks, say) must not keep
        // every field script in the form from running: what it defined before the error, and
        // the other scripts, stay available (#3940).
        for &(i, d) in setup.doc_scripts {
            if let Err(e) = ctx.eval(Source::from_bytes(d)) {
                doc_errors.push((i, doc_error(i, &e)));
            }
        }
        let r = ctx.eval(Source::from_bytes(script))?;
        if !r.is_undefined() {
            out.result = Some(text(&r, ctx)?);
        }
        let rc = ev.get(js_string!("rc"), ctx)?;
        out.rc = rc.to_boolean();
        let v = ev.get(js_string!("value"), ctx)?;
        out.value = text(&v, ctx)?;
        let c = ev.get(js_string!("change"), ctx)?;
        out.change = text(&c, ctx)?;
        Ok(())
    })();
    if let Err(e) = result {
        out.error = Some(e.to_string());
    }
    doc_errors.sort_by_key(|(i, _)| *i);
    out.doc_errors = doc_errors.into_iter().map(|(_, e)| e).collect();
    if let Some(h) = ctx.remove_data::<Shared>() {
        let mut h = h.into_inner();
        out.reset_limit_exceeded = h.reset_limit_exceeded;
        out.changed = h.changed.iter().filter_map(|i| h.fields.get(*i).cloned()).collect();
        out.alerts = std::mem::take(&mut h.alerts);
        out.console = std::mem::take(&mut h.console);
        out.requests = std::mem::take(&mut h.requests);
        if h.dropped > 0 {
            out.console.push(format!("the script's messages, console lines and requests passed their limits; {} more were left out", h.dropped));
        }
        ctx.insert_data(RefCell::new(h));
    }
    out
}

#[cfg(test)]
mod chain_budget_tests {
    use super::refuse;
    /// Shift assignments remain right-associative while comparison tokens consume no chain budget.
    #[test]
    fn comparisons_and_shift_assignments_are_distinguished() {
        for token in ["==", "===", "!=", "!==", "<=", ">="] {
            assert_eq!(refuse(&format!("var a=1;{}true;", format!("if(a{token}1){{;}}").repeat(5000))), None);
        }
        for token in ["=", ">>=", ">>>=", "<<=", "**=", "&&=", "||=", "??="] {
            assert!(refuse(&format!("var a;{}1", format!("a{token}").repeat(5000))).is_some(), "{token}");
        }
    }
    /// Newline boundaries admit flat libraries but never erase a continued expression.
    #[test]
    fn newline_continuations_keep_their_chain_budget() {
        for part in ["a=>\n", "a=\n", "a=function(){}.\nx=", "a=1\n**", "a=1\nin x=", "a=1\ninstanceof x="] {
            assert!(refuse(&format!("{}1", part.repeat(6000))).is_some(), "{part}");
        }
        for newline in ["\n", "\r\n", "/*\n*/", "// comment\n"] {
            let flat = (0..300).map(|i| format!("const f{i}=()=>1{newline}")).collect::<String>();
            assert_eq!(refuse(&flat), None, "{newline:?}");
        }
    }
    /// Inner statement separators must not reset a surrounding expression's active chain.
    #[test]
    fn inner_separators_do_not_erase_enclosing_chains() {
        assert!(refuse(&format!("{}1", "a=function(){;}.x=".repeat(3000))).is_some());
        assert!(refuse(&format!("{}({}1)", "a=>".repeat(200), "b=>".repeat(100))).is_some());
        assert_eq!(refuse(&format!("var s='{}'; /* {} */ true;", "=>=".repeat(5000), "a=".repeat(5000))), None);
    }
}

#[cfg(test)]
mod literal_chain_tests {
    use super::refuse;
    /// Literal semicolons cannot erase a surrounding assignment chain; interpolations remain code.
    #[test]
    fn literal_separators_cannot_bypass_chain_guards() {
        for part in ["a=`;`.x=", "a=/;/.x=", "a=/[;,]/.x="] {
            assert!(refuse(&format!("{}1", part.repeat(6000))).is_some(), "{part}");
        }
        assert!(refuse(&format!("`${{{}1}}`", "a=".repeat(6000))).is_some());
        assert_eq!(refuse(&format!("var x=`{}${{1+1}}`; true;", "=>;=".repeat(6000))), None);
    }
}

#[cfg(test)]
mod review4_lexical_tests {
    use super::refuse;
    /// Function/control blocks may separate statements; array/object/expression contexts may not.
    #[test]
    fn block_statements_and_control_regex_literals_are_not_expression_chains() {
        for head in ["function f(){", "const f=()=>{", "if(true){"] {
            assert_eq!(refuse(&format!("{head}{} }}", "x=1\n".repeat(6000))), None);
        }
        for head in ["if(true)", "while(false)", "for(;false;)", "with({})", "if(false);else", "do"] {
            assert_eq!(refuse(&format!("{head} /{}/.test('x');", "a=".repeat(6000))), None, "{head}");
        }
        for fragment in ["a=({x:1}).x=\n", "a=()=>{x=1\n}.x=\n", "a=(x\n/2).x=\n"] {
            assert!(refuse(&format!("{}1", fragment.repeat(6000))).is_some(), "{fragment}");
        }
    }
}
