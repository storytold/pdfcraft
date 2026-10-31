//! Optional-content regression fixtures for issue #3213.
use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, write_incremental};

/// Build a single-page PDF fixture with a hidden OCG (`4 0 R`), a shown OCG (`5 0 R`),
/// a form appearance (`7 0 R`), an annotation (`8 0 R`), and an OCMD (`9 0 R`).
fn fixture(membership: &str, flags: i64, appearance_membership: &str) -> Document {
    let ink = "1 0 0 rg 0 0 100 50 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [4 0 R 5 0 R] /D << /OFF [4 0 R] /ON [5 0 R] /Order [4 0 R 5 0 R] >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Properties << /Existing 5 0 R /PCFlOC0 5 0 R /Private (keep) >> >> /Contents 6 0 R /Annots [8 0 R] >>".into(),
        "<< /Type /OCG /Name (Hidden layer) /Private (keep layer) >>".into(),
        "<< /Type /OCG /Name (Shown layer) >>".into(),
        "<< /Length 0 >>\nstream\n\nendstream".into(),
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 100 50] {appearance_membership} /Length {} >>\nstream\n{ink}\nendstream", ink.len()),
        format!("<< /Type /Annot /Subtype /Square /Rect [50 50 150 100] /C [1 0 0] /AP << /N 7 0 R >> /F {flags} {membership} /Private (keep annotation) >>"),
        "<< /Type /OCMD /OCGs [4 0 R] /P /AllOn /Private (keep membership) >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Root 1 0 R /Size {} >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

/// Serialize `doc` using an incremental save.
fn saved(doc: &Document) -> Vec<u8> {
    write_incremental(doc, &SaveOptions::default()).unwrap()
}

/// Optionally export test PDF bytes when `PDFCRAFT_3213_ARTIFACTS` is set.
fn export(name: &str, stage: &str, bytes: &[u8]) {
    if let Some(directory) = std::env::var_os("PDFCRAFT_3213_ARTIFACTS") {
        std::fs::write(std::path::Path::new(&directory).join(format!("{name}-{stage}.pdf")), bytes).unwrap();
    }
}

/// Verify that transferable optional-content memberships are bound to flattened appearances
/// after saving and reopening, while hidden-flag annotations remain untouched.
#[test]
fn flatten_keeps_annotation_optional_content_after_save_and_reopen() {
    let mut lost = Vec::new();
    for (name, membership, flags, ap_oc) in [
        ("off", "/OC 4 0 R", 4, ""),
        ("on", "/OC 5 0 R", 4, ""),
        ("ocmd", "/OC 9 0 R", 4, ""),
        ("independent_ap", "/OC 4 0 R", 4, "/OC 5 0 R"),
        ("direct_ocmd", "/OC << /Type /OCMD /OCGs [4 0 R] /P /AllOn /Private (keep inline) >>", 4, ""),
        ("ocmd_ve", "/OC << /Type /OCMD /VE [/And 4 0 R [/Not 5 0 R]] /Private (keep expression) >>", 4, ""),
        ("visible", "", 4, ""),
        ("hidden_flag", "", 2, ""),
    ] {
        let mut doc = fixture(membership, flags, ap_oc);
        let original_appearance = (*doc.get(ObjRef::new(7, 0))).clone();
        let expected_membership = doc.get(pdfcraft_cos::ObjRef::new(8, 0)).as_dict().unwrap().get(b"OC").cloned();
        export(name, "before", &saved(&doc));
        let count = pdfcraft_edit::flatten(&mut doc, &[0], true, false).unwrap();
        let bytes = saved(&doc);
        export(name, "flat", &bytes);
        let reopened = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(count, usize::from(flags != 2), "{name}");
        let paints = paint_memberships(&reopened);
        if flags != 2 && paints != vec![(ObjRef::new(7, 0), expected_membership)] {
            lost.push(name);
        }
        assert_eq!(*reopened.get(ObjRef::new(7, 0)), original_appearance, "{name}: preserve appearance");
    }
    assert!(lost.is_empty(), "flattened paint loses its original membership: {lost:?}");
}

