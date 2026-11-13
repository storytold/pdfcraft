use super::{page_content_bytes, reopen, text_page};
use crate::text;

const MIXED: &str = "Agpq 中文 简体汉字 联络 编辑 保存 重开 123";

fn has_hans() -> bool {
    pdfcraft_fonts::CRAFT_FONTS.iter().any(|face| face.covers("Hans"))
}

#[test]
fn mixed_text_existing_paragraph_uses_hans_for_missing_source_glyphs() {
    if !has_hans() {
        return;
    }
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Source) Tj 0 -100 Td (Untouched neighbour) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some(MIXED), &Default::default()).unwrap();
    let doc = reopen(&doc);
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].editor_text, MIXED);
    assert!(String::from_utf8_lossy(&page_content_bytes(&doc, 0)).contains("Untouched neighbour"));
}
