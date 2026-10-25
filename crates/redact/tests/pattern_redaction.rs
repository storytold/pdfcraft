//! Saved-output confidentiality regressions for #3647, using only synthetic PDFs.

use std::sync::Arc;

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_redact::{PatternProblem, RedactError, apply, marks};
use pdfcraft_testkit::{Pdf, stream_obj};

const SECRET: &[u8] = b"CELLSECRET";
const CELL: &[u8] = b"BT /F1 20 Tf 40 120 Td (CELLSECRET) Tj ET\n";
const PATTERN: &str =
    "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 500 300] /XStep 500 /YStep 300 /Resources << /Font << /F1 4 0 R >> >>";
const AREA: [f64; 4] = [30.0, 105.0, 215.0, 150.0];

fn fixture(cell: &[u8], compressed: bool, partial: bool, inherited: bool) -> Document {
    let resources = "/Resources << /Font << /F1 4 0 R >> /Pattern << /P0 6 0 R >> >>";
    let parent_res = if inherited { resources } else { "" };
    let own_res = if inherited { "" } else { resources };
    let width = if partial { 400 } else { 170 };
    let content = format!("BT /F1 20 Tf 40 250 Td (PUBLIC) Tj ET\n/Pattern cs /P0 scn 35 110 {width} 35 re f\n");
    let bytes = Pdf::new()
        .objs(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            format!("<< /Type /Pages /Kids [3 0 R] /Count 1 {parent_res} >>").into_bytes(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R {own_res} >>").into_bytes(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
            stream_obj("", content.as_bytes()),
            stream_obj(PATTERN, cell),
        ])
        .build();
    let mut doc = Document::open(Arc::new(bytes)).unwrap();
    if compressed {
        let pattern = doc.get(ObjRef::new(6, 0));
        let Object::Stream(pattern) = &*pattern else { panic!("fixture pattern must be a stream") };
        doc.set(ObjRef::new(6, 0), Object::Stream(Stream::flate(pattern.dict.clone(), cell)));
    }
    doc
}

fn mark(doc: &mut Document, page: usize) {
    let shape = Shape::Redact { quads: vec![rect_quad(AREA)], overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(doc, &NewAnnotation { page, shape, style, contents: String::new(), author: "Synthetic test".into() }, &Meta::default()).unwrap();
}

fn saved(doc: &Document) -> Vec<u8> {
    write_full(doc, &SaveOptions::default()).unwrap()
}

fn has_decoded_secret(bytes: Vec<u8>) -> bool {
    let doc = Document::open(Arc::new(bytes)).unwrap();
    doc.object_numbers().into_iter().any(|num| {
        let object = doc.try_get(num).unwrap();
        matches!(&*object, Object::Stream(s) if s.decoded().unwrap().windows(SECRET.len()).any(|w| w == SECRET))
    })
}

/// Success must remove the payload from the saved object graph. Refusal must be atomic even
/// for the direct library caller, not just the engine's working clone.
fn assert_confidential_or_refused(mut doc: Document, expect_refusal: bool) {
    mark(&mut doc, 0);
    let before = saved(&doc);
    assert!(has_decoded_secret(before.clone()), "the fixture must carry the secret");
    match apply(&mut doc, None) {
        Ok(report) => {
            assert!(!expect_refusal, "a retained text-bearing cell must refuse");
            assert_eq!(report.patterns_removed, 1);
            assert!(!has_decoded_secret(saved(&doc)), "successful redaction retained the pattern cell's secret in the saved PDF");
        }
        Err(error) => {
            assert!(expect_refusal, "a fully removed pattern should apply: {error}");
            assert!(matches!(error, RedactError::Pattern { reason: PatternProblem::Content, .. }), "{error}");
            assert_eq!(saved(&doc), before, "a refused pattern must leave the entire document unchanged");
            assert_eq!(marks(&doc).len(), 1, "the pending mark must survive refusal");
        }
    }
}

#[test]
fn pattern_whole_fill_saved_output_is_confidential_or_refused() {
    for compressed in [false, true] {
        assert_confidential_or_refused(fixture(CELL, compressed, false, false), false);
    }
}

#[test]
fn pattern_partial_fill_saved_output_is_confidential_or_refused() {
    let cell = [CELL, b"BT /F1 20 Tf 300 120 Td (SAFE) Tj ET\n"].concat();
    for compressed in [false, true] {
        assert_confidential_or_refused(fixture(&cell, compressed, true, false), true);
    }
}

#[test]
fn pattern_inherited_resources_saved_output_is_confidential_or_refused() {
    assert_confidential_or_refused(fixture(CELL, true, false, true), false);
}

#[test]
fn pattern_vector_only_control_still_applies_and_preserves_the_cell() {
    let cell = b"0.2 0.4 0.6 rg 40 120 15 15 re f\n";
    let mut doc = fixture(cell, true, true, false);
    mark(&mut doc, 0);
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.marks, 1);
    assert_eq!(report.paths_clipped, 1);
    assert!(marks(&doc).is_empty());
    let reopened = Document::open(Arc::new(saved(&doc))).unwrap();
    let page = &pdfcraft_model::pages(&reopened)[0];
    let res = reopened.dict(page.dict.get(b"Resources").unwrap()).unwrap();
    let patterns = reopened.dict(res.get(b"Pattern").unwrap()).unwrap();
    let pattern = reopened.resolve(patterns.get(b"P0").unwrap());
    let Object::Stream(pattern) = &*pattern else { panic!("pattern must be preserved") };
    assert_eq!(pattern.decoded().unwrap(), cell);
}

