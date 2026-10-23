//! pdfcraft-ocr — Scan & OCR ▸ Recognize text (L4).
//!
//! Recognition runs the ocrs engine (MIT/Apache-2.0) with its pre-trained models (CC-BY-SA-4.0,
//! fetched by `cargo xtask models`; see ATTRIBUTION.toml). The caller renders a page to pixels;
//! [`Ocr::recognize`] finds the words in them, and [`text_layer`] turns words placed in user space
//! into page content: invisible text (rendering mode 3) over each word, so the page becomes a
//! searchable image (Acrobat's "Searchable Image (Exact)": the image is left untouched).
//!
//! The models read the Latin alphabet (English and other languages written without accents).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::{Path, PathBuf};

pub use pdfcraft_fonts::helvetica_width;

/// The model files, as named in ATTRIBUTION.toml.
pub const DETECTION_MODEL: &str = "text-detection.rten";
pub const RECOGNITION_MODEL: &str = "text-recognition.rten";

/// The recognition model's alphabet, as it was trained (ocrs-models' `DEFAULT_ALPHABET`): its
/// output classes follow this order. The ocrs crate's own copy has an `E` where the training had
/// `€`, which made a scanned "€250" read as "E250".
pub const ALPHABET: &str = " 0123456789!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~€ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The languages the models read (ISO 639-1). English uses the bundled ocrs Latin model;
/// Hebrew uses Tesseract (`heb+eng`) when present and writes Unicode `/PCOCR` text layers.
pub const LANGUAGES: &[(&str, &str)] = &[("en", "English"), ("he", "Hebrew")];

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

/// Where the two model files are.
#[derive(Clone, Debug, PartialEq)]
pub struct Models {
    pub detection: PathBuf,
    pub recognition: PathBuf,
}

