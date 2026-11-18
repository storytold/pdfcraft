use super::*;

#[test]
fn turkish_appearance_preserves_glyph_mapping_and_unknown_data_on_restyle() {
    let mut doc = fixture();
    let shape = Shape::TextBox { rect: [20.0, 200.0, 500.0, 300.0], font_size: 16.0 };
    let mut annotation = new(0, shape);
    annotation.contents = "İstanbul Iğdır Şişli ç Ç ğ Ğ ı I i İ ö Ö ş Ş ü Ü â Â î Î û Û".into();
    let index = add_annotation(&mut doc, &annotation, &meta("tr")).unwrap();
    let r = annots(&doc, page_refs(&doc).unwrap()[0])[index].as_ref().unwrap();
    doc.update_dict(r, |d| d.set(b"UnknownKey".to_vec(), PdfString::text("koru"))).unwrap();
    let da = annot_dict(&doc, r).get(b"DA").cloned();
    let font_resources = |doc: &Document| {
        let d = annot_dict(doc, r);
        let ap = doc.dict(d.get(b"AP").unwrap()).unwrap();
        let normal = doc.resolve(ap.get(b"N").unwrap());
        let Object::Stream(s) = normal.as_ref() else { panic!() };
        let resources = doc.dict(s.dict.get(b"Resources").unwrap()).unwrap();
        doc.dict(resources.get(b"Font").unwrap()).unwrap()
    };
    let unicode = |doc: &Document| {
        font_resources(doc)
            .iter()
            .find_map(|(_, o)| {
                let font = doc.dict(o)?;
                if font.name(b"Subtype") != Some(b"Type3") {
                    return None;
                }
                let stream = doc.resolve(font.get(b"ToUnicode")?);
                let Object::Stream(stream) = stream.as_ref() else { return None };
                stream.decoded().ok()
            })
            .unwrap()
    };
    let before = unicode(&doc);
    assert!(pdfcraft_fonts::undrawn_mark(&annot_dict(&doc, r)).is_none());
    set_font_size(&mut doc, 0, index, 18.0, &meta("tr-edit")).unwrap();
    assert_eq!(before, unicode(&doc), "restyling preserves the embedded glyph mapping");
    assert_eq!(text(&annot_dict(&doc, r), b"UnknownKey"), "koru");
    assert_eq!(text(&annot_dict(&doc, r), b"Contents"), annotation.contents);
    assert_ne!(annot_dict(&doc, r).get(b"DA"), da.as_ref(), "the requested size changed");
    let doc = reopen(&doc);
    assert!(pdfcraft_fonts::undrawn_mark(&annot_dict(&doc, r)).is_none());
}

#[test]
fn ascii_textbox_keeps_helvetica_without_embedding_a_program() {
    let mut doc = fixture();
    let before = doc.scan_objects().filter(|(_, o)| o.as_dict().is_some_and(|d| d.contains(b"FontFile2"))).count();
    let i = add_annotation(&mut doc, &new(0, Shape::TextBox { rect: [20.0, 200.0, 500.0, 300.0], font_size: 16.0 }), &meta("ascii")).unwrap();
    assert!(ap_content(&doc, &list(&doc, 0)[i]).contains("/Helv 16 Tf"));
    assert_eq!(doc.scan_objects().filter(|(_, o)| o.as_dict().is_some_and(|d| d.contains(b"FontFile2"))).count(), before);
}

#[test]
fn nfd_textboxes_and_callouts_keep_logical_text_when_drawn_and_restyled() {
    let mut doc = fixture();
    let original = "I\u{307}stanbul C\u{327}ag\u{306}r\u{131} s\u{327}o\u{308}u\u{308}";
    for shape in [
        Shape::TextBox { rect: [20.0, 200.0, 500.0, 300.0], font_size: 16.0 },
        Shape::Callout {
            rect: [50.0, 350.0, 500.0, 450.0],
            knee: [30.0, 400.0],
            point: [10.0, 300.0],
            font_size: 16.0,
            ending: LineEnding::OpenArrow,
        },
    ] {
        let mut annotation = new(0, shape);
        annotation.contents = original.into();
        let index = add_annotation(&mut doc, &annotation, &meta("nfd")).unwrap();
        let r = annots(&doc, page_refs(&doc).unwrap()[0])[index].as_ref().unwrap();
        assert!(pdfcraft_fonts::undrawn_mark(&annot_dict(&doc, r)).is_none(), "NFD must be drawn");
        assert!(ap_content(&doc, &annot_dict(&doc, r)).contains("/ActualText"));
        set_font_size(&mut doc, 0, index, 18.0, &meta("nfd-size")).unwrap();
        assert_eq!(text(&annot_dict(&doc, r), b"Contents"), original);
    }
    let doc = reopen(&doc);
    for annotation in list(&doc, 0).iter().filter(|d| d.name(b"Subtype") == Some(b"FreeText")) {
        assert_eq!(text(annotation, b"Contents"), original);
        assert!(pdfcraft_fonts::undrawn_mark(annotation).is_none());
    }
    let maps = doc
        .scan_objects()
        .filter_map(|(_, object)| {
            let Object::Stream(stream) = object.as_ref() else { return None };
            String::from_utf8(stream.decoded().ok()?).ok()
        })
        .collect::<Vec<_>>();
    assert!(maps.iter().any(|map| map.contains("<00490307>")), "ToUnicode retains the decomposed I + dot");
}

#[test]
fn nfd_freetext_measurement_matches_nfc_and_wrapping_never_orphans_marks() {
    let nfd = "G\u{306}G\u{306}G\u{306}";
    assert!(pdfcraft_fonts::latin_text::required(nfd));
    assert!((appearance::text_width(nfd, 16.0) - appearance::text_width("ĞĞĞ", 16.0)).abs() < 0.01);
    let lines = appearance::wrap(nfd, 16.0, appearance::text_width("Ğ", 16.0) * 0.5);
    assert_eq!(lines, ["G\u{306}", "G\u{306}", "G\u{306}"]);
}

#[test]
fn nfd_freetext_keeps_the_nfc_baseline_unless_actual_mark_ink_would_clip() {
    let mut doc = fixture();
    let mut baselines = Vec::new();
    for value in ["Ğ".to_string(), "G\u{306}".to_string(), format!("a{}", "\u{301}".repeat(8))] {
        let mut annotation = new(0, Shape::TextBox { rect: [20.0, 200.0, 500.0, 400.0], font_size: 16.0 });
        annotation.contents = value.clone();
        let index = add_annotation(&mut doc, &annotation, &meta("nfd-bounds")).unwrap();
        let content = ap_content(&doc, &list(&doc, 0)[index]);
        let baseline = content
            .lines()
            .find_map(|line| line.split_once(" Tm ").and_then(|(matrix, _)| matrix.split_whitespace().last()))
            .unwrap()
            .parse::<f64>()
            .unwrap();
        baselines.push(baseline);
        if pdfcraft_fonts::latin_text::required(&value) {
            let ink = pdfcraft_fonts::latin_text::plan(&value, false).unwrap().bounds;
            assert!(baseline + ink[3] * 16.0 <= 400.001, "stacked mark ink must stay in the actual clip");
        }
    }
    assert_eq!(baselines[0], baselines[1], "canonical NFC/NFD must retain the same ordinary baseline");
}
