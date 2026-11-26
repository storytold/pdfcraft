use super::*;

fn page(content: &str, second_face: &str) -> Document {
    let mut doc = text_page(content);
    doc.update_dict(pdfcraft_cos::ObjRef::new(6, 0), |font| {
        *font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name(second_face));
        font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    })
    .unwrap();
    doc
}

fn multi_stream_page(contents: &[&str]) -> Document {
    let mut doc = page("", "Helvetica");
    let streams =
        contents.iter().map(|content| Object::Ref(doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.as_bytes().to_vec()))))).collect();
    set_page_contents(&mut doc, Object::Array(streams), None);
    doc
}

fn panel_form(doc: &mut Document, content: &str, matrix: [i64; 6]) {
    let mut dict = Dict::new();
    dict.set(b"Subtype".to_vec(), Object::name("Form"));
    dict.set(b"BBox".to_vec(), Object::Array([0, 0, 600, 800].map(Object::Int).to_vec()));
    dict.set(b"Matrix".to_vec(), Object::Array(matrix.map(Object::Int).to_vec()));
    let form = doc.add(Object::Stream(Stream::from_raw(dict, content.as_bytes().to_vec())));
    let page = pdfcraft_model::pages(doc)[0].obj;
    doc.update_dict(page, |page| {
        let mut resources = page.get(b"Resources").and_then(Object::as_dict).cloned().unwrap();
        let mut xobjects = Dict::new();
        xobjects.set(b"Panel".to_vec(), Object::Ref(form));
        resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
        page.set(b"Resources".to_vec(), Object::Dict(resources));
    })
    .unwrap();
}

#[test]
fn separates_rows_when_background_is_in_a_separate_stream() {
    let doc = multi_stream_page(&["q 60 690 200 25 re f Q", "BT /F1 12 Tf 72 700 Td (Panel) Tj 0 -18 Td (Outside) Tj ET"]);

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Panel", "Outside"]);
}

#[test]
fn separates_rows_when_distinct_panels_are_in_separate_streams() {
    let doc =
        multi_stream_page(&["60 690 200 25 re f BT /F1 12 Tf 72 700 Td (Upper) Tj ET", "60 670 200 20 re f BT /F1 12 Tf 72 682 Td (Lower) Tj ET"]);

    let blocks = text::reading_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Upper", "Lower"]);
}

#[test]
fn joins_rows_when_same_panel_spans_separate_streams() {
    let doc = multi_stream_page(&["60 650 200 65 re f BT /F1 12 Tf 72 700 Td (First) Tj ET", "BT /F1 12 Tf 72 682 Td (Second) Tj ET"]);

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["First\nSecond"]);
}

#[test]
fn separates_rows_when_rectangle_and_fill_are_in_separate_streams() {
    let doc = multi_stream_page(&["q 2 0 0 2 20 10 cm 120 352.5 -100 -12.5 re", "f Q BT /F1 12 Tf 72 700 Td (Panel) Tj 0 -18 Td (Outside) Tj ET"]);

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Panel", "Outside"]);
}

#[test]
fn separates_page_rows_when_transformed_background_is_in_a_form_stream() {
    let mut doc = multi_stream_page(&["/Panel Do", "BT /F1 12 Tf 72 700 Td (Panel) Tj 0 -18 Td (Outside) Tj ET"]);
    panel_form(&mut doc, "20 340 100 12.5 re f", [2, 0, 0, 2, 20, 10]);

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Panel", "Outside"]);
}

#[test]
fn separates_rows_when_distinct_panels_are_in_form_streams() {
    let mut doc = multi_stream_page(&["/Panel Do q 1 0 0 1 0 -18 cm /Panel Do Q"]);
    panel_form(&mut doc, "60 690 200 17 re f BT /F1 12 Tf 72 700 Td (Row) Tj ET", [1, 0, 0, 1, 0, 0]);

    let blocks = text::reading_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Row", "Row"]);
}

#[test]
fn joins_rows_when_page_panel_contains_form_text() {
    let mut doc = multi_stream_page(&["60 650 200 65 re f BT /F1 12 Tf 72 700 Td (First) Tj ET", "/Panel Do"]);
    panel_form(&mut doc, "BT /F1 12 Tf 72 682 Td (Second) Tj ET", [1, 0, 0, 1, 0, 0]);

    let blocks = text::reading_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["First\nSecond"]);
}

#[test]
fn separates_rows_when_size_change_is_large() {
    let doc = page("BT /F1 24 Tf 72 700 Td (Heading) Tj /F1 10 Tf 0 -24 Td (Body) Tj ET", "Helvetica");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Heading", "Body"]);
}

#[test]
fn separates_equal_size_rows_when_families_differ() {
    let doc = page("BT /F1 12 Tf 72 700 Td (Heading) Tj /F2 12 Tf 0 -16 Td (Body) Tj ET", "Courier");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Heading", "Body"]);
}

#[test]
fn separates_equal_style_rows_when_crossing_a_filled_container() {
    let doc = page("q 0.9 g 60 690 200 25 re f Q BT /F1 12 Tf 72 700 Td (Panel) Tj 0 -18 Td (Outside) Tj ET", "Helvetica");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Panel", "Outside"]);
}