#[test]
fn unrelated_font_widths_do_not_exhaust_pattern_budget() {
    let mut doc = fixture(b"40 120 15 15 re f\n", false, true, false);
    doc.update_dict(ObjRef::new(4, 0), |font| {
        font.set(b"W".to_vec(), Object::Array(vec![Object::Int(500); 17_000]));
    })
    .unwrap();
    mark(&mut doc, 0);
    assert_eq!(apply(&mut doc, None).unwrap().marks, 1);
}

#[test]
fn shared_annotation_appearance_on_unselected_page_is_refused() {
    let mut doc = two_pages();
    let pages = pdfcraft_model::pages(&doc);
    for page in &pages {
        let mut resources = doc.dict(page.dict.get(b"Resources").unwrap()).unwrap();
        resources.remove(b"Pattern");
        doc.update_dict(page.obj, |dict| dict.set(b"Resources".to_vec(), Object::Dict(resources.clone()))).unwrap();
    }
    let ap = doc.add(Object::Stream(Stream::from_raw(
        Dict::from_iter([
            (b"Subtype".to_vec(), Object::name("Form")),
            (b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 500.into(), 300.into()])),
            (
                b"Resources".to_vec(),
                Object::Dict(Dict::from_iter([(
                    b"Pattern".to_vec(),
                    Object::Dict(Dict::from_iter([(b"P0".to_vec(), Object::Ref(ObjRef::new(6, 0)))])),
                )])),
            ),
        ]),
        b"/Pattern cs /P0 scn 35 110 170 35 re f".to_vec(),
    )));
    for page in pages {
        let annotation = doc.add(Object::Dict(Dict::from_iter([
            (b"Type".to_vec(), Object::name("Annot")),
            (b"Subtype".to_vec(), Object::name("Stamp")),
            (b"Rect".to_vec(), Object::Array(vec![30.into(), 105.into(), 215.into(), 150.into()])),
            (b"AP".to_vec(), Object::Dict(Dict::from_iter([(b"N".to_vec(), Object::Ref(ap))]))),
        ])));
        doc.update_dict(page.obj, |dict| dict.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(annotation)]))).unwrap();
    }
    mark(&mut doc, 0);
    let before = saved(&doc);
    let error = apply(&mut doc, Some(&[0])).unwrap_err();
    assert_eq!(error, RedactError::Pattern { page: 2, reason: PatternProblem::Content });
    assert_eq!(saved(&doc), before);
    assert_eq!(marks(&doc).len(), 1);
}

