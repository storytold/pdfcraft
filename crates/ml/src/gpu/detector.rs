//! The ocrs text detection model on the GPU, from the page's RGB bytes to a thresholded mask of
//! text pixels at page size, as ocrs' `detect_words` computes it up to the mask: ocrs' grey
//! values, padded with black to at least the model size and resized to it (as rten's
//! `resize_image`: bilinear, half-pixel, clamped), the model, its output cut to the unpadded part,
//! resized back to the page and thresholded.

use super::graph::{Graph, Tensor, count};
use super::{Gpu, GpuError, params, to_u32};
use crate::onnx::Onnx;

/// The ocrs detection model's input size.
const MODEL_H: usize = 800;
const MODEL_W: usize = 600;

pub struct Detector {
    graph: Graph,
}

impl Detector {
    pub fn new(gpu: &Gpu, onnx: &Onnx) -> Result<Self, GpuError> {
        Ok(Self { graph: Graph::build(gpu, onnx, None)?.0 })
    }

    /// The text mask of an RGB page (`w` x `h`, 3 bytes a pixel): one row of `w.div_ceil(32)`
    /// words per page row, a bit per pixel, set where the text probability exceeds `threshold`.
    pub fn detect(&self, gpu: &Gpu, rgb: &[u8], w: usize, h: usize, threshold: f32) -> Result<Vec<u32>, GpuError> {
        if w == 0 || h == 0 || Some(rgb.len()) != count(&[w, h, 3]).ok() {
            return Err(GpuError::Model(format!("{} bytes for a {w} x {h} RGB page", rgb.len())));
        }
        gpu.scoped(|| {
            let (ph, pw) = (h.max(MODEL_H), w.max(MODEL_W));
            let src = gpu.upload_bytes(rgb)?;
            let mut scratch = Vec::new();
            let mut enc = gpu.encoder();
            let input = gpu.storage(MODEL_H * MODEL_W)?;
            scratch.push(input.clone());
            let bits = |v: f32| v.to_bits() as usize;
            let p = gpu.uniform(&params(&[w, h, pw, ph, MODEL_W, MODEL_H, bits(pw as f32 / MODEL_W as f32), bits(ph as f32 / MODEL_H as f32)])?)?;
            gpu.elementwise(&mut enc, "grey_resize", &[&src, &input, &p], MODEL_H * MODEL_W)?;
            let mask = self.graph.run(gpu, &mut enc, Tensor { buf: input, shape: [1, 1, MODEL_H, MODEL_W] }, &mut scratch)?;
            if mask.shape != [1, 1, MODEL_H, MODEL_W] {
                return Err(GpuError::Model(format!("detector output {:?}", mask.shape)));
            }
            // The part of the mask that covers the page, not the padding.
            let (vh, vw) = (MODEL_H - (ph - h) * MODEL_H / ph, MODEL_W - (pw - w) * MODEL_W / pw);
            let row_words = w.div_ceil(32);
            let out = count(&[row_words, h])?;
            let packed = gpu.storage(out)?;
            scratch.push(packed.clone());
            let p2 = gpu.uniform(&params(&[
                MODEL_W,
                vw,
                vh,
                w,
                h,
                row_words,
                bits(vw as f32 / w as f32),
                bits(vh as f32 / h as f32),
                bits(threshold),
                0,
                0,
                0,
            ])?)?;
            gpu.elementwise(&mut enc, "mask_up", &[&mask.buf, &packed, &p2], out)?;
            let _ = to_u32(out)?;
            let result = gpu.read(enc, &packed, out);
            gpu.recycle(scratch);
            result
        })
    }
}
