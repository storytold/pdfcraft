//! PP-OCR recognition (Cyrillic, and the Latin words around it) on the local RTen runtime: CTC
//! decoding of a mobile recognition model over line crops. The detector and the reading-order
//! analysis are shared with the English recogniser (ocrs).

use std::io::Read;
use std::path::Path;

use rten_tensor::{NdTensor, Tensor, prelude::*};

use crate::OcrError;

/// The models' input height; the width follows the crop's own aspect ratio.
const HEIGHT: usize = 48;
/// The widest line the recogniser will read, in input pixels: a crop of a much taller box (a
/// whole page detected as one line) is refused rather than expanded into an enormous tensor.
const MAX_WIDTH: usize = 4096;
/// The width the input is padded to at least; PP-OCR pads short lines the same way.
const MIN_WIDTH: usize = 320;
/// How much white around the line's own box the crop keeps, so a descender or a serif that the
/// detector's box cut off is still read.
const PAD: f32 = 2.0;

pub(crate) struct PpOcr {
    model: rten::Model,
    alphabet: Vec<char>,
}

impl PpOcr {
    /// Load `model` with its `dictionary`. `classes` is the model's output class count
    /// (CTC blank + one per dictionary line + the appended space); a mismatch means the
    /// wrong dictionary is installed, so fail before the model is ever used.
    pub fn load(model: &Path, dictionary: &Path, classes: usize) -> Result<Self, OcrError> {
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
        if alphabet.len() != classes {
            return Err(load(dictionary, &format!("the model has {classes} output classes but the dictionary gives {}", alphabet.len())));
        }
        let model = rten::Model::load_file(model).map_err(|e| load(model, &e))?;
        Ok(Self { model, alphabet })
    }

    /// Read the text in `rect` (image pixels `[left, top, right, bottom]`) of `rgb`.
    pub fn recognize(&self, rgb: &[u8], width: u32, height: u32, rect: [f32; 4]) -> Result<String, OcrError> {
        let values = self.scores(rgb, width, height, rect)?;
        if values.is_empty() {
            return Ok(String::new());
        }
        decode(&values, &self.alphabet)
    }

