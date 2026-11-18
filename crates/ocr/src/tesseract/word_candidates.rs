//! Explicit selected-word proposals. Normal page recognition never calls this module.
use super::{
    Duration, Instant, ModelProfile, OcrError, OcrImage, RecognizeOptions, TIMEOUT, TSV_HEADER, TempDir, TesseractCli, WordCandidate,
    list_langs_from, set_private, tsv_row,
};

const MAX_WORD_SIDE: u32 = 2_000;
const MAX_WORD_PIXELS: u32 = 4_000_000;
const MAX_WORD_TEXT: usize = 4_096;
const MAX_WORD_OUTPUT: u64 = 4 * 1024 * 1024;

struct WordRaster {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

fn validate_crop(image: &OcrImage) -> Result<(), OcrError> {
    if image.width() > MAX_WORD_SIDE
        || image.height() > MAX_WORD_SIDE
        || image.width().checked_mul(image.height()).is_none_or(|pixels| pixels > MAX_WORD_PIXELS)
    {
        return Err(OcrError::Recognize("selected-word crops must be at most 2000 pixels per side and 4 million pixels".into()));
    }
    Ok(())
}

fn remaining(started: Instant, budget: Duration) -> Result<Duration, OcrError> {
    budget.checked_sub(started.elapsed()).filter(|left| !left.is_zero()).ok_or(OcrError::Timeout(budget.as_secs()))
}

/// Composite over white before detecting ink: hidden RGB under transparent pixels must not
/// enlarge the crop or appear in the proposal. Opaque RGB bytes are preserved exactly.
fn tight_ink(image: &OcrImage) -> Result<WordRaster, OcrError> {
    validate_crop(image)?;
    let mut rgb = Vec::with_capacity(image.rgba().len() / 4 * 3);
    let (mut left, mut top, mut right, mut bottom) = (image.width(), image.height(), 0, 0);
    for (index, pixel) in image.rgba().chunks_exact(4).enumerate() {
        let [red, green, blue, alpha] = pixel else { continue };
        let composite = |value: u8| ((u32::from(value) * u32::from(*alpha) + 255 * (255 - u32::from(*alpha)) + 127) / 255) as u8;
        let (r, g, b) = (composite(*red), composite(*green), composite(*blue));
        rgb.extend_from_slice(&[r, g, b]);
        // Fixed integer luminance, not a minimum-channel mask; detection never thresholds ink.
        let luminance = (299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b)) / 1000;
        if luminance < 245 {
            let x = (index % image.width() as usize) as u32;
            let y = (index / image.width() as usize) as u32;
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
    }
    if right <= left || bottom <= top {
        return Err(OcrError::Recognize("the selected-word crop contains no visible ink".into()));
    }
    let width = right - left;
    let height = bottom - top;
    let mut ink = Vec::with_capacity((width * height * 3) as usize);
    for y in top..bottom {
        let start = ((y * image.width() + left) * 3) as usize;
        let end = start + (width * 3) as usize;
        let row = rgb.get(start..end).ok_or_else(|| OcrError::Recognize("the selected-word RGB buffer is incomplete".into()))?;
        ink.extend_from_slice(row);
    }
    Ok(WordRaster { width, height, rgb: ink })
}

fn padded(ink: &WordRaster, padding: u32) -> Result<WordRaster, OcrError> {
    // Only the two fixed borders below reach this function; keep the guard for future callers
    // so arbitrary padding cannot turn a small selected word into an unbounded allocation.
    if !matches!(padding, 8 | 16) {
        return Err(OcrError::Recognize("selected-word padding must be 8 or 16 pixels".into()));
    }
    let width = ink.width.checked_add(padding * 2).ok_or(OcrError::ImageTooLarge)?;
    let height = ink.height.checked_add(padding * 2).ok_or(OcrError::ImageTooLarge)?;
    let len = width.checked_mul(height).and_then(|n| n.checked_mul(3)).ok_or(OcrError::ImageTooLarge)? as usize;
    let mut rgb = vec![255; len];
    for y in 0..ink.height {
        let from = (y * ink.width * 3) as usize;
        let to = (((y + padding) * width + padding) * 3) as usize;
        let count = (ink.width * 3) as usize;
        let source = ink.rgb.get(from..from + count).ok_or_else(|| OcrError::Recognize("the selected-word ink buffer is incomplete".into()))?;
        let destination = rgb.get_mut(to..to + count).ok_or_else(|| OcrError::Recognize("the selected-word padded buffer is incomplete".into()))?;
        destination.copy_from_slice(source);
    }
    Ok(WordRaster { width, height, rgb })
}

fn candidate(tsv: &str, padding: u32) -> Result<Option<WordCandidate>, OcrError> {
    if !tsv.lines().any(|line| line == TSV_HEADER || tsv_row(line).is_some()) {
        return Err(OcrError::Recognize("the selected-word reader returned non-TSV output".into()));
    }
    let mut found = None;
    for line in tsv.lines().filter(|line| line.split('\t').next() == Some("5")) {
        // A tab inside recognized text must not silently truncate a proposal to its prefix.
        if line.split('\t').count() != 12 {
            return Err(OcrError::Recognize("the selected-word reader returned extra or missing TSV columns".into()));
        }
        let row = tsv_row(line).ok_or_else(|| OcrError::Recognize("the selected-word reader returned a malformed TSV word".into()))?;
        if row.text.trim().is_empty() {
            continue;
        }
        if row.text.len() > MAX_WORD_TEXT {
            return Err(OcrError::Recognize("the selected-word text exceeds 4096 bytes".into()));
        }
        if found.is_some() {
            return Ok(None); // A multi-word crop is not a single-word proposal.
        }
        if row.text.contains(char::is_whitespace)
            || row.text.contains(char::is_control)
            || row.left < 0.0
            || row.top < 0.0
            || row.width <= 0.0
            || row.height <= 0.0
        {
            return Ok(None);
        }
        found = Some(WordCandidate {
            text: row.text.to_owned(),
            confidence: (row.conf >= 0.0).then(|| row.conf.clamp(0.0, 100.0)),
            padding,
            source: "tesseract".into(),
        });
    }
    Ok(found)
}

/// The injected read operation makes the shared deadline, single-word filter and separate
/// confidences testable without an installed OCR binary or changing the page recognizer.
fn proposals(
    image: &OcrImage,
    started: Instant,
    budget: Duration,
    mut read: impl FnMut(&WordRaster, u32, Duration) -> Result<String, OcrError>,
) -> Result<Vec<WordCandidate>, OcrError> {
    remaining(started, budget)?;
    let ink = tight_ink(image)?;
    let mut candidates = Vec::new();
    for padding in [8, 16] {
        let raster = padded(&ink, padding)?;
        let tsv = read(&raster, padding, remaining(started, budget)?)?;
        remaining(started, budget)?;
        if let Some(proposal) = candidate(&tsv, padding)? {
            candidates.push(proposal);
        }
    }
    Ok(candidates)
}

fn language(engine: &TesseractCli, options: &RecognizeOptions, started: Instant, budget: Duration) -> Result<String, OcrError> {
    let required = options.model_profile.required_packs(&options.language)?;
    let expression = required.join("+");
    if options.model_profile == ModelProfile::Standard && !super::super::is_turkish_language(&options.language) {
        return Ok(expression);
    }
    let mut installed = engine.langs.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if installed.is_empty() {
        let (out, err) = engine.run_capture(&["--list-langs"], remaining(started, budget)?, engine.max_output.min(MAX_WORD_OUTPUT))?;
        installed = list_langs_from(&out, &err).into_iter().map(|code| (code.clone(), code)).collect();
        // This discovery shares the call's deadline, unlike a separate availability probe.
        remaining(started, budget)?;
    }
    let missing: Vec<&str> = required.into_iter().filter(|pack| !installed.iter().any(|(code, _)| code.as_str() == *pack)).collect();
    if !missing.is_empty() {
        return Err(OcrError::Recognize(format!(
            "selected-word OCR profile {} is missing installed data: {}",
            options.model_profile.id(),
            missing.join(", ")
        )));
    }
    Ok(expression)
}

fn word_args<'a>(image: &'a str, language: &'a str) -> [&'a str; 8] {
    [image, "stdout", "-l", language, "--psm", "8", "-c", "tessedit_create_tsv=1"]
}

