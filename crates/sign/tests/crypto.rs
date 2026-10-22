//! The cryptographic core against files made by OpenSSL 3 (tests/data/README.md).

use pdfcraft_sign::der::{Time, Tlv};
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

/// One element with BER's indefinite length: contents, then the end-of-contents octets.
fn indefinite(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![tag, 0x80];
    v.extend_from_slice(body);
    v.extend([0, 0]);
    v
}

/// An OCTET STRING split into segments, BER's constructed form.
fn segmented(bytes: &[u8]) -> Vec<u8> {
    let parts: Vec<u8> = bytes.chunks(64).flat_map(der::octets).collect();
    indefinite(0x24, &parts)
}

/// A `.p12` in the shape Windows writes one: the PFX, its ContentInfo and the `[0]` holding the
/// AuthenticatedSafe all in the indefinite form, and the AuthenticatedSafe split into segments.
/// Nothing changes meaning, and the MAC still covers the same octets, so whatever a reader makes
/// of it, it is making it of the form and not of the contents.
///
/// With `deep`, every ContentInfo inside is rewritten the same way, as a real Windows file has it.
/// That re-encodes the AuthenticatedSafe, which is what the MAC covers, so the MacData is dropped
/// — RFC 7292 §4 makes it optional — and a wrong password is then caught by the decryption.
fn as_windows_writes_it(der: &[u8], deep: bool) -> Vec<u8> {
    let pfx = Tlv::parse_all(der).unwrap().children().unwrap();
    let auth = pfx[1].children().unwrap();
    let mut content = auth[1].inner().unwrap().value.to_vec();
    if deep {
        let mut inner = Vec::new();
        for ci in Tlv::parse_all(&content).unwrap().children().unwrap() {
            let p = ci.children().unwrap();
            let body = p[1].inner().unwrap();
            // A Data content is an OCTET STRING, so it gets segmented; an EncryptedData is a
            // SEQUENCE and stays as it is inside its indefinite wrappers.
            let wrapped = if body.tag == 0x04 { segmented(body.value) } else { body.raw.to_vec() };
            let zero = indefinite(0xA0, &wrapped);
            inner.extend(indefinite(0x30, &[p[0].raw, zero.as_slice()].concat()));
        }
        content = indefinite(0x30, &inner);
    }
    let zero = indefinite(0xA0, &segmented(&content));
    let mut body = pfx[0].raw.to_vec();
    body.extend(indefinite(0x30, &[auth[0].raw, zero.as_slice()].concat()));
    if !deep {
        for extra in &pfx[2..] {
            body.extend_from_slice(extra.raw);
        }
    }
    indefinite(0x30, &body)
}

#[test]
fn opens_pkcs12_as_windows_writes_it() {
    for file in ["rsa-aes.p12", "rsa-legacy.p12", "chain.p12"] {
        let der = data(file);
        let want = pkcs12::open(&der, "test").unwrap();
        for deep in [false, true] {
            let ber = as_windows_writes_it(&der, deep);
            assert_ne!(ber, der, "{file} ({deep})");
            let got = pkcs12::open(&ber, "test").unwrap_or_else(|e| panic!("{file} in BER (deep: {deep}): {e}"));
            assert_eq!(got.certificate.raw, want.certificate.raw, "{file} ({deep})");
            assert_eq!(got.key.public_key(), want.key.public_key(), "{file} ({deep})");
            assert_eq!(got.chain.len(), want.chain.len(), "{file} ({deep})");
            assert_eq!(got.friendly_name, want.friendly_name, "{file} ({deep})");
            assert!(matches!(pkcs12::open(&ber, "nope"), Err(SignError::WrongPassword)), "{file} ({deep})");
        }
    }
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
        assert!(sd.verify_signature(signer, &digest).unwrap(), "{file}");
        assert_eq!(sd.certificates.len(), 1 + id.chain.len());
        // Tampering with the signature breaks it.
        let mut bad = sd.clone();
        bad.signer.signature[5] ^= 1;
        assert!(!bad.verify_signature(signer, &digest).unwrap(), "{file}");
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
        assert!(sd.verify_signature(&back.certificate, &d).unwrap());
    }
}

#[test]
fn new_digital_ids_use_the_stronger_round_count_for_the_key_and_the_mac() {
    let name = Name::build("Grace Hopper", "", "", "", "");
    let now = Time { year: 2026, month: 10, day: 14, hour: 12, minute: 0, second: 0 };
    let key = pdfcraft_sign::PrivateKey::generate_p256().unwrap();
    let certificate = Certificate::self_signed(&name, &key, now, 5, &[0x42]).unwrap();
    let id = pkcs12::DigitalId { key, certificate, chain: Vec::new(), friendly_name: None };
    let p12 = pkcs12::write(&id, "s3cret12").unwrap();
    // OWASP's figure for PBKDF2-HMAC-SHA-256, spelled out so lowering the constant fails here.
    const ROUNDS: u64 = 600_000;
    // MacData's iteration count, the last field of the file.
    let pfx = Tlv::parse_all(&p12).unwrap().children().unwrap();
    let mac = pfx[2].children().unwrap();
    assert_eq!(mac[2].u64().unwrap(), ROUNDS);
    // The same INTEGER three times: the MAC and the PBKDF2 parameters of both encrypted parts.
    let rounds = pdfcraft_sign::der::int(ROUNDS);
    assert_eq!(p12.windows(rounds.len()).filter(|w| *w == rounds.as_slice()).count(), 3);
    assert_eq!(pkcs12::open(&p12, "s3cret12").unwrap().certificate, id.certificate);
}

