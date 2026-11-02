//! Full rewrites must preserve the graph when temporary parse/packing caches are evicted.
//! Fixtures are generated here; no third-party PDF assets are used.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Dict, Document, NewEncryption, ObjRef, Object, PdfString, SaveOptions, XrefEntry, write_full};

const NODES: usize = 260;
const EDITED: usize = 129;

fn node(i: usize) -> ObjRef {
    ObjRef::new(301 + i as u32, (i % 5 + 1) as u16)
}

fn stream_body(i: usize) -> Vec<u8> {
    format!("opaque stream {i}: preserve these bytes\n").into_bytes()
}

/// Nonzero generations, sparse numbers, shared references, a cycle and interleaved streams.
/// A raw classic xref avoids depending on the writer under test to construct the input.
fn fixture() -> Vec<u8> {
    let mut objects = vec![
        (ObjRef::new(10, 2), format!("<< /Type /Catalog /Pages 20 0 R /Extension {} /Shared 42 3 R /Freed 43 8 R /Missing 9000 0 R >>", node(0))),
        (ObjRef::new(20, 0), "<< /Type /Pages /Kids [] /Count 0 >>".into()),
        (ObjRef::new(42, 3), "<< /Token (shared extension value) >>".into()),
        (ObjRef::new(43, 8), "<< /Token (deleted extension value) >>".into()),
        (ObjRef::new(70, 4), "<< /Title (original title) >>".into()),
    ];
    for i in 0..NODES {
        let fields = format!("/Value {i} /Next {} /Shared 42 3 R /Opaque << /Nested [42 3 R (retained data)] >>", node((i + 1) % NODES));
        let body = if i % 19 == 0 {
            let raw = stream_body(i);
            format!("<< {fields} /Length {} >>\nstream\n{}endstream", raw.len(), String::from_utf8(raw).unwrap())
        } else {
            format!("<< {fields} >>")
        };
        objects.push((node(i), body));
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = BTreeMap::new();
    for (reference, body) in objects {
        offsets.insert(reference.num, (bytes.len(), reference.generation));
        bytes.extend_from_slice(format!("{} {} obj\n{body}\nendobj\n", reference.num, reference.generation).as_bytes());
    }
    let size = offsets.last_key_value().unwrap().0 + 1;
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
    for n in 0..size {
        match offsets.get(&n) {
            Some((offset, generation)) => bytes.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes()),
            None => bytes.extend_from_slice(b"0000000000 65535 f \n"),
        }
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {size} /Root 10 2 R /Info 70 4 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    bytes
}

fn edited_document() -> Document {
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    assert!(doc.repair_log().is_empty());
    assert_eq!(doc.root(), Some(ObjRef::new(10, 2)));
    assert_eq!(doc.generation(node(EDITED).num), node(EDITED).generation);
    doc.update_dict(node(EDITED), |d| d.set(b"Value".to_vec(), Object::Int(8129))).unwrap();
    doc.free(ObjRef::new(43, 8));
    let added = doc.add(Object::String(PdfString::text("added extension value")));
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"Added".to_vec(), Object::Ref(added))).unwrap();
    doc
}

