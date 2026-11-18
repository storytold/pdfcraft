//! pdfcraft-ocr — Scan & OCR ▸ Recognize text (L4).
//!
//! The caller renders a page to pixels ([`OcrImage`]) and hands it to a [`Recognizer`]. Two
//! engines exist: the pure-Rust `OcrsRecognizer` (ocrs, MIT/Apache-2.0, with its pre-trained
//! models, CC-BY-SA-4.0, fetched by `cargo xtask models`; see ATTRIBUTION.toml) and — off the
//! web — `tesseract::TesseractCli`, which drives an installed `tesseract` binary as an external
//! process. `merge` combines two engines' readings (ROVER), `preprocess` cleans the raster up
//! between rendering and recognition, and [`text_layer`] turns words placed in user space into
//! page content: invisible text (rendering mode 3) over each word, so the page becomes a
//! searchable image (Acrobat's "Searchable Image (Exact)": the image is left untouched).
//!
//! Visible editable output uses [`pdfcraft_fonts::TextPlan::for_editable_words`], approved
//! document outlines and shaped bundled-Inter Latin clusters; unsupported input is refused
//! before export. The legacy scalar plan still counts `?` substitutions. The engine's invisible
//! searchable layer uses empty Type3 programs and ToUnicode maps, preserving Unicode without
//! display fonts. WinAnsi-only invisible text keeps the standard Helvetica layout here.
//!
//! The ocrs models read the Latin alphabet (English and other languages written without
//! accents); Tesseract (the `tesseract` module, off the web build) reads whatever its installed
//! language packs cover.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod confidence;
pub mod merge;
mod model_profile;
mod ocrs;
mod pitch;
pub mod preprocess;
#[cfg(not(target_arch = "wasm32"))]
pub mod tesseract;
mod tiles;

pub use confidence::{ConfidenceBand, band_for, is_suspect, low_confidence};
pub use merge::{MergeStrategy, StrategyOutcome, mean_confidence, recognize_with_strategy, rover_merge};
pub use model_profile::ModelProfile;
pub use ocrs::{
    ALPHABET, DETECTION_MODEL, DETECTION_MODEL_SHA256, LANGUAGES, Models, Ocr, OcrsRecognizer, RECOGNITION_MODEL, RECOGNITION_MODEL_SHA256,
};

pub use pdfcraft_fonts::TextPlan;
pub use pdfcraft_fonts::helvetica_width;

/// Whether a requested OCR language is Turkish. Accept its ISO codes and locale forms so an
/// explicit `tr-TR`/`tr_TR.UTF-8` request cannot become an English reading at another layer.
pub fn is_turkish_language(code: &str) -> bool {
    let code = code.trim();
    code.eq_ignore_ascii_case("tur") || code.split(['-', '_', '.']).next().is_some_and(|base| base.eq_ignore_ascii_case("tr"))
}

/// Hard cap on either side of an image any code in this crate will touch, in pixels; larger
/// images are rejected with [`OcrError::ImageTooLarge`] instead of being processed.
pub const MAX_SIDE: u32 = 10_000;

/// Hard cap on an image's TOTAL area, in pixels (64 MP): sides under [`MAX_SIDE`] can still
/// multiply out to a raster no engine should have to read; larger images are rejected with
/// [`OcrError::ImageTooLarge`] instead of being processed.
pub const MAX_PIXELS: u32 = 64_000_000;

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    #[error("the text recognition models are not installed (reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them)")]
    NoModels,
    #[error("loading {0}: {1}")]
    Load(String, String),
    #[error("text recognition failed: {0}")]
    Recognize(String),
    #[error("the image is empty")]
    EmptyImage,
    #[error("the image is larger than the {MAX_SIDE}-pixel side or {MAX_PIXELS}-pixel limit")]
    ImageTooLarge,
    #[error("the image buffer holds {got} bytes, expected {want} for {width}x{height} RGBA")]
    ImageSize { got: usize, want: usize, width: u32, height: u32 },
    #[error("the tesseract program is not available: {0}")]
    NoTesseract(String),
    #[error("tesseract did not finish within {0} seconds")]
    Timeout(u64),
    #[error("tesseract produced more than {0} bytes of output")]
    OutputTooLarge(u64),
    #[error("tesseract failed: {0}")]
    Process(String),
}

/// An RGBA8 raster, row-major, top-left origin: what the engines read. The fields are private
/// so the invariants (non-zero, capped, exactly `width * height * 4` bytes) hold by
/// construction: an [`OcrImage`] in hand has always been validated, whatever produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OcrImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl OcrImage {
    /// Validate the sides ([`MAX_SIDE`]) and total area ([`MAX_PIXELS`]) and that `rgba` holds
    /// exactly `width * height * 4` bytes (checked arithmetic; the counts come from documents,
    /// so they are untrusted).
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<OcrImage, OcrError> {
        if width == 0 || height == 0 {
            return Err(OcrError::EmptyImage);
        }
        if width > MAX_SIDE || height > MAX_SIDE {
            return Err(OcrError::ImageTooLarge);
        }
        let pixels = (width as usize).checked_mul(height as usize).ok_or(OcrError::ImageTooLarge)?;
        if pixels > MAX_PIXELS as usize {
            return Err(OcrError::ImageTooLarge);
        }
        match pixels.checked_mul(4) {
            Some(want) if rgba.len() == want => Ok(OcrImage { width, height, rgba }),
            Some(want) => Err(OcrError::ImageSize { got: rgba.len(), want, width, height }),
            None => Err(OcrError::ImageTooLarge),
        }
    }

    /// The image's width, in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The image's height, in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The RGBA bytes, row-major, exactly `width * height * 4` of them.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// The smallest side, in pixels.
    pub fn min_side(&self) -> u32 {
        self.width.min(self.height)
    }

    /// One RGBA byte at integer pixel (`x`, `y`), edges clamped; 255 (white) outside. Only
    /// whole-pixel positions and channel 0–3 make sense; anything else reads clamped.
    pub fn byte(&self, x: f64, y: f64, ch: usize) -> u8 {
        let stride = self.width as usize * 4;
        if stride == 0 || self.height == 0 || ch > 3 {
            return 255;
        }
        let xi = (x.round().max(0.0) as usize).min(self.width as usize - 1);
        let yi = (y.round().max(0.0) as usize).min(self.height as usize - 1);
        self.rgba.get(yi * stride + xi * 4 + ch).copied().unwrap_or(255)
    }

    /// The grayscale value of the pixel at (`x`, `y`), 0 (ink) … 255 (paper), with the borders
    /// clamped (edges replicate). `None` only when the image is empty.
    pub fn gray(&self, x: i64, y: i64) -> Option<u8> {
        let x = x.clamp(0, self.width as i64 - 1);
        let y = y.clamp(0, self.height as i64 - 1);
        let row = y as usize * self.width as usize * 4;
        let o = row + x as usize * 4;
        let px = self.rgba.get(o..o + 4)?;
        Some(((299 * px[0] as u32 + 587 * px[1] as u32 + 114 * px[2] as u32) / 1000) as u8)
    }
}

/// A recognised word: its text and its box in image pixels `[left, top, right, bottom]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub rect: [f32; 4],
    /// How sure the engine is, 0–100, when it reports a confidence at all (`None`: the ocrs
    /// engine does not).
    pub confidence: Option<f32>,
    /// Which engine read this word: the recogniser's [`Recognizer::id`], or `"ROVER"` when two
    /// engines were merged and the surviving text is the merged choice.
    pub source: String,
}

impl Word {
    pub fn new(text: impl Into<String>, rect: [f32; 4], source: &str) -> Word {
        Word { text: text.into(), rect, confidence: None, source: source.into() }
    }

    /// Set the confidence (builder style), clamped to the documented 0–100 scale (a negative
    /// value is 0, anything over 100 is 100). NaN means no confidence was reported (`None`),
    /// the same unknown the engines that report nothing carry.
    pub fn with_confidence(mut self, confidence: f32) -> Word {
        self.confidence = if confidence.is_nan() { None } else { Some(confidence.clamp(0.0, 100.0)) };
        self
    }
}

/// One optional reading of an explicitly selected word crop. This is a proposal, never an
/// automatic correction. Its confidence belongs to the crop reading, not the original word.
#[derive(Clone, Debug, PartialEq)]
pub struct WordCandidate {
    pub text: String,
    pub confidence: Option<f32>,
    /// Symmetric white border added around the tightly bounded ink, in pixels.
    pub padding: u32,
    pub source: String,
}

/// A recognised line of words, in reading order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Line {
    pub words: Vec<Word>,
}

impl Line {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

/// What a recogniser needs to know beyond the pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecognizeOptions {
    /// Language code from [`LANGUAGES`] (`"en"`, `"de"`, …). Engines map it to their own codes
    /// (Tesseract: `tesseract::language_code`, allow-listed before anything reaches a process);
    /// unknown codes keep the Standard engine default; TurkishExtended requires Turkish.
    pub language: String,
    /// Fixed model selection, validated for the language and engine before recognition.
    pub model_profile: ModelProfile,
}

impl Default for RecognizeOptions {
    fn default() -> Self {
        RecognizeOptions { language: "en".into(), model_profile: ModelProfile::Standard }
    }
}

