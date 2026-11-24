//! The document's pages as pictures: what the page view, the Pages panel, Organize and the print preview ask the
//! framework's texture contract for. Revisions say when a picture is out of date; render jobs own what they read.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use craft::behavior::kurbo::Size;
use craft::pages::ui::{PageRevision, PageSource};
use craft::ui::primitives::textures::{LoadFailure, LoadedPicture, Revision, ThumbRender, ThumbRequest, ThumbSource};
use egui::{Color32, ColorImage, Rect};
use pdfcraft_engine::{DocId, Session};
use pdfcraft_render::DocInfo;
use pdfcraft_render::{PRINT_TAG, PageRenderer, RenderConfig, RenderRequest, RenderWarning, RequestKind, Tile};

use super::cache::{grayscale, texture_image};

/// A print preview picture: the page, in colour or as the monochrome print will be.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct PrintPage {
    /// The page, 0-based.
    pub page: usize,
    /// Converted to luminance, as Print ▸ Print in grayscale prints it.
    pub grayscale: bool,
}

/// Which generation of each page's picture is current: a base for the whole document and a bump per page. A picture's
/// revision includes both, so a change makes the page's picture a new one and leaves the stale one as the stand-in.
#[derive(Clone, Debug, Default)]
pub struct PageRevisions {
    base: u64,
    bumps: HashMap<usize, u64>,
}

impl PageRevisions {
    /// Every page's picture is out of date.
    pub fn bump_all(&mut self) {
        self.base = self.base.wrapping_add(1);
    }

    /// Only `page`'s picture is out of date.
    pub fn bump(&mut self, page: usize) {
        let n = self.bumps.entry(page).or_insert(0);
        *n = n.wrapping_add(1);
    }

    /// The generation of `page`'s picture: the document's and the page's own.
    pub fn of(&self, page: usize) -> (u64, u64) {
        (self.base, self.bumps.get(&page).copied().unwrap_or(0))
    }
}

#[derive(Default)]
struct Reports {
    errors: HashMap<usize, String>,
    cut: BTreeSet<usize>,
}

/// What render jobs found out about pages: why a page could not be drawn, and which pages were cut at the
/// renderer's limits. Shared with the jobs, which run on other threads.
#[derive(Clone, Default)]
pub struct RenderReports(Arc<Mutex<Reports>>);

impl RenderReports {
    fn lock(&self) -> MutexGuard<'_, Reports> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records that `page` could not be rendered.
    pub fn error(&self, page: usize, message: String) {
        self.lock().errors.insert(page, message);
    }

    /// Records that `page`'s content was cut at the renderer's limits.
    pub fn cut(&self, page: usize) {
        self.lock().cut.insert(page);
    }

    /// Forgets everything known about `page`: its content changed.
    pub fn forget(&self, page: usize) {
        let mut reports = self.lock();
        reports.errors.remove(&page);
        reports.cut.remove(&page);
    }

    /// Forgets every page's reports.
    pub fn clear(&self) {
        let mut reports = self.lock();
        reports.errors.clear();
        reports.cut.clear();
    }

    /// The pages that could not be rendered with the reason, in page order.
    pub fn errors(&self) -> Vec<(usize, String)> {
        let mut all: Vec<(usize, String)> = self.lock().errors.iter().map(|(p, e)| (*p, e.clone())).collect();
        all.sort_unstable_by_key(|(p, _)| *p);
        all
    }

    /// The pages cut at the renderer's limits, in page order.
    pub fn cut_pages(&self) -> Vec<usize> {
        self.lock().cut.iter().copied().collect()
    }
}

/// One document's pages as a [`PageSource`] and the source of print previews. It owns what a render job reads, so
/// jobs outlive the pass that made them.
pub struct DocPages<'a> {
    info: Option<&'a DocInfo>,
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
    rotation: u16,
    revisions: Option<&'a PageRevisions>,
    reports: Option<RenderReports>,
}

impl<'a> DocPages<'a> {
    /// The pages of document `doc`; with no such document there are no pages.
    pub fn new(session: &'a Session, doc: DocId) -> Self {
        let document = session.get(doc);
        Self {
            info: document.map(|d| &d.info),
            bytes: document.map(|d| d.display.clone()).unwrap_or_default(),
            config: session.render_config(doc).unwrap_or_default(),
            rotation: 0,
            revisions: None,
            reports: None,
        }
    }

    /// Shows the pages turned by the view's rotation (degrees clockwise).
    #[must_use]
    pub fn rotated(mut self, degrees: u16) -> Self {
        self.rotation = degrees % 360;
        self
    }

