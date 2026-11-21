//! Confidence: one classifier ([`band_for`]) drives the review overlay, the legend, the counts
//! and the uncertain-word navigation; the batch rule ([`low_confidence`]) is a separate
//! threshold with its own range check. The two rules are deliberately never merged.

/// The three confidence bands (the review overlay's green / yellow / red).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfidenceBand {
    Low,
    Medium,
    High,
}

impl ConfidenceBand {
    /// The legend line for this band, so the meaning never rests on colour alone.
    pub fn legend(self) -> &'static str {
        match self {
            ConfidenceBand::High => "HIGH (>= 90%)",
            ConfidenceBand::Medium => "MEDIUM (70-89%)",
            ConfidenceBand::Low => "LOW (< 70%)",
        }
    }
}

/// The one classifier: `>= 90` is High, `70..=89` Medium, `< 70` Low. Out-of-range values
/// clamp (negative → Low, above 100 → High); NaN is Low.
pub fn band_for(confidence: f32) -> ConfidenceBand {
    if confidence >= 90.0 {
        ConfidenceBand::High
    } else if confidence >= 70.0 {
        ConfidenceBand::Medium
    } else {
        ConfidenceBand::Low
    }
}

/// The batch rule, separate from [`band_for`]: a word is flagged when `0 <= conf <= threshold`
/// (default 60). Negative or above-threshold values are not flagged; NaN is not.
pub fn low_confidence(confidence: f32, threshold: f32) -> bool {
    (0.0..=threshold).contains(&confidence)
}

/// A word whose confidence lands in the Low band — a suspect for review. A word without a
/// reported confidence has no signal and is never a suspect.
pub fn is_suspect(confidence: Option<f32>) -> bool {
    confidence.is_some_and(|c| band_for(c) == ConfidenceBand::Low)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boundary matrix: 89 and 90, 69 and 70 straddle the bands.
    #[test]
    fn bands_straddle_the_boundaries() {
        assert_eq!(band_for(89.0), ConfidenceBand::Medium);
        assert_eq!(band_for(90.0), ConfidenceBand::High);
        assert_eq!(band_for(69.0), ConfidenceBand::Low);
        assert_eq!(band_for(70.0), ConfidenceBand::Medium);
        assert_eq!(band_for(0.0), ConfidenceBand::Low);
        assert_eq!(band_for(100.0), ConfidenceBand::High);
    }

    /// Out-of-range values clamp, and NaN lands in the Low band instead of comparing equal to
    /// nothing.
    #[test]
    fn out_of_range_confidences_clamp() {
        assert_eq!(band_for(-0.5), ConfidenceBand::Low, "negative → Low");
        assert_eq!(band_for(1000.0), ConfidenceBand::High, "above 100 → High");
        assert_eq!(band_for(f32::NAN), ConfidenceBand::Low);
        assert_eq!(band_for(f32::NEG_INFINITY), ConfidenceBand::Low);
        assert_eq!(band_for(f32::INFINITY), ConfidenceBand::High);
    }

    /// The batch threshold is its own rule: everything in `0..=threshold` flags, including
    /// words band_for calls Medium (60 < 70), and negatives never do.
    #[test]
    fn batch_threshold_is_separate_from_the_bands() {
        assert!(low_confidence(60.0, 60.0), "at the threshold it flags");
        assert!(low_confidence(0.0, 60.0));
        assert!(!low_confidence(60.5, 60.0));
        assert!(!low_confidence(-1.0, 60.0), "negative confidences are out of the rule's range");
        assert!(!low_confidence(f32::NAN, 60.0));
        // The rules disagree on real values: 65 is inside the Low band yet outside the batch
        // rule with the default threshold (60), which is why the two must not be merged.
        assert!(!low_confidence(65.0, 60.0));
        assert_eq!(band_for(65.0), ConfidenceBand::Low);
    }

    /// Unconfident words are not suspects; Low-band words are.
    #[test]
    fn suspects_need_a_low_band_confidence() {
        assert!(!is_suspect(None));
        assert!(!is_suspect(Some(95.0)));
        assert!(!is_suspect(Some(70.0)));
        assert!(is_suspect(Some(69.9)));
        assert!(is_suspect(Some(0.0)));
    }

    /// The legend names the thresholds, so the overlay never rests on colour alone.
    #[test]
    fn legend_text_matches_the_classifier() {
        assert_eq!(ConfidenceBand::High.legend(), "HIGH (>= 90%)");
        assert_eq!(ConfidenceBand::Medium.legend(), "MEDIUM (70-89%)");
        assert_eq!(ConfidenceBand::Low.legend(), "LOW (< 70%)");
    }
}
