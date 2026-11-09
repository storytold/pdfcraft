//! PdfCraft patch (104): recovered text, for glyphs whose strict text is missing or
//! untrustworthy.
//!
//! [`crate::font::Glyph::as_unicode`] reads a glyph's text the way ISO 32000-2 §9.10.2 lays
//! out: `/ToUnicode`, then a known encoding or glyph name. Search, plain copy and text
//! extraction keep that strict reading. Some fonts still say which character a glyph draws in
//! a way the specification doesn't define. The rules here read those, for callers that ask
//! for recovered text explicitly ([`crate::font::Glyph::unicode_and_recovery`]); nothing here
//! changes `as_unicode`.
//!
//! Each rule ([`Rule`]) has two halves:
//!
//! - a font-level guard, decided once per font from a [`FontFacts`] and cached
//!   ([`RecoveryCache`]): whether the font qualifies at all, and anything the rule wants to
//!   keep from that decision;
//! - a per-glyph reading from a [`GlyphFacts`], which also says whether the rule only fills a
//!   glyph without strict text ([`RecoveryMode::Fill`], the default) or replaces strict text it
//!   judges untrustworthy ([`RecoveryMode::Override`], with the reason).
//!
//! Rules are named by the mechanism they read, never by a language or a producer, and tried
//! in [`RecoveryRule::ORDER`]: the first that reads a glyph wins.

use crate::font::FontFlags;
use crate::font::generated::symbol;
use crate::font::{glyph_name_numbers, glyph_name_to_unicode, is_numbered_glyph_name};
use hayro_cmap::BfString;
use hayro_syntax::object::dict::keys::{
    BASE_FONT, ENCODING, FLAGS, FONT_DESC, FONT_WEIGHT, ITALIC_ANGLE,
};
use hayro_syntax::object::{Dict, Name};
use skrifa::raw::TableProvider;
use skrifa::raw::tables::post::DEFAULT_GLYPH_NAMES;
use skrifa::raw::types::Version16Dot16;
use skrifa::{FontRef, GlyphId};
use std::sync::{Arc, OnceLock};

/// A way to recover a glyph's text, named by the mechanism it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecoveryRule {
    /// Glyph names that number Latin-1 characters in decimal (`/C83` for `S`, `/MT252` for
    /// `ü`), whatever code shows them, in a simple Type 1 or CFF font: see
    /// [`NumberedLatin1Names`].
    NumberedLatin1Names,
    /// Glyph ids in the standard Macintosh glyph order (the 258 names of the TrueType `post`
    /// table, format 1), from a TrueType program that keeps its original glyph ids, or from
    /// glyph names that only number the glyph id (`/g55`): see [`MacGlyphOrder`].
    MacGlyphOrder,
}

impl RecoveryRule {
    /// Every rule, in the order they are tried.
    pub const ORDER: [Self; 2] = [Self::NumberedLatin1Names, Self::MacGlyphOrder];

    /// A stable name for logs and tests.
    pub fn name(self) -> &'static str {
        match self {
            Self::NumberedLatin1Names => "numbered-latin1-names",
            Self::MacGlyphOrder => "mac-glyph-order",
        }
    }
}

#[cfg(feature = "test-counters")]
thread_local! {
    /// Asks of the recovery rules (`RecoveryCache::recover`, before any guard is decided) on
    /// this thread.
    static RULE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times the recovery rules were asked about a glyph on this thread, guards
/// included: for tests that show a reader runs no recovery code at all. Only with the
/// `test-counters` feature (a dev-dependency feature; release builds count nothing).
#[cfg(feature = "test-counters")]
pub fn recovery_rule_calls() -> usize {
    RULE_CALLS.with(std::cell::Cell::get)
}

/// Whether a recovered reading fills a glyph that has no strict text or replaces its strict
/// text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryMode {
    /// The glyph had no strict text (or only control characters).
    Fill,
    /// The glyph's strict text is replaced, for `reason`.
    Override {
        /// Why the strict text is not to be trusted.
        reason: &'static str,
    },
}

/// A glyph's recovered text and how it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovery {
    /// The recovered text.
    pub text: BfString,
    /// The rule that read it.
    pub rule: RecoveryRule,
    /// Whether it fills an empty glyph or overrides strict text.
    pub mode: RecoveryMode,
}

