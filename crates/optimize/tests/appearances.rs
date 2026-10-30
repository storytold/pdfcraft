//! Contributor-original image shared by page content and annotation appearances.

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_optimize::{Compression, Settings, effective_resolutions, optimize};
use std::sync::Arc;

fn numbers(values: &[f64]) -> Object {
    Object::Array(values.iter().copied().map(Object::Real).collect())
}

fn fixture() -> (Document, ObjRef, ObjRef, ObjRef) {
    let mut doc = Document::new_empty();
    let pixels: Vec<u8> = (0..720).flat_map(|y| (0..720).map(move |x| if (x / 2 + y / 2) % 2 == 0 { 20 } else { 235 })).collect();
    let mut image = Dict::new();
    image.set(b"Type".to_vec(), Object::name("XObject"));
    image.set(b"Subtype".to_vec(), Object::name("Image"));
    image.set(b"Width".to_vec(), Object::Int(720));
    image.set(b"Height".to_vec(), Object::Int(720));
    image.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    image.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    let image = doc.add(Stream::from_raw(image, pixels));
    let mut xobjects = Dict::new();
    xobjects.set(b"Im".to_vec(), Object::Ref(image));
    let mut resources = Dict::new();
    resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
    let mut form = Dict::new();
    form.set(b"Type".to_vec(), Object::name("XObject"));
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), numbers(&[0.0, 0.0, 1.0, 1.0]));
    form.set(b"Resources".to_vec(), Object::Dict(resources.clone()));
    let form = doc.add(Stream::flate(form, b"/Im Do"));
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), Object::Ref(form));
    let mut annot = Dict::new();
    annot.set(b"Subtype".to_vec(), Object::name("Stamp"));
    annot.set(b"Rect".to_vec(), numbers(&[80.0, 80.0, 440.0, 440.0]));
    annot.set(b"F".to_vec(), Object::Int(4));
    annot.set(b"AP".to_vec(), Object::Dict(ap));
    let annot = doc.add(annot);
    let contents = doc.add(Stream::flate(Dict::new(), b"q 36 0 0 36 20 730 cm /Im Do Q"));
    let root = doc.root().unwrap();
    let pages = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), numbers(&[0.0, 0.0, 612.0, 792.0]));
    page.set(b"Contents".to_vec(), Object::Ref(contents));
    page.set(b"Resources".to_vec(), Object::Dict(resources));
    page.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(annot)]));
    let page = doc.add(page);
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    (doc, image, form, annot)
}

#[test]
fn appearance_use_keeps_shared_image_at_its_largest_size() {
    let (mut doc, image, _, _) = fixture();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    assert_eq!(effective_resolutions(&doc, &[page]).get(&image), Some(&144.0));
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    let mut settings = Settings::default();
    settings.gray.compression = Compression::Flate;
    let report = optimize(&mut doc, &settings).unwrap();
    assert_eq!(report.images_resampled, 0, "{report:?}");
    let after = write_full(&doc, &SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(after.clone())).unwrap();
    let page = pdfcraft_model::pages(&reopened)[0].obj;
    assert!(effective_resolutions(&reopened, &[page]).values().any(|ppi| *ppi == 144.0));
    let render = |bytes| {
        let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
        let page = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(page.error.is_none(), "{:?}", page.error);
        page.rgba
    };
    assert_eq!(render(before), render(after), "lossless optimization preserves the stamp pixels after save/reopen");
}

