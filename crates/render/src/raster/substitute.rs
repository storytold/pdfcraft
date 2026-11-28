//! Adapts `pdfcraft_fonts::fallback` to hayro's Type 0 font resolver callbacks.
//!
//! `resolve` selects the stand-in. `further_faces` supplies ranked faces for missing Unicode
//! glyphs.

use std::sync::Arc;

use craft_pdf::interpret::font::{FallbackFontQuery, FontData, FontQuery};
use craft_pdf::interpret::hayro_cmap::CidFamily;
use pdfcraft_fonts::EmbeddedFace;
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

fn face_data(face: &'static EmbeddedFace) -> (FontData, u32) {
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
