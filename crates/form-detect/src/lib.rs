//! FFDetr inference, independent of PDF parsing and editing. Inputs are page images; outputs
//! are field proposals in normalized, top-left coordinates. The engine owns page transforms,
//! labels, existing-field filtering and edits. No downloads happen here.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use rten::{Dimension, Model, NodeId, RunOptions};
use rten_tensor::{AsView, Layout, NdTensor, TensorView};
use sha2::{Digest, Sha256};

#[cfg(feature = "test-model")]
pub mod test_model;

/// The first supported export deliberately matches the tested CommonForms CPU configuration.
pub const IMAGE_SIZE: usize = 576;
pub const MODEL_FILE: &str = "ffdetr-576.onnx";
/// The self-contained export verified against the Python reference; see the crate README.
pub const MODEL_SHA256: &str = "5e5920c8cbf7287b9fef4d67c0d20728a7155e884ae638ee75926b1bbae79370";
const MAX_MODEL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PIXELS: usize = 12_000_000;
const MAX_QUERIES: usize = 1000;
const MAX_CLASSES: usize = 256;
const NMS_IOU: f32 = 0.1;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read form detection model: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid form detection model: {0}")]
    Model(String),
    #[error("form field detection failed: {0}")]
    Inference(String),
    #[error("invalid page image: {0}")]
    Image(&'static str),
    #[error("confidence must be a finite number between 0 and 1")]
    Confidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    CheckBox,
    Signature,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub kind: Kind,
    /// [left, top, right, bottom], normalized to [0, 1] in the displayed page image.
    pub rect: [f32; 4],
    pub confidence: f32,
}

/// A reusable, CPU-only model. The caller should retain one instance between jobs.
pub struct Detector {
    model: Model,
    input: NodeId,
    boxes: NodeId,
    logits: NodeId,
    threads: Arc<rten::ThreadPool>,
}

impl Detector {
    /// Load a self-contained ONNX export. External tensor paths are deliberately unsupported.
    pub fn load(path: &Path) -> Result<Self, Error> {
        if !std::fs::metadata(path)?.is_file() {
            return Err(Error::Model("expected a regular model file".into()));
        }
        let file = std::fs::File::open(path)?;
        if file.metadata()?.len() > MAX_MODEL_BYTES {
            return Err(Error::Model("file exceeds 256 MiB".into()));
        }
        let mut bytes = Vec::new();
        file.take(MAX_MODEL_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_MODEL_BYTES {
            return Err(Error::Model("file exceeds 256 MiB".into()));
        }
        // Tool arguments may name arbitrary files. Only approved graphs may execute: a tiny
        // malicious ONNX graph could otherwise request enormous intermediate allocations.
        let digest = Sha256::digest(&bytes);
        let hash: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let approved = hash == MODEL_SHA256;
        #[cfg(feature = "test-model")]
        let approved = approved || digest == Sha256::digest(test_model::bytes());
        if !approved {
            return Err(Error::Model(format!("unrecognized model SHA-256 {hash}; expected the approved FFDetr 576px export ({MODEL_SHA256})")));
        }
        let model = Model::load(bytes).map_err(|e| Error::Model(e.to_string()))?;
        let input = model.node_id("input").map_err(|e| Error::Model(e.to_string()))?;
        let boxes = model.node_id("dets").map_err(|e| Error::Model(e.to_string()))?;
        let logits = model.node_id("labels").map_err(|e| Error::Model(e.to_string()))?;
        let shape = model.node_info(input).and_then(|n| n.shape());
        let expected = [1, 3, IMAGE_SIZE, IMAGE_SIZE].map(Dimension::Fixed);
        if model.input_ids() != [input] || shape.as_deref() != Some(expected.as_slice()) {
            return Err(Error::Model("expected one float input of shape [1, 3, 576, 576]".into()));
        }
        let count = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 2);
        Ok(Self { model, input, boxes, logits, threads: Arc::new(rten::ThreadPool::with_num_threads(count)) })
    }

    /// Detect fields from premultiplied RGBA8 pixels. Transparent pixels are composited onto
    /// white. Invalid buffers and oversized images return errors before allocating a tensor.
    pub fn detect(&self, rgba: &[u8], width: u32, height: u32, confidence: f32) -> Result<Vec<Proposal>, Error> {
        validate_confidence(confidence)?;
        let input = preprocess(rgba, width, height)?;
        let mut options = RunOptions::default();
        options.thread_pool = Some(self.threads.clone());
        let outputs = self
            .model
            .run(vec![(self.input, input.view().into())], &[self.boxes, self.logits], Some(options))
            .map_err(|e| Error::Inference(e.to_string()))?;
        let boxes = outputs.first().and_then(|v| v.as_tensor_view::<f32>()).ok_or_else(|| Error::Inference("missing float box output".into()))?;
        let logits = outputs.get(1).and_then(|v| v.as_tensor_view::<f32>()).ok_or_else(|| Error::Inference("missing float class output".into()))?;
        decode(boxes, logits, confidence)
    }
}

pub fn validate_confidence(value: f32) -> Result<(), Error> {
    if value.is_finite() && (0.0..=1.0).contains(&value) { Ok(()) } else { Err(Error::Confidence) }
}

