use super::*;

const TURKISH: &str = "ç Ç ğ Ğ ı I i İ ö Ö ş Ş ü Ü â Â î Î û Û\nİstanbul Iğdır Çağrı Şişli Öğrenci GÜNEŞ ışık İŞ SÖZLEŞMESİ";

fn extracted(doc: &Document) -> String {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(bytes), Default::default());
    let result = renderer.render(pdfcraft_render::RenderRequest { page: 2, kind: pdfcraft_render::RequestKind::Text, ..Default::default() });
    assert!(result.error.is_none(), "{:?}", result.error);
    result.text.unwrap().plain_text()
}

#[test]
fn turkish_added_text_keeps_unicode_and_real_fallback_after_save_reopen() {
    let mut doc = fixture();
    let text = AddedText { rect: [20.0, 500.0, 580.0, 780.0], text: TURKISH.into(), size: 14.0, ..AddedText::default() };
    assert_eq!(first_undrawable(&text), None);
    add_content(&mut doc, 2, &Content::Text(text.clone())).unwrap();
    assert_eq!(extracted(&doc).trim(), TURKISH);
    let doc = reopen(&doc);
    assert_eq!(extracted(&doc).trim(), TURKISH);
    let Content::Text(saved) = &list_added(&doc)[0].content else { panic!() };
    assert_eq!(saved.text, TURKISH);
    let page = &pdfcraft_model::pages(&doc)[2];
    let resources = doc.dict(page.dict.get(b"Resources").unwrap()).unwrap();
    let fonts = doc.dict(resources.get(b"Font").unwrap()).unwrap();
    let embedded: Vec<_> = fonts.iter().filter_map(|(_, object)| doc.dict(object).filter(|f| f.name(b"Subtype") == Some(b"Type0"))).collect();
    assert_eq!(embedded.len(), 1);
    assert_eq!(pdfcraft_fonts::pdf::embedded_sfnt(&doc, &embedded[0]).unwrap(), pdfcraft_fonts::INTER_REGULAR);
    // The fallback only fills gaps: existing Latin text still uses the chosen standard font.
    let drawn = streams(&doc, 2).join("\n");
    assert!(drawn.contains("/PCFHelvetica 14 Tf"), "{drawn}");
    assert!(drawn.contains("/PCFull"), "{drawn}");
}

