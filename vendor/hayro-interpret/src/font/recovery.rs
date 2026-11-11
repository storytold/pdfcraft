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

mod candidate;
mod cjk_order;
mod common_traditional;
mod document;
mod orders;
mod survey;
#[allow(
    dead_code,
    reason = "the width evidence is read by the standard-order rules that build on this layer"
)]
mod widths;

use cjk_order::StandardCjkGlyphOrder;
use crate::font::FontFlags;
use crate::font::{glyph_name_numbers, is_numbered_glyph_name};
use hayro_cmap::BfString;
use hayro_syntax::object::dict::keys::{
    BASE_FONT, ENCODING, FLAGS, FONT_DESC, FONT_WEIGHT, ITALIC_ANGLE,
};
use hayro_syntax::object::{Dict, Name};
use orders::{mac_glyph_char, symbol_char};
use skrifa::raw::TableProvider;
use skrifa::raw::types::Version16Dot16;
use skrifa::{FontRef, GlyphId};
use std::sync::{Arc, OnceLock};

pub(crate) use document::font_key;
pub use document::RecoveryDocument;

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
    /// Glyph ids of a Type 0 font through `Identity-H` or `Identity-V` that follow a public
    /// order of Traditional Chinese characters (Big5, Unicode, Big5 by Unicode, Adobe-CNS1) and
    /// an offset, found from the document's text: see `StandardCjkGlyphOrder`.
    StandardCjkGlyphOrder,
}

impl RecoveryRule {
    /// Every rule, in the order they are tried.
    pub const ORDER: [Self; 3] = [
        Self::NumberedLatin1Names,
        Self::MacGlyphOrder,
        Self::StandardCjkGlyphOrder,
    ];

    /// A stable name for logs and tests.
    pub fn name(self) -> &'static str {
        match self {
            Self::NumberedLatin1Names => "numbered-latin1-names",
            Self::MacGlyphOrder => "mac-glyph-order",
            Self::StandardCjkGlyphOrder => "standard-cjk-glyph-order",
        }
    }
}

/// The work the recovery layer counts for tests ([`recovery_counters`], the `test-counters`
/// feature); without the feature [`count`] does nothing.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Counter {
    /// A glyph asked of the rules (`RecoveryCache::recover`): the one check every glyph costs.
    RuleCall,
    /// A font's guards decided (`Guards::decide`).
    GuardDecision,
    /// A font's [`FontFacts`] built.
    FactsBuilt,
    /// A glyph's [`GlyphFacts`] built.
    GlyphFactsBuilt,
    /// A guard walk over an embedded program's tables (its `cmap` and `post`, its `loca`, its
    /// metrics).
    ProgramWalk,
    /// A Type 3 font's dictionary hashed for its cache key.
    Type3KeyHash,
    /// A font's guards taken from its document's decisions ([`RecoveryDocument`]) instead of
    /// decided again.
    DecisionReused,
    /// A page whose content the document survey read ([`RecoveryDocument::prepare`]).
    SurveyPage,
    /// A glyph the document survey counted.
    SurveyGlyph,
}

/// Counts `counter` on this thread (only with the `test-counters` feature).
#[inline]
pub(crate) fn count(counter: Counter) {
    #[cfg(feature = "test-counters")]
    COUNTERS.with(|c| {
        let mut n = c.get();
        let field = match counter {
            Counter::RuleCall => &mut n.rule_calls,
            Counter::GuardDecision => &mut n.guard_decisions,
            Counter::FactsBuilt => &mut n.facts_built,
            Counter::GlyphFactsBuilt => &mut n.glyph_facts_built,
            Counter::ProgramWalk => &mut n.program_walks,
            Counter::Type3KeyHash => &mut n.type3_key_hashes,
            Counter::DecisionReused => &mut n.decisions_reused,
            Counter::SurveyPage => &mut n.survey_pages,
            Counter::SurveyGlyph => &mut n.survey_glyphs,
        };
        *field = field.saturating_add(1);
        c.set(n);
    });
    #[cfg(not(feature = "test-counters"))]
    let _ = counter;
}

