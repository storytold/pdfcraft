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
    render_page_at_size(bytes, index, (100, 100))
}

/// Explicit raster dimensions expose `/UserUnit` and sheet scaling without a default-size render hiding them.
fn render_page_at_size(bytes: Vec<u8>, index: usize, size: (u32, u32)) -> RenderedPage {
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
    assert_eq!((page.width, page.height), size);
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

/// Printing keeps occupied page resource names and distinct annotation appearances intact,
/// whether the page resources are direct or inherited from the page tree.
#[test]
fn annotation_appearances_do_not_replace_page_xobjects() {
    for inherited in [false, true] {
        let (mut doc, ap, widget) = widget_fixture(Some(Object::name("Form")), None);
        let page = pdfcraft_model::pages(&doc)[0].obj;
        let mut xobjects = Dict::new();
        for (name, color) in [("PCPrA0", "1 0 0"), ("PCPrA1", "0 0 1"), ("PCPrA3", "1 0 0")] {
            let dict = Dict::from_iter([
                (b"Type".to_vec(), Object::name("XObject")),
                (b"Subtype".to_vec(), Object::name("Form")),
                (b"BBox".to_vec(), numbers(&[0.0, 0.0, 10.0, 10.0])),
            ]);
            let form = doc.add(Object::Stream(Stream::flate(dict, format!("{color} rg 0 0 10 10 re f").as_bytes())));
            xobjects.set(name.as_bytes().to_vec(), Object::Ref(form));
        }
        let xobjects = doc.add(Object::Dict(xobjects));
        let resources = doc.add(Object::Dict(Dict::from_iter([(b"XObject".to_vec(), Object::Ref(xobjects))])));
        let contents = doc.add(Object::Stream(Stream::flate(
            Dict::new(),
            b"q 1 0 0 1 60 20 cm /PCPrA0 Do Q\nq 1 0 0 1 70 20 cm /PCPrA1 Do Q\nq 1 0 0 1 80 20 cm /PCPrA3 Do Q",
        )));
        let second_ap = doc.add(Object::Stream(Stream::flate(doc.get(ap).as_dict().unwrap().clone(), b"/Ink cs 1 1 0 scn 0 0 20 10 re f")));
        let mut second_widget = doc.get(widget).as_dict().unwrap().clone();
        second_widget.set(b"Rect".to_vec(), numbers(&[10.0, 50.0, 50.0, 70.0]));
        second_widget.set(b"AP".to_vec(), Object::Dict(Dict::from_iter([(b"N".to_vec(), Object::Ref(second_ap))])));
        let second_widget = doc.add(Object::Dict(second_widget));
        doc.update_dict(page, |d| {
            d.set(b"Resources".to_vec(), Object::Ref(resources));
            d.set(b"Contents".to_vec(), Object::Ref(contents));
            d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(widget), Object::Ref(second_widget)]));
        })
        .unwrap();
        if inherited {
            let parent = doc.get(page).as_dict().unwrap().reference(b"Parent").unwrap();
            doc.update_dict(parent, |d| d.set(b"Resources".to_vec(), Object::Ref(resources))).unwrap();
            doc.update_dict(page, |d| {
                d.remove(b"Resources");
            })
            .unwrap();
        }
        let before = saved(&doc);
        let source = render(before.clone());
        for (x, color) in [(65, [255, 0, 0, 255]), (75, [0, 0, 255, 255]), (85, [255, 0, 0, 255])] {
            assert_eq!(pixel(&source, x, 75), color, "source page artwork");
        }
        for (y, color) in [(40, [255, 255, 0, 255]), (70, [0, 255, 0, 255])] {
            assert_eq!(pixel(&source, 30, y), color, "source widget appearance");
        }
        let printed = render(impose(&doc, &print_settings(Content::Document)).unwrap());
        for (x, color) in [(65, [255, 0, 0, 255]), (75, [0, 0, 255, 255]), (85, [255, 0, 0, 255])] {
            assert_eq!(pixel(&printed, x, 75), color, "printed page artwork, inherited={inherited}");
        }
        for (y, color) in [(40, [255, 255, 0, 255]), (70, [0, 255, 0, 255])] {
            assert_eq!(pixel(&printed, 30, y), color, "printed widget appearance, inherited={inherited}");
        }
        assert_eq!(printed.rgba, source.rgba, "printing must retain page artwork and both appearances, inherited={inherited}");
        assert_eq!(saved(&doc), before, "printing must leave the source resources unchanged");
    }
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

