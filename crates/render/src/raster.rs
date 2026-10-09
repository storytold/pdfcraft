//! Background page rasterization.
//!
//! A `RenderPool` parses lazily once and shares that immutable parser between workers. Render
//! caches stay worker-local. Requests carry a caller tag that is echoed back.
//!
//! **Robustness:** every page render runs under `catch_unwind`. A panic inside the renderer on a
//! malformed page yields an error and permanently retires the pool's shared parser generation.
//! Workers discard its caches/results and fall back to private parsers. There is no shared
//! rebuild loop, and a timed-out shared initialization is never started again in that pool.

use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{Receiver, sync_channel};

mod work_queue;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use work_queue::{ResultReceiver, WorkQueue};

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::font::{FontData, FontQuery};
use hayro::hayro_interpret::hayro_cmap::CidFamily;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings, render_into, render_size};

use crate::Pixels;

/// Monotonic-ish timer that is safe on wasm32 (where `std::time::Instant` panics).
#[derive(Clone, Copy)]
struct Stopwatch(#[cfg(not(target_arch = "wasm32"))] std::time::Instant);

impl Stopwatch {
    fn start() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        return Self(std::time::Instant::now());
        #[cfg(target_arch = "wasm32")]
        return Self();
    }
    fn millis(self) -> u32 {
        #[cfg(not(target_arch = "wasm32"))]
        return self.0.elapsed().as_millis() as u32;
        #[cfg(target_arch = "wasm32")]
        return 0;
    }
}

/// Per-document rendering configuration.
#[derive(Clone, Debug, Default)]
pub struct RenderConfig {
    /// User or owner password for encrypted documents.
    pub password: Option<Arc<str>>,
    /// Layer (optional content group) visibility overrides: (object number, generation, visible).
    pub layers: Arc<Vec<(i32, i32, bool)>>,
    /// View ▸ Hide all comments: markup annotations aren't drawn (fields and links still are).
    pub hide_comments: bool,
}

impl RenderConfig {
    fn settings(&self) -> InterpreterSettings {
        let standard = InterpreterSettings::default().font_resolver;
        InterpreterSettings {
            ocg_overrides: self.layers.clone(),
            hide_comments: self.hide_comments,
            font_resolver: Arc::new(move |query| japanese_fallback(query).or_else(|| standard(query))),
            ..InterpreterSettings::default()
        }
    }
}

/// A Japanese face from craft-fonts for a CID font of the Adobe-Japan1 collection that the PDF
/// doesn't embed (`HeiseiMin-W3`, `KozGoPro-Medium`, …). hayro's own substitutes for fonts that
/// aren't embedded are the Latin standard 14, so such text drew nothing. `None` for every other
/// font, and when PdfCraft was built without craft-fonts. Only Japanese: the pinned craft-fonts
/// has no other CJK faces, and its later Chinese face is Noto CJK, which AGENTS.md §1.1 rules out.
fn japanese_fallback(query: &FontQuery) -> Option<(FontData, u32)> {
    let FontQuery::Fallback(f) = query else { return None };
    if f.character_collection.as_ref()?.family != CidFamily::AdobeJapan1 {
        return None;
    }
    let face = match japanese_face(f.post_script_name.as_deref().unwrap_or_default(), f.is_serif, f.is_bold || f.font_weight >= 600) {
        // BIZ UDMincho before the document face (Shippori Mincho): it covers half-width katakana
        // (U+FF61–U+FF9F), which Shippori Mincho lacks, and hayro draws a CID the substitute
        // can't map by Unicode by glyph index, i.e. as an unrelated glyph (ﬁ, ﬂ, …).
        JapaneseFace::Mincho => pdfcraft_fonts::ui_japanese_fonts()
            .into_iter()
            .find(|c| c.family == "BIZ UDMincho" && c.style == "Regular")
            .or_else(pdfcraft_fonts::document_japanese_font),
        JapaneseFace::Gothic { bold } => {
            let faces = pdfcraft_fonts::ui_japanese_fonts();
            let style = if bold { "Bold" } else { "Regular" };
            faces.iter().find(|c| c.family == "BIZ UDPGothic" && c.style == style).or(faces.first()).copied()
        }
    }?;
    Some((Arc::new(face.bytes), 0))
}

#[derive(Debug, PartialEq)]
enum JapaneseFace {
    Mincho,
    Gothic { bold: bool },
}

/// Which kind of Japanese face stands in for the font named `name`: Mincho names (`HeiseiMin`,
/// `KozMin`, `Ryumin`, `MS-Mincho`) a serif Mincho; Gothic names (`…Gothic…`, `HeiseiKakuGo`,
/// `KozGo`, `…Maru…`) a sans Gothic; any other name by the font descriptor's serif flag.
fn japanese_face(name: &str, serif: bool, bold: bool) -> JapaneseFace {
    let name = name.to_ascii_lowercase();
    if name.contains("min") {
        JapaneseFace::Mincho
    } else if ["goth", "kakugo", "kozgo", "kaku", "maru"].iter().any(|k| name.contains(k)) || !serif {
        JapaneseFace::Gothic { bold }
    } else {
        JapaneseFace::Mincho
    }
}

/// Hard cap on a rendered side, to bound memory at extreme zoom levels (tiling arrives in M3.3).
pub const MAX_SIDE: f32 = 8192.0;
/// Hard cap on rendered pixels per page (~64 MP ≈ 256 MB RGBA).
pub const MAX_PIXELS: f32 = 64.0e6;

/// What a request asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum RequestKind {
    /// A raster of the page.
    #[default]
    Pixels,
    /// The page's text layer (glyphs with Unicode and boxes); `scale` is ignored.
    Text,
}

/// A region of a scaled page, in device pixels (for tiled rendering of large pages).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct RenderRequest {
    pub page: usize,
    pub kind: RequestKind,
    /// Render only this region (device pixels at `scale`). `None` renders the whole page.
    pub tile: Option<Tile>,
    /// Device pixels per PDF point.
    pub scale: f32,
    /// Caller-defined tag, echoed back (used for zoom generations / thumbnail vs page).
    pub tag: u64,
}

#[derive(Debug)]
pub struct RenderedPage {
    pub request: RenderRequest,
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8, row-major. Empty when `error` is set.
    pub rgba: Pixels,
    /// Why the page could not be rendered (renderer panic, empty page box, …).
    pub error: Option<String>,
    /// For `RequestKind::Text`.
    pub text: Option<Arc<crate::text::PageText>>,
    pub millis: u32,
}

/// Clamp a requested scale so the output respects `MAX_SIDE` and `MAX_PIXELS`.
pub fn effective_scale(width_pt: f32, height_pt: f32, scale: f32) -> f32 {
    let (w, h) = (width_pt.max(1.0), height_pt.max(1.0));
    let by_side = MAX_SIDE / w.max(h);
    let by_area = (MAX_PIXELS / (w * h)).sqrt();
    // The caps always win: a floor here once let a 934-million-point-wide page (a fuzzed file)
    // render 9 million pixels wide. Only guard against a zero or non-finite scale.
    let capped = scale.min(by_side).min(by_area);
    if capped.is_finite() && capped > 0.0 { capped } else { by_side.min(by_area).max(f32::MIN_POSITIVE) }
}

/// Device pixels that cover `pt` points at `scale`: rounded up, as poppler's `pdftoppm` does, so
/// the last partial row or column of a page is drawn (anti-aliased against the background) rather
/// than cut off. Float noise up to 1/100 px is not rounded up, so 612 pt at 150 dpi stays 1275 px.
/// At least 1; saturates instead of overflowing.
pub fn device_pixels(pt: f32, scale: f32) -> u32 {
    let px = pt * scale - 0.01;
    if px.is_finite() { px.ceil().max(1.0) as u32 } else { 1 }
}

/// Render one page with a caller-owned parser and cache. Panics inside the renderer are caught
/// and reported as `Err((message, panicked))`.
type Output = (u32, u32, Pixels, Option<Arc<crate::text::PageText>>);

fn render_page<'a>(pdf: &'a Pdf, cache: &RenderCache<'a>, settings: &InterpreterSettings, req: RenderRequest) -> Result<Output, (String, bool)> {
    if req.kind == RequestKind::Text {
        return match catch_unwind(AssertUnwindSafe(|| crate::text::extract_page(pdf, req.page, settings))) {
            Ok(Some(t)) => Ok((0, 0, Pixels::default(), Some(Arc::new(t)))),
            Ok(None) => Err((format!("page {} does not exist", req.page + 1), false)),
            Err(panic) => Err((format!("text extraction crashed on page {}: {}", req.page + 1, panic_message(&panic)), true)),
        };
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let pages = pdf.pages();
        let page = pages.get(req.page).ok_or_else(|| format!("page {} does not exist", req.page + 1))?;
        let (w, h) = page.render_dimensions();
        if !(w.is_finite() && h.is_finite()) || w < 0.5 || h < 0.5 {
            return Err(format!("page {} has an empty or invalid page box ({w}×{h} pt)", req.page + 1));
        }
        let rs = match req.tile {
            // Tiles are bounded by construction, so the page itself may be arbitrarily large.
            Some(t) => {
                let scale = req.scale.clamp(0.01, 400.0);
                let (tw, th) = (t.w.clamp(1, 4096), t.h.clamp(1, 4096));
                RenderSettings {
                    x_scale: scale,
                    y_scale: scale,
                    width: Some(tw as u16),
                    height: Some(th as u16),
                    x_offset: t.x as f32,
                    y_offset: t.y as f32,
                    bg_color: WHITE,
                }
            }
            None => {
                let scale = effective_scale(w, h, req.scale);
                // hayro floors the size when none is given, losing the partial edge pixels. The
                // scale keeps each side within MAX_SIDE (give or take float noise), so it fits u16.
                let side = |pt: f32| device_pixels(pt, scale).min(MAX_SIDE as u32) as u16;
                RenderSettings { x_scale: scale, y_scale: scale, width: Some(side(w)), height: Some(side(h)), bg_color: WHITE, ..Default::default() }
            }
        };
        // Rendered straight into a buffer of whole pixels, which the GUI takes over as is.
        let (w, h) = render_size(page, &rs);
        let mut pixels = Pixels::zeroed(w.into(), h.into()).ok_or_else(|| format!("page {} is too large to render", req.page + 1))?;
        render_into(page, cache, settings, &rs, pixels.bytes_mut());
        Ok((w.into(), h.into(), pixels, None))
    }));
    match result {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err((e, false)),
        Err(panic) => Err((format!("renderer crashed on page {}: {}", req.page + 1, panic_message(&panic)), true)),
    }
}

fn finish(req: RenderRequest, start: Stopwatch, r: Result<Output, (String, bool)>) -> RenderedPage {
    let millis = start.millis();
    match r {
        Ok((width, height, rgba, text)) => RenderedPage { request: req, width, height, rgba, error: None, text, millis },
        Err((e, _)) => RenderedPage { request: req, width: 0, height: 0, rgba: Pixels::default(), error: Some(e), text: None, millis },
    }
}

/// A single-threaded renderer over one parsed document (used by the CLI and tests).
pub struct PageRenderer {
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
    pdf: Option<Pdf>,
    settings: InterpreterSettings,
}

impl PageRenderer {
    pub fn new(bytes: Arc<Vec<u8>>, config: RenderConfig) -> Self {
        let pdf = parse(&bytes, config.password.as_deref());
        let settings = config.settings();
        Self { bytes, config, pdf, settings }
    }

    pub fn page_count(&self) -> usize {
        self.pdf.as_ref().map(|p| p.pages().len()).unwrap_or(0)
    }

    /// Render one page. Never panics.
    pub fn render(&mut self, req: RenderRequest) -> RenderedPage {
        let start = Stopwatch::start();
        let Some(pdf) = self.pdf.as_ref() else { return finish(req, start, Err(("the document could not be parsed".into(), false))) };
        let cache = RenderCache::new();
        let r = render_page(pdf, &cache, &self.settings, req);
        if matches!(r, Err((_, true))) {
            self.pdf = parse(&self.bytes, self.config.password.as_deref());
        }
        finish(req, start, r)
    }
}

