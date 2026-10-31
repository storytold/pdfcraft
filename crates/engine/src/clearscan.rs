//! Scan & OCR ▸ ClearScan: replace scanned lettering with Type 3 outlines.
//!
//! [`Session::clearscan_document`] renders each page, runs [`pdfcraft_ocr::clearscan`], and
//! replaces what the page shows (one undoable step, through Replace Pages, so comments, links
//! and bookmarks stay). A panic in the pipeline becomes a per-page error and the other pages
//! still run. The desktop dialog runs the same work as [`Session::clearscan_job`] on a worker
//! and [`Session::apply_clearscan`] on the UI thread; [`Session::clearscan_document`] is those
//! two steps in one call.

use std::sync::Arc;

use pdfcraft_render::{PageInfo, PageRenderer, RenderConfig, RenderRequest, RequestKind};

use crate::ocr::{self, Ocr};
use crate::{DocId, Edit, EditError, Session};

/// The long side of an A4 sheet in points. Matches Recognize Text's downsample rule.
const SHEET: f32 = 842.0;

/// ClearScan settings. `dpi` is the resolution a page is rendered at (72–600). `sensitivity`
/// is 0 (darkest ink only) … 100 (faint strokes); 50 is the Sauvola paper default.
/// `downsample` is an integer 1–8 applied after rendering. `language` must be one the models
/// read; the current models read Latin regardless of the hint.
#[derive(Clone, Debug, PartialEq)]
pub struct ClearScanSettings {
    pub dpi: f32,
    pub language: String,
    pub sensitivity: u8,
    pub downsample: u8,
    pub skip_text_pages: bool,
}

impl Default for ClearScanSettings {
    fn default() -> Self {
        ClearScanSettings { dpi: 300.0, language: "en".into(), sensitivity: 50, downsample: 1, skip_text_pages: true }
    }
}

impl ClearScanSettings {
    fn checked(&self) -> Result<(), ClearScanError> {
        if !self.dpi.is_finite() || !(72.0..=600.0).contains(&self.dpi) {
            return Err(ClearScanError::Failed(format!("dpi must be a finite number from 72 to 600, got {}", self.dpi)));
        }
        if self.sensitivity > 100 {
            return Err(ClearScanError::Failed(format!("sensitivity must be 0..=100, got {}", self.sensitivity)));
        }
        if !(1..=8).contains(&self.downsample) {
            return Err(ClearScanError::Failed(format!("downsample must be 1..=8, got {}", self.downsample)));
        }
        if !ocr::LANGUAGES.iter().any(|(code, _)| *code == self.language) {
            return Err(ClearScanError::Failed(format!("unsupported language {:?}", self.language)));
        }
        Ok(())
    }
}

/// Why ClearScan stopped.
#[derive(Debug, thiserror::Error)]
pub enum ClearScanError {
    /// The text recognition models are not installed. The text matches [`ocr::OcrError::NoModels`].
    #[error("the text recognition models are not installed (reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them)")]
    NoModels,
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Edit(#[from] EditError),
}

/// One progress event. `page` is 0-based. `stage` is `render`, a pipeline stage, `skipped`,
/// `done` or the page's error is reported on the result rather than here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClearScanProgress {
    pub page: usize,
    pub done: usize,
    pub total: usize,
    pub stage: &'static str,
}

/// What one page became.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClearScanPage {
    pub page: usize,
    pub text: String,
    pub glyphs: usize,
    pub clusters: usize,
    pub erased: u64,
    pub bytes: usize,
    pub skipped: Option<String>,
    pub error: Option<String>,
}

/// The pages ClearScan visited. `cancelled` is set when the progress callback returned `false`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClearScanReport {
    pub pages: Vec<ClearScanPage>,
    pub cancelled: bool,
}

