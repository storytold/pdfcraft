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

/// Two pages, listed by the /Kids array object 6; page 3 holds text field 5, listed in /Fields.
fn form() -> Vec<u8> {
    write_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>",
        "<< /Type /Pages /Kids 6 0 R /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [5 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /V (Ann) /Rect [20 200 120 220] /P 3 0 R >>",
        "[3 0 R 4 0 R]",
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

/// `bytes` is refused, for (among others) `expected`.
fn assert_refused(what: &str, bytes: &[u8], expected: &str) {
    let (status, modification) = verdict(bytes);
    assert!(
        matches!(&modification, Modification::Disallowed(kinds) if kinds.contains(&expected)) && status != Status::Valid,
        "{what}: {status:?} {modification:?}, expected {expected}"
    );
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
        kind::PAGES,
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
        kind::PAGES,
    );
    // Reordering the pages through the node's /Kids array object.
    assert_refused(
        "pages reordered",
        &edit_after(&signed, |doc| doc.set(r(6), Object::Array(vec![Object::Ref(r(4)), Object::Ref(r(3))]))),
        kind::PAGES,
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
        kind::STRUCTURE,
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
        assert_refused(&format!("P={p} field dropped"), &set_fields(Vec::new()), kind::OTHER);
        let dead = ObjRef { num: 5, generation: 3 };
        assert_refused(&format!("P={p} field listed by a dead generation"), &set_fields(vec![Object::Ref(dead)]), kind::OTHER);
        // The field's slot taken by a newer generation: another object, listed in its place.
        let newer = ObjRef { num: 5, generation: 1 };
        let replaced = edit_after(&set_fields(vec![Object::Ref(newer)]), |doc| {
            let mut field = doc.get(r(5)).as_dict().cloned().unwrap();
            field.set(b"V".to_vec(), PdfString::text("Eve"));
            doc.set(newer, Object::Dict(field));
        });
        assert_refused(&format!("P={p} field replaced by a newer generation"), &replaced, kind::OTHER);
    }
}

#[test]
fn a_signed_fields_widget_may_be_filled_but_not_hidden_or_moved() {
    let signed = certified(2);
    let edit = |f: &dyn Fn(&mut pdfcraft_cos::Dict)| edit_after(&signed, |doc| doc.update_dict(r(5), |w| f(w)).unwrap());
    let filled = edit(&|w| w.set(b"V".to_vec(), PdfString::text("Bob")));
    assert_eq!(verdict(&filled), (Status::Valid, Modification::Allowed(vec![kind::FORM_FILL])));
    assert_refused("hidden", &edit(&|w| w.set(b"F".to_vec(), Object::Int(2))), kind::PAGE_CONTENT);
    assert_refused("moved", &edit(&|w| w.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(0); 4]))), kind::PAGE_CONTENT);
}

/// A new appearance stream drawing `content` over a 300 × 300 page.
fn appearance(doc: &mut Document, content: &str) -> Object {
    let mut d = pdfcraft_cos::Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array([0, 0, 300, 300].map(Object::Int).to_vec()));
    let stream = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(d, content.as_bytes().to_vec())));
    Object::Dict(pdfcraft_cos::Dict::from_iter([(b"N".to_vec(), Object::Ref(stream))]))
}

