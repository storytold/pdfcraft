//! Scan & OCR ▸ Recognize text ▸ page orientation and skew, from the pixels of a scan.
//!
//! - [`skew`] finds the small turn that makes the text lines level (deskew) with a projection
//!   profile: ink pixels counted per line along each candidate angle peak sharply when the angle
//!   follows the text lines. It also tells whether the lines run across the page or down it (a
//!   page scanned sideways).
//! - [`rotate`] and [`quarter_turn`] turn an RGBA image; [`unturn`] maps a point in a quarter-turned
//!   image back to the original one.
//! - [`plausibility`] scores recognised text: upright text reads as words, text read upside down
//!   or sideways reads as a jumble of letters, digits and marks. Recognize text tries the other
//!   quarter turns on a page whose text does not read as words, and keeps the one that does.
//!
//! Angles are in degrees, counter-clockwise as seen on screen.

use crate::Line;

/// The longest side the skew search samples the image at, in pixels.
const SAMPLE_SIDE: u32 = 1200;
/// Ink is darker than this (0–255 luminance).
const INK: u32 = 128;
/// At most this many ink pixels are projected for each angle.
const MAX_POINTS: usize = 200_000;
/// Fewer ink pixels than this is not text to measure.
const MIN_POINTS: usize = 200;
/// The search range and steps (degrees): coarse, then fine around the best coarse angle.
const RANGE: f32 = 15.0;
const COARSE: f32 = 0.5;
const FINE: f32 = 0.05;
/// The straightened profile must be at least this much sharper than the unturned one, or the
/// "skew" is noise in a picture or a page without lines of text.
const MIN_GAIN: f64 = 1.05;

/// What [`skew`] found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Skew {
    /// The turn (counter-clockwise on screen) that makes the lines level or plumb; 0 for none.
    pub degrees: f32,
    /// The lines run down the page (a page scanned on its side), not across it.
    pub vertical: bool,
}

fn luminance(p: &[u8]) -> u32 {
    match p {
        [r, g, b, ..] => (u32::from(*r) * 299 + u32::from(*g) * 587 + u32::from(*b) * 114) / 1000,
        [l, ..] => u32::from(*l),
        [] => 255,
    }
}

/// How sharply `points` projected onto the normal of direction `degrees` peak: the profile's
/// mean square over its mean, squared (1 for an even spread). `vertical` projects onto the
/// lines' other axis (lines running down the image).
fn sharpness(points: &[[f32; 2]], degrees: f32, vertical: bool) -> f64 {
    let (s, c) = degrees.to_radians().sin_cos();
    // Lines turned clockwise by `degrees` on screen (y down): along a row, y - x·tan stays the
    // same; along a column, x + y·tan does.
    let project = |[x, y]: [f32; 2]| if vertical { x * c + y * s } else { y * c - x * s };
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for p in points {
        let v = project(*p);
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return 0.0;
    }
    // ponytail: one bin per sampled pixel; the sampled image is at most SAMPLE_SIDE square, so
    // the profile stays a few thousand bins.
    let bins = ((hi - lo) as usize).saturating_add(1).min(4 * SAMPLE_SIDE as usize);
    let mut profile = vec![0u32; bins];
    for p in points {
        let i = ((project(*p) - lo) as usize).min(bins - 1);
        if let Some(b) = profile.get_mut(i) {
            *b = b.saturating_add(1);
        }
    }
    let n = points.len() as f64;
    let squares: f64 = profile.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
    squares * bins as f64 / (n * n)
}