/// The recovery layer's work on this thread so far, for tests that show what a page costs
/// (the `test-counters` feature).
#[cfg(feature = "test-counters")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecoveryCounters {
    /// Glyphs asked of the rules: one check each.
    pub rule_calls: usize,
    /// Fonts whose guards were decided.
    pub guard_decisions: usize,
    /// Font facts built (for the guards, and for each glyph of a font some rule takes).
    pub facts_built: usize,
    /// Glyph facts built (only for glyphs of a font some rule takes).
    pub glyph_facts_built: usize,
    /// Guard walks over embedded programs' tables.
    pub program_walks: usize,
    /// Type 3 font dictionaries hashed for a cache key.
    pub type3_key_hashes: usize,
    /// Fonts whose guards came from their document's decisions ([`RecoveryDocument`]), not
    /// decided again.
    pub decisions_reused: usize,
    /// Pages the document survey read.
    pub survey_pages: usize,
    /// Glyphs the document survey counted.
    pub survey_glyphs: usize,
}

#[cfg(feature = "test-counters")]
impl RecoveryCounters {
    /// The work done since `before` (a reading taken earlier on the same thread), field by
    /// field: what a test measures without naming every counter.
    #[must_use]
    pub fn since(&self, before: &Self) -> Self {
        Self {
            rule_calls: self.rule_calls.saturating_sub(before.rule_calls),
            guard_decisions: self.guard_decisions.saturating_sub(before.guard_decisions),
            facts_built: self.facts_built.saturating_sub(before.facts_built),
            glyph_facts_built: self
                .glyph_facts_built
                .saturating_sub(before.glyph_facts_built),
            program_walks: self.program_walks.saturating_sub(before.program_walks),
            type3_key_hashes: self.type3_key_hashes.saturating_sub(before.type3_key_hashes),
            decisions_reused: self.decisions_reused.saturating_sub(before.decisions_reused),
            survey_pages: self.survey_pages.saturating_sub(before.survey_pages),
            survey_glyphs: self.survey_glyphs.saturating_sub(before.survey_glyphs),
        }
    }
}

#[cfg(feature = "test-counters")]
thread_local! {
    static COUNTERS: std::cell::Cell<RecoveryCounters> = const {
        std::cell::Cell::new(RecoveryCounters {
            rule_calls: 0,
            guard_decisions: 0,
            facts_built: 0,
            glyph_facts_built: 0,
            program_walks: 0,
            type3_key_hashes: 0,
            decisions_reused: 0,
            survey_pages: 0,
            survey_glyphs: 0,
        })
    };
}

/// The recovery layer's work on this thread so far ([`RecoveryCounters`]).
#[cfg(feature = "test-counters")]
pub fn recovery_counters() -> RecoveryCounters {
    COUNTERS.with(std::cell::Cell::get)
}

/// PdfCraft patch (104) test hooks (the `test-counters` feature, enabled by pdfcraft-render's
/// dev-dependency only): the shared pieces the rules read, for tests outside the crate.
#[cfg(feature = "test-counters")]
pub mod test_hooks {
    /// Whether the width evidence proves a reading that gives each character its glyph width
    /// ([`super::widths::widths_agree`] and `proves`).
    pub fn widths_prove(widths: &[(char, f32)]) -> bool {
        super::widths::widths_agree(&widths.iter().copied().collect()).proves()
    }

    /// The character the standard Macintosh order gives glyph id `gid`.
    pub fn mac_glyph_char(gid: u32) -> Option<char> {
        super::orders::mac_glyph_char(gid)
    }

    /// The character the CFF standard string `sid` names.
    pub fn standard_string_char(sid: u32) -> Option<char> {
        super::orders::standard_string_char(sid)
    }

