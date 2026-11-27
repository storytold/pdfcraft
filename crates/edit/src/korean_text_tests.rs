use super::{page_content_bytes, reopen, text_page};
use crate::added::{AddedText, Content, add_content, first_undrawable, list_added, text_rect, update_content};
use crate::text;

/// Hangul syllables, a compatibility jamo typed on its own, Latin and digits on one line (#7675).
const KOREAN: &str = "Agpq 안녕하세요 한국어 ㅋ 123";

fn page_text(doc: &crate::Document) -> String {
    text::text_lines(doc, 0).unwrap().iter().map(|line| line.text.as_str()).collect()
}

/// Add Text with Korean: drawn with the Korean face, kept as typed after a save and reopen, and
/// retyped in place. Without a Korean face the item is refused, naming the build input, and the
/// page is left as it was.
#[test]
fn added_korean_text_is_drawn_and_retyped_after_reopen() {
    let mut doc = text_page("BT /F1 12 Tf 72 650 Td (Untouched neighbour) Tj ET");
    let mut item = AddedText { text: KOREAN.into(), rect: [72.0, 480.0, 560.0, 520.0], size: 16.0, ..Default::default() };
    item.rect = text_rect(&item);
    if pdfcraft_fonts::document_korean_font().is_none() {
        let before = page_content_bytes(&doc, 0);
        assert!(add_content(&mut doc, 0, &Content::Text(item)).unwrap_err().to_string().contains("CRAFT_FONTS_DIR"));
        assert_eq!(page_content_bytes(&doc, 0), before);
        return;
    }
    assert_eq!(first_undrawable(&item), None, "every character of {KOREAN:?} has a face");
    let index = add_content(&mut doc, 0, &Content::Text(item.clone())).unwrap();
    let mut doc = reopen(&doc);
    assert_eq!(list_added(&doc)[0].content, Content::Text(item.clone()));
    assert_eq!(page_text(&doc), format!("Untouched neighbour{KOREAN}"));
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(!content.contains("(?"), "no character is written as ?: {content}");

    let mut changed = AddedText { text: "Agpq 수정됨 123".into(), ..item };
    changed.rect = text_rect(&changed);
    update_content(&mut doc, 0, index, &Content::Text(changed.clone())).unwrap();
    let doc = reopen(&doc);
    assert_eq!(list_added(&doc)[0].content, Content::Text(changed.clone()));
    assert_eq!(page_text(&doc), format!("Untouched neighbour{}", changed.text));
}
