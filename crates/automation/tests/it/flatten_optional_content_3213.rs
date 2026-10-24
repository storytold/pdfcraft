//! Optional-content flattening through the real open, edit, save and reopen tools.
use std::{collections::HashMap, sync::Arc};

use pdfcraft_automation::{Automation, Content};
use pdfcraft_cos::{Document, ObjRef, Object};
use pdfcraft_engine::DocId;
use serde_json::{Value, json};

fn fixture(membership: &str, flags: i64, appearance_membership: &str) -> Vec<u8> {
    let ink = "1 0 0 rg 0 0 100 50 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [4 0 R 5 0 R] /D << /OFF [4 0 R] /ON [5 0 R] /Order [4 0 R 5 0 R] >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /XObject << /SharedAppearance 7 0 R >> /Properties << /PCFlOC0 5 0 R /OriginalMembership 9 0 R /Private (keep) >> >> /Contents 6 0 R /Annots [8 0 R] >>".into(),
        "<< /Type /OCG /Name (Hidden layer) /Private (keep layer) >>".into(),
        "<< /Type /OCG /Name (Shown layer) >>".into(),
        "<< /Length 0 >>\nstream\n\nendstream".into(),
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 100 50] {appearance_membership} /Length {} >>\nstream\n{ink}\nendstream", ink.len()),
        format!("<< /Type /Annot /Subtype /Square /Rect [50 50 150 100] /AP << /N 7 0 R >> /F {flags} {membership} /Private (keep annotation) >>"),
        "<< /Type /OCMD /OCGs [4 0 R] /P /AllOn /Private (keep membership) >>".into(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Root 1 0 R /Size {} >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

// A full save can renumber objects. Preserve graph identity, including the catalog's
// OCG references, while comparing every dictionary entry and stream byte exactly.
fn same_graph(before: &Document, expected: &Object, after: &Document, actual: &Object, refs: &mut HashMap<ObjRef, ObjRef>) {
    match (expected, actual) {
        (Object::Ref(a), Object::Ref(b)) => {
            if let Some(mapped) = refs.get(a) {
                assert_eq!(mapped, b, "the same original object must retain one saved identity");
            } else {
                assert!(!refs.values().any(|mapped| mapped == b), "distinct original objects must remain distinct");
                refs.insert(*a, *b);
            }
            same_graph(before, &before.get(*a), after, &after.get(*b), refs);
        }
        (Object::Dict(a), Object::Dict(b)) => {
            assert_eq!(a.len(), b.len());
            for (key, value) in a.iter() {
                same_graph(before, value, after, b.get(key).expect("preserved dictionary entry"), refs);
            }
        }
        (Object::Array(a), Object::Array(b)) => {
            assert_eq!(a.len(), b.len());
            for (a, b) in a.iter().zip(b) {
                same_graph(before, a, after, b, refs);
            }
        }
        (Object::Stream(a), Object::Stream(b)) => {
            assert_eq!(a.raw, b.raw, "original appearance bytes");
            same_graph(before, &Object::Dict(a.dict.clone()), after, &Object::Dict(b.dict.clone()), refs);
        }
        _ => assert_eq!(expected, actual),
    }
}

fn resources(doc: &Document) -> pdfcraft_cos::Dict {
    let page = pdfcraft_model::pages(doc).remove(0);
    doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone()
}

// This fixture's generated content has only names and numeric operands. Associate the
// actual Do invocation with its BDC property, rather than only checking resource presence.
fn paints(doc: &Document) -> Vec<(Object, Option<Object>)> {
    let page = pdfcraft_model::pages(doc).remove(0);
    let resources = doc.resolve(page.dict.get(b"Resources").unwrap());
    let resources = resources.as_dict().unwrap();
    let properties = doc.resolve(resources.get(b"Properties").unwrap());
    let contents = page.dict.get(b"Contents").unwrap();
    let resolved = doc.resolve(contents);
    let streams = resolved.as_array().cloned().unwrap_or_else(|| vec![contents.clone()]);
    let mut context = Vec::new();
    let mut paints = Vec::new();
    for stream in streams {
        let stream = doc.resolve(&stream);
        let Object::Stream(stream) = &*stream else { panic!("expected content stream") };
        let bytes = stream.decoded().unwrap();
        let tokens = std::str::from_utf8(&bytes).unwrap().split_ascii_whitespace().collect::<Vec<_>>();
        for (index, token) in tokens.iter().enumerate() {
            match *token {
                "BDC" => {
                    assert_eq!(tokens[index - 2], "/OC");
                    context.push(Some(properties.as_dict().unwrap().get(tokens[index - 1].trim_start_matches('/').as_bytes()).unwrap().clone()));
                }
                "BMC" => context.push(None),
                "EMC" => {
                    context.pop().expect("balanced marked content");
                }
                "Do" => {
                    let xobjects = doc.resolve(resources.get(b"XObject").unwrap());
                    let appearance = xobjects.as_dict().unwrap().get(tokens[index - 1].trim_start_matches('/').as_bytes()).unwrap().clone();
                    paints.push((appearance, context.iter().rev().find_map(Clone::clone)));
                }
                _ => {}
            }
        }
    }
    assert!(context.is_empty());
    paints
}

#[test]
fn flatten_tools_keep_membership_bound_to_appearance_after_save_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    for (name, membership, flags, ap_membership) in [
        ("off", "/OC 4 0 R", 4, ""),
        ("on", "/OC 5 0 R", 4, ""),
        ("ocmd", "/OC 9 0 R", 4, ""),
        ("direct_ocmd", "/OC << /Type /OCMD /OCGs [4 0 R] /P /AllOn /Private (keep inline) >>", 4, ""),
        ("independent_ap", "/OC 4 0 R", 4, "/OC 5 0 R"),
        ("visible", "", 4, ""),
        ("hidden_flag", "", 2, ""),
    ] {
        let input = fixture(membership, flags, ap_membership);
        let original = Document::open(Arc::new(input.clone())).unwrap();
        let expected = original.get(ObjRef::new(8, 0)).as_dict().unwrap().get(b"OC").cloned();
        std::fs::write(directory.path().join("input.pdf"), &input).unwrap();
        let mut a = Automation::new().with_root(directory.path()).unwrap();
        let id = call(&mut a, "doc_open", json!({"path": "input.pdf"}))["doc"].as_u64().unwrap();
        call(&mut a, "doc_flatten", json!({"doc": id, "comments": true, "fields": false}));
        call(&mut a, "doc_save", json!({"doc": id, "path": "saved.pdf"}));
        let reopened = call(&mut a, "doc_open", json!({"path": "saved.pdf"}))["doc"].as_u64().unwrap();
        let disk = std::fs::read(directory.path().join("saved.pdf")).unwrap();
        let reopened_bytes = a.session().save_bytes(DocId(reopened)).unwrap();
        for bytes in [disk.as_slice(), reopened_bytes.as_slice()] {
            let doc = Document::open(Arc::new(bytes.to_vec())).unwrap();
            let mut refs = HashMap::new();
            let before_catalog = original.get(original.root().unwrap());
            let after_catalog = doc.get(doc.root().unwrap());
            same_graph(
                &original,
                before_catalog.as_dict().unwrap().get(b"OCProperties").unwrap(),
                &doc,
                after_catalog.as_dict().unwrap().get(b"OCProperties").unwrap(),
                &mut refs,
            );
            let original_resources = resources(&original);
            let actual_resources = resources(&doc);
            let original_properties = original.resolve(original_resources.get(b"Properties").unwrap());
            let actual_properties = doc.resolve(actual_resources.get(b"Properties").unwrap());
            for (key, value) in original_properties.as_dict().unwrap().iter() {
                same_graph(&original, value, &doc, actual_properties.as_dict().unwrap().get(key).unwrap(), &mut refs);
            }
            let xobjects = doc.resolve(actual_resources.get(b"XObject").unwrap());
            let preserved_ap = xobjects.as_dict().unwrap().get(b"SharedAppearance").unwrap();
            same_graph(&original, &Object::Ref(ObjRef::new(7, 0)), &doc, preserved_ap, &mut refs);
            let actual = paints(&doc);
            if flags == 2 {
                assert!(actual.is_empty());
                let page = pdfcraft_model::pages(&doc).remove(0);
                let annots = doc.resolve(page.dict.get(b"Annots").unwrap());
                let annots = annots.as_array().unwrap();
                assert_eq!(annots.len(), 1);
                same_graph(&original, &Object::Ref(ObjRef::new(8, 0)), &doc, &annots[0], &mut refs);
            } else {
                assert_eq!(actual.len(), 1, "{name}: one original appearance invocation");
                assert_eq!(&actual[0].0, preserved_ap, "{name}: Do invokes the preserved AP object");
                match (&expected, &actual[0].1) {
                    (Some(expected), Some(actual)) => same_graph(&original, expected, &doc, actual, &mut refs),
                    (None, None) => {}
                    _ => panic!("{name}: exact BDC/Do binding loses membership"),
                }
            }
        }
        if let Some(path) = std::env::var_os("PDFCRAFT_3213_WORKFLOW_ARTIFACTS") {
            let path = std::path::Path::new(&path);
            std::fs::write(path.join(format!("{name}-before.pdf")), input).unwrap();
            std::fs::write(path.join(format!("{name}-saved.pdf")), disk).unwrap();
            std::fs::write(path.join(format!("{name}-reopened.pdf")), reopened_bytes.as_slice()).unwrap();
        }
    }
}
