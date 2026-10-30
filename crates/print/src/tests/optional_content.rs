use super::*;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};

fn numbers(values: &[f64]) -> Object {
    Object::Array(values.iter().copied().map(Object::Real).collect())
}

fn layer_fixture(off: bool) -> (Document, ObjRef, ObjRef, ObjRef) {
    let mut doc = fixture(1);
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let group = doc.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("OCG")),
        (b"Name".to_vec(), Object::String(pdfcraft_cos::PdfString::text("Annotation Layer"))),
    ])));
    let properties = Dict::from_iter([
        (b"OCGs".to_vec(), Object::Array(vec![Object::Ref(group)])),
        (
            b"D".to_vec(),
            Object::Dict(Dict::from_iter([
                (b"BaseState".to_vec(), Object::name("ON")),
                (b"OFF".to_vec(), Object::Array(if off { vec![Object::Ref(group)] } else { vec![] })),
            ])),
        ),
    ]);
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"OCProperties".to_vec(), Object::Dict(properties))).unwrap();
    let form = Dict::from_iter([
        (b"Subtype".to_vec(), Object::name("Form")),
        (b"BBox".to_vec(), numbers(&[0.0, 0.0, 20.0, 10.0])),
        (b"Resources".to_vec(), Object::Dict(Dict::new())),
    ]);
    let appearance = doc.add(Object::Stream(Stream::flate(form, b"0 1 0 rg 0 0 20 10 re f")));
    let annotation = doc.add(Object::Dict(Dict::from_iter([
        (b"Subtype".to_vec(), Object::name("Square")),
        (b"F".to_vec(), Object::Int(4)),
        (b"Rect".to_vec(), numbers(&[10.0, 20.0, 50.0, 40.0])),
        (b"OC".to_vec(), Object::Ref(group)),
        (b"AP".to_vec(), Object::Dict(Dict::from_iter([(b"N".to_vec(), Object::Ref(appearance))]))),
    ])));
    let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), b"0 0 1 rg 0 0 8 8 re f")));
    doc.update_dict(page, |d| {
        d.set(b"MediaBox".to_vec(), numbers(&[0.0, 0.0, 100.0, 100.0]));
        d.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
        d.set(b"Contents".to_vec(), Object::Ref(contents));
        d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(annotation)]));
    })
    .unwrap();
    (doc, group, annotation, appearance)
}

fn render(bytes: Vec<u8>) -> RenderedPage {
    let page = PageRenderer::new(Arc::new(bytes), RenderConfig::default()).render(RenderRequest {
        page: 0,
        kind: RequestKind::Pixels,
        tile: None,
        scale: 1.0,
        tag: 0,
        preview: false,
    });
    assert_eq!(page.error, None);
    assert!(page.warnings.is_empty(), "{:?}", page.warnings);
    assert_eq!((page.width, page.height), (100, 100));
    page
}

fn print(doc: &Document) -> Result<Vec<u8>, PrintError> {
    impose(doc, &Settings { pages: vec![0], paper: (100.0, 100.0), layout: Layout::Size(SizeMode::Actual), ..Settings::default() })
}

#[test]
fn vector_print_preserves_annotation_layer_membership() {
    for off in [false, true] {
        let (doc, _, _, _) = layer_fixture(off);
        let before = write_full(&doc, &SaveOptions::default()).unwrap();
        let printed = print(&doc).unwrap();
        let reopened = Document::open(Arc::new(printed.clone())).unwrap();
        assert_eq!(pdfcraft_model::pages(&reopened).len(), 1);
        let page = render(printed);
        let pixel = |x: usize, y: usize| &page.rgba[(y * 100 + x) * 4..(y * 100 + x + 1) * 4];
        assert_eq!(pixel(30, 70), if off { [255, 255, 255, 255] } else { [0, 255, 0, 255] }, "layer off={off}");
        assert_eq!(pixel(4, 96), [0, 0, 255, 255], "ordinary page artwork");
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    }
}

fn saved(doc: &Document) -> Vec<u8> {
    write_full(doc, &SaveOptions::default()).unwrap()
}

