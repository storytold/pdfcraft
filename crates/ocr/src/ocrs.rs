//! The pure-Rust engine: the ocrs crate with its pre-trained models, wrapped as a
//! [`Recognizer`] (`OcrsRecognizer`). Reading is offline and local. English is read by the ocrs
//! recognition model, which reports no confidence (`confidence: None`); the other languages by
//! PaddleOCR's multilingual recognizer over the lines ocrs detected, with its mean character
//! confidence (0–100).

use std::path::{Path, PathBuf};

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

/// Language choices (BCP 47 tags); see README.md for validation limits.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("de", "Deutsch"),
    ("ja", "日本語"),
    ("fr", "Français"),
    ("es", "Español"),
    ("ca", "Català"),
    ("it", "Italiano"),
    ("nl", "Nederlands"),
    ("da", "Dansk"),
    ("nb", "Norsk bokmål"),
    ("sv", "Svenska"),
    ("fi", "Suomi"),
    ("pt", "Português"),
    ("zh-Hant", "繁體中文"),
    ("zh-Hans", "简体中文"),
];

/// The model files, as named in ATTRIBUTION.toml.
pub const DETECTION_MODEL: &str = "text-detection.onnx";
pub const RECOGNITION_MODEL: &str = "text-recognition.onnx";
pub const MULTILINGUAL_MODEL: &str = "multilingual-recognition.onnx";
pub const ORIENTATION_MODEL: &str = "document-orientation.onnx";
pub const DICTIONARY: &str = "multilingual-dictionary.txt";

pub const ALPHABET: &str = " 0123456789!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~€ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Where the detector and English recognizer are; the multilingual assets live alongside them.
#[derive(Clone, Debug, PartialEq)]
pub struct Models {
    pub detection: PathBuf,
    pub recognition: PathBuf,
}

impl Models {
    pub fn dirs_beside_exe(exe_dir: &Path) -> Vec<PathBuf> {
        vec![exe_dir.join("models"), exe_dir.join("../Resources/models"), exe_dir.join("../share/pdfcraft/models")]
    }

    /// A complete model set in `dir`: the detection and recognition pair, preferring current ONNX
    /// files over legacy RTEN files (an incomplete update never mixes formats or hides an intact
    /// legacy pair), and the multilingual recognition, orientation and dictionary files.
    pub fn in_dir(dir: &Path) -> Option<Models> {
        if ![MULTILINGUAL_MODEL, ORIENTATION_MODEL, DICTIONARY].iter().all(|f| dir.join(f).is_file()) {
            return None;
        }
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

/// A loaded recogniser. Loading takes a moment; keep one and reuse it.
pub struct Ocr {
    engine: ocrs::OcrEngine,
    multilingual: super::multilingual::Multilingual,
}

impl Ocr {
    pub fn load(models: &Models) -> Result<Ocr, OcrError> {
        let load = |p: &Path| rten::Model::load_file(p).map_err(|e| OcrError::Load(p.display().to_string(), e.to_string()));
        let params = ocrs::OcrEngineParams {
            detection_model: Some(load(&models.detection)?),
            recognition_model: Some(load(&models.recognition)?),
            alphabet: Some(ALPHABET.into()),
            ..Default::default()
        };
        let engine = ocrs::OcrEngine::new(params).map_err(|e| OcrError::Load("ocr engine".into(), e.to_string()))?;
        let dir = models.recognition.parent().ok_or(OcrError::NoModels)?;
        Ok(Ocr { engine, multilingual: super::multilingual::Multilingual::load(dir)? })
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
                        Word::new(w.to_string(), [r.left() as f32, r.top() as f32, r.right() as f32, r.bottom() as f32], "ocrs")
                    })
                    .filter(|w| !w.text.trim().is_empty())
                    .collect(),
            })
            .filter(|l| !l.words.is_empty())
            .collect())
    }

    /// Read `image` in `language` (a [`LANGUAGES`] code) with the multilingual recognizer, over
    /// the text lines the ocrs detector finds.
    pub fn recognize_language(&self, image: &image::RgbImage, language: &str) -> Result<Vec<Line>, OcrError> {
        if !LANGUAGES.iter().any(|l| l.0 == language) {
            return Err(OcrError::Recognize(format!("unsupported language: {language}")));
        }
        checked_len(image.width(), image.height())?;
        use rten_imageproc::BoundingRect;
        let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
        let source = ocrs::ImageSource::from_bytes(image.as_raw(), image.dimensions()).map_err(|e| err(&e))?;
        let input = self.engine.prepare_input(source).map_err(|e| err(&e))?;
        let found = self.engine.detect_words(&input).map_err(|e| err(&e))?;
        let boxes = self.engine.find_text_lines(&input, &found);
        let lines: Vec<_> = boxes
            .into_iter()
            .map(|line| {
                let rect = line.iter().map(|r| r.bounding_rect()).fold([f32::INFINITY, f32::INFINITY, 0.0f32, 0.0f32], |a, r| {
                    [a[0].min(r.left()), a[1].min(r.top()), a[2].max(r.right()), a[3].max(r.bottom())]
                });
                let directions: Vec<_> = line.iter().map(reading_direction).collect();
                let corners: Vec<_> = line.iter().flat_map(|r| r.corners()).map(|p| [p.x, p.y]).collect();
                let skewed = super::lines::LineBox::fit(&directions, &corners).filter(skewed_enough);
                (rect, skewed)
            })
            .collect();
        // CJK's centred punctuation can split one horizontal line into several detections.
        // Join their geometry before padded crops reach the recognizer, avoiding duplicate ink.
        let lines = if matches!(language, "zh-Hant" | "zh-Hans") {
            super::lines::join_fragments(lines.into_iter().map(|(rect, _)| rect).collect()).into_iter().map(|rect| (rect, None)).collect()
        } else {
            lines
        };
        let mut out = Vec::new();
        for (rect, skewed) in lines {
            let words = match skewed {
                Some(line) => self.multilingual.read_skewed_line(image, &line, language)?,
                None => self.multilingual.read_line(image, rect, language)?,
            };
            let words = super::typography::correct(words, language);
            if !words.is_empty() {
                out.push(Line { words });
            }
        }
        Ok(out)
    }

    /// How far clockwise `image` must turn to be upright (0, 90, 180 or 270); uncertain,
    /// nearly blank and nearly solid pages stay at 0.
    pub fn orientation(&self, image: &image::RgbImage) -> Result<u16, OcrError> {
        self.multilingual.orientation(image)
    }
}