#[test]
fn pattern_refusal_on_second_page_does_not_change_the_first_page() {
    let mut doc = two_pages();
    mark(&mut doc, 0);
    mark(&mut doc, 1);
    let before = saved(&doc);
    let result = apply(&mut doc, None);
    assert!(result.is_err(), "text-carrying pattern must refuse the whole apply");
    assert_eq!(saved(&doc), before);
    assert_eq!(marks(&doc).len(), 2);
}

fn two_pages() -> Document {
    let mut doc = fixture(CELL, false, true, false);
    let source_page = pdfcraft_model::pages(&doc)[0].dict.clone();
    let mut clean = source_page.clone();
    let mut clean_resources = doc.dict(clean.get(b"Resources").unwrap()).unwrap();
    clean_resources.remove(b"Pattern");
    clean.set(b"Resources".to_vec(), Object::Dict(clean_resources));
    let content = doc.add(Object::Stream(Stream::from_raw(Default::default(), b"BT /F1 20 Tf 40 120 Td (DIRECTSECRET) Tj ET".to_vec())));
    clean.set(b"Contents".to_vec(), Object::Ref(content));
    let first = doc.add(Object::Dict(clean));
    doc.update_dict(ObjRef::new(2, 0), |pages| {
        pages.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(first), Object::Ref(ObjRef::new(3, 0))]));
        pages.set(b"Count".to_vec(), Object::Int(2));
    })
    .unwrap();
    doc
}

fn pattern_stream(doc: &Document) -> Stream {
    let object = doc.get(ObjRef::new(6, 0));
    let Object::Stream(stream) = &*object else { panic!("fixture pattern must be a stream") };
    stream.clone()
}

fn assert_refused(mut doc: Document, reason: PatternProblem) {
    mark(&mut doc, 0);
    let before = saved(&doc);
    let error = apply(&mut doc, None).unwrap_err();
    assert_eq!(error, RedactError::Pattern { page: 1, reason });
    assert!(!error.to_string().contains("CELLSECRET"), "errors must not disclose page text");
    assert_eq!(saved(&doc), before, "even direct callers must be unchanged");
    assert_eq!(marks(&doc).len(), 1);
}

#[test]
fn pattern_refusal_does_not_block_redacting_other_selected_pages() {
    let mut doc = two_pages();
    mark(&mut doc, 0);
    mark(&mut doc, 1);
    let report = apply(&mut doc, Some(&[0])).unwrap();
    assert_eq!(report.marks, 1);
    assert_eq!(marks(&doc).len(), 1);
    assert_eq!(marks(&doc)[0].page, 1);
    assert!(has_decoded_secret(saved(&doc)), "the unselected page must remain intact");
}

#[test]
fn pattern_nested_form_and_nested_pattern_text_are_refused() {
    for nested_pattern in [false, true] {
        let mut doc = fixture(CELL, false, true, false);
        let mut inner = pattern_stream(&doc);
        if !nested_pattern {
            inner.dict.remove(b"PatternType");
            inner.dict.set(b"Type".to_vec(), Object::name("XObject"));
            inner.dict.set(b"Subtype".to_vec(), Object::name("Form"));
        }
        let reference = doc.add(Object::Stream(inner));
        let resources = Dict::from_iter([(
            if nested_pattern { b"Pattern".to_vec() } else { b"XObject".to_vec() },
            Object::Dict(Dict::from_iter([(b"Inner".to_vec(), Object::Ref(reference))])),
        )]);
        let mut outer = pattern_stream(&doc);
        outer.dict.set(b"Resources".to_vec(), Object::Dict(resources));
        let content = if nested_pattern { b"/Pattern cs /Inner scn 35 110 400 35 re f".as_slice() } else { b"/Inner Do" };
        doc.set(ObjRef::new(6, 0), Object::Stream(Stream::flate(outer.dict, content)));
        assert_refused(doc, PatternProblem::Content);
    }
}

