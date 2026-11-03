//! Contributor-original synthetic stream collision; no external document assets.
use pdfcraft_cos::{Document, ObjRef, Object};
use std::sync::Arc;

fn collision_with(kids: &str, extra: &[&str]) -> Document {
    let page = "/Figure <</MCID 0>> BDC 10 10 20 20 re f EMC\nq /Form Do Q";
    let form = "/Figure <</MCID 0>> BDC 0 0 5 5 re f EMC";
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R /MarkInfo << /Marked true >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /StructParents 0 /Resources << /XObject << /Form 10 0 R >> >> >>"
            .into(),
        format!("<< /Length {} >>\nstream\n{}\nendstream", page.len(), page),
        "<< /Type /StructTreeRoot /K 6 0 R /ParentTree 9 0 R >>".into(),
        "<< /Type /StructElem /S /Document /P 5 0 R /K [7 0 R 8 0 R] >>".into(),
        "<< /Type /StructElem /S /Figure /Pg 3 0 R /P 6 0 R /K 0 /Alt (Page figure) >>".into(),
        format!("<< /Type /StructElem /S /Figure /Pg 3 0 R /P 6 0 R /K {kids} /Alt (Form figure) >>"),
        "<< /Nums [0 [7 0 R] 1 8 0 R] >>".into(),
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 5 5] /StructParents 1 /Length {} >>\nstream\n{}\nendstream", form.len(), form),
    ];
    let extra_pages = extra
        .iter()
        .enumerate()
        .filter(|(_, object)| object.contains("/Type /Page "))
        .map(|(index, _)| format!("{} 0 R", index + 11))
        .collect::<Vec<_>>();
    if !extra_pages.is_empty() {
        objects[1] = format!("<< /Type /Pages /Kids [3 0 R {}] /Count {} >>", extra_pages.join(" "), extra_pages.len() + 1);
    }
    objects.extend(extra.iter().map(|s| (*s).to_string()));
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, object).as_bytes());
    }
    let start = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n", objects.len() + 1, start).as_bytes());
    Document::open(Arc::new(bytes)).unwrap()
}

fn stream(doc: &Document, number: u32) -> Vec<u8> {
    let object = doc.get(ObjRef::new(number, 0));
    let Object::Stream(s) = &*object else { panic!("stream required") };
    s.decoded().unwrap()
}

fn run(decorative: bool) {
    let mut doc = collision_with("<< /Type /MCR /MCID 0 /Stm 10 0 R /Pg 3 0 R >>", &[]);
    assert!(!doc.is_modified());
    let before = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let original_streams = [stream(&doc, 4), stream(&doc, 10)];
    let error = mutate(&mut doc, decorative, 8).unwrap_err();
    assert!(error.contains("form"), "useful error: {error}");
    assert!(!doc.is_modified());
    assert_eq!([stream(&doc, 4), stream(&doc, 10)], original_streams);
    assert_eq!(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap(), before);
    assert_eq!(pdfcraft_a11y::tags(&doc).iter().map(|t| t.object.num).collect::<Vec<_>>(), [6, 7, 8]);
}

#[test]
fn decorative_refuses_form_collision() {
    run(true);
}
#[test]
fn artifact_refuses_form_collision() {
    run(false);
}

fn mutate(doc: &mut Document, decorative: bool, number: u32) -> Result<(), String> {
    if decorative {
        pdfcraft_a11y::mark_decorative(doc, ObjRef::new(number, 0)).map_err(|e| e.to_string())
    } else {
        pdfcraft_a11y::mark_tag_artifact(doc, ObjRef::new(number, 0)).map_err(|e| e.to_string())
    }
}

