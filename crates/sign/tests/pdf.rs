//! Signing and validating whole documents.

use std::sync::Arc;

use pdfcraft_cos::{Document, Object, PdfString, SaveOptions, write_incremental};
use pdfcraft_sign::{Modification, SignError, SignOptions, Status, Time, TimestampAuthority, TrustStore, der, pkcs12, signatures};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// One page with text and an unsigned signature field "Approval".
fn fixture() -> Vec<u8> {
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>",
        b"<< /Length 44 >>\nstream\nBT /F1 14 Tf 20 250 Td (Contract text) Tj ET\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
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

#[test]
fn standalone_timestamp_scan_survives_cache_batches_and_object_streams() {
    let mut doc = open(&fixture());
    let filler: Vec<_> = (0..300).map(|i| Object::Ref(doc.add(Object::Int(i)))).collect();
    let mut stamp = pdfcraft_cos::Dict::new();
    stamp.set(b"Type".to_vec(), Object::name("Sig"));
    stamp.set(b"SubFilter".to_vec(), Object::name("ETSI.RFC3161"));
    stamp.set(b"ByteRange".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(1), Object::Int(2), Object::Int(1)]));
    stamp.set(b"Contents".to_vec(), Object::String(PdfString::literal("invalid-token")));
    let stamp = doc.add(Object::Dict(stamp));
    doc.update_dict(doc.root().unwrap(), |catalog| {
        catalog.set(b"UninspectedObjects".to_vec(), Object::Array(filler));
        catalog.set(b"UninspectedTimestamp".to_vec(), Object::Ref(stamp));
    })
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
    let doc = open(&bytes);
    assert!(doc.object_numbers().len() > 300);
    let listed = signatures(&doc, &bytes, &TrustStore::default());
    assert!(listed.iter().any(|s| s.doc_timestamp && s.status != Status::Valid));
    assert!(listed.iter().any(|s| s.field == "Approval" && !s.signed));
}

fn opts() -> SignOptions {
    SignOptions {
        page: 0,
        rect: Some([20.0, 100.0, 220.0, 150.0]),
        reason: Some("I approve this document".into()),
        location: Some("London".into()),
        date: "D:20261002120000+01'00'".into(),
        ..SignOptions::default()
    }
}

#[test]
fn signing_then_validating_with_and_without_trust() {
    for file in ["rsa-aes.p12", "ec-p256.p12", "ec-p384.p12", "chain.p12"] {
        let id = pkcs12::open(&data(file), "test").unwrap();
        let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
        assert!(signed.starts_with(&fixture()), "{file}: an incremental update");
        let doc = open(&signed);
        let sigs = signatures(&doc, &signed, &TrustStore::default());
        let s = sigs.iter().find(|s| s.signed).expect("a signed field");
        assert_eq!(s.field, "Signature1");
        assert_eq!(s.status, Status::Unknown, "{file}: {:?}", s.details);
        assert_eq!(s.modification, Modification::None);
        assert_eq!(s.signer.as_deref(), id.certificate.subject.common_name());
        assert_eq!((s.reason.as_deref(), s.location.as_deref(), s.page), (Some("I approve this document"), Some("London"), Some(0)));
        assert!(s.visible && s.signed_len == signed.len() && s.revision == 2);
        assert!(s.details.iter().any(|d| d.contains("identity is unknown")));
        // The unsigned field is listed too.
        assert!(sigs.iter().any(|s| s.field == "Approval" && !s.signed));
        // Trusting the signer (or its root) makes it valid.
        let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
        let trusted = signatures(&doc, &signed, &TrustStore { certs: vec![anchor] });
        let s = trusted.iter().find(|s| s.signed).unwrap();
        assert_eq!(s.status, Status::Valid, "{file}: {:?}", s.details);
        assert_eq!(s.chain.len(), if file == "chain.p12" { 2 } else { 1 });
        // Changing one signed byte breaks it.
        let mut tampered = signed.clone();
        let i = tampered.windows(13).position(|w| w == b"Contract text").unwrap();
        tampered[i] = b'K';
        let s = signatures(&open(&tampered), &tampered, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
        assert_eq!(s.status, Status::Invalid);
        assert!(s.details[0].contains("altered or corrupted"), "{:?}", s.details);
    }
}

#[test]
fn signing_an_existing_field_and_counter_signing() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let first = pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { field: Some("Approval".into()), ..opts() }).unwrap();
    let id2 = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let second = pdfcraft_sign::sign(&open(&first), &id2, &opts()).unwrap();
    let doc = open(&second);
    let sigs = signatures(&doc, &second, &TrustStore::default());
    let a = sigs.iter().find(|s| s.field == "Approval").unwrap();
    assert!(a.signed);
    assert_eq!(a.rect, Some([150.0, 20.0, 280.0, 70.0]));
    assert_eq!(a.revision, 2);
    assert_eq!(a.modification, Modification::Allowed(vec!["signature".into()]), "{:?}", a.details);
    assert_eq!(a.status, Status::Unknown);
    let b = sigs.iter().find(|s| s.field == "Signature1").unwrap();
    assert_eq!((b.revision, b.modification.clone()), (3, Modification::None));
    assert!(matches!(
        pdfcraft_sign::sign(&doc, &id, &SignOptions { field: Some("Approval".into()), ..opts() }),
        Err(pdfcraft_sign::SignError::Pdf(_))
    ));
}