/// The skew of the text in an RGBA (or grey) image, `width` × `height` pixels, or `None` when
/// there is too little ink to tell. `degrees` is 0 when the lines are level already (within the
/// search's fine step) or there are no lines to level.
pub fn skew(pixels: &[u8], width: u32, height: u32) -> Option<Skew> {
    let area = (width as usize).checked_mul(height as usize)?;
    if area == 0 {
        return None;
    }
    let channels = pixels.len() / area;
    if channels == 0 {
        return None;
    }
    let step = (width.max(height) / SAMPLE_SIDE).max(1);
    let mut points = Vec::new();
    for y in (0..height).step_by(step as usize) {
        for x in (0..width).step_by(step as usize) {
            let at = (y as usize * width as usize + x as usize) * channels;
            if luminance(pixels.get(at..at + channels).unwrap_or(&[])) < INK {
                points.push([(x / step) as f32, (y / step) as f32]);
            }
        }
    }
    if points.len() < MIN_POINTS {
        return None;
    }
    if points.len() > MAX_POINTS {
        let keep = points.len().div_ceil(MAX_POINTS);
        points = points.into_iter().step_by(keep).collect();
    }
    let best = |vertical: bool, around: f32, range: f32, by: f32| {
        let steps = (range / by).round() as i32;
        (-steps..=steps)
            .map(|i| around + i as f32 * by)
            .map(|a| (a, sharpness(&points, a, vertical)))
            .fold((0.0f32, f64::NEG_INFINITY), |acc, x| if x.1 > acc.1 { x } else { acc })
    };
    let across = best(false, 0.0, RANGE, COARSE);
    let down = best(true, 0.0, RANGE, COARSE);
    let vertical = down.1 > across.1;
    let coarse = if vertical { down.0 } else { across.0 };
    let (degrees, sharp) = best(vertical, coarse, COARSE, FINE);
    let level = sharpness(&points, 0.0, vertical);
    let degrees = (degrees / FINE).round() * FINE;
    let degrees = if degrees.abs() >= FINE && sharp >= level * MIN_GAIN { degrees } else { 0.0 };
    Some(Skew { degrees, vertical })
}