/// The kind of font a glyph comes from, as far as the rules care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FontKind {
    /// A simple Type 1 font, embedded (`/FontFile`) or standard.
    Type1,
    /// A simple font with a CFF program (`/FontFile3`).
    Cff,
    /// A simple TrueType font.
    TrueType,
    /// A Type 0 font whose descendant has a TrueType (or OpenType) program.
    CidTrueType,
    /// Any other Type 0 font.
    CidOther,
}

/// What a font says about itself, for the rules' font-level guards and readings. Built
/// cheaply from borrowed parts of the font; the program is parsed only by guards that look at
/// it.
pub(crate) struct FontFacts<'a> {
    pub(crate) kind: FontKind,
    /// `/BaseFont` as written, subset tag included (readers strip it themselves).
    pub(crate) base_font: Option<&'a str>,
    /// The font descriptor's `/Flags`.
    pub(crate) flags: Option<&'a FontFlags>,
    /// Whether the font has a `/ToUnicode` map (or reads Unicode in a way of its own, such as
    /// a UCS2 CMap or `/ToUnicode /Identity-H`).
    pub(crate) has_to_unicode: bool,
    /// Simple fonts: whether `/Encoding` is present (a name or a dictionary); always `false`
    /// for a Type 0 font, whose `/Encoding` is its CMap.
    pub(crate) has_encoding: bool,
    /// Simple fonts: the glyph name the encoding (with `/Differences`) gives a code.
    pub(crate) names: Option<&'a dyn Fn(u8) -> Option<&'a str>>,
    /// The embedded TrueType or OpenType program, when there is one.
    pub(crate) program: Option<&'a FontRef<'a>>,
    /// Simple TrueType fonts: the glyph a code draws and the code's strict text.
    pub(crate) code_glyph: Option<&'a dyn Fn(u8) -> (GlyphId, Option<BfString>)>,
}

/// What one glyph says, for a rule's reading.
///
/// A rule reading the embedded program's own Unicode `cmap` in reverse (glyph id to character)
/// would fit here unchanged: its guard builds the reverse map once per font from
/// [`FontFacts::program`], and its reading looks up [`GlyphFacts::gid`], overriding strict text
/// only for a reason decided in the guard (a degenerate `/ToUnicode`, say). Add the character
/// code or CID here when a rule needs them.
pub(crate) struct GlyphFacts<'a> {
    /// The glyph id the code draws.
    pub(crate) gid: GlyphId,
    /// Simple fonts: the code's glyph name.
    pub(crate) name: Option<&'a str>,
    /// The strict text, when it is more than control characters.
    pub(crate) strict: Option<&'a BfString>,
}

/// One recovery rule.
pub(crate) trait Rule {
    /// Which rule this is.
    const RULE: RecoveryRule;
    /// What the font-level decision keeps.
    type Guard;
    /// Whether the font qualifies, decided once per font.
    fn guard(font: &FontFacts<'_>) -> Option<Self::Guard>;
    /// The glyph's text, and whether it fills or overrides. A rule that only fills returns
    /// [`RecoveryMode::Fill`]; its reading is then used only for glyphs without strict text.
    fn read(
        guard: &Self::Guard,
        font: &FontFacts<'_>,
        glyph: &GlyphFacts<'_>,
    ) -> Option<(BfString, RecoveryMode)>;
}

/// Every rule's font-level decision for one font.
pub(crate) struct Guards {
    numbered: Option<<NumberedLatin1Names as Rule>::Guard>,
    mac: Option<<MacGlyphOrder as Rule>::Guard>,
}

impl Guards {
    fn decide(font: &FontFacts<'_>) -> Self {
        Self {
            numbered: NumberedLatin1Names::guard(font),
            mac: MacGlyphOrder::guard(font),
        }
    }

    /// Whether no rule takes the font: nothing is ever recovered from it.
    fn none(&self) -> bool {
        self.numbered.is_none() && self.mac.is_none()
    }
}

/// The font-level decisions of one font, made the first time a glyph of it asks.
#[derive(Debug, Default)]
pub(crate) struct RecoveryCache(OnceLock<Guards>);

/// A font's style for text the recovery rules read ([`crate::font::Glyph::recovery_style`]),
/// worked out once per font and shared by its glyphs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveredFontStyle {
    /// `/BaseFont` as written, subset tag included (readers strip it themselves).
    pub name: Option<Arc<str>>,
    /// The descriptor's ForceBold flag or `/FontWeight` of 600 or more, an embedded program's
    /// weight of 600 or more, or its `head.macStyle` bit 0.
    pub bold: bool,
    /// The descriptor's Italic flag or a slanted `/ItalicAngle`, an embedded program's own
    /// italic style, or its `head.macStyle` bit 1.
    pub italic: bool,
}

