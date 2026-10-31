//! Bulk crop content detection and margin calculation algorithms (M4.4).
//!
//! Scans rendered page rasters for content bounds and calculates safe crop margins (in PDF points)
//! with user-defined padding. Mode-aware union operations allow single or multi-page boundary fitting.

use std::sync::Arc;

use pdfcraft_render::{PageInfo, PageRenderer, RenderConfig, RenderRequest};

use crate::{DocId, Session};

/// The maximum dimension in pixels used for content detection rasters.
const DETECT_RASTER_SIDE: f32 = 800.0;
/// Default non-white luminance threshold (0..=255).
const CONTENT_THRESHOLD: u8 = 245;
/// Minimum crop box dimension in points to prevent degenerate pages.
const MIN_CROP_DIM: f64 = 18.0;

/// Scans an RGBA raster for content pixels (non-white, chromatic, and sufficiently opaque).
///
/// Uses directional inward scans from top/bottom and boundary-narrowed horizontal sweeps
/// to minimize pixel inspections and enable early exits without heap allocation.
///
/// Returns `[min_x, min_y, max_x, max_y]` in pixel coordinates (0-indexed, y down),
/// or `None` if the entire image is blank / white.
pub fn detect_content_bounds_rgba(rgba: &[u8], width: usize, height: usize, threshold: u8) -> Option<[usize; 4]> {
    let row_stride = width.checked_mul(4)?;
    let expected_len = row_stride.checked_mul(height)?;
    if width == 0 || height == 0 || rgba.len() < expected_len {
        return None;
    }

    #[inline(always)]
    fn is_content(px: &[u8], threshold: u8) -> bool {
        // `chunks_exact(4)` guarantees `px.len() == 4`
        let alpha = px[3];
        if alpha <= 32 {
            return false;
        }

        let r = px[0] as u32;
        let g = px[1] as u32;
        let b = px[2] as u32;

        // Perceived luminance (ITU-R BT.601)
        let luminance = (299 * r + 587 * g + 114 * b) / 1000;
        // Also detect saturated colors (colored text, stamps, highlighters)
        let max_c = r.max(g).max(b);
        let min_c = r.min(g).min(b);
        let is_chromatic = (max_c - min_c) > 30;

        luminance < threshold as u32 || is_chromatic
    }

    let buf = &rgba[..expected_len];

    // 1. Scan top-down for min_y
    let min_y = buf.chunks_exact(row_stride).position(|row| row.as_chunks::<4>().0.iter().any(|px| is_content(px, threshold)))?;

    // 2. Scan bottom-up for max_y (only inspecting rows >= min_y)
    let active_from_top = &buf[min_y * row_stride..];
    let max_y_offset =
        active_from_top.chunks_exact(row_stride).rposition(|row| row.as_chunks::<4>().0.iter().any(|px| is_content(px, threshold))).unwrap_or(0);
    let max_y = min_y + max_y_offset;

    // 3. Scan horizontally only within active vertical band [min_y..=max_y]
    let active_band = &buf[min_y * row_stride..(max_y + 1) * row_stride];
    let mut min_x = width;
    let mut max_x = 0usize;

    for row in active_band.chunks_exact(row_stride) {
        if min_x == 0 && max_x == width - 1 {
            break; // Full horizontal span already reached
        }

        let pixels = row.as_chunks::<4>().0;

        // Scan left edge up to current min_x
        for (x, px) in pixels.iter().take(min_x).enumerate() {
            if is_content(px, threshold) {
                min_x = x;
                break;
            }
        }

        // Scan right edge down to current max_x
        for (x, px) in pixels.iter().enumerate().skip(max_x).rev() {
            if is_content(px, threshold) {
                max_x = x;
                break;
            }
        }
    }

    (min_x <= max_x).then_some([min_x, min_y, max_x, max_y])
}

