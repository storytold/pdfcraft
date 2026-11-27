//! Edit text on a scanned page: an OCR word is invisible text over the scan, so rewriting it must
//! also hide the scanned word. The paper colour around the word becomes the cover drawn over it
//! ([`pdfcraft_edit::BlockStyle::cover`]) and the colour of its ink becomes the new text's.

use std::sync::Arc;

use pdfcraft_edit::TextBlock;
use pdfcraft_render::{PageInfo, PageRenderer, RenderConfig, RenderRequest, Tile};

/// Device pixels per point the word's surroundings are read at.
const SCALE: f32 = 3.0;
/// The most pixels read for one paragraph; a larger area is read at a lower resolution.
const MAX_PIXELS: f32 = 4_000_000.0;
/// How far around the word (points) the paper is read.
const RING: f32 = 4.0;

/// The paper and ink colours (RGB 0–1) of invisible paragraph `b` on `page`, or `None` when the
/// paragraph is visible text or the page can't be read.
pub(crate) fn sample(cos: &pdfcraft_cos::Document, info: &PageInfo, page: usize, b: &TextBlock) -> Option<([f64; 3], [f64; 3])> {
    if !b.invisible {
        return None;
    }
    // The word's box in view space (points, y down).
    let corners = [[b.rect[0], b.rect[1]], [b.rect[2], b.rect[1]], [b.rect[0], b.rect[3]], [b.rect[2], b.rect[3]]]
        .map(|[x, y]| info.user_to_view(x as f32, y as f32));
    let (x0, x1) = corners.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, z), c| (a.min(c[0]), z.max(c[0])));
    let (y0, y1) = corners.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, z), c| (a.min(c[1]), z.max(c[1])));
    let word = [x0, y0, x1, y1];
    let area = [x0 - RING, y0 - RING, x1 + RING, y1 + RING];
    if word.iter().any(|v| !v.is_finite()) || x1 <= x0 || y1 <= y0 {
        return None;
    }
    let pixels = (area[2] - area[0]) * (area[3] - area[1]) * SCALE * SCALE;
    let scale = if pixels > MAX_PIXELS { SCALE * (MAX_PIXELS / pixels).sqrt() } else { SCALE };
    let (page_w, page_h) = (info.width * scale, info.height * scale);
    let clamp = |v: f32, max: f32| v.clamp(0.0, max.max(0.0)).floor() as u32;
    let (tx0, ty0) = (clamp(area[0] * scale, page_w), clamp(area[1] * scale, page_h));
    let (tx1, ty1) = (clamp(area[2] * scale, page_w), clamp(area[3] * scale, page_h));
    if tx1 <= tx0 || ty1 <= ty0 {
        return None;
    }
    let tile = Tile { x: tx0, y: ty0, w: tx1 - tx0, h: ty1 - ty0 };
    // The working file as it is: written out only if the document holds unsaved changes, which
    // it doesn't between edits (each one reopens the file it wrote).
    let bytes = if cos.is_modified() {
        Arc::new(pdfcraft_cos::write_incremental(cos, &pdfcraft_cos::SaveOptions::default()).ok()?)
    } else {
        cos.bytes().clone()
    };
    // The document's true colours, whatever the reading colours are set to.
    let shot = PageRenderer::new(bytes, RenderConfig::default()).render(RenderRequest { page, scale, tile: Some(tile), ..Default::default() });
    if shot.error.is_some() {
        return None;
    }
    let pad = ((y1 - y0) * 0.1).clamp(0.5, 2.0);
    let (mut paper, mut ink) = (Vec::new(), Vec::new());
    for (i, px) in shot.rgba.as_chunks::<4>().0.iter().enumerate() {
        let (col, row) = (i % shot.width.max(1) as usize, i / shot.width.max(1) as usize);
        let x = (tx0 as f32 + col as f32 + 0.5) / scale;
        let y = (ty0 as f32 + row as f32 + 0.5) / scale;
        let rgb = [px[0], px[1], px[2]].map(|c| f64::from(c) / 255.0);
        if x >= x0 && x <= x1 && y >= y0 && y <= y1 {
            ink.push(rgb);
        } else if x < x0 - pad || x > x1 + pad || y < y0 - pad || y > y1 + pad {
            paper.push(rgb);
        }
    }
    let by_luma = |a: &[f64; 3], b: &[f64; 3]| luma(a).total_cmp(&luma(b));
    ink.sort_by(by_luma);
    let paper = paper_colour(&mut paper);
    // The ink is the average of the word's darkest pixels; a box without any is printed in black.
    let darkest = ink.get(..ink.len().div_ceil(8)).unwrap_or_default();
    let ink = if darkest.is_empty() {
        [0.0; 3]
    } else {
        let sum = darkest.iter().fold([0.0; 3], |s, c| [s[0] + c[0], s[1] + c[1], s[2] + c[2]]);
        sum.map(|v| v / darkest.len() as f64)
    };
    let ink = if luma(&paper) - luma(&ink) < 0.25 { [0.0; 3] } else { ink };
    Some((paper, ink))
}

