//! Signing and validating whole documents.

use std::sync::Arc;

use printcraft_cos::{Document, Object, PdfString, SaveOptions, write_incremental};
use printcraft_sign::{Modification, SignError, SignOptions, Status, Time, TimestampAuthority, TrustStore, der, pkcs12, signatures};

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
        let signed = printcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
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
    let first = printcraft_sign::sign(&open(&fixture()), &id, &SignOptions { field: Some("Approval".into()), ..opts() }).unwrap();
    let id2 = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let second = printcraft_sign::sign(&open(&first), &id2, &opts()).unwrap();
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
        printcraft_sign::sign(&doc, &id, &SignOptions { field: Some("Approval".into()), ..opts() }),
        Err(printcraft_sign::SignError::Pdf(_))
    ));
}

/// Append a revision that changes object `num` with `f`.
fn edit_after(signed: &[u8], f: impl FnOnce(&mut Document)) -> Vec<u8> {
    let mut doc = open(signed);
    f(&mut doc);
    write_incremental(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap()
}

fn add_comment(doc: &mut Document) {
    let mut d = printcraft_cos::Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name("Text"));
    d.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(10), Object::Int(10), Object::Int(30), Object::Int(30)]));
    d.set(b"Contents".to_vec(), PdfString::text("A note"));
    let r = doc.add(Object::Dict(d));
    let page = printcraft_cos::ObjRef { num: 3, generation: 0 };
    doc.update_dict(page, |p| {
        if let Some(Object::Array(a)) = p.get_mut(b"Annots") {
            a.push(Object::Ref(r));
        }
    })
    .unwrap();
}

fn change_text(doc: &mut Document) {
    let mut d = printcraft_cos::Dict::new();
    d.set(b"Length".to_vec(), Object::Int(40));
    let s = printcraft_cos::Stream::from_raw(d, b"BT /F1 14 Tf 20 250 Td (Other text) Tj ET".to_vec());
    doc.set(printcraft_cos::ObjRef { num: 4, generation: 0 }, Object::Stream(s));
}

#[test]
fn later_changes_are_classified_under_the_signature_permissions() {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let check = |bytes: &[u8]| signatures(&open(bytes), bytes, &trust).into_iter().find(|s| s.signed).unwrap();
    // Approval signature: comments are permitted, rewriting page content is not.
    let signed = printcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let s = check(&edit_after(&signed, add_comment));
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert_eq!(s.modification, Modification::Allowed(vec!["comments".into()]));
    let s = check(&edit_after(&signed, change_text));
    assert_eq!(s.status, Status::Invalid);
    assert_eq!(s.modification, Modification::Disallowed(vec!["page content".into()]));
    // Certified with "no changes allowed": even a comment invalidates it.
    let certified = printcraft_sign::sign(&open(&fixture()), &id, &SignOptions { certify: Some(1), ..opts() }).unwrap();
    let s = check(&certified);
    assert_eq!((s.certify, s.status), (Some(1), Status::Valid));
    assert_eq!(check(&edit_after(&certified, add_comment)).status, Status::Invalid);
    // Certified allowing comments: fine.
    let certified3 = printcraft_sign::sign(&open(&fixture()), &id, &SignOptions { certify: Some(3), ..opts() }).unwrap();
    assert_eq!(check(&edit_after(&certified3, add_comment)).status, Status::Valid);
}

#[test]
fn invisible_signatures_and_refusals() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = printcraft_sign::sign(&open(&fixture()), &id, &SignOptions { rect: None, ..opts() }).unwrap();
    let s = signatures(&open(&signed), &signed, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
    assert!(!s.visible);
    assert!(printcraft_sign::sign(&open(&fixture()), &id, &SignOptions { page: 5, ..opts() }).is_err());
    assert_eq!(printcraft_sign::pdf::display_date("D:20261002120000+01'00'"), "2026.10.02 12:00:00 +01'00'");
}

/// A deterministic TSA: signs an RFC 3161 response locally with a test digital ID at a fixed
/// time. No sockets; the whole stamping path runs in-process.
struct TestTsa {
    id: pkcs12::DigitalId,
    time: Time,
}

