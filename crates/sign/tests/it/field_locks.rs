//! Fields a signature locks (ISO 32000-2 §12.7.5.5 `/Lock`, §12.8.2.4 FieldMDP): a change made
//! after signing to a field the signature locks is not a permitted form fill (#6757).

use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, PdfString, SaveOptions, Stream, write_incremental};
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

const AMOUNT: u32 = 6;
const AMOUNT_AP: u32 = 7;
const NOTE: u32 = 8;
const CITY: u32 = 11;

/// A form with signature field "Approval" (object 5) carrying `lock` as its `/Lock`, a text field
/// "amount" with an appearance, a text field "note", and a field "address" whose kid is
/// "address.city".
fn form(lock: &str) -> Vec<u8> {
    write_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 8 0 R 10 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [5 0 R 6 0 R 8 0 R 11 0 R] >>",
        "<< >>",
        &format!("<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 {lock} >>"),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (amount) /V (100) /Rect [20 200 120 220] /P 3 0 R /F 4 /AP << /N 7 0 R >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 100 20] /Length 26 >>\nstream\nBT /Helv 10 Tf (100) Tj ET\nendstream",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (note) /V (hello) /Rect [20 170 120 190] /P 3 0 R /F 4 >>",
        "<< >>",
        "<< /FT /Tx /T (address) /Kids [11 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /T (city) /Parent 10 0 R /V (Leeds) /Rect [20 140 120 160] /P 3 0 R /F 4 >>",
    ])
}

/// `form(lock)` signed in its "Approval" field as an approval signature, and the trust that
/// makes that signature Valid.
fn signed(lock: &str) -> (Vec<u8>, TrustStore) {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let opts = SignOptions { field: Some("Approval".into()), date: "D:20261002120000+01'00'".into(), ..SignOptions::default() };
    let bytes = pdfcraft_sign::sign(&open(&form(lock)), &id, &opts).unwrap();
    let mut trust = TrustStore::default();
    trust.certs.push(id.certificate.clone());
    (bytes, trust)
}

fn approval(bytes: &[u8], trust: &TrustStore) -> pdfcraft_sign::SignatureInfo {
    signatures(&open(bytes), bytes, trust).into_iter().find(|s| s.field == "Approval").unwrap()
}

fn edit_after(signed: &[u8], f: impl FnOnce(&mut Document)) -> Vec<u8> {
    let mut doc = open(signed);
    f(&mut doc);
    write_incremental(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap()
}

fn r(num: u32) -> ObjRef {
    ObjRef { num, generation: 0 }
}

/// Fill text field `num` with `value`, as a form filler does.
fn fill(num: u32, value: &str) -> impl FnOnce(&mut Document) {
    let value = value.to_owned();
    move |doc| doc.update_dict(r(num), |d| d.set(b"V".to_vec(), Object::String(PdfString::text(&value)))).unwrap()
}

/// Repaint "amount" without touching its value.
fn repaint_amount(doc: &mut Document) {
    let body = b"BT /Helv 10 Tf (999999) Tj ET".to_vec();
    let mut d = pdfcraft_cos::Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(100), Object::Int(20)]));
    d.set(b"Length".to_vec(), Object::Int(body.len() as i64));
    doc.set(r(AMOUNT_AP), Object::Stream(Stream::from_raw(d, body)));
}

fn assert_locked(s: &pdfcraft_sign::SignatureInfo, what: &str) {
    assert_eq!(s.status, Status::Invalid, "{what}: {:?}", s.details);
    assert!(matches!(&s.modification, Modification::Disallowed(k) if k.contains(&kind::LOCKED_FIELD)), "{what}: {:?}", s.modification);
}

fn assert_fill_allowed(s: &pdfcraft_sign::SignatureInfo, what: &str) {
    assert_eq!((s.status, s.modification.clone()), (Status::Valid, Modification::Allowed(vec![kind::FORM_FILL])), "{what}: {:?}", s.details);
}

