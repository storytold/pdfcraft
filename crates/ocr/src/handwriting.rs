//! Single-column line segmentation for upright historical handwriting.
//!
//! A print detector often splits connected cursive strokes or misses them entirely.
//! This profile instead estimates the baseline spacing from the horizontal ink
//! projection. It is deliberately separate from the multi-column print profile.
use crate::OcrError;
use image::{RgbImage, imageops};

const MAX_SIDE: u32 = 1800;

/// Locate horizontal lines without modifying the source image. Analysis memory
/// and work are bounded independently of the page's render resolution.
pub(crate) fn lines(image: &RgbImage) -> Result<Vec<[f32; 4]>, OcrError> {
    let (source_w, source_h) = image.dimensions();
    if source_w < 8 || source_h < 8 {
        return Ok(Vec::new());
    }
    // A supplied line crop already contains the context required by the model.
    if source_h as f64 / (source_w as f64) < 0.2 {
        return Ok(vec![[0.0, 0.0, source_w as f32, source_h as f32]]);
    }
    let scale = (MAX_SIDE as f64 / source_w.max(source_h) as f64).min(1.0);
    let small = imageops::resize(
        image,
        (source_w as f64 * scale).round().max(1.0) as u32,
        (source_h as f64 * scale).round().max(1.0) as u32,
        imageops::FilterType::Triangle,
    );
    let (w, h) = (small.width() as usize, small.height() as usize);
    let grey: Vec<u32> = small.pixels().map(|p| (299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2])) / 1000).collect();
    // Suppress dark photographic frames connected to the edge, including their
    // curves and lettering. A fixed crop would also discard legitimate margins.
    let mut frame = vec![false; w * h];
    let mut visited = vec![false; w * h];
    let mut stack = Vec::new();
    let mut component = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let seed = y * w + x;
            let at_edge = x < w / 50 || x >= w - w / 50 || y < h / 50 || y >= h - h / 50;
            if !at_edge || visited[seed] || grey[seed] >= 180 {
                continue;
            }
            visited[seed] = true;
            stack.push(seed as u32);
            component.clear();
            let (mut left, mut top, mut right, mut bottom) = (x, y, x, y);
            while let Some(index) = stack.pop() {
                component.push(index);
                let (cx, cy) = (index as usize % w, index as usize / w);
                (left, top, right, bottom) = (left.min(cx), top.min(cy), right.max(cx), bottom.max(cy));
                for ny in cy.saturating_sub(1)..(cy + 2).min(h) {
                    for nx in cx.saturating_sub(1)..(cx + 2).min(w) {
                        let next = ny * w + nx;
                        if !visited[next] && grey[next] < 180 {
                            visited[next] = true;
                            stack.push(next as u32);
                        }
                    }
                }
            }
            let (cw, ch) = (right - left + 1, bottom - top + 1);
            let long_rule = (ch > h / 2 && cw < w * 8 / 100) || (cw > w / 2 && ch < h * 8 / 100);
            let broad_frame = component.len() > w * h * 15 / 1000 && (ch > h / 2 || cw > w / 2);
            // An ordinary connected word touching the edge is not a scanner frame.
            if long_rule || broad_frame {
                for &index in &component {
                    frame[index as usize] = true;
                }
            }
        }
    }
    let mut integral = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0;
        for x in 0..w {
            row += grey[y * w + x];
            integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row;
        }
    }
    let radius = (w / 70).max(8);
    let mut ink = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let (x0, y0) = (x.saturating_sub(radius), y.saturating_sub(radius));
            let (x1, y1) = ((x + radius + 1).min(w), (y + radius + 1).min(h));
            let sum = integral[y1 * (w + 1) + x1] + integral[y0 * (w + 1) + x0] - integral[y0 * (w + 1) + x1] - integral[y1 * (w + 1) + x0];
            let mean = sum / ((x1 - x0) * (y1 - y0)) as u32;
            let value = grey[y * w + x];
            ink[y * w + x] = !frame[y * w + x] && value < 180 && value + 22 < mean;
        }
    }
    // Rules and scanner borders have much longer continuous runs than letters.
    // Remove them from analysis only; the original page and OCR crop stay intact.
    for x in 0..w {
        if (0..h).filter(|&y| ink[y * w + x]).count() > h * 35 / 100 {
            for y in 0..h {
                for nx in x.saturating_sub(2)..(x + 3).min(w) {
                    ink[y * w + nx] = false;
                }
            }
        }
    }
    for y in 0..h {
        if ink[y * w..(y + 1) * w].iter().filter(|&&v| v).count() > w / 2 {
            for ny in y.saturating_sub(2)..(y + 3).min(h) {
                ink[ny * w..(ny + 1) * w].fill(false);
            }
        }
    }
    let rows: Vec<f64> = ink.chunks_exact(w).map(|row| row.iter().filter(|&&v| v).count() as f64).collect();
    let count = rows.iter().sum::<f64>();
    if count < 30.0 {
        return Ok(Vec::new());
    }
    if count / (w * h) as f64 > 0.3 {
        return Err(layout_error());
    }
    let smooth: Vec<f64> = (0..h).map(|y| rows[y.saturating_sub(2)..(y + 3).min(h)].iter().sum::<f64>() / 5.0).collect();
    let mean = smooth.iter().sum::<f64>() / h as f64;
    let lo = (w / 100).max(8).min(h.saturating_sub(3));
    let hi = (w * 12 / 100).max(lo + 2).min(h - 1);
    let corr: Vec<f64> = (0..=hi).map(|lag| (lag..h).map(|y| (smooth[y] - mean) * (smooth[y - lag] - mean)).sum()).collect();
    let period = (lo..hi).filter(|&i| corr[i] > corr[i - 1] && corr[i] >= corr[i + 1]).max_by(|&a, &b| corr[a].total_cmp(&corr[b]));
    let Some(period) = period.filter(|&p| corr[p] > corr[0] * 0.1) else {
        return Err(layout_error());
    };
    let threshold = (smooth.iter().copied().fold(0.0, f64::max) * 0.04).max(4.0);
    let mut candidates: Vec<usize> = (2..h - 2).filter(|&y| smooth[y] > threshold && smooth[y - 2..=y + 2].iter().all(|&v| smooth[y] >= v)).collect();
    candidates.sort_by(|&a, &b| smooth[b].total_cmp(&smooth[a]));
    let mut peaks = Vec::new();
    for y in candidates {
        if peaks.iter().all(|&other: &usize| y.abs_diff(other) as f64 > period as f64 * 0.65) {
            peaks.push(y);
        }
    }
    peaks.sort_unstable();
    if peaks.len() < 2 || peaks.len() > 150 {
        return Err(layout_error());
    }
    let mut regions = Vec::new();
    for (i, &peak) in peaks.iter().enumerate() {
        let pad = (period as f64 * 0.12).round() as usize;
        let reach = (period as f64 * 0.65).round() as usize;
        let top = if i > 0 { ((peaks[i - 1] + peak) / 2).saturating_sub(pad) } else { peak.saturating_sub(reach) };
        let bottom = peaks.get(i + 1).map_or((peak + reach).min(h), |&next| ((peak + next) / 2 + pad).min(h));
        let (top, bottom) = (top.max(peak.saturating_sub(reach)), bottom.min(peak + reach));
        let columns: Vec<usize> = (0..w).filter(|&x| (top..bottom).filter(|&y| ink[y * w + x]).count() > 1).collect();
        let (Some(&left), Some(&right)) = (columns.first(), columns.last()) else { continue };
        let left = left.saturating_sub(6);
        let right = (right + 7).min(w);
        if right - left < 5 || bottom - top < 5 {
            continue;
        }
        regions.push([
            left as f32 * source_w as f32 / w as f32,
            top as f32 * source_h as f32 / h as f32,
            right as f32 * source_w as f32 / w as f32,
            bottom as f32 * source_h as f32 / h as f32,
        ]);
    }
    Ok(regions)
}

