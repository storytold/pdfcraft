//! Reading Chinese text off a page image.
//!
//! The ocrs engine finds where the text is; PP-OCR's recognition model reads it. The two are
//! used together because ocrs's own recognition model reads the Latin alphabet only, while
//! PP-OCR's model reads Chinese — but was trained on whole text lines rather than on single
//! words, so the detector's boxes are grouped into lines and each line image is read as a line.
//!
//! The models and the dictionary are PP-OCRv6's, unmodified; the dictionary names the model's
//! output classes, one character each, and the model has one more class for the blank and one
//! for the space that follows the dictionary.

use std::path::Path;

use rten_tensor::{AsView, Layout, NdTensor};

use crate::{Line, OcrError, Word};

/// The height the recognition model reads a line image at. Its width follows the line's shape.
const LINE_HEIGHT: usize = 48;

/// How far apart two words on one baseline can be and still be one line, in word heights.
/// Chinese is written without spaces between its words, so a line is a run of words whose gaps
/// are small compared with the words themselves.
const LINE_GAP: f32 = 2.0;

/// A box in image pixels: left, top, right, bottom.
#[derive(Clone, Copy)]
pub(crate) struct TextBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl TextBox {
    fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    /// The height of the line this box sits on, for comparing boxes that belong together.
    fn middle(&self) -> f32 {
        (self.y0 + self.y1) * 0.5
    }
}

/// The Chinese recognition model, and the dictionary that names its output classes.
pub(crate) struct Chinese {
    model: rten::Model,
    alphabet: String,
}

impl Chinese {
    /// Load the recognition model and the dictionary that goes with it.
    pub(crate) fn load(recognition: &Path, dict: &Path) -> Result<Chinese, OcrError> {
        let model = rten::Model::load_file(recognition).map_err(|e| OcrError::Load(recognition.display().to_string(), e.to_string()))?;
        let raw = std::fs::read_to_string(dict).map_err(|e| OcrError::Load(dict.display().to_string(), e.to_string()))?;
        // One class per dictionary entry, one for the blank, and one for the space that
        // follows the dictionary: the model's class count is the dictionary's plus two.
        let mut alphabet = String::new();
        for entry in raw.lines() {
            let entry = entry.trim_end_matches(['\r', '\n']);
            if entry.chars().count() != 1 {
                return Err(OcrError::Load(dict.display().to_string(), "the dictionary's entries are not single characters".into()));
            }
            alphabet.push_str(entry);
        }
        alphabet.push(' ');
        Ok(Chinese { model, alphabet })
    }

    /// Read the lines of text in the page image, each as one word covering the whole line.
    ///
    /// `rgb` is the page as three bytes per pixel, `width` × `height` pixels.
    pub(crate) fn read(&self, rgb: &[u8], width: usize, height: usize, lines: &[Vec<TextBox>]) -> Result<Vec<Line>, OcrError> {
        let mut out = Vec::new();
        for line in lines {
            let Some(text) = self.read_line(rgb, width, height, line)? else { continue };
            if text.trim().is_empty() {
                continue;
            }
            let mut rect = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for b in line {
                rect[0] = rect[0].min(b.x0);
                rect[1] = rect[1].min(b.y0);
                rect[2] = rect[2].max(b.x1);
                rect[3] = rect[3].max(b.y1);
            }
            out.push(Line { words: vec![Word { text, rect }] });
        }
        Ok(out)
    }

    /// Read one line of text from the page image.
    fn read_line(&self, rgb: &[u8], width: usize, height: usize, line: &[TextBox]) -> Result<Option<String>, OcrError> {
        Ok(self.read_line_scored(rgb, width, height, line)?.map(|(text, _, _)| text))
    }

