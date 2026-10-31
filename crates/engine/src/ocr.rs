//! Scan & OCR ▸ Recognize text: render pages, read the words in them and add an invisible text
//! layer ([`Edit::AddOcrText`]), making scanned pages searchable and selectable.
//!
//! Recognition is slow (about a second a page), so it is split from the edit: [`Session::ocr_job`]
//! captures what it needs, [`OcrJob::run`] works anywhere (the UI runs it on a worker thread) and
//! [`Session::apply_ocr`] applies the result as one undoable step. The result is only applied
//! while the document is still the one that was read — [`Session::apply_ocr_checked`] refuses a
//! stale result with a named reason instead of writing into a changed document.

use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

use pdfcraft_render::{PageInfo, PageRenderer, RenderConfig, RenderRequest, RequestKind};

pub use pdfcraft_ocr::{
    ConfidenceBand, LANGUAGES, MergeStrategy, Models, Ocr, OcrError, PlacedWord, Recognizer, band_for, is_suspect, low_confidence,
};

use crate::{DocId, Edit, EditError, Session};

pub mod dictionary;
pub(crate) mod layer;

/// Bound recognition/review inputs before cloning or planning glyphs.
pub fn validate_ocr_words<'a>(words: impl IntoIterator<Item = &'a PlacedWord>) -> Result<usize, String> {
    layer::validate_words(words).map_err(|e| e.to_string())
}

/// Which recognition engine to use. The default is ocrs, the pure-Rust engine; "Automatic" is
/// an ensemble — ocrs reading first, Tesseract as a second opinion, merged per the strategy —
/// when both are available.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EngineChoice {
    /// The pure-Rust ocrs engine (the default).
    #[default]
    Ocrs,
    /// The installed `tesseract` binary, as an external process.
    Tesseract,
    /// Ensemble: the default engine first, the other one as a secondary when available.
    Auto,
}

impl EngineChoice {
    /// Parse a choice; unknown names fall back to the default (ocrs), like Acrobat's dialogs
    /// fall back for unknown persisted values.
    pub fn parse(s: &str) -> EngineChoice {
        match s.trim().to_ascii_lowercase().as_str() {
            "tesseract" => EngineChoice::Tesseract,
            "auto" | "automatic" | "ensemble" => EngineChoice::Auto,
            _ => EngineChoice::Ocrs,
        }
    }

    /// The strategy this choice reads with when none was chosen. Automatic exists to bring in
    /// the second engine, so it asks it when the first reading is weak or of unknown
    /// confidence ([`MergeStrategy::ConfidenceWeighted`]); a single named engine reads alone.
    pub fn default_strategy(self) -> MergeStrategy {
        match self {
            EngineChoice::Auto => MergeStrategy::ConfidenceWeighted,
            EngineChoice::Ocrs | EngineChoice::Tesseract => MergeStrategy::PrimaryOnly,
        }
    }

    /// The name settings and tools use for this choice.
    pub fn name(self) -> &'static str {
        match self {
            EngineChoice::Ocrs => "ocrs",
            EngineChoice::Tesseract => "tesseract",
            EngineChoice::Auto => "auto",
        }
    }
}

/// What to produce from a recognition. **Searchable** (the default) adds the existing
/// invisible-text layer on the original page; **EditableText** writes a *new* document with the
/// reviewed words as visible text and no scan image (the writer is the Verify screen's; the mode
/// is plumbed through here). Unknown mode strings mean Searchable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OutputMode {
    #[default]
    Searchable,
    EditableText,
}

impl OutputMode {
    /// Parse a mode name (trimmed, case-insensitive); unknown strings mean Searchable.
    pub fn parse(s: &str) -> OutputMode {
        match s.trim().to_ascii_lowercase().as_str() {
            "editable" | "editable-text" | "editable_text" | "editabletext" => OutputMode::EditableText,
            _ => OutputMode::Searchable,
        }
    }

    /// The name settings and tools use for this mode.
    pub fn name(self) -> &'static str {
        match self {
            OutputMode::Searchable => "searchable",
            OutputMode::EditableText => "editable",
        }
    }
}

/// Recognize Text settings.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrSettings {
    /// Resolution pages are rendered at for recognition (Acrobat's "Downsample to" choices).
    pub dpi: f32,
    /// Language code from [`LANGUAGES`] (the models read the Latin alphabet, whatever this says).
    pub language: String,
    /// Leave pages that already have text alone (Acrobat reports "page contains renderable
    /// text" and skips them). When `false`, such pages are read too (a picture on a page that
    /// also has text, #1028) and only the words that aren't already there are added.
    pub skip_text_pages: bool,
    /// What counts as the page's text when `skip_text_pages` is on: only its own (page content
    /// and form fields). Comments and text boxes are left out, and are not drawn into the picture
    /// that is read, so a scan with a few comments on it is still a scan. `false`: everything
    /// the page shows.
    pub native_text_only: bool,
    /// Leave whole files that already have text alone (the batch flow's "Skip files that
    /// already contain text"; a skipped file is its own result bucket, never an OCR success).
    pub skip_text_files: bool,
    /// Read pages even when the skip options would leave them alone ("Force OCR (override
    /// skip options)").
    pub force_ocr: bool,
    /// Which engine reads the pages.
    pub engine: EngineChoice,
    /// How the engines' readings combine (see [`MergeStrategy`]).
    pub strategy: MergeStrategy,
    /// Preprocessing, all off by default (a scan is passed through as rendered):
    /// straighten pages scanned at 90°/180°/270°.
    pub auto_rotate: bool,
    /// Preprocessing: straighten skewed text lines.
    pub deskew: bool,
    /// Preprocessing: 3×3 median filter.
    pub denoise: bool,
    /// Preprocessing: Sauvola binarization.
    pub binarize: bool,
    /// What to produce (searchable text layer, or a new editable document).
    pub output_mode: OutputMode,
    /// Restrict recognition to this rectangle of the rendered page image, in pixels
    /// `[x0, y0, x1, y1]` (the Verify screen's "Re-OCR this region").
    pub region: Option<[f32; 4]>,
    /// The batch note's separate per-word threshold: words with `0 <= conf <= threshold` are
    /// counted for "N low-confidence word(s) on page(s) …" (see [`low_confidence`]; unrelated
    /// to the review overlay's bands, which use [`band_for`]).
    pub low_confidence_threshold: f32,
    /// Where the Tesseract program lives, when the user configured it (a preference — the
    /// `PDFCRAFT_TESSERACT` environment variable is the fallback in the engine; never taken
    /// from document data). A configured path must be absolute to an existing, executable
    /// file; an invalid configuration makes the engine unavailable rather than silently
    /// searching the PATH. `None` means the PATH search decides.
    pub tesseract_path: Option<String>,
}

/// A recognised word whose box is at least this much covered by the page's own text is that
/// text read again, not a word in a picture.
const ALREADY_ON_PAGE: f32 = 0.5;

/// The long side of an A4 sheet in points (a letter page's, 792, is shorter): "Downsample to"
/// is a resolution on paper this size or smaller.
const SHEET: f32 = 842.0;

/// How much of `word` the `text` boxes cover, from 0 to 1. Boxes are `[x0, y0, x1, y1]` in page
/// view space. Overlaps between the text boxes are counted twice (glyphs rarely overlap), so the
/// sum is capped at the whole word.
fn covered(word: [f32; 4], text: &[[f32; 4]]) -> f32 {
    // `max(0.0)` also turns a NaN side into 0: a degenerate box covers nothing.
    let area = |[x0, y0, x1, y1]: [f32; 4]| (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let total = area(word);
    if !total.is_finite() || total <= 0.0 {
        return 0.0;
    }
    let inside: f32 =
        text.iter().map(|g| area([word[0].max(g[0]), word[1].max(g[1]), word[2].min(g[2]), word[3].min(g[3])])).filter(|a| a.is_finite()).sum();
    (inside / total).min(1.0)
}

impl Default for OcrSettings {
    fn default() -> Self {
        OcrSettings {
            dpi: 300.0,
            language: "en".into(),
            skip_text_pages: true,
            native_text_only: false,
            skip_text_files: false,
            force_ocr: false,
            engine: EngineChoice::default(),
            strategy: MergeStrategy::default(),
            auto_rotate: false,
            deskew: false,
            denoise: false,
            binarize: false,
            output_mode: OutputMode::default(),
            region: None,
            low_confidence_threshold: 60.0,
            tesseract_path: None,
        }
    }
}

/// What recognition found on one page.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrPage {
    pub page: usize,
    pub words: Vec<PlacedWord>,
    /// Words recognised but left out because the page's own text already has them there.
    pub already_on_page: usize,
    /// Why the page was not read, if it was skipped.
    pub skipped: Option<String>,
    /// Notes about a partial reading: the words stand, but something on the way did not (an
    /// ensemble partner failed and the page keeps the primary engine's reading alone).
    pub notes: Vec<String>,
}

impl OcrPage {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }

    /// Mean of the confidences the page's words report (`None` when none do).
    pub fn mean_confidence(&self) -> Option<f32> {
        let (sum, n) = self.words.iter().filter_map(|w| w.confidence).fold((0.0f64, 0usize), |(s, n), c| (s + c as f64, n + 1));
        (n > 0).then_some((sum / n as f64) as f32)
    }

    /// The words to flag for review: their confidence is in the Low band ([`band_for`]).
    /// Excluded are words the caller wants gone — removed words, words in the user dictionary,
    /// skipped words — which is review state, so this reports the indices and the reviewer
    /// filters them.
    pub fn suspects(&self) -> Vec<usize> {
        self.words.iter().enumerate().filter(|(_, w)| pdfcraft_ocr::is_suspect(w.confidence)).map(|(i, _)| i).collect()
    }
}

/// The engines a job reads with: one primary and, for ensemble strategies, an optional
/// secondary.
pub struct Recognizers {
    pub primary: Arc<dyn Recognizer>,
    pub secondary: Option<Arc<dyn Recognizer>>,
}

struct EngineCache {
    ocrs: Option<Arc<dyn Recognizer>>,
}

/// How long a negative engine probe (no program there) is remembered. A short TTL, not
/// forever: a tesseract installed while the app runs is found by the next job, without a
/// restart. An engine that was found is remembered for the session.
#[cfg(not(target_arch = "wasm32"))]
const NEGATIVE_TTL: Duration = Duration::from_secs(30);

/// The remembered Tesseract probe: the configured location it answers for, the engine it
/// found (`None` for a probe that failed), and when that probe happened.
#[cfg(not(target_arch = "wasm32"))]
struct CachedTesseract {
    key: Option<String>,
    found: Option<Arc<dyn Recognizer>>,
    at: Instant,
}

/// Whether a probe result still counts: a found engine for the session, a failed probe until
/// [`NEGATIVE_TTL`] passes (split out so tests can pass the clock).
#[cfg(not(target_arch = "wasm32"))]
fn probe_fresh(found: bool, at: Instant, now: Instant) -> bool {
    found || now.duration_since(at) < NEGATIVE_TTL
}

/// The remembered Tesseract probe (one slot: the last location asked about).
#[cfg(not(target_arch = "wasm32"))]
static TESSERACT_PROBE: Mutex<Option<CachedTesseract>> = Mutex::new(None);

/// Whether a background probe, started for the UI by [`tesseract_engine_now`], is running now.
/// At most one runs at a time.
#[cfg(not(target_arch = "wasm32"))]
static PROBING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The Tesseract engine: found once (it answers `--list-langs` through a process) and then
/// remembered, per configured path; a failed probe is retried after [`NEGATIVE_TTL`]. No
/// static mutex is held while the discovery process runs: the cache is read under the lock,
/// released for the probe, and the result is written back when it arrives (concurrent probes
/// of the same location may both run; the last write wins and both answers are valid). A
/// user-configured path (`settings.tesseract_path`, else the `PDFCRAFT_TESSERACT`
/// environment variable) is validated before any process starts; an invalid configuration is
/// unavailable rather than a PATH fallback. This waits for the probe: the UI thread uses
/// [`tesseract_engine_now`].
#[cfg(not(target_arch = "wasm32"))]
fn tesseract_engine(configured: Option<&str>) -> Option<Arc<dyn Recognizer>> {
    if let Cached::Fresh(found) = cached_tesseract(configured) {
        return found;
    }
    let key = configured.map(str::to_string);
    // The probe (a `--list-langs` process, up to the timeout) runs outside every static mutex.
    let found = match configured {
        Some(path) => pdfcraft_ocr::tesseract::TesseractCli::at(std::path::PathBuf::from(path)).map(|t| Arc::new(t) as Arc<dyn Recognizer>),
        None => pdfcraft_ocr::tesseract::TesseractCli::find().map(|t| Arc::new(t) as Arc<dyn Recognizer>),
    };
    *TESSERACT_PROBE.lock().unwrap_or_else(|e| e.into_inner()) = Some(CachedTesseract { key, found: found.clone(), at: Instant::now() });
    found
}

