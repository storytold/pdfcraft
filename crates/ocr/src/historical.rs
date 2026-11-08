//! Optional historical-script recognition. Model files are installed separately.
//!
//! The historical Kraken PP-OCRv6 recognizer uses RGB ink values, 96-pixel lines
//! and 16 pixels of horizontal padding. This differs from the modern recognizer.
use crate::{Line, Models, Ocr, OcrError, OcrImage, RecognizeOptions, Recognizer, Word};
use image::{RgbImage, imageops};
use rten_tensor::{NdTensor, Tensor, prelude::*};
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

pub const MODEL: &str = "historical-recognition.onnx";
pub const DICTIONARY: &str = "historical-dictionary.json";
/// [`Recognizer::id`] of the historical print recognizer, and the source of its words.
pub const PRINT: &str = "historical";
/// [`Recognizer::id`] of the historical handwriting recognizer.
pub const HANDWRITING: &str = "historical-handwriting";
const HEIGHT: u32 = 96;
const PADDING: u32 = 16;
const MAX_WIDTH: u32 = 8192;

/// Cursive strokes can make the print detector report the same line twice.
/// Merge substantially overlapping regions before recognition, retaining the
/// detector's reading order. A little context keeps ascenders and descenders.
pub(crate) fn line_regions(regions: Vec<[f32; 4]>, (width, height): (u32, u32)) -> Vec<[f32; 4]> {
    let mut merged: Vec<[f32; 4]> = Vec::new();
    for region in regions {
        if region.iter().any(|value| !value.is_finite()) || region[2] - region[0] < 3.0 || region[3] - region[1] < 3.0 {
            continue;
        }
        let mut region = region;
        let mut index = merged.len();
        let mut i = 0;
        while i < merged.len() {
            let other = merged[i];
            let overlap_width = (region[2].min(other[2]) - region[0].max(other[0])).max(0.0);
            let overlap_height = (region[3].min(other[3]) - region[1].max(other[1])).max(0.0);
            if overlap_width >= 0.7 * (region[2] - region[0]).min(other[2] - other[0])
                && overlap_height >= 0.7 * (region[3] - region[1]).min(other[3] - other[1])
            {
                region = [region[0].min(other[0]), region[1].min(other[1]), region[2].max(other[2]), region[3].max(other[3])];
                index = index.min(i);
                merged.remove(i);
                i = 0;
            } else {
                i += 1;
            }
        }
        merged.insert(index.min(merged.len()), region);
    }
    merged
        .into_iter()
        .map(|[left, top, right, bottom]| {
            let pad = (bottom - top) * 0.2;
            [(left - pad * 0.5).max(0.0), (top - pad).max(0.0), (right + pad * 0.5).min(width as f32), (bottom + pad).min(height as f32)]
        })
        .collect()
}

/// An explicitly installed historical recognizer; loading never accesses the network.
pub struct Historical {
    model: rten::Model,
    alphabet: Vec<String>,
}

fn error(message: impl std::fmt::Display) -> OcrError {
    OcrError::Recognize(message.to_string())
}

impl Historical {
    pub fn load(dir: &Path) -> Result<Self, OcrError> {
        let dictionary = dir.join(DICTIONARY);
        if std::fs::metadata(&dictionary).map_err(error)?.len() > 1_000_000 {
            return Err(error("historical recognition dictionary is too large"));
        }
        let alphabet: Vec<String> = serde_json::from_slice(&std::fs::read(dictionary).map_err(error)?).map_err(error)?;
        if alphabet.len() != 1623 || alphabet.first().is_none_or(|blank| !blank.is_empty()) || alphabet.iter().any(|value| value.len() > 64) {
            return Err(error("invalid historical recognition dictionary"));
        }
        let path = dir.join(MODEL);
        if std::fs::metadata(&path).map_err(error)?.len() > 100_000_000 {
            return Err(error("historical recognition model is too large"));
        }
        let model = rten::Model::load_file(&path).map_err(|e| OcrError::Load(path.display().to_string(), e.to_string()))?;
        Ok(Self { model, alphabet })
    }

