//! Background page rasterization.
//!
//! A `RenderPool` owns N worker threads. Each worker parses the shared bytes once (hayro's parser
//! is lazy, so this is cheap) and keeps its own render cache. Requests carry a caller tag that is
//! echoed back; the UI drops results for stale tags (e.g. after a zoom change).
//!
//! **Robustness:** every page render runs under `catch_unwind`. A panic inside the renderer on a
//! malformed page yields a `RenderedPage` with `error` set, and the worker rebuilds its parser and
//! cache before continuing, so one bad page can never take down the app or poison later pages.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings, render};

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
        InterpreterSettings { ocg_overrides: self.layers.clone(), hide_comments: self.hide_comments, ..InterpreterSettings::default() }
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
    pub rgba: Vec<u8>,
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

/// Render one page with a caller-owned parser and cache. Panics inside the renderer are caught
/// and reported as `Err((message, panicked))`.
type Output = (u32, u32, Vec<u8>, Option<Arc<crate::text::PageText>>);

fn render_page<'a>(pdf: &'a Pdf, cache: &RenderCache<'a>, settings: &InterpreterSettings, req: RenderRequest) -> Result<Output, (String, bool)> {
    if req.kind == RequestKind::Text {
        return match catch_unwind(AssertUnwindSafe(|| crate::text::extract_page(pdf, req.page, settings))) {
            Ok(Some(t)) => Ok((0, 0, Vec::new(), Some(Arc::new(t)))),
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
                RenderSettings { x_scale: scale, y_scale: scale, bg_color: WHITE, ..Default::default() }
            }
        };
        let pixmap = render(page, cache, settings, &rs);
        Ok((pixmap.width() as u32, pixmap.height() as u32, pixmap.data_as_u8_slice().to_vec(), None))
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
        Err((e, _)) => RenderedPage { request: req, width: 0, height: 0, rgba: Vec::new(), error: Some(e), text: None, millis },
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