/// What the UI may know about the Tesseract probe without waiting for it.
#[cfg(not(target_arch = "wasm32"))]
enum Probe {
    /// An answer: the engine, or `None` when there is none. It may be an old one that a
    /// background probe is refreshing.
    Known(Option<Arc<dyn Recognizer>>),
    /// No answer yet: the first background probe is running (or could not be started).
    Pending,
}

/// What the cache holds for a location.
#[cfg(not(target_arch = "wasm32"))]
enum Cached {
    /// A usable answer (see [`probe_fresh`]).
    Fresh(Option<Arc<dyn Recognizer>>),
    /// A failed probe older than [`NEGATIVE_TTL`]: still the best known answer, but due again.
    Stale(Option<Arc<dyn Recognizer>>),
    /// Nothing for this location.
    Missing,
}

/// [`tesseract_engine`] for the UI thread, which must never wait on a process: the cached
/// answer is returned at once, and when it is missing or due for a retry one background thread
/// runs the probe (it fills the cache). The answer is [`Probe::Pending`] only until the first
/// probe of a location lands, so a retry never makes the choice flicker.
#[cfg(not(target_arch = "wasm32"))]
fn tesseract_engine_now(configured: Option<&str>) -> Probe {
    let known = match cached_tesseract(configured) {
        Cached::Fresh(found) => return Probe::Known(found),
        Cached::Stale(found) => Some(found),
        Cached::Missing => None,
    };
    if !PROBING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        let configured = configured.map(str::to_string);
        let started = std::thread::Builder::new().name("pdfcraft-tesseract-probe".into()).spawn(move || {
            let _ = tesseract_engine(configured.as_deref());
            PROBING.store(false, std::sync::atomic::Ordering::Release);
        });
        if started.is_err() {
            PROBING.store(false, std::sync::atomic::Ordering::Release);
        }
    }
    known.map_or(Probe::Pending, Probe::Known)
}

/// Whether the Tesseract probe is running in the background right now. The interface asks it
/// while it shows an engine choice that read as "still checking", to look again soon.
pub fn engine_probe_running() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        PROBING.load(std::sync::atomic::Ordering::Acquire)
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// What the cache holds for `configured`. Never starts a process.
#[cfg(not(target_arch = "wasm32"))]
fn cached_tesseract(configured: Option<&str>) -> Cached {
    let slot = TESSERACT_PROBE.lock().unwrap_or_else(|e| e.into_inner());
    cache_lookup(slot.as_ref(), configured, Instant::now())
}

/// [`cached_tesseract`] over an explicit slot and clock (so tests can pass both).
#[cfg(not(target_arch = "wasm32"))]
fn cache_lookup(slot: Option<&CachedTesseract>, configured: Option<&str>, now: Instant) -> Cached {
    match slot {
        Some(c) if c.key.as_deref() == configured => {
            if probe_fresh(c.found.is_some(), c.at, now) {
                Cached::Fresh(c.found.clone())
            } else {
                Cached::Stale(c.found.clone())
            }
        }
        _ => Cached::Missing,
    }
}

/// Whether the ocrs models are installed, remembered like the Tesseract probe (found: for the
/// session; missing: for [`NEGATIVE_TTL`]), so per-frame UI checks do not search the disk.
#[cfg(not(target_arch = "wasm32"))]
fn models_installed_cached() -> bool {
    static MODELS: Mutex<Option<(bool, Instant)>> = Mutex::new(None);
    let mut slot = MODELS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((found, at)) = *slot
        && probe_fresh(found, at, Instant::now())
    {
        return found;
    }
    let found = Models::find().is_some();
    *slot = Some((found, Instant::now()));
    found
}

/// The web build has no disk search worth caching.
#[cfg(target_arch = "wasm32")]
fn models_installed_cached() -> bool {
    Models::find().is_some()
}

/// The program's `--version` line, remembered per program path for the session (`ocr_status`
/// asks every time it is called; the version of an installed binary does not change under a
/// running app often enough to start a process each time).
#[cfg(not(target_arch = "wasm32"))]
fn tesseract_version(program: &str) -> Option<String> {
    static VERSIONS: Mutex<Vec<(String, Option<String>)>> = Mutex::new(Vec::new());
    if let Some((_, v)) = VERSIONS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(p, _)| p == program) {
        return v.clone();
    }
    // Validated (absolute, existing, executable) before the process starts; run outside the lock.
    let path = std::path::PathBuf::from(program);
    let version = pdfcraft_ocr::tesseract::program_is_usable(&path).then(|| pdfcraft_ocr::tesseract::TesseractCli::new(path).version()).flatten();
    let mut cache = VERSIONS.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() < 16 && !cache.iter().any(|(p, _)| p == program) {
        cache.push((program.to_string(), version.clone()));
    }
    version
}

/// The Tesseract program the engine would run, named for `ocr_status`: the user-configured
/// location when one is set, else what the PATH search resolves to. This function itself
/// starts no process (the PATH search only looks at files), so a path here says where the
/// engine looks, not that a program answers there; [`engines_status`], which reports it, does
/// run the cached `--list-langs` probe and a cached `--version`. `None` when nothing is
/// configured and the PATH holds no `tesseract` (or, on the web, when there is no process to
/// run).
#[cfg(not(target_arch = "wasm32"))]
pub fn tesseract_program_path(configured: Option<&str>) -> Option<String> {
    match configured {
        Some(p) => Some(p.to_string()),
        None => pdfcraft_ocr::tesseract::path_search().map(|p| p.to_string_lossy().into_owned()),
    }
}

/// One engine's status, for `ocr_status`: whether it can run, why not when it cannot, and for
/// Tesseract the program it would run and that program's version (both `None` on the web).
#[derive(Clone, Debug, PartialEq)]
pub struct EngineStatus {
    pub id: &'static str,
    pub available: bool,
    pub why_not: Option<String>,
    pub program: Option<String>,
    pub version: Option<String>,
}

/// The engines' status, in the order the Engine dropdown lists them: ocrs (built in), then
/// tesseract (the external program). Tesseract's availability is judged the same way
/// [`recognizers`] judges it under the default settings: the `PDFCRAFT_TESSERACT` environment
/// variable when set, else the PATH. This waits for the (cached) Tesseract probe; the tools
/// and tests call it, never the UI thread.
pub fn engines_status() -> Vec<EngineStatus> {
    engines_status_for(&OcrSettings::default())
}

/// [`engines_status`] for the location a job's `settings` name: `settings.tesseract_path` when
/// set, else the environment variable, else the PATH, as [`recognizers`] decides it. (No tool
/// takes a location, because an agent must not choose which program runs; the tools report the
/// default settings.)
pub fn engines_status_for(settings: &OcrSettings) -> Vec<EngineStatus> {
    let ocrs = Models::find().is_some();
    let mut out = vec![EngineStatus { id: "ocrs", available: ocrs, why_not: (!ocrs).then(models_missing), program: None, version: None }];
    #[cfg(not(target_arch = "wasm32"))]
    {
        let configured = configured_tesseract(settings);
        let configured = configured.as_deref();
        let available = tesseract_engine(configured).is_some();
        let program = tesseract_program_path(configured);
        // The version is a `--version` run against the validated program path, remembered per
        // path; a program that answers `--list-langs` but not `--version` still counts as
        // available.
        let version = if available { program.as_deref().and_then(tesseract_version) } else { None };
        let why_not = (!available).then(|| match configured {
            Some(p) => format!("the configured tesseract location is not an absolute path to an existing, executable file: {p}"),
            None => tesseract_unavailable(),
        });
        out.push(EngineStatus { id: "tesseract", available, why_not, program, version });
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = settings;
        out.push(EngineStatus { id: "tesseract", available: false, why_not: Some(tesseract_unavailable()), program: None, version: None });
    }
    out
}

/// The web build never spawns processes, so it never has Tesseract.
#[cfg(target_arch = "wasm32")]
fn tesseract_engine(configured: Option<&str>) -> Option<Arc<dyn Recognizer>> {
    let _ = configured;
    None
}

/// The configured Tesseract location: the user preference wins; the environment variable is
/// the fallback (a preference file rarely survives onto another machine). Both are user
/// choices — this is never taken from document data.
fn configured_tesseract(settings: &OcrSettings) -> Option<String> {
    settings.tesseract_path.clone().or_else(|| std::env::var_os("PDFCRAFT_TESSERACT").map(|v| v.to_string_lossy().into_owned()))
}

/// The engine selector: builds each engine once and hands out the pair `settings.engine`
/// asks for. "Automatic" is an ensemble when both engines are available; a named engine that
/// cannot start fails with its cause named.
pub fn recognizers(settings: &OcrSettings) -> Result<Recognizers, String> {
    // The Tesseract probe (a `--list-langs` process) runs before any static mutex is taken, so
    // recognition on other threads is never blocked behind someone's discovery. A job that
    // names its engines never starts it: only Tesseract and Automatic read anything from it,
    // so an ocrs-only job pays no process and no wait.
    let configured = configured_tesseract(settings);
    let tesseract =
        if matches!(settings.engine, EngineChoice::Tesseract | EngineChoice::Auto) { tesseract_engine(configured.as_deref()) } else { None };
    static ENGINES: Mutex<Option<EngineCache>> = Mutex::new(None);
    let mut slot = ENGINES.lock().unwrap_or_else(|e| e.into_inner());
    let cache = slot.get_or_insert_with(|| EngineCache { ocrs: None });
    if cache.ocrs.is_none() {
        // Cheap to re-attempt while the models are missing; expensive work is cached forever.
        cache.ocrs = pdfcraft_ocr::OcrsRecognizer::find().ok().map(|r| Arc::new(r) as Arc<dyn Recognizer>);
    }
    let ocrs = cache.ocrs.clone();
    match settings.engine {
        EngineChoice::Ocrs => Ok(Recognizers { primary: ocrs.ok_or_else(models_missing)?, secondary: None }),
        EngineChoice::Tesseract => Ok(Recognizers { primary: tesseract.ok_or_else(tesseract_unavailable)?, secondary: None }),
        EngineChoice::Auto => match (ocrs, tesseract) {
            (Some(p), s) => Ok(Recognizers { primary: p, secondary: s }),
            (None, Some(p)) => Ok(Recognizers { primary: p, secondary: None }),
            (None, None) => Err(models_missing()),
        },
    }
}

/// Why Tesseract cannot run, named for the user.
#[cfg(not(target_arch = "wasm32"))]
fn tesseract_unavailable() -> String {
    "the tesseract program was not found (install it, or add it to the PATH)".into()
}

/// Why Tesseract cannot run, named for the user (the web build has no processes).
#[cfg(target_arch = "wasm32")]
fn tesseract_unavailable() -> String {
    "the tesseract engine is not available in the web build".into()
}

/// Why the ocrs engine cannot run, in the end-user wording the UI catalogs translate (the
/// same text as [`pdfcraft_ocr::OcrError::NoModels`]: no developer commands in front of users).
fn models_missing() -> String {
    pdfcraft_ocr::OcrError::NoModels.to_string()
}

/// Whether recognition can run at all: the ocrs models are installed, or (off the web) a
/// Tesseract program is found. This may run the Tesseract probe (a process, cached) and so
/// waits for it: tools and tests call it; the UI asks [`engine_unavailable_reason`], which
/// never waits.
pub fn available() -> bool {
    let configured = std::env::var_os("PDFCRAFT_TESSERACT").map(|v| v.to_string_lossy().into_owned());
    Models::find().is_some() || tesseract_engine(configured.as_deref()).is_some()
}

