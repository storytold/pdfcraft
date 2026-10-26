//! A document's script session: one JavaScript context for as long as the document is open, as
//! Acrobat keeps one per document (#269). The document-level scripts run once, when the session
//! starts (one added or changed later runs before the next script), and what any script defines
//! (variables, functions) stays for the next. The timers scripts set (`app.setInterval`,
//! `app.setTimeOut`) are kept and reported in every [`Outcome`]; the caller fires one with
//! [`ScriptSession::fire`] when it falls due. The caller owns the clock: nothing here waits or
//! runs on its own.
//!
//! Every run gets the form as the caller holds it then. Field objects read it live, so an
//! object a script kept from an earlier run sees the current value.
//!
//! The context lives on a thread of its own (with the script stack), as an XFA session's
//! does. A script that runs past [`RUN_LIMIT`] is abandoned on the thread and the session is
//! dead from then on, its timers with it. A script that fails from a fault in the engine (a
//! panic) loses the context: the next run starts a new one, without the old timers. On wasm
//! there are no threads: the context lives here.

use std::sync::Arc;
use std::time::Duration;

use boa_engine::property::Attribute;
use boa_engine::{Context, JsValue, Source, js_string};

use crate::{DocInfo, Event, FieldState, Host, Limits, Outcome, TimerCode, Timers};

/// How long one script or timer may run, from when the engine is ready for it.
pub const RUN_LIMIT: Duration = Duration::from_secs(5);

/// What one run is for.
enum Task {
    Script {
        source: String,
        event: Event,
    },
    /// A timer that fell due, by its id.
    Timer(u32),
}

struct Job {
    task: Task,
    doc: DocInfo,
    fields: Arc<Vec<FieldState>>,
    /// The document-level scripts as they are now, for a context that starts with this job.
    doc_scripts: Arc<Vec<String>>,
    /// Document-level scripts added or changed since the context started (with their places
    /// among all of them), to run first in a context that is already running.
    define: Vec<(usize, String)>,
}

/// The event timers run with: `event.type` "App", `event.name` "Timer".
fn timer_event() -> Event {
    Event { name: "Timer".into(), kind: "App".into(), target: None, value: String::new(), change: String::new(), will_commit: false }
}

/// Run each document-level script in `ctx`, on its own (#3940): the errors of the ones that
/// failed, as [`Outcome::doc_errors`] words them.
fn define(ctx: &mut Context, scripts: impl Iterator<Item = (usize, impl AsRef<str>)>) -> Vec<String> {
    let mut errors = Vec::new();
    for (i, d) in scripts {
        let d = d.as_ref();
        let failed = match crate::refuse(d) {
            Some(why) => Some(why),
            None => ctx.eval(Source::from_bytes(d)).err().map(|e| e.to_string()),
        };
        errors.extend(failed.map(|why| crate::doc_error(i, &why)));
    }
    errors
}

/// The context, on the session's thread.
struct Engine {
    ctx: Context,
}

impl Engine {
    /// A context for the document `doc`, with the document-level scripts run in it, and the
    /// errors of those that failed (the others ran). What they printed, changed or asked for
    /// is reported with the first run.
    fn start(doc_scripts: &[String], limits: Limits, doc: &DocInfo, fields: Arc<Vec<FieldState>>) -> Result<(Engine, Vec<String>), String> {
        let mut ctx = Context::builder()
            .host_hooks(std::rc::Rc::new(crate::NoCodeFromStrings))
            .build()
            .map_err(|e| format!("the script engine could not start: {e}"))?;
        ctx.runtime_limits_mut().set_loop_iteration_limit(limits.loop_iterations);
        ctx.runtime_limits_mut().set_recursion_limit(limits.recursion);
        ctx.insert_data(std::cell::RefCell::new(Host::new(fields, doc.clone(), Some(Timers::default()))));
        crate::install(&mut ctx).map_err(|e| format!("the script engine could not start: {e}"))?;
        let ev = crate::event_object(&mut ctx, &Event::doc("Init"));
        ctx.register_global_property(js_string!("event"), ev, Attribute::all()).map_err(|e| format!("the script engine could not start: {e}"))?;
        let errors = define(&mut ctx, doc_scripts.iter().enumerate());
        Ok((Engine { ctx }, errors))
    }

