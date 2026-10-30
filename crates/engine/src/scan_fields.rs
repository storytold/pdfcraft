//! Prepare a form ▸ field detection on scanned pages. A page that is a picture has no text and
//! no drawn shapes to read, so it is rendered and its lines and boxes are read from the pixels
//! ([`pdfcraft_forms::detect::raster`]). The labels come from the page's text layer (an earlier
//! Recognize text), else from text recognition when its models are installed. Without either,
//! the fields are still found and get the default names (Text1, Check Box1, …).
//!
//! Detection works in the page as displayed (after `/Rotate`), in points with y up, so a scan
//! stored sideways and turned upright by `/Rotate` reads like any other page. The fields are
//! mapped back to user space and turned with the page (`/MK /R`), so typed text reads upright.

use pdfcraft_forms::detect::{Candidate, Word, detect, raster};
use pdfcraft_render::{PageInfo, PageRenderer, PageText, RenderRequest};

use crate::{DocId, Document, Session, ocr};

/// Pixels per point the page is rendered at for its lines (144 dpi).
const SCALE: f32 = 2.0;
/// The longest side of that rendering, in pixels.
const MAX_SIDE: f32 = 3000.0;
/// A page is a scan when its images cover at least this share of it.
const COVERED: f64 = 0.5;
/// A picture inside Form artwork is taken for a scan when it has at least this resolution over
/// the area the artwork covers (a quarter of the pixels a 72 dpi scan would have).
const MIN_FORM_DPI: f64 = 36.0;
/// Recognised "words" less tall than this (points) are specks read as letters, and would make
/// the box they sit on look filled in.
const MIN_WORD_HEIGHT: f64 = 4.0;

/// Whether `page` is a picture of a page: images cover most of it.
pub(crate) fn is_scan(doc: &Document, page: usize, info: &PageInfo) -> bool {
    scan_images(doc, page, info).is_some()
}

/// The raster images (larger than one pixel) `page` draws, each with the share of the page (its
/// crop box) it covers, when together they cover most of it ([`is_scan`]); `None` otherwise.
/// Enhance ▸ Scanned document cleans these up ([`crate::scan_clean`]).
pub(crate) fn scan_images(doc: &Document, page: usize, info: &PageInfo) -> Option<Vec<(pdfcraft_edit::PageImage, f64)>> {
    let area = page_area(info)?;
    let images: Vec<(pdfcraft_edit::PageImage, f64)> = doc
        .page_images(page)
        .into_iter()
        .filter(|i| !i.is_form && i.width > 1 && i.height > 1)
        .filter_map(|i| Some((share(info, area, &i)?, i)))
        .map(|(share, i)| (i, share))
        .collect();
    let covered: f64 = images.iter().map(|(_, share)| share).sum();
    (covered >= COVERED).then_some(images)
}

/// The grouped Form artwork on `page` that holds a raster picture, each with the share of the
/// page it covers, when it and the page's own images together cover most of the page: a scan
/// drawn through a Form XObject, which [`scan_images`] does not count and Enhance ▸ Scanned
/// document can't clean yet, so it reports these. Empty otherwise.
pub(crate) fn scan_forms(doc: &Document, page: usize, info: &PageInfo) -> Vec<(pdfcraft_edit::PageImage, f64)> {
    let (Some(area), Some(editor)) = (page_area(info), doc.editor.as_ref()) else { return Vec::new() };
    let mut covered = 0.0;
    let mut forms = Vec::new();
    for i in doc.page_images(page) {
        let Some(share) = share(info, area, &i) else { continue };
        if !i.is_form {
            if i.width > 1 && i.height > 1 {
                covered += share;
            }
        } else if i.object.is_some_and(|form| holds_scan(&editor.cos, form, share * area)) {
            covered += share;
            forms.push((i, share));
        }
    }
    if covered >= COVERED { forms } else { Vec::new() }
}

/// Whether Form `form`, covering `points` square points of the page, draws a picture that could
/// be a scan of it: one with at least [`MIN_FORM_DPI`] over that area, not a logo inside a page
/// of vector artwork.
fn holds_scan(cos: &pdfcraft_cos::Document, form: pdfcraft_cos::ObjRef, points: f64) -> bool {
    let Some(picture) = crate::copy_image::picture_in_form(cos, form) else { return false };
    let obj = cos.get(picture);
    let pdfcraft_cos::Object::Stream(s) = &*obj else { return false };
    let px = |k: &[u8]| s.dict.int(k).unwrap_or(0).max(0) as f64;
    let per_point = MIN_FORM_DPI / 72.0;
    px(b"Width") * px(b"Height") >= points * per_point * per_point
}

/// The area of `info`'s crop box, when it is a usable one.
fn page_area(info: &PageInfo) -> Option<f64> {
    let [cx0, cy0, cx1, cy1] = info.crop.map(f64::from);
    let area = (cx1 - cx0) * (cy1 - cy0);
    (area.is_finite() && area > 0.0).then_some(area)
}