/// Why `engine` cannot run right now, named for the UI's tooltips (the Engine dropdown
/// disables the choice with this text); `None` when it can. The app UI has no Tesseract
/// location field of its own, so this judges the environment variable, exactly like
/// [`available`] (no tool takes a per-job location either; `OcrSettings::tesseract_path` is
/// for library callers, and [`recognizers`] validates it on its own).
///
/// Called on the UI thread every frame, so it never waits on a process: the answers are
/// cached ([`NEGATIVE_TTL`] for a missing engine, refreshed in the background), and until the
/// first Tesseract probe lands the choice reads as "still checking" (see
/// [`engine_probe_running`], to ask for another frame meanwhile).
pub fn engine_unavailable_reason(engine: EngineChoice) -> Option<String> {
    let ocrs = models_installed_cached();
    #[cfg(not(target_arch = "wasm32"))]
    let tesseract = {
        let configured = std::env::var_os("PDFCRAFT_TESSERACT").map(|v| v.to_string_lossy().into_owned());
        match tesseract_engine_now(configured.as_deref()) {
            Probe::Known(Some(_)) => Ok(()),
            Probe::Known(None) => Err(tesseract_unavailable()),
            Probe::Pending => Err(tesseract_checking()),
        }
    };
    #[cfg(target_arch = "wasm32")]
    let tesseract: Result<(), String> = Err(tesseract_unavailable());
    match engine {
        EngineChoice::Ocrs => (!ocrs).then(models_missing),
        EngineChoice::Tesseract => tesseract.err(),
        EngineChoice::Auto => match (ocrs, tesseract) {
            (true, _) | (false, Ok(())) => None,
            (false, Err(why)) => Some(format!("{}; {why}", models_missing())),
        },
    }
}

/// Why Tesseract cannot be chosen yet: its probe is still running in the background.
#[cfg(not(target_arch = "wasm32"))]
fn tesseract_checking() -> String {
    "still checking whether the tesseract program can run; try again in a moment".into()
}

/// What a started job expects of the document when its result is applied: the same open
/// document, unchanged. [`Session::stale_reason`] names any difference it finds.
#[derive(Clone)]
pub struct OcrGuard {
    pub(crate) doc: DocId,
    pub(crate) pages: usize,
    pub(crate) revisions: usize,
    /// The document's working bytes when the job started; any edit rewrites them, so a shared
    /// `Arc` pointer means "untouched".
    pub(crate) bytes: Arc<Vec<u8>>,
}

/// Everything recognition needs from a document, detached from the session.
pub struct OcrJob {
    bytes: Arc<Vec<u8>>,
    password: Option<String>,
    infos: Vec<PageInfo>,
    /// Per page: the render scale (pixels per point) that shows its images at their own
    /// resolution, if it has any.
    native: Vec<Option<f32>>,
    pub pages: Vec<usize>,
    pub settings: OcrSettings,
    /// What to check before applying this job's result.
    pub guard: OcrGuard,
}

/// A page's "has text" probe for skip-text: at least one non-blank text run. A failed probe
/// (the text layer could not be read) never enables a skip.
fn page_has_text(text: Option<&Arc<pdfcraft_render::PageText>>) -> bool {
    text.is_some_and(|t| t.glyphs.iter().any(|g| !g.text.trim().is_empty()))
}

/// Clamp a region to the image, snapping outward to whole pixels. `None` when nothing of it
/// is inside the image (empty, inverted, NaN, or wholly outside).
pub fn clamp_region(region: [f32; 4], width: u32, height: u32) -> Option<[u32; 4]> {
    let (w, h) = (f64::from(width), f64::from(height));
    if region.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let (x0, y0, x1, y1) = (region[0] as f64, region[1] as f64, region[2] as f64, region[3] as f64);
    let (x0, y0) = ((x0.floor().max(0.0).min(w)) as u32, (y0.floor().max(0.0).min(h)) as u32);
    let (x1, y1) = ((x1.ceil().max(0.0).min(w)) as u32, (y1.ceil().max(0.0).min(h)) as u32);
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
}

/// Map a recognized word from its source page's user space into the editable output's page
/// user space (the new page is the source's displayed size, unrotated): user → view → new
/// user. The box never moves relative to what the reviewer saw.
fn editable_word(w: &PlacedWord, info: &PageInfo) -> PlacedWord {
    let to_new = |p: [f64; 2]| {
        let v = info.user_to_view(p[0] as f32, p[1] as f32);
        [f64::from(v[0]), f64::from(info.height) - f64::from(v[1])]
    };
    let origin = to_new(w.origin);
    let br = to_new([w.origin[0] + w.across[0], w.origin[1] + w.across[1]]);
    let tl = to_new([w.origin[0] + w.up[0], w.origin[1] + w.up[1]]);
    PlacedWord {
        text: w.text.clone(),
        origin,
        across: [br[0] - origin[0], br[1] - origin[1]],
        up: [tl[0] - origin[0], tl[1] - origin[1]],
        ..w.clone()
    }
}

impl OcrJob {
    /// The pixels-per-point this job renders `page` at (the real render dpi is this times 72).
    /// The Verify screen uses it to express a region picked on its own preview in the pixels
    /// the job will actually read. A scan is read at its own resolution (enlarging it only
    /// blurs the letters), at most the chosen one.
    pub fn render_scale(&self, page: usize) -> f32 {
        let wanted = self.settings.dpi.clamp(72.0, 600.0) / 72.0;
        let Some(native) = self.native.get(page).copied().flatten() else { return wanted };
        // A scan whose page is larger than a sheet (often a box given in pixels, one point per
        // pixel, #1848) is measured as if shrunk to one: otherwise 1 px/pt is already its full
        // image, and no "Downsample to" choice could ask for less.
        let fit = self.infos.get(page).map_or(1.0, |i| (SHEET / i.width.max(i.height).max(1e-3)).min(1.0));
        native.clamp(fit, wanted * fit)
    }

    /// Whether any of the job's pages already has text (at least one non-blank run): the
    /// batch flow's "Skip files that already contain text" probe. A failed probe never
    /// enables a skip, so a page whose text cannot be read counts as no text there.
    pub fn has_text(&self) -> bool {
        let mut r = PageRenderer::new(self.bytes.clone(), RenderConfig { password: self.password.as_deref().map(Arc::from), ..Default::default() });
        self.pages.iter().any(|&page| {
            let t = r.render(RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
            page_has_text(t.text.as_ref())
        })
    }

    /// Read the pages. `progress(done, total)` is called before each page; returning `false`
    /// stops (the pages read so far are returned).
    pub fn run(self, recognizers: &Recognizers, mut progress: impl FnMut(usize, usize) -> bool) -> Vec<OcrPage> {
        let config =
            RenderConfig { password: self.password.as_deref().map(Arc::from), hide_comments: self.settings.native_text_only, ..Default::default() };
        let mut r = PageRenderer::new(self.bytes.clone(), config);
        let mut out = Vec::new();
        let total = self.pages.len();
        let options = pdfcraft_ocr::RecognizeOptions { language: self.settings.language.clone() };
        for (done, &page) in self.pages.iter().enumerate() {
            if !progress(done, total) {
                break;
            }
            let Some(info) = self.infos.get(page) else { continue };
            let skip = |why: &str| OcrPage { page, words: Vec::new(), already_on_page: 0, skipped: Some(why.into()), notes: Vec::new() };
            // The boxes of the page's own text (an earlier OCR layer's included), in view space.
            let existing: Vec<[f32; 4]> = r
                .render(RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() })
                .text
                .map(|t| t.glyphs.iter().filter(|g| !g.text.trim().is_empty()).map(|g| g.rect).collect())
                .unwrap_or_default();
            if self.settings.skip_text_pages && !self.settings.force_ocr && !existing.is_empty() {
                out.push(skip("the page already contains text"));
                continue;
            }
            // A scan is read at its own resolution (enlarging it only blurs the letters), at
            // most the chosen one.
            let scale = self.render_scale(page);
            let shot = r.render(RenderRequest { page, scale, ..Default::default() });
            if let Some(e) = shot.error {
                out.push(skip(&e));
                continue;
            }
            let full = match pdfcraft_ocr::OcrImage::new(shot.width, shot.height, shot.rgba.to_vec()) {
                Ok(img) => img,
                Err(e) => {
                    out.push(skip(&e.to_string()));
                    continue;
                }
            };
            // The optional region: crop the rendered page, clamped; an empty region skips the
            // page with a named reason (nothing was asked of it).
            let (image, offset) = match self.settings.region {
                None => (full, [0.0f32; 2]),
                Some(region) => match clamp_region(region, shot.width, shot.height) {
                    Some([x0, y0, x1, y1]) => match crop(&full, x0, y0, x1, y1) {
                        Some(c) => (c, [x0 as f32, y0 as f32]),
                        None => {
                            out.push(skip("the selected region is empty"));
                            continue;
                        }
                    },
                    None => {
                        out.push(skip("the selected region is outside the page"));
                        continue;
                    }
                },
            };
            // Preprocessing sits between the page raster and recognition; its word boxes come
            // back in processed coordinates and are mapped to the ORIGINAL raster's before
            // placement. The render dpi is the real one (pixels per point × 72), never a
            // fixed value.
            let pre = pdfcraft_ocr::preprocess::preprocess(
                image,
                scale * 72.0,
                &pdfcraft_ocr::preprocess::PreprocessOptions {
                    auto_rotate: self.settings.auto_rotate,
                    deskew: self.settings.deskew,
                    denoise: self.settings.denoise,
                    binarize: self.settings.binarize,
                },
            );
            let (image, inverse) = match pre {
                Ok(p) => (p.image, p.inverse_transform),
                Err(e) => {
                    out.push(skip(&e.to_string()));
                    continue;
                }
            };
            let outcome = match pdfcraft_ocr::recognize_with_strategy(
                recognizers.primary.as_ref(),
                recognizers.secondary.as_deref(),
                self.settings.strategy,
                &image,
                &options,
            ) {
                Ok(o) => o,
                Err(e) => {
                    out.push(skip(&e.to_string()));
                    continue;
                }
            };
            let scale = shot.width as f32 / info.width.max(1e-3);
            let to_user = |x: f32, y: f32| {
                // processed pixels → original raster pixels (the exact inverse) → past the
                // region crop → page view space.
                let (ox, oy) = inverse.apply(f64::from(x), f64::from(y));
                let [u, v] = info.view_to_user((ox as f32 + offset[0]) / scale, (oy as f32 + offset[1]) / scale);
                [u as f64, v as f64]
            };
            // Validate recognition geometry and compare against existing text in view space,
            // undoing preprocessing and region offsets before testing overlap.
            let (fresh, already): (Vec<_>, Vec<_>) = outcome.lines.iter().flat_map(|l| &l.words).filter(|w| box_writable(w)).partition(|w| {
                let [l, t, r, b] = w.rect;
                let bounds = [(l, t), (r, t), (r, b), (l, b)].into_iter().fold(
                    [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY],
                    |mut bounds, (x, y)| {
                        let (x, y) = inverse.apply(f64::from(x), f64::from(y));
                        let x = (x as f32 + offset[0]) / scale;
                        let y = (y as f32 + offset[1]) / scale;
                        bounds[0] = bounds[0].min(x);
                        bounds[1] = bounds[1].min(y);
                        bounds[2] = bounds[2].max(x);
                        bounds[3] = bounds[3].max(y);
                        bounds
                    },
                );
                covered(bounds, &existing) < ALREADY_ON_PAGE
            });
            let words = fresh.into_iter().map(|w| PlacedWord::place(w, to_user)).collect::<Vec<_>>();
            let mut notes = outcome.notes;
            // Only visible editable output needs display glyphs. Searchable output preserves
            // every Unicode character with empty Type3 programs and ToUnicode maps.
            if self.settings.output_mode == OutputMode::EditableText && !words.is_empty() {
                let missing = pdfcraft_fonts::TextPlan::for_words(words.iter().map(|w| w.text.as_str())).missing();
                if missing > 0 {
                    notes.push(format!("{missing} characters had no font that could show them and appear as ? in the output"));
                }
            }
            out.push(OcrPage { page, words, already_on_page: already.len(), skipped: None, notes });
        }
        progress(total, total);
        out
    }
}

