//! Automatic content detection and white margin removal (`organize.remove-white-margins`).
//!
//! Scans a page's visual raster output to find the bounding box of non-white content
//! (text, images, vector artwork) and computes the corresponding crop rectangle in PDF user space.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

use crate::{Document, EditError};

/// Scans the rendered page to find the bounding box of visible non-white content in PDF user space.
///
/// Returns `Ok(Some([x0, y0, x1, y1]))` in user space if content was found, or `Ok(None)` if
/// the page is entirely white or empty.
pub fn detect_page_content_box(doc: &Document, page: usize) -> Result<Option<[f64; 4]>, EditError> {
    let config = RenderConfig { password: doc.password.as_deref().map(Arc::from), layers: doc.config.layers.clone(), ..Default::default() };
    let mut renderer = PageRenderer::new(doc.display.clone(), config);
    let all_boxes = doc.page_boxes();
    detect_page_content_box_impl(doc, page, &mut renderer, &all_boxes)
}

/// Bounding box of detected non-white content for a page: `(page_index, Option<[x0, y0, x1, y1]>)`.
pub type PageContentBox = (usize, Option<[f64; 4]>);

/// Batch content box detection reusing a single [`PageRenderer`] and `page_boxes` snapshot across multiple pages.
pub fn detect_pages_content_boxes(doc: &Document, pages: &[usize]) -> Result<Vec<PageContentBox>, EditError> {
    let config = RenderConfig { password: doc.password.as_deref().map(Arc::from), layers: doc.config.layers.clone(), ..Default::default() };
    let mut renderer = PageRenderer::new(doc.display.clone(), config);
    let all_boxes = doc.page_boxes();
    let mut results = Vec::with_capacity(pages.len());
    for &page in pages {
        let bbox = detect_page_content_box_impl(doc, page, &mut renderer, &all_boxes)?;
        results.push((page, bbox));
    }
    Ok(results)
}

/// Internal helper scanning the raster output of a single page using a shared renderer and page boxes snapshot.
fn detect_page_content_box_impl(
    doc: &Document,
    page: usize,
    renderer: &mut PageRenderer,
    all_boxes: &[[[f64; 4]; 5]],
) -> Result<Option<[f64; 4]>, EditError> {
    let Some(info) = doc.info.pages.get(page) else {
        return Err(EditError::Invalid(format!("there is no page {}", page.saturating_add(1))));
    };

    if info.width <= 0.5 || info.height <= 0.5 {
        return Ok(None);
    }

    // Adapt scale so rendering is fast and capped in memory while providing sub-point or 1-point accuracy.
    let max_dim = info.width.max(info.height);
    let scale = if max_dim.is_finite() && max_dim > 0.0 { (1200.0 / max_dim).clamp(0.2, 1.5) } else { 1.0 };

    let shot = renderer.render(RenderRequest { page, scale, ..Default::default() });
    if let Some(e) = shot.error {
        return Err(EditError::Invalid(format!("page {} could not be rendered: {e}", page + 1)));
    }

    let w = shot.width;
    let h = shot.height;
    if w == 0 || h == 0 {
        return Ok(None);
    }

    let bytes = shot.rgba.as_ref();
    let mut min_x = u32::MAX;
    let mut max_x = 0;
    let mut min_y = u32::MAX;
    let mut max_y = 0;

    for y in 0..h {
        let row_offset = match (y as usize).checked_mul(w as usize).and_then(|v| v.checked_mul(4)) {
            Some(o) => o,
            None => break,
        };
        for x in 0..w {
            let idx = row_offset.saturating_add((x as usize).saturating_mul(4));
            if idx.saturating_add(3) < bytes.len() {
                let r = bytes[idx];
                let g = bytes[idx + 1];
                let b = bytes[idx + 2];
                let a = bytes[idx + 3];

                // Check for non-white content: pixel must be non-transparent and not near-white paper.
                if a > 15 {
                    let a_f = a as f32;
                    let rf = ((r as f32 * 255.0) / a_f).min(255.0) as u8;
                    let gf = ((g as f32 * 255.0) / a_f).min(255.0) as u8;
                    let bf = ((b as f32 * 255.0) / a_f).min(255.0) as u8;

                    // Anything darker than 248 on any color channel is considered ink/content.
                    if rf < 248 || gf < 248 || bf < 248 {
                        if x < min_x {
                            min_x = x;
                        }
                        if x > max_x {
                            max_x = x;
                        }
                        if y < min_y {
                            min_y = y;
                        }
                        if y > max_y {
                            max_y = y;
                        }
                    }
                }
            }
        }
    }

    if min_x > max_x || min_y > max_y {
        return Ok(None);
    }

    let eff_scale = pdfcraft_render::effective_scale(info.width, info.height, scale);
    let s = if eff_scale.is_finite() && eff_scale > 0.001 { eff_scale as f64 } else { 1.0 };
    let vx0 = min_x as f64 / s;
    let vy0 = min_y as f64 / s;
    let vx1 = (max_x.saturating_add(1)) as f64 / s;
    let vy1 = (max_y.saturating_add(1)) as f64 / s;

    // Convert corners from view space to PDF user space.
    let corners = [[vx0, vy0], [vx1, vy0], [vx0, vy1], [vx1, vy1]].map(|[x, y]| info.view_to_user(x as f32, y as f32));

    let ux0 = corners.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min) as f64;
    let ux1 = corners.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max) as f64;
    let uy0 = corners.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min) as f64;
    let uy1 = corners.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max) as f64;

    if !ux0.is_finite() || !ux1.is_finite() || !uy0.is_finite() || !uy1.is_finite() {
        return Ok(None);
    }

    // Clamp within the page's MediaBox (or crop box when page boxes are unavailable).
    let media = all_boxes.get(page).map(|boxes| boxes[0]).unwrap_or_else(|| info.crop.map(|v| v as f64));
    let min_media_x = media[0].min(media[2]);
    let max_media_x = media[0].max(media[2]);
    let min_media_y = media[1].min(media[3]);
    let max_media_y = media[1].max(media[3]);

    let rx0 = ux0.min(ux1).max(min_media_x).min(max_media_x);
    let rx1 = ux0.max(ux1).max(min_media_x).min(max_media_x);
    let ry0 = uy0.min(uy1).max(min_media_y).min(max_media_y);
    let ry1 = uy0.max(uy1).max(min_media_y).min(max_media_y);

    if rx1 - rx0 < 1.0 || ry1 - ry0 < 1.0 {
        return Ok(None);
    }

    Ok(Some([rx0, ry0, rx1, ry1]))
}

