//! Scan & OCR ▸ Deskew pages: straighten a scanned page's skew by rotating its picture about
//! the page centre. The page keeps its size, its margins and everything that is not the picture
//! (invisible OCR text, comments, links), so a straightened page looks like the page it was.
//!
//! The angle comes from the same estimator OCR preprocessing uses (projection profile over row
//! bands), so the two agree; a page that is straight, blank, or unclear is left alone.

use std::sync::Arc;

use pdfcraft_render::{RenderConfig, RenderRequest};

use crate::{DocId, Edit, EditError, ImageResolution, Session};

/// The densest a page is read at: 600 dpi, as for OCR.
const MAX_SCALE: f32 = 600.0 / 72.0;
/// The resolution a page without a picture is read at.
const DEFAULT_SCALE: f32 = 300.0 / 72.0;
/// The most pixels the page is rendered with.
const MAX_PIXELS: f64 = 40_000_000.0;
/// JPEG quality of the rotated picture.
const QUALITY: u8 = 90;
/// Beyond this the estimate is not a skew but a different orientation: leave the page alone.
const MAX_ANGLE: f64 = 10.0;

/// What [`Session::deskew_page`] did with one page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Deskewed {
    /// The page was rotated, by this many degrees (positive = counter-clockwise).
    Straightened(f64),
    /// Measured and already straight enough: untouched.
    AlreadyStraight,
    /// No skew could be measured (a full-page photograph, a drawing, a blank page): untouched,
    /// for the user to look at.
    Unmeasured,
}