#[test]
fn pattern_inside_form_resources_is_refused() {
    let mut doc = fixture(CELL, false, false, false);
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let resources = doc.dict(page.dict.get(b"Resources").unwrap()).unwrap();
    let form = doc.add(Object::Stream(Stream::from_raw(
        Dict::from_iter([
            (b"Type".to_vec(), Object::name("XObject")),
            (b"Subtype".to_vec(), Object::name("Form")),
            (b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 500.into(), 300.into()])),
            (b"Resources".to_vec(), Object::Dict(resources)),
        ]),
        b"/Pattern cs /P0 scn 35 110 170 35 re f".to_vec(),
    )));
    let resources = Dict::from_iter([(b"XObject".to_vec(), Object::Dict(Dict::from_iter([(b"Fm".to_vec(), Object::Ref(form))])))]);
    doc.update_dict(page.obj, |dict| dict.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    doc.set(ObjRef::new(5, 0), Object::Stream(Stream::from_raw(Dict::new(), b"/Fm Do".to_vec())));
    assert_refused(doc, PatternProblem::Content);
}

#[test]
fn pattern_images_and_uncolored_text_are_refused() {
    let inline = b"q 10 0 0 10 40 120 cm BI /W 1 /H 1 /CS /G /BPC 8 ID \x00 EI Q";
    assert_refused(fixture(inline, true, true, false), PatternProblem::Content);
    let mut uncolored = fixture(CELL, false, true, false);
    uncolored.update_dict(ObjRef::new(6, 0), |dict| dict.set(b"PaintType".to_vec(), Object::Int(2))).unwrap();
    assert_refused(uncolored, PatternProblem::Content);
}

#[test]
fn pattern_indirect_filter_and_parameters_are_decoded_not_skipped() {
    let mut doc = fixture(CELL, true, true, false);
    let filter = doc.add(Object::name("FlateDecode"));
    let predictor = doc.add(Object::Int(1));
    let parms = doc.add(Object::Dict(Dict::from_iter([(b"Predictor".to_vec(), Object::Ref(predictor))])));
    doc.update_dict(ObjRef::new(6, 0), |dict| {
        dict.set(b"Filter".to_vec(), Object::Ref(filter));
        dict.set(b"DecodeParms".to_vec(), Object::Ref(parms));
    })
    .unwrap();
    assert_refused(doc, PatternProblem::Content);
}

#[test]
fn pattern_damaged_filters_and_unparsed_content_fail_closed() {
    for bad in [b"BT (unterminated".as_slice(), b"] BT /F1 20 Tf (CELLSECRET) Tj ET", b"BX UnknownPaint EX"] {
        assert_refused(fixture(bad, false, true, false), PatternProblem::Unreadable);
    }
    let mut damaged = fixture(CELL, true, true, false);
    let stream = pattern_stream(&damaged);
    let cut = stream.raw.get(..stream.raw.len() / 2).unwrap().to_vec();
    damaged.set(ObjRef::new(6, 0), Object::Stream(Stream::from_raw(stream.dict, cut)));
    assert_refused(damaged, PatternProblem::Unreadable);
    let mut parameters = fixture(CELL, true, true, false);
    parameters
        .update_dict(ObjRef::new(6, 0), |dict| {
            dict.set(b"DecodeParms".to_vec(), Object::Dict(Dict::from_iter([(b"Predictor".to_vec(), Object::Int(99))])));
        })
        .unwrap();
    assert_refused(parameters, PatternProblem::Unreadable);
}

