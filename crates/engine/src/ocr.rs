//! Scan & OCR ▸ Recognize text: render pages, read the words in them and add an invisible text
//! layer ([`Edit::AddOcrText`]), making scanned pages searchable and selectable.
//!
//! Recognition is slow (about a second a page), so it is split from the edit: [`Session::ocr_job`]
//! captures what it needs, [`OcrJob::run`] works anywhere (the UI runs it on a worker thread) and
//! [`Session::apply_ocr`] applies the result as one undoable step.

use std::sync::{Arc, Mutex};

use pdfcraft_render::{PageInfo, PageRenderer, RenderConfig, RenderRequest, RequestKind};

pub use pdfcraft_ocr::scan::Correction;
pub use pdfcraft_ocr::{LANGUAGES, Models, Ocr, OcrError, PlacedWord, language_for_locale};

use crate::{DocId, Edit, EditError, Session};

/// Recognize Text settings.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(not(target_arch = "wasm32"), serde(default, deny_unknown_fields))]
pub struct OcrSettings {
    /// Resolution pages are rendered at for recognition (Acrobat's "Downsample to" choices).
    pub dpi: f32,
    /// Language code from [`LANGUAGES`]; recognized using the multilingual model.
    pub language: String,
    /// Leave pages that already have text alone (Acrobat reports "page contains renderable
    /// text" and skips them). When `false`, such pages are read too (a picture on a page that
    /// also has text, #1028) and only the words that aren't already there are added.
    pub skip_text_pages: bool,
    pub auto_rotate: bool,
    pub deskew: bool,
    /// Detect the paper edge before alignment/OCR. Opt-in for ordinary recognition.
    pub auto_crop: bool,
    /// Extra paper-edge padding, in displayed PDF points (0..20).
    pub crop_margin: f64,
    /// Disable to straighten scans without adding a text layer.
    pub recognize: bool,
}

/// A recognised word whose box is at least this much covered by the page's own text is that
/// text read again, not a word in a picture.
const ALREADY_ON_PAGE: f32 = 0.5;

/// The long side of an A4 sheet in points (a letter page's, 792, is shorter): "Downsample to"
/// is a resolution on paper this size or smaller.
const SHEET: f32 = 842.0;

/// How much of `word` the `text` boxes cover, from 0 to 1. Boxes are `[x0, y0, x1, y1]` in page
/// view space. Overlaps between the text boxes are counted twice (glyphs rarely overlap), so the
/// sum is capped at the whole word.
fn covered(word: [f32; 4], text: &[[f32; 4]]) -> f32 {
    // `max(0.0)` also turns a NaN side into 0: a degenerate box covers nothing.
    let area = |[x0, y0, x1, y1]: [f32; 4]| (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let total = area(word);
    if !total.is_finite() || total <= 0.0 {
        return 0.0;
    }
    let inside: f32 =
        text.iter().map(|g| area([word[0].max(g[0]), word[1].max(g[1]), word[2].min(g[2]), word[3].min(g[3])])).filter(|a| a.is_finite()).sum();
    (inside / total).min(1.0)
}

/// Undo scan correction and crop offsets before testing overlap with original page text.
fn original_view_rect(word: [f32; 4], correction: Correction, crop: [u32; 4], points_per_pixel: [f32; 2]) -> [f32; 4] {
    let [l, t, r, b] = word;
    let [left, top, right, bottom] = crop;
    let corners = [(l, t), (r, t), (l, b), (r, b)].map(|(x, y)| {
        let [x, y] = correction.to_source(x as f64, y as f64, right.saturating_sub(left) as f64, bottom.saturating_sub(top) as f64);
        [(x + left as f64) as f32 * points_per_pixel[0], (y + top as f64) as f32 * points_per_pixel[1]]
    });
    [
        corners.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min),
        corners.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min),
        corners.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max),
        corners.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max),
    ]
}