fn page_owned(kids: &str, extra: &[&str]) -> Document {
    let mut doc = collision_with(kids, extra);
    let page_kids = doc.get(ObjRef::new(8, 0)).as_dict().unwrap().get(b"K").unwrap().clone();
    let original = collision_with("<< /Type /MCR /MCID 0 /Stm 10 0 R /Pg 3 0 R >>", &[]);
    let form_kids = original.get(ObjRef::new(8, 0)).as_dict().unwrap().get(b"K").unwrap().clone();
    doc.update_dict(ObjRef::new(7, 0), |d| d.set(b"K".to_vec(), page_kids)).unwrap();
    doc.update_dict(ObjRef::new(8, 0), |d| d.set(b"K".to_vec(), form_kids)).unwrap();
    // Reopen an incremental save to keep the fixture's object IDs and start from a clean state.
    Document::open(Arc::new(pdfcraft_cos::write_incremental(&doc, &Default::default()).unwrap())).unwrap()
}

#[test]
fn unsupported_owned_references_refuse_without_changing_document_or_saved_data() {
    let other_page_content = "/P <</MCID 0>> BDC 40 40 5 5 re f EMC";
    let other_page_stream = format!("<< /Length {} >>\nstream\n{other_page_content}\nendstream", other_page_content.len());
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("<< /Type /MCR /MCID 0 /Stm 10 0 R >>", vec![]),
        ("<< /Type /MCR /MCID 2 /Stm 10 0 R >>", vec![]),
        ("<< /Type /MCR /MCID 0 /Stm null >>", vec![]),
        ("<< /Type /MCR /MCID 0 /Stm 99 0 R >>", vec![]),
        ("<< /Type /MCR /MCID 0 /Stm (invalid) >>", vec![]),
        ("<< /Type /MCR /MCID 0 /StmOwn 10 0 R >>", vec![]),
        ("<< /Type /MCR /MCID 0 /StmOwn null >>", vec![]),
        ("11 0 R", vec!["<< /Type /MCR /MCID 0 /Stm 10 0 R >>"]),
        ("11 0 R", vec!["12 0 R", "<< /Type /MCR /MCID 0 /Stm 10 0 R >>"]),
        ("[0 << /S /Figure /K 0 >>]", vec![]),
        ("[0 11 0 R]", vec!["<< /S /Figure /K 0 >>"]),
        ("[0 << /Type /MCR /MCID 0 /Stm 10 0 R >>]", vec![]),
        ("11 0 R", vec!["[0 12 0 R]", "<< /Type /MCR /MCID 0 /Stm 10 0 R >>"]),
        ("[0 [11 0 R]]", vec!["<< /Type /MCR /MCID 0 /Stm 10 0 R >>"]),
        ("[0 << /Type /MCR /MCID 0 /Pg 99 0 R >>]", vec![]),
        ("<< /Type /MCR /MCID 0 /Pg null >>", vec![]),
        ("<< /Type /MCR /MCID 0 /Pg 3 1 R >>", vec![]),
        (
            "[0 << /Type /MCR /MCID 0 /Pg 11 0 R >>]",
            vec!["<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 12 0 R >>", &other_page_stream],
        ),
        ("[0 << /MCID 0 /Pg 3 0 R >>]", vec![]),
        ("[0 << /Type /MCR /Pg 3 0 R >>]", vec![]),
        ("[0 99 0 R]", vec![]),
        ("[0 11 1 R]", vec!["<< /Type /MCR /MCID 0 /Pg 3 0 R >>"]),
        ("[0 11 0 R]", vec!["[11 0 R]"]),
        ("[0 11 0 R]", vec!["<< /S /Figure /K << /Type /MCR /MCID 0 /Stm 10 0 R >> >>"]),
        ("11 0 R", vec!["[12 0 R]", "<< /S /Figure /K << /Type /MCR /MCID 0 /Stm 10 0 R >> >>"]),
    ];
    for decorative in [true, false] {
        for (kids, extra) in &cases {
            let mut doc = collision_with(kids, extra);
            let before = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
            let original_streams = [stream(&doc, 4), stream(&doc, 10)];
            let original_objects: Vec<_> = (1..=10).map(|i| doc.get(ObjRef::new(i, 0))).collect();
            let original_alts = pdfcraft_a11y::figures(&doc).into_iter().map(|figure| figure.alt).collect::<Vec<_>>();
            let error = match mutate(&mut doc, decorative, 8) {
                Ok(()) => panic!("decorative={decorative} K={kids} must refuse"),
                Err(error) => error,
            };
            assert!(
                error.contains("form") || error.contains("unsupported") || error.contains("invalid") || error.contains("child structure elements"),
                "ownership refusal needs a useful reason: {error}"
            );
            assert!(!doc.is_modified(), "refusal must not create dirty state: {kids}");
            assert_eq!([stream(&doc, 4), stream(&doc, 10)], original_streams, "{kids}");
            assert_eq!(
                (1..=10).map(|i| doc.get(ObjRef::new(i, 0))).collect::<Vec<_>>(),
                original_objects,
                "tags, Alt and ParentTree stay unchanged: {kids}"
            );
            let saved = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
            assert_eq!(saved, before, "saved object graph remains exact: {kids}");
            let reopened = Document::open(Arc::new(saved)).unwrap();
            let page = &pdfcraft_model::pages(&reopened)[0];
            let Object::Stream(page_stream) = &*reopened.resolve(page.dict.get(b"Contents").unwrap()) else { panic!("page stream") };
            assert_eq!(page_stream.decoded().unwrap(), original_streams[0]);
            let resources = reopened.dict(page.dict.get(b"Resources").unwrap()).unwrap();
            let xobjects = reopened.dict(resources.get(b"XObject").unwrap()).unwrap();
            let Object::Stream(form_stream) = &*reopened.resolve(xobjects.get(b"Form").unwrap()) else { panic!("form stream") };
            assert_eq!(form_stream.decoded().unwrap(), original_streams[1]);
            assert_eq!(
                pdfcraft_a11y::figures(&reopened).into_iter().map(|figure| figure.alt).collect::<Vec<_>>(),
                original_alts,
                "all original figures, including children, survive: {kids}"
            );
        }
    }
}