    /// The document survey of `pdf`, font by font: each counted font's object number,
    /// `/BaseFont` without its subset tag, and (code, times shown, the number a rule reads),
    /// with whether the survey passed a cap; `None` when it was cancelled.
    #[allow(clippy::type_complexity, reason = "a test's view of the survey")]
    pub fn survey_report(
        pdf: &hayro_syntax::Pdf,
        settings: &crate::InterpreterSettings,
    ) -> Option<(bool, Vec<(Option<i32>, Vec<u8>, Vec<(u16, u64, Option<u32>)>)>)> {
        let (over, fonts) = super::survey::report(pdf, settings)?;
        Some((over, fonts.into_iter().map(|f| (f.object, f.base, f.codes)).collect()))
    }

    /// Caps the glyphs the document surveys run on this thread may count (`None`: the usual
    /// cap), so a test can pass it with a small document.
    pub fn set_survey_glyph_cap(cap: Option<u64>) {
        super::survey::GLYPH_CAP.with(|c| c.set(cap));
    }

    /// The character the standard CJK order `order` (`big5`, `uni`, `uni_big5`, `cns1`) has at
    /// `position`, before the rule's output scope.
    pub fn cjk_order_char(order: &str, position: u32) -> Option<char> {
        super::cjk_order::hooks::order_char(order, position)
    }

    /// Whether the standard CJK order rule's guards pass for a search with these scores (the
    /// best reading's, its rival's), close ratio and number of different glyphs.
    pub fn cjk_guards(best: f64, rival: f64, close: f64, glyphs: usize) -> bool {
        super::cjk_order::hooks::guards(best, rival, close, glyphs)
    }

    /// Whether `c` is one of the 750 common characters the standard CJK order rule scores by.
    pub fn cjk_common(c: char) -> bool {
        super::cjk_order::hooks::common(c)
    }

    /// Whether the standard CJK order rule outputs `c` read in `order`.
    pub fn cjk_outputs(order: &str, c: char) -> bool {
        super::cjk_order::hooks::outputs(order, c)
    }

    /// The standard CJK order rule's search of `numbers` (glyph number, times shown; sorted): the
    /// best order and offset, its score, the rival's score,
    /// the close ratio, the glyphs and whether the guards pass.
    #[allow(clippy::type_complexity, reason = "a test's view of the search")]
    pub fn cjk_search(
        numbers: &[(u32, u64)],
    ) -> Option<(&'static str, i64, f64, f64, f64, usize, bool)> {
        super::cjk_order::hooks::searched(numbers)
    }

    /// The reading the standard CJK order rule gives a group named `name` of subsets whose
    /// counts are `members` (each sorted by number).
    pub fn cjk_group_reading(
        name: &str,
        members: &[Vec<(u32, u64)>],
    ) -> Option<(&'static str, i64)> {
        super::cjk_order::hooks::group_reading_of(name, members)
    }

    /// Whether the standard CJK order rule's two ways of counting the glyphs each offset reads as
    /// output (run by run, and by a Fourier transform) agree for `numbers`.
    pub fn cjk_output_counts_agree(numbers: &[(u32, u64)]) -> bool {
        super::cjk_order::hooks::output_counts_agree(numbers)
    }

    /// Whether `name` (a `/BaseFont` without its subset tag) is a producer's placeholder to the
    /// standard CJK order rule.
    pub fn cjk_placeholder_name(name: &str) -> bool {
        super::cjk_order::hooks::placeholder(name)
    }

    /// The character the standard CJK order rule outputs for glyph `number` read in `order` at
    /// `offset`.
    pub fn cjk_output(order: &str, offset: i64, number: u32) -> Option<char> {
        super::cjk_order::hooks::output_char(order, offset, number)
    }

    /// Verification hook: the standard CJK order rule's view of every font the survey of `pdf`
    /// counts, as text lines (tab-separated: object number, `/BaseFont` without its subset tag, codes,
    /// glyphs shown, the single font's search, the group's, the decision and its readings
    /// `code:char`), or `None` when the survey was cancelled. The first line says whether the
    /// survey passed a cap.
    pub fn cjk_report(
        pdf: &hayro_syntax::Pdf,
        settings: &crate::InterpreterSettings,
    ) -> Option<Vec<String>> {
        super::cjk_order::hooks::report(pdf, settings)
    }