/// Verify that multiple annotations sharing one appearance stream keep separate memberships
/// without mutating the shared appearance stream itself.
#[test]
fn flatten_shared_appearance_keeps_separate_memberships_and_original_stream() {
    let mut doc = fixture("/OC 4 0 R", 4, "");
    let first = ObjRef::new(8, 0);
    let ap = ObjRef::new(7, 0);
    let original_ap = (*doc.get(ap)).clone();
    let mut second = doc.get(first).as_dict().unwrap().clone();
    second.set(b"OC".to_vec(), Object::Ref(ObjRef::new(5, 0)));
    second.set(b"Rect".to_vec(), Object::Array([50, 120, 150, 170].into_iter().map(Object::Int).collect()));
    let second = doc.add(Object::Dict(second));
    let mut plain = doc.get(first).as_dict().unwrap().clone();
    plain.remove(b"OC");
    plain.set(b"Rect".to_vec(), Object::Array([0, 0, 20, 10].into_iter().map(Object::Int).collect()));
    let plain = doc.add(Object::Dict(plain));
    doc.update_dict(ObjRef::new(3, 0), |page| {
        page.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(first), Object::Ref(second), Object::Ref(plain)]))
    })
    .unwrap();
    export("shared", "before", &saved(&doc));
    assert_eq!(pdfcraft_edit::flatten(&mut doc, &[0], true, false).unwrap(), 3);
    let bytes = saved(&doc);
    export("shared", "flat", &bytes);
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(*doc.get(ap), original_ap, "a shared appearance must never acquire either annotation's /OC");
    assert_eq!(paint_memberships(&doc), vec![(ap, Some(Object::Ref(ObjRef::new(4, 0)))), (ap, Some(Object::Ref(ObjRef::new(5, 0)))), (ap, None)]);
}

/// Associate each `Do` invocation with its active `/OC` marked-content block.
fn paint_memberships(doc: &Document) -> Vec<(ObjRef, Option<Object>)> {
    let page = pdfcraft_model::pages(doc).remove(0);
    let resources = doc.resolve(page.dict.get(b"Resources").unwrap());
    let resources = resources.as_dict().unwrap();
    let properties = resources.get(b"Properties").map(|p| doc.resolve(p));
    let xobjects = resources.get(b"XObject").map(|p| doc.resolve(p));
    let content = doc.resolve(page.dict.get(b"Contents").unwrap());
    let streams = match &*content {
        Object::Array(list) => list.clone(),
        _ => vec![page.dict.get(b"Contents").unwrap().clone()],
    };
    let mut contexts = Vec::new();
    let mut paints = Vec::new();
    for stream in streams {
        let resolved = doc.resolve(&stream);
        let Object::Stream(stream) = &*resolved else { panic!("content stream") };
        let parsed = pdfcraft_content::parse(&stream.decoded().unwrap());
        assert_eq!(parsed.skipped, 0);
        for op in parsed.ops {
            if op.is("BDC") {
                let membership = if op.name(0) == Some(b"OC") {
                    Some(properties.as_ref().unwrap().as_dict().unwrap().get(op.name(1).unwrap()).unwrap().clone())
                } else {
                    None
                };
                contexts.push(membership);
            } else if op.is("BMC") {
                contexts.push(None);
            } else if op.is("EMC") {
                assert!(contexts.pop().is_some(), "balanced marked content");
            } else if op.is("Do") {
                let reference = xobjects.as_ref().unwrap().as_dict().unwrap().reference(op.name(0).unwrap()).unwrap();
                paints.push((reference, contexts.iter().rev().find_map(Clone::clone)));
            }
        }
    }
    assert!(contexts.is_empty(), "every optional-content block must close");
    paints
}