/// What [`Session::clearscan_job`] captured, detached from the session so a worker can run it.
pub struct ClearScanJob {
    bytes: Arc<Vec<u8>>,
    password: Option<String>,
    infos: Vec<PageInfo>,
    native: Vec<Option<f32>>,
    /// Pages to read, 0-based, already checked.
    pub pages: Vec<usize>,
    settings: ClearScanSettings,
}

/// One page of a [`ClearScanJob`] at a time. The desktop drains this on a worker.
/// The web runs one [`ClearScanCursor::step`] per frame, so the browser can paint and Cancel.
pub struct ClearScanCursor {
    renderer: PageRenderer,
    infos: Vec<PageInfo>,
    native: Vec<Option<f32>>,
    pages: Vec<usize>,
    settings: ClearScanSettings,
    scan_options: pdfcraft_ocr::clearscan::ClearScanOptions,
    /// The next index into `pages`.
    index: usize,
    recognizer: Option<Arc<Ocr>>,
    report: Vec<ClearScanPage>,
    built: Vec<(usize, Vec<u8>)>,
}

/// Pages a [`ClearScanJob`] built, plus the report. [`Session::apply_clearscan`] turns `built`
/// into one undo step. Empty `built` applies nothing.
pub struct ClearScanBuilt {
    pub report: ClearScanReport,
    pub built: Vec<(usize, Vec<u8>)>,
}

impl ClearScanJob {
    /// A run that advances one page per [`ClearScanCursor::step`].
    pub fn cursor(self) -> ClearScanCursor {
        let ClearScanJob { bytes, password, infos, native, pages, settings } = self;
        let config = RenderConfig { password: password.as_deref().map(Arc::from), hide_comments: true, ..Default::default() };
        let scan_options = scan_options(&settings);
        ClearScanCursor {
            renderer: PageRenderer::new(bytes, config),
            infos,
            native,
            pages,
            settings,
            scan_options,
            index: 0,
            recognizer: None,
            report: Vec::new(),
            built: Vec::new(),
        }
    }

    /// Run the pipeline. Returning `false` from `progress` stops before the next page. Pages
    /// already built are in [`ClearScanBuilt::built`]; the caller still applies them. Missing
    /// models are [`ClearScanError::NoModels`] and `built` is not returned.
    pub fn run(self, mut progress: impl FnMut(ClearScanProgress) -> bool) -> Result<ClearScanBuilt, ClearScanError> {
        let mut cursor = self.cursor();
        loop {
            if let Some(built) = cursor.step(&mut progress)? {
                return Ok(built);
            }
        }
    }
}

