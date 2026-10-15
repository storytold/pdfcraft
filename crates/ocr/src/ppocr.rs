//! PP-OCR recognition (Chinese, accented Latin) on the local RTen runtime: CTC decoding of a
//! mobile recognition model over line crops. The detector and reading-order analysis are
//! shared with the English recogniser (ocrs).

use std::io::Read;
use std::path::Path;

use rten_tensor::{NdTensor, Tensor, prelude::*};

use crate::OcrError;

const HEIGHT: usize = 48;
const MAX_WIDTH: usize = 4096;

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

    pub fn recognize(&self, rgb: &[u8], width: u32, height: u32, rect: [f32; 4], preferred: &str) -> Result<String, OcrError> {
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
            return Err(OcrError::Recognize("PP-OCR output has an unexpected shape".into()));
        }
        let values: Vec<f32> = output.iter().copied().collect();
        decode(&values, &self.alphabet, preferred)
    }
}

/// How close a preferred letter has to be to the top score before it replaces that score.
/// Far-away letters, including names from another language, stay as read.
const PREFERENCE_MARGIN: f32 = 1.0;

fn decode(values: &[f32], alphabet: &[char], preferred: &str) -> Result<String, OcrError> {
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
        let best = preferred_near(row, alphabet, preferred, best);
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

/// If a letter this language writes scores within [`PREFERENCE_MARGIN`] of the winner, use the
/// strongest such letter. A winner that is already preferred, or that leads by more than the
/// margin, is unchanged, so a name from another language is kept.
fn preferred_near(row: &[f32], alphabet: &[char], preferred: &str, best: usize) -> usize {
    if preferred.is_empty() {
        return best;
    }
    let liked = |i: usize| alphabet.get(i).is_some_and(|c| c.is_ascii_alphabetic() || preferred.contains(*c));
    // A space, digit or mark that won stays. The preference only separates close letters
    // (Danish å against à), it does not turn a gap into a letter.
    if liked(best) || !alphabet.get(best).is_some_and(|c| c.is_alphabetic()) {
        return best;
    }
    let Some(best_v) = row.get(best).copied() else { return best };
    let mut chosen = None;
    let mut chosen_v = f32::NEG_INFINITY;
    for (i, v) in row.iter().copied().enumerate() {
        if liked(i) && best_v - v <= PREFERENCE_MARGIN && v > chosen_v {
            chosen = Some(i);
            chosen_v = v;
        }
    }
    chosen.unwrap_or(best)
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
        assert_eq!(decode(&scores, &alphabet, "").unwrap(), "中中文A");
        assert!(decode(&[f32::NAN; 4], &alphabet, "").is_err());
    }

    /// A close preferred letter replaces the winner. A letter that leads by more than the margin stays.
    #[test]
    fn a_close_preferred_letter_wins_and_a_clear_other_letter_stays() {
        let alphabet = ['\0', 'à', 'å'];
        let close = [0.0, 5.0, 4.2];
        assert_eq!(decode(&close, &alphabet, "å").unwrap(), "å");
        let clear = [0.0, 5.0, 3.0];
        assert_eq!(decode(&clear, &alphabet, "å").unwrap(), "à");
        assert_eq!(decode(&close, &alphabet, "").unwrap(), "à");
        // A winning space is a gap, not a letter the language can replace.
        let gap = [0.0, 5.0, 4.5, 4.4];
        assert_eq!(preferred_near(&gap, &['\0', ' ', 'à', 'å'], "å", 1), 1);
    }

    #[test]
    fn dictionary_must_match_the_models_class_count() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ppocr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dict = dir.join("keys.txt");
        std::fs::write(&dict, "a\nb\n").unwrap(); // blank + a + b + space = 4 classes.
        // The dictionary is validated before the model is opened, so a missing model is fine here.
        let Err(err) = PpOcr::load(&dir.join("missing.onnx"), &dict, 5) else { panic!("a 5-class model must reject a 4-class dictionary") };
        assert!(matches!(&err, OcrError::Load(p, m) if p.ends_with("keys.txt") && m.contains("5 output classes")), "{err:?}");
        let Err(err) = PpOcr::load(&dir.join("missing.onnx"), &dict, 4) else { panic!("a missing model must fail to load") };
        assert!(matches!(&err, OcrError::Load(p, _) if p.ends_with("missing.onnx")), "{err:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