#[test]
fn all_appearance_states_and_physical_page_units_are_measured() {
    for kind in [b"N", b"R", b"D"] {
        for states in [false, true] {
            let (mut doc, image, form, annot) = fixture();
            let page = pdfcraft_model::pages(&doc)[0].obj;
            let value = if states {
                let mut options = Dict::new();
                let Object::Stream(mut small) = doc.get(form).as_ref().clone() else { panic!() };
                small.dict.set(b"BBox".to_vec(), numbers(&[0.0, 0.0, 10.0, 10.0]));
                let small = doc.add(small);
                options.set(b"A".to_vec(), Object::Ref(small));
                // /AS selects only the small-use A state; the inactive large state must count.
                options.set(b"Z".to_vec(), Object::Ref(form));
                Object::Ref(doc.add(options))
            } else {
                Object::Ref(form)
            };
            let mut ap = Dict::new();
            ap.set(kind.to_vec(), value);
            let ap = doc.add(ap);
            let rect = doc.add(numbers(&[440.0, 440.0, 80.0, 80.0]));
            doc.update_dict(annot, |d| {
                d.set(b"AP".to_vec(), Object::Ref(ap));
                d.set(b"Rect".to_vec(), Object::Ref(rect));
                d.set(b"AS".to_vec(), Object::name("A"));
            })
            .unwrap();
            let annots = doc.add(Object::Array(vec![Object::Ref(annot)]));
            doc.update_dict(page, |d| {
                d.set(b"Annots".to_vec(), Object::Ref(annots));
                d.set(b"UserUnit".to_vec(), Object::Int(2));
                d.remove(b"Contents");
            })
            .unwrap();
            assert_eq!(effective_resolutions(&doc, &[page]).get(&image), Some(&72.0), "{kind:?}, states={states}");
        }
    }
}

#[test]
fn appearance_matrix_is_included_once_and_page_resources_are_available() {
    let (mut doc, image, form, annot) = fixture();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let Object::Stream(mut appearance) = doc.get(form).as_ref().clone() else { panic!() };
    appearance.dict.set(b"BBox".to_vec(), numbers(&[10.0, 20.0, 11.0, 21.0]));
    // A 90-degree turn and scaling. The transformed bounding box is fitted to /Rect.
    appearance.dict.set(b"Matrix".to_vec(), numbers(&[0.0, 2.0, -3.0, 0.0, 60.0, -20.0]));
    appearance.dict.remove(b"Resources");
    appearance = Stream::flate(appearance.dict, b"q 1 0 0 1 10 20 cm /Im Do Q");
    doc.set(form, appearance);
    doc.update_dict(annot, |d| d.set(b"Rect".to_vec(), numbers(&[80.0, 80.0, 440.0, 260.0]))).unwrap();
    assert_eq!(effective_resolutions(&doc, &[page]).get(&image), Some(&144.0));
}

#[test]
fn unreadable_appearance_prevents_unsafe_downsampling() {
    for bad in [Object::Null, numbers(&[0.0, 0.0, 0.0, 1.0]), numbers(&[0.0, 0.0, 1.0]), numbers(&[0.0, 0.0, f64::INFINITY, 1.0])] {
        let (mut doc, image, form, _) = fixture();
        let page = pdfcraft_model::pages(&doc)[0].obj;
        let Object::Stream(mut appearance) = doc.get(form).as_ref().clone() else { panic!() };
        appearance.dict.set(b"BBox".to_vec(), bad);
        doc.set(form, appearance);
        assert_eq!(effective_resolutions(&doc, &[page]).get(&image), Some(&f64::INFINITY));
    }
}

#[test]
fn indirect_matrix_numbers_and_missing_subtype_keep_the_appearance_scale() {
    let (mut doc, image, form, _) = fixture();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let scale = doc.add(Object::Int(100));
    let matrix = doc.add(Object::Array(vec![Object::Ref(scale), 0.into(), 0.into(), Object::Ref(scale), 0.into(), 0.into()]));
    let Object::Stream(mut appearance) = doc.get(form).as_ref().clone() else { panic!() };
    appearance.dict.set(b"Matrix".to_vec(), Object::Ref(matrix));
    appearance.dict.remove(b"Subtype");
    doc.set(form, appearance);
    assert_eq!(effective_resolutions(&doc, &[page]).get(&image), Some(&144.0));
}

