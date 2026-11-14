//! PdfCraft patch (107): the standard glyph orders and encodings the recovery rules read, in one
//! place, each read through the Adobe Glyph List ([`glyph_name_to_unicode`]) and the candidate
//! policy ([`super::candidate`]):
//!
//! - **The standard Macintosh glyph order**: the 258 glyph names a TrueType `post` table of
//!   format 1 implies (Apple's TrueType Reference Manual, `post` table), read-fonts'
//!   `DEFAULT_GLYPH_NAMES` (`space` is 3, `A` 36, `a` 68).
//! - **The CFF standard strings**: the 391 strings every CFF font shares, string ids (SIDs) 0 to
//!   390 (Adobe Technical Note #5176, *The Compact Font Format Specification*, Appendix A),
//!   read-fonts' `STANDARD_STRINGS` (`space` is 1, `A` 34, `a` 66). Its first 229 (0 to 228) are
//!   also the ISOAdobe charset, in that order (Appendix C).
//! - **The Symbol font's built-in encoding** (ISO 32000-2 Annex D.5), hayro-interpret's own
//!   table.
//!
//! Both glyph-name tables come from read-fonts, already a dependency, so there is one copy of
//! each; pdfcraft-render's `text::recovery_tests` read them through the rules.

use super::candidate;
use crate::font::generated::symbol;
use crate::font::glyph_name_to_unicode;
use skrifa::raw::ps::string::STANDARD_STRINGS;
use skrifa::raw::tables::post::DEFAULT_GLYPH_NAMES;

/// How many glyphs the standard Macintosh order names.
#[allow(
    dead_code,
    reason = "read by the standard-order rules that build on this layer"
)]
pub(crate) const MAC_GLYPH_COUNT: u32 = 258;

/// The glyph name glyph id `gid` has in the standard Macintosh order; nothing past its 258.
pub(crate) fn mac_glyph_name(gid: u32) -> Option<&'static str> {
    DEFAULT_GLYPH_NAMES.get(usize::try_from(gid).ok()?).copied()
}

/// The character the standard Macintosh order gives glyph id `gid`, through the Adobe Glyph
/// List and the candidate policy: nothing past the 258 standard glyphs, for a control (`.null`)
/// or for a private use character (`apple`).
pub(crate) fn mac_glyph_char(gid: u32) -> Option<char> {
    candidate::choose(glyph_name_to_unicode(mac_glyph_name(gid)?))
}

/// The CFF standard string with id `sid`; nothing past the 391 (TN #5176 Appendix A).
#[allow(
    dead_code,
    reason = "read by the standard-order rules that build on this layer"
)]
pub(crate) fn standard_string(sid: u32) -> Option<&'static str> {
    STANDARD_STRINGS.get(usize::try_from(sid).ok()?).copied()
}

/// The character the CFF standard string `sid` names as a glyph name, through the Adobe Glyph
/// List and the candidate policy: nothing for `.notdef`, for the strings that name no character
/// in the glyph list (`001.000`, `Black`) or for one it puts in the private use area (the
/// expert set's small capitals, `Asmall`).
#[allow(
    dead_code,
    reason = "read by the standard-order rules that build on this layer"
)]
pub(crate) fn standard_string_char(sid: u32) -> Option<char> {
    if sid == 0 {
        return None;
    }
    candidate::choose(glyph_name_to_unicode(standard_string(sid)?))
}

/// The character the Symbol font's built-in encoding gives `code`, through the Adobe Glyph
/// List. The glyph list puts the Symbol font's serif and sans-serif ©, ® and ™ in the private
/// use area; they are the plain characters. Other private-use glyphs (the pieces of large
/// brackets and braces) read as nothing.
pub(crate) fn symbol_char(code: u8) -> Option<char> {
    let name = symbol::get(code)?;
    let c = match name {
        "copyrightserif" | "copyrightsans" => '\u{a9}',
        "registerserif" | "registersans" => '\u{ae}',
        "trademarkserif" | "trademarksans" => '\u{2122}',
        _ => glyph_name_to_unicode(name)?,
    };
    candidate::choose([c])
}