/// Filling a form changes a field's value and the appearance that shows it. Under P=2, a widget
/// given a new appearance, border or default appearance with its value unchanged, or a new
/// widget added to the page and the form, is a change to the annotations (P=3 only), whatever
/// it draws over the signed page. A fill that regenerates the appearance stays a form fill.
#[test]
fn a_widget_restyled_or_added_without_a_value_change_is_not_a_form_fill() {
    let signed = certified(2);
    let edit = |f: &dyn Fn(&mut Document)| edit_after(&signed, |doc| f(doc));
    let restyled = edit(&|doc| {
        let ap = appearance(doc, "1 1 1 rg 0 0 300 300 re f");
        doc.update_dict(r(5), |w| w.set(b"AP".to_vec(), ap)).unwrap();
    });
    assert_refused("appearance replaced, value unchanged", &restyled, kind::COMMENTS);
    let bordered = edit(&|doc| {
        doc.update_dict(r(5), |w| {
            w.set(b"MK".to_vec(), Object::Dict(pdfcraft_cos::Dict::from_iter([(b"BG".to_vec(), Object::Array(vec![Object::Int(1)]))])));
            w.set(b"DA".to_vec(), PdfString::text("/Helv 0 Tf 0 g"));
        })
        .unwrap();
    });
    assert_refused("border and default appearance changed", &bordered, kind::COMMENTS);
    let added = edit(&|doc| {
        let ap = appearance(doc, "1 1 1 rg 0 0 300 300 re f");
        let mut w = pdfcraft_cos::Dict::new();
        w.set(b"Type".to_vec(), Object::name("Annot"));
        w.set(b"Subtype".to_vec(), Object::name("Widget"));
        w.set(b"FT".to_vec(), Object::name("Tx"));
        w.set(b"T".to_vec(), PdfString::text("Cover"));
        w.set(b"Rect".to_vec(), Object::Array([0, 0, 300, 300].map(Object::Int).to_vec()));
        w.set(b"P".to_vec(), Object::Ref(r(3)));
        w.set(b"AP".to_vec(), ap);
        let w = doc.add(Object::Dict(w));
        doc.update_dict(r(3), |p| {
            if let Some(Object::Array(a)) = p.get_mut(b"Annots") {
                a.push(Object::Ref(w));
            }
        })
        .unwrap();
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| {
            if let Some(Object::Dict(form)) = c.get_mut(b"AcroForm")
                && let Some(Object::Array(fields)) = form.get_mut(b"Fields")
            {
                fields.push(Object::Ref(w));
            }
        })
        .unwrap();
    });
    assert_refused("a page-sized text widget added", &added, kind::COMMENTS);
    // The control: a fill with a regenerated appearance.
    let filled = edit(&|doc| {
        let ap = appearance(doc, "/Tx BMC BT /Helv 12 Tf 2 4 Td (Bob) Tj ET EMC");
        doc.update_dict(r(5), |w| {
            w.set(b"V".to_vec(), PdfString::text("Bob"));
            w.set(b"AP".to_vec(), ap);
        })
        .unwrap();
    });
    assert_eq!(verdict(&filled), (Status::Valid, Modification::Allowed(vec![kind::FORM_FILL])));
}

/// A radio group: field 5 holds the value, and its widgets 6 and 7 on page 3 hold none of their
/// own, only the state each shows (`/AS`).
fn radio_form() -> Vec<u8> {
    write_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [6 0 R 7 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] >>",
        "<< /FT /Btn /Ff 49152 /T (Choice) /V /Off /Kids [6 0 R 7 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [20 200 40 220] /P 3 0 R /AS /Off /AP << /N << /A 8 0 R /Off 9 0 R >> >> >>",
        "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [60 200 80 220] /P 3 0 R /AS /Off /AP << /N << /B 8 0 R /Off 9 0 R >> >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 14 >>\nstream\n0 0 20 20 re f\nendstream",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 0 >>\nstream\n\nendstream",
    ])
}

/// Choosing a radio button sets the group's value on the field and the state of each widget: a
/// form fill, though no widget holds a value of its own.
#[test]
fn choosing_a_radio_button_whose_value_is_on_its_parent_is_a_form_fill() {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let opts = SignOptions {
        page: 1,
        rect: Some([20.0, 100.0, 220.0, 150.0]),
        date: "D:20261002120000+01'00'".into(),
        certify: Some(2),
        ..SignOptions::default()
    };
    let signed = pdfcraft_sign::sign(&open(&radio_form()), &id, &opts).unwrap();
    let chosen = edit_after(&signed, |doc| {
        doc.update_dict(r(5), |f| f.set(b"V".to_vec(), Object::name("A"))).unwrap();
        doc.update_dict(r(6), |w| w.set(b"AS".to_vec(), Object::name("A"))).unwrap();
    });
    assert_eq!(verdict(&chosen), (Status::Valid, Modification::Allowed(vec![kind::FORM_FILL])));
    // The same widget restyled without a new value is no fill.
    let restyled = edit_after(&signed, |doc| doc.update_dict(r(6), |w| w.set(b"AS".to_vec(), Object::name("A"))).unwrap());
    assert_refused("a radio widget's state changed without the group's value", &restyled, kind::COMMENTS);
}
