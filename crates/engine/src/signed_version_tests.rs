//! Compare signed version: the file a signature's `/ByteRange` covers, compared with the
//! document as it is now, and the refusals when that range can't be cut out of the file.

use std::sync::Arc;

use crate::compare::Kind;
use crate::tests::fixture;
use crate::{AddedText, DocId, Edit, EditError, Session, SignOptions};

fn digital_id() -> pdfcraft_sign::DigitalId {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    pdfcraft_sign::pkcs12::open(&p12, "test").unwrap()
}

/// A session (clock after the test certificate's start date) holding `fixture(2)` signed once,
/// invisibly. Returns the session, the document and the signed file.
fn signed_session() -> (Session, DocId, Arc<Vec<u8>>) {
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("contract.pdf", None, Arc::new(fixture(2)), None).unwrap();
    let signed = s.sign(id, &digital_id(), SignOptions { page: 0, reason: Some("Approved".into()), ..SignOptions::default() }).unwrap();
    s.mark_signed(id, signed.clone(), None).unwrap();
    (s, id, signed)
}

/// The signed file with its `/ByteRange` numbers replaced by `numbers`, padded to the same width
/// so nothing else in the file moves.
fn with_byte_range(signed: &[u8], numbers: &str) -> Vec<u8> {
    let key = b"/ByteRange [";
    let at = signed.windows(key.len()).position(|w| w == key).unwrap() + key.len();
    let width = signed[at..].iter().position(|b| *b == b']').unwrap();
    assert!(numbers.len() <= width, "{numbers:?} is wider than the range it replaces");
    let mut out = signed.to_vec();
    let mut patch = numbers.as_bytes().to_vec();
    patch.resize(width, b' ');
    out[at..at + width].copy_from_slice(&patch);
    out
}

/// The signature's own byte range, as written: [0, a, b, c].
fn byte_range(signed: &[u8]) -> [u64; 4] {
    let key = b"/ByteRange [";
    let at = signed.windows(key.len()).position(|w| w == key).unwrap() + key.len();
    let end = at + signed[at..].iter().position(|b| *b == b']').unwrap();
    let nums: Vec<u64> = std::str::from_utf8(&signed[at..end]).unwrap().split_whitespace().map(|n| n.parse().unwrap()).collect();
    nums.try_into().unwrap()
}

#[test]
fn the_signed_version_is_compared_with_the_text_added_after_signing() {
    let (mut s, id, signed) = signed_session();
    // Nothing added yet: the signature covers the whole file and the text is the same.
    let (version, c) = s.compare_signed_version(id, 0).unwrap();
    assert!(version.is_current() && version.len == signed.len() && version.revision == 2, "{version:?}");
    assert!(c.identical(), "{:?}", c.changes);
    // Text added to page 1 after signing is an insertion on the current document's page 1.
    let note = AddedText { rect: [20.0, 250.0, 180.0, 280.0], text: "Added note".into(), ..Default::default() };
    s.apply(id, Edit::AddText { page: 0, text: note }).unwrap();
    let before = s.docs.len();
    let (version, c) = s.compare_signed_version(id, 0).unwrap();
    assert_eq!(s.docs.len(), before, "the signed version is closed after comparing");
    assert_eq!((version.field.as_str(), version.revision, version.len), (s.get(id).unwrap().signatures[0].field.as_str(), 2, signed.len()));
    assert!(!version.is_current() && version.current_len > version.len, "{version:?}");
    assert_eq!((c.count(Kind::Inserted), c.count(Kind::Replaced), c.count(Kind::Deleted)), (1, 0, 0), "{:?}", c.changes);
    let ch = &c.changes[0];
    assert_eq!((ch.new.text.as_str(), ch.new.page), ("Added note", 0));
    assert_eq!((c.old_words, c.new_words), (4, 6));
    // Opened on its own, the signed version is the file as signed, under its own name.
    let old = s.open_signed_version(id, 0).unwrap();
    let doc = s.get(old).unwrap();
    assert_eq!((doc.name.as_str(), doc.bytes.as_slice()), ("contract (signed version).pdf", signed.as_slice()));
    assert!(doc.dirty || doc.path.is_none(), "a new, unsaved document");
}

#[test]
fn signatures_that_cannot_give_a_signed_version_are_refused_by_name() {
    let (mut s, id, signed) = signed_session();
    // A signature that doesn't exist.
    let e = s.compare_signed_version(id, 3).unwrap_err();
    assert!(matches!(&e, EditError::Invalid(m) if m.contains("signature 4") && m.contains("1 signature")), "{e}");
    let plain = s.open("plain.pdf", None, Arc::new(fixture(1)), None).unwrap();
    assert!(matches!(s.compare_signed_version(plain, 0), Err(EditError::Invalid(m)) if m.contains("no signatures")));
    // Byte ranges that don't fit the file: past its end, a number past any length, too few
    // numbers, a negative start, not starting at 0, and a second range before the first ends.
    let [_, a, b, c] = byte_range(&signed);
    let field = s.get(id).unwrap().signatures[0].field.clone();
    for numbers in [
        format!("0 {a} {b} {}", c + 1),
        format!("0 {a} {b} 99999999999999999999"),
        format!("0 {a} 18446744073709551615 {c}"),
        format!("0 {a} {b}"),
        format!("-1 {a} {b} {c}"),
        format!("1 {a} {b} {c}"),
        format!("0 {a} {} {c}", a - 1),
    ] {
        let bytes = Arc::new(with_byte_range(&signed, &numbers));
        let doc = s.open("broken.pdf", None, bytes, None).unwrap();
        let sig = &s.get(doc).unwrap().signatures[0];
        assert!(sig.signed && sig.signed_len == 0, "{numbers}: {sig:?}");
        let before = s.docs.len();
        let e = s.compare_signed_version(doc, 0).unwrap_err();
        assert!(matches!(&e, EditError::Sign(m) if m.contains("byte range") && m.contains(&format!("{field:?}"))), "{numbers}: {e}");
        assert!(matches!(s.open_signed_version(doc, 0), Err(EditError::Sign(_))), "{numbers}");
        assert_eq!(s.docs.len(), before, "{numbers}: nothing is left open");
    }
}

#[test]
fn an_unsigned_signature_field_has_no_signed_version() {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [5 0 R] >>",
        "<< /Length 0 >>\nstream\n\nendstream",
        "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [20 20 180 60] /P 3 0 R >>",
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut s = Session::new();
    let id = s.open("form.pdf", None, Arc::new(pdf), None).unwrap();
    assert!(s.get(id).unwrap().signatures.iter().any(|x| x.field == "Approval" && !x.signed));
    let e = s.compare_signed_version(id, 0).unwrap_err();
    assert!(matches!(&e, EditError::Sign(m) if m.contains("\"Approval\" is not signed")), "{e}");
}