const INCLUDE_AMOUNT: &str = "/Lock << /Type /SigFieldLock /Action /Include /Fields [(amount)] >>";

#[test]
fn changing_a_field_the_signature_locks_invalidates_it() {
    let (bytes, trust) = signed(INCLUDE_AMOUNT);
    assert_eq!(approval(&bytes, &trust).status, Status::Valid);
    assert_locked(&approval(&edit_after(&bytes, fill(AMOUNT, "999999")), &trust), "value and appearance");
    let both = edit_after(&bytes, |doc| {
        fill(AMOUNT, "999999")(doc);
        repaint_amount(doc);
    });
    assert_locked(&approval(&both, &trust), "value and appearance");
    assert_locked(&approval(&edit_after(&bytes, repaint_amount), &trust), "appearance only");
    // A field the lock does not name is filled as before.
    assert_fill_allowed(&approval(&edit_after(&bytes, fill(NOTE, "changed")), &trust), "unlocked note");
}

#[test]
fn without_a_lock_the_same_change_is_a_form_fill() {
    let (bytes, trust) = signed("");
    assert_fill_allowed(&approval(&edit_after(&bytes, fill(AMOUNT, "999999")), &trust), "no lock");
}

#[test]
fn exclude_locks_every_field_it_does_not_name() {
    let (bytes, trust) = signed("/Lock << /Type /SigFieldLock /Action /Exclude /Fields [(note)] >>");
    assert_locked(&approval(&edit_after(&bytes, fill(AMOUNT, "1")), &trust), "amount under Exclude [note]");
    assert_fill_allowed(&approval(&edit_after(&bytes, fill(NOTE, "changed")), &trust), "note under Exclude [note]");
}

#[test]
fn all_locks_every_field() {
    let (bytes, trust) = signed("/Lock << /Type /SigFieldLock /Action /All >>");
    assert_locked(&approval(&edit_after(&bytes, fill(NOTE, "changed")), &trust), "note under All");
    assert_locked(&approval(&edit_after(&bytes, fill(CITY, "York")), &trust), "address.city under All");
}

/// A name in `/Fields` locks the fields below it too: "address" covers "address.city".
#[test]
fn a_locked_parent_locks_its_kids() {
    let (bytes, trust) = signed("/Lock << /Type /SigFieldLock /Action /Include /Fields [(address)] >>");
    assert_locked(&approval(&edit_after(&bytes, fill(CITY, "York")), &trust), "address.city under Include [address]");
    assert_fill_allowed(&approval(&edit_after(&bytes, fill(AMOUNT, "1")), &trust), "amount under Include [address]");
}

/// Removing a locked field, or adding a second field under its name, changes it too.
#[test]
fn removing_or_shadowing_a_locked_field_changes_it() {
    let (bytes, trust) = signed(INCLUDE_AMOUNT);
    let removed = edit_after(&bytes, |doc| {
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| {
            if let Some(Object::Dict(form)) = c.get_mut(b"AcroForm") {
                form.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(r(5)), Object::Ref(r(NOTE)), Object::Ref(r(10))]));
            }
        })
        .unwrap();
    });
    assert_eq!(approval(&removed, &trust).status, Status::Invalid);
    let shadowed = edit_after(&bytes, |doc| {
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"FT".to_vec(), Object::name("Tx"));
        d.set(b"T".to_vec(), Object::String(PdfString::text("amount")));
        d.set(b"V".to_vec(), Object::String(PdfString::text("999999")));
        let twin = doc.add(Object::Dict(d));
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| {
            if let Some(Object::Dict(form)) = c.get_mut(b"AcroForm")
                && let Some(Object::Array(fields)) = form.get_mut(b"Fields")
            {
                fields.push(Object::Ref(twin));
            }
        })
        .unwrap();
    });
    assert_eq!(approval(&shadowed, &trust).status, Status::Invalid);
}

