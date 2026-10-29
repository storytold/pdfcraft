//! Flatten a photographed page: straighten a sheet that was curved, folded or photographed at an
//! angle, so its lines of text run straight across.
//!
//! The UVDoc network (Verhoeven et al., 2023; the PaddlePaddle ONNX export, Apache-2.0, fetched
//! by `cargo xtask models`) looks at the photo at a fixed 488 × 712 pixels and predicts a coarse
//! backward map: for each point of the flat page, where it is in the photo. The exported model
//! would then stretch that map to the photo's size and resample the photo itself, holding several
//! float copies of a full-size photo at once (about 40 bytes a pixel, 1.6 GB for a 40-megapixel
//! page). Instead [`Flattener::grid`] asks the model only for the coarse map (an inner node of the
//! pinned model; see [`GRID_NODE`]), feeding it the photo already shrunk to the network's size,
//! and [`unwarp`] resamples the full-size photo through the map itself, one output pixel at a
//! time. Memory stays at the input and output pictures, time grows with the pixel count only in
//! that last, cheap step, and the result has the photo's full resolution.
//!
//! The resampling follows the model's own: the map is stretched bilinearly with aligned corners,
//! and the photo is read bilinearly with aligned corners. Where the map points outside the photo,
//! the result is white (the model's own padding is black), as in Enhance camera image's
//! four-corner warp. A map with a coordinate that is not a finite number is an error, not white:
//! the model failed, and a white or partly white page would hide that.
//!
//! Cost on a 32-thread Zen 5 CPU: about 0.1 s for the network whatever the photo's size, then
//! about 30 ms a megapixel for the resampling (0.4 s at 12 megapixels, 1.1 s at 38), with no
//! more memory than the two pictures. So the photo is flattened at its full resolution.

use std::path::{Path, PathBuf};

use rten::{Model, NodeId};
use rten_tensor::{AsView, Layout, NdTensor};

use crate::{Models, OcrError};

/// The model file, as named in ATTRIBUTION.toml.
pub const FLATTEN_MODEL: &str = "uvdoc.onnx";

/// The size the network reads the photo at (the exported model's first Resize, PaddleX's
/// `upsample_size`).
pub const NET_WIDTH: usize = 488;
pub const NET_HEIGHT: usize = 712;

/// The model's input.
const INPUT_NODE: &str = "image";
/// The network's coarse backward map, `[1, 2, h, w]` in grid-sample coordinates (x then y, -1 to
/// 1 across the photo): the value the exported model stretches to the photo's size before its
/// GridSample (ONNX value `Add.83`, which the export passes through an Identity named
/// `p2o.pd_op.add.41.0` that rten's graph optimiser removes). An inner node, so this is tied to the
/// model file `cargo xtask models` pins by SHA-256; another export fails to load with a clear
/// error rather than misbehaving. Checked against onnxruntime running the whole model on the same
/// 488 × 712 input: the maps agree to within 5e-5.
const GRID_NODE: &str = "Add.83";

/// The most pixels a photo may have, in and out (the camera tool renders at most 40 megapixels).
pub const MAX_PIXELS: u64 = 64_000_000;
/// The largest coarse map accepted from the model (it is 45 × 31 for this one).
const MAX_GRID: usize = 1 << 20;

/// The coarse backward map: for `w` × `h` points spread evenly over the flat page (corners
/// included), where each is in the photo, as grid-sample coordinates (-1 is the first pixel's
/// centre, 1 the last one's). `xy` holds `[x, y]` row by row.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub width: usize,
    pub height: usize,
    pub xy: Vec<[f32; 2]>,
}

/// The flattening model, loaded (it takes a moment, so load it once and share it).
pub struct Flattener {
    model: Model,
    input: NodeId,
    grid: NodeId,
}

impl std::fmt::Debug for Flattener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Flattener")
    }
}

/// Where the flattening model is: the first of [`Models::search_dirs`] that has it.
pub fn find_model() -> Option<PathBuf> {
    Models::search_dirs().into_iter().map(|d| d.join(FLATTEN_MODEL)).find(|p| p.is_file())
}

/// Whether the flattening model is installed.
pub fn available() -> bool {
    find_model().is_some()
}

