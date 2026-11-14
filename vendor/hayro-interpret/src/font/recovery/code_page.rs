//! PdfCraft patch (107): [`super::RecoveryRule::CodePageByOutline`], text shown in a Windows code
//! page through a font that reads as WinAnsi.
//!
//! Fonts made for Central European and Baltic Windows (`Times New Roman CE`, `Arial Baltic`, and
//! subsets of them under any name) draw the letters of Windows-1250 or Windows-1257 at those code
//! pages' codes: `č` at 0xE8, `ő` at 0xF5, `ā` at 0xE2. Many were embedded with `/Encoding
//! /WinAnsiEncoding`, or with the Latin-1 glyph names at those codes (`egrave`, `otilde`,
//! `acircumflex`) and a `/ToUnicode` map that repeats them: what the PDF says each code is (ISO
//! 32000-2 §9.6.5.2, §9.6.5.4 and Annex D.2; §9.10.2 and §9.10.3) is not what its glyph draws. The
//! strict text keeps what the PDF says; this rule reads what the glyphs draw, for Copy with
//! formatting.
//!
//! The evidence is the font's own outlines, drawn by the same parsers that render them; no other
//! font is consulted. At each code from 0x80 on whose strict text is the Windows-1252 character,
//! the glyph the code draws is classified by its accent ([`super::accents`]) and compared with the
//! character each code page has at that code: a caron where Windows-1250 has `č` and Windows-1252
//! `è` is a vote for Windows-1250, a grave a vote for Windows-1252. Windows-1254 and Mac OS
//! Central European are counted too, as rivals: a font they explain at least as well is not read
//! ([`decide`]).
//!
//! Every glyph the program has at those codes votes, not only the ones a page shows: the decision
//! is made once per font, before any page's text is known. A subset has the glyphs its document
//! uses; a font embedded whole that replaced only some letters of Windows-1252 with a code page's
//! keeps Windows-1252 glyphs at the other codes, and is not read.

use super::accents::{self, Mark, Shape};
use super::{Counter, FontFacts, FontKind, GlyphFacts, RecoveryMode, RecoveryRule, Rule, count};
use super::{symbol_font, untagged};
use crate::font::generated::glyph_names;
use crate::font::standard_font::StandardFont;
use hayro_cmap::BfString;
use kurbo::{BezPath, PathEl};

/// Windows-1252, codes 0x80 to 0xFF (0: no character).
const CP1252: [u16; 128] = [
    0x20AC, 0x0000, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x0000, 0x017D, 0x0000, 0x0000, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x0000, 0x017E, 0x0178, 0x00A0, 0x00A1, 0x00A2, 0x00A3,
    0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00BA, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x00BF, 0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF, 0x00D0, 0x00D1, 0x00D2, 0x00D3,
    0x00D4, 0x00D5, 0x00D6, 0x00D7, 0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB,
    0x00EC, 0x00ED, 0x00EE, 0x00EF, 0x00F0, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x00FF,
];

/// Windows-1250, codes 0x80 to 0xFF (0: no character).
const CP1250: [u16; 128] = [
    0x20AC, 0x0000, 0x201A, 0x0000, 0x201E, 0x2026, 0x2020, 0x2021, 0x0000, 0x2030, 0x0160, 0x2039,
    0x015A, 0x0164, 0x017D, 0x0179, 0x0000, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0000, 0x2122, 0x0161, 0x203A, 0x015B, 0x0165, 0x017E, 0x017A, 0x00A0, 0x02C7, 0x02D8, 0x0141,
    0x00A4, 0x0104, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x015E, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x017B,
    0x00B0, 0x00B1, 0x02DB, 0x0142, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x0105, 0x015F, 0x00BB,
    0x013D, 0x02DD, 0x013E, 0x017C, 0x0154, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x0139, 0x0106, 0x00C7,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x011A, 0x00CD, 0x00CE, 0x010E, 0x0110, 0x0143, 0x0147, 0x00D3,
    0x00D4, 0x0150, 0x00D6, 0x00D7, 0x0158, 0x016E, 0x00DA, 0x0170, 0x00DC, 0x00DD, 0x0162, 0x00DF,
    0x0155, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x013A, 0x0107, 0x00E7, 0x010D, 0x00E9, 0x0119, 0x00EB,
    0x011B, 0x00ED, 0x00EE, 0x010F, 0x0111, 0x0144, 0x0148, 0x00F3, 0x00F4, 0x0151, 0x00F6, 0x00F7,
    0x0159, 0x016F, 0x00FA, 0x0171, 0x00FC, 0x00FD, 0x0163, 0x02D9,
];

