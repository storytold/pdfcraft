//! Scan & OCR ▸ Recognize text: render pages, read the words in them and add an invisible text
//! layer ([`Edit::AddOcrText`]), making scanned pages searchable and selectable.
//!
//! Recognition is slow (about a second a page), so it is split from the edit: [`Session::ocr_job`]
//! captures what it needs, [`OcrJob::run`] works anywhere (the UI runs it on a worker thread) and
//! [`Session::apply_ocr`] applies the result as one undoable step.
//!
//! On a scan with no text of its own, recognition also straightens the page first, as Acrobat
//! does: a slightly turned scan image is turned level ([`OcrSettings::deskew`], [`ImageEdit::RotateBy`])
//! and a page scanned sideways or upside down gets the `/Rotate` that shows it upright
//! ([`OcrSettings::auto_rotate`]). Both are lossless: the image's pixels are not changed.

use std::sync::{Arc, Mutex};

use pdfcraft_render::{PageInfo, PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};

pub use pdfcraft_ocr::{LANGUAGES, Models, Ocr, OcrError, PlacedWord};

use pdfcraft_ocr::{Line, orient};

use crate::{DocId, Edit, EditError, ImageEdit, Session};

/// Recognize Text settings.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrSettings {
    /// Resolution pages are rendered at for recognition (Acrobat's "Downsample to" choices).
    pub dpi: f32,
    /// Language code from [`LANGUAGES`] (the models read the Latin alphabet, whatever this says).
    pub language: String,
    /// Leave pages that already have text alone (Acrobat reports "page contains renderable
    /// text" and skips them). When `false`, such pages are read too (a picture on a page that
    /// also has text, #1028) and only the words that aren't already there are added.
    pub skip_text_pages: bool,
    /// What counts as the page's text when `skip_text_pages` is on: only its own (page content
    /// and form fields). Comments and text boxes are left out, and are not drawn into the picture
    /// that is read, so a scan with a few comments on it is still a scan. `false`: everything
    /// the page shows.
    pub native_text_only: bool,
    /// Turn pages scanned sideways or upside down upright (their `/Rotate`), when their text
    /// only reads as words that way. Only pages without text of their own.
    pub auto_rotate: bool,
    /// Turn a slightly turned scan image level before reading it. Only pages without text of
    /// their own whose scan image covers at least half the page.
    pub deskew: bool,
}

/// A recognised word whose box is at least this much covered by the page's own text is that
/// text read again, not a word in a picture.
const ALREADY_ON_PAGE: f32 = 0.5;

/// A scan image covers at least this share of its page (as `scan_fields::is_scan` counts it).
const SCAN_COVERS: f64 = 0.5;

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

impl Default for OcrSettings {
    fn default() -> Self {
        OcrSettings { dpi: 300.0, language: "en".into(), skip_text_pages: true, native_text_only: false, auto_rotate: true, deskew: true }
    }
}

/// What recognition found on one page.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrPage {
    pub page: usize,
    pub words: Vec<PlacedWord>,
    /// Words recognised but left out because the page's own text already has them there.
    pub already_on_page: usize,
    /// Why the page was not read, if it was skipped.
    pub skipped: Option<String>,
    /// Quarter turns clockwise added to the page's `/Rotate` to show it upright.
    pub turn: u8,
    /// The scan image to level, if it needs it.
    pub straighten: Option<Straighten>,
    /// The document's edit generation when the page was read. Recognition runs in the
    /// background: [`Session::apply_ocr`] refuses the corrections when the document has changed
    /// since (a page turned, a scan moved or replaced, text added), as they were worked out for
    /// the page as it was.
    pub generation: u64,
}

/// A scan image that recognition levels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Straighten {
    /// Its index in `Document::page_images`.
    pub image: usize,
    /// The turn that levels it, in degrees counter-clockwise as displayed.
    pub degrees: f32,
}