#[test]
fn pattern_oversized_decoding_and_resource_cycles_fail_closed() {
    let large = vec![b' '; (1 << 20) + 1];
    assert_refused(fixture(&large, true, true, false), PatternProblem::Limit);
    let mut cycle = fixture(b"0 0 20 20 re f", false, true, false);
    cycle
        .update_dict(ObjRef::new(6, 0), |dict| {
            dict.set(
                b"Resources".to_vec(),
                Object::Dict(Dict::from_iter([(
                    b"Pattern".to_vec(),
                    Object::Dict(Dict::from_iter([(b"Self".to_vec(), Object::Ref(ObjRef::new(6, 0)))])),
                )])),
            );
        })
        .unwrap();
    assert_refused(cycle, PatternProblem::Unreadable);
}

#[test]
fn pattern_plain_nested_form_control_still_applies() {
    let mut doc = fixture(b"/Plain Do", false, true, false);
    let form = doc.add(Object::Stream(Stream::from_raw(
        Dict::from_iter([
            (b"Type".to_vec(), Object::name("XObject")),
            (b"Subtype".to_vec(), Object::name("Form")),
            (b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 500.into(), 300.into()])),
        ]),
        b"40 120 15 15 re f".to_vec(),
    )));
    doc.update_dict(ObjRef::new(6, 0), |dict| {
        dict.set(
            b"Resources".to_vec(),
            Object::Dict(Dict::from_iter([(b"XObject".to_vec(), Object::Dict(Dict::from_iter([(b"Plain".to_vec(), Object::Ref(form))])))])),
        );
    })
    .unwrap();
    mark(&mut doc, 0);
    assert_eq!(apply(&mut doc, None).unwrap().marks, 1);
    assert!(marks(&doc).is_empty());
}

#[test]
fn pattern_unused_and_annotation_appearance_resources_are_checked() {
    let mut unused = fixture(CELL, false, false, false);
    unused.set(ObjRef::new(5, 0), Object::Stream(Stream::from_raw(Dict::new(), b"0 0 20 20 re f".to_vec())));
    assert_refused(unused, PatternProblem::Content);

    let mut appearance = fixture(CELL, false, false, false);
    let page = pdfcraft_model::pages(&appearance)[0].clone();
    let mut resources = appearance.dict(page.dict.get(b"Resources").unwrap()).unwrap();
    let patterns = resources.remove(b"Pattern").unwrap();
    appearance.update_dict(page.obj, |dict| dict.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    appearance.set(ObjRef::new(5, 0), Object::Stream(Stream::from_raw(Dict::new(), b"0 0 20 20 re f".to_vec())));
    let ap = appearance.add(Object::Stream(Stream::from_raw(
        Dict::from_iter([
            (b"Subtype".to_vec(), Object::name("Form")),
            (b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 500.into(), 300.into()])),
            (b"Resources".to_vec(), Object::Dict(Dict::from_iter([(b"Pattern".to_vec(), patterns)]))),
        ]),
        b"/Pattern cs /P0 scn 35 110 170 35 re f".to_vec(),
    )));
    let annotation = appearance.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Annot")),
        (b"Subtype".to_vec(), Object::name("Stamp")),
        (b"Rect".to_vec(), Object::Array(vec![300.into(), 200.into(), 400.into(), 250.into()])),
        (b"AP".to_vec(), Object::Dict(Dict::from_iter([(b"N".to_vec(), Object::Ref(ap))]))),
    ])));
    appearance.update_dict(page.obj, |dict| dict.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(annotation)]))).unwrap();
    let mut implicit = appearance.clone();
    implicit
        .update_dict(ap, |dict| {
            dict.remove(b"Subtype");
        })
        .unwrap();
    assert_refused(implicit, PatternProblem::Content);
    assert_refused(appearance, PatternProblem::Content);
}