/// What a font keeps for the recovery rules: the decisions' cache, and the parts of its
/// dictionary [`FontFacts`] needs that the font doesn't keep otherwise.
#[derive(Debug, Default)]
pub(crate) struct FontRecovery {
    pub(crate) cache: RecoveryCache,
    /// `/BaseFont` as written, subset tag included (readers strip it themselves).
    pub(crate) base_font: Option<Arc<str>>,
    /// The font descriptor's `/Flags` (a Type 0 font's descendant's).
    pub(crate) flags: Option<FontFlags>,
    /// Simple fonts: whether `/Encoding` is present (`false` for a Type 0 font).
    pub(crate) has_encoding: bool,
    /// The descriptor's `/FontWeight` is 600 or more.
    descriptor_bold: bool,
    /// The descriptor's `/ItalicAngle` isn't upright.
    descriptor_italic: bool,
    /// The font's style for recovered text, worked out on first use.
    style: OnceLock<RecoveredFontStyle>,
}

impl FontRecovery {
    /// From a simple font's dictionary, or a Type 0 font's with its `descendant`.
    pub(crate) fn new(dict: &Dict<'_>, descendant: Option<&Dict<'_>>) -> Self {
        let base_font = dict
            .get::<Name<'_>>(BASE_FONT)
            .map(|n| Arc::from(n.as_str()));
        let descriptor = descendant.unwrap_or(dict).get::<Dict<'_>>(FONT_DESC);
        // Read as the editor reads `/Flags` (pdfcraft-fonts): any number, a real one truncated
        // (hayro-syntax's integer read takes `262144.0` as 262144); bits ISO 32000-2 leaves
        // undefined are dropped and the defined ones kept, so a stray bit doesn't cost the font
        // its ForceBold.
        let flags = descriptor
            .as_ref()
            .and_then(|d| d.get::<u32>(FLAGS))
            .map(FontFlags::from_bits_truncate);
        // The descriptor's weight and italic angle, read as the editor reads them, so the strict
        // and recovered glyphs of a font take the same weight and slant.
        let descriptor_bold = descriptor
            .as_ref()
            .and_then(|d| d.get::<f32>(FONT_WEIGHT))
            .is_some_and(|w| w >= 600.0);
        let descriptor_italic = descriptor
            .as_ref()
            .and_then(|d| d.get::<f32>(ITALIC_ANGLE))
            .is_some_and(|a| a.abs() > 0.1);
        Self {
            cache: RecoveryCache::default(),
            base_font,
            flags,
            descriptor_bold,
            descriptor_italic,
            has_encoding: descendant.is_none() && dict.contains_key(ENCODING),
            style: OnceLock::new(),
        }
    }

    /// The font's style for recovered text, from its flags and `program` (the embedded program
    /// and its metadata, asked for only the first time).
    pub(crate) fn style(
        &self,
        program: impl FnOnce() -> Option<crate::font::OutlineFontData>,
    ) -> RecoveredFontStyle {
        self.style
            .get_or_init(|| {
                let (mut bold, mut italic) = self.flags.as_ref().map_or((false, false), |f| {
                    (
                        f.contains(FontFlags::FORCE_BOLD),
                        f.contains(FontFlags::ITALIC),
                    )
                });
                bold |= self.descriptor_bold;
                italic |= self.descriptor_italic;
                if let Some(data) = program() {
                    bold |= data.weight.is_some_and(|w| w >= 600);
                    italic |= data.is_italic;
                    // `head.macStyle` bit 0 (bold) and bit 1 (italic): subsets that keep a
                    // regular OS/2 weight still mark their bold face here.
                    let bytes: &[u8] = (*data.data).as_ref();
                    if let Ok(head) = FontRef::new(bytes).and_then(|f| f.head()) {
                        let style = head.mac_style().bits();
                        bold |= style & 1 != 0;
                        italic |= style & 2 != 0;
                    }
                }
                RecoveredFontStyle {
                    name: self.base_font.clone(),
                    bold,
                    italic,
                }
            })
            .clone()
    }
}

impl std::fmt::Debug for Guards {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Guards")
            .field("numbered", &self.numbered.is_some())
            .field("mac", &self.mac.is_some())
            .finish()
    }
}