fn pixel(page: &RenderedPage, x: usize, y: usize) -> &[u8] {
    let offset = (y * page.width as usize + x) * 4;
    &page.rgba[offset..offset + 4]
}

fn assert_appearance(page: &RenderedPage, visible: bool) {
    assert_eq!(pixel(page, 30, 70), if visible { [0, 255, 0, 255] } else { [255, 255, 255, 255] });
    assert_eq!(pixel(page, 4, 96), [0, 0, 255, 255], "ordinary page artwork");
}

fn add_group(doc: &mut Document, name: &str, off: bool) -> ObjRef {
    let group = doc.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("OCG")),
        (b"Name".to_vec(), Object::String(pdfcraft_cos::PdfString::text(name))),
    ])));
    let root = doc.root().unwrap();
    let mut properties = doc.resolve(doc.get(root).as_dict().unwrap().get(b"OCProperties").unwrap()).as_dict().unwrap().clone();
    let mut groups = properties.get(b"OCGs").unwrap().as_array().unwrap().clone();
    groups.push(Object::Ref(group));
    properties.set(b"OCGs".to_vec(), Object::Array(groups));
    if off {
        let mut config = properties.get(b"D").unwrap().as_dict().unwrap().clone();
        let mut hidden = config.get(b"OFF").unwrap().as_array().unwrap().clone();
        hidden.push(Object::Ref(group));
        config.set(b"OFF".to_vec(), Object::Array(hidden));
        properties.set(b"D".to_vec(), Object::Dict(config));
    }
    doc.update_dict(root, |d| d.set(b"OCProperties".to_vec(), Object::Dict(properties))).unwrap();
    group
}

fn membership(groups: Vec<Object>, policy: Option<&str>, expression: Option<Vec<Object>>) -> Dict {
    let mut dict = Dict::from_iter([
        (b"Type".to_vec(), Object::name("OCMD")),
        (b"OCGs".to_vec(), Object::Array(groups)),
        (b"PrivatePolicy".to_vec(), Object::Array(vec![Object::Int(19), Object::name("KeepMe")])),
    ]);
    if let Some(policy) = policy {
        dict.set(b"P".to_vec(), Object::name(policy));
    }
    if let Some(expression) = expression {
        dict.set(b"VE".to_vec(), Object::Array(expression));
    }
    dict
}

fn printed_form(doc: &Document) -> Dict {
    let page = pdfcraft_model::pages(doc)[0].dict.clone();
    let resources = doc.resolve(page.get(b"Resources").unwrap());
    let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    doc.resolve(xobjects.as_dict().unwrap().get(b"P0").unwrap()).as_dict().unwrap().clone()
}

fn form_properties(doc: &Document) -> Dict {
    let form = printed_form(doc);
    let resources = doc.resolve(form.get(b"Resources").unwrap());
    doc.resolve(resources.as_dict().unwrap().get(b"Properties").unwrap()).as_dict().unwrap().clone()
}