impl Default for OcrSettings {
    fn default() -> Self {
        OcrSettings {
            dpi: 300.0,
            language: "en".into(),
            skip_text_pages: true,
            auto_rotate: true,
            deskew: true,
            auto_crop: false,
            crop_margin: 2.0,
            recognize: true,
        }
    }
}

/// What recognition found on one page.
#[derive(Clone, PartialEq)]
pub struct OcrPage {
    pub page: usize,
    pub words: Vec<PlacedWord>,
    /// Words recognised but left out because the page's own text already has them there.
    pub already_on_page: usize,
    /// Why the page was not read, if it was skipped.
    pub skipped: Option<String>,
    pub correction: Correction,
    /// Detected crop in original PDF user space, before rotation and deskew; None if uncertain.
    pub crop: Option<[f64; 4]>,
    /// Snapshot guard: never apply positions to pages changed while recognition was running.
    source: Arc<Vec<u8>>,
}

impl std::fmt::Debug for OcrPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OcrPage")
            .field("page", &self.page)
            .field("words", &self.words)
            .field("skipped", &self.skipped)
            .field("correction", &self.correction)
            .field("crop", &self.crop)
            .finish_non_exhaustive()
    }
}

impl OcrPage {
    pub fn changed(&self) -> bool {
        self.correction.changed() || self.crop.is_some()
    }
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

/// The recogniser, loaded once (it takes a moment) and shared.
pub fn engine() -> Result<Arc<Ocr>, String> {
    static OCR: Mutex<Option<Arc<Ocr>>> = Mutex::new(None);
    let mut slot = OCR.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(o) = slot.as_ref() {
        return Ok(o.clone());
    }
    let o = Arc::new(Ocr::find().map_err(|e| e.to_string())?);
    *slot = Some(o.clone());
    Ok(o)
}

/// Whether the recognition models are installed.
pub fn available() -> bool {
    Models::find().is_some()
}

/// Everything recognition needs from a document, detached from the session.
pub struct OcrJob {
    bytes: Arc<Vec<u8>>,
    password: Option<String>,
    infos: Vec<PageInfo>,
    /// Per page: the render scale (pixels per point) that shows its images at their own
    /// resolution, if it has any.
    native: Vec<Option<f32>>,
    pub pages: Vec<usize>,
    pub settings: OcrSettings,
}

impl OcrJob {
    /// Read the pages. `progress(done, total)` is called before each page; returning `false`
    /// stops (the pages read so far are returned).
    pub fn run(self, ocr: &Ocr, progress: impl FnMut(usize, usize) -> bool) -> Vec<OcrPage> {
        let failure = |why: &str| {
            self.pages
                .iter()
                .map(|&page| OcrPage {
                    page,
                    words: Vec::new(),
                    already_on_page: 0,
                    skipped: Some(why.into()),
                    correction: Correction::default(),
                    crop: None,
                    source: self.bytes.clone(),
                })
                .collect()
        };
        if !self.settings.dpi.is_finite()
            || !(72.0..=600.0).contains(&self.settings.dpi)
            || !LANGUAGES.iter().any(|l| l.0 == self.settings.language)
            || !self.settings.crop_margin.is_finite()
            || !(0.0..=20.0).contains(&self.settings.crop_margin)
        {
            return failure("invalid OCR language, resolution or crop margin (0..20 pt)");
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_inner(ocr, progress)))
            .unwrap_or_else(|_| failure("scan processing failed safely; the document was not changed"))
    }

