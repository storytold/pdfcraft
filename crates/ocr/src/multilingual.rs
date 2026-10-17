//! PaddleOCR's Apache-2.0 PP-OCRv6 recognizer and document orientation classifier.
//! Preprocessing follows the pinned inference.yml files in ATTRIBUTION.toml.
use crate::{OcrError, Word};
use image::{RgbImage, imageops};
use rten_tensor::{NdTensor, Tensor, prelude::*};

pub(crate) struct Multilingual {
    recognition: rten::Model,
    orientation: rten::Model,
    alphabet: Vec<String>,
}

fn error(e: impl std::fmt::Display) -> OcrError {
    OcrError::Recognize(e.to_string())
}

impl Multilingual {
    pub fn load(dir: &std::path::Path) -> Result<Self, OcrError> {
        let load = |name: &str| rten::Model::load_file(dir.join(name)).map_err(|e| OcrError::Load(name.into(), e.to_string()));
        let dictionary_path = dir.join(crate::DICTIONARY);
        if std::fs::metadata(&dictionary_path).map_err(error)?.len() > 1_000_000 {
            return Err(error("recognition dictionary is too large"));
        }
        let dictionary = std::fs::read_to_string(dictionary_path).map_err(error)?;
        let mut alphabet = vec![String::new()]; // CTC blank
        alphabet.extend(dictionary.lines().map(str::to_owned));
        alphabet.push(" ".into());
        Ok(Self { recognition: load(crate::MULTILINGUAL_MODEL)?, orientation: load(crate::ORIENTATION_MODEL)?, alphabet })
    }

    /// Clockwise correction. Low-confidence and nearly blank/solid pages stay put.
    pub fn orientation(&self, image: &RgbImage) -> Result<u16, OcrError> {
        let ink = image.pixels().filter(|p| p.0.iter().copied().map(u32::from).sum::<u32>() < 450).count();
        if ink < 100 || !(0.002..0.95).contains(&(ink as f64 / (image.width() as f64 * image.height() as f64).max(1.0))) {
            return Ok(0);
        }
        // Centre the preprocessing on content. A centre crop of the paper itself can contain
        // only blank margins on sparse letters, especially when scanned sideways.
        let bounds = image
            .enumerate_pixels()
            .filter(|(_, _, p)| p.0.iter().copied().map(u32::from).sum::<u32>() < 450)
            .fold([image.width(), image.height(), 0, 0], |a, (x, y, _)| [a[0].min(x), a[1].min(y), a[2].max(x + 1), a[3].max(y + 1)]);
        let crop = imageops::crop_imm(image, bounds[0], bounds[1], bounds[2] - bounds[0], bounds[3] - bounds[1]).to_image();
        let image = &crop;
        let scale = 256.0 / image.width().min(image.height()).max(1) as f64;
        let w = (image.width() as f64 * scale).round() as u32;
        let h = (image.height() as f64 * scale).round() as u32;
        if w > 8192 || h > 8192 {
            return Ok(0);
        }
        let resized = imageops::resize(image, w, h, imageops::FilterType::Triangle);
        let crop = imageops::crop_imm(&resized, (w - 224) / 2, (h - 224) / 2, 224, 224).to_image();
        let mut input = NdTensor::<f32, 4>::zeros([1, 3, 224, 224]);
        for (x, y, p) in crop.enumerate_pixels() {
            for c in 0..3 {
                input[[0, c, y as usize, x as usize]] = (p[c] as f32 / 255.0 - [0.485, 0.456, 0.406][c]) / [0.229, 0.224, 0.225][c];
            }
        }
        let output: Tensor<f32> = self.orientation.run_one(input.view().into(), None).map_err(error)?.try_into().map_err(error)?;
        let scores: Vec<f32> = output.iter().copied().collect();
        if scores.len() != 4 || scores.iter().any(|v| !v.is_finite()) {
            return Err(error("invalid orientation model output"));
        }
        // The exported model includes softmax; applying it again destroys confidence.
        let best = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1));
        Ok(match best {
            Some((i, &score)) if score >= 0.90 => ((4 - i) % 4 * 90) as u16,
            _ => 0,
        })
    }

    pub fn read_line(&self, image: &RgbImage, rect: [f32; 4]) -> Result<Vec<Word>, OcrError> {
        let [l, t, r, b] = rect;
        let x = l.floor().max(0.0) as u32;
        let y = t.floor().max(0.0) as u32;
        let w = (r.ceil() as u32).min(image.width()).saturating_sub(x);
        let h = (b.ceil() as u32).min(image.height()).saturating_sub(y);
        if w == 0 || h == 0 {
            return Ok(Vec::new());
        }
        let crop = imageops::crop_imm(image, x, y, w, h).to_image();
        let rw = ((48.0 * w as f64 / h as f64).ceil() as u32).clamp(1, 3200);
        let width = rw.max(320) as usize;
        let resized = imageops::resize(&crop, rw, 48, imageops::FilterType::Triangle);
        let mut input = NdTensor::<f32, 4>::zeros([1, 3, 48, width]);
        // Paddle's recognizer takes BGR in [-1,1], with zero right-padding.
        for (x, y, p) in resized.enumerate_pixels() {
            for c in 0..3 {
                input[[0, c, y as usize, x as usize]] = p[2 - c] as f32 / 127.5 - 1.0;
            }
        }
        let output: Tensor<f32> = self.recognition.run_one(input.view().into(), None).map_err(error)?.try_into().map_err(error)?;
        let shape = output.shape();
        if shape.len() != 3 || shape[0] != 1 || shape[2] != self.alphabet.len() {
            return Err(error(format!("invalid recognition model output: {shape:?}, dictionary {}", self.alphabet.len())));
        }
        let steps = shape[1];
        let classes = shape[2];
        let values: Vec<f32> = output.iter().copied().collect();
        let mut words = Vec::new();
        let (mut text, mut previous, mut start, mut end, mut confidence, mut count) = (String::new(), 0, 0, 0, 0.0, 0);
        let scale = width as f32 / rw as f32 * w as f32 / steps.max(1) as f32;
        let flush = |words: &mut Vec<Word>, text: &mut String, start: usize, end: usize, confidence: f32, count: usize| {
            if !text.is_empty() && confidence / count.max(1) as f32 >= 0.5 {
                words.push(Word {
                    text: std::mem::take(text),
                    rect: [x as f32 + start as f32 * scale, y as f32, (x as f32 + (end + 1) as f32 * scale).min(r), (y + h) as f32],
                });
            } else {
                text.clear();
            }
        };
        for (step, row) in values.chunks_exact(classes).enumerate() {
            if step as f32 * scale >= w as f32 {
                break;
            }
            let Some((index, &score)) = row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else { continue };
            if index != 0 && index != previous {
                let Some(ch) = self.alphabet.get(index) else { continue };
                if ch.trim().is_empty() {
                    flush(&mut words, &mut text, start, end, confidence, count);
                    confidence = 0.0;
                    count = 0;
                } else {
                    if text.is_empty() {
                        start = step;
                    }
                    text.push_str(ch);
                    end = step;
                    confidence += score;
                    count += 1;
                }
            }
            if index != 0 && index == previous && !text.is_empty() {
                end = step;
            }
            previous = index;
        }
        flush(&mut words, &mut text, start, end, confidence, count);
        Ok(words)
    }
}
