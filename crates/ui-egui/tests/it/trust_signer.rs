//! The Signatures panel's one-click "Add to trusted certificates" must store the signer shown in
//! the row, not the chain top the document embedded in the CMS (audit v3-098). Signing with a
//! certificate whose CMS carries its issuer chain gives a signature whose `chain.last()` (the
//! self-signed root) differs from its `certificate` (the signer); the click must trust the signer.

use crate::support;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::SignatureStatus;
use pdfcraft_ui_egui::{PdfCraftApp, SignStep};

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length 45 >> stream
BT /F1 14 Tf 20 150 Td (Please sign below) Tj ET
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("pdfcraft-trust-signer-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn harness(dir: std::path::PathBuf) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("contract.pdf", None, FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.export_dir_override = Some(dir.to_string_lossy().into_owned());
        app.save_override = Some(dir.join("contract_signed.pdf").to_string_lossy().into_owned());
        app
    });
    h.run_steps(6);
    h
}

/// One click trusts the signer the row shows, and as a signer only: never the root the document embeds, and never as a
/// root.
#[test]
fn one_click_trust_stores_the_displayed_signer_not_the_embedded_root() {
    let dir = dir();
    let mut h = harness(dir);
    // Sign with chain.p12, whose CMS embeds the signer's issuer chain (signer + self-signed root).
    let p12 = concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/chain.p12");
    let id = pdfcraft_engine::sign::pkcs12::open(&std::fs::read(p12).unwrap(), "test").unwrap();
    let entry = h.state_mut().add_digital_id(p12, &id.certificate);
    h.state_mut().start_signing(0, None, None, None);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.selected = Some(entry);
        d.step = SignStep::SignAs;
        d.password = "test".into();
    }
    h.run_steps(2);
    support::outside_menus(&h, "Sign").click();
    h.run_steps(6);

    // The signature validates as Unknown and carries a two-certificate chain whose top (the root)
    // is a different certificate from the displayed signer.
    let (signer, root) = {
        let s = h.state();
        let sig = s.session.get(s.views[0].id).unwrap().signatures.iter().find(|x| x.signed).cloned().expect("a signed field");
        assert_eq!(sig.status, SignatureStatus::Unknown, "untrusted before the click");
        assert_eq!(sig.chain.len(), 2, "the CMS embeds the signer and its root: {:?}", sig.chain.len());
        let signer = sig.certificate.clone().expect("a signer certificate");
        let root = sig.chain.last().cloned().expect("a chain top");
        assert_ne!(signer.raw, root.raw, "the embedded chain top must differ from the signer for this test to be meaningful");
        (signer, root)
    };

    // Expand the signature row and use the one-click trust button.
    h.get_by_label_contains("Signed by").click();
    h.run_steps(2);
    h.get_by_label("Add to trusted certificates").click();
    h.run_steps(3);

    // The trust store holds exactly the displayed signer, never the document-supplied root, and
    // holds it as a signer only: one click makes no certificate a root that vouches for what it
    // issues (#6758).
    let session = &h.state().session;
    let trusted = session.trusted_signers().to_vec();
    assert!(trusted.iter().any(|c| c.raw == signer.raw), "the displayed signer should be trusted");
    assert!(!trusted.iter().any(|c| c.raw == root.raw), "the embedded root must not be trusted by the one-click action");
    assert!(session.trusted_certificates().is_empty(), "one click trusts no root");
    let sig = session.get(h.state().views[0].id).unwrap().signatures.iter().find(|x| x.signed).cloned().unwrap();
    assert_eq!(sig.status, SignatureStatus::Valid, "the trusted signer's own signature is valid");
}

/// Settings written before trust had a scope (#6758) kept one "trusted" list, filled by one-click
/// trust. Its entries are read as trusted signers, never as roots; trust saved since keeps roots
/// and signers apart.
#[test]
fn trust_from_older_settings_is_read_as_signers_and_scopes_persist() {
    let cert = |name: &str| {
        let p12 = format!("{}/../sign/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
        pdfcraft_engine::sign::pkcs12::open(&std::fs::read(p12).unwrap(), "test").unwrap().certificate
    };
    let (signer, root, tsa) = (cert("rsa-aes.p12"), cert("ec-p256.p12"), cert("ec-p384.p12"));
    let pem = pdfcraft_engine::sign::x509::to_pem;
    let mut old = PdfCraftApp::new();
    old.restore(&serde_json::json!({ "trusted": [pem(&signer)] }).to_string());
    assert!(old.session.trusted_certificates().is_empty(), "an old entry is not a root");
    assert_eq!(old.session.trusted_signers().iter().map(|c| c.raw.clone()).collect::<Vec<_>>(), std::slice::from_ref(&signer.raw));
    old.trust_root(root.clone());
    old.trust_tsa(tsa.clone());
    let saved = old.persist();
    let saved_json: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert!(saved_json.get("trusted").is_none(), "the unscoped list is no longer written");
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&saved);
    let raws = |c: &[pdfcraft_engine::sign::Certificate]| c.iter().map(|c| c.raw.clone()).collect::<Vec<_>>();
    assert_eq!(raws(fresh.session.trusted_certificates()), std::slice::from_ref(&root.raw));
    assert_eq!(raws(fresh.session.trusted_signers()), std::slice::from_ref(&signer.raw));
    assert_eq!(raws(fresh.session.trusted_tsas()), std::slice::from_ref(&tsa.raw), "timestamp authorities persist apart (#6786)");
}

