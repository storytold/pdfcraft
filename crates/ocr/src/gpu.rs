//! Recognize text on the GPU (feature `gpu`, native targets, opt-in with `PDFCRAFT_OCR_GPU=1`).
//!
//! The same pipeline as [`crate::Ocr::recognize`] on the CPU, with ocrs' two model runs on the
//! GPU (`pdfcraft-ml`): ocrs prepares the image, the GPU finds the text pixels, words come from
//! the mask as ocrs finds them, ocrs finds the lines, the lines are padded (`pad_line`), ocrs cuts
//! the line images, the GPU reads them, and the characters get their boxes from their CTC steps
//! as ocrs places them. A self-check against rten decides at load whether the GPU is used. A
//! page too large for the GPU's limits is read on the CPU; a failure of the GPU itself sends that
//! page and every later one back to the CPU.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use pdfcraft_ml::gpu::{Detector, Gpu, GpuError, Recognizer};
use pdfcraft_ml::onnx::Onnx;
use rten_imageproc::{BoundingRect, RectF, RotatedRect, find_contours, min_area_rect, simplify_polygon};
use rten_tensor::NdTensor;
use rten_tensor::prelude::*;

use crate::{ALPHABET, Line, Models, Word, clip, pad_line};

/// ocrs' grey value of black (the padding of line batches).
const BLACK: f32 = -0.5;
/// ocrs' recognition batches: line widths rounded up to this, at most this many lines.
const GROUP_WIDTH: usize = 50;
const GROUP_LINES: usize = 20;

/// Why a page was not read on the GPU.
#[derive(Debug)]
pub(crate) enum Refused {
    /// Only this page: it is beyond the GPU's limits (checked before anything is submitted), or
    /// ocrs refused it. The CPU reads it and the GPU stays in use.
    Page(String),
    /// The GPU itself (a lost device, a validation or out-of-memory error, or a model the
    /// kernels got wrong, which every page would meet): the CPU reads this and every later page.
    Gpu(String),
}

impl From<GpuError> for Refused {
    fn from(e: GpuError) -> Self {
        match e {
            GpuError::TooLarge(_) => Refused::Page(e.to_string()),
            GpuError::NoAdapter(_) | GpuError::Run(_) | GpuError::Model(_) => Refused::Gpu(e.to_string()),
        }
    }
}

pub(crate) struct GpuOcr {
    gpu: Gpu,
    det: Detector,
    rec: Recognizer,
    /// Set after a failure: every later call uses the CPU.
    broken: AtomicBool,
    reason: Mutex<Option<String>>,
}

impl GpuOcr {
    /// The GPU models from the ONNX files of `models`, kept only if they read a synthetic page
    /// and line as rten does.
    pub(crate) fn load(models: &Models, engine: &ocrs::OcrEngine) -> Result<Self, String> {
        let onnx = |p: &Path| -> Result<Onnx, String> {
            if p.extension().and_then(|e| e.to_str()) != Some("onnx") {
                return Err(format!("{} is not an ONNX model", p.display()));
            }
            Onnx::load(p).map_err(|e| e.to_string())
        };
        let gpu = Gpu::new().map_err(|e| e.to_string())?;
        let det = Detector::new(&gpu, &onnx(&models.detection)?).map_err(|e| e.to_string())?;
        let rec = Recognizer::new(&gpu, &onnx(&models.recognition)?).map_err(|e| e.to_string())?;
        if rec.classes() != ALPHABET.chars().count() + 1 {
            return Err(format!("the recognition model reads {} classes", rec.classes()));
        }
        let ocr = Self { gpu, det, rec, broken: AtomicBool::new(false), reason: Mutex::new(None) };
        ocr.self_check(models, engine)?;
        Ok(ocr)
    }

    pub(crate) fn adapter(&self) -> &str {
        self.gpu.adapter()
    }

    /// Whether the GPU is still in use, and why not if it stopped.
    pub(crate) fn status(&self) -> Result<(), String> {
        if self.broken.load(Ordering::Relaxed) {
            return Err(self.reason.lock().ok().and_then(|r| r.clone()).unwrap_or_default());
        }
        Ok(())
    }