/// Append a revision that changes object `num` with `f`.
fn edit_after(signed: &[u8], f: impl FnOnce(&mut Document)) -> Vec<u8> {
    let mut doc = open(signed);
    f(&mut doc);
    write_incremental(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap()
}

fn add_comment(doc: &mut Document) {
    let mut d = pdfcraft_cos::Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name("Text"));
    d.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(10), Object::Int(10), Object::Int(30), Object::Int(30)]));
    d.set(b"Contents".to_vec(), PdfString::text("A note"));
    let r = doc.add(Object::Dict(d));
    let page = pdfcraft_cos::ObjRef { num: 3, generation: 0 };
    doc.update_dict(page, |p| {
        if let Some(Object::Array(a)) = p.get_mut(b"Annots") {
            a.push(Object::Ref(r));
        }
    })
    .unwrap();
}

fn change_text(doc: &mut Document) {
    let mut d = pdfcraft_cos::Dict::new();
    d.set(b"Length".to_vec(), Object::Int(40));
    let s = pdfcraft_cos::Stream::from_raw(d, b"BT /F1 14 Tf 20 250 Td (Other text) Tj ET".to_vec());
    doc.set(pdfcraft_cos::ObjRef { num: 4, generation: 0 }, Object::Stream(s));
}

#[test]
fn later_changes_are_classified_under_the_signature_permissions() {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let check = |bytes: &[u8]| signatures(&open(bytes), bytes, &trust).into_iter().find(|s| s.signed).unwrap();
    // Approval signature: comments are permitted, rewriting page content is not.
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let s = check(&edit_after(&signed, add_comment));
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert_eq!(s.modification, Modification::Allowed(vec!["comments".into()]));
    let s = check(&edit_after(&signed, change_text));
    assert_eq!(s.status, Status::Invalid);
    assert_eq!(s.modification, Modification::Disallowed(vec!["page content".into()]));
    // Certified with "no changes allowed": even a comment invalidates it.
    let certified = pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { certify: Some(1), ..opts() }).unwrap();
    let s = check(&certified);
    assert_eq!((s.certify, s.status), (Some(1), Status::Valid));
    assert_eq!(check(&edit_after(&certified, add_comment)).status, Status::Invalid);
    // Certified allowing comments: fine.
    let certified3 = pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { certify: Some(3), ..opts() }).unwrap();
    assert_eq!(check(&edit_after(&certified3, add_comment)).status, Status::Valid);
}

#[test]
fn invisible_signatures_and_refusals() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { rect: None, ..opts() }).unwrap();
    let s = signatures(&open(&signed), &signed, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
    assert!(!s.visible);
    assert!(pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { page: 5, ..opts() }).is_err());
    assert_eq!(pdfcraft_sign::pdf::display_date("D:20261002120000+01'00'"), "2026.10.02 12:00:00 +01'00'");
}

/// A deterministic TSA: signs an RFC 3161 response locally with a test digital ID at a fixed
/// time. No sockets; the whole stamping path runs in-process.
struct TestTsa {
    id: pkcs12::DigitalId,
    time: Time,
}

impl TimestampAuthority for TestTsa {
    fn timestamp(&self, request: &[u8]) -> Result<Vec<u8>, SignError> {
        let q = pdfcraft_sign::timestamp::parse_request(request)?;
        pdfcraft_sign::timestamp::respond(
            &self.id.key,
            &self.id.certificate,
            &self.id.chain,
            pdfcraft_sign::DigestAlg::Sha256,
            &q,
            "1.2.3.4",
            self.time,
            7,
        )
    }
}

#[test]
fn signing_with_a_timestamp_embeds_a_verified_rfc3161_token() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let signed = pdfcraft_sign::sign_with_timestamp(&open(&fixture()), &id, &opts(), &tsa).unwrap();
    assert!(signed.starts_with(&fixture()), "still an incremental update");
    let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
    let trust = TrustStore { certs: vec![anchor.clone(), tsa.id.certificate.clone()] };
    let s = signatures(&open(&signed), &signed, &trust).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert!(s.timestamp, "{:?}", s.details);
    assert_eq!(s.timestamp_time, Some(tsa.time), "the token's generation time, verified over the signature value");
    assert!(s.details.iter().any(|d| d.contains("trusted time")), "{:?}", s.details);
    // A trusted signer with an untrusted TSA: still valid, but the token's time is unverified.
    let s = signatures(&open(&signed), &signed, &TrustStore { certs: vec![anchor] }).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert!(s.timestamp);
    assert_eq!(s.timestamp_time, None);
    assert!(s.details.iter().any(|d| d.contains("unverified timestamp")), "{:?}", s.details);
    // Untrusted, the signature stays intact-but-unknown; the timestamp is still noted.
    let s = signatures(&open(&signed), &signed, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Unknown);
    assert!(s.timestamp && s.timestamp_time.is_none());
}