#[test]
fn page_only_owned_references_still_become_artifacts() {
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("0", vec![]),
        ("<< /Type /MCR /MCID 0 /Pg 3 0 R >>", vec![]),
        ("11 0 R", vec!["<< /Type /MCR /MCID 0 /Pg 3 0 R >>"]),
        ("11 0 R", vec!["12 0 R", "<< /Type /MCR /MCID 0 /Pg 3 0 R >>"]),
        ("[11 0 R 11 0 R]", vec!["<< /Type /MCR /MCID 0 /Pg 3 0 R >>"]),
        ("11 0 R", vec!["[12 0 R]", "<< /Type /MCR /MCID 0 /Pg 3 0 R >>"]),
    ];
    for decorative in [true, false] {
        for (kids, extra) in &cases {
            let mut doc = page_owned(kids, extra);
            let original_form = stream(&doc, 10);
            mutate(&mut doc, decorative, 7).unwrap();
            let page = pdfcraft_model::pages(&doc)[0].dict.get(b"Contents").unwrap().as_ref().unwrap();
            let content = String::from_utf8(stream(&doc, page.num)).unwrap();
            assert!(content.contains("/Artifact BMC"), "page content changes for {kids}: {content}");
            assert!(!content.contains("/MCID"));
            assert_eq!(stream(&doc, 10), original_form);
            assert_eq!(pdfcraft_a11y::tags(&doc).iter().map(|t| t.object.num).collect::<Vec<_>>(), [6, 8]);
        }
    }
}

#[test]
fn ordinary_alt_edit_remains_available_for_a_stream_figure() {
    let mut doc = collision_with("<< /Type /MCR /MCID 0 /Stm 10 0 R >>", &[]);
    let original = [stream(&doc, 4), stream(&doc, 10)];
    pdfcraft_a11y::set_alt(&mut doc, ObjRef::new(8, 0), Some("  Blue square  ")).unwrap();
    assert_eq!(pdfcraft_a11y::figures(&doc)[1].alt.as_deref(), Some("Blue square"));
    assert_eq!([stream(&doc, 4), stream(&doc, 10)], original);
}