    /// The candidate policy's choice among `candidates`.
    pub fn choose(candidates: &[char]) -> Option<char> {
        super::candidate::choose(candidates.iter().copied())
    }

    /// Whether the glyphs `drawn` the Macintosh order reads as accented letters are as wide as
    /// those it reads as their base letters, with the widths `width` gives.
    pub fn accents_as_wide_as_bases(drawn: &[u32], width: &dyn Fn(u16) -> Option<f32>) -> bool {
        super::accents_as_wide_as_bases(drawn, width)
    }
}

/// How many times the recovery rules were asked about a glyph on this thread, guards
/// included: for tests that show a reader runs no recovery code at all. Only with the
/// `test-counters` feature (a dev-dependency feature; release builds count nothing).
#[cfg(feature = "test-counters")]
pub fn recovery_rule_calls() -> usize {
    recovery_counters().rule_calls
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
    /// A Type 0 font whose descendant is an embedded CID-keyed CFF program (`CIDFontType0`),
    /// whose glyphs are found by CID through the program's charset (ISO 32000-2 §9.7.4.2:
    /// `/CIDToGIDMap` applies to `CIDFontType2` only; a stream there is a producer's error, and
    /// such a font is left as `CidOther`).
    CidCff,
    /// Any other Type 0 font.
    CidOther,
    /// A Type 3 font.
    Type3,
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
    /// Type 0 fonts: whether `has_to_unicode` comes from a `/ToUnicode` map alone (not the
    /// `/Identity-H` name or an Adobe character collection), so its answers can be checked
    /// against a reading through [`Self::id_text`].
    #[allow(
        dead_code,
        reason = "read by the standard-order rules that build on this layer"
    )]
    pub(crate) to_unicode_checkable: bool,
    /// The numbers [`GlyphFacts::id`] takes in this font, for rules that read them in a standard
    /// order: the CIDs of a CID-keyed CFF program's charset, the codes of a Type 3 font that
    /// have a glyph procedure.
    #[allow(
        dead_code,
        reason = "read by the standard-order rules that build on this layer"
    )]
    pub(crate) ids: Option<&'a dyn Fn() -> Vec<u32>>,
    /// The width the font gives an id (`/W` by CID, `/Widths` by code), when it gives one.
    #[allow(
        dead_code,
        reason = "read by the standard-order rules that build on this layer"
    )]
    pub(crate) id_width: Option<&'a dyn Fn(u32) -> Option<f32>>,
    /// For a Type 0 font shown through an identity CMap and `/CIDToGIDMap`: the strict text of
    /// the code that draws glyph id `id`.
    #[allow(
        dead_code,
        reason = "read by the standard-order rules that build on this layer"
    )]
    pub(crate) id_text: Option<&'a dyn Fn(u32) -> Option<BfString>>,
    /// Type 0 fonts: whether the encoding CMap is `Identity-H` or `Identity-V`, whatever the
    /// `/CIDToGIDMap` (ISO 32000-2 §9.7.5.2: two-byte codes that are CIDs).
    pub(crate) identity_encoding: bool,
    /// How many glyphs the embedded program has: a TrueType or OpenType program's `maxp`
    /// count, a CFF program's number of charstrings. `None` without an embedded program.
    #[allow(
        dead_code,
        reason = "read by the rules that read a Type 0 font's glyph ids in a standard order"
    )]
    pub(crate) program_glyphs: Option<u32>,
    /// How often the font's glyphs are shown across its document, and its group of subsets of
    /// one font ([`RecoveryDocument`]'s survey): for the guards of rules whose evidence is the
    /// document's text. `None` without a surveyed document, or for a font the survey doesn't
    /// count.
    pub(crate) usage: Option<Arc<document::FontUsage>>,
}

