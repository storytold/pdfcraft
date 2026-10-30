//! Synthetic scans of the OCR ground-truth texts, and how well a recognition reads them.
//!
//! Shared by the regression tests (`tests/it/ocr_ground_truth.rs`) and the benchmarks
//! (`examples/ocr_bench.rs` and `examples/ocr_mdpbench.rs`, which include this file by path),
//! together with the scoring and the MDPBench file names and predictions the benchmarks share.
//! The texts live in `tests/data/ocr/<lang>[-<source>].tsv` (`doc<TAB>source<TAB>text`, `#`
//! comments). Each row is wrapped onto an A4 page in a committed OFL face, then degraded the way
//! a scanner or a phone camera does it: resolution, blur, sensor noise, a slight skew and JPEG
//! compression. Nothing is random between runs: the noise comes from a seeded generator.

#![allow(dead_code)] // The test and the examples each use a different part.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use pdfcraft_engine::ocr;

/// One ground-truth file.
pub struct Fixture {
    /// The OCR document language, from the file name (`de` for `de.tsv` and `de-wikipedia.tsv`).
    pub lang: String,
    pub file: String,
    pub docs: Vec<Doc>,
}

/// One document: its rows, each a printed line or a paragraph that is wrapped to the page.
pub struct Doc {
    pub name: String,
    pub source: String,
    pub rows: Vec<String>,
}

impl Doc {
    pub fn text(&self) -> String {
        self.rows.join("\n")
    }
}

pub fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ocr")
}

pub fn load(path: &Path) -> Result<Fixture, String> {
    let file = path.file_name().and_then(|f| f.to_str()).ok_or("no file name")?.to_string();
    let lang = file.split(['-', '.']).next().unwrap_or_default().to_string();
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut docs: Vec<Doc> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut cols = line.splitn(3, '\t');
        let (Some(name), Some(source), Some(row)) = (cols.next(), cols.next(), cols.next()) else {
            return Err(format!("{file}:{}: expected doc<TAB>source<TAB>text", n + 1));
        };
        match docs.last_mut() {
            Some(d) if d.name == name => d.rows.push(row.to_string()),
            _ => docs.push(Doc { name: name.into(), source: source.into(), rows: vec![row.into()] }),
        }
    }
    Ok(Fixture { lang, file, docs })
}

/// Every ground-truth file, in name order.
pub fn all() -> Result<Vec<Fixture>, String> {
    let dir = data_dir();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "tsv"))
        .collect();
    paths.sort();
    paths.iter().map(|p| load(p)).collect()
}

/// Whether PdfCraft has a recogniser for `language` (an [`ocr::LANGUAGES`] code).
pub fn ocr_language_known(language: &str) -> bool {
    ocr::LANGUAGES.iter().any(|(code, _)| *code == language)
}

/// The recogniser a fixture written in `lang` is read with: its own language when PdfCraft has a
/// recogniser for it, otherwise English (the ocrs models read the Latin alphabet without
/// accents, whatever the document's language).
pub fn recogniser_for(lang: &str) -> &'static str {
    ocr::LANGUAGES.iter().map(|(code, _)| *code).find(|code| *code == lang).unwrap_or("en")
}