/// One text-recognition engine. Implementations are shared across threads
/// (`Arc<dyn Recognizer>`); `available` and `recognize` may spawn processes only off the web
/// build (see the `tesseract` module).
pub trait Recognizer: Send + Sync {
    /// The engine's short name (`"ocrs"`, `"tesseract"`): what [`Word::source`] carries and
    /// what settings and tools name it by.
    fn id(&self) -> &'static str;
    /// Whether this engine can run right now (models installed, binary found).
    fn available(&self) -> bool;
    /// The languages it reads, as `(code, name)` pairs (`"en"`, `"English"`).
    fn languages(&self) -> Vec<(String, String)>;
    /// Installed model profiles this recognizer can select per call. Older implementations
    /// support only Standard unless they explicitly opt into additional fixed profiles.
    fn model_profiles(&self) -> Vec<ModelProfile> {
        vec![ModelProfile::Standard]
    }
    /// Read the words in `image`, in reading order.
    fn recognize(&self, image: &OcrImage, options: &RecognizeOptions) -> Result<Vec<Line>, OcrError>;
    /// Propose readings of a caller-selected word crop, only after an explicit review request.
    /// The caller must retain the original text, confidence and geometry until a separate
    /// user-approved apply operation. Unsupported engines fail rather than silently rerun OCR.
    fn word_candidates(&self, _image: &OcrImage, _options: &RecognizeOptions) -> Result<Vec<WordCandidate>, OcrError> {
        Err(OcrError::Recognize("this recognizer does not support selected-word candidates".into()))
    }
}

/// A word placed on a page, in PDF user space: the bottom-left corner of its box, and the
/// vectors along its bottom edge (`across`, left → right as read) and its left edge (`up`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedWord {
    pub text: String,
    pub origin: [f64; 2],
    pub across: [f64; 2],
    pub up: [f64; 2],
    /// Confidence 0–100, when the engine reported one.
    pub confidence: Option<f32>,
    /// Which engine read this word (see [`Word::source`]).
    pub source: String,
}

impl PlacedWord {
    /// Place a pixel-space word with `to_user`, which maps an image pixel (x right, y down) to
    /// user space.
    pub fn place(word: &Word, to_user: impl Fn(f32, f32) -> [f64; 2]) -> PlacedWord {
        let [l, t, r, b] = word.rect;
        let o = to_user(l, b);
        let br = to_user(r, b);
        let tl = to_user(l, t);
        PlacedWord {
            text: word.text.clone(),
            origin: o,
            across: [br[0] - o[0], br[1] - o[1]],
            up: [tl[0] - o[0], tl[1] - o[1]],
            confidence: word.confidence,
            source: word.source.clone(),
        }
    }
}

/// A narrow, separate space after a recognised word, when its neighbour continues the same
/// line. Original word boxes stay unchanged. CJK boundaries, overlaps, line breaks and
/// column-sized gaps retain their existing geometry-based extraction.
pub fn word_separator(previous: &PlacedWord, next: &PlacedWord) -> Option<PlacedWord> {
    let cjk = |text: &str| {
        text.chars()
            .any(|c| matches!(c as u32, 0x1100..=0x11FF | 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3134F))
    };
    if previous.text.is_empty()
        || next.text.is_empty()
        || previous.text.chars().next_back().is_some_and(char::is_whitespace)
        || next.text.chars().next().is_some_and(char::is_whitespace)
        || cjk(&previous.text)
        || cjk(&next.text)
    {
        return None;
    }
    let (pw, nw) = (previous.across[0].hypot(previous.across[1]), next.across[0].hypot(next.across[1]));
    let (ph, nh) = (previous.up[0].hypot(previous.up[1]), next.up[0].hypot(next.up[1]));
    if [pw, nw, ph, nh].iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return None;
    }
    let u = previous.across.map(|v| v / pw);
    let v = next.across.map(|v| v / nw);
    if u[0] * v[0] + u[1] * v[1] <= 0.0 || (u[0] * v[1] - u[1] * v[0]).abs() > 0.15 {
        return None;
    }
    let end = [previous.origin[0] + previous.across[0], previous.origin[1] + previous.across[1]];
    let gap = (next.origin[0] - end[0]) * u[0] + (next.origin[1] - end[1]) * u[1];
    let centre_delta = [
        next.origin[0] - previous.origin[0] + 0.5 * (next.across[0] - previous.across[0] + next.up[0] - previous.up[0]),
        next.origin[1] - previous.origin[1] + 0.5 * (next.across[1] - previous.across[1] + next.up[1] - previous.up[1]),
    ];
    let offset = centre_delta[1] * u[0] - centre_delta[0] * u[1];
    let height = ph.max(nh);
    if !gap.is_finite() || !offset.is_finite() || gap < 0.0 || gap >= height * 3.0 || offset.abs() >= height * 0.5 {
        return None;
    }
    // A sliver carries the known boundary without stretching a word or filling a table gap.
    let across = u.map(|v| v * ph * 0.01);
    if (0..2).any(|i| !(end[i] + across[i] + 0.2 * previous.up[i]).is_finite()) {
        return None;
    }
    Some(PlacedWord { text: " ".into(), origin: end, across, up: previous.up, confidence: None, source: String::new() })
}

/// Helvetica's ascender and descender (em fractions): a word box spans about this much.
const BOX_EM: f64 = 0.93;
const DESCENT: f64 = 0.21;
/// Helvetica's cap height and x-height (em fractions).
const CAP_HEIGHT: f64 = 0.718;
const X_HEIGHT: f64 = 0.523;

/// How far `text`'s ink reaches above and below the baseline, in em fractions: a word box is
/// tight around the ink, so "DT" spans the cap height only, "sun" the x-height, and "gap" reaches
/// down to the descender. Text without letters or digits keeps the full ascender-descender span.
fn ink_extent(text: &str) -> (f64, f64) {
    if !text.chars().any(char::is_alphanumeric) {
        return (BOX_EM - DESCENT, -DESCENT);
    }
    let tall = text
        .chars()
        .any(|c| c.is_uppercase() || c.is_ascii_digit() || "bdfhijklt!?/\\()[]{}|'\"#$%&@".contains(c) || (c.is_alphabetic() && !c.is_ascii()));
    let low = text.chars().any(|c| "gjpqy()[]{}|_".contains(c));
    (if tall { CAP_HEIGHT } else { X_HEIGHT }, if low { -DESCENT } else { 0.0 })
}

