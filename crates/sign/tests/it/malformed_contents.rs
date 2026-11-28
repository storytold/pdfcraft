//! A signature whose `/Contents` is not a well-formed hexadecimal string (ISO 32000-2 §7.3.4.3,
//! §12.8.1 Table 255) is a signature that can't be checked, never an unsigned field, and bytes
//! after the signed revision that change nothing are not described as an empty list of changes.

use std::sync::Arc;

use pdfcraft_cos::Document;
use pdfcraft_sign::{Modification, SignError, SignOptions, SignatureInfo, Status, TrustStore, pkcs12, signatures};

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

/// One page whose signature widget, in /Fields and the page's /Annots, is `widget`; `more`
/// objects follow from number 6 on.
fn page_with(widget: &str, more: &[&str]) -> Vec<u8> {
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Annots [5 0 R] >>",
        "<< /Length 0 >>\nstream\n\nendstream",
        widget,
    ];
    objs.extend_from_slice(more);
    write_pdf(&objs)
}

/// One page with an unsigned signature field "Approval".
fn fixture() -> Vec<u8> {
    page_with("<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 >>", &[])
}

/// The fixture signed in its "Approval" field, and where its `/Contents` string starts (`<`) and
/// ends (one past `>`): the gap its `/ByteRange` leaves out.
fn signed() -> (Vec<u8>, usize, usize) {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let opts = SignOptions { field: Some("Approval".into()), date: "D:20261115120000Z".into(), ..SignOptions::default() };
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts).unwrap();
    let doc = open(&signed);
    let s = signatures(&doc, &signed, &TrustStore::default()).into_iter().find(|s| s.field == "Approval").unwrap();
    assert!(s.signed, "{:?}", s.details);
    let at = signed.windows(10).rposition(|w| w == b"/ByteRange").unwrap();
    let nums: Vec<usize> = std::str::from_utf8(&signed[at + 10..at + 80])
        .unwrap()
        .split(|c: char| !c.is_ascii_digit())
        .filter(|n| !n.is_empty())
        .take(3)
        .map(|n| n.parse().unwrap())
        .collect();
    let (start, end) = (nums[1], nums[2]);
    assert_eq!((signed[start], signed[end - 1]), (b'<', b'>'));
    (signed, start, end)
}

fn approval(bytes: &[u8]) -> SignatureInfo {
    signatures(&open(bytes), bytes, &TrustStore::default()).into_iter().find(|s| s.field == "Approval").unwrap()
}

/// No sentence is left with an empty pair of brackets where a list or a reason should be.
fn no_empty_brackets(s: &SignatureInfo) {
    for d in &s.details {
        assert!(!d.message.contains("()"), "{:?}", d.message);
    }
}

#[test]
fn a_signature_whose_contents_are_not_a_hex_string_is_listed_as_invalid_not_unsigned() {
    let (signed, start, end) = signed();
    // Each keeps the file's length, so every offset stays where it was.
    let cut = start + 1 + 200;
    // (what, where, the bytes written there)
    let variants: [(&str, usize, Vec<u8>); 4] = [
        ("a non-hex digit", start + 5, b"G".to_vec()),
        ("an odd number of digits", start + 5, b" ".to_vec()),
        ("no closing bracket", end - 1, b" ".to_vec()),
        ("contents cut short", cut, [b">".to_vec(), vec![b' '; end - cut - 1]].concat()),
    ];
    for (what, at, written) in variants {
        let mut bytes = signed.clone();
        bytes[at..at + written.len()].copy_from_slice(&written);
        let s = approval(&bytes);
        assert!(s.signed, "{what}: listed as unsigned: {:?}", s.details);
        assert_eq!(s.status, Status::Invalid, "{what}: {:?}", s.details);
        assert_eq!(s.summary(), "Signature is invalid", "{what}");
        assert!(!s.details.is_empty(), "{what}: no reason given");
        no_empty_brackets(&s);
    }
}

#[test]
fn a_signature_dictionary_that_cannot_be_read_says_why_and_cannot_be_signed_over() {
    let (mut bytes, start, _) = signed();
    bytes[start + 5] = b'G';
    let s = approval(&bytes);
    assert!(
        s.details.iter().any(|d| d.message.starts_with("The signature dictionary could not be read (") && d.message.contains("hex")),
        "{:?}",
        s.details
    );
    // The field still holds a signature: signing it again would overwrite it.
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let opts = SignOptions { field: Some("Approval".into()), ..SignOptions::default() };
    match pdfcraft_sign::sign(&open(&bytes), &id, &opts) {
        Err(SignError::Pdf(e)) => assert_eq!(e, "the field \"Approval\" is already signed"),
        other => panic!("{:?}", other.map(|b| b.len())),
    }
}