#[test]
fn resource_less_shared_appearances_use_each_pages_resources_and_units() {
    let (mut doc, first_image, form, _) = fixture();
    let first = pdfcraft_model::pages(&doc)[0].obj;
    let second_image = doc.add(doc.get(first_image).as_ref().clone());
    let Object::Stream(mut appearance) = doc.get(form).as_ref().clone() else { panic!() };
    appearance.dict.remove(b"Resources");
    doc.set(form, appearance);
    let mut second_page = doc.get(first).as_dict().unwrap().clone();
    let mut objects = Dict::new();
    objects.set(b"Im".to_vec(), Object::Ref(second_image));
    let mut resources = Dict::new();
    resources.set(b"XObject".to_vec(), Object::Dict(objects));
    second_page.set(b"Resources".to_vec(), Object::Dict(resources));
    second_page.set(b"UserUnit".to_vec(), Object::Int(2));
    second_page.remove(b"Contents");
    let second = doc.add(second_page);
    for pages in [[first, second], [second, first]] {
        let measured = effective_resolutions(&doc, &pages);
        assert_eq!(measured.get(&first_image), Some(&144.0));
        assert_eq!(measured.get(&second_image), Some(&72.0));
    }
}

#[test]
fn malformed_appearance_containers_and_failed_decodes_disable_downsampling() {
    for case in 0..5 {
        let (mut doc, image, form, annot) = fixture();
        let page = pdfcraft_model::pages(&doc)[0].obj;
        match case {
            0 => doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Int(1))).unwrap(),
            1 => doc.update_dict(annot, |d| d.set(b"AP".to_vec(), Object::name("invalid"))).unwrap(),
            2 => {
                let mut states = Dict::new();
                states.set(b"Bad".to_vec(), Object::Dict(Dict::new()));
                let mut ap = Dict::new();
                ap.set(b"N".to_vec(), Object::Dict(states));
                doc.update_dict(annot, |d| d.set(b"AP".to_vec(), Object::Dict(ap))).unwrap();
            }
            _ => {
                let Object::Stream(mut appearance) = doc.get(form).as_ref().clone() else { panic!() };
                if case == 3 {
                    appearance.dict.set(b"Filter".to_vec(), Object::name("UnknownDecode"));
                } else {
                    appearance.dict.set(b"Matrix".to_vec(), numbers(&[1e308, -1e308, 1e308, 1e308, 0.0, 0.0]));
                    appearance.dict.set(b"BBox".to_vec(), numbers(&[-1e308, -1e308, 1e308, 1e308]));
                }
                doc.set(form, appearance);
            }
        }
        assert_eq!(effective_resolutions(&doc, &[page]).get(&image), Some(&f64::INFINITY), "case {case}");
    }
}

#[test]
fn excessive_appearance_work_and_form_depth_disable_downsampling() {
    for deep in [false, true] {
        let (mut doc, image, form, annot) = fixture();
        let page = pdfcraft_model::pages(&doc)[0].obj;
        if deep {
            let mut inner = form;
            for _ in 0..14 {
                let Object::Stream(mut outer) = doc.get(form).as_ref().clone() else { panic!() };
                let mut objects = Dict::new();
                objects.set(b"Inner".to_vec(), Object::Ref(inner));
                let mut resources = Dict::new();
                resources.set(b"XObject".to_vec(), Object::Dict(objects));
                outer.dict.set(b"Resources".to_vec(), Object::Dict(resources));
                inner = doc.add(Stream::flate(outer.dict, b"/Inner Do"));
            }
            let mut ap = Dict::new();
            ap.set(b"N".to_vec(), Object::Ref(inner));
            doc.update_dict(annot, |d| d.set(b"AP".to_vec(), Object::Dict(ap))).unwrap();
        } else {
            doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(annot); 100_001]))).unwrap();
        }
        assert_eq!(effective_resolutions(&doc, &[page]).get(&image), Some(&f64::INFINITY), "deep={deep}");
    }
}