#[test]
fn refusal_preserves_preexisting_document_changes() {
    for decorative in [true, false] {
        let mut doc = collision_with("<< /Type /MCR /MCID 0 /Stm 10 0 R >>", &[]);
        pdfcraft_a11y::set_alt(&mut doc, ObjRef::new(8, 0), Some("Saved later")).unwrap();
        let before = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
        let modified = doc.modified_objects();
        assert!(mutate(&mut doc, decorative, 8).is_err());
        assert!(doc.is_modified());
        assert_eq!(doc.modified_objects(), modified);
        assert_eq!(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap(), before);
        assert_eq!(pdfcraft_a11y::figures(&doc)[1].alt.as_deref(), Some("Saved later"));
    }
}

#[test]
fn owned_reference_walk_accepts_limits_and_refuses_excess_depth_or_work() {
    // The root ownership array counts as one visit. Each nested array adds one depth level.
    let cases = [
        (format!("[{}]", "0 ".repeat(4095)), true),
        (format!("[{}]", "0 ".repeat(4096)), false),
        (format!("{}0{}", "[".repeat(64), "]".repeat(64)), true),
        (format!("{}0{}", "[".repeat(65), "]".repeat(65)), false),
    ];
    for decorative in [true, false] {
        for (kids, supported) in &cases {
            let mut doc = page_owned(kids, &[]);
            let before = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
            let result = mutate(&mut doc, decorative, 7);
            assert_eq!(result.is_ok(), *supported, "decorative={decorative}, result={result:?}");
            if *supported {
                let page = pdfcraft_model::pages(&doc)[0].dict.get(b"Contents").unwrap().as_ref().unwrap();
                assert!(String::from_utf8(stream(&doc, page.num)).unwrap().contains("/Artifact BMC"));
                assert_eq!(pdfcraft_a11y::tags(&doc).iter().map(|t| t.object.num).collect::<Vec<_>>(), [6, 8]);
            } else {
                assert!(!doc.is_modified());
                assert_eq!(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap(), before);
            }
        }
    }
}

#[test]
fn decorative_refuses_matching_invalid_element_and_content_pages() {
    for page in [ObjRef::new(99, 0), ObjRef::new(3, 1)] {
        for kids in [format!("<< /Type /MCR /MCID 0 /Pg {page} >>"), "0".into(), "<< /Type /MCR /MCID 0 >>".into()] {
            let mut doc = collision_with(&kids, &[]);
            doc.update_dict(ObjRef::new(8, 0), |d| d.set(b"Pg".to_vec(), Object::Ref(page))).unwrap();
            let mut doc = Document::open(Arc::new(pdfcraft_cos::write_incremental(&doc, &Default::default()).unwrap())).unwrap();
            let before = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
            let error = pdfcraft_a11y::mark_decorative(&mut doc, ObjRef::new(8, 0)).unwrap_err();
            assert!(error.to_string().contains("form"));
            assert!(!doc.is_modified());
            assert_eq!(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap(), before);
        }
    }
}