/// Windows-1257, codes 0x80 to 0xFF (0: no character).
const CP1257: [u16; 128] = [
    0x20AC, 0x0000, 0x201A, 0x0000, 0x201E, 0x2026, 0x2020, 0x2021, 0x0000, 0x2030, 0x0000, 0x2039,
    0x0000, 0x00A8, 0x02C7, 0x00B8, 0x0000, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0000, 0x2122, 0x0000, 0x203A, 0x0000, 0x00AF, 0x02DB, 0x0000, 0x00A0, 0x0000, 0x00A2, 0x00A3,
    0x00A4, 0x0000, 0x00A6, 0x00A7, 0x00D8, 0x00A9, 0x0156, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00C6,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00F8, 0x00B9, 0x0157, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x00E6, 0x0104, 0x012E, 0x0100, 0x0106, 0x00C4, 0x00C5, 0x0118, 0x0112,
    0x010C, 0x00C9, 0x0179, 0x0116, 0x0122, 0x0136, 0x012A, 0x013B, 0x0160, 0x0143, 0x0145, 0x00D3,
    0x014C, 0x00D5, 0x00D6, 0x00D7, 0x0172, 0x0141, 0x015A, 0x016A, 0x00DC, 0x017B, 0x017D, 0x00DF,
    0x0105, 0x012F, 0x0101, 0x0107, 0x00E4, 0x00E5, 0x0119, 0x0113, 0x010D, 0x00E9, 0x017A, 0x0117,
    0x0123, 0x0137, 0x012B, 0x013C, 0x0161, 0x0144, 0x0146, 0x00F3, 0x014D, 0x00F5, 0x00F6, 0x00F7,
    0x0173, 0x0142, 0x015B, 0x016B, 0x00FC, 0x017C, 0x017E, 0x02D9,
];

/// Windows-1254, codes 0x80 to 0xFF (0: no character).
const CP1254: [u16; 128] = [
    0x20AC, 0x0000, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x0000, 0x0000, 0x0000, 0x0000, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x0000, 0x0000, 0x0178, 0x00A0, 0x00A1, 0x00A2, 0x00A3,
    0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00BA, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x00BF, 0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF, 0x011E, 0x00D1, 0x00D2, 0x00D3,
    0x00D4, 0x00D5, 0x00D6, 0x00D7, 0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x0130, 0x015E, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB,
    0x00EC, 0x00ED, 0x00EE, 0x00EF, 0x011F, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x0131, 0x015F, 0x00FF,
];

/// Mac OS Central European, codes 0x80 to 0xFF (0: no character).
const MAC_CE: [u16; 128] = [
    0x00C4, 0x0100, 0x0101, 0x00C9, 0x0104, 0x00D6, 0x00DC, 0x00E1, 0x0105, 0x010C, 0x00E4, 0x010D,
    0x0106, 0x0107, 0x00E9, 0x0179, 0x017A, 0x010E, 0x00ED, 0x010F, 0x0112, 0x0113, 0x0116, 0x00F3,
    0x0117, 0x00F4, 0x00F6, 0x00F5, 0x00FA, 0x011A, 0x011B, 0x00FC, 0x2020, 0x00B0, 0x0118, 0x00A3,
    0x00A7, 0x2022, 0x00B6, 0x00DF, 0x00AE, 0x00A9, 0x2122, 0x0119, 0x00A8, 0x2260, 0x0123, 0x012E,
    0x012F, 0x012A, 0x2264, 0x2265, 0x012B, 0x0136, 0x2202, 0x2211, 0x0142, 0x013B, 0x013C, 0x013D,
    0x013E, 0x0139, 0x013A, 0x0145, 0x0146, 0x0143, 0x00AC, 0x221A, 0x0144, 0x0147, 0x2206, 0x00AB,
    0x00BB, 0x2026, 0x00A0, 0x0148, 0x0150, 0x00D5, 0x0151, 0x014C, 0x2013, 0x2014, 0x201C, 0x201D,
    0x2018, 0x2019, 0x00F7, 0x25CA, 0x014D, 0x0154, 0x0155, 0x0158, 0x2039, 0x203A, 0x0159, 0x0156,
    0x0157, 0x0160, 0x201A, 0x201E, 0x0161, 0x015A, 0x015B, 0x00C1, 0x0164, 0x0165, 0x00CD, 0x017D,
    0x017E, 0x016A, 0x00D3, 0x00D4, 0x016B, 0x016E, 0x00DA, 0x016F, 0x0170, 0x0171, 0x0172, 0x0173,
    0x00DD, 0x00FD, 0x0137, 0x017B, 0x0141, 0x017C, 0x0122, 0x02C7,
];