/// A text face to set the scan in.
pub struct Face {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// The committed OFL faces (`assets/fonts/`): a sans and a monospace.
pub fn builtin_faces() -> Vec<Face> {
    vec![
        Face { name: "Inter".into(), bytes: include_bytes!("../../../../../assets/fonts/Inter-Regular.ttf").to_vec() },
        Face { name: "JetBrainsMono".into(), bytes: include_bytes!("../../../../../assets/fonts/JetBrainsMono-Regular.ttf").to_vec() },
    ]
}

/// How a page is scanned.
#[derive(Clone, Copy, Debug)]
pub struct Condition {
    pub name: &'static str,
    pub dpi: u32,
    /// Standard deviation of the sensor noise, in grey levels.
    pub noise: f32,
    /// Gaussian blur at 300 dpi, in pixels (scaled with the resolution).
    pub blur: f32,
    /// Clockwise skew in degrees.
    pub skew: f32,
    /// JPEG quality; `None` writes a PNG.
    pub jpeg: Option<u8>,
}

pub const CONDITIONS: &[Condition] = &[
    Condition { name: "300clean", dpi: 300, noise: 0.0, blur: 0.0, skew: 0.0, jpeg: None },
    Condition { name: "300noisy-skew1", dpi: 300, noise: 12.0, blur: 0.8, skew: 1.0, jpeg: Some(85) },
    Condition { name: "200noisy", dpi: 200, noise: 12.0, blur: 0.6, skew: 0.0, jpeg: Some(80) },
    Condition { name: "150jpeg65", dpi: 150, noise: 10.0, blur: 0.5, skew: 0.0, jpeg: Some(65) },
];

pub fn condition(name: &str) -> Option<Condition> {
    CONDITIONS.iter().copied().find(|c| c.name == name)
}

/// Text size on the page, in points.
const POINT_SIZE: f32 = 11.0;
/// Baseline to baseline, in multiples of the point size.
const LINE_SPACING: f32 = 1.9;
/// A4, in inches.
const PAGE: (f32, f32) = (8.27, 11.69);
const MARGIN: f32 = 0.9;

/// The scale that sets `font` at [`POINT_SIZE`] on a page scanned at `dpi`: an em of
/// `POINT_SIZE * dpi / 72` pixels. `Font::pt_to_px_scale` takes points at 96 dpi (it multiplies by
/// 96/72 itself), so the scan resolution goes in as a fraction of 96.
fn text_scale<F: Font>(font: &F, dpi: f32) -> PxScale {
    let em = POINT_SIZE * dpi / 72.0;
    // A face without units per em has no em to scale; its height stands in for it.
    font.pt_to_px_scale(POINT_SIZE * dpi / 96.0).unwrap_or(PxScale::from(em))
}

/// The characters of `text` that `face` has no glyph for.
pub fn missing_glyphs(face: &Face, text: &str) -> Vec<char> {
    let Ok(font) = FontRef::try_from_slice(&face.bytes) else { return text.chars().collect() };
    let mut missing: Vec<char> = text.chars().filter(|c| !c.is_whitespace() && font.glyph_id(*c).0 == 0).collect();
    missing.sort_unstable();
    missing.dedup();
    missing
}

/// `doc` set in `face` and scanned under `cond`: the encoded image (PNG or JPEG) at `cond.dpi`.
pub fn scan(doc: &Doc, face: &Face, cond: Condition, seed: u64) -> Result<Vec<u8>, String> {
    let font = FontRef::try_from_slice(&face.bytes).map_err(|e| format!("{}: {e}", face.name))?;
    let missing = missing_glyphs(face, &doc.text());
    if !missing.is_empty() {
        return Err(format!("{} has no glyph for {}", face.name, missing.iter().collect::<String>()));
    }
    let dpi = cond.dpi as f32;
    let (w, h) = ((PAGE.0 * dpi) as u32, (PAGE.1 * dpi) as u32);
    let scale = text_scale(&font, dpi);
    let scaled = font.as_scaled(scale);
    let left = MARGIN * dpi;
    let width = w as f32 - 2.0 * left;
    let mut page = image::GrayImage::from_pixel(w, h, image::Luma([255]));
    let mut baseline = 1.1 * dpi + scaled.ascent();
    for line in doc.rows.iter().flat_map(|r| wrap(r, &scaled, width)) {
        if baseline > h as f32 - MARGIN * dpi {
            return Err(format!("{}: the text does not fit on one page", doc.name));
        }
        let mut x = left;
        let mut previous = None;
        for ch in line.chars() {
            let id = scaled.glyph_id(ch);
            if let Some(p) = previous {
                x += scaled.kern(p, id);
            }
            if let Some(outline) = font.outline_glyph(id.with_scale_and_position(scale, point(x, baseline))) {
                let b = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    let (px, py) = (b.min.x as i64 + i64::from(gx), b.min.y as i64 + i64::from(gy));
                    if (0..i64::from(w)).contains(&px) && (0..i64::from(h)).contains(&py) {
                        let p = page.get_pixel_mut(px as u32, py as u32);
                        p.0[0] = p.0[0].min((255.0 * (1.0 - coverage.clamp(0.0, 1.0))) as u8);
                    }
                });
            }
            x += scaled.h_advance(id);
            previous = Some(id);
        }
        baseline += POINT_SIZE * dpi / 72.0 * LINE_SPACING;
    }
    let page = if cond.skew != 0.0 { rotate(&page, cond.skew) } else { page };
    let mut page = if cond.blur > 0.0 { image::imageops::blur(&page, cond.blur * dpi / 300.0) } else { page };
    if cond.noise > 0.0 {
        let mut rng = Rng(seed | 1);
        for p in page.pixels_mut() {
            p.0[0] = (f32::from(p.0[0]) + rng.gauss() * cond.noise).round().clamp(0.0, 255.0) as u8;
        }
    }
    let mut out = Vec::new();
    match cond.jpeg {
        Some(q) => {
            use image::ImageEncoder;
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q)
                .write_image(page.as_raw(), w, h, image::ExtendedColorType::L8)
                .map_err(|e| e.to_string())?;
        }
        None => page.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| e.to_string())?,
    }
    Ok(out)
}

