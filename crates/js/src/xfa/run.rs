use super::globals::install;
use super::host::{Shared, XHost};
use super::model::{XfaDoc, XfaEvent, XfaNode, XfaOutcome, clip_result};
use super::*;

/// Run `script` for `event` on the form `root` (the root subform), from node `event.target`.
pub fn run_xfa(script: &str, event: &XfaEvent, doc: &XfaDoc, root: &XfaNode, limits: Limits) -> XfaOutcome {
    let failed = |why: String| XfaOutcome { error: Some(why), ..Default::default() };
    if let Some(why) = refuse(script) {
        return failed(why);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::scope(|scope| {
            let spawned = std::thread::Builder::new()
                .name("pdfcraft-xfa-js".into())
                .stack_size(crate::SCRIPT_STACK)
                .spawn_scoped(scope, || run_here(script, event, doc, root.clone(), limits, &|| {}));
            match spawned {
                Ok(thread) => thread.join().unwrap_or_else(|_| failed("the script stopped with an internal error".into())),
                Err(e) => failed(format!("the script engine could not start: {e}")),
            }
        })
    }
    #[cfg(target_arch = "wasm32")]
    run_here(script, event, doc, root.clone(), limits, &|| {})
}

/// [`run_xfa`] with a time limit: a script still running `timeout` after the engine is ready
/// (loops inside nested function calls can run for hours within the engine's per-frame loop
/// limit) is left on its thread, which ends when the engine's own limits stop it, and an
/// outcome with `abandoned` set comes back at once. In the browser build there are no
/// threads: the script runs to its limits.
pub fn run_xfa_within(script: &str, event: &XfaEvent, doc: &XfaDoc, root: XfaNode, limits: Limits, timeout: std::time::Duration) -> XfaOutcome {
    if let Some(why) = refuse(script) {
        return XfaOutcome { error: Some(why), ..Default::default() };
    }
    let (script, event, doc) = (script.to_string(), event.clone(), doc.clone());
    run_within("pdfcraft-xfa-js", timeout, move |started| run_here(&script, &event, &doc, root, limits, started))
}

/// Longest the script engine may take to be ready for a script (its thread and context, and
/// for the first script in a process the engine's own set-up) before the script is abandoned.
/// It doesn't count against the script's time: on a busy machine the start alone can take
/// longer than the tightest limit, and abandoning a script turns the form's scripts off.
pub(crate) const ENGINE_START_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Run `script` on its own thread (with the script stack) and wait at most `timeout` for its
/// outcome, counted from when it calls `started` (once the engine is ready); past that the
/// thread is abandoned and the outcome says so. On wasm there are no threads: the script runs
/// here.
pub(crate) fn run_within(name: &str, timeout: std::time::Duration, script: impl FnOnce(&dyn Fn()) -> XfaOutcome + Send + 'static) -> XfaOutcome {
    run_within_limits(name, ENGINE_START_LIMIT, timeout, script)
}

/// [`run_within`] with the engine's start-up limit given.
pub(crate) fn run_within_limits(
    name: &str,
    start_limit: std::time::Duration,
    timeout: std::time::Duration,
    script: impl FnOnce(&dyn Fn()) -> XfaOutcome + Send + 'static,
) -> XfaOutcome {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::mpsc::RecvTimeoutError;
        use std::time::Instant;
        enum Step {
            Started,
            Done(XfaOutcome),
        }
        let failed = |why: String| XfaOutcome { error: Some(why), ..Default::default() };
        let abandoned = |why: String| XfaOutcome { error: Some(why), abandoned: true, ..Default::default() };
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name(name.into()).stack_size(crate::SCRIPT_STACK).spawn(move || {
            // The receiver is gone when the caller stopped waiting: nothing to report then.
            let started = tx.clone();
            let outcome = script(&move || {
                let _ = started.send(Step::Started);
            });
            let _ = tx.send(Step::Done(outcome));
        });
        if let Err(e) = spawned {
            return failed(format!("the script engine could not start: {e}"));
        }
        // Wait for the engine within its start-up limit, then for the script within its own.
        let (mut running, mut limit) = (false, start_limit);
        let mut deadline = Instant::now().checked_add(limit);
        loop {
            let left = deadline.map_or(limit, |d| d.saturating_duration_since(Instant::now()));
            match rx.recv_timeout(left) {
                Ok(Step::Done(o)) => return o,
                Ok(Step::Started) => {
                    // A second call changes nothing: the script's time has already begun.
                    if !running {
                        (running, limit) = (true, timeout);
                        deadline = Instant::now().checked_add(limit);
                    }
                }
                Err(RecvTimeoutError::Timeout) if running => {
                    return abandoned(format!("the script ran longer than {:.1} s and was abandoned", timeout.as_secs_f64()));
                }
                Err(RecvTimeoutError::Timeout) => {
                    return abandoned(format!("the script engine took longer than {:.0} s to start", start_limit.as_secs_f64()));
                }
                Err(RecvTimeoutError::Disconnected) => return failed("the script stopped with an internal error".into()),
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (name, start_limit, timeout);
        script(&|| {})
    }
}

/// The first script in a process sets the engine up (its built-ins, parser and compiler come
/// into use): a trivial script takes that on, so a form's script is never timed with it.
pub(crate) fn warm_up() {
    static WARM: std::sync::Once = std::sync::Once::new();
    // `call_once_force`: a poisoned `Once` (a panic while warming up) only means warming up again.
    WARM.call_once_force(|_| {
        let mut ctx = Context::default();
        let _ = ctx.eval(Source::from_bytes(b"(function () { return eval('0'); }).call({})"));
    });
}

/// `started` is called once the engine is ready: a time limit on the script runs from then.
pub(crate) fn run_here(script: &str, event: &XfaEvent, doc: &XfaDoc, root: XfaNode, limits: Limits, started: &dyn Fn()) -> XfaOutcome {
    warm_up();
    let mut ctx = match Context::builder().host_hooks(std::rc::Rc::new(crate::CheckedCodeFromStrings)).build() {
        Ok(ctx) => ctx,
        Err(e) => return XfaOutcome { error: Some(format!("the script engine could not start: {e}")), ..Default::default() },
    };
    ctx.runtime_limits_mut().set_loop_iteration_limit(limits.loop_iterations);
    ctx.runtime_limits_mut().set_recursion_limit(limits.recursion);
    let host_state = XHost::new(root, &event.target, doc);
    let current = host_state.current;
    ctx.insert_data(RefCell::new(host_state));
    let mut out = XfaOutcome::default();
    let result = (|| -> JsResult<()> {
        install(&mut ctx, event, doc, current)?;
        // Ready for the script: its time starts now, the set-up above not charged to it.
        started();
        ctx.register_global_property(js_string!("__xfa_src"), s(script), Attribute::CONFIGURABLE)?;
        // Direct eval inside a function called on the node: `this` is the node and the
        // completion value is the script's last expression (what a calculate script yields).
        let r = ctx.eval(Source::from_bytes(b"(function () { return eval(__xfa_src); }).call(__xfa_this)"))?;
        if !r.is_undefined() {
            // Best effort: a value that can't be made text (a symbol, say) is no failure.
            out.result = Some(text(&r, &mut ctx).unwrap_or_default());
            if r.is_boolean() {
                out.result_bool = Some(r.to_boolean());
            } else if r.is_null() {
                out.result = Some(String::new());
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        out.error = Some(e.to_string());
    }
    if let Some(h) = ctx.remove_data::<Shared>() {
        let h = h.into_inner();
        out.notes = h.notes();
        out.effects = h.effects;
        out.console = h.console;
    }
    clip_result(&mut out);
    out
}
