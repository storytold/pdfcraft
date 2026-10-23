//! Signing online: timestamps and revocation evidence fetched through a fake network.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pdfcraft_cos::Document;
use pdfcraft_sign::der::{self, tag};
use pdfcraft_sign::ltv::{Fetch, Online, sign_online};
use pdfcraft_sign::{Certificate, DigestAlg, Name, PrivateKey, SignError, SignOptions, Status, Time, TrustStore, pkcs12, signatures};

fn t(year: u32, month: u32, day: u32) -> Time {
    Time { year, month, day, hour: 0, minute: 0, second: 0 }
}

fn ext(oid: &str, value: &[u8]) -> Vec<u8> {
    der::seq(&[&der::oid(oid), &der::octets(value)])
}

fn uri(u: &str) -> Vec<u8> {
    der::tlv(tag::ctx_prim(6), u.as_bytes())
}

/// A certificate for `key` named `cn`, signed by `issuer` (or by itself), with `exts`.
fn cert(issuer: Option<&pkcs12::DigitalId>, cn: &str, key: &PrivateKey, serial: &[u8], exts: &[&[u8]]) -> Certificate {
    let subject = Name::build(cn, "", "", "", "").raw;
    let (signing_key, issuer_name) = match issuer {
        Some(ca) => (&ca.key, ca.certificate.subject.raw.clone()),
        None => (key, subject.clone()),
    };
    let alg = signing_key.signature_algorithm(DigestAlg::Sha256);
    let tbs = der::seq(&[
        &der::explicit(0, &der::int(2)),
        &der::uint(serial),
        &alg,
        &issuer_name,
        &der::seq(&[&t(2026, 1, 1).encode(), &t(2030, 1, 1).encode()]),
        &subject,
        &key.public_key().spki(),
        &der::explicit(3, &der::seq(exts)),
    ]);
    let sig = signing_key.sign(DigestAlg::Sha256, &tbs).unwrap();
    Certificate::parse(&der::seq(&[&tbs, &alg, &der::bit_string(&sig)])).unwrap()
}

/// A leaf for `cn` issued by `ca`, naming the test URLs for its issuer, OCSP and CRL.
fn issue(ca: &pkcs12::DigitalId, cn: &str, key: &PrivateKey, serial: &[u8]) -> Certificate {
    let aia = der::seq(&[
        &der::seq(&[&der::oid("1.3.6.1.5.5.7.48.1"), &uri("http://ocsp.test/")]),
        &der::seq(&[&der::oid("1.3.6.1.5.5.7.48.2"), &uri("http://ca.test/ca.cer")]),
    ]);
    let cdp = der::seq(&[&der::seq(&[&der::explicit(0, &der::explicit(0, &uri("http://crl.test/ca.crl")))])]);
    let key_usage = der::tlv(tag::BIT_STRING, &[6, 0b1100_0000]);
    cert(Some(ca), cn, key, serial, &[&ext("2.5.29.15", &key_usage), &ext("1.3.6.1.5.5.7.1.1", &aia), &ext("2.5.29.31", &cdp)])
}

/// A self-signed CA (basicConstraints CA, keyCertSign and cRLSign).
fn ca(cn: &str) -> pkcs12::DigitalId {
    let key = PrivateKey::generate_p256().unwrap();
    let basic = der::seq(&[&der::boolean(true)]);
    let key_usage = der::tlv(tag::BIT_STRING, &[1, 0b0000_0110]);
    let certificate = cert(None, cn, &key, &[1], &[&ext("2.5.29.19", &basic), &ext("2.5.29.15", &key_usage)]);
    pkcs12::DigitalId { key, certificate, chain: Vec::new(), friendly_name: None }
}

/// A self-signed timestamp authority: extended key usage timeStamping, critical (RFC 3161 §2.3).
fn tsa(cn: &str) -> pkcs12::DigitalId {
    let key = PrivateKey::generate_p256().unwrap();
    let eku = der::seq(&[&der::oid("2.5.29.37"), &der::boolean(true), &der::octets(&der::seq(&[&der::oid("1.3.6.1.5.5.7.3.8")]))]);
    let certificate = cert(None, cn, &key, &[2], &[&eku]);
    pkcs12::DigitalId { key, certificate, chain: Vec::new(), friendly_name: None }
}

/// An OCSP answer from `ca` about `cert`: good, or revoked since June 2026.
fn ocsp(ca: &pkcs12::DigitalId, cert: &Certificate, revoked: bool) -> Vec<u8> {
    let h = DigestAlg::Sha1;
    let cert_id = der::seq(&[
        &h.algorithm(),
        &der::octets(&h.digest(&[&cert.issuer.raw])),
        &der::octets(&h.digest(&[&ca.certificate.public_key.key_bits()])),
        &der::uint(&cert.serial),
    ]);
    let status = if revoked { der::tlv(tag::ctx(1), &t(2026, 6, 1).encode()) } else { der::tlv(tag::ctx_prim(0), &[]) };
    let single = der::seq(&[&cert_id, &status, &t(2026, 9, 1).encode(), &der::tlv(tag::ctx(0), &t(2027, 1, 1).encode())]);
    let tbs = der::seq(&[&der::tlv(tag::ctx(1), &ca.certificate.subject.raw), &t(2026, 9, 1).encode(), &der::seq(&[&single])]);
    let alg = ca.key.signature_algorithm(DigestAlg::Sha256);
    let sig = ca.key.sign(DigestAlg::Sha256, &tbs).unwrap();
    let basic = der::seq(&[&tbs, &alg, &der::bit_string(&sig)]);
    der::seq(&[&der::int(0), &der::explicit(0, &der::seq(&[&der::oid("1.3.6.1.5.5.7.48.1.1"), &der::octets(&basic)]))])
}

