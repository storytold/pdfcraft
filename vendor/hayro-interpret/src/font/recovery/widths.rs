//! PdfCraft patch (104): width evidence, shared by the rules that read glyphs in a standard
//! order without the font naming them.
//!
//! A reading of a font's glyphs as characters can be checked against the glyphs' own widths: an
//! `m` is wider than an `n`, an `n` or an `o` than an `i` or an `l`, and a capital or a `w` than
//! an `i`, a `j`, an `l` or a full stop, in every proportional Latin typeface. A reading off by
//! one or more glyphs puts an `l` where the `m` is, or an `m` where the `l` is, and breaks one
//! of these. A monospaced font, or one with too few of these letters, proves nothing.

use std::collections::HashMap;

/// Fewest width comparisons that must hold, with none contradicted, for the order to be taken.
const MIN_AGREEING: usize = 3;

/// Wide glyphs and narrow ones, by the characters they are read as: every glyph read as one of
/// the first is at least [`MARGIN`] times as wide as every glyph read as one of the second.
const WIDE_NARROW: [(&str, &str); 2] = [
    // Capitals other than the narrow `F`, `I`, `J`, `L` and `T`, and `m` and `w`, against the
    // narrowest glyphs.
    ("mwMWABCDEGHKNOPRSUVXYZ", "ilj.,:;!\u{2019}I"),
    // Round and stemmed lowercase letters against `i`, `l`, `j` and the full stop and comma.
    ("abdeghnopqu", "ilj.,"),
];

/// How much wider a wide glyph must be than a narrow one to agree.
const MARGIN: f32 = 1.2;

/// `m` against `n`, a closer pair.
const M_OVER_N: f32 = 1.15;

/// Width comparisons a reading of a font's glyphs passes and fails, given the width of every
/// character it reads (the first glyph read as a character gives its width).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct WidthEvidence {
    /// Comparisons that hold with their margin.
    pub(crate) agree: usize,
    /// Comparisons the widths contradict or don't decide: the glyph read as the wide character
    /// is narrower, or wider by less than the margin (an `n` as wide as an `l`).
    pub(crate) contradict: usize,
}

impl WidthEvidence {
    /// Whether the evidence proves the reading: at least [`MIN_AGREEING`] comparisons hold and
    /// none fails.
    pub(crate) fn proves(&self) -> bool {
        self.agree >= MIN_AGREEING && self.contradict == 0
    }
}

/// The width comparisons of a reading that gives `widths` (character to glyph width).
pub(crate) fn widths_agree(widths: &HashMap<char, f32>) -> WidthEvidence {
    let mut evidence = WidthEvidence::default();
    let mut compare = |wide: char, narrow: char, margin: f32| {
        let (Some(&w), Some(&n)) = (widths.get(&wide), widths.get(&narrow)) else {
            return;
        };
        if !(w.is_finite() && n.is_finite()) || n <= 0.0 {
            return;
        }
        if w >= n * margin {
            evidence.agree = evidence.agree.saturating_add(1);
        } else {
            evidence.contradict = evidence.contradict.saturating_add(1);
        }
    };
    for (wide, narrow) in WIDE_NARROW {
        for w in wide.chars() {
            for n in narrow.chars() {
                compare(w, n, MARGIN);
            }
        }
    }
    compare('m', 'n', M_OVER_N);
    evidence
}

/// The width of each character `read` gives the ids, from `width` (the first id read as a
/// character gives its width; ids without a width are left out).
pub(crate) fn read_widths(
    ids: &[u32],
    read: impl Fn(u32) -> Option<char>,
    width: &dyn Fn(u32) -> Option<f32>,
) -> HashMap<char, f32> {
    let mut widths = HashMap::new();
    for &id in ids {
        if let (Some(c), Some(w)) = (read(id), width(id)) {
            widths.entry(c).or_insert(w);
        }
    }
    widths
}
