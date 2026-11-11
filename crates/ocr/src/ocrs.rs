//! The pure-Rust engine: the ocrs crate with its pre-trained models, wrapped as a
//! [`Recognizer`] (`OcrsRecognizer`). Reading is offline and local; the models do not report a
//! confidence, so its words have `confidence: None`.

use std::path::{Path, PathBuf};

use rten_imageproc::{RectF, RotatedRect};

use super::{Line, MAX_PIXELS, MAX_SIDE, OcrError, OcrImage, RecognizeOptions, Recognizer, Word};

/// The validated byte length of a `width` × `height` RGBA raster, with the same caps
/// [`OcrImage::new`] applies: zero sides are empty, sides over [`MAX_SIDE`] or totals over
/// [`MAX_PIXELS`] are too large. `Ocr::recognize` takes raw dimensions (they come from
/// callers, ultimately from documents, so they are untrusted), and the naive
/// `width * height * 4` overflows `u32` for absurd values — the caps are checked first, in
/// `usize`, so nothing overflows and nothing huge is ever touched.
fn checked_len(width: u32, height: u32) -> Result<usize, OcrError> {
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
    pixels.checked_mul(4).ok_or(OcrError::ImageTooLarge)
}

/// The languages the models read (ISO 639-1); all use the Latin alphabet without accents.
pub const LANGUAGES: &[(&str, &str)] = &[("en", "English")];

/// The model files, as named in ATTRIBUTION.toml.
pub const DETECTION_MODEL: &str = "text-detection.onnx";
pub const RECOGNITION_MODEL: &str = "text-recognition.onnx";
/// Their SHA-256s, as in ATTRIBUTION.toml. [`Ocr::from_bytes`] loads nothing else: rten's ONNX
/// parser can spin forever on bytes that aren't a model (64 × 0xff does).
pub const DETECTION_MODEL_SHA256: &str = "a917b23dbd9524b465df7e922641b3ff2981623df4ded5a0234004ef2fee7cfe";
pub const RECOGNITION_MODEL_SHA256: &str = "86c145c2edb96c8caed5b1ebb8f44d706408922211451c309c625157dd6061c5";

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

pub const ALPHABET: &str = " 0123456789!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~€ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Where the two model files are.
#[derive(Clone, Debug, PartialEq)]
pub struct Models {
    pub detection: PathBuf,
    pub recognition: PathBuf,
}

impl Models {
    pub fn dirs_beside_exe(exe_dir: &Path) -> Vec<PathBuf> {
        vec![exe_dir.join("models"), exe_dir.join("../Resources/models"), exe_dir.join("../share/pdfcraft/models")]
    }

    /// The models in `dir`, preferring current ONNX files over legacy RTEN files. An incomplete
    /// update never mixes formats or hides an intact legacy pair.
    pub fn in_dir(dir: &Path) -> Option<Models> {
        [(DETECTION_MODEL, RECOGNITION_MODEL), ("text-detection.rten", "text-recognition.rten")].into_iter().find_map(|(detection, recognition)| {
            let m = Models { detection: dir.join(detection), recognition: dir.join(recognition) };
            (m.detection.is_file() && m.recognition.is_file()).then_some(m)
        })
    }

    /// Look for the models: `$PDFCRAFT_MODELS`, then `models/` beside the executable (and
    /// `Resources/models` in a macOS bundle), then the source tree's `assets/models/`.
    pub fn find() -> Option<Models> {
        Self::search_dirs().iter().find_map(|d| Self::in_dir(d))
    }

    /// The directories [`Models::find`] looks in, in order.
    pub fn search_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(d) = std::env::var_os("PDFCRAFT_MODELS") {
            dirs.push(PathBuf::from(d));
        }
        if let Some(exe) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            dirs.extend(Self::dirs_beside_exe(&exe));
        }
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models"));
        dirs
    }
}

/// How far each found line is widened before its image is cut out for recognition, as fractions
/// of the line's median word height: above and below every word, and before its first and after
/// its last word. ocrs grows each word box by a fixed 3 pixels, a sliver of a letter on a page
/// scanned at 300 dpi or more: the line images clipped the first and last letters ("The" read as
/// "he", "and" as "an", closing full stops lost), and the more so the higher the resolution.
/// Chosen on synthetic pages at 150 to 400 dpi, clean and noisy, and on MDPBench's pages. More
/// padding above and below costs small, noisy text (the recogniser scales each line image to a
/// fixed height, so the margin leaves less height for the letters): at 0.15 the 8 pt "Telefon"
/// label of a noisy form scanned at 150 dpi read "Teleton".
const LINE_PAD_ACROSS: f32 = 0.12;
const LINE_PAD_ALONG: f32 = 0.1;