#[test]
fn small_appearance_only_images_are_still_downsampled() {
    let (mut doc, image, _, annot) = fixture();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| {
        d.remove(b"Contents");
    })
    .unwrap();
    doc.update_dict(annot, |d| d.set(b"Rect".to_vec(), numbers(&[80.0, 80.0, 116.0, 116.0]))).unwrap();
    let mut settings = Settings::default();
    settings.gray.compression = Compression::Flate;
    let report = optimize(&mut doc, &settings).unwrap();
    assert_eq!(report.images_resampled, 1);
    let Object::Stream(image) = doc.get(image).as_ref().clone() else { panic!() };
    assert_eq!(image.dict.int(b"Width"), Some(75));
}

#[test]
fn annotations_without_appearances_do_not_disable_page_downsampling() {
    let (mut doc, _, _, annot) = fixture();
    doc.update_dict(annot, |d| {
        d.remove(b"AP");
    })
    .unwrap();
    let mut settings = Settings::default();
    settings.gray.compression = Compression::Flate;
    assert_eq!(optimize(&mut doc, &settings).unwrap().images_resampled, 1);
}

#[test]
fn zero_area_signature_appearances_do_not_block_downsampling() {
    for rect in [[0.0, 0.0, 0.0, 0.0], [80.0, 80.0, 80.0, 440.0], [80.0, 80.0, 440.0, 80.0]] {
        for empty_bbox in [false, true] {
            let (mut doc, image, form, annot) = fixture();
            let page = pdfcraft_model::pages(&doc)[0].obj;
            let root = doc.root().unwrap();
            let mut fields = Dict::new();
            fields.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(annot)]));
            doc.update_dict(root, |d| d.set(b"AcroForm".to_vec(), Object::Dict(fields))).unwrap();
            doc.update_dict(annot, |d| {
                d.set(b"Subtype".to_vec(), Object::name("Widget"));
                d.set(b"FT".to_vec(), Object::name("Sig"));
                d.set(b"Rect".to_vec(), numbers(&rect));
            })
            .unwrap();
            if empty_bbox {
                let Object::Stream(mut appearance) = doc.get(form).as_ref().clone() else { panic!() };
                appearance.dict.set(b"BBox".to_vec(), numbers(&[0.0, 0.0, 0.0, 0.0]));
                doc.set(form, appearance);
            }
            assert_eq!(effective_resolutions(&doc, &[page]).get(&image), Some(&1440.0), "rect={rect:?}, empty_bbox={empty_bbox}");
            let mut settings = Settings::default();
            settings.gray.compression = Compression::Flate;
            assert_eq!(optimize(&mut doc, &settings).unwrap().images_resampled, 1);
            let Object::Stream(image) = doc.get(image).as_ref().clone() else { panic!() };
            assert_eq!(image.dict.int(b"Width"), Some(75));
        }
    }
}

#[test]
fn invalid_appearance_rectangles_still_disable_downsampling() {
    for rect in [Object::Null, numbers(&[0.0, 0.0, 1.0]), numbers(&[0.0, 0.0, f64::INFINITY, 1.0]), numbers(&[0.0, 0.0, f64::NAN, 1.0])] {
        let (mut doc, image, _, annot) = fixture();
        let page = pdfcraft_model::pages(&doc)[0].obj;
        doc.update_dict(annot, |d| d.set(b"Rect".to_vec(), rect)).unwrap();
        assert_eq!(effective_resolutions(&doc, &[page]).get(&image), Some(&f64::INFINITY));
    }
    let (mut doc, image, _, annot) = fixture();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(annot, |d| {
        d.remove(b"Rect");
    })
    .unwrap();
    assert_eq!(effective_resolutions(&doc, &[page]).get(&image), Some(&f64::INFINITY));
}
