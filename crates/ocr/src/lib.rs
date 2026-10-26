//! pdfcraft-ocr — Scan & OCR ▸ Recognize text (L4).
//!
//! Recognition runs the ocrs engine (MIT/Apache-2.0) with its pre-trained models (CC-BY-SA-4.0,
//! fetched by `cargo xtask models`; see ATTRIBUTION.toml). The caller renders a page to pixels;
//! [`Ocr::recognize`] finds the words in them, and [`text_layer`] turns words placed in user space
//! into page content: invisible text (rendering mode 3) over each word, so the page becomes a
//! searchable image (Acrobat's "Searchable Image (Exact)": the image is left untouched).
//!
//! The bundled upstream recognition model uses a 96-character alphabet. Accented Western European
//! text (for example, German `ä ö ü ß` and French `é è ê ë à ç œ`) is supported only when the
//! installed recognition model was trained with the matching 182-character [`LATIN_ALPHABET`]
//! (183 output classes including the CTC blank), or a matching custom alphabet sidecar is supplied.
//! Changing an alphabet mapping alone cannot teach new characters to an existing model.
//!
//! The extended alphabet is limited to characters that can be represented by the PDF text layer's
//! standard Helvetica / WinAnsi encoding. It does not cover every Latin-script language; for
//! example, Polish `ł`, Turkish `ı`, Vietnamese tone-mark combinations, and Unicode combining marks
//! require a different alphabet/model and potentially a Unicode-capable PDF text font.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::{Path, PathBuf};

pub use pdfcraft_fonts::helvetica_width;

/// The model files, as named in ATTRIBUTION.toml.
pub const DETECTION_MODEL: &str = "text-detection.rten";
pub const RECOGNITION_MODEL: &str = "text-recognition.rten";
/// Optional custom alphabet file placed alongside `text-recognition.rten`.
pub const ALPHABET_FILE: &str = "text-recognition.alphabet";

/// The base recognition model's 96-character alphabet, as it was trained (ocrs-models'
/// `DEFAULT_ALPHABET`): its output classes follow this order. The ocrs crate's own copy has an `E`
/// where the training had `€`, which made a scanned "€250" read as "E250".
pub const ALPHABET: &str = " 0123456789!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~€ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The extended Western European (WinAnsi / Latin-1 + CP1252) alphabet (182 characters):
/// [`ALPHABET`] (96) followed by 14 punctuation/currency symbols and 72 accented letters
/// (32 uppercase Latin-1 letters, 32 lowercase, and `ŒœŠšŸŽžƒ`), all drawable by standard
/// Helvetica in `WinAnsiEncoding`.
///
/// The order is the model's output-class order, so it must match the alphabet the extended
/// recognition model was trained with. A model with `LATIN_ALPHABET.chars().count() + 1`
/// (183) output classes selects this alphabet; see [`Models::alphabet_for`].
///
/// The first literal intentionally duplicates [`ALPHABET`] because `concat!` only accepts
/// literals. Keep the two in sync; `the_alphabet_is_the_trained_one` verifies the prefix.
pub const LATIN_ALPHABET: &str = concat!(
    " 0123456789!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~€ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
    "¡¢£¥§«¬°±µ¶·»¿ÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏÐÑÒÓÔÕÖ×ØÙÚÛÜÝÞßàáâãäåæçèéêëìíîïðñòóôõö÷øùúûüýþÿŒœŠšŸŽžƒ"
);

/// Languages whose common spellings can be represented by [`LATIN_ALPHABET`] (ISO 639-1).
///
/// This is capability metadata for a compatible extended recognition model, not a claim that the
/// bundled 96-character model recognizes every listed language. The model's output-class count
/// determines which alphabet [`Ocr::load`] selects. Nothing in this crate enforces a language hint.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("da", "Danish"),
    ("no", "Norwegian"),
    ("sv", "Swedish"),
    ("fi", "Finnish"),
    ("is", "Icelandic"),
    ("ca", "Catalan"),
    ("gl", "Galician"),
    ("eu", "Basque"),
    ("af", "Afrikaans"),
    ("id", "Indonesian"),
    ("ms", "Malay"),
    ("sw", "Swahili"),
];

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    /// Shown to end users (release packages ship the models; a source build fetches them with
    /// `cargo xtask models`). The exact string in `#[error(...)]` is a UI catalog key; changing it
    /// requires updating the catalogs as well.
    #[error("the text recognition models are not installed (reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them)")]
    NoModels,
    /// A model or OCR configuration file could not be loaded or validated.
    #[error("loading {0}: {1}")]
    Load(String, String),
    /// The OCR engine rejected the image or failed during recognition.
    #[error("text recognition failed: {0}")]
    Recognize(String),
    /// The supplied image has zero dimensions or no pixel data.
    #[error("the image is empty")]
    EmptyImage,
}

