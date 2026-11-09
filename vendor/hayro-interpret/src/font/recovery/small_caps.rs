//! PdfCraft patch (104): [`AdobeSmallCaps`], rule M, the Adobe Glyph List's private use small
//! capitals and oldstyle figures read as the characters they are forms of.
//!
//! The Adobe Glyph List names an expert font's small capitals and oldstyle figures with code
//! points in Adobe's corporate use subarea of the private use area: `Asmall` is U+F761,
//! `zerooldstyle` U+F730, `Ydieresissmall` U+F7FF. Each sits at U+F700 plus the Latin-1 code of
//! the character it is a form of (U+F761 is a form of `a`, 0x61). A font whose `/ToUnicode` maps
//! its codes to these, or whose glyph names are these names, gives its text as private use
//! characters, which ISO 32000-2 §9.10.2 then reads as they are: nothing anyone can search or
//! paste. The strict text keeps them; Copy with formatting reads each as the character it is a
//! form of, and a small capital as a lowercase letter shown in small capitals
//! (`font-variant: small-caps`), which is how the glyph looks.
//!
//! A code point is read only when the glyph list names it `…small` or `…oldstyle`, and the rest
//! of that name is, in the glyph list, the same character (up to case) as the code point's
//! Latin-1 offset: `Asmall` is U+F761 and `A` is `A`, a form of `a`. Any other private use
//! character still reads nothing.

use super::{FontFacts, FontKind, GlyphFacts, RecoveryMode, RecoveryRule, Rule, symbol_font};
use crate::font::generated::glyph_names;
use crate::font::glyph_name_to_unicode;
use hayro_cmap::BfString;

/// [`RecoveryRule::AdobeSmallCaps`].
pub(crate) struct AdobeSmallCaps;

/// Why [`AdobeSmallCaps`] replaces the strict text.
const PRIVATE_FORM: &str = "the strict text is a private use small capital or oldstyle figure of the Adobe \
     Glyph List, which stands for its base character";

/// The character a private use small capital or oldstyle figure of the Adobe Glyph List is a
/// form of, and whether it is a small capital (a letter); nothing for any other character.
pub(crate) fn small_cap_base(c: char) -> Option<(char, bool)> {
    let u = u32::from(c);
    if !(0xF720..=0xF7FF).contains(&u) {
        return None;
    }
    let name = glyph_names::get_reverse(c)?;
    let (stem, small) = match name.strip_suffix("small") {
        Some(stem) => (stem, true),
        None => (name.strip_suffix("oldstyle")?, false),
    };
    let base = char::from_u32(u.checked_sub(0xF700)?)?;
    let named = glyph_name_to_unicode(stem)?;
    let same = named == base || named.to_lowercase().eq(std::iter::once(base));
    (same && !base.is_control()).then_some((base, small && base.is_alphabetic()))
}

/// Whether `text` has a character [`small_cap_base`] reads.
fn has_small_cap(text: &BfString) -> bool {
    match text {
        BfString::Char(c) => small_cap_base(*c).is_some(),
        BfString::String(s) => s.chars().any(|c| small_cap_base(c).is_some()),
    }
}

impl Rule for AdobeSmallCaps {
    const RULE: RecoveryRule = RecoveryRule::AdobeSmallCaps;
    type Guard = ();

    /// A simple font (Type 1, CFF, TrueType or Type 3) with `/ToUnicode` (which may map codes
    /// to these code points) or with a glyph name the glyph list reads as one; the reading
    /// looks at each glyph's strict text. A Type 0 font never qualifies: in the corpus its
    /// `/ToUnicode` maps into this range for other reasons (a Korean font's private use
    /// characters at U+F734 …, an OCR text layer's identity map), and expert fonts with small
    /// capitals are simple fonts. Fonts named as symbol fonts never qualify either.
    fn guard(font: &FontFacts<'_>) -> Option<()> {
        if !matches!(
            font.kind,
            FontKind::Type1 | FontKind::Cff | FontKind::TrueType | FontKind::Type3
        ) || symbol_font(font.base_font)
        {
            return None;
        }
        let named = font.names.is_some_and(|names| {
            (0..=u8::MAX)
                .filter_map(names)
                .filter_map(glyph_name_to_unicode)
                .any(|c| small_cap_base(c).is_some())
        });
        (font.has_to_unicode || named).then_some(())
    }

    fn read(_: &(), _: &FontFacts<'_>, glyph: &GlyphFacts<'_>) -> Option<(BfString, RecoveryMode)> {
        let strict = glyph.strict?;
        if !has_small_cap(strict) {
            return None;
        }
        let base = |c: char| small_cap_base(c).map_or(c, |(b, _)| b);
        let text = match strict {
            BfString::Char(c) => BfString::Char(base(*c)),
            BfString::String(s) => BfString::String(s.chars().map(base).collect()),
        };
        Some((
            text,
            RecoveryMode::Override {
                reason: PRIVATE_FORM,
            },
        ))
    }
}

/// Whether recovered `text` read by `rule` is shown in small capitals: [`AdobeSmallCaps`]
/// reading a letter.
pub(crate) fn shown_small(rule: RecoveryRule, strict: Option<&BfString>) -> bool {
    rule == RecoveryRule::AdobeSmallCaps
        && match strict {
            Some(BfString::Char(c)) => small_cap_base(*c).is_some_and(|(_, small)| small),
            Some(BfString::String(s)) => s
                .chars()
                .any(|c| small_cap_base(c).is_some_and(|(_, small)| small)),
            None => false,
        }
}