/// A deterministic timestamp authority: a self-signed certificate issued for time stamping alone,
/// answering RFC 3161 requests in-process at a fixed time.
struct TestTsa {
    id: pdfcraft_engine::sign::DigitalId,
    time: pdfcraft_engine::sign::Time,
}

impl TestTsa {
    fn new() -> Self {
        use pdfcraft_engine::sign::{Certificate, DigitalId, Name, PrivateKey, Time, der};
        let key = PrivateKey::generate_p256().unwrap();
        let name = Name::build("Test TSA", "", "PdfCraft Tests", "", "US");
        let alg = key.preferred_digest();
        let sig_alg = key.signature_algorithm(alg);
        let from = Time { year: 2026, month: 10, day: 2, hour: 0, minute: 0, second: 0 };
        let until = Time { year: 2036, ..from };
        let eku = der::seq(&[&der::oid("2.5.29.37"), &der::boolean(true), &der::octets(&der::seq(&[&der::oid("1.3.6.1.5.5.7.3.8")]))]);
        let tbs = der::seq(&[
            &der::explicit(0, &der::int(2)),
            &der::uint(&[0x31]),
            &sig_alg,
            &name.raw,
            &der::seq(&[&from.encode(), &until.encode()]),
            &name.raw,
            &key.public_key().spki(),
            &der::explicit(3, &der::seq(&[&eku])),
        ]);
        let signature = key.sign(alg, &tbs).unwrap();
        let certificate = Certificate::parse(&der::seq(&[&tbs, &sig_alg, &der::bit_string(&signature)])).unwrap();
        let id = DigitalId { key, certificate, chain: Vec::new(), friendly_name: None };
        TestTsa { id, time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 } }
    }
}

impl pdfcraft_engine::sign::TimestampAuthority for TestTsa {
    fn timestamp(&self, request: &[u8]) -> Result<Vec<u8>, pdfcraft_engine::sign::SignError> {
        use pdfcraft_engine::sign::timestamp;
        let q = timestamp::parse_request(request)?;
        timestamp::respond(
            timestamp::TimestampSigner {
                key: &self.id.key,
                certificate: &self.id.certificate,
                chain: &self.id.chain,
                digest: pdfcraft_engine::sign::DigestAlg::Sha256,
            },
            timestamp::TimestampIssuance { query: &q, policy: "1.2.3.4", generation_time: self.time, serial: 7 },
        )
    }
}

/// #6788 review: on a document timestamp, the one-click action trusts its authority for
/// timestamps. Trusted as a signer it would leave the timestamp Unknown (#6758).
#[test]
fn one_click_trust_on_a_document_timestamp_trusts_its_authority_for_timestamps() {
    let tsa = TestTsa::new();
    let stamped = {
        let mut app = PdfCraftApp::new();
        app.open_bytes("contract.pdf", None, FIXTURE.to_vec()).expect("opens");
        let id = app.views[0].id;
        app.session.timestamp_document(id, &tsa, "D:20261006120000Z".into()).expect("stamps").to_vec()
    };
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("stamped.pdf", None, stamped.clone()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.choose_right_panel(Some(pdfcraft_ui_egui::RightPanel::Signatures));
        app
    });
    h.run_steps(6);
    let timestamp = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().signatures.iter().find(|x| x.doc_timestamp).cloned().expect("a document timestamp")
    };
    assert_eq!(timestamp(&h).status, SignatureStatus::Unknown, "untrusted before the click");

    h.get_by_label_contains("Signed by").click();
    h.run_steps(2);
    h.get_by_label("Add to trusted certificates").click();
    h.run_steps(3);

    let session = &h.state().session;
    let raws = |c: &[pdfcraft_engine::sign::Certificate]| c.iter().map(|c| c.raw.clone()).collect::<Vec<_>>();
    assert_eq!(raws(session.trusted_tsas()), std::slice::from_ref(&tsa.id.certificate.raw), "trusted for timestamps");
    assert!(session.trusted_signers().is_empty(), "not as a signer");
    assert!(session.trusted_certificates().is_empty(), "not as a root");
    assert_eq!(timestamp(&h).status, SignatureStatus::Valid, "the timestamp is trusted time");
}
