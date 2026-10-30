//! Scan & OCR ▸ Recognize text: render pages, read the words in them and add an invisible text
//! layer ([`Edit::AddOcrText`]), making scanned pages searchable and selectable.
//!
//! Recognition is slow (about a second a page), so it is split from the edit: [`Session::ocr_job`]
//! captures what it needs, [`OcrJob::run`] works anywhere (the UI runs it on a worker thread) and
//! [`Session::apply_ocr`] applies the result as one undoable step.

use std::sync::{Arc, Mutex};

use pdfcraft_render::{PageInfo, PageRenderer, RenderConfig, RenderRequest, RequestKind};

pub use pdfcraft_ocr::{LANGUAGES, Models, Ocr, OcrError, PlacedWord};

use crate::{DocId, Edit, EditError, Session};

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
    /// Save as PDF/A after recognition (Acrobat's "Make scans PDF/A compliant"): the document is
    /// converted to this level in the same undoable step, and what it still breaks is reported.
    pub pdfa: Option<pdfcraft_preflight::Level>,
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

impl Default for OcrSettings {
    fn default() -> Self {
        OcrSettings { dpi: 300.0, language: "en".into(), skip_text_pages: true, native_text_only: false, pdfa: None }
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
            let skip = |why: &str| OcrPage { page, words: Vec::new(), already_on_page: 0, skipped: Some(why.into()) };
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
            let lines = match ocr.recognize(&shot.rgba, shot.width, shot.height) {
                Ok(l) => l,
                Err(e) => {
                    out.push(skip(&e.to_string()));
                    continue;
                }
            };
            let scale = shot.width as f32 / info.width.max(1e-3);
            let to_user = |x: f32, y: f32| {
                let [u, v] = info.view_to_user(x / scale, y / scale);
                [u as f64, v as f64]
            };
            // Words the page's own text already has are not added a second time.
            let (fresh, already): (Vec<_>, Vec<_>) =
                lines.iter().flat_map(|l| &l.words).partition(|w| covered(w.rect.map(|v| v / scale), &existing) < ALREADY_ON_PAGE);
            let words = fresh.into_iter().map(|w| PlacedWord::place(w, to_user)).collect();
            out.push(OcrPage { page, words, already_on_page: already.len(), skipped: None });
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
        self.apply_ocr_with(id, found, &OcrSettings::default()).map(|a| a.words)
    }

    /// [`Self::apply_ocr`] with what `settings` ask for after recognition: with
    /// [`OcrSettings::pdfa`], the document is converted to PDF/A in the same undoable step
    /// (the text layer is invisible text, which needs no embedded font, ISO 19005-2
    /// §6.2.11.4.1) and checked afterwards. When the conversion can't be applied (security that
    /// doesn't allow removing the encryption, a signed file that would need a full rewrite) the
    /// words are still added and the reason is reported.
    pub fn apply_ocr_with(&mut self, id: DocId, found: &[OcrPage], settings: &OcrSettings) -> Result<OcrApplied, EditError> {
        let mut edits: Vec<Edit> =
            found.iter().filter(|p| !p.words.is_empty()).map(|p| Edit::AddOcrText { page: p.page, words: p.words.clone() }).collect();
        let words = found.iter().map(|p| p.words.len()).sum();
        let Some(level) = settings.pdfa else {
            if !edits.is_empty() {
                self.apply(id, Edit::Batch { label: "Recognize text".into(), edits })?;
            }
            return Ok(OcrApplied { words, pdfa: None });
        };
        let without = edits.clone();
        edits.push(Edit::ConvertPdfA { level });
        let refused = match self.apply(id, Edit::Batch { label: "Recognize text".into(), edits }) {
            Ok(()) => None,
            // Only the conversion can be refused where the words alone are fine.
            Err(e @ (EditError::NotPermitted(_) | EditError::SignedRewrite(_) | EditError::Invalid(_))) => {
                if !without.is_empty() {
                    self.apply(id, Edit::Batch { label: "Recognize text".into(), edits: without })?;
                }
                Some(e.to_string())
            }
            Err(e) => return Err(e),
        };
        let remaining = if refused.is_none() { self.get(id).map(|d| d.pdfa_verify(level)).unwrap_or_default() } else { Vec::new() };
        Ok(OcrApplied { words, pdfa: Some(OcrPdfA { level, refused, remaining }) })
    }