/// The fake network: fixed answers by URL, a timestamp authority, and a log of what was asked.
struct Net {
    answers: HashMap<String, Vec<u8>>,
    tsa: pkcs12::DigitalId,
    asked: Mutex<Vec<String>>,
}

impl Fetch for Net {
    fn get(&self, url: &str) -> Result<Vec<u8>, SignError> {
        self.asked.lock().unwrap().push(format!("GET {url}"));
        self.answers.get(url).cloned().ok_or_else(|| SignError::Crypto(format!("{url}: no answer")))
    }

    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<Vec<u8>, SignError> {
        self.asked.lock().unwrap().push(format!("POST {url} {content_type}"));
        if url == "http://tsa.test/" {
            let q = pdfcraft_sign::timestamp::parse_request(body)?;
            let cert = &self.tsa.certificate;
            return pdfcraft_sign::timestamp::respond(&self.tsa.key, cert, &[], DigestAlg::Sha256, &q, "1.2.3.4", t(2026, 10, 2), 9);
        }
        self.answers.get(url).cloned().ok_or_else(|| SignError::Crypto(format!("{url}: no answer")))
    }
}

fn fixture() -> Vec<u8> {
    let objs: [&[u8]; 4] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>",
        b"<< /Length 0 >>\nstream\n\nendstream",
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
    SignOptions { page: 0, date: "D:20261002120000-06'00'".into(), ..SignOptions::default() }
}

/// A card-like signer: its chain isn't on the card, only named by the certificate's AIA.
fn setup(revoked: bool) -> (pkcs12::DigitalId, pkcs12::DigitalId, Net) {
    let root = ca("Test Root");
    let key = PrivateKey::generate_p256().unwrap();
    let certificate = issue(&root, "Signer", &key, &[0x42]);
    let answers = HashMap::from([
        ("http://ca.test/ca.cer".to_string(), root.certificate.raw.clone()),
        ("http://ocsp.test/".to_string(), ocsp(&root, &certificate, revoked)),
    ]);
    let signer = pkcs12::DigitalId { key, certificate, chain: Vec::new(), friendly_name: None };
    (root, signer, Net { answers, tsa: tsa("Test TSA"), asked: Mutex::new(Vec::new()) })
}

#[test]
fn online_signing_makes_a_b_lta_file_that_validates_offline() {
    let (root, signer, net) = setup(false);
    let online = Online { tsa: Some("http://tsa.test/"), ltv: true, fetch: &net };
    let signed = sign_online(&open(&fixture()), &signer, &opts(), &online).unwrap();
    assert!(signed.problems.is_empty(), "{:?}", signed.problems);
    let asked = net.asked.lock().unwrap().clone();
    assert!(asked.contains(&"GET http://ca.test/ca.cer".to_string()), "the missing issuer is fetched: {asked:?}");
    assert!(asked.contains(&"POST http://ocsp.test/ application/ocsp-request".to_string()), "{asked:?}");
    assert_eq!(asked.iter().filter(|a| a.starts_with("POST http://tsa.test/")).count(), 2, "signature and document timestamps");
    assert!(!asked.iter().any(|a| a.contains("crl.test")), "a deciding OCSP answer makes the CRL unnecessary: {asked:?}");

    let bytes = signed.bytes;
    let doc = open(&bytes);
    let trust = TrustStore { certs: vec![root.certificate.clone(), net.tsa.certificate.clone()] };
    let all = signatures(&doc, &bytes, &trust);
    let field = all.iter().find(|s| s.signed && !s.doc_timestamp).unwrap();
    assert_eq!(field.status, Status::Valid, "{:?}", field.details);
    assert!(field.timestamp_time.is_some(), "B-T: {:?}", field.details);
    assert!(field.details.iter().any(|d| d.message.contains("embedded revocation information")), "B-LT: {:?}", field.details);
    let stamp = all.iter().find(|s| s.doc_timestamp).expect("B-LTA: a document timestamp");
    assert_eq!(stamp.status, Status::Valid, "{:?}", stamp.details);
}

#[test]
fn a_revoked_signer_stops_online_signing() {
    let (_, signer, net) = setup(true);
    let online = Online { tsa: None, ltv: true, fetch: &net };
    let e = sign_online(&open(&fixture()), &signer, &opts(), &online).unwrap_err();
    assert!(e.to_string().contains("revoked"), "{e}");
}

#[test]
fn unreachable_revocation_servers_are_reported_and_the_file_is_still_signed() {
    let (_, signer, mut net) = setup(false);
    net.answers.remove("http://ocsp.test/");
    let online = Online { tsa: None, ltv: true, fetch: &net };
    let signed = sign_online(&open(&fixture()), &signer, &opts(), &online).unwrap();
    assert_eq!(signed.problems.len(), 1, "{:?}", signed.problems);
    assert!(signed.problems[0].contains("Signer"), "{:?}", signed.problems);
    let asked = net.asked.lock().unwrap().clone();
    assert!(asked.contains(&"GET http://crl.test/ca.crl".to_string()), "falls back to the CRL: {asked:?}");
    let doc = open(&signed.bytes);
    assert_eq!(signatures(&doc, &signed.bytes, &TrustStore::default()).iter().filter(|s| s.signed).count(), 1);
}

#[test]
fn a_failing_timestamp_authority_fails_signing() {
    let (_, signer, net) = setup(false);
    let online = Online { tsa: Some("http://down.test/"), ltv: false, fetch: &net };
    assert!(sign_online(&open(&fixture()), &signer, &opts(), &online).is_err());
}
