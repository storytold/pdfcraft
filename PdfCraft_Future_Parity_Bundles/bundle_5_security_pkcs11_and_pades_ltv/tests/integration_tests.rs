//! Comprehensive Integration Tests for Bundle 5: Enterprise Security, PKCS#11, and PAdES LTV.

use pdfcraft_public_key_crypt::PublicKeySecurityHandler;
use pdfcraft_pades_ltv::{DocumentSecurityStoreBuilder, Rfc3161Client};
use pdfcraft_pkcs11_bridge::Pkcs11Session;
use pdfcraft_signature_history::{CertificateTrustStatus, DocumentRevision, FieldLockDefinition, LockAction, RevisionHistoryScanner, SignatureIntegrity, SignaturePropertiesSummary};

#[test]
fn test_public_key_recipient_envelope_security() {
    let recipient_data = b"[ <4142434445464748> <3132333435363738> ]";
    let mut handler = PublicKeySecurityHandler::new(recipient_data);
    assert!(!handler.is_unlocked());

    handler.unlock_with_private_key("Recipient-2", b"valid_user_private_key").expect("Unlock failed");
    assert!(handler.is_unlocked());
    assert!(handler.active_permissions().can_print_high_res());
}

#[test]
fn test_pades_ltv_and_rfc3161_timestamp_pipeline() {
    let digest = [0xABu8; 32];
    let req = Rfc3161Client::build_timestamp_request(&digest, 123456789);
    assert!(req.len() > 32);

    let mut dss = DocumentSecurityStoreBuilder::new();
    let cert_id = dss.add_certificate(vec![0x30, 0x82, 0x01]);
    let ocsp_id = dss.add_ocsp_response(vec![0x30, 0x82, 0x02]);
    dss.register_vri_entry("11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF", cert_id, ocsp_id);

    let dss_serialized = dss.serialize_dss_dict(100);
    assert!(dss_serialized.contains("/Type /DSS"));
    assert!(dss_serialized.contains("/VRI <<"));
}

#[test]
fn test_pkcs11_hardware_signing_session() {
    let mut session = Pkcs11Session::open_session(1);
    session.login_user_pin("8888").unwrap();

    let certs = session.list_certificates().unwrap();
    assert_eq!(certs.len(), 1);

    let digest = [0x12u8; 32];
    let sig_bytes = session.sign_digest(&certs[0].label, &digest).unwrap();
    assert_eq!(sig_bytes.len(), 256);
}

#[test]
fn test_multi_revision_rollback_and_fieldmdp_locks() {
    let multi_rev_pdf = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
xref\n0 2\n0000000000 65535 f \r\n0000000009 00000 n \r\n\
trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n50\n%%EOF\n\
3 0 obj\n<< /Type /Sig /Name (Alice) /M (D:20261021) >>\nendobj\n\
xref\n3 1\n0000000100 00000 n \r\n\
trailer\n<< /Size 4 /Root 1 0 R /Prev 50 >>\nstartxref\n160\n%%EOF";

    let revisions = RevisionHistoryScanner::scan_revisions(multi_rev_pdf);
    assert_eq!(revisions.len(), 2);
    assert!(!revisions[0].is_signed);
    assert!(revisions[1].is_signed);
    assert_eq!(revisions[1].signer_name.as_deref(), Some("Alice"));

    // Extract historical snapshot of revision 1 (before signature)
    let rev1_snapshot = RevisionHistoryScanner::extract_historical_snapshot(multi_rev_pdf, &revisions[0]);
    assert!(rev1_snapshot.ends_with(b"%%EOF"));
    assert!(!String::from_utf8_lossy(rev1_snapshot).contains("/Type /Sig"));

    // FieldMDP Lock verification
    let mut locked_fields = std::collections::HashSet::new();
    locked_fields.insert("SSN".to_string());
    locked_fields.insert("DateSigned".to_string());

    let lock = FieldLockDefinition { action: LockAction::Include(locked_fields) };
    assert!(lock.is_field_locked("SSN"));
    assert!(!lock.is_field_locked("Notes"));
}
