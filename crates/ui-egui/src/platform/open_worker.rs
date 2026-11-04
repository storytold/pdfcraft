//! Native document reads and preparation run away from the frame thread.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use crate::RecoveryStore;
use pdfcraft_engine::{OpenContext, PreparedDocument, SourceKind};
use pdfcraft_render::OpenError;

const MAX_QUEUED_OPENS: usize = 16;

pub(crate) enum Input {
    Path(PathBuf),
    RemotePath(PathBuf),
    Recovery(RecoveryStore, String),
    Bytes(Arc<Vec<u8>>),
}

pub(crate) enum ResultKind {
    Conversion(Arc<Vec<u8>>),
    /// Office document already converted to PDF bytes on a background thread.
    ConvertedPdf(Arc<Vec<u8>>),
    Prepared { bytes: Arc<Vec<u8>>, result: Box<Result<PreparedDocument, OpenError>> },
    ReadFailed(String),
    RemoteFailed(String),
    RecoveryFailed(String),
    Failed(String),
}

pub(crate) struct ResultMessage {
    pub token: u64,
    pub kind: ResultKind,
}

struct Job {
    token: u64,
    context: OpenContext,
    name: String,
    path: Option<String>,
    password: Option<String>,
    input: Input,
    reply: Sender<ResultMessage>,
    #[cfg(test)]
    gate: Option<Arc<PrepareGate>>,
}

struct Queue {
    jobs: VecDeque<Job>,
    shutdown: bool,
    repaint: Option<egui::Context>,
}

pub(crate) struct OpenWorker {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    #[cfg(test)]
    gate: Option<Arc<PrepareGate>>,
}

impl OpenWorker {
    pub fn new(#[cfg(test)] gate: Option<Arc<PrepareGate>>) -> std::io::Result<Self> {
        let queue = Arc::new((Mutex::new(Queue { jobs: VecDeque::new(), shutdown: false, repaint: None }), Condvar::new()));
        let worker_queue = queue.clone();
        std::thread::Builder::new().name("pdfcraft-open".into()).spawn(move || run(worker_queue))?;
        Ok(Self {
            queue,
            #[cfg(test)]
            gate,
        })
    }

    pub fn set_context(&self, ctx: &egui::Context) {
        let (lock, _) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
        queue.repaint = Some(ctx.clone());
    }

    pub fn submit(
        &self,
        token: u64,
        context: OpenContext,
        name: String,
        path: Option<String>,
        password: Option<String>,
        input: Input,
    ) -> Result<Receiver<ResultMessage>, String> {
        let (reply, receiver) = mpsc::channel();
        let job = Job {
            token,
            context,
            name,
            path,
            password,
            input,
            reply,
            #[cfg(test)]
            gate: self.gate.clone(),
        };
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.shutdown {
            return Err("the document-open worker has stopped".into());
        }
        if queue.jobs.len() >= MAX_QUEUED_OPENS {
            return Err("too many documents are waiting to open".into());
        }
        queue.jobs.push_back(job);
        ready.notify_one();
        Ok(receiver)
    }
}

impl Drop for OpenWorker {
    fn drop(&mut self) {
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
        queue.shutdown = true;
        queue.jobs.clear();
        queue.repaint = None;
        ready.notify_one();
    }
}

