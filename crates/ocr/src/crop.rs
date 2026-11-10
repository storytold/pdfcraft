//! Conservative paper-edge detection, not a bounding box around text or handwriting.
//! Work at bounded resolution and require a long, coherent edge against a neutral scanner
//! background. Indistinguishable white paper on white, small marks and blank scans stay put.
use image::{RgbImage, imageops};

/// Pixel-edge coordinates in the original, displayed scan: left, top, right, bottom.
pub fn paper_bounds(image: &RgbImage) -> Option<[u32; 4]> {
    let (width, height) = image.dimensions();
    if width < 64 || height < 64 {
        return None;
    }
    let ratio = (900.0 / width.max(height) as f64).min(1.0);
    let small =
        imageops::resize(image, (width as f64 * ratio).round() as u32, (height as f64 * ratio).round() as u32, imageops::FilterType::Triangle);
    let (w, h) = small.dimensions();
    // Extremely narrow input must not make the sampling bands exceed the image.
    if w < 32 || h < 32 {
        return None;
    }
    let left = edge(&small, false, false);
    let right = w - edge(&small, false, true);
    let top = edge(&small, true, false);
    let bottom = h - edge(&small, true, true);
    // A tiny central island is not a confidently located sheet of paper.
    if right.saturating_sub(left) < w / 3 || bottom.saturating_sub(top) < h / 3 {
        return None;
    }
    if left == 0 && top == 0 && right == w && bottom == h {
        return None;
    }
    Some([
        (left as f64 * width as f64 / w as f64).floor() as u32,
        (top as f64 * height as f64 / h as f64).floor() as u32,
        (right as f64 * width as f64 / w as f64).ceil() as u32,
        (bottom as f64 * height as f64 / h as f64).ceil() as u32,
    ])
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max)
}

fn edge(image: &RgbImage, horizontal: bool, reverse: bool) -> u32 {
    let (length, cross) = if horizontal { (image.height(), image.width()) } else { (image.width(), image.height()) };
    let pixel = |d: u32, q: u32| {
        let d = if reverse { length - 1 - d } else { d };
        let (x, y) = if horizontal { (q, d) } else { (d, q) };
        image.get_pixel_checked(x, y).map_or([255.0; 3], |p| p.0.map(f64::from))
    };
    let band = (length / 100).clamp(4, 9);
    let average = |start: u32, end: u32, q: u32| {
        let mut color = [0.0; 3];
        for d in start..end {
            for (sum, value) in color.iter_mut().zip(pixel(d, q)) {
                *sum += value / (end - start) as f64;
            }
        }
        color
    };
    let mut edges = Vec::new();
    const RAYS: u32 = 96;
    for n in 0..RAYS {
        let q = ((n as f64 + 0.5) / RAYS as f64 * cross as f64) as u32;
        // Ignore the thin white/grey frame that scanners sometimes add at the outermost edge.
        let background = average(band, 2 * band, q);
        let lo = background.into_iter().fold(255.0, f64::min);
        let hi = background.into_iter().fold(0.0, f64::max);
        if hi - lo > 45.0 || !(hi < 100.0 || lo > 215.0) {
            continue;
        }
        // First substantial, persistent transition from a black/white platen. A thin ruled
        // line is not a page edge: all three bands beyond it must differ from the background.
        let change = if hi < 100.0 { 32.0 } else { 18.0 };
        let noise = if hi < 100.0 { 30.0 } else { 14.0 };
        for d in 2 * band..length * 45 / 100 {
            let before = average(d - band, d, q);
            let after = average(d, d + band, q);
            let beyond = average(d + band, d + 2 * band, q);
            let interior = average(d + 2 * band, d + 3 * band, q);
            if distance(background, before) > noise {
                break;
            }
            if distance(background, after) >= change
                && distance(background, beyond) >= change
                && distance(background, interior) >= change
                && distance(before, after) >= change * 0.75
            {
                // Locate the edge within the averaging band, retaining one sample of padding.
                let threshold = distance(background, beyond) * 0.5;
                let boundary = (d.saturating_sub(band)..d + 2 * band).find(|&v| distance(background, pixel(v, q)) >= threshold);
                if let Some(boundary) = boundary {
                    edges.push(boundary.saturating_sub(1));
                }
                break;
            }
        }
    }
    edges.sort_unstable();
    // Long support (including corners outside the paper) prevents isolated text, drawings,
    // fingers and scanner dirt from defining the crop. Small slants are allowed, keeping the
    // nearby outer edges beyond the cluster to retain slanted paper corners. An isolated
    // transition far outside the paper-edge consensus (e.g. a finger) is not a paper corner.
    let tolerance = (length / 40).max(3);
    let required = (RAYS * 3 / 5) as usize;
    let best = edges.iter().copied().map(|d| (d, edges.iter().filter(|&&e| e >= d && e <= d + tolerance).count())).max_by_key(|&(_, count)| count);
    match best {
        Some((d, count)) if count >= required && d >= band => edges.iter().copied().find(|&e| e >= d.saturating_sub(tolerance)).unwrap_or(d),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn detects_black_white_and_colored_paper_without_trimming_writing_margins() {
        for (background, paper) in [([25; 3], [245; 3]), ([255; 3], [230; 3]), ([25; 3], [145, 70, 170])] {
            let image = RgbImage::from_fn(800, 600, |x, y| if (180..800).contains(&x) && y < 490 { Rgb(paper) } else { Rgb(background) });
            let [l, t, r, b] = paper_bounds(&image).unwrap();
            assert!((170..=180).contains(&l), "{l}");
            assert_eq!((t, r), (0, 800));
            assert!((490..=500).contains(&b), "{b}");
        }
    }

    #[test]
    fn leaves_blank_pages_rules_and_ambiguous_edges_alone() {
        assert_eq!(paper_bounds(&RgbImage::new(64, 100_000)), None);
        for color in [0, 240, 255] {
            assert_eq!(paper_bounds(&RgbImage::from_pixel(800, 600, Rgb([color; 3]))), None);
        }
        let ruled =
            RgbImage::from_fn(800, 600, |x, y| if (80..720).contains(&x) && y > 70 && y % 35 < 2 { Rgb([90, 170, 90]) } else { Rgb([255; 3]) });
        assert_eq!(paper_bounds(&ruled), None);
        let writing = RgbImage::from_fn(800, 600, |x, y| {
            if (100..680).contains(&x) && (90..450).contains(&y) && y % 30 < 9 && x % 20 < 13 { Rgb([20, 40, 150]) } else { Rgb([255; 3]) }
        });
        assert_eq!(paper_bounds(&writing), None);
    }

    #[test]
    fn scanner_frame_partial_occlusion_and_slanted_edges_keep_paper_corners() {
        let image = RgbImage::from_fn(800, 600, |x, y| {
            if x == 0 {
                Rgb([255; 3])
            } else if x >= 180 + y / 18 && y < 490 {
                Rgb([235; 3])
            } else if (620..700).contains(&x) && (520..555).contains(&y) {
                Rgb([180; 3])
            } else if x > 500 && y >= 490 {
                Rgb([90, 55, 45])
            } else {
                Rgb([25; 3])
            }
        });
        let [l, t, r, b] = paper_bounds(&image).unwrap();
        assert!((170..=180).contains(&l), "{l}");
        assert_eq!((t, r), (0, 800));
        assert!((490..=500).contains(&b), "{b}");
    }
}
