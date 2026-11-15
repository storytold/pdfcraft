use super::*;

const SECRET: &[u8] = b"/Span /P0 BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC";
const DESCRIPTION: &[u8] = b"<< /ActualText (LEAK_ACTUAL) /Alt (LEAK_ALT) /E (LEAK_EXPANSION) /T (LEAK_TITLE) >>";
const PUBLIC: &[u8] = b"<< /ActualText (PUBLIC_ACTUAL) /Alt (PUBLIC_ALT) /E (PUBLIC_EXPANSION) /T (PUBLIC_TITLE) >>";

fn resources(doc: &Document, page: usize) -> Dict {
    let page = pdfcraft_model::pages(doc).swap_remove(page);
    doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone()
}

fn property(doc: &Document, resources: &Dict, name: &[u8]) -> Option<Dict> {
    let properties = doc.resolve(resources.get(b"Properties")?);
    doc.resolve(properties.as_dict()?.get(name)?).as_dict().cloned()
}

fn assert_description(dict: &Dict) {
    for key in [&b"ActualText"[..], b"Alt", b"E", b"T"] {
        assert!(dict.contains(key), "surviving description lost {key:?}");
    }
}

fn add_second_page(doc: &mut Document, data: &[u8], resources: Object) -> ObjRef {
    let mut page = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().clone();
    let contents = doc.add(Object::Stream(Stream::from_raw(Dict::new(), data.to_vec())));
    page.set(b"Contents".to_vec(), Object::Ref(contents));
    page.set(b"Resources".to_vec(), resources);
    let second = doc.add(Object::Dict(page));
    doc.update_dict(ObjRef::new(2, 0), |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(3, 0)), Object::Ref(second)]));
        d.set(b"Count".to_vec(), Object::Int(2));
    })
    .unwrap();
    second
}

