use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, write_full};
use pdfcraft_organize::{combine, page_count};

/// One tagged page with a class whose meaning differs between the two source documents.
fn source(placement: &str, indirect: Option<&str>) -> Document {
    let class = if indirect.is_some() { "10 0 R" } else { "/HeadingClass" };
    let content = "/P <</MCID 0>> BDC 0 0 m 10 10 l S EMC";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 6 0 R /MarkInfo << /Marked true >> /Lang (en-US) >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /StructParents 0 >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "null".into(),
        format!("<< /Type /StructTreeRoot /K [7 0 R] /ParentTree 9 0 R /ClassMap << /HeadingClass << /O /Layout /Placement /{placement} >> >> >>"),
        "<< /Type /StructElem /S /Document /P 6 0 R /K [8 0 R] >>".into(),
        format!("<< /Type /StructElem /S /P /P 7 0 R /Pg 3 0 R /K 0 /C {class} >>"),
        "<< /Nums [0 [8 0 R]] >>".into(),
        indirect.unwrap_or("/HeadingClass").into(),
        "/HeadingClass".into(),
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
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Document::open(Arc::new(bytes)).unwrap()
}

fn saved_combination(a: &Document, b: &Document) -> Document {
    let merged = combine(&[("A", a), ("B", b)]).unwrap();
    Document::open(Arc::new(write_full(&merged, &SaveOptions::default()).unwrap())).unwrap()
}

#[test]
fn conflicting_indirect_class_names_keep_each_sources_meaning() {
    for indirect in ["/HeadingClass", "[/HeadingClass 3]", "[11 0 R 3]"] {
        let out = saved_combination(&source("Block", None), &source("Inline", Some(indirect)));
        let catalog = out.get(out.root().unwrap());
        let root = out.resolve(catalog.as_dict().unwrap().get(b"StructTreeRoot").unwrap());
        let root = root.as_dict().unwrap();
        let classes = out.resolve(root.get(b"ClassMap").unwrap());
        let classes = classes.as_dict().unwrap();
        let documents = root.get(b"K").unwrap().as_array().unwrap();
        for (document, expected) in documents.iter().zip([b"Block".as_slice(), b"Inline".as_slice()]) {
            let document = out.resolve(document);
            let children = document.as_dict().unwrap().get(b"K").unwrap();
            let heading = out.resolve(children.as_array().and_then(|children| children.first()).unwrap_or(children));
            let class = out.resolve(heading.as_dict().unwrap().get(b"C").unwrap());
            let name = match class.as_array() {
                Some(array) => {
                    assert_eq!(array[1].as_int(), Some(3), "the class revision is unchanged");
                    out.resolve(&array[0]).as_name().unwrap().to_vec()
                }
                None => class.as_name().unwrap().to_vec(),
            };
            let attributes = out.resolve(classes.get(&name).unwrap());
            assert_eq!(attributes.as_dict().unwrap().name(b"Placement"), Some(expected), "{indirect}");
        }
    }
}

#[test]
fn a_cyclic_class_reference_is_bounded_and_keeps_both_pages() {
    let out = saved_combination(&source("Block", None), &source("Inline", Some("10 0 R")));
    assert_eq!(page_count(&out).unwrap(), 2);
}

fn role_source(standard: &str, indirect: bool) -> Document {
    let mut doc = source("Block", None);
    let mut roles = Dict::new();
    roles.set(b"CustomRole".to_vec(), Object::name(standard));
    doc.update_dict(ObjRef::new(6, 0), |d| d.set(b"RoleMap".to_vec(), Object::Dict(roles))).unwrap();
    doc.update_dict(ObjRef::new(8, 0), |d| {
        d.set(b"S".to_vec(), if indirect { Object::Ref(ObjRef::new(11, 0)) } else { Object::name("CustomRole") });
    })
    .unwrap();
    doc.set(ObjRef::new(11, 0), Object::name("CustomRole"));
    doc
}

