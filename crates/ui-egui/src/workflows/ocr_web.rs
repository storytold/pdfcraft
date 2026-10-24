//! Scan & OCR in the browser (wasm32 only).
//!
//! There are no model files to find: the first time recognition is needed the page downloads
//! them from `models/` beside itself (packaging/web/package.sh puts them there) and installs the
//! recogniser they load. Pages are then read by Web Workers (apps/pdfcraft-ocr-worker), so the
//! page keeps responding and two pages are read at once: the page renders a page, hands its
//! pixels to an idle worker and places the lines that come back. Where workers can't run (the
//! worker files are missing, the browser refuses them, they never get ready) the page reads the
//! pages itself, one per frame.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::ocr_route::{Inbox, Router};
use pdfcraft_engine::ocr::{DETECTION_MODEL, FileJob, Line, Ocr, OcrPage, OcrSettings, OcrShot, OcrSteps, RECOGNITION_MODEL};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

/// The models are about 12 MB together; a response far larger isn't one of them.
const MAX_MODEL_BYTES: u32 = 64 << 20;
/// Each worker holds the models and needs about 600 MB while it reads a page (wasm memory never
/// shrinks), so a few at most.
const MAX_WORKERS: usize = 2;
/// A worker that hasn't loaded the models by then is given up on.
const START_TIMEOUT_MS: f64 = 60_000.0;
/// Workers with nothing to do are closed after this, giving their memory back.
const IDLE_CLOSE_MS: f64 = 60_000.0;
/// The `<meta>` naming the worker's loader script (xtask/src/web.rs writes it).
const WORKER_META: &str = "meta[name=\"pdfcraft-ocr-worker\"]";

fn now() -> f64 {
    js_sys::Date::now()
}

// ---- The models -------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    NotStarted,
    Loading,
    /// Why the last download failed; [`start`] tries again.
    Failed(String),
}

static STATE: Mutex<State> = Mutex::new(State::NotStarted);

thread_local! {
    /// The downloaded model files, kept to hand to each worker.
    static MODELS: RefCell<Option<[js_sys::Uint8Array; 2]>> = const { RefCell::new(None) };
}

pub fn state() -> State {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Download and load the models unless that is already happening; repaint when it's done.
pub fn start(ctx: egui::Context) {
    {
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if *s == State::Loading {
            return;
        }
        *s = State::Loading;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let next = match load().await {
            Ok((ocr, files)) => {
                pdfcraft_engine::ocr::install(ocr);
                MODELS.with_borrow_mut(|m| *m = Some(files));
                State::NotStarted
            }
            Err(e) => State::Failed(e),
        };
        *STATE.lock().unwrap_or_else(|e| e.into_inner()) = next;
        ctx.request_repaint();
    });
}

async fn load() -> Result<(Ocr, [js_sys::Uint8Array; 2]), String> {
    let detection = fetch(DETECTION_MODEL).await?;
    let recognition = fetch(RECOGNITION_MODEL).await?;
    let ocr = Ocr::from_bytes(detection.to_vec(), recognition.to_vec()).map_err(|e| e.to_string())?;
    Ok((ocr, [detection, recognition]))
}

/// `models/<name>`, relative to the page, so the site works under any path.
async fn fetch(name: &str) -> Result<js_sys::Uint8Array, String> {
    use wasm_bindgen_futures::JsFuture;
    let url = format!("models/{name}");
    let failed = |e: JsValue| format!("{url}: {}", e.as_string().unwrap_or_else(|| format!("{e:?}")));
    let window = web_sys::window().ok_or("no window")?;
    let resp = JsFuture::from(window.fetch_with_str(&url)).await.map_err(failed)?;
    let resp: web_sys::Response = resp.dyn_into().map_err(failed)?;
    if !resp.ok() {
        return Err(format!("{url}: HTTP {}", resp.status()));
    }
    let buf = JsFuture::from(resp.array_buffer().map_err(failed)?).await.map_err(failed)?;
    let bytes = js_sys::Uint8Array::new(&buf);
    if bytes.length() > MAX_MODEL_BYTES {
        return Err(format!("{url}: {} bytes is too large for a model", bytes.length()));
    }
    Ok(bytes)
}

// ---- The workers ------------------------------------------------------------------------------

enum Slot {
    Starting,
    Loading,
    Idle,
    /// Reading the page sent under this id.
    Busy(u64),
    Dead,
}

