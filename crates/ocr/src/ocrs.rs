//! The pure-Rust engine: the ocrs crate with its pre-trained models, wrapped as a
//! [`Recognizer`] (`OcrsRecognizer`). Reading is offline and local; the models do not report a
//! confidence, so its words have `confidence: None`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rten::Dimension;
use rten_imageproc::{BoundingRect, Point, RotatedRect, Vec2};

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

/// The languages recognition offers. English uses the ocrs model. Chinese and the accented
/// Latin languages use a PP-OCR model on the same detector when that model is installed.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("zh", "Chinese (Simplified) and English"),
    ("la", "Latin (accented)"),
    ("fr", "French"),
    ("de", "German"),
    ("es", "Spanish"),
    ("pt", "Portuguese"),
    ("it", "Italian"),
    ("nl", "Dutch"),
    ("da", "Danish"),
    ("no", "Norwegian"),
    ("sv", "Swedish"),
    ("is", "Icelandic"),
    ("fi", "Finnish"),
    ("cs", "Czech"),
    ("pl", "Polish"),
];

pub const CHINESE_MODEL: &str = "chinese-recognition.onnx";
pub const CHINESE_DICTIONARY: &str = "chinese-keys.txt";
pub const LATIN_MODEL: &str = "latin-recognition.onnx";
pub const LATIN_DICTIONARY: &str = "latin-keys.txt";

