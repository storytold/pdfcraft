//! One script engine for a whole form event. Every script of the event runs on one thread
//! (with the script stack), each in a fresh JavaScript context on a fresh copy of the form as
//! the caller holds it, so scripts can't see each other's variables or objects and a script
//! that failed halfway leaves no state behind in the engine; what a script changed (up to its
//! error, when it failed) reaches the next one only through the caller, which applies the
//! effects and sends the form as it then is. Before,
//! every script also got a thread of its own and the caller rebuilt the form for each: a form
//! with hundreds of calculations spent its open-time budget on that.
//!
//! A script that runs past its time limit is abandoned on the thread and the session is dead
//! from then on. A script that fails from a fault in the engine (a panic) loses only itself:
//! the thread is replaced for the next script. On wasm there are no threads: scripts run here.

use crate::Limits;
use crate::xfa::{XfaDoc, XfaEvent, XfaNode, XfaOutcome};

/// Tests: milliseconds the engine is made to take before it is ready for a script.
#[cfg(test)]
pub(crate) static START_DELAY_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// What one script run needs.
#[derive(Clone, Debug)]
pub struct Script {
    pub source: String,
    pub formcalc: bool,
    pub event: XfaEvent,
    /// The form as the caller holds it now.
    pub root: XfaNode,
    pub limits: Limits,
    pub timeout: std::time::Duration,
}

/// Run `s` here: a fresh context (JavaScript) or interpreter (FormCalc) on its copy of the
/// form. `started` is called once the engine is ready for the script (its context built), so
/// a time limit on the script runs from then, not from when it was asked for.
fn run_script(s: Script, doc: &XfaDoc, started: &dyn Fn()) -> XfaOutcome {
    // Tests make the engine slow to be ready, to see that the time isn't the script's.
    #[cfg(test)]
    let started = &|| {
        let ms = START_DELAY_MS.load(std::sync::atomic::Ordering::Relaxed);
        if ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
        started();
    };
    if s.formcalc {
        // FormCalc has no engine to start: its time (and its own deadline) begins at once.
        started();
        return crate::formcalc::run_formcalc_at(&s.source, &s.event, doc, s.root, s.limits, Some(s.timeout));
    }
    if let Some(why) = crate::refuse(&s.source) {
        started();
        return XfaOutcome { error: Some(why), ..Default::default() };
    }
    crate::xfa::run_here(&s.source, &s.event, doc, s.root, s.limits, started)
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
    use std::time::Instant;

    use super::{Script, run_script};
    use crate::xfa::{ENGINE_START_LIMIT, XfaDoc, XfaOutcome};

    enum Reply {
        /// The thread is up.
        Ready,
        /// The engine is ready for the script just sent: its time starts now.
        Started,
        Outcome(Box<XfaOutcome>),
    }

    /// The engine's thread, started for the first script and kept for the event.
    struct Worker {
        to: Sender<Box<Script>>,
        from: Receiver<Reply>,
    }

    /// The scripts of one event, run on one thread.
    pub struct XfaSession {
        doc: XfaDoc,
        worker: Option<Worker>,
        /// A script ran past its limit: its thread is left to it and nothing more runs.
        dead: Option<String>,
    }

    impl XfaSession {
        pub fn new(doc: XfaDoc) -> XfaSession {
            XfaSession { doc, worker: None, dead: None }
        }

        /// Why nothing more runs, once a script was abandoned.
        pub fn dead(&self) -> Option<&str> {
            self.dead.as_deref()
        }

        /// The thread, started now when there is none (the first script, or after one failed
        /// with a fault in the engine). `Err`: it could not start.
        fn worker(&mut self) -> Result<&mut Worker, String> {
            if self.worker.is_none() {
                let (to, inbox) = channel::<Box<Script>>();
                let (reply, from) = channel::<Reply>();
                let doc = self.doc.clone();
                let spawned = std::thread::Builder::new().name("pdfcraft-xfa-session".into()).stack_size(crate::SCRIPT_STACK).spawn(move || {
                    crate::xfa::warm_up();
                    // The caller is gone when a send fails: nothing to report then.
                    let _ = reply.send(Reply::Ready);
                    while let Ok(s) = inbox.recv() {
                        let started = reply.clone();
                        let o = run_script(*s, &doc, &move || {
                            let _ = started.send(Reply::Started);
                        });
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

        /// Run one script, waiting at most its `timeout` from when the engine is ready for it
        /// (the engine itself gets [`ENGINE_START_LIMIT`] to be ready); past that it is
        /// abandoned on its thread and the session is dead.
        pub fn run(&mut self, script: Script) -> XfaOutcome {
            let abandoned = |why: String| XfaOutcome { error: Some(why), abandoned: true, ..Default::default() };
            let failed = |why: String| XfaOutcome { error: Some(why), ..Default::default() };
            if let Some(why) = &self.dead {
                return abandoned(why.clone());
            }
            let timeout = script.timeout;
            let worker = match self.worker() {
                Ok(w) => w,
                Err(why) => return failed(why),
            };
            if worker.to.send(Box::new(script)).is_err() {
                // The thread is gone (a fault in the engine): the next script gets a new one.
                self.worker = None;
                return failed("the script engine stopped with an internal error".into());
            }
            // The engine's start, then the script's own time.
            let (mut running, mut limit) = (false, ENGINE_START_LIMIT);
            let mut deadline = Instant::now().checked_add(limit);
            loop {
                let left = deadline.map_or(limit, |d| d.saturating_duration_since(Instant::now()));
                match worker.from.recv_timeout(left) {
                    Ok(Reply::Outcome(o)) => return *o,
                    Ok(Reply::Started) => {
                        // A second call changes nothing: the script's time has already begun.
                        if !running {
                            (running, limit) = (true, timeout);
                            deadline = Instant::now().checked_add(limit);
                        }
                    }
                    // Not expected here: wait on for the outcome.
                    Ok(Reply::Ready) => continue,
                    Err(RecvTimeoutError::Timeout) => {
                        let why = if running {
                            format!("the script ran longer than {:.1} s and was abandoned", timeout.as_secs_f64())
                        } else {
                            format!("the script engine took longer than {:.0} s to start", ENGINE_START_LIMIT.as_secs_f64())
                        };
                        self.dead = Some(why.clone());
                        // Dropping the sender ends the thread's loop once the script is done.
                        self.worker = None;
                        return abandoned(why);
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
    use super::{Script, run_script};
    use crate::xfa::{XfaDoc, XfaOutcome};

    /// The scripts of one event, run here (no threads on wasm: a script's own limits bound it).
    pub struct XfaSession {
        doc: XfaDoc,
    }

    impl XfaSession {
        pub fn new(doc: XfaDoc) -> XfaSession {
            XfaSession { doc }
        }

        pub fn dead(&self) -> Option<&str> {
            None
        }

        pub fn run(&mut self, script: Script) -> XfaOutcome {
            run_script(script, &self.doc, &|| {})
        }
    }
}

pub use native::XfaSession;
