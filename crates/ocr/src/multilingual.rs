//! PaddleOCR's Apache-2.0 PP-OCRv6 recognizer and document orientation classifier.
//! Preprocessing follows the pinned inference.yml files in ATTRIBUTION.toml.
use crate::{OcrError, Word, lines::LineBox};
use image::{Rgb, RgbImage, imageops};
use rten_tensor::{NdTensor, Tensor, prelude::*};

pub(crate) struct Multilingual {
    recognition: rten::Model,
    orientation: rten::Model,
    alphabet: Vec<String>,
    latin_alphabet: Vec<bool>,
}

// Keep European names intact across Latin languages. This is a script restriction,
// not a per-language spelling dictionary; Japanese and Chinese retain the complete vocabulary.
fn latin_entry(entry: &str) -> bool {
    entry.chars().all(|ch| {
        !matches!(ch as u32, 0x2e80..=0xa4cf | 0xf900..=0xfaff | 0xff00..=0xffef | 0x1b000..=0x1b16f | 0x20000..=0x323af)
            && (!ch.is_alphabetic() || ch <= '\u{017f}')
    })
}

fn decoder_mask<'a>(language: &str, latin_alphabet: &'a [bool]) -> Option<&'a [bool]> {
    (!matches!(language, "ja" | "zh-Hant" | "zh-Hans")).then_some(latin_alphabet)
}

fn best_class<'a>(row: &'a [f32], allowed: Option<&[bool]>) -> Option<(usize, &'a f32)> {
    row.iter()
        .enumerate()
        .filter(|(i, score)| score.is_finite() && allowed.is_none_or(|mask| mask.get(*i).copied().unwrap_or(false)))
        .max_by(|a, b| a.1.total_cmp(b.1))
}

fn line_crop(image: &RgbImage, rect: [f32; 4]) -> Option<[u32; 4]> {
    let [l, t, r, b] = rect;
    if rect.iter().any(|v| !v.is_finite()) || r <= l || b <= t {
        return None;
    }
    let pad = (b - t).max(1.0);
    let x = (l - 0.3 * pad).floor().clamp(0.0, image.width() as f32) as u32;
    let y = (t - 0.15 * pad).floor().clamp(0.0, image.height() as f32) as u32;
    let right = (r + 0.3 * pad).ceil().clamp(0.0, image.width() as f32) as u32;
    let bottom = (b + 0.15 * pad).ceil().clamp(0.0, image.height() as f32) as u32;
    let (w, h) = (right.saturating_sub(x), bottom.saturating_sub(y));
    (w > 0 && h > 0).then_some([x, y, w, h])
}

/// The pixel at `(x, y)` (pixel centres at half-integers), interpolated bilinearly, with
/// white paper outside the image.
fn bilinear(image: &RgbImage, x: f32, y: f32) -> Rgb<u8> {
    let (x, y) = (x - 0.5, y - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |x: f32, y: f32| -> [f32; 3] {
        if x < 0.0 || y < 0.0 || x >= image.width() as f32 || y >= image.height() as f32 {
            return [255.0; 3];
        }
        image.get_pixel(x as u32, y as u32).0.map(f32::from)
    };
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
    Rgb(std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = c[i] + (d[i] - c[i]) * fx;
        (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8
    }))
}