/// `(code, model file, dictionary file, output classes)`. The class count is CTC blank plus
/// one dictionary line plus the appended space, so a truncated dictionary fails at load.
const PP_OCR: &[(&str, &str, &str, usize)] = &[
    ("zh", CHINESE_MODEL, CHINESE_DICTIONARY, 6625),
    ("la", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("fr", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("de", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("es", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("pt", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("it", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("nl", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("da", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("no", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("sv", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("is", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("fi", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("cs", LATIN_MODEL, LATIN_DICTIONARY, 504),
    ("pl", LATIN_MODEL, LATIN_DICTIONARY, 504),
];

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
        Self::find_for("en")
    }

    /// English is the ocrs pair. Another offered language is that detector plus its own
    /// recognition model (the dictionary sits beside it).
    pub fn find_for(language: &str) -> Option<Models> {
        if language == "en" {
            return Self::search_dirs().iter().find_map(|d| Self::in_dir(d));
        }
        let (model, dictionary, _) = pp_ocr_spec(language)?;
        Self::search_dirs().iter().find_map(|dir| {
            let detection = [DETECTION_MODEL, "text-detection.rten"].into_iter().map(|name| dir.join(name)).find(|path| path.is_file())?;
            if !dir.join(model).is_file() || dictionary_file(dir, dictionary).is_none() {
                return None;
            }
            Some(Models { detection, recognition: dir.join(model) })
        })
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
    /// Detection-model input, width then height. `None` when the model does not
    /// publish a fixed size; those pages are detected in one pass.
    window: Option<(u32, u32)>,
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
        let window = detector_window(&detection);
        let params = ocrs::OcrEngineParams {
            detection_model: Some(detection),
            recognition_model: Some(recognition),
            alphabet: Some(ALPHABET.into()),
            ..Default::default()
        };
        let engine = ocrs::OcrEngine::new(params).map_err(|e| OcrError::Load("ocr engine".into(), e.to_string()))?;
        Ok(Ocr { engine, window })
    }

    /// Load the models found by [`Models::find`].
    pub fn find() -> Result<Ocr, OcrError> {
        Self::load(&Models::find().ok_or(OcrError::NoModels)?)
    }

    /// Recognise the text in an RGBA (or RGB, or grey) image, `width` × `height` pixels.
    pub fn recognize(&self, pixels: &[u8], width: u32, height: u32) -> Result<Vec<Line>, OcrError> {
        self.recognize_reading(pixels, width, height, None, "")
    }

    /// The same as [`Self::recognize`]. When `ppocr` is set, detection still uses this engine
    /// and each line is read by that model instead of the English recogniser.
    fn recognize_reading(
        &self,
        pixels: &[u8],
        width: u32,
        height: u32,
        ppocr: Option<&crate::ppocr::PpOcr>,
        preferred: &str,
    ) -> Result<Vec<Line>, OcrError> {
        if pixels.is_empty() {
            return Err(OcrError::EmptyImage);
        }
        let want = checked_len(width, height)?;
        // ocrs wants 1 or 3 channels. PP-OCR wants three.
        let mut rgb: std::borrow::Cow<[u8]> = if pixels.len() == want {
            pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect::<Vec<u8>>().into()
        } else {
            pixels.into()
        };
        let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
        let count = (width as usize).saturating_mul(height as usize).max(1);
        if ppocr.is_some() && rgb.len() / count == 1 {
            rgb = rgb.iter().flat_map(|p| [*p, *p, *p]).collect::<Vec<_>>().into();
        }
        let channels = match rgb.len() / count {
            3 => 3,
            1 => 1,
            _ => 3,
        };
        let found = if self.needs_tiles(width, height) {
            self.detect_tiled(&rgb, width, height, channels)?
        } else {
            let source = ocrs::ImageSource::from_bytes(&rgb, (width, height)).map_err(|e| err(&e))?;
            let input = self.engine.prepare_input(source).map_err(|e| err(&e))?;
            self.engine.detect_words(&input).map_err(|e| err(&e))?
        };
        let source = ocrs::ImageSource::from_bytes(&rgb, (width, height)).map_err(|e| err(&e))?;
        let input = self.engine.prepare_input(source).map_err(|e| err(&e))?;
        let lines = self.engine.find_text_lines(&input, &found);
        if let Some(ppocr) = ppocr {
            if lines.len() > 4096 {
                return Err(OcrError::Recognize("page has more than 4096 text lines".into()));
            }
            let mut result = Vec::new();
            for line in lines {
                let words: Vec<[f32; 4]> = line.iter().map(|r| r.bounding_rect()).map(|r| [r.left(), r.top(), r.right(), r.bottom()]).collect();
                let mut read = Vec::new();
                for rect in line_pieces(&words) {
                    let text = ppocr.recognize(&rgb, width, height, rect, preferred)?;
                    if !text.is_empty() {
                        read.push(Word::new(text, rect, "ocrs"));
                    }
                }
                if !read.is_empty() {
                    result.push(Line { words: read });
                }
            }
            return Ok(result);
        }
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

    fn needs_tiles(&self, width: u32, height: u32) -> bool {
        self.window.is_some_and(|(dw, dh)| width > dw || height > dh)
    }

    fn detect_tiled(&self, rgb: &[u8], width: u32, height: u32, channels: u32) -> Result<Vec<RotatedRect>, OcrError> {
        let Some((window_w, window_h)) = self.window else {
            let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
            let source = ocrs::ImageSource::from_bytes(rgb, (width, height)).map_err(|e| err(&e))?;
            let input = self.engine.prepare_input(source).map_err(|e| err(&e))?;
            return self.engine.detect_words(&input).map_err(|e| err(&e));
        };
        let (tile_w, tile_h, overlap) = tile_geometry(width, height, window_w, window_h);
        let mut hits = Vec::new();
        for origin_y in tile_origins(height, tile_h, tile_h.saturating_sub(overlap).max(1)) {
            let tile_height = tile_h.min(height.saturating_sub(origin_y));
            for origin_x in tile_origins(width, tile_w, tile_w.saturating_sub(overlap).max(1)) {
                let tile_width = tile_w.min(width.saturating_sub(origin_x));
                // A remainder sliver at the edge of a large page is too small to detect.
                // A page that is itself under 64 px on one side is still a real scan.
                if (tile_width < 64 && tile_width < width) || (tile_height < 64 && tile_height < height) {
                    continue;
                }
                let tile = Tile { x: origin_x, y: origin_y, w: tile_width, h: tile_height };
                if !tile_has_ink(rgb, width, tile, channels) {
                    continue;
                }
                let Some(crop) = crop_plane(rgb, width, origin_x, origin_y, tile_width, tile_height, channels) else {
                    return Err(OcrError::Recognize("the page image is shorter than its dimensions".into()));
                };
                let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
                let source = ocrs::ImageSource::from_bytes(&crop, (tile_width, tile_height)).map_err(|e| err(&e))?;
                let input = self.engine.prepare_input(source).map_err(|e| err(&e))?;
                let found = self.engine.detect_words(&input).map_err(|e| err(&e))?;
                hits.extend(found.into_iter().map(|rect| placed_hit(rect, origin_x, origin_y, tile_width, tile_height)));
            }
        }
        Ok(dedup_hits(hits))
    }
}

/// The ocrs engine as a [`Recognizer`] (the default engine).
pub struct OcrsRecognizer {
    engine: Ocr,
    /// Loaded from bytes ([`OcrsRecognizer::from_ocr`]), not from model files: it stays
    /// available without them.
    in_memory: bool,
    /// PP-OCR models loaded on first use, keyed by model file name so French and German share one.
    extra: Mutex<Vec<(&'static str, Arc<crate::ppocr::PpOcr>)>>,
}

impl OcrsRecognizer {
    /// Load the models from `models`.
    pub fn load(models: &Models) -> Result<OcrsRecognizer, OcrError> {
        Ok(OcrsRecognizer { engine: Ocr::load(models)?, in_memory: false, extra: Mutex::new(Vec::new()) })
    }

    /// A recogniser loaded some other way than from model files: the web build downloads the
    /// models and loads them with [`Ocr::from_bytes`].
    pub fn from_ocr(engine: Ocr) -> OcrsRecognizer {
        OcrsRecognizer { engine, in_memory: true, extra: Mutex::new(Vec::new()) }
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

    fn recognize(&self, image: &OcrImage, options: &RecognizeOptions) -> Result<Vec<Line>, OcrError> {
        let Some((model, dictionary, classes)) = pp_ocr_spec(&options.language) else {
            return self.engine.recognize(image.rgba(), image.width(), image.height());
        };
        let reader = self.ppocr_for(model, dictionary, classes)?;
        self.engine.recognize_reading(image.rgba(), image.width(), image.height(), Some(&reader), preferred_letters(&options.language))
    }
}

impl OcrsRecognizer {
    fn ppocr_for(&self, model: &'static str, dictionary: &str, classes: usize) -> Result<Arc<crate::ppocr::PpOcr>, OcrError> {
        let mut slot = self.extra.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, loaded)) = slot.iter().find(|(name, _)| *name == model) {
            return Ok(Arc::clone(loaded));
        }
        let models = Models::find().ok_or(OcrError::NoModels)?;
        let dir = models.detection.parent().ok_or(OcrError::NoModels)?;
        let path = dir.join(model);
        let dict = dictionary_beside(&path, dictionary).ok_or(OcrError::NoModels)?;
        if !path.is_file() {
            return Err(OcrError::NoModels);
        }
        let loaded = Arc::new(crate::ppocr::PpOcr::load(&path, &dict, classes)?);
        slot.push((model, Arc::clone(&loaded)));
        Ok(loaded)
    }
}

fn pp_ocr_spec(language: &str) -> Option<(&'static str, &'static str, usize)> {
    PP_OCR.iter().find(|(code, ..)| *code == language).map(|(_, model, dictionary, classes)| (*model, *dictionary, *classes))
}

/// Letters a document language prefers when two readings are close. Empty means no preference.
pub fn preferred_letters(language: &str) -> &'static str {
    match language {
        "fr" => "àâæçéèêëîïôœùûüÿÀÂÆÇÉÈÊËÎÏÔŒÙÛÜŸ",
        "de" => "äöüßÄÖÜẞ",
        "es" => "áéíñóúü¿¡ÁÉÍÑÓÚÜ",
        "pt" => "áàâãçéêíóôõúÁÀÂÃÇÉÊÍÓÔÕÚ",
        "it" => "àèéìíîòóùÀÈÉÌÍÎÒÓÙ",
        "nl" => "áéíóúëïöüÁÉÍÓÚËÏÖÜ",
        "da" | "no" => "åæøÅÆØ",
        "sv" | "fi" => "åäöÅÄÖ",
        "is" => "áðéíóúýþæöÁÐÉÍÓÚÝÞÆÖ",
        "cs" => "áčďéěíňóřšťúůýžÁČĎÉĚÍŇÓŘŠŤÚŮÝŽ",
        "pl" => "ąćęłńóśźżĄĆĘŁŃÓŚŹŻ",
        _ => "",
    }
}

/// Whether `language` can be read from the installed model files. English is the ocrs pair.
/// The other offered languages need the detector plus their recognition model and dictionary.
pub fn models_installed(language: &str) -> bool {
    if language == "en" {
        return Models::find().is_some();
    }
    let Some((model, dictionary, _)) = pp_ocr_spec(language) else { return false };
    Models::search_dirs().iter().any(|dir| {
        dir.join(model).is_file()
            && dictionary_file(dir, dictionary).is_some()
            && (dir.join(DETECTION_MODEL).is_file() || dir.join("text-detection.rten").is_file())
    })
}

/// `latin-keys.txt` is the attributed name. A checkout that still has `latin-dict.txt` (the
/// same one-character-per-line dictionary) is accepted so recognition can start.
pub(crate) fn dictionary_beside(model: &Path, name: &str) -> Option<PathBuf> {
    dictionary_file(model.parent()?, name)
}

fn dictionary_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let primary = dir.join(name);
    if primary.is_file() {
        return Some(primary);
    }
    if name == LATIN_DICTIONARY {
        let alt = dir.join("latin-dict.txt");
        if alt.is_file() {
            return Some(alt);
        }
    }
    None
}

const PIECE_ASPECT: f32 = 40.0;

/// Gather word boxes into pieces no wider than [`PIECE_ASPECT`] heights. A single word that is
/// already wider stays one piece, so one long line cannot fail the page.
fn line_pieces(words: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let union = |a: [f32; 4], b: [f32; 4]| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
    let aspect = |r: [f32; 4]| (r[2] - r[0]) / (r[3] - r[1]).max(1.0);
    let mut pieces = Vec::new();
    let mut current: Option<[f32; 4]> = None;
    for w in words.iter().copied().filter(|w| w.iter().all(|v| v.is_finite())) {
        current = Some(match current {
            Some(c) if aspect(union(c, w)) > PIECE_ASPECT => {
                pieces.push(c);
                w
            }
            Some(c) => union(c, w),
            None => w,
        });
    }
    pieces.extend(current);
    pieces
}

struct Hit {
    rect: RotatedRect,
    margin: f32,
}

#[derive(Clone, Copy)]
struct Tile {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

fn detector_window(model: &rten::Model) -> Option<(u32, u32)> {
    let shape = model.input_shape(0)?;
    let height = fixed_dim(shape.get(2)?)?;
    let width = fixed_dim(shape.get(3)?)?;
    Some((width, height))
}

fn fixed_dim(dim: &Dimension) -> Option<u32> {
    match dim {
        Dimension::Fixed(v) => u32::try_from(*v).ok().filter(|n| *n > 0),
        Dimension::Symbolic(_) => None,
    }
}

fn tile_geometry(width: u32, height: u32, window_w: u32, window_h: u32) -> (u32, u32, u32) {
    let mut tile_w = (window_w.saturating_mul(3) / 2).max(1).min(width.max(1));
    let mut tile_h = (window_h.saturating_mul(3) / 2).max(1).min(height.max(1));
    for _ in 0..16 {
        let overlap = (tile_w.min(tile_h) / 5).min(tile_w.min(tile_h) / 2);
        let nx = tile_origins(width, tile_w, tile_w.saturating_sub(overlap).max(1)).len();
        let ny = tile_origins(height, tile_h, tile_h.saturating_sub(overlap).max(1)).len();
        if nx <= 8 && ny <= 8 {
            return (tile_w, tile_h, overlap);
        }
        let next_w = if nx > 8 && tile_w < width { tile_w.saturating_mul(2).min(width) } else { tile_w };
        let next_h = if ny > 8 && tile_h < height { tile_h.saturating_mul(2).min(height) } else { tile_h };
        if next_w == tile_w && next_h == tile_h {
            return (tile_w, tile_h, overlap);
        }
        tile_w = next_w;
        tile_h = next_h;
    }
    let overlap = (tile_w.min(tile_h) / 5).min(tile_w.min(tile_h) / 2);
    (tile_w, tile_h, overlap)
}

fn tile_origins(len: u32, tile: u32, step: u32) -> Vec<u32> {
    let step = step.max(1);
    let mut origins = Vec::new();
    let mut cursor = 0u32;
    loop {
        origins.push(cursor);
        let size = tile.min(len.saturating_sub(cursor));
        if cursor.saturating_add(size) >= len || origins.len() >= 64 {
            break;
        }
        let next = cursor.saturating_add(step);
        if next <= cursor {
            break;
        }
        cursor = next;
    }
    origins
}

fn crop_plane(src: &[u8], stride: u32, x: u32, y: u32, width: u32, height: u32, channels: u32) -> Option<Vec<u8>> {
    let channels = channels as usize;
    let row_bytes = (width as usize).checked_mul(channels)?;
    let mut out = Vec::new();
    let rows = y..y.checked_add(height)?;
    for row in rows {
        let start = (row as usize).checked_mul(stride as usize)?.checked_add(x as usize)?.checked_mul(channels)?;
        let end = start.checked_add(row_bytes)?;
        out.extend_from_slice(src.get(start..end)?);
    }
    Some(out)
}

fn placed_hit(rect: RotatedRect, origin_x: u32, origin_y: u32, tile_w: u32, tile_h: u32) -> Hit {
    let shifted = RotatedRect::new(
        Point::from_yx(rect.center().y + origin_y as f32, rect.center().x + origin_x as f32),
        rect.up_axis(),
        rect.width(),
        rect.height(),
    );
    let centre = shifted.center();
    let margin = (centre.x - origin_x as f32)
        .min(origin_x as f32 + tile_w as f32 - centre.x)
        .min(centre.y - origin_y as f32)
        .min(origin_y as f32 + tile_h as f32 - centre.y);
    Hit { rect: shifted, margin }
}

fn dedup_hits(mut hits: Vec<Hit>) -> Vec<RotatedRect> {
    hits.sort_by(|a, b| b.margin.total_cmp(&a.margin));
    let mut kept: Vec<Hit> = Vec::new();
    for hit in hits {
        if let Some(earlier) = kept.iter_mut().find(|earlier| box_iou(earlier.rect, hit.rect) > 0.3) {
            // Two tiles often each see part of the same word. Dropping either part
            // loses the letters that sat on the tile boundary, so keep the union.
            earlier.rect = union_axis_aligned(earlier.rect, hit.rect);
            earlier.margin = earlier.margin.max(hit.margin);
            continue;
        }
        kept.push(hit);
    }
    kept.into_iter().map(|hit| hit.rect).collect()
}

fn union_axis_aligned(a: RotatedRect, b: RotatedRect) -> RotatedRect {
    let u = a.bounding_rect().union(b.bounding_rect());
    let width = u.width().max(1.0);
    let height = u.height().max(1.0);
    RotatedRect::new(Point::from_yx(u.top() + height / 2.0, u.left() + width / 2.0), Vec2::from_yx(-1.0, 0.0), width, height)
}

fn box_iou(a: RotatedRect, b: RotatedRect) -> f32 {
    let a = a.bounding_rect();
    let b = b.bounding_rect();
    let overlap = a.intersection(b);
    if overlap.width() <= 0.0 || overlap.height() <= 0.0 {
        return 0.0;
    }
    let union = a.union(b).area();
    if union <= f32::EPSILON { 0.0 } else { overlap.area() / union }
}

const INK_STEP: u32 = 4;
const INK_DARK: u8 = 200;

fn tile_has_ink(src: &[u8], stride: u32, tile: Tile, channels: u32) -> bool {
    if channels == 0 || tile.w == 0 || tile.h == 0 {
        return true;
    }
    let mut row = 0u32;
    while row < tile.h {
        let mut col = 0u32;
        while col < tile.w {
            match sample_darkness(src, stride, tile.x.saturating_add(col), tile.y.saturating_add(row), channels) {
                Some(value) if value < INK_DARK => return true,
                None => return true,
                Some(_) => {}
            }
            col = col.saturating_add(INK_STEP);
        }
        row = row.saturating_add(INK_STEP);
    }
    false
}

fn sample_darkness(src: &[u8], stride: u32, x: u32, y: u32, channels: u32) -> Option<u8> {
    let index = (y as usize).checked_mul(stride as usize)?.checked_add(x as usize)?.checked_mul(channels as usize)?;
    if channels == 1 {
        return src.get(index).copied();
    }
    let pixel = src.get(index..index.saturating_add(3))?;
    Some(pixel[0].min(pixel[1]).min(pixel[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french_prefers_its_accents_and_english_prefers_none() {
        assert!(preferred_letters("fr").contains('ç') && preferred_letters("fr").contains('ù'));
        assert!(preferred_letters("de").contains('ß'));
        assert!(preferred_letters("pl").contains('ł'));
        assert!(preferred_letters("en").is_empty() && preferred_letters("la").is_empty());
        assert!(pp_ocr_spec("fr").is_some() && pp_ocr_spec("en").is_none());
    }

    #[test]
    fn overlapping_tile_hits_join_into_one_box() {
        let left = RotatedRect::new(Point::from_yx(20.0, 50.0), Vec2::from_yx(-1.0, 0.0), 100.0, 20.0);
        let right = RotatedRect::new(Point::from_yx(20.0, 90.0), Vec2::from_yx(-1.0, 0.0), 60.0, 20.0);
        let merged = dedup_hits(vec![Hit { rect: left, margin: 40.0 }, Hit { rect: right, margin: 8.0 }]);
        assert_eq!(merged.len(), 1, "the two views of one word must not both survive as separate boxes");
        let bounds = merged[0].bounding_rect();
        assert!(bounds.left() <= 0.5 && bounds.right() >= 119.5, "the joined box must cover both hits: {bounds:?}");
    }

    #[test]
    fn tiles_on_a_magazine_page_stay_a_three_by_three_grid() {
        let (tile_w, tile_h, overlap) = tile_geometry(2263, 3200, 600, 800);
        assert_eq!((tile_w, tile_h, overlap), (900, 1200, 180));
        assert_eq!(tile_origins(2263, tile_w, tile_w - overlap).len(), 3);
        assert_eq!(tile_origins(3200, tile_h, tile_h - overlap).len(), 3);
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