    /// Versions the pictures by `revisions`.
    #[must_use]
    pub fn revisions(mut self, revisions: &'a PageRevisions) -> Self {
        self.revisions = Some(revisions);
        self
    }

    /// Has render jobs report failures and cut pages to `reports`.
    #[must_use]
    pub fn reports(mut self, reports: &RenderReports) -> Self {
        self.reports = Some(reports.clone());
        self
    }

    fn revision_of(&self, page: usize, request: &ThumbRequest, print: Option<bool>) -> Revision {
        let generation = self.revisions.map_or((0, 0), |r| r.of(page));
        let region = request.region.map(|r| [r.min.x.to_bits(), r.min.y.to_bits(), r.max.x.to_bits(), r.max.y.to_bits()]);
        Revision::of(&(generation, request.side, region, self.rotation, print))
    }

    /// The job making `page` at `request`; `print` is `Some(grayscale)` for a print preview.
    fn render_page(&self, page: usize, request: &ThumbRequest, print: Option<bool>) -> ThumbRender {
        let reports = self.reports.clone();
        let Some(info) = self.info.and_then(|i| i.pages.get(page)) else {
            return ThumbRender::Later(Box::new(|_| Err(LoadFailure::Missing)));
        };
        // The region is in the turned frame the view shows; the renderer draws the page unturned.
        let region = request.region.map(|r| unturned(r, self.rotation));
        let (pw, ph) = (f64::from(info.width), f64::from(info.height));
        let part = region.map_or((pw, ph), |r| (f64::from(r.width()) * pw, f64::from(r.height()) * ph));
        let scale = (f64::from(request.side) / part.0.max(part.1).max(1e-3)) as f32;
        if !scale.is_finite() || scale <= 0.0 {
            return ThumbRender::Later(Box::new(|_| Err(LoadFailure::Unreadable)));
        }
        let tile = region.map(|r| Tile {
            x: (f64::from(r.min.x) * pw * f64::from(scale)).floor().max(0.0) as u32,
            y: (f64::from(r.min.y) * ph * f64::from(scale)).floor().max(0.0) as u32,
            w: (part.0 * f64::from(scale)).ceil().max(1.0) as u32,
            h: (part.1 * f64::from(scale)).ceil().max(1.0) as u32,
        });
        let (bytes, rotation) = (self.bytes.clone(), self.rotation);
        // Installed faces draw on screen only: a print preview must match the printout.
        let config = if print.is_some() { self.config.clone() } else { self.config.clone().on_screen() };
        let tag = if print.is_some() { PRINT_TAG } else { 0 };
        ThumbRender::Later(Box::new(move |cancel| {
            if cancel.is_cancelled() {
                return Err(LoadFailure::Missing);
            }
            let mut renderer = PageRenderer::new(bytes, config);
            let out = renderer.render(RenderRequest { page, kind: RequestKind::Pixels, tile, scale, tag, preview: false });
            if cancel.is_cancelled() {
                return Err(LoadFailure::Missing);
            }
            if out.warnings.contains(&RenderWarning::ContentTruncated)
                && let Some(reports) = &reports
            {
                reports.cut(page);
            }
            if let Some(error) = out.error {
                log::warn!("page {}: {error}", page + 1);
                if let Some(reports) = &reports {
                    reports.error(page, error.clone());
                }
                return Err(LoadFailure::Message(error));
            }
            let size = [out.width as usize, out.height as usize];
            if size[0] == 0 || size[1] == 0 {
                return Err(LoadFailure::Unreadable);
            }
            let image = turned(texture_image(size, out.rgba), rotation);
            Ok(LoadedPicture::Image(if print == Some(true) { grayscale(image) } else { image }))
        }))
    }
}

impl PageSource for DocPages<'_> {
    fn page_revision(&self, page: usize) -> Option<PageRevision> {
        let (document, page) = self.revisions?.of(page);
        Some(PageRevision { document, page })
    }

    fn page_count(&self) -> usize {
        self.info.map_or(0, |i| i.pages.len())
    }

    fn page_size(&self, page: usize) -> Size {
        let Some(p) = self.info.and_then(|i| i.pages.get(page)) else { return Size::ZERO };
        let (w, h) = (f64::from(p.width), f64::from(p.height));
        if self.rotation % 180 == 90 { Size::new(h, w) } else { Size::new(w, h) }
    }

    fn page_label(&self, page: usize) -> String {
        self.info.and_then(|i| i.pages.get(page)).map_or_else(|| (page + 1).to_string(), |p| p.label.clone())
    }
}

