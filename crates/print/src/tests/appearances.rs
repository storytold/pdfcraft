use super::*;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};

fn numbers(values: &[f64]) -> Object {
    Object::Array(values.iter().copied().map(Object::Real).collect())
}

// Contributor-original artwork uses a named color resource, without fonts or external assets.
fn widget_fixture(subtype: Option<Object>, matrix: Option<[f64; 6]>) -> (Document, ObjRef, ObjRef) {
    let mut doc = fixture(1);
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let mut colors = Dict::new();
    colors.set(b"Ink".to_vec(), Object::name("DeviceRGB"));
    let mut resources = Dict::new();
    resources.set(b"ColorSpace".to_vec(), Object::Dict(colors));
    let resources = doc.add(Object::Dict(resources));
    let mut d = Dict::new();
    d.set(b"BBox".to_vec(), numbers(&[0.0, 0.0, 20.0, 10.0]));
    d.set(b"Resources".to_vec(), Object::Ref(resources));
    d.set(b"PrivateKey".to_vec(), Object::name("PreserveMe"));
    if let Some(subtype) = subtype {
        d.set(b"Subtype".to_vec(), subtype);
    }
    if let Some(matrix) = matrix {
        d.set(b"Matrix".to_vec(), numbers(&matrix));
    }
    let mut stream = Stream::flate(d, b"/Ink cs 0 1 0 scn 0 0 20 10 re f");
    stream.dict.set(b"DecodeParms".to_vec(), Object::Dict(Dict::new()));
    let ap = doc.add(Object::Stream(stream));
    let mut widget = Dict::new();
    widget.set(b"Subtype".to_vec(), Object::name("Widget"));
    widget.set(b"FT".to_vec(), Object::name("Tx"));
    widget.set(b"F".to_vec(), Object::Int(4));
    widget.set(b"Rect".to_vec(), numbers(&[10.0, 20.0, 50.0, 40.0]));
    let mut appearance = Dict::new();
    appearance.set(b"N".to_vec(), Object::Ref(ap));
    widget.set(b"AP".to_vec(), Object::Dict(appearance));
    let widget = doc.add(Object::Dict(widget));
    let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), b"0 0 1 rg 0 0 8 8 re f")));
    doc.update_dict(page, |d| {
        d.set(b"MediaBox".to_vec(), numbers(&[0.0, 0.0, 100.0, 100.0]));
        d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(widget)]));
        d.set(b"Contents".to_vec(), Object::Ref(contents));
        d.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
    })
    .unwrap();
    (doc, ap, widget)
}

fn print_settings(content: Content) -> Settings {
    Settings { pages: vec![0], paper: (100.0, 100.0), layout: Layout::Size(SizeMode::Actual), content, ..Settings::default() }
}

fn render(bytes: Vec<u8>) -> RenderedPage {
    render_page(bytes, 0)
}