/// `line` with the same margins as [`line_crop`], resampled straight: returns the image and
/// the line coordinates of its left edge.
fn straighten(image: &RgbImage, line: &LineBox) -> Option<(RgbImage, f32)> {
    let pad = line.height().max(1.0);
    let (u0, v0) = (line.u[0] - 0.3 * pad, line.v[0] - 0.15 * pad);
    let w = (line.length() + 0.6 * pad).ceil();
    let h = (line.height() + 0.3 * pad).ceil();
    // A line never exceeds the image's own perimeter; anything longer is broken geometry.
    let limit = 2.0 * (image.width() + image.height()) as f32;
    if !(w >= 1.0 && h >= 1.0 && w <= limit && h <= limit) {
        return None;
    }
    let straight = RgbImage::from_fn(w as u32, h as u32, |i, j| {
        let [x, y] = line.point(u0 + i as f32 + 0.5, v0 + j as f32 + 0.5);
        bilinear(image, x, y)
    });
    Some((straight, u0))
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
        let latin_alphabet = alphabet.iter().map(|entry| latin_entry(entry)).collect();
        Ok(Self { recognition: load(crate::MULTILINGUAL_MODEL)?, orientation: load(crate::ORIENTATION_MODEL)?, alphabet, latin_alphabet })
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

    pub fn read_line(&self, image: &RgbImage, rect: [f32; 4], language: &str) -> Result<Vec<Word>, OcrError> {
        let Some([x, y, w, h]) = line_crop(image, rect) else {
            return Ok(Vec::new());
        };
        // Recognition sees the margin, but the invisible text keeps the detected line's
        // own height instead of inheriting the padded crop's enlarged height.
        let [_, top, _, bottom] = rect;
        let top = top.clamp(0.0, image.height() as f32);
        let bottom = bottom.clamp(0.0, image.height() as f32);
        if bottom <= top {
            return Ok(Vec::new());
        }
        let crop = imageops::crop_imm(image, x, y, w, h).to_image();
        let words = self.recognize(&crop, language)?;
        Ok(words
            .into_iter()
            .map(|(text, start, end, confidence)| {
                word(image, text, [x as f32 + start, top, (x as f32 + end).min(image.width() as f32), bottom], confidence)
            })
            .collect())
    }

    /// Read a skewed line straightened along its own direction, so the recognizer sees
    /// neither the neighbouring lines a skewed line's bounding box takes in nor a sloping
    /// baseline. Each word's box is the axis-aligned bounds of its stretch of the line.
    pub fn read_skewed_line(&self, image: &RgbImage, line: &LineBox, language: &str) -> Result<Vec<Word>, OcrError> {
        let Some((straight, u0)) = straighten(image, line) else {
            return Ok(Vec::new());
        };
        let (width, height) = (image.width() as f32, image.height() as f32);
        let words = self.recognize(&straight, language)?;
        Ok(words
            .into_iter()
            .filter_map(|(text, start, end, confidence)| {
                let [l, t, r, b] = line.bounds(u0 + start, u0 + end);
                let rect = [l.clamp(0.0, width), t.clamp(0.0, height), r.clamp(0.0, width), b.clamp(0.0, height)];
                (rect[2] > rect[0] && rect[3] > rect[1]).then(|| word(image, text, rect, confidence))
            })
            .collect())
    }

    /// The words recognized in a line image, with their horizontal extent in its pixels and
    /// their mean character confidence (0–1).
    fn recognize(&self, crop: &RgbImage, language: &str) -> Result<Vec<(String, f32, f32, f32)>, OcrError> {
        let (w, h) = crop.dimensions();
        let rw = ((48.0 * w as f64 / h as f64).ceil() as u32).clamp(1, 3200);
        let width = rw.max(320) as usize;
        let resized = imageops::resize(crop, rw, 48, imageops::FilterType::Triangle);
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
        let flush = |words: &mut Vec<(String, f32, f32, f32)>, text: &mut String, start: usize, end: usize, confidence: f32, count: usize| {
            let mean = confidence / count.max(1) as f32;
            if !text.is_empty() && mean >= 0.5 {
                words.push((std::mem::take(text), start as f32 * scale, ((end + 1) as f32 * scale).min(w as f32), mean));
            } else {
                text.clear();
            }
        };
        for (step, row) in values.chunks_exact(classes).enumerate() {
            if step as f32 * scale >= w as f32 {
                break;
            }
            let allowed = decoder_mask(language, &self.latin_alphabet);
            let Some((index, &score)) = best_class(row, allowed) else { continue };
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

/// A recognized word: the engine id is the recognizer's (`ocrs`), its confidence on the 0–100
/// scale. Its box is the line's height narrowed to the word's own ink ([`ink_rows`]), as tight as
/// the other engines' boxes: the text layer sizes a word from its letters' ink height, so a box as
/// tall as the whole line would make "in" a far larger font than "België" beside it, and the
/// gap between them would no longer read as a space.
fn word(image: &RgbImage, text: String, rect: [f32; 4], confidence: f32) -> Word {
    let [l, t, r, b] = rect;
    let (t, b) = ink_rows(image, rect).unwrap_or((t, b));
    Word::new(text, [l, t, r, b], "ocrs").with_confidence(confidence * 100.0)
}

/// The first and last pixel rows inside `rect` that hold ink: pixels darker than halfway between
/// the region's lightest and darkest luminance. `None` when the region is empty or has too little
/// contrast to tell ink from paper (the line's own height is kept then).
fn ink_rows(image: &RgbImage, rect: [f32; 4]) -> Option<(f32, f32)> {
    let [l, t, r, b] = rect;
    if rect.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let (w, h) = (image.width() as f32, image.height() as f32);
    let (x0, x1) = (l.floor().clamp(0.0, w) as u32, r.ceil().clamp(0.0, w) as u32);
    let (y0, y1) = (t.floor().clamp(0.0, h) as u32, b.ceil().clamp(0.0, h) as u32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let luma = |x: u32, y: u32| {
        let Rgb([red, green, blue]) = *image.get_pixel(x, y);
        (u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114) / 1000
    };
    let (mut darkest, mut lightest) = (u32::MAX, 0);
    for y in y0..y1 {
        for x in x0..x1 {
            let v = luma(x, y);
            darkest = darkest.min(v);
            lightest = lightest.max(v);
        }
    }
    if lightest.saturating_sub(darkest) < 48 {
        return None;
    }
    let threshold = darkest + (lightest - darkest) / 2;
    let inked = |y: u32| (x0..x1).any(|x| luma(x, y) < threshold);
    let top = (y0..y1).find(|&y| inked(y))?;
    let bottom = (y0..y1).rev().find(|&y| inked(y))?;
    Some((top as f32, (bottom + 1) as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A word's box narrows from the line's height to its ink; a blank or flat region keeps the
    /// line's height, and a box outside the image is no ink at all.
    #[test]
    fn word_boxes_narrow_to_their_ink() {
        let mut image = RgbImage::from_pixel(40, 30, Rgb([250, 250, 250]));
        for y in 12..20 {
            for x in 5..15 {
                image.put_pixel(x, y, Rgb([20, 20, 20]));
            }
        }
        assert_eq!(ink_rows(&image, [0.0, 0.0, 20.0, 30.0]), Some((12.0, 20.0)));
        assert_eq!(word(&image, "in".into(), [0.0, 0.0, 20.0, 30.0], 0.9).rect, [0.0, 12.0, 20.0, 20.0]);
        assert_eq!(ink_rows(&image, [20.0, 0.0, 40.0, 30.0]), None, "no contrast: no ink");
        assert_eq!(word(&image, "x".into(), [20.0, 0.0, 40.0, 30.0], 0.9).rect, [20.0, 0.0, 40.0, 30.0]);
        assert_eq!(ink_rows(&image, [50.0, 0.0, 60.0, 30.0]), None);
        assert_eq!(ink_rows(&image, [f32::NAN, 0.0, 10.0, 30.0]), None);
    }

    #[test]
    fn chinese_decoder_keeps_han_and_full_width_punctuation() {
        let alphabet = ["", "A", "體", "体", "，", "。"];
        let mask: Vec<_> = alphabet.iter().map(|s| latin_entry(s)).collect();
        for language in ["zh-Hant", "zh-Hans", "ja"] {
            for chosen in 2..alphabet.len() {
                let mut scores = vec![0.1; alphabet.len()];
                scores[chosen] = 0.9;
                assert_eq!(best_class(&scores, decoder_mask(language, &mask)).unwrap().0, chosen, "{language}");
            }
        }
        assert!(decoder_mask("it", &mask).is_some());
    }

    #[test]
    fn latin_decoder_preserves_names_accents_and_punctuation() {
        for entry in ["", " ", "Muñoz", "Ørsted", "Šimková", "Reykjavík", "Weiß", "français", "cœur", "ÉÇÙ", "’", "€", "123,."] {
            assert!(latin_entry(entry), "{entry}");
        }
        for entry in ["新", "，", "。", "あ", "ｱ", "Ａ", "Ω", "Ж", "ə", "ƀ", "é新", "𠀀"] {
            assert!(!latin_entry(entry), "{entry}");
        }
        let alphabet = ["", ",", "，", "新", "œ"];
        let mask: Vec<_> = alphabet.iter().map(|s| latin_entry(s)).collect();
        let row = [0.1, 0.8, 0.95, 0.9, 0.6];
        assert_eq!(best_class(&row, Some(&mask)).unwrap().0, 1);
        assert_eq!(best_class(&row, None).unwrap().0, 2, "Japanese retains full-width punctuation");
        assert_eq!(best_class(&[0.1, 0.1, 0.9, 0.95, 0.8], Some(&mask)).unwrap().0, 4);
        assert_eq!(best_class(&[0.6, 0.1, 0.9, 0.95, 0.2], Some(&mask)).unwrap().0, 0, "CTC blank stays available");
    }

    #[test]
    fn straightening_a_skewed_line_levels_it_with_white_outside_the_image() {
        // A dark 100 × 10 bar turned 4° clockwise, whose line also runs off the image's left edge.
        let (s, c) = 4f32.to_radians().sin_cos();
        let image = RgbImage::from_fn(140, 60, |x, y| {
            let (dx, dy) = (x as f32 + 0.5 - 50.0, y as f32 + 0.5 - 30.0);
            let (u, v) = (dx * c + dy * s, -dx * s + dy * c);
            if (-50.0..50.0).contains(&u) && (-5.0..5.0).contains(&v) { Rgb([0, 0, 0]) } else { Rgb([200, 200, 200]) }
        });
        let at = |u: f32, v: f32| [50.0 + u * c - v * s, 30.0 + u * s + v * c];
        let corners = [at(-50.0, -5.0), at(50.0, -5.0), at(50.0, 5.0), at(-50.0, 5.0)];
        let line = LineBox::fit(&[([c, s], 100.0)], &corners).unwrap();
        let (straight, u0) = straighten(&image, &line).unwrap();
        // 100 × 10 with margins of 3 and 1.5, give or take the fit's rounding.
        assert!(matches!(straight.dimensions(), (106..=107, 13)), "{:?}", straight.dimensions());
        assert!((u0 - (line.u[0] - 3.0)).abs() < 1e-3);
        // Every row through the bar is dark from end to end: the bar is level.
        for y in 4..10 {
            assert!((6..100).all(|x| straight.get_pixel(x, y)[0] < 60), "row {y} is not level");
        }
        // The margin rows hold the page around the bar, and the left margin runs off the image.
        assert!((6..100).all(|x| straight.get_pixel(x, 0)[0] > 150));
        assert_eq!(*straight.get_pixel(0, 12), Rgb([255, 255, 255]));
        let broken = LineBox { across: [1.0, 0.0], u: [0.0, 1e9], v: [0.0, 10.0] };
        assert!(straighten(&image, &broken).is_none());
    }

    #[test]
    fn crop_includes_accents_and_full_stops_and_stays_inside_image() {
        let image = RgbImage::new(100, 100);
        assert_eq!(line_crop(&image, [20.0, 30.0, 70.0, 50.0]), Some([14, 27, 62, 26]));
        assert_eq!(line_crop(&image, [0.0, 0.0, 100.0, 100.0]), Some([0, 0, 100, 100]));
        for rect in [[f32::NAN, 0.0, 10.0, 10.0], [0.0, 0.0, f32::INFINITY, 10.0], [20.0, 20.0, 10.0, 30.0], [1.0; 4], [200.0, 20.0, 300.0, 40.0]] {
            assert_eq!(line_crop(&image, rect), None);
        }
        assert_eq!(line_crop(&RgbImage::new(0, 0), [0.0, 0.0, 10.0, 10.0]), None);
    }
}