/// A `.p12` whose MAC asks for `rounds` iterations. Its content is empty: opening it must stop
/// at the MAC, before the content is read.
fn p12_with_mac_rounds(rounds: u64) -> Vec<u8> {
    use pdfcraft_sign::der;
    let auth_safe = der::seq(&[]);
    let digest = der::seq(&[&DigestAlg::Sha256.algorithm(), &der::octets(&[0; 32])]);
    let mac_data = der::seq(&[&digest, &der::octets(&[1; 16]), &der::int(rounds)]);
    der::seq(&[&der::int(3), &der::seq(&[&der::oid("1.2.840.113549.1.7.1"), &der::explicit(0, &der::octets(&auth_safe))]), &mac_data])
}

#[test]
fn a_p12_asking_for_too_many_rounds_is_refused_before_hashing() {
    // 2^32 + 1 used to wrap to one round; 20 million took seconds and 4 billion about an hour,
    // all spent on the password check (#1847).
    for rounds in [(1u64 << 32) + 1, 20_000_000, 4_000_000_000] {
        let err = pkcs12::open(&p12_with_mac_rounds(rounds), "password").expect_err("refused");
        assert!(matches!(err, SignError::Unsupported(_)), "{rounds}: {err}");
    }
    // An ordinary count still gets as far as the password.
    assert!(matches!(pkcs12::open(&p12_with_mac_rounds(2048), "password"), Err(SignError::WrongPassword)));
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

/// Base64 in 64-column lines, for PEM armour.
fn encode_base64(der: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in der.chunks(3) {
        let n = u32::from(c[0]) << 16 | u32::from(*c.get(1).unwrap_or(&0)) << 8 | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            s.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    let lines: Vec<&str> = s.as_bytes().chunks(64).map(|l| std::str::from_utf8(l).unwrap()).collect();
    lines.join("\n")
}

/// A PKCS #7 certificate bundle (degenerate SignedData, RFC 5652 §5.2) holding `certs`, laid out
/// as `openssl crl2pkcs7 -nocrl` writes it.
fn p7b(certs: &[&[u8]]) -> Vec<u8> {
    use pdfcraft_sign::der::{explicit, int, oid, seq, set_of, tlv};
    let encap = seq(&[&oid(cms::DATA)]);
    let mut sd = vec![int(1), set_of(&[]), encap];
    if !certs.is_empty() {
        sd.push(tlv(0xA0, &certs.concat()));
    }
    sd.push(set_of(&[]));
    let refs: Vec<&[u8]> = sd.iter().map(Vec::as_slice).collect();
    seq(&[&oid(cms::SIGNED_DATA), &explicit(0, &seq(&refs))])
}

#[test]
fn certificates_load_from_pkcs7_bundles() {
    use pdfcraft_sign::x509::{load_certificates, to_pem};
    let leaf = load_certificates(&data("rsa.crt.pem")).unwrap().remove(0);
    let root = load_certificates(&data("ca.crt.pem")).unwrap().remove(0);
    let bundle = p7b(&[&leaf.raw, &root.raw]);
    // .p7b / .p7c in DER, the way Windows and Acrobat export a chain.
    assert_eq!(load_certificates(&bundle).unwrap(), vec![leaf.clone(), root.clone()]);
    // The same bundle as PEM `PKCS7` and `CMS` blocks, after a key and a plain certificate.
    let b64 = encode_base64(&bundle);
    for label in ["PKCS7", "CMS"] {
        let text = format!(
            "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n{}-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            to_pem(&leaf),
            b64
        );
        assert_eq!(load_certificates(text.as_bytes()).unwrap(), vec![leaf.clone(), leaf.clone(), root.clone()], "{label}");
    }
    // An END that closes a different block ends nothing.
    let mismatched = format!("-----BEGIN PKCS7-----\n{b64}\n-----END CERTIFICATE-----\n");
    assert!(load_certificates(mismatched.as_bytes()).is_err());
    // A block that never ends is a damaged file, not a shorter list.
    let unclosed = format!("{}-----BEGIN PKCS7-----\n{b64}\n", to_pem(&leaf));
    assert!(load_certificates(unclosed.as_bytes()).is_err());
    // A bundle with no certificate, and a SignedData that isn't one, are errors, not empty lists.
    assert!(load_certificates(&p7b(&[])).is_err());
    let not_signed_data = pdfcraft_sign::der::seq(&[&pdfcraft_sign::der::oid(cms::DATA), &pdfcraft_sign::der::explicit(0, &[])]);
    assert!(load_certificates(&not_signed_data).is_err());
    // A SignedData out of shape: no signerInfos SET, or a version that isn't an INTEGER.
    {
        use pdfcraft_sign::der::{explicit, int, oid, seq, set_of, tlv};
        let wrap = |sd: &[&[u8]]| seq(&[&oid(cms::SIGNED_DATA), &explicit(0, &seq(sd))]);
        let (certs, encap) = (tlv(0xA0, &[leaf.raw.as_slice(), &root.raw].concat()), seq(&[&oid(cms::DATA)]));
        assert_eq!(load_certificates(&wrap(&[&int(1), &set_of(&[]), &encap, &certs, &set_of(&[])])).unwrap().len(), 2);
        assert!(load_certificates(&wrap(&[&int(1), &set_of(&[]), &encap, &certs])).is_err());
        assert!(load_certificates(&wrap(&[&set_of(&[]), &set_of(&[]), &encap, &certs, &set_of(&[])])).is_err());
        // An empty encapContentInfo, a repeated [0], and [1] CRLs before [0] certificates.
        let crls = tlv(0xA1, &[]);
        assert!(load_certificates(&wrap(&[&int(1), &set_of(&[]), &seq(&[]), &certs, &set_of(&[])])).is_err());
        assert!(load_certificates(&wrap(&[&int(1), &set_of(&[]), &encap, &certs, &certs, &set_of(&[])])).is_err());
        assert!(load_certificates(&wrap(&[&int(1), &set_of(&[]), &encap, &crls, &certs, &set_of(&[])])).is_err());
        assert_eq!(load_certificates(&wrap(&[&int(1), &set_of(&[]), &encap, &certs, &crls, &set_of(&[])])).unwrap().len(), 2);
    }
    let mut trailing = bundle.clone();
    trailing.extend_from_slice(&[0x05, 0x00]);
    assert!(load_certificates(&trailing).is_err());
    assert!(cms::certificate_bundle(&bundle).is_ok());
    // Truncated anywhere: an error, never a panic.
    for n in 0..bundle.len() {
        let _ = load_certificates(&bundle[..n]);
    }
}

// ── CRL and OCSP verification (RFC 5280, RFC 6960) ─────────────────────────────────────────

use pdfcraft_sign::der::{self, tag};
use pdfcraft_sign::revocation::{CertificateList, OcspResponse, RevocationStatus};

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
    // RFC 6960's real encodings: good [0] IMPLICIT NULL, revoked [1] IMPLICIT RevokedInfo,
    // unknown [2] IMPLICIT; nextUpdate [0] EXPLICIT GeneralizedTime.
    let status = match status {
        RevocationStatus::Good => der::tlv(tag::ctx_prim(0), &[]),
        RevocationStatus::Revoked { at } => der::tlv(tag::ctx(1), &at.encode()),
        RevocationStatus::Unknown => der::tlv(tag::ctx_prim(2), &[]),
    };
    let single = der::seq(&[&cert_id, &status, &this.encode(), &der::tlv(tag::ctx(0), &next.encode())]);
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

/// Issue #159: `.p12` files that aren't clean DER still open — trailing
/// whitespace, PEM armour or a bare base64 body — and real damage still fails,
/// with the reason in the message.
#[test]
fn opens_wrapped_pkcs12_files_and_still_rejects_broken_ones() {
    use base64::Engine as _;
    let der = data("rsa-aes.p12");
    let signer = pkcs12::open(&der, "test").unwrap();

    // An editor or a download appends whitespace.
    let mut trailing = der.clone();
    trailing.extend_from_slice(b"\r\n \n");
    assert_eq!(pkcs12::open(&trailing, "test").unwrap().certificate, signer.certificate);

    // PEM armour, the way OpenSSL and government portals present them.
    let b64 = base64::engine::general_purpose::STANDARD.encode(&der);
    let mut pem = String::from("-----BEGIN PKCS12-----\n");
    for line in b64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).unwrap());
        pem.push('\n');
    }
    pem.push_str("-----END PKCS12-----\n");
    assert_eq!(pkcs12::open(pem.as_bytes(), "test").unwrap().certificate, signer.certificate);

    // The base64 body alone, armour stripped.
    assert_eq!(pkcs12::open(b64.as_bytes(), "test").unwrap().certificate, signer.certificate);

    // A truncated file still fails, and says why.
    let err = pkcs12::open(&der[..64], "test").expect_err("truncated file");
    assert!(err.to_string().contains("runs past the end"), "{err}");
    let err = pkcs12::open(b"", "test").expect_err("empty file");
    assert!(err.to_string().contains("truncated DER"), "{err}");
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