/// Whether a word's pixel box may reach the content stream: width and height (right − left,
/// bottom − top) must be positive and finite. Anything else — a zero, negative, NaN or
/// infinite size — is dropped before placement rather than written into the page.
fn box_writable(word: &pdfcraft_ocr::Word) -> bool {
    let [l, t, r, b] = word.rect;
    let (width, height) = (r - l, b - t);
    width > 0.0 && height > 0.0 && width.is_finite() && height.is_finite()
}

/// Crop `[x0, y0, x1, y1]` out of `image` as a new [`OcrImage`] (the sides are already clamped
/// by [`clamp_region`]).
fn crop(image: &pdfcraft_ocr::OcrImage, x0: u32, y0: u32, x1: u32, y1: u32) -> Option<pdfcraft_ocr::OcrImage> {
    let (w, h) = (x1.checked_sub(x0)?, y1.checked_sub(y0)?);
    let stride = image.width() as usize * 4;
    let mut out = Vec::with_capacity((w as usize).checked_mul(h as usize)?.checked_mul(4)?);
    for y in y0..y1 {
        let start = y as usize * stride + x0 as usize * 4;
        let row = image.rgba().get(start..start + (x1 - x0) as usize * 4)?;
        out.extend_from_slice(row);
    }
    pdfcraft_ocr::OcrImage::new(w, h, out).ok()
}

impl Session {
    /// Capture what recognising `pages` (0-based; empty = all) of document `id` needs.
    pub fn ocr_job(&self, id: DocId, pages: &[usize], settings: OcrSettings) -> Option<OcrJob> {
        let doc = self.get(id)?;
        let all = doc.info.pages.len();
        let pages: Vec<usize> = if pages.is_empty() { (0..all).collect() } else { pages.iter().copied().filter(|p| *p < all).collect() };
        let native = (0..all)
            .map(|p| {
                if !pages.contains(&p) {
                    return None;
                }
                doc.page_images(p)
                    .iter()
                    .filter(|i| i.width > 1 && i.height > 1)
                    .map(|i| {
                        let m = i.matrix;
                        let (w, h) = (m[0].hypot(m[1]), m[2].hypot(m[3]));
                        (i.width as f64 / w.max(1e-6)).max(i.height as f64 / h.max(1e-6)) as f32
                    })
                    .reduce(f32::max)
            })
            .collect();
        let guard = OcrGuard { doc: id, pages: all, revisions: doc.revision_ends().len(), bytes: doc.bytes.clone() };
        Some(OcrJob { bytes: doc.bytes.clone(), password: doc.password.clone(), infos: doc.info.pages.clone(), native, pages, settings, guard })
    }

    /// Add the words found by [`OcrJob::run`] as one undoable step. Returns the number of words.
    pub fn apply_ocr(&mut self, id: DocId, found: &[OcrPage]) -> Result<usize, EditError> {
        let edits: Vec<Edit> =
            found.iter().filter(|p| !p.words.is_empty()).map(|p| Edit::AddOcrText { page: p.page, words: p.words.clone() }).collect();
        let words = found.iter().map(|p| p.words.len()).sum();
        if !edits.is_empty() {
            self.apply(id, Edit::Batch { label: "Recognize text".into(), edits })?;
        }
        Ok(words)
    }

    /// Why a job's result may not be applied to this session any more: the document was closed,
    /// edited, re-saved, or its pages changed since the job started. `None` means it is safe.
    pub fn stale_reason(&self, guard: &OcrGuard) -> Option<String> {
        let Some(doc) = self.get(guard.doc) else {
            return Some("the document is no longer open".into());
        };
        if !Arc::ptr_eq(&doc.bytes, &guard.bytes) {
            return Some("the document changed since recognition started".into());
        }
        let now = doc.info.pages.len();
        if now != guard.pages {
            return Some(format!("the page count changed from {} to {} since recognition started", guard.pages, now));
        }
        let revisions = doc.revision_ends().len();
        if revisions != guard.revisions {
            let had = guard.revisions;
            return Some(format!("the document gained revisions ({had} → {revisions}) since recognition started"));
        }
        None
    }

    /// What applying a result to document `id` expects of it right now. After a checked apply
    /// succeeds the document has changed (that apply is its newest edit), so a caller that
    /// applies more of the same review in further steps (page by page) takes a fresh guard
    /// here; any other edit in between still makes the next apply stale.
    pub fn ocr_guard(&self, id: DocId) -> Option<OcrGuard> {
        let doc = self.get(id)?;
        Some(OcrGuard { doc: id, pages: doc.info.pages.len(), revisions: doc.revision_ends().len(), bytes: doc.bytes.clone() })
    }

    /// [`Self::apply_ocr`], but only while the document is still the one that was read; a stale
    /// result is refused with a named reason and nothing is written.
    pub fn apply_ocr_checked(&mut self, id: DocId, found: &[OcrPage], guard: &OcrGuard) -> Result<usize, String> {
        if guard.doc != id {
            return Err("the result belongs to another document".into());
        }
        if let Some(why) = self.stale_reason(guard) {
            return Err(why);
        }
        self.apply_ocr(id, found).map_err(|e| e.to_string())
    }

    /// The editable-text output: a NEW document with the recognized words as VISIBLE text at
    /// their original boxes and no scan image — one page per source page, blank where nothing
    /// was recognized, each page the displayed size of its source. The open document is never
    /// touched; the caller chooses where the bytes are written. Refuses when there are no
    /// words at all (an empty document would silently lose the scan's content).
    pub fn editable_ocr_bytes(&self, id: DocId, found: &[OcrPage]) -> Result<Arc<Vec<u8>>, String> {
        let doc = self.get(id).ok_or("no such document")?;
        let words = layer::validate_words(found.iter().flat_map(|p| &p.words)).map_err(|e| e.to_string())?;
        if words == 0 {
            return Err("the recognition found no words, so there is no editable text to write".into());
        }
        // The words per page, in user space; and one plan over all of them, so a face's Type3
        // font is built once and shared by every page that uses it.
        let page_words: Vec<Vec<PlacedWord>> = (0..doc.info.pages.len())
            .map(|p| {
                let Some(info) = doc.info.pages.get(p) else { return Vec::new() };
                found.iter().find(|f| f.page == p).map(|f| f.words.iter().map(|w| editable_word(w, info)).collect()).unwrap_or_default()
            })
            .collect();
        layer::validate_words(page_words.iter().flatten()).map_err(|e| e.to_string())?;
        let mut plan = pdfcraft_fonts::TextPlan::for_words(page_words.iter().flat_map(|ws| ws.iter().map(|w| w.text.as_str())));
        let pages: Vec<(f64, f64, pdfcraft_cos::Dict, Vec<u8>)> = (0..doc.info.pages.len())
            .map(|p| {
                let info = doc.info.pages.get(p);
                (info.map_or(612.0, |i| f64::from(i.width)), info.map_or(792.0, |i| f64::from(i.height)), pdfcraft_cos::Dict::new(), Vec::new())
            })
            .collect();
        let mut created = pdfcraft_create::from_contents(&pages).map_err(|e| e.to_string())?;
        // One shared Helvetica for every page: added once into the new document, then linked
        // from each page's resources as /PCHelv (the name the text layers draw with). The
        // plan's fonts are linked per page too; their objects are shared.
        let mut font = pdfcraft_cos::Dict::new();
        font.set(b"Type".to_vec(), pdfcraft_cos::Object::name("Font"));
        font.set(b"Subtype".to_vec(), pdfcraft_cos::Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), pdfcraft_cos::Object::name("Helvetica"));
        font.set(b"Encoding".to_vec(), pdfcraft_cos::Object::name("WinAnsiEncoding"));
        let fr = created.add(font);
        // The page tree's kids, direct array or indirect reference (a fresh create document
        // keeps /Kids inline) — the output's pages are linked through them.
        let kids = created
            .root()
            .and_then(|r| created.get(r).as_dict().cloned())
            .and_then(|d| d.reference(b"Pages"))
            .and_then(|p| created.get(p).as_dict().cloned())
            .and_then(|d| match d.get(b"Kids").map(|k| created.resolve(k)).as_deref() {
                Some(pdfcraft_cos::Object::Array(a)) => Some(a.clone()),
                Some(pdfcraft_cos::Object::Ref(r)) => created.get(*r).as_array().cloned(),
                _ => None,
            });
        for (pi, kid) in kids.into_iter().flatten().enumerate() {
            let pdfcraft_cos::Object::Ref(page) = kid else { continue };
            let _ = created.update_dict(page, |d| {
                let mut res = d.get(b"Resources").and_then(|o| o.as_dict().cloned()).unwrap_or_default();
                let mut fonts = res.get(b"Font").and_then(|o| o.as_dict().cloned()).unwrap_or_default();
                fonts.set(b"PCHelv".to_vec(), pdfcraft_cos::Object::Ref(fr));
                res.set(b"Font".to_vec(), pdfcraft_cos::Object::Dict(fonts));
                d.set(b"Resources".to_vec(), pdfcraft_cos::Object::Dict(res));
            });
            // The page's content is written after the fonts carry their resource names, so
            // the runs can reference them.
            if page_words.get(pi).is_some_and(|ws| !ws.is_empty()) {
                pdfcraft_edit::add_type3_fonts(&mut created, pi, &mut plan).map_err(|e| e.to_string())?;
                let word_offset = page_words.iter().take(pi).map(Vec::len).sum();
                let content = pdfcraft_ocr::visible_text_layer_at(&page_words[pi], &plan, word_offset);
                let mut dict = pdfcraft_cos::Dict::new();
                dict.set(b"PCMark".to_vec(), pdfcraft_cos::Object::name("OCR"));
                let stream = created.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::flate(dict, &content)));
                let _ = created.update_dict(page, |d| d.set(b"Contents".to_vec(), pdfcraft_cos::Object::Ref(stream)));
            }
        }
        self.write_new(&created).map_err(|e| e.to_string())
    }

    /// Recognise text on `pages` and add it, in one call (the CLI and agents use this).
    pub fn recognize_text(&mut self, id: DocId, pages: &[usize], settings: OcrSettings) -> Result<Vec<OcrPage>, String> {
        let job = self.ocr_job(id, pages, settings).ok_or("no such document")?;
        if let Some(why) = self.get(id).and_then(|d| d.read_only_reason.clone()) {
            return Err(why);
        }
        let recognizers = recognizers(&job.settings)?;
        let found = job.run(&recognizers, |_, _| true);
        self.apply_ocr(id, &found).map_err(|e| e.to_string())?;
        Ok(found)
    }
}

/// What [`recognize_file`] did to one file.
#[derive(Clone, Debug, PartialEq)]
pub struct FileResult {
    /// The new file (incrementally saved), or the original when nothing was recognised.
    pub bytes: Arc<Vec<u8>>,
    pub pages: Vec<OcrPage>,
    /// Why the whole file was left alone ("Skip files that already contain text"): a skipped
    /// file is its own result bucket, never counted as an OCR success.
    pub file_skipped: Option<String>,
}

impl FileResult {
    pub fn words(&self) -> usize {
        self.pages.iter().map(|p| p.words.len()).sum()
    }

    /// Whether the file was passed over entirely (never an OCR success).
    pub fn skipped(&self) -> bool {
        self.file_skipped.is_some()
    }
}

/// The output names a multi-file run would write more than once: the batch writes every
/// result into one folder under its input's file name, so two inputs of the same name from
/// different folders would overwrite each other. Compared case-insensitively (the Windows and
/// macOS file systems are), in first-seen order, each listed once.
pub fn duplicate_output_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut listed = std::collections::HashSet::new();
    let mut out = Vec::new();
    for name in names {
        let key = name.to_lowercase();
        if !seen.insert(key.clone()) && listed.insert(key) {
            out.push(name.to_string());
        }
    }
    out
}