#[test]
fn vector_print_preserves_ocmd_policies_and_visibility_expressions() {
    for indirect in [false, true] {
        for group_shape in 0..3 {
            let single_group = group_shape != 0;
            for (policy, visible) in [("AllOn", false), ("AnyOn", !single_group), ("AnyOff", true), ("AllOff", single_group)] {
                let (mut doc, off, annotation, appearance) = layer_fixture(true);
                let on = add_group(&mut doc, "Visible", false);
                let groups = if single_group { vec![Object::Ref(off)] } else { vec![Object::Ref(off), Object::Ref(on)] };
                let mut dict = membership(groups, Some(policy), None);
                if group_shape == 2 {
                    dict.set(b"OCGs".to_vec(), Object::Ref(off));
                }
                let private = dict.get(b"PrivatePolicy").unwrap().clone();
                let oc = if indirect { Object::Ref(doc.add(Object::Dict(dict))) } else { Object::Dict(dict) };
                doc.update_dict(annotation, |d| d.set(b"OC".to_vec(), oc)).unwrap();
                let before = saved(&doc);
                let original_appearance = doc.get(appearance);
                let printed = print(&doc).unwrap();
                assert_appearance(&render(printed.clone()), visible);
                let reopened = Document::open(Arc::new(printed)).unwrap();
                let props = form_properties(&reopened);
                let membership = props.get(b"PCPrOC0").unwrap();
                assert!(membership.as_ref().is_some(), "direct OCMD must be promoted for named-resource rendering");
                assert_eq!(reopened.resolve(membership).as_dict().unwrap().get(b"PrivatePolicy"), Some(&private));
                assert_appearance(&render(print(&reopened).unwrap()), visible);
                assert_eq!(doc.get(appearance), original_appearance);
                assert_eq!(saved(&doc), before, "policy={policy}, indirect={indirect}, single={single_group}");
            }
        }
        for case in 0..6 {
            let (mut doc, off, annotation, _) = layer_fixture(true);
            let on = add_group(&mut doc, "Visible", false);
            let mut dict = membership(vec![Object::Ref(off), Object::Ref(on)], None, None);
            let visible = match case {
                0 => true, // Missing P defaults to AnyOn.
                1 => {
                    dict.set(b"P".to_vec(), Object::name("AllOn"));
                    dict.set(b"VE".to_vec(), Object::Array(vec![Object::name("Not"), Object::Ref(off)]));
                    true // VE overrides the false AllOn policy.
                }
                2 => {
                    dict.set(b"P".to_vec(), Object::name("AnyOn"));
                    dict.set(b"VE".to_vec(), Object::Array(vec![Object::name("Not"), Object::Ref(on)]));
                    false // VE also overrides a true policy.
                }
                3 => {
                    dict.remove(b"OCGs");
                    true // Missing OCGs is a valid no-op.
                }
                4 => {
                    dict.set(b"OCGs".to_vec(), Object::Array(vec![]));
                    true
                }
                _ => {
                    dict.remove(b"OCGs");
                    dict.set(b"VE".to_vec(), Object::Array(vec![Object::name("Not"), Object::Ref(on)]));
                    false // VE-only membership is valid.
                }
            };
            let oc = if indirect { Object::Ref(doc.add(Object::Dict(dict))) } else { Object::Dict(dict) };
            doc.update_dict(annotation, |d| d.set(b"OC".to_vec(), oc)).unwrap();
            let before = saved(&doc);
            assert_appearance(&render(print(&doc).unwrap()), visible);
            assert_eq!(saved(&doc), before, "OCMD case={case}, indirect={indirect}");
        }
    }
}

#[test]
fn annotation_and_appearance_layers_are_independent_and_shared_appearances_stay_shared() {
    for annotation_off in [false, true] {
        for appearance_off in [false, true] {
            let (mut doc, _, annotation, appearance) = layer_fixture(annotation_off);
            let appearance_group = add_group(&mut doc, "Appearance", appearance_off);
            doc.update_dict(appearance, |d| d.set(b"OC".to_vec(), Object::Ref(appearance_group))).unwrap();
            let before = saved(&doc);
            let first = print(&doc).unwrap();
            assert_appearance(&render(first.clone()), !annotation_off && !appearance_off);
            let reopened = Document::open(Arc::new(first)).unwrap();
            assert_appearance(&render(print(&reopened).unwrap()), !annotation_off && !appearance_off);
            assert_eq!(saved(&doc), before);
            assert_eq!(doc.get(annotation).as_dict().unwrap().get(b"AP").unwrap().as_dict().unwrap().reference(b"N"), Some(appearance));
        }
    }
    let (mut doc, _, annotation, appearance) = layer_fixture(true);
    let visible = add_group(&mut doc, "Visible", false);
    let mut other = doc.get(annotation).as_dict().unwrap().clone();
    other.set(b"OC".to_vec(), Object::Ref(visible));
    other.set(b"Rect".to_vec(), numbers(&[10.0, 50.0, 50.0, 70.0]));
    let other = doc.add(Object::Dict(other));
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(annotation), Object::Ref(other)]))).unwrap();
    let before = saved(&doc);
    let original = doc.get(appearance);
    let first = print(&doc).unwrap();
    let reopened = Document::open(Arc::new(first.clone())).unwrap();
    for bytes in [first, print(&reopened).unwrap()] {
        let image = render(bytes);
        assert_appearance(&image, false);
        assert_eq!(pixel(&image, 30, 40), [0, 255, 0, 255], "the other annotation uses the same appearance in a visible layer");
    }
    let form = printed_form(&reopened);
    let resources = reopened.resolve(form.get(b"Resources").unwrap());
    let objects = reopened.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    let objects = objects.as_dict().unwrap();
    assert_eq!(objects.get(b"PCPrA0"), objects.get(b"PCPrA1"), "membership belongs to each invocation, not an appearance copy");
    assert_eq!(doc.get(appearance), original);
    assert_eq!(saved(&doc), before);
}