/// Verify that unsupported optional-content memberships retain the annotation, its transitive
/// replies and pop-ups, and its AcroForm field links.
#[test]
fn unsupported_membership_keeps_annotations_replies_and_field_links() {
    use pdfcraft_cos::Stream;
    let direct_ocg = Object::Dict({
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Type".to_vec(), Object::Name(b"OCG".to_vec()));
        d
    });
    let wrong_type = Object::Dict({
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Type".to_vec(), Object::Name(b"Other".to_vec()));
        d
    });
    for membership in [Object::Null, Object::Int(7), Object::Ref(ObjRef::new(999, 0)), Object::Ref(ObjRef::new(7, 0)), direct_ocg, wrong_type] {
        for widget in [false, true] {
            let mut doc = fixture("", 4, "");
            let retained = ObjRef::new(8, 0);
            let mut successful = doc.get(retained).as_dict().unwrap().clone();
            successful.remove(b"OC");
            let successful = doc.add(Object::Dict(successful));
            let empty_appearance = if widget {
                let mut empty = doc.get(ObjRef::new(7, 0)).as_dict().unwrap().clone();
                empty.set(b"BBox".to_vec(), Object::Array(vec![Object::Int(0); 4]));
                let reference = doc.add(Object::Stream(Stream::from_raw(empty, Vec::new())));
                let mut ap = pdfcraft_cos::Dict::new();
                ap.set(b"N".to_vec(), Object::Ref(reference));
                Some(Object::Dict(ap))
            } else {
                None
            };
            doc.update_dict(retained, |d| {
                d.set(b"OC".to_vec(), membership.clone());
                d.set(b"IRT".to_vec(), Object::Ref(successful));
                if widget {
                    d.set(b"Subtype".to_vec(), Object::Name(b"Widget".to_vec()));
                    d.set(b"AP".to_vec(), empty_appearance.clone().unwrap());
                } else {
                    d.remove(b"AP");
                }
            })
            .unwrap();
            let mut reply = doc.get(retained).as_dict().unwrap().clone();
            reply.remove(b"OC");
            reply.set(b"Subtype".to_vec(), Object::Name(b"Square".to_vec()));
            reply.set(b"IRT".to_vec(), Object::Ref(retained));
            reply.set(b"AP".to_vec(), doc.get(successful).as_dict().unwrap().get(b"AP").unwrap().clone());
            let reply = doc.add(Object::Dict(reply));
            let mut grandchild = doc.get(reply).as_dict().unwrap().clone();
            grandchild.set(b"IRT".to_vec(), Object::Ref(reply));
            let grandchild = doc.add(Object::Dict(grandchild));
            let popup = doc.add(Object::Dict({
                let mut d = pdfcraft_cos::Dict::new();
                d.set(b"Subtype".to_vec(), Object::Name(b"Popup".to_vec()));
                d.set(b"Parent".to_vec(), Object::Ref(retained));
                d
            }));
            doc.update_dict(ObjRef::new(3, 0), |d| {
                d.set(
                    b"Annots".to_vec(),
                    Object::Array(vec![
                        Object::Ref(successful),
                        Object::Ref(retained),
                        Object::Ref(reply),
                        Object::Ref(grandchild),
                        Object::Ref(popup),
                    ]),
                )
            })
            .unwrap();
            if widget {
                let fields = doc.add(Object::Dict({
                    let mut d = pdfcraft_cos::Dict::new();
                    d.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(retained)]));
                    d
                }));
                doc.update_dict(ObjRef::new(1, 0), |d| d.set(b"AcroForm".to_vec(), Object::Ref(fields))).unwrap();
            }
            let originals = [retained, reply, grandchild, popup].map(|r| (*doc.get(r)).clone());
            let original_fields = doc.get(ObjRef::new(1, 0)).as_dict().unwrap().get(b"AcroForm").map(|o| (*doc.resolve(o)).clone());
            assert_eq!(pdfcraft_edit::flatten(&mut doc, &[0], true, true).unwrap(), 1, "{membership:?}");
            let doc = Document::open(Arc::new(saved(&doc))).unwrap();
            for (reference, original) in [retained, reply, grandchild, popup].into_iter().zip(originals) {
                assert_eq!(*doc.get(reference), original);
            }
            let page = pdfcraft_model::pages(&doc).remove(0);
            assert_eq!(
                doc.resolve(page.dict.get(b"Annots").unwrap()).as_array().unwrap(),
                &vec![Object::Ref(retained), Object::Ref(reply), Object::Ref(grandchild), Object::Ref(popup)]
            );
            assert_eq!(doc.get(ObjRef::new(1, 0)).as_dict().unwrap().get(b"AcroForm").map(|o| (*doc.resolve(o)).clone()), original_fields);
        }
    }
    // A stream that claims Type OCG must not qualify through Object::as_dict().
    let mut doc = fixture("/OC 4 0 R", 4, "");
    let mut d = pdfcraft_cos::Dict::new();
    d.set(b"Type".to_vec(), Object::Name(b"OCG".to_vec()));
    doc.set(ObjRef::new(4, 0), Object::Stream(Stream::from_raw(d, Vec::new())));
    let before = saved(&doc);
    assert_eq!(pdfcraft_edit::flatten(&mut doc, &[0], true, false).unwrap(), 0);
    assert_eq!(saved(&doc), before);

    // Shared-subexpression /VE fan-out (10^16 paths within depth 16) stops at the node budget
    // and retains the annotation unchanged instead of hanging.
    let mut doc = fixture("/OC 9 0 R", 4, "");
    let mut target = ObjRef::new(4, 0);
    for _ in 0..16 {
        let mut expr = vec![Object::Name(b"Or".to_vec())];
        expr.extend(std::iter::repeat_n(Object::Ref(target), 10));
        target = doc.add(Object::Array(expr));
    }
    let ve = (*doc.get(target)).clone();
    doc.update_dict(ObjRef::new(9, 0), |d| {
        d.set(b"VE".to_vec(), ve.clone());
    })
    .unwrap();
    let before = saved(&doc);
    assert_eq!(pdfcraft_edit::flatten(&mut doc, &[0], true, false).unwrap(), 0);
    assert_eq!(saved(&doc), before);
}

