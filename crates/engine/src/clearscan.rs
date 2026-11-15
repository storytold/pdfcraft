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

use crate::ocr::{self, Models, Ocr, OcrError};
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
    geometries: Vec<pdfcraft_ocr::clearscan::PageGeometry>,
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
    geometries: Vec<pdfcraft_ocr::clearscan::PageGeometry>,
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
        let ClearScanJob { bytes, password, infos, geometries, native, pages, settings } = self;
        let config = RenderConfig { password: password.as_deref().map(Arc::from), hide_comments: true, ..Default::default() };
        let scan_options = scan_options(&settings);
        ClearScanCursor {
            renderer: PageRenderer::new(bytes, config),
            infos,
            geometries,
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
        let ready = match self.prepare_step(&mut progress) {
            PreparedStep::Advance(done) => return Ok(done),
            PreparedStep::Ready(ready) => ready,
        };
        if self.recognizer.is_none() {
            // ClearScan reads with the ocrs models. Tesseract being installed is not enough.
            if Models::find().is_none() {
                return Err(ClearScanError::NoModels);
            }
            match Ocr::find() {
                Ok(ocr) => self.recognizer = Some(Arc::new(ocr)),
                Err(OcrError::NoModels) => return Err(ClearScanError::NoModels),
                Err(error) => return Err(ClearScanError::Failed(error.to_string())),
            }
        }
        let Some(ocr) = self.recognizer.clone() else {
            return Err(ClearScanError::Failed("text recognition did not start".into()));
        };
        Ok(self.finish_step(*ready, progress, |pre| ocr.recognize_page(pre)))
    }

    #[cfg(test)]
    fn step_with_recognizer(
        &mut self,
        mut progress: impl FnMut(ClearScanProgress) -> bool,
        recognize: impl FnMut(&pdfcraft_ocr::PreprocessedPage) -> Result<Vec<pdfcraft_ocr::AnnotatedLine>, OcrError>,
    ) -> Result<Option<ClearScanBuilt>, ClearScanError> {
        match self.prepare_step(&mut progress) {
            PreparedStep::Advance(done) => Ok(done),
            PreparedStep::Ready(ready) => Ok(self.finish_step(*ready, progress, recognize)),
        }
    }

    fn prepare_step(&mut self, progress: &mut impl FnMut(ClearScanProgress) -> bool) -> PreparedStep {
        let Some(page) = self.pages.get(self.index).copied() else {
            return PreparedStep::Advance(Some(self.finish(false)));
        };
        let done = self.index;
        let total = self.pages.len();
        if !progress(ClearScanProgress { page, done, total, stage: "render" }) {
            return PreparedStep::Advance(Some(self.finish(true)));
        }
        let Some(info) = self.infos.get(page).cloned() else {
            self.report.push(failed_page(page, "the page is not in the document"));
            return PreparedStep::Advance(self.advance(false));
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
            return PreparedStep::Advance(self.advance(false));
        }
        let Some(geometry) = self.geometries.get(page).copied() else {
            self.report.push(failed_page(page, "page geometry is missing"));
            return PreparedStep::Advance(self.advance(false));
        };
        let native = self.native.get(page).copied().flatten();
        let req_scale = render_scale(self.settings.dpi, &info, native);
        let eff_scale = pdfcraft_render::effective_scale(info.width, info.height, req_scale);
        let shot = self.renderer.render(RenderRequest { page, scale: req_scale, ..Default::default() });
        if let Some(error) = shot.error {
            self.report.push(failed_page(page, &format!("page {} could not be rendered: {error}", page.saturating_add(1))));
            return PreparedStep::Advance(self.advance(false));
        }
        if shot.width == 0 || shot.height == 0 {
            self.report.push(failed_page(page, "the rendered page is empty"));
            return PreparedStep::Advance(self.advance(false));
        }
        let ppp = 1.0 / f64::from(eff_scale);
        if !eff_scale.is_finite() || eff_scale <= 0.0 || !ppp.is_finite() || ppp <= 0.0 {
            self.report.push(failed_page(page, "the page scale is not a finite number"));
            return PreparedStep::Advance(self.advance(false));
        }
        PreparedStep::Ready(Box::new(ReadyStep { page, done, total, shot, geometry, ppp }))
    }

    fn finish_step(
        &mut self,
        ready: ReadyStep,
        mut progress: impl FnMut(ClearScanProgress) -> bool,
        mut recognize: impl FnMut(&pdfcraft_ocr::PreprocessedPage) -> Result<Vec<pdfcraft_ocr::AnnotatedLine>, OcrError>,
    ) -> Option<ClearScanBuilt> {
        let ReadyStep { page, done, total, shot, geometry, ppp } = ready;
        let mut stop = false;
        let outcome = crate::guard(|| {
            pdfcraft_ocr::clearscan::clearscan_image_with_geometry(
                pdfcraft_ocr::clearscan::ScanRaster {
                    pixels: &shot.rgba,
                    width: shot.width,
                    height: shot.height,
                    channels: 4,
                    points_per_pixel: ppp,
                },
                &self.scan_options,
                geometry,
                &mut recognize,
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
        self.advance(stop)
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

enum PreparedStep {
    Advance(Option<ClearScanBuilt>),
    Ready(Box<ReadyStep>),
}

struct ReadyStep {
    page: usize,
    done: usize,
    total: usize,
    shot: pdfcraft_render::RenderedPage,
    geometry: pdfcraft_ocr::clearscan::PageGeometry,
    ppp: f64,
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
        let cos = doc.editor.as_ref().map(|e| &e.cos).ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        let geometries = pdfcraft_model::pages(cos)
            .iter()
            .map(|p| {
                let (_, display_height) = p.display_size(cos);
                pdfcraft_ocr::clearscan::PageGeometry { crop: p.crop(cos), display_height, view_matrix: p.view_matrix(cos) }
            })
            .collect();
        Ok(ClearScanJob {
            bytes: doc.bytes.clone(),
            password: doc.password.clone(),
            infos: doc.info.pages.clone(),
            geometries,
            native,
            pages,
            settings,
        })
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
            .map(|(page, pdf)| Edit::ReplacePageContent {
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
    let capped = scale.min(cap).max(0.05);
    // `device_pixels` rounds up, so a page right near `MAX_PIXELS` (e.g. A0/A1 at 300 dpi) can
    // still exceed `MAX_PIXELS` by one row/column unless nudged inside the cap.
    let mut eff = pdfcraft_render::effective_scale(info.width, info.height, capped);
    while u64::from(pdfcraft_render::device_pixels(info.width, eff)).saturating_mul(u64::from(pdfcraft_render::device_pixels(info.height, eff)))
        > pdfcraft_ocr::grey::MAX_PIXELS
        && eff > 0.05
    {
        eff = (eff * 0.995).max(0.05);
    }
    eff
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

    #[test]
    fn preserves_geometry_and_aligns_ink_under_link_with_rotate_crop_user_unit_and_skew() {
        use pdfcraft_cos::{Dict, Object, SaveOptions, Stream, write_full};

        fn disp_to_user(view_m: [f64; 6], x: f64, y: f64) -> [f64; 2] {
            [view_m[0] * x + view_m[2] * y + view_m[4], view_m[1] * x + view_m[3] * y + view_m[5]]
        }

        fn skew_disp(x: f64, y: f64, angle_rad: f64, cx: f64, cy: f64) -> [f64; 2] {
            let (sin, cos) = angle_rad.sin_cos();
            let dx = x - cx;
            let dy = y - cy;
            [cx + dx * cos - dy * sin, cy + dx * sin + dy * cos]
        }

        fn rotated_bbox_in_user(view_m: [f64; 6], angle_rad: f64, cx: f64, cy: f64, rect_disp: [f64; 4]) -> [f64; 4] {
            let [x0, y0, x1, y1] = rect_disp;
            let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
            let (mut ux0, mut uy0, mut ux1, mut uy1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
            for [x, y] in corners {
                let [sx, sy] = skew_disp(x, y, angle_rad, cx, cy);
                let [ux, uy] = disp_to_user(view_m, sx, sy);
                ux0 = ux0.min(ux);
                uy0 = uy0.min(uy);
                ux1 = ux1.max(ux);
                uy1 = uy1.max(uy);
            }
            [ux0, uy0, ux1, uy1]
        }

        fn user_rect_to_view_px(info: &PageInfo, rect: [f64; 4]) -> [i32; 4] {
            let p0 = info.user_to_view(rect[0] as f32, rect[1] as f32);
            let p1 = info.user_to_view(rect[2] as f32, rect[3] as f32);
            let x0 = p0[0].min(p1[0]).floor() as i32;
            let y0 = p0[1].min(p1[1]).floor() as i32;
            let x1 = p0[0].max(p1[0]).ceil() as i32;
            let y1 = p0[1].max(p1[1]).ceil() as i32;
            [x0, y0, x1, y1]
        }

        fn count_dark_and_centroid(shot: &pdfcraft_render::RenderedPage, px_rect: [i32; 4]) -> (usize, f64, f64) {
            let w = shot.width as i32;
            let h = shot.height as i32;
            let mut count = 0_usize;
            let (mut sx, mut sy) = (0.0_f64, 0.0_f64);
            for y in px_rect[1].max(0)..px_rect[3].min(h) {
                for x in px_rect[0].max(0)..px_rect[2].min(w) {
                    let idx = ((y as usize) * (shot.width as usize) + (x as usize)) * 4;
                    if shot.rgba[idx] < 128 {
                        count += 1;
                        sx += f64::from(x) + 0.5;
                        sy += f64::from(y) + 0.5;
                    }
                }
            }
            if count == 0 { (0, 0.0, 0.0) } else { (count, sx / count as f64, sy / count as f64) }
        }

        let mut session = Session::new();
        let blank = session.create_blank(260.0, 320.0, 1).expect("blank");
        let mut cos = pdfcraft_cos::Document::open(blank).expect("open");
        let page_ref = pdfcraft_model::pages(&cos)[0].obj;
        let media_box = Object::Array(vec![0.into(), 0.into(), 260.into(), 320.into()]);
        let crop_box = Object::Array(vec![20.into(), 40.into(), 200.into(), 260.into()]);
        cos.update_dict(page_ref, |d| {
            d.set(b"MediaBox".to_vec(), media_box.clone());
            d.set(b"CropBox".to_vec(), crop_box.clone());
            d.set(b"Rotate".to_vec(), Object::Int(90));
            d.set(b"UserUnit".to_vec(), Object::Real(2.0));
        })
        .expect("update page");

        let model_page = pdfcraft_model::pages(&cos).swap_remove(0);
        let (disp_w, disp_h) = model_page.display_size(&cos);
        assert_eq!((disp_w, disp_h), (440.0, 360.0));
        let view_m = model_page.view_matrix(&cos);

        let angle = 2.0_f64.to_radians();
        let (cx, cy) = (disp_w * 0.5, disp_h * 0.5);
        let (sin, cos_a) = angle.sin_cos();
        let e = cx - cx * cos_a + cy * sin;
        let f = cy - cx * sin - cy * cos_a;
        // Draw white fill on full MediaBox, then tilted word bars + one target 'H' inside display space.
        let mut content = String::from("1 1 1 rg 0 0 260 320 re f\nq\n");
        let [va, vb, vc, vd, ve, vf] = view_m;
        content.push_str(&format!("{va:.6} {vb:.6} {vc:.6} {vd:.6} {ve:.6} {vf:.6} cm\n"));
        content.push_str(&format!("{cos_a:.6} {sin:.6} {:.6} {cos_a:.6} {e:.6} {f:.6} cm\n0 0 0 rg\n", -sin));
        let target_rc = (1_usize, 5_usize);
        let bg_rc = (4_usize, 1_usize);
        let mut target_disp_rect = [0.0; 4];
        let mut bg_disp_rect = [0.0; 4];
        for r in 0..6_usize {
            let y0 = 50.0 + r as f64 * 48.0;
            for c in 0..7_usize {
                let x0 = 44.0 + c as f64 * 52.0;
                if (r, c) == target_rc {
                    let (w, h) = (22.0, 18.0);
                    target_disp_rect = [x0, y0, x0 + w, y0 + h];
                    // Draw a single connected dark 'H' so ClearScan clusters & replaces it with Type 3.
                    content.push_str(&format!(
                        "{x0:.2} {y0:.2} 5.00 {h:.2} re f\n{:.2} {y0:.2} 5.00 {h:.2} re f\n{x0:.2} {:.2} {w:.2} 4.00 re f\n",
                        x0 + w - 5.0,
                        y0 + 7.0
                    ));
                } else {
                    if (r, c) == bg_rc {
                        bg_disp_rect = [x0, y0, x0 + 28.0, y0 + 10.0];
                    }
                    content.push_str(&format!("{x0:.2} {y0:.2} 28.00 10.00 re f\n"));
                }
            }
        }
        content.push_str("Q\n");

        let link_user_rect = rotated_bbox_in_user(view_m, angle, cx, cy, target_disp_rect);
        let bg_user_rect = rotated_bbox_in_user(view_m, angle, cx, cy, bg_disp_rect);

        let stream_ref = cos.add(Object::Stream(Stream::flate(Dict::new(), content.as_bytes())));
        let link_annot = cos.add(Object::Dict({
            let mut d = Dict::new();
            d.set(b"Type".to_vec(), Object::name("Annot"));
            d.set(b"Subtype".to_vec(), Object::name("Link"));
            d.set(b"Rect".to_vec(), Object::Array(link_user_rect.iter().map(|v| Object::Real(*v)).collect()));
            d
        }));
        cos.update_dict(page_ref, |d| {
            d.set(b"Contents".to_vec(), Object::Ref(stream_ref));
            d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(link_annot)]));
        })
        .expect("attach content and link");

        let pdf_bytes = Arc::new(write_full(&cos, &SaveOptions::default()).expect("write"));
        let id = session.open("skewed.pdf", None, Arc::clone(&pdf_bytes), None).expect("open");
        let info_before = session.get(id).expect("doc").info.pages[0].clone();
        let link_view_px = user_rect_to_view_px(&info_before, link_user_rect);
        let bg_view_px = user_rect_to_view_px(&info_before, bg_user_rect);

        let mut before_renderer = PageRenderer::new(pdf_bytes, RenderConfig { hide_comments: true, ..Default::default() });
        let before_shot = before_renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(before_shot.error.is_none(), "{:?}", before_shot.error);
        let (before_link_ink, before_link_cx, before_link_cy) = count_dark_and_centroid(&before_shot, link_view_px);
        let (before_bg_ink, before_bg_cx, before_bg_cy) = count_dark_and_centroid(&before_shot, bg_view_px);
        assert!(before_link_ink > 30, "expected link ink before ClearScan, got {before_link_ink}");
        assert!(before_bg_ink > 30, "expected bg ink before ClearScan, got {before_bg_ink}");

        // Target 'H' center in deskewed binary coordinates (fractional display coords map cleanly).
        let target_cx_u = (target_disp_rect[0] + target_disp_rect[2]) * 0.5 / disp_w;
        let target_cy_v = 1.0 - (target_disp_rect[1] + target_disp_rect[3]) * 0.5 / disp_h;

        let job = session
            .clearscan_job(id, &[], ClearScanSettings { dpi: 150.0, language: "en".into(), sensitivity: 50, downsample: 1, skip_text_pages: true })
            .expect("job");
        let mut cursor = job.cursor();
        let built = loop {
            if let Some(done) = cursor
                .step_with_recognizer(
                    |_| true,
                    |pre| {
                        assert!(pre.angle_radians.abs() > 0.015, "expected ~2deg skew estimate, got {}", pre.angle_radians);
                        let w = pre.binary.width();
                        let h = pre.binary.height();
                        let approx_x = (target_cx_u * f64::from(w)).round() as i64;
                        let approx_y = (target_cy_v * f64::from(h)).round() as i64;
                        let radius = 40_i64;
                        let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0_u32, 0_u32);
                        let mut any = false;
                        for y in (approx_y - radius).max(0)..(approx_y + radius).min(i64::from(h)) {
                            for x in (approx_x - radius).max(0)..(approx_x + radius).min(i64::from(w)) {
                                if pre.binary.is_ink(x, y) {
                                    any = true;
                                    let ux = x as u32;
                                    let uy = y as u32;
                                    left = left.min(ux);
                                    top = top.min(uy);
                                    right = right.max(ux.saturating_add(1));
                                    bottom = bottom.max(uy.saturating_add(1));
                                }
                            }
                        }
                        assert!(any, "expected ink near target 'H' window ({approx_x}, {approx_y})");
                        let rect = [left as f32, top as f32, right as f32, bottom as f32];
                        let cap = rect[3] - rect[1];
                        Ok(vec![pdfcraft_ocr::AnnotatedLine {
                            words: vec![pdfcraft_ocr::AnnotatedWord {
                                text: "H".into(),
                                rect,
                                chars: vec![pdfcraft_ocr::CharBox { char: 'H', rect }],
                            }],
                            baseline: rect[3],
                            x_height: cap * 0.5,
                            cap_height: cap,
                            descent: 0.0,
                        }])
                    },
                )
                .expect("step")
            {
                break done;
            }
        };
        assert_eq!(built.report.pages.len(), 1);
        assert!(built.report.pages[0].error.is_none(), "{:?}", built.report.pages[0].error);
        assert!(built.report.pages[0].glyphs >= 1);

        session.apply_built(id, built.built).expect("apply ClearScan");

        // 1. Original page geometry and annotations stay untouched.
        let after_doc = session.get(id).expect("doc");
        let after_bytes = after_doc.bytes.clone();
        let after_cos = pdfcraft_cos::Document::open(Arc::clone(&after_bytes)).expect("parse after");
        let after_page = pdfcraft_model::pages(&after_cos).swap_remove(0);
        let d = after_page.dict;
        assert_eq!(d.get(b"MediaBox"), Some(&media_box));
        assert_eq!(d.get(b"CropBox"), Some(&crop_box));
        assert_eq!(d.get(b"Rotate"), Some(&Object::Int(90)));
        assert_eq!(d.get(b"UserUnit").and_then(|o| o.as_f64()), Some(2.0));
        assert!(d.contains(b"Annots"), "link annotation should remain");
        let info_after = &after_doc.info.pages[0];
        assert!((info_after.width - 440.0).abs() < 1e-3);
        assert!((info_after.height - 360.0).abs() < 1e-3);

        // 2. Rendered ink under both the link's /Rect (Type 3 'H') and the opposite-quadrant
        //    background block stays aligned within 2 pt.
        let mut after_renderer = PageRenderer::new(after_bytes, RenderConfig { hide_comments: true, ..Default::default() });
        let after_shot = after_renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(after_shot.error.is_none(), "{:?}", after_shot.error);
        let (after_link_ink, after_link_cx, after_link_cy) = count_dark_and_centroid(&after_shot, link_view_px);
        let (after_bg_ink, after_bg_cx, after_bg_cy) = count_dark_and_centroid(&after_shot, bg_view_px);
        assert!(
            after_link_ink * 2 >= before_link_ink,
            "ink under link /Rect dropped or shifted away: before={before_link_ink}, after={after_link_ink}"
        );
        assert!(
            (after_link_cx - before_link_cx).abs() <= 2.0 && (after_link_cy - before_link_cy).abs() <= 2.0,
            "link ink centroid shifted: before=({before_link_cx:.2}, {before_link_cy:.2}), after=({after_link_cx:.2}, {after_link_cy:.2})"
        );
        assert!(after_bg_ink * 2 >= before_bg_ink, "background ink dropped or shifted away: before={before_bg_ink}, after={after_bg_ink}");
        assert!(
            (after_bg_cx - before_bg_cx).abs() <= 2.0 && (after_bg_cy - before_bg_cy).abs() <= 2.0,
            "background ink centroid shifted: before=({before_bg_cx:.2}, {before_bg_cy:.2}), after=({after_bg_cx:.2}, {after_bg_cy:.2})"
        );

        // 3. Searchable glyph 'H' itself lies inside the link's /Rect.
        let text = after_renderer.render(RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
        assert!(text.error.is_none(), "{:?}", text.error);
        let glyphs: Vec<_> = text.text.iter().flat_map(|p| p.glyphs.iter()).collect();
        let h_glyph = glyphs.iter().find(|g| g.text == "H").expect("expected searchable H glyph");
        let gcx = f64::from(h_glyph.rect[0] + h_glyph.rect[2]) * 0.5;
        let gcy = f64::from(h_glyph.rect[1] + h_glyph.rect[3]) * 0.5;
        assert!(
            gcx >= f64::from(link_view_px[0])
                && gcx <= f64::from(link_view_px[2])
                && gcy >= f64::from(link_view_px[1])
                && gcy <= f64::from(link_view_px[3]),
            "glyph 'H' center ({gcx:.2}, {gcy:.2}) landed outside link view rect {link_view_px:?}"
        );
    }

    #[test]
    fn render_scale_keeps_large_a0_and_a1_pages_within_max_pixels() {
        let mut session = Session::new();
        for (i, (w, h)) in [(1684.0_f32, 2384.0_f32), (2384.0_f32, 3370.0_f32)].into_iter().enumerate() {
            let blank = session.create_blank(f64::from(w), f64::from(h), 1).expect("blank");
            let id = session.open(format!("large-{i}.pdf"), None, blank, None).expect("open");
            let info = &session.get(id).expect("doc").info.pages[0];
            let scale = render_scale(300.0, info, None);
            let px = u64::from(pdfcraft_render::device_pixels(w, scale)) * u64::from(pdfcraft_render::device_pixels(h, scale));
            assert!(px <= pdfcraft_ocr::grey::MAX_PIXELS, "{w}x{h} rendered {px} px > MAX_PIXELS");
        }
    }
}
