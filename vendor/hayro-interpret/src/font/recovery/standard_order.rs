//! PdfCraft patch (104): [`StandardStringOrder`], rule N, a font whose CIDs or codes are CFF
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
    /// widths prove the standard strings' reading.
    override_strict: bool,
}

/// Why [`StandardStringOrder`] replaces the strict text of a Type 3 font.
const ASCII_GUESS: &str = "the strict text reads the codes as ASCII, but they are CFF standard string ids, \
     which the glyph widths confirm";

/// The last string id read: SIDs 1 to 149 are printable ASCII and the other glyphs of Adobe's
/// standard encoding (quotes, dashes, the bullet, ligatures, accents; TN #5176 Appendix A).
/// Past them the ISOAdobe charset adds Latin-1's symbols and accented capitals, where a
/// converter's numbering was seen to depart from the strings' (a `©` drawn at code 166,
/// `minus`), so those ids read nothing.
const MAX_SID: u32 = 149;

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
        if !plausible_reading(ids.iter().map(|&id| (standard_string_char(id), basic(id)))) {
            return None;
        }
        if !widths_agree(&read_widths(&ids, standard_string_char, width)).proves() {
            return None;
        }
        Some(StandardOrder {
            override_strict: type3,
        })
    }

    fn read(
        guard: &StandardOrder,
        _: &FontFacts<'_>,
        glyph: &GlyphFacts<'_>,
    ) -> Option<(BfString, RecoveryMode)> {
        let c = standard_string_char(glyph.id?)?;
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
