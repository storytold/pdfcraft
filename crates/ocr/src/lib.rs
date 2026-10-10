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
pub const DETECTION_MODEL: &str = "text-detection.rten";
pub const RECOGNITION_MODEL: &str = "text-recognition.rten";
pub const CHINESE_MODEL: &str = "chinese-recognition.onnx";
pub const CHINESE_DICTIONARY: &str = "chinese-keys.txt";
pub const LATIN_MODEL: &str = "latin-recognition.onnx";
pub const LATIN_DICTIONARY: &str = "latin-keys.txt";

/// The PP-OCR languages: (code, model file, dictionary file, output classes = CTC blank +
/// one per dictionary line + the appended space). The class count is pinned per model, so a
/// wrong or truncated dictionary fails at load instead of decoding garbage.
const PP_OCR: &[(&str, &str, &str, usize)] = &[("zh", CHINESE_MODEL, CHINESE_DICTIONARY, 6625), ("la", LATIN_MODEL, LATIN_DICTIONARY, 504)];

/// The languages the models read (ISO 639-1).
pub const LANGUAGES: &[(&str, &str)] = &[("en", "English"), ("zh", "Chinese (Simplified) and English"), ("la", "Latin (accented)")];

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
    #[error("unsupported OCR language {0:?}; choose en, zh or la")]
    Language(String),
}

/// Where a language's detector and recogniser are. The PP-OCR languages also require their
/// character dictionary beside the recognition model.
#[derive(Clone, Debug, PartialEq)]
pub struct Models {
    pub detection: PathBuf,
    pub recognition: PathBuf,
}

impl Models {
    /// The models in `dir`, if both files are there.
    pub fn in_dir(dir: &Path) -> Option<Models> {
        Self::in_dir_for(dir, "en")
    }

    pub fn in_dir_for(dir: &Path, language: &str) -> Option<Models> {
        let recognition = if language == "en" {
            RECOGNITION_MODEL
        } else {
            let &(_, model, dictionary, _) = PP_OCR.iter().find(|(code, ..)| *code == language)?;
            if !dir.join(dictionary).is_file() {
                return None;
            }
            model
        };
        let m = Models { detection: dir.join(DETECTION_MODEL), recognition: dir.join(recognition) };
        (m.detection.is_file() && m.recognition.is_file()).then_some(m)
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
/// English segments are words; Chinese and Latin segments are whole lines.
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
    ppocr: Option<ppocr::PpOcr>,
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
            ..Default::default()
        };
        let engine = ocrs::OcrEngine::new(params).map_err(|e| OcrError::Load("ocr engine".into(), e.to_string()))?;
        Ok(Ocr { engine, ppocr })
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
                let rect = line
                    .iter()
                    .map(|r| r.bounding_rect())
                    .fold(None, |rect: Option<rten_imageproc::Rect<f32>>, r| Some(rect.map_or(r, |b| b.union(r))));
                let Some(r) = rect else { continue };
                let rect = [r.left(), r.top(), r.right(), r.bottom()];
                let text = ppocr.recognize(&rgb, width, height, rect)?;
                if !text.is_empty() {
                    result.push(Line { words: vec![Word { text, rect }] });
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

fn num(v: f64) -> String {
    let s = format!("{:.4}", if v.abs() < 1e-9 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Page content that writes `words` as invisible text in `/PCHelv` (standard Helvetica,
/// WinAnsiEncoding), each word stretched to its box so selection and search highlight the
/// right place. Marked content `/OCR` so it can be told apart from the page's own text.
pub fn text_layer(words: &[PlacedWord]) -> Vec<u8> {
    let mut out = b"/OCR BMC\nBT\n3 Tr\n/PCHelv 1 Tf\n".to_vec();
    for w in words {
        let width = helvetica_width(&w.text, 1.0);
        let height = w.up[0].hypot(w.up[1]);
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        // The em square: its height spans the box; its width is stretched to fit the word.
        let em = [w.up[0] / BOX_EM, w.up[1] / BOX_EM];
        let ax = [w.across[0] / width, w.across[1] / width];
        let o = [w.origin[0] + em[0] * DESCENT, w.origin[1] + em[1] * DESCENT];
        let m = [ax[0], ax[1], em[0], em[1], o[0], o[1]];
        let nums: Vec<String> = m.iter().map(|v| num(*v)).collect();
        out.extend_from_slice(nums.join(" ").as_bytes());
        out.extend_from_slice(b" Tm ");
        out.extend(pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(&w.text)));
        out.extend_from_slice(b" Tj\n");
    }
    out.extend_from_slice(b"ET\nEMC\n");
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

    #[test]
    fn text_layer_is_invisible_text_fitted_to_each_box() {
        let w = PlacedWord { text: "Hi".into(), origin: [10.0, 20.0], across: [30.0, 0.0], up: [0.0, 9.3] };
        let s = String::from_utf8(text_layer(&[w])).unwrap();
        assert!(s.contains("3 Tr") && s.contains("/PCHelv 1 Tf"), "{s}");
        let width = helvetica_width("Hi", 1.0);
        assert!(s.contains(&format!("{} 0 0 10 10 22.1 Tm (Hi) Tj", num(30.0 / width))), "{s}");
        assert!(s.starts_with("/OCR BMC") && s.ends_with("EMC\n"));
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