/// Preserve a malformed reply that targets a retained popup instead of a markup annotation.
#[test]
fn unsupported_membership_keeps_popup_replies_after_save_and_reopen() {
    let mut doc = fixture("/OC null", 4, "");
    let root = ObjRef::new(8, 0);
    let popup = doc.add(Object::Dict({
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Subtype".to_vec(), Object::Name(b"Popup".to_vec()));
        d.set(b"Parent".to_vec(), Object::Ref(root));
        d.set(b"Private".to_vec(), Object::String(pdfcraft_cos::PdfString::text("keep popup")));
        d
    }));
    let mut reply = doc.get(root).as_dict().unwrap().clone();
    reply.remove(b"OC");
    reply.set(b"IRT".to_vec(), Object::Ref(popup));
    let reply = doc.add(Object::Dict(reply));
    let mut drawable = doc.get(root).as_dict().unwrap().clone();
    drawable.remove(b"OC");
    let drawable = doc.add(Object::Dict(drawable));
    doc.update_dict(ObjRef::new(3, 0), |d| {
        d.set(b"Annots".to_vec(), Object::Array([root, popup, reply, drawable].map(Object::Ref).to_vec()));
    })
    .unwrap();
    let originals = [root, popup, reply].map(|r| (*doc.get(r)).clone());
    assert_eq!(pdfcraft_edit::flatten(&mut doc, &[0], true, false).unwrap(), 1, "only the unrelated annotation can flatten");
    let doc = Document::open(Arc::new(saved(&doc))).unwrap();
    for (reference, original) in [root, popup, reply].into_iter().zip(originals) {
        assert_eq!(*doc.get(reference), original, "the retained popup graph stays unchanged");
    }
    let page = pdfcraft_model::pages(&doc).remove(0);
    assert_eq!(doc.resolve(page.dict.get(b"Annots").unwrap()).as_array().unwrap(), &[root, popup, reply].map(Object::Ref));
    assert_eq!(paint_memberships(&doc), vec![(ObjRef::new(7, 0), None)]);
}

/// Undrawable text in a retained reply must not block an unrelated drawable annotation.
#[test]
fn unsupported_membership_keeps_undrawable_reply_without_refusing_flatten() {
    for marked_appearance in [false, true] {
        let mut doc = fixture("/OC null", 4, "");
        let root = ObjRef::new(8, 0);
        let mut reply = doc.get(root).as_dict().unwrap().clone();
        reply.remove(b"OC");
        reply.set(b"Subtype".to_vec(), Object::Name(b"FreeText".to_vec()));
        reply.set(b"IRT".to_vec(), Object::Ref(root));
        reply.set(b"Contents".to_vec(), Object::String(pdfcraft_cos::PdfString::text("漢")));
        if marked_appearance {
            pdfcraft_fonts::set_undrawn_mark(&mut reply, Some('漢'));
        } else {
            reply.remove(b"AP");
        }
        assert_eq!(pdfcraft_annot::appearance::undrawable_text(&reply).map(|(_, c)| c), Some('漢'));
        let reply = doc.add(Object::Dict(reply));
        let mut drawable = doc.get(root).as_dict().unwrap().clone();
        drawable.remove(b"OC");
        let drawable = doc.add(Object::Dict(drawable));
        doc.update_dict(ObjRef::new(3, 0), |d| {
            d.set(b"Annots".to_vec(), Object::Array([root, reply, drawable].map(Object::Ref).to_vec()));
        })
        .unwrap();
        // Compare serialized dictionaries before and after flatten, including PDF string syntax.
        let mut doc = Document::open(Arc::new(saved(&doc))).unwrap();
        let originals = [root, reply].map(|r| (*doc.get(r)).clone());
        assert_eq!(pdfcraft_edit::flatten(&mut doc, &[0], true, false).unwrap(), 1, "retained text is not baked into page content");
        let doc = Document::open(Arc::new(saved(&doc))).unwrap();
        for (reference, original) in [root, reply].into_iter().zip(originals) {
            assert_eq!(*doc.get(reference), original);
        }
        let page = pdfcraft_model::pages(&doc).remove(0);
        assert_eq!(doc.resolve(page.dict.get(b"Annots").unwrap()).as_array().unwrap(), &[root, reply].map(Object::Ref));
        assert_eq!(paint_memberships(&doc), vec![(ObjRef::new(7, 0), None)]);
    }
}