    fn run_inner(&self, ocr: &Ocr, mut progress: impl FnMut(usize, usize) -> bool) -> Vec<OcrPage> {
        let config = RenderConfig { password: self.password.as_deref().map(Arc::from), ..Default::default() };
        let mut r = PageRenderer::new(self.bytes.clone(), config);
        let mut out = Vec::new();
        let total = self.pages.len();
        for (done, &page) in self.pages.iter().enumerate() {
            if !progress(done, total) {
                break;
            }
            let Some(info) = self.infos.get(page) else { continue };
            let skip = |why: &str| OcrPage {
                page,
                words: Vec::new(),
                already_on_page: 0,
                skipped: Some(why.into()),
                correction: Correction::default(),
                crop: None,
                source: self.bytes.clone(),
            };
            // The boxes of the page's own text (an earlier OCR layer's included), in view space.
            let existing: Vec<[f32; 4]> = r
                .render(RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() })
                .text
                .map(|t| t.glyphs.iter().filter(|g| !g.text.trim().is_empty()).map(|g| g.rect).collect())
                .unwrap_or_default();
            if self.settings.skip_text_pages && !existing.is_empty() {
                out.push(skip("the page already contains text"));
                continue;
            }
            let shot = r.render(RenderRequest { page, scale: self.scale(page, info), ..Default::default() });
            if let Some(e) = shot.error {
                out.push(skip(&e));
                continue;
            }
            let Some(rgba) = image::RgbaImage::from_raw(shot.width, shot.height, shot.rgba.to_vec()) else {
                out.push(skip("invalid rendered image"));
                continue;
            };
            let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
            let to_pdf = |x: f64, y: f64| {
                info.view_to_user(x as f32 * info.width / shot.width as f32, y as f32 * info.height / shot.height as f32).map(f64::from)
            };
            let bounds = self
                .settings
                .auto_crop
                .then(|| pdfcraft_ocr::crop::paper_bounds(&rgb))
                .flatten()
                .map(|[l, t, r, b]| {
                    let mx = (self.settings.crop_margin * shot.width as f64 / info.width as f64).ceil() as u32;
                    let my = (self.settings.crop_margin * shot.height as f64 / info.height as f64).ceil() as u32;
                    [l.saturating_sub(mx), t.saturating_sub(my), r.saturating_add(mx).min(shot.width), b.saturating_add(my).min(shot.height)]
                })
                .filter(|b| *b != [0, 0, shot.width, shot.height]);
            let crop = bounds.map(|[l, t, r, b]| {
                let a = to_pdf(l as f64, t as f64);
                let b = to_pdf(r as f64, b as f64);
                [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
            });
            let [left, top, right, bottom] = bounds.unwrap_or([0, 0, shot.width, shot.height]);
            let rgb = image::imageops::crop_imm(&rgb, left, top, right - left, bottom - top).to_image();
            let rotation = if self.settings.auto_rotate {
                match ocr.orientation(&rgb) {
                    Ok(r) => r,
                    Err(e) => {
                        out.push(skip(&e.to_string()));
                        continue;
                    }
                }
            } else {
                0
            };
            let mut correction = Correction { rotation, deskew: 0.0 };
            let turned = correction.apply(&rgb);
            if self.settings.deskew {
                correction.deskew = pdfcraft_ocr::scan::deskew(&turned);
            }
            let corrected = Correction { rotation: 0, ..correction }.apply(&turned);
            let result = if self.settings.recognize { ocr.recognize_language(&corrected, &self.settings.language) } else { Ok(Vec::new()) };
            let lines = match result {
                Ok(l) => l,
                Err(e) => {
                    out.push(skip(&e.to_string()));
                    continue;
                }
            };
            let to_user = |x: f32, y: f32| {
                let [x, y] = correction.to_source(x as f64, y as f64, rgb.width() as f64, rgb.height() as f64);
                to_pdf(x + left as f64, y + top as f64)
            };
            // Map recognized boxes back through crop/orientation before comparing
            // them with existing text in the original page's view coordinates.
            let (fresh, already): (Vec<_>, Vec<_>) = lines.iter().flat_map(|l| &l.words).partition(|w| {
                let rect = original_view_rect(
                    w.rect,
                    correction,
                    [left, top, right, bottom],
                    [info.width / shot.width as f32, info.height / shot.height as f32],
                );
                covered(rect, &existing) < ALREADY_ON_PAGE
            });
            let words = fresh.into_iter().map(|w| PlacedWord::place(w, to_user)).collect();
            out.push(OcrPage { page, words, already_on_page: already.len(), skipped: None, correction, crop, source: self.bytes.clone() });
        }
        progress(total, total);
        out
    }

