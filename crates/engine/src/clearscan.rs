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

impl ClearScanReport {
    /// Record an apply-time failure for 0-based `page` in this report, replacing its earlier
    /// built entry when present or appending a failed entry otherwise.
    pub fn record_failure(&mut self, page: usize, error: impl Into<String>) {
        let msg = error.into();
        if let Some(slot) = self.pages.iter_mut().find(|p| p.page == page) {
            *slot = failed_page(page, &msg);
        } else {
            self.pages.push(failed_page(page, &msg));
        }
    }
}

/// What [`Session::clearscan_job`] captured, detached from the session so a worker can run it.
pub struct ClearScanJob {
    bytes: Arc<Vec<u8>>,
    password: Option<String>,
    infos: Vec<PageInfo>,
    geometries: Vec<pdfcraft_ocr::clearscan::PageGeometry>,
    native: Vec<Option<f32>>,
    skip_reasons: Vec<Option<String>>,
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
    skip_reasons: Vec<Option<String>>,
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
        let ClearScanJob { bytes, password, infos, geometries, native, skip_reasons, pages, settings } = self;
        let config = RenderConfig { password: password.as_deref().map(Arc::from), hide_comments: true, ..Default::default() };
        let scan_options = scan_options(&settings);
        ClearScanCursor {
            renderer: PageRenderer::new(bytes, config),
            infos,
            geometries,
            native,
            skip_reasons,
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

    fn skip_page(
        &mut self,
        page: usize,
        done: usize,
        total: usize,
        reason: String,
        progress: &mut impl FnMut(ClearScanProgress) -> bool,
    ) -> PreparedStep {
        let _ = progress(ClearScanProgress { page, done, total, stage: "skipped" });
        self.report.push(ClearScanPage {
            page,
            text: String::new(),
            glyphs: 0,
            clusters: 0,
            erased: 0,
            bytes: 0,
            skipped: Some(reason),
            error: None,
        });
        let _ = progress(ClearScanProgress { page, done: done.saturating_add(1), total, stage: "done" });
        PreparedStep::Advance(self.advance(false))
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
            return self.skip_page(page, done, total, "the page already contains text".into(), progress);
        }
        if let Some(reason) = self.skip_reasons.get(page).and_then(|r| r.as_ref()).cloned() {
            return self.skip_page(page, done, total, reason, progress);
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
        if let Some(p) = cos.permissions() {
            crate::edit::check_permission(
                &Edit::ReplacePageContent { pages: Vec::new(), name: String::new(), bytes: Arc::new(Vec::new()), src_pages: Vec::new() },
                &p,
            )?;
        }
        let tagged_pages: std::collections::HashSet<usize> = if pages.is_empty() {
            std::collections::HashSet::new()
        } else {
            pdfcraft_a11y::tags(cos).into_iter().filter_map(|tag| tag.page).collect()
        };
        let mut skip_reasons = vec![None; count];
        for &page in &pages {
            if let Some(slot) = skip_reasons.get_mut(page) {
                *slot = non_scan_reason(cos, page, doc.info.pages.get(page), tagged_pages.contains(&page));
            }
        }
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
            skip_reasons,
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
        let mut report = built.report;
        let partitioned = self.partition_built(id, built.built)?;
        for (page, err) in partitioned.rejected {
            report.record_failure(page, err.to_string());
        }
        if !partitioned.edits.is_empty() {
            self.apply(id, Edit::Batch { label: "ClearScan".into(), edits: partitioned.edits })?;
        }
        Ok(report)
    }

    /// Apply pages a [`ClearScanJob`] already built, as one undoable "ClearScan" step. Returns
    /// any `(page, error)` entries that failed validation so callers (such as the UI) can fold
    /// them into their [`ClearScanReport`] via [`ClearScanReport::record_failure`].
    pub fn apply_clearscan(&mut self, id: DocId, built: Vec<(usize, Vec<u8>)>) -> Result<Vec<(usize, EditError)>, ClearScanError> {
        self.apply_built(id, built)
    }

    fn apply_built(&mut self, id: DocId, built: Vec<(usize, Vec<u8>)>) -> Result<Vec<(usize, EditError)>, ClearScanError> {
        if built.is_empty() {
            return Ok(Vec::new());
        }
        let mut partitioned = self.partition_built(id, built)?;
        if partitioned.edits.is_empty()
            && let Some((_, err)) = partitioned.rejected.pop()
        {
            return Err(ClearScanError::Edit(err));
        }
        if !partitioned.edits.is_empty() {
            self.apply(id, Edit::Batch { label: "ClearScan".into(), edits: partitioned.edits })?;
        }
        Ok(partitioned.rejected)
    }

    fn partition_built(&self, id: DocId, built: Vec<(usize, Vec<u8>)>) -> Result<PartitionedBuilt, ClearScanError> {
        if built.is_empty() {
            return Ok(PartitionedBuilt::default());
        }
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if let Some(why) = &doc.read_only_reason {
            return Err(ClearScanError::Edit(EditError::ReadOnly(why.clone())));
        }
        let cos = doc.editor.as_ref().map(|e| &e.cos).ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        if let Some(p) = cos.permissions() {
            crate::edit::check_permission(
                &Edit::ReplacePageContent { pages: Vec::new(), name: String::new(), bytes: Arc::new(Vec::new()), src_pages: Vec::new() },
                &p,
            )?;
        }
        let mut edits = Vec::with_capacity(built.len());
        let mut rejected = Vec::new();
        for (page, pdf) in built {
            let name = format!("clearscan-{}.pdf", page.saturating_add(1));
            let bytes = Arc::new(pdf);
            let mut probe = cos.clone();
            let check = crate::sources::open_source_into(&name, &bytes, &probe)
                .and_then(|src| pdfcraft_organize::replace_page_content(&mut probe, &[page], &src, &[0]).map_err(EditError::from));
            match check {
                Ok(()) => edits.push(Edit::ReplacePageContent { pages: vec![page], name, bytes, src_pages: vec![0] }),
                Err(err) => rejected.push((page, err)),
            }
        }
        Ok(PartitionedBuilt { edits, rejected })
    }
}

#[derive(Default)]
struct PartitionedBuilt {
    edits: Vec<Edit>,
    rejected: Vec<(usize, EditError)>,
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

fn non_scan_reason(cos: &pdfcraft_cos::Document, page: usize, info: Option<&PageInfo>, in_struct_tree: bool) -> Option<String> {
    let Ok(content) = pdfcraft_edit::page_scan_content(cos, page) else {
        return Some("the page contents could not be inspected".into());
    };
    if content.has_tagged_content || in_struct_tree {
        return Some("the page is tagged and replacing its contents would break its structure tree".into());
    }
    if content.has_optional_content {
        return Some("the page uses optional content layers".into());
    }
    let Ok(images) = pdfcraft_edit::page_images(cos, page) else {
        return Some("the page contents could not be inspected".into());
    };
    if images.iter().any(|i| i.is_form) {
        return Some("the page contains form artwork rather than a single scanned image".into());
    }
    let rasters: Vec<_> = images.iter().filter(|i| !i.is_form && i.width > 1 && i.height > 1).collect();
    let Some(image) = rasters.first() else {
        return Some("the page does not contain a scanned image".into());
    };
    if rasters.len() > 1 {
        return Some("the page contains multiple images rather than a single scan".into());
    }
    if content.has_vector_or_inline {
        return Some("the page contains vector graphics alongside the image".into());
    }
    let Some(info) = info else {
        return Some("page geometry is missing".into());
    };
    if crate::scan_fields::image_crop_share(info, image).is_none_or(|s| s < crate::scan_fields::COVERED) {
        return Some("the image covers less than half the page".into());
    }
    None
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

        // Rasterize the tilted fixture onto a flat 260x320 user-space page so the tested page is
        // a real single-image scan wrapped in /CropBox, /Rotate 90 and /UserUnit 2.0.
        let mut flat = pdfcraft_cos::Document::open(session.create_blank(260.0, 320.0, 1).expect("flat blank")).expect("open flat");
        let flat_page = pdfcraft_model::pages(&flat)[0].obj;
        let flat_stream = flat.add(Object::Stream(Stream::flate(Dict::new(), content.as_bytes())));
        flat.update_dict(flat_page, |d| d.set(b"Contents".to_vec(), Object::Ref(flat_stream))).expect("set flat stream");
        let flat_bytes = Arc::new(write_full(&flat, &SaveOptions::default()).expect("write flat"));
        let mut flat_renderer = PageRenderer::new(flat_bytes, RenderConfig::default());
        let raster = flat_renderer.render(RenderRequest { page: 0, scale: 2.0, ..Default::default() });
        assert!(raster.error.is_none(), "{:?}", raster.error);
        let mut rgb = Vec::with_capacity((raster.width as usize).saturating_mul(raster.height as usize).saturating_mul(3));
        for px in raster.rgba.as_chunks::<4>().0 {
            rgb.extend_from_slice(&px[..3]);
        }
        let mut img_dict = Dict::new();
        img_dict.set(b"Type".to_vec(), Object::name("XObject"));
        img_dict.set(b"Subtype".to_vec(), Object::name("Image"));
        img_dict.set(b"Width".to_vec(), Object::Int(i64::from(raster.width)));
        img_dict.set(b"Height".to_vec(), Object::Int(i64::from(raster.height)));
        img_dict.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
        img_dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        let img_ref = cos.add(Object::Stream(Stream::flate(img_dict, &rgb)));
        let mut xo_dict = Dict::new();
        xo_dict.set(b"Im0".to_vec(), Object::Ref(img_ref));
        let mut res_dict = Dict::new();
        res_dict.set(b"XObject".to_vec(), Object::Dict(xo_dict));
        let stream_ref = cos.add(Object::Stream(Stream::flate(Dict::new(), b"q 260 0 0 320 0 0 cm /Im0 Do Q\n")));
        let link_annot = cos.add(Object::Dict({
            let mut d = Dict::new();
            d.set(b"Type".to_vec(), Object::name("Annot"));
            d.set(b"Subtype".to_vec(), Object::name("Link"));
            d.set(b"Rect".to_vec(), Object::Array(link_user_rect.iter().map(|v| Object::Real(*v)).collect()));
            d
        }));
        cos.update_dict(page_ref, |d| {
            d.set(b"Resources".to_vec(), Object::Dict(res_dict));
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

    fn assert_clearscan_skipped(bytes: Arc<Vec<u8>>, expected_reason_substr: &str) {
        let mut session = Session::new();
        let id = session.open("non-scan.pdf", None, bytes, None).expect("open");
        let mut stages = Vec::new();
        let report = session
            .clearscan_document(id, &[], ClearScanSettings::default(), |event| {
                stages.push(event.stage);
                true
            })
            .expect("skipping non-scan pages must not fail or require OCR models");
        assert!(!report.cancelled);
        assert_eq!(report.pages.len(), 1, "{report:?}");
        let page = &report.pages[0];
        let skipped = page.skipped.as_deref().expect("page should be skipped");
        assert!(skipped.contains(expected_reason_substr), "expected {expected_reason_substr:?} in {skipped:?}");
        assert!(page.error.is_none(), "{page:?}");
        assert!(stages.contains(&"skipped"), "progress must report skipped stage: {stages:?}");
        assert!(stages.contains(&"done"), "progress must report done stage: {stages:?}");
        assert!(session.undo(id).is_err(), "skipped non-scan page must not create an edit");
    }

    #[test]
    fn clearscan_skips_vector_art_and_hybrid_vector_pages() {
        use pdfcraft_cos::{Document, Object, SaveOptions, Stream, write_full};

        let session = Session::new();
        let blank = session.create_blank(200.0, 200.0, 1).expect("blank");
        let mut cos = Document::open(blank).expect("open blank");
        let page_ref = pdfcraft_model::pages(&cos)[0].obj;
        let vec_stream = cos.add(Object::Stream(Stream::flate(pdfcraft_cos::Dict::new(), b"0 0 1 rg 20 20 120 120 re f\n")));
        cos.update_dict(page_ref, |d| d.set(b"Contents".to_vec(), Object::Ref(vec_stream))).expect("set vector contents");
        let vec_pdf = Arc::new(write_full(&cos, &SaveOptions::default()).expect("write vector pdf"));
        assert_clearscan_skipped(vec_pdf, "scanned image");

        // A single full-page image paired with overlaid vector paths must also be skipped.
        let png = crate::export::encode_png(32, 32, &vec![250_u8; 32 * 32 * 4]).expect("png");
        let scan_bytes = session.create_from_images(&[("scan.png".into(), png)]).expect("scan pdf");
        let mut cos2 = Document::open(scan_bytes).expect("open scan");
        let page2 = pdfcraft_model::pages(&cos2).swap_remove(0);
        let overlay = cos2.add(Object::Stream(Stream::flate(pdfcraft_cos::Dict::new(), b"1 0 0 RG 2 w 4 4 20 20 re S\n")));
        let old_contents = page2.dict.get(b"Contents").cloned().expect("contents");
        cos2.update_dict(page2.obj, |d| d.set(b"Contents".to_vec(), Object::Array(vec![old_contents, Object::Ref(overlay)])))
            .expect("append vector overlay");
        let hybrid_pdf = Arc::new(write_full(&cos2, &SaveOptions::default()).expect("write hybrid pdf"));
        assert_clearscan_skipped(hybrid_pdf, "vector graphics");
    }

    #[test]
    fn clearscan_skips_multi_image_small_image_and_ocg_pages() {
        use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};

        let png = Arc::new(crate::export::encode_png(32, 32, &vec![250_u8; 32 * 32 * 4]).expect("png"));

        // 1. Multiple images on one page.
        let mut session = Session::new();
        let blank = session.create_blank(200.0, 200.0, 1).expect("blank");
        let id = session.open("multi.pdf", None, blank, None).expect("open");
        session
            .apply(id, Edit::AddImage { page: 0, rect: Some([10.0, 10.0, 190.0, 95.0]), name: "top.png".into(), bytes: Arc::clone(&png) })
            .expect("top image");
        session
            .apply(id, Edit::AddImage { page: 0, rect: Some([10.0, 105.0, 190.0, 190.0]), name: "bot.png".into(), bytes: Arc::clone(&png) })
            .expect("bottom image");
        let multi_pdf = session.save_bytes(id).expect("save multi");
        assert_clearscan_skipped(multi_pdf, "multiple images");

        // 2. One image covering less than half of the page.
        let mut s_small = Session::new();
        let blank2 = s_small.create_blank(200.0, 200.0, 1).expect("blank2");
        let id2 = s_small.open("small.pdf", None, blank2, None).expect("open small");
        s_small
            .apply(id2, Edit::AddImage { page: 0, rect: Some([10.0, 10.0, 60.0, 60.0]), name: "logo.png".into(), bytes: Arc::clone(&png) })
            .expect("small image");
        let small_pdf = s_small.save_bytes(id2).expect("save small");
        assert_clearscan_skipped(small_pdf, "less than half");

        // 3. Single full-page image inside an optional content (OCG) layer.
        let scan_bytes = s_small.create_from_images(&[("scan.png".into(), (*png).clone())]).expect("scan");
        let mut cos = Document::open(scan_bytes).expect("open ocg base");
        let mut ocg_dict = Dict::new();
        ocg_dict.set(b"Type".to_vec(), Object::name("OCG"));
        ocg_dict.set(b"Name".to_vec(), Object::String(pdfcraft_cos::PdfString::text("Hidden layer")));
        let ocg_ref = cos.add(Object::Dict(ocg_dict));
        let root_ref = cos.trailer().reference(b"Root").expect("root");
        cos.update_dict(root_ref, |c| {
            let mut d_cfg = Dict::new();
            d_cfg.set(b"OFF".to_vec(), Object::Array(vec![Object::Ref(ocg_ref)]));
            let mut props = Dict::new();
            props.set(b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ocg_ref)]));
            props.set(b"D".to_vec(), Object::Dict(d_cfg));
            c.set(b"OCProperties".to_vec(), Object::Dict(props));
        })
        .expect("set OCProperties");
        let page = pdfcraft_model::pages(&cos).swap_remove(0);
        let mut res = page.dict.get(b"Resources").and_then(|o| cos.resolve(o).as_dict().cloned()).unwrap_or_default();
        let mut prop_map = Dict::new();
        prop_map.set(b"L1".to_vec(), Object::Ref(ocg_ref));
        res.set(b"Properties".to_vec(), Object::Dict(prop_map));
        let bdc = cos.add(Object::Stream(Stream::flate(Dict::new(), b"/OC /L1 BDC\n")));
        let emc = cos.add(Object::Stream(Stream::flate(Dict::new(), b"\nEMC\n")));
        let orig_contents = page.dict.get(b"Contents").cloned().expect("contents");
        cos.update_dict(page.obj, |p| {
            p.set(b"Resources".to_vec(), Object::Dict(res));
            p.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(bdc), orig_contents, Object::Ref(emc)]));
        })
        .expect("wrap in OC");
        let ocg_pdf = Arc::new(write_full(&cos, &SaveOptions::default()).expect("write ocg pdf"));
        assert_clearscan_skipped(ocg_pdf, "optional content layers");
    }

    #[test]
    fn clearscan_skips_tagged_pages_and_preserves_structure_tree() {
        use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};

        let png = crate::export::encode_png(32, 32, &vec![250_u8; 32 * 32 * 4]).expect("png");
        let session = Session::new();
        let scan_bytes = session.create_from_images(&[("scan.png".into(), png)]).expect("scan");

        // 1. Tagged scanned page with /StructTreeRoot, /ParentTree, /StructParents and /MCID 0.
        let mut cos = Document::open(Arc::clone(&scan_bytes)).expect("open scan");
        let page = pdfcraft_model::pages(&cos).swap_remove(0);
        let root_ref = cos.trailer().reference(b"Root").expect("root");
        let tree_ref = cos.add(Object::Null);
        let doc_elem_ref = cos.add(Object::Null);
        let fig_elem_ref = cos.add(Object::Null);
        let parent_tree_ref = cos.add(Object::Null);

        let mut fig_dict = Dict::new();
        fig_dict.set(b"Type".to_vec(), Object::name("StructElem"));
        fig_dict.set(b"S".to_vec(), Object::name("Figure"));
        fig_dict.set(b"P".to_vec(), Object::Ref(doc_elem_ref));
        fig_dict.set(b"Pg".to_vec(), Object::Ref(page.obj));
        fig_dict.set(b"Alt".to_vec(), Object::String(pdfcraft_cos::PdfString::text("Scanned figure")));
        fig_dict.set(b"K".to_vec(), Object::Int(0));
        cos.set(fig_elem_ref, Object::Dict(fig_dict));

        let mut doc_dict = Dict::new();
        doc_dict.set(b"Type".to_vec(), Object::name("StructElem"));
        doc_dict.set(b"S".to_vec(), Object::name("Document"));
        doc_dict.set(b"P".to_vec(), Object::Ref(tree_ref));
        doc_dict.set(b"K".to_vec(), Object::Array(vec![Object::Ref(fig_elem_ref)]));
        cos.set(doc_elem_ref, Object::Dict(doc_dict));

        let mut parent_tree = Dict::new();
        parent_tree.set(b"Nums".to_vec(), Object::Array(vec![Object::Int(0), Object::Array(vec![Object::Ref(fig_elem_ref)])]));
        cos.set(parent_tree_ref, Object::Dict(parent_tree));

        let mut tree_dict = Dict::new();
        tree_dict.set(b"Type".to_vec(), Object::name("StructTreeRoot"));
        tree_dict.set(b"K".to_vec(), Object::Ref(doc_elem_ref));
        tree_dict.set(b"ParentTree".to_vec(), Object::Ref(parent_tree_ref));
        cos.set(tree_ref, Object::Dict(tree_dict));

        cos.update_dict(root_ref, |c| {
            let mut mark_info = Dict::new();
            mark_info.set(b"Marked".to_vec(), Object::Bool(true));
            c.set(b"MarkInfo".to_vec(), Object::Dict(mark_info));
            c.set(b"StructTreeRoot".to_vec(), Object::Ref(tree_ref));
        })
        .expect("set StructTreeRoot");

        let bdc = cos.add(Object::Stream(Stream::flate(Dict::new(), b"/Figure << /MCID 0 >> BDC\n")));
        let emc = cos.add(Object::Stream(Stream::flate(Dict::new(), b"\nEMC\n")));
        let orig_contents = page.dict.get(b"Contents").cloned().expect("contents");
        cos.update_dict(page.obj, |p| {
            p.set(b"StructParents".to_vec(), Object::Int(0));
            p.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(bdc), orig_contents, Object::Ref(emc)]));
        })
        .expect("wrap in MCID");

        let tagged_pdf = Arc::new(write_full(&cos, &SaveOptions::default()).expect("write tagged pdf"));
        assert_clearscan_skipped(Arc::clone(&tagged_pdf), "tagged");

        let check_cos = Document::open(tagged_pdf).expect("reopen");
        let after_page = pdfcraft_model::pages(&check_cos).swap_remove(0);
        assert_eq!(after_page.dict.get(b"StructParents").and_then(Object::as_int), Some(0));
        assert!(pdfcraft_edit::page_scan_content(&check_cos, 0).expect("scan content").has_tagged_content);
        assert!(pdfcraft_a11y::tags(&check_cos).iter().any(|t| t.kind == "Figure" && t.page == Some(0)));

        // 2. A scan page referenced by /Pg in /StructTreeRoot even without /StructParents or /MCID.
        let mut cos2 = Document::open(scan_bytes).expect("open scan 2");
        let page2 = pdfcraft_model::pages(&cos2).swap_remove(0);
        let root2 = cos2.trailer().reference(b"Root").expect("root2");
        let tree2 = cos2.add(Object::Null);
        let elem2 = cos2.add(Object::Null);
        let mut elem2_dict = Dict::new();
        elem2_dict.set(b"Type".to_vec(), Object::name("StructElem"));
        elem2_dict.set(b"S".to_vec(), Object::name("Figure"));
        elem2_dict.set(b"P".to_vec(), Object::Ref(tree2));
        elem2_dict.set(b"Pg".to_vec(), Object::Ref(page2.obj));
        cos2.set(elem2, Object::Dict(elem2_dict));
        let mut tree2_dict = Dict::new();
        tree2_dict.set(b"Type".to_vec(), Object::name("StructTreeRoot"));
        tree2_dict.set(b"K".to_vec(), Object::Ref(elem2));
        cos2.set(tree2, Object::Dict(tree2_dict));
        cos2.update_dict(root2, |c| c.set(b"StructTreeRoot".to_vec(), Object::Ref(tree2))).expect("set root2 tree");
        let pg_only_pdf = Arc::new(write_full(&cos2, &SaveOptions::default()).expect("write pg-only pdf"));
        assert_clearscan_skipped(pg_only_pdf, "tagged");
    }

    #[test]
    fn apply_clearscan_isolates_failed_page_and_job_checks_modify_permission() {
        let mut session = Session::new();
        let blank2 = session.create_blank(200.0, 200.0, 2).expect("blank2");
        let id = session.open("two.pdf", None, blank2, None).expect("open");

        // Build a valid single-page replacement PDF with visible text.
        let mut repl_cos = pdfcraft_cos::Document::open(session.create_blank(200.0, 200.0, 1).expect("repl")).expect("open repl");
        let repl_page = pdfcraft_model::pages(&repl_cos)[0].obj;
        let font_ref = repl_cos.add(pdfcraft_cos::Object::Dict({
            let mut d = pdfcraft_cos::Dict::new();
            d.set(b"Type".to_vec(), pdfcraft_cos::Object::name("Font"));
            d.set(b"Subtype".to_vec(), pdfcraft_cos::Object::name("Type1"));
            d.set(b"BaseFont".to_vec(), pdfcraft_cos::Object::name("Helvetica"));
            d
        }));
        let mut fonts = pdfcraft_cos::Dict::new();
        fonts.set(b"F1".to_vec(), pdfcraft_cos::Object::Ref(font_ref));
        let mut res = pdfcraft_cos::Dict::new();
        res.set(b"Font".to_vec(), pdfcraft_cos::Object::Dict(fonts));
        let stream_ref = repl_cos
            .add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::flate(pdfcraft_cos::Dict::new(), b"BT /F1 12 Tf 20 100 Td (Survived) Tj ET\n")));
        repl_cos
            .update_dict(repl_page, |d| {
                d.set(b"Resources".to_vec(), pdfcraft_cos::Object::Dict(res));
                d.set(b"Contents".to_vec(), pdfcraft_cos::Object::Ref(stream_ref));
            })
            .expect("set repl contents");
        let valid_page_pdf = pdfcraft_cos::write_full(&repl_cos, &pdfcraft_cos::SaveOptions::default()).expect("write repl").to_vec();

        // Mixing a valid built page with a corrupt built page still applies the valid page in one undo step
        // and returns the rejected page so callers can update their report.
        let rejected = session
            .apply_clearscan(id, vec![(0, valid_page_pdf.clone()), (1, b"not a pdf".to_vec())])
            .expect("valid page should survive corrupt sibling");
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].0, 1);
        let mut report = ClearScanReport {
            pages: vec![
                ClearScanPage { page: 0, text: "Survived".into(), glyphs: 8, clusters: 7, erased: 100, bytes: 500, skipped: None, error: None },
                ClearScanPage { page: 1, text: "Bad".into(), glyphs: 3, clusters: 3, erased: 30, bytes: 9, skipped: None, error: None },
            ],
            cancelled: false,
        };
        for (page, err) in rejected {
            report.record_failure(page, err.to_string());
        }
        assert!(report.pages[0].error.is_none());
        assert!(report.pages[1].error.is_some());
        assert_eq!(report.pages[1].glyphs, 0);
        assert_eq!(session.get(id).expect("doc").can_undo(), Some("ClearScan"));
        assert!(!session.get(id).expect("doc").text_lines(0).is_empty());
        assert!(session.get(id).expect("doc").text_lines(1).is_empty());

        // When every built entry is invalid, apply_clearscan returns an error.
        let err = session.apply_clearscan(id, vec![(1, b"not a pdf".to_vec()), (99, valid_page_pdf)]).expect_err("all invalid should fail");
        assert!(matches!(err, ClearScanError::Edit(_)), "unexpected error: {err:?}");

        // A document encrypted with user permissions that forbid modification is rejected upfront by clearscan_job.
        let mut plain = pdfcraft_cos::Document::open(session.create_blank(200.0, 200.0, 1).expect("plain")).expect("open plain");
        plain
            .set_encryption(&pdfcraft_cos::NewEncryption {
                algorithm: pdfcraft_cos::Algorithm::Aes256,
                user_password: "",
                owner_password: "owner-secret",
                permissions: 0,
                encrypt_metadata: true,
                seed: [7; 32],
            })
            .expect("encrypt");
        let locked_bytes = Arc::new(pdfcraft_cos::write_full(&plain, &pdfcraft_cos::SaveOptions::default()).expect("write locked"));
        let locked_id = session.open("locked.pdf", None, locked_bytes, None).expect("open locked");
        let job_err = match session.clearscan_job(locked_id, &[], ClearScanSettings::default()) {
            Ok(_) => panic!("modify=false should be rejected by clearscan_job"),
            Err(err) => err,
        };
        assert!(matches!(job_err, ClearScanError::Edit(EditError::NotPermitted(_))), "unexpected error: {job_err:?}");
    }

    #[test]
    fn clearscan_allows_white_backdrop_under_scan_but_skips_overlays_and_coloured_fills() {
        use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};

        let png = crate::export::encode_png(32, 32, &vec![250_u8; 32 * 32 * 4]).expect("png");
        let mut session = Session::new();
        let scan_bytes = session.create_from_images(&[("scan.png".into(), png)]).expect("scan");

        let wrap = |prefix: &[u8], suffix: &[u8]| -> Arc<Vec<u8>> {
            let mut cos = Document::open(Arc::clone(&scan_bytes)).expect("open scan");
            let page = pdfcraft_model::pages(&cos).swap_remove(0);
            let orig = page.dict.get(b"Contents").cloned().expect("contents");
            let mut parts = Vec::new();
            if !prefix.is_empty() {
                parts.push(Object::Ref(cos.add(Object::Stream(Stream::flate(Dict::new(), prefix)))));
            }
            parts.push(orig);
            if !suffix.is_empty() {
                parts.push(Object::Ref(cos.add(Object::Stream(Stream::flate(Dict::new(), suffix)))));
            }
            cos.update_dict(page.obj, |p| p.set(b"Contents".to_vec(), Object::Array(parts))).expect("update contents");
            Arc::new(write_full(&cos, &SaveOptions::default()).expect("write"))
        };

        // White background rectangles painted before the scan image are NOT flagged as vector art.
        for backdrop in [b"1 1 1 rg 0 0 32 32 re f\n".as_slice(), b"1 g 0 0 32 32 re f\n", b"0 0 0 0 k 0 0 32 32 re f\n"] {
            let pdf = wrap(backdrop, b"");
            let cos = Document::open(Arc::clone(&pdf)).expect("reopen");
            assert!(!pdfcraft_edit::page_scan_content(&cos, 0).expect("scan content").has_vector_or_inline);
            let id = session.open("backdrop.pdf", None, pdf, None).expect("open");
            let job = session.clearscan_job(id, &[], ClearScanSettings::default()).expect("job");
            assert!(job.skip_reasons[0].is_none(), "white backdrop must not skip scan: {:?}", job.skip_reasons[0]);
        }

        // White fill AFTER the image (e.g., white-out overlay) and coloured fill BEFORE the image MUST still skip.
        assert_clearscan_skipped(wrap(b"", b"\n1 1 1 rg 4 4 10 10 re f\n"), "vector");
        assert_clearscan_skipped(wrap(b"1 0 0 rg 0 0 32 32 re f\n", b""), "vector");
        assert_clearscan_skipped(wrap(b"1 1 1 rg 0 0 32 32 re B\n", b""), "vector");
    }
}