#[test]
fn separates_rows_when_containers_are_adjacent() {
    let doc = page("q 60 690 200 25 re f 60 670 200 20 re f Q BT /F1 12 Tf 72 700 Td (Upper) Tj 0 -18 Td (Lower) Tj ET", "Helvetica");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.len(), 2);
}

#[test]
fn joins_multiline_when_inside_the_same_filled_container() {
    let doc = page("q 0.9 g 60 650 200 65 re f Q BT /F1 12 Tf 72 700 Td (First) Tj 0 -18 Td (Second) Tj 0 -18 Td (Third) Tj ET", "Helvetica");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["First\nSecond\nThird"]);
}

#[test]
fn joins_rows_when_a_fill_is_painted_over_one_of_them() {
    // A box painted after the text covers it (a redaction); it is not a panel behind the text.
    let doc = page("BT /F1 12 Tf 72 700 Td (First) Tj 0 -18 Td (Second) Tj ET 0 g 70 696 60 20 re f", "Helvetica");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["First\nSecond"]);
}

#[test]
fn joins_rows_when_their_panel_is_repainted_behind_each_row() {
    let doc = page(
        "0.9 g 60 650 200 65 re f BT /F1 12 Tf 0 g 72 700 Td (First) Tj ET 0.9 g 60 650 200 65 re f BT /F1 12 Tf 0 g 72 682 Td (Second) Tj ET",
        "Helvetica",
    );

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["First\nSecond"]);
}

#[test]
fn joins_rows_when_an_edit_wraps_a_fallback_word_onto_its_own_row() {
    // The bundled Inter face draws the Cyrillic Helvetica lacks; wrapped alone onto a row, the
    // word is still part of the paragraph that was edited.
    let mut doc = page(
        "BT /F1 12 Tf 14.4 TL 72 700 Td (The quick brown fox jumps over the lazy dog) Tj T* (and keeps running through the long field.) Tj ET",
        "Helvetica",
    );
    let edited = "The quick brown fox jumps over the lazy dog \u{049b}\u{0438}\u{0437}\u{0438}\u{043b}\nand keeps running through the long field.";
    text::rewrite_block(&mut doc, 0, 0, Some(edited), &text::BlockStyle::default()).unwrap();

    let blocks = text::text_blocks(&reopen(&doc), 0).unwrap();

    assert_eq!(blocks.len(), 1, "{:?}", blocks.iter().map(|b| &b.editor_text).collect::<Vec<_>>());
    assert!(blocks[0].rows.len() > 2, "the word wraps onto a row of its own: {:?}", blocks[0].editor_text);
}

#[test]
fn joins_plain_multiline_when_no_container_exists() {
    let doc = page("BT /F1 12 Tf 72 700 Td (First) Tj 0 -18 Td (Second) Tj ET", "Helvetica");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["First\nSecond"]);
}

#[test]
fn joins_inline_styles_when_family_and_size_change() {
    let doc = page("BT /F1 12 Tf 72 700 Td (Plain ) Tj /F2 20 Tf (mixed) Tj ET", "Courier-Bold");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["Plain mixed"]);
}

#[test]
fn joins_rows_when_subset_and_weight_names_differ() {
    let mut doc = page("BT /F1 12 Tf 72 700 Td (First) Tj /F2 12 Tf 0 -18 Td (Second) Tj ET", "GHIJKL+Helvetica-Bold");
    doc.update_dict(pdfcraft_cos::ObjRef::new(5, 0), |font| font.set(b"BaseFont".to_vec(), Object::name("ABCDEF+Helvetica"))).unwrap();

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["First\nSecond"]);
}

#[test]
fn joins_rows_when_stacked_weight_suffixes_differ() {
    for (regular, bold) in [("TimesNewRomanPSMT", "TimesNewRomanPS-BoldMT"), ("MinionPro-Regular", "MinionPro-BoldIt")] {
        let mut doc = page("BT /F1 12 Tf 72 700 Td (First) Tj /F2 12 Tf 0 -18 Td (Second) Tj ET", bold);
        doc.update_dict(pdfcraft_cos::ObjRef::new(5, 0), |font| font.set(b"BaseFont".to_vec(), Object::name(regular))).unwrap();

        let blocks = text::text_blocks(&doc, 0).unwrap();

        assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["First\nSecond"], "{regular} / {bold}");
    }
}

#[test]
fn joins_rows_when_a_highlight_hugs_one_word() {
    let doc = page("q 1 1 0 rg 98 697 36 12 re f Q BT /F1 12 Tf 72 700 Td (Plain ) Tj (word) Tj ( rest) Tj 0 -18 Td (Second row) Tj ET", "Helvetica");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["Plain word rest\nSecond row"]);
}