    /// Stop using the GPU after `e`.
    pub(crate) fn fail(&self, e: &str) {
        self.broken.store(true, Ordering::Relaxed);
        if let Ok(mut r) = self.reason.lock() {
            r.get_or_insert_with(|| e.to_string());
        }
    }

    /// The GPU against rten on a synthetic page (bars as text) and a synthetic line: the masks
    /// may differ in a few pixels at the threshold, the best classes in a few steps at a tie.
    fn self_check(&self, models: &Models, engine: &ocrs::OcrEngine) -> Result<(), String> {
        let (w, h) = (640usize, 480usize);
        let mut rgb = vec![255u8; w * h * 3];
        for (row, top) in [60usize, 140, 220, 300].into_iter().enumerate() {
            for y in top..top + 24 {
                for x in (40..600).filter(|x| (x / (9 + row)) % 3 != 0) {
                    if let Some(px) = rgb.get_mut((y * w + x) * 3..(y * w + x) * 3 + 3) {
                        px.fill(20);
                    }
                }
            }
        }
        let threshold = engine.detection_threshold();
        let gpu_mask = self.det.detect(&self.gpu, &rgb, w, h, threshold).map_err(|e| e.to_string())?;
        let source = ocrs::ImageSource::from_bytes(&rgb, (w as u32, h as u32)).map_err(|e| e.to_string())?;
        let input = engine.prepare_input(source).map_err(|e| e.to_string())?;
        let cpu = engine.detect_text_pixels(&input).map_err(|e| e.to_string())?;
        let row_words = w.div_ceil(32);
        let mut differ = 0usize;
        for y in 0..h {
            for x in 0..w {
                let g = gpu_mask.get(y * row_words + x / 32).is_some_and(|word| (word >> (x % 32)) & 1 == 1);
                let c = cpu.get([y, x]).is_some_and(|&p| p > threshold);
                differ += usize::from(g != c);
            }
        }
        if differ * 1000 > w * h {
            return Err(format!("detection differs from rten in {differ} of {} pixels", w * h));
        }
        let width = 200;
        let line: Vec<f32> = (0..64 * width).map(|i| if ((i % width) / 7 + (i / width) / 9) % 3 == 0 { BLACK } else { BLACK + 1.0 }).collect();
        let gpu_best = self.rec.recognize(&self.gpu, &[(line.clone(), 1, width)]).map_err(|e| e.to_string())?.best;
        let model = rten::Model::load_file(&models.recognition).map_err(|e| e.to_string())?;
        let x = NdTensor::from_data([1, 1, 64, width], line);
        let out: NdTensor<f32, 3> = model.run_one(x.view().into(), None).map_err(|e| e.to_string())?.try_into().map_err(|e| format!("{e:?}"))?;
        let [steps, _, classes] = out.shape();
        let cpu_best: Vec<u32> = (0..steps)
            .map(|t| {
                (0..classes)
                    .max_by(|&a, &b| out.get([t, 0, a]).copied().unwrap_or(f32::MIN).total_cmp(&out.get([t, 0, b]).copied().unwrap_or(f32::MIN)))
                    .unwrap_or(0) as u32
            })
            .collect();
        let got = gpu_best.first().cloned().unwrap_or_default();
        let same = got.iter().zip(&cpu_best).filter(|(a, b)| a == b).count();
        if got.len() != cpu_best.len() || same * 50 < cpu_best.len() * 49 {
            return Err(format!("recognition agrees with rten in {same} of {} steps", cpu_best.len()));
        }
        Ok(())
    }