impl FontFacts<'_> {
    /// The facts of a font of `kind` that says nothing more about itself.
    pub(crate) fn of_kind(kind: FontKind) -> Self {
        Self {
            kind,
            base_font: None,
            flags: None,
            has_to_unicode: false,
            has_encoding: false,
            names: None,
            program: None,
            code_glyph: None,
            to_unicode_checkable: false,
            ids: None,
            id_width: None,
            id_text: None,
            identity_encoding: false,
            program_glyphs: None,
            usage: None,
        }
    }
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
    /// The number a standard-order rule reads ([`FontFacts::ids`]): a Type 0 glyph's CID, a
    /// Type 3 glyph's code.
    #[allow(
        dead_code,
        reason = "read by the standard-order rules that build on this layer"
    )]
    pub(crate) id: Option<u32>,
}

/// One recovery rule.
pub(crate) trait Rule {
    /// Which rule this is.
    const RULE: RecoveryRule;
    /// The font kinds the rule can take. A font of any other kind is never asked of it: its
    /// guard isn't decided and its facts aren't built, so a font no rule reads costs one
    /// check per glyph and nothing else.
    const KINDS: &'static [FontKind];
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
    cjk: Option<<StandardCjkGlyphOrder as Rule>::Guard>,
}

impl Guards {
    /// Whether some rule can take a font of `kind` ([`Rule::KINDS`]).
    fn read(kind: FontKind) -> bool {
        NumberedLatin1Names::KINDS.contains(&kind)
            || MacGlyphOrder::KINDS.contains(&kind)
            || StandardCjkGlyphOrder::KINDS.contains(&kind)
    }

    fn decide(font: &FontFacts<'_>) -> Self {
        count(Counter::GuardDecision);
        Self {
            numbered: guard::<NumberedLatin1Names>(font),
            mac: guard::<MacGlyphOrder>(font),
            cjk: guard::<StandardCjkGlyphOrder>(font),
        }
    }

    /// Whether no rule takes the font: nothing is ever recovered from it.
    fn none(&self) -> bool {
        self.numbered.is_none() && self.mac.is_none() && self.cjk.is_none()
    }
}

/// The font-level decisions of one font, made the first time a glyph of it asks, or taken from
/// its document's ([`RecoveryDocument`]) when the font was read with one.
#[derive(Debug, Default)]
pub(crate) struct RecoveryCache {
    guards: OnceLock<Arc<Guards>>,
    /// The document the font was read from, and the font's key there.
    document: OnceLock<(Arc<RecoveryDocument>, u128)>,
}

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
            .field("cjk", &self.cjk)
            .finish()
    }
}

impl RecoveryCache {
    /// Keep the font's decisions in `document`, under `key` (its dictionary's cache key): set
    /// once, before any glyph of the font asks.
    pub(crate) fn attach(&self, document: Arc<RecoveryDocument>, key: u128) {
        let _ = self.document.set((document, key));
    }

    /// The recovered reading of one glyph of a font of `kind`: `facts` builds the font's facts
    /// (of that kind; called for the guards the first time, and again only when some rule takes
    /// the font), `glyph` the glyph's (built only when some rule takes the font). A font of a
    /// kind no rule reads is turned away before either is built or a guard decided.
    pub(crate) fn recover<'a, 'g>(
        &self,
        kind: FontKind,
        facts: impl Fn() -> FontFacts<'a>,
        glyph: impl FnOnce() -> GlyphFacts<'g>,
    ) -> Option<Recovery> {
        count(Counter::RuleCall);
        if !Guards::read(kind) {
            return None;
        }
        let facts = || {
            count(Counter::FactsBuilt);
            facts()
        };
        let guards = self.guards.get_or_init(|| match self.document.get() {
            Some((document, key)) => document.guards(*key, || {
                let mut font = facts();
                font.usage = document.usage(*key);
                Guards::decide(&font)
            }),
            None => Arc::new(Guards::decide(&facts())),
        });
        if guards.none() {
            return None;
        }
        let font = facts();
        count(Counter::GlyphFactsBuilt);
        let glyph = &glyph();
        RecoveryRule::ORDER.iter().find_map(|rule| match rule {
            RecoveryRule::NumberedLatin1Names => {
                apply::<NumberedLatin1Names>(guards.numbered.as_ref(), &font, glyph)
            }
            RecoveryRule::MacGlyphOrder => {
                apply::<MacGlyphOrder>(guards.mac.as_ref(), &font, glyph)
            }
            RecoveryRule::StandardCjkGlyphOrder => {
                apply::<StandardCjkGlyphOrder>(guards.cjk.as_ref(), &font, glyph)
            }
        })
    }
}

