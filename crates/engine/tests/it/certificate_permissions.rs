//! What a certificate-protected document grants each recipient (ISO 32000-2, §7.6.5, Tables 23-27):
//! the permissions an envelope carries are read as revision 3's, bit 2 allows everything, equal
//! permissions share one CMS object, and the refusals say what is wrong.
use crate::support::certificate::{identity, one_page};

use std::sync::Arc;

use pdfcraft_engine::sign::DigitalId;
use pdfcraft_engine::{Algorithm, CertificateProtection, CertificateRecipient, DocId, Edit, Session};
use pdfcraft_render::OpenError;

/// Printing low resolution, filling forms and accessibility: the bits Acrobat's "printing low,
/// fill-sign" writes (3, 9 and 10), the `00000304` the envelope holds.
const LOW_FILL_SIGN: i32 = 0x304;

/// Ada Lovelace's digital ID: another RSA key than [`identity`].
fn bob() -> DigitalId {
    let p12 = std::fs::read(format!("{}/../sign/tests/data/chain.p12", env!("CARGO_MANIFEST_DIR"))).unwrap();
    pdfcraft_engine::sign::pkcs12::open(&p12, "test").unwrap()
}

/// A one-page file encrypted for `recipients` (certificate, `/P`), as Save writes it.
fn protect(recipients: &[(&DigitalId, i32)], algorithm: Algorithm) -> Arc<Vec<u8>> {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("plain.pdf", None, Arc::new(one_page(&[])), None).unwrap();
    let protection = CertificateProtection {
        recipients: recipients.iter().map(|(i, p)| CertificateRecipient { certificate: i.certificate.raw.clone(), permissions: *p }).collect(),
        algorithm,
        encrypt_metadata: true,
        seed: s.new_seed().unwrap(),
    };
    s.apply(id, Edit::ProtectWithCertificates(protection)).unwrap();
    s.save_bytes(id).unwrap()
}

fn open_as(file: &Arc<Vec<u8>>, who: DigitalId) -> (Session, DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    s.add_identity(who);
    let id = s.open("secret.pdf", None, file.clone(), None).unwrap();
    (s, id)
}

#[test]
fn a_restricted_recipient_gets_the_permissions_the_envelope_holds() {
    let (alice, bob) = (identity(), bob());
    for algorithm in [Algorithm::Aes256, Algorithm::Aes128] {
        let file = protect(&[(&alice, -1), (&bob, LOW_FILL_SIGN)], algorithm);
        let (s, id) = open_as(&file, self::bob());
        let p = s.get(id).unwrap().permissions().unwrap();
        assert!(p.print() && !p.print_high_quality(), "printing low");
        assert!(p.fill_forms() && p.extract_for_accessibility(), "fill and sign, accessibility");
        assert!(!p.modify() && !p.copy() && !p.annotate(), "nothing else");
        // The owner has everything.
        let (s, id) = open_as(&file, identity());
        let doc = s.get(id).unwrap();
        assert!(doc.permissions().unwrap().print_high_quality() && doc.allows_security_change());
    }
}

#[test]
fn bit_2_of_the_envelope_permits_everything() {
    let file = protect(&[(&identity(), 0b10)], Algorithm::Aes256);
    let (s, id) = open_as(&file, identity());
    let doc = s.get(id).unwrap();
    let p = doc.permissions().unwrap();
    assert!(p.print_high_quality() && p.modify() && p.copy() && p.annotate() && p.assemble() && p.fill_forms());
    assert!(doc.allows_security_change(), "it permits change of encryption too");
}

#[test]
fn recipients_with_equal_permissions_share_one_cms_object() {
    let (alice, bob) = (identity(), bob());
    let count = |file: &Arc<Vec<u8>>| match pdfcraft_cos::Document::open(file.clone()) {
        Err(pdfcraft_cos::CosError::NeedsCertificate(envelopes)) => {
            let people: usize = envelopes.iter().map(|e| pdfcraft_engine::sign::envelope::recipient_count(e)).sum();
            (envelopes.len(), people)
        }
        other => panic!("expected NeedsCertificate, got {other:?}"),
    };
    assert_eq!(count(&protect(&[(&alice, LOW_FILL_SIGN), (&bob, LOW_FILL_SIGN)], Algorithm::Aes256)), (1, 2));
    // The owner's envelope (every permission) stays apart from the restricted one's.
    let file = protect(&[(&alice, -1), (&bob, LOW_FILL_SIGN)], Algorithm::Aes256);
    assert_eq!(count(&file), (2, 2));
    // One envelope with two recipients opens for each of them.
    let shared = protect(&[(&alice, LOW_FILL_SIGN), (&bob, LOW_FILL_SIGN)], Algorithm::Aes128);
    for who in [identity(), self::bob()] {
        let (s, id) = open_as(&shared, who);
        assert!(s.get(id).unwrap().permissions().unwrap().print());
    }
}

