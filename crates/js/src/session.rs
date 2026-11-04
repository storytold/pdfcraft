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
        let failed = |why: String| Outcome { rc: true, error: Some(why), ..Default::default() };
        if let Some(why) = crate::refuse(&job.script) {
            return failed(why);
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
                },
                Limits::default(),
            );
            let (fields, reset_calls) = crate::host::host(ctx)
                .map(|h| {
                    let h = h.borrow();
                    (h.fields.clone(), h.reset_calls)
                })
                .unwrap_or((job.fields, 0));
            let mut out = crate::run::run_in_context(
                ctx,
                &job.script,
                &job.event,
                &job.doc,
                fields,
                crate::run::ScriptSetup { doc_scripts: &[], skipped: &[], calculations: &job.calculations, initialize: false, reset_calls },
                Limits::default(),
            );
            out.reset_limit_exceeded |= initial.reset_limit_exceeded;
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
                crate::run::ScriptSetup { doc_scripts: &[], skipped: &[], calculations: &job.calculations, initialize: false, reset_calls: 0 },
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
/// Native document runtime with one bounded-stack worker and at most one active invocation.
/// Dropping it releases the sender; after any active job finishes the worker exits and drops its realm.
#[derive(Default)]
pub struct ScriptSession {
    sender: Option<mpsc::SyncSender<(Job, mpsc::Sender<InteractiveEvent>)>>,
    busy: Arc<std::sync::atomic::AtomicBool>,
}
#[cfg(not(target_arch = "wasm32"))]
impl ScriptSession {
    /// Start a bounded-stack native worker lazily and reject a second active job, including during prompts.
    pub fn start(&mut self, job: Job) -> Result<mpsc::Receiver<InteractiveEvent>, String> {
        if self.sender.is_none() {
            let (tx, rx) = mpsc::sync_channel::<(Job, mpsc::Sender<InteractiveEvent>)>(1);
            let permit = WorkerPermit::acquire()?;
            std::thread::Builder::new()
                .name("pdfcraft-document-js".into())
                .stack_size(crate::run::DOCUMENT_WORKER_STACK)
                .spawn(move || {
                    let _permit = permit;
                    let mut realm = Realm::new();
                    while let Ok((job, reply)) = rx.recv() {
                        crate::interaction::set_host(Some(reply.clone()));
                        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| realm.run(job)));
                        crate::interaction::set_host(None);
                        let out = out.unwrap_or_else(|_| {
                            realm = Realm::new();
                            Outcome { rc: true, error: Some("script engine stopped unexpectedly".into()), ..Default::default() }
                        });
                        let _ = reply.send(InteractiveEvent::Complete(out));
                    }
                })
                .map_err(|e| e.to_string())?;
            self.sender = Some(tx);
        }
        if self.busy.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_err() {
            return Err("another script is active in this document".into());
        }
        let (tx, rx) = mpsc::channel();
        let Some(sender) = self.sender.as_ref() else {
            self.busy.store(false, std::sync::atomic::Ordering::SeqCst);
            return Err("script worker unavailable".into());
        };
        sender.try_send((job, tx)).map_err(|_| {
            self.busy.store(false, std::sync::atomic::Ordering::SeqCst);
            "script worker disconnected"
        })?;
        Ok(rx)
    }
    /// An asynchronous completion remains pending until its document outcome is accepted or discarded.
    pub fn is_pending(&self) -> bool {
        self.busy.load(std::sync::atomic::Ordering::SeqCst)
    }
    /// Acknowledge a received completion before applying its edits on the same document realm.
    pub fn acknowledge_completion(&mut self) {
        self.busy.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    /// Run a job with one total deadline, discarding its entire realm when the host times out.
    pub fn run(&mut self, job: Job, answers: &[Option<String>]) -> Outcome {
        self.run_with_limit(job, answers, std::time::Duration::from_secs(10))
    }
    /// A separate limit permits deterministic timeout regressions without ten-second test delays.
    fn run_with_limit(&mut self, job: Job, answers: &[Option<String>], limit: std::time::Duration) -> Outcome {
        let started = std::time::Instant::now();
        let rx = match self.start(job) {
            Ok(rx) => rx,
            Err(e) => return Outcome { error: Some(e), ..Default::default() },
        };
        match Self::receive(&rx, answers, started, limit) {
            Some(out) => {
                self.acknowledge_completion();
                out
            }
            None => {
                // The old worker has no route into this replacement realm. Its reservation
                // remains charged until cooperative execution finishes and the thread exits.
                *self = Self::default();
                Outcome { error: Some("script worker did not complete within its limit".into()), ..Default::default() }
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
}
#[cfg(target_arch = "wasm32")]
impl Default for ScriptSession {
    /// Create the web runtime without native worker threads or a native prompt host.
    fn default() -> Self {
        Self { realm: Realm::new() }
    }
}
#[cfg(target_arch = "wasm32")]
impl ScriptSession {
    /// Run in the web-local realm; native prompt replies are unavailable on this host.
    pub fn run(&mut self, job: Job, _answers: &[Option<String>]) -> Outcome {
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
    /// A waiting prompt refuses concurrent document runs immediately instead of blocking the caller behind it.
    fn a_waiting_prompt_refuses_reentrant_host_runs_without_blocking_the_ui() {
        let mut session = ScriptSession::default();
        let receiver = session.start(job("app.response({cQuestion:'Wait'});")).unwrap();
        let InteractiveEvent::Prompt(_, reply) = receiver.recv_timeout(std::time::Duration::from_secs(2)).unwrap() else { panic!("expected prompt") };
        let started = std::time::Instant::now();
        let refused = session.run(job("buffer;"), &[]);
        assert!(refused.error.as_deref().unwrap().contains("another script is active"));
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
            format!("var a=0;{}a;", "a=(1===1)?1:2;".repeat(5000)),
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
        assert!(session.run_with_limit(job("buffer='late';held.custom=99;"), &[], std::time::Duration::ZERO).error.is_some());
        assert_eq!(session.run(job("buffer+'/'+typeof held.custom"), &[]).result.as_deref(), Some("/undefined"));
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