#[test]
fn annotation_layers_keep_colliding_inherited_and_shared_page_properties() {
    for indirect_resources in [false, true] {
        for indirect_properties in [false, true] {
            for inherited in [false, true] {
                let (mut doc, hidden, annotation, appearance) = layer_fixture(true);
                let visible = add_group(&mut doc, "Visible", false);
                let unrelated = Object::Dict(Dict::from_iter([(b"PrivateKey".to_vec(), Object::name("KeepMe"))]));
                let properties = Dict::from_iter([
                    (b"PCPrOC0".to_vec(), Object::Ref(hidden)),
                    (b"PCPrOC1".to_vec(), Object::Ref(visible)),
                    (b"PCPrOC3".to_vec(), Object::Null),
                    (b"Unrelated".to_vec(), unrelated.clone()),
                ]);
                let properties = if indirect_properties { Object::Ref(doc.add(Object::Dict(properties))) } else { Object::Dict(properties) };
                let resources = Dict::from_iter([(b"Properties".to_vec(), properties)]);
                let resources = if indirect_resources { Object::Ref(doc.add(Object::Dict(resources))) } else { Object::Dict(resources) };
                // The appearance shares the page's resource owner, so in-place extension would change both.
                doc.update_dict(appearance, |d| d.set(b"Resources".to_vec(), resources.clone())).unwrap();
                let mut other = doc.get(annotation).as_dict().unwrap().clone();
                other.set(b"OC".to_vec(), Object::Ref(visible));
                other.set(b"Rect".to_vec(), numbers(&[10.0, 50.0, 50.0, 70.0]));
                let other = doc.add(Object::Dict(other));
                let contents = doc.add(Object::Stream(Stream::flate(
                    Dict::new(),
                    b"0 0 1 rg 0 0 8 8 re f\n/OC /PCPrOC0 BDC 1 0 0 rg 60 60 10 10 re f EMC\n/OC /PCPrOC1 BDC 0 0 1 rg 70 60 10 10 re f EMC\n/Span /Unrelated BDC 1 0 1 rg 80 60 10 10 re f EMC",
                )));
                let page = pdfcraft_model::pages(&doc)[0].obj;
                doc.update_dict(page, |d| {
                    d.set(b"Contents".to_vec(), Object::Ref(contents));
                    d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(annotation), Object::Ref(other)]));
                })
                .unwrap();
                if inherited {
                    let parent = doc.get(page).as_dict().unwrap().reference(b"Parent").unwrap();
                    doc.update_dict(parent, |d| d.set(b"Resources".to_vec(), resources)).unwrap();
                    doc.update_dict(page, |d| {
                        d.remove(b"Resources");
                    })
                    .unwrap();
                } else {
                    doc.update_dict(page, |d| d.set(b"Resources".to_vec(), resources)).unwrap();
                }
                let before = saved(&doc);
                let original_appearance = doc.get(appearance);
                let first = print(&doc).unwrap();
                let reopened = Document::open(Arc::new(first.clone())).unwrap();
                let properties = form_properties(&reopened);
                assert_eq!(properties.get(b"PCPrOC3"), Some(&Object::Null));
                assert_eq!(properties.get(b"Unrelated"), Some(&unrelated));
                assert_eq!(properties.get(b"PCPrOC2"), properties.get(b"PCPrOC0"));
                assert_eq!(properties.get(b"PCPrOC4"), properties.get(b"PCPrOC1"));
                for bytes in [first, print(&reopened).unwrap()] {
                    let image = render(bytes);
                    assert_appearance(&image, false);
                    assert_eq!(pixel(&image, 30, 40), [0, 255, 0, 255]);
                    assert_eq!(pixel(&image, 65, 35), [255, 255, 255, 255], "existing hidden page content");
                    assert_eq!(pixel(&image, 75, 35), [0, 0, 255, 255], "existing visible page content");
                    assert_eq!(pixel(&image, 85, 35), [255, 0, 255, 255], "unrelated marked-content property");
                }
                assert_eq!(doc.get(appearance), original_appearance);
                assert_eq!(saved(&doc), before, "resources={indirect_resources}, properties={indirect_properties}, inherited={inherited}");
            }
        }
    }
}