fn layout_error() -> OcrError {
    OcrError::Recognize("handwriting needs upright horizontal lines in one text column; no reliable line spacing was found".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn blank_pages_and_line_crops_are_bounded() {
        assert!(lines(&RgbImage::from_pixel(400, 600, Rgb([255; 3]))).unwrap().is_empty());
        assert!(lines(&RgbImage::new(0, 0)).unwrap().is_empty());
        assert_eq!(lines(&RgbImage::from_pixel(400, 50, Rgb([255; 3]))).unwrap(), [[0.0, 0.0, 400.0, 50.0]]);
    }

    #[test]
    fn extreme_dimensions_and_solid_frames_never_produce_invalid_regions() {
        for (width, height) in [(8, 8), (8, 9000), (9000, 8), (1, 8), (8, 1), (0, 100), (500, 800)] {
            for shade in [0, 128, 255] {
                let image = RgbImage::from_pixel(width, height, Rgb([shade; 3]));
                if let Ok(regions) = lines(&image) {
                    for rect in regions {
                        assert!(rect.iter().all(|value| value.is_finite()));
                        assert!(rect[0] >= 0.0 && rect[1] >= 0.0);
                        assert!(rect[2] <= width as f32 && rect[3] <= height as f32);
                        assert!(rect[2] > rect[0] && rect[3] > rect[1]);
                    }
                }
            }
        }
    }

    #[test]
    fn single_column_rows_survive_tinted_paper_and_a_vertical_rule() {
        let mut page = RgbImage::from_pixel(600, 850, Rgb([230, 218, 180]));
        // A curved scanner frame must not widen the text crops or become a row.
        for y in 0..850 {
            for x in 0..20 + y % 11 {
                page.put_pixel(x, y, Rgb([45; 3]));
            }
        }
        for y in 810..850 {
            for x in 0..600 {
                page.put_pixel(x, y, Rgb([45; 3]));
            }
        }
        for y in 20..830 {
            page.put_pixel(45, y, Rgb([70, 60, 45]));
        }
        for row in 0..12 {
            for letter in 0..32 {
                let left = 90 + letter * 13;
                let top = 100 + row * 48;
                for y in top..top + 16 {
                    for x in left..left + 5 {
                        page.put_pixel(x, y, Rgb([70, 60, 45]));
                    }
                }
            }
        }
        let regions = lines(&page).unwrap();
        assert_eq!(regions.len(), 12);
        for (i, rect) in regions.iter().enumerate() {
            assert!(rect[0] > 45.0 && rect[2] <= 600.0 && rect[3] < 810.0);
            assert!(rect[1] <= (100 + i * 48) as f32 && rect[3] >= (116 + i * 48) as f32);
        }
    }

    #[test]
    fn connected_words_touching_the_page_edge_remain_in_the_line_regions() {
        let mut page = RgbImage::from_pixel(600, 850, Rgb([240; 3]));
        for row in 0..12 {
            let top = 100 + row * 48;
            for letter in 0..32 {
                let left = letter * 13;
                for y in top..top + 16 {
                    for x in left..left + 5 {
                        page.put_pixel(x, y, Rgb([70; 3]));
                    }
                }
            }
            // Five touching letters simulate a cursive word at x=0, well within
            // the old two-percent exclusion band. It must not be flood-erased.
            for x in 0..60 {
                page.put_pixel(x, top + 12, Rgb([70; 3]));
            }
        }
        let regions = lines(&page).unwrap();
        assert_eq!(regions.len(), 12);
        assert!(regions.iter().all(|rect| rect[0] == 0.0));
    }
}
