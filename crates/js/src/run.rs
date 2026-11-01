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

/// Longest string converted to the host or built by `repeat` / `padStart` / `padEnd` / `join`, in
/// UTF-16 code units.
pub(crate) const MAX_STRING_UNITS: usize = 4 << 20;
/// Most string bytes copied into a host collection (`strings()`, `alerts`, `console`, `requests`).
pub const MAX_HOST_BYTES: usize = 1 << 20;
/// Most alerts one script run may record.
pub const MAX_ALERTS: usize = xfa::MAX_ALERTS;
/// Most console lines one script run may record.
pub const MAX_CONSOLE: usize = xfa::MAX_CONSOLE;
/// Most viewer requests one script run may record.
pub const MAX_REQUESTS: usize = 1_000;
/// Largest `ArrayBuffer` or `SharedArrayBuffer` a script may allocate, in bytes.
const MAX_BUFFER_BYTES: u64 = 16 << 20;

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
            let spawned = std::thread::Builder::new()
                .name("pdfcraft-js".into())
                .stack_size(SCRIPT_STACK)
                .spawn_scoped(scope, || run_here(script, event, doc, fields, kept, skipped, limits));
            match spawned {
                Ok(thread) => thread.join().unwrap_or_else(|_| failed("the script stopped with an internal error".into())),
                Err(e) => failed(format!("the script engine could not start: {e}")),
            }
        })
    }
    #[cfg(target_arch = "wasm32")]
    run_here(script, event, doc, fields, kept, skipped, limits)
}

/// Host hooks that refuse to compile code from strings (`eval`, `Function` and its generator
/// and async kin). Such code is built at run time, so it never passed [`refuse`], and boa's
/// recursive parser would overflow the stack on a deeply nested one.
struct NoCodeFromStrings;

impl HostHooks for NoCodeFromStrings {
    fn ensure_can_compile_strings(&self, _: Realm, _: &[JsString], _: &JsString, _: bool, _: &mut Context) -> JsResult<()> {
        Err(JsNativeError::typ().with_message("scripts can't compile code from strings (eval, Function)").into())
    }

    fn max_buffer_size(&self, _context: &mut Context) -> u64 {
        MAX_BUFFER_BYTES
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
        let mut total_bytes = 0usize;
        let mut params = Vec::with_capacity(parameters.len());
        for p in parameters {
            let s = p.to_std_string_lossy();
            total_bytes = total_bytes.saturating_add(s.len());
            if total_bytes > MAX_SCRIPT_BYTES {
                return Err(JsNativeError::range().with_message("the code is too long to compile").into());
            }
            if let Some(why) = refuse(&s) {
                return Err(JsNativeError::range().with_message(why).into());
            }
            params.push(s);
        }
        let body_str = body.to_std_string_lossy();
        total_bytes = total_bytes.saturating_add(body_str.len());
        if total_bytes > MAX_SCRIPT_BYTES {
            return Err(JsNativeError::range().with_message("the code is too long to compile").into());
        }
        if let Some(why) = refuse(&body_str) {
            return Err(JsNativeError::range().with_message(why).into());
        }
        // As one text: brackets left open in one parameter carry into the next.
        let mut source = params.join(",");
        source.push('\n');
        source.push_str(&body_str);
        match refuse(&source) {
            Some(why) => Err(JsNativeError::range().with_message(why).into()),
            None => Ok(()),
        }
    }

    fn max_buffer_size(&self, _context: &mut Context) -> u64 {
        MAX_BUFFER_BYTES
    }
}

fn string_repeat(this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    if this.is_null_or_undefined() {
        return Err(error("String.prototype.repeat called on null or undefined"));
    }
    let s = this.to_string(ctx)?;
    let n = arg(args, 0).to_number(ctx)?;
    if n < 0.0 || n.is_infinite() {
        return Err(JsNativeError::range().with_message("repeat count must be non-negative and finite").into());
    }
    let count = if n.is_nan() { 0usize } else { n.trunc() as usize };
    if count == 0 || s.is_empty() {
        return Ok(js_string!().into());
    }
    let total = s.len().checked_mul(count).ok_or_else(|| JsNativeError::range().with_message(format!("string length exceeds {MAX_STRING_UNITS}")))?;
    if total > MAX_STRING_UNITS {
        return Err(JsNativeError::range().with_message(format!("string length exceeds {MAX_STRING_UNITS}")).into());
    }
    let unit_vec: Vec<u16> = s.iter().collect();
    let mut out = Vec::with_capacity(total);
    for _ in 0..count {
        out.extend_from_slice(&unit_vec);
    }
    Ok(JsString::from(&out[..]).into())
}

