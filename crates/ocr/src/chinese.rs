//! PP-OCRv4 Chinese/English recognition, entirely on the local RTen runtime.
//! The detector and reading-order analysis are shared with the English recogniser.

use std::io::Read;
use std::path::Path;

use rten_tensor::{NdTensor, Tensor, prelude::*};

use crate::OcrError;

const HEIGHT: usize = 48;
const MAX_WIDTH: usize = 4096;

/// The model files, as named in ATTRIBUTION.toml.
pub const CHINESE_MODEL: &str = "chinese-recognition.onnx";
pub const CHINESE_DICTIONARY: &str = "chinese-keys.txt";

pub(crate) struct Chinese {
    model: rten::Model,
    alphabet: Vec<char>,
}

impl Chinese {
    pub fn load(model: &Path, dictionary: &Path) -> Result<Self, OcrError> {
        let load = |p: &Path, e: &dyn std::fmt::Display| OcrError::Load(p.display().to_string(), e.to_string());
        let mut bytes = Vec::new();
        std::fs::File::open(dictionary)
            .map_err(|e| load(dictionary, &e))?
            .take(128 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| load(dictionary, &e))?;
        if bytes.len() > 128 * 1024 {
            return Err(load(dictionary, &"character dictionary exceeds 128 KiB"));
        }
        let text = std::str::from_utf8(&bytes).map_err(|e| load(dictionary, &e))?;
        let mut alphabet = vec!['\0']; // CTC blank, not a printable character.
        for line in text.lines() {
            let mut chars = line.chars();
            let c = chars.next().filter(|c| !c.is_control()).ok_or_else(|| load(dictionary, &"invalid character entry"))?;
            if chars.next().is_some() {
                return Err(load(dictionary, &"dictionary entries must contain one character"));
            }
            alphabet.push(c);
        }
        alphabet.push(' '); // PP-OCR's use_space_char option.
        if alphabet.len() != 6625 {
            return Err(load(dictionary, &"PP-OCRv4 expects 6623 dictionary characters"));
        }
        let model = rten::Model::load_file(model).map_err(|e| load(model, &e))?;
        Ok(Self { model, alphabet })
    }

    /// Recognise one line, skipping boxes whose aspect ratio exceeds the recogniser's cap
    /// (`Ok(None)`) so one oversized decorative rule cannot fail an entire page.
    pub(crate) fn recognize_line(&self, rgb: &[u8], width: u32, height: u32, rect: [f32; 4]) -> Result<Option<String>, OcrError> {
        let Some((tensor_width, data)) = prepare_line_tensor(rgb, width, height, rect)? else {
            return Ok(None);
        };
        self.infer(tensor_width, data).map(Some)
    }

    fn infer(&self, tensor_width: usize, data: Vec<f32>) -> Result<String, OcrError> {
        let input = NdTensor::from_data([1, 3, HEIGHT, tensor_width], data);
        let output: Tensor<f32> = self
            .model
            .run_one(input.into(), None)
            .map_err(|e| OcrError::Recognize(e.to_string()))?
            .try_into()
            .map_err(|e| OcrError::Recognize(format!("unexpected recognition output: {e}")))?;
        let shape = output.shape();
        if shape.len() != 3 || shape.first() != Some(&1) || shape.get(2) != Some(&self.alphabet.len()) {
            return Err(OcrError::Recognize("PP-OCRv4 output has an unexpected shape".into()));
        }
        let values: Vec<f32> = output.iter().copied().collect();
        decode(&values, &self.alphabet)
    }
}

