//! A document holds at most one certification, and it is its first signature: approval
//! signatures follow it (ISO 32000-2 §12.8.1, §12.8.2.2.1). Certifying a document that already
//! holds a signature is refused before anything is written; an unsigned one is still certified.

use std::sync::Arc;

use pdfcraft_cos::Document;
use pdfcraft_sign::{SignError, SignOptions, TrustStore, pkcs12};

fn id() -> pdfcraft_sign::DigitalId {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/ec-p256.p12")).unwrap();
    pkcs12::open(&p12, "test").unwrap()
}

/// One page with an unsigned signature field `Approval`, and `more` objects from number 5 on.
fn fixture(more: &[&str]) -> Vec<u8> {
    let fields = if more.is_empty() { "[4 0 R]" } else { "[4 0 R 5 0 R]" };
    let catalog = format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields {fields} /SigFlags 3 >> >>");
    let mut objs: Vec<String> = vec![
        catalog,
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [4 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 >>".into(),
    ];
    objs.extend(more.iter().map(|o| o.to_string()));
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

fn open(b: &[u8]) -> Document {
    Document::open(Arc::new(b.to_vec())).unwrap()
}

fn opts(certify: Option<u8>) -> SignOptions {
    SignOptions { page: 0, rect: Some([20.0, 100.0, 220.0, 150.0]), date: "D:20261002120000+01'00'".into(), certify, ..SignOptions::default() }
}

fn refused(r: Result<Vec<u8>, SignError>) {
    match r {
        Err(SignError::Pdf(m)) => assert!(m.contains("already signed, so it can't be certified"), "{m}"),
        other => panic!("expected the certification to be refused, got {:?}", other.map(|b| b.len())),
    }
}

#[test]
fn a_document_with_an_approval_signature_cannot_be_certified() {
    let signed = pdfcraft_sign::sign(&open(&fixture(&[])), &id(), &opts(None)).unwrap();
    for level in 1..=3 {
        refused(pdfcraft_sign::sign(&open(&signed), &id(), &opts(Some(level))));
        // Signing an existing empty field as a certification is refused the same way.
        refused(pdfcraft_sign::sign(&open(&signed), &id(), &SignOptions { field: Some("Approval".into()), ..opts(Some(level)) }));
    }
    // An approval signature after it is still made.
    let again = pdfcraft_sign::sign(&open(&signed), &id(), &SignOptions { field: Some("Approval".into()), ..opts(None) }).unwrap();
    let doc = open(&again);
    assert_eq!(pdfcraft_sign::signatures(&doc, &again, &TrustStore::default()).iter().filter(|s| s.signed).count(), 2);
}

#[test]
fn a_certified_document_cannot_be_certified_again() {
    let certified = pdfcraft_sign::sign(&open(&fixture(&[])), &id(), &opts(Some(3))).unwrap();
    refused(pdfcraft_sign::sign(&open(&certified), &id(), &opts(Some(2))));
    let doc = open(&certified);
    let listed = pdfcraft_sign::signatures(&doc, &certified, &TrustStore::default());
    assert_eq!(listed.iter().filter(|s| s.certify.is_some()).count(), 1, "still one certification");
    // Signing after the certification is an approval signature, which it permits.
    let approved = pdfcraft_sign::sign(&doc, &id(), &SignOptions { field: Some("Approval".into()), ..opts(None) }).unwrap();
    let listed = pdfcraft_sign::signatures(&open(&approved), &approved, &TrustStore::default());
    assert_eq!(listed.iter().filter(|s| s.signed).count(), 2);
    assert_eq!(listed.iter().filter(|s| s.certify.is_some()).count(), 1);
}

#[test]
fn a_document_timestamp_counts_as_a_signature() {
    // A document timestamp in a field: any signature already present rules out certifying.
    let stamp = "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Timestamp) /Rect [0 0 0 0] /P 3 0 R \
                 /V << /Type /DocTimeStamp /Filter /Adobe.PPKLite /SubFilter /ETSI.RFC3161 /ByteRange [0 10 20 10] /Contents <3082> >> >>";
    refused(pdfcraft_sign::sign(&open(&fixture(&[stamp])), &id(), &opts(Some(2))));
}

#[test]
fn an_unsigned_document_is_still_certified() {
    for level in 1..=3 {
        let certified = pdfcraft_sign::sign(&open(&fixture(&[])), &id(), &opts(Some(level))).unwrap();
        let listed = pdfcraft_sign::signatures(&open(&certified), &certified, &TrustStore::default());
        assert_eq!(listed.iter().find(|s| s.signed).and_then(|s| s.certify), Some(level));
    }
    // An unsigned signature field does not count as a signature.
    let certified = pdfcraft_sign::sign(&open(&fixture(&[])), &id(), &SignOptions { field: Some("Approval".into()), ..opts(Some(2)) }).unwrap();
    let listed = pdfcraft_sign::signatures(&open(&certified), &certified, &TrustStore::default());
    assert_eq!(listed.iter().find(|s| s.field == "Approval").and_then(|s| s.certify), Some(2));
}