#[test]
fn annotation_reference_refusal_names_the_unsupported_content_and_preserves_its_links() {
    for decorative in [true, false] {
        let mut doc = collision_with(
            "[0 << /Type /OBJR /Obj 11 0 R /Pg 3 0 R >>]",
            &["<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /P 3 0 R /StructParent 2 >>"],
        );
        doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(11, 0))]))).unwrap();
        doc.update_dict(ObjRef::new(9, 0), |d| {
            d.set(
                b"Nums".to_vec(),
                Object::Array(vec![
                    Object::Int(0),
                    Object::Array(vec![Object::Ref(ObjRef::new(7, 0))]),
                    Object::Int(1),
                    Object::Ref(ObjRef::new(8, 0)),
                    Object::Int(2),
                    Object::Ref(ObjRef::new(8, 0)),
                ]),
            )
        })
        .unwrap();
        let mut doc = Document::open(Arc::new(pdfcraft_cos::write_incremental(&doc, &Default::default()).unwrap())).unwrap();
        let before = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
        let original_objects: Vec<_> = (1..=11).map(|i| doc.get(ObjRef::new(i, 0))).collect();
        let error = mutate(&mut doc, decorative, 8).unwrap_err();
        assert!(error.contains("annotation or object references"), "useful OBJR refusal: {error}");
        assert!(!doc.is_modified());
        assert_eq!((1..=11).map(|i| doc.get(ObjRef::new(i, 0))).collect::<Vec<_>>(), original_objects);
        let saved = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
        assert_eq!(saved, before);
        let reopened = Document::open(Arc::new(saved)).unwrap();
        let page = &pdfcraft_model::pages(&reopened)[0];
        let annotation = reopened.resolve(&page.dict.get(b"Annots").unwrap().as_array().unwrap()[0]);
        assert_eq!(annotation.as_dict().unwrap().get(b"StructParent"), Some(&Object::Int(2)));
        let catalog = reopened.get(reopened.root().unwrap());
        let root = reopened.dict(catalog.as_dict().unwrap().get(b"StructTreeRoot").unwrap()).unwrap();
        let tree = reopened.dict(root.get(b"ParentTree").unwrap()).unwrap();
        let nums = tree.get(b"Nums").unwrap().as_array().unwrap();
        let tag = reopened.resolve(&nums[5]);
        assert_eq!(tag.as_dict().unwrap().get(b"K").unwrap().as_array().unwrap()[0], Object::Int(0));
        assert_eq!(nums[4], Object::Int(2));
    }
}

fn assert_artifact_refusal_preserves(doc: &mut Document, expected: &pdfcraft_a11y::ArtifactError) {
    let state = doc.read_state();
    let dirty = doc.is_modified();
    let modified = doc.modified_objects();
    let saved = pdfcraft_cos::write_full(doc, &Default::default()).unwrap();
    let objects: Vec<_> = (1..=12).map(|number| doc.get(ObjRef::new(number, 0))).collect();
    let streams = [stream(doc, 4), stream(doc, 10)];
    let tags = pdfcraft_a11y::tags(doc);
    let alts = pdfcraft_a11y::figures(doc).into_iter().map(|figure| figure.alt).collect::<Vec<_>>();
    assert!(tags.iter().any(|tag| tag.object == ObjRef::new(7, 0)), "the malformed leaf remains reachable");
    assert_eq!(pdfcraft_a11y::mark_tag_artifact(doc, ObjRef::new(7, 0)).as_ref(), Err(expected));
    assert!(state.matches(doc), "refusal preserves the exact overlay and allocates no stream");
    assert_eq!(doc.is_modified(), dirty);
    assert_eq!(doc.modified_objects(), modified);
    assert_eq!((1..=12).map(|number| doc.get(ObjRef::new(number, 0))).collect::<Vec<_>>(), objects);
    assert_eq!([stream(doc, 4), stream(doc, 10)], streams);
    let after = pdfcraft_cos::write_full(doc, &Default::default()).unwrap();
    assert_eq!(after, saved);
    // Full saves renumber objects; incremental saves retain the fixture IDs checked below.
    let reopened = Document::open(Arc::new(pdfcraft_cos::write_incremental(doc, &Default::default()).unwrap())).unwrap();
    assert_eq!(pdfcraft_a11y::tags(&reopened), tags);
    assert_eq!(pdfcraft_a11y::figures(&reopened).into_iter().map(|figure| figure.alt).collect::<Vec<_>>(), alts);
    assert_eq!([stream(&reopened, 4), stream(&reopened, 10)], streams);
    assert_eq!(reopened.get(ObjRef::new(3, 0)), objects[2]);
    assert_eq!(reopened.get(ObjRef::new(9, 0)), objects[8]);
    assert_eq!(reopened.get(ObjRef::new(7, 0)), objects[6]);
    assert_eq!(
        reopened.get(ObjRef::new(9, 0)).as_dict().unwrap().get(b"Nums").unwrap().as_array().unwrap()[1],
        Object::Array(vec![Object::Ref(ObjRef::new(7, 0))])
    );
}