/// A code page a font's codes may be in, besides Windows-1252.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodePage {
    /// Windows-1250, Central European.
    Windows1250,
    /// Windows-1257, Baltic.
    Windows1257,
    /// Windows-1254, Turkish: a rival only.
    Windows1254,
    /// Mac OS Central European: a rival only.
    MacCentralEurope,
}

impl CodePage {
    /// Every code page compared, in the order a tie is broken.
    pub(crate) const ALL: [Self; 4] = [
        Self::Windows1250,
        Self::Windows1257,
        Self::Windows1254,
        Self::MacCentralEurope,
    ];

    fn table(self) -> &'static [u16; 128] {
        match self {
            Self::Windows1250 => &CP1250,
            Self::Windows1257 => &CP1257,
            Self::Windows1254 => &CP1254,
            Self::MacCentralEurope => &MAC_CE,
        }
    }

    /// Whether the rule reads a font in this code page; the others are only rivals.
    fn read(self) -> bool {
        matches!(self, Self::Windows1250 | Self::Windows1257)
    }

    /// The character at `code`, 0x80 or above (below, every page compared is ASCII).
    pub(crate) fn decode(self, code: u8) -> Option<char> {
        high(self.table(), code)
    }
}

/// The character `table` has at `code`, 0x80 or above.
fn high(table: &[u16; 128], code: u8) -> Option<char> {
    let at = usize::from(code.checked_sub(0x80)?);
    table
        .get(at)
        .copied()
        .filter(|u| *u != 0)
        .and_then(|u| char::from_u32(u32::from(u)))
}

/// The Windows-1252 (WinAnsi) character at `code`, 0x80 or above.
pub(crate) fn windows_1252(code: u8) -> Option<char> {
    high(&CP1252, code)
}

/// Whether `c` is how a font that reads as WinAnsi reads `code`, 0x80 or above: Windows-1252's
/// character, or what ISO 32000-2 Annex D.2 gives WinAnsiEncoding besides: the ordinary space
/// and hyphen at 0xA0 and 0xAD (as `Encoding::text_alias` reads them), and the bullet at the codes
/// Windows-1252 leaves undefined.
fn reads_as_winansi(code: u8, c: char) -> bool {
    match windows_1252(code) {
        Some(w) => c == w || (code == 0xA0 && c == ' ') || (code == 0xAD && c == '-'),
        None => c == '\u{2022}',
    }
}

/// Most outline points the rule draws for one font, all its codes together. Real accented letters
/// take tens to a few hundred points each; a font past this is not read, so a page of such fonts
/// can't keep the rule busy. With the classifier's 4,096 points a glyph, it bounds one font's
/// work.
const MAX_FONT_POINTS: usize = 1 << 16;

/// The points of `path` (on the curve and off it), as the classifier counts them.
fn points(path: &BezPath) -> usize {
    path.elements()
        .iter()
        .map(|e| match e {
            PathEl::MoveTo(_) | PathEl::LineTo(_) => 1,
            PathEl::QuadTo(..) => 2,
            PathEl::CurveTo(..) => 3,
            PathEl::ClosePath => 0,
        })
        .fold(0_usize, usize::saturating_add)
}

