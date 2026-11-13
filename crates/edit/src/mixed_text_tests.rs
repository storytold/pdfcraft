use super::{covered_japanese_chars, page_content_bytes, reopen, text_page};
use crate::added::{AddedText, Content, add_content, delete_content, first_undrawable, list_added, text_rect, update_content};
use crate::text;

const MIXED: &str = "Agpq 中文 简体汉字 联络 编辑 保存 重开 123";

fn has_hans() -> bool {
    pdfcraft_fonts::CRAFT_FONTS.iter().any(|face| face.covers("Hans"))
}

#[test]
fn mixed_text_added_chinese_is_drawn_and_retyped_after_reopen() {
    let mut doc = text_page("BT /F1 12 Tf 72 650 Td (Untouched neighbour) Tj ET");
    let mut item = AddedText { text: MIXED.into(), rect: [72.0, 480.0, 560.0, 520.0], size: 16.0, ..Default::default() };
    item.rect = text_rect(&item);
    if !has_hans() {
        assert!(add_content(&mut doc, 0, &Content::Text(item)).unwrap_err().to_string().contains("CRAFT_FONTS_DIR"));
        assert!(!doc.is_modified());
        return;
    }
    assert_eq!(first_undrawable(&item), None);
    let index = add_content(&mut doc, 0, &Content::Text(item.clone())).unwrap();
    let mut doc = reopen(&doc);
    assert_eq!(list_added(&doc)[0].content, Content::Text(item.clone()));
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|line| line.text.as_str()).collect::<String>(), format!("Untouched neighbour{MIXED}"));
    let mut changed = AddedText { text: "Agpq 中文 123 保存".into(), ..item };
    changed.rect = text_rect(&changed);
    update_content(&mut doc, 0, index, &Content::Text(changed.clone())).unwrap();
    let doc = reopen(&doc);
    assert_eq!(list_added(&doc)[0].content, Content::Text(changed.clone()));
    assert_eq!(
        text::text_lines(&doc, 0).unwrap().iter().map(|line| line.text.as_str()).collect::<String>(),
        format!("Untouched neighbour{}", changed.text)
    );
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

#[test]
fn mixed_text_wrapped_paragraph_stays_whole_across_repeated_edits() {
    if !has_hans() {
        return;
    }
    let mut doc = text_page("BT /F1 24 Tf 20 700 Td (Source) Tj ET");
    let style = text::BlockStyle { width: Some(180.0), ..Default::default() };
    for replacement in [MIXED.to_owned(), format!("{MIXED} 测试"), format!("{MIXED} 再次编辑")] {
        text::rewrite_block(&mut doc, 0, 0, Some(&replacement), &style).unwrap();
        doc = reopen(&doc);
        let blocks = text::text_blocks(&doc, 0).unwrap();
        assert_eq!(blocks.len(), 1, "all wrapped rows remain the same paragraph: {blocks:?}");
        assert_eq!(blocks[0].editor_text.split_whitespace().collect::<String>(), replacement.split_whitespace().collect::<String>());
    }
}

#[test]
fn mixed_text_missing_glyph_refuses_without_changing_the_document() {
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Source) Tj ET");
    let before = page_content_bytes(&doc, 0);
    let item = AddedText { text: "Agpq \u{10ffff}".into(), rect: [72.0, 480.0, 560.0, 520.0], ..Default::default() };
    let error = add_content(&mut doc, 0, &Content::Text(item)).unwrap_err().to_string();
    assert!(error.contains("U+10FFFF"), "{error}");
    assert_eq!(page_content_bytes(&doc, 0), before);
    assert!(!doc.is_modified());
}