#[test]
fn pattern_image_xobjects_and_missing_resources_are_refused() {
    let mut doc = fixture(b"/Picture Do", false, true, false);
    let image = doc.add(Object::Stream(Stream::from_raw(
        Dict::from_iter([
            (b"Type".to_vec(), Object::name("XObject")),
            (b"Subtype".to_vec(), Object::name("Image")),
            (b"Width".to_vec(), Object::Int(1)),
            (b"Height".to_vec(), Object::Int(1)),
            (b"ColorSpace".to_vec(), Object::name("DeviceGray")),
            (b"BitsPerComponent".to_vec(), Object::Int(8)),
        ]),
        vec![0],
    )));
    doc.update_dict(ObjRef::new(6, 0), |dict| {
        dict.set(
            b"Resources".to_vec(),
            Object::Dict(Dict::from_iter([(b"XObject".to_vec(), Object::Dict(Dict::from_iter([(b"Picture".to_vec(), Object::Ref(image))])))])),
        );
    })
    .unwrap();
    assert_refused(doc, PatternProblem::Content);
    let mut missing = fixture(b"0 0 20 20 re f", false, true, false);
    missing
        .update_dict(ObjRef::new(6, 0), |dict| {
            dict.remove(b"Resources");
        })
        .unwrap();
    assert_refused(missing, PatternProblem::Unreadable);
}

#[test]
fn pattern_indirect_vector_filter_control_still_applies() {
    let mut doc = fixture(b"40 120 15 15 re f", true, true, false);
    let filter = doc.add(Object::name("FlateDecode"));
    let parms = doc.add(Object::Dict(Dict::from_iter([(b"Predictor".to_vec(), Object::Int(1))])));
    doc.update_dict(ObjRef::new(6, 0), |dict| {
        dict.set(b"Filter".to_vec(), Object::Array(vec![Object::Ref(filter)]));
        dict.set(b"DecodeParms".to_vec(), Object::Array(vec![Object::Ref(parms)]));
    })
    .unwrap();
    mark(&mut doc, 0);
    assert_eq!(apply(&mut doc, None).unwrap().marks, 1);
}

#[test]
fn pattern_depth_and_total_decoded_budgets_fail_closed() {
    let mut deep = fixture(b"0 0 20 20 re f", false, true, false);
    let mut nested = Object::Int(1);
    for _ in 0..70 {
        nested = Object::Array(vec![Object::Ref(deep.add(nested))]);
    }
    deep.update_dict(ObjRef::new(6, 0), |dict| dict.set(b"Extra".to_vec(), nested)).unwrap();
    assert_refused(deep, PatternProblem::Limit);

    let mut total = fixture(b"0 0 20 20 re f", false, true, false);
    let blank = vec![b' '; 1 << 20];
    let template = pattern_stream(&total).dict;
    let mut patterns = Dict::new();
    for index in 0..9 {
        let cell = total.add(Object::Stream(Stream::flate(template.clone(), &blank)));
        patterns.set(format!("P{index}").into_bytes(), Object::Ref(cell));
    }
    total
        .update_dict(ObjRef::new(3, 0), |dict| {
            dict.set(b"Resources".to_vec(), Object::Dict(Dict::from_iter([(b"Pattern".to_vec(), Object::Dict(patterns))])));
        })
        .unwrap();
    assert_refused(total, PatternProblem::Limit);
}

#[test]
fn pattern_replacement_text_without_text_showing_is_refused() {
    assert_refused(fixture(b"/Span << /ActualText (CELLSECRET) >> BDC 40 120 20 20 re f EMC", false, true, false), PatternProblem::Content);
    let mut named = fixture(b"/Span /Description BDC 40 120 20 20 re f EMC", true, true, false);
    named
        .update_dict(ObjRef::new(6, 0), |dict| {
            dict.set(
                b"Resources".to_vec(),
                Object::Dict(Dict::from_iter([(
                    b"Properties".to_vec(),
                    Object::Dict(Dict::from_iter([(
                        b"Description".to_vec(),
                        Object::Dict(Dict::from_iter([(b"Alt".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"CELLSECRET".to_vec())))])),
                    )])),
                )])),
            );
        })
        .unwrap();
    assert_refused(named, PatternProblem::Content);
}