/// Verify that shared/inherited `/Resources` and `/Properties` dictionaries are never mutated in
/// place and that consecutive flatten runs allocate distinct `/PCFlOC{n}` property names.
#[test]
fn flatten_preserves_shared_resources_and_uses_new_names_on_each_call() {
    for inherited in [false, true] {
        for indirect in [false, true] {
            let mut doc = fixture("/OC 4 0 R", 4, "");
            let mut resources = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().get(b"Resources").unwrap().as_dict().unwrap().clone();
            let original_properties = resources.get(b"Properties").unwrap().clone();
            if indirect {
                let r = doc.add(original_properties.clone());
                resources.set(b"Properties".to_vec(), Object::Ref(r));
            }
            let resources = if indirect { Object::Ref(doc.add(Object::Dict(resources))) } else { Object::Dict(resources) };
            let owner = ObjRef::new(if inherited { 2 } else { 3 }, 0);
            if inherited {
                doc.update_dict(ObjRef::new(3, 0), |d| {
                    d.remove(b"Resources");
                })
                .unwrap();
            }
            doc.update_dict(owner, |d| d.set(b"Resources".to_vec(), resources.clone())).unwrap();
            let original_resource_object = (*doc.resolve(&resources)).clone();
            assert_eq!(pdfcraft_edit::flatten(&mut doc, &[0], true, false).unwrap(), 1);
            doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(8, 0))]))).unwrap();
            assert_eq!(pdfcraft_edit::flatten(&mut doc, &[0], true, false).unwrap(), 1);
            let doc = Document::open(Arc::new(saved(&doc))).unwrap();
            assert_eq!(paint_memberships(&doc), vec![(ObjRef::new(7, 0), Some(Object::Ref(ObjRef::new(4, 0)))); 2]);
            let page = pdfcraft_model::pages(&doc).remove(0);
            let effective = doc.resolve(page.dict.get(b"Resources").unwrap());
            let properties = doc.resolve(effective.as_dict().unwrap().get(b"Properties").unwrap());
            let properties = properties.as_dict().unwrap();
            for (key, value) in original_properties.as_dict().unwrap().iter() {
                assert_eq!(properties.get(key), Some(value));
            }
            assert_eq!(properties.len(), original_properties.as_dict().unwrap().len() + 2);
            if inherited || indirect {
                assert_eq!(*doc.resolve(&resources), original_resource_object);
            }
        }
    }
}

/// Verify that malformed `/Resources` or `/Properties` entries retain `/OC` annotations without
/// generating appearances, while inline annotations with valid `/OC` still flatten cleanly.
#[test]
fn malformed_resources_and_direct_ocg_stay_unchanged_before_appearance_generation() {
    for malformed_resources in [false, true] {
        let mut doc = fixture("/OC 4 0 R", 4, "");
        if malformed_resources {
            doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Resources".to_vec(), Object::Null)).unwrap();
        } else {
            let mut resources = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().get(b"Resources").unwrap().as_dict().unwrap().clone();
            resources.set(b"Properties".to_vec(), Object::Int(1));
            doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
        }
        doc.update_dict(ObjRef::new(8, 0), |d| {
            d.remove(b"AP");
        })
        .unwrap();
        let before = saved(&doc);
        assert_eq!(pdfcraft_edit::flatten(&mut doc, &[0], true, false).unwrap(), 0);
        assert_eq!(saved(&doc), before);
    }
    let mut doc = fixture("/OC 4 0 R", 4, "");
    let annotation = doc.get(ObjRef::new(8, 0)).as_dict().unwrap().clone();
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Dict(annotation)]))).unwrap();
    assert_eq!(pdfcraft_edit::flatten(&mut doc, &[0], true, false).unwrap(), 1);
    let doc = Document::open(Arc::new(saved(&doc))).unwrap();
    assert_eq!(paint_memberships(&doc), vec![(ObjRef::new(7, 0), Some(Object::Ref(ObjRef::new(4, 0))))]);
}
