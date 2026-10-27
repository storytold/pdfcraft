use super::*;

/// How long one page may render before the pool gives up on it (the watchdog).
pub const STUCK_AFTER: std::time::Duration = std::time::Duration::from_secs(20);

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum BusyStage {
    /// Parsing the document, shared or private. Not timed: a huge or damaged file is slow to
    /// open, not stuck, and giving up on it would fail every page of the pool. (Parsing has
    /// its own limits.) Rendering is timed from the moment parsing ends.
    Parsing,
    SharedRender,
    Private,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub(super) struct Busy {
    pub(super) request: RenderRequest,
    since: std::time::Instant,
    stage: BusyStage,
    pub(super) cancelled: Arc<AtomicBool>,
    valid: Arc<AtomicBool>,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[derive(Clone)]
pub(super) struct BusyPublishGate {
    pub(super) request: RenderRequest,
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: Arc<Mutex<Receiver<()>>>,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[derive(Clone)]
pub(super) struct RenderGate {
    pub(super) page: usize,
    pub(super) shared_generation: bool,
    pub(super) claimed: Arc<std::sync::atomic::AtomicBool>,
    pub(super) entered: Arc<std::sync::Barrier>,
    pub(super) release: Arc<std::sync::Barrier>,
    pub(super) finished: Arc<std::sync::Barrier>,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[derive(Clone)]
pub(super) struct ParseGate {
    pub(super) entered: Arc<std::sync::Barrier>,
    pub(super) release: Arc<std::sync::Barrier>,
}

/// State shared by the pool and its workers.
#[derive(Default)]
pub(super) struct Shared {
    /// Desired requests, active jobs and bounded completed results share one deduplication key.
    pub(super) queue: Arc<WorkQueue>,
    /// Per worker id: the request it is rendering and since when.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) busy: Mutex<Vec<Option<Busy>>>,
    /// Exact requests the watchdog gave up on: answered with an error at once, so a pathological
    /// request cannot trap every worker in turn while other scales, tiles, or generations remain usable.
    pub(super) stuck: Mutex<std::collections::HashSet<RequestFailureKey>>,
    stats: AtomicRenderStats,
    /// Per worker id: set to stop that worker's render at its next content operator once nobody
    /// can receive its answer (the pool was dropped, or the watchdog gave up on the render).
    /// Lock `busy` first when holding both.
    pub(super) stop: Mutex<Vec<Arc<std::sync::atomic::AtomicBool>>>,
    /// Test hook: hold one worker at the render boundary until the test releases it.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(super) render_gates: Mutex<Vec<RenderGate>>,
    /// Test hook: hold parser initialization until the test releases it.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(super) parse_gate: Mutex<Option<ParseGate>>,
    /// Test hook: make rendering this page take this long (cancellable).
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(super) slow_page: Mutex<Option<(usize, std::time::Duration)>>,
    /// Test hook: make parser initialization take this long.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(super) slow_parse: Mutex<Option<std::time::Duration>>,
    /// Test hook: count parser initializations, including failed parses.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(super) parses: std::sync::atomic::AtomicUsize,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) parser: PoolParser,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(super) panic_page: Mutex<Option<usize>>,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(super) render_started: std::sync::atomic::AtomicUsize,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(super) busy_publish_gate: Mutex<Option<BusyPublishGate>>,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(super) parser_stage_entries: Mutex<Vec<RenderRequest>>,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    shared_render_delay: Mutex<Option<std::time::Duration>>,
}

pub(super) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn same_request(a: RenderRequest, b: RenderRequest) -> bool {
    // `preview` is excluded on purpose: it names the same work for cancellation, only the
    // detail level of the result differs.
    a.page == b.page && a.kind == b.kind && a.tile == b.tile && a.scale.to_bits() == b.scale.to_bits() && a.tag == b.tag
}

#[cfg(not(target_arch = "wasm32"))]
impl Shared {
    fn cancel_in_flight(&self, obsolete: &[RenderRequest]) {
        let busy = lock(&self.busy);
        for busy in busy.iter().flatten() {
            if obsolete.iter().copied().any(|request| same_request(request, busy.request)) {
                busy.cancelled.store(true, Ordering::Release);
                busy.valid.store(false, Ordering::Release);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn set_busy_stage(shared: &Shared, id: usize, stage: BusyStage) -> bool {
    let mut slots = lock(&shared.busy);
    let Some(Some(busy)) = slots.get_mut(id) else { return false };
    if busy.stage != stage {
        busy.stage = stage;
        busy.since = std::time::Instant::now();
    }
    true
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(super) enum ParserState {
    #[default]
    Empty,
    Loading,
    Ready(Option<Arc<Pdf>>),
    /// A page failed after initialization: isolate subsequent work per worker.
    Private,
    /// Retired before it became ready (defensive: parsing is not timed, and only a shared
    /// render retires the parser). Retrying it on every worker cannot help.
    Failed,
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct PoolParser {
    pub(super) state: Mutex<ParserState>,
    ready: std::sync::Condvar,
    pub(super) valid: Arc<AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for PoolParser {
    fn default() -> Self {
        Self { state: Mutex::default(), ready: std::sync::Condvar::new(), valid: Arc::new(AtomicBool::new(true)) }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl PoolParser {
    /// `None` means use a private parser. `Some(None)` is a terminal parse failure.
    pub(super) fn acquire(&self, load: impl FnOnce() -> Option<Pdf>, cancelled: impl Fn() -> bool) -> Option<Option<Arc<Pdf>>> {
        let mut state = lock(&self.state);
        loop {
            if cancelled() {
                return Some(None);
            }
            match &*state {
                ParserState::Ready(pdf) => return Some(pdf.clone()),
                ParserState::Private => return None,
                ParserState::Failed => return Some(None),
                ParserState::Loading => {
                    // Drop never joins a parser. A waiting worker must still notice
                    // its pool's disconnected wake channel while another thread is
                    // stuck in initialization and cannot signal this condition.
                    state = self.ready.wait_timeout(state, std::time::Duration::from_millis(50)).unwrap_or_else(std::sync::PoisonError::into_inner).0;
                }
                ParserState::Empty => {
                    *state = ParserState::Loading;
                    break;
                }
            }
        }
        drop(state);
        // Parsing cannot hold the lifecycle mutex: watchdog/drop must remain nonblocking.
        // Even a panic in initialization publishes one terminal failure and wakes waiters.
        let pdf = catch_unwind(AssertUnwindSafe(load)).ok().flatten().map(Arc::new);
        let mut state = lock(&self.state);
        let result = match &*state {
            ParserState::Loading => {
                *state = ParserState::Ready(pdf.clone());
                Some(pdf)
            }
            ParserState::Private => None,
            _ => Some(None), // the watchdog retired initialization while it ran
        };
        self.ready.notify_all();
        result
    }

    pub(super) fn abandon(&self) {
        self.retire();
    }

    /// After a shared render failed or timed out: later work parses privately, per worker
    /// (for the rest of the pool's life: the old one-parser-per-worker memory use). A parser
    /// that never became ready stays failed.
    fn retire(&self) {
        let mut state = lock(&self.state);
        self.valid.store(false, Ordering::Release);
        *state = match &*state {
            ParserState::Loading | ParserState::Empty | ParserState::Failed => ParserState::Failed,
            _ => ParserState::Private,
        };
        self.ready.notify_all();
    }
}

/// Renders pages on worker threads, most urgent request first.
///
/// **Watchdog:** a render running longer than `stuck_after` (default [`STUCK_AFTER`]) is given
/// up on. A timed-out *shared* render retires that parser and retries still-desired work
/// privately: the page may only have been waiting behind another page's decode. A private
/// timeout is reported as an error for that page and the page is not attempted again. Threads
/// cannot be killed: the stuck worker is told to stop at its next content operator, exits when
/// its render returns, and its late result is dropped; a single long operator, such as decoding
/// a huge image, still runs to its end. At most `threads` replacements are started per pool, so
/// a document full of pathological pages cannot spawn threads without bound. If every worker is
/// retired after that budget is spent, pending and future requests fail explicitly instead of
/// waiting forever.
pub struct RenderPool {
    pub(super) shared: Arc<Shared>,
    wake: Mutex<Vec<SyncSender<()>>>,
    pub(super) results: ResultReceiver,
    pub(super) results_tx: Arc<WorkQueue>,
    bytes: Arc<Vec<u8>>,
    pub(super) config: RenderConfig,
    pub(super) _workers: Mutex<Vec<JoinHandle<()>>>,
    stuck_after: std::time::Duration,
    replacements_left: Mutex<usize>,
    pub(super) retired_workers: Mutex<usize>,
    /// Used when threads are unavailable (wasm32 without atomics, or spawn failure): requests are
    /// rendered on the calling thread inside `try_recv`, one per call.
    inline: Option<std::cell::RefCell<PageRenderer>>,
}

impl RenderPool {
    pub fn new(bytes: Arc<Vec<u8>>, threads: usize, config: RenderConfig) -> Self {
        Self::with_mode(bytes, threads, config, false)
    }

    fn with_mode(bytes: Arc<Vec<u8>>, threads: usize, config: RenderConfig, inline: bool) -> Self {
        let threads = if inline || cfg!(target_arch = "wasm32") { 0 } else { threads.max(1) };
        let shared = Arc::new(Shared::default());
        let results_tx = shared.queue.clone();
        let results = ResultReceiver(shared.queue.clone());
        let mut pool = Self {
            shared,
            wake: Mutex::new(Vec::new()),
            results,
            results_tx,
            bytes: bytes.clone(),
            config: config.clone(),
            _workers: Mutex::new(Vec::new()),
            stuck_after: STUCK_AFTER,
            replacements_left: Mutex::new(threads),
            retired_workers: Mutex::new(0),
            inline: None,
        };
        for _ in 0..threads {
            pool.spawn_worker();
        }
        if lock(&pool._workers).is_empty() {
            pool.inline = Some(std::cell::RefCell::new(PageRenderer::new(bytes, config)));
        }
        pool
    }

    /// Start one worker thread; returns whether it started.
    fn spawn_worker(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        return false;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (id, stop) = {
                let mut busy = lock(&self.shared.busy);
                busy.push(None);
                let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
                lock(&self.shared.stop).push(stop.clone());
                (busy.len() - 1, stop)
            };
            let (wtx, wrx) = sync_channel::<()>(1);
            let (shared, out, bytes, config) = (self.shared.clone(), self.results_tx.clone(), self.bytes.clone(), self.config.clone());
            match std::thread::Builder::new().name(format!("pdfcraft-render-{id}")).spawn(move || worker(id, bytes, config, shared, wrx, out, stop)) {
                Ok(h) => {
                    lock(&self.wake).push(wtx);
                    lock(&self._workers).push(h);
                    true
                }
                Err(e) => {
                    log::error!("could not spawn render worker {id}: {e}");
                    false
                }
            }
        }
    }

    /// A pool without worker threads: renders inside `try_recv` (the web path; also used in tests).
    pub fn new_inline(bytes: Arc<Vec<u8>>, config: RenderConfig) -> Self {
        Self::with_mode(bytes, 0, config, true)
    }

    /// `true` when rendering happens on the caller's thread (no worker threads available).
    pub fn is_inline(&self) -> bool {
        self.inline.is_some()
    }

    /// Snapshot the work counters collected by this pool.
    pub fn stats(&self) -> RenderStats {
        self.inline.as_ref().map_or_else(|| self.shared.stats.snapshot(), |renderer| renderer.borrow().stats())
    }

    /// Change the watchdog limit (tests and benchmarks).
    pub fn set_stuck_after(&mut self, limit: std::time::Duration) {
        self.stuck_after = limit;
    }

    /// Replace desired work (most urgent first), retaining matching in-flight/completed jobs.
    /// Obsolete results are released before they reach the caller, and active obsolete renders
    /// are asked to stop at their next cancellation point.
    ///
    /// The contract: a result is delivered only while its request is in the latest queue.
    /// A caller still waiting for a page (or text, or a thumbnail) must keep listing it in
    /// every call, or the finished result is dropped to save memory.
    pub fn set_queue(&self, requests: Vec<RenderRequest>) {
        #[cfg(not(target_arch = "wasm32"))]
        let obsolete = self.shared.queue.replace(requests);
        #[cfg(target_arch = "wasm32")]
        let _ = self.shared.queue.replace(requests);
        #[cfg(not(target_arch = "wasm32"))]
        self.shared.cancel_in_flight(&obsolete);
        self.wake_workers();
    }

    fn wake_workers(&self) {
        for w in lock(&self.wake).iter() {
            // One pending wakeup is sufficient; UI polling must not grow wake channels.
            let _ = w.try_send(());
        }
    }

    pub fn try_recv(&self) -> Option<RenderedPage> {
        if let Some(r) = &self.inline {
            let req = self.shared.queue.pop()?;
            let page = r.borrow_mut().render(req);
            self.shared.queue.finish_inline(req);
            return Some(page);
        }
        self.watchdog();
        let page = self.results.try_recv().ok();
        // Consuming a result releases byte/count capacity. An invalid parser generation
        // can also requeue work without returning a page, so wake in both cases.
        self.wake_workers();
        page.or_else(|| {
            let workers = lock(&self._workers).len();
            if workers == 0 || *lock(&self.retired_workers) < workers {
                return None;
            }
            // Count successful handles, not reserved busy slots: thread spawning
            // can fail. No worker remains able to service these pending requests.
            let request = self.shared.queue.pop()?;
            self.shared.queue.finish_inline(request);
            Some(RenderedPage {
                request,
                width: 0,
                height: 0,
                rgba: Pixels::default(),
                error: Some("all render workers exceeded their time limits; close and reopen the document to try again".into()),
                text: None,
                warnings: Vec::new(),
                millis: 0,
            })
        })
    }

    /// Give up on renders that exceeded `stuck_after` (see the type docs).
    pub(super) fn watchdog(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut gave_up = Vec::new();
            {
                let mut busy = lock(&self.shared.busy);
                // Lock order: `busy`, then `stop`.
                let stop = lock(&self.shared.stop);
                for (id, slot) in busy.iter_mut().enumerate() {
                    if let Some(busy) = slot.clone()
                        && busy.stage != BusyStage::Parsing
                        && busy.since.elapsed() > self.stuck_after
                    {
                        *slot = None; // the worker sees this and exits when it returns
                        // Told to stop at its next operator; its replacement retries the page.
                        busy.cancelled.store(true, Ordering::Release);
                        busy.valid.store(false, Ordering::Release);
                        if let Some(s) = stop.get(id) {
                            s.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        gave_up.push(busy);
                    }
                }
            }
            {
                let mut retired = lock(&self.retired_workers);
                *retired = retired.saturating_add(gave_up.len());
            }
            for busy in gave_up {
                let req = busy.request;
                self.shared.parser.retire();
                if busy.stage == BusyStage::SharedRender {
                    // The shared cold-decode gate can make a healthy page wait
                    // behind another page. Only its private retry can attribute
                    // the timeout to this page; do not blacklist it here.
                    self.shared.queue.retry(req);
                } else {
                    lock(&self.shared.stuck).insert(req.into());
                    let what = if req.kind == RequestKind::Text { "text extraction for page" } else { "page" };
                    let error = format!(
                        "{what} {} took longer than {:.0} s and was skipped; the page may be damaged or extremely complex",
                        req.page + 1,
                        self.stuck_after.as_secs_f32().max(1.0)
                    );
                    self.shared.queue.abandon(RenderedPage {
                        request: req,
                        width: 0,
                        height: 0,
                        rgba: Pixels::default(),
                        error: Some(error),
                        text: None,
                        warnings: Vec::new(),
                        millis: 0,
                    });
                }
                let mut left = lock(&self.replacements_left);
                if *left > 0 && self.spawn_worker() {
                    *left -= 1;
                }
            }
        }
    }
}

/// A pool's threads after [`RenderPool::retire`]. Idle ones exit at once; one still rendering is
/// told to stop at its next drawing operator and exits when its render returns, which a single
/// long operator (such as decoding a huge image) can delay.
pub struct Retiring(pub(crate) Vec<JoinHandle<()>>);

impl Retiring {
    /// The threads that haven't exited yet.
    pub fn running(&mut self) -> usize {
        self.0.retain(|h| !h.is_finished());
        self.0.len()
    }

    /// Whether every thread has exited.
    pub fn exited(&self) -> bool {
        self.0.iter().all(JoinHandle::is_finished)
    }
}

impl RenderPool {
    /// No replacement threads: when the watchdog gives up on a render, the request (and any
    /// later one) is answered with an error instead of starting another thread. For a caller that
    /// bounds how many render threads exist at once: the stuck thread keeps running until its
    /// operator returns, and a replacement would run beside it.
    pub fn without_replacements(self) -> Self {
        *lock(&self.replacements_left) = 0;
        self
    }

    /// This pool's threads that haven't exited (workers and replacements; 0 when rendering inline).
    pub fn threads(&self) -> usize {
        lock(&self._workers).iter().filter(|h| !h.is_finished()).count()
    }

    /// Drop the pool but keep its threads' handles, for a caller that bounds how many render
    /// threads run at once: a render the watchdog gave up on can outlive its pool.
    pub fn retire(self) -> Retiring {
        let handles = std::mem::take(&mut *lock(&self._workers));
        drop(self);
        Retiring(handles)
    }
}

impl Drop for RenderPool {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        for busy in lock(&self.shared.busy).iter().flatten() {
            busy.cancelled.store(true, Ordering::Release);
            busy.valid.store(false, Ordering::Release);
        }
        for s in lock(&self.shared.stop).iter() {
            s.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// A worker's interpreter settings: the document's, stopping once the request is cancelled.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn worker_settings(config: &RenderConfig, stop: &Arc<std::sync::atomic::AtomicBool>) -> InterpreterSettings {
    InterpreterSettings { cancelled: Some(stop.clone()), ..config.settings() }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn worker(
    id: usize,
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
    shared: Arc<Shared>,
    wake: Receiver<()>,
    out: Arc<WorkQueue>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    let warnings = Arc::new(Mutex::new(Vec::new()));
    // A local cell keeps Pdf borrows used by RenderCache stable. Drop both on an
    // epoch change/panic before publishing an error or taking another request.
    loop {
        let reset_result = {
            let parsed = std::cell::OnceCell::new();
            loop {
                if parsed.get().is_some_and(|(_, _, shared_generation)| *shared_generation) && !shared.parser.valid.load(Ordering::Acquire) {
                    break None;
                }
                let next = shared.queue.pop();
                let Some(req) = next else {
                    if wake.recv().is_err() {
                        return; // pool dropped
                    }
                    continue;
                };
                if shared.queue.cancel_obsolete(req) {
                    continue;
                }
                #[cfg(all(test, not(target_arch = "wasm32")))]
                if let Some(gate) = lock(&shared.busy_publish_gate).clone().filter(|gate| same_request(gate.request, req)) {
                    let _ = gate.entered.send(());
                    let _ = lock(&gate.release).recv();
                }
                let request_cancelled = Arc::new(AtomicBool::new(false));
                let request_valid = Arc::new(AtomicBool::new(true));
                if lock(&shared.stuck).contains(&req.into()) {
                    let error = format!("page {} was skipped earlier because rendering failed or took too long", req.page + 1);
                    let page = RenderedPage {
                        request: req,
                        width: 0,
                        height: 0,
                        rgba: Pixels::default(),
                        error: Some(error),
                        text: None,
                        warnings: Vec::new(),
                        millis: 0,
                    };
                    if out.send_valid(page, None).is_err() {
                        return;
                    }
                    continue;
                }
                let start = Stopwatch::start();
                shared.stats.record_request(req);
                let stage = parsed.get().map_or(
                    BusyStage::Parsing,
                    |(_, _, shared_generation)| {
                        if *shared_generation { BusyStage::SharedRender } else { BusyStage::Private }
                    },
                );
                if let Some(slot) = lock(&shared.busy).get_mut(id) {
                    *slot = Some(Busy {
                        request: req,
                        since: std::time::Instant::now(),
                        stage,
                        cancelled: request_cancelled.clone(),
                        valid: request_valid.clone(),
                    });
                }
                if shared.queue.cancel_obsolete(req) {
                    if lock(&shared.busy).get_mut(id).is_none_or(|slot| slot.take().is_none()) {
                        return;
                    }
                    continue;
                }
                #[cfg(all(test, not(target_arch = "wasm32")))]
                lock(&shared.parser_stage_entries).push(req);
                // A request cancelled while it waited for the parser must not leave its abort
                // behind as this worker's parse: every later request would then fail as unparsable.
                let aborted = || request_cancelled.load(Ordering::Acquire) || stop.load(Ordering::Acquire);
                if parsed.get().is_none() {
                    let init = {
                        let load = || {
                            #[cfg(test)]
                            {
                                shared.parses.fetch_add(1, Ordering::Relaxed);
                                if let Some(gate) = lock(&shared.parse_gate).clone() {
                                    gate.entered.wait();
                                    gate.release.wait();
                                }
                                let delay = *lock(&shared.slow_parse);
                                if let Some(delay) = delay {
                                    std::thread::sleep(delay);
                                }
                            }
                            shared.stats.parser_builds.fetch_add(1, Ordering::Relaxed);
                            parse(&bytes, config.password.as_deref())
                        };
                        match shared.parser.acquire(load, || {
                            request_cancelled.load(Ordering::Acquire)
                                || stop.load(Ordering::Acquire)
                                || matches!(wake.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected))
                        }) {
                            Some(pdf) => {
                                let shared_generation = pdf.is_some();
                                (pdf, RenderCache::new(), shared_generation)
                            }
                            None => {
                                let pdf = if !request_cancelled.load(Ordering::Acquire)
                                    && !stop.load(Ordering::Acquire)
                                    && set_busy_stage(&shared, id, BusyStage::Parsing)
                                {
                                    load().map(Arc::new)
                                } else {
                                    None
                                };
                                (pdf, RenderCache::new(), false)
                            }
                        }
                    };
                    if aborted() {
                        if lock(&shared.busy).get_mut(id).is_none_or(|slot| slot.take().is_none()) {
                            return;
                        }
                        // Release the active key, retrying if demand returned during parsing.
                        shared.queue.retry(req);
                        continue;
                    }
                    let _ = parsed.set(init);
                }
                // Set just above when it was empty.
                let Some((pdf, cache, shared_generation)) = parsed.get() else {
                    if lock(&shared.busy).get_mut(id).is_none_or(|slot| slot.take().is_none()) {
                        return;
                    }
                    continue;
                };
                // Acquisition may have waited or parsed privately after shared
                // fallback. Retire before any render work if the watchdog gave
                // this request to a replacement while initialization ran.
                let stage = if *shared_generation { BusyStage::SharedRender } else { BusyStage::Private };
                if !set_busy_stage(&shared, id, stage) {
                    return;
                }
                if shared.queue.cancel_obsolete(req) {
                    if lock(&shared.busy).get_mut(id).is_none_or(|slot| slot.take().is_none()) {
                        return;
                    }
                    continue;
                }
                if *shared_generation && !shared.parser.valid.load(Ordering::Acquire) {
                    if lock(&shared.busy).get_mut(id).is_none_or(|slot| slot.take().is_none()) {
                        return;
                    }
                    break Some((
                        finish(req, start, Err(("the shared parser was retired".into(), false)), Vec::new()),
                        vec![request_valid, shared.parser.valid.clone()],
                    ));
                }
                #[cfg(test)]
                {
                    shared.render_started.fetch_add(1, Ordering::Relaxed);
                    let shared_delay = *lock(&shared.shared_render_delay);
                    if *shared_generation && let Some(delay) = shared_delay {
                        std::thread::sleep(delay);
                    }
                    // Copy out first: a guard held in the `if let` would block the other workers.
                    let slow = *lock(&shared.slow_page);
                    if let Some((page, delay)) = slow
                        && page == req.page
                    {
                        let deadline = std::time::Instant::now() + delay;
                        while std::time::Instant::now() < deadline && !request_cancelled.load(Ordering::Acquire) {
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }
                }
                #[cfg(all(test, not(target_arch = "wasm32")))]
                let render_gate = lock(&shared.render_gates)
                    .iter()
                    .find(|gate| {
                        gate.page == req.page
                            && gate.shared_generation == *shared_generation
                            && gate.claimed.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
                    })
                    .cloned();
                #[cfg(all(test, not(target_arch = "wasm32")))]
                if let Some(gate) = &render_gate {
                    gate.entered.wait();
                    gate.release.wait();
                }
                let settings = warning_settings(&worker_settings(&config, &request_cancelled), warnings.clone());
                lock(&warnings).clear();
                let r = catch_unwind(AssertUnwindSafe(|| {
                    #[cfg(test)]
                    if *lock(&shared.panic_page) == Some(req.page) {
                        panic!("injected page panic");
                    }
                    match pdf {
                        Some(pdf) => render_page(pdf, cache, &settings, &config, req),
                        None => Err(("the document could not be parsed".into(), false)),
                    }
                }))
                .unwrap_or_else(|panic| Err((format!("renderer crashed on page {}: {}", req.page + 1, panic_message(&panic)), true)));
                let request_warnings = take_warnings(&warnings);
                // If the watchdog cleared our slot meanwhile, it already answered for this request
                // and started a replacement: drop the late result and retire.
                let abandoned = lock(&shared.busy).get_mut(id).is_none_or(|slot| slot.take().is_none());
                if abandoned {
                    #[cfg(all(test, not(target_arch = "wasm32")))]
                    if let Some(gate) = render_gate {
                        gate.finished.wait();
                    }
                    return;
                }
                let panicked = matches!(r, Err((_, true)));
                if panicked {
                    shared.parser.abandon();
                    lock(&shared.stuck).insert(req.into());
                    break Some((finish(req, start, r, request_warnings), vec![request_valid]));
                }
                let mut valid = vec![request_valid];
                if *shared_generation {
                    valid.push(shared.parser.valid.clone());
                }
                if *shared_generation && !shared.parser.valid.load(Ordering::Acquire) {
                    break Some((finish(req, start, r, request_warnings), valid));
                }
                if out.send_valid_with_tokens(finish(req, start, r, request_warnings), valid).is_err() {
                    return;
                }
            }
        }; // drop the parser and its borrowing cache before delivering reset_result
        if let Some((page, valid)) = reset_result
            && out.send_valid_with_tokens(page, valid).is_err()
        {
            return;
        }
    }
}