impl OcrPage {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

/// The recogniser, loaded once (it takes a moment) and shared.
pub fn engine() -> Result<Arc<Ocr>, String> {
    static OCR: Mutex<Option<Arc<Ocr>>> = Mutex::new(None);
    let mut slot = OCR.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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

/// One page read by [`OcrJob::read`]: the quarter turns clockwise it was read at, the level it
/// was read at, and what it says (in the pixels of the levelled, turned rendering).
struct Reading {
    turn: u8,
    straighten: Option<Straighten>,
    lines: Vec<Line>,
}

/// Everything recognition needs from a document, detached from the session.
pub struct OcrJob {
    bytes: Arc<Vec<u8>>,
    password: Option<String>,
    infos: Vec<PageInfo>,
    /// Per page: the render scale (pixels per point) that shows its images at their own
    /// resolution, if it has any.
    native: Vec<Option<f32>>,
    /// Per page: its scan image, if one covers at least half of it (index in
    /// `Document::page_images`, and where it is in user space).
    scan_image: Vec<Option<(usize, [f64; 4])>>,
    /// The document's edit generation when the job was made.
    generation: u64,
    pub pages: Vec<usize>,
    pub settings: OcrSettings,
}

impl OcrJob {
    /// Read the pages. `progress(done, total)` is called before each page; returning `false`
    /// stops (the pages read so far are returned).
    pub fn run(self, ocr: &Ocr, mut progress: impl FnMut(usize, usize) -> bool) -> Vec<OcrPage> {
        let config =
            RenderConfig { password: self.password.as_deref().map(Arc::from), hide_comments: self.settings.native_text_only, ..Default::default() };
        let mut r = PageRenderer::new(self.bytes.clone(), config);
        let mut out = Vec::new();
        let total = self.pages.len();
        for (done, &page) in self.pages.iter().enumerate() {
            if !progress(done, total) {
                break;
            }
            let Some(info) = self.infos.get(page) else { continue };
            let generation = self.generation;
            let skip =
                |why: &str| OcrPage { page, words: Vec::new(), already_on_page: 0, skipped: Some(why.into()), turn: 0, straighten: None, generation };
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
            let scale = shot.width as f32 / info.width.max(1e-3);
            // Straightening a page with text of its own would move the picture from under it.
            let scan = if existing.is_empty() { self.scan_image.get(page).copied().flatten() } else { None };
            let correct = existing.is_empty() && (self.settings.auto_rotate || self.settings.deskew);
            let mut read = self.read(ocr, &shot, correct, scan, |[x, y]| info.user_to_view(x, y).map(|v| v * scale));
            // A level that the text doesn't confirm (a photo, a drawing) is not applied.
            if matches!(&read, Ok(r) if r.straighten.is_some() && orient::reads_as_words(&r.lines) != Some(true)) {
                read = self.read(ocr, &shot, correct, None, |_| [0.0; 2]);
            }
            let read = match read {
                Ok(r) => r,
                Err(e) => {
                    out.push(skip(&e.to_string()));
                    continue;
                }
            };
            let (w, h, turn) = (shot.width, shot.height, read.turn);
            let to_user = |x: f32, y: f32| {
                let [x, y] = orient::unturn([x, y], w, h, turn);
                let [u, v] = info.view_to_user(x / scale, y / scale);
                [u as f64, v as f64]
            };
            // Words the page's own text already has are not added a second time.
            let (fresh, already): (Vec<_>, Vec<_>) =
                read.lines.iter().flat_map(|l| &l.words).partition(|w| covered(w.rect.map(|v| v / scale), &existing) < ALREADY_ON_PAGE);
            let words: Vec<PlacedWord> = fresh.into_iter().map(|w| PlacedWord::place(w, to_user)).collect();
            // Nothing read: nothing confirms the corrections either.
            let (turn, straighten) = if words.is_empty() { (0, None) } else { (read.turn, read.straighten) };
            out.push(OcrPage { page, words, already_on_page: already.len(), skipped: None, turn, straighten, generation });
        }
        progress(total, total);
        out
    }

    /// Read one rendered page. With `correct`, first level it when
    /// `scan` (its scan image and where it is in user space; `to_view` maps user space to
    /// the rendering's pixels) is turned slightly and deskew is on, then read it turned each
    /// way until its text reads as words, when auto-rotate is on.
    fn read(
        &self,
        ocr: &Ocr,
        shot: &RenderedPage,
        correct: bool,
        scan: Option<(usize, [f64; 4])>,
        to_view: impl Fn([f32; 2]) -> [f32; 2],
    ) -> Result<Reading, OcrError> {
        let (rgba, width, height) = (&shot.rgba[..], shot.width, shot.height);
        let found = if correct { orient::skew(rgba, width, height) } else { None };
        let mut straighten = None;
        let level: std::borrow::Cow<[u8]> = match (found, scan) {
            (Some(s), Some((image, rect))) if self.settings.deskew && s.degrees != 0.0 => {
                straighten = Some(Straighten { image, degrees: s.degrees });
                // The same centre ImageEdit::RotateBy turns the image about.
                let centre = [((rect[0] + rect[2]) / 2.0) as f32, ((rect[1] + rect[3]) / 2.0) as f32];
                orient::rotate(rgba, width, height, s.degrees, to_view(centre)).into()
            }
            _ => rgba.into(),
        };
        // Lines running down the page: it lies on its side, so try the side turns first.
        let pairs: &[[u8; 2]] = match found {
            Some(s) if self.settings.auto_rotate && s.vertical => &[[1, 3], [0, 2]],
            Some(_) if self.settings.auto_rotate => &[[0, 2], [1, 3]],
            _ => &[],
        };
        let read_turned = |q: u8| {
            if q == 0 {
                return ocr.recognize(&level, width, height);
            }
            let (px, w, h) = orient::quarter_turn(&level, width, height, q);
            ocr.recognize(&px, w, h)
        };
        let mut upright = None;
        // Text read upside down still makes some lowercase "words", so each way is weighed
        // against the opposite one: the way up reads many more plausible characters.
        // ponytail: two recognitions a page (four when the lines' direction was misjudged); a
        // pixel-only up/down test (ascenders against descenders) would save one.
        for &[a, b] in pairs {
            let (la, lb) = (read_turned(a)?, read_turned(b)?);
            let (ga, gb) = (orient::plausibility(&la).0, orient::plausibility(&lb).0);
            let (q, lines, other) = if gb > ga { (b, lb, la) } else { (a, la, lb) };
            let verdict = orient::reads_as_words(&lines);
            if verdict == Some(true) {
                return Ok(Reading { turn: q, straighten, lines });
            }
            match (q, a, b) {
                (0, ..) => upright = Some(lines),
                (_, 0, _) | (_, _, 0) => upright = Some(other),
                _ => {}
            }
            // Too little text to tell which way up it is: no other way finds more.
            if verdict.is_none() {
                break;
            }
        }
        let lines = match upright {
            Some(l) => l,
            None => read_turned(0)?,
        };
        Ok(Reading { turn: 0, straighten, lines })
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
        let scan_image = (0..all)
            .map(|p| {
                let info = doc.info.pages.get(p)?;
                if !pages.contains(&p) {
                    return None;
                }
                let [cx0, cy0, cx1, cy1] = info.crop.map(f64::from);
                let page_area = (cx1 - cx0) * (cy1 - cy0);
                let (index, area, rect) = doc
                    .page_images(p)
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| !i.is_form && i.width > 1 && i.height > 1)
                    .map(|(n, i)| {
                        let r = i.visible_rect.unwrap_or(i.rect);
                        let w = (r[2].min(cx1) - r[0].max(cx0)).max(0.0);
                        let h = (r[3].min(cy1) - r[1].max(cy0)).max(0.0);
                        (n, w * h, r)
                    })
                    .filter(|(_, a, _)| a.is_finite())
                    .max_by(|a, b| a.1.total_cmp(&b.1))?;
                (page_area.is_finite() && area >= SCAN_COVERS * page_area).then_some((index, rect))
            })
            .collect();
        Some(OcrJob {
            bytes: doc.bytes.clone(),
            password: doc.password.clone(),
            infos: doc.info.pages.clone(),
            native,
            scan_image,
            generation: doc.edit_generation(),
            pages,
            settings,
        })
    }

    /// Add the words found by [`OcrJob::run`] as one undoable step. Returns the number of words.
    pub fn apply_ocr(&mut self, id: DocId, found: &[OcrPage]) -> Result<usize, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let changed = || EditError::Invalid("the document changed while its text was being recognised; recognise it again".into());
        let mut edits = Vec::new();
        for p in found.iter().filter(|p| !p.words.is_empty()) {
            // Both corrections are relative to the page as it was read.
            if (p.turn % 4 != 0 || p.straighten.is_some()) && p.generation != doc.edit_generation() {
                return Err(changed());
            }
            // The words are in user space, which neither correction moves.
            if let Some(s) = p.straighten {
                // RotateBy turns clockwise as drawn; the level is counter-clockwise.
                edits.push(Edit::EditPageImage { page: p.page, index: s.image, change: ImageEdit::RotateBy(-f64::from(s.degrees)) });
            }
            if p.turn % 4 != 0 {
                edits.push(Edit::RotatePages { pages: vec![p.page], degrees: 90 * i64::from(p.turn % 4) });
            }
            edits.push(Edit::AddOcrText { page: p.page, words: p.words.clone() });
        }
        let words = found.iter().map(|p| p.words.len()).sum();
        if !edits.is_empty() {
            self.apply(id, Edit::Batch { label: "Recognize text".into(), edits })?;
        }
        Ok(words)
    }

    /// Recognise text on `pages` and add it, in one call (the CLI and agents use this).
    pub fn recognize_text(&mut self, id: DocId, pages: &[usize], settings: OcrSettings) -> Result<Vec<OcrPage>, String> {
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
    if s.apply_ocr(id, &pages).map_err(|e| e.to_string())? == 0 {
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