#[test]
fn pattern_resource_names_are_not_dictionary_categories_or_descriptions() {
    for name in ["Pattern", "Alt", "E", "Resources"] {
        for has_pattern in [false, true] {
            let mut doc = fixture(b"40 120 15 15 re f", false, true, false);
            let font = Object::Dict(Dict::from_iter([(name.as_bytes().to_vec(), Object::Ref(ObjRef::new(4, 0)))]));
            if has_pattern {
                doc.update_dict(ObjRef::new(6, 0), |dict| {
                    dict.set(b"Resources".to_vec(), Object::Dict(Dict::from_iter([(b"Font".to_vec(), font)])));
                })
                .unwrap();
            } else {
                doc.update_dict(ObjRef::new(3, 0), |dict| {
                    dict.set(b"Resources".to_vec(), Object::Dict(Dict::from_iter([(b"Font".to_vec(), font)])));
                })
                .unwrap();
                doc.set(ObjRef::new(5, 0), Object::Stream(Stream::from_raw(Dict::new(), b"35 110 400 35 re f".to_vec())));
            }
            mark(&mut doc, 0);
            assert_eq!(apply(&mut doc, None).unwrap().marks, 1, "resource named {name}, pattern: {has_pattern}");
        }
    }
}

#[test]
fn pattern_opaque_and_external_xobjects_are_refused() {
    for kind in ["PS", "Bogus", "missing", "Form-PS", "Form-Ref"] {
        let mut doc = fixture(b"/X Do", false, true, false);
        let mut dict = Dict::from_iter([(b"Type".to_vec(), Object::name("XObject"))]);
        match kind {
            "missing" => {}
            "Form-PS" => {
                dict.set(b"Subtype".to_vec(), Object::name("Form"));
                dict.set(b"Subtype2".to_vec(), Object::name("PS"));
            }
            "Form-Ref" => {
                dict.set(b"Subtype".to_vec(), Object::name("Form"));
                dict.set(
                    b"Ref".to_vec(),
                    Object::Dict(Dict::from_iter([(b"F".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"external.pdf".to_vec())))])),
                );
            }
            subtype => {
                dict.set(b"Subtype".to_vec(), Object::name(subtype));
            }
        }
        // Reference XObjects can have an empty fallback: that cannot establish safety of
        // the external representation. No external file is read by this test or the guard.
        let data = if kind == "Form-Ref" { b"".as_slice() } else { b"/Helvetica findfont 20 scalefont setfont (CELLSECRET) show" };
        let object = doc.add(Object::Stream(Stream::from_raw(dict, data.to_vec())));
        doc.update_dict(ObjRef::new(6, 0), |dict| {
            dict.set(
                b"Resources".to_vec(),
                Object::Dict(Dict::from_iter([(b"XObject".to_vec(), Object::Dict(Dict::from_iter([(b"X".to_vec(), Object::Ref(object))])))])),
            );
        })
        .unwrap();
        assert_refused(doc, PatternProblem::Unreadable);
    }
}