/// Converts pixel bounding coordinates in raster view space into unrotated MediaBox
/// user-space margins `[left, bottom, right, top]` for `BoxSpec::Margins`.
///
/// Uses `PageInfo::view_to_user` to transform the raster bounding box corners into
/// PDF user space, correctly handling `/Rotate` and pages whose existing CropBox
/// is smaller than their MediaBox. Insets by `padding_pt` and clamps to avoid degenerate crops.
pub fn content_bounds_to_user_margins(
    bounds: [usize; 4],
    raster_w: usize,
    raster_h: usize,
    info: &PageInfo,
    media_box: [f64; 4],
    padding_pt: f64,
) -> Option<[f64; 4]> {
    let [min_x, min_y, max_x, max_y] = bounds;
    if raster_w == 0
        || raster_h == 0
        || !info.width.is_finite()
        || info.width <= 0.0
        || !info.height.is_finite()
        || info.height <= 0.0
        || !padding_pt.is_finite()
        || min_x > max_x
        || max_x >= raster_w
        || min_y > max_y
        || max_y >= raster_h
    {
        return None;
    }

    let rw = raster_w as f32;
    let rh = raster_h as f32;

    // Pixel box in view space points [0..width, 0..height] (y-down)
    let vx0 = (min_x as f32 / rw) * info.width;
    let vy0 = (min_y as f32 / rh) * info.height;
    let vx1 = ((max_x + 1) as f32 / rw) * info.width;
    let vy1 = ((max_y + 1) as f32 / rh) * info.height;

    // Map all four view-space corners to PDF user space
    let c1 = info.view_to_user(vx0, vy0);
    let c2 = info.view_to_user(vx1, vy0);
    let c3 = info.view_to_user(vx0, vy1);
    let c4 = info.view_to_user(vx1, vy1);

    let u_min_x = (c1[0].min(c2[0]).min(c3[0]).min(c4[0])) as f64;
    let u_max_x = (c1[0].max(c2[0]).max(c3[0]).max(c4[0])) as f64;
    let u_min_y = (c1[1].min(c2[1]).min(c3[1]).min(c4[1])) as f64;
    let u_max_y = (c1[1].max(c2[1]).max(c3[1]).max(c4[1])) as f64;

    if !u_min_x.is_finite() || !u_max_x.is_finite() || !u_min_y.is_finite() || !u_max_y.is_finite() {
        return None;
    }

    let mx0 = media_box[0].min(media_box[2]);
    let mx1 = media_box[0].max(media_box[2]);
    let my0 = media_box[1].min(media_box[3]);
    let my1 = media_box[1].max(media_box[3]);

    let media_w = mx1 - mx0;
    let media_h = my1 - my0;
    if media_w <= 0.0 || media_h <= 0.0 {
        return None;
    }

    let pad = padding_pt.max(0.0);

    // Margins measured from MediaBox edges in unrotated user space
    let mut left = ((u_min_x - mx0) - pad).clamp(0.0, media_w);
    let mut bottom = ((u_min_y - my0) - pad).clamp(0.0, media_h);
    let mut right = ((mx1 - u_max_x) - pad).clamp(0.0, media_w);
    let mut top = ((my1 - u_max_y) - pad).clamp(0.0, media_h);

    // Guard against degenerate / overlapping crops
    let max_horiz = (media_w - MIN_CROP_DIM).max(0.0);
    if left + right > max_horiz && left + right > 0.0 {
        let k = max_horiz / (left + right);
        left *= k;
        right *= k;
    }

    let max_vert = (media_h - MIN_CROP_DIM).max(0.0);
    if bottom + top > max_vert && bottom + top > 0.0 {
        let k = max_vert / (bottom + top);
        bottom *= k;
        top *= k;
    }

    #[inline]
    fn floor_2dp(v: f64) -> f64 {
        ((v + 1e-9) * 100.0).floor() / 100.0
    }

    Some([floor_2dp(left), floor_2dp(bottom), floor_2dp(right), floor_2dp(top)])
}