    /// Recognize one upright line, returning word boxes in the source image.
    pub fn read_line(&self, image: &RgbImage, rect: [f32; 4]) -> Result<Vec<Word>, OcrError> {
        if rect.iter().any(|value| !value.is_finite()) {
            return Err(error("historical recognition requires finite line coordinates"));
        }
        let [left, top, right, bottom] = rect;
        let x = (left.floor().max(0.0) as u32).min(image.width());
        let y = (top.floor().max(0.0) as u32).min(image.height());
        let width = (right.ceil().max(0.0) as u32).min(image.width()).saturating_sub(x);
        let height = (bottom.ceil().max(0.0) as u32).min(image.height()).saturating_sub(y);
        if width == 0 || height == 0 {
            return Ok(Vec::new());
        }
        let resized_width = (f64::from(width) * f64::from(HEIGHT) / f64::from(height)).round().max(1.0);
        if resized_width > f64::from(MAX_WIDTH) {
            return Err(error("the historical text line is too wide; split the line or use a taller line region"));
        }
        let resized_width = resized_width as u32;
        let padded_width = (resized_width + PADDING * 2) as usize;
        let crop = imageops::crop_imm(image, x, y, width, height).to_image();
        let resized = imageops::resize(&crop, resized_width, HEIGHT, imageops::FilterType::Lanczos3);
        let mut input = NdTensor::<f32, 4>::zeros([1, 3, HEIGHT as usize, padded_width]);
        for (px, py, pixel) in resized.enumerate_pixels() {
            for channel in 0..3 {
                input[[0, channel, py as usize, (px + PADDING) as usize]] = 1.0 - f32::from(pixel[channel]) / 255.0;
            }
        }
        let output: Tensor<f32> = self.model.run_one(input.view().into(), None).map_err(error)?.try_into().map_err(error)?;
        decode(&self.alphabet, &output, [x, y, width, height], resized_width, padded_width)
    }
}

/// The optional historical model as a [`Recognizer`]: historical print ([`PRINT`]) over the
/// lines the ocrs detector finds, or upright handwriting in one column ([`HANDWRITING`]) over
/// ink-projection lines. It reads one model for every language, so the requested language is
/// ignored. It reports no confidence; the page orientation comes from the modern models'
/// classifier, which it loads for line detection anyway. Loading reads local files only.
pub struct HistoricalRecognizer {
    ocr: Ocr,
    model: Historical,
    handwriting: bool,
}

impl HistoricalRecognizer {
    /// Load the ocrs detector from `models` and the add-on from `dir`; `handwriting` picks the
    /// line finder.
    pub fn load(models: &Models, dir: &Path, handwriting: bool) -> Result<HistoricalRecognizer, OcrError> {
        Ok(HistoricalRecognizer { ocr: Ocr::load(models)?, model: Historical::load(dir)?, handwriting })
    }
}

impl Recognizer for HistoricalRecognizer {
    fn id(&self) -> &'static str {
        if self.handwriting { HANDWRITING } else { PRINT }
    }

    fn available(&self) -> bool {
        true
    }

    fn languages(&self) -> Vec<(String, String)> {
        // One model for historical German-script sources; the language setting is not used.
        vec![("de".into(), "Deutsch".into())]
    }

    fn recognize(&self, image: &OcrImage, _options: &RecognizeOptions) -> Result<Vec<Line>, OcrError> {
        let rgb: Vec<u8> = image.rgba().as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let rgb = RgbImage::from_raw(image.width(), image.height(), rgb).ok_or(OcrError::EmptyImage)?;
        let mut lines =
            if self.handwriting { self.ocr.recognize_handwriting(&rgb, &self.model)? } else { self.ocr.recognize_historical(&rgb, &self.model)? };
        let id = self.id();
        for word in lines.iter_mut().flat_map(|l| l.words.iter_mut()) {
            word.source = id.into();
        }
        Ok(lines)
    }

    fn orientation(&self, image: &RgbImage) -> Option<Result<u16, OcrError>> {
        Some(self.ocr.orientation(image))
    }
}

