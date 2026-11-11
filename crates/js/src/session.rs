//! One sandboxed realm per open document. No context crosses a document boundary.
#[cfg(not(target_arch = "wasm32"))]
use crate::interaction::InteractiveEvent;
use crate::{DocInfo, Event, FieldState, Limits, Outcome};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;

/// One event snapshot; the worker owns its context and refreshes this data between invocations.
#[derive(Clone)]
pub struct Job {
    pub script: String,
    pub event: Event,
    pub doc: DocInfo,
    pub fields: Arc<Vec<FieldState>>,
    pub doc_scripts: Vec<String>,
    /// Target positions in this job's field snapshot and their ordered script sources.
    pub calculations: Vec<(usize, String)>,
}
/// Live field instances are independent of restorable PDF object-number allocation.
#[derive(Default)]
struct FieldInstances {
    current: std::collections::HashMap<u64, u64>,
    last_ids: Vec<u64>,
    next: u64,
}
impl FieldInstances {
    /// Retire absent instances immediately, including history changes between two script jobs.
    fn sync(&mut self, ids: impl IntoIterator<Item = u64>) {
        let ids: Vec<_> = ids.into_iter().collect();
        if ids == self.last_ids {
            return;
        }
        let present: std::collections::BTreeSet<_> = ids.iter().copied().collect();
        self.last_ids = ids;
        self.current.retain(|id, _| present.contains(id));
        for id in present {
            if !self.current.contains_key(&id) {
                self.next = self.next.saturating_add(1);
                self.current.insert(id, self.next);
            }
        }
    }
    /// Stamp the immutable job snapshot with current instance tokens before worker dispatch.
    fn stamp(&mut self, job: &mut Job) {
        self.stamp_fields(&mut job.fields);
    }
    /// Stamp a snapshot in place, copying it only when a token actually changes: a caller that
    /// keeps its snapshot between callbacks (the engine does) then shares it with the worker.
    fn stamp_fields(&mut self, fields: &mut Arc<Vec<FieldState>>) {
        self.sync(fields.iter().filter_map(|f| f.object_id));
        let token =
            |current: &std::collections::HashMap<u64, u64>, field: &FieldState| field.object_id.and_then(|id| current.get(&id).copied()).unwrap_or(0);
        if fields.iter().all(|field| field.instance_id == token(&self.current, field)) {
            return;
        }
        for field in Arc::make_mut(fields) {
            field.instance_id = token(&self.current, field);
        }
    }
}
impl ScriptSession {
    /// Stamp the caller's retained snapshot with the current instance tokens before it builds a
    /// job, so the worker's own stamping finds nothing to change and does not copy it.
    pub fn stamp_fields(&mut self, fields: &mut Arc<Vec<FieldState>>) {
        self.instances.stamp_fields(fields);
    }
    /// Notify the retained runtime of live PDF fields after an edit or history step.
    pub fn sync_field_ids(&mut self, ids: impl IntoIterator<Item = u64>) {
        self.instances.sync(ids);
    }

    /// Invalidate icon handles after restoring an earlier PDF object graph.
    pub fn retire_icon_handles(&mut self) {
        self.icon_epoch = self.icon_epoch.wrapping_add(1);
        self.icon_versions = Default::default();
        self.icon_tracking_overflowed = false;
    }

    /// Invalidate only the recorded references that changed in a history step.
    pub fn retire_changed_icons(&mut self, mut changed: impl FnMut(u64) -> bool) {
        if self.icon_tracking_overflowed {
            self.retire_icon_handles();
            return;
        }
        for (id, version) in Arc::make_mut(&mut self.icon_versions) {
            if changed(*id) {
                *version = version.wrapping_add(1);
            }
        }
    }

