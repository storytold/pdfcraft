//! pdfcraft-ocr — Scan & OCR ▸ Recognize text (L4).
//!
//! Recognition runs the ocrs engine (MIT/Apache-2.0) with its pre-trained models (CC-BY-SA-4.0,
//! fetched by `cargo xtask models`; see ATTRIBUTION.toml). The caller renders a page to pixels;
//! [`Ocr::recognize`] finds text in them. The engine writes placed text as an invisible layer
//! (rendering mode 3), making a searchable image without changing the original image.
//! [`text_layer`] is the legacy WinAnsi writer; the engine supplies Unicode fonts for text
//! outside WinAnsi (Chinese, and Latin letters no standard font encodes).
//!
//! English uses ocrs; Chinese (`zh`) and accented Latin (`la`) use PP-OCR models on the same
//! local runtime, each with its own recognition model and character dictionary.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use rten_imageproc::BoundingRect;
use std::path::{Path, PathBuf};

mod ppocr;

pub use pdfcraft_fonts::helvetica_width;

/// The model files, as named in ATTRIBUTION.toml.
pub const DETECTION_MODEL: &str = "text-detection.onnx";
pub const RECOGNITION_MODEL: &str = "text-recognition.onnx";
pub const CHINESE_MODEL: &str = "chinese-recognition.onnx";
pub const CHINESE_DICTIONARY: &str = "chinese-keys.txt";
pub const LATIN_MODEL: &str = "latin-recognition.onnx";
pub const LATIN_DICTIONARY: &str = "latin-keys.txt";

/// The PP-OCR languages: (code, model file, dictionary file, output classes = CTC blank +
/// one per dictionary line + the appended space). The class count is pinned per model, so a
/// wrong or truncated dictionary fails at load instead of decoding garbage.
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

/// The recognition model's alphabet, as it was trained (ocrs-models' `DEFAULT_ALPHABET`): its
/// output classes follow this order. The ocrs crate's own copy has an `E` where the training had
/// `€`, which made a scanned "€250" read as "E250".
pub const ALPHABET: &str = " 0123456789!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~€ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The languages the models read (ISO 639-1). `la` uses the Latin dictionary; a listed letter can still be misread.
/// The named languages use that same model and, when two letters score close together, prefer
/// the ones that language writes (`å` for Danish, `à` for French) without forbidding the others.
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

/// Letters a document language prefers when the recogniser is unsure. Empty means no preference.
pub fn preferred_letters(language: &str) -> &'static str {
    match language {
        "fr" => "àâæçéèêëîïôœùûüÿÀÂÆÇÉÈÊËÎÏÔŒÙÛÜŸ",
        // ß is lowercase-only in most German text (words in full uppercase spell it "SS"), but
        // the 2017 reform gave it the capital ẞ for exactly the cases that still need one letter
        // (a name, an all-caps heading): "STRASSE" and "STRAẞE" are both correct today.
        "de" => "äöüßÄÖÜẞ",
        "es" => "áéíñóúü¿¡ÁÉÍÑÓÚÜ",
        "pt" => "áàâãçéêíóôõúÁÀÂÃÇÉÊÍÓÔÕÚ",
        "it" => "àèéìíîòóùÀÈÉÌÍÎÒÓÙ",
        // The stress mark (klemtoonteken) that separates "wíl" from "wil" falls on any vowel, not
        // only a/e/o: "dít" and "nú" are as valid as "dát". The diaeresis (ë, ï, ö, ü) breaks up a
        // vowel pair that would otherwise read as one sound ("ideeën", "ruïne", "zoölogie").
        "nl" => "áéíóúëïöüÁÉÍÓÚËÏÖÜ",
        "da" | "no" => "åæøÅÆØ",
        "sv" | "fi" => "åäöÅÄÖ",
        "is" => "áðéíóúýþæöÁÐÉÍÓÚÝÞÆÖ",
        "cs" => "áčďéěíňóřšťúůýžÁČĎÉĚÍŇÓŘŠŤÚŮÝŽ",
        "pl" => "ąćęłńóśźżĄĆĘŁŃÓŚŹŻ",
        _ => "",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    /// Shown to end users (release packages ship the models; a source build fetches them with
    /// `cargo xtask models`). The UI catalogs key this exact text.
    #[error("the text recognition models are not installed (reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them)")]
    NoModels,
    #[error("loading {0}: {1}")]
    Load(String, String),
    #[error("text recognition failed: {0}")]
    Recognize(String),
    #[error("the image is empty")]
    EmptyImage,
    #[error("unsupported OCR language {0:?}; choose a language from ocr_status")]
    Language(String),
}

