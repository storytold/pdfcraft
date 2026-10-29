//! Layout regions: where a page's paragraphs, headings, captions, tables and figures are, and in
//! which order they are read.
//!
//! The PP-DocLayoutV3 network (PaddlePaddle, Apache-2.0; the ONNX export fetched by
//! `cargo xtask models`, pinned by SHA-256) finds a page's regions and ranks them in reading
//! order. Its input conventions come from the configuration published with the model
//! (`inference.yml`: the page stretched to 800 × 800 pixels whatever its shape, channels first,
//! values not normalised) and from running it: the pixels are scaled to 0–1 (unscaled values give
//! no regions at all), and the boxes come back in the page picture's own pixels, as it is told the
//! scale through `scale_factor`.
//!
//! [`Layout::regions`] returns the regions; `layout::paragraphs` puts recognised lines into them.

use std::path::Path;

use rten::{Model, NodeId};
use rten_tensor::{AsView, Layout as _, NdTensor};

use crate::OcrError;

/// The size the network reads the page at.
const SIDE: usize = 800;
/// The network's inputs: the picture, its size as read, and the scale from the page picture.
const IMAGE_NODE: &str = "image";
const SHAPE_NODE: &str = "im_shape";
const SCALE_NODE: &str = "scale_factor";
/// The network's output, one row per region: label index, score, the box (left, top, right,
/// bottom, in page picture pixels) and the region's rank in reading order.
const BOXES_NODE: &str = "fetch_name_0";
/// Regions the network is less sure of than this are not used (the model configuration's own
/// display threshold).
const MIN_SCORE: f32 = 0.5;
/// At most this many regions are taken from a page (the network proposes a few hundred).
const MAX_REGIONS: usize = 1_000;

/// The region labels, in the order of the network's label index (from the model's
/// `inference.yml`).
const LABELS: [&str; 25] = [
    "abstract",
    "algorithm",
    "aside_text",
    "chart",
    "content",
    "display_formula",
    "doc_title",
    "figure_title",
    "footer",
    "footer_image",
    "footnote",
    "formula_number",
    "header",
    "header_image",
    "image",
    "inline_formula",
    "number",
    "paragraph_title",
    "reference",
    "reference_content",
    "seal",
    "table",
    "text",
    "vertical_text",
    "vision_footnote",
];

/// A region of the page: what it is, its box in image pixels `[left, top, right, bottom]`, and
/// its rank in reading order (lower first).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Region {
    pub label: &'static str,
    pub rect: [f32; 4],
    pub rank: f32,
}

/// The layout model, loaded.
pub(crate) struct Layout {
    model: Model,
    image: NodeId,
    shape: NodeId,
    scale: NodeId,
    boxes: NodeId,
}

impl Layout {
    pub fn load(path: &Path) -> Result<Layout, OcrError> {
        let failed = |e: String| OcrError::Load(path.display().to_string(), e);
        let model = Model::load_file(path).map_err(|e| failed(e.to_string()))?;
        let node = |name: &str| {
            model.find_node(name).ok_or_else(|| failed(format!("it has no node {name:?}: not the PP-DocLayoutV3 model PdfCraft expects")))
        };
        let (image, shape, scale, boxes) = (node(IMAGE_NODE)?, node(SHAPE_NODE)?, node(SCALE_NODE)?, node(BOXES_NODE)?);
        Ok(Layout { model, image, shape, scale, boxes })
    }