#[test]
fn a_malformed_timestamp_response_fails_signing_without_a_file() {
    struct Bad;
    impl TimestampAuthority for Bad {
        fn timestamp(&self, _request: &[u8]) -> Result<Vec<u8>, SignError> {
            Ok(b"not der".to_vec())
        }
    }
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    assert!(pdfcraft_sign::sign_with_timestamp(&open(&fixture()), &id, &opts(), &Bad).is_err());
}

#[test]
fn a_document_timestamp_covers_the_file_and_validates() {
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let stamped = pdfcraft_sign::timestamp_document(&open(&fixture()), &tsa, "D:20261006120000Z").unwrap();
    assert!(stamped.starts_with(&fixture()), "an incremental update");
    let trust_tsa = TrustStore { certs: vec![tsa.id.certificate.clone()] };
    let s = signatures(&open(&stamped), &stamped, &trust_tsa).into_iter().find(|s| s.doc_timestamp).unwrap();
    assert!(s.doc_timestamp && s.timestamp);
    assert_eq!(s.sub_filter.as_deref(), Some("ETSI.RFC3161"));
    assert_eq!(s.timestamp_time, Some(tsa.time));
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert_eq!(s.modification, Modification::None);
    assert_eq!(s.signed_len, stamped.len());
    // A later byte change breaks the timestamp's imprint.
    let mut tampered = stamped.clone();
    let i = tampered.windows(13).position(|w| w == b"Contract text").unwrap();
    tampered[i] = b'K';
    let s = signatures(&open(&tampered), &tampered, &TrustStore::default()).into_iter().find(|s| s.doc_timestamp).unwrap();
    assert_eq!(s.status, Status::Invalid);
    // Combined with a field signature: both are listed, the stamp covers both revisions.
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let both = pdfcraft_sign::timestamp_document(&open(&signed), &tsa, "D:20261006130000Z").unwrap();
    let all = signatures(&open(&both), &both, &TrustStore { certs: vec![tsa.id.certificate.clone()] });
    let field = all.iter().find(|s| s.signed && !s.doc_timestamp).unwrap();
    let allowed = match &field.modification {
        Modification::Allowed(k) => k,
        other => panic!("{other:?}"),
    };
    assert!(allowed.contains(&"signature".to_string()), "{allowed:?}");
    let stamp = all.iter().find(|s| s.doc_timestamp).unwrap();
    assert_eq!(stamp.status, Status::Valid, "{:?}", stamp.details);
}

#[test]
fn cached_discovery_still_validates_current_timestamp_bytes_ranges_and_trust() {
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let mut source = open(&fixture());
    source
        .update_dict(source.root().unwrap(), |d| {
            d.remove(b"AcroForm");
        })
        .unwrap();
    let bytes = pdfcraft_sign::timestamp_document(&source, &tsa, "D:20261006120000Z").unwrap();
    let doc = open(&bytes);
    let cache = pdfcraft_sign::DigestCache::default();
    let trusted = TrustStore { certs: vec![tsa.id.certificate.clone()] };
    let check = |doc: &Document, bytes: &[u8], trust: &TrustStore| {
        let listed = pdfcraft_sign::pdf::list_cached(doc, bytes, trust, &cache);
        assert_eq!(listed.len(), 1);
        assert!(listed[0].doc_timestamp);
        listed.into_iter().next().unwrap()
    };
    assert_eq!(check(&doc, &bytes, &TrustStore::default()).status, Status::Unknown);
    assert_eq!(check(&doc, &bytes, &trusted).status, Status::Valid);
    assert_eq!(check(&doc, &bytes, &TrustStore::default()).status, Status::Unknown);
    let mut tampered = bytes.clone();
    let at = tampered.windows(13).position(|w| w == b"Contract text").unwrap();
    tampered[at] = b'K';
    assert_eq!(check(&doc, &tampered, &trusted).status, Status::Invalid);
    assert_eq!(check(&doc, &[], &trusted).status, Status::Invalid);
    let stamp = doc.scan_objects().find(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"SubFilter") == Some(b"ETSI.RFC3161"))).unwrap().0;
    let mut edited = doc.clone();
    edited.update_dict(stamp, |d| d.set(b"ByteRange".to_vec(), Object::Array(vec![Object::Int(-1)]))).unwrap();
    assert_eq!(check(&edited, &bytes, &trusted).status, Status::Invalid);
    assert_eq!(check(&doc, &bytes, &trusted).status, Status::Valid);
    // Even a malformed token in the same byte ranges must be read again after discovery.
    let br = doc.get(stamp).as_dict().unwrap().get(b"ByteRange").unwrap().as_array().unwrap().clone();
    let gap_start = br[1].as_int().unwrap() as usize;
    tampered = bytes.clone();
    tampered[gap_start + 1] = b'F';
    tampered[gap_start + 2] = b'F';
    assert_eq!(check(&doc, &tampered, &trusted).status, Status::Invalid);
}