/// Where a language's detector and recogniser are. The PP-OCR languages also require their
/// character dictionary beside the recognition model.
#[derive(Clone, Debug, PartialEq)]
pub struct Models {
    pub detection: PathBuf,
    pub recognition: PathBuf,
    pub language: &'static str,
}

impl Models {
    /// A complete model pair in `dir`, preferring current ONNX files over legacy RTEN files.
    /// An incomplete update never mixes formats or hides an intact legacy pair.
    pub fn in_dir(dir: &Path) -> Option<Models> {
        Self::in_dir_for(dir, "en")
    }

    /// English keeps a complete ONNX pair ahead of a complete legacy RTEN pair, and never mixes
    /// the two. Other languages use that same detector with their own recognition model and dictionary.
    pub fn in_dir_for(dir: &Path, language: &str) -> Option<Models> {
        let language = LANGUAGES.iter().find(|l| l.0 == language)?.0;
        let english = [(DETECTION_MODEL, RECOGNITION_MODEL), ("text-detection.rten", "text-recognition.rten")].into_iter().find_map(|(detection, recognition)| {
            let m = Models { detection: dir.join(detection), recognition: dir.join(recognition), language };
            (m.detection.is_file() && m.recognition.is_file()).then_some(m)
        });
        if language == "en" {
            return english;
        }
        let &(_, model, dictionary, _) = PP_OCR.iter().find(|(code, ..)| *code == language)?;
        if !dir.join(dictionary).is_file() || !dir.join(model).is_file() {
            return None;
        }
        let detection = english.as_ref().map(|m| m.detection.clone()).or_else(|| {
            [DETECTION_MODEL, "text-detection.rten"].into_iter().map(|name| dir.join(name)).find(|path| path.is_file())
        })?;
        Some(Models { detection, recognition: dir.join(model), language })
    }

    /// Look for the models: `$PDFCRAFT_MODELS`, then where the release packages install them
    /// beside the executable ([`Models::dirs_beside_exe`]), then the source tree's `assets/models/`.
    pub fn find() -> Option<Models> {
        Self::find_for("en")
    }