impl Models {
    /// The models in `dir`, if both files are there.
    pub fn in_dir(dir: &Path) -> Option<Models> {
        let m = Models { detection: dir.join(DETECTION_MODEL), recognition: dir.join(RECOGNITION_MODEL) };
        (m.detection.is_file() && m.recognition.is_file()).then_some(m)
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
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models"));
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
        Ok(Ocr { engine })
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
        let rgb: std::borrow::Cow<[u8]> = if pixels.len() == (width * height * 4) as usize {
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

    /// Recognise the text in an image using `lang` (`"en"` or `"he"`).
    pub fn recognize_lang(&self, pixels: &[u8], width: u32, height: u32, lang: &str) -> Result<Vec<Line>, OcrError> {
        if width == 0 || height == 0 || pixels.is_empty() {
            return Err(OcrError::EmptyImage);
        }
        let code = lang.trim().to_ascii_lowercase();
        if code == "he" || code.starts_with("he-") || code.starts_with("he_") || code == "iw" || code == "heb" {
            // The bundled model reads only the Latin alphabet: on a Hebrew page it would invent
            // Latin text, so report what is missing instead.
            return recognize_tesseract(pixels, width, height, "heb+eng");
        }
        self.recognize(pixels, width, height)
    }
}

/// Locate the `tesseract` executable and optional `tessdata` directory.
fn find_tesseract() -> Option<(PathBuf, Option<PathBuf>)> {
    let exe_name = if cfg!(windows) { "tesseract.exe" } else { "tesseract" };
    let mut candidates = Vec::new();
    if let Some(p) = std::env::var_os("PDFCRAFT_TESSERACT") {
        candidates.push(PathBuf::from(p));
    }
    let mut exe_dirs: Vec<PathBuf> = std::env::current_exe().ok().and_then(|e| e.parent().map(Models::dirs_beside_exe)).unwrap_or_default();
    if cfg!(debug_assertions) {
        exe_dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models"));
        exe_dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models"));
    }
    for dir in exe_dirs {
        candidates.push(dir.join("tesseract").join(exe_name));
        candidates.push(dir.join(exe_name));
    }
    if cfg!(windows) {
        candidates.push(PathBuf::from(r"C:\Program Files\Tesseract-OCR\tesseract.exe"));
        candidates.push(PathBuf::from(r"C:\Program Files (x86)\Tesseract-OCR\tesseract.exe"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(local).join(r"Programs\Tesseract-OCR\tesseract.exe"));
        }
    }
    let bin = candidates.into_iter().find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(exe_name));

    let mut data_dirs = Vec::new();
    if let Some(d) = std::env::var_os("TESSDATA_PREFIX") {
        data_dirs.push(PathBuf::from(d));
    }
    for dir in Models::search_dirs() {
        data_dirs.push(dir.join("tessdata"));
        data_dirs.push(dir.join("tesseract").join("tessdata"));
        data_dirs.push(dir);
    }
    if let Some(parent) = bin.parent() {
        data_dirs.push(parent.join("tessdata"));
        data_dirs.push(parent.to_path_buf());
    }
    let tessdata = data_dirs.into_iter().find(|d| d.join("heb.traineddata").is_file());
    Some((bin, tessdata))
}

/// Run Tesseract on an image (`RGBA`, `RGB`, or 1-channel grey) and parse its TSV word boxes.
fn recognize_tesseract(pixels: &[u8], width: u32, height: u32, tess_lang: &str) -> Result<Vec<Line>, OcrError> {
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    let count = (width as usize).saturating_mul(height as usize);
    if pixels.len() == count.saturating_mul(4) {
        ppm.reserve(count.saturating_mul(3));
        for px in pixels.as_chunks::<4>().0 {
            ppm.extend_from_slice(&[px[0], px[1], px[2]]);
        }
    } else if pixels.len() == count.saturating_mul(3) {
        ppm.extend_from_slice(pixels);
    } else if pixels.len() == count {
        ppm.reserve(count.saturating_mul(3));
        for &g in pixels {
            ppm.extend_from_slice(&[g, g, g]);
        }
    } else {
        return Err(OcrError::EmptyImage);
    }

    for psm in ["3", "6", "11"] {
        let lines = run_tesseract_ppm(&ppm, tess_lang, psm)?;
        if !lines.is_empty() {
            return Ok(lines);
        }
    }
    Ok(Vec::new())
}

fn run_tesseract_ppm(ppm: &[u8], tess_lang: &str, psm: &str) -> Result<Vec<Line>, OcrError> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let (bin, tessdata) = find_tesseract().ok_or_else(|| OcrError::Recognize("Tesseract OCR was not found".into()))?;
    let mut cmd = Command::new(&bin);
    cmd.arg("stdin").arg("stdout").arg("-l").arg(tess_lang).arg("--psm").arg(psm);
    if let Some(dir) = tessdata {
        cmd.arg("--tessdata-dir").arg(dir);
    }
    cmd.arg("-c").arg("tessedit_create_tsv=1").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| OcrError::Recognize(format!("could not start {}: {e}", bin.display())))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(ppm);
    }
    let out = child.wait_with_output().map_err(|e| OcrError::Recognize(e.to_string()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(OcrError::Recognize(stderr.trim().to_string()));
    }
    let tsv = String::from_utf8_lossy(&out.stdout);
    let mut lines: Vec<((i32, i32, i32, i32), Line)> = Vec::new();
    for row in tsv.lines().skip(1) {
        let cols: Vec<&str> = row.split('\t').collect();
        let Some(&level) = cols.first() else { continue };
        if level != "5" || cols.len() < 12 {
            continue;
        }
        let text = cols[11].trim();
        if text.is_empty() {
            continue;
        }
        let conf: f32 = cols[10].parse().unwrap_or(-1.0);
        if conf < 0.0 {
            continue;
        }
        let page_num: i32 = cols[1].parse().unwrap_or(0);
        let block_num: i32 = cols[2].parse().unwrap_or(0);
        let par_num: i32 = cols[3].parse().unwrap_or(0);
        let line_num: i32 = cols[4].parse().unwrap_or(0);
        let left: f32 = cols[6].parse().unwrap_or(0.0);
        let top: f32 = cols[7].parse().unwrap_or(0.0);
        let w: f32 = cols[8].parse().unwrap_or(0.0);
        let h: f32 = cols[9].parse().unwrap_or(0.0);
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let key = (page_num, block_num, par_num, line_num);
        let word = Word { text: text.to_string(), rect: [left, top, left + w, top + h] };
        match lines.last_mut() {
            Some((k, line)) if *k == key => line.words.push(word),
            _ => lines.push((key, Line { words: vec![word] })),
        }
    }
    Ok(lines
        .into_iter()
        .map(|(_, mut l)| {
            l.words.retain(|w| !w.text.trim().is_empty());
            l
        })
        .filter(|l| !l.words.is_empty())
        .collect())
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

fn is_rtl_char(c: char) -> bool {
    !c.is_ascii_digit() && matches!(u32::from(c), 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

fn is_combining_mark(c: char) -> bool {
    matches!(u32::from(c), 0x0300..=0x036F | 0x0591..=0x05BD | 0x05BF | 0x05C1..=0x05C2 | 0x05C4..=0x05C5 | 0x05C7 | 0xFB1E)
}

fn mirror_bracket(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        other => other,
    }
}

/// Convert a word's logical text into visual left-to-right order for PDF drawing.
fn visual_word_text(text: &str) -> String {
    if !text.chars().any(is_rtl_char) {
        return text.to_string();
    }
    let mut clusters: Vec<(bool, String)> = Vec::new();
    for c in text.chars() {
        if is_combining_mark(c)
            && let Some((_, last)) = clusters.last_mut()
        {
            last.push(c);
            continue;
        }
        let prev_rtl = clusters.last().map_or(true, |(r, _)| *r);
        let rtl = if is_rtl_char(c) {
            true
        } else if c.is_alphanumeric() {
            false
        } else {
            prev_rtl
        };
        let mut s = String::new();
        s.push(if rtl { mirror_bracket(c) } else { c });
        clusters.push((rtl, s));
    }
    let mut runs: Vec<(bool, Vec<String>)> = Vec::new();
    for (rtl, cl) in clusters {
        match runs.last_mut() {
            Some((r, list)) if *r == rtl => list.push(cl),
            _ => runs.push((rtl, vec![cl])),
        }
    }
    runs.reverse();
    let mut out = String::with_capacity(text.len());
    for (rtl, mut list) in runs {
        if rtl {
            list.reverse();
        }
        for cl in list {
            out.push_str(&cl);
        }
    }
    out
}

/// Page content that writes `words` as invisible text in `/PCHelv` (standard Helvetica,
/// WinAnsiEncoding) or `/PCOCR` (Identity-H Unicode Type0 font for Hebrew and other non-WinAnsi
/// scripts), each word stretched to its box so selection and search highlight the right place.
/// Marked content `/OCR` so it can be told apart from the page's own text.
pub fn text_layer(words: &[PlacedWord]) -> Vec<u8> {
    // `3 Tr` and the font are graphics state, which `ET` doesn't reset: without `q … Q` they
    // would stay set for every later content stream on the page, so text added after OCR (a
    // header or footer) drew invisibly (#3029).
    let mut out = b"/OCR BMC\nq\nBT\n3 Tr\n/PCHelv 1 Tf\n".to_vec();
    let mut ordered: std::borrow::Cow<[PlacedWord]> = std::borrow::Cow::Borrowed(words);
    if words.iter().any(|w| w.text.chars().any(is_rtl_char)) {
        // PDF content streams draw visual lines from left to right so both the viewer's text
        // extractor (`reverse_rtl`) and Edit Text (`bidi::to_logical`) recover the full RTL line.
        let mut v = words.to_vec();
        let mut start = 0;
        while start < v.len() {
            let y0 = v[start].origin[1];
            let h0 = v[start].up[0].hypot(v[start].up[1]).max(1.0);
            let mut end = start + 1;
            while end < v.len() && (v[end].origin[1] - y0).abs() < h0 * 0.5 {
                end += 1;
            }
            if v[start..end].iter().any(|w| w.text.chars().any(is_rtl_char)) {
                v[start..end].sort_by(|a, b| a.origin[0].total_cmp(&b.origin[0]));
            }
            start = end;
        }
        ordered = std::borrow::Cow::Owned(v);
    }
    let mut using_pcocr = false;
    for w in ordered.iter() {
        let height = w.up[0].hypot(w.up[1]);
        if height <= 0.0 || w.text.is_empty() {
            continue;
        }
        let needs_unicode = w.text.chars().any(|c| pdfcraft_fonts::win_ansi_byte(c).is_none() || is_rtl_char(c));
        let (width, encoded) = if needs_unicode {
            let visual = visual_word_text(&w.text);
            let units: Vec<u16> = visual.encode_utf16().filter(|&u| u != 0 && u != 0xFFFF).collect();
            if units.is_empty() {
                continue;
            }
            let hex: String = units.iter().map(|u| format!("{u:04X}")).collect();
            (units.len() as f64 * 0.5, format!("<{hex}>").into_bytes())
        } else {
            let width = helvetica_width(&w.text, 1.0);
            if width <= 0.0 {
                continue;
            }
            (width, pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(&w.text)))
        };
        if needs_unicode != using_pcocr {
            out.extend_from_slice(if needs_unicode { b"/PCOCR 1 Tf\n" } else { b"/PCHelv 1 Tf\n" });
            using_pcocr = needs_unicode;
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
        out.extend(encoded);
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

    #[test]
    fn hebrew_text_layer_writes_visual_identity_h_hex() {
        let w = PlacedWord { text: "שלום".into(), origin: [100.0, 50.0], across: [40.0, 0.0], up: [0.0, 12.0] };
        let s = String::from_utf8(text_layer(&[w])).unwrap();
        // "שלום" in visual left-to-right order is "םולש": U+05DD, U+05D5, U+05DC, U+05E9.
        assert!(s.contains("/PCOCR 1 Tf"), "{s}");
        assert!(s.contains("<05DD05D505DC05E9> Tj"), "{s}");
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