/// Skew from the page's long straight rules (table borders, frames), for pages where the text
/// lines are too sparse to judge (a photo page with a caption frame): only pixels that belong to
/// a long horizontal run are scored, so photographs contribute nothing. Returns the angle in the
/// same convention as [`pdfcraft_ocr::preprocess::estimate_skew`], and only when the winning
/// angle beats 0° by the same margin that estimator asks for.
fn estimate_skew_by_rules(rgba: &[u8], w: u32, h: u32) -> Option<f64> {
    let (w, h) = (w as usize, h as usize);
    if w < 64 || h < 64 || rgba.len() < w * h * 4 {
        return None;
    }
    let gray: Vec<u8> = rgba.chunks_exact(4).map(|px| ((u32::from(px[0]) * 299 + u32::from(px[1]) * 587 + u32::from(px[2]) * 114) / 1000) as u8).collect();
    let (mut lo, mut hi) = (255u8, 0u8);
    for &g in &gray {
        lo = lo.min(g);
        hi = hi.max(g);
    }
    if hi <= lo + 8 {
        return None; // a blank page has no skew
    }
    let t = (u32::from(lo) + u32::from(hi)) / 2;
    let ink = |x: usize, y: usize| u32::from(gray[y * w + x]) < t;
    // A rule is ink that is thin: a photograph's blotches are tall runs of ink, so they are
    // dropped before anything is measured.
    const THICK: usize = 9;
    let thin = |x: usize, y: usize| -> bool {
        if !ink(x, y) {
            return false;
        }
        let top = y.saturating_sub(THICK / 2);
        let bottom = (y + THICK / 2).min(h - 1);
        !((top..=bottom).all(|yy| ink(x, yy)) && bottom - top + 1 >= THICK)
    };
    let min_run = (w / 25).max(24);
    let mut keep = vec![false; w * h];
    for y in 0..h {
        let mut x = 0;
        while x < w {
            if !thin(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < w && thin(x, y) {
                x += 1;
            }
            if x - start >= min_run {
                for i in start..x {
                    keep[y * w + i] = true;
                }
            }
        }
    }
    if keep.iter().filter(|&&k| k).count() < 200 {
        return None;
    }
    let cx = w as f64 / 2.0;
    let max_shift = (cx * 5f64.to_radians().tan()).ceil() as i64 + 2;
    let score = |phi_deg: f64| -> f64 {
        let slope = phi_deg.to_radians().tan();
        let mut profile = vec![0u64; h + 2 * max_shift as usize + 1];
        for y in 0..h {
            for x in 0..w {
                if !keep[y * w + x] {
                    continue;
                }
                let r = y as f64 - (x as f64 - cx) * slope;
                let idx = r.round() as i64 + max_shift;
                if let Some(p) = profile.get_mut(idx as usize) {
                    *p += 1;
                }
            }
        }
        profile.iter().map(|p| (*p as f64).powi(2)).sum()
    };
    let flat = score(0.0);
    let mut best = (0.0f64, flat);
    let mut phi = -5.0;
    while phi <= 5.0 + 1e-9 {
        let v = score(phi);
        if v > best.1 {
            best = (phi, v);
        }
        phi += 0.25;
    }
    let (mut angle, mut best_score) = best;
    let mut d = -0.25;
    while d <= 0.25 + 1e-9 {
        let v = score(angle + d);
        if v > best_score {
            best_score = v;
            angle += d;
        }
        d += 0.05;
    }
    // Rules are unambiguous evidence (a photograph cannot make a long horizontal run), so the
    // bar is lower than the text estimator's.
    if best_score < 1.005 * flat || angle.abs() < 0.1 {
        return None;
    }
    Some(angle)
}


/// Skew from the strongest straight edges on the page (table rules, frames), for the pages whose
/// text is too sparse for [`pdfcraft_ocr::preprocess::estimate_skew_detail`] — a photograph
/// montage with a thin caption frame still has its rules, and a photograph's own edges spread
/// their votes evenly while a rule's do not.
///
/// Works on a Sobel edge map at about 200 dpi: each edge pixel votes, weighted by its gradient,
/// for the angle that would put its row on a common line. Returns the angle in the same
/// convention as the text estimator, and only when it beats 0° by `1.02`.
fn estimate_skew_by_edges(rgba: &[u8], w: u32, h: u32) -> Option<(f64, f64)> {
    let (w0, h0) = (w as usize, h as usize);
    if w0 < 64 || h0 < 64 || rgba.len() < w0 * h0 * 4 {
        return None;
    }
    // Down to about 200 dpi worth of pixels: enough for a rule, cheap enough to score many angles.
    let step = ((w0.max(h0) as f64 / 1700.0).ceil() as usize).max(1);
    let (w, h) = (w0 / step, h0 / step);
    if w < 64 || h < 64 {
        return None;
    }
    let mut gray = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (x * step, y * step);
            let i = (sy * w0 + sx) * 4;
            let p = &rgba[i..i + 3];
            gray[y * w + x] = 0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]);
        }
    }
    // Sobel: an edge pixel's weight, kept only when the gradient is worth trusting.
    let at = |x: usize, y: usize| gray[y * w + x];
    let mut pts: Vec<(f64, f64, f64)> = Vec::new();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let gx = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1)) - (at(x - 1, y - 1) + 2.0 * at(x - 1, y) + at(x - 1, y + 1));
            let gy = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1)) - (at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1));
            let mag = gx.abs() + gy.abs();
            if mag < 120.0 {
                continue;
            }
            pts.push((x as f64, y as f64, f64::from(mag)));
        }
    }
    if pts.len() < 500 {
        return None;
    }
    let cx = w as f64 / 2.0;
    let max_shift = (cx * 5f64.to_radians().tan()).ceil() as i64 + 2;
    let score = |phi_deg: f64| -> f64 {
        let slope = phi_deg.to_radians().tan();
        let mut profile = vec![0f64; h + 2 * max_shift as usize + 1];
        for &(x, y, m) in &pts {
            let r = y - (x - cx) * slope;
            let idx = r.round() as i64 + max_shift;
            if let Some(p) = profile.get_mut(idx as usize) {
                *p += m;
            }
        }
        profile.iter().map(|p| p * p).sum()
    };
    let flat = score(0.0);
    let mut best = (0.0f64, flat);
    let mut phi = -5.0;
    while phi <= 5.0 + 1e-9 {
        let v = score(phi);
        if v > best.1 {
            best = (phi, v);
        }
        phi += 0.25;
    }
    let (mut angle, mut best_score) = best;
    let mut d = -0.25;
    while d <= 0.25 + 1e-9 {
        let v = score(angle + d);
        if v > best_score {
            best_score = v;
            angle += d;
        }
        d += 0.05;
    }
    if angle.abs() < 0.05 || !best_score.is_finite() || !flat.is_finite() {
        return None;
    }
    Some((angle, best_score / flat))
}