    /// Attach icon versions to the snapshot and record when history must invalidate all handles because tracking filled up.
    fn stamp_icons(&mut self, job: &mut Job) {
        const MAX_TRACKED_ICONS: usize = 16_384;
        for id in job.fields.iter().filter_map(|field| field.icon) {
            if self.icon_versions.contains_key(&id) {
                continue;
            }
            if self.icon_versions.len() == MAX_TRACKED_ICONS {
                self.icon_tracking_overflowed = true;
                break;
            }
            Arc::make_mut(&mut self.icon_versions).insert(id, 0);
        }
        job.doc.icon_epoch = self.icon_epoch;
        job.doc.icon_versions = self.icon_versions.clone();
    }
}
/// Boa context and initialization sources retained by exactly one open-document runtime.
pub(crate) struct Realm {
    context: Option<crate::Context>,
    scripts: Vec<String>,
}
impl Realm {
    /// Create an empty isolated realm; the first job initializes its document scripts.
    fn new() -> Self {
        Self { context: None, scripts: Vec::new() }
    }
    /// Validate sources, initialize once when needed, and execute this job in the retained document realm.
    fn run(&mut self, job: Job) -> Outcome {
        let out = self.run_job(job);
        // The realm keeps its globals and field objects, not the job's field snapshot: the engine
        // keeps that snapshot and patches it in place for the next callback.
        if let Some(ctx) = self.context.as_ref() {
            crate::run::release_fields(ctx);
        }
        out
    }
    fn run_job(&mut self, job: Job) -> Outcome {
        let failed = |why: String| Outcome { rc: true, execution_refused: true, realm_untouched: true, error: Some(why), ..Default::default() };
        // A script too long or too deeply nested to run is that script's error, as on `main`: the
        // value stays and the edit's other scripts still run. The realm is not touched.
        if let Some(why) = crate::refuse(&job.script) {
            return Outcome {
                rc: true,
                realm_untouched: true,
                value: job.event.value.clone(),
                change: job.event.change.clone(),
                error: Some(why),
                ..Default::default()
            };
        }
        if job.doc_scripts.len() > 1024 || job.doc_scripts.iter().try_fold(0usize, |n, s| n.checked_add(s.len())).is_none_or(|n| n > 1 << 20) {
            return failed("document scripts exceed their budget".into());
        }
        let initialize = self.context.is_none() || self.scripts != job.doc_scripts;
        if initialize {
            self.context = match crate::Context::builder().host_hooks(std::rc::Rc::new(crate::run::NoCodeFromStrings)).build() {
                Ok(c) => Some(c),
                Err(e) => return failed(e.to_string()),
            };
            self.scripts = job.doc_scripts.clone();
        }
        let mut skipped = Vec::new();
        let scripts: Vec<_> = job
            .doc_scripts
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                if !initialize {
                    return None;
                }
                match crate::refuse(s) {
                    Some(e) => {
                        skipped.push((i, e));
                        None
                    }
                    None => Some((i, s.as_str())),
                }
            })
            .collect();
        let Some(ctx) = self.context.as_mut() else { return failed("script realm unavailable".into()) };
        if initialize {
            let mut initial = crate::run::run_in_context(
                ctx,
                "",
                &Event::doc("Open"),
                &job.doc,
                job.fields.clone(),
                crate::run::ScriptSetup {
                    doc_scripts: &scripts,
                    skipped: &skipped,
                    calculations: &job.calculations,
                    initialize: true,
                    reset_calls: 0,
                    calculation_evaluations: 0,
                    calculation_limit_exceeded: false,
                },
                Limits::default(),
            );
            if initial.execution_refused {
                return initial;
            }
            let (fields, reset_calls, calculation_evaluations) = crate::host::host(ctx)
                .map(|h| {
                    let h = h.borrow();
                    (h.fields.clone(), h.reset_calls, h.calculation_evaluations)
                })
                .unwrap_or((job.fields, 0, 0));
            let mut out = crate::run::run_in_context(
                ctx,
                &job.script,
                &job.event,
                &job.doc,
                fields,
                crate::run::ScriptSetup {
                    doc_scripts: &[],
                    skipped: &[],
                    calculations: &job.calculations,
                    initialize: false,
                    reset_calls,
                    calculation_evaluations,
                    calculation_limit_exceeded: initial.calculation_limit_exceeded,
                },
                Limits::default(),
            );
            out.reset_limit_exceeded |= initial.reset_limit_exceeded;
            out.calculation_limit_exceeded |= initial.calculation_limit_exceeded;
            for field in initial.changed {
                if !out.changed.iter().any(|f| match (f.object_id, field.object_id) {
                    (Some(a), Some(b)) => a == b,
                    _ => f.name == field.name,
                }) {
                    out.changed.push(field);
                }
            }
            initial.alerts.append(&mut out.alerts);
            out.alerts = initial.alerts;
            initial.console.append(&mut out.console);
            out.console = initial.console;
            initial.requests.append(&mut out.requests);
            out.requests = initial.requests;
            out.doc_errors = initial.doc_errors;
            if out.error.is_none() {
                out.error = initial.error;
            }
            out
        } else {
            crate::run::run_in_context(
                ctx,
                &job.script,
                &job.event,
                &job.doc,
                job.fields,
                crate::run::ScriptSetup {
                    doc_scripts: &[],
                    skipped: &[],
                    calculations: &job.calculations,
                    initialize: false,
                    reset_calls: 0,
                    calculation_evaluations: 0,
                    calculation_limit_exceeded: false,
                },
                Limits::default(),
            )
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
static LIVE_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(not(target_arch = "wasm32"))]
const MAX_DOCUMENT_WORKERS: usize = if usize::BITS == 32 { 4 } else { 16 };
/// A reservation is released only when its worker exits, including detached timed-out workers.
#[cfg(not(target_arch = "wasm32"))]
struct WorkerPermit;
#[cfg(not(target_arch = "wasm32"))]
impl WorkerPermit {
    /// Reserve bounded address space without evicting another document's retained state.
    fn acquire() -> Result<Self, String> {
        use std::sync::atomic::Ordering::SeqCst;
        let mut live = LIVE_WORKERS.load(SeqCst);
        loop {
            if live >= MAX_DOCUMENT_WORKERS {
                return Err("document script worker limit reached; close a scripted document and retry".into());
            }
            match LIVE_WORKERS.compare_exchange(live, live.saturating_add(1), SeqCst, SeqCst) {
                Ok(_) => return Ok(Self),
                Err(current) => live = current,
            }
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
impl Drop for WorkerPermit {
    fn drop(&mut self) {
        LIVE_WORKERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(not(target_arch = "wasm32"))]
type HostTask = Box<dyn FnOnce(&mut crate::Context) -> Outcome + Send>;

#[cfg(not(target_arch = "wasm32"))]
enum WorkerRequest {
    Script { job: Box<Job>, english_retry: bool },
    Host(HostTask),
}

#[cfg(not(target_arch = "wasm32"))]
struct WorkerJob {
    request: WorkerRequest,
    reset_realm: bool,
    reply: mpsc::Sender<InteractiveEvent>,
}

#[cfg(not(target_arch = "wasm32"))]
/// Native document runtime with one bounded-stack worker and at most one active invocation.
/// Dropping it releases the sender; after any active job finishes the worker exits and drops its realm.
#[derive(Default)]
pub struct ScriptSession {
    sender: Option<mpsc::SyncSender<WorkerJob>>,
    executing: Arc<std::sync::atomic::AtomicBool>,
    reset_realm: bool,
    busy: Arc<std::sync::atomic::AtomicBool>,
    worker_lifetime: std::sync::Weak<()>,
    instances: FieldInstances,
    icon_epoch: u64,
    icon_versions: Arc<std::collections::BTreeMap<u64, u64>>,
    icon_tracking_overflowed: bool,
}
#[cfg(not(target_arch = "wasm32"))]
impl ScriptSession {
    /// Start a bounded-stack native worker lazily and reject a second active job, including during prompts.
    pub fn start(&mut self, job: Job) -> Result<mpsc::Receiver<InteractiveEvent>, String> {
        self.start_job(WorkerRequest::Script { job: Box::new(job), english_retry: false })
    }

    /// Reserve the document worker and enqueue a job. An English-date retry uses a fresh context and leaves the retained realm alone.
    fn start_job(&mut self, mut request: WorkerRequest) -> Result<mpsc::Receiver<InteractiveEvent>, String> {
        use std::sync::atomic::Ordering::SeqCst;
        if self.is_executing() {
            return Err("another script is active in this document".into());
        }
        if self.sender.is_none() {
            if self.worker_lifetime.upgrade().is_some() {
                return Err("the timed-out script is still stopping; retry once it exits".into());
            }
            let (tx, rx) = mpsc::sync_channel::<WorkerJob>(1);
            let permit = WorkerPermit::acquire()?;
            let lifetime = Arc::new(());
            self.worker_lifetime = Arc::downgrade(&lifetime);
            let executing = self.executing.clone();
            std::thread::Builder::new()
                .name("pdfcraft-document-js".into())
                .stack_size(crate::run::DOCUMENT_WORKER_STACK)
                .spawn(move || {
                    let _lifetime = lifetime;
                    let _permit = permit;
                    let mut realm = Realm::new();
                    while let Ok(WorkerJob { request, reset_realm, reply }) = rx.recv() {
                        if reset_realm {
                            realm = Realm::new();
                        }
                        let interactive = matches!(&request, WorkerRequest::Script { english_retry: false, .. });
                        crate::interaction::set_host(interactive.then(|| reply.clone()));
                        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match request {
                            WorkerRequest::Script { job, english_retry: true } => crate::with_english_printd(true, || {
                                crate::run::run_shared_here(
                                    &job.script,
                                    &job.event,
                                    &job.doc,
                                    job.fields,
                                    &job.doc_scripts,
                                    &job.calculations,
                                    Limits::default(),
                                )
                            }),
                            WorkerRequest::Script { job, english_retry: false } => realm.run(*job),
                            WorkerRequest::Host(task) => match realm.context.as_mut() {
                                Some(context) => task(context),
                                None => Outcome {
                                    execution_refused: true,
                                    realm_untouched: true,
                                    error: Some("the document script realm has not been initialized".into()),
                                    ..Default::default()
                                },
                            },
                        }));
                        crate::interaction::set_host(None);
                        let out = out.unwrap_or_else(|_| {
                            realm = Realm::new();
                            Outcome {
                                rc: true,
                                execution_refused: true,
                                error: Some("script engine stopped unexpectedly".into()),
                                ..Default::default()
                            }
                        });
                        executing.store(false, SeqCst);
                        let _ = reply.send(InteractiveEvent::Complete(out));
                    }
                })
                .map_err(|e| e.to_string())?;
            self.sender = Some(tx);
        }
        if self.busy.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_err() {
            return Err("another script is active in this document".into());
        }
        if let WorkerRequest::Script { job, english_retry: false } = &mut request {
            self.instances.stamp(job);
            self.stamp_icons(job);
        }
        let (tx, rx) = mpsc::channel();
        let Some(sender) = self.sender.as_ref() else {
            self.busy.store(false, std::sync::atomic::Ordering::SeqCst);
            return Err("script worker unavailable".into());
        };
        self.executing.store(true, SeqCst);
        sender.try_send(WorkerJob { request, reset_realm: self.reset_realm, reply: tx }).map_err(|_| {
            self.executing.store(false, SeqCst);
            self.busy.store(false, SeqCst);
            "script worker disconnected"
        })?;
        self.reset_realm = false;
        Ok(rx)
    }
    /// Whether the native worker still owns an invocation, including one waiting for a prompt.
    pub fn is_executing(&self) -> bool {
        self.executing.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// An asynchronous completion remains pending until its document outcome is accepted or discarded.
    pub fn is_pending(&self) -> bool {
        self.busy.load(std::sync::atomic::Ordering::SeqCst)
    }
    /// Acknowledge a received completion before applying its edits on the same document realm.
    pub fn acknowledge_completion(&mut self) {
        self.busy.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    /// Detach the old worker and retain its lifetime so it still occupies this document's slot.
    pub fn retire_timed_out_worker(&mut self) {
        self.sender = None;
        self.acknowledge_completion();
    }

    /// Fork the realm while keeping this document's worker reservation and execution gate.
    /// Old invocation handles cannot submit work through the successor's channel.
    pub fn fresh_realm(&mut self) -> Self {
        Self {
            sender: self.sender.take(),
            executing: self.executing.clone(),
            worker_lifetime: self.worker_lifetime.clone(),
            reset_realm: true,
            ..Self::default()
        }
    }

    /// Drop state from callbacks belonging to a rejected PDF edit.
    pub fn reset_after_refusal(&mut self) {
        *self = self.fresh_realm();
    }

    /// Run a job with one total deadline, discarding its entire realm when the host times out.
    pub fn run(&mut self, job: Job, answers: &[Option<String>]) -> Outcome {
        self.run_with_limit(job, answers, std::time::Duration::from_secs(10))
    }
    /// Run a callback within the remaining time of a larger operation.
    pub fn run_with_limit(&mut self, job: Job, answers: &[Option<String>], limit: std::time::Duration) -> Outcome {
        self.run_timed(WorkerRequest::Script { job: Box::new(job), english_retry: false }, answers, limit)
    }

    /// Retry formatting in a fresh context on the document worker, within the caller's remaining time.
    pub fn run_english_retry(&mut self, job: Job, limit: std::time::Duration) -> Outcome {
        self.run_timed(WorkerRequest::Script { job: Box::new(job), english_retry: true }, &[], limit)
    }

    /// Dispatch trusted host work to the document's already initialized realm.
    /// The callback stays on the document worker and must return cooperatively on its own.
    pub fn run_host_task(&mut self, limit: std::time::Duration, task: impl FnOnce(&mut crate::Context) -> Outcome + Send + 'static) -> Outcome {
        self.run_timed(WorkerRequest::Host(Box::new(task)), &[], limit)
    }

    /// Wait within one deadline for startup, prompt replies and completion. Detach a worker that exceeds the deadline.
    fn run_timed(&mut self, request: WorkerRequest, answers: &[Option<String>], limit: std::time::Duration) -> Outcome {
        if limit.is_zero() {
            return Outcome {
                execution_refused: true,
                realm_untouched: true,
                error: Some("script worker had no time remaining".into()),
                ..Default::default()
            };
        }
        let started = std::time::Instant::now();
        let rx = match self.start_job(request) {
            Ok(rx) => rx,
            Err(e) => return Outcome { execution_refused: true, realm_untouched: true, error: Some(e), ..Default::default() },
        };
        match Self::receive(&rx, answers, started, limit) {
            Some(out) => {
                self.acknowledge_completion();
                out
            }
            None => {
                // The old worker has no route into this replacement realm. Its reservation
                // remains charged until cooperative execution finishes and the thread exits.
                self.retire_timed_out_worker();
                Outcome { execution_refused: true, error: Some("script worker did not complete within its limit".into()), ..Default::default() }
            }
        }
    }
    /// Prompt replies consume the original deadline instead of restarting it.
    fn receive(
        rx: &mpsc::Receiver<InteractiveEvent>,
        answers: &[Option<String>],
        started: std::time::Instant,
        limit: std::time::Duration,
    ) -> Option<Outcome> {
        let mut answers = answers.iter();
        loop {
            let remaining = limit.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return None;
            }
            match rx.recv_timeout(remaining) {
                Ok(InteractiveEvent::Complete(out)) => return Some(out),
                Ok(InteractiveEvent::Prompt(_, reply)) => {
                    let _ = reply.send(answers.next().cloned().flatten());
                }
                Err(_) => return None,
            }
        }
    }
}
/// Web-local retained realm; native worker/dialog hosting is deliberately unavailable.
#[cfg(target_arch = "wasm32")]
pub struct ScriptSession {
    realm: Realm,
    instances: FieldInstances,
    icon_epoch: u64,
    icon_versions: Arc<std::collections::BTreeMap<u64, u64>>,
    icon_tracking_overflowed: bool,
}
#[cfg(target_arch = "wasm32")]
impl Default for ScriptSession {
    /// Create the web runtime without native worker threads or a native prompt host.
    fn default() -> Self {
        Self {
            realm: Realm::new(),
            instances: FieldInstances::default(),
            icon_epoch: 0,
            icon_versions: Default::default(),
            icon_tracking_overflowed: false,
        }
    }
}
#[cfg(target_arch = "wasm32")]
impl ScriptSession {
    /// Replace the web-local realm and its document bindings.
    pub fn fresh_realm(&mut self) -> Self {
        Self::default()
    }

    /// Drop state from callbacks belonging to a rejected PDF edit.
    pub fn reset_after_refusal(&mut self) {
        *self = Self::default();
    }

    /// Run in the web-local realm; native prompt replies are unavailable on this host.
    pub fn run(&mut self, mut job: Job, _answers: &[Option<String>]) -> Outcome {
        self.instances.stamp(&mut job);
        self.stamp_icons(&mut job);
        self.realm.run(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Create a small button-event job with document globals used by persistent-session regressions.
    fn job(source: &str) -> Job {
        Job {
            script: source.into(),
            event: Event::field("Mouse Up", "go", ""),
            doc: DocInfo::default(),
            fields: Arc::new(vec![FieldState::new("go", crate::FieldType::Button, vec!["old".into()])]),
            doc_scripts: vec!["var buffer='', init=1, opened=event.name;var held=getField('go');function digit(v){buffer+=v;return buffer;}".into()],
            calculations: Vec::new(),
        }
    }
    #[test]
    /// Retained Field identity, custom properties and keypad buffers persist locally while separate documents stay isolated.
    fn document_realm_keeps_keypad_state_and_isolates_documents() {
        let mut a = ScriptSession::default();
        let mut b = ScriptSession::default();
        assert_eq!(a.run(job("digit('1');"), &[]).result.as_deref(), Some("1"));
        assert_eq!(a.run(job("getField('go').custom=42;getField('go')===held;"), &[]).result.as_deref(), Some("true"));
        assert_eq!(a.run(job("getField('go').custom;"), &[]).result.as_deref(), Some("42"));
        assert_eq!(a.run(job("[digit('2'),init,opened].join('/');"), &[]).result.as_deref(), Some("12/1/Open"));
        assert_eq!(b.run(job("digit('3');"), &[]).result.as_deref(), Some("3"));
        let mut changed = job("held.value;");
        changed.fields = Arc::new(vec![FieldState::new("go", crate::FieldType::Button, vec!["live".into()])]);
        assert_eq!(a.run(changed, &[]).result.as_deref(), Some("live"));
        let mut edited = job("buffer;");
        edited.doc_scripts = vec!["var buffer='changed';".into()];
        assert_eq!(a.run(edited, &[]).result.as_deref(), Some("changed"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn host_work_reads_globals_in_the_correct_document() {
        let read_counter = |context: &mut crate::Context| {
            let result = context
                .global_object()
                .get(boa_engine::js_string!("counter"), context)
                .ok()
                .and_then(|value| value.as_number())
                .map(|number| number.to_string());
            Outcome { result, ..Default::default() }
        };
        let mut first = ScriptSession::default();
        let mut second = ScriptSession::default();
        assert!(first.run(job("var counter = 17;"), &[]).error.is_none());
        assert!(second.run(job("var counter = 31;"), &[]).error.is_none());
        let timeout = std::time::Duration::from_secs(5);
        assert_eq!(first.run_host_task(timeout, read_counter).result.as_deref(), Some("17"));
        assert_eq!(second.run_host_task(timeout, read_counter).result.as_deref(), Some("31"));
    }

    #[test]
    /// A script too large to run fails on its own, like a script that throws, without refusing the edit.
    fn an_oversized_script_is_an_error_not_a_refusal() {
        let mut session = ScriptSession::default();
        let long = format!("event.value = 1;/*{}*/", "x".repeat(300 * 1024));
        let out = session.run(job(&long), &[]);
        assert!(out.error.is_some());
        assert!(!out.execution_refused);
        assert!(session.run(job("1 + 1"), &[]).error.is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    /// A waiting prompt refuses concurrent document runs immediately instead of blocking the caller behind it.
    fn a_waiting_prompt_refuses_reentrant_host_runs_without_blocking_the_ui() {
        let mut session = ScriptSession::default();
        let receiver = session.start(job("app.response({cQuestion:'Wait'});")).unwrap();
        let InteractiveEvent::Prompt(_, reply) = receiver.recv_timeout(std::time::Duration::from_secs(2)).unwrap() else { panic!("expected prompt") };
        let started = std::time::Instant::now();
        let refused = session.run(job("buffer;"), &[]);
        assert!(refused.error.as_deref().unwrap().contains("another script is active"));
        let refused = session.run_host_task(std::time::Duration::from_secs(1), |_| panic!("host task entered a busy realm"));
        assert!(refused.execution_refused && refused.realm_untouched);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        reply.send(None).unwrap();
        assert!(matches!(receiver.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), InteractiveEvent::Complete(_)));
    }
    /// A retained realm must expose current metadata without replaying document initialization.
    #[test]
    fn persistent_document_properties_follow_the_current_snapshot() {
        let mut session = ScriptSession::default();
        let source = "[numPages,documentFileName,info.title,init,buffer].join('/');";
        let mut first = job(source);
        first.doc = DocInfo { num_pages: 1, file_name: "before.pdf".into(), info: vec![("title".into(), "Before".into())], ..Default::default() };
        assert_eq!(session.run(first, &[]).result.as_deref(), Some("1/before.pdf/Before/1/"));
        assert_eq!(session.run(job("buffer='kept';"), &[]).error, None);
        let mut next = job(source);
        next.doc = DocInfo { num_pages: 7, file_name: "after.pdf".into(), info: vec![("title".into(), "After".into())], ..Default::default() };
        assert_eq!(session.run(next, &[]).result.as_deref(), Some("7/after.pdf/After/1/kept"));
        let mut removed = job("numPages===0 && documentFileName==='' && typeof info.title==='undefined' && buffer==='kept';");
        removed.doc = DocInfo::default();
        assert_eq!(session.run(removed, &[]).result.as_deref(), Some("true"));
    }

    /// Observe real address-space growth for four simultaneously live document workers.
    #[cfg(target_os = "linux")]
    #[test]
    fn persistent_workers_do_not_reserve_a_gibibyte_per_document() {
        const CHILD: &str = "PDFCRAFT_WORKER_RESERVATION_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "session::tests::persistent_workers_do_not_reserve_a_gibibyte_per_document", "--nocapture"])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(result.status.success(), "reservation child failed: {}", String::from_utf8_lossy(&result.stderr));
            eprintln!("{}", String::from_utf8_lossy(&result.stderr));
            return;
        }
        /// Read the kernel's virtual-size metric, in bytes, for this isolated test process.
        fn virtual_bytes() -> u64 {
            let status = std::fs::read_to_string("/proc/self/status").unwrap();
            status.lines().find_map(|line| line.strip_prefix("VmSize:")?.split_whitespace().next()?.parse::<u64>().ok()).unwrap() * 1024
        }
        let before = virtual_bytes();
        let mut workers: Vec<_> = (0..4).map(|_| ScriptSession::default()).collect();
        for worker in &mut workers {
            assert_eq!(worker.run(job("digit('1');"), &[]).result.as_deref(), Some("1"));
        }
        let growth = virtual_bytes().saturating_sub(before);
        eprintln!("four live document workers: virtual address-space growth {growth} bytes");
        assert!(growth < 768 * 1024 * 1024, "four workers reserved {growth} bytes");
        for worker in &mut workers {
            assert_eq!(worker.run(job("digit('2');"), &[]).result.as_deref(), Some("12"));
        }
    }

    /// Run parser/compiler boundary cases in a child process so a stack abort is observable.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn document_worker_contains_hostile_and_boundary_scripts() {
        const CHILD: &str = "PDFCRAFT_WORKER_STACK_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "session::tests::document_worker_contains_hostile_and_boundary_scripts", "--nocapture"])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(result.status.success(), "worker child failed: {}", String::from_utf8_lossy(&result.stderr));
            return;
        }
        let mut worker = ScriptSession::default();
        for script in [
            format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000)),
            format!("{}1", "!".repeat(100_000)),
            format!("{}1", "1?1:".repeat(10_000)),
            format!("{}1", "a=>".repeat(100_000)),
            format!("{}1", "a=>".repeat(70_000)),
            format!("{}1", "a=>".repeat(257)),
            "function recursive(){return recursive();} recursive();".into(),
            "while(true){}".into(),
        ] {
            assert!(worker.run(job(&script), &[]).error.is_some());
        }
        for (script, expected) in [
            (format!("{}1{}", "(".repeat(64), ")".repeat(64)), "1"),
            (format!("1{}", "+1".repeat(50_000)), "50001"),
            (format!("var a;{}1;true;", "a=".repeat(4096)), "true"),
            (format!("{}1;true;", "1**".repeat(4096)), "true"),
            (format!("{}1;true;", "void ".repeat(64)), "true"),
            (format!("{}1;true;", "typeof ".repeat(64)), "true"),
            (format!("{}1", "false?1:".repeat(4096)), "1"),
            (format!("{}1", "!".repeat(64)), "true"),
            (format!("typeof ({}1)", "a=>".repeat(256)), "function"),
            (format!("[{}].length", vec!["()=>1"; 256].join(",")), "256"),
            ("function bounded(n){return n?bounded(n-1)+1:0;} bounded(32);".into(), "32"),
        ] {
            let result = worker.run(job(&script), &[]);
            assert_eq!(result.error, None);
            assert_eq!(result.result.as_deref(), Some(expected));
        }
        assert_eq!(worker.run(job("digit('7');"), &[]).result.as_deref(), Some("7"));
    }

    /// Isolate unbracketed parser recursion families, which evade bracket-depth checks.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn document_worker_bounds_unbracketed_expression_chains() {
        const CHILD: &str = "PDFCRAFT_EXPRESSION_CHAIN_CHILD";
        let Ok(case) = std::env::var(CHILD) else {
            for case in 0..5 {
                let result = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "session::tests::document_worker_bounds_unbracketed_expression_chains", "--nocapture"])
                    .env(CHILD, case.to_string())
                    .output()
                    .unwrap();
                assert!(result.status.success(), "expression family {case}: {}", String::from_utf8_lossy(&result.stderr));
            }
            return;
        };
        let script = match case.as_str() {
            "0" => format!("var a;{}1", "a=".repeat(70_000)),
            "1" => format!("{}1", "1**".repeat(60_000)),
            "2" => format!("{}1", "void ".repeat(40_000)),
            "3" => format!("{}Object", "new ".repeat(50_000)),
            _ => format!("var a;{}{}{}1", "a=".repeat(4096), "false?1:".repeat(4096), "1**".repeat(4096)),
        };
        let mut worker = ScriptSession::default();
        let outcome = worker.run(job(&script), &[]);
        if case == "4" {
            // Combining individually permitted boundary chains must also fit the worker.
            assert_eq!(outcome.error, None);
            assert_eq!(outcome.result.as_deref(), Some("1"));
        } else {
            assert!(outcome.error.is_some(), "over-budget chain was admitted");
        }
        assert_eq!(worker.run(job("digit('8');"), &[]).result.as_deref(), Some("8"));
    }

    /// Independent statements and comparisons must not consume a source-wide nesting budget.
    #[test]
    fn document_worker_accepts_large_flat_libraries() {
        let scripts = [
            format!("var a=1,b=0;{}b;", "if(a===1){b=2;}".repeat(2000)),
            format!("{}f299();", (0..300).map(|i| format!("const f{i}=()=>1;")).collect::<String>()),
            format!("{}f299()", (0..300).map(|i| format!("const f{i}=()=>1\n")).collect::<String>()),
            format!("var a=0\n{}a", "a=2**3\n".repeat(6000)),
            format!("var a=0;{}a;", "a+=1;".repeat(6000)),
            format!("var a=0;{}a;", "a=2**3;".repeat(6000)),
            // Within the nesting guard's 4,096 conditionals.
            format!("var a=0;{}a;", "a=(1===1)?1:2;".repeat(4000)),
        ];
        for (script, expected) in scripts.into_iter().zip(["2", "1", "1", "8", "6000", "8", "1"]) {
            let mut worker = ScriptSession::default();
            let result = worker.run(job(&script), &[]);
            assert_eq!(result.error, None);
            assert_eq!(result.result.as_deref(), Some(expected));
        }
        let block = format!("function library(){{var x=0\n{}return x}}\nlibrary()", "x=1\n".repeat(6000));
        assert_eq!(ScriptSession::default().run(job(&block), &[]).result.as_deref(), Some("1"));
        let mut document = job("f299();");
        document.doc_scripts.push((0..300).map(|i| format!("const f{i}=()=>1\n")).collect::<String>());
        let result = ScriptSession::default().run(document, &[]);
        assert_eq!(result.error, None);
        assert!(result.doc_errors.is_empty());
        assert_eq!(result.result.as_deref(), Some("1"));
    }

    #[test]
    /// Retaining the realm must not re-enable eval or dynamic Function construction.
    fn persistent_context_keeps_dynamic_compilation_blocked() {
        let mut a = ScriptSession::default();
        assert!(a.run(job("eval('1')"), &[]).error.is_some());
        assert!(a.run(job("new Function('return 1')()"), &[]).error.is_some());
    }
    /// The completion itself must not release the realm before its caller accepts the outcome.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn completion_requires_acknowledgement_before_the_next_job() {
        let mut session = ScriptSession::default();
        let rx = session.start(job("buffer='pending';")).unwrap();
        assert!(matches!(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), InteractiveEvent::Complete(_)));
        assert!(session.run(job("buffer"), &[]).error.is_some());
        session.acknowledge_completion();
        assert_eq!(session.run(job("buffer"), &[]).result.as_deref(), Some("pending"));
    }
    /// Multiple prompt round trips share one wall-clock budget.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn synchronous_prompt_waits_share_one_deadline() {
        use std::time::{Duration, Instant};
        let (tx, rx) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            for _ in 0..3 {
                std::thread::sleep(Duration::from_millis(50));
                let (answer, reply) = mpsc::channel();
                if tx.send(InteractiveEvent::Prompt(crate::interaction::Prompt::Menu(Vec::new()), answer)).is_err() {
                    return;
                }
                if reply.recv_timeout(Duration::from_secs(1)).is_err() {
                    return;
                }
            }
            let _ = tx.send(InteractiveEvent::Complete(Outcome::default()));
        });
        assert!(ScriptSession::receive(&rx, &[], Instant::now(), Duration::from_millis(80)).is_none());
        drop(rx);
        producer.join().unwrap();
    }
    /// A timed-out invocation cannot seed globals/custom Field properties in its replacement realm.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn timed_out_jobs_never_share_the_next_realm() {
        let mut session = ScriptSession::default();
        use std::time::{Duration, Instant};
        let out = session.run_with_limit(job("buffer='late';held.custom=99;"), &[], Duration::ZERO);
        assert!(out.execution_refused);
        let deadline = Instant::now() + Duration::from_secs(2);
        while session.worker_lifetime.upgrade().is_some() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(session.run(job("buffer+'/'+typeof held.custom"), &[]).result.as_deref(), Some("/undefined"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_detached_worker_holds_the_documents_slot_until_it_exits() {
        let worker = Arc::new(());
        let mut session = ScriptSession { worker_lifetime: Arc::downgrade(&worker), ..Default::default() };
        let out = session.run(job("1+1"), &[]);
        assert!(out.execution_refused);
        assert!(session.sender.is_none());
        drop(worker);
        assert_eq!(session.run(job("1+1"), &[]).result.as_deref(), Some("2"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn english_retry_is_isolated_and_obeys_the_remaining_time() {
        use std::time::Duration;
        let mut session = ScriptSession::default();
        assert_eq!(session.run(job("var retained = 17; retained;"), &[]).result.as_deref(), Some("17"));
        let retry = session.run_english_retry(job("var retryOnly = 1; typeof retained;"), Duration::from_secs(2));
        assert_eq!(retry.result.as_deref(), Some("undefined"));
        assert_eq!(session.run(job("retained + '/' + typeof retryOnly;"), &[]).result.as_deref(), Some("17/undefined"));
        let expired = session.run_english_retry(job("1;"), Duration::ZERO);
        assert!(expired.execution_refused);
    }

    #[test]
    fn icon_tracking_overflow_preserves_handles_until_history_changes() {
        let mut session = ScriptSession::default();
        Arc::make_mut(&mut session.icon_versions).extend((1..=16_384).map(|id| (id, 0)));
        let mut capture = job("var oldIcon = getField('go').buttonGetIcon();");
        Arc::make_mut(&mut capture.fields)[0].icon = Some(16_385);
        assert_eq!(session.run(capture.clone(), &[]).error, None);
        capture.script = "getField('go').buttonSetIcon(oldIcon);".into();
        assert_eq!(session.run(capture.clone(), &[]).error, None);
        session.retire_changed_icons(|_| false);
        assert!(session.run(capture, &[]).error.is_some());
    }

    #[test]
    fn history_retires_icons_without_discarding_the_document_globals() {
        let mut session = ScriptSession::default();
        let mut capture = job("var oldIcon = getField('go').buttonGetIcon(); buffer='retained';");
        Arc::make_mut(&mut capture.fields)[0].icon = Some(17);
        assert_eq!(session.run(capture.clone(), &[]).error, None);
        capture.script = "getField('go').buttonSetIcon(oldIcon);".into();
        assert_eq!(session.run(capture.clone(), &[]).error, None);
        session.retire_changed_icons(|id| id == 18);
        assert_eq!(session.run(capture.clone(), &[]).error, None);
        session.retire_changed_icons(|id| id == 17);
        assert!(session.run(capture.clone(), &[]).error.is_some());
        capture.script = "buffer;".into();
        assert_eq!(session.run(capture, &[]).result.as_deref(), Some("retained"));
    }

    /// Exercise the actual process-wide cap in isolation, then recover a slot without resetting other documents.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn document_worker_capacity_is_bounded_and_reusable() {
        const CHILD: &str = "PDFCRAFT_WORKER_CAPACITY_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "session::tests::document_worker_capacity_is_bounded_and_reusable"])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(out.status.success(), "{} {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            return;
        }
        let mut workers = Vec::new();
        for _ in 0..MAX_DOCUMENT_WORKERS {
            let mut worker = ScriptSession::default();
            assert_eq!(worker.run(job("digit('1')"), &[]).result.as_deref(), Some("1"));
            workers.push(worker);
        }
        let mut extra = ScriptSession::default();
        assert!(extra.start(job("1")).unwrap_err().contains("worker limit"));
        drop(workers.pop());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while LIVE_WORKERS.load(std::sync::atomic::Ordering::SeqCst) == MAX_DOCUMENT_WORKERS {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(extra.run(job("digit('2')"), &[]).result.as_deref(), Some("2"));
        for worker in &mut workers {
            assert_eq!(worker.run(job("digit('3')"), &[]).result.as_deref(), Some("13"));
        }
    }
    /// Reset effects are visible during Doc Open and to the first event, including retained Field objects.
    #[test]
    fn doc_open_reset_precedes_the_first_event_and_later_writes_win() {
        for reset in ["resetForm();", "resetForm(['group']);"] {
            let mut source = FieldState::new("group.source", crate::FieldType::Text, vec!["old".into()]);
            source.default = vec!["new".into()];
            let dest = FieldState::new("dest", crate::FieldType::Text, vec!["before".into()]);
            let mut task = job("getField('dest').value=held.value;held.value='old';openedValue+'/'+getField('dest').value");
            task.fields = Arc::new(vec![source, dest]);
            task.doc_scripts = vec![format!("var held=getField('group.source');{reset}var openedValue=held.value;")];
            let out = ScriptSession::default().run(task, &[]);
            assert_eq!(out.error, None);
            assert_eq!(out.result.as_deref(), Some("new/new"));
            assert_eq!(out.changed.iter().find(|f| f.name == "group.source").unwrap().value, ["old"]);
            assert_eq!(out.changed.iter().find(|f| f.name == "dest").unwrap().value, ["new"]);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod reset_budget_tests {
    use super::*;
    #[test]
    fn open_and_first_event_share_reset_budget_but_later_jobs_reset_it() {
        let mut runtime = ScriptSession::default();
        let mut job = Job {
            script: "resetForm();".into(),
            event: Event::doc("Console"),
            doc: Default::default(),
            fields: Arc::new(vec![]),
            doc_scripts: vec!["for(var i=0;i<64;i++)resetForm();".into()],
            calculations: vec![],
        };
        let out = runtime.run(job.clone(), &[]);
        assert!(out.reset_limit_exceeded);
        assert_eq!(out.requests.iter().filter(|r| matches!(r, crate::Request::Reset(_))).count(), 64);
        job.script = "resetForm();true;".into();
        let out = runtime.run(job, &[]);
        assert!(!out.reset_limit_exceeded);
        assert_eq!(out.error, None);
        assert_eq!(out.result.as_deref(), Some("true"));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod event_identity_tests {
    use super::*;
    #[test]
    fn ordinary_button_event_target_reuses_public_field_object() {
        let mut field = FieldState::new("go", crate::FieldType::Button, vec![]);
        field.object_id = Some(17);
        let job = Job {
            script: "event.target===getField('go');".into(),
            event: Event::field("Mouse Up", "go", ""),
            doc: Default::default(),
            fields: Arc::new(vec![field]),
            doc_scripts: vec![],
            calculations: vec![],
        };
        let out = ScriptSession::default().run(job, &[]);
        assert_eq!(out.error, None);
        assert_eq!(out.result.as_deref(), Some("true"));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod cache_capacity_tests {
    use super::*;
    #[test]
    fn every_exposed_field_keeps_identity_beyond_4096_entries() {
        let fields = Arc::new(
            (0..4100)
                .map(|i| {
                    let mut f = FieldState::new(format!("f{i}"), crate::FieldType::Text, vec![]);
                    f.object_id = Some(i);
                    f
                })
                .collect(),
        );
        let job=Job{script:"for(var i=0;i<4100;i++)getField('f'+i);var held=getField('f4099');held.custom=42;held===getField('f4099') && getField('f4099').custom===42;".into(),event:Event::doc("Console"),doc:Default::default(),fields,doc_scripts:vec![],calculations:vec![]};
        let mut session = ScriptSession::default();
        let out = session.run(job.clone(), &[]);
        assert_eq!(out.error, None);
        assert_eq!(out.result.as_deref(), Some("true"));
        let out = session.run(Job { script: "held===getField('f4099') && held.custom===42;".into(), ..job }, &[]);
        assert_eq!(out.result.as_deref(), Some("true"));
    }
}

#[cfg(test)]
mod cache_lifecycle_tests {
    use super::*;
    #[test]
    fn cache_follows_object_renames_and_prunes_removed_fields() {
        let mut field = FieldState::new("old", crate::FieldType::Text, vec![]);
        field.object_id = Some(17);
        let mut job = Job {
            script: "var held=getField('old');held.custom=42;".into(),
            event: Event::doc("Console"),
            doc: Default::default(),
            fields: Arc::new(vec![field.clone()]),
            doc_scripts: vec![],
            calculations: vec![],
        };
        let mut realm = Realm::new();
        assert_eq!(realm.run(job.clone()).error, None);
        for i in 0..16 {
            field.name = format!("renamed{i}");
            job.fields = Arc::new(vec![field.clone()]);
            job.script = format!("held.setFocus();held===getField('{}') && held.name==='{}' && held.custom===42;", field.name, field.name);
            let out = realm.run(job.clone());
            assert_eq!(out.error, None);
            assert_eq!(out.result.as_deref(), Some("true"));
            assert_eq!(out.requests, [crate::Request::FocusField { name: field.name.clone(), object_id: 17 }]);
            assert_eq!(crate::host::host(realm.context.as_ref().unwrap()).unwrap().borrow().objects.len(), 1);
        }
        field.object_id = Some(18);
        field.name = "replacement".into();
        job.fields = Arc::new(vec![field]);
        job.script = "getField('replacement');".into();
        assert_eq!(realm.run(job.clone()).error, None);
        assert_eq!(crate::host::host(realm.context.as_ref().unwrap()).unwrap().borrow().objects.len(), 1);
        job.script = "held.value;".into();
        assert!(realm.run(job).error.is_some());
    }
}

#[cfg(test)]
mod review13_tests {
    use super::*;
    use crate::FieldType;
    /// Build isolated jobs without document initialization side effects.
    fn job(script: &str, fields: Vec<FieldState>) -> Job {
        Job {
            script: script.into(),
            event: Event::doc("Test"),
            doc: DocInfo::default(),
            fields: Arc::new(fields),
            doc_scripts: vec![],
            calculations: vec![],
        }
    }
    /// A retired accessor must not resolve a replacement even if no job observed the deletion.
    #[test]
    fn field_instances_retire_between_jobs_without_losing_globals() {
        let mut session = ScriptSession::default();
        let mut f = FieldState::new("old", FieldType::Text, vec!["original".into()]);
        f.object_id = Some(7);
        assert_eq!(session.run(job("var held=getField('old');held.custom=42;var kept=99;", vec![f.clone()]), &[]).error, None);
        session.sync_field_ids([]);
        session.sync_field_ids([7]);
        f.name = "replacement".into();
        let out =
            session.run(job("kept===99 && getField('replacement')!==held && getField('replacement').custom===undefined;", vec![f.clone()]), &[]);
        assert_eq!(out.result.as_deref(), Some("true"));
        for script in ["held.value;", "held.value='wrong';", "held.setFocus();"] {
            let out = session.run(job(script, vec![f.clone()]), &[]);
            assert!(out.error.is_some());
            assert!(out.changed.is_empty());
        }
    }
    /// Frozen globals still expose each new event and permit nested calculation event restoration.
    #[test]
    fn frozen_global_keeps_live_event_binding() {
        let mut session = ScriptSession::default();
        let f = FieldState::new("amount", FieldType::Text, vec!["2".into()]);
        let mut first = job("event.name", vec![f.clone()]);
        first.doc_scripts = vec!["Object.freeze(globalThis);".into()];
        assert_eq!(session.run(first.clone(), &[]).result.as_deref(), Some("Test"));
        first.event = Event::doc("Next");
        first.script = "calculateNow();event.name".into();
        first.calculations = vec![(0, "event.value='3';".into())];
        let out = session.run(first, &[]);
        assert_eq!(out.error, None);
        assert_eq!(out.result.as_deref(), Some("Next"));
        assert_eq!(out.changed[0].value, ["3"]);
    }
    /// Calculation values follow the existing setter normalization, including unchecked empty boxes.
    #[test]
    fn calculate_now_normalizes_checkbox_but_preserves_text_and_combo_empty_values() {
        let mut session = ScriptSession::default();
        let fields = vec![
            FieldState::new("box", FieldType::CheckBox, vec!["Yes".into()]),
            FieldState::new("text", FieldType::Text, vec!["old".into()]),
            FieldState::new("combo", FieldType::ComboBox, vec!["old".into()]),
        ];
        let mut j = job("calculateNow();getField('box').value", fields);
        j.calculations = (0..3).map(|i| (i, "event.value='';".into())).collect();
        let out = session.run(j, &[]);
        assert_eq!(out.error, None);
        assert_eq!(out.result.as_deref(), Some("Off"));
        assert!(out.changed[0].value.is_empty());
        assert_eq!(out.changed[1].value, [""]);
        assert_eq!(out.changed[2].value, [""]);
    }
    /// Small individual lookups cannot multiply unbounded work across calculation sources.
    #[test]
    fn calculation_sources_share_one_job_budget_and_later_jobs_reset_it() {
        let mut session = ScriptSession::default();
        let fields: Vec<_> = (0..6).map(|i| FieldState::new(format!("f{i}"), FieldType::Text, vec!["1".into()])).collect();
        let mut j = job("calculateNow();", fields.clone());
        j.calculations = (0..6).map(|i| (i, "for(var q=0;q<3;q++)AFSimple_Calculate('SUM',new Array(1024).fill('x'.repeat(256)));".into())).collect();
        let out = session.run(j, &[]);
        assert!(out.error.as_deref().is_some_and(|e| e.contains("work limit")), "{:?}", out.error);
        assert_eq!(session.run(job("AFSimple_Calculate('SUM',['f0']);", fields), &[]).result.as_deref(), Some("1"));
    }
    #[test]
    fn calculate_now_continues_after_an_ordinary_field_error() {
        let mut session = ScriptSession::default();
        let fields = ["bad", "good"].map(|name| FieldState::new(name, FieldType::Text, vec!["0".into()])).to_vec();
        let mut j = job("calculateNow(); event.name + ':' + getField('good').value", fields);
        j.calculations = vec![(0, "throw new Error('broken formula');".into()), (1, "event.value='42';".into())];
        let out = session.run(j, &[]);
        assert_eq!(out.error, None);
        assert_eq!(out.result.as_deref(), Some("Test:42"));
        assert!(out.console.iter().any(|line| line.contains("broken formula")));
    }

    #[test]
    fn caught_calculation_exhaustion_stays_sticky_until_the_next_job() {
        let mut session = ScriptSession::default();
        let fields = vec![FieldState::new("amount", FieldType::Text, vec!["0".into()])];
        let mut j = job("for(var i=0;i<4097;i++){try{calculateNow()}catch(e){}} try{calculateNow()}catch(e){} count;", fields.clone());
        j.calculations = vec![(0, "count += 1;".into())];
        j.doc_scripts = vec!["var count = 0;".into()];
        let out = session.run(j.clone(), &[]);
        assert!(out.calculation_limit_exceeded);
        assert!(out.error.as_deref().is_some_and(|e| e.contains("calculation work limit")));
        assert_eq!(out.result.as_deref(), Some("4096"));
        j.script = "calculateNow();count".into();
        let next = session.run(j, &[]);
        assert!(!next.calculation_limit_exceeded);
        assert_eq!(next.result.as_deref(), Some("4097"));
    }
}