    /// Recognise text on `pages` and add it, in one call (the CLI and agents use this).
    pub fn recognize_text(&mut self, id: DocId, pages: &[usize], settings: OcrSettings) -> Result<Vec<OcrPage>, String> {
        self.recognize_text_reporting(id, pages, settings).map(|(found, _)| found)
    }

    /// [`Self::recognize_text`], also returning what was applied (the words, and the PDF/A
    /// conversion [`OcrSettings::pdfa`] asked for).
    pub fn recognize_text_reporting(&mut self, id: DocId, pages: &[usize], settings: OcrSettings) -> Result<(Vec<OcrPage>, OcrApplied), String> {
        let after = settings.clone();
        let job = self.ocr_job(id, pages, settings).ok_or("no such document")?;
        if let Some(why) = self.get(id).and_then(|d| d.read_only_reason.clone()) {
            return Err(why);
        }
        let ocr = engine()?;
        let found = job.run(&ocr, |_, _| true);
        let applied = self.apply_ocr_with(id, &found, &after).map_err(|e| e.to_string())?;
        Ok((found, applied))
    }
}

/// What [`Session::apply_ocr_with`] added and changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OcrApplied {
    pub words: usize,
    /// The PDF/A conversion, when [`OcrSettings::pdfa`] asked for one.
    pub pdfa: Option<OcrPdfA>,
}

/// How saving a recognised document as PDF/A went.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrPdfA {
    pub level: pdfcraft_preflight::Level,
    /// Why the conversion wasn't applied (the words were added without it).
    pub refused: Option<String>,
    /// What the converted document still breaks (empty: it complies, as far as PdfCraft checks).
    pub remaining: Vec<pdfcraft_preflight::Issue>,
}

impl OcrPdfA {
    /// Converted, and nothing left that PdfCraft's PDF/A check finds.
    pub fn compliant(&self) -> bool {
        self.refused.is_none() && self.remaining.is_empty()
    }
}

/// What [`recognize_file`] did to one file.
#[derive(Clone, Debug, PartialEq)]
pub struct FileResult {
    /// The new file (incrementally saved), or the original when nothing was recognised.
    pub bytes: Arc<Vec<u8>>,
    pub pages: Vec<OcrPage>,
    /// The PDF/A conversion ([`OcrSettings::pdfa`]).
    pub pdfa: Option<OcrPdfA>,
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
    let job = s.ocr_job(id, &[], settings.clone()).ok_or("the document could not be read")?;
    let pages = job.run(ocr, progress);
    let applied = s.apply_ocr_with(id, &pages, &settings).map_err(|e| e.to_string())?;
    let pdfa = applied.pdfa;
    // A file asked to be PDF/A is written converted even when no word was found.
    if applied.words == 0 && pdfa.as_ref().is_none_or(|p| p.refused.is_some()) {
        return Ok(FileResult { bytes, pages, pdfa });
    }
    // Checked like a save before the caller writes it (#2166).
    let bytes = s.save_bytes(id).map_err(|e| e.to_string())?;
    s.check_save(id, &bytes).map_err(|e| e.to_string())?;
    Ok(FileResult { bytes, pages, pdfa })
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

    /// A scan (a picture of a text page, no text of its own) and the words OCR would find on it.
    fn scan(s: &mut Session) -> (DocId, Vec<OcrPage>) {
        let text = s.create_from_text("t", "The quick brown fox jumps over the lazy dog.").unwrap();
        let id = s.open("text.pdf", None, text, None).unwrap();
        let png = export::Exporter::new(s.get(id).unwrap()).png(0, 150.0).unwrap();
        let scan = s.create_from_images_with_resolution(&[("scan.png".into(), png)], crate::ImageResolution::Dpi(150.0)).unwrap();
        let id = s.open("scan.pdf", None, scan, None).unwrap();
        let word = |text: &str, x: f64| PlacedWord { text: text.into(), origin: [x, 700.0], across: [40.0, 0.0], up: [0.0, 12.0] };
        let found = vec![OcrPage { page: 0, words: vec![word("quick", 100.0), word("lazy", 300.0)], already_on_page: 0, skipped: None }];
        (id, found)
    }

    fn font_issues(issues: &[pdfcraft_preflight::Issue]) -> Vec<&str> {
        issues.iter().filter(|i| i.clause == "6.2.11.4").map(|i| i.message.as_str()).collect()
    }