fn redact_first(doc: &mut Document) -> Result<Report, RedactError> {
    mark(doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    apply(doc, Some(&[0]))
}

#[test]
fn surviving_form_own_resources_do_not_retain_unused_shared_descriptions() {
    for hidden in [false, true] {
        let mode = if hidden { 3 } else { 0 };
        let data = format!("/Span /P0 BDC BT /F1 10 Tf {mode} Tr 10 200 Td (SECRET) Tj ET EMC /Fm Do");
        let mut doc = one_page(
            data.as_bytes(),
            "/Properties << /P0 7 0 R /Public 8 0 R >> /XObject << /Fm 9 0 R >>",
            vec![
                DESCRIPTION.to_vec(),
                PUBLIC.to_vec(),
                stream("/Type /XObject /Subtype /Form /BBox [0 0 100 120]", b"/Span /Public BDC BT /F1 10 Tf 0 Tr 10 100 Td (Public) Tj ET EMC"),
            ],
        );
        // The page and its unchanged Form both reference the same resource object. The
        // dictionary names the Form itself, but the Form's content never invokes that name.
        let shared = doc.add(Object::Dict(resources(&doc, 0)));
        for owner in [ObjRef::new(3, 0), ObjRef::new(9, 0)] {
            doc.update_dict(owner, |d| d.set(b"Resources".to_vec(), Object::Ref(shared))).unwrap();
        }
        if hidden {
            sanitize::remove_hidden(&mut doc, &[sanitize::Hidden::HiddenText]).unwrap();
        } else {
            redact_first(&mut doc).unwrap();
        }
        assert_no_secret(&doc, b"LEAK_");
        assert_no_secret(&doc, b"SECRET");
        let mut saved = reopen(&doc);
        assert_eq!(under(&mut saved, 0, &[[0.0, 90.0, 100.0, 110.0]]), 6);
        let page_resources = resources(&saved, 0);
        let xobjects = saved.resolve(page_resources.get(b"XObject").unwrap());
        let form = saved.resolve(xobjects.as_dict().unwrap().get(b"Fm").unwrap());
        let form_resources = saved.resolve(form.as_dict().unwrap().get(b"Resources").unwrap());
        let form_resources = form_resources.as_dict().unwrap();
        assert!(property(&saved, form_resources, b"P0").is_none());
        assert_description(&property(&saved, form_resources, b"Public").unwrap());
    }
}

#[test]
fn unused_property_aliases_are_retired_by_value_not_resource_name() {
    for direct in [false, true] {
        let value = if direct { std::str::from_utf8(DESCRIPTION).unwrap() } else { "7 0 R" };
        let mut doc = one_page(SECRET, &format!("/Properties << /P0 {value} >>"), vec![DESCRIPTION.to_vec()]);
        let mut second_resources = resources(&doc, 0);
        let original_properties = doc.resolve(second_resources.get(b"Properties").unwrap());
        let mut alias = Dict::new();
        alias.set(b"Alias".to_vec(), original_properties.as_dict().unwrap().get(b"P0").unwrap().clone());
        second_resources.set(b"Properties".to_vec(), Object::Dict(alias));
        add_second_page(&mut doc, b"q Q", Object::Dict(second_resources));
        redact_first(&mut doc).unwrap();
        assert_no_secret(&doc, b"LEAK_");
        let saved = reopen(&doc);
        assert!(property(&saved, &resources(&saved, 1), b"Alias").is_none(), "direct={direct}");
    }
}

#[test]
fn unrelated_unused_property_with_the_same_name_is_not_changed() {
    let mut doc = one_page(SECRET, "/Properties << /P0 7 0 R >>", vec![DESCRIPTION.to_vec(), PUBLIC.to_vec()]);
    let mut second_resources = resources(&doc, 0);
    let mut properties = Dict::new();
    properties.set(b"P0".to_vec(), Object::Ref(ObjRef::new(8, 0)));
    second_resources.set(b"Properties".to_vec(), Object::Dict(properties));
    let second = add_second_page(&mut doc, b"q Q", Object::Dict(second_resources.clone()));
    let before = doc.get(second);
    redact_first(&mut doc).unwrap();
    assert_eq!(doc.get(second), before, "an unrelated page was rewritten");
    assert_no_secret(&doc, b"LEAK_");
    let saved = reopen(&doc);
    assert_description(&property(&saved, &resources(&saved, 1), b"P0").unwrap());
}

#[test]
fn null_resources_forms_preserve_a_retired_property_still_used_by_nested_dp() {
    let mut doc = one_page(
        SECRET,
        "/Properties << /P0 7 0 R >> /XObject << /Outer 8 0 R /Inner 9 0 R >>",
        vec![
            PUBLIC.to_vec(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 100 120] /Resources null", b"/Inner Do"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 100 120]", b"/Span /P0 DP BT /F1 10 Tf 10 100 Td (Public) Tj ET"),
        ],
    );
    let shared = doc.add(Object::Dict(resources(&doc, 0)));
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Resources".to_vec(), Object::Ref(shared))).unwrap();
    add_second_page(&mut doc, b"/Outer Do", Object::Ref(shared));
    redact_first(&mut doc).unwrap();
    assert_no_secret(&doc, b"SECRET");
    let mut saved = reopen(&doc);
    assert!(property(&saved, &resources(&saved, 0), b"P0").is_none());
    assert_description(&property(&saved, &resources(&saved, 1), b"P0").unwrap());
    assert_eq!(under(&mut saved, 1, &[[0.0, 90.0, 100.0, 110.0]]), 6);
}