fn check_graph(doc: &Document) {
    assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
    let catalog = doc.get(doc.root().unwrap());
    let catalog = catalog.as_dict().unwrap();
    let shared = catalog.reference(b"Shared").unwrap();
    let shared_object = doc.get(shared);
    assert_eq!(shared_object.as_dict().unwrap().get(b"Token").unwrap().as_string().unwrap().to_text(), "shared extension value");
    assert_eq!(catalog.get(b"Freed"), Some(&Object::Null));
    assert_eq!(catalog.get(b"Missing"), Some(&Object::Null));
    assert_eq!(doc.resolve(catalog.get(b"Added").unwrap()).as_string().unwrap().to_text(), "added extension value");
    let first = catalog.reference(b"Extension").unwrap();
    let mut current = first;
    let mut visited = HashSet::new();
    for i in 0..NODES {
        assert!(visited.insert(current), "cycle closed before node {i}");
        let object = doc.get(current);
        let dict = object.as_dict().unwrap();
        assert_eq!(dict.int(b"Value"), Some(if i == EDITED { 8129 } else { i as i64 }));
        assert_eq!(dict.reference(b"Shared"), Some(shared));
        let opaque = dict.get(b"Opaque").unwrap().as_dict().unwrap().get(b"Nested").unwrap().as_array().unwrap();
        assert_eq!(opaque.first(), Some(&Object::Ref(shared)));
        assert_eq!(opaque.get(1).unwrap().as_string().unwrap().to_text(), "retained data");
        if i % 19 == 0 {
            let Object::Stream(stream) = object.as_ref() else { panic!("node {i} lost its stream") };
            assert_eq!(stream.decoded().unwrap(), stream_body(i));
        } else {
            assert!(matches!(object.as_ref(), Object::Dict(_)));
        }
        current = dict.reference(b"Next").unwrap();
    }
    assert_eq!(current, first, "the extension cycle is preserved");
    assert!(doc.object_numbers().into_iter().all(|n| doc.generation(n) == 0));
}

#[test]
fn full_rewrites_preserve_graph_across_multiple_object_streams_and_evictions() {
    for object_streams in [false, true] {
        let doc = edited_document();
        let opts = SaveOptions { object_streams, mod_date: Some("D:20261008000000Z".into()), ..SaveOptions::default() };
        let saved = write_full(&doc, &opts).unwrap();
        assert_eq!(saved, write_full(&doc, &opts).unwrap(), "unchanged inputs produce identical bytes");
        let back = Document::open(Arc::new(saved)).unwrap();
        check_graph(&back);
        let info = back.resolve(back.trailer().get(b"Info").unwrap());
        let info = info.as_dict().unwrap();
        assert_eq!(info.get(b"Title").unwrap().as_string().unwrap().to_text(), "original title");
        assert_eq!(info.get(b"ModDate").unwrap().as_string().unwrap().to_text(), "D:20261008000000Z");
        let containers: HashSet<_> = back
            .object_numbers()
            .into_iter()
            .filter_map(|num| match back.xref_entry(num) {
                Some(XrefEntry::InStream { stream, .. }) => Some(stream),
                _ => None,
            })
            .collect();
        if object_streams {
            assert!(containers.len() >= 3, "fixture must span multiple packed-object chunks");
        } else {
            assert!(containers.is_empty());
        }
        // Read an already packed input as well: eviction must not lose decoded ObjStm data.
        check_graph(&Document::open(Arc::new(write_full(&back, &opts).unwrap())).unwrap());
        // Writing must not apply the date stamp or consume the caller's edits.
        let original_info = doc.resolve(doc.trailer().get(b"Info").unwrap());
        assert!(!original_info.as_dict().unwrap().contains(b"ModDate"));
        assert!(doc.is_modified());
        assert_eq!(doc.get(node(EDITED)).as_dict().unwrap().int(b"Value"), Some(8129));
    }
}

fn protect(doc: &mut Document, algorithm: Algorithm) {
    doc.set_encryption(&NewEncryption {
        algorithm,
        user_password: "user",
        owner_password: "owner",
        permissions: -4,
        encrypt_metadata: true,
        seed: [23; 32],
    })
    .unwrap();
}

#[test]
fn encrypted_full_rewrites_preserve_data_across_evictions() {
    for algorithm in [Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256] {
        let mut doc = edited_document();
        protect(&mut doc, algorithm);
        let encrypted = Arc::new(write_full(&doc, &SaveOptions::default()).unwrap());
        let cold = Document::open_with_password(encrypted, Some("user")).unwrap();
        // The second rewrite must decrypt lazily from the input, then encrypt the new streams.
        let rewritten = Arc::new(write_full(&cold, &SaveOptions::default()).unwrap());
        assert!(Document::open_with_password(rewritten.clone(), Some("wrong")).is_err());
        let back = Document::open_with_password(rewritten, Some("owner")).unwrap();
        assert!(back.security().is_some());
        check_graph(&back);
    }
}