impl RecoveryCache {
    /// The recovered reading of one glyph: `facts` builds the font's facts (called for the
    /// guards the first time, and again only when some rule takes the font), `glyph` the
    /// glyph's (built only when some rule takes the font).
    pub(crate) fn recover<'a, 'g>(
        &self,
        facts: impl Fn() -> FontFacts<'a>,
        glyph: impl FnOnce() -> GlyphFacts<'g>,
    ) -> Option<Recovery> {
        #[cfg(feature = "test-counters")]
        RULE_CALLS.with(|c| c.set(c.get().saturating_add(1)));
        let guards = self.0.get_or_init(|| Guards::decide(&facts()));
        if guards.none() {
            return None;
        }
        let font = facts();
        let glyph = &glyph();
        RecoveryRule::ORDER.iter().find_map(|rule| match rule {
            RecoveryRule::NumberedLatin1Names => {
                apply::<NumberedLatin1Names>(guards.numbered.as_ref(), &font, glyph)
            }
            RecoveryRule::MacGlyphOrder => {
                apply::<MacGlyphOrder>(guards.mac.as_ref(), &font, glyph)
            }
        })
    }
}

/// Rule `R`'s reading of a glyph, under the fill/override policy: a reading of a glyph without
/// strict text is a fill whatever the rule says; strict text is replaced only by a rule that
/// overrides, and only with different text.
pub(crate) fn apply<R: Rule>(
    guard: Option<&R::Guard>,
    font: &FontFacts<'_>,
    glyph: &GlyphFacts<'_>,
) -> Option<Recovery> {
    let (text, mode) = R::read(guard?, font, glyph)?;
    recovery_policy(R::RULE, glyph.strict, text, mode)
}

/// The fill/override policy every rule's reading passes through: over a glyph without strict
/// text (`strict` is `None`: none, or only control characters) the reading is a fill whatever
/// the rule said; strict text is replaced only by a reading marked [`RecoveryMode::Override`],
/// and only when the text differs. Public so the policy can be tested from outside the crate
/// with a synthetic rule's reading.
pub fn recovery_policy(
    rule: RecoveryRule,
    strict: Option<&BfString>,
    text: BfString,
    mode: RecoveryMode,
) -> Option<Recovery> {
    let mode = match (strict, mode) {
        (None, _) => RecoveryMode::Fill,
        (Some(_), RecoveryMode::Fill) => return None,
        (Some(strict), RecoveryMode::Override { .. }) if *strict == text => return None,
        (Some(_), m @ RecoveryMode::Override { .. }) => m,
    };
    Some(Recovery { text, rule, mode })
}

/// Whether `text` is nothing but control characters (or empty): such a glyph has no text to
/// show, and text extraction drops it.
pub(crate) fn is_control_only(text: &BfString) -> bool {
    match text {
        BfString::Char(c) => c.is_control(),
        BfString::String(s) => s.chars().all(char::is_control),
    }
}

/// The decimal number of a glyph name made of `C`, `c` or `MT` and one to six decimal digits
/// (`C83`, `c246`, `C0046`, `MT40`); nothing for any other name. It doesn't check that the name
/// only numbers its glyph: callers do ([`is_numbered_glyph_name`]). `glyph_name_numbers` reads
/// the name both in decimal and in hexadecimal; the decimal reading is its first.
fn decimal_name_number(name: &str) -> Option<u32> {
    let base = name.split('.').next().unwrap_or_default();
    let digits = ["MT", "C", "c"]
        .into_iter()
        .find_map(|prefix| base.strip_prefix(prefix))?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // The split after the prefix, read in decimal: `glyph_name_numbers` yields it.
    let n: u32 = digits.parse().ok()?;
    glyph_name_numbers(name).any(|m| m == n).then_some(n)
}

