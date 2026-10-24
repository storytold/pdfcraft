//! pdfcraft-ocr — Scan & OCR ▸ Recognize text (L4).
//!
//! Ocrs detects text; PaddleOCR models read English, accented Latin/Japanese and
//! classify page orientation through RTen. Models are fetched by `cargo xtask models` (see
//! ATTRIBUTION.toml), never at runtime. The engine places Unicode invisible text in PDFs;
//! [`text_layer`] remains the legacy WinAnsi content helper. See README.md for limitations.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::{Path, PathBuf};
pub mod crop;
mod multilingual;
pub mod scan;

pub use pdfcraft_fonts::helvetica_width;

/// The model files, as named in ATTRIBUTION.toml.
pub const DETECTION_MODEL: &str = "text-detection.rten";
pub const RECOGNITION_MODEL: &str = "text-recognition.rten";
pub const MULTILINGUAL_MODEL: &str = "multilingual-recognition.onnx";
pub const ORIENTATION_MODEL: &str = "document-orientation.onnx";
pub const DICTIONARY: &str = "multilingual-dictionary.txt";

/// The recognition model's alphabet, as it was trained (ocrs-models' `DEFAULT_ALPHABET`): its
/// output classes follow this order. The ocrs crate's own copy has an `E` where the training had
/// `€`, which made a scanned "€250" read as "E250".
pub const ALPHABET: &str = " 0123456789!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~€ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Supported and tested language choices (ISO 639-1).
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("de", "Deutsch"),
    ("ja", "日本語"),
    ("fr", "Français"),
    ("es", "Español"),
    ("it", "Italiano"),
    ("nl", "Nederlands"),
    ("da", "Dansk"),
    ("nb", "Norsk bokmål"),
    ("sv", "Svenska"),
    ("fi", "Suomi"),
    ("pt", "Português"),
];

/// Map the primary system locale to an OCR choice, independently of UI translations.
/// Regional variants share a recognizer. Missing or unsupported locales use English.
pub fn language_for_locale(locale: Option<&str>) -> &'static str {
    let primary = locale.unwrap_or_default().trim().split(['-', '_', '.', '@']).next().unwrap_or_default();
    // Older platforms use the generic Norwegian tag for Bokmål.
    let primary = if primary.eq_ignore_ascii_case("no") { "nb" } else { primary };
    LANGUAGES.iter().find(|(code, _)| code.eq_ignore_ascii_case(primary)).map_or("en", |(code, _)| *code)
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
}

/// Where the detector and English recognizer are; multilingual assets live alongside them.
#[derive(Clone, Debug, PartialEq)]
pub struct Models {
    pub detection: PathBuf,
    pub recognition: PathBuf,
}

impl Models {
    /// The models in `dir`, if every required file is there.
    pub fn in_dir(dir: &Path) -> Option<Models> {
        let m = Models { detection: dir.join(DETECTION_MODEL), recognition: dir.join(RECOGNITION_MODEL) };
        (m.detection.is_file()
            && m.recognition.is_file()
            && [MULTILINGUAL_MODEL, ORIENTATION_MODEL, DICTIONARY].iter().all(|f| dir.join(f).is_file()))
        .then_some(m)
    }

    /// Look for the models: `$PDFCRAFT_MODELS`, then where the release packages install them
    /// beside the executable ([`Models::dirs_beside_exe`]), then the source tree's `assets/models/`.
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

/// A recognised word: its text and its box in image pixels `[left, top, right, bottom]`.
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

/// A loaded recogniser. Loading takes a moment; keep one and reuse it.
pub struct Ocr {
    engine: ocrs::OcrEngine,
    multilingual: multilingual::Multilingual,
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
        Ok(Ocr { engine, multilingual: multilingual::Multilingual::load(dir)? })
    }

    /// Load the models found by [`Models::find`].
    pub fn find() -> Result<Ocr, OcrError> {
        Self::load(&Models::find().ok_or(OcrError::NoModels)?)
    }

