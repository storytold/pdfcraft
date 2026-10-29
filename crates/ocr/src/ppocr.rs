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
        let values = self.scores(rgb, width, height, rect)?;
        if values.is_empty() {
            return Ok(String::new());
        }
        decode(&values, &self.alphabet, preferred)
    }

    /// One score per class for every time step. An empty crop is an empty vector.
    fn scores(&self, rgb: &[u8], width: u32, height: u32, rect: [f32; 4]) -> Result<Vec<f32>, OcrError> {
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
            return Ok(Vec::new());
        }
        let resized = input_width(cw, ch);
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
        Ok(output.iter().copied().collect())
    }
}

/// The width a `cw` x `ch` crop is scaled to at the recogniser's height: its own aspect ratio,
/// squeezed to [`MAX_WIDTH`] when it is longer. Lines are split at word gaps before they get
/// here; a single word that is still too long is read squeezed rather than refused, so one
/// line cannot fail the page.
fn input_width(cw: usize, ch: usize) -> usize {
    let scaled = (cw as f64 * HEIGHT as f64 / ch.max(1) as f64).ceil();
    if scaled.is_finite() { (scaled.min(MAX_WIDTH as f64) as usize).max(1) } else { MAX_WIDTH }
}

/// How close a preferred letter has to be, on the model's softmax probability, before it
/// replaces the winner. A lead larger than this stays, including a letter from another language.
const PREFERENCE_MARGIN: f32 = 0.1;

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

    /// A crop longer than the recogniser takes is squeezed to its widest input, not refused.
    #[test]
    fn an_overlong_crop_is_squeezed_not_refused() {
        assert_eq!(input_width(480, 48), 480);
        assert_eq!(input_width(4096, 48), MAX_WIDTH);
        assert_eq!(input_width(4097, 48), MAX_WIDTH);
        assert_eq!(input_width(30_000, 30), MAX_WIDTH);
        assert_eq!(input_width(10, 0), 480, "a crop without height counts as one pixel high");
        assert_eq!(input_width(1, 1000), 1);
        assert_eq!(input_width(usize::MAX, 1), MAX_WIDTH);
        assert_eq!(input_width(0, 48), 1);
    }

    /// The model accepts a crop that used to be refused, including one exactly at the old limit.
    /// Skipped when the Latin model has not been fetched.
    #[test]
    fn an_overlong_crop_is_read_instead_of_refused() {
        let Some(models) = crate::Models::find_for("la") else {
            eprintln!("skipped: Latin OCR models not installed");
            return;
        };
        let ocr = PpOcr::load(&models.recognition, &models.recognition.with_file_name(crate::LATIN_DICTIONARY), 504).unwrap();
        let (w, h) = (6000u32, 40u32);
        let rgb = vec![255u8; (w * h * 3) as usize];
        let wide = ocr.recognize(&rgb, w, h, [0.0, 0.0, w as f32, h as f32], "");
        assert!(wide.is_ok(), "an over-wide crop must not fail the page: {wide:?}");
        let at_limit = ocr.recognize(&rgb, w, h, [0.0, 0.0, 4096.0, 40.0], "");
        assert!(at_limit.is_ok(), "{at_limit:?}");
        let err = ocr.recognize(&rgb, w, h, [f32::NAN, 0.0, 10.0, 10.0], "");
        assert!(err.as_ref().is_err_and(|e| e.to_string().contains("non-finite")), "{err:?}");
    }

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
    /// Scores are the model's softmax probabilities.
    #[test]
    fn a_close_preferred_letter_wins_and_a_clear_other_letter_stays() {
        let alphabet = ['\0', 'à', 'å'];
        let close = [0.0, 0.50, 0.45];
        assert_eq!(decode(&close, &alphabet, "å").unwrap(), "å");
        let clear = [0.0, 0.50, 0.30];
        assert_eq!(decode(&clear, &alphabet, "å").unwrap(), "à");
        assert_eq!(decode(&close, &alphabet, "").unwrap(), "à");
        // A winning space is a gap, not a letter the language can replace.
        let gap = [0.0, 0.80, 0.10, 0.05];
        assert_eq!(preferred_near(&gap, &['\0', ' ', 'à', 'å'], "å", 1), 1);
        // A foreign letter that leads clearly stays. The old margin of 1.0 replaced ä (0.99) with é (0.001).
        let foreign = [0.0, 0.99, 0.001];
        assert_eq!(decode(&foreign, &['\0', 'ä', 'é'], "é").unwrap(), "ä");
    }

    /// The Latin model's class scores are softmax probabilities, so the preference margin is on
    /// that scale. Skipped when the model has not been fetched.
    #[test]
    fn the_latin_recogniser_emits_a_probability_per_class() {
        let Some(models) = crate::Models::find_for("la") else {
            eprintln!("skipped: Latin OCR models not installed");
            return;
        };
        let ocr = PpOcr::load(&models.recognition, &models.recognition.with_file_name(crate::LATIN_DICTIONARY), 504).unwrap();
        let (w, h) = (120u32, 40u32);
        let mut rgb = vec![255u8; (w * h * 3) as usize];
        for x in 8..100 {
            for y in 12..28 {
                if x % 7 < 3 {
                    let i = ((y * w + x) * 3) as usize;
                    rgb[i] = 0;
                    rgb[i + 1] = 0;
                    rgb[i + 2] = 0;
                }
            }
        }
        let values = ocr.scores(&rgb, w, h, [0.0, 0.0, w as f32, h as f32]).unwrap();
        assert!(values.len() >= ocr.alphabet.len() && values.len().is_multiple_of(ocr.alphabet.len()), "{}", values.len());
        let row = &values[..ocr.alphabet.len()];
        let sum: f32 = row.iter().sum();
        let max = row.iter().copied().fold(f32::MIN, f32::max);
        assert!((sum - 1.0).abs() < 0.02, "a class row sums to {sum}, max {max}");
        assert!((0.0..=1.0).contains(&max), "{max}");
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