/// The page's skew as this module decides it: text lines first, then the page's edges, both only
/// when the estimator trusts its own answer.
fn measure(image: &pdfcraft_ocr::OcrImage) -> Option<f64> {
    if let Some(skew) = pdfcraft_ocr::preprocess::estimate_skew_detail(image)
        && skew.clear
    {
        return Some(skew.angle);
    }
    estimate_skew_by_edges(image.rgba(), image.width(), image.height()).and_then(|(a, m)| (m >= 1.02).then_some(a))
}

/// Everything one page needs, taken from the document without changing it. Plain data, so a
/// caller may hand it to another thread.
#[derive(Clone)]
pub struct DeskewInputs {
    pages: Vec<PageInputs>,
    /// Pages left out because they already carry searchable text, with the reason to report.
    skipped: Vec<(usize, &'static str)>,
    min_angle: f64,
}

#[derive(Clone)]
struct PageInputs {
    page: usize,
    bytes: Arc<Vec<u8>>,
    password: Option<Arc<str>>,
    width: f64,
    scale: f32,
}

/// A page measured, and rotated and encoded when it needed it: ready to be put back.
pub struct Prepared {
    pub page: usize,
    pub outcome: Deskewed,
    /// Why the page could not be measured, when that is known.
    pub error: Option<String>,
    /// The straightened page as JPEG, and the dpi that keeps its physical size.
    raster: Option<(Vec<u8>, f64)>,
}

impl DeskewInputs {
    /// Measure (and rotate, and encode) every page, in parallel. Nothing here touches a document.
    pub fn prepare(self, threads: usize) -> Vec<Prepared> {
        // On the web there are no threads to spawn: measure one page after another.
        #[cfg(target_arch = "wasm32")]
        let threads = 1;
        #[cfg(not(target_arch = "wasm32"))]
        let threads = threads.max(1).min(self.pages.len().max(1));
        let mut done: Vec<Prepared> = Vec::with_capacity(self.pages.len() + self.skipped.len());
        if threads == 1 || self.pages.len() <= 1 {
            for input in &self.pages {
                done.push(prepare_page(input, self.min_angle));
            }
        } else {
            let next = std::sync::atomic::AtomicUsize::new(0);
            let slots: Vec<std::sync::Mutex<Option<Prepared>>> =
                (0..self.pages.len()).map(|_| std::sync::Mutex::new(None)).collect();
            std::thread::scope(|scope| {
                for _ in 0..threads {
                    scope.spawn(|| loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(input) = self.pages.get(i) else { return };
                        let prepared = prepare_page(input, self.min_angle);
                        *slots[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(prepared);
                    });
                }
            });
            for (input, slot) in self.pages.iter().zip(slots) {
                done.push(slot.into_inner().unwrap_or_else(|e| e.into_inner()).unwrap_or(Prepared {
                    page: input.page,
                    outcome: Deskewed::Unmeasured,
                    error: Some("the page was not prepared".into()),
                    raster: None,
                }));
            }
        }
        for (page, why) in &self.skipped {
            done.push(Prepared { page: *page, outcome: Deskewed::Unmeasured, error: Some((*why).into()), raster: None });
        }
        done.sort_by_key(|p| p.page);
        done
    }
}

/// Measure one page and, when it needs it, rotate and encode it. Touches nothing, so two threads
/// can run this on different pages of the same document.
fn prepare_page(input: &PageInputs, min_angle: f64) -> Prepared {
    let config = RenderConfig { password: input.password.clone(), hide_comments: false, ..Default::default() };
    let mut r = pdfcraft_render::PageRenderer::new(input.bytes.clone(), config);
    let shot = r.render(RenderRequest { page: input.page, scale: input.scale, ..Default::default() });
    if let Some(e) = shot.error {
        return Prepared { page: input.page, outcome: Deskewed::Unmeasured, error: Some(format!("the page could not be rendered: {e}")), raster: None };
    }
    if shot.warnings.iter().any(|w| matches!(w, pdfcraft_render::RenderWarning::ContentTruncated)) {
        // Part of the page was left out of the raster: measuring it would replace the page with
        // an incomplete picture, so it is left alone and says why.
        return Prepared {
            page: input.page,
            outcome: Deskewed::Unmeasured,
            error: Some("the page did not render completely, so straightening it would drop the part that was left out".into()),
            raster: None,
        };
    }
    let image = match pdfcraft_ocr::OcrImage::new(shot.width, shot.height, shot.rgba.to_vec()) {
        Ok(image) => image,
        Err(e) => {
            return Prepared { page: input.page, outcome: Deskewed::Unmeasured, error: Some(format!("the page could not be read as a picture: {e}")), raster: None };
        }
    };
    let angle = match measure(&image) {
        Some(angle) if angle.abs() >= min_angle && angle.abs() <= MAX_ANGLE => angle,
        other => {
            // Nothing to act on. A hint of a real tilt without the confidence to act on it is
            // worth the user's eye; anything else is straight as far as this can tell.
            let hint = other.is_none() && hint_of_tilt(&image).is_some_and(|a| a.abs() >= 1.0);
            return Prepared { page: input.page, outcome: if hint { Deskewed::Unmeasured } else { Deskewed::AlreadyStraight }, error: None, raster: None };
        }
    };
    let (rotated, _) = pdfcraft_ocr::preprocess::deskew_rotate(&image, angle);
    // Check the correction on the rotated picture itself: a real one leaves less than 60% of the
    // skew behind, so a misread page is dropped rather than made worse.
    if let Some(left) = measure(&rotated)
        && left.abs() > angle.abs() * 0.6
    {
        return Prepared { page: input.page, outcome: Deskewed::Unmeasured, error: None, raster: None };
    }
    let (rw, rh) = (rotated.width(), rotated.height());
    let mut rgb = Vec::with_capacity(rw as usize * rh as usize * 3);
    for px in rotated.rgba().chunks_exact(4) {
        let a = f32::from(px[3]) / 255.0;
        for &c in &px[..3] {
            rgb.push((f32::from(c) + 255.0 * (1.0 - a)).round().clamp(0.0, 255.0) as u8);
        }
    }
    let mut encoded = Vec::new();
    if let Err(e) = image::ImageEncoder::write_image(
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, QUALITY),
        &rgb,
        rw,
        rh,
        image::ExtendedColorType::Rgb8,
    ) {
        return Prepared { page: input.page, outcome: Deskewed::Unmeasured, error: Some(format!("the straightened page could not be encoded: {e}")), raster: None };
    }
    // The picture keeps its pixel count, so the page keeps its physical size.
    let k = shot.width as f64 / input.width.max(1e-3);
    let dpi = (k * 72.0).clamp(1.0, 1200.0);
    Prepared { page: input.page, outcome: Deskewed::Straightened(angle), error: None, raster: Some((encoded, dpi)) }
}