    /// Run `job`, first the document-level scripts it brings unless `fresh` (the context has
    /// just started with all of them).
    fn run(&mut self, job: Job, fresh: bool) -> Outcome {
        let ctx = &mut self.ctx;
        let event = match &job.task {
            Task::Script { event, .. } => event.clone(),
            Task::Timer(_) => timer_event(),
        };
        let mut out = Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), ..Default::default() };
        let Some(shared) = ctx.get_data::<crate::Shared>() else {
            out.error = Some("the script host is not set up".into());
            return out;
        };
        let pages = job.doc.num_pages;
        let code = {
            let mut h = shared.borrow_mut();
            h.fields = job.fields;
            h.doc = job.doc;
            match job.task {
                Task::Script { source, .. } => Some(TimerCode::Source(source)),
                // A timeout runs once: it is gone before it runs, so it can't clear itself.
                Task::Timer(id) => h.timers.as_mut().and_then(|t| match t.table.get(&id).map(|e| e.repeat) {
                    Some(true) => t.table.get(&id).map(|e| match &e.code {
                        TimerCode::Source(s) => TimerCode::Source(s.clone()),
                        TimerCode::Function(f) => TimerCode::Function(f.clone()),
                    }),
                    Some(false) => t.table.remove(&id).map(|e| e.code),
                    None => None,
                }),
            }
        };
        let result = (|| {
            // The pages may have changed since the last run (pages inserted or deleted).
            ctx.register_global_property(js_string!("numPages"), JsValue::from(pages as f64), Attribute::CONFIGURABLE)?;
            let ev = crate::event_object(ctx, &event);
            // A document script's `var event` makes the property fixed: then it is only set.
            if ctx.register_global_property(js_string!("event"), ev.clone(), Attribute::all()).is_err() {
                ctx.global_object().set(js_string!("event"), ev.clone(), false, ctx)?;
            }
            if !fresh {
                out.doc_errors = define(ctx, job.define.into_iter());
            }
            let r = match code {
                // A timer cleared after it fell due, before it ran: nothing to do.
                None => JsValue::undefined(),
                Some(TimerCode::Source(source)) => match crate::refuse(&source) {
                    Some(why) => return Err(crate::error(why)),
                    None => ctx.eval(Source::from_bytes(&source))?,
                },
                Some(TimerCode::Function(f)) => {
                    let this = JsValue::from(ctx.global_object());
                    f.call(&this, &[], ctx)?
                }
            };
            crate::finish(r, &ev, &mut out, ctx)
        })();
        if let Err(e) = result {
            out.error = Some(e.to_string());
        }
        if let Some(h) = ctx.get_data::<crate::Shared>() {
            crate::take_effects(&mut h.borrow_mut(), &mut out);
        }
        out
    }
}

impl Drop for Engine {
    /// The timers hold the context's objects (their handles and functions): they go first.
    fn drop(&mut self) {
        if let Some(h) = self.ctx.get_data::<crate::Shared>()
            && let Ok(mut h) = h.try_borrow_mut()
        {
            h.timers = None;
            h.icons.clear();
        }
    }
}

/// Run `job` in `engine`, starting it first when there is none.
fn serve(engine: &mut Option<Engine>, limits: Limits, job: Job) -> Outcome {
    let mut started = None;
    if engine.is_none() {
        match Engine::start(&job.doc_scripts, limits, &job.doc, Arc::clone(&job.fields)) {
            Ok((e, errors)) => {
                *engine = Some(e);
                started = Some(errors);
            }
            Err(why) => return Outcome { rc: true, error: Some(why), ..Default::default() },
        }
    }
    let Some(e) = engine.as_mut() else { return Outcome { rc: true, error: Some("the script engine is not running".into()), ..Default::default() } };
    let fresh = started.is_some();
    let mut o = e.run(job, fresh);
    // The document scripts' errors are told once, with the first run.
    if let Some(errors) = started {
        o.doc_errors = errors;
    }
    o
}

/// What the caller keeps between runs, on every platform: the document-level scripts and those
/// not yet run in the context.
struct Scripts {
    all: Arc<Vec<String>>,
    pending: Vec<(usize, String)>,
}

impl Scripts {
    fn set(&mut self, scripts: Vec<String>) {
        if *self.all == scripts {
            return;
        }
        for (i, s) in scripts.iter().enumerate() {
            if !self.all.contains(s) && !self.pending.iter().any(|(_, p)| p == s) {
                self.pending.push((i, s.clone()));
            }
        }
        self.all = Arc::new(scripts);
    }

