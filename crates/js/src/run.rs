use super::globals::{event_object, install};
use super::host::{Host, Shared, host, text};
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

/// The script thread's stack.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const SCRIPT_STACK: usize = 1 << 30;

/// Why `source` is refused, if it is: too long, or nested deeper than any form needs. Strings
/// and comments are skipped; brackets in regular expressions and templates count, which only
/// errs on the safe side.
pub(crate) fn refuse(source: &str) -> Option<String> {
    if source.len() > MAX_SCRIPT_BYTES {
        return Some(format!("the script is too long ({} KiB; the limit is {} KiB)", source.len() / 1024, MAX_SCRIPT_BYTES / 1024));
    }
    let too_deep = || Some("the script is nested too deeply to run safely".to_string());
    let b = source.as_bytes();
    let (mut depth, mut prefix, mut conditionals) = (0usize, 0usize, 0usize);
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        i += 1;
        match c {
            b'\'' | b'"' => {
                // To the closing quote (or the end of the line, where an unclosed string ends).
                while let Some(&d) = b.get(i) {
                    i += 1;
                    if d == b'\\' {
                        i += 1;
                    } else if d == c || d == b'\n' {
                        break;
                    }
                }
                prefix = 0;
            }
            b'/' if b.get(i) == Some(&b'/') => {
                while b.get(i).is_some_and(|d| *d != b'\n') {
                    i += 1;
                }
            }
            b'/' if b.get(i) == Some(&b'*') => {
                i += 1;
                while i < b.len() && !b[i..].starts_with(b"*/") {
                    i += 1;
                }
                i += 2;
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                if depth > MAX_BRACKET_DEPTH {
                    return too_deep();
                }
                prefix = 0;
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                prefix = 0;
            }
            b'!' | b'~' | b'+' | b'-' => {
                prefix += 1;
                if prefix > MAX_PREFIX_RUN {
                    return too_deep();
                }
            }
            b'?' => {
                conditionals += 1;
                if conditionals > MAX_CONDITIONALS {
                    return too_deep();
                }
                prefix = 0;
            }
            c if c.is_ascii_whitespace() => {}
            _ => prefix = 0,
        }
    }
    None
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
    let failed = |why: String| Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), error: Some(why), ..Default::default() };
    if let Some(why) = initialization_limit(doc_scripts).or_else(|| refuse(script)) {
        return failed(why);
    }
    // Link initialization is fail-closed; ordinary form scripts retain upstream's
    // skip-and-report behavior below, within the shared aggregate initialization limit.
    if event.kind == "Link"
        && let Some(why) = doc_scripts.iter().find_map(|d| refuse(d))
    {
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
        // The script's thread writes dates as this one asked (`with_english_printd`).
        let english = crate::format::printd_english();
        std::thread::scope(|scope| {
            let spawned = std::thread::Builder::new()
                .name("pdfcraft-js".into())
                .stack_size(SCRIPT_STACK)
                .spawn_scoped(scope, || crate::format::with_english_printd(english, || run_here(script, event, doc, fields, kept, skipped, limits)));
            match spawned {
                Ok(thread) => thread.join().unwrap_or_else(|_| failed("the script stopped with an internal error".into())),
                Err(e) => failed(format!("the script engine could not start: {e}")),
            }
        })
    }
    #[cfg(target_arch = "wasm32")]
    run_here(script, event, doc, fields, kept, skipped, limits)
}

pub(super) fn initialization_limit(scripts: &[String]) -> Option<String> {
    if scripts.len() > 1024 || scripts.iter().try_fold(0usize, |total, s| total.checked_add(s.len())).is_none_or(|total| total > 1 << 20) {
        Some("document JavaScript initialization exceeds its limit".into())
    } else {
        None
    }
}

/// Host hooks that refuse to compile code from strings (`eval`, `Function` and its generator
/// and async kin). Such code is built at run time, so it never passed [`refuse`], and boa's
/// recursive parser would overflow the stack on a deeply nested one.
struct NoCodeFromStrings;

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