/// A locked field's value replaced by a cycle of references is compared within a bound: the
/// signature is not Valid, and validation returns (it overflowed the stack before).
#[test]
fn a_reference_cycle_in_a_locked_field_ends() {
    let (bytes, trust) = signed(INCLUDE_AMOUNT);
    let cyclic = edit_after(&bytes, |doc| {
        let (a, b) = (doc.add(Object::Null), doc.add(Object::Null));
        doc.set(a, Object::Ref(b));
        doc.set(b, Object::Ref(a));
        doc.update_dict(r(AMOUNT), |d| d.set(b"V".to_vec(), Object::Ref(a))).unwrap();
    });
    assert_ne!(approval(&cyclic, &trust).status, Status::Valid);
}

/// A lock that covers every field covers the signature's own field: repainting its widget after
/// signing is a change to a locked field.
#[test]
fn an_all_lock_covers_the_signatures_own_widget() {
    let (bytes, trust) = signed("/Lock << /Type /SigFieldLock /Action /All >>");
    let repainted = edit_after(&bytes, |doc| {
        let body = b"0 0 1 rg 0 0 130 50 re f".to_vec();
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Form"));
        d.set(b"BBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(130), Object::Int(50)]));
        d.set(b"Length".to_vec(), Object::Int(body.len() as i64));
        let ap = doc.add(Object::Stream(Stream::from_raw(d, body)));
        let mut n = pdfcraft_cos::Dict::new();
        n.set(b"N".to_vec(), Object::Ref(ap));
        doc.update_dict(r(5), |w| w.set(b"AP".to_vec(), Object::Dict(n))).unwrap();
    });
    assert_locked(&approval(&repainted, &trust), "the signature's own widget under All");
}

/// A field added below a locked field changes it: "address" gains "address.postcode".
#[test]
fn a_new_field_below_a_locked_one_changes_it() {
    let (bytes, trust) = signed("/Lock << /Type /SigFieldLock /Action /Include /Fields [(address)] >>");
    let grown = edit_after(&bytes, |doc| {
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"T".to_vec(), Object::String(PdfString::text("postcode")));
        d.set(b"Parent".to_vec(), Object::Ref(r(10)));
        d.set(b"V".to_vec(), Object::String(PdfString::text("LS1")));
        let kid = doc.add(Object::Dict(d));
        doc.update_dict(r(10), |p| {
            if let Some(Object::Array(kids)) = p.get_mut(b"Kids") {
                kids.push(Object::Ref(kid));
            }
        })
        .unwrap();
    });
    assert_locked(&approval(&grown, &trust), "a kid added under a locked field");
}

/// A locked field with no appearance of its own is drawn from the form's defaults, so changing
/// the form's /DA changes how it looks.
#[test]
fn the_form_defaults_a_locked_field_is_drawn_with_are_locked_too() {
    let (bytes, trust) = signed("/Lock << /Type /SigFieldLock /Action /Include /Fields [(note)] >>");
    let restyled = edit_after(&bytes, |doc| {
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| {
            if let Some(Object::Dict(form)) = c.get_mut(b"AcroForm") {
                form.set(b"DA".to_vec(), Object::String(PdfString::literal("/Helv 40 Tf 1 0 0 rg")));
            }
        })
        .unwrap();
    });
    assert_locked(&approval(&restyled, &trust), "the form's /DA for a field drawn from it");
}

/// An Include lock whose names can't be read locks an unknown set of fields: a later fill leaves
/// the signature Unknown, never Valid.
#[test]
fn an_unreadable_lock_leaves_a_later_fill_unknown() {
    let (bytes, trust) = signed("/Lock << /Type /SigFieldLock /Action /Include /Fields (amount) >>");
    let s = approval(&edit_after(&bytes, fill(AMOUNT, "999999")), &trust);
    assert_eq!(s.status, Status::Unknown, "{:?}", s.details);
}