    /// The regions of a page picture: `pixels` holds `channels` (1 or 3) bytes a pixel, row by
    /// row, `width` × `height` pixels.
    pub fn regions(&self, pixels: &[u8], channels: usize, width: usize, height: usize) -> Result<Vec<Region>, OcrError> {
        let failed = |e: String| OcrError::Recognize(format!("finding the page layout: {e}"));
        let input = network_input(pixels, channels, width, height).ok_or_else(|| failed("the picture's size does not match its pixels".into()))?;
        let image = NdTensor::from_data([1, 3, SIDE, SIDE], input);
        let side = SIDE as f32;
        let shape = NdTensor::from_data([1, 2], vec![side, side]);
        // Both sides are at least one pixel (network_input checked).
        let scale = NdTensor::from_data([1, 2], vec![side / height as f32, side / width as f32]);
        let inputs = vec![(self.image, image.into()), (self.shape, shape.into()), (self.scale, scale.into())];
        let mut out = self.model.run(inputs, &[self.boxes], None).map_err(|e| failed(e.to_string()))?;
        let value = out.pop().ok_or_else(|| failed("the model returned nothing".into()))?;
        let rows: NdTensor<f32, 2> = value.try_into().map_err(|e: rten::TryFromValueError| failed(e.to_string()))?;
        let [n, columns] = rows.shape();
        if columns < 7 {
            return Err(failed(format!("the model returned {columns} columns a region, not 7")));
        }
        let row = |i: usize| -> [f32; 7] { std::array::from_fn(|c| rows.get([i, c]).copied().unwrap_or(f32::NAN)) };
        Ok(regions_from_rows((0..n.min(MAX_REGIONS)).map(row), width as f32, height as f32))
    }
}

/// The regions in the network's output rows, kept only when they are sure enough and make sense:
/// a known label, a finite box with some area inside the picture (clipped to it), and a finite
/// rank (a missing one reads last).
fn regions_from_rows(rows: impl Iterator<Item = [f32; 7]>, width: f32, height: f32) -> Vec<Region> {
    rows.filter_map(|[label, score, l, t, r, b, rank]| {
        // A score must be a real probability and a label a whole class number: anything else is
        // a malformed row, not a confident one (an infinite score is not "above the threshold",
        // and 2.7 is no class).
        if !score.is_finite() || score < MIN_SCORE || !label.is_finite() || label < 0.0 || label.fract() != 0.0 {
            return None;
        }
        // A finite whole number, not negative, so the cast is exact or saturates; `get` rejects
        // the rest.
        let label = *LABELS.get(label as usize)?;
        let rect = [l.clamp(0.0, width), t.clamp(0.0, height), r.clamp(0.0, width), b.clamp(0.0, height)];
        // `clamp` passes NaN through; `>` is false for it.
        if !(rect[2] > rect[0] && rect[3] > rect[1]) {
            return None;
        }
        Some(Region { label, rect, rank: if rank.is_finite() { rank } else { f32::MAX } })
    })
    .collect()
}