/// Where the model files are.
#[derive(Clone, Debug, PartialEq)]
pub struct Models {
    /// Path to the text-detection model file.
    pub detection: PathBuf,
    /// Path to the text-recognition model file.
    pub recognition: PathBuf,
}

impl Models {
    /// The models in `dir`, if both files are there.
    pub fn in_dir(dir: &Path) -> Option<Models> {
        let m = Models { detection: dir.join(DETECTION_MODEL), recognition: dir.join(RECOGNITION_MODEL) };
        (m.detection.is_file() && m.recognition.is_file()).then_some(m)
    }

    /// Look for the models in the supplied directories, in order.
    ///
    /// This helper also lets tests provide paths without mutating the process-global
    /// `PDFCRAFT_MODELS` environment variable, which can race with parallel tests.
    pub fn find_in(dirs: &[PathBuf]) -> Option<Models> {
        dirs.iter().find_map(|dir| Self::in_dir(dir.as_path()))
    }

    /// Look for the models: `$PDFCRAFT_MODELS`, then where the release packages install them
    /// beside the executable ([`Models::dirs_beside_exe`]), then the source tree's `assets/models/`.
    pub fn find() -> Option<Models> {
        Self::find_in(&Self::search_dirs())
    }

    /// The directories [`Models::find`] looks in, in order. An empty `PDFCRAFT_MODELS` value
    /// is ignored rather than being treated as the current working directory.
    pub fn search_dirs() -> Vec<PathBuf> {
        let override_dir = std::env::var_os("PDFCRAFT_MODELS").map(PathBuf::from);
        Self::search_dirs_with_override(override_dir)
    }

    /// Build the search list with an explicit override. Keeping this logic separate lets tests
    /// exercise precedence without mutating process-global environment variables.
    fn search_dirs_with_override(override_dir: Option<PathBuf>) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(dir) = override_dir {
            if !dir.as_os_str().is_empty() {
                dirs.push(dir);
            }
        }
        if let Some(exe) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            dirs.extend(Self::dirs_beside_exe(&exe));
        }
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models"));
        dirs
    }

    /// Where the release packages put the models, relative to the directory holding the
    /// executable (packaging/*):
    /// - `models/` beside it: the Windows MSI and portable zip;
    /// - `../Resources/models`: the macOS app bundle (`Contents/MacOS` → `Contents/Resources`);
    /// - `../share/pdfcraft/models`: the FHS-style trees (`bin/` → `share/`) of the Linux
    ///   deb/rpm/tar.gz/AppImage/Flatpak and the FreeBSD tarball, wherever they are installed.
    ///
    /// The returned paths may contain `..`; they are not canonicalized because some candidate
    /// paths may not exist. Callers that need a canonical path can use `std::fs::canonicalize`.
    pub fn dirs_beside_exe(exe_dir: &Path) -> Vec<PathBuf> {
        vec![exe_dir.join("models"), exe_dir.join("../Resources/models"), exe_dir.join("../share/pdfcraft/models")]
    }

    /// The alphabet for the recognition model. `output_classes` is the model's last output
    /// dimension, which is `alphabet.chars().count() + 1` (the extra class is the CTC blank), or
    /// `None` when the dimension is symbolic.
    ///
    /// 1. A custom alphabet file (`text-recognition.alphabet`) beside the model wins. It must be
    ///    non-empty, drawable in the text layer, and (when the class count is known) exactly
    ///    `output_classes - 1` characters long.
    /// 2. Otherwise the class count picks [`ALPHABET`] or [`LATIN_ALPHABET`].
    /// 3. A class count that matches neither is an error rather than a silent fallback, because
    ///    a misaligned alphabet reads as garbage.
    /// 4. A symbolic dimension keeps the base [`ALPHABET`].
    pub fn alphabet_for(&self, output_classes: Option<usize>) -> Result<String, OcrError> {
        let sidecar = self.recognition.with_file_name(ALPHABET_FILE);
        let custom = match std::fs::read_to_string(&sidecar) {
            Ok(contents) => Some(contents),
            // A missing sidecar is expected: use the alphabet selected by the model's class count.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            // Do not silently ignore a present but unreadable or invalid-UTF-8 configuration file.
            Err(err) => {
                return Err(OcrError::Load(sidecar.display().to_string(), err.to_string()));
            }
        };

        if let Some(custom) = custom {
            let fail = |msg: String| OcrError::Load(sidecar.display().to_string(), msg);
            let alphabet = custom.trim_end_matches(['\r', '\n']).to_owned();
            if alphabet.is_empty() {
                return Err(fail("the alphabet file is empty".into()));
            }
            if let Some(c) = pdfcraft_fonts::first_undrawable(&alphabet) {
                return Err(fail(format!("{c:?} cannot be drawn in the text layer")));
            }
            let len = alphabet.chars().count();
            let unique_len = alphabet.chars().collect::<std::collections::HashSet<_>>().len();
            if unique_len != len {
                return Err(fail("the alphabet contains duplicate characters".into()));
            }
            if let Some(n) = output_classes {
                if len + 1 != n {
                    return Err(fail(format!("the alphabet has {len} characters but the model has {n} output classes")));
                }
            }
            return Ok(alphabet);
        }

        let base = ALPHABET.chars().count() + 1;
        let latin = LATIN_ALPHABET.chars().count() + 1;
        match output_classes {
            Some(n) if n == base => Ok(ALPHABET.to_owned()),
            Some(n) if n == latin => Ok(LATIN_ALPHABET.to_owned()),
            Some(n) => Err(OcrError::Load(
                self.recognition.display().to_string(),
                format!("the model has {n} output classes; expected {base} or {latin}, or provide {ALPHABET_FILE}"),
            )),
            None => Ok(ALPHABET.to_owned()),
        }
    }
}

