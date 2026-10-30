//! A synthetic phone photo of a curled page, and a measure of how straight its lines of text are,
//! for testing page flattening (Enhance camera image ▸ Flatten curved page).
//!
//! The page is white paper with lines of dark "words" (bars of varied width). The photo bends it:
//! a bulge across the page (the middle of every line sits higher than its ends, as on a page lifted
//! by the book's spine), a slight tilt, and perspective (the far edge narrower), on a grey table.

/// Ink is darker than this (luminance, 0–255); paper and table are lighter.
const INK: f64 = 80.0;

/// The flat page: white, `w` × `h` pixels, with lines of word-sized bars. True where there is ink.
fn page_ink(w: usize, h: usize) -> Vec<bool> {
    let mut ink = vec![false; w * h];
    let mut seed: u32 = 0x2545_f491;
    let mut next = |n: u32| {
        // A small linear congruential generator: the same page every time.
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) % n.max(1)
    };
    let (margin, pitch, line) = (w / 10, 26, 10);
    let mut top = h / 12;
    while top + line < h - h / 12 {
        let mut x = margin;
        while x < w - margin {
            let word = 20 + next(50) as usize;
            for y in top..top + line {
                for xx in x..(x + word).min(w - margin) {
                    if let Some(p) = ink.get_mut(y * w + xx) {
                        *p = true;
                    }
                }
            }
            x += word + 12;
        }
        top += pitch;
    }
    ink
}

/// A `width` × `height` RGB8 photo of a curled page (see the module docs).
pub fn curled_page(width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as f64, height as f64);
    // The flat page's size, and where its top-left corner lands.
    let (pw, ph) = ((w * 0.7).round().max(2.0), (h * 0.78).round().max(2.0));
    let (pwi, phi) = (pw as usize, ph as usize);
    let ink = page_ink(pwi, phi);
    let (cx, y0) = (w / 2.0, h * 0.1);
    // Bulge height, tilt (per pixel across), and how much narrower the bottom is than the top.
    let (bulge, tilt, narrow) = (h * 0.035, 0.04, 0.12);
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for py in 0..height {
        for px in 0..width {
            // Supersample 2 × 2 for soft edges.
            let mut lum = 0.0;
            for (dx, dy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let (x, y) = (px as f64 + dx, py as f64 + dy);
                // Invert the bend by fixed-point iteration (it is gentle, so this converges):
                // x = cx + (u - pw/2)·s(v), y = y0 + v + bulge·sin(πu/pw) + tilt·(u - pw/2).
                let (mut u, mut v) = (x - cx + pw / 2.0, y - y0);
                for _ in 0..30 {
                    v = y - y0 - bulge * (std::f64::consts::PI * u / pw).sin() - tilt * (u - pw / 2.0);
                    let s = 1.0 - narrow * (v / ph);
                    u = (x - cx) / s + pw / 2.0;
                }
                lum += if u >= 0.0 && v >= 0.0 && u < pw && v < ph {
                    let at = v as usize * pwi + u as usize;
                    if ink.get(at).copied().unwrap_or(false) { 25.0 } else { 245.0 }
                } else {
                    // The table, with a little grain.
                    150.0 + ((px * 7 + py * 13) % 11) as f64
                };
            }
            let l = (lum / 4.0).round().clamp(0.0, 255.0) as u8;
            rgb.extend([l, l, l.saturating_sub(8)]);
        }
    }
    rgb
}

/// How straight and level the lines of text in an RGB8 picture are: in its middle (the central
/// 60 % each way), the ink-weighted mean share of a row that is ink. Lines that run straight across
/// fill the rows they are on (about 0.7 for [`curled_page`]'s words) whatever the scale; bent or
/// tilted lines spread their ink over more rows, each less full. 0 when there is no ink.
pub fn line_sharpness(rgb: &[u8], width: u32, height: u32) -> f64 {
    let (w, h) = (width as usize, height as usize);
    let (x0, x1, y0, y1) = (w / 5, w - w / 5, h / 5, h - h / 5);
    let (mut sum, mut sq) = (0.0, 0.0);
    for y in y0..y1 {
        let n = (x0..x1)
            .filter(|&x| {
                let i = (y * w + x) * 3;
                rgb.get(i..i + 3).is_some_and(|p| 0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]) < INK)
            })
            .count();
        let f = n as f64 / (x1 - x0).max(1) as f64;
        sum += f;
        sq += f * f;
    }
    if sum > 0.0 { sq / sum } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flat_page_is_sharper_than_the_curled_photo() {
        let (w, h) = (300, 400);
        let flat: Vec<u8> = page_ink(w, h).iter().flat_map(|&i| if i { [25; 3] } else { [245; 3] }).collect();
        let flat = line_sharpness(&flat, w as u32, h as u32);
        let photo = line_sharpness(&curled_page(w as u32, h as u32), w as u32, h as u32);
        assert!(flat > 0.6, "flat {flat}");
        assert!(photo < flat - 0.2, "photo {photo} vs flat {flat}");
    }
}
