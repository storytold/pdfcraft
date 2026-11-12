//! PdfCraft patch (105): [`StandardStringOrder`], rule N, a font whose CIDs or codes are CFF
//! standard string ids.
//!
//! A converter that turns a Type 1 font into a CID-keyed CFF font can number each glyph by the
//! string id (SID) of its name, which for the standard glyph names is its place among the CFF
//! standard strings (Adobe Technical Note #5176 Appendix A: `space` is 1, `A` 34, `a` 66). Such
//! a font has no `/ToUnicode` and an `Identity` ordering, so ISO 32000-2 §9.10.2 gives its codes
//! no Unicode value and the strict text reads nothing; read as the SIDs they are, the CIDs spell
//! the text (`The Residence address`). A Type 3 font made from such a font numbers its codes the
//! same way (`/c37` at code 37 draws `D`), where the strict text reads the codes as ASCII and
//! gets every letter shifted by 31 (`%BUB%JSFDU` for `DataDirect`).
//!
//! The order is only taken when the font proves it, because a font numbered almost the same way
//! (one glyph more or less in front, or the Macintosh order, two ahead) would copy every letter
//! as its neighbour, where today it copies nothing:
//!
//! - the ids read as text ([`super::plausible_reading`]) and are not one gapless block
//!   ([`super::renumbered_block`]), and
//! - the glyphs' own widths agree with the letters they are read as ([`widths_agree`]). A
//!   reading one off puts an `l` where the `m` is, or an `m` where the `l` is, and the widths
//!   contradict it.
//!
//! A monospaced font, or one too small to compare, proves nothing and reads nothing.

use super::widths::{read_widths, widths_agree};
use super::{
    FontFacts, FontKind, GlyphFacts, RecoveryMode, RecoveryRule, Rule, plausible_reading,
    renumbered_block, symbol_font,
};
use crate::font::is_numbered_glyph_name;
use crate::font::recovery::orders;
use hayro_cmap::BfString;

/// [`RecoveryRule::StandardStringOrder`].
pub(crate) struct StandardStringOrder;

/// What [`StandardStringOrder`] keeps for a font it takes.
#[derive(Debug)]
pub(crate) struct StandardOrder {
    /// Whether the strict text is replaced: a Type 3 font's codes read as ASCII, where the glyph
    /// widths prove the standard strings' reading and contradict the ASCII one (or the ASCII
    /// one doesn't read as text).
    override_strict: bool,
    /// The ids the font has a glyph for, sorted: an id it has none for draws `.notdef` and reads
    /// as nothing.
    ids: Vec<u32>,
}

/// Why [`StandardStringOrder`] replaces the strict text of a Type 3 font.
const ASCII_GUESS: &str = "the strict text reads the codes as ASCII, but they are CFF standard string ids, \
     which the glyph widths confirm";

/// The last string id read: SIDs 1 to 95 are printable ASCII (TN #5176 Appendix A), which the
/// width evidence checks letter by letter. Past them come the other glyphs of Adobe's standard
/// encoding and the ISOAdobe charset's Latin-1 additions (quotes, dashes, the bullet, accents,
/// symbols), whose widths prove nothing about their order, and where a converter's numbering
/// was seen to depart from the strings' (a `©` drawn at code 166, `minus`): those ids read
/// nothing.
const MAX_SID: u32 = 95;

/// The last string id read in a Type 3 font whose ASCII reading the rule overrides: the other
/// glyphs of Adobe's standard encoding (quotes, dashes, the bullet, ligatures, accents; TN #5176
/// Appendix A). There the strict text is known to be wrong (a `“` drawn at code 105 reads as an
/// `i`), so the standard strings' reading replaces it where it would otherwise be kept.
const MAX_OVERRIDE_SID: u32 = 149;

/// The character standard string id `sid` gives, up to [`MAX_SID`].
fn standard_string_char(sid: u32) -> Option<char> {
    (sid <= MAX_SID)
        .then(|| orders::standard_string_char(sid))
        .flatten()
}

/// Most ids the guard looks at: a CID-keyed program may hold 65,536 glyphs, a Type 3 font 256.
const MAX_IDS: usize = 65_536;

impl Rule for StandardStringOrder {
    const RULE: RecoveryRule = RecoveryRule::StandardStringOrder;
    const KINDS: &'static [FontKind] = &[FontKind::CidCff, FontKind::Type3];
    type Guard = StandardOrder;

    fn guard(font: &FontFacts<'_>) -> Option<StandardOrder> {
        let type3 = match font.kind {
            FontKind::CidCff => false,
            FontKind::Type3 => true,
            _ => return None,
        };
        if font.has_to_unicode || symbol_font(font.base_font) {
            return None;
        }
        let (Some(ids), Some(width)) = (font.ids, font.id_width) else {
            return None;
        };
        let mut ids: Vec<u32> = ids()
            .into_iter()
            .filter(|&id| id != 0)
            .take(MAX_IDS)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        if type3 {
            // The strict text of a Type 3 font with glyph names of the glyph list is right:
            // only numbered names (`/c37`) leave the codes to be read.
            let names = font.names?;
            let named = |id: u32| u8::try_from(id).ok().and_then(names);
            if !ids
                .iter()
                .all(|&id| named(id).is_some_and(is_numbered_glyph_name))
            {
                return None;
            }
        }
        if ids.len() < 3 || renumbered_block(&ids) {
            return None;
        }
        let basic = |id: u32| (1..=95).contains(&id);
        // A Type 3 font's glyphs past ASCII (quotes, dashes) may be read where it is overridden,
        // so they count as read here; a CID-keyed font's never are.
        let read = |id: u32| {
            if type3 && id <= MAX_OVERRIDE_SID {
                orders::standard_string_char(id)
            } else {
                standard_string_char(id)
            }
        };
        if !plausible_reading(ids.iter().map(|&id| (read(id), basic(id)))) {
            return None;
        }
        if !widths_agree(&read_widths(&ids, standard_string_char, width)).proves() {
            return None;
        }
        // A Type 3 font's strict text is its codes read as ASCII: it is replaced only when that
        // reading is evidently wrong (its widths contradict it, or it doesn't read as text);
        // otherwise the rule fills glyphs without strict text only.
        let override_strict = type3 && {
            let ascii = |id: u32| {
                u8::try_from(id)
                    .ok()
                    .filter(|b| (0x20..0x7f).contains(b))
                    .map(char::from)
            };
            let basic = |id: u32| (0x20..0x7f).contains(&id);
            widths_agree(&read_widths(&ids, ascii, width)).contradict > 0
                || !plausible_reading(ids.iter().map(|&id| (ascii(id), basic(id))))
        };
        Some(StandardOrder {
            override_strict,
            ids,
        })
    }

    fn read(
        guard: &StandardOrder,
        _: &FontFacts<'_>,
        glyph: &GlyphFacts<'_>,
    ) -> Option<(BfString, RecoveryMode)> {
        let id = glyph.id.filter(|id| guard.ids.binary_search(id).is_ok())?;
        let c = if guard.override_strict && (MAX_SID + 1..=MAX_OVERRIDE_SID).contains(&id) {
            orders::standard_string_char(id)?
        } else {
            standard_string_char(id)?
        };
        let mode = if guard.override_strict {
            RecoveryMode::Override {
                reason: ASCII_GUESS,
            }
        } else {
            RecoveryMode::Fill
        };
        Some((BfString::Char(c), mode))
    }
}