impl ThumbSource<usize> for DocPages<'_> {
    fn revision(&self, key: &usize, request: &ThumbRequest) -> Revision {
        self.revision_of(*key, request, None)
    }

    fn render(&self, key: &usize, request: &ThumbRequest) -> ThumbRender {
        self.render_page(*key, request, None)
    }
}

impl ThumbSource<PrintPage> for DocPages<'_> {
    fn revision(&self, key: &PrintPage, request: &ThumbRequest) -> Revision {
        self.revision_of(key.page, request, Some(key.grayscale))
    }

    fn render(&self, key: &PrintPage, request: &ThumbRequest) -> ThumbRender {
        self.render_page(key.page, request, Some(key.grayscale))
    }
}

/// The part of the unturned page that shows as `region` (fractions) once the page is turned `rotation` degrees
/// clockwise.
fn unturned(region: Rect, rotation: u16) -> Rect {
    let (u0, u1, v0, v1) = (region.min.x, region.max.x, region.min.y, region.max.y);
    match rotation % 360 {
        90 => Rect::from_min_max(egui::pos2(v0, 1.0 - u1), egui::pos2(v1, 1.0 - u0)),
        180 => Rect::from_min_max(egui::pos2(1.0 - u1, 1.0 - v1), egui::pos2(1.0 - u0, 1.0 - v0)),
        270 => Rect::from_min_max(egui::pos2(1.0 - v1, u0), egui::pos2(1.0 - v0, u1)),
        _ => region,
    }
}

/// `image` turned `rotation` degrees clockwise (a multiple of 90; anything else leaves it).
fn turned(image: ColorImage, rotation: u16) -> ColorImage {
    let [w, h] = image.size;
    let at = |x: usize, y: usize| image.pixels.get(y.saturating_mul(w).saturating_add(x)).copied().unwrap_or(Color32::TRANSPARENT);
    match rotation % 360 {
        90 => ColorImage::new([h, w], (0..w).flat_map(|y| (0..h).map(move |x| (x, y))).map(|(x, y)| at(y, h - 1 - x)).collect()),
        180 => ColorImage::new([w, h], image.pixels.iter().rev().copied().collect()),
        270 => ColorImage::new([h, w], (0..w).flat_map(|y| (0..h).map(move |x| (x, y))).map(|(x, y)| at(w - 1 - y, x)).collect()),
        _ => image,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PdfCraftApp;
    use craft::host::{AppInfo, Host, ManualJobs, Wake};
    use egui_kittest::Harness;

    /// Runs frames and the queued jobs until every picture has landed.
    fn settle(h: &mut Harness<'static, PdfCraftApp>, jobs: &ManualJobs) {
        for _ in 0..40 {
            h.run_steps(1);
            if jobs.run_all() == 0 && !h.state().render_pending() {
                return;
            }
        }
        panic!("the pictures never settled");
    }

    /// A page edit must queue a render for that page only and a document change one for every page: a wrong bump
    /// leaves a stale picture on screen with no error.
    #[test]
    fn a_page_edit_makes_its_picture_a_new_revision_and_leaves_the_others() {
        let jobs = Arc::new(ManualJobs::new());
        let queue = jobs.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let host = Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" }).jobs(queue.clone()).build();
                let services = crate::AppServices::new(
                    Wake::none(),
                    craft::platform::clipboard::Clipboard::new(craft::platform::clipboard::SystemClipboard::new()),
                    None,
                    None,
                    craft::platform::open::FileIntake::new(Wake::none()).0,
                    craft::platform::watch::FileWatch::new(Wake::none()).inline(),
                );
                let mut app = PdfCraftApp::with_host(Arc::new(host), services);
                let blank = app.session.create_blank(200.0, 300.0, 3).expect("a blank document");
                app.open_bytes(&egui::Context::default(), "three.pdf", None, blank.as_ref().clone()).expect("opens");
                app
            }
        });
        settle(&mut h, &jobs);
        // One frame queues what the change outdates; the jobs wait for the test to run them.
        h.state_mut().views[0].page_changed(1);
        h.run_steps(1);
        let edited = jobs.waiting();
        settle(&mut h, &jobs);
        let info = {
            let app = h.state();
            app.session.get(app.views[0].id).expect("the document").info.clone()
        };
        let ctx = h.ctx.clone();
        h.state_mut().views[0].document_changed(&ctx, &info);
        h.run_steps(1);
        let all = jobs.waiting();
        assert!(edited >= 1, "the edited page is rendered again");
        assert!(edited < all, "the other pages keep their pictures: {edited} jobs for one page, {all} for the document");
    }
}