#[test]
fn mixed_text_added_fonts_are_owned_by_the_item_and_split_at_240_glyphs() {
    if !has_hans() {
        return;
    }
    let mut doc = text_page("");
    let first = AddedText { text: covered_japanese_chars(241), rect: [20.0, 700.0, 580.0, 750.0], ..Default::default() };
    add_content(&mut doc, 0, &Content::Text(first.clone())).unwrap();
    let second = AddedText { text: MIXED.into(), rect: [20.0, 400.0, 580.0, 450.0], ..Default::default() };
    add_content(&mut doc, 0, &Content::Text(second.clone())).unwrap();
    let count = |doc: &pdfcraft_cos::Document| {
        let p = pdfcraft_model::pages(doc).remove(0);
        let res = doc.dict(p.dict.get(b"Resources").unwrap()).unwrap();
        let fonts = doc.dict(res.get(b"Font").unwrap()).unwrap();
        fonts.iter().filter(|(name, _)| name.starts_with(b"PCCjk")).count()
    };
    assert_eq!(count(&doc), 3);
    for _ in 0..3 {
        update_content(&mut doc, 0, 0, &Content::Text(first.clone())).unwrap();
        doc = reopen(&doc);
        assert_eq!(count(&doc), 3, "retyping must retire the old item's fonts");
        assert_eq!(
            text::text_lines(&doc, 0).unwrap().iter().map(|line| line.text.as_str()).collect::<String>(),
            format!("{}{}", first.text, second.text)
        );
    }
    delete_content(&mut doc, 0, 0).unwrap();
    assert_eq!(count(&doc), 1);
    assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap().iter().map(|line| line.text.as_str()).collect::<String>(), MIXED);
    delete_content(&mut doc, 0, 0).unwrap();
    assert_eq!(count(&doc), 0);
}

#[test]
fn mixed_text_outline_budget_is_checked_before_document_changes() {
    let mut doc = text_page("");
    let before = page_content_bytes(&doc, 0);
    let item = AddedText {
        text: (0x4e00..).filter_map(char::from_u32).take(pdfcraft_fonts::MAX_PLAN_GLYPHS + 1).collect(),
        rect: [72.0, 480.0, 560.0, 520.0],
        ..Default::default()
    };
    let error = add_content(&mut doc, 0, &Content::Text(item)).unwrap_err().to_string();
    assert!(error.contains("too many distinct CJK glyphs"), "{error}");
    assert_eq!(page_content_bytes(&doc, 0), before);
    assert!(!doc.is_modified());
}

#[test]
fn mixed_text_font_names_in_literal_text_do_not_own_neighbours_fonts() {
    for (text, prefix, available) in
        [("中文", b"PCCjk".as_slice(), has_hans()), ("العربية", b"PCAr".as_slice(), pdfcraft_fonts::document_arabic_font().is_some())]
    {
        if !available {
            continue;
        }
        let fonts = |doc: &pdfcraft_cos::Document| {
            let page = pdfcraft_model::pages(doc).remove(0);
            let resources = doc.dict(page.dict.get(b"Resources").unwrap()).unwrap();
            doc.dict(resources.get(b"Font").unwrap()).unwrap()
        };
        let mut original = text_page("");
        let neighbour = AddedText { text: text.into(), rect: [20.0, 600.0, 580.0, 650.0], ..Default::default() };
        add_content(&mut original, 0, &Content::Text(neighbour)).unwrap();
        original = reopen(&original);
        let owned = fonts(&original);
        let name = owned.iter().find(|(name, _)| name.starts_with(prefix)).unwrap().0;
        let literal =
            AddedText { text: format!("A /{} 12 Tf B", std::str::from_utf8(name).unwrap()), rect: [20.0, 400.0, 580.0, 450.0], ..Default::default() };
        add_content(&mut original, 0, &Content::Text(literal.clone())).unwrap();
        for delete in [false, true] {
            let mut doc = reopen(&original);
            if delete {
                delete_content(&mut doc, 0, 1).unwrap();
            } else {
                update_content(&mut doc, 0, 1, &Content::Text(AddedText { text: "Changed".into(), ..literal.clone() })).unwrap();
            }
            let doc = reopen(&doc);
            assert_eq!(fonts(&doc).get(name), owned.get(name), "neighbour's font removed: {text}, delete={delete}");
        }
    }
}

fn assert_saved_glyphs_fit(item: &AddedText) {
    let mut doc = text_page("");
    add_content(&mut doc, 0, &Content::Text(item.clone())).unwrap();
    let doc = reopen(&doc);
    let page = pdfcraft_model::pages(&doc).remove(0);
    let glyphs = text::glyph_text_lines(&doc, &page).unwrap();
    assert!(!glyphs.is_empty());
    assert_eq!(glyphs.iter().map(|run| run.text.as_str()).collect::<String>(), crate::added::lines(item).concat());
    for run in glyphs {
        assert!(run.rect[0] >= item.rect[0] - 0.02 && run.rect[2] <= item.rect[2] + 0.02, "{item:?}: {run:?}");
    }
}