/// [`RecoveryRule::NumberedLatin1Names`].
///
/// A simple Type 1 or CFF font qualifies when it has at least one numbered glyph name
/// ([`is_numbered_glyph_name`]) and every one is `C`, `c` or `MT` followed by a decimal number
/// from 32 to 255, as in `/Differences [1 /C83 /C111 /C99 …]`, which spells `Soc…` with codes
/// 1, 2 and 3. A single numbered name below 32 or above 255, in hexadecimal (`G5F`) or with
/// another prefix (`g3`, a glyph id) disqualifies the whole font: names that count a subset's
/// glyphs (`/C1`, `/C2` …) start at 1, so their low numbers give them away. A glyph whose name
/// numbers its own code reads strictly already (patch 100); this rule only fills. It has no
/// plausibility gate on what the names spell, as [`MacGlyphOrder`] has: the guard already
/// demands that every numbered name of the font be a decimal number from 32 to 255 (a counter
/// or glyph id fails at its low numbers or its prefix), and such a number is a code of the
/// font's own encoding, where a glyph id read in the Macintosh order is only a guess about the
/// subset.
///
/// Which encoding the number is a code of depends on the font ([`NumberedEncoding`]): Latin-1
/// for most, the Symbol font's built-in encoding for a copy of Symbol named with a `Sym` ending
/// (`AdvPSSym`), printable ASCII only where the high codes are known to be another encoding's.
/// Fonts named as symbol fonts (Symbol, dingbats …) never qualify (a ZapfDingbats `/c110` is a
/// square, not `n`; a font named `Greek Math Symbols` has its own order), nor does a further
/// encoding plane of a font (`AdvTT5843c571+20`, whose `/C172` is `€`, not `¬`).
pub(crate) struct NumberedLatin1Names;

/// Which encoding the numbers of [`NumberedLatin1Names`] are codes of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NumberedEncoding {
    /// ISO 8859-1: what the numbered names of a text font give (`/C246` is `ö`, `/C169` `©`).
    Latin1,
    /// The Symbol font's built-in encoding (ISO 32000-2 Annex D.5), for a copy of Symbol named
    /// with a `Sym` ending (`AdvPSSym`, Advent 3B2's Symbol): `/C176` is `°` there too, but
    /// `/C211` is `©` (copyrightserif), not `Ó`, and `/C97` is `α`.
    Symbol,
    /// Printable ASCII only, for an Advent 3B2 PostScript text font (`AdvPSTim`): its codes
    /// above 127 follow Adobe's standard encoding rather than Latin-1 (`/C177` is an en dash
    /// and `/C174` the `fi` ligature, where Latin-1 has `±` and `®`), and which encoding each
    /// high code follows isn't known, so the high codes recover nothing.
    Ascii,
}

/// The font's name without its subset tag (`ABCDEF+`, ISO 32000-2 §9.9.2).
fn untagged(base: &str) -> &str {
    match base.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => base,
    }
}

/// Whether `/BaseFont` names a further encoding plane of a font: a `+` and a number after the
/// name (`AdvTT5843c571+20`, beside the text font `AdvTT5843c571`), whose codes are the plane's
/// own and not the text font's.
fn encoding_plane(base: Option<&str>) -> bool {
    base.map(untagged)
        .and_then(|name| name.rsplit_once('+'))
        .is_some_and(|(font, plane)| {
            !font.is_empty() && !plane.is_empty() && plane.bytes().all(|b| b.is_ascii_digit())
        })
}

/// Whether `/BaseFont` names a copy of the Symbol font by a `Sym` ending (`AdvPSSym`). Names
/// with `symbol` in them are [`symbol_font`]s and never qualify: they cover other symbol sets
/// too (`Greek Math Symbols`).
fn symbol_copy(base: Option<&str>) -> bool {
    base.is_some_and(|b| untagged(b).to_ascii_lowercase().ends_with("sym"))
}

/// Whether `/BaseFont` names an Advent 3B2 PostScript font (`AdvPSTim`, `AdvPSTim-I`), whose
/// codes above 127 aren't Latin-1.
fn advent_postscript(base: Option<&str>) -> bool {
    base.is_some_and(|b| untagged(b).starts_with("AdvPS"))
}

/// The character the Symbol font's built-in encoding gives `code`, through the Adobe Glyph
/// List. The glyph list puts the Symbol font's serif and sans-serif ©, ® and ™ in the private
/// use area; they are the plain characters. Other private-use glyphs (the pieces of large
/// brackets and braces) read as nothing.
fn symbol_char(code: u8) -> Option<char> {
    let name = symbol::get(code)?;
    let c = match name {
        "copyrightserif" | "copyrightsans" => '\u{a9}',
        "registerserif" | "registersans" => '\u{ae}',
        "trademarkserif" | "trademarksans" => '\u{2122}',
        _ => glyph_name_to_unicode(name)?,
    };
    let private = matches!(u32::from(c), 0xE000..=0xF8FF);
    (!private && !c.is_control()).then_some(c)
}

impl Rule for NumberedLatin1Names {
    const RULE: RecoveryRule = RecoveryRule::NumberedLatin1Names;
    type Guard = NumberedEncoding;