impl TimestampAuthority for TestTsa {
    fn timestamp(&self, request: &[u8]) -> Result<Vec<u8>, SignError> {
        let q = printcraft_sign::timestamp::parse_request(request)?;
        printcraft_sign::timestamp::respond(
            &self.id.key,
            &self.id.certificate,
            &self.id.chain,
            printcraft_sign::DigestAlg::Sha256,
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
    let signed = printcraft_sign::sign_with_timestamp(&open(&fixture()), &id, &opts(), &tsa).unwrap();
    assert!(signed.starts_with(&fixture()), "still an incremental update");
    let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
    let s = signatures(&open(&signed), &signed, &TrustStore { certs: vec![anchor] }).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert!(s.timestamp, "{:?}", s.details);
    assert_eq!(s.timestamp_time, Some(tsa.time), "the token's generation time, verified over the signature value");
    assert!(s.details.iter().any(|d| d.contains("trusted time")), "{:?}", s.details);
    // Untrusted, the signature stays intact-but-unknown; the timestamp is still reported.
    let s = signatures(&open(&signed), &signed, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Unknown);
    assert_eq!(s.timestamp_time, Some(tsa.time));
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
    assert!(printcraft_sign::sign_with_timestamp(&open(&fixture()), &id, &opts(), &Bad).is_err());
}

#[test]
fn a_document_timestamp_covers_the_file_and_validates() {
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let stamped = printcraft_sign::timestamp_document(&open(&fixture()), &tsa, "D:20261006120000Z").unwrap();
    assert!(stamped.starts_with(&fixture()), "an incremental update");
    let s = signatures(&open(&stamped), &stamped, &TrustStore::default()).into_iter().find(|s| s.doc_timestamp).unwrap();
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
    let signed = printcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let both = printcraft_sign::timestamp_document(&open(&signed), &tsa, "D:20261006130000Z").unwrap();
    let all = signatures(&open(&both), &both, &TrustStore::default());
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
fn embedding_ltv_evidence_adds_a_dss_and_keeps_signatures_valid() {
    use printcraft_sign::dss::{self, Evidence};
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = printcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let evidence =
        Evidence { certs: vec![id.certificate.raw.clone()], ocsps: vec![b"synthetic ocsp".to_vec()], crls: vec![b"synthetic crl".to_vec()] };
    let ltv = dss::embed(&open(&signed), &evidence).unwrap();
    assert!(ltv.starts_with(&signed), "incremental");
    let doc = open(&ltv);
    // The store is in the catalog with one certificate and the /VRI entry for the signature.
    let root = doc.root().unwrap();
    let dss_dict: printcraft_cos::Dict =
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
    let dss2: printcraft_cos::Dict =
        doc2.get(doc2.root().unwrap()).as_dict().unwrap().get(b"DSS").map(|d| doc2.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap();
    let certs = dss2.get(b"Certs").map(|c| doc2.resolve(c)).and_then(|c| c.as_array().cloned()).unwrap();
    assert_eq!(certs.len(), 1, "byte-identical evidence is not duplicated");
}

#[test]
fn sign_then_ltv_then_timestamp_makes_a_b_lta_file() {
    use printcraft_sign::dss::{self, Evidence};
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let signed = printcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let ltv = dss::embed(&open(&signed), &Evidence { certs: vec![id.certificate.raw.clone()], ocsps: Vec::new(), crls: Vec::new() }).unwrap();
    let lta = printcraft_sign::timestamp_document(&open(&ltv), &tsa, "D:20261006120000Z").unwrap();
    let all = signatures(&open(&lta), &lta, &TrustStore::default());
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
    use printcraft_sign::dss::{self, Evidence};
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = printcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    // The signer's own CRL (self-signed test identity) revokes its certificate, in a window
    // that covers the signing date (October 2026).
    let (this, next) =
        (Time { year: 2026, month: 1, day: 1, hour: 0, minute: 0, second: 0 }, Time { year: 2027, month: 1, day: 1, hour: 0, minute: 0, second: 0 });
    let alg = id.key.signature_algorithm(printcraft_sign::DigestAlg::Sha256);
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
    let sig = id.key.sign(printcraft_sign::DigestAlg::Sha256, &tbs).unwrap();
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
    let rsa = printcraft_sign::Certificate::parse(&pkcs12::open(&data("rsa-aes.p12"), "test").unwrap().certificate.raw).unwrap();
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
        let signed = printcraft_sign::sign(&open(&bytes), &id, &SignOptions { certify, rect: None, ..opts() }).unwrap();
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
    let dir = std::env::temp_dir().join(format!("printcraft-keychain-{}", std::process::id()));
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
        let ids = printcraft_sign::keychain::identities(Some(&kc)).unwrap();
        assert_eq!(ids.len(), 2, "{ids:?}");
        for id in &ids {
            assert!(id.key.is_external());
            let signed = printcraft_sign::sign(&open(&fixture()), id, &opts()).unwrap();
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