#[test]
fn artifact_parent_refusals_preserve_direct_library_state_and_saved_content() {
    use pdfcraft_a11y::ArtifactError::{AmbiguousParent, NoParent};
    let cases = [
        (None, "<< >>", NoParent),
        (Some(Object::Int(6)), "<< >>", NoParent),
        (Some(Object::Ref(ObjRef::new(12, 0))), "null", NoParent),
        (Some(Object::Ref(ObjRef::new(12, 0))), "<< >>", NoParent),
        (Some(Object::Ref(ObjRef::new(12, 0))), "<< /K 3 >>", NoParent),
        (Some(Object::Ref(ObjRef::new(12, 0))), "<< /K 8 0 R >>", NoParent),
        (Some(Object::Ref(ObjRef::new(12, 0))), "<< /K [] >>", AmbiguousParent),
        (Some(Object::Ref(ObjRef::new(12, 0))), "<< /K [7 0 R 7 0 R] >>", AmbiguousParent),
    ];
    for dirty in [false, true] {
        for (parent, parent_object, expected) in &cases {
            let mut doc = page_owned("11 0 R", &["<< /Type /MCR /MCID 0 /Pg 3 0 R >>", parent_object]);
            doc.update_dict(ObjRef::new(7, 0), |dict| {
                dict.remove(b"P");
                if let Some(parent) = parent {
                    dict.set(b"P".to_vec(), parent.clone());
                }
            })
            .unwrap();
            let mut doc = Document::open(Arc::new(pdfcraft_cos::write_incremental(&doc, &Default::default()).unwrap())).unwrap();
            if dirty {
                pdfcraft_a11y::set_alt(&mut doc, ObjRef::new(8, 0), Some("Saved later")).unwrap();
            }
            assert_artifact_refusal_preserves(&mut doc, expected);
        }
    }
}

#[test]
fn artifact_mismatched_parent_refuses_before_changing_content_or_saved_state() {
    for dirty in [false, true] {
        for indirect in [false, true] {
            for root in [false, true] {
                let kids = if indirect { "11 0 R" } else { "0" };
                let mut doc = page_owned(kids, &["<< /Type /MCR /MCID 0 /Pg 3 0 R >>", "<< /Type /StructElem /S /Document /K [7 0 R] >>"]);
                if root {
                    doc.update_dict(ObjRef::new(5, 0), |dict| dict.set(b"K".to_vec(), Object::Ref(ObjRef::new(7, 0)))).unwrap();
                }
                doc.update_dict(ObjRef::new(7, 0), |dict| dict.set(b"P".to_vec(), Object::Ref(ObjRef::new(12, 0)))).unwrap();
                let mut doc = Document::open(Arc::new(pdfcraft_cos::write_incremental(&doc, &Default::default()).unwrap())).unwrap();
                if dirty {
                    pdfcraft_a11y::set_alt(&mut doc, ObjRef::new(7, 0), Some("Saved later")).unwrap();
                }
                assert_artifact_refusal_preserves(&mut doc, &pdfcraft_a11y::ArtifactError::NoParent);
            }
        }
    }
}

#[test]
fn artifact_inline_parent_does_not_become_a_removable_indirect_ancestor() {
    for dirty in [false, true] {
        for root in [false, true] {
            let mut doc = page_owned("11 0 R", &["<< /Type /MCR /MCID 0 /Pg 3 0 R >>"]);
            let parent = ObjRef::new(if root { 5 } else { 6 }, 0);
            let mut inline = pdfcraft_cos::Dict::new();
            inline.set(b"S".to_vec(), Object::name("Document"));
            inline.set(b"K".to_vec(), Object::Ref(ObjRef::new(7, 0)));
            doc.update_dict(parent, |dict| dict.set(b"K".to_vec(), Object::Dict(inline))).unwrap();
            doc.update_dict(ObjRef::new(7, 0), |dict| dict.set(b"P".to_vec(), Object::Ref(parent))).unwrap();
            let mut doc = Document::open(Arc::new(pdfcraft_cos::write_incremental(&doc, &Default::default()).unwrap())).unwrap();
            if dirty {
                pdfcraft_a11y::set_alt(&mut doc, ObjRef::new(7, 0), Some("Saved later")).unwrap();
            }
            assert_artifact_refusal_preserves(&mut doc, &pdfcraft_a11y::ArtifactError::NoParent);
        }
    }
}

