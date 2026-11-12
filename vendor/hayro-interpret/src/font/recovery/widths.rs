//! PdfCraft patch (105): width evidence, shared by the rules that read glyphs in a standard
//! order without the font naming them.
//!
//! A reading of a font's glyphs as characters can be checked against the glyphs' own widths: an
//! `m` is wider than an `n`, an `n` or an `o` than an `i` or an `l`, and a capital or a `w` than
//! an `i`, a `j`, an `l` or a full stop, in every proportional Latin typeface. A reading off by
//! one or more glyphs puts an `l` where the `m` is, or an `m` where the `l` is, and breaks one
//! of these. A monospaced font, or one with too few of these letters, proves nothing.
//!
//! Counting comparisons is not enough on its own: one glyph read as a narrow letter against a
//! few read as capitals makes several comparisons that hold, and a subset of capitals only (a
//! form's labels, a name) shifted by a glyph or more reads `J` as `I` and every other capital
//! wider than it. So a reading is proved only when none of its comparisons fails and:
//!
//! - at least [`MIN_AGREEING`] hold, between [`MIN_WIDE`] different wide characters and
//!   [`MIN_NARROW`] different narrow letters (a shift puts the second narrow letter on a wide
//!   glyph: the `i` where the `h` is, the `l` where the `k` is);
//! - one of them is between two lowercase letters: a reading in one case only is what a shift
//!   between the cases of an order looks like;
//! - letters of a kind are about as wide as one another ([`ALIKE_GROUPS`]), and `f` and `r` are
//!   wider than `i` and `l`.
//!
//! pdfcraft-render's `text::recovery_tests` read generated texts (capitals only, small subsets,
//! sentences) shifted by one to three places, 29 to 33 and between the cases, and none proves.

use std::collections::HashMap;

/// Fewest width comparisons that must hold, with none contradicted, for the order to be taken.
const MIN_AGREEING: usize = 8;

/// Fewest different wide characters in the comparisons that hold: a reading of a handful of
/// letters can fall on plausible widths by chance, and proves nothing.
const MIN_WIDE: usize = 4;

/// Fewest different narrow letters in the comparisons that hold.
const MIN_NARROW: usize = 2;

/// Wide glyphs and narrow ones, by the characters they are read as: every glyph read as one of
/// the first is at least the group's margin times as wide as every glyph read as one of the
/// second: a round lowercase letter is at least half as wide
/// again as an `i` or an `l` in the proportional Latin typefaces (Helvetica's 556 against 222,
/// Times's 444 against 278, Garamond's 404 against 258), closer to an `r` (Times's 444 against
/// 333).
const WIDE_NARROW: [(&str, &str, f32); 3] = [
    // Capitals other than the narrow `F`, `I`, `J`, `L` and `T`, and `m` and `w`, against the
    // narrowest glyphs.
    ("mwMWABCDEGHKNOPRSUVXYZ", "ilj.,:;!\u{2019}I", MARGIN),
    // Round and stemmed lowercase letters against `i`, `l`, `j` and the full stop and comma.
    ("abdeghnopqu", "ilj.,", ROUND_OVER_NARROW),
    // The same against the half-narrow `r`, `t` and `f` (Helvetica's 333 and 278, Times's 333
    // and 278): a reading shifted onto them puts a round letter there.
    ("abdeghnopqu", "rtf", ROUND_OVER_HALF_NARROW),
];

/// How much wider a round or stemmed lowercase letter must be than an `i`, an `l`, a `j`, a full
/// stop or a comma.
const ROUND_OVER_NARROW: f32 = 1.4;

/// How much wider a round or stemmed lowercase letter must be than an `r`, a `t` or an `f`
/// (Times's `e` 444 against `r` 333, Georgia's 484 against 352, Helvetica's 556 against 333).
const ROUND_OVER_HALF_NARROW: f32 = 1.3;

/// How much wider a wide glyph must be than a narrow one to agree.
const MARGIN: f32 = 1.2;

/// `m` against `n`, a closer pair.
const M_OVER_N: f32 = 1.15;

/// `f` and `r` against `i` and `l`: wider in every proportional Latin typeface, if not by much
/// (Helvetica's 278 and 333 against 222, Times's 333 against 278, Georgia's 317 and 352 against
/// 277). A reading a few glyphs off can put `f` and `i` on an `i` and an `l`.
const F_R_OVER_I_L: f32 = 1.1;