#[test]
fn pattern_named_form_resources_still_apply() {
    for name in ["Pattern", "Alt", "E", "Resources"] {
        for inside in [false, true] {
            let content = format!("/{name} Do");
            let mut doc = fixture(content.as_bytes(), false, true, false);
            let form = doc.add(Object::Stream(Stream::from_raw(
                Dict::from_iter([
                    (b"Type".to_vec(), Object::name("XObject")),
                    (b"Subtype".to_vec(), Object::name("Form")),
                    (b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 500.into(), 300.into()])),
                ]),
                b"35 110 400 35 re f".to_vec(),
            )));
            let resources = Object::Dict(Dict::from_iter([(
                b"XObject".to_vec(),
                Object::Dict(Dict::from_iter([(name.as_bytes().to_vec(), Object::Ref(form))])),
            )]));
            doc.update_dict(ObjRef::new(if inside { 6 } else { 3 }, 0), |dict| dict.set(b"Resources".to_vec(), resources)).unwrap();
            if !inside {
                doc.set(ObjRef::new(5, 0), Object::Stream(Stream::from_raw(Dict::new(), content.into_bytes())));
            }
            mark(&mut doc, 0);
            assert_eq!(apply(&mut doc, None).unwrap().marks, 1, "form named {name}, inside: {inside}");
        }
    }
}

#[test]
fn pattern_soft_mask_groups_are_checked_by_resource_context() {
    for subtype in [None, Some("Bogus"), Some("Form")] {
        for text in [false, true] {
            let mut doc = fixture(b"q /GS gs 40 120 20 20 re f Q", false, true, false);
            let mut dict = Dict::from_iter([
                (b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 500.into(), 300.into()])),
                (b"Group".to_vec(), Object::Dict(Dict::from_iter([(b"S".to_vec(), Object::name("Transparency"))]))),
                (b"Resources".to_vec(), Object::Dict(Dict::new())),
            ]);
            if let Some(subtype) = subtype {
                dict.set(b"Subtype".to_vec(), Object::name(subtype));
            }
            let group = doc.add(Object::Stream(Stream::from_raw(dict, if text { CELL.to_vec() } else { b"40 120 20 20 re f".to_vec() })));
            let state = Object::Dict(Dict::from_iter([(
                b"SMask".to_vec(),
                Object::Dict(Dict::from_iter([(b"S".to_vec(), Object::name("Luminosity")), (b"G".to_vec(), Object::Ref(group))])),
            )]));
            doc.update_dict(ObjRef::new(6, 0), |dict| {
                dict.set(
                    b"Resources".to_vec(),
                    Object::Dict(Dict::from_iter([(b"ExtGState".to_vec(), Object::Dict(Dict::from_iter([(b"GS".to_vec(), state)])))])),
                );
            })
            .unwrap();
            match (subtype, text) {
                (Some("Form"), false) => {
                    mark(&mut doc, 0);
                    assert_eq!(apply(&mut doc, None).unwrap().marks, 1);
                }
                (Some("Form"), true) => assert_refused(doc, PatternProblem::Content),
                _ => assert_refused(doc, PatternProblem::Unreadable),
            }
        }
    }
}

#[test]
fn pattern_null_resource_categories_and_entries_are_absent() {
    for null_entries in [false, true] {
        let mut doc = fixture(b"40 120 15 15 re f", false, true, false);
        let mut resources = Dict::new();
        for category in ["Pattern", "XObject", "Properties", "ExtGState"] {
            let value = if null_entries { Object::Dict(Dict::from_iter([(b"Unused".to_vec(), Object::Null)])) } else { Object::Null };
            resources.set(category.as_bytes().to_vec(), value);
        }
        doc.update_dict(ObjRef::new(6, 0), |dict| dict.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
        mark(&mut doc, 0);
        assert_eq!(apply(&mut doc, None).unwrap().marks, 1);
    }
    let mut plain = fixture(b"", false, false, false);
    plain
        .update_dict(ObjRef::new(3, 0), |dict| {
            dict.set(b"Resources".to_vec(), Object::Dict(Dict::from_iter([(b"Pattern".to_vec(), Object::Null)])));
        })
        .unwrap();
    plain.set(ObjRef::new(5, 0), Object::Stream(Stream::from_raw(Dict::new(), b"40 120 20 20 re f".to_vec())));
    mark(&mut plain, 0);
    assert_eq!(apply(&mut plain, None).unwrap().marks, 1);
}