/// What a worker said (see apps/pdfcraft-ocr-worker for the messages).
enum Event {
    Ready,
    Loaded,
    Lines(u64, Result<Vec<Line>, String>),
    Failed(Option<u64>, String),
    Died,
}

struct Member {
    worker: web_sys::Worker,
    slot: Slot,
    _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _on_error: Closure<dyn FnMut(JsValue)>,
}

struct Pool {
    members: Vec<Member>,
    events: Rc<RefCell<Vec<(usize, Event)>>>,
    started: f64,
    last_used: f64,
    /// Which run each page sent belongs to: single-file and multiple-file OCR share the workers.
    router: Router<Result<Vec<Line>, String>>,
}

thread_local! {
    static POOL: RefCell<Option<Pool>> = const { RefCell::new(None) };
}

/// How many workers to start: one core stays with the page.
fn worker_count() -> usize {
    let cores = web_sys::window().map_or(1.0, |w| w.navigator().hardware_concurrency());
    let cores = if cores.is_finite() && cores >= 1.0 { cores as usize } else { 1 };
    cores.saturating_sub(1).clamp(1, MAX_WORKERS)
}

fn field(data: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(data, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

fn message(fields: &[(&str, &JsValue)]) -> JsValue {
    let o = js_sys::Object::new();
    for (k, v) in fields {
        let _ = js_sys::Reflect::set(&o, &(*k).into(), v);
    }
    o.into()
}

impl Event {
    fn parse(data: &JsValue) -> Event {
        let id = field(data, "id").as_f64().filter(|v| v.fract() == 0.0 && *v >= 0.0).map(|v| v as u64);
        let text = |key: &str| field(data, key).as_string().unwrap_or_default();
        match field(data, "type").as_string().as_deref() {
            Some("ready") => Event::Ready,
            Some("loaded") => Event::Loaded,
            Some("lines") => match id {
                Some(id) => Event::Lines(id, pdfcraft_engine::ocr::lines_from_json(&text("lines")).map_err(|e| e.to_string())),
                None => Event::Failed(None, "a result without its page".into()),
            },
            Some("failed") => Event::Failed(id, text("error")),
            _ => Event::Failed(id, "an unknown message".into()),
        }
    }
}

impl Pool {
    /// Start the workers. None start (and the page reads by itself) when the page doesn't name
    /// the worker script or the browser refuses it.
    fn start(ctx: &egui::Context) -> Pool {
        let t = now();
        let mut pool = Pool { members: Vec::new(), events: Rc::new(RefCell::new(Vec::new())), started: t, last_used: t, router: Router::default() };
        let url = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.query_selector(WORKER_META).ok().flatten())
            .and_then(|m| m.get_attribute("content"));
        let Some(url) = url else { return pool };
        let options = web_sys::WorkerOptions::new();
        options.set_type(web_sys::WorkerType::Module);
        for i in 0..worker_count() {
            let Ok(worker) = web_sys::Worker::new_with_options(&url, &options) else { break };
            let (events, ctx2) = (pool.events.clone(), ctx.clone());
            let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                events.borrow_mut().push((i, Event::parse(&e.data())));
                ctx2.request_repaint();
            });
            let (events, ctx2) = (pool.events.clone(), ctx.clone());
            let on_error = Closure::<dyn FnMut(JsValue)>::new(move |_| {
                events.borrow_mut().push((i, Event::Died));
                ctx2.request_repaint();
            });
            worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
            pool.members.push(Member { worker, slot: Slot::Starting, _on_message: on_message, _on_error: on_error });
        }
        pool
    }

    /// Whether any worker is, or may yet be, able to read pages.
    fn usable(&self) -> bool {
        self.members.iter().any(|m| !matches!(m.slot, Slot::Dead))
    }

    fn idle(&self) -> Option<usize> {
        self.members.iter().position(|m| matches!(m.slot, Slot::Idle))
    }

    fn busy(&self) -> bool {
        self.members.iter().any(|m| matches!(m.slot, Slot::Busy(_)))
    }

    fn kill(&mut self, i: usize) {
        if let Some(m) = self.members.get_mut(i) {
            m.worker.terminate();
            m.slot = Slot::Dead;
        }
    }

    /// Act on what the workers said; the pages they finished go to the inboxes of the runs that
    /// sent them.
    fn update(&mut self) {
        let mut done = Vec::new();
        let events: Vec<_> = self.events.borrow_mut().drain(..).collect();
        for (i, event) in events {
            let Some(m) = self.members.get_mut(i) else { continue };
            let busy = if let Slot::Busy(id) = m.slot { Some(id) } else { None };
            match event {
                Event::Ready if matches!(m.slot, Slot::Starting) => {
                    let sent = MODELS.with_borrow(|files| {
                        let [d, r] = files.as_ref()?;
                        m.worker.post_message(&message(&[("type", &"models".into()), ("detection", d), ("recognition", r)])).ok()
                    });
                    if sent.is_some() {
                        m.slot = Slot::Loading;
                    } else {
                        self.kill(i);
                    }
                }
                Event::Loaded if matches!(m.slot, Slot::Loading) => m.slot = Slot::Idle,
                Event::Lines(id, lines) if busy == Some(id) => {
                    m.slot = Slot::Idle;
                    done.push((id, lines));
                }
                Event::Failed(Some(id), e) if busy == Some(id) => {
                    m.slot = Slot::Idle;
                    done.push((id, Err(e)));
                }
                // A worker that can't load the models, or fails other than on its page, is out.
                Event::Failed(..) | Event::Died => {
                    if let Some(id) = busy {
                        done.push((id, Err("the text recognition worker stopped".into())));
                    }
                    self.kill(i);
                }
                // Stale (a page from a cancelled run) or out of order: nothing to do.
                _ => {}
            }
        }
        if now() - self.started > START_TIMEOUT_MS {
            for i in 0..self.members.len() {
                if self.members.get(i).is_some_and(|m| matches!(m.slot, Slot::Starting | Slot::Loading)) {
                    self.kill(i);
                }
            }
        }
        for (id, lines) in done {
            self.router.deliver(id, lines);
        }
    }

    /// Hand `shot` to worker `i` for the run reading into `inbox`; its id, or `None` if it
    /// couldn't be sent.
    fn send(&mut self, i: usize, shot: &OcrShot, inbox: &Inbox<Result<Vec<Line>, String>>) -> Option<u64> {
        let m = self.members.get_mut(i)?;
        let id = self.router.send(inbox, |id| {
            let pixels = js_sys::Uint8Array::from(&shot.rgba[..]);
            let msg = message(&[
                ("type", &"page".into()),
                ("id", &JsValue::from_f64(id as f64)),
                ("width", &shot.width.into()),
                ("height", &shot.height.into()),
                ("pixels", &pixels),
            ]);
            // The pixels move to the worker rather than being copied again.
            m.worker.post_message_with_transfer(&msg, &js_sys::Array::of1(&pixels.buffer())).is_ok()
        })?;
        m.slot = Slot::Busy(id);
        self.last_used = now();
        Some(id)
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        for m in &self.members {
            m.worker.terminate();
        }
    }
}