/// Crop, bilinearly resize to height 48, pad to at least width 320, and normalize as BGR planes
/// (`[-1, 1]`) for PP-OCRv4. Returns `Ok(None)` when the crop is empty or exceeds `MAX_WIDTH`.
fn prepare_line_tensor(rgb: &[u8], width: u32, height: u32, rect: [f32; 4]) -> Result<Option<(usize, Vec<f32>)>, OcrError> {
    let [left, top, right, bottom] = rect;
    if !rect.iter().all(|x| x.is_finite()) {
        return Err(OcrError::Recognize("non-finite text box".into()));
    }
    let left = (left.floor() - 2.0).max(0.0) as usize;
    let top = (top.floor() - 2.0).max(0.0) as usize;
    let right = (right.ceil() + 2.0).clamp(0.0, width as f32) as usize;
    let bottom = (bottom.ceil() + 2.0).clamp(0.0, height as f32) as usize;
    let (cw, ch) = (right.saturating_sub(left), bottom.saturating_sub(top));
    if cw == 0 || ch == 0 {
        return Ok(None);
    }
    let scaled = (cw as f64 * HEIGHT as f64 / ch as f64).ceil();
    if scaled > MAX_WIDTH as f64 {
        return Ok(None);
    }
    let resized = (scaled as usize).max(1);
    let tensor_width = resized.max(320);
    let plane = HEIGHT * tensor_width;
    let mut data = vec![0.0; 3 * plane];
    for y in 0..HEIGHT {
        let sy = top as f64 + ((y as f64 + 0.5) * ch as f64 / HEIGHT as f64 - 0.5).max(0.0);
        for x in 0..resized {
            let sx = left as f64 + ((x as f64 + 0.5) * cw as f64 / resized as f64 - 0.5).max(0.0);
            let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
            let (fx, fy) = ((sx - x0 as f64) as f32, (sy - y0 as f64) as f32);
            // PP-OCRv4 / RapidOCR expects BGR plane order rather than RGB.
            for c in 0..3 {
                let sample = |xx: usize, yy: usize| {
                    let offset = (yy.min(bottom - 1) * width as usize + xx.min(right - 1)) * 3 + (2 - c);
                    rgb.get(offset).copied().unwrap_or(255) as f32
                };
                let a = sample(x0, y0) * (1.0 - fx) + sample(x0 + 1, y0) * fx;
                let b = sample(x0, y0 + 1) * (1.0 - fx) + sample(x0 + 1, y0 + 1) * fx;
                data[c * plane + y * tensor_width + x] = (a * (1.0 - fy) + b * fy) / 127.5 - 1.0;
            }
        }
    }
    Ok(Some((tensor_width, data)))
}

fn decode(values: &[f32], alphabet: &[char]) -> Result<String, OcrError> {
    if alphabet.is_empty() || !values.len().is_multiple_of(alphabet.len()) {
        return Err(OcrError::Recognize("invalid CTC output".into()));
    }
    let mut text = String::new();
    let mut previous = 0;
    for row in values.chunks_exact(alphabet.len()) {
        if !row.iter().all(|v| v.is_finite()) {
            return Err(OcrError::Recognize("non-finite recognition scores".into()));
        }
        let best = row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i);
        if best != 0
            && best != previous
            && let Some(c) = alphabet.get(best)
        {
            text.push(*c);
        }
        previous = best;
    }
    Ok(text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctc_keeps_chinese_and_repetitions_separated_by_blank() {
        let alphabet = ['\0', '中', '文', 'A'];
        let mut scores = Vec::new();
        for i in [1, 1, 0, 1, 2, 0, 3] {
            let mut row = [0.0; 4];
            row[i] = 1.0;
            scores.extend(row);
        }
        assert_eq!(decode(&scores, &alphabet).unwrap(), "中中文A");
        assert!(decode(&[f32::NAN; 4], &alphabet).is_err());
    }

    #[test]
    fn line_tensor_uses_bgr_planes_and_caps_oversized_aspect_ratio() {
        // Pure red pixel (R=255, G=0, B=0) in RGB input must map to B=-1, G=-1, R=+1.
        let rgb = [255u8, 0, 0].repeat(16 * 16);
        let (tensor_width, data) = prepare_line_tensor(&rgb, 16, 16, [2.0, 2.0, 14.0, 14.0]).unwrap().unwrap();
        let plane = HEIGHT * tensor_width;
        assert!((data[0] - (-1.0)).abs() < 1e-6, "plane 0 must be B (-1.0), got {}", data[0]);
        assert!((data[plane] - (-1.0)).abs() < 1e-6, "plane 1 must be G (-1.0), got {}", data[plane]);
        assert!((data[2 * plane] - 1.0).abs() < 1e-6, "plane 2 must be R (+1.0), got {}", data[2 * plane]);
        // An oversized line skips cleanly so page-level recognition can continue.
        let wide = [0.0, 0.0, 5000.0, 10.0];
        assert!(prepare_line_tensor(&rgb, 5000, 16, wide).unwrap().is_none());
    }
}