fn preprocess(rgba: &[u8], width: u32, height: u32) -> Result<NdTensor<f32, 4>, Error> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .filter(|n| *n > 0 && *n <= MAX_PIXELS)
        .ok_or(Error::Image("expected 1 to 12 million pixels"))?;
    if pixels.checked_mul(4) != Some(rgba.len()) {
        return Err(Error::Image("RGBA buffer length does not match width and height"));
    }
    let (width, height) = (width as usize, height as usize);
    let sample = |x: usize, y: usize, channel: usize| -> Result<f32, Error> {
        let base = y.checked_mul(width).and_then(|v| v.checked_add(x)).and_then(|v| v.checked_mul(4)).ok_or(Error::Image("pixel offset overflow"))?;
        let pixel = rgba.get(base..base.saturating_add(4)).ok_or(Error::Image("pixel outside image"))?;
        // The slice above has exactly four components, and channels are 0..3 below.
        Ok(f32::from(pixel[channel].saturating_add(255 - pixel[3])) / 255.0)
    };
    let plane = IMAGE_SIZE * IMAGE_SIZE;
    let mut data = vec![0.0; plane * 3];
    // Match the reference tensor Resize: bilinear, half-pixel centres, align_corners=false,
    // antialias=false. An antialiased image resize measurably changes field predictions.
    for y in 0..IMAGE_SIZE {
        let sy = (((y as f64 + 0.5) * height as f64 / IMAGE_SIZE as f64 - 0.5).max(0.0)) as f32;
        let y0 = (sy.floor() as usize).min(height - 1);
        let y1 = y0.saturating_add(1).min(height - 1);
        let fy = sy - y0 as f32;
        for x in 0..IMAGE_SIZE {
            let sx = (((x as f64 + 0.5) * width as f64 / IMAGE_SIZE as f64 - 0.5).max(0.0)) as f32;
            let x0 = (sx.floor() as usize).min(width - 1);
            let x1 = x0.saturating_add(1).min(width - 1);
            let fx = sx - x0 as f32;
            for (channel, (mean, std)) in MEAN.iter().zip(STD).enumerate() {
                let top = (1.0 - fx) * sample(x0, y0, channel)? + fx * sample(x1, y0, channel)?;
                let bottom = (1.0 - fx) * sample(x0, y1, channel)? + fx * sample(x1, y1, channel)?;
                let value = (1.0 - fy) * top + fy * bottom;
                if let Some(slot) = data.get_mut(channel * plane + y * IMAGE_SIZE + x) {
                    *slot = (value - mean) / std;
                }
            }
        }
    }
    // The shape and initialized data length are constants derived together above.
    Ok(NdTensor::from_data([1, 3, IMAGE_SIZE, IMAGE_SIZE], data))
}

fn decode(boxes: TensorView<'_, f32>, logits: TensorView<'_, f32>, threshold: f32) -> Result<Vec<Proposal>, Error> {
    let ([1, queries, 4], [1, class_queries, classes]) = (boxes.shape(), logits.shape()) else {
        return Err(Error::Inference("expected boxes [1, queries, 4] and logits [1, queries, classes]".into()));
    };
    if queries != class_queries || *queries > MAX_QUERIES || !(3..=MAX_CLASSES).contains(classes) {
        return Err(Error::Inference("invalid or oversized detection output".into()));
    }
    let mut proposals = Vec::new();
    for query in 0..*queries {
        let mut best: Option<(usize, f32)> = None;
        for class in 0..*classes {
            let logit = *logits.get([0, query, class]).ok_or_else(|| Error::Inference("missing class score".into()))?;
            if !logit.is_finite() {
                return Err(Error::Inference("non-finite class score".into()));
            }
            if best.is_none_or(|(_, value)| logit > value) {
                best = Some((class, logit));
            }
        }
        let Some((class, logit)) = best else { continue };
        let score = 1.0 / (1.0 + (-logit).exp());
        if score <= threshold {
            continue;
        }
        let kind = match class {
            0 => Kind::Text,
            1 => Kind::CheckBox,
            2 => Kind::Signature,
            _ => continue, // The checkpoint retains unused COCO class slots.
        };
        let values: Vec<f32> = (0..4).filter_map(|i| boxes.get([0, query, i]).copied()).collect();
        let [cx, cy, w, h] = values.as_slice() else { return Err(Error::Inference("missing box coordinates".into())) };
        if values.iter().any(|v| !v.is_finite()) {
            return Err(Error::Inference("non-finite box coordinate".into()));
        }
        if *w <= 0.0 || *h <= 0.0 {
            continue;
        }
        let rect = [(cx - w / 2.0).clamp(0.0, 1.0), (cy - h / 2.0).clamp(0.0, 1.0), (cx + w / 2.0).clamp(0.0, 1.0), (cy + h / 2.0).clamp(0.0, 1.0)];
        if rect[2] > rect[0] && rect[3] > rect[1] {
            proposals.push(Proposal { kind, rect, confidence: score });
        }
    }
    proposals.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    let mut kept: Vec<Proposal> = Vec::new();
    for proposal in proposals {
        // Class-agnostic suppression also prevents a text field and checkbox being stacked.
        if !kept.iter().any(|other| iou(other.rect, proposal.rect) > NMS_IOU) {
            kept.push(proposal);
        }
    }
    Ok(kept)
}

fn iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let intersection = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0) * (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - intersection;
    if union > 0.0 { intersection / union } else { 0.0 }
}

#[cfg(test)]
mod tests;