#[test]
fn embedding_ltv_evidence_adds_a_dss_and_keeps_signatures_valid() {
    use pdfcraft_sign::dss::{self, Evidence};
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let evidence =
        Evidence { certs: vec![id.certificate.raw.clone()], ocsps: vec![b"synthetic ocsp".to_vec()], crls: vec![b"synthetic crl".to_vec()] };
    let ltv = dss::embed(&open(&signed), &evidence).unwrap();
    assert!(ltv.starts_with(&signed), "incremental");
    let doc = open(&ltv);
    // The store is in the catalog with one certificate and the /VRI entry for the signature.
    let root = doc.root().unwrap();
    let dss_dict: pdfcraft_cos::Dict =
        doc.get(root).as_dict().unwrap().get(b"DSS").map(|d| doc.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap();
    assert_eq!(dss_dict.name(b"Type"), Some(b"DSS".as_slice()));
    assert!(dss_dict.contains(b"Certs") && dss_dict.contains(b"OCSPs") && dss_dict.contains(b"CRLs") && dss_dict.contains(b"VRI"));
    // The earlier signature still validates; the DSS counts as a permitted change.
    let s = signatures(&doc, &ltv, &TrustStore::default()).into_iter().find(|s| s.signed && !s.doc_timestamp).unwrap();
    assert_eq!(s.status, Status::Unknown);
    let allowed = match &s.modification {
        Modification::Allowed(k) => k,
        other => panic!("{other:?}"),
    };
    assert!(allowed.contains(&"document security store".to_string()), "{allowed:?}");
    // Embedding twice keeps one certificate (byte-identical dedup) and stays valid.
    let twice = dss::embed(&doc, &evidence).unwrap();
    let doc2 = open(&twice);
    let dss2: pdfcraft_cos::Dict =
        doc2.get(doc2.root().unwrap()).as_dict().unwrap().get(b"DSS").map(|d| doc2.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap();
    let certs = dss2.get(b"Certs").map(|c| doc2.resolve(c)).and_then(|c| c.as_array().cloned()).unwrap();
    assert_eq!(certs.len(), 1, "byte-identical evidence is not duplicated");
}

#[test]
fn sign_then_ltv_then_timestamp_makes_a_b_lta_file() {
    use pdfcraft_sign::dss::{self, Evidence};
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let ltv = dss::embed(&open(&signed), &Evidence { certs: vec![id.certificate.raw.clone()], ocsps: Vec::new(), crls: Vec::new() }).unwrap();
    let lta = pdfcraft_sign::timestamp_document(&open(&ltv), &tsa, "D:20261006120000Z").unwrap();
    let all = signatures(&open(&lta), &lta, &TrustStore { certs: vec![tsa.id.certificate.clone()] });
    assert_eq!(all.iter().filter(|s| s.signed).count(), 2);
    let field = all.iter().find(|s| s.signed && !s.doc_timestamp).unwrap();
    assert_eq!(field.status, Status::Unknown);
    let allowed = match &field.modification {
        Modification::Allowed(k) => k,
        other => panic!("{other:?}"),
    };
    assert!(allowed.contains(&"document security store".to_string()) && allowed.contains(&"signature".to_string()), "{allowed:?}");
    assert!(!allowed.contains(&"page content".to_string()), "{allowed:?}");
    let stamp = all.iter().find(|s| s.doc_timestamp).unwrap();
    assert_eq!(stamp.status, Status::Valid, "{:?}", stamp.details);
}

#[test]
fn an_embedded_verified_revocation_invalidates_the_signature() {
    use pdfcraft_sign::dss::{self, Evidence};
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    // The signer's own CRL (self-signed test identity) revokes its certificate, in a window
    // that covers the signing date (October 2026).
    let (this, next) =
        (Time { year: 2026, month: 1, day: 1, hour: 0, minute: 0, second: 0 }, Time { year: 2027, month: 1, day: 1, hour: 0, minute: 0, second: 0 });
    let alg = id.key.signature_algorithm(pdfcraft_sign::DigestAlg::Sha256);
    let tbs = der::seq(&[
        &der::int(1),
        &alg,
        &id.certificate.subject.raw,
        &this.encode(),
        &next.encode(),
        &der::seq(&[&der::seq(&[
            &der::uint(&id.certificate.serial),
            &Time { year: 2026, month: 6, day: 1, hour: 8, minute: 0, second: 0 }.encode(),
        ])]),
    ]);
    let sig = id.key.sign(pdfcraft_sign::DigestAlg::Sha256, &tbs).unwrap();
    let crl = der::seq(&[&tbs, &alg, &der::bit_string(&sig)]);
    let ltv = dss::embed(&open(&signed), &Evidence { certs: Vec::new(), ocsps: Vec::new(), crls: vec![crl] }).unwrap();
    let s = signatures(&open(&ltv), &ltv, &TrustStore::default()).into_iter().find(|s| s.signed && !s.doc_timestamp).unwrap();
    assert_eq!(s.status, Status::Invalid, "{:?}", s.details);
    assert!(s.details.iter().any(|d| d.contains("revoked") && d.contains("CRL")), "{:?}", s.details);
}

#[test]
fn validates_a_signature_made_by_openssl() {
    let bytes = data("openssl-signed.pdf");
    let doc = open(&bytes);
    let s = signatures(&doc, &bytes, &TrustStore::default()).into_iter().next().unwrap();
    assert_eq!((s.field.as_str(), s.sub_filter.as_deref()), ("OpenSSL", Some("adbe.pkcs7.detached")));
    assert_eq!(s.status, Status::Unknown, "{:?}", s.details);
    assert_eq!(s.signer.as_deref(), Some("Test Signer RSA"));
    assert!(s.signing_time.is_some_and(|t| t.year == 2026), "from the CMS signing-time attribute");
    assert!(!s.visible);
    let rsa = pdfcraft_sign::Certificate::parse(&pkcs12::open(&data("rsa-aes.p12"), "test").unwrap().certificate.raw).unwrap();
    let s = signatures(&doc, &bytes, &TrustStore { certs: vec![rsa] }).into_iter().next().unwrap();
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    // A later comment is allowed for this approval signature.
    let edited = edit_after(&bytes, add_comment);
    let s = signatures(&open(&edited), &edited, &TrustStore::default()).into_iter().next().unwrap();
    assert_eq!(s.modification, Modification::Allowed(vec!["comments".into()]));
}

#[test]
fn files_without_a_cross_reference_table_are_signed_with_a_full_write() {
    // No xref: the document is reconstructed, so saving rewrites (and renumbers) everything.
    let bytes = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF"
        .to_vec();
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    for certify in [None, Some(2)] {
        let signed = pdfcraft_sign::sign(&open(&bytes), &id, &SignOptions { certify, rect: None, ..opts() }).unwrap();
        let s = signatures(&open(&signed), &signed, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
        assert_eq!((s.status, s.certify, s.modification.clone()), (Status::Unknown, certify, Modification::None), "{:?}", s.details);
    }
}

/// Signing with Keychain identities, in a throwaway keychain file. Ignored by default: creating
/// a keychain touches the user's keychain search list (restored afterwards).
#[cfg(target_os = "macos")]
#[test]
#[ignore = "creates a temporary macOS keychain; run with --ignored"]
fn signing_with_keychain_identities() {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("pdfcraft-keychain-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let kc = dir.join("test.keychain-db");
    let sec = |args: &[&str]| Command::new("security").args(args).output().unwrap();
    let list = String::from_utf8(sec(&["list-keychains", "-d", "user"]).stdout).unwrap();
    let saved: Vec<String> = list.lines().map(|l| l.trim().trim_matches('"').to_string()).filter(|l| !l.is_empty()).collect();
    let k = kc.to_str().unwrap();
    assert!(sec(&["create-keychain", "-p", "pc-test", k]).status.success());
    let restore = || {
        let mut args = vec!["list-keychains", "-d", "user", "-s"];
        args.extend(saved.iter().map(String::as_str));
        sec(&args);
    };
    restore();
    // Everything that can fail runs before the clean-up below.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        sec(&["unlock-keychain", "-p", "pc-test", k]);
        // macOS imports only legacy-format PKCS #12 (SHA-1 MAC).
        for f in ["rsa-legacy.p12", "ec-legacy.p12"] {
            let p = format!("{}/tests/data/{f}", env!("CARGO_MANIFEST_DIR"));
            let out = sec(&["import", &p, "-k", k, "-P", "test", "-A"]);
            assert!(out.status.success(), "{f}: {}", String::from_utf8_lossy(&out.stderr));
        }
        let ids = pdfcraft_sign::keychain::identities(Some(&kc)).unwrap();
        assert_eq!(ids.len(), 2, "{ids:?}");
        for id in &ids {
            assert!(id.key.is_external());
            let signed = pdfcraft_sign::sign(&open(&fixture()), id, &opts()).unwrap();
            let trusted = signatures(&open(&signed), &signed, &TrustStore { certs: vec![id.certificate.clone()] });
            let s = trusted.iter().find(|s| s.signed).unwrap();
            assert_eq!(s.status, Status::Valid, "{:?}", s.details);
        }
    }));
    sec(&["delete-keychain", k]);
    restore();
    let _ = std::fs::remove_dir_all(&dir);
    result.unwrap();
}

/// `fixture()` plus a Form XObject (object 7) that the page draws.
fn fixture_with_xobject() -> Vec<u8> {
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /X1 7 0 R >> >> /Annots [6 0 R] >>",
        b"<< /Length 8 >>\nstream\n/X1 Do\n\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 >>",
        b"<< /Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >> /Length 44 >>\nstream\nBT /F1 14 Tf 20 250 Td (Contract text) Tj ET\nendstream",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// An update that declares the form dynamic XFA (/XFA + /NeedsRendering) and swaps the whole
/// page tree for a new one. Marking a form XFA must not turn a page replacement into "form
/// fill": the signed pages are gone.
fn xfa_page_swap(doc: &mut Document) {
    let xdp = b"<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"><template/></xdp:xdp>".to_vec();
    let mut xd = pdfcraft_cos::Dict::new();
    xd.set(b"Length".to_vec(), Object::Int(xdp.len() as i64));
    let xfa = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(xd, xdp)));
    let body = b"BT /F1 14 Tf 20 250 Td (Forged text) Tj ET".to_vec();
    let mut cd = pdfcraft_cos::Dict::new();
    cd.set(b"Length".to_vec(), Object::Int(body.len() as i64));
    let contents = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(cd, body)));
    let pages = doc.add(Object::Null);
    let mut page = pdfcraft_cos::Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(300), Object::Int(300)]));
    page.set(b"Contents".to_vec(), Object::Ref(contents));
    let mut font = pdfcraft_cos::Dict::new();
    font.set(b"F1".to_vec(), Object::Ref(pdfcraft_cos::ObjRef { num: 5, generation: 0 }));
    let mut res = pdfcraft_cos::Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(font));
    page.set(b"Resources".to_vec(), Object::Dict(res));
    let page = doc.add(Object::Dict(page));
    let mut tree = pdfcraft_cos::Dict::new();
    tree.set(b"Type".to_vec(), Object::name("Pages"));
    tree.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
    tree.set(b"Count".to_vec(), Object::Int(1));
    doc.set(pages, Object::Dict(tree));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| {
        c.set(b"Pages".to_vec(), Object::Ref(pages));
        if let Some(Object::Dict(form)) = c.get_mut(b"AcroForm") {
            form.set(b"XFA".to_vec(), Object::Ref(xfa));
            form.set(b"NeedsRendering".to_vec(), Object::Bool(true));
        }
    })
    .unwrap();
}