    /// Recognise the text in an RGBA (or RGB, or grey) image, `width` × `height` pixels.
    pub fn recognize(&self, pixels: &[u8], width: u32, height: u32) -> Result<Vec<Line>, OcrError> {
        if width == 0 || height == 0 || pixels.is_empty() {
            return Err(OcrError::EmptyImage);
        }
        // ocrs wants 1 or 3 channels.
        let count = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4)).ok_or(OcrError::EmptyImage)?;
        let rgb: std::borrow::Cow<[u8]> = if pixels.len() == count {
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
                        Word { text: w.to_string(), rect: [r.left() as f32, r.top() as f32, r.right() as f32, r.bottom() as f32] }
                    })
                    .filter(|w| !w.text.trim().is_empty())
                    .collect(),
            })
            .filter(|l| !l.words.is_empty())
            .collect())
    }

    pub fn recognize_language(&self, image: &image::RgbImage, language: &str) -> Result<Vec<Line>, OcrError> {
        if !LANGUAGES.iter().any(|l| l.0 == language) {
            return Err(OcrError::Recognize(format!("unsupported language: {language}")));
        }
        use rten_imageproc::BoundingRect;
        let source = ocrs::ImageSource::from_bytes(image.as_raw(), image.dimensions()).map_err(|e| OcrError::Recognize(e.to_string()))?;
        let input = self.engine.prepare_input(source).map_err(|e| OcrError::Recognize(e.to_string()))?;
        let found = self.engine.detect_words(&input).map_err(|e| OcrError::Recognize(e.to_string()))?;
        let boxes = self.engine.find_text_lines(&input, &found);
        let mut out = Vec::new();
        for line in boxes {
            let rect = line.iter().map(|r| r.bounding_rect()).fold([f32::INFINITY, f32::INFINITY, 0.0f32, 0.0f32], |a, r| {
                [a[0].min(r.left()), a[1].min(r.top()), a[2].max(r.right()), a[3].max(r.bottom())]
            });
            let words = self.multilingual.read_line(image, rect)?;
            if !words.is_empty() {
                out.push(Line { words });
            }
        }
        Ok(out)
    }

    pub fn orientation(&self, image: &image::RgbImage) -> Result<u16, OcrError> {
        self.multilingual.orientation(image)
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
pub fn ink_extent(text: &str) -> (f64, f64) {
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
    #[test]
    fn system_locale_selects_ocr_language_with_english_fallback() {
        for (locale, expected) in [
            ("de-AT", "de"),
            ("de_CH.UTF-8", "de"),
            ("JA-jp", "ja"),
            ("fr-CA", "fr"),
            ("es-MX", "es"),
            ("it-IT", "it"),
            ("nl-BE", "nl"),
            ("da-DK", "da"),
            ("nb-NO", "nb"),
            ("no_NO", "nb"),
            ("sv-FI", "sv"),
            ("fi_FI", "fi"),
            ("pt-BR", "pt"),
            ("pt_PT@euro", "pt"),
            (" EN-us ", "en"),
            ("is-IS", "en"),
            ("nn-NO", "en"),
            ("zh-Hans", "en"),
            ("C.UTF-8", "en"),
            ("POSIX", "en"),
            ("", "en"),
            ("日本語", "en"),
            ("deutsch", "en"),
        ] {
            assert_eq!(super::language_for_locale(Some(locale)), expected, "{locale}");
        }
        assert_eq!(super::language_for_locale(None), "en");
        for (code, _) in super::LANGUAGES {
            assert_eq!(super::language_for_locale(Some(code)), *code);
        }
    }

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
            for file in [DETECTION_MODEL, RECOGNITION_MODEL, MULTILINGUAL_MODEL, ORIENTATION_MODEL, DICTIONARY] {
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
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, px).unwrap()).to_rgb8();
        let multi = ocr.recognize_language(&image, "de").unwrap();
        assert!(multi.iter().map(Line::text).collect::<Vec<_>>().join(" ").contains("HELLO"), "{multi:?}");
        let _ = ocr.orientation(&image).unwrap();
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