/// Whether `line` is read straightened. Only when its baseline climbs more than a quarter of
/// its height from end to end: below that its bounding box takes in no neighbours, and a short
/// word's box angle is mostly noise. More than 20° is no line skew but a turned page, which
/// orientation handles.
fn skewed_enough(line: &super::lines::LineBox) -> bool {
    let angle = line.angle().abs();
    angle <= 20.0 && line.length() * angle.to_radians().sin() > 0.25 * line.height()
}

/// A detected word's reading direction and its length along it. The detector's boxes have
/// no orientation, so this is whichever side is nearer horizontal.
fn reading_direction(word: &rten_imageproc::RotatedRect) -> ([f32; 2], f32) {
    let up = word.up_axis();
    let side = up.perpendicular();
    if side.x.abs() >= side.y.abs() { ([side.x, side.y], word.width()) } else { ([up.x, up.y], word.height()) }
}

/// The ocrs engine as a [`Recognizer`] (the default engine).
pub struct OcrsRecognizer {
    engine: Ocr,
}

impl OcrsRecognizer {
    /// Load the models from `models`.
    pub fn load(models: &Models) -> Result<OcrsRecognizer, OcrError> {
        Ok(OcrsRecognizer { engine: Ocr::load(models)? })
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
        Models::find().is_some()
    }

    fn languages(&self) -> Vec<(String, String)> {
        LANGUAGES.iter().map(|(c, n)| ((*c).to_string(), (*n).to_string())).collect()
    }

    fn recognize(&self, image: &OcrImage, options: &RecognizeOptions) -> Result<Vec<Line>, OcrError> {
        // English keeps the ocrs recognizer; the other languages need the multilingual one.
        // An unknown code reads as English, the engine's default.
        let language = options.language.trim();
        if language == "en" || !LANGUAGES.iter().any(|l| l.0 == language) {
            return self.engine.recognize(image.rgba(), image.width(), image.height());
        }
        let rgb: Vec<u8> = image.rgba().as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let rgb = image::RgbImage::from_raw(image.width(), image.height(), rgb).ok_or(OcrError::EmptyImage)?;
        self.engine.recognize_language(&rgb, language)
    }

    fn orientation(&self, image: &image::RgbImage) -> Option<Result<u16, OcrError>> {
        Some(self.engine.orientation(image))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The multilingual recognition, orientation and dictionary files every model set needs.
    fn multilingual_set(dir: &Path) {
        for file in [MULTILINGUAL_MODEL, ORIENTATION_MODEL, DICTIONARY] {
            std::fs::write(dir.join(file), b"model").unwrap();
        }
    }

    /// A complete pair is not enough without the multilingual files.
    #[test]
    fn model_discovery_needs_the_multilingual_files() {
        let dir = tempfile::tempdir().unwrap();
        for file in [DETECTION_MODEL, RECOGNITION_MODEL] {
            std::fs::write(dir.path().join(file), b"model").unwrap();
        }
        assert_eq!(Models::in_dir(dir.path()), None);
        multilingual_set(dir.path());
        assert!(Models::in_dir(dir.path()).is_some());
    }

    /// The current ONNX pair wins when an installation also retains both legacy models.
    #[test]
    fn model_discovery_prefers_the_complete_onnx_pair() {
        let dir = tempfile::tempdir().unwrap();
        multilingual_set(dir.path());
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
            multilingual_set(dir.path());
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
            multilingual_set(dir.path());
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
        multilingual_set(dir.path());
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