/// Widen `line` (its word boxes, in reading order) as [`LINE_PAD_ACROSS`] and
/// [`LINE_PAD_ALONG`] say, after the lines were found, so finding them is unchanged. Returns the
/// box the words covered before, which the recognised words are clipped to: the text layer sizes
/// its text from them.
fn pad_line(line: &mut [RotatedRect]) -> Option<RectF> {
    let before = rten_imageproc::bounding_rect(line.iter())?;
    // A box's height runs along its up axis and its width along the line, whichever way the
    // line lies on the page.
    let mut heights: Vec<f32> = line.iter().map(RotatedRect::height).filter(|h| h.is_finite() && *h > 0.0).collect();
    heights.sort_by(f32::total_cmp);
    let height = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
    let last = line.len().saturating_sub(1);
    for (i, r) in line.iter_mut().enumerate() {
        let along = if i == 0 || i == last { 2.0 * LINE_PAD_ALONG * height } else { 0.0 };
        let across = 2.0 * LINE_PAD_ACROSS * height;
        r.resize(r.width() + along, r.height() + across);
    }
    Some(before)
}

/// `rect` ([left, top, right, bottom]) clipped to `to`; unchanged if that leaves nothing: a word
/// wholly in the padding is a letter the line found had cut off, and keeps its own box.
fn clip(rect: [f32; 4], to: Option<RectF>) -> [f32; 4] {
    let Some(to) = to else { return rect };
    let [l, t, r, b] = rect;
    let clipped = [l.max(to.left()), t.max(to.top()), r.min(to.right()), b.min(to.bottom())];
    if clipped[0] < clipped[2] && clipped[1] < clipped[3] { clipped } else { rect }
}

/// A loaded recogniser. Loading takes a moment; keep one and reuse it.
pub struct Ocr {
    engine: ocrs::OcrEngine,
}

impl Ocr {
    pub fn load(models: &Models) -> Result<Ocr, OcrError> {
        let load = |p: &Path| rten::Model::load_file(p).map_err(|e| OcrError::Load(p.display().to_string(), e.to_string()));
        Self::with_models(load(&models.detection)?, load(&models.recognition)?)
    }

    /// Load the models from the contents of their files, where there is no file system (the web
    /// build downloads them). Bytes that aren't the files ATTRIBUTION.toml names are an error.
    pub fn from_bytes(detection: Vec<u8>, recognition: Vec<u8>) -> Result<Ocr, OcrError> {
        let load = |name: &str, sha256: &str, data: Vec<u8>| {
            if sha256_hex(&data) != sha256 {
                return Err(OcrError::Load(name.into(), "not the expected model file (SHA-256 differs)".into()));
            }
            rten::Model::load(data).map_err(|e| OcrError::Load(name.into(), e.to_string()))
        };
        Self::with_models(load(DETECTION_MODEL, DETECTION_MODEL_SHA256, detection)?, load(RECOGNITION_MODEL, RECOGNITION_MODEL_SHA256, recognition)?)
    }