#[test]
fn mixed_text_unspaced_chinese_wraps_at_cjk_breaks_and_fits_saved_box() {
    if !has_hans() {
        return;
    }
    let mut item =
        AddedText { text: "中文编辑保存中文编辑保存".into(), rect: [20.0, 300.0, 68.0, 700.0], size: 12.0, ..Default::default() };
    assert_eq!(crate::added::lines(&item), ["中文编辑", "保存中文", "编辑保存"]);
    for text in ["中文编辑保存中文编辑保存", "中文（编辑），保存中文。", "中文test中文\n编辑保存中文"] {
        item.text = text.into();
        let rows = crate::added::lines(&item);
        assert!(rows.len() > 1, "{rows:?}");
        assert_eq!(rows.concat(), text.replace('\n', ""));
        assert!(rows.iter().all(|row| !row.starts_with(['）', '，', '。']) && !row.ends_with('（')), "{rows:?}");
        assert_saved_glyphs_fit(&item);
    }
}

#[test]
fn mixed_text_thai_chinese_wraps_with_saved_cjk_advances() {
    if !has_hans() || pdfcraft_fonts::document_thai_font().is_none() {
        return;
    }
    for width in [36.0, 48.0, 60.0, 72.0, 84.0, 96.0, 120.0] {
        let item =
            AddedText {
                text: "ไทย中文编辑保存ไทย中文编辑保存".into(), rect: [20.0, 300.0, 20.0 + width, 700.0], size: 12.0, ..Default::default()
            };
        assert_saved_glyphs_fit(&item);
    }
}

#[test]
fn mixed_text_measurement_matches_saved_glyph_advances_and_alignment() {
    if !has_hans() {
        return;
    }
    for spacing in [0.0, 1.25] {
        for align in [crate::added::Align::Left, crate::added::Align::Center, crate::added::Align::Right] {
            let mut doc = text_page("");
            let item = AddedText {
                text: "Agpq 中文 123".into(),
                rect: [72.0, 600.0, 572.0, 700.0],
                size: 16.0,
                char_spacing: spacing,
                align,
                ..Default::default()
            };
            let measured = crate::added::text_width(&item);
            add_content(&mut doc, 0, &Content::Text(item.clone())).unwrap();
            let doc = reopen(&doc);
            let page = pdfcraft_model::pages(&doc).remove(0);
            // Read the saved glyphs, bypassing ActualText, so correct Unicode alone cannot pass.
            let lines = text::glyph_text_lines(&doc, &page).unwrap();
            assert_eq!(lines.iter().map(|line| line.text.as_str()).collect::<String>(), item.text);
            let left = lines.iter().map(|line| line.rect[0]).fold(f64::INFINITY, f64::min);
            let right = lines.iter().map(|line| line.rect[2]).fold(f64::NEG_INFINITY, f64::max);
            assert!((right - left - measured).abs() < 0.02, "{align:?}, {spacing}: {} != {measured}", right - left);
            let expected = match align {
                crate::added::Align::Left => 72.0,
                crate::added::Align::Center => 72.0 + (500.0 - measured) / 2.0,
                _ => 572.0 - measured,
            };
            assert!((left - expected).abs() < 0.02, "{align:?}: {left} != {expected}");
        }
    }
}

#[test]
fn mixed_text_justification_does_not_stretch_cjk_glyph_code_32() {
    if !has_hans() {
        return;
    }
    let chars = covered_japanese_chars(40);
    let mut doc = text_page("");
    let item = AddedText {
        text: format!("Agpq {chars} 123 {}", "next".repeat(50)),
        rect: [72.0, 500.0, 572.0, 700.0],
        size: 8.0,
        align: crate::added::Align::Justify,
        ..Default::default()
    };
    let cjk = AddedText { text: chars.clone(), ..item.clone() };
    let expected = crate::added::text_width(&cjk);
    add_content(&mut doc, 0, &Content::Text(item)).unwrap();
    let doc = reopen(&doc);
    let page = pdfcraft_model::pages(&doc).remove(0);
    let glyphs = text::glyph_text_lines(&doc, &page).unwrap();
    let run = glyphs.iter().find(|line| line.text == chars).unwrap();
    assert!((run.rect[2] - run.rect[0] - expected).abs() < 0.02, "a CJK code 32 is not an ASCII space: {run:?}");
}
