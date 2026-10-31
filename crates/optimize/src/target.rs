//! Optimize to a size: find the sharpest settings whose result fits a byte budget.
//!
//! Nothing in the optimizer's settings says how large the result will be, so getting a deck under
//! an email limit meant trying settings by hand. [`optimize_to_target`] does the trying: it runs
//! the optimizer on a copy of the document with each rung of [`LADDER`] (the longest side images
//! keep and the JPEG quality, from sharpest to smallest), searching the ladder by bisection, and
//! returns the sharpest result that fits, or the smallest rung's if none does.

use pdfcraft_cos::{Document, SaveOptions, write_full};

use crate::{Compression, OptimizeError, Report, Settings, Stage, optimize_with_progress};

/// Settings from sharpest to smallest: (longest side in pixels, JPEG quality). Each rung gives a
/// smaller file than the one before it for the same document (JPEG size grows about with the
/// pixels and, between quality 40 and 80, with quality minus 15), which the search relies on.
pub const LADDER: [(u32, u8); 18] = [
    (1920, 75),
    (1920, 65),
    (1600, 65),
    (1600, 55),
    (1440, 55),
    (1440, 48),
    (1280, 52),
    (1280, 45),
    (1152, 45),
    (1024, 45),
    (1024, 38),
    (960, 38),
    (800, 38),
    (720, 35),
    (640, 32),
    (560, 28),
    (480, 25),
    (400, 22),
];

/// One rung tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trial {
    pub max_dimension: u32,
    pub quality: u8,
    /// Size of the saved file.
    pub bytes: usize,
}

/// The result of [`optimize_to_target`].
pub struct TargetOutcome {
    /// The optimized document, for the caller to save.
    pub doc: Document,
    /// The settings that produced it.
    pub settings: Settings,
    pub report: Report,
    /// Every rung tried, in the order tried.
    pub trials: Vec<Trial>,
    /// Size of the saved file.
    pub bytes: usize,
    /// Whether `bytes` is within the target: false when even the smallest rung is not.
    pub fits: bool,
}

/// `base` with rung `(max_dimension, quality)` applied: the longest side images and flattened
/// pages keep, and the JPEG quality of both (an image setting that isn't JPEG stays as it is).
///
/// If `base` has an explicit [`Settings::max_image_dimension`], the stricter (smaller) cap is
/// preserved so a large rung cannot widen the caller's ceiling.
fn rung(base: &Settings, (max_dimension, quality): (u32, u8)) -> Settings {
    let mut s = base.clone();
    s.target_mb = None;
    let dim = base.max_image_dimension.map_or(max_dimension, |b| b.min(max_dimension));
    s.max_image_dimension = Some(dim);
    s.flatten_tiles.max_dimension = dim;
    s.flatten_tiles.quality = quality;
    for image in [&mut s.color, &mut s.gray] {
        if matches!(image.compression, Compression::Jpeg(_)) {
            image.compression = Compression::Jpeg(quality);
        }
    }
    s
}

