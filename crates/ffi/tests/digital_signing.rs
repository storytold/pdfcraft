#![cfg(feature = "sign")]
use pdfcraft_ffi::{Engine, FfiPoint, FfiRect, FfiSignOptions};

const SAMPLE_PDF: &str = "%PDF-1.7\n\
1 0 obj\n\
<< /Type /Catalog /Pages 2 0 R >>\n\
endobj\n\
2 0 obj\n\
<< /Type /Pages /Kids [3 0 R] /Count 1 >>\n\
endobj\n\
3 0 obj\n\
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>\n\
endobj\n\
4 0 obj\n\
<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\n\
endobj\n\
5 0 obj\n\
<< /Length 54 >>\n\
stream\n\
BT\n\
/F1 24 Tf\n\
100 700 Td\n\
(OFFICIAL CONTRACT) Tj\n\
ET\n\
endstream\n\
endobj\n\
xref\n\
0 6\n\
0000000000 65535 f \n\
0000000009 00000 n \n\
0000000058 00000 n \n\
0000000115 00000 n \n\
0000000227 00000 n \n\
0000000298 00000 n \n\
trailer\n\
<< /Size 6 /Root 1 0 R >>\n\
startxref\n\
403\n\
%%EOF\n";

#[test]
fn test_digital_id_creation_and_inspection() {
    let engine = Engine::new();

    // 1. Password validation: < 6 chars fails
    let short_pw_err = engine.create_self_signed_digital_id(
        "Ada Lovelace".into(),
        "123".into(),
        "rsa2048".into(),
        Some("Storytold".into()),
        None,
        Some("ada@example.com".into()),
        Some("US".into()),
        5,
    );
    assert!(short_pw_err.is_err());

    // 2. Create RSA 2048 digital ID
    let rsa_p12 = engine
        .create_self_signed_digital_id(
            "Ada Lovelace".into(),
            "secretpass123".into(),
            "rsa2048".into(),
            Some("Storytold".into()),
            Some("Engineering".into()),
            Some("ada@example.com".into()),
            Some("US".into()),
            3,
        )
        .expect("create rsa2048 digital ID");
    assert!(!rsa_p12.is_empty());

    // 3. Inspect with incorrect password fails
    let wrong_pw_err = engine.inspect_digital_id(rsa_p12.clone(), Some("wrongpassword".into()));
    assert!(wrong_pw_err.is_err());

    // 4. Inspect with correct password succeeds
    let id_info = engine.inspect_digital_id(rsa_p12.clone(), Some("secretpass123".into())).expect("inspect digital id");
    assert_eq!(id_info.common_name, "Ada Lovelace");
    assert_eq!(id_info.organization.as_deref(), Some("Storytold"));
    assert_eq!(id_info.unit.as_deref(), Some("Engineering"));
    assert_eq!(id_info.email.as_deref(), Some("ada@example.com"));
    assert_eq!(id_info.country.as_deref(), Some("US"));
    assert!(id_info.key_algorithm.contains("RSA 2048-bit"));
    assert!(!id_info.serial_hex.is_empty());
    assert!(!id_info.sha256_fingerprint.is_empty());

    // 5. Create ECDSA P-256 digital ID
    let p256_p12 = engine
        .create_self_signed_digital_id(
            "Charles Babbage".into(),
            "analyticalengine".into(),
            "p256".into(),
            Some("Royal Society".into()),
            None,
            None,
            Some("GB".into()),
            5,
        )
        .expect("create p256 digital id");
    assert!(!p256_p12.is_empty());

    let p256_info = engine.inspect_digital_id(p256_p12, Some("analyticalengine".into())).expect("inspect p256 id");
    assert_eq!(p256_info.common_name, "Charles Babbage");
    assert_eq!(p256_info.key_algorithm, "ECDSA P-256");
}