impl ClearScanCursor {
    /// Read the next page. `None` means pages remain. `Some` is the finished run, including a
    /// cancel: the callback returned `false` before this page, or during it, and earlier pages
    /// are in [`ClearScanBuilt::built`]. A `Some` result ends the cursor.
    pub fn step(&mut self, mut progress: impl FnMut(ClearScanProgress) -> bool) -> Result<Option<ClearScanBuilt>, ClearScanError> {
        let Some(page) = self.pages.get(self.index).copied() else {
            return Ok(Some(self.finish(false)));
        };
        let done = self.index;
        let total = self.pages.len();
        if !progress(ClearScanProgress { page, done, total, stage: "render" }) {
            return Ok(Some(self.finish(true)));
        }
        let Some(info) = self.infos.get(page).cloned() else {
            self.report.push(failed_page(page, "the page is not in the document"));
            return Ok(self.advance(false));
        };
        if self.settings.skip_text_pages && page_has_text(&mut self.renderer, page) {
            let _ = progress(ClearScanProgress { page, done, total, stage: "skipped" });
            self.report.push(ClearScanPage {
                page,
                text: String::new(),
                glyphs: 0,
                clusters: 0,
                erased: 0,
                bytes: 0,
                skipped: Some("the page already contains text".into()),
                error: None,
            });
            let _ = progress(ClearScanProgress { page, done: done.saturating_add(1), total, stage: "done" });
            return Ok(self.advance(false));
        }
        let native = self.native.get(page).copied().flatten();
        let scale = render_scale(self.settings.dpi, &info, native);
        let shot = self.renderer.render(RenderRequest { page, scale, ..Default::default() });
        if let Some(error) = shot.error {
            self.report.push(failed_page(page, &format!("page {} could not be rendered: {error}", page.saturating_add(1))));
            return Ok(self.advance(false));
        }
        if shot.width == 0 || shot.height == 0 {
            self.report.push(failed_page(page, "the rendered page is empty"));
            return Ok(self.advance(false));
        }
        let ppp = f64::from(info.width) / f64::from(shot.width.max(1));
        if !ppp.is_finite() || ppp <= 0.0 {
            self.report.push(failed_page(page, "the page scale is not a finite number"));
            return Ok(self.advance(false));
        }
        if self.recognizer.is_none() {
            if !ocr::available() {
                return Err(ClearScanError::NoModels);
            }
            self.recognizer = Some(ocr::engine().map_err(ClearScanError::Failed)?);
        }
        let Some(ocr) = self.recognizer.clone() else {
            return Err(ClearScanError::Failed("text recognition did not start".into()));
        };
        let mut stop = false;
        let outcome = crate::guard(|| {
            pdfcraft_ocr::clearscan::clearscan_recognized(
                &ocr,
                pdfcraft_ocr::clearscan::ScanRaster {
                    pixels: &shot.rgba,
                    width: shot.width,
                    height: shot.height,
                    channels: 4,
                    points_per_pixel: ppp,
                },
                &self.scan_options,
                |stage| {
                    if !progress(ClearScanProgress { page, done, total, stage: stage.as_str() }) {
                        stop = true;
                    }
                },
            )
        });
        match outcome {
            Ok(Ok(out)) => {
                let bytes = out.pdf.len();
                self.report.push(ClearScanPage {
                    page,
                    text: out.text,
                    glyphs: out.glyphs,
                    clusters: out.clusters,
                    erased: out.erased,
                    bytes,
                    skipped: None,
                    error: None,
                });
                self.built.push((page, out.pdf));
            }
            Ok(Err(error)) => self.report.push(failed_page(page, &error.to_string())),
            Err(panic) => self.report.push(failed_page(page, &format!("internal error: {panic}"))),
        }
        let _ = progress(ClearScanProgress { page, done: done.saturating_add(1), total, stage: "done" });
        Ok(self.advance(stop))
    }

    /// Count this page as visited. `None` when another page is still waiting.
    fn advance(&mut self, cancelled: bool) -> Option<ClearScanBuilt> {
        self.index = self.index.saturating_add(1);
        if cancelled || self.index >= self.pages.len() { Some(self.finish(cancelled)) } else { None }
    }

    fn finish(&mut self, cancelled: bool) -> ClearScanBuilt {
        ClearScanBuilt { report: ClearScanReport { pages: std::mem::take(&mut self.report), cancelled }, built: std::mem::take(&mut self.built) }
    }
}

impl Session {
    /// ClearScan one page (0-based).
    pub fn clearscan_page(
        &mut self,
        id: DocId,
        page: usize,
        settings: ClearScanSettings,
        progress: impl FnMut(ClearScanProgress) -> bool,
    ) -> Result<ClearScanReport, ClearScanError> {
        self.clearscan_document(id, &[page], settings, progress)
    }

    /// Capture what ClearScan of `pages` (0-based; empty = every page) needs. The check is the
    /// same one [`Self::clearscan_document`] makes before any page is read.
    pub fn clearscan_job(&self, id: DocId, pages: &[usize], settings: ClearScanSettings) -> Result<ClearScanJob, ClearScanError> {
        settings.checked()?;
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if let Some(why) = &doc.read_only_reason {
            return Err(ClearScanError::Edit(EditError::ReadOnly(why.clone())));
        }
        let count = doc.info.pages.len();
        let pages = resolve_pages(count, pages)?;
        let native = (0..count).map(|page| native_scale(doc, page)).collect();
        Ok(ClearScanJob { bytes: doc.bytes.clone(), password: doc.password.clone(), infos: doc.info.pages.clone(), native, pages, settings })
    }