    /// The render scale (pixels per point) `page` is read at. A scan is read at its own
    /// resolution (enlarging it only blurs the letters), at most the chosen one.
    fn scale(&self, page: usize, info: &PageInfo) -> f32 {
        let wanted = self.settings.dpi.clamp(72.0, 600.0) / 72.0;
        let Some(native) = self.native.get(page).copied().flatten() else { return wanted };
        // A scan whose page is larger than a sheet (often a box given in pixels, one point per
        // pixel, #1848) is measured as if shrunk to one: otherwise 1 px/pt is already its full
        // image, and no "Downsample to" choice could ask for less.
        let fit = (SHEET / info.width.max(info.height).max(1e-3)).min(1.0);
        native.clamp(fit, wanted * fit)
    }
}

impl Session {
    /// Capture what recognising `pages` (0-based; empty = all) of document `id` needs.
    pub fn ocr_job(&self, id: DocId, pages: &[usize], settings: OcrSettings) -> Option<OcrJob> {
        let doc = self.get(id)?;
        let all = doc.info.pages.len();
        let pages: Vec<usize> = if pages.is_empty() { (0..all).collect() } else { pages.iter().copied().filter(|p| *p < all).collect() };
        let native = (0..all)
            .map(|p| {
                if !pages.contains(&p) {
                    return None;
                }
                doc.page_images(p)
                    .iter()
                    .filter(|i| i.width > 1 && i.height > 1)
                    .map(|i| {
                        let m = i.matrix;
                        let (w, h) = (m[0].hypot(m[1]), m[2].hypot(m[3]));
                        (i.width as f64 / w.max(1e-6)).max(i.height as f64 / h.max(1e-6)) as f32
                    })
                    .reduce(f32::max)
            })
            .collect();
        Some(OcrJob { bytes: doc.bytes.clone(), password: doc.password.clone(), infos: doc.info.pages.clone(), native, pages, settings })
    }

    /// Add the words found by [`OcrJob::run`] as one undoable step. Returns the number of words.
    pub fn apply_ocr(&mut self, id: DocId, found: &[OcrPage]) -> Result<usize, EditError> {
        let doc = self.get(id).ok_or_else(|| EditError::Invalid("no such document".into()))?;
        if found.iter().any(|p| !Arc::ptr_eq(&p.source, &doc.bytes) && p.source.as_ref() != doc.bytes.as_ref()) {
            return Err(EditError::Invalid("the document changed during scan processing; run it again".into()));
        }
        let mut edits = Vec::new();
        for p in found.iter().filter(|p| p.skipped.is_none()) {
            if !p.words.is_empty() {
                edits.push(Edit::AddOcrText { page: p.page, words: p.words.clone() });
            }
            if let Some(rect) = p.crop {
                edits.push(Edit::SetPageBox { pages: vec![p.page], which: crate::PageBox::Crop, spec: crate::BoxSpec::Rect(rect) });
            }
            if p.correction.rotation != 0 {
                edits.push(Edit::RotatePages { pages: vec![p.page], degrees: i64::from(p.correction.rotation) });
            }
            if p.correction.deskew.abs() >= 0.05 {
                edits.push(Edit::DeskewPage { page: p.page, degrees: p.correction.deskew });
            }
        }
        let words = found.iter().map(|p| p.words.len()).sum();
        if !edits.is_empty() {
            self.apply(id, Edit::Batch { label: if words > 0 { "Recognize text" } else { "Correct scans" }.into(), edits })?;
        }
        Ok(words)
    }