#[test]
fn uninspectable_forms_with_candidate_properties_fail_closed() {
    for own in [false, true] {
        for malformed in [false, true] {
            let filter = if malformed { "" } else { "/Filter /UnsupportedPropertyTest" };
            let data = if malformed { b")".as_slice() } else { b"opaque" };
            if malformed {
                assert!(pdfcraft_content::parse(data).skipped > 0, "fixture must need parser recovery");
            }
            let mut doc = one_page(
                SECRET,
                "/Properties << /P0 7 0 R >> /XObject << /Fm 8 0 R >>",
                vec![DESCRIPTION.to_vec(), stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 100 120] {filter}"), data)],
            );
            let shared = doc.add(Object::Dict(resources(&doc, 0)));
            doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Resources".to_vec(), Object::Ref(shared))).unwrap();
            if own {
                doc.update_dict(ObjRef::new(8, 0), |d| d.set(b"Resources".to_vec(), Object::Ref(shared))).unwrap();
            }
            add_second_page(&mut doc, b"/Fm Do", Object::Ref(shared));
            assert_eq!(under(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]]), 6);
            let error = redact_first(&mut doc).unwrap_err();
            assert!(matches!(error, RedactError::Cos(_)), "own Resources={own}, malformed={malformed}: {error:?}");
        }
    }
}

#[test]
fn hidden_text_in_nested_forms_retires_named_descriptions_and_keeps_public_text() {
    let mut doc = one_page(
        b"/Outer Do",
        "/Properties << /P0 7 0 R /Public 8 0 R >> /XObject << /Outer 9 0 R /Inner 10 0 R >>",
        vec![
            DESCRIPTION.to_vec(),
            PUBLIC.to_vec(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"/Inner Do"),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300]",
                b"/Span /P0 BDC BT /F1 10 Tf 3 Tr 10 200 Td (SECRET) Tj ET EMC \
                  /Span /Public BDC BT /F1 10 Tf 0 Tr 10 100 Td (Public) Tj ET EMC",
            ),
        ],
    );
    sanitize::remove_hidden(&mut doc, &[sanitize::Hidden::HiddenText]).unwrap();
    assert_no_secret(&doc, b"SECRET");
    assert_no_secret(&doc, b"LEAK_");
    let mut saved = reopen(&doc);
    assert_eq!(under(&mut saved, 0, &[[0.0, 90.0, 100.0, 110.0]]), 6);
    // The rewritten inner Form owns the surviving /Public description after resources are
    // localized; no page-level property lookup is sufficient to check it.
    assert!(saved.scan_objects().any(|(_, object)| {
        object.as_dict().and_then(|d| d.get(b"ActualText")).and_then(Object::as_string).is_some_and(|s| s.to_text() == "PUBLIC_ACTUAL")
    }));
}

#[test]
fn removing_hidden_layer_content_cleans_its_surviving_outer_description() {
    let mut doc = one_page(
        b"/Span /P0 BDC /OC /Off BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC EMC \
          /Span /Public BDC BT /F1 10 Tf 10 100 Td (Public) Tj ET EMC",
        "/Properties << /P0 7 0 R /Public 8 0 R /Off 9 0 R >>",
        vec![DESCRIPTION.to_vec(), PUBLIC.to_vec(), b"<< /Type /OCG /Name (Hidden test layer) >>".to_vec()],
    );
    let mut defaults = Dict::new();
    defaults.set(b"OFF".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(9, 0))]));
    let mut optional_content = Dict::new();
    optional_content.set(b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(9, 0))]));
    optional_content.set(b"D".to_vec(), Object::Dict(defaults));
    doc.update_dict(ObjRef::new(1, 0), |d| d.set(b"OCProperties".to_vec(), Object::Dict(optional_content))).unwrap();
    sanitize::remove_hidden(&mut doc, &[sanitize::Hidden::HiddenLayers]).unwrap();
    assert_no_secret(&doc, b"SECRET");
    assert_no_secret(&doc, b"LEAK_");
    let mut saved = reopen(&doc);
    assert_description(&property(&saved, &resources(&saved, 0), b"Public").unwrap());
    assert_eq!(under(&mut saved, 0, &[[0.0, 90.0, 100.0, 110.0]]), 6);
}