/// Appearance coordinates are independent of page units: a large BBox or Matrix can require
/// placement coefficients far below four decimal places even on an ordinary 100-point page.
#[test]
fn annotation_placement_preserves_small_transform_coefficients() {
    let (control, _, _) = widget_fixture(Some(Object::name("Form")), None);
    let control = render(saved(&control));
    for scale in [1.0, 4096.0, 75000.0] {
        for scaled_matrix in [false, true] {
            let (mut doc, ap, widget) = widget_fixture(Some(Object::name("Form")), None);
            let mut form = doc.get(ap).as_dict().unwrap().clone();
            let coordinates = if scaled_matrix { 1.0 } else { scale };
            form.set(b"BBox".to_vec(), numbers(&[0.0, 0.0, 20.0 * coordinates, 10.0 * coordinates]));
            if scaled_matrix {
                form.set(b"Matrix".to_vec(), numbers(&[scale, 0.0, 0.0, scale, 7.0, -13.0]));
            }
            doc.set(
                ap,
                Object::Stream(Stream::flate(form, format!("/Ink cs 0 1 0 scn 0 0 {} {} re f", 20.0 * coordinates, 10.0 * coordinates).as_bytes())),
            );
            for (subtype, content) in [
                ("Widget", Content::Document),
                ("Widget", Content::FormFieldsOnly),
                ("Square", Content::DocumentAndMarkups),
                ("Stamp", Content::DocumentAndStamps),
            ] {
                doc.update_dict(widget, |d| d.set(b"Subtype".to_vec(), Object::name(subtype))).unwrap();
                let before = saved(&doc);
                let source = render(before.clone());
                assert!(source.rgba == control.rgba, "equivalent source, scale={scale}, matrix={scaled_matrix}, {subtype}");
                let printed = impose(&doc, &print_settings(content)).unwrap();
                let reopened = Document::open(Arc::new(printed.clone())).unwrap();
                assert_eq!(pdfcraft_model::pages(&reopened).len(), 1);
                let printed = render(printed);
                assert_eq!(pixel(&printed, 30, 70), [0, 255, 0, 255], "scale={scale}, matrix={scaled_matrix}, {subtype}");
                if content != Content::FormFieldsOnly {
                    assert!(printed.rgba == source.rgba, "scale={scale}, matrix={scaled_matrix}, {subtype}");
                }
                // Check the serialized placement too: sub-pixel translation rounding may be
                // invisible at 72 dpi while still moving the appearance's physical corners.
                let page = &pdfcraft_model::pages(&reopened)[0];
                let page_form = reopened.get(xobject(&reopened, &page.dict, b"P0"));
                let Object::Stream(page_form) = &*page_form else { panic!("page form") };
                let parsed = pdfcraft_content::parse(&page_form.decoded().unwrap());
                assert_eq!(parsed.skipped, 0);
                let matrices: Vec<_> = parsed.ops.iter().filter(|op| op.is("cm")).collect();
                assert_eq!(matrices.len(), 1);
                let placement = Matrix(matrices[0].nums::<6>().unwrap());
                let (x, y) = if scaled_matrix { (7.0, -13.0) } else { (0.0, 0.0) };
                for ((x, y), expected) in [(placement.apply(x, y), (10.0, 20.0)), (placement.apply(x + 20.0 * scale, y + 10.0 * scale), (50.0, 40.0))]
                {
                    assert!((x - expected.0).abs() < 1e-10 && (y - expected.1).abs() < 1e-10, "corner ({x}, {y}) != {expected:?}");
                }
                assert_eq!(saved(&doc), before, "printing preserves the source");
            }
        }
    }
}