// ── tolerance for the encodings found in old and unusual signers ───────────────────────────────
// RSA signatures over SHA-256("hello") by the key in rsa-aes.p12, made with OpenSSL 3 (see
// tests/data/README.md): the standard DigestInfo, DigestInfo without the NULL parameter, the bare
// digest (which is refused: it does not say which hash algorithm it uses), and RSASSA-PSS with a
// zero-length salt.
const RSA_STD: &str = "40896f76bc460e4ae9dc523dfe136159c7ccc20eff09bfeeafd570fd41a6c016d6b2a1d771ba81642dd5bf94d2bb9c4aeead237ce0e1662678efd5ff7b8590bf7d5042f6541b001a9f7d688e60a9fde5c9b3f295bd5373aea1059b53f1341f30a4db68302161e95c6a62847205782edcd1fa99b6720eaa60b59d3f54d4ec7be396c8f1146fe35f198bd12dca608b8080a446f89ba300fd74f8df4e7f177c59dc48cf878451423dd9674df768a12f85a1b5e81a5b8df43ac0fb03c4393cfed7dceeeddf1c8aef9a37e738ff4169a6881e783c2f841fb974eb0e6ecedf649bf64fbc51920b7824d86b26527acaec5d0ebb44454606441fea2b7d7b7a6014ab9d64";
const RSA_NO_NULL: &str = "a6589a056b04903cd77d5dae3b52f8c9905403ee2cef2e7343cfdd07746cbf1034c8c510a6f7258face9cfbcacb990e30f185dd091310d88efc3457e0fd34245d525e2cb5ceaab029e7b7fd25a581379c92e6abf90a36388cfc6afba0be47c99abc8b32e82a2c750ffe7c6d04de886cbfe63685cfc0f0204f641103e3c752496a0ae341fb503c4c2389824f7f20d07a03f0e3a95d5084fc1fb1b94c6bc607a14623b1bf7180b3d7d4592e217ba08698becb1994a6ef72ccab6e239418f11c57f184f59b0d1c7407216856cd4585da6a07354191ae85f8be02e670a10017f115ccb55d96f2ea2c1f095bbcf6b50ed8763945b78054989b016119c5371d918d682";
const RSA_BARE: &str = "a5812c29d3edf0b567636aaa501a960b977e774a0ea941e41401611d8b63464e99e2e908d8d8452b97112597f06f2adee029109bf6e421eb57add689b518532232199151ed822d1766013ea44ae5cb468b4a2288cf866f8c5954b729bfd34a8ca82d73bb397a75eb76687f7d9e506dab32e2da6231ece307fe8eb929c7648f73de6dd81c38aecb78a0e7ef0e11654fbadef88c7b206079afee70db8054ada079c6456826383dfd2504f05033b58cf1610a180953e501e137b47de5a77914cfaa33b8c588f24dcdb5789e7d1288ff009918f6c2808bfa1c57398d2e007a5e6d3004a3ca568a4ae2eb5cfa14083099e81cfcb53ead9ec6650a7c58e441d117d439";
const RSA_PSS_SALT0: &str = "2cdbb02d75a80df07dcca229363644caa4391cca30a3bc1281a3f4512dbe609727767c42e62cdad7d6bd38088b2c0b5dcb439fd2c337c31b47bbc39bda0e5c1486a773cde163fa2cad8047c06217541c993b21413e5cd341a4f66d2fb6e4755b52d25935dd90af667ce3d3e1825c8e6e40274dca7cf469ae17cbc3c1c1560d87a877a46353245128120beb1108cd0cafddae5b4ef9005601cc05f114da9de4cdaee08020d5b9be27e7b48f961e40664ef592990580bc1e1f2a8107c3d71d323df120720be6e862191a0aaa3b394b96efd2f45afa5f8211e2082865b8d3e3c28f99054976162ae8f42a903bfb82c17700db93fef603c4cd85c70a0be68fda9af8";

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn rsa_signatures_are_read_with_every_digestinfo_variant() {
    use pdfcraft_sign::keys::{DigestAlg, PssParams, Scheme};
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let key = &id.certificate.public_key;
    let digest = DigestAlg::Sha256.digest(&[b"hello"]);
    for (name, sig) in [("standard", RSA_STD), ("no NULL", RSA_NO_NULL)] {
        assert!(key.verify(Scheme::RsaPkcs1, DigestAlg::Sha256, &digest, &unhex(sig)).unwrap(), "{name}");
    }
    // The bare digest, with no DigestInfo, binds no hash algorithm: not a signature we accept.
    assert!(!key.verify(Scheme::RsaPkcs1, DigestAlg::Sha256, &digest, &unhex(RSA_BARE)).unwrap(), "bare digest");
    // PSS with the salt length it declares, or any when none is declared.
    let pss = |salt_len| Scheme::RsaPss(PssParams { mgf: DigestAlg::Sha256, salt_len });
    assert!(key.verify(pss(Some(0)), DigestAlg::Sha256, &digest, &unhex(RSA_PSS_SALT0)).unwrap());
    assert!(key.verify(pss(None), DigestAlg::Sha256, &digest, &unhex(RSA_PSS_SALT0)).unwrap(), "salt length recovered");
    assert!(!key.verify(pss(Some(32)), DigestAlg::Sha256, &digest, &unhex(RSA_PSS_SALT0)).unwrap(), "declared salt length is enforced");
    // The check itself is not relaxed: another digest, a flipped bit, or the wrong scheme fail.
    let other = DigestAlg::Sha256.digest(&[b"hellp"]);
    let mut flipped = unhex(RSA_STD);
    flipped[7] ^= 1;
    for sig in [RSA_STD, RSA_NO_NULL, RSA_BARE] {
        assert!(!key.verify(Scheme::RsaPkcs1, DigestAlg::Sha256, &other, &unhex(sig)).unwrap());
    }
    assert!(!key.verify(Scheme::RsaPkcs1, DigestAlg::Sha256, &digest, &flipped).unwrap());
    assert!(!key.verify(pss(None), DigestAlg::Sha256, &digest, &unhex(RSA_STD)).unwrap());
}