/// Converts pixel bounding coordinates to PDF point margins `[left, bottom, right, top]`
/// measured from the page boundaries, inset by `padding_pt`.
///
/// Validates input dimensions and bounds, clamping margins so total horizontal and
/// vertical insets never exceed `page_dim - MIN_CROP_DIM`.
pub fn content_bounds_to_margins(
    bounds: [usize; 4],
    raster_w: usize,
    raster_h: usize,
    page_w: f64,
    page_h: f64,
    padding_pt: f64,
) -> Option<[f64; 4]> {
    let [min_x, min_y, max_x, max_y] = bounds;
    if raster_w == 0
        || raster_h == 0
        || !page_w.is_finite()
        || page_w <= 0.0
        || !page_h.is_finite()
        || page_h <= 0.0
        || !padding_pt.is_finite()
        || min_x > max_x
        || max_x >= raster_w
        || min_y > max_y
        || max_y >= raster_h
    {
        return None;
    }

    let pad = padding_pt.max(0.0);
    let rw = raster_w as f64;
    let rh = raster_h as f64;

    let left_ratio = min_x as f64 / rw;
    let right_ratio = (raster_w - 1 - max_x) as f64 / rw;
    let top_ratio = min_y as f64 / rh;
    let bottom_ratio = (raster_h - 1 - max_y) as f64 / rh;

    let mut left = (left_ratio * page_w - pad).clamp(0.0, page_w);
    let mut bottom = (bottom_ratio * page_h - pad).clamp(0.0, page_h);
    let mut right = (right_ratio * page_w - pad).clamp(0.0, page_w);
    let mut top = (top_ratio * page_h - pad).clamp(0.0, page_h);

    // Guard against degenerate / overlapping crops
    let max_horiz = (page_w - MIN_CROP_DIM).max(0.0);
    if left + right > max_horiz && left + right > 0.0 {
        let k = max_horiz / (left + right);
        left *= k;
        right *= k;
    }

    let max_vert = (page_h - MIN_CROP_DIM).max(0.0);
    if bottom + top > max_vert && bottom + top > 0.0 {
        let k = max_vert / (bottom + top);
        bottom *= k;
        top *= k;
    }

    #[inline]
    fn floor_2dp(v: f64) -> f64 {
        ((v + 1e-9) * 100.0).floor() / 100.0
    }

    // Floor to 2 decimal places to prevent rounding into content
    Some([floor_2dp(left), floor_2dp(bottom), floor_2dp(right), floor_2dp(top)])
}

/// Calculates the safe union envelope of multiple page margins `[left, bottom, right, top]`.
///
/// To safely encompass content across all pages without clipping, the union margin
/// takes the minimum margin from each edge.
pub fn union_margins(margins_list: &[[f64; 4]]) -> Option<[f64; 4]> {
    let first = *margins_list.first()?;
    Some(margins_list.iter().skip(1).fold(first, |acc, m| [acc[0].min(m[0]), acc[1].min(m[1]), acc[2].min(m[2]), acc[3].min(m[3])]))
}

enum PageScan {
    Content([f64; 4]),
    Blank,
    Error,
}

fn detect_page_with_renderer(renderer: &mut PageRenderer, info: &PageInfo, page: usize, media_box: [f64; 4], padding_pt: f64) -> PageScan {
    let max_dim = info.width.max(info.height).max(1.0);
    let scale = (DETECT_RASTER_SIDE / max_dim).clamp(0.05, 5.0);

    let shot = renderer.render(RenderRequest { page, scale, ..Default::default() });
    if shot.error.is_some() {
        return PageScan::Error;
    }

    match detect_content_bounds_rgba(&shot.rgba, shot.width as usize, shot.height as usize, CONTENT_THRESHOLD) {
        Some(bounds) => match content_bounds_to_user_margins(bounds, shot.width as usize, shot.height as usize, info, media_box, padding_pt) {
            Some(m) => PageScan::Content(m),
            None => PageScan::Error,
        },
        None => PageScan::Blank,
    }
}

/// Detects content margins on a page using the given renderer and page info.
pub fn detect_page_margins(renderer: &mut PageRenderer, info: &PageInfo, page: usize, media_box: [f64; 4], padding_pt: f64) -> Option<[f64; 4]> {
    match detect_page_with_renderer(renderer, info, page, media_box, padding_pt) {
        PageScan::Content(m) => Some(m),
        _ => None,
    }
}

impl Session {
    /// Detects content bounds on page `page` (0-based) of document `id` and returns
    /// margins `[left, bottom, right, top]` in PDF points from MediaBox, including `padding_pt`.
    pub fn detect_page_auto_crop(&self, id: DocId, page: usize, padding_pt: f64) -> Option<[f64; 4]> {
        let doc = self.get(id)?;
        let info = doc.info.pages.get(page)?;
        let boxes = doc.page_boxes();
        let media =
            boxes.get(page).map(|b| b[0]).unwrap_or_else(|| [info.crop[0] as f64, info.crop[1] as f64, info.crop[2] as f64, info.crop[3] as f64]);
        let config = RenderConfig { password: doc.password.as_deref().map(Arc::from), ..Default::default() };
        let mut renderer = PageRenderer::new(doc.bytes.clone(), config);
        detect_page_margins(&mut renderer, info, page, media, padding_pt)
    }