    /// Recognise text on `pages` and add it, in one call (the CLI and agents use this).
    pub fn recognize_text(&mut self, id: DocId, pages: &[usize], settings: OcrSettings) -> Result<Vec<OcrPage>, String> {
        if !settings.dpi.is_finite() || !(72.0..=600.0).contains(&settings.dpi) {
            return Err("OCR resolution must be between 72 and 600 dpi".into());
        }
        if !LANGUAGES.iter().any(|l| l.0 == settings.language) {
            return Err(format!("unsupported language: {}", settings.language));
        }
        if !settings.crop_margin.is_finite() || !(0.0..=20.0).contains(&settings.crop_margin) {
            return Err("crop margin must be between 0 and 20 pt".into());
        }
        if let Some(doc) = self.get(id)
            && pages.iter().any(|&p| p >= doc.info.pages.len())
        {
            return Err("OCR page does not exist".into());
        }
        let job = self.ocr_job(id, pages, settings).ok_or("no such document")?;
        if let Some(why) = self.get(id).and_then(|d| d.read_only_reason.clone()) {
            return Err(why);
        }
        let ocr = engine()?;
        let found = job.run(&ocr, |_, _| true);
        self.apply_ocr(id, &found).map_err(|e| e.to_string())?;
        Ok(found)
    }
}

/// What [`recognize_file`] did to one file.
#[derive(Clone, Debug, PartialEq)]
pub struct FileResult {
    /// The new file (incrementally saved), or the original when nothing was recognised.
    pub bytes: Arc<Vec<u8>>,
    pub pages: Vec<OcrPage>,
}

impl FileResult {
    pub fn words(&self) -> usize {
        self.pages.iter().map(|p| p.words.len()).sum()
    }
}