#[test]
fn conflicting_indirect_roles_keep_each_sources_standard_role() {
    let out = saved_combination(&role_source("H1", false), &role_source("P", true));
    let catalog = out.get(out.root().unwrap());
    let root = out.resolve(catalog.as_dict().unwrap().get(b"StructTreeRoot").unwrap());
    let root = root.as_dict().unwrap();
    let roles = out.resolve(root.get(b"RoleMap").unwrap());
    let roles = roles.as_dict().unwrap();
    for (document, expected) in root.get(b"K").unwrap().as_array().unwrap().iter().zip([b"H1".as_slice(), b"P".as_slice()]) {
        let document = out.resolve(document);
        let children = document.as_dict().unwrap().get(b"K").unwrap();
        let element = out.resolve(children.as_array().and_then(|children| children.first()).unwrap_or(children));
        let role = out.resolve(element.as_dict().unwrap().get(b"S").unwrap());
        assert_eq!(out.resolve(roles.get(role.as_name().unwrap()).unwrap()).as_name(), Some(expected));
    }
}

#[test]
fn a_cyclic_role_reference_is_bounded_and_keeps_both_pages() {
    let mut second = role_source("P", true);
    second.set(ObjRef::new(11, 0), Object::Ref(ObjRef::new(11, 0)));
    assert_eq!(page_count(&saved_combination(&role_source("H1", false), &second)).unwrap(), 2);
}

fn header_source(indirect_array: bool) -> Document {
    let mut doc = source("Block", None);
    doc.update_dict(ObjRef::new(7, 0), |d| {
        d.set(b"K".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(8, 0)), Object::Ref(ObjRef::new(12, 0))]))
    })
    .unwrap();
    doc.update_dict(ObjRef::new(8, 0), |d| d.set(b"ID".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"shared".to_vec())))).unwrap();
    let mut cell = Dict::new();
    cell.set(b"Type".to_vec(), Object::name("StructElem"));
    cell.set(b"S".to_vec(), Object::name("TD"));
    cell.set(b"P".to_vec(), Object::Ref(ObjRef::new(7, 0)));
    cell.set(b"Pg".to_vec(), Object::Ref(ObjRef::new(3, 0)));
    cell.set(b"A".to_vec(), Object::Ref(ObjRef::new(if indirect_array { 16 } else { 13 }, 0)));
    doc.set(ObjRef::new(12, 0), Object::Dict(cell));
    let mut attributes = Dict::new();
    attributes.set(b"O".to_vec(), Object::name("Table"));
    attributes.set(b"Headers".to_vec(), Object::Ref(ObjRef::new(14, 0)));
    doc.set(ObjRef::new(13, 0), Object::Dict(attributes));
    doc.set(ObjRef::new(14, 0), Object::Array(vec![Object::Ref(ObjRef::new(15, 0))]));
    doc.set(ObjRef::new(15, 0), Object::String(pdfcraft_cos::PdfString::literal(b"shared".to_vec())));
    doc.set(ObjRef::new(16, 0), Object::Array(vec![Object::Ref(ObjRef::new(13, 0)), Object::Int(3)]));
    doc
}

#[test]
fn indirect_header_ids_follow_their_own_renamed_header() {
    for indirect_array in [false, true] {
        let out = saved_combination(&header_source(false), &header_source(indirect_array));
        let catalog = out.get(out.root().unwrap());
        let root = out.resolve(catalog.as_dict().unwrap().get(b"StructTreeRoot").unwrap());
        for document in root.as_dict().unwrap().get(b"K").unwrap().as_array().unwrap() {
            let document = out.resolve(document);
            let children = document.as_dict().unwrap().get(b"K").unwrap().as_array().unwrap();
            let header = out.resolve(&children[0]);
            let id = out.resolve(header.as_dict().unwrap().get(b"ID").unwrap());
            let cell = out.resolve(&children[1]);
            let attributes = out.resolve(cell.as_dict().unwrap().get(b"A").unwrap());
            let attributes = match attributes.as_array() {
                Some(items) => {
                    assert_eq!(items[1].as_int(), Some(3));
                    out.resolve(&items[0])
                }
                None => attributes,
            };
            let headers = out.resolve(attributes.as_dict().unwrap().get(b"Headers").unwrap());
            let actual = out.resolve(&headers.as_array().unwrap()[0]);
            assert_eq!(actual.as_string().unwrap().bytes, id.as_string().unwrap().bytes);
        }
    }
}