/// Why the rule replaces the strict text.
const CODE_PAGE: &str =
    "the glyphs draw the letters of a Windows code page at codes the font reads as WinAnsi";

/// The votes for one code page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    /// Codes whose glyph draws this code page's character and not Windows-1252's.
    pub(crate) target: u32,
    /// Codes whose glyph draws Windows-1252's character and not this code page's.
    pub(crate) windows_1252: u32,
    /// Codes whose glyph draws neither, or both.
    pub(crate) neither: u32,
}

/// The vote of a glyph of `shape` at a code where a code page has `page` and Windows-1252 has
/// `shown`.
fn vote(tally: &mut Tally, shape: Shape, page: char, shown: char) {
    let target = accents::matches(shape, page) == Some(true);
    let windows = accents::matches(shape, shown) == Some(true);
    let slot = match (target, windows) {
        (true, false) => &mut tally.target,
        (false, true) => &mut tally.windows_1252,
        _ => &mut tally.neither,
    };
    *slot = slot.saturating_add(1);
}

/// The code page the votes choose, rivals included: among the code pages with no Windows-1252
/// vote and more target votes than votes for neither, the one with the most target votes, at
/// least two. A tie is decided only when the tied code pages read every
/// code that voted alike (`same`), for the first of them in [`CodePage::ALL`].
pub(crate) fn decide(
    tallies: &[(CodePage, Tally)],
    same: impl Fn(CodePage, CodePage) -> bool,
) -> Option<CodePage> {
    let open = tallies
        .iter()
        .filter(|(_, t)| t.windows_1252 == 0 && t.neither < t.target);
    let best = open.clone().map(|(_, t)| t.target).max()?;
    if best < 2 {
        return None;
    }
    let mut winners = open.filter(|(_, t)| t.target == best).map(|(p, _)| *p);
    let first = winners.next()?;
    winners.all(|p| same(first, p)).then_some(first)
}

/// Whether `/ToUnicode` text `mapped` for `code` only repeats a reading of the code the font
/// gives anyway: its encoding's (`encoding`), Windows-1252's or Latin-1's.
fn repeats_a_reading(mapped: &BfString, code: u8, encoding: Option<&BfString>) -> bool {
    let latin1 = char::from(code);
    let windows = if code < 0x80 {
        Some(latin1)
    } else {
        windows_1252(code)
    };
    encoding == Some(mapped)
        || matches!(mapped, BfString::Char(c) if *c == latin1 || Some(*c) == windows)
}

/// A font's strict text for `code` when it is one character: its `/ToUnicode` text, else its
/// encoding's.
fn strict_char(font: &FontFacts<'_>, code: u8) -> Option<char> {
    let text = font
        .to_unicode_text
        .and_then(|t| t(code))
        .or_else(|| font.encoding_text.and_then(|e| e(code)))?;
    match text {
        BfString::Char(c) => Some(c),
        BfString::String(_) => None,
    }
}