pub(super) fn recognize_word(engine: &TesseractCli, image: &OcrImage, options: &RecognizeOptions) -> Result<Vec<WordCandidate>, OcrError> {
    let started = Instant::now();
    let budget = engine.timeout.min(TIMEOUT);
    validate_crop(image)?; // Reject oversized document-derived input before discovery/processes.
    let language = language(engine, options, started, budget)?;
    proposals(image, started, budget, |raster, _, left| {
        let directory = TempDir::create()?;
        let path = directory.path.join("word.png");
        let mut file = std::fs::File::create(&path).map_err(|error| OcrError::Process(format!("creating the word image failed: {error}")))?;
        set_private(&mut file);
        use std::io::Write as _;
        let image = craft_image_codecs::Image::from_u8(raster.width, raster.height, craft_image_codecs::ChannelLayout::Rgb, raster.rgb.clone())
            .map_err(|error| OcrError::Process(format!("encoding the word image failed: {error}")))?;
        let png = craft_image_codecs::encode(&image, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default())
            .map_err(|error| OcrError::Process(format!("encoding the word pixels failed: {error}")))?;
        file.write_all(&png).map_err(|error| OcrError::Process(format!("writing the word image failed: {error}")))?;
        // Include PNG/file preparation in the shared budget too, not only child run time.
        let left = left.min(remaining(started, budget)?);
        engine.run(&word_args(&path.to_string_lossy(), &language), left, engine.max_output.min(MAX_WORD_OUTPUT))
    })
}

#[cfg(test)]
mod tests;