#[test]
fn declaring_a_form_xfa_does_not_excuse_replacing_the_pages() {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let check = |bytes: &[u8]| signatures(&open(bytes), bytes, &trust).into_iter().find(|s| s.signed).unwrap();
    for certify in [None, Some(2), Some(3)] {
        let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { certify, ..opts() }).unwrap();
        assert_eq!(check(&signed).status, Status::Valid);
        let s = check(&edit_after(&signed, xfa_page_swap));
        assert_eq!(s.status, Status::Invalid, "{certify:?}: {:?}", s.details);
        assert!(matches!(&s.modification, Modification::Disallowed(k) if k.contains(&"document structure".to_string())), "{:?}", s.modification);
    }
}

#[test]
fn a_form_xobject_relabelled_metadata_is_not_a_metadata_change() {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let signed = pdfcraft_sign::sign(&open(&fixture_with_xobject()), &id, &opts()).unwrap();
    let relabel = |doc: &mut Document| {
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Type".to_vec(), Object::name("Metadata"));
        d.set(b"Subtype".to_vec(), Object::name("Form"));
        d.set(b"BBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(300), Object::Int(300)]));
        let body = b"BT /F1 14 Tf 20 250 Td (Forged text) Tj ET".to_vec();
        d.set(b"Length".to_vec(), Object::Int(body.len() as i64));
        doc.set(pdfcraft_cos::ObjRef { num: 7, generation: 0 }, Object::Stream(pdfcraft_cos::Stream::from_raw(d, body)));
    };
    let edited = edit_after(&signed, relabel);
    let s = signatures(&open(&edited), &edited, &trust).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Invalid, "{:?}", s.details);
    assert!(matches!(&s.modification, Modification::Disallowed(k) if !k.contains(&"metadata".to_string())), "{:?}", s.modification);
    // The catalog's own XMP stream may still be updated.
    let with_xmp = edit_after(&fixture_with_xobject(), |doc| {
        let xmp = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>".to_vec();
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Type".to_vec(), Object::name("Metadata"));
        d.set(b"Subtype".to_vec(), Object::name("XML"));
        d.set(b"Length".to_vec(), Object::Int(xmp.len() as i64));
        let r = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(d, xmp)));
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| c.set(b"Metadata".to_vec(), Object::Ref(r))).unwrap();
    });
    let signed_xmp = pdfcraft_sign::sign(&open(&with_xmp), &id, &opts()).unwrap();
    let rewrite_xmp = edit_after(&signed_xmp, |doc| {
        let root = doc.root().unwrap();
        let r = doc.get(root).as_dict().unwrap().get(b"Metadata").and_then(Object::as_ref).unwrap();
        let xmp = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><!-- edited --></x:xmpmeta>".to_vec();
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Type".to_vec(), Object::name("Metadata"));
        d.set(b"Subtype".to_vec(), Object::name("XML"));
        d.set(b"Length".to_vec(), Object::Int(xmp.len() as i64));
        doc.set(r, Object::Stream(pdfcraft_cos::Stream::from_raw(d, xmp)));
    });
    let s = signatures(&open(&rewrite_xmp), &rewrite_xmp, &trust).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert_eq!(s.modification, Modification::Allowed(vec!["metadata".into()]));
}