fn string_pad(this: &JsValue, args: &[JsValue], start: bool, ctx: &mut Context) -> JsResult<JsValue> {
    if this.is_null_or_undefined() {
        return Err(error("String.prototype.pad called on null or undefined"));
    }
    let s = this.to_string(ctx)?;
    let max_len = arg(args, 0).to_length(ctx)? as usize;
    if max_len <= s.len() {
        return Ok(s.into());
    }
    let filler = match args.get(1) {
        Some(v) if !v.is_undefined() => v.to_string(ctx)?,
        _ => js_string!(" "),
    };
    if filler.is_empty() {
        return Ok(s.into());
    }
    if max_len > MAX_STRING_UNITS {
        return Err(JsNativeError::range().with_message(format!("string length exceeds {MAX_STRING_UNITS}")).into());
    }
    let fill_len = max_len - s.len();
    let filler_units: Vec<u16> = filler.iter().collect();
    let mut out = Vec::with_capacity(max_len);
    if !start {
        out.extend(s.iter());
    }
    let mut remaining = fill_len;
    while remaining > 0 {
        let take = remaining.min(filler_units.len());
        out.extend_from_slice(&filler_units[..take]);
        remaining -= take;
    }
    if start {
        out.extend(s.iter());
    }
    Ok(JsString::from(&out[..]).into())
}

fn string_pad_start(this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    string_pad(this, args, true, ctx)
}

fn string_pad_end(this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    string_pad(this, args, false, ctx)
}

fn array_join(this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    if this.is_null_or_undefined() {
        return Err(error("Array.prototype.join called on null or undefined"));
    }
    let o = this.to_object(ctx)?;
    let len = o.get(js_string!("length"), ctx)?.to_length(ctx)?;
    if len > MAX_ARRAY_LENGTH {
        return Err(JsNativeError::range().with_message(format!("array length exceeds {MAX_ARRAY_LENGTH}")).into());
    }
    let sep = match args.first() {
        Some(v) if !v.is_undefined() => v.to_string(ctx)?,
        _ => js_string!(","),
    };
    if len == 0 {
        return Ok(js_string!().into());
    }
    let total_sep = (len as usize).saturating_sub(1).saturating_mul(sep.len());
    if total_sep > MAX_STRING_UNITS {
        return Err(JsNativeError::range().with_message(format!("string length exceeds {MAX_STRING_UNITS}")).into());
    }
    let mut units = Vec::new();
    for k in 0..len {
        if k > 0 && !sep.is_empty() {
            units.extend(sep.iter());
        }
        let el = o.get(k, ctx)?;
        if !el.is_null_or_undefined() && &el != this {
            let s = el.to_string(ctx)?;
            if units.len().saturating_add(s.len()) > MAX_STRING_UNITS {
                return Err(JsNativeError::range().with_message(format!("string length exceeds {MAX_STRING_UNITS}")).into());
            }
            units.extend(s.iter());
        }
    }
    Ok(JsString::from(&units[..]).into())
}

fn array_to_locale_string(this: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    array_join(this, &[], ctx)
}

pub(crate) fn install_intrinsic_limits(ctx: &mut Context) -> JsResult<()> {
    let str_proto = ctx.intrinsics().constructors().string().prototype();
    for (name, len, f) in [
        ("repeat", 1, NativeFunction::from_fn_ptr(string_repeat)),
        ("padStart", 1, NativeFunction::from_fn_ptr(string_pad_start)),
        ("padEnd", 1, NativeFunction::from_fn_ptr(string_pad_end)),
    ] {
        let func = FunctionObjectBuilder::new(ctx.realm(), f).name(JsString::from(name)).length(len).build();
        str_proto.set(JsString::from(name), func, false, ctx)?;
    }
    let arr_proto = ctx.intrinsics().constructors().array().prototype();
    for (name, len, f) in
        [("join", 1, NativeFunction::from_fn_ptr(array_join)), ("toLocaleString", 0, NativeFunction::from_fn_ptr(array_to_locale_string))]
    {
        let func = FunctionObjectBuilder::new(ctx.realm(), f).name(JsString::from(name)).length(len).build();
        arr_proto.set(JsString::from(name), func, false, ctx)?;
    }
    Ok(())
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
    // A failed install would leave repeat, pad and join unbounded. Refuse the script instead.
    if let Err(e) = install_intrinsic_limits(&mut ctx) {
        let error = Some(format!("the script engine could not start: {e}"));
        return Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), error, ..Default::default() };
    }
    ctx.insert_data(RefCell::new(Host {
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