    fn guard(font: &FontFacts<'_>) -> Option<NumberedEncoding> {
        if !matches!(font.kind, FontKind::Type1 | FontKind::Cff) || encoding_plane(font.base_font) {
            return None;
        }
        let encoding = if symbol_font(font.base_font) {
            return None;
        } else if symbol_copy(font.base_font) {
            NumberedEncoding::Symbol
        } else if advent_postscript(font.base_font) {
            NumberedEncoding::Ascii
        } else {
            NumberedEncoding::Latin1
        };
        let names = font.names?;
        let mut any = false;
        for name in (0..=u8::MAX).filter_map(names) {
            if !is_numbered_glyph_name(name) {
                continue;
            }
            if !decimal_name_number(name).is_some_and(|n| (32..=255).contains(&n)) {
                return None;
            }
            any = true;
        }
        any.then_some(encoding)
    }

    fn read(
        encoding: &NumberedEncoding,
        _: &FontFacts<'_>,
        glyph: &GlyphFacts<'_>,
    ) -> Option<(BfString, RecoveryMode)> {
        let name = glyph.name.filter(|n| is_numbered_glyph_name(n))?;
        let code = decimal_name_number(name)
            .filter(|n| (32..=255).contains(n))
            .and_then(|n| u8::try_from(n).ok())?;
        let c = match encoding {
            NumberedEncoding::Latin1 => Some(char::from(code)),
            NumberedEncoding::Ascii => code.is_ascii().then_some(char::from(code)),
            NumberedEncoding::Symbol => symbol_char(code),
        }
        .filter(|c| !c.is_control())?;
        Some((BfString::Char(c), RecoveryMode::Fill))
    }
}

/// [`RecoveryRule::MacGlyphOrder`]: a glyph id read through the standard Macintosh glyph order
/// (the 258 glyph names a TrueType `post` table of format 1 implies, Apple's TrueType
/// Reference Manual) and the Adobe Glyph List.
///
/// Two kinds of font qualify:
///
/// - **A TrueType program that keeps its original glyph ids** (a simple TrueType font, or a
///   Type 0 font with a TrueType descendant), without `/ToUnicode`, without a Unicode `cmap`
///   subtable, without glyph names of its own (no `post` table, or format 3), and, for a
///   simple font, without `/Encoding`. The program must have at least 258 glyphs of which at
///   most half are drawn (a subset that kept the original numbering and emptied the rest),
///   and the glyphs it does draw must read as text ([`plausible`]). A subset whose glyphs were
///   renumbered from 1 is dense and fails. In a simple font of this kind the strict text is
///   the code read as WinAnsi, a last resort with no evidence behind it, so the rule overrides
///   it.
/// - **Glyph names that only number the glyph id** (`/g55`, as converters write for TrueType
///   outlines) in a simple font: every numbered name is `g` and a decimal number, and the
///   glyphs they name read as text. These only fill.
///
/// Either way the glyph ids must not form one gapless block (`renumbered_block`): converters
/// that renumber a subset's glyphs from some start (`/g31`, `/g32` …) give that away.
///
/// Fonts named as symbol fonts (Wingdings, Webdings, Symbol, Dingbats, Marlett) never qualify:
/// their glyph order is their own.
pub(crate) struct MacGlyphOrder;

/// Which kind of font [`MacGlyphOrder`] took.
#[derive(Debug)]
pub(crate) enum MacSource {
    /// The glyph ids drawn, from a program that keeps its original ids; `override_strict` when
    /// the strict text is the last-resort WinAnsi reading.
    Program { override_strict: bool },
    /// `/gNN` glyph names.
    Names,
}

/// Why [`MacGlyphOrder`] replaces the strict text of a simple TrueType font.
const WINANSI_GUESS: &str = "the strict text is the code read as WinAnsi, a last resort for a font without /Encoding, \
     /ToUnicode or a Unicode cmap";

/// The character the standard Macintosh glyph order gives glyph id `gid`, through the Adobe
/// Glyph List; nothing past the 258 standard glyphs or for a control (`.null`).
pub(crate) fn mac_glyph_char(gid: u32) -> Option<char> {
    let name = DEFAULT_GLYPH_NAMES.get(usize::try_from(gid).ok()?)?;
    glyph_name_to_unicode(name).filter(|c| !c.is_control())
}