/// A recognised word: its text and its box in image pixels `[left, top, right, bottom]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    /// Recognized text for this word.
    pub text: String,
    /// Bounding rectangle in image pixels as `[left, top, right, bottom]`.
    pub rect: [f32; 4],
}

/// A recognised line of words, in reading order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Line {
    /// Words in reading order within this line.
    pub words: Vec<Word>,
}

impl Line {
    /// Join the words in this line with single spaces, preserving their reading order.
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImageLayout {
    Grayscale,
    Rgb,
    Rgba,
}

/// Validate image dimensions and identify the supported packed pixel layout.
/// Keeping this independent of the OCR engine makes malformed-buffer cases testable.
fn validate_image_buffer(
    buffer_len: usize,
    width: u32,
    height: u32,
) -> Result<(usize, ImageLayout), OcrError> {
    let total_px = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| OcrError::Recognize("image dimensions overflow addressable memory".into()))?;
    if total_px == 0 || buffer_len == 0 {
        return Err(OcrError::EmptyImage);
    }

    let expected_rgb = total_px
        .checked_mul(3)
        .ok_or_else(|| OcrError::Recognize("RGB image buffer size overflow".into()))?;
    let expected_rgba = total_px
        .checked_mul(4)
        .ok_or_else(|| OcrError::Recognize("RGBA image buffer size overflow".into()))?;
    let layout = if buffer_len == total_px {
        ImageLayout::Grayscale
    } else if buffer_len == expected_rgb {
        ImageLayout::Rgb
    } else if buffer_len == expected_rgba {
        ImageLayout::Rgba
    } else {
        return Err(OcrError::Recognize(format!(
            "invalid image buffer length: got {buffer_len} bytes for {width}x{height}; expected {total_px} (grayscale), {expected_rgb} (RGB), or {expected_rgba} (RGBA)"
        )));
    };

    Ok((total_px, layout))
}

/// A loaded recogniser. Loading takes a moment; keep one and reuse it.
pub struct Ocr {
    engine: ocrs::OcrEngine,
}

impl Ocr {
    /// Load the detection and recognition models and select an alphabet compatible with the
    /// recognition model's output-class count or adjacent custom alphabet sidecar.
    pub fn load(models: &Models) -> Result<Ocr, OcrError> {
        let load = |p: &Path| rten::Model::load_file(p).map_err(|e| OcrError::Load(p.display().to_string(), e.to_string()));
        let detection = load(&models.detection)?;
        let recognition = load(&models.recognition)?;

        // Inspect the recognition model's final dimension (CTC classes = alphabet_len + 1) so
        // both the 96-char base model and the extended Western European model work seamlessly.
        let output_classes = recognition
            .output_ids()
            .first()
            .and_then(|&id| recognition.node_info(id))
            .and_then(|info| info.shape())
            .and_then(|dims| dims.last().copied())
            .and_then(|dim| match dim {
                rten::Dimension::Fixed(n) => Some(n),
                rten::Dimension::Symbolic(_) => None,
            });

        let alphabet = models.alphabet_for(output_classes)?;
        let params = ocrs::OcrEngineParams {
            detection_model: Some(detection),
            recognition_model: Some(recognition),
            alphabet: Some(alphabet),
            ..Default::default()
        };
        let engine = ocrs::OcrEngine::new(params).map_err(|e| OcrError::Load("ocr engine".into(), e.to_string()))?;
        Ok(Ocr { engine })
    }

    /// Load the models found by [`Models::find`].
    pub fn find() -> Result<Ocr, OcrError> {
        Self::load(&Models::find().ok_or(OcrError::NoModels)?)
    }

