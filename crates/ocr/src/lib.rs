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
//! Visible editable output uses [`pdfcraft_fonts::TextPlan`] and optional craft-fonts glyph
//! outlines; unsupported visible characters become `?` and are counted. The engine's invisible
//! searchable layer uses empty Type3 programs and ToUnicode maps, preserving Unicode without
//! display fonts. WinAnsi-only invisible text keeps the standard Helvetica layout here.
//!
//! The ocrs models read the Latin alphabet (English and other languages written without
//! accents); Tesseract (the `tesseract` module, off the web build) reads whatever its installed
//! language packs cover.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod confidence;
pub mod merge;
mod ocrs;
pub mod preprocess;
#[cfg(not(target_arch = "wasm32"))]
pub mod tesseract;

pub use confidence::{ConfidenceBand, band_for, is_suspect, low_confidence};
pub use merge::{MergeStrategy, StrategyOutcome, mean_confidence, recognize_with_strategy, rover_merge};
pub use ocrs::{ALPHABET, DETECTION_MODEL, LANGUAGES, Models, Ocr, OcrsRecognizer, RECOGNITION_MODEL};

pub use pdfcraft_fonts::TextPlan;
pub use pdfcraft_fonts::helvetica_width;

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
    /// unknown codes fall back to the engine's default.
    pub language: String,
}

impl Default for RecognizeOptions {
    fn default() -> Self {
        RecognizeOptions { language: "en".into() }
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
    /// Read the words in `image`, in reading order.
    fn recognize(&self, image: &OcrImage, options: &RecognizeOptions) -> Result<Vec<Line>, OcrError>;
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

/// One run of a word's planned characters in one Type3 font: the resource name, the codes to
/// show (one byte per character), and the run's start as an offset in em units from the word's
/// origin (the accumulated advances of the runs before it).
struct PlannedRun {
    font: String,
    codes: Vec<u8>,
    start: f64,
}

/// The planned runs of word `i` (the same index [`pdfcraft_fonts::TextPlan::for_words`] planned
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
        match runs.last_mut() {
            Some(last) if last.font == font => last.codes.push(code),
            _ => runs.push(PlannedRun { font: font.to_string(), codes: vec![code], start: total }),
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
        let size = height / BOX_EM;
        let runs = planned_runs(plan, word_offset.saturating_add(i));
        // The word's natural advance at this size: Helvetica's approximation, or the exact sum
        // of the planned glyphs' widths.
        let natural = runs.as_ref().map_or_else(|| helvetica_width(&w.text, size), |(_, total)| total * size);
        if natural.is_nan() || natural <= 0.0 {
            continue;
        }
        // Horizontal scaling as a percentage of the natural advance at this size, so the word
        // lands exactly on its box (100 = no scaling; a long box on a short word stretches it).
        let tz = (across / natural * 100.0).max(1.0);
        // The baseline sits above the box's bottom edge by the descender depth.
        let o = [w.origin[0] + vx * DESCENT * size, w.origin[1] + vy * DESCENT * size];
        out.extend_from_slice(format!("{} Tz ", num(tz)).as_bytes());
        match &runs {
            None => {
                if current.as_ref().is_some_and(|(f, s)| f != "PCHelv" || *s != size) {
                    out.extend_from_slice(format!("/PCHelv {} Tf ", num(size)).as_bytes());
                    current = Some(("PCHelv".to_string(), size));
                }
                out.extend_from_slice(format!("{} {} {} {} {} {} Tm ", num(ux), num(uy), num(vx), num(vy), num(o[0]), num(o[1])).as_bytes());
                out.extend(pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(&w.text)));
                out.extend_from_slice(b" Tj\n");
            }
            Some((planned, total)) => {
                for run in planned {
                    if current.as_ref().is_some_and(|(f, s)| *f != run.font || *s != size) {
                        out.extend_from_slice(format!("/{} {} Tf ", run.font, num(size)).as_bytes());
                        current = Some((run.font.clone(), size));
                    }
                    // The run's origin: the word's baseline plus the across vector scaled by
                    // the advances before this run (the stretch applies to runs alike).
                    let frac = run.start / total;
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
                    out.extend(pdfcraft_fonts::literal(&run.codes));
                    out.extend_from_slice(b" Tj\n");
                }
            }
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
        if width <= 0.0 || !(height > 0.0 && height.is_finite()) || !(across > 0.0 && across.is_finite()) || !w.origin.iter().all(|v| v.is_finite()) {
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
    }
    out.extend_from_slice(b"ET\nQ\nEMC\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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

    mod test_image {
        /// "HELLO WORLD" drawn with blocky 5×7 letters, 4 pixels per dot, black on white.
        pub fn hello() -> (u32, u32, Vec<u8>) {
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
            let (dot, x0, y0) = (4u32, 40u32, 40u32);
            let (w, h) = (x0 * 2 + text.len() as u32 * 6 * dot, y0 * 2 + 7 * dot);
            let mut px = vec![255u8; (w * h * 4) as usize];
            for (i, c) in text.chars().enumerate() {
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
            (w, h, px)
        }
    }
}