/// A self-signed CRL from `id` revoking its own certificate, valid from `this` to `next`.
fn self_crl(id: &pkcs12::DigitalId, this: Time, next: Time, revoked_at: Time) -> Vec<u8> {
    let alg = id.key.signature_algorithm(pdfcraft_sign::DigestAlg::Sha256);
    let tbs = der::seq(&[
        &der::int(1),
        &alg,
        &id.certificate.subject.raw,
        &this.encode(),
        &next.encode(),
        &der::seq(&[&der::seq(&[&der::uint(&id.certificate.serial), &revoked_at.encode()])]),
    ]);
    let sig = id.key.sign(pdfcraft_sign::DigestAlg::Sha256, &tbs).unwrap();
    der::seq(&[&tbs, &alg, &der::bit_string(&sig)])
}

#[test]
fn a_revoked_certificate_stays_invalid_when_it_was_also_expired_at_signing() {
    use pdfcraft_sign::dss::{self, Evidence};
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    // Signed in 2040, after the test certificate expired (2036).
    let late = SignOptions { date: "D:20400101120000Z".into(), ..opts() };
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &late).unwrap();
    let t = |year| Time { year, month: 1, day: 1, hour: 0, minute: 0, second: 0 };
    let crl = self_crl(&id, t(2039), t(2041), t(2039));
    let ltv = dss::embed(&open(&signed), &Evidence { certs: Vec::new(), ocsps: Vec::new(), crls: vec![crl] }).unwrap();
    let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
    for trust in [TrustStore::default(), TrustStore { certs: vec![anchor] }] {
        let s = signatures(&open(&ltv), &ltv, &trust).into_iter().find(|s| s.signed && !s.doc_timestamp).unwrap();
        assert!(s.details.iter().any(|d| d.contains("not valid at the time of signing")), "{:?}", s.details);
        assert_eq!(s.status, Status::Invalid, "{:?}", s.details);
        assert!(s.details.iter().any(|d| d.contains("revoked")), "{:?}", s.details);
    }
}