    /// ClearScan `pages` (0-based; empty = every page). Successful pages are one undoable
    /// "ClearScan" step. A page that already has text is skipped when `skip_text_pages` is set.
    /// Returning `false` from `progress` stops before the next page; pages already built are
    /// still applied.
    pub fn clearscan_document(
        &mut self,
        id: DocId,
        pages: &[usize],
        settings: ClearScanSettings,
        progress: impl FnMut(ClearScanProgress) -> bool,
    ) -> Result<ClearScanReport, ClearScanError> {
        let job = self.clearscan_job(id, pages, settings)?;
        let built = job.run(progress)?;
        let report = built.report;
        self.apply_clearscan(id, built.built)?;
        Ok(report)
    }

    /// Apply pages a [`ClearScanJob`] already built, as one undoable "ClearScan" step.
    pub fn apply_clearscan(&mut self, id: DocId, built: Vec<(usize, Vec<u8>)>) -> Result<(), ClearScanError> {
        self.apply_built(id, built)
    }

    fn apply_built(&mut self, id: DocId, built: Vec<(usize, Vec<u8>)>) -> Result<(), ClearScanError> {
        if built.is_empty() {
            return Ok(());
        }
        let edits = built
            .into_iter()
            .map(|(page, pdf)| Edit::ReplacePages {
                pages: vec![page],
                name: format!("clearscan-{}.pdf", page.saturating_add(1)),
                bytes: Arc::new(pdf),
                src_pages: vec![0],
            })
            .collect();
        self.apply(id, Edit::Batch { label: "ClearScan".into(), edits })?;
        Ok(())
    }
}

fn failed_page(page: usize, error: &str) -> ClearScanPage {
    ClearScanPage { page, text: String::new(), glyphs: 0, clusters: 0, erased: 0, bytes: 0, skipped: None, error: Some(error.to_string()) }
}

fn scan_options(settings: &ClearScanSettings) -> pdfcraft_ocr::clearscan::ClearScanOptions {
    let base = pdfcraft_ocr::clearscan::ClearScanOptions::default().with_sensitivity(settings.sensitivity);
    pdfcraft_ocr::clearscan::ClearScanOptions { downsample: u32::from(settings.downsample), ..base }
}

fn resolve_pages(count: usize, requested: &[usize]) -> Result<Vec<usize>, ClearScanError> {
    if requested.is_empty() {
        return Ok((0..count).collect());
    }
    let mut seen = std::collections::HashSet::new();
    let mut pages = Vec::new();
    for &page in requested {
        if page >= count {
            return Err(ClearScanError::Failed(format!("there is no page {}", page.saturating_add(1))));
        }
        if seen.insert(page) {
            pages.push(page);
        }
    }
    Ok(pages)
}

fn native_scale(doc: &crate::Document, page: usize) -> Option<f32> {
    doc.page_images(page)
        .iter()
        .filter(|image| image.width > 1 && image.height > 1)
        .map(|image| {
            let m = image.matrix;
            let (w, h) = (m[0].hypot(m[1]), m[2].hypot(m[3]));
            (image.width as f64 / w.max(1e-6)).max(image.height as f64 / h.max(1e-6)) as f32
        })
        .filter(|scale| scale.is_finite())
        .reduce(f32::max)
}

fn render_scale(dpi: f32, info: &PageInfo, native: Option<f32>) -> f32 {
    let wanted = dpi.clamp(72.0, 600.0) / 72.0;
    let fit = (SHEET / info.width.max(info.height).max(1e-3)).min(1.0);
    let scale = match native {
        Some(native) if native.is_finite() => native.clamp(fit, wanted * fit),
        _ => wanted,
    };
    let area = f64::from(info.width.max(1.0)) * f64::from(info.height.max(1.0));
    let cap = (pdfcraft_ocr::grey::MAX_PIXELS as f64 / area).sqrt();
    let cap = if cap.is_finite() { cap as f32 } else { scale };
    if !scale.is_finite() {
        return 1.0;
    }
    scale.min(cap).max(0.05)
}