/// The glyph id a name that only numbers it gives (`g55` is 55): `g` and one to five decimal
/// digits.
fn g_name_number(name: &str) -> Option<u32> {
    let base = name.split('.').next().unwrap_or_default();
    let digits = base.strip_prefix('g')?;
    if !(1..=5).contains(&digits.len()) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Whether glyph ids decoded through the standard Macintosh order read as text rather than
/// noise: at least three ids, nine in ten decoded, at least half of those letters and at least
/// five letters, at least half in the order's basic Latin range (ids 3 to 97: a text font draws
/// its plain letters, digits and punctuation far more than accented ones, while a symbol font
/// read in this order lands among the accented ones), and, among eight or more lowercase
/// letters, at least two different vowels. Renumbered glyph ids (1, 2, 3 …) decode to
/// punctuation and digits and fail.
pub(crate) fn plausible(ids: &[u32]) -> bool {
    let (mut known, mut basic, mut letters, mut lower) = (0_usize, 0_usize, 0_usize, 0_usize);
    let mut vowels = [false; 6];
    for &gid in ids {
        let Some(c) = mac_glyph_char(gid) else {
            continue;
        };
        known = known.saturating_add(1);
        if (3..=97).contains(&gid) {
            basic = basic.saturating_add(1);
        }
        if c.is_alphabetic() {
            letters = letters.saturating_add(1);
        }
        if c.is_lowercase() {
            lower = lower.saturating_add(1);
            if let Some(v) = "aeiouy".find(c).and_then(|i| vowels.get_mut(i)) {
                *v = true;
            }
        }
    }
    let total = ids.len();
    let vowel_kinds = vowels.iter().filter(|v| **v).count();
    total >= 3
        && known.saturating_mul(10) >= total.saturating_mul(9)
        && letters.saturating_mul(2) >= known
        && letters >= 5
        && basic.saturating_mul(2) >= known
        && (lower < 8 || vowel_kinds >= 2)
}

/// Whether glyph ids form one block with fewer than three gaps (four or more ids): a subset
/// that renumbered its glyphs from some start, not one that kept the original ids, which
/// leaves the unused characters out (digits, rare letters, other scripts).
fn renumbered_block(ids: &[u32]) -> bool {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let (Some(&first), Some(&last)) = (ids.first(), ids.last()) else {
        return false;
    };
    let span = usize::try_from(last.saturating_sub(first))
        .unwrap_or(usize::MAX)
        .saturating_add(1);
    ids.len() >= 4 && span.saturating_sub(ids.len()) < 3
}

/// Whether `/BaseFont` names a symbol font, whose glyph order is its own.
fn symbol_font(base: Option<&str>) -> bool {
    let Some(base) = base else { return false };
    let base = base.to_ascii_lowercase();
    ["wingding", "webding", "symbol", "dingbat", "marlett"]
        .iter()
        .any(|s| base.contains(s))
}

/// Most glyphs a program may have for [`MacGlyphOrder`] to look at its glyph table.
const MAX_PROGRAM_GLYPHS: usize = 65_536;

/// The glyph ids a TrueType program draws (a non-empty `glyf` entry), other than the first
/// three (`.notdef`, `.null`, the carriage return), when the program has at least 258 glyphs
/// and draws at most half of them; `None` otherwise or when its tables can't be read.
fn sparse_original_ids(program: &FontRef<'_>) -> Option<Vec<u32>> {
    let glyphs = usize::from(program.maxp().ok()?.num_glyphs());
    if !(258..=MAX_PROGRAM_GLYPHS).contains(&glyphs) {
        return None;
    }
    let loca = program.loca(None).ok()?;
    let mut drawn = Vec::new();
    for gid in 0..glyphs {
        let (Some(start), Some(end)) = (loca.get_raw(gid), loca.get_raw(gid.saturating_add(1)))
        else {
            break;
        };
        if end > start && gid >= 3 {
            drawn.push(u32::try_from(gid).ok()?);
        }
    }
    (drawn.len().saturating_mul(2) <= glyphs).then_some(drawn)
}

/// Whether a simple font's codes mostly read right as its strict text already: of the codes
/// whose strict text is a letter and whose glyph the Macintosh order reads as one, at least
/// half agree. A Windows driver's font keeps the WinAnsi codes (`A` at 0x41), so its strict
/// text is right and a glyph the order reads otherwise (a curly quote) is the order's guess; a
/// subset that numbers its codes 1, 2, 3 … reads as controls and punctuation instead.
fn codes_read_as_winansi(font: &FontFacts<'_>) -> bool {
    let Some(code_glyph) = font.code_glyph else {
        return false;
    };
    let (mut compared, mut agree) = (0_usize, 0_usize);
    for code in 0..=u8::MAX {
        let (gid, strict) = code_glyph(code);
        let Some(BfString::Char(strict)) = strict.filter(|_| gid.to_u32() != 0) else {
            continue;
        };
        let Some(mac) = mac_glyph_char(gid.to_u32()).filter(|c| c.is_alphabetic()) else {
            continue;
        };
        if strict.is_alphabetic() {
            compared = compared.saturating_add(1);
            agree = agree.saturating_add(usize::from(strict == mac));
        }
    }
    compared > 0 && agree.saturating_mul(2) >= compared
}

/// Whether a program says nothing about its glyphs' characters: no Unicode `cmap` subtable
/// (Windows 1 or 10, or the Unicode platform) and no glyph names of its own (`post` missing,
/// or format 3). A `post` table of format 1 *is* the standard Macintosh order and qualifies.
fn program_names_nothing(program: &FontRef<'_>) -> bool {
    if let Ok(cmap) = program.cmap() {
        let unicode = cmap.encoding_records().iter().any(|r| {
            use skrifa::raw::tables::cmap::PlatformId;
            match r.platform_id() {
                PlatformId::Unicode => true,
                PlatformId::Windows => matches!(r.encoding_id(), 1 | 10),
                _ => false,
            }
        });
        if unicode {
            return false;
        }
    }
    match program.post() {
        Ok(post) => matches!(
            post.version(),
            Version16Dot16::VERSION_1_0 | Version16Dot16::VERSION_3_0
        ),
        Err(_) => true,
    }
}

impl Rule for MacGlyphOrder {
    const RULE: RecoveryRule = RecoveryRule::MacGlyphOrder;
    type Guard = MacSource;

    fn guard(font: &FontFacts<'_>) -> Option<MacSource> {
        if symbol_font(font.base_font) {
            return None;
        }
        // Glyph names that number the glyph id.
        if let Some(names) = font.names
            && !font.has_to_unicode
            && matches!(
                font.kind,
                FontKind::Type1 | FontKind::Cff | FontKind::TrueType
            )
        {
            let numbered: Vec<&str> = (0..=u8::MAX)
                .filter_map(names)
                .filter(|n| is_numbered_glyph_name(n))
                .collect();
            let ids: Vec<u32> = numbered.iter().filter_map(|n| g_name_number(n)).collect();
            if !numbered.is_empty()
                && ids.len() == numbered.len()
                && !renumbered_block(&ids)
                && plausible(&ids)
            {
                return Some(MacSource::Names);
            }
        }
        // A program that keeps its original glyph ids.
        let simple = match font.kind {
            FontKind::TrueType => true,
            FontKind::CidTrueType => false,
            _ => return None,
        };
        if font.has_to_unicode || (simple && font.has_encoding) {
            return None;
        }
        // A simple font must be marked Symbolic and not Nonsymbolic: a nonsymbolic font's codes
        // are a standard encoding's characters (ISO 32000-2 §9.6.5.4), not a last resort.
        let symbolic = font.flags.is_some_and(|f| {
            f.contains(FontFlags::SYMBOLIC) && !f.contains(FontFlags::NON_SYMBOLIC)
        });
        if simple && !symbolic {
            return None;
        }
        let program = font.program?;
        if !program_names_nothing(program) {
            return None;
        }
        let drawn = sparse_original_ids(program)?;
        let reads = !renumbered_block(&drawn) && plausible(&drawn);
        reads.then_some(MacSource::Program {
            override_strict: simple && !codes_read_as_winansi(font),
        })
    }

    fn read(
        guard: &MacSource,
        _: &FontFacts<'_>,
        glyph: &GlyphFacts<'_>,
    ) -> Option<(BfString, RecoveryMode)> {
        match guard {
            MacSource::Names => {
                let c = mac_glyph_char(g_name_number(glyph.name?)?)?;
                Some((BfString::Char(c), RecoveryMode::Fill))
            }
            MacSource::Program { override_strict } => {
                let c = mac_glyph_char(glyph.gid.to_u32())?;
                let mode = if *override_strict {
                    RecoveryMode::Override {
                        reason: WINANSI_GUESS,
                    }
                } else {
                    RecoveryMode::Fill
                };
                Some((BfString::Char(c), mode))
            }
        }
    }
}