/// The batch note's data: how many words sit at or under `threshold` ([`low_confidence`], the
/// separate batch rule), and the 1-based, sorted pages they are on. `None` when there are
/// none, so a note is only written when there is something to review.
pub fn low_confidence_pages(pages: &[OcrPage], threshold: f32) -> Option<(usize, Vec<usize>)> {
    let (mut count, mut page_numbers) = (0usize, Vec::new());
    for p in pages {
        if p.skipped.is_some() {
            continue;
        }
        let on_page = p.words.iter().filter(|w| w.confidence.is_some_and(|c| pdfcraft_ocr::low_confidence(c, threshold))).count();
        if on_page > 0 {
            count += on_page;
            page_numbers.push(p.page + 1);
        }
    }
    page_numbers.sort_unstable();
    (count > 0).then_some((count, page_numbers))
}

/// Recognize text in multiple files: one PDF's bytes in, the searchable PDF out. `progress`
/// works as for [`OcrJob::run`]. A file that the skip-files option passes over comes back with
/// [`FileResult::file_skipped`] set and its original bytes; the caller decides what to do with
/// it, but it is never an OCR success.
pub fn recognize_file(
    name: &str,
    bytes: Arc<Vec<u8>>,
    password: Option<&str>,
    settings: OcrSettings,
    recognizers: &Recognizers,
    progress: impl FnMut(usize, usize) -> bool,
) -> Result<FileResult, String> {
    let mut s = Session::new();
    let id = s.open(name, None, bytes.clone(), password).map_err(|e| e.to_string())?;
    if let Some(why) = s.get(id).and_then(|d| d.read_only_reason.clone()) {
        return Err(why);
    }
    let job = s.ocr_job(id, &[], settings.clone()).ok_or("the document could not be read")?;
    // The skip-files probe runs before any engine work: a file with text needs no engine at
    // all, and the original bytes come back unchanged.
    if settings.skip_text_files && !settings.force_ocr && job.has_text() {
        let pages = job
            .pages
            .iter()
            .map(|&page| OcrPage {
                already_on_page: 0,
                page,
                words: Vec::new(),
                skipped: Some("the file already contains text".into()),
                notes: Vec::new(),
            })
            .collect();
        return Ok(FileResult { bytes, pages, file_skipped: Some("the file already contains text".into()) });
    }
    let pages = job.run(recognizers, progress);
    if s.apply_ocr(id, &pages).map_err(|e| e.to_string())? == 0 {
        return Ok(FileResult { bytes, pages, file_skipped: None });
    }
    // Checked like a save before the caller writes it (#2166).
    let bytes = s.save_bytes(id).map_err(|e| e.to_string())?;
    s.check_save(id, &bytes).map_err(|e| e.to_string())?;
    Ok(FileResult { bytes, pages, file_skipped: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export;

    /// #1028: a recognised word over the page's own text is that text again; one in an empty
    /// part of the page (a picture) is new.
    #[test]
    fn words_over_existing_text_count_as_already_there() {
        // Glyph em boxes of "Total" at 12 pt, five letters side by side.
        let glyphs: Vec<[f32; 4]> = (0..5).map(|i| [100.0 + 7.0 * i as f32, 50.0, 107.0 + 7.0 * i as f32, 62.0]).collect();
        // OCR's tight box around the same ink.
        assert!(covered([100.5, 52.0, 134.0, 61.0], &glyphs) >= ALREADY_ON_PAGE);
        // A word in a picture further down, and one beside the text on the same line.
        assert_eq!(covered([100.0, 200.0, 140.0, 212.0], &glyphs), 0.0);
        assert_eq!(covered([140.0, 52.0, 170.0, 61.0], &glyphs), 0.0);
        // Touching the text's last letter is not enough.
        assert!(covered([131.0, 52.0, 170.0, 61.0], &glyphs) < ALREADY_ON_PAGE);
        // A page without text has nothing to cover a word.
        assert_eq!(covered([100.5, 52.0, 134.0, 61.0], &[]), 0.0);
    }

    #[test]
    fn coverage_of_overlapping_or_broken_boxes_stays_between_zero_and_one() {
        // A glyph drawn twice (fake bold) does not cover more than the whole word.
        let twice = [[0.0, 0.0, 10.0, 10.0], [0.0, 0.0, 10.0, 10.0]];
        assert_eq!(covered([0.0, 0.0, 10.0, 10.0], &twice), 1.0);
        // Empty, inverted and non-finite boxes cover nothing and are covered by nothing.
        for word in [[5.0, 5.0, 5.0, 9.0], [9.0, 0.0, 1.0, 9.0], [f32::NAN, 0.0, 9.0, 9.0], [0.0, 0.0, f32::INFINITY, 9.0]] {
            assert_eq!(covered(word, &twice), 0.0, "{word:?}");
        }
        let broken = [[f32::NAN; 4], [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY]];
        let c = covered([0.0, 0.0, 10.0, 10.0], &broken);
        assert!((0.0..=1.0).contains(&c), "{c}");
    }
    use crate::tests::fixture;
    use pdfcraft_render::TextGlyph;

    fn glyph(text: &str) -> TextGlyph {
        TextGlyph { text: text.into(), rect: [0.0, 0.0, 1.0, 1.0], quad: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], direction: [1.0, 0.0] }
    }

    /// The skip-text probe: at least one non-blank run; blank-only or missing text never
    /// enables a skip.
    #[test]
    fn has_text_probe_needs_one_non_blank_run() {
        let none: Option<Arc<pdfcraft_render::PageText>> = None;
        assert!(!page_has_text(none.as_ref()), "a failed probe never enables a skip");
        assert!(!page_has_text(Some(Arc::new(pdfcraft_render::PageText::default())).as_ref()));
        let blank = pdfcraft_render::PageText { glyphs: vec![glyph(" "), glyph("")], ..Default::default() };
        assert!(!page_has_text(Some(Arc::new(blank)).as_ref()));
        let has = pdfcraft_render::PageText { glyphs: vec![glyph(" "), glyph("a")], ..Default::default() };
        assert!(page_has_text(Some(Arc::new(has)).as_ref()));
    }

    #[test]
    fn output_mode_parsing_falls_back_to_searchable() {
        assert_eq!(OutputMode::parse("editable"), OutputMode::EditableText);
        assert_eq!(OutputMode::parse(" Editable-Text "), OutputMode::EditableText);
        assert_eq!(OutputMode::parse("searchable"), OutputMode::Searchable);
        assert_eq!(OutputMode::parse("nonsense"), OutputMode::Searchable, "unknown strings mean Searchable");
        assert_eq!(OutputMode::parse(""), OutputMode::Searchable);
    }

    #[test]
    fn engine_choice_parsing() {
        assert_eq!(EngineChoice::parse("tesseract"), EngineChoice::Tesseract);
        assert_eq!(EngineChoice::parse(" AUTO "), EngineChoice::Auto);
        assert_eq!(EngineChoice::parse("ocrs"), EngineChoice::Ocrs);
        assert_eq!(EngineChoice::parse("wat"), EngineChoice::Ocrs, "unknown names fall back to the default");
        assert_eq!(EngineChoice::default().name(), "ocrs");
    }

    /// Automatic reads with the second engine when the first is unsure unless a strategy was
    /// chosen: "Primary only" would never run it. A named engine reads alone.
    #[test]
    fn automatic_defaults_to_the_confidence_weighted_strategy() {
        assert_eq!(EngineChoice::Auto.default_strategy(), MergeStrategy::ConfidenceWeighted);
        assert_eq!(EngineChoice::Ocrs.default_strategy(), MergeStrategy::PrimaryOnly);
        assert_eq!(EngineChoice::Tesseract.default_strategy(), MergeStrategy::PrimaryOnly);
    }

    /// Settings default to today's behavior: ocrs, PrimaryOnly, no preprocessing, searchable.
    #[test]
    fn settings_default_to_the_ocrs_status_quo() {
        let s = OcrSettings::default();
        assert_eq!(s.engine, EngineChoice::Ocrs);
        assert_eq!(s.strategy, MergeStrategy::PrimaryOnly);
        assert_eq!(s.output_mode, OutputMode::Searchable);
        assert!(!s.auto_rotate && !s.deskew && !s.denoise && !s.binarize);
        assert_eq!(s.region, None);
        assert_eq!(s.low_confidence_threshold, 60.0);
        assert_eq!(s.tesseract_path, None, "no configured tesseract path by default: the PATH decides");
    }

    /// A configured tesseract path that is not an absolute, existing, executable file makes
    /// the Tesseract choice unavailable — the explicit setting is never silently replaced by
    /// a PATH search (and this holds whatever this machine has installed).
    #[test]
    fn an_invalid_configured_tesseract_path_is_unavailable_not_a_fallback() {
        let bogus = OcrSettings { engine: EngineChoice::Tesseract, tesseract_path: Some("not/a/program".into()), ..Default::default() };
        match recognizers(&bogus) {
            Ok(r) => panic!("a bogus configured path must not resolve: {:?}", r.primary.id()),
            Err(e) => assert!(e.contains("tesseract"), "{e}"),
        }
    }

    /// A failed engine probe is remembered only for the negative TTL — a program installed
    /// while the app runs is found by a later job, without a restart — while a found engine is
    /// remembered for the session.
    #[test]
    fn a_negative_probe_expires_a_found_engine_does_not() {
        let now = Instant::now();
        let old = now.checked_sub(Duration::from_secs(31)).unwrap();
        assert!(!probe_fresh(false, old, now), "past the TTL the probe is retried");
        assert!(probe_fresh(false, now - Duration::from_secs(5), now), "a fresh failure is not re-probed");
        assert!(probe_fresh(true, old, now), "a found engine is kept");
    }

    /// `ocr_status` and a job judge the same location: one named by the job's settings is the
    /// one reported (here an invalid one, so no process starts and the answer is the same on
    /// every machine).
    #[test]
    fn the_status_follows_the_location_a_job_names() {
        let bogus = OcrSettings { tesseract_path: Some("not/a/program".into()), ..Default::default() };
        let tesseract = engines_status_for(&bogus).into_iter().find(|e| e.id == "tesseract").expect("tesseract is always listed");
        assert!(!tesseract.available, "{tesseract:?}");
        assert!(tesseract.why_not.as_deref().is_some_and(|why| why.contains("not/a/program")), "{tesseract:?}");
        assert_eq!(tesseract.program.as_deref(), Some("not/a/program"));
    }

    /// What the interface sees of the cache: a fresh answer as is, an expired failed probe as
    /// the best known answer (shown while it is retried, so the choice does not flicker), and
    /// nothing for a location nobody asked about.
    #[test]
    fn the_cache_lookup_keeps_a_stale_answer_and_knows_its_location() {
        let now = Instant::now();
        let old = now.checked_sub(Duration::from_secs(31)).unwrap();
        let failed = |key: Option<&str>, at| CachedTesseract { key: key.map(str::to_string), found: None, at };
        assert!(matches!(cache_lookup(None, None, now), Cached::Missing));
        assert!(matches!(cache_lookup(Some(&failed(None, now)), None, now), Cached::Fresh(None)));
        assert!(matches!(cache_lookup(Some(&failed(None, old)), None, now), Cached::Stale(None)));
        assert!(matches!(cache_lookup(Some(&failed(Some("/x"), now)), None, now), Cached::Missing), "another location is another question");
        let found = CachedTesseract { key: None, found: Some(Arc::new(Fake::new())), at: old };
        assert!(matches!(cache_lookup(Some(&found), None, now), Cached::Fresh(Some(_))), "a found engine stays fresh");
    }

    /// Engine discovery never deadlocks or corrupts the cache: concurrent lookups with
    /// different configured locations (all invalid, so they are refused before any process
    /// starts) all finish.
    #[test]
    fn concurrent_engine_lookups_all_finish() {
        let handles: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    let configured = if i % 2 == 0 { Some(format!("not/a/program-{i}")) } else { None };
                    let _ = tesseract_engine(configured.as_deref());
                })
            })
            .collect();
        for h in handles {
            h.join().expect("no panic, no deadlock");
        }
    }

    /// The preprocessing flags are not just plumbing: a page read with all four on still
    /// yields the same words, in the same places (needs the models).
    #[test]
    fn preprocessing_flags_flow_through_the_job() {
        if !available() {
            eprintln!("skipped: no OCR engine available");
            return;
        }
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let text = s.create_from_text("t", "The quick brown fox jumps over the lazy dog.").unwrap();
        let id = s.open("text.pdf", None, text, None).unwrap();
        let png = crate::export::Exporter::new(s.get(id).unwrap()).png(0, 150.0).unwrap();
        let scan = s.create_from_images(&[("scan.png".into(), png)]).unwrap();
        let id = s.open("scan.pdf", None, scan, None).unwrap();
        let settings = OcrSettings { auto_rotate: true, deskew: true, denoise: true, binarize: true, ..Default::default() };
        let found = s.recognize_text(id, &[], settings).unwrap();
        let text = crate::tests::page_texts(&s, id)[0].to_lowercase();
        // Binarizing an anti-aliased render thins the thin strokes, so edge letters drop
        // ("lazy" reads as "azy", "brown" as "rown"); the match rests on what stays stable —
        // the middle of "quick" (some CPUs' kernels read the model's "q" as an "a", the same
        // "uick" the OCR test in crate::tests pins) and whole short words.
        for w in ["uick", "fox", "over"] {
            assert!(text.contains(w), "{text}");
        }
        // The words were placed, meaning boxes survived the trip back to raster coordinates.
        assert!(!found[0].words.is_empty());
    }

    /// suspects(): only Low-band words, by index.
    #[test]
    fn page_suspects_are_the_low_band_words() {
        let word = |t: &str, c: Option<f32>| PlacedWord {
            text: t.into(),
            origin: [0.0; 2],
            across: [1.0, 0.0],
            up: [0.0, 1.0],
            confidence: c,
            source: "ocrs".into(),
        };
        let page = OcrPage {
            already_on_page: 0,
            page: 0,
            words: vec![word("a", Some(95.0)), word("b", Some(50.0)), word("c", None), word("d", Some(69.0))],
            skipped: None,
            notes: Vec::new(),
        };
        assert_eq!(page.suspects(), vec![1, 3]);
        let mean = page.mean_confidence().unwrap_or_default();
        assert!((mean - 71.333).abs() < 0.01, "{mean}");
    }

    /// Region clamping: interior kept, negative extents snapped outward, garbage refused.
    #[test]
    fn regions_clamp_and_refuse_garbage() {
        assert_eq!(clamp_region([10.0, 10.0, 20.0, 20.0], 100, 100), Some([10, 10, 20, 20]));
        assert_eq!(clamp_region([-5.0, -5.5, 20.4, 20.0], 100, 100), Some([0, 0, 21, 20]), "snapped outward, clamped to the image");
        assert_eq!(clamp_region([50.0, 50.0, 200.0, 200.0], 100, 100), Some([50, 50, 100, 100]));
        assert_eq!(clamp_region([10.0, 10.0, 10.0, 20.0], 100, 100), None, "empty");
        assert_eq!(clamp_region([20.0, 10.0, 10.0, 20.0], 100, 100), None, "inverted");
        assert_eq!(clamp_region([f32::NAN, 0.0, 1.0, 1.0], 100, 100), None, "NaN");
        assert_eq!(clamp_region([-20.0, -20.0, -5.0, -5.0], 100, 100), None, "wholly outside");
    }

    /// A word box only reaches the content stream with positive, finite geometry: NaN,
    /// infinite, negative and zero-sized boxes are dropped before placement, so nothing
    /// non-finite is ever stamped onto the page.
    #[test]
    fn non_finite_and_negative_word_boxes_are_dropped_before_placement() {
        let word = |rect: [f32; 4]| pdfcraft_ocr::Word::new("w", rect, "ocrs");
        for (why, rect) in [
            ("NaN width", [f32::NAN, 0.0, 10.0, 10.0]),
            ("NaN height", [0.0, 0.0, 10.0, f32::NAN]),
            ("infinite right", [0.0, 0.0, f32::INFINITY, 10.0]),
            ("1e39 height overflows to inf", [0.0, 0.0, 10.0, 1e39f64 as f32]),
            ("negative width", [10.0, 0.0, 0.0, 10.0]),
            ("negative height", [0.0, 10.0, 10.0, 0.0]),
            ("zero size", [5.0, 5.0, 5.0, 5.0]),
        ] {
            assert!(!box_writable(&word(rect)), "{why}: {rect:?}");
        }
        assert!(box_writable(&word([0.0, 0.0, 10.0, 10.0])), "a sane box is placed");
    }

    /// A crop takes exactly the asked pixels (this feeds words back to page space, so edges
    /// matter).
    #[test]
    fn crops_take_the_asked_pixels() {
        let img = pdfcraft_ocr::OcrImage::new(3, 2, (0..6).flat_map(|p| vec![p as u8; 4]).collect()).unwrap();
        let c = crop(&img, 1, 0, 3, 2).unwrap();
        assert_eq!((c.width(), c.height()), (2, 2));
        assert_eq!(c.rgba()[0], 1, "the crop starts at x=1");
        assert_eq!(c.rgba()[4], 2, "the first row's second column is x=2");
        assert_eq!(c.rgba()[8], 4, "the second row starts at y=1");
    }

    /// The stale guard: edits and closing are named and the result is refused; a page-count or
    /// revision difference is named too (any edit rewrites the working bytes, so those branches
    /// are reached with a hand-built guard).
    #[test]
    fn stale_results_are_named_and_refused() {
        let mut s = Session::new();
        let id = s.open("t.pdf", None, Arc::new(fixture(2)), None).unwrap();
        let job = s.ocr_job(id, &[], OcrSettings::default()).unwrap();
        assert_eq!(s.stale_reason(&job.guard), None, "freshly captured");
        // Any edit rewrites the working bytes.
        s.apply(id, Edit::AddOcrText { page: 0, words: vec![] }).unwrap();
        assert_eq!(s.stale_reason(&job.guard).as_deref(), Some("the document changed since recognition started"));
        assert!(s.apply_ocr_checked(id, &[], &job.guard).is_err(), "a stale result is never applied");
        // A fresh guard passes.
        let fresh = s.ocr_job(id, &[], OcrSettings::default()).unwrap();
        assert_eq!(s.stale_reason(&fresh.guard), None);
        // A page count and a revision count from another life are named too.
        let doc = s.get(id).unwrap();
        let (bytes, revisions) = (doc.bytes.clone(), doc.revision_ends().len());
        let guard = OcrGuard { doc: id, pages: 5, revisions, bytes };
        assert!(s.stale_reason(&guard).is_some_and(|r| r.contains("page count changed")), "{}", s.stale_reason(&guard).unwrap());
        let guard = OcrGuard { doc: id, pages: 2, revisions: revisions + 3, bytes: s.get(id).unwrap().bytes.clone() };
        assert!(s.stale_reason(&guard).is_some_and(|r| r.contains("revisions")), "{}", s.stale_reason(&guard).unwrap());
        // A guard for a different document refuses up front.
        let other = s.open("other.pdf", None, Arc::new(fixture(1)), None).unwrap();
        let cross = s.apply_ocr_checked(other, &[], &fresh.guard);
        assert!(cross.as_ref().is_err_and(|e| e.contains("another document")), "{cross:?}");
        // And so is a closed document.
        s.close(id);
        assert_eq!(s.stale_reason(&fresh.guard).as_deref(), Some("the document is no longer open"));
    }

    /// The stale guard, exercised end to end through the recognition flow itself (the flow the
    /// UI's Verify screen and the automation's two-phase path drive): capture a job on a
    /// synthetic scanned page, run the recognition, edit the document, and the captured result
    /// is refused with its named reason and nothing written; a fresh recognition over the
    /// edited document applies.
    #[test]
    fn stale_results_are_refused_end_to_end_through_the_recognition_flow() {
        if !available() {
            eprintln!("skipped: no OCR engine available");
            return;
        }
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let text = s.create_from_text("t", "The quick brown fox jumps over the lazy dog.").unwrap();
        let id = s.open("text.pdf", None, text, None).unwrap();
        let png = crate::export::Exporter::new(s.get(id).unwrap()).png(0, 150.0).unwrap();
        let scan = s.create_from_images(&[("scan.png".into(), png)]).unwrap();
        let id = s.open("scan.pdf", None, scan, None).unwrap();

        // Recognize: the job (carrying its guard) is captured first, then the reading runs.
        let job = s.ocr_job(id, &[], OcrSettings::default()).unwrap();
        let guard = job.guard.clone();
        let r = recognizers(&job.settings).unwrap();
        let found = job.run(&r, |_, _| true);
        assert!(!found[0].words.is_empty(), "the scan was read: {:?}", found[0].skipped);

        // The document is edited while the result sits around; any edit rewrites the bytes.
        s.apply(id, Edit::AddOcrText { page: 0, words: vec![] }).unwrap();

        // Applying the now-stale result is refused with its named reason and writes nothing.
        let after_edit = s.get(id).unwrap().bytes.clone();
        assert!(!Arc::ptr_eq(&after_edit, &guard.bytes), "the edit rewrote the working bytes");
        let revisions = s.get(id).unwrap().revision_ends().len();
        let why = s.apply_ocr_checked(id, &found, &guard).unwrap_err();
        assert_eq!(why, "the document changed since recognition started", "{why}");
        assert_eq!(s.get(id).unwrap().revision_ends().len(), revisions, "the refusal wrote no revision");
        assert!(Arc::ptr_eq(&s.get(id).unwrap().bytes, &after_edit), "the refused apply wrote nothing");
        assert!(crate::tests::page_texts(&s, id)[0].trim().is_empty(), "no text layer landed");

        // A fresh recognition over the current document applies: same flow, valid now.
        let job = s.ocr_job(id, &[], OcrSettings::default()).unwrap();
        let guard = job.guard.clone();
        assert_eq!(s.stale_reason(&guard), None, "a fresh job is not stale");
        let found = job.run(&r, |_, _| true);
        let words = s.apply_ocr_checked(id, &found, &guard).unwrap();
        assert!(words > 0, "the re-run found the words again");
        let text = crate::tests::page_texts(&s, id)[0].to_lowercase();
        assert!(text.contains("fox"), "{text}");
    }

    /// The selector refuses an unusable engine with its cause named, and resolves the ensemble
    /// when both engines exist (here: whatever this machine has for ocrs and tesseract).
    #[test]
    fn selector_names_the_missing_engine() {
        let tesseract = OcrSettings { engine: EngineChoice::Tesseract, ..Default::default() };
        match recognizers(&tesseract) {
            Ok(r) => assert_eq!(r.primary.id(), "tesseract"),
            Err(e) => assert!(e.contains("tesseract"), "{e}"),
        }
        let ocrs = OcrSettings { engine: EngineChoice::Ocrs, ..Default::default() };
        match recognizers(&ocrs) {
            Ok(r) => assert_eq!(r.primary.id(), "ocrs"),
            Err(e) => assert!(e.contains("models"), "{e}"),
        }
    }

    /// An ocrs-only choice never starts the Tesseract probe: the probe is a `--list-langs`
    /// process, and a job that names its engine needs no reading from Tesseract. The test's
    /// fake program touches a marker file when it runs, so the probe's start is observable;
    /// it needs a Unix shell, like the other process fixtures.
    #[cfg(unix)]
    #[test]
    fn an_ocrs_only_choice_never_probes_tesseract() {
        use std::os::unix::fs::PermissionsExt;
        let probe = |name: &str| {
            let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-probe-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let program = dir.join("fake-tesseract");
            std::fs::write(&program, format!("#!/bin/sh\ntouch \"$0.probed\"\nexit 1\n")).unwrap();
            let mut perms = std::fs::metadata(&program).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&program, perms).unwrap();
            (dir.join("fake-tesseract.probed"), program.to_string_lossy().into_owned())
        };
        // An ocrs-only job: the configured location stays untouched.
        let (marker, program) = probe("ocrs-only");
        let ocrs_only = OcrSettings { engine: EngineChoice::Ocrs, tesseract_path: Some(program), ..Default::default() };
        let _ = recognizers(&ocrs_only);
        assert!(!marker.exists(), "an ocrs-only job must not start the tesseract probe");
        std::fs::remove_file(&marker.parent().unwrap().join("fake-tesseract")).ok();

        // A Tesseract job does validate its program (that is how it is found usable).
        let (marker, program) = probe("tesseract");
        let wants = OcrSettings { engine: EngineChoice::Tesseract, tesseract_path: Some(program), ..Default::default() };
        let _ = recognizers(&wants);
        assert!(marker.exists(), "a tesseract job probes its program");
    }

    /// A fake engine for the batch skip logic: reads one confident word from anything, counts
    /// its calls, and never touches the filesystem.
    struct Fake {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl Fake {
        fn new() -> Fake {
            Fake { calls: std::sync::atomic::AtomicUsize::new(0) }
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Recognizer for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn available(&self) -> bool {
            true
        }
        fn languages(&self) -> Vec<(String, String)> {
            vec![("en".into(), "English".into())]
        }
        fn recognize(&self, _image: &pdfcraft_ocr::OcrImage, _options: &pdfcraft_ocr::RecognizeOptions) -> Result<Vec<pdfcraft_ocr::Line>, OcrError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![pdfcraft_ocr::Line { words: vec![pdfcraft_ocr::Word::new("word", [1.0, 1.0, 9.0, 9.0], "fake").with_confidence(95.0)] }])
        }
    }

    fn fake_recognizers() -> (Recognizers, Arc<Fake>) {
        let fake = Arc::new(Fake::new());
        (Recognizers { primary: fake.clone(), secondary: None }, fake)
    }

    /// Skip-files: a file that already has text is passed over entirely — the engine never
    /// runs, the original bytes come back, and the result is its own skipped bucket.
    #[test]
    fn skip_files_passes_text_files_over_without_running_the_engine() {
        let s = Session::new();
        let bytes = s.create_from_text("t", "Already typed here").unwrap();
        let (r, fake) = fake_recognizers();
        let settings = OcrSettings { skip_text_files: true, ..Default::default() };
        let out = recognize_file("t.pdf", bytes.clone(), None, settings, &r, |_, _| true).unwrap();
        assert_eq!(fake.calls(), 0, "the engine never ran");
        assert_eq!(out.bytes.as_ptr(), bytes.as_ptr(), "the original bytes came back");
        assert_eq!(out.file_skipped.as_deref(), Some("the file already contains text"));
        assert!(out.skipped() && out.words() == 0);
    }

    /// Force OCR overrides the file skip: the file is read even though it has text, and the
    /// result is an OCR success (new bytes, words placed), never a skipped bucket.
    #[test]
    fn force_ocr_overrides_the_file_skip() {
        let s = Session::new();
        let bytes = s.create_from_text("t", "Already typed here").unwrap();
        let (r, fake) = fake_recognizers();
        let settings = OcrSettings { skip_text_files: true, force_ocr: true, ..Default::default() };
        let out = recognize_file("t.pdf", bytes, None, settings, &r, |_, _| true).unwrap();
        assert_eq!(fake.calls(), 1);
        assert_eq!(out.file_skipped, None);
        assert_eq!(out.words(), 1);
        assert!(!out.bytes.is_empty());
    }

    /// Force OCR also overrides the per-page skip: a page with text is read anyway.
    #[test]
    fn force_ocr_overrides_the_page_skip() {
        let s = Session::new();
        let bytes = s.create_from_text("t", "A page with text").unwrap();
        let (r, fake) = fake_recognizers();
        let settings = OcrSettings { force_ocr: true, ..Default::default() };
        let out = recognize_file("t.pdf", bytes, None, settings, &r, |_, _| true).unwrap();
        assert_eq!(fake.calls(), 1, "the page's own text did not stop the run");
        assert_eq!(out.pages[0].skipped, None);
    }

    /// The per-page skip stays the default: with force off, a text page is passed over (and a
    /// second run over a searchable output is a no-op — nothing left to read).
    #[test]
    fn page_skip_leaves_text_pages_alone_and_a_rerun_is_a_noop() {
        let s = Session::new();
        let bytes = s.create_from_text("t", "A page with text").unwrap();
        let (r, fake) = fake_recognizers();
        let out = recognize_file("t.pdf", bytes, None, OcrSettings::default(), &r, |_, _| true).unwrap();
        assert_eq!(fake.calls(), 0);
        assert_eq!(out.pages[0].skipped.as_deref(), Some("the page already contains text"));
        assert_eq!(out.words(), 0);
        // Second run over the same (already text-bearing) file: skipped pages again, no calls.
        let again = recognize_file("t.pdf", out.bytes, None, OcrSettings::default(), &r, |_, _| true).unwrap();
        assert_eq!(fake.calls(), 0);
        assert_eq!(again.pages[0].skipped.as_deref(), Some("the page already contains text"));
    }

    /// Two inputs of one name (from different folders, or differing only in case) would
    /// write the same output: both are found, each once.
    #[test]
    fn duplicate_output_names_are_found_case_insensitively() {
        assert!(duplicate_output_names(["a.pdf", "b.pdf"]).is_empty());
        assert_eq!(duplicate_output_names(["a.pdf", "b.pdf", "A.PDF", "b.pdf", "a.pdf"]), ["A.PDF", "b.pdf"]);
    }

    /// The batch note's data: counts words at or under the threshold per page (1-based, sorted;
    /// skipped pages never contribute), and says nothing when there is nothing to review.
    #[test]
    fn low_confidence_pages_counts_the_batch_rule() {
        let word = |c: f32| PlacedWord {
            text: "w".into(),
            origin: [0.0; 2],
            across: [1.0, 0.0],
            up: [0.0, 1.0],
            confidence: Some(c),
            source: "ocrs".into(),
        };
        let page = |n: usize, confs: &[f32], skipped: Option<&str>| OcrPage {
            already_on_page: 0,
            page: n,
            words: confs.iter().map(|c| word(*c)).collect(),
            skipped: skipped.map(str::to_string),
            notes: Vec::new(),
        };
        let pages = vec![
            page(0, &[95.0, 40.0], None),
            page(1, &[60.0, 60.5, 10.0], None),
            page(2, &[59.0], Some("skipped pages never count")),
            page(3, &[0.0], None),
        ];
        let (count, pages_on) = low_confidence_pages(&pages, 60.0).unwrap();
        assert_eq!((count, pages_on), (4, vec![1, 2, 4]), "60 counts, 60.5 does not, the skipped page is left out");
        let all_high = vec![page(0, &[95.0, 90.5], None)];
        assert_eq!(low_confidence_pages(&all_high, 60.0), None, "no low-confidence words, no note");
    }

    /// The editable writer: refuses an empty payload, writes the words as visible text at
    /// their boxes on pages of the source's displayed size, adds no scan image, and never
    /// touches the open document.
    #[test]
    fn editable_writer_refuses_empty_and_writes_visible_words() {
        let mut s = Session::new();
        let id = s.open("src.pdf", None, Arc::new(fixture(2)), None).unwrap();
        assert!(
            s.editable_ocr_bytes(id, &[OcrPage { already_on_page: 0, page: 0, words: vec![], skipped: None, notes: Vec::new() }]).is_err(),
            "zero words are refused"
        );
        let info = s.get(id).unwrap().info.pages[0].clone();
        // A word in the source page's user space, high on the page (the fixture's pages are
        // 200 x 300 pt, so the box stays inside them).
        let w = PlacedWord {
            text: "Editable".into(),
            origin: [20.0, 200.0],
            across: [120.0, 0.0],
            up: [0.0, 12.0],
            confidence: Some(88.0),
            source: "ocrs".into(),
        };
        let found = vec![
            OcrPage { already_on_page: 0, page: 0, words: vec![w], skipped: None, notes: Vec::new() },
            OcrPage { already_on_page: 0, page: 1, words: vec![], skipped: None, notes: Vec::new() },
        ];
        let before = s.get(id).unwrap().bytes.clone();
        let bytes = s.editable_ocr_bytes(id, &found).unwrap();
        // The open document was never touched.
        assert!(Arc::ptr_eq(&s.get(id).unwrap().bytes, &before), "the source bytes are the same allocation");
        let mut out = Session::new();
        let oid = out.open("ocr.pdf", None, bytes, None).unwrap();
        let otext = crate::tests::page_texts(&out, oid)[0].clone().to_lowercase();
        assert!(otext.contains("editable"), "{otext}");
        let odoc = out.get(oid).unwrap();
        assert_eq!(odoc.info.pages.len(), 2, "the blank source page stays a page");
        assert!(crate::tests::page_texts(&out, oid)[1].trim().is_empty(), "the page with no words stays blank");
        assert!(odoc.page_images(0).is_empty(), "no scan image is carried over");
        let (w, h) = (odoc.info.pages[0].width, odoc.info.pages[0].height);
        assert_eq!((w, h), (info.width, info.height), "pages keep the source's displayed size");
    }

    /// The editable writer maps through a rotated source page: a word drawn in user space
    /// reads upright on the new page, and the new page has the source's displayed size.
    #[test]
    fn editable_writer_maps_rotated_pages() {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let text = s.create_from_text("t", "rotated source").unwrap();
        let id = s.open("r.pdf", None, text, None).unwrap();
        s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        let info = s.get(id).unwrap().info.pages[0].clone();
        assert_eq!(info.rotation, 90);
        // The word sits at view (20, 48)..(120, 60) on the displayed page; its box comes in
        // user space, exactly as recognition left it.
        let (o, br, tl) = (info.view_to_user(20.0, 60.0), info.view_to_user(120.0, 60.0), info.view_to_user(20.0, 48.0));
        let w = PlacedWord {
            text: "Word".into(),
            origin: [f64::from(o[0]), f64::from(o[1])],
            across: [f64::from(br[0] - o[0]), f64::from(br[1] - o[1])],
            up: [f64::from(tl[0] - o[0]), f64::from(tl[1] - o[1])],
            confidence: None,
            source: "ocrs".into(),
        };
        let bytes = s.editable_ocr_bytes(id, &[OcrPage { already_on_page: 0, page: 0, words: vec![w], skipped: None, notes: Vec::new() }]).unwrap();
        let mut out = Session::new();
        let oid = out.open("ocr.pdf", None, bytes, None).unwrap();
        let otext = crate::tests::page_texts(&out, oid)[0].clone();
        assert!(otext.contains("Word"), "{otext}");
        // The new page is the displayed size (portrait, not the rotated-away landscape).
        let oinfo = out.get(oid).unwrap().info.pages[0].clone();
        assert_eq!((oinfo.width, oinfo.height), (info.width, info.height));
    }

    /// The searchable layer round-trips the recognized words through the engine's own
    /// extraction, characters beyond WinAnsi included, without needing any display fonts.
    #[test]
    fn searchable_layer_round_trips_non_win_ansi_words() {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let id = s.open("src.pdf", None, Arc::new(fixture(1)), None).unwrap();
        let word = PlacedWord {
            text: "日本語".into(),
            origin: [20.0, 250.0],
            across: [80.0, 0.0],
            up: [0.0, 10.0],
            confidence: None,
            source: "ocrs".into(),
        };
        s.apply(id, Edit::AddOcrText { page: 0, words: vec![word] }).unwrap();
        let text = crate::tests::page_texts(&s, id)[0].clone();
        assert!(text.contains("日本語"), "{text}");
    }

    /// The fontless searchable layer retains its original pixels, handles multiple code
    /// fonts and supplementary Unicode, and does not replace existing resource names.
    #[test]
    fn searchable_unicode_preserves_pixels_save_undo_and_resources() {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let id = s.open("src.pdf", None, Arc::new(fixture(1)), None).unwrap();
        let before = crate::export::Exporter::new(s.get(id).unwrap()).png(0, 72.0).unwrap();
        let mut lines = vec!["中文 English 2026".to_string()];
        let chars: Vec<char> = (0x4e00..0x4f10).filter_map(char::from_u32).chain(['𠮷']).collect();
        lines.extend(chars.chunks(90).map(|c| c.iter().collect()));
        let text = lines.join("\n");
        let words: Vec<_> = lines
            .iter()
            .enumerate()
            .map(|(i, line)| PlacedWord {
                text: line.clone(),
                origin: [20.0, 80.0 - 10.0 * i as f64],
                across: [160.0, 0.0],
                up: [0.0, 5.0],
                confidence: None,
                source: "test".into(),
            })
            .collect();
        s.apply(id, Edit::AddOcrText { page: 0, words: words.clone() }).unwrap();
        assert!(crate::tests::page_texts(&s, id)[0].contains(&text));
        assert_eq!(crate::export::Exporter::new(s.get(id).unwrap()).png(0, 72.0).unwrap(), before);
        let saved = s.save_bytes(id).unwrap();
        let reopened = s.open("searchable.pdf", None, saved, None).unwrap();
        assert!(crate::tests::page_texts(&s, reopened)[0].contains(&text));
        assert_eq!(crate::export::Exporter::new(s.get(reopened).unwrap()).png(0, 72.0).unwrap(), before);
        s.undo(id).unwrap();
        assert!(!crate::tests::page_texts(&s, id)[0].contains(&text));
        s.redo(id).unwrap();
        let mut second = words[0].clone();
        second.text = "另一行".into();
        second.origin[1] = 100.0;
        s.apply(id, Edit::AddOcrText { page: 0, words: vec![second] }).unwrap();
        let extracted = crate::tests::page_texts(&s, id)[0].clone();
        assert!(extracted.contains(&text) && extracted.contains("另一行"), "{extracted}");
    }

    #[test]
    fn searchable_unicode_rejects_excessive_or_non_finite_input_atomically() {
        let mut s = Session::new();
        let id = s.open("src.pdf", None, Arc::new(fixture(1)), None).unwrap();
        let word =
            PlacedWord { text: "日".into(), origin: [20.0, 80.0], across: [30.0, 0.0], up: [0.0, 5.0], confidence: None, source: "test".into() };
        let before = crate::tests::page_texts(&s, id);
        let mut non_finite = word.clone();
        non_finite.origin[0] = f64::NAN;
        let mut overflow = word.clone();
        overflow.origin[0] = f64::MAX;
        overflow.across[0] = f64::MAX;
        let mut too_long = word.clone();
        too_long.text = "a".repeat(4 * 1024 * 1024 + 1);
        for words in [vec![non_finite], vec![overflow], vec![too_long], vec![word; 65537]] {
            assert!(s.apply(id, Edit::AddOcrText { page: 0, words }).is_err());
            assert_eq!(crate::tests::page_texts(&s, id), before);
        }
    }

    #[test]
    fn editable_export_refuses_invalid_or_excessive_aggregate_input_without_changing_source() {
        let mut s = Session::new();
        let id = s.open("src.pdf", None, Arc::new(fixture(2)), None).unwrap();
        let before = s.save_bytes(id).unwrap();
        let word =
            PlacedWord { text: "word".into(), origin: [20.0, 80.0], across: [30.0, 0.0], up: [0.0, 5.0], confidence: None, source: "test".into() };
        let page = |page, words| OcrPage { already_on_page: 0, page, words, skipped: None, notes: Vec::new() };
        let mut invalid = word.clone();
        invalid.origin[0] = f64::INFINITY;
        let mut long = word.clone();
        long.text = "a".repeat(2 * 1024 * 1024 + 1);
        for found in [
            vec![page(0, vec![invalid])],
            vec![page(0, vec![long.clone()]), page(1, vec![long])],
            vec![page(0, vec![word.clone(); 32768]), page(1, vec![word; 32769])],
        ] {
            assert!(s.editable_ocr_bytes(id, &found).is_err());
            assert_eq!(s.save_bytes(id).unwrap(), before);
        }
    }

    /// The editable output carries non-WinAnsi words as visible text that extraction reads
    /// back, in a face beyond Helvetica when the build has one.
    #[test]
    fn editable_output_round_trips_non_win_ansi_words() {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let id = s.open("src.pdf", None, Arc::new(fixture(1)), None).unwrap();
        let w = PlacedWord {
            text: "Привет".into(),
            origin: [20.0, 250.0],
            across: [80.0, 0.0],
            up: [0.0, 10.0],
            confidence: None,
            source: "ocrs".into(),
        };
        let bytes = s.editable_ocr_bytes(id, &[OcrPage { already_on_page: 0, page: 0, words: vec![w], skipped: None, notes: Vec::new() }]).unwrap();
        let mut out = Session::new();
        let oid = out.open("ocr.pdf", None, bytes, None).unwrap();
        let text = crate::tests::page_texts(&out, oid)[0].clone();
        if pdfcraft_fonts::CRAFT_FONTS.is_empty() {
            assert!(text.contains("??????"), "{text}");
        } else {
            assert!(text.contains("Привет"), "{text}");
        }
    }

    /// The editable output's VISIBLE layer — a Type3 run for the Cyrillic word, Helvetica for
    /// the Latin one — is redacted end to end: a mark over the Cyrillic word removes exactly
    /// it (the renderer's extractor no longer reads it, before or after a save and reopen), and
    /// the Latin word elsewhere on the page stays.
    #[test]
    fn the_editable_type3_layer_is_redacted_end_to_end() {
        if pdfcraft_fonts::CRAFT_FONTS.is_empty() {
            eprintln!("skipping: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
            return;
        }
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let id = s.open("src.pdf", None, Arc::new(fixture(1)), None).unwrap();
        let word = |text: &str, y: f64, w: f64| PlacedWord {
            text: text.into(),
            origin: [20.0, y],
            across: [w, 0.0],
            up: [0.0, 10.0],
            confidence: None,
            source: "ocrs".into(),
        };
        let words = vec![word("Привет", 250.0, 80.0), word("Hello", 100.0, 50.0)];
        let bytes = s.editable_ocr_bytes(id, &[OcrPage { already_on_page: 0, page: 0, words, skipped: None, notes: Vec::new() }]).unwrap();
        let mut out = Session::new().with_clock(|| 1_700_000_000);
        let oid = out.open("ocr.pdf", None, bytes, None).unwrap();
        let before = crate::tests::page_texts(&out, oid)[0].clone();
        assert!(before.contains("Привет") && before.contains("Hello"), "{before}");
        let shape = crate::Shape::Redact {
            quads: vec![pdfcraft_annot::rect_quad([10.0, 240.0, 110.0, 270.0])],
            overlay: String::new(),
            look: Default::default(),
        };
        let mark = Edit::AddAnnotation(crate::NewAnnotation {
            page: 0,
            style: crate::Style::default_for(&shape),
            shape,
            contents: String::new(),
            author: "Ada".into(),
        });
        out.apply(oid, mark).unwrap();
        out.apply(oid, Edit::ApplyRedactions { pages: None }).unwrap();
        let after = crate::tests::page_texts(&out, oid)[0].clone();
        assert!(!after.contains("Привет") && !after.contains("Пр"), "the Type3 word is gone: {after}");
        assert!(after.contains("Hello"), "the word outside the mark stays: {after}");
        // Reopened from the saved bytes, an independent read finds the same.
        let saved = out.save_bytes(oid).unwrap();
        let mut again = Session::new();
        let aid = again.open("r.pdf", None, saved, None).unwrap();
        let reread = crate::tests::page_texts(&again, aid)[0].clone();
        assert!(!reread.contains("Привет") && reread.contains("Hello"), "{reread}");
    }

    /// A shared document plan serves a multi-page editable export: each page's words must be
    /// written with their OWN encodings (the plan's offset for that page), so page 2 never
    /// shows page 1's text.
    #[test]
    fn editable_output_keeps_each_pages_own_words() {
        if pdfcraft_fonts::CRAFT_FONTS.is_empty() {
            eprintln!("skipping: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
            return;
        }
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let id = s.open("src.pdf", None, Arc::new(fixture(2)), None).unwrap();
        let word = |text: &str| PlacedWord {
            text: text.into(),
            origin: [20.0, 250.0],
            across: [80.0, 0.0],
            up: [0.0, 10.0],
            confidence: None,
            source: "ocrs".into(),
        };
        // Different scripts, so each page's words live in different planned fonts (and page 2
        // would inherit page 1's codes under page-local indexing into the shared plan).
        let found = vec![
            OcrPage { already_on_page: 0, page: 0, words: vec![word("日本語")], skipped: None, notes: Vec::new() },
            OcrPage { already_on_page: 0, page: 1, words: vec![word("Привет")], skipped: None, notes: Vec::new() },
        ];
        let bytes = s.editable_ocr_bytes(id, &found).unwrap();
        let mut out = Session::new();
        let oid = out.open("ocr.pdf", None, bytes, None).unwrap();
        let texts = crate::tests::page_texts(&out, oid);
        assert!(texts[0].contains("日本語") && !texts[0].contains("Привет"), "{texts:?}");
        assert!(texts[1].contains("Привет") && !texts[1].contains("日本語"), "{texts:?}");
    }

    /// Redacting a recognized page takes the Unicode searchable layer away for good: the
    /// apply's own verification passes over the Type3 font and an independent extractor finds
    /// nothing, including in builds with no display fonts.
    #[test]
    fn redaction_removes_the_unicode_searchable_layer() {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let id = s.open("src.pdf", None, Arc::new(fixture(1)), None).unwrap();
        let word = PlacedWord {
            text: "日本語のテキスト".into(),
            origin: [20.0, 250.0],
            across: [100.0, 0.0],
            up: [0.0, 10.0],
            confidence: None,
            source: "ocrs".into(),
        };
        s.apply(id, Edit::AddOcrText { page: 0, words: vec![word] }).unwrap();
        assert!(crate::tests::page_texts(&s, id)[0].contains("日本語"));
        let shape = pdfcraft_annot::Shape::Redact {
            quads: vec![pdfcraft_annot::rect_quad([18.0, 248.0, 122.0, 262.0])],
            overlay: String::new(),
            look: Default::default(),
        };
        let mark = Edit::AddAnnotation(pdfcraft_annot::NewAnnotation {
            page: 0,
            style: pdfcraft_annot::Style::default_for(&shape),
            shape,
            contents: String::new(),
            author: "Ada".into(),
        });
        s.apply(id, mark).unwrap();
        s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
        assert!(!crate::tests::page_texts(&s, id)[0].contains("日本語"), "the layer is gone, not just hidden");
    }

    /// The job's render scale is what the run actually renders at: the chosen dpi, or the
    /// page image's own resolution when that is lower.
    #[test]
    fn render_scale_matches_the_job_resolution() {
        let mut s = Session::new();
        let text = s.create_from_text("t", "scale probe").unwrap();
        let id = s.open("t.pdf", None, text, None).unwrap();
        let job = s.ocr_job(id, &[0], OcrSettings { dpi: 144.0, ..Default::default() }).unwrap();
        assert!((job.render_scale(0) - 2.0).abs() < 1e-4, "144 dpi = 2 px/pt, got {}", job.render_scale(0));
        let job = s.ocr_job(id, &[0], OcrSettings { dpi: 600.0, ..Default::default() }).unwrap();
        assert!((job.render_scale(0) - 600.0 / 72.0).abs() < 1e-4, "a text page has no image, so the wanted dpi wins");
        let page = job.pages[0];
        assert_eq!(page, 0);
    }

    /// #1848: an A4 scan at 300 dpi whose page box is its pixel size (2480 x 3508 pt, one pixel
    /// per point) gets a smaller bitmap for each lower "Downsample to" choice, and a low-resolution
    /// letter page is still never shrunk below one pixel per point.
    #[test]
    fn downsample_shrinks_a_page_box_given_in_pixels() {
        let mut s = Session::new();
        let page = |w: u32, h: u32| export::encode_png(w, h, &vec![255; (w * h * 4) as usize]).unwrap();
        let bytes = s.create_from_images(&[("a4.png".into(), page(2480, 3508)), ("letter.png".into(), page(612, 792))]).unwrap();
        let id = s.open("scan.pdf", None, bytes, None).unwrap();
        let mut r = PageRenderer::new(s.get(id).unwrap().bytes.clone(), RenderConfig::default());
        for (dpi, a4) in [(72.0, [595, 842]), (150.0, [1240, 1754]), (300.0, [2480, 3508]), (600.0, [2480, 3508])] {
            let job = s.ocr_job(id, &[], OcrSettings { dpi, ..Default::default() }).unwrap();
            for (page, want) in [(0, a4), (1, [612, 792])] {
                let scale = job.render_scale(page);
                let shot = r.render(RenderRequest { page, scale, ..Default::default() });
                assert!(shot.error.is_none(), "{:?}", shot.error);
                let near = |got: u32, want: u32| got.abs_diff(want) <= 1;
                assert!(near(shot.width, want[0]) && near(shot.height, want[1]), "{dpi} dpi, page {page}: {}x{}", shot.width, shot.height);
            }
        }
    }
}