/// The standard font whose widths a font of this name shares, for the width check
/// ([`widths_say_windows_1252`]): Helvetica's for Arial, Times' for Times New Roman (ISO 32000-2
/// §9.6.2.2, the standard 14 fonts' metrics). Narrow, condensed, light and black cuts have widths
/// of their own.
fn standard_widths(base: Option<&str>) -> Option<StandardFont> {
    let name: String = untagged(base?)
        .chars()
        .filter(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if ["narrow", "cond", "light", "black", "rounded"]
        .iter()
        .any(|s| name.contains(s))
    {
        return None;
    }
    let bold = ["bold", "heavy", "demi"].iter().any(|s| name.contains(s));
    let italic = ["italic", "oblique"].iter().any(|s| name.contains(s));
    if name.starts_with("arial") || name.starts_with("helvetica") {
        Some(match (bold, italic) {
            (false, false) => StandardFont::Helvetica,
            (true, false) => StandardFont::HelveticaBold,
            (false, true) => StandardFont::HelveticaOblique,
            (true, true) => StandardFont::HelveticaBoldOblique,
        })
    } else if name.starts_with("times") {
        Some(match (bold, italic) {
            (false, false) => StandardFont::TimesRoman,
            (true, false) => StandardFont::TimesBold,
            (false, true) => StandardFont::TimesItalic,
            (true, true) => StandardFont::TimesBoldItalic,
        })
    } else {
        None
    }
}

/// Whether the font's `/Widths` say its codes are Windows-1252's rather than `page`'s: in a font
/// of a family whose widths a standard font shares ([`standard_widths`]), at least two of the
/// `codes` that voted are as wide (to 2 units) as Windows-1252's character there and not as
/// `page`'s, and none the other way round.
fn widths_say_windows_1252(font: &FontFacts<'_>, page: CodePage, codes: &[u8]) -> bool {
    let (Some(width), Some(standard)) = (font.code_width, standard_widths(font.base_font)) else {
        return false;
    };
    let standard_width = |c: char| glyph_names::get_reverse(c).and_then(|n| standard.get_width(n));
    let (mut target, mut windows) = (0_u32, 0_u32);
    for &code in codes {
        let (Some(w), Some(a), Some(x)) = (width(code), page.decode(code), windows_1252(code))
        else {
            continue;
        };
        let (Some(wa), Some(wx)) = (standard_width(a), standard_width(x)) else {
            continue;
        };
        match ((w - wa).abs() <= 2.0, (w - wx).abs() <= 2.0) {
            (true, false) => target = target.saturating_add(1),
            (false, true) => windows = windows.saturating_add(1),
            _ => {}
        }
    }
    windows >= 2 && target == 0
}

/// What [`CodePageByOutline`] decided for a font: the code page it reads, and the codes
/// Windows-1252 leaves undefined whose glyphs draw that code page's letter (bit `code - 0x80`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Reading {
    page: CodePage,
    fills: u128,
}

/// [`super::RecoveryRule::CodePageByOutline`]: see the module documentation.
///
/// A simple font (Type 1, CFF or TrueType) with an embedded program, not named as a symbol font,
/// qualifies when:
///
/// - its `/ToUnicode` map, if it has one, only repeats readings the font gives anyway: each code
///   it maps reads so by the font's encoding, by Windows-1252 or by Latin-1. A map that says
///   anything else is the producer's word, and the font is left alone;
/// - its codes from 0x80 on read as WinAnsi ([`reads_as_winansi`]): a font that reads two or more
///   of them as letters of another encoding (Mac OS Roman's `Adieresis` and `Aring` at 0x80 and
///   0x81) has an encoding of its own;
/// - the glyphs at those codes whose strict text is Windows-1252's character choose Windows-1250
///   or Windows-1257 by their accents ([`decide`]): at least two codes draw that code page's
///   letter, none draws Windows-1252's, fewer draw neither, and no rival explains them as well;
/// - in a family with standard widths, its `/Widths` don't say Windows-1252
///   ([`widths_say_windows_1252`]).
///
/// It then reads each code from 0x80 on whose strict text is Windows-1252's character, where the
/// code page has a letter, as that letter, overriding the strict text (a spacing accent such as
/// Windows-1250's `˝` at 0xBD, where a `½` may be drawn, is left alone). A code Windows-1252
/// leaves undefined (WinAnsi's bullet, or no text) is read only where its own glyph doesn't
/// contradict the code page's letter (another mark, or a sign floating above the baseline).
pub(crate) struct CodePageByOutline;

impl Rule for CodePageByOutline {
    const RULE: RecoveryRule = RecoveryRule::CodePageByOutline;
    const KINDS: &'static [FontKind] = &[FontKind::Type1, FontKind::Cff, FontKind::TrueType];
    type Guard = Reading;