fn num(v: f64) -> String {
    let s = format!("{:.4}", if v.abs() < 1e-9 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Visible font sizes and scales may be much smaller than a coordinate. Four decimal
/// places can turn a supported long/narrow word into `0 Tz`, or a small face into `0 Tf`.
/// Keep the legacy spelling at ordinary sizes; small finite values use the same
/// shortest, exponent-free f64 Display policy as the COS writer, without clamping.
fn scale_num(v: f64) -> String {
    if !v.is_finite() || v == 0.0 || v.abs() >= 0.01 {
        return num(v);
    }
    v.to_string()
}

/// A standalone space uses Helvetica, already required by both layers, and never consumes
/// an entry in the words' display-font plan. Reset scaling left by a visible word's `Tz`.
fn write_separator(out: &mut Vec<u8>, space: &PlacedWord) -> bool {
    let width = helvetica_width(" ", 1.0);
    let em = space.up.map(|v| v / BOX_EM);
    let matrix =
        [space.across[0] / width, space.across[1] / width, em[0], em[1], space.origin[0] + DESCENT * em[0], space.origin[1] + DESCENT * em[1]];
    // Finite word geometry can overflow when converted to Helvetica's em and baseline.
    // Omit only that separator, before changing the output's font or horizontal scaling.
    if !matrix.iter().all(|v| v.is_finite()) {
        return false;
    }
    out.extend_from_slice(b"/PCHelv 1 Tf 100 Tz ");
    out.extend_from_slice(matrix.map(num).join(" ").as_bytes());
    out.extend_from_slice(b" Tm ( ) Tj\n");
    true
}

/// One run of a word's planned characters in one Type3 font: the resource name, the codes to
/// show (one byte per scalar or checked cluster), and the run's start as an offset in em units from the word's
/// origin (the accumulated advances of the runs before it).
struct PlannedRun {
    font: String,
    codes: Vec<u8>,
    start: f64,
    actual_text: Option<String>,
}

/// The planned runs of word `i` (the same index the text plan planned
/// the words with), or `None` when the word is drawn in Helvetica: every character has a
/// WinAnsi code, or nothing could be planned/registered for it (the pre-Unicode output).
fn planned_runs(plan: &pdfcraft_fonts::TextPlan, word: usize) -> Option<(Vec<PlannedRun>, f64)> {
    let enc = plan.encoding(word);
    if enc.is_empty() || !plan.registered(enc) {
        return None;
    }
    let mut runs: Vec<PlannedRun> = Vec::new();
    let mut total = 0.0;
    for &(f, code) in enc {
        let font = plan.fonts[f].name.as_str();
        let source = plan.cluster_text(f, code);
        match runs.last_mut() {
            Some(last) if last.font == font && last.actual_text.is_none() && source.is_none() => last.codes.push(code),
            _ => runs.push(PlannedRun { font: font.to_string(), codes: vec![code], start: total, actual_text: source.map(str::to_string) }),
        }
        total += plan.advance(f, code);
    }
    (total.is_finite() && total > 0.0).then_some((runs, total))
}

/// Page content that writes `words` as VISIBLE text at their original boxes: the
/// editable-text output's page content. WinAnsi words are set in `/PCHelv` (standard
/// Helvetica); words with characters beyond WinAnsi are set in the plan's Type3 fonts, one run
/// per font. Each word fills its box — the font size comes from the box height and the
/// horizontal scaling is computed from the real advances of the font that draws the word
/// (Helvetica's approximated, the Type3 faces' exact) and emitted as an explicit `Tz`
/// percentage. Words whose box is empty (or whose text has no measurable width) are skipped; a
/// page that ends up with nothing simply shows nothing.
pub fn visible_text_layer(words: &[PlacedWord], plan: &pdfcraft_fonts::TextPlan) -> Vec<u8> {
    visible_text_layer_at(words, plan, 0)
}

/// Visible text for words starting at `word_offset` in a shared document plan.
pub fn visible_text_layer_at(words: &[PlacedWord], plan: &pdfcraft_fonts::TextPlan, word_offset: usize) -> Vec<u8> {
    // The rendering mode and the font are graphics state, which `ET` doesn't reset: without
    // `q … Q` they would stay set for every later content stream on the page, so text added
    // after OCR (a header or footer) drew invisibly (#3029).
    let mut out = b"/OCR BMC\nq\nBT\n0 Tr\n/PCHelv 1 Tf\n".to_vec();
    // The font and size the text state is set in, so a Tf is written only when they change.
    // (The header's size-1 Helvetica covers layers of WinAnsi words only.)
    let mut current: Option<(String, f64)> = Some(("PCHelv".to_string(), 1.0));
    for (i, w) in words.iter().enumerate() {
        let height = w.up[0].hypot(w.up[1]);
        let across = w.across[0].hypot(w.across[1]);
        let (ux, uy) = (w.across[0] / across, w.across[1] / across);
        let (vx, vy) = (w.up[0] / height, w.up[1] / height);
        if !across.is_finite()
            || !height.is_finite()
            || across <= 0.0
            || height <= 0.0
            || !ux.is_finite()
            || !vx.is_finite()
            || !w.origin.iter().all(|v| v.is_finite())
        {
            continue;
        }
        let word_index = word_offset.saturating_add(i);
        let runs = planned_runs(plan, word_index);
        // Preserve the legacy NFC em fit for ordinary Latin accents. Only ink beyond
        // that font envelope (e.g. stacked marks) needs a larger vertical fit, otherwise
        // canonically equivalent NFC/NFD text would change size and baseline.
        let shaped = plan.shaped_bounds(word_index).filter(|_| runs.is_some());
        let bounds = shaped.filter(|b| b[1] < -0.3 || b[3] > 1.0);
        let size = height / bounds.map_or(BOX_EM, |b| b[3] - b[1]);
        let horizontal = runs.as_ref().map(|(_, total)| shaped.map_or((0.0, *total), |b| (b[0], b[2] - b[0])));
        // The word's natural advance at this size: Helvetica's approximation, or the exact sum
        // of the planned glyphs' widths.
        let natural = horizontal.map_or_else(|| helvetica_width(&w.text, size), |(_, span)| span * size);
        if !natural.is_finite() || natural <= 0.0 {
            continue;
        }
        // Horizontal scaling as a percentage of the natural advance at this size, so the word
        // lands exactly on its box (100 = no scaling; a long box on a short word stretches it).
        let tz = if shaped.is_some() { across / natural * 100.0 } else { (across / natural * 100.0).max(1.0) };
        // The baseline sits above the box's bottom edge by the descender depth.
        let descent = bounds.map_or(DESCENT, |b| -b[1]);
        let left = horizontal.map_or(0.0, |(left, span)| left / span);
        let o = [w.origin[0] + vx * descent * size - w.across[0] * left, w.origin[1] + vy * descent * size - w.across[1] * left];
        if !tz.is_finite() || o.iter().any(|v| !v.is_finite()) {
            continue;
        }
        out.extend_from_slice(format!("{} Tz ", scale_num(tz)).as_bytes());
        match &runs {
            None => {
                if current.as_ref().is_some_and(|(f, s)| f != "PCHelv" || *s != size) {
                    out.extend_from_slice(format!("/PCHelv {} Tf ", scale_num(size)).as_bytes());
                    current = Some(("PCHelv".to_string(), size));
                }
                out.extend_from_slice(format!("{} {} {} {} {} {} Tm ", num(ux), num(uy), num(vx), num(vy), num(o[0]), num(o[1])).as_bytes());
                out.extend(pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(&w.text)));
                out.extend_from_slice(b" Tj\n");
            }
            Some((planned, total)) => {
                let mut preceding_font = None;
                for run in planned {
                    if let Some(text) = &run.actual_text {
                        let hex: String = text.encode_utf16().map(|u| format!("{u:04X}")).collect();
                        out.extend_from_slice(format!("/Span << /ActualText <FEFF{hex}> >> BDC\n").as_bytes());
                    }
                    if current.as_ref().is_some_and(|(f, s)| *f != run.font || *s != size) {
                        out.extend_from_slice(format!("/{} {} Tf ", run.font, scale_num(size)).as_bytes());
                        current = Some((run.font.clone(), size));
                    }
                    // The run's origin: the word's baseline plus the across vector scaled by
                    // the advances before this run (the stretch applies to runs alike).
                    let frac = run.start / horizontal.map_or(*total, |(_, span)| span);
                    // Keep adjacent cluster Tj operations in the same text matrix. This
                    // preserves the scalar registrar's advances without rounding each
                    // absolute cluster origin, so NFC/NFD remain pixel-identical.
                    if preceding_font != Some(run.font.as_str()) {
                        out.extend_from_slice(
                            format!(
                                "{} {} {} {} {} {} Tm ",
                                num(ux),
                                num(uy),
                                num(vx),
                                num(vy),
                                num(o[0] + w.across[0] * frac),
                                num(o[1] + w.across[1] * frac)
                            )
                            .as_bytes(),
                        );
                    }
                    out.extend(pdfcraft_fonts::literal(&run.codes));
                    out.extend_from_slice(b" Tj\n");
                    if run.actual_text.is_some() {
                        out.extend_from_slice(b"EMC\n");
                    }
                    preceding_font = Some(run.font.as_str());
                }
            }
        }
        if let Some(next) = words.get(i.saturating_add(1))
            && let Some(space) = word_separator(w, next)
            && write_separator(&mut out, &space)
        {
            current = Some(("PCHelv".to_string(), 1.0));
        }
    }
    out.extend_from_slice(b"ET\nQ\nEMC\n");
    out
}

/// Page content that writes `words` as invisible text (rendering mode 3), each word stretched
/// to its box so selection and search highlight the right place. WinAnsi words are set in
/// `/PCHelv` (standard Helvetica); words with characters beyond WinAnsi are set in the plan's
/// Type3 fonts, one run per font, with the same exact box fit. Marked content `/OCR` so it can
/// be told apart from the page's own text.
pub fn text_layer(words: &[PlacedWord], plan: &pdfcraft_fonts::TextPlan) -> Vec<u8> {
    // `3 Tr` and the font are graphics state, which `ET` doesn't reset: without `q … Q` they
    // would stay set for every later content stream on the page, so text added after OCR (a
    // header or footer) drew invisibly (#3029).
    let mut out = b"/OCR BMC\nq\nBT\n3 Tr\n/PCHelv 1 Tf\n".to_vec();
    let mut current: Option<(String, f64)> = Some(("PCHelv".to_string(), 1.0));
    for (i, w) in words.iter().enumerate() {
        let runs = planned_runs(plan, i);
        let width = runs.as_ref().map_or_else(|| helvetica_width(&w.text, 1.0), |(_, total)| *total);
        let height = w.up[0].hypot(w.up[1]);
        let across = w.across[0].hypot(w.across[1]);
        // Only positive, finite geometry reaches the content stream: a zero-sized box cannot
        // be drawn, and a NaN or infinite one would put non-finite numbers on the page. (NaN
        // fails every comparison, so the positivity checks already refuse it; the finiteness
        // checks keep that property explicit.)
        if width <= 0.0 || !(height > 0.0 && height.is_finite() && across > 0.0 && across.is_finite() && w.origin.iter().all(|v| v.is_finite())) {
            continue;
        }
        // The em square. Helvetica words: the word's own ink (cap height or x-height,
        // descenders or not) spans the box, so the text is as tall as the scanned word. Planned
        // (Type3) words: those Helvetica letter heights do not describe the face, so the box spans
        // the ascender-descender height. Either way the width is stretched to fit the word.
        let (top, bottom) = if runs.is_some() { (BOX_EM - DESCENT, -DESCENT) } else { ink_extent(&w.text) };
        let em = [w.up[0] / (top - bottom), w.up[1] / (top - bottom)];
        let o = [w.origin[0] - em[0] * bottom, w.origin[1] - em[1] * bottom];
        match &runs {
            None => {
                let ax = [w.across[0] / width, w.across[1] / width];
                if current.as_ref().is_some_and(|(f, s)| f != "PCHelv" || *s != 1.0) {
                    out.extend_from_slice(b"/PCHelv 1 Tf ");
                    current = Some(("PCHelv".to_string(), 1.0));
                }
                let m = [ax[0], ax[1], em[0], em[1], o[0], o[1]];
                let nums: Vec<String> = m.iter().map(|v| num(*v)).collect();
                out.extend_from_slice(nums.join(" ").as_bytes());
                out.extend_from_slice(b" Tm ");
                out.extend(pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(&w.text)));
                out.extend_from_slice(b" Tj\n");
            }
            Some((planned, total)) => {
                for run in planned {
                    if current.as_ref().is_some_and(|(f, s)| *f != run.font || *s != 1.0) {
                        out.extend_from_slice(format!("/{} 1 Tf ", run.font).as_bytes());
                        current = Some((run.font.clone(), 1.0));
                    }
                    let m = [
                        w.across[0] / total,
                        w.across[1] / total,
                        em[0],
                        em[1],
                        o[0] + w.across[0] * run.start / total,
                        o[1] + w.across[1] * run.start / total,
                    ];
                    let nums: Vec<String> = m.iter().map(|v| num(*v)).collect();
                    out.extend_from_slice(nums.join(" ").as_bytes());
                    out.extend_from_slice(b" Tm ");
                    out.extend(pdfcraft_fonts::literal(&run.codes));
                    out.extend_from_slice(b" Tj\n");
                }
            }
        }
        if let Some(next) = words.get(i.saturating_add(1))
            && let Some(space) = word_separator(w, next)
            && write_separator(&mut out, &space)
        {
            current = Some(("PCHelv".to_string(), 1.0));
        }
    }
    out.extend_from_slice(b"ET\nQ\nEMC\n");
    out
}