/// Compare bad-geometry output with a no-annotation control to ensure page artwork remains visible.
#[test]
fn unusable_appearance_geometry_keeps_page_artwork() {
    let (mut control, _, _) = widget_fixture(Some(Object::name("Form")), None);
    let page = pdfcraft_model::pages(&control)[0].obj;
    control
        .update_dict(page, |d| {
            d.remove(b"Annots");
        })
        .unwrap();
    let control = render(saved(&control));
    for (bbox, matrix) in [
        (None, None),
        (Some(numbers(&[0.0, 0.0, 20.0])), None),
        (Some(numbers(&[0.0, 0.0, 0.0, 10.0])), None),
        (Some(numbers(&[0.0, 0.0, 20.0, 0.0])), None),
        (Some(numbers(&[0.0, 0.0, 20.0, 10.0])), Some([0.0; 6])),
    ] {
        let (mut doc, ap, _) = widget_fixture(Some(Object::name("Form")), matrix);
        let mut form = doc.get(ap).as_dict().unwrap().clone();
        match bbox {
            Some(bbox) => form.set(b"BBox".to_vec(), bbox),
            None => {
                form.remove(b"BBox");
            }
        }
        doc.set(ap, Object::Stream(Stream::flate(form, b"/Ink cs 0 1 0 scn 0 0 20 10 re f")));
        let before = saved(&doc);
        let printed = render(impose(&doc, &print_settings(Content::Document)).unwrap());
        assert!(printed.rgba == control.rgba, "unusable appearance is omitted without losing page artwork");
        assert_eq!(saved(&doc), before);
    }
}