/// Optimize a copy of `doc` with `base`, with the longest side and quality chosen so the saved
/// file is at most `target_bytes`: the sharpest rung of [`LADDER`] that fits. `doc` is not
/// changed. `progress` also gets a [`Stage::Trial`] before each rung; returning `false` stops
/// with [`OptimizeError::Cancelled`].
///
/// A document whose own save is already within `target_bytes` is returned as it is: no rung is
/// tried (`trials` is empty), because even the sharpest rung re-encodes every image and could
/// make the file larger.
///
/// Each rung is a whole optimization of the document (fragments, flattening, every image), so a
/// search costs up to ceil(log2([`LADDER`].len())) of those: five passes at most.
///
/// The size is the optimizer's own full save (object streams as `base` says), which the caller's
/// save of the returned document matches or, with extra clean-up such as merging identical
/// objects, undercuts.
pub fn optimize_to_target(
    doc: &Document,
    base: &Settings,
    target_bytes: u64,
    progress: &mut dyn FnMut(Stage) -> bool,
) -> Result<TargetOutcome, OptimizeError> {
    // Nothing to search for when the document already fits: every rung re-encodes its images, so
    // the sharpest one could well be larger than the file as it is.
    let unchanged_opts = SaveOptions { object_streams: base.effective_object_streams(), ..SaveOptions::default() };
    let unchanged = write_full(doc, &unchanged_opts)?.len();
    if unchanged as u64 <= target_bytes {
        return Ok(TargetOutcome {
            doc: doc.clone(),
            settings: base.clone(),
            report: Report::default(),
            trials: Vec::new(),
            bytes: unchanged,
            fits: true,
        });
    }
    let of = LADDER.len();
    // Most rungs the bisection tries: ceil(log2(of + 1)).
    let max_trials = (usize::BITS - of.leading_zeros()) as usize;
    let mut trials = Vec::new();
    let mut try_rung = |i: usize, trials: &mut Vec<Trial>| -> Result<(Document, Settings, Report, usize), OptimizeError> {
        let &(max_dimension, quality) = LADDER.get(i).ok_or(OptimizeError::NoPages)?;
        if !progress(Stage::Trial { n: trials.len() + 1, of: max_trials }) {
            return Err(OptimizeError::Cancelled);
        }
        let settings = rung(base, (max_dimension, quality));
        let mut copy = doc.clone();
        let mut trial_progress = |_| progress(Stage::Trial { n: trials.len() + 1, of: max_trials });
        let report = optimize_with_progress(&mut copy, &settings, &mut trial_progress)?;
        let opts = SaveOptions { object_streams: settings.effective_object_streams(), ..SaveOptions::default() };
        let bytes = write_full(&copy, &opts)?.len();
        trials.push(Trial { max_dimension, quality, bytes });
        Ok((copy, settings, report, bytes))
    };
    // The first rung that fits, assuming each rung is no larger than the one before.
    let (mut lo, mut hi) = (0usize, of - 1);
    let mut best: Option<(Document, Settings, Report, usize)> = None;
    let mut smallest: Option<(Document, Settings, Report, usize)> = None;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let tried = try_rung(mid, &mut trials)?;
        if tried.3 as u64 <= target_bytes {
            best = Some(tried);
            if mid == 0 {
                break;
            }
            hi = mid - 1;
        } else {
            if smallest.as_ref().is_none_or(|s| tried.3 < s.3) {
                smallest = Some(tried);
            }
            lo = mid + 1;
        }
    }
    let (fits, (doc, settings, report, bytes)) = match (best, smallest) {
        (Some(b), _) => (true, b),
        (None, Some(s)) => (false, s),
        // A backstop, not a path the search takes: the loop always tries the last rung, so a
        // document too large for the whole ladder ends in `smallest`.
        (None, None) => (false, try_rung(of - 1, &mut trials)?),
    };
    Ok(TargetOutcome { doc, settings, report, trials, bytes, fits })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The search assumes each rung is smaller than the one before; check that under the size
    /// model the ladder's comment states.
    #[test]
    fn ladder_gets_smaller_rung_by_rung() {
        let size = |(d, q): (u32, u8)| f64::from(d) * f64::from(d) * (f64::from(q) - 15.0);
        for w in LADDER.windows(2) {
            assert!(size(w[1]) < size(w[0]), "{w:?}");
        }
    }

    #[test]
    fn a_rung_sets_every_place_the_size_is_decided() {
        let s = rung(&crate::Preset::Presentation.settings(), (1280, 48));
        assert_eq!(s.max_image_dimension, Some(1280));
        assert_eq!((s.flatten_tiles.max_dimension, s.flatten_tiles.quality), (1280, 48));
        assert_eq!(s.color.compression, Compression::Jpeg(48));
        assert_eq!(s.gray.compression, Compression::Jpeg(48));
        // A setting that isn't JPEG is left alone.
        let mut base = Settings::default();
        base.color.compression = Compression::Flate;
        assert_eq!(rung(&base, (800, 42)).color.compression, Compression::Flate);
    }

    #[test]
    fn a_rung_preserves_stricter_caller_max_image_dimension() {
        let mut base = crate::Preset::Presentation.settings();
        base.max_image_dimension = Some(1024);
        let s = rung(&base, (3840, 85));
        assert_eq!(s.max_image_dimension, Some(1024));
        assert_eq!(s.flatten_tiles.max_dimension, 1024);

        // When rung dimension is smaller than base, the tighter rung dimension is used.
        let s2 = rung(&base, (800, 42));
        assert_eq!(s2.max_image_dimension, Some(800));
        assert_eq!(s2.flatten_tiles.max_dimension, 800);
    }
}
