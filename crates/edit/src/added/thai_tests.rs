//! Existing Thai text drawn from the bundled Thai outlines (`thai_glyph_resources`): one Type 3
//! code per distinct glyph and cluster text, in fonts of `GLYPHS_PER_FONT` codes each. The
//! outline checks need the craft-fonts Thai face; the budget check runs without it.

use pdfcraft_cos::{Dict, Document};
use pdfcraft_fonts::source_shaping::{ShapedGlyph, ShapedRun};

use super::*;

/// `count` copies of `glyph`, each with cluster text of its own (so each needs a code of its own).
fn distinct(glyph: &ShapedGlyph, count: usize) -> ShapedRun {
    let glyphs = (0..count).map(|i| ShapedGlyph { position: [i as f64, 0.0], text: format!("{i}"), ..glyph.clone() }).collect();
    ShapedRun { glyphs, advance: count as f64 }
}

#[test]
fn thai_glyph_resources_refuse_a_row_past_the_glyph_budget() {
    let glyph = ShapedGlyph { id: Default::default(), position: [0.0, 0.0], text: String::new() };
    let over = ShapedRun { glyphs: vec![glyph; 65_535], advance: 0.0 };
    let (mut doc, mut fonts) = (Document::new_empty(), Dict::new());
    assert!(thai_glyph_resources(&mut doc, &mut fonts, &over).is_err());
    assert!(fonts.is_empty(), "a refused row adds no font");
}

/// Codes fill a font up to `GLYPHS_PER_FONT`, the next distinct cluster starts another font at
/// code 1, and the same glyph with the same text again reuses its code. One glyph drawn for two
/// cluster texts takes two codes, since each code's ToUnicode gives its own text.
#[test]
fn thai_glyph_resources_shard_distinct_clusters_at_the_font_boundary() {
    let Some(face) = pdfcraft_fonts::document_thai_font() else {
        eprintln!(
            "skipping thai_glyph_resources_shard_distinct_clusters_at_the_font_boundary: built without craft-fonts (set CRAFT_FONTS_DIR to run it)"
        );
        return;
    };
    let ko = pdfcraft_fonts::source_shaping::shape(face.bytes, "ก").unwrap().glyphs.remove(0);
    for (clusters, fonts_used) in [(GLYPHS_PER_FONT - 1, 1), (GLYPHS_PER_FONT, 1), (GLYPHS_PER_FONT + 1, 2)] {
        let mut shaped = distinct(&ko, clusters);
        shaped.glyphs.push(shaped.glyphs[0].clone());
        let (mut doc, mut fonts) = (Document::new_empty(), Dict::new());
        let placed = thai_glyph_resources(&mut doc, &mut fonts, &shaped).unwrap();
        assert_eq!((fonts.len(), placed.len()), (fonts_used, clusters + 1), "{clusters} clusters");
        assert_eq!(placed.last(), placed.first(), "a repeated glyph and text reuses its code");
        let codes: std::collections::HashSet<_> = placed.iter().collect();
        assert_eq!(codes.len(), clusters, "every distinct cluster has a code of its own");
        assert_eq!(placed[GLYPHS_PER_FONT - 2].1, [(GLYPHS_PER_FONT - 1) as u8]);
        if clusters > GLYPHS_PER_FONT {
            assert_eq!(placed[GLYPHS_PER_FONT - 1].1, [GLYPHS_PER_FONT as u8]);
            assert_eq!(placed[GLYPHS_PER_FONT].1, [1], "the next font starts at code 1");
            assert_ne!(placed[GLYPHS_PER_FONT].0, placed[0].0);
        }
    }
    // At the budget: one cluster repeated 65 534 times needs one code.
    let mut row = distinct(&ko, 1);
    row.glyphs = vec![row.glyphs[0].clone(); 65_534];
    let (mut doc, mut fonts) = (Document::new_empty(), Dict::new());
    let placed = thai_glyph_resources(&mut doc, &mut fonts, &row).unwrap();
    assert_eq!((fonts.len(), placed.len()), (1, 65_534));
}