    /// Recognise the text in an RGBA (or RGB, or grey) image, `width` × `height` pixels.
    pub fn recognize(&self, pixels: &[u8], width: u32, height: u32) -> Result<Vec<Line>, OcrError> {
        let (_, layout) = validate_image_buffer(pixels.len(), width, height)?;

        // ocrs wants 1 or 3 channels. Composite RGBA onto white so transparent backgrounds stay white.
        let rgb: std::borrow::Cow<[u8]> = if layout == ImageLayout::Rgba {
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[r, g, b, a]| {
                    let blend = |c: u8| (255 - ((255 - u16::from(c)) * u16::from(a)) / 255) as u8;
                    [blend(r), blend(g), blend(b)]
                })
                .collect::<Vec<u8>>()
                .into()
        } else {
            pixels.into()
        };
        let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
        let source = ocrs::ImageSource::from_bytes(&rgb, (width, height)).map_err(|e| err(&e))?;
        let input = self.engine.prepare_input(source).map_err(|e| err(&e))?;
        let found = self.engine.detect_words(&input).map_err(|e| err(&e))?;
        let lines = self.engine.find_text_lines(&input, &found);
        let read = self.engine.recognize_text(&input, &lines).map_err(|e| err(&e))?;
        use ocrs::TextItem;
        Ok(read
            .into_iter()
            .flatten()
            .map(|line| Line {
                words: line
                    .words()
                    .map(|w| {
                        let r = w.bounding_rect();
                        Word { text: w.to_string(), rect: [r.left() as f32, r.top() as f32, r.right() as f32, r.bottom() as f32] }
                    })
                    .filter(|w| !w.text.trim().is_empty())
                    .collect(),
            })
            .filter(|l| !l.words.is_empty())
            .collect())
    }
}

/// A word placed on a page, in PDF user space: the bottom-left corner of its box, and the
/// vectors along its bottom edge (`across`, left → right as read) and its left edge (`up`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedWord {
    /// Word text to write into the invisible PDF text layer.
    pub text: String,
    /// Bottom-left origin of the word in PDF user-space coordinates.
    pub origin: [f64; 2],
    /// Vector from the left to the right side of the word, in PDF user space.
    pub across: [f64; 2],
    /// Vector from the bottom to the top side of the word, in PDF user space.
    pub up: [f64; 2],
}

impl PlacedWord {
    /// Place a pixel-space word with `to_user`, which maps an image pixel (x right, y down) to
    /// user space.
    pub fn place(word: &Word, to_user: impl Fn(f32, f32) -> [f64; 2]) -> PlacedWord {
        let [l, t, r, b] = word.rect;
        let o = to_user(l, b);
        let br = to_user(r, b);
        let tl = to_user(l, t);
        PlacedWord { text: word.text.clone(), origin: o, across: [br[0] - o[0], br[1] - o[1]], up: [tl[0] - o[0], tl[1] - o[1]] }
    }
}

/// Helvetica's ascender and descender (em fractions): a word box spans about this much.
const BOX_EM: f64 = 0.93;
const DESCENT: f64 = 0.21;
/// Helvetica's cap height and x-height (em fractions).
const CAP_HEIGHT: f64 = 0.718;
const X_HEIGHT: f64 = 0.523;

/// Accented or ascending lowercase letters whose ink reaches the cap/ascender line. Listed
/// explicitly: `æ œ ø µ ç` are x-height letters and are deliberately absent.
const TALL_LOWER_ACCENTED: &str = "àáâãäåèéêëìíîïñòóôõöùúûüýÿšžßðþƒ";

/// True if `c` rises to or near the cap/ascender line in Helvetica.
fn is_tall_glyph(c: char) -> bool {
    c.is_uppercase()
        || c.is_ascii_digit()
        || "bdfhijklt!?/\\()[]{}|'\"#$%&@€£¥§¶¡¿".contains(c)
        || TALL_LOWER_ACCENTED.contains(c)
}

/// True if `c` reaches below the baseline (descenders, cedillas, brackets, inverted Spanish
/// punctuation).
fn is_low_glyph(c: char) -> bool {
    "gjpqyçÇýÿþƒµ()[]{}|_¡¿§¶".contains(c)
}

/// How far `text`'s ink reaches above and below the baseline, in em fractions: a word box is
/// tight around the ink, so "DT" spans the cap height only, "sun" the x-height, and "gap" or
/// "reçu" reaches down to the descender. Text without letters or digits keeps the full
/// ascender-descender span.
fn ink_extent(text: &str) -> (f64, f64) {
    if !text.chars().any(char::is_alphanumeric) {
        return (BOX_EM - DESCENT, -DESCENT);
    }
    let tall = text.chars().any(is_tall_glyph);
    let low = text.chars().any(is_low_glyph);
    (if tall { CAP_HEIGHT } else { X_HEIGHT }, if low { -DESCENT } else { 0.0 })
}