#[test]
fn an_untrusted_timestamp_does_not_set_the_validation_time() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    // The TSA claims 2027 (inside the signer's validity) but is not trusted; the signer's own
    // clock says 2040, after the certificate expired.
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2027, month: 1, day: 1, hour: 0, minute: 0, second: 0 },
    };
    let late = SignOptions { date: "D:20400101120000Z".into(), ..opts() };
    let signed = pdfcraft_sign::sign_with_timestamp(&open(&fixture()), &id, &late, &tsa).unwrap();
    let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
    let s = signatures(&open(&signed), &signed, &TrustStore { certs: vec![anchor.clone()] }).into_iter().find(|s| s.signed).unwrap();
    assert_ne!(s.status, Status::Valid, "{:?}", s.details);
    assert_eq!(s.timestamp_time, None, "an untrusted token's time is not a trusted time");
    assert!(s.details.iter().any(|d| d.contains("not valid at the time of signing")), "{:?}", s.details);
    assert!(!s.details.iter().any(|d| d.contains("trusted time")), "{:?}", s.details);
    assert!(s.details.iter().any(|d| d.contains("unverified timestamp")), "{:?}", s.details);
    // Trusting the TSA makes its time authoritative: the certificate was valid then.
    let trust = TrustStore { certs: vec![anchor, tsa.id.certificate.clone()] };
    let s = signatures(&open(&signed), &signed, &trust).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.timestamp_time, Some(tsa.time));
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
}

