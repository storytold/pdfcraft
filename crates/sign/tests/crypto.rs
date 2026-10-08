//! The cryptographic core against files made by OpenSSL 3 (tests/data/README.md).

use pdfcraft_sign::der::Time;
use pdfcraft_sign::keys::DigestAlg;
use pdfcraft_sign::{Certificate, Name, PublicKey, SignError, cms, pkcs12};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn pem(name: &str) -> Vec<u8> {
    let text = String::from_utf8(data(name)).unwrap();
    let b64: String = text.lines().filter(|l| !l.starts_with("-----")).collect();
    decode_base64(&b64)
}

fn decode_base64(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        _ => 63,
    };
    let bytes: Vec<u8> = s.bytes().filter(|c| *c != b'=').map(val).collect();
    bytes
        .chunks(4)
        .flat_map(|c| {
            let n = c.iter().enumerate().fold(0u32, |acc, (i, v)| acc | (*v as u32) << (18 - 6 * i));
            let k = c.len() * 6 / 8;
            (0..k).map(move |i| (n >> (16 - 8 * i)) as u8)
        })
        .collect()
}

#[test]
fn opens_every_openssl_flavour_of_pkcs12() {
    for (file, cn, key) in [
        ("rsa-aes.p12", "Test Signer RSA", "RSA 2048-bit"),
        ("rsa-legacy.p12", "Test Signer RSA", "RSA 2048-bit"),
        ("ec-p256.p12", "Test Signer EC", "ECDSA P-256"),
        ("ec-p384.p12", "Test Signer P384", "ECDSA P-384"),
        ("chain.p12", "Ada Lovelace", "RSA 2048-bit"),
    ] {
        let id = pkcs12::open(&data(file), "test").unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(id.certificate.subject.common_name(), Some(cn), "{file}");
        assert_eq!(id.key.public_key().describe(), key, "{file}");
        assert_eq!(&id.certificate.public_key, id.key.public_key());
        assert!(matches!(pkcs12::open(&data(file), "nope"), Err(SignError::WrongPassword)), "{file}");
    }
    assert_eq!(pkcs12::open(&data("rsa-aes.p12"), "test").unwrap().friendly_name.as_deref(), Some("Test Signer RSA"));
    let chain = pkcs12::open(&data("chain.p12"), "test").unwrap();
    assert_eq!(chain.chain.len(), 1);
    assert_eq!(chain.certificate.subject.email(), Some("ada@example.com"));
    assert!(chain.chain[0].is_ca && chain.chain[0].is_self_signed());
    assert!(chain.certificate.signed_by(&chain.chain[0].public_key), "the leaf is issued by the root");
    assert!(!chain.certificate.is_self_signed());
    assert_eq!(chain.certificate.key_usage.map(|u| u & 0b11), Some(0b11), "digitalSignature + nonRepudiation");
}

#[test]
fn certificates_parse_as_openssl_made_them() {
    let c = Certificate::parse(&pem("rsa.crt.pem")).unwrap();
    // The fixtures were made when the app was called PrintCraft.
    assert_eq!(c.subject.display(), "CN=Test Signer RSA, O=PrintCraft Tests, C=US");
    assert_eq!(c.serial_hex(), "03E9");
    assert!(c.is_self_signed());
    assert!(c.not_after.year >= c.not_before.year + 9);
    let ca = Certificate::parse(&pem("ca.crt.pem")).unwrap();
    assert!(ca.is_ca && ca.is_self_signed());
    assert!(matches!(ca.public_key, PublicKey::P256(_)));
}

#[test]
fn signatures_round_trip_for_every_key_type() {
    for file in ["rsa-aes.p12", "ec-p256.p12", "ec-p384.p12", "chain.p12"] {
        let id = pkcs12::open(&data(file), "test").unwrap();
        let alg = id.key.preferred_digest();
        let digest = alg.digest(&[b"the document bytes"]);
        let sig = cms::sign_detached(&id.key, &id.certificate, &id.chain, alg, &digest).unwrap();
        let mut padded = sig.clone();
        padded.extend([0u8; 64]);
        let sd = cms::SignedData::parse(&padded).unwrap();
        let signer = sd.signer_certificate().expect("the signer's certificate is embedded");
        assert_eq!(signer, &id.certificate);
        assert_eq!(sd.signer.message_digest.as_deref(), Some(&digest[..]));
        assert!(sd.signer.signing_certificate, "CAdES signing-certificate-v2");
        assert!(sd.verify_signature(signer, &digest), "{file}");
        assert_eq!(sd.certificates.len(), 1 + id.chain.len());
        // Tampering with the signature breaks it.
        let mut bad = sd.clone();
        bad.signer.signature[5] ^= 1;
        assert!(!bad.verify_signature(signer, &digest), "{file}");
    }
}

#[test]
fn new_digital_ids_are_self_signed_and_survive_a_p12_round_trip() {
    let name = Name::build("Grace Hopper", "Compilers", "Navy", "grace@example.com", "us");
    let now = Time { year: 2026, month: 10, day: 2, hour: 12, minute: 0, second: 0 };
    for key in [pdfcraft_sign::PrivateKey::generate_rsa(2048).unwrap(), pdfcraft_sign::PrivateKey::generate_p256().unwrap()] {
        let cert = Certificate::self_signed(&name, &key, now, 5, &[0x42, 0x01]).unwrap();
        assert!(cert.is_self_signed());
        assert_eq!(cert.subject.display(), "C=US, O=Navy, OU=Compilers, CN=Grace Hopper, E=grace@example.com");
        assert_eq!(cert.not_after.year, 2031);
        assert_eq!(cert.key_usage.map(|u| u & 0b11), Some(0b11));
        let id = pkcs12::DigitalId { key, certificate: cert, chain: Vec::new(), friendly_name: Some("Grace Hopper".into()) };
        let p12 = pkcs12::write(&id, "s3cret").unwrap();
        let back = pkcs12::open(&p12, "s3cret").unwrap();
        assert_eq!(back.certificate, id.certificate);
        assert_eq!(back.friendly_name.as_deref(), Some("Grace Hopper"));
        assert!(matches!(pkcs12::open(&p12, "wrong"), Err(SignError::WrongPassword)));
        let d = DigestAlg::Sha256.digest(&[b"x"]);
        let sig = cms::sign_detached(&back.key, &back.certificate, &[], DigestAlg::Sha256, &d).unwrap();
        let sd = cms::SignedData::parse(&sig).unwrap();
        assert!(sd.verify_signature(&back.certificate, &d));
    }
}

#[test]
fn certificates_load_from_pem_and_der_and_export_as_pem() {
    let pem_text = data("rsa.crt.pem");
    let certs = pdfcraft_sign::x509::load_certificates(&pem_text).unwrap();
    assert_eq!(certs.len(), 1);
    let der = certs[0].raw.clone();
    assert_eq!(pdfcraft_sign::x509::load_certificates(&der).unwrap()[0], certs[0]);
    let back = pdfcraft_sign::x509::to_pem(&certs[0]);
    assert_eq!(pdfcraft_sign::x509::load_certificates(back.as_bytes()).unwrap()[0], certs[0]);
    assert!(pdfcraft_sign::x509::load_certificates(b"hello").is_err());
}
