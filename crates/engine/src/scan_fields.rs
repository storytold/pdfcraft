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
/// Recognised "words" less tall than this (points) are specks read as letters, and would make
/// the box they sit on look filled in.
const MIN_WORD_HEIGHT: f64 = 4.0;

/// Whether `page` is a picture of a page: images cover most of it.
pub(crate) fn is_scan(doc: &Document, page: usize, info: &PageInfo) -> bool {
    let [cx0, cy0, cx1, cy1] = info.crop.map(f64::from);
    let page_area = (cx1 - cx0) * (cy1 - cy0);
    if !page_area.is_finite() || page_area <= 0.0 {
        return false;
    }
    let covered: f64 = doc
        .page_images(page)
        .iter()
        .filter(|i| !i.is_form && i.width > 1 && i.height > 1)
        .map(|i| {
            let r = i.visible_rect.unwrap_or(i.rect);
            let w = (r[2].min(cx1) - r[0].max(cx0)).max(0.0);
            let h = (r[3].min(cy1) - r[1].max(cy0)).max(0.0);
            w * h
        })
        .filter(|a| a.is_finite())
        .sum();
    covered >= COVERED * page_area
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
        // Field detection reads the original raster's boxes and labels, without applying
        // document cleanup. Deskew resampling can join a checkbox to its adjacent label
        // ("□Haupt"), making it look like text inside an already-filled field.
        let settings = ocr::OcrSettings { auto_rotate: false, deskew: false, auto_crop: false, ..Default::default() };
        let (Ok(engine), Some(job)) = (ocr::engine(), self.ocr_job(id, &[page], settings)) else {
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