/// Letters about as wide as one another in a proportional Latin typeface: the narrow `i`, `l`
/// and `j` (Helvetica's 222 each, Times's 278 each), and the round and stemmed lowercase letters
/// (Helvetica's 556 each, Times's 444 to 500). No two of a group may differ by more than
/// [`ALIKE`]: a reading of capitals as lowercase letters gives `j` the width of a `J`, and one
/// a few glyphs off gives an `o` the width of the `r` it is.
const ALIKE_GROUPS: [&str; 2] = ["ilj", "abdeghnopqu"];

/// How much two letters of an [`ALIKE_GROUPS`] group may differ in width.
const ALIKE: f32 = 1.3;

/// Width comparisons a reading of a font's glyphs passes and fails, given the width of every
/// character it reads (the first glyph read as a character gives its width).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct WidthEvidence {
    /// Comparisons that hold with their margin.
    pub(crate) agree: usize,
    /// The different characters read wide, and read narrow, in the comparisons that hold.
    pub(crate) wide: Vec<char>,
    pub(crate) narrow: Vec<char>,
    /// Whether a comparison between two lowercase letters holds.
    pub(crate) lowercase: bool,
    /// Comparisons the widths contradict or don't decide: the glyph read as the wide character
    /// is narrower, or wider by less than the margin (an `n` as wide as an `l`).
    pub(crate) contradict: usize,
}

impl WidthEvidence {
    /// Whether the evidence proves the reading: none of its comparisons fails, at least
    /// [`MIN_AGREEING`] hold, between at least [`MIN_WIDE`] different wide characters and
    /// [`MIN_NARROW`] different narrow letters (punctuation doesn't count), and one of them
    /// between two lowercase letters.
    pub(crate) fn proves(&self) -> bool {
        let narrow_letters = self.narrow.iter().filter(|c| c.is_alphabetic()).count();
        self.contradict == 0
            && self.agree >= MIN_AGREEING
            && self.wide.len() >= MIN_WIDE
            && narrow_letters >= MIN_NARROW
            && self.lowercase
    }
}

/// The width comparisons of a reading that gives `widths` (character to glyph width).
pub(crate) fn widths_agree(widths: &HashMap<char, f32>) -> WidthEvidence {
    let mut evidence = WidthEvidence::default();
    // `sides`: whether the comparison's characters count as wide and narrow ones (`m` against
    // `n` compares two wide letters, and counts toward neither side).
    let mut compare = |wide: char, narrow: char, margin: f32, sides: bool| {
        let (Some(&w), Some(&n)) = (widths.get(&wide), widths.get(&narrow)) else {
            return;
        };
        if !(w.is_finite() && n.is_finite()) || n <= 0.0 {
            return;
        }
        if w >= n * margin {
            evidence.agree = evidence.agree.saturating_add(1);
            if sides {
                if !evidence.wide.contains(&wide) {
                    evidence.wide.push(wide);
                }
                if !evidence.narrow.contains(&narrow) {
                    evidence.narrow.push(narrow);
                }
                evidence.lowercase |= wide.is_lowercase() && narrow.is_lowercase();
            }
        } else {
            evidence.contradict = evidence.contradict.saturating_add(1);
        }
    };
    for (wide, narrow, margin) in WIDE_NARROW {
        for w in wide.chars() {
            for n in narrow.chars() {
                compare(w, n, margin, true);
            }
        }
    }
    compare('m', 'n', M_OVER_N, false);
    for wide in ['f', 'r'] {
        for narrow in ['i', 'l'] {
            compare(wide, narrow, F_R_OVER_I_L, false);
        }
    }
    for group in ALIKE_GROUPS {
        let present: Vec<f32> = group
            .chars()
            .filter_map(|c| widths.get(&c).copied())
            .filter(|w| w.is_finite() && *w > 0.0)
            .collect();
        let (min, max) = present
            .iter()
            .fold((f32::INFINITY, 0.0_f32), |(lo, hi), &w| {
                (lo.min(w), hi.max(w))
            });
        if present.len() >= 2 && max > min * ALIKE {
            evidence.contradict = evidence.contradict.saturating_add(1);
        }
    }
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