#[test]
fn test_document_digital_signing_and_verification() {
    let engine = Engine::new();

    // 1. Create a digital ID for signing
    let p12_bytes = engine
        .create_self_signed_digital_id(
            "Ada Lovelace".into(),
            "supersecret123".into(),
            "rsa2048".into(),
            Some("Analytical Labs".into()),
            None,
            Some("ada@analytical.org".into()),
            Some("GB".into()),
            10,
        )
        .expect("create digital ID");

    // 2. Open PDF document
    let doc_id = engine.open_bytes("contract.pdf".into(), SAMPLE_PDF.as_bytes().to_vec(), None).expect("open document");

    // 3. Initially, no signatures
    let initial_sigs = engine.list_digital_signatures(doc_id).expect("list signatures");
    assert_eq!(initial_sigs.len(), 0);

    // 4. Digitally sign document with visual graphic and certify level
    let sign_opts = FfiSignOptions {
        field_name: None,
        page: Some(0),
        rect: Some(FfiRect { x: 50.0, y: 50.0, width: 200.0, height: 60.0 }),
        reason: Some("Approved and Certified".into()),
        location: Some("London, UK".into()),
        contact: Some("ada@analytical.org".into()),
        certify_level: Some(1), // No changes allowed
        graphic_text: Some("Ada Lovelace".into()),
        graphic_strokes: None,
        out_path: None,
    };

    let sign_result = engine.sign_document_p12(doc_id, p12_bytes.clone(), Some("supersecret123".into()), sign_opts).expect("sign document");

    assert!(sign_result.success);
    assert!(!sign_result.bytes.is_empty());
    assert_eq!(sign_result.signer_name, "Ada Lovelace");
    assert_eq!(sign_result.revision, 1);

    // 5. Query signatures on the updated document
    let sigs_after = engine.list_digital_signatures(doc_id).expect("list signatures after sign");
    assert_eq!(sigs_after.len(), 1);

    let sig = &sigs_after[0];
    assert!(sig.signed);
    assert_eq!(sig.signer.as_deref(), Some("Ada Lovelace"));
    assert_eq!(sig.reason.as_deref(), Some("Approved and Certified"));
    assert_eq!(sig.location.as_deref(), Some("London, UK"));
    assert_eq!(sig.contact.as_deref(), Some("ada@analytical.org"));
    assert_eq!(sig.certify, Some(1));
    assert_eq!(sig.page, Some(0));
    assert!(sig.rect.is_some());
    // In self-signed without external CA trust store, status is either valid or unknown depending on trust store
    assert!(sig.status == "valid" || sig.status == "unknown");

    // 6. Test drawn strokes graphic signing with P-256
    let p256_p12 = engine
        .create_self_signed_digital_id(
            "Charles Babbage".into(),
            "secondsecret456".into(),
            "p256".into(),
            Some("Royal Society".into()),
            None,
            None,
            Some("GB".into()),
            5,
        )
        .expect("create p256 digital id");

    let drawn_strokes = vec![vec![FfiPoint { x: 0.1, y: 0.5 }, FfiPoint { x: 0.3, y: 0.8 }, FfiPoint { x: 0.9, y: 0.2 }]];

    let p256_sign_opts = FfiSignOptions {
        field_name: None,
        page: Some(0),
        rect: Some(FfiRect { x: 300.0, y: 50.0, width: 180.0, height: 50.0 }),
        reason: Some("Witnessed and Reviewed".into()),
        location: Some("Cambridge".into()),
        contact: None,
        certify_level: None,
        graphic_text: None,
        graphic_strokes: Some(drawn_strokes),
        out_path: None,
    };

    let p256_result = engine.sign_document_p12(doc_id, p256_p12, Some("secondsecret456".into()), p256_sign_opts).expect("sign with p256");

    assert!(p256_result.success);
    assert_eq!(p256_result.signer_name, "Charles Babbage");
    assert_eq!(p256_result.revision, 2);

    let sigs_after2 = engine.list_digital_signatures(doc_id).expect("list signatures after 2nd sign");
    assert_eq!(sigs_after2.len(), 2);
    assert_eq!(sigs_after2[1].signer.as_deref(), Some("Charles Babbage"));
    assert_eq!(sigs_after2[1].reason.as_deref(), Some("Witnessed and Reviewed"));

    engine.close(doc_id).expect("close document");
}
