//! Approved bundled faces for missing glyphs in non-embedded simple PDF fonts.
//! No system font lookup, file I/O, font embedding or document mutation occurs here.
use hayro::hayro_interpret::font::{FontData, FontQuery};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use std::sync::{Arc, LazyLock};

struct Face {
    data: FontData,
    font: FontRef<'static>,
    serif: bool,
    monospace: bool,
    bold: bool,
}

// FontRef reads table headers, not every glyph. Initialized only on a missing-glyph query;
// immutable data/coverage is shared by render workers, and the interpreter caches outlines.
static FACES: LazyLock<Vec<Face>> = LazyLock::new(|| {
    let mut inputs: Vec<(&'static [u8], bool, bool, bool)> = vec![
        (pdfcraft_fonts::INTER_REGULAR, false, false, false),
        (pdfcraft_fonts::INTER_SEMIBOLD, false, false, true),
        (pdfcraft_fonts::JETBRAINS_MONO_REGULAR, false, true, false),
    ];
    // Prefer a real serif where the optional, approved build input provides one.
    // Other CID/CJK substitution remains in the existing Japanese resolver.
    inputs.extend(
        pdfcraft_fonts::CRAFT_FONTS
            .iter()
            .filter(|f| f.covers("Jpan") && f.family.contains("Mincho"))
            .map(|f| (f.bytes, true, false, f.style.contains("Bold"))),
    );
    inputs
        .into_iter()
        .filter_map(|(bytes, serif, monospace, bold)| {
            Some(Face { data: Arc::new(bytes), font: FontRef::from_index(bytes, 0).ok()?, serif, monospace, bold })
        })
        .collect()
});

/// Choose a covering approved face for a `MissingGlyph` query; return `None` for other queries
/// or uncovered characters. Prefer matching serif/monospace traits, then weight, with registry
/// order breaking ties. Returned bytes are shared and the face index is zero; no font is loaded
/// from the filesystem or installed-font configuration.
pub(crate) fn resolve(query: &FontQuery) -> Option<(FontData, u32)> {
    let FontQuery::MissingGlyph { character, standard_font } = query else {
        return None;
    };
    FACES
        .iter()
        .filter(|f| f.font.charmap().map(*character).is_some_and(|g| g != GlyphId::NOTDEF))
        .min_by_key(|f| {
            (
                u8::from(f.serif != standard_font.is_serif()) * 2 + u8::from(f.monospace != standard_font.is_monospace()),
                f.bold != standard_font.is_bold(),
            )
        })
        .map(|f| (f.data.clone(), 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hayro::hayro_interpret::font::StandardFont;

    /// Verify coverage and family/weight selection against the shared bundled bytes, including
    /// optional Mincho preference, an uncovered character and reuse of the same font-data Arc.
    #[test]
    fn bundled_coverage_and_style_selection_are_deterministic() {
        let get = |standard_font, character| resolve(&FontQuery::MissingGlyph { character, standard_font });
        let (regular, _) = get(StandardFont::Helvetica, 'Ж').unwrap();
        let (bold, _) = get(StandardFont::HelveticaBold, 'Ж').unwrap();
        let (mono, _) = get(StandardFont::Courier, 'Ж').unwrap();
        assert_eq!(regular.as_ref().as_ref(), pdfcraft_fonts::INTER_REGULAR);
        assert_eq!(bold.as_ref().as_ref(), pdfcraft_fonts::INTER_SEMIBOLD);
        assert_eq!(mono.as_ref().as_ref(), pdfcraft_fonts::JETBRAINS_MONO_REGULAR);
        let (serif, _) = get(StandardFont::TimesRoman, 'Ж').unwrap();
        let mincho = pdfcraft_fonts::CRAFT_FONTS.iter().find(|f| {
            f.covers("Jpan")
                && f.family.contains("Mincho")
                && FontRef::from_index(f.bytes, 0).is_ok_and(|font| font.charmap().map('Ж').is_some_and(|g| g != GlyphId::NOTDEF))
        });
        assert_eq!(serif.as_ref().as_ref(), mincho.map_or(pdfcraft_fonts::INTER_REGULAR, |f| f.bytes));
        assert!(get(StandardFont::Helvetica, 'ľ').is_some());
        assert!(get(StandardFont::Helvetica, '\u{1fae0}').is_none());
        assert!(Arc::ptr_eq(&regular, &get(StandardFont::Helvetica, 'Ж').unwrap().0));
    }
}