/// The share of the crop box (of area `area`) that `image` covers, when it is a number.
fn share(info: &PageInfo, area: f64, image: &pdfcraft_edit::PageImage) -> Option<f64> {
    let [cx0, cy0, cx1, cy1] = info.crop.map(f64::from);
    let r = image.visible_rect.unwrap_or(image.rect);
    let w = (r[2].min(cx1) - r[0].max(cx0)).max(0.0);
    let h = (r[3].min(cy1) - r[1].max(cy0)).max(0.0);
    let share = w * h / area;
    share.is_finite().then_some(share)
}

/// The page as displayed, in points with y up: where detection on a scan works.
struct Frame<'a>(&'a PageInfo);

impl Frame<'_> {
    /// The box around view-space points (y down).
    fn view_box(&self, points: impl IntoIterator<Item = [f32; 2]>) -> Option<[f64; 4]> {
        let h = f64::from(self.0.height);
        let mut out = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for [x, y] in points {
            let (x, y) = (f64::from(x), h - f64::from(y));
            out = [out[0].min(x), out[1].min(y), out[2].max(x), out[3].max(y)];
        }
        out.iter().all(|v| v.is_finite()).then_some(out)
    }

    /// A user-space rectangle (or the box around user-space points) in the frame.
    fn user_box(&self, points: impl IntoIterator<Item = [f64; 2]>) -> Option<[f64; 4]> {
        self.view_box(points.into_iter().map(|[x, y]| self.0.user_to_view(x as f32, y as f32)))
    }

    /// A frame rectangle in user space.
    fn user_rect(&self, r: [f64; 4]) -> [f64; 4] {
        let h = f64::from(self.0.height);
        let a = self.0.view_to_user(r[0] as f32, (h - r[1]) as f32);
        let b = self.0.view_to_user(r[2] as f32, (h - r[3]) as f32);
        [f64::from(a[0].min(b[0])), f64::from(a[1].min(b[1])), f64::from(a[0].max(b[0])), f64::from(a[1].max(b[1]))]
    }
}

fn corners(r: [f64; 4]) -> [[f64; 2]; 4] {
    [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]]
}

impl Session {
    /// The fields on scanned `page` of `id`, in user space, and how far the page turns them
    /// (its `/Rotate`). `text` is the page's text layer, `existing` its widget rectangles.
    pub(crate) fn detect_on_scan(
        &self,
        id: DocId,
        r: &mut PageRenderer,
        page: usize,
        text: Option<&PageText>,
        existing: &[[f64; 4]],
        taken: &[String],
    ) -> (Vec<Candidate>, i64) {
        let Some(info) = self.get(id).and_then(|d| d.info.pages.get(page)) else { return (Vec::new(), 0) };
        let frame = Frame(info);
        let scale = SCALE.min(MAX_SIDE / info.width.max(info.height).max(1.0));
        let shot = r.render(RenderRequest { page, scale, ..Default::default() });
        if shot.error.is_some() || shot.width == 0 {
            return (Vec::new(), 0);
        }
        let px_per_pt = f64::from(shot.width) / f64::from(info.width.max(1e-3));
        let shapes = raster::shapes(&shot.rgba, shot.width, shot.height, px_per_pt);
        if shapes.boxes.is_empty() && shapes.rules.is_empty() {
            return (Vec::new(), 0);
        }
        let mut words: Vec<Word> = text
            .map(|t| crate::js::words_with(t, |x, y| [x, y]))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(text, v)| Some(Word { text, rect: frame.view_box([[v[0], v[1]], [v[2], v[3]]])? }))
            .collect();
        if words.is_empty() {
            words = self.recognized_words(id, page, &frame);
        }
        let existing: Vec<[f64; 4]> = existing.iter().filter_map(|r| frame.user_box(corners(*r))).collect();
        let found = detect(&words, &shapes, &existing, taken);
        let turn = i64::from(info.rotation % 360);
        (found.into_iter().map(|c| Candidate { rect: frame.user_rect(c.rect), ..c }).collect(), turn)
    }

    /// The words text recognition reads on `page`, in the frame; none without the models.
    fn recognized_words(&self, id: DocId, page: usize, frame: &Frame) -> Vec<Word> {
        if !ocr::available() {
            return Vec::new();
        }
        let (Ok(engine), Some(job)) = (ocr::engine(), self.ocr_job(id, &[page], ocr::OcrSettings::default())) else {
            return Vec::new();
        };
        job.run(&engine, |_, _| true)
            .into_iter()
            .flat_map(|p| p.words)
            .filter_map(|w| {
                let o = w.origin;
                let points = [
                    o,
                    [o[0] + w.across[0], o[1] + w.across[1]],
                    [o[0] + w.up[0], o[1] + w.up[1]],
                    [o[0] + w.across[0] + w.up[0], o[1] + w.across[1] + w.up[1]],
                ];
                Some(Word { rect: frame.user_box(points)?, text: w.text })
            })
            .filter(|w| w.rect[3] - w.rect[1] >= MIN_WORD_HEIGHT)
            .collect()
    }
}