/// Close the workers once they have been idle a while (called when no recognition is running).
/// The page only draws a frame when something happens, so one is asked for when the idle time
/// runs out: otherwise the workers' memory stays taken until the next click or key.
pub fn close_idle_workers(ctx: &egui::Context) {
    POOL.with_borrow_mut(|pool| {
        // Answers still arriving for a cancelled run free their workers (and are dropped).
        if let Some(p) = pool.as_mut() {
            p.update();
        }
        let Some(p) = pool.as_ref().filter(|p| !p.busy()) else { return };
        let idle = now() - p.last_used;
        if idle > IDLE_CLOSE_MS {
            *pool = None;
        } else {
            // `as` saturates (and a NaN becomes 0); a little past the limit, so the check passes.
            let left = (IDLE_CLOSE_MS - idle).max(0.0) as u64;
            ctx.request_repaint_after(std::time::Duration::from_millis(left.saturating_add(50)));
        }
    });
}

// ---- Reading pages ----------------------------------------------------------------------------

/// One run of [`OcrSteps`] through the workers, advanced a step per frame by [`Pages::pump`].
pub struct Pages {
    steps: OcrSteps,
    /// Pages with a worker: id, position in the run, the page (its pixels gone to the worker).
    reading: Vec<(u64, usize, OcrShot)>,
    /// Where the workers' answers for this run's pages arrive (dropped with the run, so a
    /// cancelled run's late answers are let go).
    inbox: Inbox<Result<Vec<Line>, String>>,
    found: Vec<(usize, OcrPage)>,
    next: usize,
    rendered_all: bool,
}