#[test]
fn ecdsa_signatures_are_read_as_der_non_minimal_der_or_raw() {
    use pdfcraft_sign::der::{self, Tlv};
    use pdfcraft_sign::keys::{DigestAlg, Scheme};
    for (file, field) in [("ec-p256.p12", 32usize), ("ec-p384.p12", 48)] {
        let id = pkcs12::open(&data(file), "test").unwrap();
        let alg = id.key.preferred_digest();
        let msg = b"signed content";
        let digest = alg.digest(&[msg]);
        let sig = id.key.sign(alg, msg).unwrap();
        let pk = id.key.public_key();
        assert!(pk.verify(Scheme::Ecdsa, alg, &digest, &sig).unwrap(), "{file}: DER");
        let ints = Tlv::parse_all(&sig).unwrap().children().unwrap();
        let fixed = |i: &Tlv<'_>| {
            let m = i.uint_bytes();
            let mut v = vec![0u8; field - m.len()];
            v.extend_from_slice(m);
            v
        };
        let (r, s) = (fixed(&ints[0]), fixed(&ints[1]));
        // Raw r ‖ s (PKCS #11 tokens).
        let raw = [r.clone(), s.clone()].concat();
        assert!(pk.verify(Scheme::Ecdsa, alg, &digest, &raw).unwrap(), "{file}: raw");
        // Integers padded with redundant leading zeros.
        let padded = |v: &[u8]| der::tlv(0x02, &[&[0u8, 0][..], v].concat());
        let loose = der::seq(&[&padded(&r), &padded(&s)]);
        assert!(pk.verify(Scheme::Ecdsa, alg, &digest, &loose).unwrap(), "{file}: non-minimal DER");
        // Still a real check.
        let mut bad = raw.clone();
        bad[3] ^= 0x40;
        assert!(!pk.verify(Scheme::Ecdsa, alg, &digest, &bad).unwrap());
        assert!(!pk.verify(Scheme::Ecdsa, alg, &DigestAlg::Sha256.digest(&[b"other"]), &raw).unwrap());
        assert!(!pk.verify(Scheme::Ecdsa, alg, &digest, &[1, 2, 3]).unwrap());
    }
}

