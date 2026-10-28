//! Comprehensive Integration Tests for Bundle 4: Typography Editing, Subsetting, and Reflow.

use pdfcraft_subsetter::FontSubsetExpander;
use pdfcraft_reflow::{BidiClassifier, ParagraphReflowEngine, TextDirection};
use pdfcraft_content_editor::{BatesConfig, BatesNumberer, ContentDeltaPatcher, VectorElement, VectorPathEditor};

#[test]
fn test_in_place_editing_with_font_subset_expansion() {
    let mut font_expander = FontSubsetExpander::new("CBAAAA+Helvetica");
    // Initial subset has 'H', 'e', 'l', 'o'
    font_expander.register_existing(1, 'H', 722);
    font_expander.register_existing(2, 'e', 556);
    font_expander.register_existing(3, 'l', 222);
    font_expander.register_existing(4, 'o', 556);

    // In-place edit: user types "World!" -> 'W', 'r', 'd', '!' need new GIDs allocated
    let (gid_w, _) = font_expander.resolve_or_allocate_glyph('W');
    assert!(gid_w >= 5);

    let cmap = font_expander.generate_to_unicode_cmap();
    assert!(cmap.contains("<0005> <0057>")); // 'W' is 0x0057

    // Patch content stream AST
    let original_stream = b"BT /F1 12 Tf 100 700 Td (Hello) Tj ET";
    let patched_stream = ContentDeltaPatcher::patch_text(original_stream, "Hello", "Hello World!");
    let patched_str = String::from_utf8_lossy(&patched_stream);
    assert!(patched_str.contains("(Hello World!) Tj"));
}

#[test]
fn test_paragraph_reflow_and_bidi_classification() {
    // 1. Bi-directional run detection
    let mixed_text = "English text and عربي Arabic text";
    let runs = BidiClassifier::segment_runs(mixed_text);
    assert!(runs.len() >= 2);
    let rtl_run = runs.iter().find(|r| r.direction == TextDirection::RightToLeft).unwrap();
    assert!(rtl_run.text.contains("عربي"));

    // 2. Reflow text with hanging bullet indents
    let bullet_list = "• Item 1: This is a long explanation that will wrap onto another line.\n• Item 2: Short.";
    let lines = ParagraphReflowEngine::reflow_text(bullet_list, 140.0, 10.0, 13.0);
    assert!(lines.len() >= 3);
}

#[test]
fn test_vector_path_editing_and_bates_stamping() {
    // 1. Vector path manipulation
    let mut vec_editor = VectorPathEditor::new();
    vec_editor.add_element(VectorElement {
        id: 101,
        path_ops: "10 10 50 50 re f".into(),
        stroke_color: None,
        fill_color: Some([0.0, 1.0, 0.0]), // Green
        line_width: 1.0,
    });
    // Recolor to cyan
    vec_editor.recolor_fill(101, [0.0, 1.0, 1.0]);
    let stream_out = vec_editor.serialize_to_stream();
    assert!(String::from_utf8_lossy(&stream_out).contains("0.000 1.000 1.000 rg"));

    // 2. Legal Bates numbering
    let bates_cfg = BatesConfig::default();
    let stamp = BatesNumberer::format_stamp(&bates_cfg, 5);
    assert_eq!(stamp, "CONFIDENTIAL-000006");

    let overlay = BatesNumberer::generate_overlay_stream(&bates_cfg, 5, 612.0, 792.0);
    assert!(String::from_utf8_lossy(&overlay).contains("CONFIDENTIAL-000006"));
}