fn parse(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Option<Pdf> {
    catch_unwind(AssertUnwindSafe(|| Pdf::new_with_password(bytes.clone(), password.unwrap_or("")).ok())).ok().flatten()
}

pub(crate) fn panic_message(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown panic".into())
}

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
#[derive(Clone, Copy)]
struct Busy {
    request: RenderRequest,
    since: std::time::Instant,
    stage: BusyStage,
}

/// State shared by the pool and its workers.
#[derive(Default)]
struct Shared {
    /// Desired requests, active jobs and bounded completed results share one deduplication key.
    queue: Arc<WorkQueue>,
    /// Per worker id: the request it is rendering and since when.
    #[cfg(not(target_arch = "wasm32"))]
    busy: Mutex<Vec<Option<Busy>>>,
    /// Pages (and request kinds) the watchdog gave up on: answered with an error at once, so a
    /// pathological page cannot trap every worker in turn.
    stuck: Mutex<std::collections::HashSet<(usize, RequestKind)>>,
    /// Per worker id: set to stop that worker's render at its next content operator once nobody
    /// can receive its answer (the pool was dropped, or the watchdog gave up on the render).
    /// Lock `busy` first when holding both.
    stop: Mutex<Vec<Arc<std::sync::atomic::AtomicBool>>>,
    /// Test hook: make one page slow.
    #[cfg(test)]
    slow_page: Mutex<Option<(usize, std::time::Duration)>>,
    /// Test hook: count parser initializations, including failed parses.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    parses: std::sync::atomic::AtomicUsize,
    #[cfg(not(target_arch = "wasm32"))]
    parser: PoolParser,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    panic_page: Mutex<Option<usize>>,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    slow_parse: Mutex<Option<std::time::Duration>>,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    render_started: std::sync::atomic::AtomicUsize,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    shared_render_delay: Mutex<Option<std::time::Duration>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
enum ParserState {
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
struct PoolParser {
    state: Mutex<ParserState>,
    ready: std::sync::Condvar,
    valid: Arc<AtomicBool>,
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
    fn acquire(&self, load: impl FnOnce() -> Option<Pdf>, cancelled: impl Fn() -> bool) -> Option<Option<Arc<Pdf>>> {
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
                    state = self.ready.wait_timeout(state, std::time::Duration::from_millis(50)).unwrap_or_else(|p| p.into_inner()).0;
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

    fn abandon(&self) {
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
    shared: Arc<Shared>,
    wake: Mutex<Vec<SyncSender<()>>>,
    results: ResultReceiver,
    results_tx: Arc<WorkQueue>,
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
    _workers: Mutex<Vec<JoinHandle<()>>>,
    stuck_after: std::time::Duration,
    replacements_left: Mutex<usize>,
    retired_workers: Mutex<usize>,
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

    /// Change the watchdog limit (tests and benchmarks).
    pub fn set_stuck_after(&mut self, limit: std::time::Duration) {
        self.stuck_after = limit;
    }

    /// Replace desired work (most urgent first), retaining matching in-flight/completed jobs.
    /// Obsolete results are released before they reach the caller. In-flight work is not killed.
    ///
    /// The contract: a result is delivered only while its request is in the latest queue.
    /// A caller still waiting for a page (or text, or a thumbnail) must keep listing it in
    /// every call, or the finished result is dropped to save memory.
    pub fn set_queue(&self, requests: Vec<RenderRequest>) {
        self.shared.queue.replace(requests);
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
                millis: 0,
            })
        })
    }

    /// Give up on renders that exceeded `stuck_after` (see the type docs).
    fn watchdog(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut gave_up = Vec::new();
            {
                let mut busy = lock(&self.shared.busy);
                // Lock order: `busy`, then `stop`.
                let stop = lock(&self.shared.stop);
                for (id, slot) in busy.iter_mut().enumerate() {
                    if let Some(busy) = *slot
                        && busy.stage != BusyStage::Parsing
                        && busy.since.elapsed() > self.stuck_after
                    {
                        *slot = None; // the worker sees this and exits when it returns
                        // Told to stop at its next operator; its replacement retries the page.
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
                    lock(&self.shared.stuck).insert((req.page, req.kind));
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

impl Drop for RenderPool {
    fn drop(&mut self) {
        for s in lock(&self.shared.stop).iter() {
            s.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// A worker's interpreter settings: the document's, stopping once `stop` is set.
#[cfg(not(target_arch = "wasm32"))]
fn worker_settings(config: &RenderConfig, stop: &Arc<std::sync::atomic::AtomicBool>) -> InterpreterSettings {
    InterpreterSettings { cancelled: Some(stop.clone()), ..config.settings() }
}

#[cfg(not(target_arch = "wasm32"))]
fn worker(
    id: usize,
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
    shared: Arc<Shared>,
    wake: Receiver<()>,
    out: Arc<WorkQueue>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    let settings = worker_settings(&config, &stop);
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
                if lock(&shared.stuck).contains(&(req.page, req.kind)) {
                    let error = format!("page {} was skipped earlier because rendering failed or took too long", req.page + 1);
                    let page = RenderedPage { request: req, width: 0, height: 0, rgba: Pixels::default(), error: Some(error), text: None, millis: 0 };
                    if out.send_valid(page, None).is_err() {
                        return;
                    }
                    continue;
                }
                let start = Stopwatch::start();
                let stage = parsed.get().map_or(
                    BusyStage::Parsing,
                    |(_, _, shared_generation)| {
                        if *shared_generation { BusyStage::SharedRender } else { BusyStage::Private }
                    },
                );
                if let Some(slot) = lock(&shared.busy).get_mut(id) {
                    *slot = Some(Busy { request: req, since: std::time::Instant::now(), stage });
                }
                let (pdf, cache, shared_generation) = parsed.get_or_init(|| {
                    let load = || {
                        #[cfg(test)]
                        {
                            shared.parses.fetch_add(1, Ordering::Relaxed);
                            let delay = *lock(&shared.slow_parse);
                            if let Some(delay) = delay {
                                std::thread::sleep(delay);
                            }
                        }
                        parse(&bytes, config.password.as_deref())
                    };
                    match shared.parser.acquire(load, || matches!(wake.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected))) {
                        Some(pdf) => {
                            let shared_generation = pdf.is_some();
                            (pdf, RenderCache::new(), shared_generation)
                        }
                        None => {
                            let pdf = if set_busy_stage(&shared, id, BusyStage::Parsing) { load().map(Arc::new) } else { None };
                            (pdf, RenderCache::new(), false)
                        }
                    }
                });
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
                    break Some((finish(req, start, Err(("the shared parser was retired".into(), false))), Some(shared.parser.valid.clone())));
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
                        std::thread::sleep(delay);
                    }
                }
                let r = catch_unwind(AssertUnwindSafe(|| {
                    #[cfg(test)]
                    if *lock(&shared.panic_page) == Some(req.page) {
                        panic!("injected page panic");
                    }
                    match pdf {
                        Some(pdf) => render_page(pdf, cache, &settings, req),
                        None => Err(("the document could not be parsed".into(), false)),
                    }
                }))
                .unwrap_or_else(|panic| Err((format!("renderer crashed on page {}: {}", req.page + 1, panic_message(&panic)), true)));
                // If the watchdog cleared our slot meanwhile, it already answered for this request
                // and started a replacement: drop the late result and retire.
                let abandoned = lock(&shared.busy).get_mut(id).is_none_or(|slot| slot.take().is_none());
                if abandoned {
                    return;
                }
                let panicked = matches!(r, Err((_, true)));
                if panicked {
                    shared.parser.abandon();
                    lock(&shared.stuck).insert((req.page, req.kind));
                    break Some((finish(req, start, r), None));
                }
                let valid = shared_generation.then(|| shared.parser.valid.clone());
                if *shared_generation && !shared.parser.valid.load(Ordering::Acquire) {
                    break Some((finish(req, start, r), valid));
                }
                if out.send_valid(finish(req, start, r), valid).is_err() {
                    return;
                }
            }
        }; // drop the parser and its borrowing cache before delivering reset_result
        if let Some((page, valid)) = reset_result
            && out.send_valid(page, valid).is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_arch = "wasm32"))]
    fn receive_before_deadline(pool: &RenderPool) -> RenderedPage {
        // Generous: a loaded CI machine runs many render tests at once.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(page) = pool.try_recv() {
                return page;
            }
            assert!(std::time::Instant::now() < deadline, "no worker result");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn shared_parser_is_lazy_and_initializes_once_under_contention() {
        use std::sync::{Barrier, atomic::AtomicUsize};
        let parser = Arc::new(PoolParser::default());
        let loads = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(16));
        assert_eq!(loads.load(Ordering::Relaxed), 0);
        let jobs: Vec<_> = (0..16)
            .map(|_| {
                let (parser, loads, barrier) = (parser.clone(), loads.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    parser
                        .acquire(
                            || {
                                loads.fetch_add(1, Ordering::Relaxed);
                                Pdf::new(ONE_PAGE.to_vec()).ok()
                            },
                            || false,
                        )
                        .unwrap()
                        .unwrap()
                })
            })
            .collect();
        let pdfs: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
        assert_eq!(loads.load(Ordering::Relaxed), 1);
        assert!(pdfs.iter().all(|pdf| Arc::ptr_eq(pdf, &pdfs[0])));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn initializer_panic_publishes_one_failure_and_wakes_waiters() {
        use std::sync::{Barrier, atomic::AtomicUsize};
        let parser = Arc::new(PoolParser::default());
        let loads = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(8));
        let jobs: Vec<_> = (0..8)
            .map(|_| {
                let (parser, loads, barrier) = (parser.clone(), loads.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    assert!(
                        parser
                            .acquire(
                                || {
                                    loads.fetch_add(1, Ordering::Relaxed);
                                    panic!("injected parser panic");
                                },
                                || false
                            )
                            .unwrap()
                            .is_none()
                    );
                })
            })
            .collect();
        for job in jobs {
            job.join().unwrap();
        }
        assert_eq!(loads.load(Ordering::Relaxed), 1);
        assert!(parser.acquire(|| panic!("must not retry"), || false).unwrap().is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn waiting_worker_can_cancel_before_initializer_returns() {
        use std::{
            sync::{atomic::AtomicUsize, mpsc},
            time::Duration,
        };
        let parser = Arc::new(PoolParser::default());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let initializer = {
            let parser = parser.clone();
            std::thread::spawn(move || {
                parser.acquire(
                    || {
                        started_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Pdf::new(ONE_PAGE.to_vec()).ok()
                    },
                    || false,
                )
            })
        };
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let checks = Arc::new(AtomicUsize::new(0));
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = {
            let (parser, cancelled, checks) = (parser.clone(), cancelled.clone(), checks.clone());
            std::thread::spawn(move || {
                let result = parser.acquire(
                    || panic!("must not initialize twice"),
                    || {
                        checks.fetch_add(1, Ordering::Relaxed);
                        cancelled.load(Ordering::Acquire)
                    },
                );
                done_tx.send(result.unwrap().is_none()).unwrap();
            })
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while checks.load(Ordering::Relaxed) == 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        cancelled.store(true, Ordering::Release);
        assert!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap());
        waiter.join().unwrap();
        release_tx.send(()).unwrap();
        assert!(initializer.join().unwrap().unwrap().is_some());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn page_panic_retires_shared_generation_and_healthy_pages_use_private_parsers() {
        let pool = RenderPool::new(Arc::new(ONE_PAGE_TWICE.to_vec()), 3, RenderConfig::default());
        *lock(&pool.shared.panic_page) = Some(0);
        *lock(&pool.shared.slow_page) = Some((1, std::time::Duration::from_millis(50)));
        pool.set_queue(vec![
            RenderRequest { page: 0, scale: 1.0, ..Default::default() },
            RenderRequest { page: 1, scale: 1.0, ..Default::default() },
        ]);
        let mut pages = [receive_before_deadline(&pool), receive_before_deadline(&pool)];
        pages.sort_by_key(|page| page.request.page);
        assert!(pages[0].error.as_deref().is_some_and(|message| message.contains("crashed")));
        assert!(pages[1].error.is_none(), "{:?}", pages[1].error);
        assert_eq!((pages[1].width, pages[1].height), (100, 50));
        assert!(!pool.shared.parser.valid.load(Ordering::Acquire));
        assert!(matches!(*lock(&pool.shared.parser.state), ParserState::Private));
        assert!((2..=4).contains(&pool.shared.parses.load(Ordering::Relaxed)));
        let before = pool.shared.parses.load(Ordering::Relaxed);
        pool.set_queue(vec![RenderRequest { page: 0, tag: 1, scale: 1.0, ..Default::default() }]);
        assert!(receive_before_deadline(&pool).error.is_some());
        assert_eq!(pool.shared.parses.load(Ordering::Relaxed), before, "a failed page does not trigger repeated parsing");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn results_queued_before_invalidation_are_retried_privately() {
        let pool = RenderPool::new(Arc::new(ONE_PAGE.to_vec()), 1, RenderConfig::default());
        pool.set_queue(vec![RenderRequest { tag: 77, scale: 1.0, ..Default::default() }]);
        let mut old = pool.results.recv_timeout(std::time::Duration::from_secs(8)).unwrap();
        old.width = 999; // a stale surface must never reach the caller
        pool.shared.parser.abandon();
        pool.results_tx.send_valid(old, Some(pool.shared.parser.valid.clone())).unwrap();
        let page = receive_before_deadline(&pool);
        assert_eq!((page.width, page.height, page.request.tag), (100, 50, 77));
        assert!(page.error.is_none());
        assert_eq!(pool.shared.parses.load(Ordering::Relaxed), 2);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_slow_parse_is_not_a_timeout_and_its_pages_render() {
        use std::time::{Duration, Instant};
        // Shared, then the private parsers used after a shared failure.
        for private in [false, true] {
            let mut pool = RenderPool::new(Arc::new(ONE_PAGE_TWICE.to_vec()), 2, RenderConfig::default());
            if private {
                *lock(&pool.shared.parser.state) = ParserState::Private;
                pool.shared.parser.valid.store(false, Ordering::Release);
            }
            // The parse takes 2 s against a 10 ms limit: the watchdog must leave it alone.
            *lock(&pool.shared.slow_parse) = Some(Duration::from_secs(2));
            pool.set_stuck_after(Duration::from_millis(10));
            pool.set_queue(vec![
                RenderRequest { page: 0, scale: 1.0, tag: 60, ..Default::default() },
                RenderRequest { page: 1, scale: 1.0, tag: 61, ..Default::default() },
            ]);
            let started = Instant::now();
            while pool.shared.parses.load(Ordering::Relaxed) == 0 {
                assert!(started.elapsed() < Duration::from_secs(5), "parsing did not start");
                std::thread::sleep(Duration::from_millis(2));
            }
            // Run the watchdog well past its limit while the parse is still sleeping.
            for _ in 0..20 {
                assert!(pool.try_recv().is_none(), "nothing may be given up on while parsing");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(lock(&pool.shared.stuck).is_empty());
            assert_eq!(*lock(&pool.retired_workers), 0);
            assert!(!matches!(*lock(&pool.shared.parser.state), ParserState::Failed));
            // Rendering is timed from the end of parsing: give it a limit it can't miss.
            pool.set_stuck_after(Duration::from_secs(60));
            let mut pages = [receive_before_deadline(&pool), receive_before_deadline(&pool)];
            pages.sort_by_key(|page| page.request.page);
            for (page, tag) in pages.iter().zip([60, 61]) {
                assert!(page.error.is_none(), "{:?}", page.error);
                assert_eq!((page.width, page.height, page.request.tag), (100, 50, tag));
            }
            if !private {
                assert_eq!(pool.shared.parses.load(Ordering::Relaxed), 1, "one shared parse");
                assert!(matches!(*lock(&pool.shared.parser.state), ParserState::Ready(Some(_))));
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn shared_timeouts_retry_healthy_pages_privately_before_blacklisting() {
        use std::time::Duration;
        let mut pool = RenderPool::new(Arc::new(ONE_PAGE_TWICE.to_vec()), 2, RenderConfig::default());
        // Limits and delays far apart, so a loaded machine can't flip the outcome: a healthy
        // page renders privately well within 1 s, the injected delays are 3 s and 6 s.
        pool.set_stuck_after(Duration::from_secs(1));
        // Both shared workers wait as if behind the same cold decode. Only page0
        // remains pathological in an independent parser; page1 must recover.
        *lock(&pool.shared.shared_render_delay) = Some(Duration::from_secs(3));
        *lock(&pool.shared.slow_page) = Some((0, Duration::from_secs(6)));
        pool.set_queue(vec![
            RenderRequest { page: 0, scale: 1.0, tag: 40, ..Default::default() },
            RenderRequest { page: 1, scale: 1.0, tag: 41, ..Default::default() },
        ]);
        let mut pages = [receive_before_deadline(&pool), receive_before_deadline(&pool)];
        pages.sort_by_key(|page| page.request.page);
        assert!(pages[0].error.as_deref().is_some_and(|message| message.contains("took longer")));
        assert!(pages[1].error.is_none(), "{:?}", pages[1].error);
        assert_eq!((pages[1].width, pages[1].height, pages[1].request.tag), (100, 50, 41));
        assert!(lock(&pool.shared.stuck).contains(&(0, RequestKind::Pixels)));
        assert!(!lock(&pool.shared.stuck).contains(&(1, RequestKind::Pixels)));
        assert!((2..=3).contains(&pool.shared.parses.load(Ordering::Relaxed)), "one shared parser and one or two reusable private parsers");
        assert!(lock(&pool._workers).len() <= 4, "the existing replacement cap is unchanged");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn exhausted_private_retry_budget_reports_pending_and_future_requests() {
        use std::time::Duration;
        let mut pool = RenderPool::new(Arc::new(ONE_PAGE_TWICE.to_vec()), 1, RenderConfig::default());
        pool.set_stuck_after(Duration::from_millis(300));
        *lock(&pool.shared.slow_page) = Some((0, Duration::from_secs(3)));
        let bad = RenderRequest { page: 0, scale: 1.0, tag: 50, ..Default::default() };
        let healthy = RenderRequest { page: 1, scale: 1.0, tag: 51, ..Default::default() };
        pool.set_queue(vec![bad, healthy]);
        let bad_result = receive_before_deadline(&pool);
        assert_eq!(bad_result.request, bad);
        assert!(bad_result.error.as_deref().is_some_and(|message| message.contains("took longer")));
        let exhausted = receive_before_deadline(&pool);
        assert_eq!(exhausted.request, healthy);
        assert!(exhausted.error.as_deref().is_some_and(|message| message.contains("all render workers") && message.contains("reopen")));
        assert_eq!(pool.shared.parses.load(Ordering::Relaxed), 2);
        assert_eq!(lock(&pool._workers).len(), 2, "no threads beyond the original replacement cap");
        assert!(!lock(&pool.shared.stuck).contains(&(1, RequestKind::Pixels)), "the healthy page itself is not blacklisted");
        let later = RenderRequest { tag: 52, ..healthy };
        pool.set_queue(vec![later]);
        let result = receive_before_deadline(&pool);
        assert_eq!(result.request, later);
        assert!(result.error.as_deref().is_some_and(|message| message.contains("all render workers")));
        assert_eq!(pool.shared.parses.load(Ordering::Relaxed), 2, "exhaustion performs no parser work on the consumer");
        let workers = std::mem::take(&mut *lock(&pool._workers));
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(pool.try_recv().is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn replacing_a_request_during_initialization_skips_its_obsolete_render() {
        use std::time::{Duration, Instant};
        let pool = RenderPool::new(Arc::new(ONE_PAGE_TWICE.to_vec()), 1, RenderConfig::default());
        *lock(&pool.shared.slow_parse) = Some(Duration::from_millis(200));
        *lock(&pool.shared.slow_page) = Some((0, Duration::from_secs(10)));
        pool.set_queue(vec![RenderRequest { page: 0, scale: 1.0, tag: 10, ..Default::default() }]);
        let deadline = Instant::now() + Duration::from_secs(2);
        while pool.shared.parses.load(Ordering::Relaxed) == 0 {
            assert!(Instant::now() < deadline, "first request did not start parsing");
            std::thread::sleep(Duration::from_millis(2));
        }
        let replaced = Instant::now();
        pool.set_queue(vec![RenderRequest { page: 1, scale: 1.0, tag: 11, ..Default::default() }]);
        let page = receive_before_deadline(&pool);
        assert_eq!((page.request.page, page.request.tag), (1, 11));
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!((page.width, page.height), (100, 50));
        assert!(replaced.elapsed() < Duration::from_secs(8), "obsolete page (10 s) delayed the replacement request");
        assert_eq!(pool.shared.parses.load(Ordering::Relaxed), 1, "the useful parser is retained");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn idle_and_skipped_workers_release_source_without_parsing() {
        for skipped in [false, true] {
            let shared = Arc::new(Shared::default());
            if skipped {
                let req = RenderRequest { page: 0, ..Default::default() };
                shared.queue.replace(vec![req]);
                lock(&shared.stuck).insert((req.page, req.kind));
            }
            let bytes = Arc::new(ONE_PAGE.to_vec());
            let source = Arc::downgrade(&bytes);
            let (wake_tx, wake_rx) = sync_channel(1);
            let out = shared.queue.clone();
            let results = ResultReceiver(shared.queue.clone());
            // A dropped pool disconnects this channel. Running synchronously makes this check
            // deterministic even when a worker starts after the pool has already been replaced.
            drop(wake_tx);
            worker(0, bytes, RenderConfig::default(), shared.clone(), wake_rx, out, Arc::new(std::sync::atomic::AtomicBool::new(false)));
            assert_eq!(shared.parses.load(std::sync::atomic::Ordering::Relaxed), 0);
            assert!(source.upgrade().is_none(), "retired workers release their source bytes");
            if skipped {
                assert!(results.try_recv().unwrap().error.as_deref().is_some_and(|e| e.contains("skipped earlier")));
            }
            assert!(results.try_recv().is_err());
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn worker_reuses_parser_and_releases_source_after_rendering() {
        let bytes = Arc::new(ONE_PAGE.to_vec());
        let source = Arc::downgrade(&bytes);
        let pool = RenderPool::new(bytes, 1, RenderConfig::default());
        assert!(!pool.is_inline(), "the lifecycle check needs a native worker");
        for (tag, kind) in [RequestKind::Pixels, RequestKind::Text, RequestKind::Pixels].into_iter().enumerate() {
            pool.set_queue(vec![RenderRequest { page: 0, kind, scale: 1.0, tag: tag as u64, ..Default::default() }]);
            let page = pool.results.recv_timeout(std::time::Duration::from_secs(8)).expect("worker result");
            assert!(page.error.is_none(), "{:?}", page.error);
            assert_eq!(page.request.tag, tag as u64);
            match kind {
                RequestKind::Pixels => assert_eq!((page.width, page.height), (100, 50)),
                RequestKind::Text => assert!(page.text.is_some()),
            }
            assert_eq!(pool.shared.parses.load(std::sync::atomic::Ordering::Relaxed), 1, "requests reuse one parser");
        }
        drop(pool);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while source.upgrade().is_some() {
            assert!(std::time::Instant::now() < until, "dropped pools retire their workers and release the source");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Public-API regressions run in normal workspace CI; dependency unit tests do not.
    fn function_limits_calculator_eval(program: &str) -> Option<Vec<f32>> {
        use hayro::hayro_interpret::Function;
        use hayro_syntax::object::{FromBytes, Object};
        let data = format!("<< /FunctionType 4 /Domain [] /Length {} >> stream\n{program}\nendstream", program.len());
        let object = Object::from_bytes(data.as_bytes()).unwrap();
        Function::new(&object)?.eval(Default::default()).map(|values| values.to_vec())
    }

    #[test]
    fn function_limits_idiv_zero_is_refused() {
        assert!(function_limits_calculator_eval("{ 1 0 idiv }").is_none());
    }

    #[test]
    fn function_limits_idiv_overflow_is_refused() {
        assert!(function_limits_calculator_eval("{ -2147483648 -1 idiv }").is_none());
    }

    #[test]
    fn function_limits_large_logical_shifts_discard_all_bits() {
        for shift in [32, -32, 2147483647, -2147483648] {
            assert_eq!(function_limits_calculator_eval(&format!("{{ 7 {shift} bitshift }}")), Some(vec![0.0]));
        }
        assert_eq!(function_limits_calculator_eval("{ 1073741824 1 bitshift }"), Some(vec![-2147483648.0]));
        assert_eq!(function_limits_calculator_eval("{ -2147483648 -1 bitshift }"), Some(vec![1073741824.0]));
    }

    #[test]
    fn function_limits_rotation_accepts_the_most_negative_integer() {
        assert_eq!(function_limits_calculator_eval("{ 1 2 3 3 -2147483648 roll }"), Some(vec![3.0, 1.0, 2.0]));
    }

    #[test]
    fn function_limits_large_indices_are_refused() {
        assert!(function_limits_calculator_eval("{ 1 4294967295 index }").is_none());
    }

    #[test]
    fn function_limits_operand_stack_boundary() {
        assert_eq!(function_limits_calculator_eval(&format!("{{ 1 {} }}", "dup ".repeat(63))).unwrap().len(), 64);
        assert!(function_limits_calculator_eval(&format!("{{ 1 {} }}", "dup ".repeat(64))).is_none());
    }

    #[test]
    fn function_limits_calculator_arithmetic_and_depth() {
        for (program, expected) in [
            ("{ -5 2 idiv }", vec![-2.0]),
            ("{ 7 3 bitshift }", vec![56.0]),
            ("{ 142 -3 bitshift }", vec![17.0]),
            ("{ 1 2 3 3 -1 roll }", vec![2.0, 3.0, 1.0]),
        ] {
            assert_eq!(function_limits_calculator_eval(program), Some(expected));
        }
        let mut program = "{ 0 }".to_owned();
        for _ in 1..64 {
            program = format!("{{ true {program} if }}");
        }
        assert_eq!(function_limits_calculator_eval(&program), Some(vec![0.0]));
        assert!(function_limits_calculator_eval(&format!("{{ true {program} if }}")).is_none());
    }

    #[test]
    fn function_limits_calculator_work_boundary() {
        // With no loops, each admitted operator can execute at most once. The exact parse
        // boundary can be evaluated, while construction rejects the next operator.
        assert_eq!(function_limits_calculator_eval(&format!("{{ {} }}", "0 pop ".repeat(5000))), Some(vec![]));
        assert!(function_limits_calculator_eval(&format!("{{ {} 0 }}", "0 pop ".repeat(5000))).is_none());
    }

    #[test]
    fn function_limits_calculator_work_counts_unselected_branches() {
        // 9,996 branch operators + two procedure openings + true + ifelse = 10,000 tokens.
        let branch = "0 pop ".repeat(2499);
        assert_eq!(function_limits_calculator_eval(&format!("{{ true {{ {branch} }} {{ {branch} }} ifelse }}")), Some(vec![]));
        assert!(function_limits_calculator_eval(&format!("{{ true {{ {branch} }} {{ {branch} 0 }} ifelse }}")).is_none());
    }
    #[test]
    fn function_limits_public_construction_work_and_depth() {
        use hayro::hayro_interpret::Function;
        use hayro_syntax::object::{FromBytes, Object};

        let mut data = "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>".to_owned();
        for _ in 1..64 {
            data = format!("<< /FunctionType 3 /Domain [0 1] /Functions [{data}] /Bounds [] /Encode [0 1] >>");
        }
        let object = Object::from_bytes(data.as_bytes()).unwrap();
        assert_eq!(Function::new(&object).unwrap().eval([0.5].into_iter().collect()).unwrap().as_slice(), &[0.5]);
        let over = format!("<< /FunctionType 3 /Domain [0 1] /Functions [{data}] /Bounds [] /Encode [0 1] >>");
        assert!(Function::new(&Object::from_bytes(over.as_bytes()).unwrap()).is_none());

        // One root plus 10,000 leaves is one node past the common budget. This input remains
        // below a megabyte and does not attempt excessive recursion or an allocation failure.
        let leaf = "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> ";
        for children in [9999, 10_000] {
            let data = format!(
                "<< /FunctionType 3 /Domain [0 1] /Functions [{}] /Bounds [{}] /Encode [{}] >>",
                leaf.repeat(children),
                "0.5 ".repeat(children - 1),
                "0 1 ".repeat(children)
            );
            let object = Object::from_bytes(data.as_bytes()).unwrap();
            assert_eq!(Function::new(&object).is_some(), children == 9999);
        }
    }
    #[test]
    fn function_limits_stitching_cycles_are_refused_and_shared_children_work() {
        use hayro::hayro_interpret::Function;
        use hayro_syntax::object::{Object, ObjectIdentifier};

        let pdf = |functions: &str| {
            hayro_syntax::Pdf::new(
                format!(
                    "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20] >> endobj\n\
             {functions}\ntrailer << /Root 1 0 R >>\n%%EOF"
                )
                .into_bytes(),
            )
            .unwrap()
        };
        for functions in [
            "4 0 obj << /FunctionType 3 /Domain [0 1] /Functions [4 0 R] /Bounds [] /Encode [0 1] >> endobj",
            "4 0 obj << /FunctionType 3 /Domain [0 1] /Functions [5 0 R] /Bounds [] /Encode [0 1] >> endobj\n5 0 obj << /FunctionType 3 /Domain [0 1] /Functions [4 0 R] /Bounds [] /Encode [0 1] >> endobj",
            "4 0 obj << /FunctionType 3 /Domain [0 1] /Functions [] /Bounds [] /Encode [] >> endobj",
            "4 0 obj << /FunctionType 3 /Domain [0 1] /Functions [99 0 R] /Bounds [] /Encode [0 1] >> endobj",
            "4 0 obj << /FunctionType 3 /Domain [0 1] /Functions [5 0 R 99 0 R] /Bounds [0.5] /Encode [0 1 0 1] >> endobj\n5 0 obj << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> endobj",
        ] {
            let parsed = pdf(functions);
            let object = parsed.xref().get::<Object<'_>>(ObjectIdentifier::new(4, 0)).unwrap();
            assert!(Function::new(&object).is_none(), "accepted invalid children: {functions}");
        }
        let parsed = pdf(
            "4 0 obj << /FunctionType 3 /Domain [0 1] /Functions [5 0 R 5 0 R] /Bounds [0.5] /Encode [0 1 0 1] >> endobj\n5 0 obj << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> endobj",
        );
        let object = parsed.xref().get::<Object<'_>>(ObjectIdentifier::new(4, 0)).unwrap();
        let function = Function::new(&object).unwrap();
        let output = function.eval([0.75].into_iter().collect()).unwrap();
        assert!((output[0] - 0.5).abs() < 0.001);
    }

    #[test]
    fn function_limits_shared_references_count_toward_construction_work() {
        use hayro::hayro_interpret::Function;
        use hayro_syntax::object::{Object, ObjectIdentifier};
        for children in [9999, 10_000] {
            let bytes = format!(
                "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
                 2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
                 3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20] >> endobj\n\
                 4 0 obj << /FunctionType 3 /Domain [0 1] /Functions [{}] /Bounds [{}] /Encode [{}] >> endobj\n\
                 5 0 obj << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> endobj\n\
                 trailer << /Root 1 0 R >>\n%%EOF",
                "5 0 R ".repeat(children),
                "0.5 ".repeat(children - 1),
                "0 1 ".repeat(children)
            )
            .into_bytes();
            let parsed = hayro_syntax::Pdf::new(bytes).unwrap();
            let object = parsed.xref().get::<Object<'_>>(ObjectIdentifier::new(4, 0)).unwrap();
            assert_eq!(Function::new(&object).is_some(), children == 9999);
        }
    }
    #[test]
    fn function_limits_invalid_calculator_keeps_the_page_renderable() {
        use super::*;
        let program = "{ pop 1 0 idiv }";
        let content = "/S sh 0 0 1 rg 0 0 20 20 re f";
        let bytes = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20] /Resources << /Shading << /S 4 0 R >> >> /Contents 6 0 R >> endobj\n\
             4 0 obj << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 20 0] /Function 5 0 R >> endobj\n\
             5 0 obj << /FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1] /Length {} >> stream\n{program}\nendstream endobj\n\
             6 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF",
            program.len(),
            content.len()
        )
        .into_bytes();
        let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
        let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!((page.width, page.height), (20, 20));
        let (pixels, remainder) = page.rgba.as_chunks::<4>();
        assert!(remainder.is_empty());
        assert!(pixels.iter().all(|pixel| *pixel == [0, 0, 255, 255]));
    }
    #[test]
    fn watchdog_skips_a_stuck_page_and_keeps_rendering() {
        use super::*;
        let mut pool = RenderPool::new(Arc::new(ONE_PAGE_TWICE.to_vec()), 1, RenderConfig::default());
        // Exercise the original private-worker replacement guarantee. Shared
        // contention gets one isolated attempt, covered by the tests above.
        *lock(&pool.shared.parser.state) = ParserState::Private;
        pool.shared.parser.valid.store(false, Ordering::Release);
        pool.set_stuck_after(std::time::Duration::from_millis(1000));
        *lock(&pool.shared.slow_page) = Some((0, std::time::Duration::from_secs(4)));
        let req = |page| RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale: 0.5, tag: 0 };
        pool.set_queue(vec![req(0)]);
        let recv = |pool: &RenderPool| {
            let t = std::time::Instant::now();
            loop {
                if let Some(p) = pool.try_recv() {
                    return p;
                }
                assert!(t.elapsed() < std::time::Duration::from_secs(8), "no result");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        let started = std::time::Instant::now();
        let first = recv(&pool);
        assert!(started.elapsed() < std::time::Duration::from_millis(3000), "the watchdog answers before the render ends");
        assert!(first.error.as_deref().is_some_and(|e| e.contains("took longer")), "{:?}", first.error);
        // The single worker is stuck, yet page 2 still renders (on the replacement worker).
        pool.set_queue(vec![req(1)]);
        let second = recv(&pool);
        assert!(second.error.is_none() && second.request.page == 1, "{:?}", second.error);
        // Asking for the stuck page again fails fast instead of trapping another worker.
        pool.set_queue(vec![req(0)]);
        let again = recv(&pool);
        assert!(again.error.as_deref().is_some_and(|e| e.contains("skipped earlier")), "{:?}", again.error);
        // The stuck worker's late result is dropped, not delivered twice.
        std::thread::sleep(std::time::Duration::from_millis(4200));
        assert!(pool.try_recv().is_none());
    }

    #[test]
    fn a_request_being_rendered_is_not_rendered_again() {
        use super::*;
        let pool = RenderPool::new(Arc::new(ONE_PAGE_TWICE.to_vec()), 2, RenderConfig::default());
        *lock(&pool.shared.slow_page) = Some((0, std::time::Duration::from_millis(1000)));
        let req = |page| RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale: 0.5, tag: 0 };
        pool.set_queue(vec![req(0)]);
        let t = std::time::Instant::now();
        while !lock(&pool.shared.busy).iter().flatten().any(|busy| busy.request.page == 0) {
            assert!(t.elapsed() < std::time::Duration::from_secs(8), "page 1 never started");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // What the canvas sends next: the page it is still waiting for, then another one.
        pool.set_queue(vec![req(0), req(1)]);
        let mut answers = Vec::new();
        let t = std::time::Instant::now();
        while answers.len() < 2 || lock(&pool.shared.busy).iter().any(Option::is_some) {
            assert!(t.elapsed() < std::time::Duration::from_secs(8), "answers so far: {answers:?}");
            if let Some(p) = pool.try_recv() {
                assert!(p.error.is_none(), "{:?}", p.error);
                answers.push(p.request.page);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        while let Some(p) = pool.try_recv() {
            answers.push(p.request.page);
        }
        // Page 2 went to the idle worker instead of waiting behind a second copy of page 1.
        assert_eq!(answers, vec![1, 0]);
        assert_eq!(pool.shared.render_started.load(Ordering::Relaxed), 2, "each page rendered once");
    }

    #[test]
    fn re_sent_queues_render_each_page_once() {
        use super::*;
        let pages = 12;
        let mut pdf = format!(
            "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 200 200] >> endobj\n",
            (0..pages).map(|i| format!("{} 0 R", 4 + i)).collect::<Vec<_>>().join(" ")
        );
        let body: String = (0..3000).map(|k| format!("{} {} 5 5 re f\n", k * 7 % 195, k * 13 % 195)).collect();
        pdf += &format!("3 0 obj << /Length {} >> stream\n{body}endstream endobj\n", body.len());
        for i in 0..pages {
            pdf += &format!("{} 0 obj << /Type /Page /Parent 2 0 R /Contents 3 0 R >> endobj\n", 4 + i);
        }
        pdf += "trailer << /Root 1 0 R >>\n%%EOF";
        let pool = RenderPool::new(Arc::new(pdf.into_bytes()), 3, RenderConfig::default());
        let wanted: Vec<RenderRequest> =
            (0..pages).map(|page| RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale: 2.0, tag: 2000 }).collect();
        // The canvas's loop: take what arrived, then queue every page still missing whenever
        // that list changes (requests already rendering included).
        let mut got = vec![0; pages];
        let mut last = Vec::new();
        let t = std::time::Instant::now();
        while got.contains(&0) || lock(&pool.shared.busy).iter().any(Option::is_some) {
            assert!(t.elapsed() < std::time::Duration::from_secs(30), "rendered so far: {got:?}");
            while let Some(p) = pool.try_recv() {
                assert!(p.error.is_none(), "{:?}", p.error);
                got[p.request.page] += 1;
            }
            let queue: Vec<RenderRequest> = wanted.iter().copied().filter(|r| got[r.page] == 0).collect();
            if queue != last {
                pool.set_queue(queue.clone());
                last = queue;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        while let Some(p) = pool.try_recv() {
            got[p.request.page] += 1;
        }
        assert_eq!(got, vec![1; pages]);
        assert_eq!(pool.shared.render_started.load(Ordering::Relaxed), pages, "no duplicated work was hidden by delivery filtering");
    }

    #[test]
    fn effective_scale_never_exceeds_the_caps() {
        for (w, h, s) in [(934_775_807.0, 792.0, 0.25), (612.0, 792.0, 0.0), (1.0e9, 1.0e9, 1.0), (612.0, 792.0, 2.0)] {
            let k = super::effective_scale(w, h, s);
            assert!(k > 0.0 && k.is_finite(), "{w}×{h} @ {s}: {k}");
            assert!(w * k <= super::MAX_SIDE + 1.0 && h * k <= super::MAX_SIDE + 1.0, "{w}×{h} @ {s}: {k}");
            assert!(w * k * h * k <= super::MAX_PIXELS * 1.01, "{w}×{h} @ {s}: {k}");
        }
        assert_eq!(super::effective_scale(612.0, 792.0, 2.0), 2.0);
    }

    use super::*;

    // Metadata-only: the test budget is 128 pixels and no image buffer is allocated.
    #[test]
    fn image_resampling_rejects_anisotropic_target_growth() {
        assert_eq!(hayro::image_resampling_size(16, 1, 1, 16, 0.5, 32.0, 128), None);
        assert_eq!(hayro::image_resampling_size(16, 1, 3, 48, 0.5, 32.0, 128), None);
        assert_eq!(hayro::image_resampling_size(16, 1, 4, 64, 0.5, 32.0, 128), None);
    }

    #[test]
    fn image_resampling_bounds_intermediate_before_planning() {
        // Source and destination are each 128 pixels, but their crossed dimensions are 256.
        assert_eq!(hayro::image_resampling_size(16, 8, 4, 512, 0.5, 2.0, 128), None);
    }

    #[test]
    fn image_resampling_rejects_invalid_sources_and_scales() {
        assert_eq!(hayro::image_resampling_size(4, 4, 3, 47, 0.5, 0.5, 128), None);
        assert_eq!(hayro::image_resampling_size(4, 4, 3, 49, 0.5, 0.5, 128), None);
        for scale in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0] {
            assert_eq!(hayro::image_resampling_size(4, 4, 3, 48, 0.5, scale, 128), None);
        }
    }

    #[test]
    fn image_resampling_keeps_valid_sizes_and_exact_limits() {
        assert_eq!(hayro::image_resampling_size(16, 8, 1, 128, 0.5, 0.5, 128), Some((8, 4)));
        assert_eq!(hayro::image_resampling_size(16, 1, 3, 48, 0.5, 8.0, 128), Some((8, 8)));
        assert_eq!(hayro::image_resampling_size(16, 8, 4, 512, 1.0, 1.0, 128), Some((16, 8)));
        assert_eq!(hayro::image_resampling_size(0, 8, 1, 0, 1.0, 1.0, 128), None);
        assert_eq!(hayro::image_resampling_size(16, 9, 1, 144, 1.0, 1.0, 128), None);
        assert_eq!(hayro::image_resampling_size(65_536, 1, 1, 65_536, 1.0, 1.0, u64::MAX), None);
        for channels in [1, 3, 4] {
            assert_eq!(hayro::image_resampling_size(65_536, 1, channels, 65_536 * channels, 1.0 / 4096.0, 1.0, 65_536), Some((16, 1)));
        }
        assert_eq!(hayro::image_resampling_size(65_535, 1, 1, 65_535, 1.0, 1.0, 65_535), Some((65_535, 1)));
        assert_eq!(hayro::image_resampling_size((1 << 20) + 1, 1, 1, (1 << 20) + 1, 1.0 / 4096.0, 1.0, 1 << 28), None);
    }

    #[test]
    fn image_resampling_padded_backend_dimensions_stay_checked() {
        // A Type 3 image's two-pixel frame may reach u16::MAX exactly, never wrap to zero.
        assert_eq!(hayro::image_resampling_size(65_531 + 4, 1 + 4, 4, 65_535 * 5 * 4, 1.0, 1.0, 65_535 * 5), Some((65_535, 5)));
        assert_eq!(hayro::image_resampling_size(65_531 + 4, 1 + 4, 4, 65_535 * 5 * 4, 1.0, 1.0, 65_535 * 5 - 1), None);
        assert_eq!(hayro::image_resampling_size(65_532 + 4, 1 + 4, 4, 65_536 * 5 * 4, 1.0, 1.0, 1 << 28), None);
    }
    fn strip_image_pdf(width: u32, body: &str, space: &str, encoded: &str, alpha: Option<&str>) -> Vec<u8> {
        let (mask_ref, mask_obj) = match alpha {
            Some(data) => (
                "/SMask 6 0 R",
                format!(
                    "6 0 obj << /Type /XObject /Subtype /Image /Width {width} /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /ASCIIHexDecode /Length {} >> stream\n{data}>\nendstream endobj\n",
                    data.len() + 1
                ),
            ),
            None => ("", String::new()),
        };
        format!(
            "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 20 4] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >> endobj\n\
             4 0 obj << /Length {} >> stream\n{body}endstream endobj\n\
             5 0 obj << /Type /XObject /Subtype /Image /Width {width} /Height 1 /ColorSpace /{space} /BitsPerComponent 8 {mask_ref} /Filter /ASCIIHexDecode /Length {} >> stream\n{encoded}>\nendstream endobj\n\
             {mask_obj}trailer << /Root 1 0 R >>\n%%EOF", body.len(), encoded.len() + 1,
        ).into_bytes()
    }

    // Safe on the original renderer too: 192 KiB RGB / 64 KiB gray sources shrink to 16 pixels.
    #[test]
    fn image_resampling_wide_sources_shrink_before_backend_side_limits() {
        for (space, encoded, expected) in
            [("DeviceRGB", "ff0000".repeat(65_536), [255, 0, 0, 255]), ("DeviceGray", "7f".repeat(65_536), [127, 127, 127, 255])]
        {
            let pdf = strip_image_pdf(65_536, "q 16 0 0 1 2 2 cm /Im0 Do Q\n", space, &encoded, None);
            let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
            let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..Default::default() });
            assert!(page.error.is_none(), "{space}: {:?}", page.error);
            assert_eq!((page.width, page.height), (20, 4));
            let pixel = |x: usize| &page.rgba[(20 + x) * 4..][..4];
            assert_eq!(pixel(1), &[255, 255, 255, 255], "{space}: left placement");
            assert_eq!(pixel(3), &expected, "{space}: source image was not dropped");
            assert_eq!(pixel(17), &expected, "{space}: right placement");
            assert_eq!(pixel(19), &[255, 255, 255, 255], "{space}: right placement");
        }
    }

    #[test]
    fn image_resampling_wide_transparent_sources_still_shrink() {
        let data = "ff0000".repeat(65_536);
        let alpha = "7f".repeat(65_536);
        let pdf = strip_image_pdf(65_536, "q 16 0 0 1 2 2 cm /Im0 Do Q\n", "DeviceRGB", &data, Some(&alpha));
        let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
        let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..Default::default() });
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!((page.width, page.height), (20, 4));
        assert_eq!(&page.rgba[(20 + 3) * 4..][..4], &[255, 128, 128, 255]);
    }

    #[test]
    fn image_resampling_mismatched_alpha_mask_keeps_pixels_and_placement() {
        let data = "ff0000".repeat(16);
        let alpha = "7f".repeat(8);
        let pdf = strip_image_pdf(16, "q 16 0 0 1 2 2 cm /Im0 Do Q\n", "DeviceRGB", &data, Some(&alpha));
        let pdf = String::from_utf8(pdf)
            .unwrap()
            .replace("6 0 obj << /Type /XObject /Subtype /Image /Width 16", "6 0 obj << /Type /XObject /Subtype /Image /Width 8");
        let mut renderer = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
        let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..Default::default() });
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!((page.width, page.height), (20, 4));
        let pixel = |x: usize| &page.rgba[(20 + x) * 4..][..4];
        assert_eq!(pixel(1), &[255, 255, 255, 255]);
        assert_eq!(pixel(3), &[255, 128, 128, 255]);
        assert_eq!(pixel(17), &[255, 128, 128, 255]);
        assert_eq!(pixel(19), &[255, 255, 255, 255]);
    }
    // GREEN-only integration: the original renderer would attempt GiB buffers. The source is
    // only 96 KiB, and the fixed guard rejects the target before the resampling plan/allocation.
    #[test]
    fn resampling_fallback_preserves_original_geometry_and_pixels() {
        let data = "ff0000".repeat(32_767);
        let body = "q 16383.5 0 0 1000000000 2 2 cm /Im0 Do Q\n";
        let pdf = format!(
            "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >> endobj\n\
             4 0 obj << /Length {} >> stream\n{body}endstream endobj\n\
             5 0 obj << /Type /XObject /Subtype /Image /Width 32767 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Interpolate true /Filter /ASCIIHexDecode /Length {} >> stream\n{data}>\nendstream endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF",
            body.len(),
            data.len() + 1,
        );
        let mut renderer = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
        let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..Default::default() });
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!((page.width, page.height), (20, 20));
        let pixel = |x: usize, y: usize| &page.rgba[(y * 20 + x) * 4..][..4];
        assert_eq!(pixel(0, 5), &[255, 255, 255, 255]);
        assert_eq!(pixel(3, 5), &[255, 0, 0, 255]);
        assert_eq!(pixel(19, 5), &[255, 0, 0, 255]);
    }

    const ONE_PAGE: &[u8] = b"%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Contents 4 0 R >> endobj
4 0 obj << /Length 35 >> stream
0 0 1 rg 10 10 30 20 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";

    const ONE_PAGE_TWICE: &[u8] = b"%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Contents 4 0 R >> endobj
4 0 obj << /Length 35 >> stream
0 0 1 rg 10 10 30 20 re f
endstream endobj
5 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Contents 4 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    #[test]
    fn large_resource_indexes_preserve_pixels_and_text() {
        // #307: opening pages used to retain a hash table for every large resource
        // dictionary on every page, even before rendering. The vendored lazy index
        // must still resolve late keys, inherited resources and repeated renders.
        fn fixture(large: bool) -> Vec<u8> {
            let mut fonts = String::new();
            let mut forms = String::new();
            if large {
                for i in 0..100 {
                    fonts.push_str(&format!("/UnusedFont{i} 6 0 R "));
                    forms.push_str(&format!("/UnusedForm{i} 7 0 R "));
                }
            }
            let body = "BT /F1 12 Tf 10 55 Td (Shared resources) Tj ET q 1 0 0 1 10 10 cm /Last Do Q";
            let form = "0 0 1 rg 0 0 30 20 re f";
            format!(
                "%PDF-1.7\n\
                 1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
                 2 0 obj << /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 /MediaBox [0 0 150 100] /Resources 5 0 R >> endobj\n\
                 3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj\n\
                 4 0 obj << /Length {} >> stream\n{body}\nendstream endobj\n\
                 5 0 obj << /Font << {fonts}/F1 6 0 R >> /XObject << {forms}/Last 7 0 R >> >> endobj\n\
                 6 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n\
                 7 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 30 20] /Length {} >> stream\n{form}\nendstream endobj\n\
                 8 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources 5 0 R /Rotate 90 >> endobj\n\
                 trailer << /Root 1 0 R >>\n%%EOF",
                body.len(),
                form.len()
            )
            .into_bytes()
        }
        let mut small = PageRenderer::new(Arc::new(fixture(false)), RenderConfig::default());
        let mut large = PageRenderer::new(Arc::new(fixture(true)), RenderConfig::default());
        for page in [0, 1, 0] {
            for scale in [1.0, 2.0] {
                let req = RenderRequest { page, scale, ..Default::default() };
                let (a, b) = (small.render(req), large.render(req));
                assert!(a.error.is_none() && b.error.is_none());
                assert_eq!((a.width, a.height, &a.rgba), (b.width, b.height, &b.rgba));
                assert!(b.rgba.as_chunks::<4>().0.contains(&[0, 0, 255, 255]));
            }
            let req = RenderRequest { page, kind: RequestKind::Text, ..Default::default() };
            let (a, b) = (small.render(req), large.render(req));
            assert!(a.error.is_none() && b.error.is_none());
            assert_eq!(a.text.as_ref().unwrap().plain_text(), b.text.as_ref().unwrap().plain_text());
            assert!(b.text.as_ref().unwrap().plain_text().contains("Shared resources"));
        }
    }

    #[test]
    fn renders_a_blue_rectangle() {
        let pool = RenderPool::new(Arc::new(ONE_PAGE.to_vec()), 1, RenderConfig::default());
        pool.set_queue(vec![RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 7 }]);
        let page = loop {
            if let Some(p) = pool.try_recv() {
                break p;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!((page.width, page.height, page.request.tag), (100, 50, 7));
        // Pixel (20, 30) in y-down device space is inside the rect drawn at y 10..30 (y-up).
        let px = |x: u32, y: u32| &page.rgba[((y * page.width + x) * 4) as usize..][..4];
        assert_eq!(px(20, 30), &[0, 0, 255, 255]);
        assert_eq!(px(80, 5), &[255, 255, 255, 255]);
    }

    #[test]
    fn pixels_are_a_plain_renders_bytes_in_a_buffer_a_gui_can_take_over() {
        let mut r = PageRenderer::new(Arc::new(ONE_PAGE.to_vec()), RenderConfig::default());
        let out = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 2.0, tag: 0 });
        assert!(out.error.is_none(), "{:?}", out.error);
        // The same bytes as hayro's own render to a pixmap, with the same settings.
        let pdf = Pdf::new(Arc::new(ONE_PAGE.to_vec())).expect("parses");
        let pages = pdf.pages();
        let page = pages.first().expect("a page");
        let rs = RenderSettings { x_scale: 2.0, y_scale: 2.0, width: Some(200), height: Some(100), bg_color: WHITE, ..Default::default() };
        let plain = hayro::render(page, &RenderCache::new(), &RenderConfig::default().settings(), &rs);
        assert_eq!((out.width, out.height), (200, 100));
        assert_eq!(&*out.rgba, plain.data_as_u8_slice());
        // Whole pixels, aligned, handed over without a copy.
        let at = out.rgba.as_ptr();
        assert_eq!(at as usize % 4, 0);
        let words = out.rgba.into_words();
        assert_eq!(words.as_ptr().cast::<u8>(), at);
    }

    #[test]
    fn render_into_a_buffer_of_the_wrong_length_leaves_it_untouched() {
        let pdf = Pdf::new(Arc::new(ONE_PAGE.to_vec())).expect("parses");
        let pages = pdf.pages();
        let page = pages.first().expect("a page");
        let rs = RenderSettings { width: Some(10), height: Some(10), bg_color: WHITE, ..Default::default() };
        let settings = RenderConfig::default().settings();
        for len in [0, 399, 401, 4000] {
            let mut buf = vec![7u8; len];
            render_into(page, &RenderCache::new(), &settings, &rs, &mut buf);
            assert!(buf.iter().all(|&b| b == 7), "a {len}-byte buffer was written");
        }
    }

    #[test]
    fn missing_page_and_garbage_input_fail_gracefully() {
        let mut r = PageRenderer::new(Arc::new(ONE_PAGE.to_vec()), RenderConfig::default());
        assert!(r.render(RenderRequest { page: 9, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }).error.is_some());
        let mut g = PageRenderer::new(Arc::new(b"not a pdf at all".to_vec()), RenderConfig::default());
        assert_eq!(g.page_count(), 0);
        assert!(g.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }).error.is_some());
    }

    #[test]
    fn scale_is_clamped_for_huge_pages() {
        // A 200-inch-square page at 4x would be 57,600 px per side.
        let s = effective_scale(14_400.0, 14_400.0, 4.0);
        assert!(14_400.0 * s <= MAX_SIDE + 0.5);
        assert!((14_400.0 * s).powi(2) <= MAX_PIXELS * 1.01);
    }

    /// Issue #102: an A4 page (595.28×841.89 pt) rendered 595×841 at 72 dpi, dropping the last
    /// partial row and column. Like pdftoppm it is now 596×842, the edge pixels partly covered.
    #[test]
    fn fractional_page_sizes_round_up() {
        let pdf = b"%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 595.28 841.89] /Contents 4 0 R >> endobj
4 0 obj << /Length 31 >> stream
0 0 1 rg 0 0 595.28 841.89 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";
        let mut r = PageRenderer::new(Arc::new(pdf.to_vec()), RenderConfig::default());
        let req = RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 };
        let p = r.render(req);
        assert!(p.error.is_none(), "{:?}", p.error);
        assert_eq!((p.width, p.height), (596, 842));
        let px = |x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..4].to_vec();
        assert_eq!(px(594, 840), vec![0, 0, 255, 255]);
        // The partial column and row blend the fill into the white page.
        for (x, y) in [(595, 400), (300, 841), (595, 841)] {
            let c = px(x, y);
            assert!(c[0] < 255 && c[2] == 255, "({x}, {y}) is not partly covered: {c:?}");
        }
        assert_eq!(r.render(RenderRequest { scale: 300.0 / 72.0, ..req }).width, 2481);
        // A tile over the corner draws the same edge pixels.
        let t = r.render(RenderRequest { tile: Some(Tile { x: 594, y: 840, w: 2, h: 2 }), ..req });
        assert_eq!((t.width, t.height), (2, 2));
        assert_eq!(&t.rgba[12..16], &px(595, 841)[..]);
        // Float noise doesn't add a row: Letter at 150 dpi is 1275 px wide.
        assert_eq!((device_pixels(612.0, 150.0 / 72.0), device_pixels(0.2, 1.0), device_pixels(f32::NAN, 1.0)), (1275, 1, 1));
    }

    /// A checkbox whose `/AP /N` is a state dictionary must draw the `/AS` state; a NoView
    /// annotation must not draw at all. (Regression test for the vendored hayro patch.)
    #[test]
    fn widget_appearance_states() {
        let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Annots [4 0 R 7 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Widget /FT /Btn /T (cb) /V /Yes /AS /Yes /Rect [10 10 30 30]
   /AP << /N << /Yes 5 0 R /Off 6 0 R >> >> >> endobj
5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 24 >> stream
1 0 0 rg 0 0 20 20 re f
endstream endobj
6 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 24 >> stream
0 0 1 rg 0 0 20 20 re f
endstream endobj
7 0 obj << /Type /Annot /Subtype /Square /F 32 /Rect [60 60 90 90] /AP << /N 8 0 R >> >> endobj
8 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 30 30] /Length 24 >> stream
0 1 0 rg 0 0 30 30 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";
        let mut r = PageRenderer::new(Arc::new(pdf.to_vec()), RenderConfig::default());
        let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
        assert!(p.error.is_none(), "{:?}", p.error);
        let px = |x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..4].to_vec();
        assert_eq!(px(20, 80), vec![255, 0, 0, 255], "checkbox must show its /Yes state");
        assert_eq!(px(75, 25), vec![255, 255, 255, 255], "NoView annotation must not be drawn");
    }

    /// Quartz writes `/AP /N` as an indirect dictionary of states, and the next object in
    /// the file is often some other stream. That dictionary must not be read as that stream,
    /// or the checkbox (whose real appearance is `/AS`) is skipped.
    #[test]
    fn indirect_appearance_state_dict_is_not_another_stream() {
        let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Widget /FT /Btn /T (cb) /V /Yes /AS /Yes /Rect [10 10 30 30]
   /AP 9 0 R >> endobj
9 0 obj << /N 10 0 R >> endobj
10 0 obj << /Yes 5 0 R /Off 6 0 R >> endobj
11 0 obj << /Length 24 >> stream
0 0 1 rg 0 0 20 20 re f
endstream
endobj
5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 24 >> stream
1 0 0 rg 0 0 20 20 re f
endstream
endobj
6 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 24 >> stream
0 1 0 rg 0 0 20 20 re f
endstream
endobj
trailer << /Root 1 0 R >>
%%EOF";
        let mut r = PageRenderer::new(Arc::new(pdf.to_vec()), RenderConfig::default());
        let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
        assert!(p.error.is_none(), "{:?}", p.error);
        let px = |x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..4].to_vec();
        assert_eq!(px(20, 80), vec![255, 0, 0, 255], "checkbox must show the /Yes appearance, not the decoy stream");
    }

    /// An appearance whose /BBox is a line or a point, or whose /Matrix collapses it to one, has
    /// no mapping onto the annotation's /Rect: the scale (/Rect size over transformed box size)
    /// divided by zero, and the infinite or NaN matrix reached the device with everything the
    /// appearance drew. Vendored hayro-interpret patch: such appearances are skipped. Clipped to
    /// their box they show nothing, and MuPDF and Poppler draw nothing for them either.
    #[test]
    fn degenerate_annotation_appearances_are_skipped() {
        use hayro::hayro_interpret::font::Glyph;
        use hayro::hayro_interpret::hayro_syntax::Pdf;
        use hayro::hayro_interpret::{
            BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, Paint, PathDrawMode, SoftMask, TransformExt, interpret_page,
        };
        use kurbo::{Affine, BezPath, Rect};

        /// Counts the geometry that reaches a device, and how much of it is not finite.
        #[derive(Default)]
        struct Geometry {
            drawn: usize,
            non_finite: usize,
        }
        impl Geometry {
            fn see(&mut self, finite: bool) {
                self.drawn += 1;
                self.non_finite += usize::from(!finite);
            }
        }
        impl<'a> Device<'a> for Geometry {
            fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
            fn set_blend_mode(&mut self, _: BlendMode) {}
            fn draw_path(&mut self, path: &BezPath, transform: Affine, _: &Paint<'a>, _: &PathDrawMode) {
                self.see(path.is_finite() && transform.is_finite());
            }
            fn push_clip_path(&mut self, clip: &ClipPath) {
                self.see(clip.path.is_finite());
            }
            fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
            fn draw_glyph(&mut self, _: &Glyph<'a>, transform: Affine, glyph_transform: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {
                self.see(transform.is_finite() && glyph_transform.is_finite());
            }
            fn draw_image(&mut self, _: Image<'a, '_>, transform: Affine) {
                self.see(transform.is_finite());
            }
            fn pop_clip_path(&mut self) {}
            fn pop_transparency_group(&mut self) {}
        }

        let pdf = |form: &str| {
            let content = "1 0 0 rg 0 0 20 20 re f BT /F1 12 Tf 2 5 Td (Hi) Tj ET";
            format!(
                "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Square /Rect [10 10 60 40] /AP << /N 5 0 R >> >> endobj
5 0 obj << /Type /XObject /Subtype /Form {form} /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> /Length {} >> stream
{content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
                content.len()
            )
        };
        let geometry = |bytes: &[u8]| {
            let parsed = Pdf::new(Arc::new(bytes.to_vec())).expect("parses");
            let page = &parsed.pages()[0];
            let cache = InterpreterCache::new();
            let initial = page.initial_transform(true).to_kurbo();
            let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, 100.0, 100.0), &cache, page.xref(), InterpreterSettings::default());
            let mut device = Geometry::default();
            interpret_page(page, &mut ctx, &mut device);
            device
        };
        let render = |bytes: Vec<u8>| {
            let mut r = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
            r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 })
        };
        for (case, form) in [
            ("zero-width /BBox", "/BBox [0 0 0 20]"),
            ("zero-height /BBox", "/BBox [0 0 20 0]"),
            ("point /BBox", "/BBox [5 5 5 5]"),
            ("/Matrix with a zero column", "/BBox [0 0 20 20] /Matrix [0 0 0 1 0 0]"),
            ("/Matrix with a zero row", "/BBox [0 0 20 20] /Matrix [1 0 0 0 0 0]"),
        ] {
            let bytes = pdf(form).into_bytes();
            let device = geometry(&bytes);
            assert_eq!(device.non_finite, 0, "{case}: {} of {} drawing calls were not finite", device.non_finite, device.drawn);
            let p = render(bytes);
            assert!(p.error.is_none(), "{case}: {:?}", p.error);
            assert!(p.rgba.as_chunks::<4>().0.iter().all(|c| *c == [255, 255, 255, 255]), "{case}: nothing is drawn");
        }

        // A valid appearance still reaches the device, and fills its /Rect and only its /Rect.
        let valid = pdf("/BBox [0 0 20 20]").into_bytes();
        let device = geometry(&valid);
        assert!(device.drawn >= 4 && device.non_finite == 0, "{} drawn, {} non-finite", device.drawn, device.non_finite);
        let p = render(valid);
        assert!(p.error.is_none(), "{:?}", p.error);
        let px = |x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..4].to_vec();
        assert_eq!(px(11, 61), vec![255, 0, 0, 255]);
        assert_eq!(px(58, 88), vec![255, 0, 0, 255]);
        assert_eq!(px(61, 75), vec![255, 255, 255, 255], "right of /Rect");
        assert_eq!(px(30, 58), vec![255, 255, 255, 255], "above /Rect");
    }

    /// ISO 32000-2 §12.5.5: the transformed appearance box is scaled and translated so that its
    /// lower-left and upper-right corners land on those of /Rect. The translation ignored the
    /// scale, so a box that doesn't start at the origin and differs in size from /Rect was drawn
    /// shifted, partly outside /Rect (vendored hayro-interpret patch). MuPDF, Poppler, PDFium and
    /// pdf.js fill /Rect exactly for each of these.
    #[test]
    fn annotation_appearance_box_maps_onto_rect() {
        let filled = |form: &str, content: &str| {
            let pdf = format!(
                "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Square /Rect [10 10 60 40] /AP << /N 5 0 R >> >> endobj
5 0 obj << /Type /XObject /Subtype /Form {form} /Length {} >> stream
{content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
                content.len()
            );
            let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
            let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
            assert!(p.error.is_none(), "{form}: {:?}", p.error);
            // The bounding box of everything drawn, in device pixels.
            let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
            for y in 0..p.height {
                for x in 0..p.width {
                    if p.rgba[((y * p.width + x) * 4) as usize..][..4] != [255, 255, 255, 255] {
                        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                    }
                }
            }
            (x0, y0, x1, y1)
        };
        // /Rect [10 10 60 40] is device x 10..=59, y 60..=89.
        let rect = (10, 60, 59, 89);
        for (form, content) in [
            ("/BBox [10 10 30 30]", "1 0 0 rg 10 10 20 20 re f"),
            ("/BBox [0 0 20 20] /Matrix [1 0 0 1 10 10]", "1 0 0 rg 0 0 20 20 re f"),
            ("/BBox [-20 -5 0 15]", "1 0 0 rg -20 -5 20 20 re f"),
            // Unchanged: a box at the origin, and an offset box as large as /Rect.
            ("/BBox [0 0 20 20]", "1 0 0 rg 0 0 20 20 re f"),
            ("/BBox [100 100 150 130]", "1 0 0 rg 100 100 50 30 re f"),
        ] {
            assert_eq!(filled(form, content), rect, "{form}");
        }
    }

    /// A Highlight annotation without an appearance stream (common in older and generated files)
    /// was not drawn at all. Vendored hayro-interpret patch: its /QuadPoints are filled with /C at
    /// /CA, blended with Multiply, as PdfCraft draws its own highlights (pdfcraft-annot); MuPDF,
    /// Poppler and PDFium draw these fixtures the same way. An /AP still wins, malformed
    /// /QuadPoints or no /C draw nothing, and Hidden, NoView and "Hide all comments" still apply.
    #[test]
    fn highlights_without_an_appearance_are_drawn() {
        let render = |annot: &str, config: RenderConfig| {
            let content = "0 g 100 40 10 20 re f";
            let pdf = format!(
                "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Annots [5 0 R] >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Annot /Subtype /Highlight /Rect [15 35 120 65] {annot} >> endobj
6 0 obj << /Type /XObject /Subtype /Form /BBox [15 35 120 65] /Length 24 >> stream
0 0 1 rg 15 35 50 30 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
                content.len()
            );
            let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), config);
            let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
            assert!(p.error.is_none(), "{annot}: {:?}", p.error);
            p
        };
        let px = |p: &RenderedPage, x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..4].to_vec();
        let (white, black, yellow) = (vec![255, 255, 255, 255], vec![0, 0, 0, 255], vec![255, 255, 0, 255]);
        let quad = "/QuadPoints [15 65 120 65 15 35 120 35]";

        let p = render(&format!("{quad} /C [1 1 0]"), RenderConfig::default());
        assert_eq!(px(&p, 30, 50), yellow, "the quad is filled with /C");
        assert_eq!(px(&p, 105, 50), black, "Multiply keeps what is underneath");
        assert_eq!(px(&p, 150, 50), white, "outside the quad");
        assert_eq!(px(&p, 30, 30), white, "above the quad");

        // Two quads (one per line of text); the gap between them stays white.
        let p = render("/QuadPoints [15 65 60 65 15 50 60 50 70 50 120 50 70 35 120 35] /C [0 1 1]", RenderConfig::default());
        assert_eq!(px(&p, 30, 40), vec![0, 255, 255, 255]);
        assert_eq!(px(&p, 90, 60), vec![0, 255, 255, 255]);
        assert_eq!((px(&p, 90, 40), px(&p, 30, 60)), (white.clone(), white.clone()));

        // /CA and gray /C.
        let p = render(&format!("{quad} /C [1 0 0] /CA 0.5"), RenderConfig::default());
        let half = px(&p, 30, 50);
        assert!(half[0] == 255 && (126..=129).contains(&half[1]) && half[1] == half[2], "{half:?}");
        let p = render(&format!("{quad} /C [0.5]"), RenderConfig::default());
        let gray = px(&p, 30, 50);
        assert!((126..=129).contains(&gray[0]) && gray[0] == gray[1] && gray[1] == gray[2], "{gray:?}");

        // An appearance stream wins: the blue form, not a yellow highlight.
        let p = render(&format!("{quad} /C [1 1 0] /AP << /N 6 0 R >>"), RenderConfig::default());
        assert_eq!(px(&p, 30, 50), vec![0, 0, 255, 255]);
        assert_eq!(px(&p, 90, 50), white, "no highlight outside the appearance's own drawing");

        // Nothing is drawn for malformed geometry, no colour, or a hidden annotation.
        let hide = RenderConfig { hide_comments: true, ..RenderConfig::default() };
        for (annot, config) in [
            ("/QuadPoints [10 10 20] /C [1 1 0]", RenderConfig::default()),
            ("/QuadPoints [15 65 120 65 15 35 /x 35] /C [1 1 0]", RenderConfig::default()),
            ("/QuadPoints [] /C [1 1 0]", RenderConfig::default()),
            ("/QuadPoints 7 /C [1 1 0]", RenderConfig::default()),
            (quad, RenderConfig::default()),
            (&format!("{quad} /C []"), RenderConfig::default()),
            (&format!("{quad} /C [1 1]"), RenderConfig::default()),
            (&format!("{quad} /C [1 1 0] /F 2"), RenderConfig::default()),
            (&format!("{quad} /C [1 1 0] /F 32"), RenderConfig::default()),
            (&format!("{quad} /C [1 1 0]"), hide),
        ] {
            let p = render(annot, config);
            assert_eq!(px(&p, 30, 50), white, "{annot}");
            assert_eq!(px(&p, 105, 50), black, "{annot}");
        }
    }

    #[test]
    fn hiding_comments_keeps_fields() {
        let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Annots [4 0 R 6 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (t) /Rect [10 10 30 30] /AP << /N 5 0 R >> >> endobj
5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 24 >> stream
1 0 0 rg 0 0 20 20 re f
endstream endobj
6 0 obj << /Type /Annot /Subtype /Square /F 4 /Rect [60 60 90 90] /AP << /N 7 0 R >> >> endobj
7 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 30 30] /Length 24 >> stream
0 1 0 rg 0 0 30 30 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";
        let render = |hide: bool| {
            let mut r = PageRenderer::new(Arc::new(pdf.to_vec()), RenderConfig { hide_comments: hide, ..RenderConfig::default() });
            r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 })
        };
        let px = |p: &RenderedPage, x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..4].to_vec();
        let shown = render(false);
        assert_eq!(px(&shown, 75, 25), vec![0, 255, 0, 255], "the comment is drawn");
        let hidden = render(true);
        assert_eq!(px(&hidden, 75, 25), vec![255, 255, 255, 255], "Hide all comments");
        assert_eq!(px(&hidden, 20, 80), vec![255, 0, 0, 255], "fields stay");
    }

    #[test]
    fn inline_pool_renders_in_priority_order() {
        let pool = RenderPool::new_inline(Arc::new(ONE_PAGE.to_vec()), RenderConfig::default());
        assert!(pool.is_inline());
        assert!(pool.try_recv().is_none());
        pool.set_queue(vec![
            RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 1 },
            RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 0.5, tag: 2 },
        ]);
        assert_eq!(pool.try_recv().map(|p| p.request.tag), Some(1));
        assert_eq!(pool.try_recv().map(|p| (p.request.tag, p.width)), Some((2, 50)));
        assert!(pool.try_recv().is_none());
    }

    #[test]
    fn dropping_the_pool_cancels_its_renders() {
        const TEXT: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length 35 >> stream
BT /F1 12 Tf 20 70 Td (Hello) Tj ET
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";
        let pool = RenderPool::new(Arc::new(ONE_PAGE.to_vec()), 1, RenderConfig::default());
        let flag = lock(&pool.shared.stop).first().cloned().expect("one worker");
        let settings = worker_settings(&pool.config, &flag);
        assert!(settings.cancelled.as_ref().is_some_and(|c| Arc::ptr_eq(c, &flag)));
        let shapes = Pdf::new(Arc::new(ONE_PAGE.to_vec())).expect("parses");
        let text = Pdf::new(Arc::new(TEXT.to_vec())).expect("parses");
        let pages = shapes.pages();
        let page = pages.first().expect("a page");
        let rs = RenderSettings { bg_color: WHITE, ..Default::default() };
        let blue = |s: &InterpreterSettings| {
            let p = hayro::render(page, &RenderCache::new(), s, &rs).sample(20, 30);
            [p.r, p.g, p.b, p.a] == [0, 0, 255, 255]
        };
        let glyphs = |s: &InterpreterSettings| crate::text::extract_page(&text, 0, s).map_or(0, |t| t.glyphs.len());
        assert!(blue(&settings) && glyphs(&settings) == 5, "a live pool renders and reads everything");
        drop(pool);
        assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!blue(&settings) && glyphs(&settings) == 0, "once the pool is gone, interpretation stops before drawing");
    }

    #[test]
    fn the_watchdog_stops_the_render_it_gives_up_on() {
        let mut pool = RenderPool::new(Arc::new(ONE_PAGE_TWICE.to_vec()), 1, RenderConfig::default());
        // Private parsers: a shared render's first timeout is retried privately, which would
        // give up on (and stop) a second worker too.
        *lock(&pool.shared.parser.state) = ParserState::Private;
        pool.shared.parser.valid.store(false, Ordering::Release);
        pool.set_stuck_after(std::time::Duration::from_millis(100));
        *lock(&pool.shared.slow_page) = Some((0, std::time::Duration::from_millis(1500)));
        pool.set_queue(vec![RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 0.5, tag: 0 }]);
        let t = std::time::Instant::now();
        let first = loop {
            if let Some(p) = pool.try_recv() {
                break p;
            }
            assert!(t.elapsed() < std::time::Duration::from_secs(30), "no answer");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(first.error.as_deref().is_some_and(|e| e.contains("took longer")), "{:?}", first.error);
        // The abandoned worker is told to stop at its next operator; its replacement isn't.
        let stop: Vec<bool> = lock(&pool.shared.stop).iter().map(|s| s.load(std::sync::atomic::Ordering::Relaxed)).collect();
        assert_eq!(stop, vec![true, false]);
    }

    #[test]
    fn extracts_text_with_positions() {
        let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length 60 >> stream
BT /F1 12 Tf 20 70 Td (Hello World) Tj 0 -30 Td (Second) Tj ET
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";
        let mut r = PageRenderer::new(Arc::new(pdf.to_vec()), RenderConfig::default());
        let out = r.render(RenderRequest { page: 0, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0 });
        assert!(out.error.is_none(), "{:?}", out.error);
        let t = out.text.expect("text");
        assert_eq!(t.plain_text(), "Hello World\nSecond");
        // "H" sits at x=20, baseline y=70 (y-up) → view y ≈ 100-70 = 30 at the baseline.
        let h = &t.glyphs[0].rect;
        assert!((h[0] - 20.0).abs() < 0.5 && h[1] < 30.0 && h[3] > 30.0, "{h:?}");
        assert_eq!(t.find("world").len(), 1);
    }

    #[test]
    fn layer_override_hides_content() {
        let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] /D << /Order [5 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /Properties << /L1 5 0 R >> >> >> endobj
4 0 obj << /Length 44 >> stream
/OC /L1 BDC 1 0 0 rg 0 0 100 100 re f EMC
endstream endobj
5 0 obj << /Type /OCG /Name (Red) >> endobj
trailer << /Root 1 0 R >>
%%EOF";
        let px = |cfg: RenderConfig| {
            let mut r = PageRenderer::new(Arc::new(pdf.to_vec()), cfg);
            let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 0.5, tag: 0 });
            p.rgba[..4].to_vec()
        };
        assert_eq!(px(RenderConfig::default()), vec![255, 0, 0, 255], "layer is on by default");
        let off = RenderConfig { layers: Arc::new(vec![(5, 0, false)]), ..Default::default() };
        assert_eq!(px(off), vec![255, 255, 255, 255], "toggled-off layer must not draw");
    }

    /// From `cargo xtask fuzz`: a tiling pattern with no /Resources inherits the page's, where
    /// its own name points back at it. It painted itself until the stack overflowed (vendored
    /// hayro-interpret patch: `MAX_PAINT_NESTING`). Ordinary patterns must still paint.
    #[test]
    fn self_referencing_tiling_pattern_terminates() {
        let pdf = |pattern_body: &str| {
            format!(
                "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /Pattern << /P1 5 0 R >> >> >> endobj
4 0 obj << /Length 30 >> stream
/Pattern cs /P1 scn 0 0 40 40 re f
endstream endobj
5 0 obj << /Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Length {} >> stream
{pattern_body}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
                pattern_body.len()
            )
        };
        let render = |doc: String| {
            let mut r = PageRenderer::new(Arc::new(doc.into_bytes()), RenderConfig::default());
            r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 })
        };
        let looped = render(pdf("/Pattern cs /P1 scn 0 0 10 10 re f"));
        assert_eq!((looped.width, looped.height), (40, 40));
        let red = render(pdf("1 0 0 rg 0 0 10 10 re f"));
        assert!(red.error.is_none(), "{:?}", red.error);
        assert_eq!(&red.rgba[((20 * 40 + 20) * 4)..][..4], &[255, 0, 0, 255], "a normal tiling pattern still paints");
    }

    /// From `cargo xtask fuzz`: a Type 3 font without /Resources inherits the page's, where its
    /// own name is defined, and its glyph shows text in itself (vendored patch:
    /// `MAX_PAINT_NESTING`). Must terminate; a normal Type 3 glyph must still paint.
    #[test]
    fn self_referencing_type3_glyph_terminates() {
        let pdf = |proc_body: &str| {
            format!(
                "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length 31 >> stream
BT /F1 20 Tf 10 10 Td (A) Tj ET
endstream endobj
5 0 obj << /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /FirstChar 65 /LastChar 65 /Widths [1]
  /Encoding << /Differences [65 /a] >> /CharProcs << /a 6 0 R >> >> endobj
6 0 obj << /Length {} >> stream
{proc_body}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
                proc_body.len()
            )
        };
        let render = |doc: String| {
            let mut r = PageRenderer::new(Arc::new(doc.into_bytes()), RenderConfig::default());
            r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 })
        };
        let looped = render(pdf("1 0 d0 BT /F1 1 Tf (A) Tj ET 0 0 1 1 re f"));
        assert_eq!((looped.width, looped.height), (40, 40));
        let plain = render(pdf("1 0 d0 0 0 1 1 re f"));
        assert!(plain.error.is_none(), "{:?}", plain.error);
        assert!(plain.rgba.as_chunks::<4>().0.iter().any(|p| p[0] < 128), "a normal Type 3 glyph still paints");
    }

    /// From `cargo xtask fuzz`: a tiling pattern whose /XStep and /YStep dwarf its /BBox got a
    /// cell pixmap of `step × scale` pixels, bounded only by u16 (2 GB and more). The vendored
    /// hayro patch `tiling_cell_scale` keeps the cell within 3000 pixels a side.
    #[test]
    fn tiling_cells_stay_small_whatever_the_step() {
        assert_eq!(hayro::tiling_cell_scale(2.0, 10.0, 3000.0), 2.0, "ordinary cells keep their scale");
        for step in [1.0e5f32, -1.0e5, 4.0e9] {
            let s = hayro::tiling_cell_scale(2.0, step, 3000.0);
            assert!(s * step.abs() <= 3000.5, "{step}: {s}");
        }
        assert_eq!(hayro::tiling_cell_scale(2.0, f32::INFINITY, 3000.0), 2.0);
        assert_eq!(hayro::tiling_cell_scale(2.0, 0.0, 3000.0), 2.0);
        // End to end: such a pattern renders, and the page's other content still draws.
        let content = "/Pattern cs /P1 scn 0 0 40 40 re f 1 0 0 rg 0 0 4 4 re f";
        let pdf = format!(
            "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /Pattern << /P1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 100000 /YStep 100000 /Length 20 >> stream
0 0 1 rg 0 0 10 10 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
            content.len()
        );
        let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
        let page = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255]);
    }

    /// Contributor-original JPEG 2000: one unsigned 8-bit pixel per component, one tile,
    /// no decomposition, reversible wavelet, and one empty packet per component. Empty
    /// coefficients decode to the midpoint (128) after the unsigned DC level shift.
    fn jpx_midpoint_pdf(bpc: u8, components: u16) -> Vec<u8> {
        let mut codestream = vec![0xff, 0x4f, 0xff, 0x51]; // SOC, SIZ.
        codestream.extend_from_slice(&(38 + 3 * components).to_be_bytes());
        codestream.extend_from_slice(&0_u16.to_be_bytes()); // Rsiz.
        for value in [1_u32, 1, 0, 0, 1, 1, 0, 0] {
            codestream.extend_from_slice(&value.to_be_bytes());
        }
        codestream.extend_from_slice(&components.to_be_bytes());
        for _ in 0..components {
            codestream.extend_from_slice(&[7, 1, 1]); // Eight bits, no subsampling.
        }
        // COD: LRCP, one layer, no transform between components, 64x64 codeblocks,
        // zero decomposition levels, reversible wavelet. QCD: no quantization.
        codestream.extend_from_slice(&[0xff, 0x52, 0, 12, 0, 0, 0, 1, 0, 0, 4, 4, 0, 1]);
        codestream.extend_from_slice(&[0xff, 0x5c, 0, 4, 0x40, 0x40]);
        codestream.extend_from_slice(&[0xff, 0x90, 0, 10, 0, 0]); // SOT, tile zero.
        codestream.extend_from_slice(&(14 + u32::from(components)).to_be_bytes());
        codestream.extend_from_slice(&[0, 1, 0xff, 0x93]); // One tile part, SOD.
        codestream.extend(std::iter::repeat_n(0, usize::from(components)));
        codestream.extend_from_slice(&[0xff, 0xd9]); // EOC.

        let content = "q 20 0 0 20 5 5 cm /Im1 Do Q 1 0 0 rg 0 0 4 4 re f";
        let mut pdf = format!(
            "%PDF-1.7\n\
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R >> >> >> endobj\n\
4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
5 0 obj << /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent {bpc} /Filter /JPXDecode /Length {} >> stream\n",
            content.len(),
            codestream.len()
        )
        .into_bytes();
        pdf.extend_from_slice(&codestream);
        pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF");
        pdf
    }

    #[test]
    fn jpx_stream_bit_depth_boundaries_do_not_panic() {
        use hayro::hayro_syntax::bit_reader::BitReader;
        use hayro::hayro_syntax::object::stream::ImageDecodeParams;

        for (bpc, expected) in [(1, 1), (8, 128), (16, 32_896), (31, 1_077_952_576), (32, 2_155_905_152)] {
            let pdf = Pdf::new(Arc::new(jpx_midpoint_pdf(bpc, 1))).unwrap();
            let stream = pdf.objects().into_iter().filter_map(|object| object.into_stream()).find(|stream| !stream.filters().is_empty()).unwrap();
            let decoded = stream.decoded_image(&ImageDecodeParams { bpc: Some(bpc), ..Default::default() }).unwrap();
            let image = decoded.image_data.unwrap();
            assert_eq!((image.width, image.height, image.bits_per_component), (1, 1, bpc));
            assert_eq!(decoded.data.len(), usize::from(bpc).div_ceil(8));
            assert_eq!(BitReader::new(&decoded.data).read(bpc), Some(expected), "{bpc}-bit midpoint");
        }
    }

    #[test]
    fn jpx_streams_reject_out_of_range_bit_depths() {
        use hayro::hayro_syntax::object::stream::ImageDecodeParams;

        for bpc in [0, 33, 255] {
            let pdf = Pdf::new(Arc::new(jpx_midpoint_pdf(bpc, 1))).unwrap();
            let stream = pdf.objects().into_iter().filter_map(|object| object.into_stream()).find(|stream| !stream.filters().is_empty()).unwrap();
            assert!(stream.decoded_image(&ImageDecodeParams { bpc: Some(bpc), ..Default::default() }).is_err());
        }
    }

    #[test]
    fn jpx_gray_rgb_and_alpha_keep_eight_bit_samples() {
        use hayro::hayro_syntax::object::stream::ImageDecodeParams;

        for components in [1, 3, 4] {
            let pdf = Pdf::new(Arc::new(jpx_midpoint_pdf(8, components))).unwrap();
            let stream = pdf.objects().into_iter().filter_map(|object| object.into_stream()).find(|stream| !stream.filters().is_empty()).unwrap();
            let decoded = stream.decoded_image(&ImageDecodeParams::default()).unwrap();
            let expected_colors = usize::from(components.min(3));
            assert_eq!(decoded.data.as_ref(), vec![128; expected_colors]);
            let alpha = decoded.image_data.unwrap().alpha;
            assert_eq!(alpha, (components == 4).then(|| vec![128]));
        }
    }

    #[test]
    fn jpx_32_bit_image_does_not_prevent_other_page_content_from_rendering() {
        for bpc in [8, 32] {
            let mut renderer = PageRenderer::new(Arc::new(jpx_midpoint_pdf(bpc, 1)), RenderConfig::default());
            let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
            assert!(page.error.is_none(), "{bpc} bits: {:?}", page.error);
            assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255], "other page content draws");
            if bpc == 8 {
                assert_eq!(&page.rgba[((25 * 40 + 15) * 4)..][..4], &[128, 128, 128, 255], "the valid image paints gray");
            }
        }
    }

    /// From the nightly `cargo xtask fuzz` (CI caps each child at 4 GiB): a stencil mask claiming
    /// /W 4294967295 and a CCITT image claiming /Columns 4294967295 each allocated 4 GiB while
    /// decoding (locally: 9.6 GB and 4.3 GB). Vendored hayro patches `image_size_ok` and
    /// `ccitt_size_ok` refuse such sizes before decoding; the rest of the page still draws.
    #[test]
    fn absurd_mask_and_fax_sizes_are_refused_before_decoding() {
        assert!(hayro::hayro_interpret::image_size_ok(8000, 8000));
        assert!(!hayro::hayro_interpret::image_size_ok(4_294_967_295, 2));
        assert!(!hayro::hayro_interpret::image_size_ok(0, 10));
        assert!(hayro::hayro_syntax::ccitt_size_ok(1728, 2200), "a fax page");
        assert!(!hayro::hayro_syntax::ccitt_size_ok(4_294_967_295, 26));
        assert!(!hayro::hayro_syntax::ccitt_size_ok(1 << 19, 1 << 12));
        let page = |content: &str, xobject: &str| {
            let pdf = format!(
                "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj {xobject} endobj
trailer << /Root 1 0 R >>
%%EOF",
                content.len()
            );
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
                let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }));
            });
            let page = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("an absurd image must not stall the renderer");
            assert!(page.error.is_none(), "{:?}", page.error);
            assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255], "the rest of the page draws");
        };
        let red = "1 0 0 rg 0 0 4 4 re f";
        page(&format!("q 20 0 0 20 5 5 cm BI /W 4294967295 /H 2 /IM true /BPC 1 ID \u{0}\u{ff}\u{ff}\u{0} EI Q {red}"), "<< >>");
        let fax = "<< /Type /XObject /Subtype /Image /Width 81 /Height 26 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms << /Columns 4294967295 /Rows 26 /K -1 >> /Length 4 >> stream\n\u{0}\u{0}\u{0}\u{0}\nendstream";
        page(&format!("q 20 0 0 20 5 5 cm /Im1 Do Q {red}"), fax);
    }

    /// From the nightly `cargo xtask fuzz`: a FlateDecode predictor with `/Columns
    /// 9223372036854775807` wrapped to a 2^61-byte row allocation (an abort on any machine), and
    /// a line width of 9223372036854775807 made stroke expansion allocate 10 GB. Vendored hayro
    /// patches: saturating predictor rows refused when longer than the data, and stroke widths
    /// clamped to a few canvases. The page still renders, with the huge stroke covering it.
    #[test]
    fn absurd_predictor_columns_and_line_widths_render() {
        let render = |streams: &[&str]| {
            let mut objs = String::new();
            let mut refs = Vec::new();
            for (i, s) in streams.iter().enumerate() {
                let n = 4 + i;
                refs.push(format!("{n} 0 R"));
                objs.push_str(&format!("{n} 0 obj {s} endobj\n"));
            }
            let pdf = format!(
                "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents [{}] >> endobj
{objs}trailer << /Root 1 0 R >>
%%EOF",
                refs.join(" ")
            );
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
                let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }));
            });
            let page = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("must not stall the renderer");
            assert!(page.error.is_none(), "{:?}", page.error);
            page
        };
        let plain = |content: &str| format!("<< /Length {} >> stream\n{content}\nendstream", content.len());
        // Stored (uncompressed) zlib data, so the stream needs no encoder: header, one final
        // stored block of two bytes, checksum.
        let predicted = "<< /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 9223372036854775807 >> /Length 13 >> stream\nx\u{1}\u{1}\u{2}\u{0}\u{fd}\u{ff}\u{0}\u{0}\u{0}\u{1}\u{0}\u{1}\nendstream";
        let page = render(&[predicted, &plain("1 0 0 rg 0 0 4 4 re f")]);
        assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255], "the other content stream draws");
        let page = render(&[&plain("1 0 0 RG 9223372036854775807 w 10 20 m 30 20 l S")]);
        // Butt caps: the stroke covers the band over the segment (x 10 to 30) top to bottom.
        for (x, y) in [(20, 0), (20, 39), (12, 0), (28, 39)] {
            assert_eq!(&page.rgba[((y * 40 + x) * 4)..][..4], &[255, 0, 0, 255], "({x}, {y}) under the huge stroke");
        }
        assert_eq!(&page.rgba[((20 * 40 + 2) * 4)..][..4], &[255, 255, 255, 255], "beyond the butt cap");
    }

    /// Render a 40 × 40 pt page at 1:1 on its own thread, with `/GS1` (from `gs1`) in its
    /// resources, and fail instead of waiting when the renderer stalls.
    fn render_with_gs1(content: &str, gs1: &str) -> RenderedPage {
        let pdf = format!(
            "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /ExtGState << /GS1 {gs1} >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
            content.len()
        );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
            let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }));
        });
        let page = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("must not stall the renderer");
        assert!(page.error.is_none(), "{:?}", page.error);
        page
    }

    /// The pixel at device (x, y), y down, of a 40-pixel-wide page.
    fn px40(page: &RenderedPage, x: usize, y: usize) -> &[u8] {
        &page.rgba[((y * 40 + x) * 4)..][..4]
    }

    /// From `cargo xtask fuzz`: an ExtGState `/D [[11] 0]` on a line that starts at x =
    /// 92234775807 cut it into billions of dashes, and stroke expansion allocated more than
    /// 5 GB. A tiny dash on an ordinary line does the same (`[0.000001] 0 d` on 20 pt passed
    /// 10 GB), and so does a long pattern on many short subpaths, because every subpath restarts
    /// it. Vendored hayro patch: a stroke whose dash pattern could cut it into more than
    /// `MAX_DASHES_PER_STROKE` pieces is drawn solid.
    #[test]
    fn dash_patterns_that_cut_a_stroke_into_billions_of_pieces_render() {
        let red = [255, 0, 0, 255];
        // The fuzzed shape. A line that far out isn't drawn even undashed (its coordinates are
        // beyond the rasterizer's range), so this case only proves the page finishes and the
        // content after the stroke is drawn.
        let page = render_with_gs1("1 0 0 RG 4 w /GS1 gs 92234775807 20 m 30 20 l S 0 0 1 rg 0 0 4 4 re f", "<< /D [[11] 0] >>");
        assert_eq!(px40(&page, 1, 38), &[0, 0, 255, 255], "the square after the stroke is drawn");
        for (what, content, gs1) in [
            ("a tiny dash on an ordinary line", "1 0 0 RG 4 w [0.000001] 0 d 10 20 m 30 20 l S", "<< >>"),
            ("a tiny dash from an ExtGState", "1 0 0 RG 4 w /GS1 gs 10 20 m 30 20 l S", "<< /D [[0.000001] 0] >>"),
            (
                "a tiny dash under a shrinking matrix",
                "1 0 0 RG 0.0000001 0 0 0.0000001 0 0 cm 40000000 w [1] 0 d 100000000 200000000 m 300000000 200000000 l S",
                "<< >>",
            ),
        ] {
            let page = render_with_gs1(content, gs1);
            // Device row 20 is user y 20; the stroke covers x 10 to 30.
            for x in [12, 20, 28] {
                assert_eq!(px40(&page, x, 20), &red, "{what}: ({x}, 20) is drawn, solid");
            }
            assert_eq!(px40(&page, 35, 20), &[255, 255, 255, 255], "{what}: beyond the end of the line");
        }
        // A long pattern on many short subpaths: 9,999 tiny dashes, then a long one, restarted on
        // each of 10,000 subpaths 0.01 pt long, is 100 million pieces from a 300 KB stream.
        let mut many = String::from("1 0 0 RG 4 w [");
        many.push_str(&"0.000001 ".repeat(9_999));
        many.push_str("1000] 0 d ");
        many.push_str(&"10 20 m 10.01 20 l ".repeat(10_000));
        many.push_str("S 0 0 1 rg 0 0 4 4 re f");
        let page = render_with_gs1(&many, "<< >>");
        assert_eq!(px40(&page, 1, 38), &[0, 0, 255, 255], "a pattern restarted on many subpaths finishes");
        // A fine pattern below the cap still dashes: 200,000 pieces of 0.0001 pt cover about
        // half of each pixel, so the line is neither solid red nor missing.
        let page = render_with_gs1("1 0 0 RG 4 w [0.0001] 0 d 10 20 m 30 20 l S", "<< >>");
        let p = px40(&page, 20, 20);
        assert!(p[0] == 255 && (40..=215).contains(&p[1]), "a fine dash below the cap is drawn dashed: {p:?}");
        // An ordinary dash pattern still dashes: butt-capped 4 pt dashes from x 0.
        let page = render_with_gs1("1 0 0 RG 4 w [4 4] 0 d 0 20 m 40 20 l S", "<< >>");
        assert_eq!(px40(&page, 2, 20), &red, "inside the first dash");
        assert_eq!(px40(&page, 6, 20), &[255, 255, 255, 255], "inside the first gap");
        assert_eq!(px40(&page, 10, 20), &red, "inside the second dash");
    }

    /// From `cargo xtask fuzz` triage: a dash array whose entries sum to less than zero made
    /// kurbo's search for the starting dash loop for ever (`[-1 -1] 0 d` never finished).
    /// ISO 32000-2 §8.4.3.6 requires nonnegative entries. Vendored hayro patch: a pattern with a
    /// negative entry, or a period that isn't positive, is drawn solid.
    #[test]
    fn invalid_dash_patterns_terminate_and_are_drawn_solid() {
        for (what, content, gs1) in [
            ("[-1 -1]", "1 0 0 RG 4 w [-1 -1] 0 d 10 20 m 30 20 l S", "<< >>"),
            ("[-1] with a phase", "1 0 0 RG 4 w [-1] 5 d 10 20 m 30 20 l S", "<< >>"),
            ("[-2 -3] from an ExtGState", "1 0 0 RG 4 w /GS1 gs 10 20 m 30 20 l S", "<< /D [[-2 -3] 0] >>"),
            ("[-1 3], a negative entry in a positive period", "1 0 0 RG 4 w [-1 3] 0 d 10 20 m 30 20 l S", "<< >>"),
        ] {
            let page = render_with_gs1(content, gs1);
            for x in [12, 20, 28] {
                assert_eq!(px40(&page, x, 20), &[255, 0, 0, 255], "{what}: ({x}, 20) is drawn, solid");
            }
        }
    }

    /// From the nightly `cargo xtask fuzz`: a Type 3 glyph that shows several glyphs of its own
    /// font. The nesting cap bounds the depth but not the breadth: eight glyphs per glyph, sixteen
    /// deep, is 8^16 paints. Vendored hayro-interpret patch: nested paints (forms, Type 3 glyphs,
    /// tiling patterns inside one another) share a per-page budget.
    #[test]
    fn type3_glyphs_that_fan_out_into_themselves_finish() {
        let pdf = "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length 53 >> stream
BT /F1 20 Tf 10 10 Td (A) Tj ET 1 0 0 rg 0 0 4 4 re f
endstream endobj
5 0 obj << /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /FirstChar 65 /LastChar 65 /Widths [1]
  /Encoding << /Differences [65 /a] >> /CharProcs << /a 6 0 R >> >> endobj
6 0 obj << /Length 48 >> stream
1 0 d0 BT /F1 1 Tf (AAAAAAAA) Tj ET 0 0 1 1 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut r = PageRenderer::new(Arc::new(pdf.as_bytes().to_vec()), RenderConfig::default());
            let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }));
        });
        let page = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("a self-multiplying Type 3 glyph must not stall the renderer");
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255], "the rest of the page draws");
    }

    /// From the nightly `cargo xtask fuzz`: many small inline images, each with "EI" (followed
    /// by a space) inside its data, and no text anywhere. To decide whether such an "EI" ends the
    /// data, the parser re-read the rest of the stream as content, and that re-read did the same
    /// for every inline image it met, nesting through the whole stream: 50 images (3 KB) ran for
    /// over ten minutes. Vendored hayro-syntax patch: the re-read doesn't nest, and the search is
    /// bounded.
    #[test]
    fn inline_images_with_ei_in_their_data_parse_in_linear_time() {
        let image = b"q 1 0 0 1 0 0 cm\nBI\n/IM true\n/W 8\n/H 2\n/BPC 1\nID \x01 EI \x02\nEI Q\n";
        let content: Vec<u8> = image.iter().copied().cycle().take(image.len() * 200).collect();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut iter = hayro::hayro_syntax::content::TypedIter::new(&content);
            let mut ops = 0usize;
            while iter.next().is_some() {
                ops += 1;
            }
            let _ = tx.send(ops);
        });
        let ops = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("inline images must not stall the parser");
        assert!(ops >= 200 * 4, "every image and its q/cm/Q are read: {ops}");
    }

    /// From the nightly `cargo xtask fuzz`: a JBIG2 image whose stream declares a page and a
    /// generic region of 65535 × 65535 pixels (each side within JBIG2's limit) behind a small
    /// /Width and /Height: decoding its 4.3 billion pixels one by one took minutes. Vendored
    /// hayro-jbig2 patch: at most 2^28 pixels per bitmap, and 2^26 per symbol dictionary.
    #[test]
    fn jbig2_regions_of_billions_of_pixels_are_refused() {
        // Embedded JBIG2 segments (ISO 14492 §7.2): page information, then an immediate generic
        // region, both 65535 × 65535, followed by arithmetic-coded data.
        let mut jbig2 = Vec::new();
        let mut segment = |number: u32, kind: u8, data: &[u8]| {
            jbig2.extend_from_slice(&number.to_be_bytes());
            jbig2.extend_from_slice(&[kind, 0, 1]);
            jbig2.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
            jbig2.extend_from_slice(data);
        };
        let side = 65535u32.to_be_bytes();
        let mut page = Vec::new();
        page.extend_from_slice(&side);
        page.extend_from_slice(&side);
        page.extend_from_slice(&[0; 8]); // resolution
        page.extend_from_slice(&[0, 0, 0]); // flags, striping
        segment(0, 48, &page);
        let mut region = Vec::new();
        region.extend_from_slice(&side);
        region.extend_from_slice(&side);
        region.extend_from_slice(&[0; 9]); // x, y, combination operator
        region.push(0); // arithmetic coding, template 0
        region.extend_from_slice(&[3, 0xff, 0xfd, 0xff, 2, 0xfe, 0xfe, 0xfe]); // AT pixels
        region.extend_from_slice(&[0x5a; 64]);
        segment(1, 38, &region);
        let content = b"q 20 0 0 20 5 5 cm /Im1 Do Q 1 0 0 rg 0 0 4 4 re f";
        let mut pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R >> >> >> endobj\n".to_vec();
        pdf.extend_from_slice(format!("4 0 obj << /Length {} >> stream\n", content.len()).as_bytes());
        pdf.extend_from_slice(content);
        pdf.extend_from_slice(b"\nendstream endobj\n");
        pdf.extend_from_slice(
            format!("5 0 obj << /Type /XObject /Subtype /Image /Width 8 /Height 8 /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode /Length {} >> stream\n", jbig2.len())
                .as_bytes(),
        );
        pdf.extend_from_slice(&jbig2);
        pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
            let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }));
        });
        let page = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("a huge JBIG2 region must not stall the renderer");
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255], "the rest of the page draws");
    }

    /// From `cargo xtask fuzz`: one changed byte in a symbol dictionary's arithmetic-coded data
    /// hung the renderer. Two of its loops can stop advancing: an export run of length 0, and a
    /// height class whose first width is already the end-of-class marker. Past the end of its
    /// data the decoder can return those for ever. Vendored hayro-jbig2 patch: at most one height
    /// class per new symbol and `2 × symbols + 1` export runs, each plus `EXTRA_EMPTY_STEPS`.
    /// These synthetic dictionaries (two new symbols each, a few bytes of arbitrary data) decode
    /// the same way. In the second every class after the first symbol adds 13 to the height, so it
    /// only fails when the height overflows `u32`, after 330 million empty classes (about five
    /// seconds; a delta of 0 would never end). The page draws it as twenty images, so that the
    /// stall without the patch is well past the timeout on any machine.
    #[test]
    fn jbig2_symbol_dictionaries_that_never_advance_terminate() {
        for (what, data, copies) in
            [("zero-length export runs", &[0x05, 0x70, 0x05, 0x14, 0x02, 0x1d, 0x56, 0x3f][..], 1), ("empty height classes", &[0x06, 0x99][..], 20)]
        {
            // Embedded JBIG2 segments (ISO 14492 §7.2): page information, then a symbol dictionary.
            let mut jbig2 = Vec::new();
            let mut segment = |number: u32, kind: u8, data: &[u8]| {
                jbig2.extend_from_slice(&number.to_be_bytes());
                jbig2.extend_from_slice(&[kind, 0, 1]);
                jbig2.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
                jbig2.extend_from_slice(data);
            };
            let mut page = Vec::new();
            page.extend_from_slice(&8u32.to_be_bytes());
            page.extend_from_slice(&8u32.to_be_bytes());
            page.extend_from_slice(&[0; 8]); // resolution
            page.extend_from_slice(&[0, 0, 0]); // flags, striping
            segment(0, 48, &page);
            let mut dictionary = vec![0, 0]; // arithmetic coding, template 0, no refinement
            dictionary.extend_from_slice(&[3, 0xff, 0xfd, 0xff, 2, 0xfe, 0xfe, 0xfe]); // AT pixels
            dictionary.extend_from_slice(&1u32.to_be_bytes()); // exported symbols
            dictionary.extend_from_slice(&2u32.to_be_bytes()); // new symbols
            dictionary.extend_from_slice(data);
            segment(1, 0, &dictionary);
            // Each copy is its own image XObject (5, 6, …), all drawn by the page.
            let images: Vec<usize> = (5..5 + copies).collect();
            let mut content: String = images.iter().map(|n| format!("q 20 0 0 20 5 5 cm /Im{n} Do Q ")).collect();
            content.push_str("1 0 0 rg 0 0 4 4 re f");
            let names: String = images.iter().map(|n| format!("/Im{n} {n} 0 R ")).collect();
            let mut pdf = format!("%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /XObject << {names}>> >> >> endobj\n").into_bytes();
            pdf.extend_from_slice(format!("4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n", content.len()).as_bytes());
            for n in &images {
                pdf.extend_from_slice(
                    format!("{n} 0 obj << /Type /XObject /Subtype /Image /Width 8 /Height 8 /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode /Length {} >> stream\n", jbig2.len())
                        .as_bytes(),
                );
                pdf.extend_from_slice(&jbig2);
                pdf.extend_from_slice(b"\nendstream endobj\n");
            }
            pdf.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF\n");
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
                let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }));
            });
            let page = rx
                .recv_timeout(std::time::Duration::from_secs(20))
                .unwrap_or_else(|_| panic!("{what}: a JBIG2 symbol dictionary must not stall the renderer"));
            assert!(page.error.is_none(), "{what}: {:?}", page.error);
            assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255], "{what}: the rest of the page draws");
        }
    }

    /// Zlib data that inflates to `len` zero bytes.
    fn zlib_zeros(len: usize) -> Vec<u8> {
        use std::io::Write;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        let chunk = vec![0u8; 1 << 20];
        let mut left = len;
        while left > 0 {
            let n = left.min(chunk.len());
            z.write_all(&chunk[..n]).unwrap();
            left -= n;
        }
        z.finish().unwrap()
    }

    /// A raw deflate stream that flate2 rejects at once and the fallback inflater expands to at
    /// least `len` zero bytes: a one-byte stored block whose length check is wrong (flate2 refuses
    /// it, the fallback carries on), then a fixed-Huffman block of 258-byte copies.
    fn deflate_for_the_fallback(len: usize) -> Vec<u8> {
        let mut out = vec![0b000]; // not final, stored; the rest of the byte is padding
        out.extend_from_slice(&[1, 0, 0, 0, 0]); // LEN 1, NLEN 0 (should be !1), the byte 0
        let (mut acc, mut bits) = (0u64, 0u32);
        // Deflate packs bits from the least significant end; Huffman codes go most significant
        // bit first.
        let mut put = |value: u32, n: u32, huffman: bool| {
            for i in 0..n {
                let bit = if huffman { (value >> (n - 1 - i)) & 1 } else { (value >> i) & 1 };
                acc |= u64::from(bit) << bits;
                bits += 1;
                if bits == 8 {
                    out.push(acc as u8);
                    (acc, bits) = (0, 0);
                }
            }
        };
        put(1, 1, false); // final block
        put(1, 2, false); // fixed Huffman codes
        for _ in 0..len / 258 + 1 {
            put(0b1100_0101, 8, true); // length code 285: 258 bytes
            put(0, 5, true); // distance code 0: 1 byte back
        }
        put(0, 7, true); // end of block
        put(0, 7, false); // flush
        out
    }

    /// LZW codes (MSB first, early change) for `fills` table fills of zero runs: after a clear,
    /// code 0, then every code is the one being defined (the previous entry plus a zero), so
    /// entry n holds n - 256 zeros. One fill is about 7 MB of output from 5 KB of codes.
    fn lzw_zeros(fills: usize) -> Vec<u8> {
        let width = |size: usize| match size + 1 {
            2048.. => 12,
            1024.. => 11,
            512.. => 10,
            _ => 9,
        };
        let (mut out, mut acc, mut bits) = (Vec::new(), 0u64, 0u32);
        let mut emit = |code: usize, w: u32| {
            acc = (acc << w) | code as u64;
            bits += w;
            while bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        };
        let mut size = 258;
        for _ in 0..fills {
            emit(256, width(size)); // clear
            size = 258;
            emit(0, width(size));
            for code in 258..4096 {
                emit(code, width(size));
                size = code + 1;
            }
        }
        emit(257, width(size)); // end of data
        emit(0, 7); // flush the last partial byte
        out
    }

    /// Found while triaging `cargo xtask fuzz` on Windows, by hand rather than by the fuzzer
    /// (random mutations almost never make a high-ratio stream): a 2 MB page whose content
    /// stream, or whose 8 × 8 image, inflated to 2 GB took the renderer past 3 GB in under a
    /// second. Flate (through flate2 and through the fallback inflater), LZW and RunLength expand
    /// without limit. Vendored hayro-syntax patch: each stops at `MAX_DECODED_STREAM`, and an image
    /// of known size at its own rows.
    #[test]
    fn decompression_bombs_stop_at_the_stream_limit() {
        use hayro::hayro_syntax::object::stream::ImageDecodeParams;
        use hayro::hayro_syntax::object::{Object, Stream};
        let limit = hayro::hayro_syntax::MAX_DECODED_STREAM;
        let big = limit + (32 << 20);
        let mut run_length = [0x81u8, 0].repeat(big / 128 + 1); // runs of 128 zeros
        run_length.push(128);
        let streams: [(&str, Vec<u8>); 4] = [
            ("/FlateDecode", zlib_zeros(big)),
            ("/FlateDecode", deflate_for_the_fallback(big)),
            ("/LZWDecode", lzw_zeros(big / 7_000_000 + 1)),
            ("/RunLengthDecode", run_length),
        ];
        let mut pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] >> endobj\n".to_vec();
        for (i, (filter, data)) in streams.iter().enumerate() {
            pdf.extend_from_slice(format!("{} 0 obj << /Filter {filter} /Length {} >> stream\n", i + 4, data.len()).as_bytes());
            pdf.extend_from_slice(data);
            pdf.extend_from_slice(b"\nendstream endobj\n");
        }
        pdf.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF\n");
        let doc = Pdf::new(pdf).expect("parses");
        let found: Vec<Stream<'_>> = doc.objects().into_iter().filter_map(|o| if let Object::Stream(s) = o { Some(s) } else { None }).collect();
        assert_eq!(found.len(), 4, "all four bomb streams are found");
        for stream in &found {
            let what = format!("{:?}", stream.filters());
            let decoded = stream.decoded().expect("a bomb still decodes, up to the limit");
            assert_eq!(decoded.len(), limit, "{what}: stops at MAX_DECODED_STREAM, not at {big} bytes");
            // An 8 × 8 one-byte grey image has 64 pixel bytes, plus at most a predictor tag byte
            // for each.
            let params = ImageDecodeParams { bpc: Some(8), num_components: Some(1), width: 8, height: 8, ..ImageDecodeParams::default() };
            let image = stream.decoded_image(&params).expect("decodes as an image");
            assert!(image.data.len() <= 128, "{what}: an 8 × 8 image decodes at most 128 bytes, not {}", image.data.len());
        }
    }

    /// Review of the decompression-bomb fix: one stream stopping at `MAX_DECODED_STREAM` isn't
    /// enough when streams add up. A page's `/Contents` array can name the same bomb many times,
    /// and a form that paints itself nests fifty deep, each level keeping its content while the
    /// next runs. Vendored patches: a `/Contents` array shares one `MAX_DECODED_STREAM`
    /// (hayro-syntax), and page contents, forms, tiling patterns and Type 3 glyphs share a
    /// per-page budget (hayro-interpret `MAX_PAGE_CONTENT`).
    #[test]
    fn page_contents_and_nested_forms_share_one_budget() {
        let limit = hayro::hayro_syntax::MAX_DECODED_STREAM;
        let bomb = zlib_zeros(limit + (32 << 20));
        // Three references to one bomb in /Contents decode no more than one stream's worth.
        let mut pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents [4 0 R 4 0 R 4 0 R] >> endobj\n".to_vec();
        pdf.extend_from_slice(format!("4 0 obj << /Filter /FlateDecode /Length {} >> stream\n", bomb.len()).as_bytes());
        pdf.extend_from_slice(&bomb);
        pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
        let doc = Pdf::new(pdf).expect("parses");
        let page = doc.pages().first().expect("one page");
        let contents = page.page_stream().map_or(0, <[u8]>::len);
        assert!(contents <= limit + 3, "the /Contents array decodes {contents} bytes, over one stream's {limit}");

        // A form that paints itself before 256 MiB of spaces: every level would keep its content.
        let mut content = b"/Fm0 Do ".to_vec();
        content.resize(limit, b' ');
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        std::io::Write::write_all(&mut z, &content).unwrap();
        let form = z.finish().unwrap();
        drop(content);
        let page_content = b"/Fm0 Do 1 0 0 rg 0 0 4 4 re f";
        let mut pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /XObject << /Fm0 5 0 R >> >> >> endobj\n".to_vec();
        pdf.extend_from_slice(format!("4 0 obj << /Length {} >> stream\n", page_content.len()).as_bytes());
        pdf.extend_from_slice(page_content);
        pdf.extend_from_slice(b"\nendstream endobj\n");
        pdf.extend_from_slice(
            format!("5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 40 40] /Resources << /XObject << /Fm0 5 0 R >> >> /Filter /FlateDecode /Length {} >> stream\n", form.len()).as_bytes(),
        );
        pdf.extend_from_slice(&form);
        pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
            let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }));
        });
        let page = rx.recv_timeout(std::time::Duration::from_secs(60)).expect("a self-painting form must not stall the renderer");
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255], "the rest of the page draws");
    }

    /// The page content budget counts what is held at once, not everything decoded: a form
    /// with 2 MiB of content painted three hundred times (600 MiB in all) draws every time.
    #[test]
    fn content_painted_again_and_again_keeps_drawing() {
        let mut content = b"1 0 0 rg 0 0 4 4 re f ".to_vec();
        content.resize(2 << 20, b' ');
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        std::io::Write::write_all(&mut z, &content).unwrap();
        let form = z.finish().unwrap();
        let mut page_content = "/Fm0 Do ".repeat(299);
        page_content.push_str("q 1 0 0 1 30 30 cm /Fm0 Do Q");
        let mut pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /XObject << /Fm0 5 0 R >> >> >> endobj\n".to_vec();
        pdf.extend_from_slice(format!("4 0 obj << /Length {} >> stream\n{page_content}\nendstream endobj\n", page_content.len()).as_bytes());
        pdf.extend_from_slice(
            format!("5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 40 40] /Filter /FlateDecode /Length {} >> stream\n", form.len()).as_bytes(),
        );
        pdf.extend_from_slice(&form);
        pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
        let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
        let page = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
        assert!(page.error.is_none(), "{:?}", page.error);
        // The last paint is at user (30, 30)–(34, 34): device row 40 - 32 = 8.
        assert_eq!(&page.rgba[((8 * 40 + 32) * 4)..][..4], &[255, 0, 0, 255], "the 300th paint is drawn");
    }

    /// Review of the decompression-bomb fix: a Type 0 sampled function read every sample of its
    /// stream into a 4-byte value, then a table entry each, so 256 MiB of 1-bit samples asked for
    /// over 8 GB. Vendored hayro-interpret patch: at most `MAX_SAMPLE_VALUES` samples, read no
    /// further than `/Size` asks.
    #[test]
    fn sampled_functions_with_billions_of_samples_are_refused() {
        let bomb = zlib_zeros(hayro::hayro_syntax::MAX_DECODED_STREAM + (32 << 20));
        let page_content = b"/Sh0 sh 1 0 0 rg 0 0 4 4 re f";
        let mut pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /Shading << /Sh0 5 0 R >> >> >> endobj\n".to_vec();
        pdf.extend_from_slice(format!("4 0 obj << /Length {} >> stream\n", page_content.len()).as_bytes());
        pdf.extend_from_slice(page_content);
        pdf.extend_from_slice(
            b"\nendstream endobj\n5 0 obj << /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 40 0] /Function 6 0 R >> endobj\n",
        );
        pdf.extend_from_slice(
            format!("6 0 obj << /FunctionType 0 /Domain [0 1] /Range [0 1] /Size [1073741824] /BitsPerSample 1 /Filter /FlateDecode /Length {} >> stream\n", bomb.len()).as_bytes(),
        );
        pdf.extend_from_slice(&bomb);
        pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
            let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }));
        });
        let page = rx.recv_timeout(std::time::Duration::from_secs(60)).expect("a sampled-function bomb must not stall the renderer");
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255], "the rest of the page draws");
    }

    /// From `cargo xtask fuzz`: a mesh shading whose points lie far off the page (a 147-byte
    /// Coons patch stream decoded onto ±40000) was sampled pixel by pixel over its whole
    /// bounding box, up to 65536 × 65536 entries: a synthetic page passed 6 GB in six seconds.
    /// Vendored hayro-interpret patch: mesh shadings are sampled only where they are drawn,
    /// within eight visits per pixel of that area plus four per triangle in it.
    #[test]
    fn mesh_shadings_far_off_the_page_render() {
        let render = |shading: Vec<u8>| {
            let content = b"/Sh0 sh 1 0 0 rg 0 0 4 4 re f";
            let mut pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /Shading << /Sh0 5 0 R >> >> >> endobj\n".to_vec();
            pdf.extend_from_slice(format!("4 0 obj << /Length {} >> stream\n", content.len()).as_bytes());
            pdf.extend_from_slice(content);
            pdf.extend_from_slice(b"\nendstream endobj\n5 0 obj ");
            pdf.extend_from_slice(&shading);
            pdf.extend_from_slice(b" endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
                let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }));
            });
            let page = rx.recv_timeout(std::time::Duration::from_secs(60)).expect("a mesh shading must not stall the renderer");
            assert!(page.error.is_none(), "{:?}", page.error);
            page
        };
        let stream = |dict: &str, data: &[u8]| {
            let mut s = format!("<< {dict} /Length {} >> stream\n", data.len()).into_bytes();
            s.extend_from_slice(data);
            s.extend_from_slice(b"\nendstream");
            s
        };
        // One Coons patch (flag, 12 points, 4 colours) with 8-bit coordinates around its square.
        let mut coons = vec![0u8];
        for p in [(0, 0), (0, 85), (0, 170), (0, 255), (85, 255), (170, 255), (255, 255), (255, 170), (255, 85), (255, 0), (170, 0), (85, 0)] {
            coons.extend_from_slice(&[p.0, p.1]);
        }
        coons.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0]);
        // A free-form triangle mesh: three vertices (flag, x, y, colour).
        let triangles = [0u8, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255];
        let mesh = "/ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8 /BitsPerFlag 8";
        let far = "/Decode [-40000 40000 -40000 40000 0 1 0 1 0 1]";
        for (what, shading) in [
            ("a Coons patch", stream(&format!("/ShadingType 6 {mesh} {far}"), &coons)),
            ("a triangle mesh", stream(&format!("/ShadingType 4 {mesh} {far}"), &triangles)),
        ] {
            let page = render(shading);
            assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255], "{what} far off the page: the rest of the page draws");
        }
        // A patch on the page, and the part of the huge one that covers it, are still shaded:
        // the centre is neither white nor transparent.
        for (what, decode) in [("on the page", "[0 40 0 40 0 1 0 1 0 1]"), ("covering the page", "[-40000 40000 -40000 40000 0 1 0 1 0 1]")] {
            let page = render(stream(&format!("/ShadingType 6 {mesh} /Decode {decode}"), &coons));
            let centre = &page.rgba[((20 * 40 + 20) * 4)..][..4];
            assert!(centre != [255, 255, 255, 255] && centre[3] == 255, "a patch {what} is shaded: {centre:?}");
        }
        // A patch filling a 4 × 4 pt rectangle as a pattern is 722 triangles of a fraction of a
        // pixel each: far more visits than its 16 pixels give, which the per-triangle allowance
        // covers. The rectangle's middle is shaded.
        let content = b"/Pattern cs /P0 scn 10 10 4 4 re f";
        let shading = stream(&format!("/ShadingType 6 {mesh} /Decode [10 14 10 14 0 1 0 1 0 1]"), &coons);
        let mut pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /Pattern << /P0 << /PatternType 2 /Shading 5 0 R >> >> >> >> endobj
