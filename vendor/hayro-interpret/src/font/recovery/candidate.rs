//! PdfCraft patch (105): the candidate policy every recovery rule's reading passes through.
//!
//! A rule names characters from evidence that isn't a `/ToUnicode` map: a glyph name, a glyph's
//! place in a standard order, a font's own `cmap`. Some of what that evidence names is not text
//! anyone wants copied, and some glyphs are named more than once. The policy here is shared so
//! every rule refuses the same things and breaks ties the same way:
//!
//! - **Never emitted:** private use characters (U+E000–U+F8FF and planes 15 and 16, which a
//!   symbolic `cmap` or a producer's own glyph list fills with symbols, not letters); C0 and C1
//!   control characters; noncharacters (U+FDD0–U+FDEF and the last two code points of every
//!   plane). A lone surrogate can't be a `char`; a number is turned into a character only through
//!   [`from_code_point`], which refuses one.
//! - **Several candidates for one glyph:** [`choose`] keeps the acceptable ones; one distinct
//!   character is the answer; among several, a single ASCII one wins (a hyphen over U+2010, a
//!   space over U+00A0, as the Adobe Glyph List names both for one glyph); anything else is
//!   ambiguous and gives nothing, whatever order the candidates came in.

use hayro_cmap::BfString;

/// Whether `c` may be emitted as recovered text: not a private use character, a control
/// character or a noncharacter.
pub(crate) fn acceptable(c: char) -> bool {
    let u = u32::from(c);
    let private = matches!(u, 0xE000..=0xF8FF | 0xF_0000..=0x10_FFFF);
    let noncharacter = matches!(u, 0xFDD0..=0xFDEF) || u & 0xFFFE == 0xFFFE;
    !(private || noncharacter || c.is_control())
}

/// Whether recovered `text` may be emitted: not empty, and every character [`acceptable`].
pub(crate) fn text_acceptable(text: &BfString) -> bool {
    match text {
        BfString::Char(c) => acceptable(*c),
        BfString::String(s) => !s.is_empty() && s.chars().all(acceptable),
    }
}

/// The character a code point names, when it is one and [`acceptable`]: a surrogate or a value
/// past U+10FFFF is no character.
#[allow(
    dead_code,
    reason = "read by the standard-order rules that build on this layer"
)]
pub(crate) fn from_code_point(u: u32) -> Option<char> {
    char::from_u32(u).filter(|c| acceptable(*c))
}

/// Most candidates [`choose`] looks at: a glyph is named a handful of times at most, and a
/// list longer than this is no evidence of one character.
const MAX_CANDIDATES: usize = 16;

/// The one character `candidates` name, under the policy above: the acceptable candidates, one
/// distinct character, or a single ASCII one among several; `None` when none is acceptable or
/// the choice is still ambiguous. The result doesn't depend on the candidates' order.
pub(crate) fn choose(candidates: impl IntoIterator<Item = char>) -> Option<char> {
    let mut kept: Vec<char> = Vec::new();
    for (seen, c) in candidates.into_iter().enumerate() {
        if seen >= MAX_CANDIDATES {
            return None;
        }
        if acceptable(c) && !kept.contains(&c) {
            kept.push(c);
        }
    }
    match kept.as_slice() {
        [] => None,
        [one] => Some(*one),
        several => {
            let mut ascii = several.iter().filter(|c| c.is_ascii());
            match (ascii.next(), ascii.next()) {
                (Some(c), None) => Some(*c),
                _ => None,
            }
        }
    }
}