/// The network's input: the picture stretched to [`SIDE`] × [`SIDE`], RGB channels first, scaled
/// to 0–1. Shrinking averages the pixels under each output pixel; enlarging interpolates.
/// `None` when `pixels` is not a `width` × `height` picture of 1 or 3 channels.
fn network_input(pixels: &[u8], channels: usize, width: usize, height: usize) -> Option<Vec<f32>> {
    if width == 0 || height == 0 || !matches!(channels, 1 | 3) || width.checked_mul(height)?.checked_mul(channels)? != pixels.len() {
        return None;
    }
    let (wx, wy) = (axis_weights(width, SIDE), axis_weights(height, SIDE));
    let at = |x: usize, y: usize, c: usize| pixels.get((y * width + x) * channels + c).copied().map_or(0.0, f32::from) / 255.0;
    // One axis, then the other: the shorter side is stretched last, so the picture in between is
    // SIDE times the shorter side (a very long, thin picture needs no more than it holds).
    let mut mid = vec![0.0_f32; width.min(height) * SIDE * channels];
    let mut out = vec![0.0_f32; 3 * SIDE * SIDE];
    let mut put = |ox: usize, oy: usize, c: usize, v: f32| {
        // A grey picture fills all three channels.
        for plane in if channels == 1 { 0..3 } else { c..c + 1 } {
            if let Some(slot) = out.get_mut((plane * SIDE + oy) * SIDE + ox) {
                *slot = v;
            }
        }
    };
    if width >= height {
        // Across first: `mid` is height rows of SIDE.
        for (y, ox) in (0..height).flat_map(|y| (0..SIDE).map(move |ox| (y, ox))) {
            for c in 0..channels {
                let v = wx.get(ox).map_or(0.0, |taps| taps.iter().map(|&(x, k)| k * at(x, y, c)).sum());
                if let Some(slot) = mid.get_mut((y * SIDE + ox) * channels + c) {
                    *slot = v;
                }
            }
        }
        for (oy, ox) in (0..SIDE).flat_map(|oy| (0..SIDE).map(move |ox| (oy, ox))) {
            for c in 0..channels {
                let v = wy
                    .get(oy)
                    .map_or(0.0, |taps| taps.iter().map(|&(y, k)| k * mid.get((y * SIDE + ox) * channels + c).copied().unwrap_or(0.0)).sum());
                put(ox, oy, c, v);
            }
        }
    } else {
        // Down first: `mid` is SIDE rows of width.
        for (oy, x) in (0..SIDE).flat_map(|oy| (0..width).map(move |x| (oy, x))) {
            for c in 0..channels {
                let v = wy.get(oy).map_or(0.0, |taps| taps.iter().map(|&(y, k)| k * at(x, y, c)).sum());
                if let Some(slot) = mid.get_mut((oy * width + x) * channels + c) {
                    *slot = v;
                }
            }
        }
        for (oy, ox) in (0..SIDE).flat_map(|oy| (0..SIDE).map(move |ox| (oy, ox))) {
            for c in 0..channels {
                let v = wx
                    .get(ox)
                    .map_or(0.0, |taps| taps.iter().map(|&(x, k)| k * mid.get((oy * width + x) * channels + c).copied().unwrap_or(0.0)).sum());
                put(ox, oy, c, v);
            }
        }
    }
    Some(out)
}

