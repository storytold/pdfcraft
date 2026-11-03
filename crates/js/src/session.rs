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
    pub calculations: Vec<(String, String)>,
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
                crate::run::ScriptSetup { doc_scripts: &scripts, skipped: &skipped, calculations: &job.calculations, initialize: true },
                Limits::default(),
            );
            let fields = crate::host::host(ctx).map(|h| h.borrow().fields.clone()).unwrap_or(job.fields);
            let mut out = crate::run::run_in_context(
                ctx,
                &job.script,
                &job.event,
                &job.doc,
                fields,
                crate::run::ScriptSetup { doc_scripts: &[], skipped: &[], calculations: &job.calculations, initialize: false },
                Limits::default(),
            );
            for field in initial.changed {
                if !out.changed.iter().any(|f| f.name == field.name) {
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
                crate::run::ScriptSetup { doc_scripts: &[], skipped: &[], calculations: &job.calculations, initialize: false },
                Limits::default(),
            )
        }
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
            let busy = self.busy.clone();
            std::thread::Builder::new()
                .name("pdfcraft-document-js".into())
                .stack_size(crate::run::DOCUMENT_WORKER_STACK)
                .spawn(move || {
                    let mut realm = Realm::new();
                    while let Ok((job, reply)) = rx.recv() {
                        crate::interaction::set_host(Some(reply.clone()));
                        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| realm.run(job)));
                        crate::interaction::set_host(None);
                        let out = out.unwrap_or_else(|_| {
                            realm = Realm::new();
                            Outcome { rc: true, error: Some("script engine stopped unexpectedly".into()), ..Default::default() }
                        });
                        busy.store(false, std::sync::atomic::Ordering::SeqCst);
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
    /// Run a job synchronously, consuming ordered prompt answers and returning a bounded worker error on failure.
    pub fn run(&mut self, job: Job, answers: &[Option<String>]) -> Outcome {
        let rx = match self.start(job) {
            Ok(rx) => rx,
            Err(e) => return Outcome { error: Some(e), ..Default::default() },
        };
        let mut answers = answers.iter();
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(InteractiveEvent::Complete(out)) => return out,
                Ok(InteractiveEvent::Prompt(_, reply)) => {
                    let _ = reply.send(answers.next().cloned().flatten());
                }
                Err(_) => return Outcome { error: Some("script worker did not complete within its limit".into()), ..Default::default() },
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
    }
}