    /// Recognise the text of an RGB page (`w` x `h`, 3 bytes a pixel).
    pub(crate) fn recognize(&self, engine: &ocrs::OcrEngine, rgb: &[u8], w: usize, h: usize) -> Result<Vec<Line>, Refused> {
        let source = ocrs::ImageSource::from_bytes(rgb, (w as u32, h as u32)).map_err(|e| Refused::Page(e.to_string()))?;
        let input = engine.prepare_input(source).map_err(|e| Refused::Page(e.to_string()))?;
        let mask = self.det.detect(&self.gpu, rgb, w, h, engine.detection_threshold())?;
        let words = word_rects(&mask, w, h);
        let mut lines = engine.find_text_lines(&input, &words);
        let before: Vec<Option<RectF>> = lines.iter_mut().map(|l| pad_line(l)).collect();
        // ocrs' line images, batched as ocrs batches them.
        let mut groups: std::collections::BTreeMap<usize, Vec<(usize, NdTensor<f32, 2>)>> = Default::default();
        for (i, line) in lines.iter().enumerate().filter(|(_, l)| !l.is_empty()) {
            let img = engine.prepare_recognition_input(&input, line).map_err(|e| Refused::Page(e.to_string()))?;
            groups.entry(img.size(1).next_multiple_of(GROUP_WIDTH)).or_default().push((i, img));
        }
        let (mut batches, mut members) = (Vec::new(), Vec::new());
        for (&width, group) in &groups {
            for chunk in group.chunks(GROUP_LINES) {
                let mut data = vec![BLACK; chunk.len() * 64 * width];
                for (k, (i, img)) in chunk.iter().enumerate() {
                    let lw = img.size(1);
                    for y in 0..64 {
                        for x in 0..lw {
                            if let (Some(dst), Some(&v)) = (data.get_mut((k * 64 + y) * width + x), img.get([y, x])) {
                                *dst = v;
                            }
                        }
                    }
                    members.push((*i, lw, width));
                }
                batches.push((data, chunk.len(), width));
            }
        }
        let best = self.rec.recognize(&self.gpu, &batches)?.best;
        let mut out: Vec<Option<Line>> = vec![None; lines.len()];
        for ((i, lw, width), steps) in members.into_iter().zip(best) {
            let (Some(line), Some(slot)) = (lines.get(i), out.get_mut(i)) else { continue };
            let words = place_words(line, &steps, lw, width, before.get(i).copied().flatten());
            if !words.is_empty() {
                *slot = Some(Line { words });
            }
        }
        Ok(out.into_iter().flatten().collect())
    }
}

/// The words of a binary mask (rows of `w.div_ceil(32)` bit words): ocrs'
/// `find_connected_component_rects` (ocrs 0.13.1, src/detection.rs, MIT OR Apache-2.0) with
/// ocrs' parameters: contours, simplified, minimum-area rectangle grown by 3 pixels, area ≥ 100.
fn word_rects(mask: &[u32], w: usize, h: usize) -> Vec<RotatedRect> {
    let row_words = w.div_ceil(32);
    let bits: Vec<bool> =
        (0..h * w).map(|i| mask.get((i / w) * row_words + (i % w) / 32).is_some_and(|word| (word >> ((i % w) % 32)) & 1 == 1)).collect();
    let mask = NdTensor::from_data([h, w], bits);
    find_contours(mask.view(), rten_imageproc::RetrievalMode::External)
        .iter()
        .filter_map(|poly| {
            let points: Vec<_> = poly.iter().map(|p| p.to_f32()).collect();
            min_area_rect(&simplify_polygon(&points, 2.0)).map(|mut r| {
                r.resize(r.width() + 6.0, r.height() + 6.0);
                r
            })
        })
        .filter(|r| r.area() >= 100.0)
        .collect()
}

