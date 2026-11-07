use super::globals::{event_object, install};
use super::host::{Host, Shared, error, host, text};
use super::model::{DocInfo, Event, FieldState, Limits, Outcome};
use super::{Arc, Attribute, BTreeSet, Context, HostHooks, JsNativeError, JsObject, JsResult, JsString, Realm, RefCell, Source, js_string};
use boa_engine::property::PropertyDescriptor;

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

#[derive(Clone, Copy)]
struct RunConfig {
    limits: Limits,
    bootstrap_effects: bool,
}

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
    run_shared_inner(script, event, doc, fields, doc_scripts, limits, true)
}

/// Run a reset's field event with document globals but without applying bootstrap field writes
/// or host requests. The actual Calculate/Format script runs with effects enabled.
pub fn run_shared_for_reset(
    script: &str,
    event: &Event,
    doc: &DocInfo,
    fields: Arc<Vec<FieldState>>,
    doc_scripts: &[String],
    limits: Limits,
) -> Outcome {
    run_shared_inner(script, event, doc, fields, doc_scripts, limits, false)
}

fn run_shared_inner(
    script: &str,
    event: &Event,
    doc: &DocInfo,
    fields: Arc<Vec<FieldState>>,
    doc_scripts: &[String],
    limits: Limits,
    bootstrap_effects: bool,
) -> Outcome {
    let config = RunConfig { limits, bootstrap_effects };
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
        // The script's thread writes dates as this one asked (`with_english_printd`).
        let english = crate::format::printd_english();
        std::thread::scope(|scope| {
            let spawned = std::thread::Builder::new()
                .name("pdfcraft-js".into())
                .stack_size(SCRIPT_STACK)
                .spawn_scoped(scope, || crate::format::with_english_printd(english, || run_here(script, event, doc, fields, kept, skipped, config)));
            match spawned {
                Ok(thread) => thread.join().unwrap_or_else(|_| failed("the script stopped with an internal error".into())),
                Err(e) => failed(format!("the script engine could not start: {e}")),
            }
        })
    }
    #[cfg(target_arch = "wasm32")]
    run_here(script, event, doc, fields, kept, skipped, config)
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

/// `doc_scripts` are the document scripts to run with their places among all of them;
/// `skipped` the ones refused before running, with the reason.
fn run_here(
    script: &str,
    event: &Event,
    doc: &DocInfo,
    fields: Arc<Vec<FieldState>>,
    doc_scripts: &[(usize, &str)],
    skipped: &[(usize, String)],
    config: RunConfig,
) -> Outcome {
    let mut ctx = match Context::builder().host_hooks(std::rc::Rc::new(NoCodeFromStrings)).build() {
        Ok(ctx) => ctx,
        Err(e) => {
            let error = Some(format!("the script engine could not start: {e}"));
            return Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), error, ..Default::default() };
        }
    };
    ctx.runtime_limits_mut().set_loop_iteration_limit(config.limits.loop_iterations);
    ctx.runtime_limits_mut().set_recursion_limit(config.limits.recursion);
    ctx.insert_data(RefCell::new(Host {
        fields,
        effects_enabled: config.bootstrap_effects,
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
        let ev = event_object(&mut ctx, event);
        ctx.register_global_property(js_string!("event"), ev.clone(), Attribute::all())?;
        // One document script that throws (calling a method PdfCraft lacks, say) must not keep
        // every field script in the form from running: what it defined before the error, and
        // the other scripts, stay available (#3940).
        for &(i, d) in doc_scripts {
            if let Err(e) = ctx.eval(Source::from_bytes(d)) {
                doc_errors.push((i, doc_error(i, &e)));
            }
        }
        if !config.bootstrap_effects
            && let Err(e) = restore_reset_event(&mut ctx, &ev, event)
        {
            // A non-configurable descriptor or frozen event can make restoration impossible.
            // The engine must abort this reset, rather than silently leave Calculate undone.
            out.event_restore_failed = true;
            return Err(error(format!("the reset event could not be restored after document scripts: {e}")));
        }
        // A fresh context is used for each field event. Restore effects before the actual
        // script, even when a document initializer threw after defining useful globals.
        host(&ctx)?.try_borrow_mut().map_err(|_| error("the script host is already in use"))?.effects_enabled = true;
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

/// The bootstrap and the actual field script share a realm so document helpers stay usable.
/// Restore core fields on the original object: closures holding `event` then see the same
/// values as the direct global. Extra properties installed by a document script remain intact.
fn restore_reset_event(ctx: &mut Context, ev: &JsObject, event: &Event) -> JsResult<()> {
    let fresh = event_object(ctx, event);
    for key in fresh.own_property_keys(ctx)? {
        let value = fresh.get(key.clone(), ctx)?;
        ev.define_property_or_throw(key, PropertyDescriptor::builder().value(value).writable(true).enumerable(true).configurable(true), ctx)?;
    }
    let global = ctx.global_object();
    let full = PropertyDescriptor::builder().value(ev.clone()).writable(true).enumerable(true).configurable(true);
    if global.define_property_or_throw(js_string!("event"), full, ctx).is_err() {
        // A frozen global may keep a non-writable `event` that already refers to `ev`.
        // Defining the same value without changing its descriptor is legal; a different
        // non-configurable value or accessor still fails, and the caller aborts the edit.
        global.define_property_or_throw(js_string!("event"), PropertyDescriptor::builder().value(ev.clone()), ctx)?;
    }
    Ok(())
}