#[test]
fn promotion_preserves_streams_and_explicit_subtypes() {
    for subtype in [None, Some(Object::name("Form")), Some(Object::name("Image")), Some(Object::Null), Some(Object::Int(7))] {
        let (mut doc, ap, _) = widget_fixture(subtype.clone(), Some([2.0, 0.0, 0.0, 3.0, 5.0, 7.0]));
        let original = doc.get(ap);
        let pages = pdfcraft_model::pages(&doc);
        let form = page_form(&mut doc, &pages, 0, Content::Document, &mut std::collections::HashMap::new()).unwrap();
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

const CORNER_COLORS: [[u8; 4]; 4] = [[255, 0, 0, 255], [0, 0, 255, 255], [0, 255, 0, 255], [255, 255, 0, 255]];

// Contributor-original, asymmetric 20 x 10 artwork: red/blue above green/yellow.
/// Shift `/CropBox` and `/Rect` together so the colored, asymmetric artwork exposes origin and rotation errors.
fn no_rotate_fixture(rotation: Object, origin: [f64; 2], subtype: &str, flags: i64) -> (Document, ObjRef, ObjRef) {
    let (mut doc, ap, annotation) = widget_fixture(Some(Object::name("Form")), None);
    let form = doc.get(ap).as_dict().unwrap().clone();
    doc.set(
        ap,
        Object::Stream(Stream::flate(form, b"0 1 0 rg 0 0 20 10 re f 1 0 0 rg 0 5 10 5 re f 0 0 1 rg 10 5 10 5 re f 1 1 0 rg 10 0 10 5 re f")),
    );
    let [x, y] = origin;
    doc.update_dict(annotation, |d| {
        d.set(b"Subtype".to_vec(), Object::name(subtype));
        d.set(b"F".to_vec(), Object::Int(flags));
        d.set(b"Rect".to_vec(), numbers(&[x + 40.0, y + 40.0, x + 60.0, y + 50.0]));
    })
    .unwrap();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| {
        d.set(b"MediaBox".to_vec(), numbers(&[x - 20.0, y - 20.0, x + 120.0, y + 120.0]));
        d.set(b"CropBox".to_vec(), numbers(&[x, y, x + 100.0, y + 100.0]));
        d.set(b"Rotate".to_vec(), rotation);
        d.remove(b"Contents");
    })
    .unwrap();
    (doc, ap, annotation)
}

/// Colored quadrant samples distinguish every quarter-turn; white margins expose extent or anchor errors.
fn assert_upright_corners(page: &RenderedPage, anchor: (usize, usize), scale: usize) {
    let (x, y) = anchor;
    for ((dx, dy), color) in [(5, 2), (15, 2), (5, 7), (15, 7)].into_iter().zip(CORNER_COLORS) {
        assert_eq!(pixel(page, x + dx * scale, y + dy * scale), color, "upright corner at {anchor:?}, scale={scale}");
    }
    for (px, py) in [(x - 2, y + 5 * scale), (x + 22 * scale, y + 5 * scale), (x + 10 * scale, y - 2), (x + 10 * scale, y + 12 * scale)] {
        assert_eq!(pixel(page, px, py), [255, 255, 255, 255], "outside the upright appearance at ({px},{py})");
    }
}

/// Literal anchors and colors test pivot, orientation, and size without copying production math.
/// Direct and indirect `Text` subtypes with `/F 4` exercise implicit `NoRotate` without `/F 16`.
/// Unflagged non-`Text` cases retain the rotating control.
#[test]
fn vector_print_preserves_no_rotate_appearance_anchors() {
    for (rotation, anchor, control) in [
        (0, (40, 50), [(45, 52), (55, 52), (45, 57), (55, 57)]),
        (90, (50, 40), [(47, 45), (47, 55), (42, 45), (42, 55)]),
        (180, (60, 50), [(55, 47), (45, 47), (55, 42), (45, 42)]),
        (270, (50, 60), [(52, 55), (52, 45), (57, 55), (57, 45)]),
    ] {
        for (subtype, reversed, indirect_subtype) in [
            ("Widget", false, false),
            ("Square", false, false),
            ("Square", true, false),
            ("Stamp", false, false),
            ("Text", false, false),
            ("Text", false, true),
        ] {
            for flags in [4, 20] {
                let (mut doc, ap, annotation) = no_rotate_fixture(Object::Int(rotation), [0.0, 0.0], subtype, flags);
                if indirect_subtype {
                    let name = doc.add(Object::name(subtype));
                    doc.update_dict(annotation, |d| d.set(b"Subtype".to_vec(), Object::Ref(name))).unwrap();
                }
                if reversed {
                    doc.update_dict(annotation, |d| d.set(b"Rect".to_vec(), numbers(&[60.0, 50.0, 40.0, 40.0]))).unwrap();
                }
                let before = saved(&doc);
                let original = doc.get(ap);
                let settings = print_settings(Content::DocumentAndMarkups);
                let first = impose(&doc, &settings).unwrap();
                let reopened = Document::open(Arc::new(first.clone())).unwrap();
                for bytes in [first, impose(&reopened, &settings).unwrap()] {
                    let page = render(bytes);
                    if flags == 20 || subtype == "Text" {
                        assert_upright_corners(&page, anchor, 1);
                    } else {
                        for ((x, y), color) in control.into_iter().zip(CORNER_COLORS) {
                            assert_eq!(pixel(&page, x, y), color, "control: rotation={rotation}, subtype={subtype}");
                        }
                    }
                }
                assert_eq!(doc.get(ap), original, "shared appearance bytes/dictionary stay unchanged");
                assert_eq!(saved(&doc), before, "rotation={rotation}, subtype={subtype}, flags={flags}");
            }
        }
    }
}

/// Compare output pixel-for-pixel with the source shifted to its `/Rect` anchor.
/// This also verifies that the exposed page area remains white.
fn assert_translated_upright_source(actual: &RenderedPage, source: &RenderedPage, anchor: (i32, i32)) {
    let (dx, dy) = (anchor.0 - 40, anchor.1 - 50);
    for y in 0..100i32 {
        for x in 0..100i32 {
            let (sx, sy) = (x - dx, y - dy);
            let expected = if (0..100).contains(&sx) && (0..100).contains(&sy) { pixel(source, sx as usize, sy as usize) } else { &[255; 4] };
            let actual = pixel(actual, x as usize, y as usize);
            assert_eq!(actual, expected, "translated source at ({x},{y})");
        }
    }
}

/// Compare print output with a rotation-zero source under shifted `/BBox` and `/Matrix` transforms.
/// This also covers inherited and normalized `/Rotate` values.
#[test]
fn no_rotate_keeps_appearance_matrix_and_shifted_inherited_page_geometry() {
    for (rotation, inherited, origin, anchor) in
        [(450, true, [-20.0, 30.0], (50, 40)), (-180, false, [15.0, -25.0], (60, 50)), (-90, true, [-20.0, 30.0], (50, 60))]
    {
        for (matrix, shifted_bbox) in
            [([2.0, 0.0, 0.0, 3.0, 7.0, -11.0], true), ([1.0, 0.0, 1.0, 1.0, 7.0, -11.0], false), ([-1.0, 0.0, 0.0, 1.0, 27.0, -11.0], false)]
        {
            let (mut doc, ap, _) = no_rotate_fixture(Object::Int(rotation), origin, "Widget", 20);
            doc.update_dict(ap, |d| {
                d.set(b"Matrix".to_vec(), numbers(&matrix));
                if shifted_bbox {
                    d.set(b"BBox".to_vec(), numbers(&[5.0, -4.0, 25.0, 6.0]));
                }
            })
            .unwrap();
            if shifted_bbox {
                let form = doc.get(ap).as_dict().unwrap().clone();
                doc.set(
                    ap,
                    Object::Stream(Stream::flate(
                        form,
                        b"q 1 0 0 1 5 -4 cm 0 1 0 rg 0 0 20 10 re f 1 0 0 rg 0 5 10 5 re f 0 0 1 rg 10 5 10 5 re f 1 1 0 rg 10 0 10 5 re f Q",
                    )),
                );
            }
            let page = pdfcraft_model::pages(&doc)[0].obj;
            if inherited {
                let parent = doc.get(page).as_dict().unwrap().reference(b"Parent").unwrap();
                let rotation = Object::Ref(doc.add(Object::Int(rotation)));
                doc.update_dict(parent, |d| d.set(b"Rotate".to_vec(), rotation)).unwrap();
                doc.update_dict(page, |d| {
                    d.remove(b"Rotate");
                })
                .unwrap();
            }
            let mut upright = doc.clone();
            upright.update_dict(page, |d| d.set(b"Rotate".to_vec(), Object::Int(0))).unwrap();
            let expected = render(saved(&upright));
            // Establish that the reference contains real asymmetric artwork, rather than
            // accepting two blank or uniformly colored renderings as equivalent.
            for color in CORNER_COLORS {
                assert!(expected.rgba.as_chunks::<4>().0.contains(&color));
            }
            let before = saved(&doc);
            let original = doc.get(ap);
            let settings = print_settings(Content::Document);
            let printed = impose(&doc, &settings).unwrap();
            let reopened = Document::open(Arc::new(printed.clone())).unwrap();
            assert_translated_upright_source(&render(printed), &expected, anchor);
            assert_translated_upright_source(&render(impose(&reopened, &settings).unwrap()), &expected, anchor);
            assert_eq!(doc.get(ap), original);
            assert_eq!(saved(&doc), before, "rotation={rotation}, inherited={inherited}, origin={origin:?}, matrix={matrix:?}");
        }
    }
}

/// Invalid `/Rotate` values normalize to zero while the eligible appearance remains upright.
#[test]
fn malformed_page_rotation_keeps_no_rotate_appearance_upright() {
    for rotation in [Object::Int(135), Object::Int(-45), Object::Real(90.5), Object::name("QuarterTurn")] {
        let (doc, _, _) = no_rotate_fixture(rotation.clone(), [-20.0, 30.0], "Widget", 20);
        assert_eq!(pdfcraft_model::pages(&doc)[0].rotation(&doc), 0);
        let before = saved(&doc);
        let printed = render(impose(&doc, &print_settings(Content::Document)).unwrap());
        assert_upright_corners(&printed, (40, 50), 1);
        assert_eq!(saved(&doc), before, "malformed Rotate={rotation:?}");
    }
}

/// `NoZoom` does not cancel print layout scaling across `UserUnit` and custom sheet sizes.
#[test]
fn no_rotate_keeps_user_units_and_sheet_scaling() {
    let (mut doc, _, annotation) = no_rotate_fixture(Object::Int(90), [-20.0, 30.0], "Widget", 28);
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"UserUnit".to_vec(), Object::Int(2))).unwrap();
    // Resolve the same flags as the existing print path; NoZoom does not cancel print scale.
    let flags = doc.add(Object::Int(28));
    doc.update_dict(annotation, |d| d.set(b"F".to_vec(), Object::Ref(flags))).unwrap();
    let before = saved(&doc);
    for (paper, layout, size, anchor, scale) in [
        ((200.0, 200.0), Layout::Size(SizeMode::Actual), (200, 200), (100, 80), 2),
        ((100.0, 100.0), Layout::Size(SizeMode::Custom(50.0)), (100, 100), (50, 40), 1),
    ] {
        let settings = Settings { paper, layout, ..print_settings(Content::Document) };
        let printed = render_page_at_size(impose(&doc, &settings).unwrap(), 0, size);
        assert_upright_corners(&printed, anchor, scale);
    }
    assert_eq!(saved(&doc), before);
}