    /// One score per class for every time step. An empty crop is an empty vector.
    fn scores(&self, rgb: &[u8], width: u32, height: u32, rect: [f32; 4]) -> Result<Vec<f32>, OcrError> {
        let [left, top, right, bottom] = rect;
        if !rect.iter().all(|x| x.is_finite()) {
            return Err(OcrError::Recognize("non-finite text box".into()));
        }
        // The recogniser reads the ink, not the padding around it: a crop that carries the whole
        // line's white margin decodes long stretches of the margin as letters.
        let left = (left.floor() - PAD).max(0.0) as usize;
        let top = (top.floor() - PAD).max(0.0) as usize;
        let right = (right.ceil() + PAD).clamp(0.0, width as f32) as usize;
        let bottom = (bottom.ceil() + PAD).clamp(0.0, height as f32) as usize;
        let (cw, ch) = (right.saturating_sub(left), bottom.saturating_sub(top));
        if cw == 0 || ch == 0 || rgb.len() < (width as usize * height as usize * 3) {
            return Ok(Vec::new());
        }
        let scaled = (cw as f64 * HEIGHT as f64 / ch as f64).ceil();
        if !(scaled >= 1.0 && scaled <= MAX_WIDTH as f64) {
            return Err(OcrError::Recognize("text line exceeds the recogniser's maximum aspect ratio".into()));
        }
        let resized = scaled as usize;
        let tensor_width = resized.max(MIN_WIDTH);
        let plane = HEIGHT * tensor_width;
        let mut data = vec![0.0; 3 * plane];
        for y in 0..HEIGHT {
            let sy = top as f64 + ((y as f64 + 0.5) * ch as f64 / HEIGHT as f64 - 0.5).max(0.0);
            let y0 = (sy.floor() as usize).min(bottom - 1);
            let y1 = (y0 + 1).min(bottom - 1);
            let fy = (sy - y0 as f64).clamp(0.0, 1.0) as f32;
            for x in 0..resized {
                let sx = left as f64 + ((x as f64 + 0.5) * cw as f64 / resized as f64 - 0.5).max(0.0);
                let x0 = (sx.floor() as usize).min(right - 1);
                let x1 = (x0 + 1).min(right - 1);
                let fx = (sx - x0 as f64).clamp(0.0, 1.0) as f32;
                for c in 0..3 {
                    let sample = |xx: usize, yy: usize| rgb.get((yy * width as usize + xx) * 3 + c).copied().unwrap_or(255) as f32;
                    let a = sample(x0, y0) * (1.0 - fx) + sample(x1, y0) * fx;
                    let b = sample(x0, y1) * (1.0 - fx) + sample(x1, y1) * fx;
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
            return Err(OcrError::Recognize("PP-OCR output has an unexpected shape".into()));
        }
        Ok(output.iter().copied().collect())
    }
}

/// Greedy CTC decoding: the best class per time step, with the repeats that the blank separates
/// collapsed, and the blank itself dropped.
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
    fn ctc_keeps_doubled_letters_that_the_blank_separates() {
        // "ааб" with a blank between the two `а`: only that pair is two letters.
        let alphabet = ['\0', 'а', 'б'];
        let mut scores = Vec::new();
        for i in [1, 1, 0, 1, 2] {
            let mut row = [0.0; 3];
            row[i] = 1.0;
            scores.extend(row);
        }
        assert_eq!(decode(&scores, &alphabet).unwrap(), "ааб");
        // Without the blank the repeat is one letter.
        let mut repeated = Vec::new();
        for i in [1, 1, 1] {
            let mut row = [0.0; 3];
            row[i] = 1.0;
            repeated.extend(row);
        }
        assert_eq!(decode(&repeated, &alphabet).unwrap(), "а");
        assert!(decode(&[f32::NAN; 3], &alphabet).is_err());
        assert!(decode(&[1.0, 0.0], &alphabet).is_err(), "a row that doesn't fill the alphabet is not a step");
    }

    /// The East Slavic model's class scores are one softmax row per time step: the decoder reads
    /// them as probabilities, and the crop it is given decides the text. Skipped when the model
    /// has not been fetched.
    #[test]
    fn the_cyrillic_recogniser_emits_a_probability_per_class() {
        let Some(models) = crate::Models::find_for("ru") else {
            eprintln!("skipped: East Slavic OCR models not installed");
            return;
        };
        let ocr = PpOcr::load(&models.recognition, &models.recognition.with_file_name(crate::ESLAV_DICTIONARY), 519).unwrap();
        let (w, h) = (120u32, 64u32);
        let mut rgb = vec![255u8; (w * h * 3) as usize];
        for x in 8..100 {
            for y in 12..40 {
                if x % 7 < 3 {
                    let i = ((y * w + x) * 3) as usize;
                    rgb[i] = 0;
                    rgb[i + 1] = 0;
                    rgb[i + 2] = 0;
                }
            }
        }
        let values = ocr.scores(&rgb, w, h, [0.0, 0.0, w as f32, h as f32]).unwrap();
        assert!(!values.is_empty() && values.len().is_multiple_of(ocr.alphabet.len()), "{}", values.len());
        let row = &values[..ocr.alphabet.len()];
        let sum: f32 = row.iter().sum();
        let max = row.iter().copied().fold(f32::MIN, f32::max);
        assert!((sum - 1.0).abs() < 0.02, "a class row sums to {sum}, max {max}");
        assert!((0.0..=1.0).contains(&max), "{max}");
        // The dictionary is the one the model was trained with, so the letters are Cyrillic.
        assert!(ocr.alphabet.iter().any(|c| ('а'..='я').contains(c)), "no Cyrillic letters in the dictionary");
    }

    #[test]
    fn dictionary_must_match_the_models_class_count() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ppocr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dict = dir.join("keys.txt");
        std::fs::write(&dict, "а\nб\n").unwrap(); // blank + а + б + space = 4 classes.
        // The dictionary is validated before the model is opened, so a missing model is fine here.
        let Err(err) = PpOcr::load(&dir.join("missing.onnx"), &dict, 5) else { panic!("a 5-class model must reject a 4-class dictionary") };
        assert!(matches!(&err, OcrError::Load(p, m) if p.ends_with("keys.txt") && m.contains("5 output classes")), "{err:?}");
        let Err(err) = PpOcr::load(&dir.join("missing.onnx"), &dict, 4) else { panic!("a missing model must fail to load") };
        assert!(matches!(&err, OcrError::Load(p, _) if p.ends_with("missing.onnx")), "{err:?}");
        // A dictionary whose lines are not single characters is refused too.
        std::fs::write(&dict, "аб\n").unwrap();
        assert!(matches!(PpOcr::load(&dir.join("missing.onnx"), &dict, 4), Err(OcrError::Load(p, _)) if p.ends_with("keys.txt")));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A crop with no room for the line, or a line far wider than the model reads, is reported
    /// rather than fed to the model.
    #[test]
    fn a_crop_outside_the_image_reads_nothing() {
        let Some(models) = crate::Models::find_for("ru") else {
            eprintln!("skipped: East Slavic OCR models not installed");
            return;
        };
        let ocr = PpOcr::load(&models.recognition, &models.recognition.with_file_name(crate::ESLAV_DICTIONARY), 519).unwrap();
        let rgb = vec![255u8; 30 * 10 * 3];
        assert_eq!(ocr.recognize(&rgb, 30, 10, [40.0, 40.0, 60.0, 60.0]).unwrap(), "");
        assert!(ocr.recognize(&rgb, 30, 10, [f32::NAN, 0.0, 10.0, 10.0]).is_err());
        // 4096 input pixels at 48 high is 85:1: a page-tall line is refused, not expanded.
        let wide = vec![255u8; 400 * 4 * 3];
        assert!(ocr.recognize(&wide, 400, 4, [0.0, 0.0, 400.0, 4.0]).is_err());
    }
}