#[test]
fn tolerated_encodings_are_noted_and_standard_ones_are_not() {
    use pdfcraft_sign::der::{self, Tlv};
    use pdfcraft_sign::keys::{DigestAlg, Scheme};
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let key = &id.certificate.public_key;
    let digest = DigestAlg::Sha256.digest(&[b"hello"]);
    let rsa_notes = |sig: &[u8]| {
        let mut notes = Vec::new();
        assert!(key.verify_noting(Scheme::RsaPkcs1, DigestAlg::Sha256, &digest, sig, &mut notes).unwrap());
        notes
    };
    assert!(rsa_notes(&unhex(RSA_STD)).is_empty(), "a standard signature needs no excuse");
    let no_null = rsa_notes(&unhex(RSA_NO_NULL));
    assert!(matches!(no_null.as_slice(), [n] if n.contains("no NULL")), "{no_null:?}");
    // A zero byte in front of the signature value: still the same signature, but not modulus-length.
    let padded: Vec<u8> = std::iter::once(0).chain(unhex(RSA_STD)).collect();
    let long = rsa_notes(&padded);
    assert!(matches!(long.as_slice(), [n] if n.contains("modulus")), "{long:?}");

    for (file, field) in [("ec-p256.p12", 32usize), ("ec-p384.p12", 48)] {
        let id = pkcs12::open(&data(file), "test").unwrap();
        let alg = id.key.preferred_digest();
        let digest = alg.digest(&[b"signed content"]);
        let sig = id.key.sign(alg, b"signed content").unwrap();
        let pk = id.key.public_key();
        let ec_notes = |sig: &[u8]| {
            let mut notes = Vec::new();
            assert!(pk.verify_noting(Scheme::Ecdsa, alg, &digest, sig, &mut notes).unwrap(), "{file}");
            notes
        };
        assert!(ec_notes(&sig).is_empty(), "{file}: DER as made");
        let ints = Tlv::parse_all(&sig).unwrap().children().unwrap();
        let fixed = |i: &Tlv<'_>| {
            let m = i.uint_bytes();
            let mut v = vec![0u8; field - m.len()];
            v.extend_from_slice(m);
            v
        };
        let (r, s) = (fixed(&ints[0]), fixed(&ints[1]));
        let raw = ec_notes(&[r.clone(), s.clone()].concat());
        assert!(matches!(raw.as_slice(), [n] if n.contains("raw")), "{file}: {raw:?}");
        let padded = |v: &[u8]| der::tlv(0x02, &[&[0u8, 0][..], v].concat());
        let loose = ec_notes(&der::seq(&[&padded(&r), &padded(&s)]));
        assert!(matches!(loose.as_slice(), [n] if n.contains("canonical DER")), "{file}: {loose:?}");
    }
}

