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
use pdfcraft_fonts::fallback::{self, Request};

/// The face that draws a CID font the document doesn't embed: the chain's stand-in (the exact
/// craft-fonts family the font names, else a Han face for an Adobe CJK collection). `None` for a
/// simple font, whose codes reach glyphs through its encoding's glyph names, and when there is no
/// such face (built without craft-fonts): hayro's standard substitute stays.
pub(super) fn resolve(query: &FontQuery) -> Option<(FontData, u32)> {
    let FontQuery::Fallback(query) = query else { return None };
    query.character_collection.as_ref()?;
    with_request(query, fallback::stand_in).map(face_data)
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