impl Flattener {
    /// Load the model at `path`.
    pub fn load(path: &Path) -> Result<Flattener, OcrError> {
        let failed = |e: String| OcrError::Load(path.display().to_string(), e);
        let model = Model::load_file(path).map_err(|e| failed(e.to_string()))?;
        let input =
            model.find_node(INPUT_NODE).ok_or_else(|| failed(format!("it has no input {INPUT_NODE:?}: not the UVDoc model PdfCraft expects")))?;
        let grid = model.find_node(GRID_NODE).ok_or_else(|| failed(format!("it has no node {GRID_NODE:?}: not the UVDoc model PdfCraft expects")))?;
        Ok(Flattener { model, input, grid })
    }

    /// Load the model found by [`find_model`].
    pub fn find() -> Result<Flattener, OcrError> {
        Self::load(&find_model().ok_or(OcrError::NoFlattenModel)?)
    }

    /// The coarse backward map for an RGB8 photo (row-major, `width` × `height`).
    pub fn grid(&self, rgb: &[u8], width: u32, height: u32) -> Result<Grid, OcrError> {
        let input = network_input(rgb, width, height)?;
        let tensor = NdTensor::from_data([1, 3, NET_HEIGHT, NET_WIDTH], input);
        let failed = |e: String| OcrError::Flatten(e);
        let mut out = self.model.run(vec![(self.input, tensor.into())], &[self.grid], None).map_err(|e| failed(e.to_string()))?;
        let value = out.pop().ok_or_else(|| failed("the model returned nothing".into()))?;
        let map: NdTensor<f32, 4> = value.try_into().map_err(|e: rten::TryFromValueError| failed(e.to_string()))?;
        let [n, c, h, w] = map.shape();
        if n != 1 || c != 2 || h < 2 || w < 2 || h.saturating_mul(w) > MAX_GRID {
            return Err(failed(format!("the model returned a {n} × {c} × {h} × {w} map, not one 2-channel grid")));
        }
        let mut xy = Vec::with_capacity(h * w);
        for y in 0..h {
            for x in 0..w {
                // In range: the shape was checked above.
                let at = |ch: usize| map.get([0, ch, y, x]).copied().unwrap_or(f32::NAN);
                xy.push([at(0), at(1)]);
            }
        }
        Ok(Grid { width: w, height: h, xy })
    }