    /// Recognize text ▸ Save as PDF/A: the words and the conversion are one undo step, and the
    /// result passes the PDF/A-2b check, saved and opened again. The text layer's Helvetica isn't
    /// embedded, which PDF/A allows for invisible text (ISO 19005-2 §6.2.11.4.1); the same font
    /// drawn visibly is still reported.
    #[test]
    fn recognized_scans_can_be_saved_as_pdfa() {
        use pdfcraft_preflight::Level;
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let (id, found) = scan(&mut s);
        assert!(!s.get(id).unwrap().pdfa_verify(Level::A2b).is_empty(), "a plain scan isn't PDF/A");
        let settings = OcrSettings { pdfa: Some(Level::A2b), ..Default::default() };
        let a = s.apply_ocr_with(id, &found, &settings).unwrap();
        let pdfa = a.pdfa.expect("a PDF/A report");
        assert!(pdfa.compliant(), "{pdfa:#?}");
        assert_eq!((a.words, pdfa.level), (2, Level::A2b));
        assert_eq!(s.get(id).unwrap().can_undo(), Some("Recognize text"));
        let saved = s.save_bytes(id).unwrap();
        let back = pdfcraft_cos::Document::open(saved.clone()).unwrap();
        assert_eq!(pdfcraft_preflight::verify(&back, Level::A2b), []);
        assert_eq!(pdfcraft_preflight::declared(&back).pdfa, Some((2, "B".into())));
        let mut r = PageRenderer::new(saved, RenderConfig::default());
        let text = r.render(RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, ..Default::default() }).text.unwrap();
        assert!(text.glyphs.iter().map(|g| g.text.as_str()).collect::<String>().contains("quick"));
        // One undo takes both back.
        s.undo(id).unwrap();
        assert!(!s.get(id).unwrap().pdfa_verify(Level::A2b).is_empty());

        // Text drawn visibly in the same, unembedded font is reported, not hidden.
        let (id, found) = scan(&mut s);
        s.apply(
            id,
            Edit::AddText { page: 0, text: crate::AddedText { rect: [72.0, 72.0, 200.0, 100.0], text: "Stamped".into(), ..Default::default() } },
        )
        .unwrap();
        let a = s.apply_ocr_with(id, &found, &OcrSettings { pdfa: Some(Level::A3b), ..Default::default() }).unwrap();
        let pdfa = a.pdfa.unwrap();
        assert!(pdfa.refused.is_none() && !pdfa.compliant());
        assert_eq!(font_issues(&pdfa.remaining), ["The font Helvetica isn't embedded"], "{:#?}", pdfa.remaining);
    }

    /// When the document's security doesn't allow removing the encryption (which PDF/A forbids),
    /// the words are still added and the refusal is reported; without words, nothing changes.
    #[test]
    fn a_pdfa_conversion_the_security_refuses_still_adds_the_words() {
        let mut s = Session::new();
        let (plain, found) = scan(&mut s);
        let mut doc = pdfcraft_cos::Document::open(s.get(plain).unwrap().bytes.clone()).unwrap();
        doc.set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            user_password: "pw",
            owner_password: "owner",
            // Changes allowed; printing and copying withheld.
            permissions: !((1 << 2) | (1 << 4)),
            encrypt_metadata: true,
            seed: [7; 32],
        })
        .unwrap();
        let bytes = Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap());
        let id = s.open("locked.pdf", None, bytes, Some("pw")).unwrap();
        let settings = OcrSettings { pdfa: Some(pdfcraft_preflight::Level::A2b), ..Default::default() };
        let a = s.apply_ocr_with(id, &found, &settings).unwrap();
        let pdfa = a.pdfa.unwrap();
        assert!(pdfa.refused.as_deref().is_some_and(|r| r.contains("encryption")), "{pdfa:?}");
        assert_eq!(a.words, 2);
        assert_eq!(s.get(id).unwrap().can_undo(), Some("Recognize text"));
        let none: Vec<OcrPage> = Vec::new();
        s.undo(id).unwrap();
        let a = s.apply_ocr_with(id, &none, &settings).unwrap();
        assert!(a.pdfa.unwrap().refused.is_some());
        assert_eq!(s.get(id).unwrap().can_undo(), None);
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
