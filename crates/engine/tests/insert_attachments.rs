use std::{collections::BTreeMap, sync::Arc};

use pdfcraft_cos::{Dict, Document, Object, PdfString, SaveOptions, Stream, write_full};
use pdfcraft_engine::{Edit, Session};
use pdfcraft_organize::FieldConflicts;

fn dict<const N: usize>(entries: [(&str, Object); N]) -> Dict {
    let mut out = Dict::new();
    for (key, value) in entries {
        out.set(key.as_bytes().to_vec(), value);
    }
    out
}

fn packet(title: &str, attachment_key: Option<Vec<u8>>, indirect_names: bool) -> Arc<Vec<u8>> {
    let mut doc = Document::new_empty();
    let pages = doc.add(Object::Null);
    let font = doc.add(dict([("Type", Object::name("Font")), ("Subtype", Object::name("Type1")), ("BaseFont", Object::name("Helvetica"))]));
    let body = format!("BT /F1 12 Tf 72 720 Td ({title}) Tj ET");
    let content = doc.add(Object::Stream(Stream::from_raw(Dict::new(), body.into_bytes())));
    let page = doc.add(dict([
        ("Type", Object::name("Page")),
        ("Parent", Object::Ref(pages)),
        ("MediaBox", Object::Array([0, 0, 612, 792].map(Object::Int).to_vec())),
        ("Contents", Object::Ref(content)),
        ("Resources", Object::Dict(dict([("Font", Object::Dict(dict([("F1", Object::Ref(font))])))]))),
    ]));
    doc.set(pages, dict([("Type", Object::name("Pages")), ("Kids", Object::Array(vec![Object::Ref(page)])), ("Count", Object::Int(1))]));
    let destinations = doc.add(dict([(
        "Names",
        Object::Array(vec![Object::String(PdfString::text("PaymentSchedule")), Object::Array(vec![Object::Ref(page), Object::name("Fit")])]),
    )]));
    let mut names = dict([("Dests", Object::Ref(destinations))]);
    if let Some(key) = attachment_key {
        let payload = format!("{title}: vendor insurance verified; review owner Procurement Operations; next review 2027-01-31.\n");
        let file = doc.add(Object::Stream(Stream::from_raw(
            dict([
                ("Type", Object::name("EmbeddedFile")),
                ("Subtype", Object::name("text/plain")),
                ("Params", Object::Dict(dict([("Size", Object::Int(payload.len() as i64))]))),
            ]),
            payload.into_bytes(),
        )));
        let spec = doc.add(dict([
            ("Type", Object::name("Filespec")),
            ("F", Object::String(PdfString::text("vendor-compliance.txt"))),
            ("EF", Object::Dict(dict([("F", Object::Ref(file))]))),
        ]));
        let leaf = doc.add(dict([
            ("Names", Object::Array(vec![Object::String(PdfString::literal(key.clone())), Object::Ref(spec)])),
            ("Limits", Object::Array(vec![Object::String(PdfString::literal(key.clone())), Object::String(PdfString::literal(key))])),
        ]));
        let tree = doc.add(dict([("Kids", Object::Array(vec![Object::Ref(leaf)]))]));
        names.set(b"EmbeddedFiles".to_vec(), Object::Ref(tree));
    }
    let names = if indirect_names { Object::Ref(doc.add(names)) } else { Object::Dict(names) };
    let root = doc.add(dict([("Type", Object::name("Catalog")), ("Pages", Object::Ref(pages)), ("Names", names)]));
    doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(root));
    Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())
}