/// `a + b` for big-endian numbers of equal width, or `None` if the sum does not fit.
fn add_be(a: &[u8], b: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![0u8; a.len()];
    let mut carry = 0u16;
    for i in (0..a.len()).rev() {
        let sum = u16::from(a[i]) + u16::from(b[i]) + carry;
        out[i] = sum as u8;
        carry = sum >> 8;
    }
    (carry == 0).then_some(out)
}

#[test]
fn weak_digests_say_so_in_words_that_are_pinned() {
    use pdfcraft_sign::keys::DigestAlg;
    // What the signature panel shows for a signature that still validates but should not be
    // relied on. Strong digests get no note at all.
    for alg in DigestAlg::ALL {
        let note = alg.weakness();
        match alg {
            DigestAlg::Sha1 => assert_eq!(
                note,
                Some(
                    "This signature uses SHA-1, a weak hash algorithm: collisions in it have been demonstrated. It still validates because many documents signed years ago use it, but it should not be relied on."
                )
            ),
            DigestAlg::Ripemd160 => assert_eq!(
                note,
                Some(
                    "This signature uses RIPEMD-160, a legacy 160-bit hash algorithm that is no longer recommended for signatures. It still validates, but it should not be relied on."
                )
            ),
            _ => assert_eq!(note, None, "{}", alg.name()),
        }
    }
}

#[test]
fn small_rsa_keys_say_so_and_others_do_not() {
    let rsa = |n: Vec<u8>| PublicKey::Rsa { n, e: vec![1, 0, 1] };
    let note = Some(
        "This signature uses an RSA key shorter than 2048 bits, which is no longer considered strong enough for signatures. It still validates, but it should not be relied on.",
    );
    // 1024 bits, with and without the leading zero byte of a DER INTEGER.
    assert_eq!(rsa(vec![0xC0; 128]).weakness(), note);
    assert_eq!(rsa([vec![0], vec![0xC0; 128]].concat()).weakness(), note);
    // 2047 bits is still short; 2048 bits and more are not.
    assert_eq!(rsa([vec![0x7F], vec![0xFF; 255]].concat()).weakness(), note);
    assert_eq!(rsa([vec![0, 0x80], vec![0; 255]].concat()).weakness(), None);
    assert_eq!(rsa(vec![0xFF; 512]).weakness(), None);
    assert_eq!(rsa(Vec::new()).weakness(), note, "an empty modulus is not a strong key");
    // The key description counts the same bits, so it never says 2048 next to the warning.
    assert_eq!(rsa([vec![0], vec![0xC0; 128]].concat()).describe(), "RSA 1024-bit");
    assert_eq!(rsa([vec![0x7F], vec![0xFF; 255]].concat()).describe(), "RSA 2047-bit");
    assert_eq!(rsa([vec![0, 0x80], vec![0; 255]].concat()).describe(), "RSA 2048-bit");
    assert_eq!(PublicKey::P256(vec![4; 65]).weakness(), None);
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    assert_eq!(id.certificate.public_key.weakness(), None, "the 2048-bit test identity");
}