    /// Detects content bounds on each page in `pages` (0-based) of document `id`,
    /// returning a list of `(page, margins)` for pages containing non-white content.
    /// Fails safely (returns `None`) if any page encounters a render error.
    pub fn detect_pages_auto_crop(&self, id: DocId, pages: &[usize], padding_pt: f64) -> Option<Vec<(usize, [f64; 4])>> {
        let doc = self.get(id)?;
        let config = RenderConfig { password: doc.password.as_deref().map(Arc::from), ..Default::default() };
        let mut renderer = PageRenderer::new(doc.bytes.clone(), config);
        let boxes = doc.page_boxes();

        let mut list = Vec::new();
        for &p in pages {
            let info = doc.info.pages.get(p)?;
            let media =
                boxes.get(p).map(|b| b[0]).unwrap_or_else(|| [info.crop[0] as f64, info.crop[1] as f64, info.crop[2] as f64, info.crop[3] as f64]);
            match detect_page_with_renderer(&mut renderer, info, p, media, padding_pt) {
                PageScan::Content(m) => list.push((p, m)),
                PageScan::Blank => {}
                PageScan::Error => return None,
            }
        }
        Some(list)
    }

    /// Calculates the safe union auto-crop margins across `pages` (0-based) of document `id`.
    ///
    /// Reuses a single `PageRenderer` instance across all pages in a single pass. If any page fails to render,
    /// returns `None` rather than producing an unsafe partial crop that could clip content.
    pub fn detect_bulk_auto_crop(&self, id: DocId, pages: &[usize], padding_pt: f64) -> Option<[f64; 4]> {
        let per_page = self.detect_pages_auto_crop(id, pages, padding_pt)?;
        let margins: Vec<[f64; 4]> = per_page.into_iter().map(|(_, m)| m).collect();
        union_margins(&margins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_raster_returns_no_bounds() {
        let white_raster = vec![255; 100 * 100 * 4];
        assert_eq!(detect_content_bounds_rgba(&white_raster, 100, 100, 245), None);
    }

    #[test]
    fn transparent_pixels_with_rgb_noise_ignored() {
        let mut raster = vec![255; 100 * 100 * 4];
        // Transparent black noise
        for px in raster.as_chunks_mut::<4>().0 {
            *px = [0, 0, 0, 0];
        }
        assert_eq!(detect_content_bounds_rgba(&raster, 100, 100, 245), None);
    }

    #[test]
    fn faint_gray_text_detected() {
        let mut raster = vec![255; 100 * 100 * 4];
        // Gray square at (20, 20) with luminance 220 < threshold 245
        for y in 20..=25 {
            for x in 20..=25 {
                let idx = (y * 100 + x) * 4;
                raster[idx] = 220;
                raster[idx + 1] = 220;
                raster[idx + 2] = 220;
                raster[idx + 3] = 255;
            }
        }
        assert_eq!(detect_content_bounds_rgba(&raster, 100, 100, 245), Some([20, 20, 25, 25]));
    }

    #[test]
    fn content_touches_edges_and_occupies_full_raster() {
        let mut raster = vec![255; 100 * 100 * 4];
        // Full width row at y = 50
        for x in 0..100 {
            let idx = (50 * 100 + x) * 4;
            raster[idx] = 0;
            raster[idx + 1] = 0;
            raster[idx + 2] = 0;
            raster[idx + 3] = 255;
        }
        assert_eq!(detect_content_bounds_rgba(&raster, 100, 100, 245), Some([0, 50, 99, 50]));
    }

    #[test]
    fn high_luminance_yellow_highlighter_detected() {
        let mut raster = vec![255; 100 * 100 * 4];
        // Pastel yellow highlighter (255, 255, 190) -> luminance is ~247 (>= 245),
        // but chroma (max_c - min_c = 65 > 30) triggers detection.
        let idx = (40 * 100 + 40) * 4;
        raster[idx..idx + 4].copy_from_slice(&[255, 255, 190, 255]);

        assert_eq!(detect_content_bounds_rgba(&raster, 100, 100, CONTENT_THRESHOLD), Some([40, 40, 40, 40]));
    }

    #[test]
    fn content_bounds_detects_centered_box() {
        let mut raster = vec![255; 100 * 100 * 4];
        // Draw black pixel rectangle from (20, 30) to (79, 69)
        for y in 30..=69 {
            for x in 20..=79 {
                let idx = (y * 100 + x) * 4;
                raster[idx] = 0;
                raster[idx + 1] = 0;
                raster[idx + 2] = 0;
                raster[idx + 3] = 255;
            }
        }

        let bounds = detect_content_bounds_rgba(&raster, 100, 100, 245);
        assert_eq!(bounds, Some([20, 30, 79, 69]));

        let margins = content_bounds_to_margins(bounds.unwrap(), 100, 100, 1000.0, 1000.0, 10.0).unwrap();
        // left_ratio: 20/100 -> 200 - 10 = 190
        // right_ratio: (100 - 1 - 79) / 100 = 20/100 -> 200 - 10 = 190
        // top_ratio: 30/100 -> 300 - 10 = 290
        // bottom_ratio: (100 - 1 - 69) / 100 = 30/100 -> 300 - 10 = 290
        assert_eq!(margins, [190.0, 290.0, 190.0, 290.0]);
    }

    #[test]
    fn invalid_bounds_rejected() {
        assert_eq!(content_bounds_to_margins([50, 50, 40, 40], 100, 100, 500.0, 500.0, 10.0), None);
        assert_eq!(content_bounds_to_margins([0, 0, 100, 50], 100, 100, 500.0, 500.0, 10.0), None);
        assert_eq!(content_bounds_to_margins([0, 0, 50, 50], 0, 100, 500.0, 500.0, 10.0), None);
        assert_eq!(content_bounds_to_margins([0, 0, 50, 50], 100, 100, -100.0, 500.0, 10.0), None);
    }

    #[test]
    fn union_margins_takes_minimum_safe_insets() {
        let m1 = [50.0, 40.0, 60.0, 30.0];
        let m2 = [20.0, 70.0, 30.0, 80.0];
        let union = union_margins(&[m1, m2]).unwrap();
        assert_eq!(union, [20.0, 40.0, 30.0, 30.0]);
    }

    #[test]
    fn user_margins_handles_rotated_and_cropped_pages() {
        // Page with MediaBox [0, 0, 200, 300] and no rotation:
        let info_unrotated = PageInfo { width: 200.0, height: 300.0, label: "1".into(), crop: [0.0, 0.0, 200.0, 300.0], rotation: 0 };
        // Content rectangle from (20, 30) to (79, 69) on 100x100 raster with 0 padding
        let bounds = [20, 30, 79, 69];
        let media = [0.0, 0.0, 200.0, 300.0];
        let m = content_bounds_to_user_margins(bounds, 100, 100, &info_unrotated, media, 0.0).unwrap();
        // In user space:
        // left: 20% of 200 = 40.0
        // right: 20% of 200 = 40.0
        // top: 30% of 300 = 90.0
        // bottom: 30% of 300 = 90.0
        assert_eq!(m, [40.0, 90.0, 40.0, 90.0]);

        // Rotated 90 degrees clockwise:
        // In PDF user space MediaBox is still [0, 0, 200, 300], but displayed width is 300, height is 200
        let info_rot90 = PageInfo { width: 300.0, height: 200.0, label: "1".into(), crop: [0.0, 0.0, 200.0, 300.0], rotation: 90 };
        // In view space on rot90: x corresponds to user space y, y corresponds to user space (200 - x)
        let m_rot = content_bounds_to_user_margins(bounds, 100, 100, &info_rot90, media, 0.0).unwrap();
        // Bounding box transformed to user space accurately produces non-degenerate margins
        assert!(m_rot.iter().all(|&v| v >= 0.0));
        assert!(m_rot[0] + m_rot[2] < 200.0);
        assert!(m_rot[1] + m_rot[3] < 300.0);

        // Pre-cropped page: CropBox [20, 30, 180, 270] smaller than MediaBox [0, 0, 200, 300]
        let info_cropped = PageInfo { width: 160.0, height: 240.0, label: "1".into(), crop: [20.0, 30.0, 180.0, 270.0], rotation: 0 };
        let m_crop = content_bounds_to_user_margins([0, 0, 99, 99], 100, 100, &info_cropped, media, 0.0).unwrap();
        // Whole cropped region touched: margins should be relative to MediaBox, i.e. [20.0, 30.0, 20.0, 30.0]
        assert_eq!(m_crop, [20.0, 30.0, 20.0, 30.0]);
    }
}