fn luma(c: &[f64; 3]) -> f64 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

/// The shades the ring's light pixels are sorted into to find the paper's.
const SHADES: usize = 32;

/// The paper's colour among the pixels of the ring around a word. Of the ring's light pixels,
/// those at least as light as the midpoint between its darkest and its lightest, the paper is the
/// shade most of them have, and its colour the median of that shade: a rule's halo runs through
/// every shade from ink to paper, the paper sits in one. So a table rule or a neighbouring letter
/// in the ring doesn't count, however much of the ring they fill: in a tight title-block cell the
/// rules and their halo are more than half of it (#8501). A grey-paper scan keeps its grey, the
/// ring's light pixels being the paper, and a stray white speck is no shade of its own. White for
/// an empty ring.
fn paper_colour(ring: &mut [[f64; 3]]) -> [f64; 3] {
    ring.sort_by(|a, b| luma(a).total_cmp(&luma(b)));
    let (Some(darkest), Some(lightest)) = (ring.first(), ring.last()) else { return [1.0; 3] };
    let mid = (luma(darkest) + luma(lightest)) / 2.0;
    let light = ring.get(ring.partition_point(|c| luma(c) < mid)..).unwrap_or_default();
    let shade = |c: &[f64; 3]| ((luma(c) * SHADES as f64).floor() as usize).min(SHADES - 1);
    let mut counts = [0usize; SHADES];
    for c in light {
        if let Some(n) = counts.get_mut(shade(c)) {
            *n += 1;
        }
    }
    // The fullest shade, the lighter one when two are as full.
    let Some(fullest) = counts.iter().enumerate().max_by_key(|(i, n)| (**n, *i)).map(|(i, _)| i) else { return [1.0; 3] };
    let of_shade: Vec<[f64; 3]> = light.iter().filter(|c| shade(c) == fullest).copied().collect();
    of_shade.get(of_shade.len() / 2).copied().unwrap_or([1.0; 3])
}

#[cfg(test)]
mod tests {
    use super::paper_colour;

    #[test]
    fn the_paper_is_the_fullest_light_shade_of_the_ring() {
        let pixels = |n: usize, v: f64| std::iter::repeat_n([v; 3], n);
        // A rule's halo: a pixel of every shade from the ink to the paper.
        let halo = |n: usize| (0..n).map(move |i| [0.2 + 0.78 * i as f64 / n as f64; 3]);
        // A tight cell: the rules are most of the ring, their halo more than the paper, which is
        // a tenth of it.
        let mut ring: Vec<[f64; 3]> = pixels(60, 0.1).chain(halo(30)).chain(pixels(10, 1.0)).collect();
        assert_eq!(paper_colour(&mut ring), [1.0; 3], "white paper, not the halo and not the rule");
        // Grey paper, a little noisy, with a few white specks and a letter in the ring stays grey.
        let noisy = (0..80).map(|i| [0.78 + 0.02 * (i % 3) as f64; 3]);
        let mut ring: Vec<[f64; 3]> = pixels(5, 0.05).chain(halo(8)).chain(noisy).chain(pixels(3, 1.0)).collect();
        let paper = paper_colour(&mut ring);
        assert!((0.78..=0.82).contains(&paper[0]), "{paper:?}");
        // Paper alone, and nothing.
        let mut ring: Vec<[f64; 3]> = pixels(20, 0.95).collect();
        assert_eq!(paper_colour(&mut ring), [0.95; 3]);
        assert_eq!(paper_colour(&mut []), [1.0; 3]);
    }
}