/// Counterrotating one use must not alter the shared stream or rotate its later unflagged use.
#[test]
fn no_rotate_is_per_invocation_when_appearances_are_shared() {
    let (mut doc, ap, annotation) = no_rotate_fixture(Object::Int(90), [0.0, 0.0], "Widget", 20);
    let mut control = doc.get(annotation).as_dict().unwrap().clone();
    control.set(b"F".to_vec(), Object::Int(4));
    control.set(b"Rect".to_vec(), numbers(&[10.0, 10.0, 30.0, 20.0]));
    let control = doc.add(Object::Dict(control));
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(annotation), Object::Ref(control)]))).unwrap();
    let before = saved(&doc);
    let original = doc.get(ap);
    let output = impose(&doc, &print_settings(Content::Document)).unwrap();
    let reopened = Document::open(Arc::new(output.clone())).unwrap();
    let rendered = render(output);
    assert_upright_corners(&rendered, (50, 40), 1);
    for ((x, y), color) in [(17, 15), (17, 25), (12, 15), (12, 25)].into_iter().zip(CORNER_COLORS) {
        assert_eq!(pixel(&rendered, x, y), color, "unflagged use of the shared appearance");
    }
    let sheet = &pdfcraft_model::pages(&reopened)[0];
    let form = reopened.get(xobject(&reopened, &sheet.dict, b"P0"));
    assert_eq!(xobject(&reopened, form.as_dict().unwrap(), b"PCPrA0"), xobject(&reopened, form.as_dict().unwrap(), b"PCPrA1"));
    assert_eq!(doc.get(ap), original);
    assert_eq!(saved(&doc), before);
}