fn render_page(bytes: Vec<u8>, index: usize) -> RenderedPage {
    let page = PageRenderer::new(Arc::new(bytes), RenderConfig::default()).render(RenderRequest {
        page: index,
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

fn pixel(page: &RenderedPage, x: usize, y: usize) -> &[u8] {
    let offset = (y * page.width as usize + x) * 4;
    &page.rgba[offset..offset + 4]
}

fn saved(doc: &Document) -> Vec<u8> {
    write_full(doc, &SaveOptions::default()).unwrap()
}

fn xobject(doc: &Document, owner: &Dict, name: &[u8]) -> ObjRef {
    let resources = doc.resolve(owner.get(b"Resources").unwrap());
    let objects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    objects.as_dict().unwrap().get(name).unwrap().as_ref().unwrap()
}

#[test]
fn missing_subtype_appearance_survives_vector_print() {
    let (control, _, _) = widget_fixture(Some(Object::name("Form")), None);
    let control = render(impose(&control, &print_settings(Content::Document)).unwrap());
    assert_eq!(pixel(&control, 30, 70), [0, 255, 0, 255], "valid Form control");
    let (doc, ap, _) = widget_fixture(None, None);
    let before = saved(&doc);
    let state = doc.read_state();
    let source_bytes = doc.bytes().clone();
    let graph: Vec<_> = doc.scan_objects().collect();
    let original = doc.get(ap);
    let source = render(before.clone());
    assert_eq!(pixel(&source, 30, 70), [0, 255, 0, 255], "source display");
    let output = impose(&doc, &print_settings(Content::Document)).unwrap();
    let reopened = Document::open(Arc::new(output.clone())).unwrap();
    assert_eq!(pdfcraft_model::pages(&reopened).len(), 1);
    let printed = render(output);
    assert_eq!(pixel(&printed, 5, 5), [255, 255, 255, 255], "outside the appearance");
    assert_eq!(pixel(&printed, 30, 70), [0, 255, 0, 255], "imposed missing-Subtype appearance");
    assert_eq!(printed.rgba, control.rgba);
    assert!(state.matches(&doc));
    assert_eq!(doc.bytes(), &source_bytes);
    assert_eq!(doc.scan_objects().collect::<Vec<_>>(), graph);
    assert_eq!(doc.get(ap), original);
    assert_eq!(saved(&doc), before);
    let source = Document::open(Arc::new(before)).unwrap();
    let page = &pdfcraft_model::pages(&source)[0];
    let annots = source.resolve(page.dict.get(b"Annots").unwrap());
    let widget = source.resolve(&annots.as_array().unwrap()[0]);
    let selected = appearance(&source, widget.as_dict().unwrap()).unwrap();
    let Object::Stream(stream) = &*source.get(selected) else { panic!("source AP stream") };
    assert!(!stream.dict.contains(b"Subtype") && !stream.dict.contains(b"Type"));
    assert_eq!(stream.decoded().unwrap(), b"/Ink cs 0 1 0 scn 0 0 20 10 re f");
    assert_eq!(source.resolve(stream.dict.get(b"Resources").unwrap()), doc.resolve(original.as_dict().unwrap().get(b"Resources").unwrap()));
    let sheet = &pdfcraft_model::pages(&reopened)[0];
    let form = reopened.get(xobject(&reopened, &sheet.dict, b"P0"));
    let promoted = reopened.get(xobject(&reopened, form.as_dict().unwrap(), b"PCPrA0"));
    let Object::Stream(promoted) = &*promoted else { panic!("promoted AP stream") };
    assert_eq!(promoted.dict.name(b"Subtype"), Some(b"Form".as_slice()));
    let Object::Stream(original) = &*original else { panic!("original AP stream") };
    assert_eq!(promoted.raw, original.raw);
    let mut expected = original.dict.clone();
    let mut actual = promoted.dict.clone();
    actual.remove(b"Subtype");
    actual.remove(b"Length");
    expected.remove(b"Length");
    let expected_resources = doc.resolve(expected.remove(b"Resources").as_ref().unwrap());
    let actual_resources = reopened.resolve(actual.remove(b"Resources").as_ref().unwrap());
    assert_eq!(actual_resources, expected_resources);
    // The writer can spell integral Real values as integers. Compare their PDF values.
    let mut actual_bytes = Vec::new();
    let mut expected_bytes = Vec::new();
    pdfcraft_cos::serialize(&Object::Dict(actual), &mut actual_bytes);
    pdfcraft_cos::serialize(&Object::Dict(expected), &mut expected_bytes);
    assert_eq!(actual_bytes, expected_bytes);
}

#[test]
fn promotion_preserves_streams_and_explicit_subtypes() {
    for subtype in [None, Some(Object::name("Form")), Some(Object::name("Image")), Some(Object::Null), Some(Object::Int(7))] {
        let (mut doc, ap, _) = widget_fixture(subtype.clone(), Some([2.0, 0.0, 0.0, 3.0, 5.0, 7.0]));
        let original = doc.get(ap);
        let form = page_form(&mut doc, 0, Content::Document, &mut std::collections::HashMap::new()).unwrap();
        assert_eq!(doc.get(ap), original, "original in the print clone");
        let promoted = xobject(&doc, doc.get(form).as_dict().unwrap(), b"PCPrA0");
        if subtype.is_some() {
            assert_eq!(promoted, ap, "an explicit subtype keeps its object");
            let printed = Document::open(Arc::new(impose(&doc, &print_settings(Content::Document)).unwrap())).unwrap();
            let sheet = &pdfcraft_model::pages(&printed)[0];
            let page_form = printed.get(xobject(&printed, &sheet.dict, b"P0"));
            let printed_ap = printed.get(xobject(&printed, page_form.as_dict().unwrap(), b"PCPrA0"));
            assert_eq!(printed_ap.as_dict().unwrap().get(b"Subtype"), subtype.as_ref(), "explicit subtype survives serialization");
        } else {
            assert_ne!(promoted, ap, "repair needs a separate object");
            let Object::Stream(mut expected) = (*original).clone() else { panic!("AP stream") };
            expected.dict.set(b"Subtype".to_vec(), Object::name("Form"));
            assert_eq!(*doc.get(promoted), Object::Stream(expected), "only Subtype changes, including encoded data and resource refs");
        }
    }
}

#[test]
fn selected_shared_appearances_keep_placement_flags_and_content_modes() {
    let mut default_images = Vec::new();
    for matrix in [None, Some([2.0, 0.0, 0.0, 3.0, 5.0, 7.0])] {
        let (mut doc, ap, widget) = widget_fixture(None, matrix);
        // Optional Type does not replace the required Subtype.
        doc.update_dict(ap, |d| d.set(b"Type".to_vec(), Object::name("XObject"))).unwrap();
        let Object::Stream(mut off) = (*doc.get(ap)).clone() else { panic!("AP stream") };
        off.raw = Stream::flate(off.dict.clone(), b"/Ink cs 1 0 0 scn 0 0 20 10 re f").raw;
        let off = doc.add(Object::Stream(off));
        let mut states = Dict::new();
        states.set(b"On".to_vec(), Object::Ref(ap));
        states.set(b"Off".to_vec(), Object::Ref(off));
        let mut appearances = Dict::new();
        appearances.set(b"N".to_vec(), Object::Dict(states));
        doc.update_dict(widget, |d| {
            d.set(b"AP".to_vec(), Object::Dict(appearances));
            d.set(b"AS".to_vec(), Object::name("On"));
        })
        .unwrap();
        let mut widgets = vec![Object::Ref(widget)];
        for (rect, flags) in [([60.0, 20.0, 90.0, 40.0], 4), ([10.0, 60.0, 50.0, 80.0], 0), ([60.0, 60.0, 90.0, 80.0], 6)] {
            let mut copy = doc.get(widget).as_dict().unwrap().clone();
            copy.set(b"Rect".to_vec(), numbers(&rect));
            copy.set(b"F".to_vec(), Object::Int(flags));
            widgets.push(Object::Ref(doc.add(Object::Dict(copy))));
        }
        let page = pdfcraft_model::pages(&doc)[0].obj;
        doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(widgets))).unwrap();
        let before = saved(&doc);
        for (mode, content) in
            [Content::Document, Content::DocumentAndMarkups, Content::DocumentAndStamps, Content::FormFieldsOnly].into_iter().enumerate()
        {
            let output = impose(&doc, &print_settings(content)).unwrap();
            let reopened = Document::open(Arc::new(output.clone())).unwrap();
            assert_eq!(pdfcraft_model::pages(&reopened).len(), 1);
            let sheet = &pdfcraft_model::pages(&reopened)[0];
            let form = reopened.get(xobject(&reopened, &sheet.dict, b"P0"));
            assert_eq!(
                xobject(&reopened, form.as_dict().unwrap(), b"PCPrA0"),
                xobject(&reopened, form.as_dict().unwrap(), b"PCPrA1"),
                "selected shared appearance keeps one promotion"
            );
            let page = render(output);
            for (x, y, expected) in
                [(30, 70, [0, 255, 0, 255]), (75, 70, [0, 255, 0, 255]), (30, 30, [255, 255, 255, 255]), (75, 30, [255, 255, 255, 255])]
            {
                assert_eq!(pixel(&page, x, y), expected, "{matrix:?}, {content:?}, ({x},{y})");
            }
            let expected = if content == Content::FormFieldsOnly { [255, 255, 255, 255] } else { [0, 0, 255, 255] };
            assert_eq!(pixel(&page, 4, 96), expected, "document artwork under {content:?}");
            if matrix.is_none() {
                default_images.push(page.rgba);
            } else {
                assert_eq!(page.rgba, default_images[mode], "Matrix must preserve the complete appearance extent under {content:?}");
            }
        }
        assert_eq!(saved(&doc), before, "shared source graph stays unchanged");
    }
}

#[test]
fn shared_promotions_span_pages_without_merging_distinct_sources() {
    let mut doc = fixture(2);
    // Contributor-original drawing and a bounded PDF comment expose repeated encoded payloads.
    let mut payload = b"/Ink cs 0 1 0 scn 0 0 20 10 re f\n%".to_vec();
    payload.extend(std::iter::repeat_n(b'x', 32 * 1024));
    payload.push(b'\n');
    let mut appearances = Vec::new();
    for marker in ["Shared", "Distinct"] {
        let mut colors = Dict::new();
        colors.set(b"Ink".to_vec(), Object::name("DeviceRGB"));
        let mut resources = Dict::new();
        resources.set(b"ColorSpace".to_vec(), Object::Dict(colors));
        resources.set(b"PrivateMarker".to_vec(), Object::name(marker));
        let resources = doc.add(Object::Dict(resources));
        let mut dict = Dict::new();
        dict.set(b"BBox".to_vec(), numbers(&[0.0, 0.0, 20.0, 10.0]));
        dict.set(b"Resources".to_vec(), Object::Ref(resources));
        appearances.push(doc.add(Object::Stream(Stream::from_raw(dict, payload.clone()))));
    }
    let pages = pdfcraft_model::pages(&doc);
    for (index, page) in pages.iter().enumerate() {
        let mut widgets = Vec::new();
        for (ap, rect) in [(appearances[0], [10.0, 20.0, 50.0, 40.0]), (appearances[0], [60.0, 20.0, 90.0, 40.0])]
            .into_iter()
            .chain((index == 1).then_some((appearances[1], [10.0, 60.0, 50.0, 80.0])))
        {
            let mut widget = Dict::new();
            widget.set(b"Subtype".to_vec(), Object::name("Widget"));
            widget.set(b"FT".to_vec(), Object::name("Tx"));
            widget.set(b"F".to_vec(), Object::Int(4));
            widget.set(b"Rect".to_vec(), numbers(&rect));
            let mut appearance = Dict::new();
            appearance.set(b"N".to_vec(), Object::Ref(ap));
            widget.set(b"AP".to_vec(), Object::Dict(appearance));
            widgets.push(Object::Ref(doc.add(Object::Dict(widget))));
        }
        doc.update_dict(page.obj, |dict| {
            dict.set(b"MediaBox".to_vec(), numbers(&[0.0, 0.0, 100.0, 100.0]));
            dict.set(b"Annots".to_vec(), Object::Array(widgets));
            dict.remove(b"Contents");
            dict.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
        })
        .unwrap();
    }
    let before = saved(&doc);
    let state = doc.read_state();
    let source_bytes = doc.bytes().clone();
    let graph: Vec<_> = doc.scan_objects().collect();
    let originals: Vec<_> = appearances.iter().map(|ap| doc.get(*ap)).collect();
    let settings = Settings { pages: vec![0, 1], ..print_settings(Content::Document) };
    for _ in 0..2 {
        let output = impose(&doc, &settings).unwrap();
        let reopened = Document::open(Arc::new(output.clone())).unwrap();
        let sheets = pdfcraft_model::pages(&reopened);
        assert_eq!(sheets.len(), 2);
        let mut shared = Vec::new();
        let mut distinct = None;
        for (index, sheet) in sheets.iter().enumerate() {
            let form = reopened.get(xobject(&reopened, &sheet.dict, b"P0"));
            let form = form.as_dict().unwrap();
            shared.push(xobject(&reopened, form, b"PCPrA0"));
            shared.push(xobject(&reopened, form, b"PCPrA1"));
            if index == 1 {
                distinct = Some(xobject(&reopened, form, b"PCPrA2"));
            }
            let rendered = render_page(output.clone(), index);
            assert_eq!(pixel(&rendered, 30, 70), [0, 255, 0, 255]);
            assert_eq!(pixel(&rendered, 75, 70), [0, 255, 0, 255]);
            assert_eq!(pixel(&rendered, 30, 30), if index == 1 { [0, 255, 0, 255] } else { [255, 255, 255, 255] });
            assert_eq!(pixel(&rendered, 75, 30), [255, 255, 255, 255]);
            assert_eq!(pixel(&rendered, 5, 5), [255, 255, 255, 255]);
        }
        let copies = reopened
            .scan_objects()
            .filter(|(_, object)| matches!(&**object, Object::Stream(stream) if stream.raw.as_ref() == payload.as_slice()))
            .count();
        eprintln!(
            "shared promotion output: bytes={}, payload_bytes={}, copies={copies}, shared={shared:?}, distinct={distinct:?}",
            output.len(),
            payload.len()
        );
        assert!(state.matches(&doc));
        assert_eq!(doc.bytes(), &source_bytes);
        assert_eq!(doc.scan_objects().collect::<Vec<_>>(), graph);
        assert_eq!(saved(&doc), before);
        for (ap, original) in appearances.iter().zip(&originals) {
            assert_eq!(doc.get(*ap), *original);
        }
        assert_eq!(copies, 2, "one encoded payload per distinct source appearance");
        assert!(shared.iter().all(|ap| *ap == shared[0]), "shared uses span both page forms");
        let distinct = distinct.unwrap();
        assert_ne!(shared[0], distinct, "identical bytes do not merge source references");
        for (ap, marker) in [(shared[0], b"Shared".as_slice()), (distinct, b"Distinct".as_slice())] {
            let object = reopened.get(ap);
            let Object::Stream(stream) = &*object else { panic!("promoted appearance stream") };
            assert_eq!(stream.raw.as_ref(), payload.as_slice());
            assert_eq!(stream.dict.name(b"Subtype"), Some(b"Form".as_slice()));
            let resources = reopened.resolve(stream.dict.get(b"Resources").unwrap());
            assert_eq!(resources.as_dict().unwrap().name(b"PrivateMarker"), Some(marker));
        }
    }
}