/// A synthetic 1024-bit key made for this test (`openssl genrsa 1024`, e = 65537) and its
/// PKCS #1 v1.5 SHA-256 signature of `hello`.
const RSA_1024_N: &str = "ae561814194f0a06475c002fc7633b81e83a738ad3f35ba1ded959be6f6b62302028d58707403f44b7bda959e446f13849896711985ac4815e58d7d323aee664771c35bf2dc3db1bcdc49ac61ae7282a24ec7a5af9f6ebe98945e6a71c4998ed31c41bbb997c4873fb4b8271fd3cf76648efcbff76972ab7bcaf831b9e203d47";
const RSA_1024_SIG: &str = "0de5110137e768a6b00201cf8340688c81a901fe309489f2c8186b7dc56e67f7e64188970a9bea926b9bd99b19985adfb0489956ffd5e0264468b678c4fd49a155a895fcb9ab127d0c3742b7a07866b3deec20a5b2f61af4cbfd2901517dc268e4d6edf4e905e0f4e05cdc740c3180971c1ec1470c1476f90ea988e2ab4cbb18";

#[test]
fn a_signature_made_with_a_small_rsa_key_verifies_with_the_note() {
    use pdfcraft_sign::keys::Scheme;
    let key = PublicKey::Rsa { n: unhex(RSA_1024_N), e: vec![1, 0, 1] };
    let weak = key.weakness().expect("a 1024-bit key is short");
    let sig = unhex(RSA_1024_SIG);
    let mut notes = Vec::new();
    let digest = DigestAlg::Sha256.digest(&[b"hello"]);
    assert!(key.verify_noting(Scheme::RsaPkcs1, DigestAlg::Sha256, &digest, &sig, &mut notes).unwrap());
    assert_eq!(notes, [weak]);
    // A signature that doesn't verify says nothing about the key.
    let mut notes = Vec::new();
    let other = DigestAlg::Sha256.digest(&[b"hullo"]);
    assert!(!key.verify_noting(Scheme::RsaPkcs1, DigestAlg::Sha256, &other, &sig, &mut notes).unwrap());
    assert!(!notes.contains(&weak), "{notes:?}");
}

#[test]
fn an_rsa_signature_value_not_below_the_modulus_is_refused() {
    use pdfcraft_sign::keys::{DigestAlg, PssParams, Scheme};
    // RFC 8017 §5.2.2 (RSAVP1): a signature representative s must be below n. Otherwise s and
    // s + n are the same number modulo n and both would verify (malleability, not forgery).
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let key = &id.certificate.public_key;
    let PublicKey::Rsa { n, .. } = key else { panic!("an RSA key") };
    let digest = DigestAlg::Sha256.digest(&[b"hello"]);
    let pss = Scheme::RsaPss(PssParams { mgf: DigestAlg::Sha256, salt_len: Some(0) });
    let sig = unhex(RSA_PSS_SALT0);
    assert!(key.verify(pss, DigestAlg::Sha256, &digest, &sig).unwrap(), "the signature as made");
    let alias = add_be(&sig, n).expect("this signature plus the modulus still has the modulus' length");
    assert_eq!(alias.len(), n.len());
    assert!(!key.verify(pss, DigestAlg::Sha256, &digest, &alias).unwrap(), "s + n is not the signature");
}

#[test]
fn hostile_rsa_signature_values_are_wrong_not_crashes() {
    use pdfcraft_sign::keys::{DigestAlg, PssParams, Scheme};
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let key = &id.certificate.public_key;
    let PublicKey::Rsa { n, .. } = key else { panic!("an RSA key") };
    let digest = DigestAlg::Sha256.digest(&[b"hello"]);
    let pss = Scheme::RsaPss(PssParams { mgf: DigestAlg::Sha256, salt_len: Some(32) });
    let mut one_more = vec![1u8];
    one_more.extend(vec![0u8; n.len()]);
    for sig in [vec![], vec![0], vec![0; n.len()], vec![0xFF; n.len()], vec![0xFF; n.len() + 40], n.clone(), one_more] {
        for scheme in [Scheme::RsaPkcs1, pss] {
            assert!(!key.verify(scheme, DigestAlg::Sha256, &digest, &sig).unwrap(), "{} bytes", sig.len());
        }
    }
    // A digest of the wrong length for PSS is a mismatch, not a crash either.
    assert!(!key.verify(pss, DigestAlg::Sha256, &[1, 2, 3], &unhex(RSA_STD)).unwrap());
}

#[test]
fn keys_of_unknown_algorithms_are_unsupported_not_wrong() {
    use pdfcraft_sign::der::{self, Tlv};
    use pdfcraft_sign::keys::{DigestAlg, Scheme};
    // An Ed448 key (1.3.101.113), which isn't supported: the key parses, checking says "can't".
    let spki = der::seq(&[&der::seq(&[&der::oid("1.3.101.113")]), &der::bit_string(&[7u8; 57])]);
    let key = PublicKey::from_spki(&Tlv::parse_all(&spki).unwrap()).unwrap();
    assert_eq!(key.spki(), spki);
    assert!(key.describe().contains("unsupported"));
    let r = key.verify(Scheme::Ecdsa, DigestAlg::Sha256, &[0; 32], &[0; 64]);
    assert!(matches!(r, Err(SignError::Unsupported(_))), "{r:?}");
    // An unknown curve likewise.
    let spki = der::seq(&[&der::seq(&[&der::oid("1.2.840.10045.2.1"), &der::oid("1.3.132.0.10")]), &der::bit_string(&[4u8; 65])]);
    let key = PublicKey::from_spki(&Tlv::parse_all(&spki).unwrap()).unwrap();
    assert!(matches!(key.verify(Scheme::Ecdsa, DigestAlg::Sha256, &[0; 32], &[0; 64]), Err(SignError::Unsupported(_))));
}