/// Words with boxes from a line's best class per step, as ocrs places them: each character
/// spans from its step to the next character's (times the model's downsampling), mapped onto
/// the line's box; characters in the padding past the line's own width are dropped; words are
/// the runs between spaces, clipped to the unpadded line (`before`).
fn place_words(line: &[RotatedRect], steps: &[u32], line_width: usize, batch_width: usize, before: Option<RectF>) -> Vec<Word> {
    let Some(rect) = rten_imageproc::bounding_rect(line.iter()) else { return Vec::new() };
    let (left, top, right, bottom) = (rect.left().floor(), rect.top().floor(), rect.right().ceil(), rect.bottom().ceil());
    let x_scale = (right - left) / line_width.max(1) as f32;
    let down = (batch_width as f32 / steps.len().max(1) as f32).round().max(1.0) as usize;
    let alphabet: Vec<char> = ALPHABET.chars().collect();
    let mut chars: Vec<(char, usize)> = Vec::new();
    let mut prev = 0u32;
    for (t, &c) in steps.iter().enumerate() {
        if c != 0
            && c != prev
            && let Some(&ch) = alphabet.get(c as usize - 1)
        {
            chars.push((ch, t));
        }
        prev = c;
    }
    let mut words = Vec::new();
    let mut current: Option<(String, [f32; 4])> = None;
    for (k, &(ch, pos)) in chars.iter().enumerate() {
        let start = pos * down;
        let end = chars.get(k + 1).map_or(line_width, |n| n.1 * down);
        let (x0, x1) = (left + (start as f32 * x_scale).floor(), left + (end as f32 * x_scale).floor());
        if x0 >= right {
            continue;
        }
        if ch == ' ' {
            words.extend(current.take());
            continue;
        }
        // The height of the line where the character is (as ocrs slices the line's polygon): the
        // word boxes the character's span overlaps.
        let (mut t, mut b) = (f32::MAX, f32::MIN);
        for wr in line.iter().map(|r| r.bounding_rect()) {
            if wr.right() >= x0 && wr.left() <= x1 {
                t = t.min(wr.top().floor());
                b = b.max(wr.bottom().ceil());
            }
        }
        let (t, b) = if t <= b { (t, b) } else { (top, bottom) };
        let r = [x0, t, x1.max(x0 + 1.0), b];
        match current.as_mut() {
            Some((text, b)) => {
                text.push(ch);
                b[1] = b[1].min(r[1]);
                b[2] = b[2].max(r[2]);
                b[3] = b[3].max(r[3]);
            }
            None => current = Some((ch.to_string(), r)),
        }
    }
    words.extend(current);
    words.into_iter().map(|(text, r)| Word { text, rect: clip(r, before) }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rten_imageproc::RectF;

    /// A page beyond the GPU's limits is read on the CPU without turning the GPU off; a failure
    /// of the GPU itself turns it off.
    #[test]
    fn only_a_failing_gpu_turns_the_gpu_off() {
        assert!(matches!(Refused::from(GpuError::TooLarge("a 5000000000-byte buffer".into())), Refused::Page(_)));
        for e in [GpuError::Run("device lost".into()), GpuError::Model("detector output".into()), GpuError::NoAdapter("none".into())] {
            assert!(matches!(Refused::from(e), Refused::Gpu(_)));
        }
    }

    /// Characters take boxes from their steps, words split at spaces, padding is dropped.
    #[test]
    fn words_are_placed_from_their_steps() {
        let line = [RotatedRect::from_rect(RectF::from_tlbr(10.0, 100.0, 50.0, 500.0))];
        let idx = |c: char| ALPHABET.chars().position(|a| a == c).map_or(0, |p| p as u32 + 1);
        // "ab cd" over 100 steps of a 400-pixel line read at width 400 (downsampling 4).
        let mut steps = vec![0u32; 101];
        for (t, c) in [(5, 'a'), (15, 'b'), (30, ' '), (40, 'c'), (50, 'd'), (100, 'x')] {
            steps[t] = idx(c);
        }
        let words = place_words(&line, &steps, 380, 400, None);
        let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["ab", "cd"], "the x in the padding is dropped");
        assert!(words[0].rect[0] < words[1].rect[0] && words[0].rect[2] <= words[1].rect[0]);
        assert!(words.iter().all(|w| w.rect[1] == 10.0 && w.rect[3] == 50.0));
        assert!(place_words(&[], &steps, 380, 400, None).is_empty());
        assert!(place_words(&line, &[], 380, 400, None).is_empty());
    }

    #[test]
    fn word_rects_of_an_empty_mask_are_none() {
        assert!(word_rects(&[0; 4], 64, 2).is_empty());
        assert!(word_rects(&[], 0, 0).is_empty());
    }
}