#[test]
fn turkish_fallback_keeps_foreign_font_names_and_reuses_its_program_on_retyping() {
    let mut doc = fixture();
    let page = &pdfcraft_model::pages(&doc)[2];
    let mut foreign = Dict::new();
    foreign.set(b"Type".to_vec(), Object::name("Font"));
    foreign.set(b"Subtype".to_vec(), Object::name("Type1"));
    foreign.set(b"BaseFont".to_vec(), Object::name("Courier"));
    let object = Object::Ref(doc.add(Object::Dict(foreign)));
    let mut fonts = Dict::new();
    fonts.set(b"PCFull0".to_vec(), object.clone());
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(page.obj, |dict| dict.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    let initial = AddedText { rect: [20.0, 500.0, 580.0, 780.0], text: "Çağrı Iğdır".into(), ..AddedText::default() };
    add_content(&mut doc, 2, &Content::Text(initial.clone())).unwrap();
    let before = pdfcraft_model::pages(&doc)[2].dict.clone();
    let resources = doc.dict(before.get(b"Resources").unwrap()).unwrap();
    let fonts = doc.dict(resources.get(b"Font").unwrap()).unwrap();
    assert_eq!(fonts.get(b"PCFull0"), Some(&object));
    let fallback = fonts.get(b"PCFull1").unwrap().clone();
    let mut doc = reopen(&doc);
    update_content(&mut doc, 2, 0, &Content::Text(AddedText { text: "İstanbul Şişli".into(), ..initial })).unwrap();
    let page = &pdfcraft_model::pages(&doc)[2];
    let resources = doc.dict(page.dict.get(b"Resources").unwrap()).unwrap();
    let fonts = doc.dict(resources.get(b"Font").unwrap()).unwrap();
    assert_eq!(fonts.get(b"PCFull0"), Some(&object));
    assert_eq!(fonts.get(b"PCFull1"), Some(&fallback));
    assert!(!fonts.contains(b"PCFull2"), "retyping reuses the full face");
    assert_eq!(extracted(&reopen(&doc)).trim(), "İstanbul Şişli");
}

#[test]
fn mixed_latin_layout_uses_real_advances_and_preserves_standard_metrics() {
    for family in [Family::Helvetica, Family::Times, Family::Courier] {
        for bold in [false, true] {
            for italic in [false, true] {
                let text = AddedText { text: "ağİşZ".into(), family, bold, italic, size: 14.0, char_spacing: 0.7, ..AddedText::default() };
                let standard = family.width("aZ", text.size, bold, italic);
                let face = pdfcraft_fonts::document_latin_fonts(bold)[0];
                let missing: f64 = "ğİş".chars().map(|c| pdfcraft_fonts::craft_glyph(face, c).unwrap().width * text.size).sum();
                let width = crate::added::text_width(&text);
                assert!((width - standard - missing - 5.0 * text.char_spacing).abs() < 1e-8, "{family:?}, {bold}, {italic}: {width}");
                let plain = AddedText { text: "ASCII Café".into(), ..text };
                assert_eq!(crate::added::text_width(&plain), family.width(&plain.text, plain.size, bold, italic) + 10.0 * plain.char_spacing);
            }
        }
    }
}

#[test]
fn unsupported_combining_marks_are_refused_before_mutating_the_document() {
    // Only bounded base/mark clusters covered by the shaped Latin route are accepted.
    for (input, mark) in [("x\u{1AB0}", '\u{1AB0}'), ("I\u{20DD}", '\u{20DD}')] {
        let mut doc = fixture();
        let before = doc.object_numbers();
        let text = AddedText { rect: [20.0, 500.0, 580.0, 780.0], text: input.into(), ..AddedText::default() };
        assert_eq!(first_undrawable(&text), Some(mark));
        assert!(add_content(&mut doc, 2, &Content::Text(text)).is_err(), "{input}");
        assert!(!doc.is_modified(), "a refused decomposed sequence must leave the document unchanged");
        assert_eq!(doc.object_numbers(), before, "no fallback objects were allocated");
        assert!(list_added(&doc).is_empty());
    }
}

const NFD_TURKISH: &str =
    "c\u{0327} C\u{0327} g\u{0306} G\u{0306} I\u{0307} o\u{0308} O\u{0308} s\u{0327} S\u{0327} u\u{0308} U\u{0308} a\u{0302} i\u{0302} u\u{0302}";

#[test]
fn nfd_added_text_is_shaped_and_keeps_original_unicode_after_retyping() {
    let mut doc = fixture();
    let initial = AddedText { rect: [20.0, 500.0, 580.0, 780.0], text: NFD_TURKISH.into(), size: 16.0, ..AddedText::default() };
    assert_eq!(first_undrawable(&initial), None);
    add_content(&mut doc, 2, &Content::Text(initial.clone())).unwrap();
    assert_eq!(extracted(&doc).trim(), NFD_TURKISH);
    let mut doc = reopen(&doc);
    let Content::Text(saved) = &list_added(&doc)[0].content else { panic!() };
    assert_eq!(saved.text, NFD_TURKISH);
    let next = "I\u{0307}stanbul C\u{0327}ag\u{0306}rı S\u{0327}is\u{0327}li";
    update_content(&mut doc, 2, 0, &Content::Text(AddedText { text: next.into(), ..initial })).unwrap();
    let doc = reopen(&doc);
    assert_eq!(extracted(&doc).trim(), next);
    let Content::Text(saved) = &list_added(&doc)[0].content else { panic!() };
    assert_eq!(saved.text, next);
}

#[test]
fn nfd_added_layout_applies_character_spacing_per_cluster_and_preserves_standard_runs() {
    for family in [Family::Helvetica, Family::Times, Family::Courier] {
        for bold in [false, true] {
            for italic in [false, true] {
                let plain = AddedText {
                    rect: [20.0, 500.0, 580.0, 780.0],
                    text: "Ag\u{0306}I\u{0307}s\u{0327}u\u{0308}Z".into(),
                    family,
                    bold,
                    italic,
                    size: 14.0,
                    ..Default::default()
                };
                let spaced = AddedText { char_spacing: 1.25, ..plain.clone() };
                let a = crate::added::text_width(&plain);
                let b = crate::added::text_width(&spaced);
                assert!((b - a - 6.0 * spaced.char_spacing).abs() < 1e-8, "{family:?} {bold} {italic}: {a} {b}");
                let mut doc = fixture();
                add_content(&mut doc, 2, &Content::Text(spaced)).unwrap();
                assert_eq!(extracted(&reopen(&doc)).trim(), plain.text);
                let drawn = streams(&doc, 2).join("\n");
                let standard = family.base_font(bold, italic).replace('-', "");
                assert!(drawn.contains(&format!("/PCF{standard} 14 Tf")), "{drawn}");
                assert!(drawn.contains("ActualText"), "NFD requires an original logical cluster: {drawn}");
                if italic {
                    assert!(drawn.contains("0.213") || drawn.contains("0.212556"), "{drawn}");
                }
            }
        }
    }
}

#[test]
fn nfd_headers_and_watermarks_keep_logical_text_in_all_standard_families() {
    for family in [Family::Helvetica, Family::Times, Family::Courier] {
        let mut doc = fixture();
        let hf = HeaderFooter {
            text: [String::new(), NFD_TURKISH.into(), String::new(), String::new(), String::new(), String::new()],
            font: family,
            underline: true,
            ..Default::default()
        };
        assert_eq!(hf.first_undrawable(), None);
        add_header_footer(&mut doc, &[2], &hf, false, &cx()).unwrap();
        assert_eq!(extracted(&reopen(&doc)).trim(), NFD_TURKISH);
    }
    let mut doc = fixture();
    let wm = Watermark { text: NFD_TURKISH.into(), font_size: 16.0, rotation: 0.0, ..Default::default() };
    assert_eq!(wm.first_undrawable(), None);
    add_watermark(&mut doc, &[2], &wm, false).unwrap();
    assert_eq!(extracted(&reopen(&doc)).trim(), NFD_TURKISH);
}

#[test]
fn unsupported_mark_after_a_valid_nfd_line_leaves_no_partial_font_objects() {
    let mut doc = fixture();
    let before = doc.object_numbers();
    let text = AddedText { text: "g\u{0306}\nx\u{1AB0}".into(), rect: [20.0, 500.0, 580.0, 780.0], ..Default::default() };
    assert_eq!(first_undrawable(&text), Some('\u{1AB0}'));
    assert!(add_content(&mut doc, 2, &Content::Text(text)).is_err());
    assert_eq!(doc.object_numbers(), before);
    assert!(!doc.is_modified());
    assert!(list_added(&doc).is_empty());
}

#[test]
fn nfd_text_budget_and_orphan_marks_are_atomic_errors() {
    for input in [format!("g\u{0306}{}", "a".repeat(262_144)), "\u{0306}g".into(), format!("g{}", "\u{0306}".repeat(33))] {
        let mut doc = fixture();
        let before = doc.object_numbers();
        let text = AddedText { text: input, rect: [20.0, 500.0, 580.0, 780.0], ..Default::default() };
        assert!(add_content(&mut doc, 2, &Content::Text(text)).is_err());
        assert_eq!(doc.object_numbers(), before);
        assert!(!doc.is_modified());
    }
}

#[test]
fn nfd_incomposable_marks_are_positioned_with_their_base_and_cluster_spacing() {
    let typed = "q\u{0301}x\u{0323}n\u{0303}\u{0304}";
    let mut doc = fixture();
    let text =
        AddedText { text: typed.into(), rect: [20.0, 500.0, 580.0, 780.0], size: 20.0, char_spacing: 1.25, italic: true, ..Default::default() };
    add_content(&mut doc, 2, &Content::Text(text.clone())).unwrap();
    assert_eq!(extracted(&reopen(&doc)).trim(), typed);
    let ops = pdfcraft_content::parse(streams(&doc, 2).join("\n").as_bytes()).ops;
    let positions: Vec<_> = ops.iter().filter(|op| op.is("Tm")).map(|op| op.nums::<6>().unwrap()).collect();
    let (mut pen, mut index) = (20.0, 0usize);
    let baseline = 780.0 - 0.95 * text.size;
    let mut raised = false;
    for cluster in ["q\u{0301}", "x\u{0323}", "n\u{0303}\u{0304}"] {
        let shaped = pdfcraft_fonts::source_shaping::shape(pdfcraft_fonts::INTER_REGULAR, cluster).unwrap();
        for glyph in shaped.glyphs {
            let matrix = positions[index];
            let expected_x = pen + (glyph.position[0] + 0.2125565616700221 * glyph.position[1]) * text.size;
            let expected_y = baseline + glyph.position[1] * text.size;
            assert!((matrix[4] - expected_x).abs() < 1e-6 && (matrix[5] - expected_y).abs() < 1e-6, "{glyph:?}: {matrix:?}");
            raised |= glyph.position[1] != 0.0;
            index += 1;
        }
        pen += shaped.advance * text.size + text.char_spacing;
    }
    assert_eq!(index, positions.len());
    assert!(raised, "a nonzero mark offset exercises the actual positioned writer");
    assert!((crate::added::text_width(&text) - (pen - 20.0)).abs() < 1e-8);
}

#[test]
fn mixed_thai_nfd_wrap_keeps_the_latin_base_and_mark_in_one_atom() {
    if pdfcraft_fonts::document_thai_font().is_none() {
        return;
    }
    // At 20 pt width the previous scalar loop put g on one row and its breve on
    // the next: the large spacing makes that separation observable and deterministic.
    let text = AddedText { text: "กg\u{0306}ก".into(), rect: [20.0, 500.0, 40.0, 780.0], size: 20.0, char_spacing: 12.0, ..Default::default() };
    assert_eq!(first_undrawable(&text), None);
    let lines = crate::added::lines(&text);
    assert_eq!(lines.concat(), text.text);
    assert!(lines.iter().any(|line| line == "g\u{0306}"), "{lines:?}");
    assert!(!lines.iter().any(|line| line.starts_with('\u{0306}')), "{lines:?}");
    let mut doc = fixture();
    add_content(&mut doc, 2, &Content::Text(text)).unwrap();
    assert_eq!(list_added(&reopen(&doc))[0].content, list_added(&doc)[0].content);
}
