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

/// Suffixes that name a small capital or an oldstyle figure after the character it is a form
/// of (`a.sc`, `a.smcp`, `A.c2sc`, `zero.onum`, `zero.oldstyle`).
const FORM_SUFFIXES: [&str; 7] = ["sc", "smcp", "c2sc", "small", "oldstyle", "onum", "osf"];

/// The character a glyph name says its glyph is a small capital or an oldstyle figure of: a
/// name the glyph list itself gives such a code point (`Asmall`, `zerooldstyle`), or a
/// character's name with a form suffix (`a.sc`, `five.oldstyle`). Nothing for any other name.
pub(crate) fn name_form_base(name: &str) -> Option<char> {
    if let Some((base, _)) = glyph_name_to_unicode(name).and_then(small_cap_base) {
        return Some(base);
    }
    let (stem, suffix) = name.split_once('.')?;
    if !FORM_SUFFIXES.contains(&suffix) {
        return None;
    }
    let mut lower = glyph_name_to_unicode(stem)?.to_lowercase();
    let base = lower.next()?;
    lower.next().is_none().then_some(base)
}

impl Rule for AdobeSmallCaps {
    const RULE: RecoveryRule = RecoveryRule::AdobeSmallCaps;
    type Guard = ();

    /// A simple font (Type 1, CFF, TrueType or Type 3) with at least one glyph name that names
    /// a small capital or an oldstyle figure ([`name_form_base`]); each glyph is then read only
    /// where its own name confirms its strict text. A private use code point from `/ToUnicode`
    /// alone is no evidence: icon fonts use this range too (Font Awesome's U+F773, U+F7A2), and
    /// a font that names nothing takes no rule's time. A Type 0 font never qualifies: in the
    /// corpus its `/ToUnicode` maps into this range for other reasons (a Korean font's private
    /// use characters at U+F734 …, an OCR text layer's identity map), and expert fonts with
    /// small capitals are simple fonts. Fonts named as symbol fonts never qualify either.
    fn guard(font: &FontFacts<'_>) -> Option<()> {
        if !matches!(
            font.kind,
            FontKind::Type1 | FontKind::Cff | FontKind::TrueType | FontKind::Type3
        ) || symbol_font(font.base_font)
        {
            return None;
        }
        let names = font.names?;
        (0..=u8::MAX)
            .filter_map(names)
            .any(|name| name_form_base(name).is_some())
            .then_some(())
    }

    fn read(_: &(), _: &FontFacts<'_>, glyph: &GlyphFacts<'_>) -> Option<(BfString, RecoveryMode)> {
        // One private use small capital or oldstyle figure, which the glyph's own name confirms;
        // a string of several characters is left as it is.
        let Some(BfString::Char(c)) = glyph.strict else {
            return None;
        };
        let (base, _) = small_cap_base(*c)?;
        if glyph.name.and_then(name_form_base) != Some(base) {
            return None;
        }
        Some((
            BfString::Char(base),
            RecoveryMode::Override {
                reason: PRIVATE_FORM,
            },
        ))
    }
}

/// Whether recovered text read by `rule` is shown in small capitals: [`AdobeSmallCaps`]
/// reading one small capital letter (a string of several characters is never styled so).
pub(crate) fn shown_small(rule: RecoveryRule, strict: Option<&BfString>) -> bool {
    rule == RecoveryRule::AdobeSmallCaps
        && matches!(strict, Some(BfString::Char(c)) if small_cap_base(*c).is_some_and(|(_, small)| small))
}