/// The angle an estimator would act on if it were sure, for a page it cannot judge.
fn hint_of_tilt(image: &pdfcraft_ocr::OcrImage) -> Option<f64> {
    if let Some(skew) = pdfcraft_ocr::preprocess::estimate_skew_detail(image) {
        return Some(skew.angle);
    }
    estimate_skew_by_edges(image.rgba(), image.width(), image.height()).map(|(a, _)| a)
}

impl Session {
    /// Gather what the deskew pass needs for `pages`, without changing the document. Plain data,
    /// so the measuring can happen on another thread.
    ///
    /// A page that already carries searchable text is left out: it is straightened by rotating its
    /// picture, which replaces the page's content, and the invisible text of an OCR layer would go
    /// with it. Such pages are reported with the reason instead.
    pub fn deskew_inputs(&self, id: DocId, pages: &[usize], min_angle: f64) -> Result<DeskewInputs, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if let Some(why) = &doc.read_only_reason {
            return Err(EditError::ReadOnly(why.clone()));
        }
        let password: Option<Arc<str>> = doc.password.as_deref().map(Arc::from);
        let mut inputs = Vec::with_capacity(pages.len());
        let mut skipped = Vec::new();
        let mut text_renderer = None;
        for &page in pages {
            let info = doc.info.pages.get(page).ok_or_else(|| EditError::Invalid(format!("there is no page {}", page.saturating_add(1))))?;
            let has_text = {
                let r = text_renderer.get_or_insert_with(|| {
                    pdfcraft_render::PageRenderer::new(doc.bytes.clone(), RenderConfig { password: password.clone(), ..Default::default() })
                });
                r.render(RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() })
                    .text
                    .is_some_and(|t| !t.plain_text().trim().is_empty())
            };
            if has_text {
                skipped.push((page, "the page has searchable text, which replacing its picture would drop"));
                continue;
            }
            let native = doc
                .page_images(page)
                .iter()
                .filter(|i| i.width > 1 && i.height > 1)
                .map(|i| {
                    let m = i.matrix;
                    let (w, h) = (m[0].hypot(m[1]), m[2].hypot(m[3]));
                    (i.width as f64 / w.max(1e-6)).max(i.height as f64 / h.max(1e-6)) as f32
                })
                .filter(|s| s.is_finite())
                .reduce(f32::max);
            let area = (info.width.max(1.0) as f64) * (info.height.max(1.0) as f64);
            let cap = (MAX_PIXELS / area).sqrt() as f32;
            let scale = native.unwrap_or(DEFAULT_SCALE).clamp(1.0, MAX_SCALE).min(cap).max(0.01);
            inputs.push(PageInputs { page, bytes: doc.bytes.clone(), password: password.clone(), width: f64::from(info.width), scale });
        }
        Ok(DeskewInputs { pages: inputs, skipped, min_angle })
    }

    /// Measure and rotate `pages` without touching the document; see [`Session::deskew_inputs`].
    pub fn prepare_deskew(&self, id: DocId, pages: &[usize], min_angle: f64, threads: usize) -> Result<Vec<Prepared>, EditError> {
        Ok(self.deskew_inputs(id, pages, min_angle)?.prepare(threads))
    }

    /// Put one prepared page back, as one undoable step.
    pub fn apply_prepared(&mut self, id: DocId, prepared: Prepared) -> Result<Deskewed, EditError> {
        let Some((raster, dpi)) = prepared.raster else { return Ok(prepared.outcome) };
        let bytes = self.create_from_images_with_resolution(&[("deskew.jpg".into(), raster)], ImageResolution::Dpi(dpi))?;
        let replace = Edit::ReplacePages { pages: vec![prepared.page], name: "deskew.pdf".into(), bytes, src_pages: vec![0] };
        self.apply(id, Edit::Batch { label: "Deskew page".into(), edits: vec![replace] })?;
        Ok(prepared.outcome)
    }

    /// Straighten `pages`: measure them all first, then put them back one by one.
    pub fn deskew_pages(&mut self, id: DocId, pages: &[usize], min_angle: f64, threads: usize) -> Result<Vec<(usize, Deskewed, Option<String>)>, EditError> {
        let prepared = self.prepare_deskew(id, pages, min_angle, threads)?;
        let mut out = Vec::with_capacity(prepared.len());
        for page in prepared {
            let index = page.page;
            let error = page.error.clone();
            let outcome = self.apply_prepared(id, page)?;
            out.push((index, outcome, error));
        }
        Ok(out)
    }

    /// Straighten page `page` (0-based) of document `id`: find the skew of its text lines and
    /// rotate the page's picture by it about the centre, white fill, the same pixels and so the
    /// same page size. Skew under `min_angle` degrees leaves the page untouched.
    pub fn deskew_page(&mut self, id: DocId, page: usize, min_angle: f64) -> Result<Deskewed, EditError> {
        Ok(self.deskew_page_report(id, page, min_angle)?.0)
    }

    /// [`Session::deskew_page`], with the reason a page was left alone when there is one (a page
    /// that carries searchable text, or one that did not render completely).
    pub fn deskew_page_report(&mut self, id: DocId, page: usize, min_angle: f64) -> Result<(Deskewed, Option<String>), EditError> {
        let mut done = self.deskew_pages(id, &[page], min_angle, 1)?;
        Ok(done.pop().map_or((Deskewed::AlreadyStraight, None), |(_, d, why)| (d, why)))
    }
}