/// `pixels` (RGBA, `width` × `height`) turned `degrees` counter-clockwise on screen about
/// `centre` (pixels), the same size; what comes in from outside is white.
pub fn rotate(pixels: &[u8], width: u32, height: u32, degrees: f32, centre: [f32; 2]) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![255u8; w.saturating_mul(h).saturating_mul(4)];
    if pixels.len() < out.len() {
        return out;
    }
    let (s, c) = degrees.to_radians().sin_cos();
    let [cx, cy] = centre;
    for y in 0..h {
        for x in 0..w {
            // The source of each output pixel: the output turned back (clockwise on screen).
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let (sx, sy) = (cx + dx * c - dy * s - 0.5, cy + dx * s + dy * c - 0.5);
            if !(sx >= 0.0 && sy >= 0.0 && sx <= (w - 1) as f32 && sy <= (h - 1) as f32) {
                continue;
            }
            // Bilinear: letters stay smooth for recognition.
            let (x0, y0) = (sx as usize, sy as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let at = |x: usize, y: usize, k: usize| f32::from(pixels.get((y * w + x) * 4 + k).copied().unwrap_or(255));
            for k in 0..4 {
                let top = at(x0, y0, k) * (1.0 - fx) + at(x1, y0, k) * fx;
                let bottom = at(x0, y1, k) * (1.0 - fx) + at(x1, y1, k) * fx;
                if let Some(o) = out.get_mut((y * w + x) * 4 + k) {
                    *o = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    out
}

/// `pixels` (RGBA, `width` × `height`) turned `quarters` quarter turns clockwise, with its new
/// width and height.
pub fn quarter_turn(pixels: &[u8], width: u32, height: u32, quarters: u8) -> (Vec<u8>, u32, u32) {
    let (w, h) = (width as usize, height as usize);
    let q = quarters % 4;
    let (nw, nh) = if q % 2 == 1 { (h, w) } else { (w, h) };
    let mut out = vec![255u8; w.saturating_mul(h).saturating_mul(4)];
    for y in 0..nh {
        for x in 0..nw {
            // The source pixel of output (x, y).
            let (sx, sy) = match q {
                1 => (y, h - 1 - x),
                2 => (w - 1 - x, h - 1 - y),
                3 => (w - 1 - y, x),
                _ => (x, y),
            };
            let (from, to) = ((sy * w + sx) * 4, (y * nw + x) * 4);
            if let (Some(src), Some(dst)) = (pixels.get(from..from + 4), out.get_mut(to..to + 4)) {
                dst.copy_from_slice(src);
            }
        }
    }
    (out, nw as u32, nh as u32)
}

/// A point in an image turned by [`quarter_turn`] (`quarters`, from `width` × `height`) back
/// in the original image.
pub fn unturn([x, y]: [f32; 2], width: u32, height: u32, quarters: u8) -> [f32; 2] {
    let (w, h) = (width as f32, height as f32);
    match quarters % 4 {
        1 => [y, h - x],
        2 => [w - x, h - y],
        3 => [w - y, x],
        _ => [x, y],
    }
}

/// How much of the recognised text reads as words: (characters in plausible words, all
/// characters). A plausible word, without the punctuation around it, is two or more letters in
/// lower case, capitals, or a capital and then lower case; or a number. Text read upside down or
/// sideways mixes cases, digits and marks inside words.
pub fn plausibility(lines: &[Line]) -> (usize, usize) {
    let (mut good, mut all) = (0usize, 0usize);
    for w in lines.iter().flat_map(|l| &l.words) {
        let core = w.text.trim_matches(|c: char| ".,;:!?\"'()[]-–—«»“”‘’".contains(c));
        let n = w.text.chars().count();
        all = all.saturating_add(n);
        let letters = core.chars().count() >= 2 && core.chars().all(char::is_alphabetic);
        let mut rest = core.chars().skip(1);
        let cased = core.chars().all(char::is_lowercase)
            || core.chars().all(char::is_uppercase)
            || (core.chars().next().is_some_and(char::is_uppercase) && rest.all(char::is_lowercase));
        let number = core.chars().next().is_some_and(|c| c.is_ascii_digit()) && core.chars().all(|c| c.is_ascii_digit() || ",.%€$".contains(c));
        if (letters && cased) || number {
            good = good.saturating_add(n);
        }
    }
    (good, all)
}

/// Text with fewer characters than this is too little to tell which way up it is.
const MIN_CHARS: usize = 20;
/// Upright text: at least this share of its characters are in plausible words.
const UPRIGHT: f64 = 0.7;

/// Whether recognised text reads as upright words ([`plausibility`]), or `None` when there is
/// too little of it to tell.
pub fn reads_as_words(lines: &[Line]) -> Option<bool> {
    let (good, all) = plausibility(lines);
    (all >= MIN_CHARS).then_some(good as f64 >= UPRIGHT * all as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Word;

    /// Lines of "text": dark dashes like words, 8 lines on a white image, drawn turned
    /// `degrees` clockwise on screen about the centre.
    fn page(width: u32, height: u32, degrees: f32) -> Vec<u8> {
        let mut px = vec![255u8; (width * height * 4) as usize];
        let (s, c) = degrees.to_radians().sin_cos();
        let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
        for line in 0..8 {
            let y = 60.0 + line as f32 * 40.0;
            for word in 0..6 {
                let x0 = 40.0 + word as f32 * 70.0;
                for dx in 0..55 {
                    for dy in 0..12 {
                        let (px0, py0) = (x0 + dx as f32 - cx, y + dy as f32 - cy);
                        // Clockwise on screen (y down).
                        let (x, y) = (cx + px0 * c - py0 * s, cy + px0 * s + py0 * c);
                        if x >= 0.0 && y >= 0.0 && (x as u32) < width && (y as u32) < height {
                            let o = ((y as u32 * width + x as u32) * 4) as usize;
                            px[o..o + 3].copy_from_slice(&[0, 0, 0]);
                        }
                    }
                }
            }
        }
        px
    }

    #[test]
    fn finds_the_turn_that_levels_the_lines() {
        let (w, h) = (500, 420);
        for tilt in [-7.0f32, -2.5, 1.0, 3.0, 9.0] {
            let found = skew(&page(w, h, tilt), w, h).expect("a tilted page has a skew");
            assert!(!found.vertical, "{tilt}: {found:?}");
            // Lines turned clockwise are levelled by the same turn counter-clockwise.
            assert!((found.degrees - tilt).abs() <= 0.15, "{tilt}: {found:?}");
        }
        // Level lines need nothing; an empty page has nothing to tell.
        assert_eq!(skew(&page(w, h, 0.0), w, h), Some(Skew { degrees: 0.0, vertical: false }));
        assert_eq!(skew(&vec![255; (w * h * 4) as usize], w, h), None);
    }

    #[test]
    fn straightening_the_pixels_levels_them() {
        let (w, h) = (500, 420);
        let tilted = page(w, h, 4.0);
        let found = skew(&tilted, w, h).unwrap();
        let level = rotate(&tilted, w, h, found.degrees, [w as f32 / 2.0, h as f32 / 2.0]);
        assert_eq!(skew(&level, w, h).map(|s| s.degrees), Some(0.0), "straightened");
    }

    #[test]
    fn a_sideways_page_has_lines_running_down() {
        let (w, h) = (500, 420);
        let (side, sw, sh) = quarter_turn(&page(w, h, 2.0), w, h, 1);
        let found = skew(&side, sw, sh).unwrap();
        assert!(found.vertical && (found.degrees - 2.0).abs() <= 0.15, "{found:?}");
    }

    #[test]
    fn quarter_turns_map_points_back() {
        // A 3 × 2 image with one black pixel at (2, 0), the top right.
        let mut px = vec![255u8; 3 * 2 * 4];
        px[8..12].copy_from_slice(&[0, 0, 0, 255]);
        let (one, w, h) = quarter_turn(&px, 3, 2, 1);
        assert_eq!((w, h), (2, 3));
        // Clockwise: the top right goes to the bottom right.
        assert_eq!(&one[(2 * 2 + 1) * 4..(2 * 2 + 1) * 4 + 3], &[0, 0, 0]);
        for q in 0..4u8 {
            let (turned, tw, th) = quarter_turn(&px, 3, 2, q);
            let at = turned.chunks(4).position(|p| p[0] == 0).unwrap();
            let centre = [(at as u32 % tw) as f32 + 0.5, (at as u32 / tw) as f32 + 0.5];
            assert_eq!(unturn(centre, 3, 2, q), [2.5, 0.5], "{q}");
            assert_eq!(quarter_turn(&turned, tw, th, (4 - q) % 4).0, px, "{q}: turned back");
        }
    }

    #[test]
    fn upright_text_reads_as_words() {
        let line = |t: &str| Line { words: t.split(' ').map(|w| Word { text: w.into(), rect: [0.0; 4] }).collect() };
        let (good, all) = plausibility(&[line("The quick brown fox, jumps over 12 LAZY dogs.")]);
        assert_eq!(good, all);
        // "The quick brown fox" read upside down by a Latin model.
        let (good, all) = plausibility(&[line("xoJ uMoJq ʞ!nb ǝɥ⊥"), line("x0j nMOJq x!nb 3ɥ1")]);
        assert!(good * 2 < all, "{good}/{all}");
        assert_eq!(plausibility(&[]), (0, 0));
        assert_eq!(reads_as_words(&[line("The quick brown fox jumps over the lazy dog.")]), Some(true));
        assert_eq!(reads_as_words(&[line("xoJ uMoJq ʞ!nb ǝɥ⊥ x0j nMOJq x!nb 3ɥ1")]), Some(false));
        assert_eq!(reads_as_words(&[line("Total")]), None, "too short to tell");
        // Malformed pixels and sizes do not panic.
        assert_eq!(skew(&[], 0, 0), None);
        assert_eq!(skew(&[0; 3], 100, 100), None);
        assert_eq!(rotate(&[0; 3], 2, 2, 5.0, [1.0, 1.0]).len(), 16);
    }
}
