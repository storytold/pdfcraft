//! The faces that draw a Type 0 font the document doesn't embed (#5329), as the substitution chain
//! (`pdfcraft_fonts::fallback`) ranks them, so the face a font report names is the face drawn.
//!
//! hayro's own substitutes are the Latin standard 14, so text in a CID font of an Adobe CJK
//! collection that isn't embedded (`STSong-Light`, `HeiseiMin-W3`, `HYSMyeongJo-Medium`, …) drew
//! nothing, and any other character such a font's substitute lacks drew nothing or an unrelated
//! glyph. Now the chain's stand-in draws such a font, and its ranked faces are the further faces
//! (`InterpreterSettings::further_faces`) for the characters the substitute lacks.

use std::sync::Arc;

use hayro::hayro_interpret::font::{FallbackFontQuery, FontData, FontQuery};
use hayro::hayro_interpret::hayro_cmap::CidFamily;
use pdfcraft_fonts::CraftFont;
use pdfcraft_fonts::fallback::{self, Request, Why};

/// The face that draws a CID font the document doesn't embed: the chain's stand-in (the exact
/// craft-fonts family the font names, else a Han face for an Adobe CJK collection). `None` for a
/// simple font, whose codes reach glyphs through its encoding's glyph names, and when there is no
/// such face (built without craft-fonts): hayro's standard substitute stays.
pub(super) fn resolve(query: &FontQuery) -> Option<(FontData, u32)> {
    let FontQuery::Fallback(query) = query else { return None };
    query.character_collection.as_ref()?;
    with_request(query, fallback::stand_in).map(face_data)
}

/// [`resolve`], but only when the stand-in is the exact family the font names or a face for its
/// collection's script; `None` when it would be a face for the nearest Han script (Traditional
/// Chinese drawn with Japanese glyph forms), so that an installed face for the script, on screen
/// (`RenderConfig::system_fonts`), comes before it.
pub(super) fn resolve_for_script(query: &FontQuery) -> Option<(FontData, u32)> {
    let FontQuery::Fallback(fallback_query) = query else { return None };
    if with_request(fallback_query, fallback::substitute).why == Why::NearestScriptFace {
        return None;
    }
    resolve(query)
}

/// Every face, best first, for the characters `query`'s substitute lacks. The stand-in is among
/// them; hayro skips the face it is already drawing with.
pub(super) fn further_faces(query: &FallbackFontQuery) -> Vec<(FontData, u32)> {
    with_request(query, fallback::further_faces).into_iter().map(face_data).collect()
}

fn face_data(face: &'static CraftFont) -> (FontData, u32) {
    (Arc::new(face.bytes), 0)
}

/// `f` of the chain's request for `query`: the font's name, the descriptor flags hayro read, its
/// `/FontWeight` and its collection's ordering.
fn with_request<T>(query: &FallbackFontQuery, f: impl FnOnce(&Request<'_>) -> T) -> T {
    let ordering = query.character_collection.as_ref().and_then(|c| match &c.family {
        CidFamily::AdobeJapan1 => Some("Japan1".to_owned()),
        CidFamily::AdobeGB1 => Some("GB1".to_owned()),
        CidFamily::AdobeCNS1 => Some("CNS1".to_owned()),
        CidFamily::AdobeKorea1 => Some("Korea1".to_owned()),
        CidFamily::AdobeIdentity => Some("Identity".to_owned()),
        // Adobe-KR, and the orderings of other registries, by name.
        CidFamily::Custom { ordering, .. } => std::str::from_utf8(ordering).ok().map(str::to_owned),
    });
    // ISO 32000-2 table 121: FixedPitch is bit 1, Serif bit 2, Italic bit 7.
    let flags = u32::from(query.is_fixed_pitch) | u32::from(query.is_serif) << 1 | u32::from(query.is_italic) << 6;
    f(&Request {
        base_font: query.post_script_name.as_deref().unwrap_or_default(),
        embedded: false,
        flags,
        ordering: ordering.as_deref(),
        weight: query.font_weight,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hayro::hayro_interpret::hayro_cmap::CharacterCollection;

    fn query(name: &str, family: CidFamily) -> FontQuery {
        FontQuery::Fallback(FallbackFontQuery {
            post_script_name: Some(name.into()),
            character_collection: Some(CharacterCollection { family, supplement: 2 }),
            ..FallbackFontQuery::default()
        })
    }

    /// The renderer tries an installed face (on screen) between the two: a craft-fonts face for
    /// the font's own script wins over it, a face for the nearest Han script (CNS1 drawn with
    /// Japanese glyph forms) does not.
    #[test]
    fn only_a_face_for_the_fonts_own_script_comes_before_an_installed_face() {
        let gb1 = query("STSong-Light", CidFamily::AdobeGB1);
        let cns1 = query("MSung-Light", CidFamily::AdobeCNS1);
        let simple = FontQuery::Fallback(FallbackFontQuery { post_script_name: Some("Unknown".into()), ..FallbackFontQuery::default() });
        assert!(resolve_for_script(&simple).is_none() && resolve(&simple).is_none(), "a simple font keeps hayro's substitute");
        if resolve(&gb1).is_none() {
            eprintln!("built without craft-fonts (CRAFT_FONTS_DIR unset): no stand-in to order");
            assert!(resolve_for_script(&gb1).is_none() && resolve(&cns1).is_none() && resolve_for_script(&cns1).is_none());
            return;
        }
        assert!(resolve_for_script(&gb1).is_some(), "GB1's own-script face comes first");
        assert!(resolve(&cns1).is_some(), "CNS1 has a nearest-script stand-in");
        assert!(resolve_for_script(&cns1).is_none(), "which yields to an installed Traditional Chinese face");
    }
}