#[test]
fn joins_rows_when_a_padded_highlight_surrounds_one_word() {
    // Highlighters pad the box past the glyphs; it is still a highlight, not a panel.
    for padded in ["98 695 36 16 re", "98 694 36 18 re", "96 693 40 20 re"] {
        let doc = page(
            &format!("q 1 1 0 rg {padded} f Q BT /F1 12 Tf 72 700 Td (Plain ) Tj (word) Tj ( rest) Tj 0 -18 Td (Second row) Tj ET"),
            "Helvetica",
        );

        let blocks = text::text_blocks(&doc, 0).unwrap();

        assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["Plain word rest\nSecond row"], "{padded}");
    }
}

#[test]
fn joins_rows_when_a_padded_highlight_sits_inside_a_one_run_line() {
    // One `Tj` draws the whole line, so the highlight's word is not a run of its own.
    let doc = page("q 1 1 0 rg 98 695 36 16 re f Q BT /F1 12 Tf 72 700 Td (Plain word rest) Tj 0 -18 Td (Second row) Tj ET", "Helvetica");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["Plain word rest\nSecond row"]);
}

#[test]
fn joins_rows_when_only_right_edges_align() {
    let doc = page("BT /F1 12 Tf 72 700 Td (Long first row) Tj ET BT /F1 12 Tf 300 682 Td (Short) Tj ET", "Helvetica");
    let lines = text::text_lines(&doc, 0).unwrap();
    let x = lines[0].rect[2] - (lines[1].rect[2] - lines[1].rect[0]);
    let doc = page(&format!("BT /F1 12 Tf 72 700 Td (Long first row) Tj ET BT /F1 12 Tf {x} 682 Td (Short) Tj ET"), "Helvetica");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["Long first row\nShort"]);
}

#[test]
fn keeps_outside_text_when_editing_a_container_paragraph() {
    let mut doc = page("q 0.9 g 60 690 200 25 re f Q BT /F1 12 Tf 72 700 Td (Panel) Tj 0 -18 Td (Outside) Tj ET", "Helvetica");
    let outside = text::text_lines(&doc, 0).unwrap()[1].clone();

    text::replace_block(&mut doc, 0, 0, "Edited").unwrap();

    let reopened = reopen(&doc);
    let lines = text::text_lines(&reopened, 0).unwrap();
    let kept = lines.iter().find(|line| line.text == "Outside").expect("outside paragraph survives");
    assert_eq!((kept.rect, kept.size, &kept.base_font), (outside.rect, outside.size, &outside.base_font));
}

#[test]
fn keeps_neighboring_panel_when_editing_an_isolated_panel() {
    let mut doc =
        multi_stream_page(&["60 690 200 25 re f BT /F1 12 Tf 72 700 Td (Upper) Tj ET", "60 670 200 20 re f BT /F1 12 Tf 72 682 Td (Lower) Tj ET"]);
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2);
    let neighbor = text::text_lines(&doc, 0).unwrap()[1].clone();

    text::replace_block(&mut doc, 0, 0, "Edited").unwrap();

    let reopened = reopen(&doc);
    let blocks = text::text_blocks(&reopened, 0).unwrap();
    assert_eq!(blocks.iter().map(|block| block.text.as_str()).collect::<Vec<_>>(), ["Edited", "Lower"]);
    let lines = text::text_lines(&reopened, 0).unwrap();
    let kept = lines.iter().find(|line| line.text == "Lower").expect("neighboring panel survives");
    assert_eq!(
        (kept.rect, kept.baseline(), kept.size, &kept.base_font, kept.color),
        (neighbor.rect, neighbor.baseline(), neighbor.size, &neighbor.base_font, neighbor.color)
    );
}

#[test]
fn separates_rows_when_container_has_a_transformed_negative_rectangle() {
    let doc = page("q 2 0 0 2 20 10 cm 120 352.5 -100 -12.5 re B Q BT /F1 12 Tf 72 700 Td (Panel) Tj 0 -18 Td (Outside) Tj ET", "Helvetica");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Panel", "Outside"]);
}

#[test]
fn joins_rows_when_rectangle_is_only_stroked_or_clipped() {
    for paint in ["S", "W n", "W* n"] {
        let doc = page(&format!("q 60 690 200 25 re {paint} Q BT /F1 12 Tf 72 700 Td (First) Tj 0 -18 Td (Second) Tj ET"), "Helvetica");

        let blocks = text::text_blocks(&doc, 0).unwrap();

        assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["First\nSecond"], "{paint}");
    }
}

#[test]
fn joins_multiline_with_inline_mixed_families() {
    let doc = page("BT /F1 12 Tf 72 700 Td (Ordinary text ) Tj /F2 12 Tf (code) Tj /F1 12 Tf 0 -18 Td (Following row) Tj ET", "Courier-Bold");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), ["Ordinary text code\nFollowing row"]);
}

#[test]
fn separates_inline_labels_when_a_container_edge_is_between_them() {
    let doc = page("q 60 690 50 25 re f Q BT /F1 12 Tf 72 700 Td (Panel) Tj 1 0 0 1 112 700 Tm (Outside) Tj ET", "Helvetica");

    let blocks = text::text_blocks(&doc, 0).unwrap();

    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Panel", "Outside"]);
}