    /// Read one line of text and report its CTC score (the sum of per-step log probabilities)
    /// and the number of time steps the decoder ran. The score divided by the steps is the
    /// model's average confidence per character: text read at the page's true orientation
    /// scores far higher than text read turned (measured on Chinese pages), which is how
    /// [`Ocr::best_chinese_turn`] chooses a page's direction.
    fn read_line_scored(&self, rgb: &[u8], width: usize, height: usize, line: &[TextBox]) -> Result<Option<(String, f64, usize)>, OcrError> {
        let Some(crop) = Crop::of(rgb, width, height, line) else { return Ok(None) };
        // The model reads a line at a fixed height, with the width following the line's shape.
        let target_width = ((LINE_HEIGHT as f32 * crop.width as f32 / crop.height as f32).round() as usize).max(8);

        let input: NdTensor<f32, 4> = NdTensor::from_data([1usize, 3, LINE_HEIGHT, target_width], crop.resample(target_width, LINE_HEIGHT));
        let output = self
            .model
            .run_one(input.view().into(), None)
            .map_err(|e| OcrError::Recognize(e.to_string()))?
            .into_tensor::<f32>()
            .ok_or_else(|| OcrError::Recognize("the recognition model returned no tensor".into()))?;
        let shape = output.shape().to_vec();
        if shape.len() != 3 {
            return Err(OcrError::Recognize(format!("the recognition model returned {} dimensions", shape.len())));
        }
        let (batch, steps, classes) = (shape[0], shape[1], shape[2]);
        let data = output.data().ok_or_else(|| OcrError::Recognize("the recognition model returned no data".into()))?;

        let mut text = String::new();
        let mut score = 0.0f64;
        let mut decoded_steps = 0usize;
        for b in 0..batch {
            let mut row = Vec::with_capacity(steps * classes);
            row.extend_from_slice(&data[b * steps * classes..(b + 1) * steps * classes]);
            let probabilities: NdTensor<f32, 2> = NdTensor::from_data([steps, classes], row);
            let hypothesis = rten::ctc::CtcDecoder::new().decode_greedy(probabilities.view());
            text.push_str(&hypothesis.to_string(&self.alphabet));
            score += f64::from(hypothesis.score());
            decoded_steps += steps;
        }
        Ok(Some((text, score, decoded_steps)))
    }

    /// The page's orientation the Chinese model reads most confidently: read the page at each
    /// of the four quarter turns (clockwise) and return the turn whose text has the highest
    /// average CTC confidence per character. A page with no readable text returns `None`.
    ///
    /// Measured on a Chinese official-document page: the upright turn scores 0.983 per
    /// character, the other three at most 0.912 — enough to pick one reliably. The share of
    /// common characters is *not* a usable signal (its ranges overlap across turns).
    pub(crate) fn best_turn(&self, engine: &ocrs::OcrEngine, rgb: &[u8], width: u32, height: u32) -> Result<Option<u8>, OcrError> {
        let mut best: Option<(u8, f64)> = None;
        for q in 0..4u8 {
            let (pixels, w, h) = if q == 0 { (rgb.to_vec(), width, height) } else { crate::orient::quarter_turn(rgb, width, height, q) };
            let lines = detect_lines(engine, &pixels, w, h)?;
            let (mut score, mut steps) = (0.0f64, 0usize);
            for line in &lines {
                if let Some((text, s, n)) = self.read_line_scored(&pixels, w as usize, h as usize, line)? {
                    if text.trim().is_empty() {
                        continue;
                    }
                    score += s;
                    steps += n;
                }
            }
            if steps == 0 {
                continue;
            }
            let confidence = score / steps as f64;
            if best.is_none_or(|(_, c)| confidence > c) {
                best = Some((q, confidence));
            }
        }
        Ok(best.map(|(q, _)| q))
    }
}

/// A rectangle of the page image, cut out for reading.
struct Crop<'a> {
    pixels: &'a [u8],
    page_width: usize,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
}