impl Pages {
    pub fn new(steps: OcrSteps) -> Pages {
        Pages { steps, reading: Vec::new(), inbox: Inbox::default(), found: Vec::new(), next: 0, rendered_all: false }
    }

    pub fn total(&self) -> usize {
        self.steps.total()
    }

    pub fn done(&self) -> usize {
        self.found.len()
    }

    /// Collect what the workers read and hand out the next page (rendering at most one a call,
    /// so a frame stays short). Returns the pages, in order, once all are read or `cancel` is set
    /// (the pages read so far; answers still coming are ignored).
    pub fn pump(&mut self, cancel: bool, ctx: &egui::Context) -> Option<Vec<OcrPage>> {
        POOL.with_borrow_mut(|pool| {
            let pool = pool.get_or_insert_with(|| Pool::start(ctx));
            pool.update();
            for (id, lines) in self.inbox.take() {
                if let Some(k) = self.reading.iter().position(|r| r.0 == id) {
                    let (_, order, shot) = self.reading.swap_remove(k);
                    self.found.push((order, shot.place(lines)));
                }
            }
            if cancel {
                return Some(self.finish());
            }
            let worker = pool.idle();
            if !self.rendered_all && (worker.is_some() || !pool.usable()) {
                match self.steps.next_page() {
                    None => self.rendered_all = true,
                    Some(Err(skipped)) => {
                        let order = self.take_order();
                        self.found.push((order, skipped));
                    }
                    Some(Ok(mut shot)) => {
                        let order = self.take_order();
                        match worker.and_then(|i| pool.send(i, &shot, &self.inbox)) {
                            Some(id) => {
                                // The worker has its own copy; drop this one.
                                shot.rgba = pdfcraft_render::Pixels::from(Vec::new());
                                self.reading.push((id, order, shot));
                            }
                            None => {
                                let lines = pdfcraft_engine::ocr::engine()
                                    .and_then(|ocr| ocr.recognize(&shot.rgba, shot.width, shot.height).map_err(|e| e.to_string()));
                                self.found.push((order, shot.place(lines)));
                            }
                        }
                    }
                }
            }
            (self.rendered_all && self.reading.is_empty()).then(|| self.finish())
        })
    }

    fn take_order(&mut self) -> usize {
        self.next += 1;
        self.next - 1
    }

    fn finish(&mut self) -> Vec<OcrPage> {
        let mut found = std::mem::take(&mut self.found);
        found.sort_by_key(|f| f.0);
        found.into_iter().map(|f| f.1).collect()
    }
}

/// Recognize text in multiple files, one file at a time through [`Pages`]; each searchable copy
/// downloads when it's done.
pub struct Batch {
    files: VecDeque<(String, Vec<u8>)>,
    settings: OcrSettings,
    current: Option<(String, FileJob, Pages)>,
    total: usize,
    pub ok: usize,
    pub words: usize,
    pub failed: Vec<String>,
}

impl Batch {
    pub fn new(files: Vec<(String, Vec<u8>)>, settings: OcrSettings) -> Batch {
        let total = files.len();
        Batch { files: files.into(), settings, current: None, total, ok: 0, words: 0, failed: Vec::new() }
    }

    /// Files finished so far.
    pub fn done(&self) -> usize {
        self.total.saturating_sub(self.files.len() + usize::from(self.current.is_some()))
    }

    /// Advance; `true` once every file is done.
    pub fn pump(&mut self, ctx: &egui::Context) -> bool {
        let Some((name, _, pages)) = self.current.as_mut() else {
            let Some((name, bytes)) = self.files.pop_front() else { return true };
            match FileJob::open(&name, Arc::new(bytes), None, self.settings.clone()) {
                Ok(mut file) => match file.job.take() {
                    Some(job) => self.current = Some((name, file, Pages::new(job.steps()))),
                    None => self.failed.push(format!("{name}: the document could not be read")),
                },
                Err(e) => self.failed.push(format!("{name}: {e}")),
            }
            return false;
        };
        let Some(found) = pages.pump(false, ctx) else { return false };
        let name = std::mem::take(name);
        let Some((_, file, _)) = self.current.take() else { return false };
        let saved = file.finish(found).and_then(|r| {
            crate::editing::download(&name, &r.bytes)?;
            Ok(r.words())
        });
        match saved {
            Ok(n) => {
                self.ok += 1;
                self.words += n;
            }
            Err(e) => self.failed.push(format!("{name}: {e}")),
        }
        false
    }
}