/// Format a finite PDF numeric operand compactly, avoiding scientific notation and negative zero.
fn num(v: f64) -> String {
    let s = format!("{:.4}", if v.abs() < 1e-9 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Page content that writes `words` as invisible text in `/PCHelv` (standard Helvetica,
/// WinAnsiEncoding), each word stretched to its box so selection and search highlight the
/// right place. Marked content `/OCR` so it can be told apart from the page's own text.
pub fn text_layer(words: &[PlacedWord]) -> Vec<u8> {
    // `3 Tr` and the font are graphics state, which `ET` doesn't reset: without `q … Q` they
    // would stay set for every later content stream on the page, so text added after OCR (a
    // header or footer) drew invisibly (#3029).
    let mut out = b"/OCR BMC\nq\nBT\n3 Tr\n/PCHelv 1 Tf\n".to_vec();
    for w in words {
        // Public fields allow callers to construct invalid placements. Never emit NaN or infinity
        // as a PDF numeric operand; skip malformed words because this API returns raw content bytes.
        if !w
            .origin
            .iter()
            .chain(w.across.iter())
            .chain(w.up.iter())
            .all(|value| value.is_finite())
        {
            continue;
        }

        let width = helvetica_width(&w.text, 1.0);
        let height = w.up[0].hypot(w.up[1]);
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            continue;
        }
        // The em square: the word's own ink (cap height or x-height, descenders or not) spans the
        // box, so the text is as tall as the scanned word; its width is stretched to fit the word.
        let (top, bottom) = ink_extent(&w.text);
        let em = [w.up[0] / (top - bottom), w.up[1] / (top - bottom)];
        let ax = [w.across[0] / width, w.across[1] / width];
        let o = [w.origin[0] - em[0] * bottom, w.origin[1] - em[1] * bottom];
        let m = [ax[0], ax[1], em[0], em[1], o[0], o[1]];
        // Finite inputs can still overflow during arithmetic, so check the final text matrix too.
        if !m.iter().all(|value| value.is_finite()) {
            continue;
        }
        let nums: Vec<String> = m.iter().map(|v| num(*v)).collect();
        out.extend_from_slice(nums.join(" ").as_bytes());
        out.extend_from_slice(b" Tm ");
        out.extend(pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(&w.text)));
        out.extend_from_slice(b" Tj\n");
    }
    out.extend_from_slice(b"ET\nQ\nEMC\n");
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Scratch paths include a per-test label because tests run in parallel and share a process ID.
    /// The directory is removed on drop, including when an assertion fails.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        /// Create a clean temporary directory uniquely named for the current test label.
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "pdfcraft-ocr-{label}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create scratch dir");
            Self(dir)
        }

        /// Return the scratch directory's path.
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        /// Remove the temporary directory when the guard leaves scope.
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Create placeholder model files for model-path discovery tests.
    fn write_models(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        for file in [DETECTION_MODEL, RECOGNITION_MODEL] {
            std::fs::write(dir.join(file), b"model").unwrap();
        }
    }

    /// Check that tightly packed grayscale, RGB, and RGBA images are accepted.
    #[test]
    fn image_buffer_validation_accepts_gray_rgb_and_rgba() {
        assert_eq!(validate_image_buffer(6, 3, 2).unwrap(), (6, ImageLayout::Grayscale));
        assert_eq!(validate_image_buffer(18, 3, 2).unwrap(), (6, ImageLayout::Rgb));
        assert_eq!(validate_image_buffer(24, 3, 2).unwrap(), (6, ImageLayout::Rgba));
    }

    /// Check that empty, mismatched, and overflowing image buffers are rejected.
    #[test]
    fn image_buffer_validation_rejects_empty_or_malformed_buffers() {
        assert!(matches!(validate_image_buffer(0, 3, 2), Err(OcrError::EmptyImage)));
        assert!(matches!(validate_image_buffer(6, 0, 2), Err(OcrError::EmptyImage)));
        assert!(matches!(validate_image_buffer(7, 3, 2), Err(OcrError::Recognize(_))));
        assert!(matches!(validate_image_buffer(usize::MAX, u32::MAX, u32::MAX), Err(OcrError::Recognize(_))));
    }

    /// Verify image-pixel coordinates are mapped correctly into PDF user space.
    #[test]
    fn places_words_through_the_mapping() {
        // 2 pixels per point, page 612 × 792, y flipped.
        let to_user = |x: f32, y: f32| [x as f64 / 2.0, 792.0 - y as f64 / 2.0];
        let w = PlacedWord::place(
            &Word { text: "Hello".into(), rect: [100.0, 200.0, 300.0, 240.0] },
            to_user,
        );
        assert_eq!(w.origin, [50.0, 672.0]);
        assert_eq!(w.across, [100.0, 0.0]);
        assert_eq!(w.up, [0.0, 20.0]);
    }

    /// Verify all built-in model alphabets are drawable by the PDF text-layer font.
    #[test]
    fn model_alphabets_fit_the_text_layer_font() {
        assert_eq!(pdfcraft_fonts::first_undrawable(ALPHABET), None);
        assert_eq!(pdfcraft_fonts::first_undrawable(LATIN_ALPHABET), None);
        for sample in [
            "Münchner Straße",        // German: ü, ß
            "français déjà vu, cœur", // French: ç, é, à, œ
            "¡Qué año!",               // Spanish: ¿, ¡, é, ñ
            "Smørrebrød",              // Danish/Norwegian: ø
            "Ærø",                     // Nordic: Æ, ø
            "Þingvellir",              // Icelandic: Þ, þ
        ] {
            assert!(
                sample.chars().all(|ch| LATIN_ALPHABET.contains(ch)),
                "extended alphabet cannot represent sample {sample:?}"
            );
            assert_eq!(pdfcraft_fonts::first_undrawable(sample), None);
        }
    }

    /// Verify German, French, Spanish, and Nordic text retains valid sizing and WinAnsi encoding.
    #[test]
    fn western_european_words_encode_and_size_accurately() {
        assert_eq!(ink_extent("reçu"), (X_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("garçon"), (X_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("été"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("größe"), (CAP_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("español"), (CAP_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("¿qué?"), (CAP_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("æon"), (X_HEIGHT, 0.0));
        assert_eq!(ink_extent("œuvre"), (X_HEIGHT, 0.0));
        assert_eq!(ink_extent("ø"), (X_HEIGHT, 0.0));
        assert_eq!(ink_extent("µ"), (X_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("ý"), (CAP_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("þ"), (CAP_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("Ça"), (CAP_HEIGHT, -DESCENT));

        let encoded = pdfcraft_fonts::win_ansi("üß");
        assert_eq!(&encoded[..], &[0xFC, 0xDF][..]);

        let text = "Münchner Straße";
        let w = PlacedWord {
            text: text.into(),
            origin: [10.0, 20.0],
            across: [80.0, 0.0],
            up: [0.0, 10.0],
        };
        let layer = text_layer(&[w]);
        assert!(layer.starts_with(b"/OCR BMC\nq\nBT\n3 Tr\n/PCHelv 1 Tf\n"));
        assert!(layer.ends_with(b"ET\nQ\nEMC\n"));
        assert!(helvetica_width(text, 1.0) > 0.0);
        let literal = pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(text));
        assert!(layer.windows(literal.len()).any(|w| w == &literal[..]));

        // Check a French phrase as well: the invisible PDF text must preserve accented letters
        // and the œ ligature instead of replacing them with question marks.
        let french_text = "français déjà vu, cœur";
        let french_word = PlacedWord {
            text: french_text.into(),
            origin: [10.0, 40.0],
            across: [120.0, 0.0],
            up: [0.0, 10.0],
        };
        let french_layer = text_layer(&[french_word]);
        let french_literal = pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(french_text));
        assert!(french_layer.windows(french_literal.len()).any(|w| w == &french_literal[..]));
    }

    /// Verify invisible PDF text is scaled and placed inside its word rectangle.
    #[test]
    fn text_layer_is_invisible_text_fitted_to_each_box() {
        let w = PlacedWord {
            text: "Hi".into(),
            origin: [10.0, 20.0],
            across: [30.0, 0.0],
            up: [0.0, 9.3],
        };
        let s = String::from_utf8(text_layer(&[w])).unwrap();
        assert!(s.contains("3 Tr") && s.contains("/PCHelv 1 Tf"), "{s}");
        let width = helvetica_width("Hi", 1.0);
        assert!(s.contains(&format!(
            "{} 0 0 {} 10 20 Tm (Hi) Tj",
            num(30.0 / width),
            num(9.3 / CAP_HEIGHT)
        )), "{s}");
        assert!(s.starts_with("/OCR BMC") && s.ends_with("EMC\n"));
    }

    /// Verify NaN and infinite placements are omitted from generated PDF operators.
    #[test]
    fn invalid_coordinates_are_not_written_to_pdf_content() {
        let invalid = PlacedWord {
            text: "Hi".into(),
            origin: [f64::NAN, 20.0],
            across: [30.0, 0.0],
            up: [0.0, 9.3],
        };
        let layer = String::from_utf8(text_layer(&[invalid])).unwrap();
        assert!(!layer.contains("NaN"));
        assert!(!layer.contains("inf"));
        assert!(!layer.contains(" Tj "));
        assert!(layer.starts_with("/OCR BMC"));
        assert!(layer.ends_with("EMC\n"));
    }

    /// Verify the OCR text layer balances its graphics-state and marked-content operators.
    #[test]
    fn the_layer_restores_the_graphics_state_it_changes() {
        let w = PlacedWord {
            text: "Hi".into(),
            origin: [10.0, 20.0],
            across: [30.0, 0.0],
            up: [0.0, 9.3],
        };
        let s = String::from_utf8(text_layer(&[w])).unwrap();
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

    /// Verify measured text height follows capitals, x-height letters, and descenders.
    #[test]
    fn text_height_tracks_caps_x_height_and_descenders() {
        assert_eq!(ink_extent("DT"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("402337"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("sun"), (X_HEIGHT, 0.0));
        assert_eq!(ink_extent("gap"), (X_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("Gallus.Ze@camtel.cm"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("Corporation"), (CAP_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("--"), (BOX_EM - DESCENT, -DESCENT));
        let w = PlacedWord {
            text: "gap".into(),
            origin: [0.0, 100.0],
            across: [20.0, 0.0],
            up: [0.0, 7.3],
        };
        let s = String::from_utf8(text_layer(&[w])).unwrap();
        let em = 7.3 / (X_HEIGHT + DESCENT);
        assert!(s.contains(&format!(
            " 0 0 {} 0 {} Tm (gap) Tj",
            num(em),
            num(100.0 + em * DESCENT)
        )), "{s}");
    }

    /// Verify rotated-page mappings preserve the recognized word reading direction.
    #[test]
    fn rotated_pages_keep_the_reading_direction() {
        let to_user = |x: f32, y: f32| [y as f64, x as f64];
        let w = PlacedWord::place(
            &Word { text: "Up".into(), rect: [0.0, 0.0, 50.0, 10.0] },
            to_user,
        );
        assert_eq!(w.across, [0.0, 50.0]);
        assert_eq!(w.up, [-10.0, 0.0]);
    }

    /// Verify explicit model directories are searched in priority order.
    #[test]
    fn finds_models_in_known_locations_and_in_directory_order() {
        let scratch = ScratchDir::new("model-discovery");
        let first = scratch.path().join("first");
        let second = scratch.path().join("second");
        write_models(&first);
        write_models(&second);

        let from_dir = Models::in_dir(&first).unwrap();
        assert_eq!(from_dir.detection, first.join(DETECTION_MODEL));
        assert_eq!(from_dir.recognition, first.join(RECOGNITION_MODEL));

        let found = Models::find_in(&[second.clone(), first.clone()]).unwrap();
        assert_eq!(found.detection, second.join(DETECTION_MODEL));
        assert_eq!(found.recognition, second.join(RECOGNITION_MODEL));

        // Exercise PDFCRAFT_MODELS precedence through the pure helper rather than mutating the
        // process environment. set_var/remove_var can be unsound in a multithreaded Unix process.
        let dirs = Models::search_dirs_with_override(Some(first.clone()));
        assert_eq!(dirs.first(), Some(&first));
        let dirs = Models::search_dirs_with_override(Some(PathBuf::new()));
        assert_ne!(dirs.first(), Some(&PathBuf::new()));
    }

    /// Verify model discovery matches all documented release package layouts.
    #[test]
    fn finds_models_in_every_release_package_layout() {
        let scratch = ScratchDir::new("package-layouts");
        let base = scratch.path();
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
            let exe_dir = root.join(exe_dir);
            let models_dir = root.join(models_dir);
            std::fs::create_dir_all(&exe_dir).unwrap();
            assert_eq!(
                Models::dirs_beside_exe(&exe_dir).iter().find_map(|d| Models::in_dir(d)),
                None,
                "{name}: no models yet"
            );
            write_models(&models_dir);
            let found = Models::dirs_beside_exe(&exe_dir)
                .iter()
                .find_map(|d| Models::in_dir(d))
                .unwrap_or_else(|| panic!("{name}: models in {} not found", models_dir.display()));
            assert_eq!(
                found.detection.canonicalize().unwrap(),
                models_dir.join(DETECTION_MODEL).canonicalize().unwrap(),
                "{name}"
            );
        }
    }

    /// Verify built-in alphabet lengths, order, and uniqueness match trained output classes.
    #[test]
    fn the_alphabet_is_the_trained_one() {
        let chars: Vec<char> = ALPHABET.chars().collect();
        assert_eq!(chars.len(), 96);
        assert_eq!(chars.iter().position(|&c| c == '€'), Some(43));
        assert_eq!(chars.iter().filter(|&&c| c == 'E').count(), 1);

        let latin_chars: Vec<char> = LATIN_ALPHABET.chars().collect();
        assert_eq!(latin_chars.len(), 182);
        assert!(LATIN_ALPHABET.starts_with(ALPHABET));
        let mut seen = std::collections::HashSet::new();
        assert!(latin_chars.iter().all(|c| seen.insert(*c)), "duplicate character in LATIN_ALPHABET");
    }

    /// Verify output-class counts and custom alphabet files select or reject alphabets correctly.
    #[test]
    fn alphabet_for_follows_the_model_class_count() {
        let scratch = ScratchDir::new("alphabet");
        let dir = scratch.path();
        let models = Models {
            detection: dir.join(DETECTION_MODEL),
            recognition: dir.join(RECOGNITION_MODEL),
        };

        assert_eq!(models.alphabet_for(Some(97)).unwrap(), ALPHABET);
        assert_eq!(models.alphabet_for(Some(183)).unwrap(), LATIN_ALPHABET);
        assert_eq!(models.alphabet_for(None).unwrap(), ALPHABET);
        assert!(models.alphabet_for(Some(169)).is_err(), "unknown class count must not fall back silently");
        assert!(models.alphabet_for(Some(0)).is_err(), "zero output classes must be rejected");

        let sidecar = dir.join(ALPHABET_FILE);
        std::fs::write(&sidecar, "abc\n").unwrap();
        assert_eq!(models.alphabet_for(Some(4)).unwrap(), "abc");
        std::fs::write(&sidecar, "abca\n").unwrap();
        assert!(models.alphabet_for(Some(5)).is_err(), "duplicate characters must be rejected");
        std::fs::write(&sidecar, "abc\n").unwrap();
        assert!(models.alphabet_for(Some(5)).is_err(), "class count mismatch must be rejected");
        std::fs::write(&sidecar, "ab中\n").unwrap();
        assert!(models.alphabet_for(Some(4)).is_err(), "undrawable character must be rejected");
        std::fs::write(&sidecar, "\n").unwrap();
        assert!(models.alphabet_for(None).is_err(), "empty alphabet must be rejected");

        std::fs::write(&sidecar, [0xFF]).unwrap();
        assert!(models.alphabet_for(None).is_err(), "invalid UTF-8 must be reported");
        std::fs::remove_file(&sidecar).unwrap();
        std::fs::create_dir(&sidecar).unwrap();
        assert!(models.alphabet_for(None).is_err(), "sidecar read errors must be reported");
    }

    /// Verify missing-model errors give actionable guidance without build-tool references.
    #[test]
    fn missing_models_message_is_for_end_users() {
        let message = OcrError::NoModels.to_string();
        let lower = message.to_lowercase();
        assert!(!lower.contains("cargo") && !lower.contains("xtask"), "message mentions build tools: {message}");
        assert!(message.contains("PDFCRAFT_MODELS"), "message must name the environment override: {message}");
    }

    /// Run an end-to-end OCR smoke test when model assets are installed.
    ///
    /// In CI, set `PDFCRAFT_REQUIRE_OCR_MODELS=1` to fail instead of silently skipping. This
    /// ASCII-only smoke test does not establish an accuracy percentage for accented text.
    #[test]
    fn reads_rendered_text() {
        let Some(models) = Models::find() else {
            let required = std::env::var_os("PDFCRAFT_REQUIRE_OCR_MODELS")
                .map_or(false, |value| !value.is_empty());
            if required {
                panic!("OCR models are required for this test but were not found");
            }
            eprintln!(
                "skipped: OCR models not installed (set PDFCRAFT_REQUIRE_OCR_MODELS=1 in CI to require them)"
            );
            return;
        };

        let ocr = Ocr::load(&models).unwrap();
        let (w, h, px) = test_image::hello();
        let lines = ocr.recognize(&px, w, h).unwrap();
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert!(text.iter().any(|t| t.contains("HELL") && t.contains("WOR")), "{text:?}");
        let word = lines
            .iter()
            .flat_map(|line| &line.words)
            .find(|word| word.text.contains("HELL"))
            .expect("no word containing 'HELL' in OCR output");
        let x0 = test_image::X0 as f32;
        assert!(
            word.rect[0] >= x0 - 30.0 && word.rect[0] < x0 + 20.0,
            "first word should start near x={x0}, got {word:?}"
        );
    }

    mod test_image {
        pub const X0: u32 = 40;

        /// Render `HELLO WORLD` as blocky 5×7 letters, 4 pixels per dot, black on white RGBA.
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
            let (dot, y0) = (4u32, 40u32);
            let glyph_count = text.chars().count() as u32;
            let (w, h) = (X0 * 2 + glyph_count * 6 * dot, y0 * 2 + 7 * dot);
            let mut px = vec![255u8; (w as usize) * (h as usize) * 4];
            for (i, c) in text.chars().enumerate() {
                if c == ' ' {
                    continue;
                }
                let (_, rows) = GLYPHS
                    .iter()
                    .find(|glyph| glyph.0 == c)
                    .unwrap_or_else(|| panic!("no glyph for {c:?}"));
                for (ry, row) in rows.iter().enumerate() {
                    for (rx, byte) in row.bytes().enumerate() {
                        if byte != b'X' {
                            continue;
                        }
                        for dy in 0..dot {
                            for dx in 0..dot {
                                let x = X0 + (i as u32 * 6 + rx as u32) * dot + dx;
                                let y = y0 + ry as u32 * dot + dy;
                                let offset = ((y as usize) * (w as usize) + (x as usize)) * 4;
                                px[offset..offset + 3].copy_from_slice(&[0, 0, 0]);
                            }
                        }
                    }
                }
            }
            (w, h, px)
        }
    }
}