    fn job(&mut self, task: Task, doc: &DocInfo, fields: Arc<Vec<FieldState>>) -> Job {
        Job { task, doc: doc.clone(), fields, doc_scripts: Arc::clone(&self.all), define: std::mem::take(&mut self.pending) }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
    use std::time::Instant;

    use super::{Engine, Job, RUN_LIMIT, Scripts, Task, serve};
    use crate::xfa::ENGINE_START_LIMIT;
    use crate::{DocInfo, Event, FieldState, Limits, Outcome};

    enum Reply {
        /// The thread is up.
        Ready,
        /// The engine is ready for the job just sent: its time starts now.
        Started,
        Outcome(Box<Outcome>),
    }

    /// The engine's thread, started for the first run and kept for the session.
    struct Worker {
        to: Sender<Box<Job>>,
        from: Receiver<Reply>,
    }

    /// A document's scripts, run in one context on one thread.
    pub struct ScriptSession {
        scripts: Scripts,
        limits: Limits,
        worker: Option<Worker>,
        /// A script ran past its limit: its thread is left to it and nothing more runs.
        dead: Option<String>,
    }

    impl ScriptSession {
        /// A session for a document with these document-level scripts. Nothing starts until
        /// the first run.
        pub fn new(doc_scripts: Vec<String>, limits: Limits) -> ScriptSession {
            ScriptSession { scripts: Scripts { all: Arc::new(doc_scripts), pending: Vec::new() }, limits, worker: None, dead: None }
        }

        /// The document-level scripts are now `scripts` (one was added, changed or deleted):
        /// those the context hasn't run run before the next script, and their errors come with
        /// its outcome. A deleted one's definitions stay, as in a context that ran it.
        pub fn set_doc_scripts(&mut self, scripts: Vec<String>) {
            self.scripts.set(scripts);
        }

        /// Why nothing more runs, once a script was abandoned.
        pub fn dead(&self) -> Option<&str> {
            self.dead.as_deref()
        }

        /// Run `script` for `event`, with the form's `fields` as they are now.
        pub fn run(&mut self, script: &str, event: &Event, doc: &DocInfo, fields: Arc<Vec<FieldState>>) -> Outcome {
            let job = self.scripts.job(Task::Script { source: script.to_string(), event: event.clone() }, doc, fields);
            self.send(job, event)
        }

        /// Run timer `id`, which fell due. A timeout is gone afterwards; an interval stays.
        pub fn fire(&mut self, id: u32, doc: &DocInfo, fields: Arc<Vec<FieldState>>) -> Outcome {
            let job = self.scripts.job(Task::Timer(id), doc, fields);
            self.send(job, &super::timer_event())
        }

        /// The thread, started now when there is none (the first run, or after a fault in the
        /// engine). `Err`: it could not start.
        fn worker(&mut self) -> Result<&mut Worker, String> {
            if self.worker.is_none() {
                let (to, inbox) = channel::<Box<Job>>();
                let (reply, from) = channel::<Reply>();
                let limits = self.limits;
                let spawned = std::thread::Builder::new().name("pdfcraft-js-session".into()).stack_size(crate::SCRIPT_STACK).spawn(move || {
                    crate::xfa::warm_up();
                    // The caller is gone when a send fails: nothing to report then.
                    let _ = reply.send(Reply::Ready);
                    let mut engine: Option<Engine> = None;
                    while let Ok(job) = inbox.recv() {
                        // The context's start (and the document scripts) count against the
                        // engine's start-up limit, not the script's.
                        if engine.is_none() {
                            let (doc, fields) = (job.doc.clone(), Arc::clone(&job.fields));
                            match Engine::start(&job.doc_scripts, limits, &doc, fields) {
                                Ok((e, errors)) => {
                                    engine = Some(e);
                                    let _ = reply.send(Reply::Started);
                                    let mut o = serve(&mut engine, limits, Job { define: Vec::new(), ..*job });
                                    o.doc_errors = errors;
                                    let _ = reply.send(Reply::Outcome(Box::new(o)));
                                    continue;
                                }
                                Err(why) => {
                                    let _ = reply.send(Reply::Started);
                                    let _ = reply.send(Reply::Outcome(Box::new(Outcome { rc: true, error: Some(why), ..Default::default() })));
                                    continue;
                                }
                            }
                        }
                        let _ = reply.send(Reply::Started);
                        let o = serve(&mut engine, limits, *job);
                        let _ = reply.send(Reply::Outcome(Box::new(o)));
                    }
                });
                spawned.map_err(|e| format!("the script engine could not start: {e}"))?;
                match from.recv_timeout(ENGINE_START_LIMIT) {
                    Ok(Reply::Ready) => {}
                    Ok(_) => return Err("the script engine answered out of turn".into()),
                    Err(RecvTimeoutError::Timeout) => {
                        return Err(format!("the script engine took longer than {:.0} s to start", ENGINE_START_LIMIT.as_secs_f64()));
                    }
                    Err(RecvTimeoutError::Disconnected) => return Err("the script engine stopped with an internal error".into()),
                }
                self.worker = Some(Worker { to, from });
            }
            self.worker.as_mut().ok_or_else(|| "the script engine is not running".into())
        }

        /// Send `job` and wait for its outcome: at most [`ENGINE_START_LIMIT`] for the engine
        /// to be ready (the first run starts it), then [`RUN_LIMIT`] for the job itself.
        fn send(&mut self, job: Job, event: &Event) -> Outcome {
            let failed =
                |why: String| Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), error: Some(why), ..Default::default() };
            if let Some(why) = &self.dead {
                return failed(why.clone());
            }
            let worker = match self.worker() {
                Ok(w) => w,
                Err(why) => return failed(why),
            };
            if worker.to.send(Box::new(job)).is_err() {
                // The thread is gone (a fault in the engine): the next run gets a new one.
                self.worker = None;
                return failed("the script engine stopped with an internal error".into());
            }
            let (mut running, mut limit) = (false, ENGINE_START_LIMIT);
            let mut deadline = Instant::now().checked_add(limit);
            loop {
                let left = deadline.map_or(limit, |d| d.saturating_duration_since(Instant::now()));
                match worker.from.recv_timeout(left) {
                    Ok(Reply::Outcome(o)) => return *o,
                    Ok(Reply::Started) => {
                        if !running {
                            (running, limit) = (true, RUN_LIMIT);
                            deadline = Instant::now().checked_add(limit);
                        }
                    }
                    Ok(Reply::Ready) => continue,
                    Err(RecvTimeoutError::Timeout) => {
                        let why = if running {
                            format!(
                                "a script ran longer than {:.0} s and was abandoned; the document's scripts are off until it is opened again",
                                RUN_LIMIT.as_secs_f64()
                            )
                        } else {
                            format!("the script engine took longer than {:.0} s to start", ENGINE_START_LIMIT.as_secs_f64())
                        };
                        self.dead = Some(why.clone());
                        // Dropping the sender ends the thread's loop once the script is done.
                        self.worker = None;
                        return failed(why);
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        self.worker = None;
                        return failed("the script stopped with an internal error".into());
                    }
                }
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod native {
    use std::sync::Arc;

    use super::{Engine, Scripts, Task, serve};
    use crate::{DocInfo, Event, FieldState, Limits, Outcome};

    /// A document's scripts, run here in one context (no threads on wasm: a script's own
    /// limits bound it).
    pub struct ScriptSession {
        scripts: Scripts,
        limits: Limits,
        engine: Option<Engine>,
    }

    impl ScriptSession {
        pub fn new(doc_scripts: Vec<String>, limits: Limits) -> ScriptSession {
            ScriptSession { scripts: Scripts { all: Arc::new(doc_scripts), pending: Vec::new() }, limits, engine: None }
        }

        pub fn set_doc_scripts(&mut self, scripts: Vec<String>) {
            self.scripts.set(scripts);
        }

        pub fn dead(&self) -> Option<&str> {
            None
        }

        pub fn run(&mut self, script: &str, event: &Event, doc: &DocInfo, fields: Arc<Vec<FieldState>>) -> Outcome {
            let job = self.scripts.job(Task::Script { source: script.to_string(), event: event.clone() }, doc, fields);
            serve(&mut self.engine, self.limits, job)
        }

        pub fn fire(&mut self, id: u32, doc: &DocInfo, fields: Arc<Vec<FieldState>>) -> Outcome {
            let job = self.scripts.job(Task::Timer(id), doc, fields);
            serve(&mut self.engine, self.limits, job)
        }
    }
}

pub use native::ScriptSession;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::ScriptSession;
    use crate::{DISPLAY_HIDDEN, DISPLAY_VISIBLE, DocInfo, Event, FieldState, FieldType, Limits, MAX_TIMERS, MIN_TIMER_MS, Outcome, Timer};

    /// Three frames as push buttons, the first one showing, as an animation keeps them.
    fn frames() -> Arc<Vec<FieldState>> {
        let frame = |i: usize| {
            let mut f = FieldState::new(format!("a.{i}"), FieldType::Button, Vec::new());
            f.display = if i == 0 { DISPLAY_VISIBLE } else { DISPLAY_HIDDEN };
            f
        };
        Arc::new((0..3).map(frame).collect())
    }

    fn doc() -> DocInfo {
        DocInfo { file_name: "frames.pdf".into(), num_pages: 1, page: 0, info: Vec::new(), xfa: false, layers: Vec::new() }
    }

    fn console() -> Event {
        Event::doc("Console")
    }

    fn run(s: &mut ScriptSession, script: &str) -> Outcome {
        s.run(script, &console(), &doc(), frames())
    }

    #[test]
    fn variables_and_functions_stay_for_the_next_script() {
        let mut s = ScriptSession::new(Vec::new(), Limits::default());
        assert_eq!(run(&mut s, "var n = 1; function twice(x) { return 2 * x; }").error, None);
        let o = run(&mut s, "n = n + 1; twice(n)");
        assert_eq!((o.error, o.result), (None, Some("4".into())));
    }

    #[test]
    fn document_scripts_run_once_and_report_their_errors_once() {
        let scripts = vec!["var opened = (typeof opened == 'undefined') ? 1 : opened + 1;".to_string(), "nope();".to_string()];
        let mut s = ScriptSession::new(scripts, Limits::default());
        let first = run(&mut s, "opened");
        assert_eq!(first.result.as_deref(), Some("1"));
        assert_eq!(first.doc_errors.len(), 1, "{:?}", first.doc_errors);
        assert!(first.doc_errors[0].starts_with("document script 2:"), "{:?}", first.doc_errors);
        let second = run(&mut s, "opened");
        assert_eq!(second.result.as_deref(), Some("1"));
        assert!(second.doc_errors.is_empty());
    }

    #[test]
    fn an_interval_is_reported_fires_and_is_cleared() {
        let mut s = ScriptSession::new(Vec::new(), Limits::default());
        let o = run(
            &mut s,
            "var on = 0; function next() { getField('a.' + on).display = display.hidden; on = (on + 1) % 3; \
             getField('a.' + on).display = display.visible; } var t = app.setInterval('next()', 250);",
        );
        assert_eq!(o.error, None);
        assert_eq!(o.timers, [Timer { id: 1, period_ms: 250, repeat: true }]);
        let o = s.fire(1, &doc(), frames());
        assert_eq!(o.error, None);
        let shown: Vec<(String, i32)> = o.changed.iter().map(|f| (f.name.clone(), f.display)).collect();
        assert_eq!(shown, [("a.0".to_string(), DISPLAY_HIDDEN), ("a.1".to_string(), DISPLAY_VISIBLE)]);
        assert_eq!(o.timers.len(), 1, "an interval stays");
        // The state moved on: the next tick shows the third frame.
        let o = s.fire(1, &doc(), frames());
        assert!(o.changed.iter().any(|f| f.name == "a.2" && f.display == DISPLAY_VISIBLE), "{:?}", o.changed);
        let o = run(&mut s, "app.clearInterval(t);");
        assert!(o.timers.is_empty());
        // A timer cleared after the caller saw it fall due does nothing.
        let o = s.fire(1, &doc(), frames());
        assert_eq!((o.error, o.changed.len()), (None, 0));
    }

    #[test]
    fn a_timeout_runs_once_and_a_function_may_stand_for_the_expression() {
        let mut s = ScriptSession::new(Vec::new(), Limits::default());
        let o = run(&mut s, "var hits = 0; app.setTimeOut(function () { hits++; }, 0);");
        assert_eq!(o.timers, [Timer { id: 1, period_ms: MIN_TIMER_MS, repeat: false }], "periods below the floor are raised to it");
        assert!(s.fire(1, &doc(), frames()).timers.is_empty());
        s.fire(1, &doc(), frames());
        assert_eq!(run(&mut s, "hits").result.as_deref(), Some("1"));
    }

    #[test]
    fn timers_are_bounded() {
        let mut s = ScriptSession::new(Vec::new(), Limits::default());
        let o = run(&mut s, &format!("for (var i = 0; i < {}; i++) app.setInterval('0', 1e12);", MAX_TIMERS + 1));
        assert!(o.error.as_deref().is_some_and(|e| e.contains("at most")), "{:?}", o.error);
        assert_eq!(o.timers.len(), MAX_TIMERS);
        assert!(o.timers.iter().all(|t| t.period_ms == u32::MAX), "periods past u32 are capped");
        // A timer's script passes the same checks as any other.
        let mut s = ScriptSession::new(Vec::new(), Limits::default());
        let o = run(&mut s, &format!("app.clearInterval(app.setInterval('{}', 20)); app.setTimeOut('{}', 20);", "x", "(".repeat(100)));
        assert_eq!(o.error, None);
        let id = o.timers.last().map_or(0, |t| t.id);
        let o = s.fire(id, &doc(), frames());
        assert!(o.error.as_deref().is_some_and(|e| e.contains("nested too deeply")), "{:?}", o.error);
    }

    #[test]
    fn a_single_run_keeps_no_timers() {
        let o = crate::run("app.setInterval('0', 100); app.clearInterval({});", &console(), &doc(), &frames(), &[], Limits::default());
        assert_eq!((o.error, o.timers.len()), (None, 0));
    }

    #[test]
    fn field_objects_kept_from_an_earlier_run_read_the_form_as_it_is_now() {
        let mut s = ScriptSession::new(Vec::new(), Limits::default());
        run(&mut s, "var f = getField('a.1');");
        let mut now = (*frames()).clone();
        now[1].display = DISPLAY_VISIBLE;
        let o = s.run("f.display", &console(), &doc(), Arc::new(now));
        assert_eq!(o.result.as_deref(), Some("0"));
    }

    #[test]
    fn document_scripts_added_later_run_before_the_next_script() {
        let mut s = ScriptSession::new(vec!["var a = 1;".to_string()], Limits::default());
        assert_eq!(run(&mut s, "a").result.as_deref(), Some("1"));
        s.set_doc_scripts(vec!["var a = 1;".to_string(), "var b = a + 1;".to_string(), "nope();".to_string()]);
        let o = run(&mut s, "a + b");
        assert_eq!(o.result.as_deref(), Some("3"));
        assert_eq!(o.doc_errors.len(), 1, "{:?}", o.doc_errors);
        assert!(o.doc_errors[0].starts_with("document script 3:"), "{:?}", o.doc_errors);
        // Already run: not again.
        s.set_doc_scripts(vec!["var a = 1;".to_string(), "var b = a + 1;".to_string(), "nope();".to_string()]);
        assert!(run(&mut s, "b = 5;").doc_errors.is_empty());
        assert_eq!(run(&mut s, "b").result.as_deref(), Some("5"));
    }

    #[test]
    fn a_button_shows_another_buttons_icon() {
        let mut s = ScriptSession::new(Vec::new(), Limits::default());
        let o =
            run(&mut s, "var icons = [getField('a.1').buttonGetIcon(), getField('a.2').buttonGetIcon()]; getField('a.0').buttonSetIcon(icons[1]);");
        assert_eq!(o.error, None);
        let icons: Vec<(String, Option<String>)> = o.changed.iter().map(|f| (f.name.clone(), f.icon.clone())).collect();
        assert_eq!(icons, [("a.0".to_string(), Some("a.2".to_string()))]);
        // Its own icon again is no icon of another's.
        let o = run(&mut s, "getField('a.0').buttonSetIcon(getField('a.0').buttonGetIcon());");
        assert!(o.changed.iter().all(|f| f.icon.is_none()), "{:?}", o.changed);
        let o = run(&mut s, "getField('a.0').buttonSetIcon(null);");
        assert!(o.error.as_deref().is_some_and(|e| e.starts_with("TypeError")), "{:?}", o.error);
    }

    #[test]
    fn layers_are_ocg_objects_a_script_turns_on_and_off() {
        let mut s = ScriptSession::new(Vec::new(), Limits::default());
        let layer = |name: &str, on: bool| crate::Layer { name: name.into(), on, initial: on };
        let d = DocInfo { layers: vec![layer("0.0", true), layer("0.1", false)], ..doc() };
        let o = s.run(
            "var o = this.getOCGs(this.pageNum); o[0].state = false; o[1].state = !o[1].initState; o.length + ' ' + o[1].name",
            &console(),
            &d,
            frames(),
        );
        assert_eq!((o.error, o.result.as_deref()), (None, Some("2 0.1")));
        assert_eq!(o.layers, [(0, false), (1, true)]);
        assert_eq!(run(&mut s, "getOCGs() === null").result.as_deref(), Some("true"), "no layers: null");
    }
}