    /// Flatten an RGB8 photo: the flat page, RGB8 at the photo's size.
    pub fn flatten(&self, rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, OcrError> {
        let grid = self.grid(rgb, width, height)?;
        unwarp(rgb, width, height, &grid)
    }
}

/// The photo's pixel count, if `rgb` really is a `width` × `height` RGB8 picture of an accepted
/// size.
fn checked_len(rgb: &[u8], width: u32, height: u32) -> Result<(usize, usize), OcrError> {
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 {
        return Err(OcrError::EmptyImage);
    }
    if pixels > MAX_PIXELS {
        return Err(OcrError::Flatten(format!("the picture is too large ({width} × {height} pixels; at most {MAX_PIXELS})")));
    }
    let (w, h) = (width as usize, height as usize);
    if w.checked_mul(h).and_then(|n| n.checked_mul(3)) != Some(rgb.len()) {
        return Err(OcrError::Flatten("the picture's size does not match its pixels".into()));
    }
    Ok((w, h))
}

/// The resampling weights for one axis from `src` to `dst` samples: for each output sample, the
/// input samples it reads and how much of each. Shrinking averages the input samples under each
/// output one (so fine print isn't aliased away); enlarging interpolates linearly, corners
/// aligned, as the model's own resize does.
fn axis_weights(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
    let last = src.saturating_sub(1);
    (0..dst)
        .map(|i| {
            if src > dst {
                let k = src as f64 / dst as f64;
                let (a, b) = (i as f64 * k, (i as f64 + 1.0) * k);
                let first = a.floor() as usize;
                let end = (b.ceil() as usize).min(src);
                (first..end)
                    .filter_map(|j| {
                        let cover = (b.min(j as f64 + 1.0) - a.max(j as f64)) / k;
                        (cover > 0.0).then_some((j, cover as f32))
                    })
                    .collect()
            } else {
                let pos = if dst > 1 { i as f64 * last as f64 / (dst - 1) as f64 } else { 0.0 };
                let j = (pos.floor() as usize).min(last);
                let t = (pos - j as f64) as f32;
                let next = (j + 1).min(last);
                if t > 0.0 && next != j { vec![(j, 1.0 - t), (next, t)] } else { vec![(j, 1.0)] }
            }
        })
        .collect()
}

/// The network's input for an RGB8 photo: the photo resampled to [`NET_WIDTH`] × [`NET_HEIGHT`],
/// as planar float32 in **BGR** order (channels blue, green, red, each 0 to 1), the order the
/// model was trained and is served with (PaddleX reads images as BGR for it).
pub fn network_input(rgb: &[u8], width: u32, height: u32) -> Result<Vec<f32>, OcrError> {
    let (w, h) = checked_len(rgb, width, height)?;
    let (xs, ys) = (axis_weights(w, NET_WIDTH), axis_weights(h, NET_HEIGHT));
    let plane = NET_WIDTH * NET_HEIGHT;
    let mut out = vec![0.0f32; plane * 3];
    for (oy, wy) in ys.iter().enumerate() {
        for (ox, wx) in xs.iter().enumerate() {
            let mut sum = [0.0f32; 3];
            for &(sy, ky) in wy {
                for &(sx, kx) in wx {
                    let i = (sy * w + sx) * 3;
                    if let Some(p) = rgb.get(i..i + 3) {
                        let k = ky * kx;
                        for (s, &v) in sum.iter_mut().zip(p) {
                            *s += k * f32::from(v);
                        }
                    }
                }
            }
            let at = oy * NET_WIDTH + ox;
            // Blue, green, red planes.
            for (plane_index, channel) in [2usize, 1, 0].into_iter().enumerate() {
                if let (Some(slot), Some(&v)) = (out.get_mut(plane_index * plane + at), sum.get(channel)) {
                    *slot = (v / 255.0).clamp(0.0, 1.0);
                }
            }
        }
    }
    Ok(out)
}

/// Resample an RGB8 photo through a backward map: the flat page, RGB8 at the photo's size. Map
/// points outside the photo come out white. A map with any coordinate that is not a finite number
/// (NaN or infinite) is an error, checked before anything is resampled.
pub fn unwarp(rgb: &[u8], width: u32, height: u32, grid: &Grid) -> Result<Vec<u8>, OcrError> {
    let (w, h) = checked_len(rgb, width, height)?;
    let (gw, gh) = (grid.width, grid.height);
    if gw < 2 || gh < 2 || gw.checked_mul(gh) != Some(grid.xy.len()) {
        return Err(OcrError::Flatten("the flattening map is malformed".into()));
    }
    if !grid.xy.iter().flatten().all(|v| v.is_finite()) {
        return Err(OcrError::Flatten("the flattening model returned a map with coordinates that are not numbers".into()));
    }
    // Where each output column and row falls in the coarse map (corners aligned).
    let place = |i: usize, n: usize, g: usize| -> (usize, f32) {
        let pos = if n > 1 { i as f64 * (g - 1) as f64 / (n - 1) as f64 } else { 0.0 };
        let j = (pos.floor() as usize).min(g - 2);
        (j, (pos - j as f64).clamp(0.0, 1.0) as f32)
    };
    let cols: Vec<(usize, f32)> = (0..w).map(|x| place(x, w, gw)).collect();
    let at = |x: usize, y: usize| grid.xy.get(y * gw + x).copied().unwrap_or([f32::NAN; 2]);
    let pixel = |x: isize, y: isize| -> [f32; 3] {
        if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
            return [255.0; 3];
        }
        let i = (y as usize * w + x as usize) * 3;
        match rgb.get(i..i + 3) {
            Some(p) => [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])],
            None => [255.0; 3],
        }
    };
    let (xmax, ymax) = ((w - 1) as f32, (h - 1) as f32);
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let (gy, ty) = place(y, h, gh);
        for &(gx, tx) in &cols {
            let lerp = |a: [f32; 2], b: [f32; 2], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            let top = lerp(at(gx, gy), at(gx + 1, gy), tx);
            let bottom = lerp(at(gx, gy + 1), at(gx + 1, gy + 1), tx);
            let [u, v] = lerp(top, bottom, ty);
            // Grid-sample coordinates to pixels, corners aligned.
            let (sx, sy) = ((u + 1.0) * 0.5 * xmax, (v + 1.0) * 0.5 * ymax);
            // Far outside: white. The bound also keeps the casts below exact (and would catch a
            // non-number, which the check above already rejects).
            if !(sx > -2.0 && sy > -2.0 && sx < xmax + 2.0 && sy < ymax + 2.0) {
                out.extend_from_slice(&[255; 3]);
                continue;
            }
            let (x0, y0) = (sx.floor(), sy.floor());
            let (ax, ay) = (sx - x0, sy - y0);
            let (x0, y0) = (x0 as isize, y0 as isize);
            let (p00, p10, p01, p11) = (pixel(x0, y0), pixel(x0 + 1, y0), pixel(x0, y0 + 1), pixel(x0 + 1, y0 + 1));
            for c in 0..3 {
                let top = p00[c] + (p10[c] - p00[c]) * ax;
                let bottom = p01[c] + (p11[c] - p01[c]) * ax;
                out.push((top + (bottom - top) * ay).round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity map: every flat point where it is in the photo.
    fn identity(gw: usize, gh: usize) -> Grid {
        let xy =
            (0..gh).flat_map(|y| (0..gw).map(move |x| [x as f32 / (gw - 1) as f32 * 2.0 - 1.0, y as f32 / (gh - 1) as f32 * 2.0 - 1.0])).collect();
        Grid { width: gw, height: gh, xy }
    }

    fn gradient(w: u32, h: u32) -> Vec<u8> {
        (0..h).flat_map(|y| (0..w).flat_map(move |x| [(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 77])).collect()
    }

    #[test]
    fn the_identity_map_gives_the_photo_back() {
        let src = gradient(37, 23);
        assert_eq!(unwarp(&src, 37, 23, &identity(31, 45)).unwrap(), src);
        assert_eq!(unwarp(&src, 37, 23, &identity(2, 2)).unwrap(), src);
    }

    #[test]
    fn a_shifted_map_moves_the_picture_and_fills_white() {
        // Every flat point reads 10 pixels further right in a 21-pixel-wide photo.
        let src = gradient(21, 5);
        let mut g = identity(3, 3);
        for p in &mut g.xy {
            p[0] += 1.0;
        }
        let out = unwarp(&src, 21, 5, &g).unwrap();
        let px = |x: usize, y: usize| &out[(y * 21 + x) * 3..][..3];
        assert_eq!(px(0, 2), &src[(2 * 21 + 10) * 3..][..3]);
        assert_eq!(px(20, 2), &[255, 255, 255], "beyond the photo is white");
    }

    #[test]
    fn hostile_maps_and_pictures_are_errors_or_white_not_panics() {
        let src = gradient(4, 4);
        // Huge but finite values point far outside the photo: white.
        for far in [1e30, -1e30, f32::MAX, f32::MIN] {
            let g = Grid { width: 2, height: 2, xy: vec![[far, far]; 4] };
            assert!(unwarp(&src, 4, 4, &g).unwrap().iter().all(|&v| v == 255), "{far}");
        }
        // Not numbers and infinities are a failed model: an error, not a white page, even when
        // only one coordinate of an otherwise good map is bad.
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let g = Grid { width: 2, height: 2, xy: vec![[bad, bad]; 4] };
            assert!(matches!(unwarp(&src, 4, 4, &g), Err(OcrError::Flatten(_))), "{bad}");
            for at in 0..8 {
                let mut g = identity(2, 2);
                g.xy[at / 2][at % 2] = bad;
                assert!(matches!(unwarp(&src, 4, 4, &g), Err(OcrError::Flatten(_))), "{bad} at {at}");
            }
        }
        // Malformed maps.
        assert!(unwarp(&src, 4, 4, &Grid { width: 2, height: 2, xy: vec![[0.0; 2]; 3] }).is_err());
        assert!(unwarp(&src, 4, 4, &Grid { width: 1, height: 4, xy: vec![[0.0; 2]; 4] }).is_err());
        assert!(unwarp(&src, 4, 4, &Grid { width: usize::MAX, height: 2, xy: vec![] }).is_err());
        // Pictures that are empty, don't match their size, or are too large.
        let g = identity(2, 2);
        assert!(matches!(unwarp(&[], 0, 0, &g), Err(OcrError::EmptyImage)));
        assert!(matches!(unwarp(&[], 0, 7, &g), Err(OcrError::EmptyImage)));
        assert!(unwarp(&src, 5, 4, &g).is_err());
        assert!(unwarp(&[], u32::MAX, u32::MAX, &g).is_err());
        assert!(network_input(&[], 0, 0).is_err());
        assert!(network_input(&src, 4, 5).is_err());
        assert!(network_input(&[], u32::MAX, 2).is_err());
        // One pixel is a picture.
        assert_eq!(unwarp(&[9, 8, 7], 1, 1, &g).unwrap(), vec![9, 8, 7]);
        assert_eq!(network_input(&[9, 8, 7], 1, 1).unwrap().len(), NET_WIDTH * NET_HEIGHT * 3);
    }

    #[test]
    fn the_network_reads_blue_green_red_from_zero_to_one() {
        // A flat colour stays that colour at any size, planes in BGR order.
        for (w, h) in [(1, 1), (100, 50), (NET_WIDTH as u32, NET_HEIGHT as u32), (2000, 3000)] {
            let src: Vec<u8> = std::iter::repeat_n([255u8, 51, 0], w as usize * h as usize).flatten().collect();
            let input = network_input(&src, w, h).unwrap();
            let plane = NET_WIDTH * NET_HEIGHT;
            for (k, want) in [0.0f32, 0.2, 1.0].into_iter().enumerate() {
                let p = &input[k * plane..(k + 1) * plane];
                assert!(p.iter().all(|v| (v - want).abs() < 1e-4), "{w}×{h} plane {k}: {} not {want}", p[0]);
            }
        }
    }

    #[test]
    fn shrinking_averages_and_enlarging_interpolates() {
        // Each output sample's weights sum to one, and every input sample is read.
        for (src, dst) in [(1, 488), (2, 488), (487, 488), (488, 488), (489, 488), (1000, 488), (5000, 712)] {
            let ws = axis_weights(src, dst);
            assert_eq!(ws.len(), dst);
            for w in &ws {
                let sum: f32 = w.iter().map(|p| p.1).sum();
                assert!((sum - 1.0).abs() < 1e-4, "{src}→{dst}: {sum}");
                assert!(w.iter().all(|p| p.0 < src));
            }
            let mut seen = vec![false; src];
            for w in &ws {
                for p in w {
                    seen[p.0] = true;
                }
            }
            assert!(seen.iter().all(|&s| s), "{src}→{dst} skips input samples");
        }
    }

    /// End to end, when the model is installed (`cargo xtask models`): a photo of a curled,
    /// tilted page in perspective comes out with level, straight lines of text.
    #[test]
    fn flattening_straightens_a_curled_page() {
        let Some(path) = find_model() else {
            eprintln!("skipped: the flattening model is not installed (cargo xtask models)");
            return;
        };
        let f = Flattener::load(&path).unwrap();
        let (w, h) = (720, 900);
        let photo = pdfcraft_testkit::photo::curled_page(w, h);
        let before = pdfcraft_testkit::photo::line_sharpness(&photo, w, h);
        let flat = f.flatten(&photo, w, h).unwrap();
        assert_eq!(flat.len(), photo.len(), "the photo's size");
        let after = pdfcraft_testkit::photo::line_sharpness(&flat, w, h);
        eprintln!("line sharpness: photo {before:.3}, flattened {after:.3}");
        assert!(after > before + 0.15 && after > 0.5, "photo {before:.3}, flattened {after:.3}");
        // Odd sizes work too: one pixel, and a long thin strip.
        assert_eq!(f.flatten(&[200, 100, 50], 1, 1).unwrap().len(), 3);
        assert_eq!(f.flatten(&vec![128; 3 * 3000], 3000, 1).unwrap().len(), 3 * 3000);
    }

    #[test]
    fn missing_model_message_is_for_end_users() {
        let message = OcrError::NoFlattenModel.to_string();
        assert!(!message.contains("cargo") && message.contains("PDFCRAFT_MODELS"), "{message}");
    }
}