/// The recognised lines as JSON, `[[["word", left, top, right, bottom], …], …]`, with the
/// word's confidence as a sixth item when it has one: how the web build's OCR workers send what
/// they found back to the page.
pub fn lines_to_json(lines: &[Line]) -> String {
    let word = |w: &Word| {
        let mut items = vec![serde_json::json!(w.text), w.rect[0].into(), w.rect[1].into(), w.rect[2].into(), w.rect[3].into()];
        items.extend(w.confidence.map(serde_json::Value::from));
        serde_json::Value::Array(items)
    };
    serde_json::Value::Array(lines.iter().map(|l| serde_json::Value::Array(l.words.iter().map(word).collect())).collect()).to_string()
}

/// At most this many words come back for one page (a dense page has a few thousand).
const MAX_JSON_WORDS: usize = 100_000;

/// [`lines_to_json`] read back, the words credited to `source` (the engine the worker ran).
/// Anything else (a word without its box, a box or confidence that isn't a finite number, too
/// many words) is an error rather than a guess.
pub fn lines_from_json(json: &str, source: &str) -> Result<Vec<Line>, OcrError> {
    let bad = |why: &str| OcrError::Recognize(format!("unreadable result: {why}"));
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| bad(&e.to_string()))?;
    let lines = value.as_array().ok_or_else(|| bad("not a list of lines"))?;
    let number = |v: &serde_json::Value| v.as_f64().map(|v| v as f32).filter(|v| v.is_finite());
    let mut count = 0usize;
    let mut out = Vec::with_capacity(lines.len().min(MAX_JSON_WORDS));
    for line in lines {
        let mut words = Vec::new();
        for word in line.as_array().ok_or_else(|| bad("a line is not a list of words"))? {
            count += 1;
            if count > MAX_JSON_WORDS {
                return Err(bad("too many words"));
            }
            let (text, rect, confidence) = match word.as_array().map(Vec::as_slice) {
                Some([serde_json::Value::String(text), l, t, r, b]) => (text, [l, t, r, b], None),
                Some([serde_json::Value::String(text), l, t, r, b, c]) => (text, [l, t, r, b], Some(c)),
                _ => return Err(bad("a word is not [text, left, top, right, bottom, confidence?]")),
            };
            let mut r = [0f32; 4];
            for (slot, v) in r.iter_mut().zip(rect) {
                *slot = number(v).ok_or_else(|| bad("a word's box is not finite numbers"))?;
            }
            let mut w = Word::new(text.clone(), r, source);
            if let Some(c) = confidence {
                w = w.with_confidence(number(c).ok_or_else(|| bad("a word's confidence is not a finite number"))?);
            }
            words.push(w);
        }
        out.push(Line { words });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editable_nfd_serialized_scale(source: &str, width: f64, height: f64) -> (f64, f64, f64) {
        let mut plan = TextPlan::for_editable_words([source]).unwrap();
        for (i, font) in plan.fonts.iter_mut().enumerate() {
            font.name = format!("PCOcr{i}");
        }
        let word = PlacedWord {
            text: source.into(),
            origin: [20.0, 20.0],
            across: [width, 0.0],
            up: [0.0, height],
            confidence: None,
            source: "synthetic".into(),
        };
        let content = String::from_utf8(visible_text_layer(&[word], &plan)).unwrap();
        let first = content.split("Tj").next().unwrap();
        let tokens: Vec<_> = first.split_whitespace().collect();
        let size = tokens.windows(3).rfind(|w| w[2] == "Tf").unwrap()[1];
        let tz = tokens.windows(2).find(|w| w[1] == "Tz").unwrap()[0];
        assert!(!size.contains(['e', 'E']) && !tz.contains(['e', 'E']), "PDF numbers cannot use exponents");
        let size = size.parse::<f64>().unwrap();
        let tz = tz.parse::<f64>().unwrap();
        let matrix = tokens.windows(7).find(|w| w[6] == "Tm").unwrap();
        assert!(matrix[..6].iter().all(|v| v.parse::<f64>().is_ok_and(f64::is_finite)));
        (size, tz, plan.advance_of(plan.encoding(0)))
    }

    #[test]
    fn editable_nfd_long_word_keeps_nonzero_serialized_scale_and_exact_width() {
        let source = format!("{}a\u{301}", "a".repeat(60_000));
        for width in [0.12, 0.24] {
            let (size, tz, advance) = editable_nfd_serialized_scale(&source, width, 10.0);
            assert!(size > 0.0 && size.is_finite() && tz > 0.0 && tz.is_finite(), "serialized size {size}, Tz {tz} must not collapse ink");
            assert!((size * advance * tz / 100.0 - width).abs() < 0.000002, "serialized scale must fit the actual word width");
        }
    }

    #[test]
    fn editable_nfd_tiny_font_keeps_nonzero_serialized_size_and_exact_width() {
        let (size, tz, advance) = editable_nfd_serialized_scale("a\u{301}", 0.12, 0.00000001);
        assert!(size > 0.0 && size.is_finite() && tz > 0.0 && tz.is_finite(), "serialized size {size}, Tz {tz} must not collapse ink");
        assert!((size * advance * tz / 100.0 - 0.12).abs() < 0.000002);
    }

    #[test]
    fn editable_nfd_cluster_content_keeps_utf16_page_offsets_rotations_and_narrow_boxes() {
        let text = "AI\u{307}g\u{306}Z";
        let mut plan = TextPlan::for_editable_words(["q\u{315}\u{301}", text]).unwrap();
        for (i, font) in plan.fonts.iter_mut().enumerate() {
            font.name = format!("PCOcr{i}");
        }
        for u in [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]] {
            let word = PlacedWord {
                text: text.into(),
                origin: [180.0, 20.0],
                across: [0.01 * u[0], 0.01 * u[1]],
                up: [-10.0 * u[1], 10.0 * u[0]],
                confidence: None,
                source: "synthetic".into(),
            };
            let content = String::from_utf8(visible_text_layer_at(&[word], &plan, 1)).unwrap();
            assert!(content.contains("<FEFF00490307>"), "original UTF-16 I plus dot: {content}");
            assert!(!content.contains("<FEFF007103150301>"), "page-local index must not write the preceding page");
            assert_eq!(content.matches("/ActualText").count(), 4, "one logical cluster per code");
            let tokens: Vec<_> = content.split_whitespace().collect();
            let matrices: Vec<_> = tokens.windows(7).filter(|w| w[6] == "Tm").collect();
            assert!(!matrices.is_empty(), "the cluster run has an explicit rotated matrix");
            for matrix in matrices {
                assert!(matrix[..6].iter().all(|v| v.parse::<f64>().is_ok_and(f64::is_finite)));
                let actual: Vec<_> = matrix[..4].iter().map(|v| v.parse::<f64>().unwrap()).collect();
                assert_eq!(actual, [u[0], u[1], -u[1], u[0]]);
            }
            let tz = tokens.windows(2).find(|w| w[1] == "Tz").unwrap()[0].parse::<f64>().unwrap();
            assert!(tz > 0.0 && tz < 1.0, "a narrow box must not be enlarged to 1%: {tz}");
        }
    }

    #[test]
    fn editable_nfd_left_and_right_ink_overhang_fit_the_word_box_at_every_rotation() {
        let mut left = false;
        let mut right = false;
        for source in ["j\u{301}", "q\u{315}\u{301}\u{301}", "A\u{315}"] {
            let mut plan = TextPlan::for_editable_words([source]).unwrap();
            for (i, font) in plan.fonts.iter_mut().enumerate() {
                font.name = format!("PCOcr{i}");
            }
            let (f, c) = plan.encoding(0)[0];
            let glyph = &plan.fonts[f].glyphs[usize::from(c) - 1].1;
            left |= glyph.bbox[0] < 0.0;
            right |= glyph.bbox[2] > glyph.width;
            for u in [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]] {
                for width in [0.01, 40.0] {
                    let word = PlacedWord {
                        text: source.into(),
                        origin: [100.0, 100.0],
                        across: u.map(|x| x * width),
                        up: [-15.0 * u[1], 15.0 * u[0]],
                        confidence: None,
                        source: "synthetic".into(),
                    };
                    let content = String::from_utf8(visible_text_layer(&[word], &plan)).unwrap();
                    let tokens: Vec<_> = content.split_whitespace().collect();
                    let size = tokens.windows(3).rfind(|w| w[2] == "Tf").unwrap()[1].parse::<f64>().unwrap();
                    let tz = tokens.windows(2).find(|w| w[1] == "Tz").unwrap()[0].parse::<f64>().unwrap() / 100.0;
                    let tm: Vec<_> = tokens.windows(7).find(|w| w[6] == "Tm").unwrap()[..6].iter().map(|v| v.parse::<f64>().unwrap()).collect();
                    for p in glyph.contours.iter().flatten() {
                        let x = tm[4] + tm[0] * p[0] * size * tz + tm[2] * p[1] * size;
                        let y = tm[5] + tm[1] * p[0] * size * tz + tm[3] * p[1] * size;
                        let along = (x - 100.0) * u[0] + (y - 100.0) * u[1];
                        assert!((-0.002..=width + 0.002).contains(&along), "{source:?}, {u:?}, width {width}: ink {along} escaped; {content}");
                    }
                }
            }
        }
        assert!(left && right, "fixtures must exercise both negative left bearing and right overhang");
    }

    #[test]
    fn searchable_word_space_with_an_overflowing_matrix_is_omitted() {
        let first = PlacedWord {
            text: "A".into(),
            origin: [0.0, 1.58e308],
            across: [1.0, 0.0],
            up: [0.0, 1.0e308],
            confidence: None,
            source: "synthetic".into(),
        };
        let second = PlacedWord { text: "B".into(), origin: [1.0, 1.58e308], ..first.clone() };
        assert!(word_separator(&first, &second).is_some(), "the raw separator geometry is finite");
        let finite_matrices = |content: &[u8]| {
            let content = std::str::from_utf8(content).unwrap();
            let tokens: Vec<_> = content.split_whitespace().collect();
            tokens.windows(7).filter(|w| w[6] == "Tm").all(|w| w[..6].iter().all(|v| v.parse::<f64>().is_ok_and(f64::is_finite)))
        };
        for word in [&first, &second] {
            assert!(finite_matrices(&text_layer(std::slice::from_ref(word), &TextPlan::default())), "the original word's matrix stays finite");
        }
        let content = text_layer(&[first, second], &TextPlan::default());
        assert!(finite_matrices(&content), "a derived separator matrix must not overflow");
        let content = String::from_utf8(content).unwrap();
        assert!(content.contains("(A) Tj") && content.contains("(B) Tj"));
        assert!(!content.contains("( ) Tj"), "only the overflowing separator is omitted");
    }

    #[test]
    fn explicit_word_spaces_are_slivers_with_bounded_line_geometry() {
        let previous = PlacedWord {
            text: "ab".into(),
            origin: [20.0, 250.0],
            across: [12.0, 0.0],
            up: [0.0, 10.0],
            confidence: None,
            source: "synthetic".into(),
        };
        let next = PlacedWord { text: "CD".into(), origin: [36.0, 250.0], ..previous.clone() };
        let space = word_separator(&previous, &next).unwrap();
        assert_eq!(space.origin, [32.0, 250.0]);
        assert_eq!(space.across, [0.1, 0.0], "the boundary does not fill the four-point word gap");
        assert_eq!(space.up, previous.up);
        for (why, changed) in [
            ("next line", PlacedWord { origin: [36.0, 230.0], ..next.clone() }),
            ("table cell", PlacedWord { origin: [80.0, 250.0], ..next.clone() }),
            ("overlap", PlacedWord { origin: [31.0, 250.0], ..next.clone() }),
            ("opposite direction", PlacedWord { across: [-12.0, 0.0], ..next.clone() }),
            ("different direction", PlacedWord { across: [0.0, 12.0], ..next.clone() }),
            ("existing space", PlacedWord { text: " CD".into(), ..next.clone() }),
            ("CJK", PlacedWord { text: "日本".into(), ..next.clone() }),
            ("nonfinite", PlacedWord { origin: [f64::NAN, 250.0], ..next.clone() }),
            ("empty box", PlacedWord { across: [0.0, 0.0], ..next.clone() }),
        ] {
            assert!(word_separator(&previous, &changed).is_none(), "{why}");
        }
    }

    #[test]
    fn visible_word_spaces_do_not_shift_the_display_font_plan_offset() {
        if pdfcraft_fonts::CRAFT_FONTS.is_empty() {
            eprintln!("skipped: built without craft-fonts");
            return;
        }
        let words: Vec<_> = [("ab", 20.0), ("CD", 32.1), ("日本", 44.2)]
            .into_iter()
            .map(|(text, x)| PlacedWord {
                text: text.into(),
                origin: [x, 20.0],
                across: [12.0, 0.0],
                up: [0.0, 10.0],
                confidence: None,
                source: String::new(),
            })
            .collect();
        let mut plan = TextPlan::for_words(["previous page", "ab", "CD", "日本"]);
        for (i, font) in plan.fonts.iter_mut().enumerate() {
            font.name = format!("PCOcr{i}");
        }
        let codes: Vec<u8> = plan.encoding(3).iter().map(|&(_, c)| c).collect();
        assert_eq!(codes.len(), 2);
        let content = String::from_utf8(visible_text_layer_at(&words, &plan, 1)).unwrap();
        assert_eq!(content.matches("Tm ( ) Tj").count(), 1, "one Latin word boundary: {content}");
        assert!(content.contains(&format!("Tm {} Tj", String::from_utf8_lossy(&pdfcraft_fonts::literal(&codes)))), "{content}");
    }

    #[test]
    fn places_words_through_the_mapping() {
        // 2 pixels per point, page 612 × 792, y flipped.
        let to_user = |x: f32, y: f32| [x as f64 / 2.0, 792.0 - y as f64 / 2.0];
        let w = PlacedWord::place(&Word::new("Hello", [100.0, 200.0, 300.0, 240.0], "ocrs"), to_user);
        assert_eq!(w.origin, [50.0, 672.0]);
        assert_eq!(w.across, [100.0, 0.0]);
        assert_eq!(w.up, [0.0, 20.0]);
        assert_eq!(w.source, "ocrs");
        assert_eq!(w.confidence, None);
    }

    /// Confidence and source survive placement.
    #[test]
    fn placed_words_carry_confidence_and_source() {
        let to_user = |x: f32, y: f32| [x as f64, y as f64];
        let w = Word::new("Hi", [1.0, 2.0, 3.0, 4.0], "tesseract").with_confidence(87.5);
        let p = PlacedWord::place(&w, to_user);
        assert_eq!(p.confidence, Some(87.5));
        assert_eq!(p.source, "tesseract");
    }

    /// The text layer is Helvetica in WinAnsiEncoding: every character the model can recognise
    /// must be one it can hold, or recognised text would be saved as `?`. A model with a wider
    /// alphabet (Chinese, for example) needs a text layer in another font first.
    #[test]
    fn the_model_alphabet_fits_the_text_layer_font() {
        assert_eq!(pdfcraft_fonts::first_undrawable(ALPHABET), None);
    }

    /// `with_confidence` is clamped to the documented 0–100 scale: negatives (and −inf) are 0,
    /// anything over 100 (and +inf) is 100, in-scale values pass through, and NaN means no
    /// confidence was reported — the same `None` unknown the unreporting engines carry.
    #[test]
    fn with_confidence_clamps_and_treats_nan_as_unknown() {
        let c = |v: f32| Word::new("w", [0.0; 4], "ocrs").with_confidence(v).confidence;
        assert_eq!(c(0.0), Some(0.0));
        assert_eq!(c(50.0), Some(50.0));
        assert_eq!(c(100.0), Some(100.0));
        assert_eq!(c(-1.0), Some(0.0), "negatives clamp to 0");
        assert_eq!(c(f32::NEG_INFINITY), Some(0.0));
        assert_eq!(c(150.0), Some(100.0), "over 100 clamps to 100");
        assert_eq!(c(f32::INFINITY), Some(100.0));
        assert_eq!(c(f32::NAN), None, "NaN is unknown, not a confidence");
        assert_eq!(c(f32::EPSILON), Some(f32::EPSILON), "in-scale values are untouched");
    }

    #[test]
    fn text_layer_is_invisible_text_fitted_to_each_box() {
        let w = PlacedWord { text: "Hi".into(), origin: [10.0, 20.0], across: [30.0, 0.0], up: [0.0, 9.3], confidence: None, source: "ocrs".into() };
        let s = String::from_utf8(text_layer(&[w], &TextPlan::default())).unwrap();
        assert!(s.contains("3 Tr") && s.contains("/PCHelv 1 Tf"), "{s}");
        let width = helvetica_width("Hi", 1.0);
        // "Hi" has no descender: its box spans the cap height, from the baseline up.
        assert!(s.contains(&format!("{} 0 0 {} 10 20 Tm (Hi) Tj", num(30.0 / width), num(9.3 / CAP_HEIGHT))), "{s}");
        assert!(s.starts_with("/OCR BMC") && s.ends_with("EMC\n"));
    }

    /// #3029: the layer's invisible rendering mode and font end with the layer. Every operator
    /// that sets them sits inside a `q … Q` pair that the layer closes itself, so content drawn
    /// after it on the page starts from the graphics state it had before.
    #[test]
    fn the_layer_restores_the_graphics_state_it_changes() {
        let w = PlacedWord { text: "Hi".into(), origin: [10.0, 20.0], across: [30.0, 0.0], up: [0.0, 9.3], confidence: None, source: "ocrs".into() };
        let s = String::from_utf8(text_layer(&[w], &TextPlan::default())).unwrap();
        let mut depth = 0_i32;
        for token in s.split_ascii_whitespace() {
            match token {
                "q" => depth += 1,
                "Q" => depth -= 1,
                "Tr" | "Tf" | "Tm" => assert!(depth > 0, "`{token}` outside q … Q in {s:?}"),
                _ => {}
            }
            assert!(depth >= 0, "unbalanced Q in {s:?}");
        }
        assert_eq!(depth, 0, "the layer leaves a q open: {s:?}");
    }

    /// A word box is tight around the ink, so the text's size and baseline come from the letters:
    /// capitals, lowercase without ascenders, descenders.
    #[test]
    fn the_text_is_as_tall_as_the_scanned_word() {
        assert_eq!(ink_extent("DT"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("402337"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("sun"), (X_HEIGHT, 0.0));
        assert_eq!(ink_extent("gap"), (X_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("Gallus.Ze@camtel.cm"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("Corporation"), (CAP_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("--"), (BOX_EM - DESCENT, -DESCENT));
        let w = PlacedWord { text: "gap".into(), origin: [0.0, 100.0], across: [20.0, 0.0], up: [0.0, 7.3], confidence: None, source: "ocrs".into() };
        let s = String::from_utf8(text_layer(&[w], &TextPlan::default())).unwrap();
        let em = 7.3 / (X_HEIGHT + DESCENT);
        assert!(s.contains(&format!(" 0 0 {} 0 {} Tm (gap) Tj", num(em), num(100.0 + em * DESCENT))), "{s}");
    }

    /// A word whose box is not positive, finite geometry never reaches the content stream:
    /// NaN, infinite and zero-sized boxes are dropped, so the layer never carries non-finite
    /// numbers.
    #[test]
    fn text_layer_refuses_non_finite_and_degenerate_boxes() {
        let word = |across: [f64; 2], up: [f64; 2], origin: [f64; 2]| PlacedWord {
            text: "Hi".into(),
            origin,
            across,
            up,
            confidence: Some(90.0),
            source: "tesseract".into(),
        };
        for (why, w) in [
            ("NaN across", word([f64::NAN, 0.0], [0.0, 9.3], [0.0; 2])),
            ("NaN up", word([30.0, 0.0], [f64::NAN, 1.0], [0.0; 2])),
            ("inf up", word([30.0, 0.0], [0.0, f64::INFINITY], [0.0; 2])),
            ("inf origin", word([30.0, 0.0], [0.0, 9.3], [0.0, f64::INFINITY])),
            ("zero across", word([0.0, 0.0], [0.0, 9.3], [0.0; 2])),
            ("NaN origin", word([30.0, 0.0], [0.0, 9.3], [0.0, f64::NAN])),
        ] {
            let s = String::from_utf8(text_layer(&[w], &TextPlan::default())).unwrap();
            assert_eq!(s, "/OCR BMC\nq\nBT\n3 Tr\n/PCHelv 1 Tf\nET\nQ\nEMC\n", "{why}: {s}");
            assert!(!s.contains("NaN") && !s.contains("inf"), "{why}: {s}");
        }
    }

    /// The editable-text layer draws visible text whose horizontal scaling is an explicit
    /// percentage computed from real Helvetica metrics: `Tz` × natural advance = box width.
    #[test]
    fn visible_text_layer_scales_by_a_percentage_of_real_metrics() {
        let w = PlacedWord { text: "Hi".into(), origin: [10.0, 20.0], across: [40.0, 0.0], up: [0.0, 9.3], confidence: None, source: "ocrs".into() };
        let s = String::from_utf8(visible_text_layer(&[w], &TextPlan::default())).unwrap();
        assert!(s.contains("0 Tr"), "visible, not invisible: {s}");
        let size = 9.3 / BOX_EM;
        let natural = helvetica_width("Hi", size);
        let tz = 40.0 / natural * 100.0;
        assert!(s.contains(&format!("{} Tz", num(tz))), "{s}");
        assert!(s.contains("(Hi) Tj"), "{s}");
        assert!(s.starts_with("/OCR BMC") && s.ends_with("EMC\n"));
    }

    /// Degenerate boxes and words with no measurable width are skipped, never divide by zero.
    #[test]
    fn visible_text_layer_skips_degenerate_words() {
        let word = |across: [f64; 2], up: [f64; 2], text: &str| PlacedWord {
            text: text.into(),
            origin: [0.0; 2],
            across,
            up,
            confidence: None,
            source: "ocrs".into(),
        };
        let words = vec![
            word([0.0, 0.0], [0.0, 9.0], "flat"),
            word([10.0, 0.0], [0.0, 0.0], "short"),
            word([f64::NAN, 0.0], [0.0, 9.0], "nan"),
            word([10.0, 0.0], [0.0, 9.0], ""),
            word([10.0, 0.0], [0.0, 9.0], "Hi"),
        ];
        let s = String::from_utf8(visible_text_layer(&words, &TextPlan::default())).unwrap();
        assert_eq!(s.matches("Tj").count(), 1, "only the one drawable word is written: {s}");
    }

    /// A word whose ORIGIN is not finite never reaches the visible layer either: a NaN or
    /// infinite origin would put "NaN"/"inf" into the content stream.
    #[test]
    fn visible_text_layer_refuses_non_finite_origins() {
        let word =
            |origin: [f64; 2]| PlacedWord { text: "Hi".into(), origin, across: [10.0, 0.0], up: [0.0, 9.0], confidence: None, source: "ocrs".into() };
        for (why, origin) in [("NaN x", [f64::NAN, 100.0]), ("inf y", [10.0, f64::INFINITY]), ("NaN both", [f64::NAN; 2])] {
            let s = String::from_utf8(visible_text_layer(&[word(origin)], &TextPlan::default())).unwrap();
            assert!(!s.contains("NaN") && !s.contains("inf"), "{why}: {s}");
            assert_eq!(s.matches("Tj").count(), 0, "{why}: {s}");
        }
    }

    /// After a Type3 word the text state goes back to Helvetica: the Latin words around a
    /// planned word must not inherit its font.
    #[test]
    fn helvetica_is_restored_after_a_planned_word() {
        if pdfcraft_fonts::CRAFT_FONTS.is_empty() {
            eprintln!("skipping: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
            return;
        }
        let word = |text: &str| PlacedWord {
            text: text.into(),
            origin: [10.0, 20.0],
            across: [60.0, 0.0],
            up: [0.0, 9.3],
            confidence: None,
            source: "ocrs".into(),
        };
        let words = [word("Hi"), word("日本"), word("Ho")];
        let mut plan = TextPlan::for_words(["Hi", "日本", "Ho"]);
        for (i, font) in plan.fonts.iter_mut().enumerate() {
            font.name = format!("PCOcr{i}");
        }
        let s = String::from_utf8(text_layer(&words, &plan)).unwrap();
        assert!(s.contains("/PCOcr0 1 Tf"), "{s}");
        // The last Helvetica selection comes after the Type3 run, and the Latin word after it
        // is drawn in Helvetica, not left in the planned font.
        assert!(s.rfind("/PCOcr0 1 Tf").unwrap_or(0) < s.rfind("/PCHelv 1 Tf").unwrap_or(usize::MAX), "{s}");
        // The `Q` closes the `q … Q` scope that keeps `3 Tr` from leaking into later content.
        assert!(s.ends_with("Tm (Ho) Tj\nET\nQ\nEMC\n"), "{s}");
        // The visible layer restores Helvetica with the word's size.
        let v = String::from_utf8(visible_text_layer(&words, &plan)).unwrap();
        assert!(v.rfind("/PCOcr0").unwrap_or(0) < v.rfind("/PCHelv").unwrap_or(usize::MAX), "{v}");
        assert!(v.contains("(Ho) Tj"), "{v}");
    }

    /// Words with characters beyond WinAnsi draw in the plan's Type3 font, one run, with the
    /// same exact box fit as the Helvetica words (the matrix x-axis spans across / advance).
    #[test]
    fn planned_words_draw_in_their_font_with_the_same_exact_fit() {
        if pdfcraft_fonts::CRAFT_FONTS.is_empty() {
            eprintln!("skipping: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
            return;
        }
        let w =
            PlacedWord { text: "日本".into(), origin: [10.0, 20.0], across: [60.0, 0.0], up: [0.0, 9.3], confidence: None, source: "ocrs".into() };
        let mut plan = TextPlan::for_words(["日本"]);
        // The names the registrar (pdfcraft_edit::add_type3_fonts) fills in — this crate
        // cannot reach it, so the test sets them where it reads them.
        for (i, font) in plan.fonts.iter_mut().enumerate() {
            font.name = format!("PCOcr{i}");
        }
        let codes: Vec<u8> = plan.encoding(0).iter().map(|&(_, c)| c).collect();
        assert_eq!(codes.len(), 2, "{codes:?}");
        let total = plan.advance_of(plan.encoding(0));
        let s = String::from_utf8(text_layer(std::slice::from_ref(&w), &plan)).unwrap();
        assert!(s.contains("/PCOcr0 1 Tf"), "{s}");
        assert!(s.contains(&format!("{} 0 0 10 10 22.1 Tm ", num(60.0 / total))), "{s}");
        assert!(s.contains(&format!("Tm {} Tj\n", String::from_utf8_lossy(&pdfcraft_fonts::literal(&codes)))), "{s}");
        // The visible layer: size from the box height, Tz from the Type3's own advance.
        let v = String::from_utf8(visible_text_layer(std::slice::from_ref(&w), &plan)).unwrap();
        let size = 9.3 / BOX_EM;
        assert!(v.contains(&format!("/PCOcr0 {} Tf ", num(size))), "{v}");
        assert!(v.contains(&format!("{} Tz", num(60.0 / (total * size) * 100.0))), "{v}");
    }

    /// Without planned characters (an empty plan: built without craft-fonts, or before the
    /// fonts are registered) the WinAnsi path stands, '?' included.
    #[test]
    fn unplanned_words_fall_back_to_win_ansi_question_marks() {
        let w =
            PlacedWord { text: "日本".into(), origin: [10.0, 20.0], across: [60.0, 0.0], up: [0.0, 9.3], confidence: None, source: "ocrs".into() };
        let s = String::from_utf8(text_layer(&[w], &TextPlan::default())).unwrap();
        assert!(s.contains("(??) Tj"), "{s}");
        assert!(!s.contains("PCOcr"), "{s}");
    }

    /// A character no face has a glyph for becomes the font's '?', still in the run, counted
    /// by the plan.
    #[test]
    fn substitutions_are_written_as_the_font_s_question_mark() {
        if pdfcraft_fonts::CRAFT_FONTS.is_empty() {
            eprintln!("skipping: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
            return;
        }
        let w = PlacedWord {
            text: "日\u{1f4a9}語".into(),
            origin: [0.0, 0.0],
            across: [60.0, 0.0],
            up: [0.0, 9.3],
            confidence: None,
            source: "ocrs".into(),
        };
        let mut plan = TextPlan::for_words(["日\u{1f4a9}語"]);
        for (i, font) in plan.fonts.iter_mut().enumerate() {
            font.name = format!("PCOcr{i}");
        }
        assert_eq!(plan.missing(), 1, "the emoji has no glyph anywhere");
        assert_eq!(plan.encoding(0).len(), 3, "it still gets an encoding: the font's ?");
        let codes: Vec<u8> = plan.encoding(0).iter().map(|&(_, c)| c).collect();
        let s = String::from_utf8(text_layer(&[w], &plan)).unwrap();
        assert!(s.contains(&format!("Tm {} Tj\n", String::from_utf8_lossy(&pdfcraft_fonts::literal(&codes)))), "{s}");
    }

    #[test]
    fn rotated_pages_keep_the_reading_direction() {
        // A page turned 90°: image x runs up the page.
        let to_user = |x: f32, y: f32| [y as f64, x as f64];
        let w = PlacedWord::place(&Word::new("Up", [0.0, 0.0, 50.0, 10.0], "ocrs"), to_user);
        assert_eq!(w.across, [0.0, 50.0]);
        assert_eq!(w.up, [-10.0, 0.0]);
    }

    /// #103: every release package's layout puts the models where the installed app looks.
    /// Mirrors packaging/{windows/package.ps1,macos/package.sh,linux/package.sh,freebsd/package.sh}.
    #[test]
    fn finds_the_models_in_every_release_package_layout() {
        let base = std::env::temp_dir().join(format!("pdfcraft-ocr-layouts-{}", std::process::id()));
        let layouts = [
            ("windows", "PdfCraft", "PdfCraft/models"),
            ("macos", "PdfCraft.app/Contents/MacOS", "PdfCraft.app/Contents/Resources/models"),
            ("linux-deb-rpm", "usr/bin", "usr/share/pdfcraft/models"),
            ("linux-tar", "pdfcraft-1.0.0-linux-x86_64/bin", "pdfcraft-1.0.0-linux-x86_64/share/pdfcraft/models"),
            ("appimage", "PdfCraft.AppDir/usr/bin", "PdfCraft.AppDir/usr/share/pdfcraft/models"),
            ("flatpak", "app/bin", "app/share/pdfcraft/models"),
            ("freebsd", "usr/local/bin", "usr/local/share/pdfcraft/models"),
        ];
        for (name, exe_dir, models_dir) in layouts {
            let root = base.join(name);
            let (exe_dir, models_dir) = (root.join(exe_dir), root.join(models_dir));
            std::fs::create_dir_all(&exe_dir).unwrap();
            std::fs::create_dir_all(&models_dir).unwrap();
            let found = || Models::dirs_beside_exe(&exe_dir).iter().find_map(|d| Models::in_dir(d));
            assert_eq!(found(), None, "{name}: no models yet");
            for file in [DETECTION_MODEL, RECOGNITION_MODEL] {
                std::fs::write(models_dir.join(file), b"model").unwrap();
            }
            let models = found().unwrap_or_else(|| panic!("{name}: models in {} not found", models_dir.display()));
            assert_eq!(models.detection.canonicalize().unwrap(), models_dir.join(DETECTION_MODEL).canonicalize().unwrap(), "{name}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The message tells end users what to do; release builds have no cargo.
    #[test]
    fn the_alphabet_is_the_trained_one() {
        let chars: Vec<char> = ALPHABET.chars().collect();
        assert_eq!(chars.len(), 96);
        // The euro sign follows the ASCII punctuation (class 44; 0 is the CTC blank), not a second E.
        assert_eq!(chars.iter().position(|&c| c == '€'), Some(43));
        assert_eq!(chars.iter().filter(|&&c| c == 'E').count(), 1);
    }

    #[test]
    fn missing_models_message_is_for_end_users() {
        let message = OcrError::NoModels.to_string();
        assert!(!message.contains("cargo") && message.contains("PDFCRAFT_MODELS"), "{message}");
    }

    /// End to end on rendered text, when the models are installed (`cargo xtask models`).
    #[test]
    fn reads_rendered_text() {
        let Some(models) = Models::find() else {
            eprintln!("skipped: OCR models not installed");
            return;
        };
        let ocr = Ocr::load(&models).unwrap();
        let (w, h, px) = test_image::hello();
        let lines = ocr.recognize(&px, w, h).unwrap();
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert!(text.iter().any(|t| t.contains("HELL") && t.contains("WOR")), "{text:?}");
        let word = lines.iter().flat_map(|l| &l.words).find(|w| w.text.contains("HELL")).unwrap();
        assert!(word.rect[0] >= 10.0 && word.rect[0] < 60.0, "{word:?}");
    }

    /// A large page with small print: shrunk whole to the detector's input, its letters are too
    /// small to find; in tiles they are found. The words come back in page pixels, once each,
    /// also where they lie in the overlap of two tiles or across a cut.
    #[test]
    fn finds_small_print_on_a_large_page() {
        let Some(models) = Models::find() else {
            eprintln!("skipped: OCR models not installed");
            return;
        };
        let ocr = Ocr::load(&models).unwrap();
        assert_eq!(ocr.detector_input, Some((600, 800)), "the tiling rule assumes the ocrs detector");
        let (w, h, dot) = (2480u32, 3508u32, DOT);
        // An A4 page at 300 dpi is cut at x = 1240 and y = 1754, its tiles overlapping by 99 and
        // 140 pixels around the cuts. The lines lie: in one tile only; across the vertical cut;
        // half in the overlap left of it; across the horizontal cut.
        let at = [(300, 400), (1170, 1000), (1100, 2500), (300, 1748)];
        let px = test_image::hello_on(w, h, dot, &at);
        let lines = ocr.recognize(&px, w, h).unwrap();
        let words: Vec<&Word> = lines.iter().flat_map(|l| &l.words).collect();
        // Two words per line, each once: none lost, none doubled where tiles overlap. The letters
        // are tiny and blocky, so the text itself may be a letter off ("HFLLO").
        assert_eq!(words.len(), 2 * at.len(), "{lines:?}");
        for (x, y) in at {
            let near = |w: &&&Word| (w.rect[0] - x as f32).abs() < 4.0 * dot as f32 && (w.rect[1] - y as f32).abs() < 4.0 * dot as f32;
            assert_eq!(words.iter().filter(near).count(), 1, "no first word at ({x}, {y}): {lines:?}");
        }
        assert!(words.iter().any(|w| w.text == "HELLO"), "{lines:?}");
    }

    /// The letter size of [`finds_small_print_on_a_large_page`], in pixels per dot of a 5×7 letter.
    const DOT: u32 = 2;

    /// The recognised words name their engine.
    #[test]
    fn ocrs_recognizer_names_its_words() {
        let Ok(rec) = OcrsRecognizer::find() else {
            eprintln!("skipped: OCR models not installed");
            return;
        };
        assert_eq!(rec.id(), "ocrs");
        assert!(rec.languages().iter().any(|(c, n)| c == "en" && n == "English"), "{:?}", rec.languages());
        let (w, h, px) = test_image::hello();
        let image = OcrImage::new(w, h, px).unwrap();
        let lines = rec.recognize(&image, &RecognizeOptions::default()).unwrap();
        let words: Vec<&Word> = lines.iter().flat_map(|l| &l.words).collect();
        assert!(!words.is_empty());
        assert!(words.iter().all(|w| w.source == "ocrs"), "{words:?}");
    }

    /// Hostile images are rejected with errors, never panics: zero sides, sides over the cap,
    /// totals over the area cap (even with legal sides), buffers that do not fit, counts that
    /// overflow.
    #[test]
    fn hostile_image_dimensions_are_rejected_not_panicked_at() {
        assert!(matches!(OcrImage::new(0, 10, vec![]), Err(OcrError::EmptyImage)));
        assert!(matches!(OcrImage::new(10, 0, vec![]), Err(OcrError::EmptyImage)));
        assert!(matches!(OcrImage::new(u32::MAX, u32::MAX, vec![]), Err(OcrError::ImageTooLarge)));
        assert!(matches!(OcrImage::new(MAX_SIDE + 1, 1, vec![0; 4]), Err(OcrError::ImageTooLarge)));
        // 10 000 × 7 000 = 70 MP: legal on both sides, over the 64 MP area cap (no allocation
        // is needed to reject it — the buffer is never inspected).
        assert!(matches!(OcrImage::new(MAX_SIDE, 7000, vec![]), Err(OcrError::ImageTooLarge)));
        // 8 000 × 8 000 = exactly the area cap: accepted shape (the short buffer proves the
        // cap check passed), while one more row is refused.
        assert!(matches!(OcrImage::new(8000, 8000, vec![0; 16]), Err(OcrError::ImageSize { .. })));
        assert!(matches!(OcrImage::new(8000, 8001, vec![0; 16]), Err(OcrError::ImageTooLarge)));
        assert!(matches!(OcrImage::new(2, 2, vec![0; 15]), Err(OcrError::ImageSize { .. })));
        let ok = OcrImage::new(2, 2, vec![255; 16]).unwrap();
        assert_eq!((ok.width(), ok.height()), (2, 2));
        assert_eq!(ok.rgba().len(), 16);
        assert_eq!(ok.gray(5, 5), Some(255), "clamped reads stay inside");
        assert_eq!(ok.gray(-1, -1), Some(255));
    }

    /// The web build loads the models from downloaded bytes; they read the same text.
    #[test]
    fn reads_rendered_text_with_models_from_bytes() {
        let Some(models) = Models::find().filter(|m| m.detection.ends_with(DETECTION_MODEL)) else {
            eprintln!("skipped: ONNX OCR models not installed");
            return;
        };
        let ocr = Ocr::from_bytes(std::fs::read(&models.detection).unwrap(), std::fs::read(&models.recognition).unwrap()).unwrap();
        let rec = OcrsRecognizer::from_ocr(ocr);
        assert!(rec.available(), "a recogniser loaded from bytes needs no model files");
        let (w, h, px) = test_image::hello();
        let lines = rec.recognize(&OcrImage::new(w, h, px).unwrap(), &RecognizeOptions::default()).unwrap();
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert!(text.iter().any(|t| t.contains("HELL") && t.contains("WOR")), "{text:?}");
    }

    /// The web build's workers send lines back as JSON; they arrive unchanged, credited to the
    /// engine the worker ran.
    #[test]
    fn lines_survive_json() {
        let lines = vec![
            Line {
                words: vec![
                    Word::new("Total \"€250\"", [1.5, 2.0, 30.25, 12.0], "ocrs"),
                    Word::new("due", [32.0, 2.0, 50.0, 12.0], "ocrs").with_confidence(87.5),
                ],
            },
            Line::default(),
            Line { words: vec![Word::new("日本語", [0.0, 20.0, 9.0, 30.0], "ocrs")] },
        ];
        assert_eq!(lines_from_json(&lines_to_json(&lines), "ocrs").unwrap(), lines);
        assert_eq!(lines_from_json(&lines_to_json(&[]), "ocrs").unwrap(), Vec::<Line>::new());
        let other = lines_from_json(&lines_to_json(&lines), "tesseract").unwrap();
        assert!(other.iter().flat_map(|l| &l.words).all(|w| w.source == "tesseract"));
    }

    /// A result that isn't what a worker sends is an error, not a crash or a guess.
    #[test]
    fn malformed_json_lines_are_an_error() {
        for json in [
            "",
            "{}",
            "[1]",
            "[[1]]",
            "[[[\"a\", 1, 2, 3]]]",
            "[[[\"a\", 1, 2, 3, 4, 5, 6]]]",
            "[[[1, 1, 2, 3, 4]]]",
            "[[[\"a\", \"1\", 2, 3, 4]]]",
            "[[[\"a\", 1e999, 2, 3, 4]]]",
            "[[[\"a\", null, 2, 3, 4]]]",
            "[[[\"a\", 1, 2, 3, 4, null]]]",
            "[[[\"a\", 1, 2, 3, 4, \"90\"]]]",
        ] {
            assert!(matches!(lines_from_json(json, "ocrs"), Err(OcrError::Recognize(_))), "{json}");
        }
        let many = format!("[[{}]]", vec!["[\"a\",0,0,1,1]"; MAX_JSON_WORDS + 1].join(","));
        assert!(lines_from_json(&many, "ocrs").is_err());
        let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
        assert!(lines_from_json(&deep, "ocrs").is_err());
    }

    /// A download that isn't a model (an HTML error page, a truncated file) is an error, not a
    /// crash or a hang.
    #[test]
    fn models_from_bad_bytes_are_an_error() {
        for (detection, recognition) in [(Vec::new(), Vec::new()), (b"<!doctype html>".to_vec(), b"RTEN".to_vec()), (vec![0xff; 64], vec![0; 64])] {
            assert!(matches!(Ocr::from_bytes(detection, recognition), Err(OcrError::Load(..))));
        }
    }

    /// The hashes [`Ocr::from_bytes`] checks are the ones `cargo xtask models` verifies.
    #[test]
    fn model_hashes_match_attribution() {
        let manifest = include_str!("../../../ATTRIBUTION.toml");
        for (file, sha256) in [(DETECTION_MODEL, DETECTION_MODEL_SHA256), (RECOGNITION_MODEL, RECOGNITION_MODEL_SHA256)] {
            let entry = manifest
                .split("[[fetched]]")
                .find(|e| e.contains(&format!("file = \"{file}\"")))
                .unwrap_or_else(|| panic!("{file} is not in ATTRIBUTION.toml"));
            assert!(entry.lines().any(|l| l == format!("sha256 = \"{sha256}\"")), "{file}: SHA-256 differs from ATTRIBUTION.toml");
        }
    }

    mod test_image {
        /// "HELLO WORLD" drawn with blocky 5×7 letters, 4 pixels per dot, black on white.
        pub fn hello() -> (u32, u32, Vec<u8>) {
            let (dot, x0, y0) = (4u32, 40u32, 40u32);
            let (w, h) = (x0 * 2 + 11 * 6 * dot, y0 * 2 + 7 * dot);
            (w, h, hello_on(w, h, dot, &[(x0, y0)]))
        }

        /// A white `w` × `h` page with "HELLO WORLD" at each of `at` (top left corner), `dot`
        /// pixels per dot of the letters.
        pub fn hello_on(w: u32, h: u32, dot: u32, at: &[(u32, u32)]) -> Vec<u8> {
            const GLYPHS: &[(char, [&str; 7])] = &[
                ('H', ["X...X", "X...X", "X...X", "XXXXX", "X...X", "X...X", "X...X"]),
                ('E', ["XXXXX", "X....", "X....", "XXXX.", "X....", "X....", "XXXXX"]),
                ('L', ["X....", "X....", "X....", "X....", "X....", "X....", "XXXXX"]),
                ('O', [".XXX.", "X...X", "X...X", "X...X", "X...X", "X...X", ".XXX."]),
                ('W', ["X...X", "X...X", "X...X", "X.X.X", "X.X.X", "XX.XX", "X...X"]),
                ('R', ["XXXX.", "X...X", "X...X", "XXXX.", "X.X..", "X..X.", "X...X"]),
                ('D', ["XXXX.", "X...X", "X...X", "X...X", "X...X", "X...X", "XXXX."]),
            ];
            let text = "HELLO WORLD";
            let mut px = vec![255u8; (w * h * 4) as usize];
            for ((i, c), &(x0, y0)) in at.iter().flat_map(|p| text.chars().enumerate().map(move |c| (c, p))) {
                let Some((_, rows)) = GLYPHS.iter().find(|g| g.0 == c) else { continue };
                for (ry, row) in rows.iter().enumerate() {
                    for (rx, b) in row.bytes().enumerate() {
                        if b != b'X' {
                            continue;
                        }
                        for dy in 0..dot {
                            for dx in 0..dot {
                                let x = x0 + (i as u32 * 6 + rx as u32) * dot + dx;
                                let y = y0 + ry as u32 * dot + dy;
                                let o = ((y * w + x) * 4) as usize;
                                px[o..o + 3].copy_from_slice(&[0, 0, 0]);
                            }
                        }
                    }
                }
            }
            px
        }
    }
}
