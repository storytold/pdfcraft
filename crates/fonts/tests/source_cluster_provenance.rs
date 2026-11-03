use pdfcraft_fonts::{
    TextDirection,
    source_shaping::{ShapedRun, shape, shape_with_direction},
};

/// Require every glyph to retain a nonempty range in the original UTF-8 input;
/// an auxiliary glyph without Unicode text must share that range with its owner.
fn assert_original_ranges(text: &str, shaped: &ShapedRun) {
    for glyph in &shaped.glyphs {
        assert!(glyph.logical_range.start < glyph.logical_range.end, "every emitted glyph has an actual input cluster");
        let source = text.get(glyph.logical_range.clone()).expect("range is in the original UTF-8 input");
        if !glyph.text.is_empty() {
            assert_eq!(source, glyph.text, "Unicode owner and logical provenance agree");
        } else {
            assert!(
                shaped.glyphs.iter().any(|owner| owner.logical_range == glyph.logical_range && !owner.text.is_empty()),
                "auxiliary marks retain their cluster owner even without their own text"
            );
        }
    }
}

/// Repeated identical letters after multibyte input and bidi controls need distinct
/// original offsets under every paragraph base; text searching or placeholder ranges
/// would not preserve this provenance. Combining-mark ranges remain valid too.
#[test]
fn source_cluster_provenance_uses_original_offsets_across_bidi_and_controls() {
    let bytes = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
    let text = "é\u{2067}AA\u{2069} é";
    for direction in [TextDirection::Auto, TextDirection::LeftToRight, TextDirection::RightToLeft] {
        let shaped = shape_with_direction(bytes, text, direction).unwrap();
        assert_original_ranges(text, &shaped);
        let mut repeated: Vec<_> =
            shaped.glyphs.iter().filter(|glyph| glyph.text == "A").map(|glyph| (glyph.logical_range.start, glyph.logical_range.end)).collect();
        repeated.sort_unstable();
        assert_eq!(repeated, [(5, 6), (6, 7)], "identical text after multibyte prefix/control keeps distinct offsets");
    }
    let combining = "x\u{0301} x\u{0301}";
    assert_original_ranges(combining, &shape(bytes, combining).unwrap());
}

/// Repeated Arabic base-plus-mark clusters retain separate original byte ranges,
/// including empty-text auxiliary marks. PDF Unicode ownership reassignment must
/// not alter the shaped glyph IDs, positions, text or provenance.
#[test]
fn source_cluster_provenance_keeps_arabic_auxiliary_marks_with_repeated_bases() {
    let Some(font) = pdfcraft_fonts::document_arabic_font() else {
        eprintln!("set CRAFT_FONTS_DIR for Arabic provenance acceptance");
        return;
    };
    let text = "سَسَ 123";
    let shaped = shape(font.bytes, text).unwrap();
    assert_original_ranges(text, &shaped);
    assert!(shaped.glyphs.iter().any(|glyph| glyph.text.is_empty()), "the fixture must exercise auxiliary mark glyphs");
    for expected in [0..4, 4..8] {
        assert!(shaped.glyphs.iter().any(|glyph| glyph.logical_range == expected));
    }
    let before: Vec<_> = shaped.glyphs.iter().map(|glyph| (glyph.id, glyph.position, glyph.text.clone(), glyph.logical_range.clone())).collect();
    let mappings = shaped.pdf_mappings(font.bytes).unwrap();
    assert_eq!(mappings.len(), shaped.glyphs.len());
    let after: Vec<_> = shaped.glyphs.iter().map(|glyph| (glyph.id, glyph.position, glyph.text.clone(), glyph.logical_range.clone())).collect();
    assert_eq!(before, after, "PDF Unicode reassignment must not rewrite shaping provenance or geometry");
}