#[test]
fn a_certificate_protected_document_is_reported_encrypted_and_counts_recipients() {
    let (alice, bob) = (identity(), bob());
    let file = protect(&[(&alice, LOW_FILL_SIGN), (&bob, LOW_FILL_SIGN)], Algorithm::Aes256);
    let (s, id) = open_as(&file, identity());
    assert!(s.get(id).unwrap().info.encrypted, "the file is encrypted, though the renderer reads a decrypted copy");
    let method = s.get(id).unwrap().security_summary().unwrap().method;
    assert!(method.contains("2 recipients"), "one object, two recipients: {method}");
    // Still so after an edit and its refresh (the owner may edit).
    let file = protect(&[(&alice, -1), (&bob, LOW_FILL_SIGN)], Algorithm::Aes256);
    let (mut s, id) = open_as(&file, identity());
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "T".into() }).unwrap();
    assert!(s.get(id).unwrap().info.encrypted);
    assert!(s.get(id).unwrap().security_summary().unwrap().certificate);
}

#[test]
fn the_refusals_say_why() {
    let file = protect(&[(&identity(), -1)], Algorithm::Aes256);
    let refused = |who: Option<DigitalId>| {
        let mut s = Session::new();
        if let Some(who) = who {
            s.add_identity(who);
        }
        match s.open("secret.pdf", None, file.clone(), None) {
            Err(OpenError::NeedsCertificate(why)) => why,
            other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
        }
    };
    let none = refused(None);
    assert!(none.contains("no digital ID"), "{none}");
    assert!(none.contains("serial") && none.contains("Test Signer RSA"), "the recipients are named: {none}");
    let other = refused(Some(bob()));
    assert!(other.contains("none of your digital IDs") && other.contains("serial"), "{other}");
}

#[test]
fn an_envelope_for_us_that_cannot_be_opened_says_so() {
    // The key transport OID changed from RSA to RSA-OAEP (1.2.840.113549.1.1.7): addressed to us,
    // but not something this reader opens.
    let alice = identity();
    let seed = [7u8; 20];
    let content = [&seed[..], &pdfcraft_cos::permission_bytes(-1)].concat();
    let mut envelope = pdfcraft_engine::sign::envelope::envelope(&alice.certificate, &content, Algorithm::Aes256).unwrap();
    let rsa = [0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01];
    let at = envelope.windows(rsa.len()).position(|w| w == rsa).expect("the key transport algorithm");
    envelope[at + rsa.len() - 1] = 0x07;
    let mut cos = pdfcraft_cos::Document::open(Arc::new(one_page(&[]))).unwrap();
    cos.set_public_key_encryption(&pdfcraft_cos::PublicKeyEncryption {
        algorithm: Algorithm::Aes256,
        recipients: vec![envelope],
        seed,
        permissions: -1,
        encrypt_metadata: true,
    })
    .unwrap();
    let file = Arc::new(pdfcraft_cos::write_full(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap());
    let mut s = Session::new();
    s.add_identity(alice);
    match s.open("secret.pdf", None, file, None) {
        Err(OpenError::NeedsCertificate(why)) => {
            assert!(why.contains("can't be opened") && why.contains("key encryption"), "{why}");
            assert!(!why.contains("none of your digital IDs"), "{why}");
        }
        other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn a_restricted_certificate_file_does_not_point_to_an_owner_password() {
    let file = protect(&[(&identity(), -1), (&bob(), LOW_FILL_SIGN)], Algorithm::Aes256);
    let (mut s, id) = open_as(&file, bob());
    let error = s.apply(id, Edit::SetInfo { key: "Title".into(), value: "T".into() }).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("don't allow") && !text.contains("owner password"), "{text}");
}
