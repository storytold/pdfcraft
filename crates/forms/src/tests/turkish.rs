use super::*;

#[test]
fn turkish_fields_use_fallback_without_replacing_source_font_or_da() {
    let mut doc = fixture();
    let original = doc.get(pdfcraft_cos::ObjRef::new(5, 0));
    let form = pdfcraft_cos::ObjRef::new(4, 0);
    let da = doc.get(form).as_dict().unwrap().get(b"DA").cloned();
    for (name, value) in [("name", "İstanbul Iğdır Şişli"), ("notes", "ç Ç ğ Ğ ı I i İ\nö Ö ş Ş ü Ü â Â î Î û Û"), ("zip", "İğışü")]
    {
        set_value(&mut doc, name, &FieldValue::Text(value.into())).unwrap();
    }
    assert!(undrawn_fields(&doc).is_empty());
    assert_eq!(doc.get(pdfcraft_cos::ObjRef::new(5, 0)), original);
    let af = doc.get(form);
    assert_eq!(af.as_dict().unwrap().get(b"DA"), da.as_ref());
    let field = field(&fields(&doc), "name").widgets[0].clone();
    assert!(ap(&doc, &field).contains("/PCLa"), "a Latin fallback glyph must be drawn");
    let doc = reopen(&doc);
    assert!(undrawn_fields(&doc).is_empty());
    assert_eq!(fields(&doc).iter().find(|f| f.name == "zip").unwrap().value, ["İğışü"]);
}

#[test]
fn non_scrolling_turkish_field_fit_uses_the_appearance_font_metrics() {
    let mut doc = fixture();
    let r = pdfcraft_cos::ObjRef::new(10, 0);
    doc.update_dict(r, |d| {
        d.set(b"DA".to_vec(), PdfString::literal(b"/Helv 12 Tf 0 g".to_vec()));
        let mut bs = Dict::new();
        bs.set(b"W".to_vec(), Object::Real(2.0));
        d.set(b"BS".to_vec(), Object::Dict(bs));
        d.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 40.0, 30.0].into_iter().map(Object::Real).collect()));
    })
    .unwrap();
    let all = fields(&doc);
    let f = all.iter().find(|f| f.name == "name").unwrap();
    let box_ = appearance::text_box(&doc, f, &f.widgets[0]);
    for text in ["İİİİ", "ıııııııııııı", "ĞĞĞĞ", "Iğdır"] {
        assert_eq!(
            box_.fits(text),
            pdfcraft_fonts::shaped_text::fallback_width(text, 12.0, false, true, pdfcraft_fonts::helvetica_width) <= 32.0 + 1e-6,
            "{text}"
        );
    }
}

#[test]
fn automatic_turkish_field_keeps_the_embedded_ascent_and_descent_inside_its_box() {
    let mut doc = fixture();
    let r = pdfcraft_cos::ObjRef::new(10, 0);
    doc.update_dict(r, |d| {
        d.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 1100.0, 100.0].into_iter().map(Object::Real).collect()));
    })
    .unwrap();
    set_value(&mut doc, "name", &FieldValue::Text("Ğ".into())).unwrap();
    let widget = doc.get(r);
    let ap = doc.dict(widget.as_dict().unwrap().get(b"AP").unwrap()).unwrap();
    let normal = doc.resolve(ap.get(b"N").unwrap());
    let Object::Stream(stream) = normal.as_ref() else { panic!() };
    let resources = doc.dict(stream.dict.get(b"Resources").unwrap()).unwrap();
    let fonts = doc.dict(resources.get(b"Font").unwrap()).unwrap();
    let metrics = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, &doc.dict(fonts.iter().next().unwrap().1).unwrap());
    let decoded = stream.decoded().unwrap();
    let body = String::from_utf8_lossy(&decoded);
    let size: f64 = body.lines().find(|line| line.ends_with(" Tf")).unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    let baseline: f64 =
        body.lines().find_map(|line| line.split_once(" Tm ").and_then(|(matrix, _)| matrix.split_whitespace().last())).unwrap().parse().unwrap();
    assert!(
        baseline + metrics.ascent * size <= 99.0,
        "the capital breve must not be clipped: size {size}, baseline {baseline}, ascent {}",
        metrics.ascent
    );
    assert!(baseline + metrics.descent * size >= 1.0, "descenders must stay inside the lower border");
}