/// Rule `R`'s guard for `font`, asked only of a font of a kind it can take ([`Rule::KINDS`]).
fn guard<R: Rule>(font: &FontFacts<'_>) -> Option<R::Guard> {
    R::KINDS
        .contains(&font.kind)
        .then(|| R::guard(font))
        .flatten()
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

/// The fill/override policy every rule's reading passes through. A reading the candidate policy
/// refuses (`candidate`: empty, or with a private use character, a control character or a
/// noncharacter) is nothing. Over a glyph without strict text (`strict` is `None`: none, or only
/// control characters) the reading is a fill whatever the rule said; strict text is replaced
/// only by a reading marked [`RecoveryMode::Override`], and only when the text differs. Public
/// so the policy can be tested from outside the crate with a synthetic rule's reading.
pub fn recovery_policy(
    rule: RecoveryRule,
    strict: Option<&BfString>,
    text: BfString,
    mode: RecoveryMode,
) -> Option<Recovery> {
    if !candidate::text_acceptable(&text) {
        return None;
    }
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

impl Rule for NumberedLatin1Names {
    const RULE: RecoveryRule = RecoveryRule::NumberedLatin1Names;
    const KINDS: &'static [FontKind] = &[FontKind::Type1, FontKind::Cff];
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

/// The last glyph id [`MacGlyphOrder`] reads. Fonts made for Windows (Arial, Times New Roman,
/// Tahoma, Verdana, the `TTE…t00` subsets of their printer drivers) keep the standard Macintosh
/// order through the ellipsis (171) and leave it at 172, where they have `Agrave` and the order
/// the no-break space: from there on a glyph is its neighbour's (the en dash where the order has
/// `oe`, the right quote where it has the left one, the euro sign where it has `fraction`), so
/// those ids read as nothing.
const MAC_ORDER_HOLDS: u32 = 171;

/// Glyph ids up to [`MAC_ORDER_HOLDS`] where fonts of those families put another glyph than the
/// order: `notequal` (143, Courier's `AE`), `pi` (155, a private-use glyph in Franklin Gothic),
/// `Omega` (159, Arial Narrow) and `Delta` (168, Trebuchet). They read as nothing.
const MAC_ORDER_DIFFERS: [u32; 4] = [143, 155, 159, 168];

/// Whether [`MacGlyphOrder`] reads glyph id `gid`: up to [`MAC_ORDER_HOLDS`], but not the ids of
/// [`MAC_ORDER_DIFFERS`].
fn mac_order_reads(gid: u32) -> bool {
    gid <= MAC_ORDER_HOLDS && !MAC_ORDER_DIFFERS.contains(&gid)
}

/// Accented letters of the standard Macintosh order and their base letters (glyph ids): in a
/// Latin typeface an accented letter is as wide as its base letter (`Ä` as `A`, `é` as `e`). The
/// accented `i` letters are left out: they are often wider than the `i`.
const ACCENTED_BASES: [(u16, u16); 28] = [
    (98, 36),  // Adieresis, A
    (99, 36),  // Aring
    (100, 38), // Ccedilla, C
    (101, 40), // Eacute, E
    (102, 49), // Ntilde, N
    (103, 50), // Odieresis, O
    (104, 56), // Udieresis, U
    (105, 68), // aacute, a
    (106, 68), // agrave
    (107, 68), // acircumflex
    (108, 68), // adieresis
    (109, 68), // atilde
    (110, 68), // aring
    (111, 70), // ccedilla, c
    (112, 72), // eacute, e
    (113, 72), // egrave
    (114, 72), // ecircumflex
    (115, 72), // edieresis
    (120, 81), // ntilde, n
    (121, 82), // oacute, o
    (122, 82), // ograve
    (123, 82), // ocircumflex
    (124, 82), // odieresis
    (125, 82), // otilde
    (126, 88), // uacute, u
    (127, 88), // ugrave
    (128, 88), // ucircumflex
    (129, 88), // udieresis
];

/// Whether the glyphs `drawn` that the standard Macintosh order reads as accented letters are
/// as wide as the glyphs it reads as their base letters (to 1% of the wider), wherever the
/// program gives both a width. A font whose own order is the Macintosh one shifted (an Apple
/// font with glyphs of its own first: Skia's `h` is glyph 104, the order's `Udieresis`) puts
/// other letters there, of other widths, and fails.
fn accents_match_bases(program: &FontRef<'_>, drawn: &[u32]) -> bool {
    count(Counter::ProgramWalk);
    use skrifa::MetadataProvider;
    use skrifa::instance::{LocationRef, Size};
    let metrics = program.glyph_metrics(Size::unscaled(), LocationRef::default());
    accents_as_wide_as_bases(drawn, |gid| {
        metrics.advance_width(GlyphId::new(u32::from(gid)))
    })
}

/// [`accents_match_bases`] with the widths `width` gives by glyph id (or by any number a rule
/// reads in the Macintosh order, such as a CID that is a glyph id); widths that aren't finite
/// and positive are left out. Shared by every rule that reads the order.
pub(crate) fn accents_as_wide_as_bases(drawn: &[u32], width: impl Fn(u16) -> Option<f32>) -> bool {
    let width = |gid: u16| width(gid).filter(|w| w.is_finite() && *w > 0.0);
    ACCENTED_BASES
        .iter()
        .filter(|(accented, _)| drawn.contains(&u32::from(*accented)))
        .all(|&(accented, base)| match (width(accented), width(base)) {
            (Some(a), Some(b)) => (a - b).abs() <= a.max(b) * 0.01,
            _ => true,
        })
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
    plausible_reading(
        ids.iter()
            .map(|&gid| (mac_glyph_char(gid), (3..=97).contains(&gid))),
    )
}

/// Whether a standard order's reading of a font's ids reads as text, as [`plausible`] says it
/// for the Macintosh order: each id's character (`None` when the order gives none) and whether
/// the id is in the order's basic Latin range (printable ASCII). Shared by every rule that reads
/// a standard order.
pub(crate) fn plausible_reading(readings: impl Iterator<Item = (Option<char>, bool)>) -> bool {
    let (mut total, mut known, mut basic, mut letters, mut lower) =
        (0_usize, 0_usize, 0_usize, 0_usize, 0_usize);
    let mut vowels = [false; 6];
    for (c, in_basic) in readings {
        total = total.saturating_add(1);
        let Some(c) = c else {
            continue;
        };
        known = known.saturating_add(1);
        if in_basic {
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
    count(Counter::ProgramWalk);
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
    count(Counter::ProgramWalk);
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
    const KINDS: &'static [FontKind] = &[
        FontKind::Type1,
        FontKind::Cff,
        FontKind::TrueType,
        FontKind::CidTrueType,
    ];
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
        let reads =
            !renumbered_block(&drawn) && plausible(&drawn) && accents_match_bases(program, &drawn);
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
                let id = g_name_number(glyph.name?)?;
                let c = Some(id)
                    .filter(|id| mac_order_reads(*id))
                    .and_then(mac_glyph_char)?;
                Some((BfString::Char(c), RecoveryMode::Fill))
            }
            MacSource::Program { override_strict } => {
                let c = Some(glyph.gid.to_u32())
                    .filter(|id| mac_order_reads(*id))
                    .and_then(mac_glyph_char)?;
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