fn attachments(doc: &Document) -> Vec<(Vec<u8>, Vec<u8>)> {
    fn visit(doc: &Document, tree: &Object, out: &mut Vec<(Vec<u8>, Vec<u8>)>) {
        let tree = doc.resolve(tree);
        let tree = tree.as_dict().unwrap();
        if let Some(names) = tree.get(b"Names") {
            for pair in doc.resolve(names).as_array().unwrap().as_chunks::<2>().0 {
                let key = pair[0].as_string().unwrap();
                let spec = doc.resolve(&pair[1]);
                let ef = doc.resolve(spec.as_dict().unwrap().get(b"EF").unwrap());
                let file = doc.resolve(ef.as_dict().unwrap().get(b"F").unwrap());
                let Object::Stream(file) = file.as_ref() else { panic!("embedded file stream expected") };
                out.push((key.bytes.to_vec(), file.decoded().unwrap()));
            }
        }
        if let Some(kids) = tree.get(b"Kids") {
            for kid in doc.resolve(kids).as_array().unwrap() {
                visit(doc, kid, out);
            }
        }
    }
    let catalog = doc.get(doc.root().unwrap());
    let names = doc.resolve(catalog.as_dict().unwrap().get(b"Names").unwrap());
    let mut out = Vec::new();
    if let Some(tree) = names.as_dict().unwrap().get(b"EmbeddedFiles") {
        visit(doc, tree, &mut out);
    }
    out
}

fn assert_destination_retained(doc: &Document) {
    let catalog = doc.get(doc.root().unwrap());
    let names = doc.resolve(catalog.as_dict().unwrap().get(b"Names").unwrap());
    let dests = doc.resolve(names.as_dict().unwrap().get(b"Dests").expect("target destination tree retained"));
    let values = dests.as_dict().unwrap().get(b"Names").unwrap().as_array().unwrap();
    assert_eq!(values[0].as_string().unwrap().to_text(), "PaymentSchedule");
    let page = values[1].as_array().unwrap()[0].as_ref().unwrap();
    assert_eq!(doc.get(page).as_dict().unwrap().name(b"Type"), Some(b"Page".as_slice()));
}

fn insert(bytes: Arc<Vec<u8>>) -> Edit {
    Edit::InsertPagesFrom { name: "Facilities Compliance Annex.pdf".into(), bytes, pages: None, at: 1, field_conflicts: FieldConflicts::Rename }
}

#[test]
fn inserted_catalog_attachments_follow_native_undo_redo() {
    let unicode = "合规审查.txt";
    let be: Vec<u8> = [0xfe, 0xff].into_iter().chain(unicode.encode_utf16().flat_map(u16::to_be_bytes)).collect();
    let le: Vec<u8> = [0xff, 0xfe].into_iter().chain(unicode.encode_utf16().flat_map(u16::to_le_bytes)).collect();
    for key in [b"vendor-compliance.txt".to_vec(), be, le] {
        // Check raw keys to preserve the incoming encoding, including LE files
        // whose labels PdfString::to_text does not currently decode.
        let mut renamed = key.clone();
        match key.get(..2) {
            Some([0xfe, 0xff]) => renamed.extend(" (2)".encode_utf16().flat_map(u16::to_be_bytes)),
            Some([0xff, 0xfe]) => renamed.extend(" (2)".encode_utf16().flat_map(u16::to_le_bytes)),
            _ => renamed.extend_from_slice(b" (2)"),
        }
        for indirect in [false, true] {
            let target = packet("Facilities Service Agreement", Some(key.clone()), indirect);
            let source = packet("Facilities Compliance Annex", Some(key.clone()), true);
            let mut session = Session::new();
            session.set_javascript(false);
            let id = session.open("Facilities Contract.pdf", None, target, None).unwrap();
            session.apply(id, Edit::Batch { label: "Merge Annex".into(), edits: vec![insert(source.clone())] }).unwrap();
            let bytes = session.save_bytes(id).unwrap();
            let merged = Document::open(bytes).unwrap();
            let entries = attachments(&merged);
            assert_eq!(entries.len(), 2, "target and source attachments must survive");
            assert!(entries.windows(2).all(|p| p[0].0 < p[1].0), "name tree keys sorted and unique");
            let by_key: BTreeMap<_, _> = entries.into_iter().collect();
            assert!(by_key[&key].starts_with(b"Facilities Service Agreement:"));
            assert!(by_key[&renamed].starts_with(b"Facilities Compliance Annex:"));
            assert_destination_retained(&merged);
            assert_eq!(session.get(id).unwrap().info.pages.len(), 2);
            assert_eq!(session.get(id).unwrap().can_undo(), Some("Merge Annex"));
            session.undo(id).unwrap();
            let undone = Document::open(session.save_bytes(id).unwrap()).unwrap();
            assert_eq!(attachments(&undone).len(), 1);
            assert_destination_retained(&undone);
            assert_eq!(session.get(id).unwrap().info.pages.len(), 1);
            session.redo(id).unwrap();
            let redone = Document::open(session.save_bytes(id).unwrap()).unwrap();
            assert_eq!(attachments(&redone).len(), 2);
            assert_destination_retained(&redone);
            assert_eq!(session.get(id).unwrap().info.pages.len(), 2);
            assert_eq!(attachments(&Document::open(source).unwrap()).len(), 1);
        }
    }
}