/// For each of `dst` output samples along one axis of `src` input samples: the input samples it
/// reads and their weights (summing to 1). Shrinking averages the input samples each output one
/// covers; enlarging interpolates linearly between the two nearest, sample centres aligned.
pub(crate) fn axis_weights(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
    let last = src.saturating_sub(1);
    let k = src as f64 / dst.max(1) as f64;
    (0..dst)
        .map(|i| {
            if src > dst {
                let (a, b) = (i as f64 * k, (i as f64 + 1.0) * k);
                // Both in 0..=src: the casts saturate at worst.
                let (first, end) = (a.floor() as usize, (b.ceil() as usize).min(src));
                (first..end)
                    .filter_map(|j| {
                        let cover = (b.min(j as f64 + 1.0) - a.max(j as f64)) / k;
                        (cover > 0.0).then_some((j, cover as f32))
                    })
                    .collect()
            } else {
                let pos = ((i as f64 + 0.5) * k - 0.5).clamp(0.0, last as f64);
                let j = (pos.floor() as usize).min(last);
                let t = (pos - j as f64) as f32;
                let next = (j + 1).min(last);
                if t > 0.0 && next != j { vec![(j, 1.0 - t), (next, t)] } else { vec![(j, 1.0)] }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_weights_sum_to_one() {
        for (src, dst) in [(1, 800), (799, 800), (800, 800), (801, 800), (2480, 800), (3508, 800), (100_000, 800)] {
            let w = axis_weights(src, dst);
            assert_eq!(w.len(), dst);
            for (i, taps) in w.iter().enumerate() {
                let sum: f32 = taps.iter().map(|t| t.1).sum();
                assert!((sum - 1.0).abs() < 1e-4, "{src} → {dst}, sample {i}: {sum}");
                assert!(taps.iter().all(|t| t.0 < src), "{src} → {dst}, sample {i}: {taps:?}");
            }
        }
    }

    /// The network reads a page of any shape as 800 × 800, channels first, 0–1; a grey page fills
    /// all three channels, and a picture whose pixels don't match its size is refused.
    #[test]
    fn the_input_is_the_page_stretched_to_the_network_size() {
        // A 4 × 2 picture: left half black, right half white, so after stretching the left half of
        // every row is 0 and the right half 1.
        let rgb: Vec<u8> = (0..8).flat_map(|i| if i % 4 < 2 { [0, 0, 0] } else { [255, 255, 255] }).collect();
        let input = network_input(&rgb, 3, 4, 2).unwrap();
        assert_eq!(input.len(), 3 * SIDE * SIDE);
        for c in 0..3 {
            let at = |x: usize, y: usize| input[c * SIDE * SIDE + y * SIDE + x];
            assert_eq!((at(0, 0), at(SIDE - 1, SIDE - 1)), (0.0, 1.0));
            assert!(at(SIDE / 2 - 120, 400) < 0.01 && at(SIDE / 2 + 120, 400) > 0.99);
        }
        let grey = network_input(&[0, 255], 1, 2, 1).unwrap();
        assert_eq!((grey[0], grey[SIDE * SIDE], grey[2 * SIDE * SIDE + SIDE - 1]), (0.0, 0.0, 1.0));
        // A large page is averaged down, not sampled: one black dot in every block of pixels that
        // shrinks to one reads as that block's mean grey, in a wide page and in a tall one.
        for (w, h, kx, ky) in [(4800, 3200, 6, 4), (3200, 4800, 4, 6)] {
            let dots: Vec<u8> = (0..w * h).map(|i| if i % w % kx == 0 && i / w % ky == 0 { 0 } else { 255 }).collect();
            let small = network_input(&dots, 1, w, h).unwrap();
            let mean = 1.0 - 1.0 / (kx * ky) as f32;
            assert!(small.iter().all(|v| (v - mean).abs() < 1e-3), "{w} × {h}");
        }
        for (len, c, w, h) in [(5, 3, 4, 2), (24, 3, 0, 2), (8, 4, 2, 1), (0, 1, 0, 0), (1, 1, usize::MAX, 2)] {
            assert_eq!(network_input(&vec![0; len], c, w, h), None, "{len} bytes, {c} channels, {w} × {h}");
        }
    }

    /// Rows the network is unsure of, with an unknown or fractional label, an infinite score, or
    /// with a box that is empty, inverted, not a number or outside the picture are dropped; boxes
    /// reaching past the edge are clipped.
    #[test]
    fn regions_keep_only_sensible_rows() {
        let rows = [
            [22.0, 0.9, 10.0, 20.0, 110.0, 60.0, 3.0],
            [17.0, 0.6, -5.0, -5.0, 2000.0, 40.0, f32::NAN],
            [22.0, 0.4, 10.0, 20.0, 110.0, 60.0, 1.0],
            [f32::NAN, 0.9, 10.0, 20.0, 110.0, 60.0, 1.0],
            [25.0, 0.9, 10.0, 20.0, 110.0, 60.0, 1.0],
            [-1.0, 0.9, 10.0, 20.0, 110.0, 60.0, 1.0],
            [22.0, f32::NAN, 10.0, 20.0, 110.0, 60.0, 1.0],
            [22.0, 0.9, 50.0, 20.0, 50.0, 60.0, 1.0],
            [22.0, 0.9, 60.0, 20.0, 50.0, 60.0, 1.0],
            [22.0, 0.9, f32::NAN, 20.0, 110.0, 60.0, 1.0],
            [22.0, 0.9, 900.0, 20.0, 1000.0, 60.0, 1.0],
            [22.0, 0.9, f32::NEG_INFINITY, 20.0, f32::INFINITY, 60.0, 1.0],
            [22.0, f32::INFINITY, 10.0, 20.0, 110.0, 60.0, 1.0],
            [21.6, 0.9, 10.0, 20.0, 110.0, 60.0, 1.0],
        ];
        let got = regions_from_rows(rows.into_iter(), 800.0, 600.0);
        assert_eq!(
            got,
            [
                Region { label: "text", rect: [10.0, 20.0, 110.0, 60.0], rank: 3.0 },
                Region { label: "paragraph_title", rect: [0.0, 0.0, 800.0, 40.0], rank: f32::MAX },
                Region { label: "text", rect: [0.0, 20.0, 800.0, 60.0], rank: 1.0 },
            ]
        );
    }
}
