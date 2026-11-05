//! A certified document's page tree, catalog and form keep the roles they had when signed: a
//! later revision that relabels them, drops a field from the form, or hides or moves a signed
//! field's widget is no form fill.

use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, PdfString, SaveOptions, write_incremental};
use pdfcraft_sign::{Modification, SignOptions, Status, TrustStore, kind, pkcs12, signatures};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn open(b: &[u8]) -> Document {
    Document::open(Arc::new(b.to_vec())).unwrap()
}

/// A classic-xref PDF of `objs`, numbered from 1, with object 1 as the catalog.
fn write_pdf(objs: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn r(num: u32) -> ObjRef {
    ObjRef { num, generation: 0 }
}

/// Two pages; page 3 holds text field 5, listed in /Fields.
fn form() -> Vec<u8> {
    write_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [5 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /V (Ann) /Rect [20 200 120 220] /P 3 0 R >>",
    ])
}

fn certified(p: u8) -> Vec<u8> {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let opts = SignOptions {
        page: 1,
        rect: Some([20.0, 100.0, 220.0, 150.0]),
        date: "D:20261002120000+01'00'".into(),
        certify: Some(p),
        ..SignOptions::default()
    };
    pdfcraft_sign::sign(&open(&form()), &id, &opts).unwrap()
}

fn edit_after(signed: &[u8], f: impl FnOnce(&mut Document)) -> Vec<u8> {
    let mut doc = open(signed);
    f(&mut doc);
    write_incremental(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap()
}

/// The certification signature of `bytes`: its status and modification verdict.
fn verdict(bytes: &[u8]) -> (Status, Modification) {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let s = signatures(&open(bytes), bytes, &trust).into_iter().find(|s| s.signed && s.certify.is_some()).unwrap();
    (s.status, s.modification)
}

fn assert_refused(what: &str, bytes: &[u8]) {
    let (status, modification) = verdict(bytes);
    assert!(matches!(modification, Modification::Disallowed(_)) && status != Status::Valid, "{what}: {status:?} {modification:?}");
}

#[test]
fn a_signed_page_tree_node_is_judged_as_the_page_tree() {
    let signed = certified(2);
    // Relabelled as a field and dropping a page.
    assert_refused(
        "node relabelled",
        &edit_after(&signed, |doc| {
            doc.update_dict(r(2), |n| {
                n.remove(b"Type");
                n.set(b"T".to_vec(), PdfString::text("x"));
                n.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(r(4))]));
                n.set(b"Count".to_vec(), Object::Int(1));
            })
            .unwrap();
        }),
    );
    // Turning every page through an inherited attribute.
    assert_refused(
        "inherited rotation",
        &edit_after(&signed, |doc| {
            doc.update_dict(r(2), |n| {
                n.set(b"DR".to_vec(), Object::Dict(Default::default()));
                n.set(b"Rotate".to_vec(), Object::Int(180));
            })
            .unwrap();
        }),
    );
}

#[test]
fn a_signed_catalog_is_judged_as_the_catalog_whatever_its_type() {
    let signed = certified(2);
    assert_refused(
        "catalog relabelled",
        &edit_after(&signed, |doc| {
            let content = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Default::default(), b"0 0 m 300 300 l S".to_vec())));
            let pages = doc.add(Object::Dict(Default::default()));
            let mut page = pdfcraft_cos::Dict::new();
            page.set(b"Parent".to_vec(), Object::Ref(pages));
            page.set(b"MediaBox".to_vec(), Object::Array([0, 0, 300, 300].map(Object::Int).to_vec()));
            page.set(b"Contents".to_vec(), Object::Ref(content));
            let page = doc.add(Object::Dict(page));
            doc.update_dict(pages, |n| {
                n.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
                n.set(b"Count".to_vec(), Object::Int(1));
            })
            .unwrap();
            let root = doc.root().unwrap();
            doc.update_dict(root, |c| {
                c.remove(b"Type");
                c.set(b"FT".to_vec(), Object::name("Tx"));
                c.set(b"Pages".to_vec(), Object::Ref(pages));
            })
            .unwrap();
        }),
    );
}

#[test]
fn a_field_dropped_from_the_form_is_no_change_any_level_permits() {
    for p in [1, 2] {
        let signed = certified(p);
        let set_fields = |fields: Vec<Object>| {
            edit_after(&signed, move |doc| {
                let root = doc.root().unwrap();
                let mut form =
                    doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm")).map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap();
                let kept: Vec<Object> = fields
                    .into_iter()
                    .chain(
                        form.get(b"Fields")
                            .and_then(Object::as_array)
                            .into_iter()
                            .flatten()
                            .filter(|f| f.as_ref().is_some_and(|f| f.num != 5))
                            .cloned(),
                    )
                    .collect();
                form.set(b"Fields".to_vec(), Object::Array(kept));
                doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(form))).unwrap();
            })
        };
        assert_refused(&format!("P={p} field dropped"), &set_fields(Vec::new()));
        assert_refused(&format!("P={p} field listed by a dead generation"), &set_fields(vec![Object::Ref(ObjRef { num: 5, generation: 3 })]));
    }
}

#[test]
fn a_signed_fields_widget_may_be_filled_but_not_hidden_or_moved() {
    let signed = certified(2);
    let edit = |f: &dyn Fn(&mut pdfcraft_cos::Dict)| edit_after(&signed, |doc| doc.update_dict(r(5), |w| f(w)).unwrap());
    let filled = edit(&|w| w.set(b"V".to_vec(), PdfString::text("Bob")));
    assert_eq!(verdict(&filled), (Status::Valid, Modification::Allowed(vec![kind::FORM_FILL])));
    assert_refused("hidden", &edit(&|w| w.set(b"F".to_vec(), Object::Int(2))));
    assert_refused("moved", &edit(&|w| w.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(0); 4]))));
}
