//! One sandboxed realm per open document. No context crosses a document boundary.
#[cfg(not(target_arch = "wasm32"))]
use crate::interaction::InteractiveEvent;
use crate::{DocInfo, Event, FieldState, Limits, Outcome};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;

#[derive(Clone)]
pub struct Job {
    pub script: String,
    pub event: Event,
    pub doc: DocInfo,
    pub fields: Arc<Vec<FieldState>>,
    pub doc_scripts: Vec<String>,
    pub calculations: Vec<(String, String)>,
}
pub(crate) struct Realm {
    context: Option<crate::Context>,
    scripts: Vec<String>,
}
impl Realm {
    fn new() -> Self {
        Self { context: None, scripts: Vec::new() }
    }
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
#[derive(Default)]
pub struct ScriptSession {
    sender: Option<mpsc::SyncSender<(Job, mpsc::Sender<InteractiveEvent>)>>,
    busy: Arc<std::sync::atomic::AtomicBool>,
}
#[cfg(not(target_arch = "wasm32"))]
impl ScriptSession {
    pub fn start(&mut self, job: Job) -> Result<mpsc::Receiver<InteractiveEvent>, String> {
        if self.sender.is_none() {
            let (tx, rx) = mpsc::sync_channel::<(Job, mpsc::Sender<InteractiveEvent>)>(1);
            let busy = self.busy.clone();
            std::thread::Builder::new()
                .name("pdfcraft-document-js".into())
                .stack_size(crate::run::SCRIPT_STACK)
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
#[cfg(target_arch = "wasm32")]
pub struct ScriptSession {
    realm: Realm,
}
#[cfg(target_arch = "wasm32")]
impl Default for ScriptSession {
    fn default() -> Self {
        Self { realm: Realm::new() }
    }
}
#[cfg(target_arch = "wasm32")]
impl ScriptSession {
    pub fn run(&mut self, job: Job, _answers: &[Option<String>]) -> Outcome {
        self.realm.run(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    #[test]
    fn persistent_context_keeps_dynamic_compilation_blocked() {
        let mut a = ScriptSession::default();
        assert!(a.run(job("eval('1')"), &[]).error.is_some());
    }
}