fn run(queue: Arc<(Mutex<Queue>, Condvar)>) {
    loop {
        let job = {
            let (lock, ready) = &*queue;
            let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
            while state.jobs.is_empty() && !state.shutdown {
                state = ready.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
            if state.shutdown {
                return;
            }
            state.jobs.pop_front()
        };
        let Some(job) = job else { continue };
        #[cfg(test)]
        if let Some(gate) = &job.gate {
            gate.pause_before_prepare();
        }
        let token = job.token;
        let reply = job.reply.clone();
        #[cfg(test)]
        let gate = job.gate.clone();
        let queue_for_office = queue.clone();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prepare(job)))
            .unwrap_or_else(|_| PrepareOutcome::Ready(ResultKind::Failed("opening failed unexpectedly in the background worker".into())));
        match outcome {
            PrepareOutcome::Ready(kind) => {
                let _ = reply.send(ResultMessage { token, kind });
                let repaint = {
                    let (lock, _) = &*queue;
                    lock.lock().unwrap_or_else(PoisonError::into_inner).repaint.clone()
                };
                if let Some(ctx) = repaint {
                    ctx.request_repaint();
                }
                #[cfg(test)]
                if let Some(gate) = &gate {
                    let _ = gate.finished.send(());
                }
            }
            PrepareOutcome::ConvertOffice { name, bytes } => {
                let spawn_res = std::thread::Builder::new().name("pdfcraft-office".into()).spawn(move || {
                    let kind = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pdfcraft_engine::office::convert(&name, &bytes))) {
                        Ok(Ok(pdf)) => ResultKind::ConvertedPdf(Arc::new(pdf)),
                        Ok(Err(error)) => ResultKind::Failed(error.to_string()),
                        Err(_) => ResultKind::Failed("opening failed unexpectedly in the background worker".into()),
                    };
                    let _ = reply.send(ResultMessage { token, kind });
                    let repaint = {
                        let (lock, _) = &*queue_for_office;
                        lock.lock().unwrap_or_else(PoisonError::into_inner).repaint.clone()
                    };
                    if let Some(ctx) = repaint {
                        ctx.request_repaint();
                    }
                    #[cfg(test)]
                    if let Some(gate) = &gate {
                        let _ = gate.finished.send(());
                    }
                });
                if let Err(e) = spawn_res {
                    let kind = ResultKind::Failed(format!("Office conversion worker could not start: {e}"));
                    let _ = reply.send(ResultMessage { token, kind });
                    let repaint = {
                        let (lock, _) = &*queue;
                        lock.lock().unwrap_or_else(PoisonError::into_inner).repaint.clone()
                    };
                    if let Some(ctx) = repaint {
                        ctx.request_repaint();
                    }
                    #[cfg(test)]
                    if let Some(gate) = &gate {
                        let _ = gate.finished.send(());
                    }
                }
            }
        }
    }
}

enum PrepareOutcome {
    Ready(ResultKind),
    ConvertOffice { name: String, bytes: Arc<Vec<u8>> },
}

fn prepare(job: Job) -> PrepareOutcome {
    let bytes = match &job.input {
        Input::Path(path) => match std::fs::read(path) {
            Ok(bytes) => Arc::new(bytes),
            Err(error) => return PrepareOutcome::Ready(ResultKind::ReadFailed(error.to_string())),
        },
        Input::RemotePath(path) => match pdfcraft_engine::read_remote_document(path) {
            Ok(bytes) => Arc::new(bytes),
            Err(error) => return PrepareOutcome::Ready(ResultKind::RemoteFailed(error.to_string())),
        },
        Input::Recovery(store, key) => match store.read(key) {
            Ok(bytes) => Arc::new(bytes),
            Err(error) => return PrepareOutcome::Ready(ResultKind::RecoveryFailed(error.to_string())),
        },
        Input::Bytes(bytes) => bytes.clone(),
    };
    match pdfcraft_engine::source_kind(&job.name, &bytes) {
        Some(SourceKind::Image | SourceKind::Text) => return PrepareOutcome::Ready(ResultKind::Conversion(bytes)),
        Some(SourceKind::Office) => return PrepareOutcome::ConvertOffice { name: job.name, bytes },
        _ => {}
    }
    let result = job.context.prepare_open(job.name, job.path, bytes.clone(), job.password.as_deref());
    PrepareOutcome::Ready(ResultKind::Prepared { bytes, result: Box::new(result) })
}

#[cfg(test)]
pub(crate) struct PrepareGate {
    pub started: Sender<()>,
    release: Mutex<Receiver<()>>,
    pub finished: Sender<()>,
}

#[cfg(test)]
impl PrepareGate {
    pub fn pause_before_prepare(&self) {
        let _ = self.started.send(());
        if let Ok(release) = self.release.lock() {
            let _ = release.recv();
        }
    }
}

#[cfg(test)]
pub(crate) fn prepare_gate() -> (Arc<PrepareGate>, Receiver<()>, Sender<()>, Receiver<()>) {
    let (started, started_rx) = mpsc::channel();
    let (release_tx, release) = mpsc::channel();
    let (finished, finished_rx) = mpsc::channel();
    (Arc::new(PrepareGate { started, release: Mutex::new(release), finished }), started_rx, release_tx, finished_rx)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