    fn guard(font: &FontFacts<'_>) -> Option<Reading> {
        if symbol_font(font.base_font) {
            return None;
        }
        let draw = font.code_outline?;
        // The outline `code` draws; `None` once the font has drawn more than `MAX_FONT_POINTS`,
        // which ends the guard with no reading.
        let left = std::cell::Cell::new(MAX_FONT_POINTS);
        let outline = |code: u8| -> Option<Option<BezPath>> {
            let path = draw(code);
            let used = path.as_ref().map_or(0, points);
            left.set(left.get().checked_sub(used)?);
            Some(path)
        };
        let encoding = font.encoding_text?;
        if font.has_to_unicode {
            let to_unicode = font.to_unicode_text?;
            for code in 0..=u8::MAX {
                if let Some(mapped) = to_unicode(code)
                    && !repeats_a_reading(&mapped, code, encoding(code).as_ref())
                {
                    return None;
                }
            }
        }
        // A font whose codes from 0x80 on read as the letters of another encoding (Mac OS Roman
        // glyph names: `Adieresis` at 0x80, `Aring` at 0x81) is no candidate. A producer's own
        // names for a punctuation mark (`endash` at 0x85) or for one letter don't make it one.
        let other = (0x80..=u8::MAX)
            .filter(|&code| {
                strict_char(font, code)
                    .is_some_and(|c| c.is_alphabetic() && !reads_as_winansi(code, c))
            })
            .count();
        if other > 1 {
            return None;
        }
        let mut tallies = CodePage::ALL.map(|p| (p, Tally::default()));
        let mut voted = Vec::new();
        for code in 0x80..=u8::MAX {
            let Some(shown) = strict_char(font, code).filter(|c| Some(*c) == windows_1252(code))
            else {
                continue;
            };
            let differs = |p: CodePage| p.decode(code).is_some_and(|a| a != shown);
            if !CodePage::ALL.into_iter().any(differs) {
                continue;
            }
            let Some(path) = outline(code)? else {
                continue;
            };
            count(Counter::OutlineClassified);
            let Some(shape) = accents::classify(&path) else {
                continue;
            };
            voted.push(code);
            for (page, tally) in &mut tallies {
                if let Some(a) = page.decode(code).filter(|a| *a != shown) {
                    vote(tally, shape, a, shown);
                }
            }
        }
        let same = |a: CodePage, b: CodePage| voted.iter().all(|&c| a.decode(c) == b.decode(c));
        let page = decide(&tallies, same).filter(|p| p.read())?;
        if widths_say_windows_1252(font, page, &voted) {
            return None;
        }
        // The codes Windows-1252 leaves undefined (WinAnsi's bullet, or no text) whose own glyphs
        // don't contradict the code page's letter: they draw its mark, or a letter whose mark the
        // classifier can't place (the apostrophe of a `ť` that starts below the `t`'s top), not
        // another mark or a sign floating above the baseline (a bullet).
        let mut fills = 0_u128;
        for code in 0x80..=u8::MAX {
            let Some(letter) = page.decode(code).filter(|_| windows_1252(code).is_none()) else {
                continue;
            };
            if !strict_char(font, code).is_none_or(|c| c == '\u{2022}') {
                continue;
            }
            let Some(path) = outline(code)? else {
                continue;
            };
            count(Counter::OutlineClassified);
            let letter_like = accents::classify(&path).is_some_and(|shape| {
                accents::matches(shape, letter) == Some(true)
                    || matches!(shape.mark, Mark::Nothing | Mark::Other)
            });
            if letter_like {
                fills |= 1_u128 << (code & 0x7F);
            }
        }
        Some(Reading { page, fills })
    }

    fn read(
        reading: &Reading,
        _: &FontFacts<'_>,
        glyph: &GlyphFacts<'_>,
    ) -> Option<(BfString, RecoveryMode)> {
        let code = glyph.code.filter(|c| *c >= 0x80)?;
        let read = reading.page.decode(code).filter(|c| c.is_alphabetic())?;
        let windows = windows_1252(code);
        let filled = reading.fills & (1_u128 << (code & 0x7F)) != 0;
        let replaces = match glyph.strict {
            Some(BfString::Char(c)) if windows.is_some() => Some(*c) == windows,
            Some(BfString::Char('\u{2022}')) | None => windows.is_none() && filled,
            _ => false,
        };
        replaces.then_some((
            BfString::Char(read),
            RecoveryMode::Override { reason: CODE_PAGE },
        ))
    }
}
