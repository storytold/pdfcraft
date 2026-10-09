//! PP-OCRv4 Chinese/English recognition, entirely on the local RTen runtime.
//! The detector and reading-order analysis are shared with the English recogniser.

use std::io::Read;
use std::path::Path;

use rten_tensor::{NdTensor, Tensor, prelude::*};

use crate::OcrError;

const HEIGHT: usize = 48;
const MAX_WIDTH: usize = 4096;

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

    pub fn recognize(&self, rgb: &[u8], width: u32, height: u32, rect: [f32; 4]) -> Result<String, OcrError> {
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
            return Ok(String::new());
        }
        let scaled = (cw as f64 * HEIGHT as f64 / ch as f64).ceil();
        if scaled > MAX_WIDTH as f64 {
            return Err(OcrError::Recognize("text line exceeds the recogniser's maximum aspect ratio".into()));
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
                for c in 0..3 {
                    let sample = |xx: usize, yy: usize| {
                        let offset = (yy.min(bottom - 1) * width as usize + xx.min(right - 1)) * 3 + c;
                        rgb.get(offset).copied().unwrap_or(255) as f32
                    };
                    let a = sample(x0, y0) * (1.0 - fx) + sample(x0 + 1, y0) * fx;
                    let b = sample(x0, y0 + 1) * (1.0 - fx) + sample(x0 + 1, y0 + 1) * fx;
                    data[c * plane + y * tensor_width + x] = (a * (1.0 - fy) + b * fy) / 127.5 - 1.0;
                }
            }
        }
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
}