#[test]
fn nfd_fields_preserve_source_values_and_draw_all_visible_clusters() {
    let mut doc = fixture();
    let source_font = doc.get(pdfcraft_cos::ObjRef::new(5, 0));
    let values = [
        ("name", "I\u{307}stanbul G\u{306}u\u{308}nes\u{327}"),
        ("notes", "C\u{327}ag\u{306}\ns\u{327}o\u{308}u\u{308}"),
        ("zip", "G\u{306}I\u{307}"),
    ];
    for (name, value) in values {
        set_value(&mut doc, name, &FieldValue::Text(value.into())).unwrap();
    }
    assert!(undrawn_fields(&doc).is_empty(), "supported NFD must have a drawable appearance");
    assert_eq!(doc.get(pdfcraft_cos::ObjRef::new(5, 0)), source_font);
    let doc = reopen(&doc);
    let all = fields(&doc);
    for (name, value) in values {
        let field = field(&all, name);
        assert_eq!(field.value, [value], "stored /V must preserve NFD exactly");
        let content = ap(&doc, &field.widgets[0]);
        assert!(content.contains("/ActualText"), "{name}: shaped clusters retain logical text");
    }
    let maps = doc
        .scan_objects()
        .filter_map(|(_, object)| {
            let Object::Stream(stream) = object.as_ref() else { return None };
            String::from_utf8(stream.decoded().ok()?).ok()
        })
        .collect::<Vec<_>>();
    assert!(maps.iter().any(|map| map.contains("<00490307>")), "I + dot must remain decomposed in ToUnicode");
    assert!(maps.iter().any(|map| map.contains("<00470306>")), "G + breve must remain decomposed in ToUnicode");
}

#[test]
fn nfd_choice_display_and_password_do_not_rewrite_export_or_secret_values() {
    let mut doc = fixture();
    let label = "I\u{307}zmir";
    for object in [23, 24] {
        doc.update_dict(pdfcraft_cos::ObjRef::new(object, 0), |d| {
            d.set(
                b"Opt".to_vec(),
                Object::Array(vec![Object::Array(vec![Object::String(PdfString::text("tr")), Object::String(PdfString::text(label))])]),
            );
        })
        .unwrap();
    }
    for name in ["country", "toppings"] {
        set_value(&mut doc, name, &FieldValue::Choice(vec!["tr".into()])).unwrap();
    }
    let secret = "G\u{306}I\u{307}";
    set_value(&mut doc, "pin", &FieldValue::Text(secret.into())).unwrap();
    assert!(undrawn_fields(&doc).is_empty());
    let doc = reopen(&doc);
    let all = fields(&doc);
    for name in ["country", "toppings"] {
        let field = field(&all, name);
        assert_eq!(field.value, ["tr"]);
        assert!(ap(&doc, &field.widgets[0]).contains("/ActualText"));
    }
    let pin = field(&all, "pin");
    assert_eq!(pin.value, [secret]);
    let drawn = ap(&doc, &pin.widgets[0]);
    assert!(drawn.contains("(****) Tj"), "password retains the existing scalar mask length");
    assert!(!drawn.contains("/ActualText"), "password appearance must not expose secret text");
}

#[test]
fn nfd_second_line_missing_glyph_does_not_add_partial_fallback_objects() {
    let doc = fixture();
    let all = fields(&doc);
    let field = field(&all, "notes");
    let before = doc.object_numbers().len();
    let form = doc.get(pdfcraft_cos::ObjRef::new(4, 0));
    let (_, missing) = appearance::field_appearance_drawn(&doc, field, &field.widgets[0], &["G\u{306}\n🦀".into()], false);
    assert!(missing.is_some());
    assert_eq!(doc.object_numbers().len(), before);
    assert_eq!(doc.get(pdfcraft_cos::ObjRef::new(4, 0)), form);
}

#[test]
fn nfd_comb_keeps_clusters_in_cells_and_max_len_counts_original_scalars() {
    let mut doc = fixture();
    let value = "G\u{306}I\u{307}";
    set_value(&mut doc, "zip", &FieldValue::Text(value.into())).unwrap();
    let all = fields(&doc);
    let zip = field(&all, "zip");
    let content = ap(&doc, &zip.widgets[0]);
    assert_eq!(content.matches("/ActualText").count(), 2, "each occupied cell owns one complete logical cluster");
    assert!(set_value(&mut doc, "zip", &FieldValue::Text("G\u{306}I\u{307}u\u{308}".into())).is_err());
    assert_eq!(field(&fields(&doc), "zip").value, [value]);
}

#[test]
fn nfd_automatic_field_fits_actual_stacked_mark_ink() {
    let mut doc = fixture();
    let r = pdfcraft_cos::ObjRef::new(10, 0);
    doc.update_dict(r, |d| d.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 1100.0, 100.0].into_iter().map(Object::Real).collect()))).unwrap();
    let value = format!("a{}", "\u{301}".repeat(8));
    set_value(&mut doc, "name", &FieldValue::Text(value.clone())).unwrap();
    let all = fields(&doc);
    let content = ap(&doc, &field(&all, "name").widgets[0]);
    let size: f64 = content.lines().find(|line| line.ends_with(" Tf")).unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    let baseline: f64 =
        content.lines().find_map(|line| line.split_once(" Tm ").and_then(|(matrix, _)| matrix.split_whitespace().last())).unwrap().parse().unwrap();
    let bounds = pdfcraft_fonts::latin_text::plan(&value, false).unwrap().bounds;
    assert!(baseline + bounds[3] * size <= 99.001, "all positioned upper marks must stay in the widget clip");
    assert!(baseline + bounds[1] * size >= 0.999, "lower ink must stay in the widget clip");
}