// ECDSA signatures by OpenSSL 3 over a digest of "hello": (SubjectPublicKeyInfo, digest, DER signature).
const P521: (&str, &str, &str) = (
    "30819b301006072a8648ce3d020106052b8104002303818600040079586325f44d28973f270dcb9595a6309b93c3898eedb901b08873694c690265e61c1cbf5b1a339cacdcfe3ece7e00744bf6ed158d663209c8acf378ddda2e824100b1386a84a2efa8d4f78b6cf7018e58fac5bb9a57f33b3ef584649ba57910a163ba026245ad9e1b93597b5db1f05431267b3953c205312d0c8bf0d910eef237f438",
    "9b71d224bd62f3785d96d46ad3ea3d73319bfbc2890caadae2dff72519673ca72323c3d99ba5c11d7c7acc6e14b8c5da0c4663475c2e5c3adef46f73bcdec043",
    "30818802420156bae253f050fc9202661280cbb8b1eedb8d4767aee63afdd130d376f50a18097902d0c6bd17eb8e4b4383f355751f3578e06850d5f3dfd8a86f3085e7100535b4024201f92f6f722290652195edf7b3168a91425a4e8a70386e06af6bceafd8a5f26dc19e5c13e7566e789d9459433e911e439819fdc350ea8ab1df6421257eaa55e23cf8",
);
const BP256: (&str, &str, &str) = (
    "305a301406072a8648ce3d020106092b2403030208010107034200049ba05e64295ad9b86295d9cfa717a2549474e83abb2825be32a0fae5297b07344f9e9018cf394001f98d634680b1e20be49f6118c726fdad4b11ccae99b95358",
    "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    "304402203815740bcccfdec285f1e168f28b4529bb39eba2683085260653c7a53dd2e69e022048ba910ce1f9141d8213fa1804a3d548271e0b9c307f6b26c80951be7e7a59a2",
);
const BP384: (&str, &str, &str) = (
    "307a301406072a8648ce3d020106092b240303020801010b03620004874bc125ea23dd5553b1e5440c5cb064032f9a6cbdea271745232ade525b082951d0b251a45b2af74d3325dbe7872f9635b6223c6b2cd4d7f24a50fd9058f946a154919adb24e268975df951e561884b14f899bc87af0820c50f3b706b7acb4c",
    "59e1748777448c69de6b800d7a33bbfb9ff1b463e44354c3553bcdb9c666fa90125a3c79f90397bdf5f6a13de828684f",
    "30640230284776ae1458b137bfe0f1628193983ae8bd5d58d84957fb215da42c5ba0e48400aaba4960b6e0a23aa8d7c20084cbba02301000b700ae6d946ad032f7a08e64c565bb6227e722edd2813dcbc5dfa79d099cf93ea8d48b4293869942a4f68671a915",
);

#[test]
fn verifies_ecdsa_on_p521_and_the_brainpool_curves() {
    use pdfcraft_sign::der::Tlv;
    use pdfcraft_sign::keys::{DigestAlg, Scheme};
    for (name, (spki, digest, sig), alg, describe) in [
        ("P-521", P521, DigestAlg::Sha512, "ECDSA P-521"),
        ("brainpoolP256r1", BP256, DigestAlg::Sha256, "ECDSA brainpoolP256r1"),
        ("brainpoolP384r1", BP384, DigestAlg::Sha384, "ECDSA brainpoolP384r1"),
    ] {
        let spki = unhex(spki);
        let key = PublicKey::from_spki(&Tlv::parse_all(&spki).unwrap()).unwrap();
        assert_eq!(key.describe(), describe);
        assert_eq!(key.spki(), spki, "{name}: re-encodes");
        let (digest, sig) = (unhex(digest), unhex(sig));
        assert!(key.verify(Scheme::Ecdsa, alg, &digest, &sig).unwrap(), "{name}");
        let mut bad = digest.clone();
        bad[0] ^= 1;
        assert!(!key.verify(Scheme::Ecdsa, alg, &bad, &sig).unwrap(), "{name}: other digest");
        // The digest named by the algorithm identifier may be shorter than the curve (SHA-256 on
        // P-521 etc.): that is a different digest, so it must not verify this signature.
        assert!(!key.verify(Scheme::Ecdsa, alg, &digest[..digest.len() / 2], &sig).unwrap(), "{name}: short digest");
    }
}