/// Recognize text in multiple files: one PDF's bytes in, the searchable PDF out. `progress`
/// works as for [`OcrJob::run`].
pub fn recognize_file(
    name: &str,
    bytes: Arc<Vec<u8>>,
    password: Option<&str>,
    settings: OcrSettings,
    ocr: &Ocr,
    progress: impl FnMut(usize, usize) -> bool,
) -> Result<FileResult, String> {
    let mut s = Session::new();
    let id = s.open(name, None, bytes.clone(), password).map_err(|e| e.to_string())?;
    if let Some(why) = s.get(id).and_then(|d| d.read_only_reason.clone()) {
        return Err(why);
    }
    let job = s.ocr_job(id, &[], settings).ok_or("the document could not be read")?;
    let pages = job.run(ocr, progress);
    let words = s.apply_ocr(id, &pages).map_err(|e| e.to_string())?;
    if words == 0 && !pages.iter().any(|p| p.changed() && p.skipped.is_none()) {
        return Ok(FileResult { bytes, pages });
    }
    // Checked like a save before the caller writes it (#2166).
    let bytes = s.save_bytes(id).map_err(|e| e.to_string())?;
    s.check_save(id, &bytes).map_err(|e| e.to_string())?;
    Ok(FileResult { bytes, pages })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export;

    #[test]
    fn existing_text_is_detected_after_crop_and_each_quarter_turn() {
        let existing = [[15.0, 30.0, 25.0, 40.0]];
        for (rotation, word) in
            [(0, [10.0, 20.0, 30.0, 40.0]), (90, [60.0, 10.0, 80.0, 30.0]), (180, [170.0, 60.0, 190.0, 80.0]), (270, [20.0, 170.0, 40.0, 190.0])]
        {
            let rect = original_view_rect(word, Correction { rotation, deskew: 0.0 }, [20, 40, 220, 140], [0.5, 0.5]);
            assert_eq!(rect, existing[0], "rotation {rotation}");
            assert_eq!(covered(rect, &existing), 1.0);
            assert_eq!(covered(rect, &[[50.0, 50.0, 60.0, 60.0]]), 0.0);
        }
    }

    /// #1028: a recognised word over the page's own text is that text again; one in an empty
    /// part of the page (a picture) is new.
    #[test]
    fn words_over_existing_text_count_as_already_there() {
        // Glyph em boxes of "Total" at 12 pt, five letters side by side.
        let glyphs: Vec<[f32; 4]> = (0..5).map(|i| [100.0 + 7.0 * i as f32, 50.0, 107.0 + 7.0 * i as f32, 62.0]).collect();
        // OCR's tight box around the same ink.
        assert!(covered([100.5, 52.0, 134.0, 61.0], &glyphs) >= ALREADY_ON_PAGE);
        // A word in a picture further down, and one beside the text on the same line.
        assert_eq!(covered([100.0, 200.0, 140.0, 212.0], &glyphs), 0.0);
        assert_eq!(covered([140.0, 52.0, 170.0, 61.0], &glyphs), 0.0);
        // Touching the text's last letter is not enough.
        assert!(covered([131.0, 52.0, 170.0, 61.0], &glyphs) < ALREADY_ON_PAGE);
        // A page without text has nothing to cover a word.
        assert_eq!(covered([100.5, 52.0, 134.0, 61.0], &[]), 0.0);
    }

    /// #1848: an A4 scan at 300 dpi whose page box is its pixel size (2480 x 3508 pt, one pixel
    /// per point) gets a smaller bitmap for each lower "Downsample to" choice, and a low-resolution
    /// letter page is still never shrunk below one pixel per point.
    #[test]
    fn downsample_shrinks_a_page_box_given_in_pixels() {
        let mut s = Session::new();
        let page = |w: u32, h: u32| export::encode_png(w, h, &vec![255; (w * h * 4) as usize]).unwrap();
        let bytes = s.create_from_images(&[("a4.png".into(), page(2480, 3508)), ("letter.png".into(), page(612, 792))]).unwrap();
        let id = s.open("scan.pdf", None, bytes, None).unwrap();
        let mut r = PageRenderer::new(s.get(id).unwrap().bytes.clone(), RenderConfig::default());
        for (dpi, a4) in [(72.0, [595, 842]), (150.0, [1240, 1754]), (300.0, [2480, 3508]), (600.0, [2480, 3508])] {
            let job = s.ocr_job(id, &[], OcrSettings { dpi, ..Default::default() }).unwrap();
            for (page, want) in [(0, a4), (1, [612, 792])] {
                let scale = job.scale(page, &job.infos[page]);
                let shot = r.render(RenderRequest { page, scale, ..Default::default() });
                assert!(shot.error.is_none(), "{:?}", shot.error);
                let near = |got: u32, want: u32| got.abs_diff(want) <= 1;
                assert!(near(shot.width, want[0]) && near(shot.height, want[1]), "{dpi} dpi, page {page}: {}x{}", shot.width, shot.height);
            }
        }
    }

    #[test]
    fn coverage_of_overlapping_or_broken_boxes_stays_between_zero_and_one() {
        // A glyph drawn twice (fake bold) does not cover more than the whole word.
        let twice = [[0.0, 0.0, 10.0, 10.0], [0.0, 0.0, 10.0, 10.0]];
        assert_eq!(covered([0.0, 0.0, 10.0, 10.0], &twice), 1.0);
        // Empty, inverted and non-finite boxes cover nothing and are covered by nothing.
        for word in [[5.0, 5.0, 5.0, 9.0], [9.0, 0.0, 1.0, 9.0], [f32::NAN, 0.0, 9.0, 9.0], [0.0, 0.0, f32::INFINITY, 9.0]] {
            assert_eq!(covered(word, &twice), 0.0, "{word:?}");
        }
        let broken = [[f32::NAN; 4], [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY]];
        let c = covered([0.0, 0.0, 10.0, 10.0], &broken);
        assert!((0.0..=1.0).contains(&c), "{c}");
    }
}