    pub fn find_for(language: &str) -> Option<Models> {
        Self::search_dirs().iter().find_map(|d| Self::in_dir_for(d, language))
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

    /// Where the release packages put the models, relative to the directory holding the
    /// executable (packaging/*):
    /// - `models/` beside it: the Windows MSI and portable zip;
    /// - `../Resources/models`: the macOS app bundle (`Contents/MacOS` → `Contents/Resources`);
    /// - `../share/pdfcraft/models`: the FHS-style trees (`bin/` → `share/`) of the Linux
    ///   deb/rpm/tar.gz/AppImage/Flatpak and the FreeBSD tarball, wherever they are installed.
    pub fn dirs_beside_exe(exe_dir: &Path) -> Vec<PathBuf> {
        vec![exe_dir.join("models"), exe_dir.join("../Resources/models"), exe_dir.join("../share/pdfcraft/models")]
    }
}

/// A recognised text segment and its box in image pixels `[left, top, right, bottom]`.
/// English segments are words; Chinese and Latin segments are lines, or pieces of a long line.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub rect: [f32; 4],
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

/// The longest piece of a line the PP-OCR recognisers read at once, as width over height. A line
/// across a newspaper's columns can be a hundred heights long; read whole it is squeezed past
/// legibility (or, before, refused, and the page with it). Longer lines are read in pieces split
/// at the gaps between their words.
const PIECE_ASPECT: f32 = 40.0;

/// `words` (boxes `[left, top, right, bottom]`, left to right along one line) gathered into
/// pieces of at most [`PIECE_ASPECT`]: each piece is the box around the words it takes. A word
/// longer than that on its own is a piece of its own. Non-finite boxes are left out.
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

/// A loaded recogniser. Loading takes a moment; keep one and reuse it.
pub struct Ocr {
    engine: ocrs::OcrEngine,
    ppocr: Option<ppocr::PpOcr>,
    /// Letters this document language prefers when two readings are close. Empty for English,
    /// Chinese and the unrestricted Latin choice.
    preferred: &'static str,
}

impl Ocr {
    pub fn load(models: &Models) -> Result<Ocr, OcrError> {
        let load = |p: &Path| rten::Model::load_file(p).map_err(|e| OcrError::Load(p.display().to_string(), e.to_string()));
        let name = models.recognition.file_name().and_then(|f| f.to_str()).unwrap_or_default();
        let ppocr = PP_OCR
            .iter()
            .find(|(_, model, _, _)| *model == name)
            .map(|(_, _, dictionary, classes)| ppocr::PpOcr::load(&models.recognition, &models.recognition.with_file_name(dictionary), *classes))
            .transpose()?;
        let params = ocrs::OcrEngineParams {
            detection_model: Some(load(&models.detection)?),
            recognition_model: if ppocr.is_none() { Some(load(&models.recognition)?) } else { None },
            alphabet: Some(ALPHABET.into()),
            ..Default::default()
        };
        let engine = ocrs::OcrEngine::new(params).map_err(|e| OcrError::Load("ocr engine".into(), e.to_string()))?;
        Ok(Ocr { engine, ppocr, preferred: preferred_letters(models.language) })
    }

    /// Load the models found by [`Models::find`].
    pub fn find() -> Result<Ocr, OcrError> {
        Self::find_for("en")
    }

    pub fn find_for(language: &str) -> Result<Ocr, OcrError> {
        if !LANGUAGES.iter().any(|l| l.0 == language) {
            return Err(OcrError::Language(language.into()));
        }
        Self::load(&Models::find_for(language).ok_or(OcrError::NoModels)?)
    }