#[test]
fn bytes_after_the_signed_revision_that_change_nothing_name_no_empty_list() {
    let (mut bytes, ..) = signed();
    bytes.extend_from_slice(b"\n%a comment after the end of the file\n");
    let s = approval(&bytes);
    assert_eq!(s.modification, Modification::Allowed(Vec::new()));
    assert!(
        s.details
            .iter()
            .any(|d| d.message == "Data was added to the file since this signature was applied, but no change to the document was found."),
        "{:?}",
        s.details
    );
    no_empty_brackets(&s);
}

/// An update that adds only an object no signed object held can still change what a signed page
/// shows: a reference to an object that doesn't exist reads as null (ISO 32000-2 §7.3.10), so
/// adding the object a signed page's `/CropBox` already names crops the page while no signed
/// object changes. The detail says the check found no change, never that the data changes nothing.
#[test]
fn an_added_object_a_signed_page_already_names_is_not_said_to_change_nothing() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let opts = SignOptions { field: Some("Approval".into()), date: "D:20261115120000Z".into(), ..SignOptions::default() };
    let unsigned = write_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /CropBox 99 0 R /Contents 4 0 R /Annots [5 0 R] >>",
        "<< /Length 0 >>
stream

endstream",
        "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 >>",
    ]);
    let mut bytes = pdfcraft_sign::sign(&open(&unsigned), &id, &opts).unwrap();
    let prev = {
        let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
        std::str::from_utf8(&bytes[at + 9..]).unwrap().split_whitespace().next().unwrap().parse::<usize>().unwrap()
    };
    let object = bytes.len() + 1;
    bytes.extend_from_slice(
        b"
99 0 obj
[0 0 100 100]
endobj
",
    );
    let xref = bytes.len();
    bytes.extend_from_slice(
        format!(
            "xref
99 1
{object:010} 00000 n 
trailer
<< /Size 100 /Root 1 0 R /Prev {prev} >>
startxref
{xref}
%%EOF
"
        )
        .as_bytes(),
    );
    let s = approval(&bytes);
    assert!(s.signed, "{:?}", s.details);
    assert!(!s.details.iter().any(|d| d.message.contains("changes nothing")), "{:?} {:?}", s.modification, s.details);
    // The update is seen. With no signed object changed the list of changes is empty, and the
    // detail says what the check found; a check that counts the added object as a change lists it.
    match &s.modification {
        Modification::None => panic!("the update after the signature was not seen: {:?}", s.details),
        Modification::Allowed(changes) if changes.is_empty() => assert!(
            s.details
                .iter()
                .any(|d| d.message == "Data was added to the file since this signature was applied, but no change to the document was found."),
            "{:?}",
            s.details
        ),
        Modification::Allowed(_) | Modification::Disallowed(_) => {}
    }
    no_empty_brackets(&s);
}

/// Without a signer's own bytes around it: a value that names a signature dictionary that can't
/// be parsed is a signature, and one under a field with no name to list it by still makes the
/// document count as signed. A value that names an object that does not exist is null and stays
/// unsigned.
#[test]
fn an_unreadable_value_counts_as_a_signature_even_without_a_name() {
    let broken = "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 10] /Contents <30G1> >>";
    let named = page_with("<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /V 6 0 R /Rect [150 20 280 70] /P 3 0 R /F 4 >>", &[broken]);
    let s = approval(&named);
    assert!(s.signed && s.status == Status::Invalid, "{:?}", s.details);
    let unnamed = page_with("<< /Type /Annot /Subtype /Widget /FT /Sig /V 6 0 R /Rect [150 20 280 70] /P 3 0 R /F 4 >>", &[broken]);
    let listed = signatures(&open(&unnamed), &unnamed, &TrustStore::default());
    assert!(listed.iter().any(|s| s.field == "UnreadFields" && s.signed), "{listed:?}");
    let dangling = page_with("<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /V 9 0 R /Rect [150 20 280 70] /P 3 0 R /F 4 >>", &[]);
    let s = approval(&dangling);
    assert!(!s.signed, "{:?}", s.details);
}