/// Greedy wrapping at spaces; a word wider than the line stands alone.
fn wrap<F: Font>(row: &str, font: &ab_glyph::PxScaleFont<F>, width: f32) -> Vec<String> {
    let advance = |s: &str| s.chars().map(|c| font.h_advance(font.glyph_id(c))).sum::<f32>();
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in row.split(' ').filter(|w| !w.is_empty()) {
        let trial = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if cur.is_empty() || advance(&trial) <= width {
            cur = trial;
        } else {
            lines.push(std::mem::replace(&mut cur, word.to_string()));
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Rotate clockwise by `degrees` about the centre, bilinear, white where the page had nothing.
fn rotate(src: &image::GrayImage, degrees: f32) -> image::GrayImage {
    let (w, h) = src.dimensions();
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let at = |x: i64, y: i64| {
        if x < 0 || y < 0 || x >= i64::from(w) || y >= i64::from(h) { 255.0 } else { f32::from(src.get_pixel(x as u32, y as u32).0[0]) }
    };
    image::GrayImage::from_fn(w, h, |x, y| {
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        // The inverse rotation finds where this pixel came from.
        let (sx, sy) = (cos * dx + sin * dy + cx, -sin * dx + cos * dy + cy);
        let (x0, y0) = (sx.floor(), sy.floor());
        let (fx, fy) = (sx - x0, sy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
        let bottom = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
        image::Luma([(top * (1.0 - fy) + bottom * fy).round() as u8])
    })
}

/// xorshift64*, with Box–Muller for normally distributed noise.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 + 0.5) / (1u64 << 24) as f32
    }

    fn gauss(&mut self) -> f32 {
        let (u, v) = (self.next(), self.next());
        (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
    }
}

/// A stable seed for one page.
pub fn seed(parts: &[&str]) -> u64 {
    parts.iter().flat_map(|p| p.bytes().chain([0])).fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

/// How many words of the ground truth came back exactly, counted as a multiset so reading order
/// does not matter. Three overlapping classes: all words, words with a letter outside ASCII, and
/// tokens with a digit or a symbol (amounts, dates, `§`, `°C`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Score {
    pub words: usize,
    pub words_ok: usize,
    pub accented: usize,
    pub accented_ok: usize,
    pub symbols: usize,
    pub symbols_ok: usize,
    /// Words recognised that the ground truth does not have.
    pub extra: usize,
}

impl Score {
    pub fn add(&mut self, o: Score) {
        self.words += o.words;
        self.words_ok += o.words_ok;
        self.accented += o.accented;
        self.accented_ok += o.accented_ok;
        self.symbols += o.symbols;
        self.symbols_ok += o.symbols_ok;
        self.extra += o.extra;
    }

    pub fn words_rate(&self) -> f64 {
        rate(self.words_ok, self.words)
    }

    pub fn accented_rate(&self) -> f64 {
        rate(self.accented_ok, self.accented)
    }

    pub fn symbols_rate(&self) -> f64 {
        rate(self.symbols_ok, self.symbols)
    }
}

fn rate(ok: usize, all: usize) -> f64 {
    if all == 0 { 1.0 } else { ok as f64 / all as f64 }
}

/// Quotes, brackets and sentence punctuation around a word; a hyphen or a symbol inside it stays.
fn trim_punctuation(token: &str) -> &str {
    let t = token.trim_matches(|c: char| ".,;:!?\"'()[]{}„“”‚‘’«»‹›…".contains(c));
    if t.is_empty() { token } else { t }
}

pub fn tokens(text: &str) -> Vec<String> {
    text.split_whitespace().map(|t| trim_punctuation(t).to_string()).collect()
}

fn is_accented(t: &str) -> bool {
    t.chars().any(|c| c.is_alphabetic() && !c.is_ascii())
}

fn is_symbolic(t: &str) -> bool {
    t.chars().any(|c| c.is_ascii_digit() || !(c.is_alphanumeric() || c == '-' || c == '\''))
}

pub fn score(truth: &str, got: &str) -> Score {
    let mut have: BTreeMap<String, usize> = BTreeMap::new();
    for t in tokens(got) {
        *have.entry(t).or_default() += 1;
    }
    let mut s = Score::default();
    for t in tokens(truth) {
        let ok = match have.get_mut(&t) {
            Some(n) if *n > 0 => {
                *n -= 1;
                true
            }
            _ => false,
        };
        s.words += 1;
        s.words_ok += usize::from(ok);
        if is_accented(&t) {
            s.accented += 1;
            s.accented_ok += usize::from(ok);
        }
        if is_symbolic(&t) {
            s.symbols += 1;
            s.symbols_ok += usize::from(ok);
        }
    }
    s.extra = have.values().sum();
    s
}

/// The language of an MDPBench page image (`de_book_12.png` is `de`), when `file` is a plain file
/// name: no directory part, no leading dot, a language of letters, digits and hyphens. The
/// examples read `DIR/images/<file>` and write `OUT/<recogniser>/<stem>.md`, so a name from an
/// edited annotation file that is anything else is not read and nothing is written for it.
pub fn mdpbench_language(file: &str) -> Option<&str> {
    let plain = !file.starts_with('.') && !file.contains(['/', '\\']) && Path::new(file).file_name().is_some_and(|n| n == file);
    let (lang, _) = file.split_once('_').filter(|_| plain)?;
    (!lang.is_empty() && lang.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then_some(lang)
}

/// Writes one page's prediction for MDPBench's own evaluation to `out/<recogniser>/<image
/// stem>.md`, one paragraph per line of `text` (the engine currently returns a page as one line,
/// so a page is one paragraph; see #5157), and returns its path. A page that could not be
/// read (`text` is `None`) gets an empty prediction: the evaluation skips a page that has none,
/// which would leave the failures out of the score, while an empty one scores as nothing found.
pub fn write_prediction(out: &Path, recogniser: &str, file: &str, text: Option<&str>) -> std::io::Result<PathBuf> {
    let invalid = |what: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, what);
    if !ocr_language_known(recogniser) {
        return Err(invalid(format!("{recogniser} is not an OCR language")));
    }
    if mdpbench_language(file).is_none() {
        return Err(invalid(format!("{file} is not an MDPBench image name")));
    }
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    let dir = out.join(recogniser);
    let path = dir.join(format!("{stem}.md"));
    let body = match text {
        Some(text) => text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n\n") + "\n",
        None => String::new(),
    };
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&path, body)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoring_counts_words_as_a_multiset_and_classes_overlap() {
        let s = score("Größe 2,50 € „Größe“ und", "und Grösse 2,50 € Größe extra");
        assert_eq!((s.words, s.words_ok), (5, 4));
        assert_eq!((s.accented, s.accented_ok), (2, 1), "one Größe of two came back");
        assert_eq!((s.symbols, s.symbols_ok), (2, 2), "2,50 and €");
        assert_eq!(s.extra, 2, "Grösse and extra are not in the truth");
        assert_eq!(score("", "").words_rate(), 1.0);
    }

    #[test]
    fn rows_of_one_document_stay_together() {
        let dir = std::env::temp_dir().join(format!("ocr-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("de-test.tsv");
        std::fs::write(&path, "# comment\na\tsrc\teins\na\tsrc\tzwei\nb\tsrc\tdrei\n").unwrap();
        let f = load(&path).unwrap();
        assert_eq!(f.lang, "de");
        assert_eq!(f.docs.len(), 2);
        assert_eq!(f.docs[0].rows, ["eins", "zwei"]);
        std::fs::write(&path, "kaputt\n").unwrap();
        assert!(load(&path).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_scan_is_deterministic_and_has_the_requested_size() {
        let doc = Doc { name: "t".into(), source: "s".into(), rows: vec!["Größe".into()] };
        let face = &builtin_faces()[0];
        let cond = condition("150jpeg65").unwrap();
        let a = scan(&doc, face, cond, 7).unwrap();
        assert_eq!(a, scan(&doc, face, cond, 7).unwrap(), "same seed, same bytes");
        let img = image::load_from_memory(&a).unwrap();
        assert_eq!((img.width(), img.height()), ((8.27 * 150.0) as u32, (11.69 * 150.0) as u32));
        let missing = Doc { name: "t".into(), source: "s".into(), rows: vec!["Ⴀ".into()] };
        assert!(scan(&missing, face, cond, 7).unwrap_err().contains("no glyph"));
    }

    #[test]
    fn the_scan_sets_text_at_its_point_size() {
        let faces = builtin_faces();
        let font = FontRef::try_from_slice(&faces[0].bytes).unwrap();
        for dpi in [150.0, 300.0] {
            let scaled = font.as_scaled(text_scale(&font, dpi));
            let em = scaled.h_scale_factor() * font.units_per_em().unwrap();
            assert!((em - POINT_SIZE * dpi / 72.0).abs() < 0.01, "an em of {em} px at {dpi} dpi");
        }
    }

    #[test]
    fn an_mdpbench_image_name_is_a_plain_file_name() {
        assert_eq!(mdpbench_language("de_book_12.png"), Some("de"));
        assert_eq!(mdpbench_language("zh-t_book_1_indoor_light_top.jpg"), Some("zh-t"));
        let bad = [
            "../de_book_12.png",
            "images/de_book_12.png",
            "..\\de_x.png",
            "/etc/de_x.png",
            "de_../../outside.png",
            "de_x\\..\\..\\outside.png",
            ".._x.png",
            "._x.png",
            "_x.png",
            "nolang.png",
            "",
        ];
        for name in bad {
            assert_eq!(mdpbench_language(name), None, "{name}");
        }
    }

    #[test]
    fn a_page_that_failed_still_has_a_prediction() {
        let out = std::env::temp_dir().join(format!("ocr-pred-{}", std::process::id()));
        let read = write_prediction(&out, "en", "de_book_12.png", Some(" eins \n\nzwei\n")).unwrap();
        assert_eq!(read, out.join("en").join("de_book_12.md"));
        assert_eq!(std::fs::read_to_string(&read).unwrap(), "eins\n\nzwei\n");
        let failed = write_prediction(&out, "en", "de_book_13.png", None).unwrap();
        assert_eq!(std::fs::read_to_string(&failed).unwrap(), "", "an empty prediction, not none");
        assert!(write_prediction(&out, "en", "de_../../outside.png", Some("x")).is_err());
        assert!(write_prediction(&out, "..", "de_book_12.png", Some("x")).is_err());
        // A prediction that cannot be written is an error, not a page left out.
        let blocked = out.join("blocked");
        std::fs::write(&blocked, "a file where the folder should be").unwrap();
        assert!(write_prediction(&blocked, "en", "de_book_12.png", Some("x")).is_err());
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn a_fixture_is_read_with_its_own_recogniser_or_english() {
        assert_eq!(recogniser_for("en"), "en");
        assert_eq!(recogniser_for("tlh"), "en", "a language PdfCraft has no recogniser for falls back to English");
        for (code, _) in ocr::LANGUAGES {
            assert_eq!(recogniser_for(code), *code);
            assert!(ocr_language_known(code));
        }
        assert!(!ocr_language_known("tlh"));
    }
}