fn reference_chain(doc: &mut Document, mut value: Object, references: usize) -> Object {
    for _ in 0..references {
        value = Object::Ref(doc.add(value));
    }
    value
}

fn cycle(doc: &mut Document, two_nodes: bool) -> Object {
    let first = doc.add(Object::Null);
    let second = if two_nodes { doc.add(Object::Ref(first)) } else { first };
    doc.set(first, Object::Ref(second));
    Object::Ref(first)
}

#[test]
fn absent_annotation_membership_and_null_resource_maps_remain_valid() {
    for case in 0..6 {
        let (mut doc, _, annotation, _) = layer_fixture(true);
        match case {
            0 => {
                doc.update_dict(annotation, |d| {
                    d.remove(b"OC");
                })
                .unwrap();
            }
            1 => doc.update_dict(annotation, |d| d.set(b"OC".to_vec(), Object::Null)).unwrap(),
            2 => doc.update_dict(annotation, |d| d.set(b"OC".to_vec(), Object::Ref(ObjRef::new(900_000, 0)))).unwrap(),
            _ => {
                let page = pdfcraft_model::pages(&doc)[0].obj;
                let resources = match case {
                    3 => Object::Null,
                    4 => Object::Dict(Dict::from_iter([(b"Properties".to_vec(), Object::Null)])),
                    _ => Object::Dict(Dict::from_iter([(b"Properties".to_vec(), Object::Ref(ObjRef::new(900_000, 0)))])),
                };
                doc.update_dict(page, |d| d.set(b"Resources".to_vec(), resources)).unwrap();
            }
        }
        let before = saved(&doc);
        assert_appearance(&render(print(&doc).unwrap()), case < 3);
        assert_eq!(saved(&doc), before, "null/absent case={case}");
    }
}

#[test]
fn invalid_annotation_membership_is_refused_without_mutation() {
    for case in 0..9 {
        let (mut doc, _, annotation, _) = layer_fixture(false);
        let value = match case {
            0 => Object::Int(7),
            1 => Object::name("Layer"),
            2 => Object::Array(vec![]),
            3 => Object::Dict(Dict::new()),
            4 => Object::Dict(Dict::from_iter([(b"Type".to_vec(), Object::name("Page"))])),
            5 => Object::Dict(Dict::from_iter([(b"Type".to_vec(), Object::name("OCG"))])),
            6 => Object::Ref(doc.add(Object::Stream(Stream::flate(membership(vec![], None, None), b"")))),
            7 => cycle(&mut doc, false),
            _ => cycle(&mut doc, true),
        };
        doc.update_dict(annotation, |d| d.set(b"OC".to_vec(), value)).unwrap();
        let before = saved(&doc);
        assert!(matches!(print(&doc), Err(PrintError::Invalid(_))), "invalid membership case={case}");
        assert_eq!(saved(&doc), before, "invalid membership case={case}");
    }
}