fn decode(
    alphabet: &[String],
    output: &Tensor<f32>,
    [x, y, width, height]: [u32; 4],
    resized_width: u32,
    padded_width: usize,
) -> Result<Vec<Word>, OcrError> {
    let shape = output.shape();
    if alphabet.is_empty() || shape.len() != 3 || shape[0] != 1 || shape[2] != alphabet.len() || shape[1] == 0 || resized_width == 0 {
        return Err(error("invalid historical recognition output shape"));
    }
    let step_width = padded_width as f32 / shape[1] as f32;
    let coordinate =
        |step: usize| x as f32 + ((step as f32 * step_width - PADDING as f32) / resized_width as f32 * width as f32).clamp(0.0, width as f32);
    let values: Vec<f32> = output.iter().copied().collect();
    let mut words = Vec::new();
    let (mut text, mut previous, mut start, mut end) = (String::new(), 0, 0, 0);
    let flush = |words: &mut Vec<Word>, text: &mut String, start, end| {
        if !text.is_empty() && coordinate(end) > coordinate(start) {
            words.push(Word::new(text.nfc().collect::<String>(), [coordinate(start), y as f32, coordinate(end), (y + height) as f32], PRINT));
        }
        text.clear();
    };
    for (step, row) in values.chunks_exact(alphabet.len()).enumerate() {
        if row.iter().any(|value| !value.is_finite()) {
            return Err(error("historical recognition produced non-finite probabilities"));
        }
        let Some((token, _)) = row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else { continue };
        if token != 0 && token != previous {
            let Some(character) = alphabet.get(token) else { continue };
            if character.trim().is_empty() {
                flush(&mut words, &mut text, start, end);
            } else {
                if text.is_empty() {
                    start = step;
                }
                text.push_str(character);
                end = step + 1;
            }
        } else if token != 0 && !text.is_empty() {
            end = step + 1;
        }
        previous = token;
    }
    flush(&mut words, &mut text, start, end);
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probabilities(tokens: &[usize], classes: usize) -> Tensor<f32> {
        let mut scores = Tensor::zeros(&[1, tokens.len(), classes]);
        for (step, &token) in tokens.iter().enumerate() {
            scores[[0, step, token]] = 1.0;
        }
        scores
    }

    #[test]
    fn cursive_regions_merge_duplicates_but_keep_distinct_rows_and_columns() {
        let regions = line_regions(
            vec![
                [20.0, 10.0, 180.0, 40.0],
                [18.0, 14.0, 178.0, 44.0],
                [22.0, 60.0, 180.0, 85.0],
                [250.0, 10.0, 390.0, 40.0],
                [42.0, 0.0, 43.0, 12.0],
            ],
            (400, 100),
        );
        assert_eq!(regions.len(), 3);
        assert!(regions[0][0] < 18.0 && regions[0][1] < 10.0 && regions[0][3] > 44.0);
        assert!(regions[1][1] > regions[0][3], "separate baselines must stay separate");
        assert!(regions[2][0] > regions[0][2], "separate columns must stay separate");
        assert!(regions.iter().flatten().all(|value| value.is_finite()));
    }

    #[test]
    fn ctc_keeps_blank_separated_repeats_historical_letters_and_nfc() {
        let alphabet: Vec<String> = ["", "a", " ", "ſ", "u", "\u{0308}"].into_iter().map(str::to_owned).collect();
        let scores = probabilities(&[0, 1, 1, 0, 1, 2, 3, 4, 5, 0], alphabet.len());
        let words = decode(&alphabet, &scores, [50, 30, 600, 60], 600, 632).unwrap();
        assert_eq!(words.iter().map(|word| word.text.as_str()).collect::<Vec<_>>(), ["aa", "ſü"]);
        for word in &words {
            assert!(word.rect[0] >= 50.0 && word.rect[2] <= 650.0 && word.rect[2] > word.rect[0]);
            assert_eq!((word.rect[1], word.rect[3]), (30.0, 90.0));
        }
    }

    #[test]
    fn invalid_outputs_fail_without_partial_words_or_panics() {
        let alphabet = vec![String::new(), "a".into()];
        for shape in [&[0][..], &[1, 0, 2][..], &[1, 5, 3][..], &[2, 5, 2][..]] {
            assert!(decode(&alphabet, &Tensor::zeros(shape), [0, 0, 10, 10], 10, 42).is_err());
        }
        let mut scores = probabilities(&[1, 0, 1], 2);
        scores[[0, 2, 1]] = f32::NAN;
        assert!(decode(&alphabet, &scores, [0, 0, 10, 10], 10, 42).is_err());
        assert!(decode(&[], &Tensor::zeros(&[1, 2, 0]), [0, 0, 10, 10], 10, 42).is_err());
    }

    #[test]
    fn blank_and_padding_only_predictions_do_not_create_words() {
        let alphabet = vec![String::new(), "a".into()];
        assert!(decode(&alphabet, &probabilities(&[0, 0, 0, 0], 2), [0, 0, 10, 10], 10, 42).unwrap().is_empty());
        assert!(decode(&alphabet, &probabilities(&[1, 0, 0, 0, 0, 0], 2), [0, 0, 10, 10], 10, 42).unwrap().is_empty());
    }
}