/// Calculates the margins `[left, bottom, right, top]` from the page's MediaBox to its visible non-white content.
///
/// Margins are measured in PDF points inward from the MediaBox, matching the format used by
/// [`pdfcraft_organize::BoxSpec::Margins`] and the Set Page Boxes dialog.
/// Returns `Ok(Some([left, bottom, right, top]))` if content was found, or `Ok(None)` if the page has no content.
pub fn page_content_margins(doc: &Document, page: usize) -> Result<Option<[f64; 4]>, EditError> {
    let Some(rect) = detect_page_content_box(doc, page)? else {
        return Ok(None);
    };

    let Some(info) = doc.info.pages.get(page) else {
        return Ok(None);
    };

    let media = doc.page_boxes().get(page).map(|boxes| boxes[0]).unwrap_or_else(|| info.crop.map(|v| v as f64));
    let min_media_x = media[0].min(media[2]);
    let max_media_x = media[0].max(media[2]);
    let min_media_y = media[1].min(media[3]);
    let max_media_y = media[1].max(media[3]);

    let left = (rect[0] - min_media_x).max(0.0);
    let bottom = (rect[1] - min_media_y).max(0.0);
    let right = (max_media_x - rect[2]).max(0.0);
    let top = (max_media_y - rect[3]).max(0.0);

    // Round to 2 decimal places like the dialog does.
    let round2 = |v: f64| (v * 100.0).round() / 100.0;
    Ok(Some([round2(left), round2(bottom), round2(right), round2(top)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;

    /// Verifies detection of non-white content and margin computation on a text-bearing document.
    #[test]
    fn white_margins_detection_and_margins_on_text_page() {
        let s = Session::new();
        let bytes = s.create_from_text("Title", "Hello world from PdfCraft white margins test").unwrap();
        let mut s = Session::new();
        let id = s.open("doc.pdf", None, bytes, None).unwrap();
        let doc = s.get(id).unwrap();

        // Detect content box on page 0.
        let content_box = detect_page_content_box(doc, 0).unwrap();
        assert!(content_box.is_some(), "content box should be found on page with text");
        let [x0, y0, x1, y1] = content_box.unwrap();
        assert!(x1 > x0, "content width should be positive: {x0}..{x1}");
        assert!(y1 > y0, "content height should be positive: {y0}..{y1}");

        // Content on a Letter page with a small paragraph should occupy only part of the page.
        assert!(x0 > 10.0, "should have left white margin: {x0}");
        assert!(x1 < 600.0, "should have right white margin: {x1}");

        // Margins.
        let margins = page_content_margins(doc, 0).unwrap().unwrap();
        assert!(margins[0] > 10.0, "left margin: {}", margins[0]);
        assert!(margins[2] > 10.0, "right margin: {}", margins[2]);

        // Removing white margins crops the page.
        s.remove_white_margins(id, &[0]).unwrap();
        let updated = s.get(id).unwrap();
        let new_crop = updated.info.pages[0].crop;
        assert!(new_crop[2] - new_crop[0] < 612.0, "crop width should be smaller than 612: {:?}", new_crop);
    }

    /// Verifies that an empty blank page yields no content box or margins, and removal is safe.
    #[test]
    fn white_margins_empty_page_returns_none() {
        let s = Session::new();
        let bytes = s.create_blank(612.0, 792.0, 1).unwrap();
        let mut s = Session::new();
        let id = s.open("blank.pdf", None, bytes, None).unwrap();
        let doc = s.get(id).unwrap();

        let content_box = detect_page_content_box(doc, 0).unwrap();
        assert!(content_box.is_none(), "completely blank page should have no content box");

        let margins = page_content_margins(doc, 0).unwrap();
        assert!(margins.is_none(), "blank page should return None for margins");

        // Removing margins on blank page changes nothing and does not fail.
        s.remove_white_margins(id, &[0]).unwrap();
        assert_eq!(s.get(id).unwrap().info.pages[0].crop, [0.0, 0.0, 612.0, 792.0]);
    }

    /// Verifies content detection and margin removal on a page rotated by 90 degrees.
    #[test]
    fn white_margins_rotated_page_content_detection() {
        use crate::Edit;
        let s = Session::new();
        let bytes = s.create_from_text("Title", "Rotated page content text").unwrap();
        let mut s = Session::new();
        let id = s.open("rotated.pdf", None, bytes, None).unwrap();

        // Rotate page by 90 degrees.
        s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        let doc = s.get(id).unwrap();
        assert_eq!(doc.info.pages[0].rotation, 90);

        let content_box = detect_page_content_box(doc, 0).unwrap();
        assert!(content_box.is_some(), "content box should be found on rotated page");

        s.remove_white_margins(id, &[0]).unwrap();
        let updated = s.get(id).unwrap();
        let new_crop = updated.info.pages[0].crop;
        assert!(new_crop[2] - new_crop[0] > 0.0, "rotated crop should have positive dimensions");
    }
}