    fn with_models(detection: rten::Model, recognition: rten::Model) -> Result<Ocr, OcrError> {
        let params = ocrs::OcrEngineParams {
            detection_model: Some(detection),
            recognition_model: Some(recognition),
            alphabet: Some(ALPHABET.into()),
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
        if pixels.is_empty() {
            return Err(OcrError::EmptyImage);
        }
        let want = checked_len(width, height)?;
        // ocrs wants 1 or 3 channels.
        let rgb: std::borrow::Cow<[u8]> = if pixels.len() == want {
            pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect::<Vec<u8>>().into()
        } else {
            pixels.into()
        };
        let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
        let source = ocrs::ImageSource::from_bytes(&rgb, (width, height)).map_err(|e| err(&e))?;
        let input = self.engine.prepare_input(source).map_err(|e| err(&e))?;
        let found = self.engine.detect_words(&input).map_err(|e| err(&e))?;
        let mut lines = self.engine.find_text_lines(&input, &found);
        let before: Vec<Option<RectF>> = lines.iter_mut().map(|l| pad_line(l)).collect();
        let read = self.engine.recognize_text(&input, &lines).map_err(|e| err(&e))?;
        use ocrs::TextItem;
        Ok(read
            .into_iter()
            .zip(before)
            .filter_map(|(line, before)| line.map(|l| (l, before)))
            .map(|(line, before)| Line {
                words: line
                    .words()
                    .map(|w| {
                        let r = w.bounding_rect();
                        Word::new(w.to_string(), clip([r.left() as f32, r.top() as f32, r.right() as f32, r.bottom() as f32], before), "ocrs")
                    })
                    .filter(|w| !w.text.trim().is_empty())
                    .collect(),
            })
            .filter(|l| !l.words.is_empty())
            .collect())
    }
}

/// The ocrs engine as a [`Recognizer`] (the default engine).
pub struct OcrsRecognizer {
    engine: Ocr,
    /// Loaded from bytes ([`OcrsRecognizer::from_ocr`]), not from model files: it stays
    /// available without them.
    in_memory: bool,
}

impl OcrsRecognizer {
    /// Load the models from `models`.
    pub fn load(models: &Models) -> Result<OcrsRecognizer, OcrError> {
        Ok(OcrsRecognizer { engine: Ocr::load(models)?, in_memory: false })
    }

    /// A recogniser loaded some other way than from model files: the web build downloads the
    /// models and loads them with [`Ocr::from_bytes`].
    pub fn from_ocr(engine: Ocr) -> OcrsRecognizer {
        OcrsRecognizer { engine, in_memory: true }
    }

    /// Load the models found by [`Models::find`].
    pub fn find() -> Result<OcrsRecognizer, OcrError> {
        Self::load(&Models::find().ok_or(OcrError::NoModels)?)
    }
}

impl Recognizer for OcrsRecognizer {
    fn id(&self) -> &'static str {
        "ocrs"
    }

    fn available(&self) -> bool {
        self.in_memory || Models::find().is_some()
    }

    fn languages(&self) -> Vec<(String, String)> {
        LANGUAGES.iter().map(|(c, n)| ((*c).to_string(), (*n).to_string())).collect()
    }

    fn recognize(&self, image: &OcrImage, _options: &RecognizeOptions) -> Result<Vec<Line>, OcrError> {
        self.engine.recognize(image.rgba(), image.width(), image.height())
    }
}

#[cfg(test)]
mod tests {
    use rten_imageproc::{PointF, Vec2};

    use super::*;

    /// A found line is widened by fractions of its word height: every word above and below, the
    /// first and the last word along the line; the box returned is the line's before.
    #[test]
    fn a_line_is_padded_by_its_word_height() {
        let word = |l: f32, r: f32| RotatedRect::from_rect(RectF::from_tlbr(100.0, l, 140.0, r));
        let mut line = vec![word(10.0, 90.0), word(100.0, 200.0), word(210.0, 260.0)];
        let before = pad_line(&mut line).unwrap();
        assert_eq!([before.left(), before.top(), before.right(), before.bottom()], [10.0, 100.0, 260.0, 140.0]);
        let (across, along) = (2.0 * 0.12 * 40.0, 2.0 * 0.1 * 40.0);
        assert!(line.iter().all(|r| (r.height() - (40.0 + across)).abs() < 1e-3));
        assert!((line[0].width() - (80.0 + along)).abs() < 1e-3 && (line[2].width() - (50.0 + along)).abs() < 1e-3);
        assert!((line[1].width() - 100.0).abs() < 1e-3, "a word inside the line keeps its width");
        // A box on its side (up axis horizontal) keeps its height across the line and its width
        // along it.
        let mut side = vec![RotatedRect::new(PointF::from_yx(0.0, 0.0), Vec2::from_yx(0.0, 1.0), 40.0, 80.0)];
        pad_line(&mut side).unwrap();
        assert!((side[0].height() - (80.0 + 2.0 * 0.12 * 80.0)).abs() < 1e-3 && (side[0].width() - (40.0 + 2.0 * 0.1 * 80.0)).abs() < 1e-3);
        assert!(pad_line(&mut []).is_none());
    }

    #[test]
    fn recognised_words_are_clipped_to_the_line_found() {
        let line = Some(RectF::from_tlbr(100.0, 10.0, 140.0, 260.0));
        assert_eq!(clip([4.0, 94.0, 60.0, 146.0], line), [10.0, 100.0, 60.0, 140.0]);
        // A word wholly in the padding keeps its own box rather than none.
        assert_eq!(clip([262.0, 94.0, 270.0, 146.0], line), [262.0, 94.0, 270.0, 146.0]);
        assert_eq!(clip([1.0, 2.0, 3.0, 4.0], None), [1.0, 2.0, 3.0, 4.0]);
    }

