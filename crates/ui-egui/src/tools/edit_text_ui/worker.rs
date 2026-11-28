//! One bounded draft worker per view. Queueing replaces the pending request; results are
//! published only while their serial is current. No thread is spawned per keystroke.
use pdfcraft_engine::{BlockStyle, TextDraftSession, TextDraftSource, TextEditLayout};
use pdfcraft_render::RenderedPage;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

pub(super) struct Request {
    pub source: Arc<TextDraftSource>,
    pub text: String,
    pub style: BlockStyle,
    pub scale: f32,
}

pub(super) struct Output {
    pub layout: TextEditLayout,
    pub page: RenderedPage,
}

pub(super) struct Reply {
    pub serial: u64,
    pub result: Result<Output, String>,
}

#[derive(Default)]
struct Queue {
    serial: u64,
    request: Option<(u64, Request)>,
    reply: Option<Reply>,
    shutdown: bool,
    /// The editor closed: drop the session (and the document snapshot it holds) once the work
    /// in flight is done.
    release: bool,
    #[cfg(test)]
    paused: bool,
    #[cfg(test)]
    prepared: usize,
    #[cfg(test)]
    pause_publish: bool,
    #[cfg(test)]
    publishing: bool,
}

pub(super) struct DraftWorker {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    #[cfg(target_arch = "wasm32")]
    session: Option<(Arc<TextDraftSource>, TextDraftSession)>,
}

fn prepare(session: &mut Option<(Arc<TextDraftSource>, TextDraftSession)>, request: &Request) -> Result<pdfcraft_engine::PreparedTextDraft, String> {
    if session.as_ref().is_none_or(|(source, _)| !Arc::ptr_eq(source, &request.source)) {
        *session = Some((request.source.clone(), request.source.open()?));
    }
    let (_, session) = session.as_ref().ok_or("the draft session could not be opened")?;
    session.prepare(&request.text, &request.style)
}

impl DraftWorker {
    pub fn new(ctx: &egui::Context) -> Result<Self, String> {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let shared = queue.clone();
            let ctx = ctx.clone();
            std::thread::Builder::new()
                .name("pdfcraft-text-draft".into())
                .spawn(move || run(shared, ctx))
                .map_err(|e| format!("could not start the text preview worker: {e}"))?;
        }
        #[cfg(target_arch = "wasm32")]
        let _ = ctx;
        Ok(Self {
            queue,
            #[cfg(target_arch = "wasm32")]
            session: None,
        })
    }

    #[cfg(test)]
    pub fn pause(&self, paused: bool) {
        self.queue.0.lock().unwrap_or_else(PoisonError::into_inner).paused = paused;
        self.queue.1.notify_one();
    }

    #[cfg(test)]
    pub fn queued(&self) -> (Option<String>, usize) {
        let queue = self.queue.0.lock().unwrap_or_else(PoisonError::into_inner);
        (queue.request.as_ref().map(|(_, request)| request.text.clone()), queue.prepared)
    }

    #[cfg(test)]
    pub fn pause_publication(&self, paused: bool) {
        self.queue.0.lock().unwrap_or_else(PoisonError::into_inner).pause_publish = paused;
        self.queue.1.notify_one();
    }

    #[cfg(test)]
    pub fn publishing(&self) -> bool {
        self.queue.0.lock().unwrap_or_else(PoisonError::into_inner).publishing
    }

    pub fn submit(&mut self, request: Request) -> u64 {
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
        queue.serial = queue.serial.wrapping_add(1);
        let serial = queue.serial;
        queue.reply = None;
        queue.request = Some((serial, request));
        ready.notify_one();
        serial
    }

    pub fn cancel(&mut self) {
        let mut queue = self.queue.0.lock().unwrap_or_else(PoisonError::into_inner);
        queue.serial = queue.serial.wrapping_add(1);
        queue.request = None;
        queue.reply = None;
    }

    /// Cancel, and release the session of the paragraph that was open: the worker stays for the
    /// next paragraph, but the closed one's document snapshot must not stay with it.
    pub fn release(&mut self) {
        self.cancel();
        #[cfg(target_arch = "wasm32")]
        {
            self.session = None;
        }
        self.queue.0.lock().unwrap_or_else(PoisonError::into_inner).release = true;
        self.queue.1.notify_one();
    }

    pub fn poll(&mut self) -> Option<Reply> {
        // Keep the existing single-threaded web build functional. Native preparation and
        // rendering both run on the worker; the web build still performs one combined pass.
        #[cfg(target_arch = "wasm32")]
        if let Some((serial, request)) = self.queue.0.lock().unwrap_or_else(PoisonError::into_inner).request.take() {
            let result = pdfcraft_engine::guard(|| {
                let draft = prepare(&mut self.session, &request)?;
                let page = draft.preview.render_background(request.scale)?;
                Ok(Output { layout: draft.layout, page })
            })
            .and_then(|result| result);
            return Some(Reply { serial, result });
        }
        self.queue.0.lock().unwrap_or_else(PoisonError::into_inner).reply.take()
    }
}

impl Drop for DraftWorker {
    fn drop(&mut self) {
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
        queue.shutdown = true;
        queue.request = None;
        queue.reply = None;
        ready.notify_one();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn paused(queue: &Queue) -> bool {
    #[cfg(test)]
    {
        queue.paused
    }
    #[cfg(not(test))]
    {
        let _ = queue;
        false
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run(shared: Arc<(Mutex<Queue>, Condvar)>, ctx: egui::Context) {
    let mut session = None;
    loop {
        let (serial, request) = {
            let (lock, ready) = &*shared;
            let mut queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
            while (queue.request.is_none() || paused(&queue)) && !queue.shutdown && !queue.release {
                queue = ready.wait(queue).unwrap_or_else(PoisonError::into_inner);
            }
            if queue.shutdown {
                return;
            }
            if std::mem::take(&mut queue.release) {
                drop(queue);
                session = None;
                continue;
            }
            let Some(request) = queue.request.take() else { continue };
            request
        };
        #[cfg(test)]
        {
            shared.0.lock().unwrap_or_else(PoisonError::into_inner).prepared += 1;
        }
        let result = pdfcraft_engine::guard(|| {
            let draft = prepare(&mut session, &request)?;
            // Superseded drafts need neither a raster nor an upload to the GPU.
            let queue = shared.0.lock().unwrap_or_else(PoisonError::into_inner);
            if queue.shutdown || queue.serial != serial {
                return Ok(None);
            }
            drop(queue);
            let page = draft.preview.render_background(request.scale)?;
            Ok(Some(Output { layout: draft.layout, page }))
        })
        .and_then(|result| result);
        if result.is_err() {
            session = None;
        }
        let mut queue = shared.0.lock().unwrap_or_else(PoisonError::into_inner);
        #[cfg(test)]
        {
            queue.publishing = true;
            while queue.pause_publish && !queue.shutdown {
                queue = shared.1.wait(queue).unwrap_or_else(PoisonError::into_inner);
            }
            queue.publishing = false;
        }
        if !queue.shutdown && queue.serial == serial {
            match result {
                Ok(Some(output)) => queue.reply = Some(Reply { serial, result: Ok(output) }),
                Err(error) => queue.reply = Some(Reply { serial, result: Err(error) }),
                Ok(None) => continue,
            }
            // Do not hold the queue lock while entering egui's context.
            drop(queue);
            ctx.request_repaint();
        }
    }
}