/// Evaluate in a fresh Boa context with string-code generation disabled and COW fields.
/// Link actions initialize document scripts under a synthetic Doc/Open event, then restore
/// the real event before the action; other callers rebuild definitions under a separate copy
/// of their actual event, with field writes and viewer requests disabled at the host boundary
/// (except an explicitly requested Doc/Open). Diagnostics and definitions remain available;
/// the requested script and the functions it calls then run with effects enabled.
/// A Link initialization error stops evaluation. Other hosts record document-script errors
/// and continue with later initializers and the action, preserving upstream form behavior.
/// Collected field changes, diagnostics and requests can still accompany an error; this
/// function only returns an Outcome, leaving eligibility and effect application to the caller.
/// Callers enforce the aggregate initialization limit and preflight refused Link initializers.
/// `doc_scripts` keeps original indices; `skipped` records ordinary initializers refused
/// before evaluation. Limits are cooperative, not hard wall-time or heap quotas.
pub(super) fn run_here(
    script: &str,
    event: &Event,
    doc: &DocInfo,
    fields: Arc<Vec<FieldState>>,
    doc_scripts: &[(usize, &str)],
    skipped: &[(usize, String)],
    limits: Limits,
) -> Outcome {
    let mut ctx = match Context::builder().host_hooks(std::rc::Rc::new(NoCodeFromStrings)).build() {
        Ok(ctx) => ctx,
        Err(e) => {
            let error = Some(format!("the script engine could not start: {e}"));
            return Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), error, ..Default::default() };
        }
    };
    ctx.runtime_limits_mut().set_loop_iteration_limit(limits.loop_iterations);
    ctx.runtime_limits_mut().set_recursion_limit(limits.recursion);
    ctx.insert_data(RefCell::new(Host {
        effects_enabled: true,
        fields,
        changed: BTreeSet::new(),
        alerts: Vec::new(),
        console: Vec::new(),
        requests: Vec::new(),
        doc: doc.clone(),
        kept: 0,
        dropped: 0,
    }));
    let mut out = Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), ..Default::default() };
    let mut doc_errors: Vec<(usize, String)> = skipped.iter().map(|(i, why)| (*i, doc_error(*i, why))).collect();
    let result = (|| -> JsResult<()> {
        install(&mut ctx)?;
        // Link actions have their own document initialization contract. Existing field,
        // button and console callers initialize under their actual event, not a synthetic Open.
        let link = event.kind == "Link";
        let ev = event_object(&mut ctx, event, link);
        // The Link target is the document/global object. Keeping its event binding
        // non-enumerable avoids a global -> event -> target cycle during serialization.
        let event_attributes = if link { Attribute::WRITABLE | Attribute::CONFIGURABLE } else { Attribute::all() };
        ctx.register_global_property(js_string!("event"), ev.clone(), event_attributes)?;
        // Explicit Link initialization belongs to its transaction, as does an explicitly
        // requested Doc/Open. Other events only rebuild definitions: their authoritative
        // field/page snapshot must not be replaced by an initializer's defaults. Gate at
        // the host boundary, rather than restoring fields after JS has read wrong values.
        let initialize_effects = link || (event.kind == "Doc" && event.name == "Open");
        host(&ctx)?.borrow_mut().effects_enabled = initialize_effects;
        let initialization_event = if link { Event::doc("Open") } else { event.clone() };
        // Keep bootstrap event writes (value/rc/change) out of the actual event too.
        let bootstrap = if initialize_effects && !link { ev.clone() } else { event_object(&mut ctx, &initialization_event, link) };
        ctx.register_global_property(js_string!("event"), bootstrap, event_attributes)?;
        // Unrequested field initialization is tolerant upstream (#3940). A user-activated
        // Link retains its all-or-error initialization contract before its own action runs.
        for &(i, d) in doc_scripts {
            if link {
                ctx.eval(Source::from_bytes(d))?;
            } else if let Err(e) = ctx.eval(Source::from_bytes(d)) {
                doc_errors.push((i, doc_error(i, &e)));
            }
        }
        host(&ctx)?.borrow_mut().effects_enabled = true;
        ctx.register_global_property(js_string!("event"), ev.clone(), event_attributes)?;
        let r = ctx.eval(Source::from_bytes(script))?;
        if !r.is_undefined() {
            out.result = Some(text(&r, &mut ctx)?);
        }
        let rc = ev.get(js_string!("rc"), &mut ctx)?;
        out.rc = rc.to_boolean();
        let v = ev.get(js_string!("value"), &mut ctx)?;
        out.value = text(&v, &mut ctx)?;
        let c = ev.get(js_string!("change"), &mut ctx)?;
        out.change = text(&c, &mut ctx)?;
        Ok(())
    })();
    if let Err(e) = result {
        out.error = Some(e.to_string());
    }
    doc_errors.sort_by_key(|(i, _)| *i);
    out.doc_errors = doc_errors.into_iter().map(|(_, e)| e).collect();
    if let Some(h) = ctx.remove_data::<Shared>() {
        let h = h.into_inner();
        out.changed = h.changed.iter().map(|i| h.fields[*i].clone()).collect();
        out.alerts = h.alerts;
        out.console = h.console;
        out.requests = h.requests;
        if h.dropped > 0 {
            out.console.push(format!("the script's messages, console lines and requests passed their limits; {} more were left out", h.dropped));
        }
    }
    out
}