fn page_has_text(renderer: &mut PageRenderer, page: usize) -> bool {
    let shot = renderer.render(RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
    shot.text.as_ref().is_some_and(|text| text.glyphs.iter().any(|glyph| !glyph.text.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_doc() -> (Session, DocId) {
        let mut session = Session::new();
        let bytes = session.create_from_text("notes", "Hello clearscan").expect("text");
        let id = session.open("notes.pdf", None, bytes, None).expect("open");
        (session, id)
    }

    #[test]
    fn clearscan_skips_a_page_that_already_has_text_and_does_not_edit_it() {
        let (mut session, id) = text_doc();
        let report = session.clearscan_document(id, &[], ClearScanSettings::default(), |_| true).expect("clearscan");
        assert!(!report.cancelled);
        assert!(report.pages.iter().all(|page| page.skipped.as_deref() == Some("the page already contains text")), "{report:?}");
        assert!(session.undo(id).is_err());
    }

    #[test]
    fn clearscan_refuses_a_missing_page_and_bad_settings() {
        let (mut session, id) = text_doc();
        let missing = session.clearscan_page(id, 4, ClearScanSettings::default(), |_| true).expect_err("page");
        assert!(missing.to_string().contains("no page 5"), "{missing}");
        let dpi =
            session.clearscan_document(id, &[], ClearScanSettings { dpi: f32::NAN, ..ClearScanSettings::default() }, |_| true).expect_err("dpi");
        assert!(dpi.to_string().contains("dpi"), "{dpi}");
        let language = session
            .clearscan_document(id, &[], ClearScanSettings { language: "xx".into(), ..ClearScanSettings::default() }, |_| true)
            .expect_err("language");
        assert!(language.to_string().contains("language"), "{language}");
        assert!(session.clearscan_page(DocId(999), 0, ClearScanSettings::default(), |_| true).is_err());
        assert!(session.undo(id).is_err());
    }

    #[test]
    fn a_cancelled_clearscan_leaves_the_document() {
        let (mut session, id) = text_doc();
        let report = session.clearscan_document(id, &[], ClearScanSettings::default(), |_| false).expect("cancel");
        assert!(report.cancelled);
        assert!(report.pages.is_empty(), "{report:?}");
        assert!(session.undo(id).is_err());
    }

    #[test]
    fn a_clearscan_cursor_reads_one_page_and_cancel_keeps_it() {
        let mut session = Session::new();
        let first = session.create_from_text("a", "Alpha page").expect("first");
        let id = session.open("a.pdf", None, first, None).expect("open");
        let second = session.create_from_text("b", "Beta page").expect("second");
        session
            .apply(
                id,
                Edit::InsertPagesFrom {
                    name: "b.pdf".into(),
                    bytes: second,
                    pages: None,
                    at: 1,
                    field_conflicts: pdfcraft_organize::FieldConflicts::Rename,
                },
            )
            .expect("insert");
        let job = session.clearscan_job(id, &[], ClearScanSettings::default()).expect("job");
        assert_eq!(job.pages.len(), 2);
        let mut cursor = job.cursor();
        let mut finished_pages = 0_usize;
        let pending = cursor
            .step(|event| {
                if event.stage == "done" {
                    finished_pages = finished_pages.saturating_add(1);
                }
                true
            })
            .expect("first page");
        assert!(pending.is_none(), "the second page is still waiting");
        assert_eq!(finished_pages, 1);
        let built = cursor.step(|_| false).expect("cancel").expect("the cancel finishes the run");
        assert!(built.report.cancelled);
        assert_eq!(built.report.pages.len(), 1, "{:?}", built.report);
        assert!(built.report.pages[0].skipped.is_some());
        assert!(built.built.is_empty());
    }

    #[test]
    fn a_picture_page_is_a_missing_model_error_or_one_undo_step() {
        let mut session = Session::new();
        let png = crate::export::encode_png(32, 32, &vec![255_u8; 32 * 32 * 4]).expect("png");
        let bytes = session.create_from_images(&[("blank.png".into(), png)]).expect("images");
        let id = session.open("blank.pdf", None, bytes, None).expect("open");
        match session.clearscan_page(id, 0, ClearScanSettings::default(), |_| true) {
            Err(ClearScanError::NoModels) => assert!(session.undo(id).is_err()),
            Ok(report) => {
                let page = report.pages.first().expect("page");
                assert!(page.skipped.is_none(), "{page:?}");
                if page.error.is_none() {
                    assert!(page.bytes > 0, "{page:?}");
                    assert!(session.undo(id).is_ok());
                }
            }
            Err(error) => panic!("{error}"),
        }
    }

    /// The session applies a ClearScan page the same way `clearscan_document` does, without the
    /// recognition models: the letter stays searchable, and undo puts the blank page back.
    #[test]
    fn replacing_a_page_with_clearscan_output_is_searchable_and_undoes() {
        let (width, height) = (64_u32, 48_u32);
        let mut pixels = vec![250_u8; (width * height) as usize];
        for y in 10..30 {
            for x in 8..12 {
                let index = y as usize * width as usize + x as usize;
                if let Some(slot) = pixels.get_mut(index) {
                    *slot = 0;
                }
            }
        }
        let base = pdfcraft_ocr::clearscan::ClearScanOptions::default();
        let options = pdfcraft_ocr::clearscan::ClearScanOptions {
            preprocess: pdfcraft_ocr::PreprocessedOptions { max_skew_degrees: 1.0, ..base.preprocess },
            ..base
        };
        let produced = pdfcraft_ocr::clearscan::clearscan_image(
            pdfcraft_ocr::clearscan::ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 },
            &options,
            lines_for_ink,
            |_| {},
        )
        .expect("clearscan");
        assert!(produced.text.contains('H'), "{:?}", produced.text);
        let mut session = Session::new();
        let blank = session.create_blank(200.0, 200.0, 1).expect("blank");
        let id = session.open("blank.pdf", None, blank, None).expect("open");
        session.apply_built(id, vec![(0, produced.pdf)]).expect("replace");
        let bytes = session.get(id).expect("doc").bytes.clone();
        let mut renderer = PageRenderer::new(bytes, RenderConfig::default());
        let text = renderer.render(RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, ..RenderRequest::default() });
        assert!(text.error.is_none(), "{:?}", text.error);
        let found: String = text.text.iter().flat_map(|page| page.glyphs.iter().map(|glyph| glyph.text.as_str())).collect();
        assert!(found.contains('H'), "extracted {found:?}");
        assert!(session.undo(id).is_ok());
    }

    fn lines_for_ink(page: &pdfcraft_ocr::PreprocessedPage) -> Result<Vec<pdfcraft_ocr::AnnotatedLine>, pdfcraft_ocr::OcrError> {
        let binary = &page.binary;
        let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0_u32, 0_u32);
        let mut any = false;
        for y in 0..binary.height() {
            for x in 0..binary.width() {
                if binary.is_ink(i64::from(x), i64::from(y)) {
                    any = true;
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x.saturating_add(1));
                    bottom = bottom.max(y.saturating_add(1));
                }
            }
        }
        if !any {
            return Ok(Vec::new());
        }
        let rect = [left as f32, top as f32, right as f32, bottom as f32];
        let cap = rect[3] - rect[1];
        Ok(vec![pdfcraft_ocr::AnnotatedLine {
            words: vec![pdfcraft_ocr::AnnotatedWord { text: "H".into(), rect, chars: vec![pdfcraft_ocr::CharBox { char: 'H', rect }] }],
            baseline: rect[3],
            x_height: cap * 0.5,
            cap_height: cap,
            descent: 0.0,
        }])
    }
}