    /// Raw dimensions are untrusted (they come from callers, ultimately from documents): zero
    /// sides, sides or pixel totals over the caps, and products that would overflow the naive
    /// `width * height * 4` are errors, never panics, before anything is touched.
    #[test]
    fn raw_dimensions_are_capped_and_checked_not_panicked_at() {
        assert!(matches!(checked_len(0, 10), Err(OcrError::EmptyImage)));
        assert!(matches!(checked_len(10, 0), Err(OcrError::EmptyImage)));
        assert!(matches!(checked_len(MAX_SIDE + 1, 1), Err(OcrError::ImageTooLarge)));
        // 65536² would wrap u32 in the old `width * height * 4`; the side cap refuses it first.
        assert!(matches!(checked_len(u32::MAX, u32::MAX), Err(OcrError::ImageTooLarge)));
        // Legal sides whose product is over the 64 MP area cap.
        assert!(matches!(checked_len(MAX_SIDE, 7000), Err(OcrError::ImageTooLarge)));
        assert_eq!(checked_len(2, 2).unwrap(), 16);
        // Exactly the area cap fits: 8000 × 8000 = 64 MP.
        assert_eq!(checked_len(8000, 8000).unwrap(), 8000usize * 8000 * 4);
    }

    /// The current ONNX pair wins when an installation also retains both legacy models.
    #[test]
    fn model_discovery_prefers_the_complete_onnx_pair() {
        let dir = tempfile::tempdir().unwrap();
        for file in ["text-detection.onnx", "text-recognition.onnx"] {
            std::fs::write(dir.path().join(file), b"model").unwrap();
        }
        let expected = Some(Models { detection: dir.path().join("text-detection.onnx"), recognition: dir.path().join("text-recognition.onnx") });
        assert_eq!(Models::in_dir(dir.path()), expected, "a complete ONNX pair is sufficient");
        for file in ["text-detection.rten", "text-recognition.rten"] {
            std::fs::write(dir.path().join(file), b"model").unwrap();
        }
        assert_eq!(Models::in_dir(dir.path()), expected, "the complete ONNX pair takes precedence over legacy models");
    }

    /// An interrupted ONNX update still permits an intact legacy pair in the same directory.
    #[test]
    fn model_discovery_falls_back_to_legacy_when_the_onnx_pair_is_incomplete() {
        for current in ["text-detection.onnx", "text-recognition.onnx"] {
            let dir = tempfile::tempdir().unwrap();
            for file in [current, "text-detection.rten", "text-recognition.rten"] {
                std::fs::write(dir.path().join(file), b"model").unwrap();
            }
            assert_eq!(
                Models::in_dir(dir.path()),
                Some(Models { detection: dir.path().join("text-detection.rten"), recognition: dir.path().join("text-recognition.rten") }),
                "{current} alone must not hide a complete legacy pair"
            );
        }
    }

    /// Detection and recognition must come from one complete format pair, never a mixed update.
    #[test]
    fn model_discovery_refuses_mixed_format_pairs() {
        for pair in [["text-detection.onnx", "text-recognition.rten"], ["text-detection.rten", "text-recognition.onnx"]] {
            let dir = tempfile::tempdir().unwrap();
            for file in pair {
                std::fs::write(dir.path().join(file), b"model").unwrap();
            }
            assert_eq!(Models::in_dir(dir.path()), None, "{pair:?} is not a complete model pair");
        }
    }

    /// Existing source builds and PDFCRAFT_MODELS overrides retain their complete RTEN pair.
    #[test]
    fn model_discovery_keeps_legacy_only_installations() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Models::in_dir(dir.path()), None);
        std::fs::write(dir.path().join("text-detection.rten"), b"model").unwrap();
        assert_eq!(Models::in_dir(dir.path()), None, "one legacy model is incomplete");
        std::fs::write(dir.path().join("text-recognition.rten"), b"model").unwrap();
        assert_eq!(
            Models::in_dir(dir.path()),
            Some(Models { detection: dir.path().join("text-detection.rten"), recognition: dir.path().join("text-recognition.rten") })
        );
    }
}