#[test]
fn a_timestamp_token_may_sign_with_a_different_digest_than_its_imprint() {
    let tsa = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let imprint = pdfcraft_sign::DigestAlg::Sha1.digest(&[b"document"]);
    let q = pdfcraft_sign::timestamp::TimestampQuery::new(pdfcraft_sign::DigestAlg::Sha1, imprint.clone()).unwrap();
    let time = Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 };
    let resp =
        pdfcraft_sign::timestamp::respond(&tsa.key, &tsa.certificate, &tsa.chain, pdfcraft_sign::DigestAlg::Sha256, &q, "1.2.3.4", time, 9).unwrap();
    let token = pdfcraft_sign::timestamp::parse_response(&resp, &q).unwrap();
    assert_eq!((token.digest, token.imprint, token.gen_time), (pdfcraft_sign::DigestAlg::Sha1, imprint, time));
}

#[cfg(windows)]
#[test]
fn windows_store_enumeration_and_missing_identity() {
    assert!(pdfcraft_sign::windows::identities().is_ok());
    assert!(pdfcraft_sign::windows::find("windows:no such signer").is_err());
}

/// Uses only newly created software-backed CNG keys, removed even when signing fails.
#[cfg(windows)]
#[test]
#[ignore = "creates temporary certificates in the Windows Current User Personal store"]
fn signing_with_windows_store_identities() {
    use std::process::Command;
    fn powershell(script: &str) -> std::process::Output {
        // Load the certificate provider explicitly in a profile-free Windows PowerShell child.
        let script = format!(
            r#"$ErrorActionPreference='Stop'; Import-Module "$PSHOME\Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1"; {script}"#
        );
        Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().unwrap()
    }
    struct Certificates(Vec<String>);
    impl Drop for Certificates {
        fn drop(&mut self) {
            for thumbprint in &self.0 {
                let out =
                    powershell(&format!("$ErrorActionPreference='Stop'; Remove-Item -LiteralPath 'Cert:\\CurrentUser\\My\\{thumbprint}' -DeleteKey"));
                if !out.status.success() {
                    eprintln!("test certificate cleanup failed: {}", String::from_utf8_lossy(&out.stderr));
                }
            }
        }
    }
    let mut created = Certificates(Vec::new());
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).unwrap();
    let unique: String = random.iter().map(|b| format!("{b:02x}")).collect();
    for algorithm in ["RSA", "ECDSA_nistP256", "ECDSA_nistP384"] {
        let name = format!("PdfCraft Test {unique} {algorithm}");
        let rsa = if algorithm == "RSA" { "-KeyLength 2048" } else { "" };
        let out = powershell(&format!(
            "$ErrorActionPreference='Stop'; $c=New-SelfSignedCertificate -Subject 'CN={name}' -CertStoreLocation 'Cert:\\CurrentUser\\My' -Provider 'Microsoft Software Key Storage Provider' -KeyAlgorithm {algorithm} {rsa} -KeyUsage DigitalSignature -KeyExportPolicy NonExportable; $c.Thumbprint"
        ));
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let thumbprint = String::from_utf8(out.stdout).unwrap().trim().to_string();
        assert_eq!(thumbprint.len(), 40);
        assert!(thumbprint.bytes().all(|b| b.is_ascii_hexdigit()));
        created.0.push(thumbprint);
        let listed = pdfcraft_sign::windows::identities().unwrap();
        let listed_id = listed.iter().find(|id| id.certificate.subject.common_name() == Some(name.as_str())).expect("new identity listed");
        let reference = pdfcraft_sign::windows::reference(&listed_id.certificate);
        let id = pdfcraft_sign::windows::find(&reference).unwrap();
        assert_eq!(pdfcraft_sign::windows::find(&format!("windows:{name}")).unwrap().certificate.raw, id.certificate.raw);
        assert!(id.key.is_external());
        let mut options = opts();
        let now = powershell("(Get-Date).ToUniversalTime().ToString('yyyyMMddHHmmss')");
        assert!(now.status.success());
        options.date = format!("D:{}Z", String::from_utf8(now.stdout).unwrap().trim());
        let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &options).unwrap();
        let validated = signatures(&open(&signed), &signed, &TrustStore { certs: vec![id.certificate.clone()] });
        let signature = validated.iter().find(|s| s.signed).unwrap();
        assert_eq!(signature.status, Status::Valid, "{:?}", signature.details);
        assert_eq!(signature.modification, Modification::None);
        assert_eq!(signature.signer.as_deref(), Some(name.as_str()));
    }
    let thumbprints = created.0.clone();
    drop(created);
    for thumbprint in thumbprints {
        let out = powershell(&format!("if (Test-Path -LiteralPath 'Cert:\\CurrentUser\\My\\{thumbprint}') {{ exit 1 }}"));
        assert!(out.status.success(), "test certificate remains: {thumbprint}");
    }
}