#[test]
fn inserting_into_a_document_without_attachments_retains_other_names() {
    let mut session = Session::new();
    session.set_javascript(false);
    let id = session.open("Facilities Contract.pdf", None, packet("Facilities Service Agreement", None, true), None).unwrap();
    session.apply(id, insert(packet("Facilities Compliance Annex", Some(b"vendor-compliance.txt".to_vec()), true))).unwrap();
    let result = Document::open(session.save_bytes(id).unwrap()).unwrap();
    assert_eq!(attachments(&result).len(), 1);
    assert_destination_retained(&result);
    session.undo(id).unwrap();
    assert!(attachments(&Document::open(session.save_bytes(id).unwrap()).unwrap()).is_empty());
}

#[test]
fn inserting_without_source_attachments_does_not_rewrite_target_names() {
    let mut session = Session::new();
    session.set_javascript(false);
    let id = session
        .open("Facilities Contract.pdf", None, packet("Facilities Service Agreement", Some(b"contract-evidence.txt".to_vec()), true), None)
        .unwrap();
    let before = Document::open(session.save_bytes(id).unwrap()).unwrap();
    let names = before.get(before.root().unwrap()).as_dict().unwrap().get(b"Names").unwrap().clone();
    assert!(matches!(names, Object::Ref(_)));
    session.apply(id, insert(packet("Facilities Payment Schedule", None, false))).unwrap();
    let after = Document::open(session.save_bytes(id).unwrap()).unwrap();
    assert_eq!(after.get(after.root().unwrap()).as_dict().unwrap().get(b"Names"), Some(&names));
    assert_eq!(attachments(&after), attachments(&before));
    assert_destination_retained(&after);
}

#[test]
fn selecting_no_pages_does_not_import_catalog_attachments() {
    let mut session = Session::new();
    session.set_javascript(false);
    let id = session
        .open("Facilities Contract.pdf", None, packet("Facilities Service Agreement", Some(b"contract-evidence.txt".to_vec()), true), None)
        .unwrap();
    let before = Document::open(session.save_bytes(id).unwrap()).unwrap();
    session
        .apply(
            id,
            Edit::InsertPagesFrom {
                name: "Facilities Compliance Annex.pdf".into(),
                bytes: packet("Facilities Compliance Annex", Some(b"vendor-compliance.txt".to_vec()), true),
                pages: Some(Vec::new()),
                at: 1,
                field_conflicts: FieldConflicts::Rename,
            },
        )
        .unwrap();
    assert_eq!(session.get(id).unwrap().info.pages.len(), 1);
    let after = Document::open(session.save_bytes(id).unwrap()).unwrap();
    assert_eq!(attachments(&after), attachments(&before));
    assert_destination_retained(&after);
}

#[test]
fn failed_insert_batch_rolls_back_pages_and_attachments() {
    let mut session = Session::new();
    session.set_javascript(false);
    let id = session
        .open("Facilities Contract.pdf", None, packet("Facilities Service Agreement", Some(b"contract-evidence.txt".to_vec()), true), None)
        .unwrap();
    assert!(
        session
            .apply(
                id,
                Edit::Batch {
                    label: "Merge Annex".into(),
                    edits: vec![
                        insert(packet("Facilities Compliance Annex", Some(b"vendor-compliance.txt".to_vec()), true)),
                        Edit::RotatePages { pages: vec![99], degrees: 90 },
                    ]
                }
            )
            .is_err()
    );
    assert_eq!(session.get(id).unwrap().info.pages.len(), 1);
    assert_eq!(session.get(id).unwrap().can_undo(), None);
    let result = Document::open(session.save_bytes(id).unwrap()).unwrap();
    assert_eq!(attachments(&result).len(), 1);
    assert_destination_retained(&result);
}