/// State shared by the pool and its workers.
#[derive(Default)]
struct Shared {
    /// Pending requests, most urgent last (workers pop from the end).
    queue: Mutex<Vec<RenderRequest>>,
    /// Per worker id: the request it is rendering and since when.
    #[cfg(not(target_arch = "wasm32"))]
    busy: Mutex<Vec<Option<(RenderRequest, std::time::Instant)>>>,
    /// Pages (and request kinds) the watchdog gave up on: answered with an error at once, so a
    /// pathological page cannot trap every worker in turn.
    stuck: Mutex<std::collections::HashSet<(usize, RequestKind)>>,
    /// Test hook: make one page slow.
    #[cfg(test)]
    slow_page: Mutex<Option<(usize, std::time::Duration)>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Renders pages on worker threads, most urgent request first.
///
/// **Watchdog:** a render running longer than `stuck_after` (default [`STUCK_AFTER`]) is reported
/// as an error for that page, the page is not attempted again, and a replacement worker takes
/// the stuck one's place (threads cannot be killed; the stuck one exits when its render
/// finally returns, and its late result is dropped). At most `threads` replacements are started
/// per pool, so a document full of pathological pages cannot spawn threads without bound.
pub struct RenderPool {
    shared: Arc<Shared>,
    wake: Mutex<Vec<Sender<()>>>,
    results: Receiver<RenderedPage>,
    results_tx: Sender<RenderedPage>,
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
    _workers: Mutex<Vec<JoinHandle<()>>>,
    /// Errors produced by the watchdog, handed out by `try_recv`.
    abandoned: Mutex<Vec<RenderedPage>>,
    stuck_after: std::time::Duration,
    replacements_left: Mutex<usize>,
    /// Used when threads are unavailable (wasm32 without atomics, or spawn failure): requests are
    /// rendered on the calling thread inside `try_recv`, one per call.
    inline: Option<std::cell::RefCell<PageRenderer>>,
}

impl RenderPool {
    pub fn new(bytes: Arc<Vec<u8>>, threads: usize, config: RenderConfig) -> Self {
        let (results_tx, results) = channel();
        let threads = if cfg!(target_arch = "wasm32") { 0 } else { threads.max(1) };
        let mut pool = Self {
            shared: Arc::default(),
            wake: Mutex::new(Vec::new()),
            results,
            results_tx,
            bytes: bytes.clone(),
            config: config.clone(),
            _workers: Mutex::new(Vec::new()),
            abandoned: Mutex::new(Vec::new()),
            stuck_after: STUCK_AFTER,
            replacements_left: Mutex::new(threads),
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
            let id = {
                let mut busy = lock(&self.shared.busy);
                busy.push(None);
                busy.len() - 1
            };
            let (wtx, wrx) = channel::<()>();
            let (shared, out, bytes, config) = (self.shared.clone(), self.results_tx.clone(), self.bytes.clone(), self.config.clone());
            match std::thread::Builder::new().name(format!("pdfcraft-render-{id}")).spawn(move || worker(id, bytes, config, shared, wrx, out)) {
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
        let mut pool = Self::new(bytes.clone(), 0, config.clone());
        if pool.inline.is_none() {
            // Native `new` always spawns at least one worker; drop to inline explicitly.
            pool = Self { shared: Arc::default(), wake: Mutex::new(Vec::new()), _workers: Mutex::new(Vec::new()), ..pool };
            pool.inline = Some(std::cell::RefCell::new(PageRenderer::new(bytes, config)));
        }
        pool
    }

    /// `true` when rendering happens on the caller's thread (no worker threads available).
    pub fn is_inline(&self) -> bool {
        self.inline.is_some()
    }

    /// Change the watchdog limit (tests and benchmarks).
    pub fn set_stuck_after(&mut self, limit: std::time::Duration) {
        self.stuck_after = limit;
    }

    /// Replace the pending queue (most urgent first). In-flight renders are not interrupted.
    pub fn set_queue(&self, mut requests: Vec<RenderRequest>) {
        requests.reverse(); // workers pop from the end
        *lock(&self.shared.queue) = requests;
        for w in lock(&self.wake).iter() {
            let _ = w.send(());
        }
    }

    pub fn try_recv(&self) -> Option<RenderedPage> {
        if let Some(r) = &self.inline {
            let next = lock(&self.shared.queue).pop();
            return next.map(|req| r.borrow_mut().render(req));
        }
        self.watchdog();
        if let Some(p) = lock(&self.abandoned).pop() {
            return Some(p);
        }
        self.results.try_recv().ok()
    }

    /// Give up on renders that exceeded `stuck_after` (see the type docs).
    fn watchdog(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut gave_up = Vec::new();
            {
                let mut busy = lock(&self.shared.busy);
                for slot in busy.iter_mut() {
                    if let Some((req, since)) = *slot
                        && since.elapsed() > self.stuck_after
                    {
                        *slot = None; // the worker sees this and exits when it returns
                        gave_up.push(req);
                    }
                }
            }
            for req in gave_up {
                lock(&self.shared.stuck).insert((req.page, req.kind));
                let what = if req.kind == RequestKind::Text { "text extraction for page" } else { "page" };
                let error = format!(
                    "{what} {} took longer than {:.0} s and was skipped; the page may be damaged or extremely complex",
                    req.page + 1,
                    self.stuck_after.as_secs_f32().max(1.0)
                );
                lock(&self.abandoned).push(RenderedPage {
                    request: req,
                    width: 0,
                    height: 0,
                    rgba: Vec::new(),
                    error: Some(error),
                    text: None,
                    millis: 0,
                });
                let mut left = lock(&self.replacements_left);
                if *left > 0 && self.spawn_worker() {
                    *left -= 1;
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn worker(id: usize, bytes: Arc<Vec<u8>>, config: RenderConfig, shared: Arc<Shared>, wake: Receiver<()>, out: Sender<RenderedPage>) {
    let settings = config.settings();
    // Outer loop: (re)build parser + cache; rebuilt after a renderer panic.
    loop {
        let pdf = parse(&bytes, config.password.as_deref());
        let cache = RenderCache::new();
        loop {
            let next = lock(&shared.queue).pop();
            let Some(req) = next else {
                if wake.recv().is_err() {
                    return; // pool dropped
                }
                continue;
            };
            if lock(&shared.stuck).contains(&(req.page, req.kind)) {
                let error = format!("page {} was skipped earlier because it took too long to render", req.page + 1);
                let page = RenderedPage { request: req, width: 0, height: 0, rgba: Vec::new(), error: Some(error), text: None, millis: 0 };
                if out.send(page).is_err() {
                    return;
                }
                continue;
            }
            let start = Stopwatch::start();
            if let Some(slot) = lock(&shared.busy).get_mut(id) {
                *slot = Some((req, std::time::Instant::now()));
            }
            #[cfg(test)]
            {
                // Copy out first: a guard held in the `if let` would block the other workers.
                let slow = *lock(&shared.slow_page);
                if let Some((page, delay)) = slow
                    && page == req.page
                {
                    std::thread::sleep(delay);
                }
            }
            let r = match &pdf {
                Some(pdf) => render_page(pdf, &cache, &settings, req),
                None => Err(("the document could not be parsed".into(), false)),
            };
            // If the watchdog cleared our slot meanwhile, it already answered for this request
            // and started a replacement: drop the late result and retire.
            let abandoned = lock(&shared.busy).get_mut(id).is_none_or(|slot| slot.take().is_none());
            if abandoned {
                return;
            }
            let panicked = matches!(r, Err((_, true)));
            if out.send(finish(req, start, r)).is_err() {
                return;
            }
            if panicked {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn watchdog_skips_a_stuck_page_and_keeps_rendering() {
        use super::*;
        let mut pool = RenderPool::new(Arc::new(ONE_PAGE_TWICE.to_vec()), 1, RenderConfig::default());
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
}