    /// Recognise the text in an RGBA (or RGB, or grey) image, `width` × `height` pixels.
    pub fn recognize(&self, pixels: &[u8], width: u32, height: u32) -> Result<Vec<Line>, OcrError> {
        let count = (width as usize)
            .checked_mul(height as usize)
            .filter(|n| *n <= 64 * 1024 * 1024)
            .ok_or_else(|| OcrError::Recognize("image exceeds 64 megapixels".into()))?;
        if count == 0 || pixels.is_empty() {
            return Err(OcrError::EmptyImage);
        }
        let channels = pixels.len() / count;
        if !pixels.len().is_multiple_of(count) || !matches!(channels, 1 | 3 | 4) {
            return Err(OcrError::Recognize("image must contain exactly 1, 3 or 4 channels per pixel".into()));
        }
        // ocrs wants 1 or 3 channels.
        let rgb: std::borrow::Cow<[u8]> = if channels == 4 {
            pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect::<Vec<u8>>().into()
        } else if channels == 1 && self.ppocr.is_some() {
            pixels.iter().flat_map(|p| [*p; 3]).collect::<Vec<_>>().into()
        } else {
            pixels.into()
        };
        let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
        let source = ocrs::ImageSource::from_bytes(&rgb, (width, height)).map_err(|e| err(&e))?;
        let input = self.engine.prepare_input(source).map_err(|e| err(&e))?;
        let found = self.engine.detect_words(&input).map_err(|e| err(&e))?;
        let lines = self.engine.find_text_lines(&input, &found);
        if let Some(ppocr) = &self.ppocr {
            if lines.len() > 4096 {
                return Err(OcrError::Recognize("page has more than 4096 text lines".into()));
            }
            let mut result = Vec::new();
            for line in lines {
                let words: Vec<[f32; 4]> = line.iter().map(|r| r.bounding_rect()).map(|r| [r.left(), r.top(), r.right(), r.bottom()]).collect();
                let mut read = Vec::new();
                for rect in line_pieces(&words) {
                    let text = ppocr.recognize(&rgb, width, height, rect, self.preferred)?;
                    if !text.is_empty() {
                        read.push(Word { text, rect });
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
    pub text: String,
    pub origin: [f64; 2],
    pub across: [f64; 2],
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

/// Page content that writes `words` as invisible text in `/PCHelv` (standard Helvetica,
/// WinAnsiEncoding), each word stretched to its box so selection and search highlight the
/// right place. Marked content `/OCR` so it can be told apart from the page's own text.
pub fn text_layer(words: &[PlacedWord]) -> Vec<u8> {
    // `3 Tr` and the font are graphics state, which `ET` doesn't reset: without `q … Q` they
    // would stay set for every later content stream on the page, so text added after OCR (a
    // header or footer) drew invisibly (#3029).
    let mut out = b"/OCR BMC\nq\nBT\n3 Tr\n/PCHelv 1 Tf\n".to_vec();
    for w in words {
        let width = helvetica_width(&w.text, 1.0);
        let height = w.up[0].hypot(w.up[1]);
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        // The em square: the word's own ink (cap height or x-height, descenders or not) spans the
        // box, so the text is as tall as the scanned word; its width is stretched to fit the word.
        let (top, bottom) = ink_extent(&w.text);
        let em = [w.up[0] / (top - bottom), w.up[1] / (top - bottom)];
        let ax = [w.across[0] / width, w.across[1] / width];
        let o = [w.origin[0] - em[0] * bottom, w.origin[1] - em[1] * bottom];
        let m = [ax[0], ax[1], em[0], em[1], o[0], o[1]];
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
mod tests {
    use super::*;

    #[test]
    fn places_words_through_the_mapping() {
        // 2 pixels per point, page 612 × 792, y flipped.
        let to_user = |x: f32, y: f32| [x as f64 / 2.0, 792.0 - y as f64 / 2.0];
        let w = PlacedWord::place(&Word { text: "Hello".into(), rect: [100.0, 200.0, 300.0, 240.0] }, to_user);
        assert_eq!(w.origin, [50.0, 672.0]);
        assert_eq!(w.across, [100.0, 0.0]);
        assert_eq!(w.up, [0.0, 20.0]);
    }

    /// The text layer is Helvetica in WinAnsiEncoding: every character the model can recognise
    /// must be one it can hold, or recognised text would be saved as `?`. A model with a wider
    /// alphabet (Chinese, for example) needs a text layer in another font first.
    #[test]
    fn the_model_alphabet_fits_the_text_layer_font() {
        assert_eq!(pdfcraft_fonts::first_undrawable(ALPHABET), None);
    }

    #[test]
    fn text_layer_is_invisible_text_fitted_to_each_box() {
        let w = PlacedWord { text: "Hi".into(), origin: [10.0, 20.0], across: [30.0, 0.0], up: [0.0, 9.3] };
        let s = String::from_utf8(text_layer(&[w])).unwrap();
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
        let w = PlacedWord { text: "Hi".into(), origin: [10.0, 20.0], across: [30.0, 0.0], up: [0.0, 9.3] };
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
        let w = PlacedWord { text: "gap".into(), origin: [0.0, 100.0], across: [20.0, 0.0], up: [0.0, 7.3] };
        let s = String::from_utf8(text_layer(&[w])).unwrap();
        let em = 7.3 / (X_HEIGHT + DESCENT);
        assert!(s.contains(&format!(" 0 0 {} 0 {} Tm (gap) Tj", num(em), num(100.0 + em * DESCENT))), "{s}");
    }

    #[test]
    fn rotated_pages_keep_the_reading_direction() {
        // A page turned 90°: image x runs up the page.
        let to_user = |x: f32, y: f32| [y as f64, x as f64];
        let w = PlacedWord::place(&Word { text: "Up".into(), rect: [0.0, 0.0, 50.0, 10.0] }, to_user);
        assert_eq!(w.across, [0.0, 50.0]);
        assert_eq!(w.up, [-10.0, 0.0]);
    }

    /// The current ONNX pair wins when an installation also retains both legacy models.
    #[test]
    fn model_discovery_prefers_the_complete_onnx_pair() {
        let dir = tempfile::tempdir().unwrap();
        for file in ["text-detection.onnx", "text-recognition.onnx"] {
            std::fs::write(dir.path().join(file), b"model").unwrap();
        }
        let expected = Some(Models { detection: dir.path().join("text-detection.onnx"), recognition: dir.path().join("text-recognition.onnx"), language: "en" });
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
                Some(Models { detection: dir.path().join("text-detection.rten"), recognition: dir.path().join("text-recognition.rten"), language: "en" }),
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
            Some(Models { detection: dir.path().join("text-detection.rten"), recognition: dir.path().join("text-recognition.rten"), language: "en" })
        );
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

    /// A line across a newspaper's columns is read in pieces split at word gaps, each at most
    /// [`PIECE_ASPECT`] long, instead of failing the page.
    #[test]
    fn a_long_line_is_read_in_pieces_at_word_gaps() {
        // 30 words, 100 wide with 10 between them, 20 high: 3,290 wide, 164 heights.
        let words: Vec<[f32; 4]> = (0..30).map(|i| [i as f32 * 110.0, 10.0, i as f32 * 110.0 + 100.0, 30.0]).collect();
        let pieces = line_pieces(&words);
        assert!(pieces.len() >= 5, "{pieces:?}");
        for piece in &pieces {
            assert!((piece[2] - piece[0]) / (piece[3] - piece[1]) <= PIECE_ASPECT, "{piece:?}");
        }
        // Together they cover every word, and no word is cut.
        assert_eq!(pieces.first().map(|p| p[0]), Some(0.0));
        assert_eq!(pieces.last().map(|p| p[2]), Some(29.0 * 110.0 + 100.0));
        for word in &words {
            assert!(pieces.iter().any(|p| p[0] <= word[0] && word[2] <= p[2]), "{word:?} is cut");
        }
        // A short line stays whole; a word longer than a piece on its own is its own piece.
        assert_eq!(line_pieces(&words[..3]).len(), 1);
        assert_eq!(line_pieces(&[[0.0, 0.0, 5000.0, 20.0], [5010.0, 0.0, 5100.0, 20.0]]).len(), 2);
        assert!(line_pieces(&[[f32::NAN, 0.0, 1.0, 1.0]]).is_empty());
    }

    /// A gap the size of a column gutter starts a new piece, and a piece exactly at the limit
    /// stays one piece. The next word past that limit does not.
    #[test]
    fn a_column_gap_starts_a_new_piece_and_the_limit_stays_whole() {
        let gutter = [[0.0, 0.0, 200.0, 20.0], [3000.0, 0.0, 3200.0, 20.0]];
        assert_eq!(line_pieces(&gutter), vec![[0.0, 0.0, 200.0, 20.0], [3000.0, 0.0, 3200.0, 20.0]]);
        assert_eq!(line_pieces(&[[0.0, 0.0, 800.0, 20.0]]).len(), 1, "aspect 40 is still one piece");
        let over = line_pieces(&[[0.0, 0.0, 800.0, 20.0], [800.0, 0.0, 820.0, 20.0]]);
        assert_eq!(over.len(), 2, "{over:?}");
        assert!(line_pieces(&[]).is_empty());
        assert!(line_pieces(&[[f32::INFINITY, 0.0, 10.0, 10.0], [0.0, f32::NEG_INFINITY, 10.0, 10.0]]).is_empty());
        let ordered = line_pieces(&[[0.0, 0.0, 100.0, 20.0], [2000.0, 0.0, 2100.0, 20.0], [4000.0, 0.0, 4100.0, 20.0]]);
        for pair in ordered.windows(2) {
            assert!(pair[0][0] <= pair[1][0], "{ordered:?}");
        }
    }

    /// #524 and #3264: German is a named language on the accented Latin model, and it prefers
    /// ä ö ü ß. The same model serves the other European names. English stays on its own model.
    #[test]
    fn named_european_languages_use_the_latin_model() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-langs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for file in [DETECTION_MODEL, RECOGNITION_MODEL, LATIN_MODEL, LATIN_DICTIONARY, CHINESE_MODEL, CHINESE_DICTIONARY] {
            std::fs::write(dir.join(file), b"x").unwrap();
        }
        for (code, _) in LANGUAGES {
            let models = Models::in_dir_for(&dir, code).unwrap_or_else(|| panic!("{code} was not found"));
            let expect = if *code == "en" {
                RECOGNITION_MODEL
            } else if *code == "zh" {
                CHINESE_MODEL
            } else {
                LATIN_MODEL
            };
            assert!(models.recognition.ends_with(expect), "{code}: {}", models.recognition.display());
            if !matches!(*code, "en" | "zh" | "la") {
                assert!(!preferred_letters(code).is_empty(), "{code} has no preferred letters");
            }
        }
        let german = preferred_letters("de");
        assert!(german.contains('ä') && german.contains('ö') && german.contains('ü') && german.contains('ß'), "{german}");
        assert!(Models::in_dir_for(&dir, "xx").is_none());
        assert!(matches!(Ocr::find_for("xx"), Err(OcrError::Language(_))));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A language can only prefer a letter the Latin dictionary actually contains.
    #[test]
    fn preferred_letters_are_in_the_latin_dictionary() {
        let Some(models) = Models::find_for("la") else {
            eprintln!("skipped: Latin OCR models not installed");
            return;
        };
        let path = models.recognition.with_file_name(LATIN_DICTIONARY);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let have: std::collections::BTreeSet<char> = text.chars().collect();
        for (code, _) in LANGUAGES {
            for ch in preferred_letters(code).chars() {
                assert!(have.contains(&ch), "{code} prefers {ch} (U+{:04X}), which {} does not contain", u32::from(ch), path.display());
            }
        }
    }

    /// Every named language's complete set of standard diacritics and special letters, lower and
    /// upper case, checked letter by letter against [`preferred_letters`] (#572): a language that
    /// is missing one of its own vowels, or has gained a letter that isn't its own, fails here by
    /// name and by character, not as a vague "wrong length" count. The sets below are each
    /// language's published orthography (the accented letters its own spelling rules use), not
    /// merely what the Latin dictionary happens to contain; `preferred_letters_are_in_the_latin_dictionary`
    /// checks that separately. `en`, `zh` and `la` keep no preference (`la` reads every accented
    /// letter without favouring one language's own), and are covered by the empty-string case.
    #[test]
    fn every_named_language_prefers_exactly_its_own_alphabet() {
        let reference: &[(&str, &str)] = &[
            // à â æ ç é è ê ë î ï ô œ ù û ü ÿ: the accents, ligatures and diaereses of French
            // spelling (façade, Noël, août, capharnaüm, haïr), each with its capital.
            ("fr", "àâæçéèêëîïôœùûüÿÀÂÆÇÉÈÊËÎÏÔŒÙÛÜŸ"),
            // ä ö ü: the umlauts. ß: the lowercase-only eszett; ẞ is its 2017-reform capital, used
            // in an all-uppercase name or heading where plain "SS" would lose the distinction.
            ("de", "äöüßÄÖÜẞ"),
            // á é í ó ú: the acute accent on every vowel. ñ: the distinct letter eñe. ü: the
            // trema in "güe"/"güi" (pingüino). ¿ ¡: the opening question and exclamation marks.
            ("es", "áéíñóúü¿¡ÁÉÍÑÓÚÜ"),
            // á à â ã ç é ê í ó ô õ ú: Portuguese's acute, grave, circumflex and tilde vowels plus
            // the cedilla (ação, informação, área, à, ânimo, café, você, Brasília, avó, avô, açúcar).
            ("pt", "áàâãçéêíóôõúÁÀÂÃÇÉÊÍÓÔÕÚ"),
            // à è é ì í î ò ó ù: the grave and acute stress marks Italian spelling uses (città,
            // perché, cioè, lì, sì, ciò, più) plus the rarer í/î of formal and literary use.
            ("it", "àèéìíîòóùÀÈÉÌÍÎÒÓÙ"),
            // á é í ó ú: the stress mark (klemtoonteken) Dutch can put on any vowel (wíl, dát,
            // nú). ë ï ö ü: the diaeresis that keeps a vowel pair from reading as one sound
            // (ideeën, ruïne, zoölogie, reünie).
            ("nl", "áéíóúëïöüÁÉÍÓÚËÏÖÜ"),
            // å æ ø: the three letters Danish and (Bokmål) Norwegian add after z, not diacritics
            // on a Latin letter but letters of their own alphabets.
            ("da", "åæøÅÆØ"),
            ("no", "åæøÅÆØ"),
            // å ä ö: the three letters Swedish and Finnish add after z (å only by convention in
            // Finnish, in names and Swedish loanwords; ä and ö are native Finnish vowels).
            ("sv", "åäöÅÄÖ"),
            ("fi", "åäöÅÄÖ"),
            // á ð é í ó ú ý þ æ ö: the whole of Icelandic's extension of the Latin alphabet (its
            // own letters eth and thorn, the acute vowels, and æ/ö).
            ("is", "áðéíóúýþæöÁÐÉÍÓÚÝÞÆÖ"),
            // á č ď é ě í ň ó ř š ť ú ů ý ž: Czech's full diacritic set (háček, čárka and kroužek).
            ("cs", "áčďéěíňóřšťúůýžÁČĎÉĚÍŇÓŘŠŤÚŮÝŽ"),
            // ą ć ę ł ń ó ś ź ż: the nine letters Polish spelling adds to the plain Latin alphabet.
            ("pl", "ąćęłńóśźżĄĆĘŁŃÓŚŹŻ"),
            ("en", ""),
            ("zh", ""),
            ("la", ""),
        ];
        assert_eq!(
            reference.iter().map(|(code, _)| *code).collect::<std::collections::BTreeSet<_>>(),
            LANGUAGES.iter().map(|(code, _)| *code).collect::<std::collections::BTreeSet<_>>(),
            "every language in LANGUAGES has a reference entry here, and vice versa"
        );
        for (code, expect) in reference {
            let have: std::collections::BTreeSet<char> = preferred_letters(code).chars().collect();
            let want: std::collections::BTreeSet<char> = expect.chars().collect();
            assert_eq!(have.len(), expect.chars().count(), "{code}: a letter is repeated in {expect:?}");
            for ch in &want {
                assert!(have.contains(ch), "{code} is missing {ch} (U+{:04X}) from its own alphabet", u32::from(*ch));
            }
            for ch in &have {
                assert!(want.contains(ch), "{code} prefers {ch} (U+{:04X}), which is not one of its own letters", u32::from(*ch));
            }
        }
    }

    #[test]
    fn french_prefers_cedilla_and_grave_and_does_not_claim_the_nordic_ring() {
        assert!(preferred_letters("fr").contains('ç') && preferred_letters("fr").contains('ù'));
        assert!(preferred_letters("da").contains('å') && !preferred_letters("da").contains('à'));
        assert!(preferred_letters("la").is_empty() && preferred_letters("en").is_empty());
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