".to_vec();
        pdf.extend_from_slice(
            format!(
                "4 0 obj << /Length {} >> stream
",
                content.len()
            )
            .as_bytes(),
        );
        pdf.extend_from_slice(content);
        pdf.extend_from_slice(
            b"
endstream endobj
5 0 obj ",
        );
        pdf.extend_from_slice(&shading);
        pdf.extend_from_slice(
            b" endobj
trailer << /Root 1 0 R >>
%%EOF
",
        );
        let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
        let page = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
        assert!(page.error.is_none(), "{:?}", page.error);
        let middle = &page.rgba[((28 * 40 + 12) * 4)..][..4];
        assert!(middle != [255, 255, 255, 255] && middle[3] == 255, "a small patch is shaded: {middle:?}");
    }

    /// The other half of the mesh-shading patch: ten thousand triangles that each cover half
    /// of this 1000 × 1000 canvas walked their bounding boxes ten thousand times over (ten
    /// billion visits). The visit budget stops that work at eight per pixel plus four per
    /// triangle.
    #[test]
    fn many_overlapping_mesh_triangles_finish() {
        // Each triangle: three vertices of flag, x, y, red, green, blue; half of the page.
        let one = [0u8, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255];
        let triangles = one.repeat(10_000);
        let content = b"/Sh0 sh";
        let mut pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /Shading << /Sh0 5 0 R >> >> >> endobj
"
        .to_vec();
        pdf.extend_from_slice(
            format!(
                "4 0 obj << /Length {} >> stream
",
                content.len()
            )
            .as_bytes(),
        );
        pdf.extend_from_slice(content);
        pdf.extend_from_slice(
            format!("
endstream endobj
5 0 obj << /ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8 /BitsPerFlag 8 /Decode [0 40 0 40 0 1 0 1 0 1] /Length {} >> stream
", triangles.len())
                .as_bytes(),
        );
        pdf.extend_from_slice(&triangles);
        pdf.extend_from_slice(
            b"
endstream endobj
trailer << /Root 1 0 R >>
%%EOF
",
        );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
            let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 25.0, tag: 0 }));
        });
        let page = rx.recv_timeout(std::time::Duration::from_secs(60)).expect("overlapping mesh triangles must not stall the renderer");
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!((page.width, page.height), (1000, 1000));
        // The triangles cover the lower left half: a pixel there is shaded.
        let p = &page.rgba[((900 * 1000 + 100) * 4)..][..4];
        assert!(p != [255, 255, 255, 255] && p[3] == 255, "the triangles are drawn: {p:?}");
    }

    /// Review of the mesh-shading patch: every patch was cut into 722 triangles before anything
    /// limited them (about 150 KB per patch), and a lattice mesh with `/VerticesPerRow 0` pushed
    /// empty rows for ever. Vendored hayro-interpret patch: patches are triangulated one at a
    /// time, meshes hold at most `MAX_MESH_PATCHES` patches and `MAX_MESH_TRIANGLES` triangles,
    /// and a lattice needs two vertices per row.
    #[test]
    fn mesh_shadings_with_many_patches_or_no_rows_finish() {
        let render = |shading: Vec<u8>| {
            let content = b"/Sh0 sh 1 0 0 rg 0 0 4 4 re f";
            let mut pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /Shading << /Sh0 5 0 R >> >> >> endobj\n".to_vec();
            pdf.extend_from_slice(format!("4 0 obj << /Length {} >> stream\n", content.len()).as_bytes());
            pdf.extend_from_slice(content);
            pdf.extend_from_slice(b"\nendstream endobj\n5 0 obj ");
            pdf.extend_from_slice(&shading);
            pdf.extend_from_slice(b" endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
                let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }));
            });
            let page = rx.recv_timeout(std::time::Duration::from_secs(60)).expect("a mesh shading must not stall the renderer");
            assert!(page.error.is_none(), "{:?}", page.error);
            assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255], "the rest of the page draws");
        };
        let stream = |dict: &str, data: &[u8]| {
            let mut s = format!("<< {dict} /Length {} >> stream\n", data.len()).into_bytes();
            s.extend_from_slice(data);
            s.extend_from_slice(b"\nendstream");
            s
        };
        let mesh = "/ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8";
        // Forty thousand Coons patches beside the page: 6 GB of triangles if collected first.
        let mut coons = vec![0u8];
        for p in [(0, 0), (0, 85), (0, 170), (0, 255), (85, 255), (170, 255), (255, 255), (255, 170), (255, 85), (255, 0), (170, 0), (85, 0)] {
            coons.extend_from_slice(&[p.0, p.1]);
        }
        coons.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0]);
        render(stream(&format!("/ShadingType 6 {mesh} /BitsPerFlag 8 /Decode [100 140 100 140 0 1 0 1 0 1]"), &coons.repeat(40_000)));
        // A lattice with no vertices per row.
        render(stream(&format!("/ShadingType 5 {mesh} /VerticesPerRow 0 /Decode [0 40 0 40 0 1 0 1 0 1]"), &[0u8; 64]));
    }

    /// From the nightly `cargo xtask fuzz`: an embedded Type 1 font program holding a long run of
    /// integers. read-fonts 0.39 (through skrifa 0.42) looked ahead after every integer by
    /// parsing the next token, which looked ahead again, recursing through the whole run: a long
    /// run hung, and 20,000 numbers overflowed the stack and aborted the process. Fixed upstream:
    /// the vendored hayro-interpret now uses skrifa 0.47 (read-fonts 0.44).
    #[test]
    fn type1_font_programs_with_long_runs_of_numbers_load_quickly() {
        let mut font = b"%!PS-AdobeFont-1.0: Fuzz 001\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n".to_vec();
        for _ in 0..20_000 {
            font.extend_from_slice(b"1 ");
        }
        let content = b"BT /F1 12 Tf 5 20 Td (Hi) Tj ET 1 0 0 rg 0 0 4 4 re f";
        let mut pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj\n".to_vec();
        pdf.extend_from_slice(format!("4 0 obj << /Length {} >> stream\n", content.len()).as_bytes());
        pdf.extend_from_slice(content);
        pdf.extend_from_slice(b"\nendstream endobj\n5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Fuzz /FontDescriptor 6 0 R >> endobj\n");
        pdf.extend_from_slice(b"6 0 obj << /Type /FontDescriptor /FontName /Fuzz /Flags 32 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile 7 0 R >> endobj\n");
        pdf.extend_from_slice(format!("7 0 obj << /Length {} /Length1 {} /Length2 0 /Length3 0 >> stream\n", font.len(), font.len()).as_bytes());
        pdf.extend_from_slice(&font);
        pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
            let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }));
        });
        let page = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("a Type 1 font must not stall the renderer");
        assert!(page.error.is_none(), "{:?}", page.error);
        assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255], "the rest of the page draws");
    }

    /// From `cargo xtask fuzz`: an inline image claiming /W 4294967295 over four bytes of data
    /// hung in resampling (vendored hayro patch: `MAX_IMAGE_PIXELS`).
    #[test]
    fn absurd_image_dimensions_are_skipped() {
        let content = "q 20 0 0 20 5 5 cm BI /W 4294967295 /H 2 /BPC 8 /CS /G ID \u{0}\u{ff}\u{ff}\u{0} EI Q 1 0 0 rg 0 0 4 4 re f";
        let pdf = format!(
            "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
            content.len()
        );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
            let _ = tx.send(r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 }));
        });
        let page = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("an absurd image must not hang the renderer");
        assert!(page.error.is_none(), "{:?}", page.error);
        // The rest of the page still draws: the red square at the bottom left (y-down: last rows).
        assert_eq!(&page.rgba[((38 * 40 + 1) * 4)..][..4], &[255, 0, 0, 255]);
    }

    /// From `cargo xtask fuzz`: a CID font whose /W range spans every u32 inserted billions of
    /// widths (vendored hayro-interpret patch: `MAX_CID`). Must finish quickly.
    #[test]
    fn huge_cid_width_ranges_terminate() {
        let pdf = "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 40] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length 35 >> stream
BT /F1 12 Tf 10 10 Td <0041> Tj ET
endstream endobj
5 0 obj << /Type /Font /Subtype /Type0 /BaseFont /Helvetica /Encoding /Identity-H /DescendantFonts [6 0 R] >> endobj
6 0 obj << /Type /Font /Subtype /CIDFontType2 /BaseFont /Helvetica /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >>
  /FontDescriptor 7 0 R /W [0 4294967295 500] /W2 [0 4294967295 -1000 250 880] >> endobj
7 0 obj << /Type /FontDescriptor /FontName /Helvetica /Flags 32 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 >> endobj
trailer << /Root 1 0 R >>
%%EOF";
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut r = PageRenderer::new(Arc::new(pdf.as_bytes().to_vec()), RenderConfig::default());
            let px = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
            let text = r.render(RenderRequest { page: 0, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0 });
            let _ = tx.send((px.error, text.error));
        });
        let (px, text) = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("rendering a huge /W range must not hang");
        assert!(px.is_none() && text.is_none(), "{px:?} {text:?}");
    }

    /// "BBBA" at 20 pt in a Type 3 font whose glyph (for both codes) is a solid 300 × 700 box,
    /// with the given /FirstChar, /LastChar and /Widths. No font program is needed, and Type 3
    /// widths are read like those of every other simple font.
    fn type3_boxes(widths: &str) -> Vec<u8> {
        let content = "BT /F1 20 Tf 20 40 Td (BBBA) Tj ET";
        format!(
            "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] {widths}
  /Encoding << /Differences [65 /a /a] >> /CharProcs << /a 6 0 R >> >> endobj
6 0 obj << /Length 27 >> stream
600 0 d0 100 0 400 700 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
            content.len()
        )
        .into_bytes()
    }

    /// Simple-font widths (vendored hayro-interpret patch to `read_widths`): an entry of /Widths
    /// that isn't a number ended the array, so every later code lost its width, and a /LastChar
    /// below /FirstChar (or one that overflowed) made the whole font fail to load, replaced by a
    /// standard font. MuPDF, Poppler, PDFium and pdf.js skip just that entry and keep the font.
    #[test]
    fn malformed_simple_font_widths_keep_later_widths_and_the_font() {
        let render = |widths: &str| {
            let mut r = PageRenderer::new(Arc::new(type3_boxes(widths)), RenderConfig::default());
            let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
            assert!(p.error.is_none(), "{widths}: {:?}", p.error);
            p
        };
        // Glyph boxes span x 22..28 plus 12 pt per advance, and y 46..60 (y down).
        let inked = |p: &RenderedPage, x: u32| (47..59).all(|y| p.rgba[((y * p.width + x) * 4) as usize] < 64);
        for widths in [
            "/FirstChar 65 /LastChar 66 /Widths [600 600]",
            "/FirstChar 65 /LastChar 66 /Widths [null 600]",
            "/FirstChar 65 /LastChar 66 /Widths [/W 600]",
        ] {
            let p = render(widths);
            assert!([25, 37, 49, 61].iter().all(|x| inked(&p, *x)), "{widths}: each B advances 600");
            assert!(!inked(&p, 31), "{widths}");
        }
        for widths in ["/FirstChar 66 /LastChar 65 /Widths [600 600]", "/FirstChar 4294967295 /LastChar 66 /Widths [600 600]"] {
            let p = render(widths);
            // The font's own box, drawn without advances (no width applies), not a stand-in font.
            assert!(inked(&p, 23) && inked(&p, 27), "{widths}: the Type 3 glyph is drawn");
            assert!(!inked(&p, 37), "{widths}: no width, no advance");
        }
    }

    /// Simple-font widths (vendored hayro-interpret patch to `read_widths`): /FirstChar sized an
    /// allocation, one entry per code below it, before any width was read. 50,000,000 already
    /// took 276 MB; 4294967295 asks for 34 GB. Codes of a simple font stop at 255.
    #[test]
    fn huge_simple_font_first_char_terminates() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut r = PageRenderer::new(Arc::new(type3_boxes("/FirstChar 4294967295 /LastChar 4294967295 /Widths [600]")), RenderConfig::default());
            let px = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
            let text = r.render(RenderRequest { page: 0, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0 });
            let _ = tx.send((px, text.error));
        });
        let (px, text) = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("a huge /FirstChar must not exhaust memory");
        assert!(px.error.is_none() && text.is_none(), "{:?} {text:?}", px.error);
        assert!(px.rgba[((50 * px.width + 25) * 4) as usize] < 64, "the glyph is drawn");
    }

    #[test]
    fn tiles_match_full_render() {
        let mut r = PageRenderer::new(Arc::new(ONE_PAGE.to_vec()), RenderConfig::default());
        let full = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 2.0, tag: 0 });
        let tile = Tile { x: 30, y: 40, w: 50, h: 30 };
        let part = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: Some(tile), scale: 2.0, tag: 0 });
        assert_eq!((part.width, part.height), (50, 30));
        for y in 0..30u32 {
            for x in 0..50u32 {
                let a = &full.rgba[(((y + 40) * full.width + x + 30) * 4) as usize..][..4];
                let b = &part.rgba[((y * 50 + x) * 4) as usize..][..4];
                assert_eq!(a, b, "pixel {x},{y}");
            }
        }
    }

    /// A Japanese CID font that isn't embedded (Adobe-Japan1, as `HeiseiMin-W3` with
    /// `UniJIS-UCS2-H` in #260's test file) drew nothing: hayro's substitutes for fonts that
    /// aren't embedded are Latin-only. With craft-fonts (the build input release builds embed),
    /// such text now draws in a Japanese face; without it, nothing changes.
    #[test]
    fn non_embedded_japanese_cid_fonts_draw_with_a_craft_fonts_face() {
        let pdf = |base_font: &str| {
            let content = "BT /F1 40 Tf 5 15 Td <65E5672C> Tj ET";
            format!(
                "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 60] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Font /Subtype /Type0 /BaseFont /{base_font} /Encoding /UniJIS-UCS2-H /DescendantFonts [6 0 R] >> endobj
6 0 obj << /Type /Font /Subtype /CIDFontType0 /BaseFont /{base_font} /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >>
  /FontDescriptor 7 0 R /DW 1000 >> endobj
7 0 obj << /Type /FontDescriptor /FontName /{base_font} /Flags 6 /FontBBox [0 -141 1000 859] /ItalicAngle 0 /Ascent 859 /Descent -141 /CapHeight 700 /StemV 80 >> endobj
trailer << /Root 1 0 R >>
%%EOF",
                content.len()
            )
        };
        for base_font in ["HeiseiMin-W3", "HeiseiKakuGo-W5"] {
            let mut r = PageRenderer::new(Arc::new(pdf(base_font).into_bytes()), RenderConfig::default());
            let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
            assert!(p.error.is_none(), "{base_font}: {:?}", p.error);
            let inked = p.rgba.as_chunks::<4>().0.iter().filter(|c| c[0] < 128).count();
            if pdfcraft_fonts::document_japanese_font().is_none() {
                eprintln!("built without craft-fonts (CRAFT_FONTS_DIR unset): no Japanese face to check");
                continue;
            }
            // 日本 at 40 pt covers a few hundred dark pixels; a blank or missing-glyph run doesn't.
            assert!(inked > 300, "{base_font}: 日本 is drawn ({inked} dark pixels)");
        }
    }

    /// Half-width katakana are common in Japanese documents set in a non-embedded Mincho font
    /// such as HeiseiMin-W3. The Mincho substitute must have those glyphs: one that lacks them
    /// (Shippori Mincho) draws each CID by glyph index instead, as unrelated glyphs (ﬁ, ﬂ).
    #[test]
    fn mincho_substitute_covers_half_width_katakana() {
        use hayro::hayro_interpret::font::FallbackFontQuery;
        use hayro::hayro_interpret::hayro_cmap::CharacterCollection;
        use skrifa::MetadataProvider;
        let query = FontQuery::Fallback(FallbackFontQuery {
            post_script_name: Some("HeiseiMin-W3".into()),
            character_collection: Some(CharacterCollection { family: CidFamily::AdobeJapan1, supplement: 2 }),
            ..FallbackFontQuery::default()
        });
        let Some((data, index)) = super::japanese_fallback(&query) else {
            eprintln!("built without craft-fonts (CRAFT_FONTS_DIR unset): no Japanese face to check");
            return;
        };
        let font = skrifa::FontRef::from_index((*data).as_ref(), index).expect("a craft-fonts face parses");
        let charmap = font.charmap();
        let missing: Vec<char> = ('\u{FF61}'..='\u{FF9F}').chain(['日', '本']).filter(|&c| charmap.map(c).is_none()).collect();
        assert!(missing.is_empty(), "the Mincho substitute lacks {missing:?}");
    }

    #[test]
    fn japanese_font_names_pick_mincho_or_gothic() {
        use super::JapaneseFace::{Gothic, Mincho};
        for (name, serif, bold, face) in [
            ("HeiseiMin-W3", false, false, Mincho),
            ("KozMinPro-Regular", false, false, Mincho),
            ("Ryumin-Light", false, false, Mincho),
            ("MS-PMincho", false, false, Mincho),
            ("HiraMinProN-W3", false, false, Mincho),
            ("HeiseiKakuGo-W5", true, false, Gothic { bold: false }),
            ("KozGoPro-Bold", true, true, Gothic { bold: true }),
            ("GothicBBB-Medium", true, false, Gothic { bold: false }),
            ("MS-Gothic", false, false, Gothic { bold: false }),
            ("HiraKakuProN-W6", false, true, Gothic { bold: true }),
            ("Unknown-Japanese", true, false, Mincho),
            ("Unknown-Japanese", false, false, Gothic { bold: false }),
        ] {
            assert_eq!(super::japanese_face(name, serif, bold), face, "{name}");
        }
    }

    /// An alpha soft mask whose transparency group has no /CS (as Chrome writes gradient text)
    /// must still mask. Regression test for the vendored hayro-interpret patch.
    #[test]
    fn soft_mask_without_group_colour_space_is_applied() {
        let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /ExtGState << /M 5 0 R >> >> >> endobj
4 0 obj << /Length 31 >> stream
/M gs 1 0 0 rg 0 0 100 100 re f
endstream endobj
5 0 obj << /Type /ExtGState /SMask << /Type /Mask /S /Alpha /G 6 0 R >> >> endobj
6 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Group << /S /Transparency /I true >> /Length 20 >> stream
0 g 0 0 50 100 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";
        let mut r = PageRenderer::new(Arc::new(pdf.to_vec()), RenderConfig::default());
        let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
        let px = |x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..4].to_vec();
        assert_eq!(px(25, 50), vec![255, 0, 0, 255], "inside the mask");
        assert_eq!(px(75, 50), vec![255, 255, 255, 255], "outside the mask");
    }

    /// ISO 32000-2 §8.5.4: `W` / `W*` clip with the current path once whichever path-painting
    /// operator ends it has painted it, `S`, `f` or `B` as much as `n`. The vendored interpreter
    /// applied the clip only on `n`: after `re W* S` everything later painted outside the clip,
    /// and the forgotten clip stayed pending, so a later `re n` (even after `Q`) clipped
    /// content that should show. MuPDF, Poppler and PDFium clip in every case below.
    #[test]
    fn clipping_path_applies_after_any_painting_operator() {
        let render = |content: &str| {
            let pdf = format!(
                "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
                content.len()
            );
            let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
            let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0 });
            assert!(p.error.is_none(), "{content}: {:?}", p.error);
            p
        };
        let px = |p: &RenderedPage, x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..4].to_vec();
        let red = vec![255, 0, 0, 255];
        // The clip is the square x 20..60, y 20..60 (device rows 40..80); then the page is filled red.
        for op in ["n", "S", "s", "f", "F", "f*", "B", "B*", "b", "b*"] {
            for clip in ["W", "W*"] {
                let p = render(&format!("q 0 0 1 RG 4 w 0.8 g 20 20 40 40 re {clip} {op} 1 0 0 rg 0 0 100 100 re f Q"));
                assert_eq!(px(&p, 40, 60), red, "{clip} {op}: inside the clip");
                assert_ne!(px(&p, 5, 5), red, "{clip} {op}: outside the clip");
                assert_ne!(px(&p, 95, 95), red, "{clip} {op}: outside the clip");
                // The operator that ends the path paints under the old clip: the outer half of
                // the 4 pt stroke (x 18..20) shows outside the new one.
                let stroked = !["n", "f", "F", "f*"].contains(&op);
                let edge = if stroked { vec![0, 0, 255, 255] } else { vec![255, 255, 255, 255] };
                assert_eq!(px(&p, 18, 60), edge, "{clip} {op}: just outside the clip");
            }
        }
        // A clip is used once: it doesn't linger for a later `n`, inside or outside `q`/`Q`.
        for content in [
            "q 0 0 1 RG 70 70 10 10 re W S Q q 10 10 20 20 re n 1 0 0 rg 0 0 100 100 re f Q",
            "q 0 0 1 RG 0 0 100 100 re W S 10 10 20 20 re n 1 0 0 rg 0 0 100 100 re f Q",
        ] {
            let p = render(content);
            assert!([(5, 5), (50, 50), (95, 95)].iter().all(|(x, y)| px(&p, *x, *y) == red), "{content}: no stray clip");
        }
    }
}