#[test]
fn artifact_missing_page_content_precedes_invalid_parent_without_mutation() {
    let mut doc = page_owned("11 0 R", &["<< /Type /MCR /MCID 1 /Pg 3 0 R >>"]);
    doc.update_dict(ObjRef::new(7, 0), |dict| {
        dict.remove(b"P");
    })
    .unwrap();
    let mut doc = Document::open(Arc::new(pdfcraft_cos::write_incremental(&doc, &Default::default()).unwrap())).unwrap();
    assert_artifact_refusal_preserves(&mut doc, &pdfcraft_a11y::ArtifactError::ContentNotFound);
}

#[test]
fn artifact_indirect_content_removes_scalar_or_array_parent_child_and_preserves_drawing() {
    for (root, scalar) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut doc = page_owned("11 0 R", &["<< /Type /MCR /MCID 0 /Pg 3 0 R >>"]);
        let parent = ObjRef::new(if root { 5 } else { 6 }, 0);
        if root {
            doc.update_dict(ObjRef::new(7, 0), |dict| dict.set(b"P".to_vec(), Object::Ref(parent))).unwrap();
            doc.update_dict(parent, |dict| {
                dict.set(b"K".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0))]))
            })
            .unwrap();
        }
        if scalar {
            doc.update_dict(parent, |dict| dict.set(b"K".to_vec(), Object::Ref(ObjRef::new(7, 0)))).unwrap();
        }
        let original_form = stream(&doc, 10);
        let other_figure = doc.get(ObjRef::new(8, 0));
        pdfcraft_a11y::mark_tag_artifact(&mut doc, ObjRef::new(7, 0)).unwrap();
        let reopened = Document::open(Arc::new(pdfcraft_cos::write_incremental(&doc, &Default::default()).unwrap())).unwrap();
        let contents = reopened.get(ObjRef::new(3, 0)).as_dict().unwrap().get(b"Contents").unwrap().as_ref().unwrap();
        let ops = pdfcraft_content::parse(&stream(&reopened, contents.num)).ops;
        assert!(ops.iter().any(|op| op.op == b"BMC" && op.operands == [Object::name("Artifact")]));
        assert!(!ops.iter().any(|op| op.op == b"BDC"));
        let rectangle = ops.iter().find(|op| op.op == b"re").unwrap();
        assert_eq!(rectangle.nums::<4>(), Some([10.0, 10.0, 20.0, 20.0]));
        assert!(ops.iter().any(|op| op.op == b"f"));
        assert!(ops.iter().any(|op| op.op == b"Do" && op.operands == [Object::name("Form")]));
        assert_eq!(stream(&reopened, 10), original_form);
        assert_eq!(reopened.get(ObjRef::new(8, 0)), other_figure);
        let remaining = if scalar { vec![] } else { vec![Object::Ref(ObjRef::new(8, 0))] };
        assert_eq!(reopened.get(parent).as_dict().unwrap().get(b"K"), Some(&Object::Array(remaining)));
        assert!(!reopened.get(ObjRef::new(7, 0)).as_dict().unwrap().contains(b"P"));
        let expected_tags = match (root, scalar) {
            (false, false) => vec![6, 8],
            (false, true) => vec![6],
            (true, false) => vec![8],
            (true, true) => vec![],
        };
        assert_eq!(pdfcraft_a11y::tags(&reopened).iter().map(|tag| tag.object.num).collect::<Vec<_>>(), expected_tags);
        assert_eq!(
            reopened.get(ObjRef::new(9, 0)).as_dict().unwrap().get(b"Nums").unwrap().as_array().unwrap()[1],
            Object::Array(vec![Object::Null])
        );
    }
}