#[test]
fn encryption_dependencies_stay_outside_object_streams_across_evictions() {
    let mut doc = edited_document();
    protect(&mut doc, Algorithm::Aes256);
    let encrypt = doc.trailer().reference(b"Encrypt").unwrap();
    let filters = doc.get(encrypt).as_dict().unwrap().get(b"CF").cloned().unwrap();
    let filters = doc.add(filters);
    let mut next = filters;
    for i in (0..140).rev() {
        let mut dict = Dict::new();
        dict.set(b"Value".to_vec(), Object::Int(i));
        dict.set(b"Next".to_vec(), Object::Ref(next));
        next = doc.add(dict);
    }
    doc.update_dict(encrypt, |dict| {
        dict.set(b"CF".to_vec(), Object::Ref(filters));
        dict.set(b"PrivateChain".to_vec(), Object::Ref(next));
    })
    .unwrap();
    let saved = Arc::new(write_full(&doc, &SaveOptions::default()).unwrap());
    let cold = Document::open_with_password(saved, Some("user")).unwrap();
    let rewritten = Arc::new(write_full(&cold, &SaveOptions::default()).unwrap());
    let back = Document::open_with_password(rewritten, Some("user")).unwrap();
    check_graph(&back);
    let encrypt = back.trailer().reference(b"Encrypt").unwrap();
    assert!(matches!(back.xref_entry(encrypt.num), Some(XrefEntry::InFile { .. })));
    let encrypt = back.get(encrypt);
    let encrypt = encrypt.as_dict().unwrap();
    let filters = encrypt.reference(b"CF").unwrap();
    assert!(matches!(back.xref_entry(filters.num), Some(XrefEntry::InFile { .. })));
    let mut current = encrypt.reference(b"PrivateChain").unwrap();
    for i in 0..140 {
        assert!(matches!(back.xref_entry(current.num), Some(XrefEntry::InFile { .. })));
        let object = back.get(current);
        let dict = object.as_dict().unwrap();
        assert_eq!(dict.int(b"Value"), Some(i));
        current = dict.reference(b"Next").unwrap();
    }
    assert_eq!(current, filters);
}

#[test]
fn large_packed_strings_round_trip_across_byte_limited_batches() {
    let mut doc = Document::new_empty();
    let mut references = Vec::new();
    // More than 8 MiB in fewer than 100 objects: a byte cap may flush before the count cap.
    for i in 0..70u8 {
        let mut bytes = vec![b'a' + i % 26; 192 * 1024];
        bytes[0] = i;
        references.push(Object::Ref(doc.add(Object::String(PdfString::literal(bytes)))));
    }
    doc.update_dict(doc.root().unwrap(), |dict| dict.set(b"Extension".to_vec(), Object::Array(references))).unwrap();
    let source = Arc::new(write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap());
    drop(doc);
    let cold = Document::open(source).unwrap();
    let back = Document::open(Arc::new(write_full(&cold, &SaveOptions::default()).unwrap())).unwrap();
    assert!(back.repair_log().is_empty());
    let catalog = back.get(back.root().unwrap());
    let values = catalog.as_dict().unwrap().get(b"Extension").unwrap().as_array().unwrap();
    assert_eq!(values.len(), 70);
    for (i, reference) in values.iter().enumerate() {
        let value = back.resolve(reference);
        let bytes = &value.as_string().unwrap().bytes;
        assert_eq!(bytes.len(), 192 * 1024);
        assert_eq!(bytes[0], i as u8);
        assert!(bytes[1..].iter().all(|&b| b == b'a' + i as u8 % 26));
    }
}