/// Refuse non-finite `NoRotate` placement without changing the source.
/// Excluded and degenerate appearances retain their omission behavior.
#[test]
fn no_rotate_refuses_nonfinite_placement_and_still_skips_degenerate_appearances() {
    for (rotation, rect) in [(90, [1e308, 0.0, 1.7e308, 1e308]), (180, [1e308, 0.0, 1.7e308, 1e308]), (270, [-1e308, 0.0, -0.3e308, 1e308])] {
        for (subtype, flags) in [("Widget", 20), ("Text", 4)] {
            let (mut doc, ap, annotation) = no_rotate_fixture(Object::Int(rotation), [0.0, 0.0], subtype, flags);
            doc.update_dict(annotation, |d| d.set(b"Rect".to_vec(), numbers(&rect))).unwrap();
            let before = saved(&doc);
            let original = doc.get(ap);
            assert!(
                matches!(impose(&doc, &print_settings(Content::DocumentAndMarkups)), Err(PrintError::Invalid(_))),
                "rotation={rotation}, subtype={subtype}, flags={flags}: unrepresentable placement"
            );
            assert_eq!(doc.get(ap), original);
            assert_eq!(saved(&doc), before);
            let mut excluded = doc.clone();
            for flags in [16, 22] {
                excluded.update_dict(annotation, |d| d.set(b"F".to_vec(), Object::Int(flags))).unwrap();
                let printed = render(impose(&excluded, &print_settings(Content::DocumentAndMarkups)).unwrap());
                assert!(printed.rgba.as_chunks::<4>().0.iter().all(|pixel| *pixel == [255, 255, 255, 255]), "excluded malformed appearance");
            }
        }
    }
    let (mut doc, ap, _) = no_rotate_fixture(Object::Int(90), [0.0, 0.0], "Widget", 20);
    doc.update_dict(ap, |d| d.set(b"BBox".to_vec(), numbers(&[0.0, 0.0, 0.0, 10.0]))).unwrap();
    let before = saved(&doc);
    let printed = render(impose(&doc, &print_settings(Content::Document)).unwrap());
    assert!(
        printed.rgba.as_chunks::<4>().0.iter().all(|pixel| *pixel == [255, 255, 255, 255]),
        "degenerate appearance keeps its existing omission behavior"
    );
    assert_eq!(saved(&doc), before);
}