#[test]
fn optional_content_reference_resolution_is_bounded_and_keeps_terminal_group_identity() {
    for references in [64, 65] {
        for target in ["membership", "type", "properties", "resources"] {
            let (mut doc, group, annotation, _) = layer_fixture(true);
            match target {
                "membership" => {
                    // The final OCG is already indirect and counts as one hop.
                    let value = reference_chain(&mut doc, Object::Ref(group), references - 1);
                    doc.update_dict(annotation, |d| d.set(b"OC".to_vec(), value)).unwrap();
                }
                "type" => {
                    let value = reference_chain(&mut doc, Object::name("OCG"), references);
                    doc.update_dict(group, |d| d.set(b"Type".to_vec(), value)).unwrap();
                }
                _ => {
                    let value = reference_chain(&mut doc, Object::Dict(Dict::new()), references);
                    let resources = if target == "properties" { Object::Dict(Dict::from_iter([(b"Properties".to_vec(), value)])) } else { value };
                    let page = pdfcraft_model::pages(&doc)[0].obj;
                    doc.update_dict(page, |d| d.set(b"Resources".to_vec(), resources)).unwrap();
                }
            }
            let before = saved(&doc);
            let result = print(&doc);
            if references == 64 {
                let bytes = result.unwrap();
                // Check Type aliases structurally: rendering them would also test the
                // renderer's separate reference-resolution behavior.
                if target == "type" {
                    let reopened = Document::open(Arc::new(bytes)).unwrap();
                    let root = reopened.get(reopened.root().unwrap());
                    let catalog = reopened.resolve(root.as_dict().unwrap().get(b"OCProperties").unwrap());
                    let groups = catalog.as_dict().unwrap().get(b"OCGs").unwrap().as_array().unwrap();
                    assert_eq!(form_properties(&reopened).get(b"PCPrOC0"), groups.first());
                } else {
                    assert_appearance(&render(bytes), false);
                }
            } else {
                assert!(matches!(result, Err(PrintError::Invalid(_))), "target={target}");
            }
            assert_eq!(saved(&doc), before, "target={target}, references={references}");
        }
    }
}

#[test]
fn malformed_page_resource_maps_cannot_remove_optional_content_guards() {
    for target in ["Resources", "Properties"] {
        for case in 0..4 {
            let (mut doc, _, _, _) = layer_fixture(true);
            let value = match case {
                0 => Object::Int(7),
                1 => Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), b"")))),
                2 => cycle(&mut doc, false),
                _ => cycle(&mut doc, true),
            };
            let resources = if target == "Properties" { Object::Dict(Dict::from_iter([(b"Properties".to_vec(), value)])) } else { value };
            let page = pdfcraft_model::pages(&doc)[0].obj;
            doc.update_dict(page, |d| d.set(b"Resources".to_vec(), resources)).unwrap();
            let before = saved(&doc);
            assert!(matches!(print(&doc), Err(PrintError::Invalid(_))), "target={target}, case={case}");
            assert_eq!(saved(&doc), before, "target={target}, case={case}");
        }
    }
}

#[test]
fn malformed_membership_on_annotations_that_do_not_draw_does_not_block_printing() {
    for case in 0..6 {
        let (mut doc, _, annotation, _) = layer_fixture(false);
        doc.update_dict(annotation, |d| {
            d.set(b"OC".to_vec(), Object::Int(7));
            match case {
                0 => d.set(b"F".to_vec(), Object::Int(0)),
                1 => d.set(b"F".to_vec(), Object::Int(6)),
                2 => d.set(b"Subtype".to_vec(), Object::name("Link")),
                3 => {
                    d.remove(b"AP");
                }
                4 => d.set(b"Rect".to_vec(), Object::Array(vec![])),
                _ => {}
            }
        })
        .unwrap();
        // Resource validation is also lazy: these maps are irrelevant when no guarded
        // annotation invocation will be appended.
        let page = pdfcraft_model::pages(&doc)[0].obj;
        let resources =
            if case % 2 == 0 { Object::Dict(Dict::from_iter([(b"Properties".to_vec(), Object::Int(7))])) } else { cycle(&mut doc, false) };
        doc.update_dict(page, |d| d.set(b"Resources".to_vec(), resources)).unwrap();
        let before = saved(&doc);
        let bytes = if case == 5 {
            impose(
                &doc,
                &Settings {
                    pages: vec![0],
                    paper: (100.0, 100.0),
                    layout: Layout::Size(SizeMode::Actual),
                    content: Content::Document,
                    ..Settings::default()
                },
            )
            .unwrap()
        } else {
            print(&doc).unwrap()
        };
        assert_appearance(&render(bytes), false);
        assert_eq!(saved(&doc), before, "excluded annotation case={case}");
    }
}