impl<'a> Crop<'a> {
    /// The rectangle the line's words cover, cut out of the page image.
    fn of(pixels: &'a [u8], page_width: usize, page_height: usize, line: &[TextBox]) -> Option<Crop<'a>> {
        let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for b in line {
            left = left.min(b.x0);
            top = top.min(b.y0);
            right = right.max(b.x1);
            bottom = bottom.max(b.y1);
        }
        let left = left.floor().max(0.0) as usize;
        let top = top.floor().max(0.0) as usize;
        let right = (right.ceil().max(0.0) as usize).min(page_width);
        let bottom = (bottom.ceil().max(0.0) as usize).min(page_height);
        let width = right.saturating_sub(left);
        let height = bottom.saturating_sub(top);
        if width == 0 || height == 0 {
            return None;
        }
        Some(Crop { pixels, page_width, left, top, width, height })
    }

    /// The crop scaled to `width` × `height`, as three planes of values the way the model was
    /// trained: each pixel scaled from 0..255 into -1..1.
    fn resample(&self, width: usize, height: usize) -> Vec<f32> {
        let mut out = vec![0f32; 3 * width * height];
        for channel in 0..3 {
            for y in 0..height {
                // The pixel at the centre of the target pixel, in the crop's own coordinates.
                let source_y = (y as f32 + 0.5) * self.height as f32 / height as f32 - 0.5;
                let y0 = source_y.floor().max(0.0) as usize;
                let y1 = (y0 + 1).min(self.height - 1);
                let weight_y = (source_y - y0 as f32).clamp(0.0, 1.0);
                for x in 0..width {
                    let source_x = (x as f32 + 0.5) * self.width as f32 / width as f32 - 0.5;
                    let x0 = source_x.floor().max(0.0) as usize;
                    let x1 = (x0 + 1).min(self.width - 1);
                    let weight_x = (source_x - x0 as f32).clamp(0.0, 1.0);
                    let value = self.pixel(x0, y0, channel) * (1.0 - weight_x) * (1.0 - weight_y)
                        + self.pixel(x1, y0, channel) * weight_x * (1.0 - weight_y)
                        + self.pixel(x0, y1, channel) * (1.0 - weight_x) * weight_y
                        + self.pixel(x1, y1, channel) * weight_x * weight_y;
                    out[channel * width * height + y * width + x] = (value / 255.0 - 0.5) / 0.5;
                }
            }
        }
        out
    }

    fn pixel(&self, x: usize, y: usize, channel: usize) -> f32 {
        self.pixels[((self.top + y) * self.page_width + self.left + x) * 3 + channel] as f32
    }
}

/// The lines of text the detector found in the page image.
///
/// The detector is the ocrs engine's own, which finds text on a page whatever script it is
/// written in. The boxes it returns are the words; the lines are the runs of words that sit on
/// one baseline.
pub(crate) fn detect_lines(engine: &ocrs::OcrEngine, rgb: &[u8], width: u32, height: u32) -> Result<Vec<Vec<TextBox>>, OcrError> {
    let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
    let source = ocrs::ImageSource::from_bytes(rgb, (width, height)).map_err(|e| err(&e))?;
    let input = engine.prepare_input(source).map_err(|e| err(&e))?;
    let found = engine.detect_words(&input).map_err(|e| err(&e))?;
    let boxes: Vec<TextBox> = found
        .iter()
        .map(|word| {
            let mut box_ = TextBox { x0: f32::MAX, y0: f32::MAX, x1: f32::MIN, y1: f32::MIN };
            for corner in word.corners() {
                box_.x0 = box_.x0.min(corner.x);
                box_.y0 = box_.y0.min(corner.y);
                box_.x1 = box_.x1.max(corner.x);
                box_.y1 = box_.y1.max(corner.y);
            }
            box_
        })
        .collect();
    Ok(group_lines(&boxes))
}

/// Group the detected words into the lines of text they form.
///
/// Words on one baseline belong to the same line; a gap between them that is wide compared with
/// the words themselves ends the line. The lines come back in reading order: top to bottom, and
/// left to right within a line.
pub(crate) fn group_lines(words: &[TextBox]) -> Vec<Vec<TextBox>> {
    let mut lines = Vec::new();
    for baseline in baselines(words) {
        let mut line: Vec<TextBox> = Vec::new();
        for word in baseline {
            if let Some(previous) = line.last() {
                let height = previous.height().max(word.height()).max(1.0);
                if previous.x1 + LINE_GAP * height < word.x0 {
                    lines.push(std::mem::take(&mut line));
                }
            }
            line.push(word);
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

/// The words of each baseline, the baselines in reading order: top to bottom, and left to right
/// within a baseline.
fn baselines(words: &[TextBox]) -> Vec<Vec<TextBox>> {
    // Words on the same baseline, in the order they were detected.
    let mut groups: Vec<Vec<TextBox>> = Vec::new();
    let mut by_height: Vec<TextBox> = words.to_vec();
    by_height.sort_by(|a, b| a.middle().partial_cmp(&b.middle()).unwrap_or(std::cmp::Ordering::Equal));
    for word in by_height {
        match baseline_of(&groups, word) {
            Some(i) => groups[i].push(word),
            None => groups.push(vec![word]),
        }
    }
    // Reading order: the baselines by their height on the page.
    groups.sort_by(|a, b| top_of(a).partial_cmp(&top_of(b)).unwrap_or(std::cmp::Ordering::Equal));
    for group in groups.iter_mut() {
        group.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal));
    }
    groups
}

/// The baseline a word belongs to: the one it overlaps most, when they overlap enough to be the
/// same line of text.
fn baseline_of(groups: &[Vec<TextBox>], word: TextBox) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut best_overlap = 0.0f32;
    for (i, group) in groups.iter().enumerate() {
        let tallest = group.iter().fold(0.0f32, |tallest, b| tallest.max(b.height()));
        let overlap = word.y1.min(bottom_of(group)) - word.y0.max(top_of(group));
        let shortest = word.height().min(tallest);
        let same_line = overlap / shortest >= 0.3 && (word.middle() - group[0].middle()).abs() <= 0.6 * tallest.max(word.height());
        if same_line && overlap > best_overlap {
            best_overlap = overlap;
            best = Some(i);
        }
    }
    best
}

/// The topmost edge of a group of boxes.
fn top_of(group: &[TextBox]) -> f32 {
    group.iter().map(|b| b.y0).fold(f32::MAX, f32::min)
}

/// The bottommost edge of a group of boxes.
fn bottom_of(group: &[TextBox]) -> f32 {
    group.iter().map(|b| b.y1).fold(f32::MIN, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A word box, in the coordinates the detector gives.
    fn box_at(x0: f32, y0: f32, x1: f32, y1: f32) -> TextBox {
        TextBox { x0, y0, x1, y1 }
    }

    /// The words on one baseline are grouped together, and the baselines come back in reading
    /// order: top to bottom, and left to right within a baseline.
    #[test]
    fn baselines_group_the_words_of_a_line_and_order_them_for_reading() {
        let first = box_at(10.0, 100.0, 30.0, 120.0);
        let second = box_at(40.0, 101.0, 60.0, 121.0);
        let below = box_at(10.0, 200.0, 30.0, 220.0);
        let groups = baselines(&[second, below, first]);
        assert_eq!(groups.len(), 2, "one baseline for the first line, one for the second");
        assert_eq!(groups[0].len(), 2, "the two words of the first line are one baseline");
        assert!((groups[0][0].x0 - 10.0).abs() < f32::EPSILON, "left to right within a line");
        assert!((groups[0][1].x0 - 40.0).abs() < f32::EPSILON);
        assert!((groups[1][0].y0 - 200.0).abs() < f32::EPSILON, "the baselines are ordered top to bottom");
    }

    /// A word that sits on another line belongs to no baseline of the words given.
    #[test]
    fn a_word_off_the_line_belongs_to_no_baseline() {
        let line = vec![vec![box_at(10.0, 100.0, 30.0, 120.0)]];
        assert_eq!(baseline_of(&line, box_at(10.0, 100.0, 30.0, 120.0)), Some(0), "the same line");
        assert_eq!(baseline_of(&line, box_at(10.0, 200.0, 30.0, 220.0)), None, "another line");
    }

    /// A wide gap ends a line of text; a narrow one does not.
    #[test]
    fn a_wide_gap_ends_the_line_and_a_narrow_one_does_not() {
        let near = group_lines(&[box_at(10.0, 100.0, 30.0, 120.0), box_at(35.0, 100.0, 55.0, 120.0)]);
        assert_eq!(near.len(), 1, "a gap narrower than the words keeps them on one line");
        let far = group_lines(&[box_at(10.0, 100.0, 30.0, 120.0), box_at(200.0, 100.0, 220.0, 120.0)]);
        assert_eq!(far.len(), 2, "a gap wider than the words ends the line");
    }

    /// The crop is the rectangle the line covers, and it never reaches outside the page.
    #[test]
    fn the_crop_is_the_line_rectangle_clipped_to_the_page() {
        let pixels = vec![0u8; 8 * 8 * 3];
        let line = vec![box_at(2.0, 3.0, 5.0, 6.0)];
        let crop = Crop::of(&pixels, 8, 8, &line).expect("the crop of a line");
        assert_eq!((crop.left, crop.top, crop.width, crop.height), (2, 3, 3, 3));
        // A box that runs past the edge of the page is clipped to it.
        let past = vec![box_at(6.0, 6.0, 20.0, 20.0)];
        let clipped = Crop::of(&pixels, 8, 8, &past).expect("the crop of a line");
        assert_eq!((clipped.left, clipped.top, clipped.width, clipped.height), (6, 6, 2, 2));
        // An empty line has no crop.
        assert!(Crop::of(&pixels, 8, 8, &[]).is_none(), "an empty line has no crop");
    }

    /// The crop is resampled into the range the model was trained on: three planes, each
    /// value scaled from 0..255 into -1..1.
    #[test]
    fn the_crop_resamples_into_the_range_the_model_was_trained_on() {
        // A 2x2 page: red, green, blue, white.
        let mut pixels = vec![0u8; 2 * 2 * 3];
        pixels[0..3].copy_from_slice(&[255, 0, 0]);
        pixels[3..6].copy_from_slice(&[0, 255, 0]);
        pixels[6..9].copy_from_slice(&[0, 0, 255]);
        pixels[9..12].copy_from_slice(&[255, 255, 255]);
        let line = vec![box_at(0.0, 0.0, 2.0, 2.0)];
        let crop = Crop::of(&pixels, 2, 2, &line).expect("the crop of the page");
        let out = crop.resample(2, 2);
        assert_eq!(out.len(), 3 * 2 * 2, "three planes of the target size");
        // The planes are separate: the first is red, the second green, the third blue.
        let at = |channel: usize, x: usize, y: usize| out[channel * 4 + y * 2 + x];
        assert!((at(0, 0, 0) - 1.0).abs() < 1e-6, "red at 255 is 1");
        assert!((at(0, 1, 0) + 1.0).abs() < 1e-6, "red at 0 is -1");
        assert!((at(1, 0, 0) + 1.0).abs() < 1e-6, "green at 0 is -1");
        assert!((at(2, 0, 1) - 1.0).abs() < 1e-6, "blue at 255 is 1");
        // The fourth pixel is white, so every channel of it is 255.
        assert!((at(2, 1, 1) - 1.0).abs() < 1e-6, "the white pixel's blue is 1");
        assert!((at(0, 1, 1) - 1.0).abs() < 1e-6, "the white pixel's red is 1");
    }

    /// The Chinese recogniser reads a Chinese page: the words are found and the text comes out
    /// right. Opt-in, because it needs the models and a page to read:
    ///
    /// - `PDFCRAFT_MODELS`: the folder holding the models;
    /// - `PDFCRAFT_CN_RGB`: the page as raw RGB bytes;
    /// - `PDFCRAFT_CN_W`, `PDFCRAFT_CN_H`: the page's size in pixels.
    ///
    /// The models are PP-OCRv6's, unmodified.
    #[test]
    fn chinese_recognition_reads_a_chinese_page() {
        let Ok(_models_dir) = std::env::var("PDFCRAFT_MODELS") else { return };
        let Ok(pixels_path) = std::env::var("PDFCRAFT_CN_RGB") else { return };
        let Ok(width) = std::env::var("PDFCRAFT_CN_W").unwrap_or_default().parse::<u32>() else { return };
        let Ok(height) = std::env::var("PDFCRAFT_CN_H").unwrap_or_default().parse::<u32>() else { return };
        let pixels = std::fs::read(&pixels_path).expect("the page's pixels");
        let models = crate::Models::find().expect("the models");
        let ocr = crate::Ocr::load(&models).expect("the recogniser");
        let lines = ocr.recognize_in(&pixels, width, height, "zh").expect("the text");
        let text: String = lines.iter().map(|l| l.text()).collect();
        assert!(text.contains("签发人：林炳豪"), "{text}");
        assert!(text.contains("福建省财政厅数字财政建设领导小组办公室"), "{text}");
        assert!(text.contains("为深化财税体制改革"), "{text}");
    }

    /// The same page, written out as one line per recognised line, so that the text another
    /// engine reads can be compared with it word for word.
    #[test]
    fn chinese_recognition_writes_a_page_out() {
        let Ok(_models_dir) = std::env::var("PDFCRAFT_MODELS") else { return };
        let Ok(pixels_path) = std::env::var("PDFCRAFT_CN_RGB") else { return };
        let Ok(out_path) = std::env::var("PDFCRAFT_CN_TSV") else { return };
        let Ok(width) = std::env::var("PDFCRAFT_CN_W").unwrap_or_default().parse::<u32>() else { return };
        let Ok(height) = std::env::var("PDFCRAFT_CN_H").unwrap_or_default().parse::<u32>() else { return };
        let pixels = std::fs::read(&pixels_path).expect("the page's pixels");
        let models = crate::Models::find().expect("the models");
        let ocr = crate::Ocr::load(&models).expect("the recogniser");
        let lines = ocr.recognize_in(&pixels, width, height, "zh").expect("the text");
        let out = lines
            .iter()
            .flat_map(|l| l.words.iter())
            .map(|w| format!("{}\t{}\t{}\t{}\t{}\n", w.rect[0], w.rect[1], w.rect[2], w.rect[3], w.text))
            .collect::<String>();
        std::fs::write(&out_path, out).expect("the page's text");
    }
}
