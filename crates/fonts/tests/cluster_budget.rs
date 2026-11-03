use pdfcraft_cos::Document;
use pdfcraft_fonts::{GlyphOutline, ShapedCluster, ShapedScript, embed_shaped_clusters};
use std::collections::HashMap;

/// Repeated uses of one outline must count toward the emitted geometry budget.
#[test]
fn repeated_glyphs_cannot_multiply_the_outline_output_budget() {
    let mut doc = Document::new_empty();
    let outline = GlyphOutline { contours: vec![vec![[0.0, 0.0]; 1000]], width: 1.0, bbox: [0.0, 0.0, 1.0, 1.0] };
    let outlines = HashMap::from([((ShapedScript::Syriac, 1), outline)]);
    let cluster = ShapedCluster { script: ShapedScript::Syriac, glyphs: vec![(1, [0.0, 0.0]); 1001], advance: 1.0, text: "a".into() };
    let result = embed_shaped_clusters(&mut doc, &[&cluster], &outlines);
    assert!(result.is_err(), "repeated outlines must count against the emitted geometry budget");
}
