//! The cryptographic core against files made by OpenSSL 3 (tests/data/README.md).

use printcraft_sign::der::Time;
use printcraft_sign::keys::DigestAlg;
use printcraft_sign::{Certificate, Name, PublicKey, SignError, cms, pkcs12};

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
    for key in [printcraft_sign::PrivateKey::generate_rsa(2048).unwrap(), printcraft_sign::PrivateKey::generate_p256().unwrap()] {
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
    let certs = printcraft_sign::x509::load_certificates(&pem_text).unwrap();
    assert_eq!(certs.len(), 1);
    let der = certs[0].raw.clone();
    assert_eq!(printcraft_sign::x509::load_certificates(&der).unwrap()[0], certs[0]);
    let back = printcraft_sign::x509::to_pem(&certs[0]);
    assert_eq!(printcraft_sign::x509::load_certificates(back.as_bytes()).unwrap()[0], certs[0]);
    assert!(printcraft_sign::x509::load_certificates(b"hello").is_err());
}

// ── CRL and OCSP verification (RFC 5280, RFC 6960) ─────────────────────────────────────────

use printcraft_sign::der::{self, tag};
use printcraft_sign::revocation::{CertificateList, OcspResponse, RevocationStatus};

/// A self-issued CRL from `issuer`: optional revocation entry, signed with the issuer's key.
fn crl(issuer: &pkcs12::DigitalId, revoked_serial: Option<(&[u8], Time)>, this: Time, next: Time) -> Vec<u8> {
    let alg = issuer.key.signature_algorithm(DigestAlg::Sha256);
    let mut tbs = vec![der::int(1), alg.clone(), issuer.certificate.subject.raw.clone(), this.encode(), next.encode()];
    if let Some((serial, at)) = revoked_serial {
        tbs.push(der::seq(&[&der::seq(&[&der::uint(serial), &at.encode()])]));
    }
    let refs: Vec<&[u8]> = tbs.iter().map(Vec::as_slice).collect();
    let tbs = der::seq(&refs);
    let sig = issuer.key.sign(DigestAlg::Sha256, &tbs).unwrap();
    der::seq(&[&tbs, &alg, &der::bit_string(&sig)])
}

/// A self-issued OCSP response from `issuer` answering for its own serial.
fn ocsp(issuer: &pkcs12::DigitalId, status: RevocationStatus, this: Time, next: Time) -> Vec<u8> {
    let cert_id = der::seq(&[
        &DigestAlg::Sha1.algorithm(),
        &der::octets(&DigestAlg::Sha1.digest(&[&issuer.certificate.issuer.raw])),
        &der::octets(&DigestAlg::Sha1.digest(&[&issuer.certificate.public_key.key_bits()])),
        &der::uint(&issuer.certificate.serial),
    ]);
    let status = match status {
        RevocationStatus::Good => der::tlv(tag::ctx(0), &[]),
        RevocationStatus::Revoked { at } => der::tlv(tag::ctx(1), &der::seq(&[&at.encode()])),
        RevocationStatus::Unknown => der::tlv(tag::ctx(2), &[]),
    };
    let single = der::seq(&[&cert_id, &status, &this.encode(), &next.encode()]);
    let responder_id = der::tlv(tag::ctx(1), &issuer.certificate.subject.raw);
    let tbs = der::seq(&[&responder_id, &this.encode(), &der::seq(&[&single])]);
    let alg = issuer.key.signature_algorithm(DigestAlg::Sha256);
    let sig = issuer.key.sign(DigestAlg::Sha256, &tbs).unwrap();
    let basic = der::seq(&[&tbs, &alg, &der::bit_string(&sig)]);
    let basic_type = der::oid("1.3.6.1.5.5.7.48.1.1");
    der::seq(&[&der::int(0), &der::explicit(0, &der::seq(&[&basic_type, &der::octets(&basic)]))])
}

fn mid_june() -> Time {
    Time { year: 2026, month: 6, day: 15, hour: 12, minute: 0, second: 0 }
}

#[test]
fn crls_verify_and_report_revocation() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let (this, next) = (mid_june(), Time { year: 2026, month: 7, day: 15, hour: 12, minute: 0, second: 0 });
    let issuer = &id.certificate;
    let at = mid_june();
    // Not on the list: good.
    let clean = CertificateList::parse(&crl(&id, None, this, next)).unwrap();
    assert_eq!(clean.check(issuer, issuer, at), RevocationStatus::Good);
    // On the list: revoked with its date.
    let revoked =
        CertificateList::parse(&crl(&id, Some((&issuer.serial, Time { year: 2026, month: 6, day: 1, hour: 8, minute: 0, second: 0 })), this, next))
            .unwrap();
    assert_eq!(
        revoked.check(issuer, issuer, at),
        RevocationStatus::Revoked { at: Time { year: 2026, month: 6, day: 1, hour: 8, minute: 0, second: 0 } }
    );
    // Outside the validity window: unknown.
    let later = Time { year: 2026, month: 8, day: 1, hour: 0, minute: 0, second: 0 };
    assert_eq!(clean.check(issuer, issuer, later), RevocationStatus::Unknown);
    // A tampered CRL does not verify: unknown, never a false "good".
    let mut tampered = crl(&id, None, this, next);
    let t = tampered.windows(4).position(|w| w == &[0x6F, 0x6B, 0x00, 0x00][..]).unwrap_or(20);
    tampered[t] ^= 0xFF;
    assert_ne!(CertificateList::parse(&tampered).unwrap().check(issuer, issuer, at), RevocationStatus::Good);
    // A different issuer's key does not verify it.
    let other = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    assert_eq!(
        CertificateList::parse(&crl(&id, None, this, next)).unwrap().check(&other.certificate, &other.certificate, at),
        RevocationStatus::Unknown
    );
}

#[test]
fn ocsp_responses_verify_and_match_the_certificate() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let (this, next) = (mid_june(), Time { year: 2026, month: 7, day: 15, hour: 12, minute: 0, second: 0 });
    let issuer = &id.certificate;
    let at = mid_june();
    let good = OcspResponse::parse(&ocsp(&id, RevocationStatus::Good, this, next)).unwrap();
    assert_eq!(good.check(issuer, issuer, at), RevocationStatus::Good);
    let revoked = OcspResponse::parse(&ocsp(
        &id,
        RevocationStatus::Revoked { at: Time { year: 2026, month: 6, day: 2, hour: 9, minute: 0, second: 0 } },
        this,
        next,
    ))
    .unwrap();
    assert_eq!(
        revoked.check(issuer, issuer, at),
        RevocationStatus::Revoked { at: Time { year: 2026, month: 6, day: 2, hour: 9, minute: 0, second: 0 } }
    );
    // Outside the window: unknown.
    let later = Time { year: 2026, month: 8, day: 1, hour: 0, minute: 0, second: 0 };
    assert_eq!(good.check(issuer, issuer, later), RevocationStatus::Unknown);
    // Signed by a different key than the named responder: unknown.
    let other = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let wrong_key = OcspResponse::parse(&ocsp(&other, RevocationStatus::Good, this, next)).unwrap();
    assert_eq!(wrong_key.check(issuer, issuer, at), RevocationStatus::Unknown);
    // An error response is rejected at parse time.
    let error = der::seq(&[&der::int(6)]);
    assert!(OcspResponse::parse(&error).is_err());
}
