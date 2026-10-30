//! Synthetic recipient envelopes, including independent OpenSSL interoperability.
#![cfg(not(target_arch = "wasm32"))]

use aws_lc_rs::rsa::{Pkcs1PrivateDecryptingKey, PrivateDecryptingKey};
use pdfcraft_sign::{Certificate, DigestAlg, SignError, Time, der, envelope, pkcs12};
use zeroize::Zeroizing;

fn identity() -> pkcs12::DigitalId {
    let mut id = pkcs12::open(include_bytes!("../data/rsa-aes.p12"), "test").unwrap();
    // Issue a synthetic encryption-only certificate using the existing test key. No binary
    // fixture is committed, and the encoded certificate agrees with its parsed capabilities.
    let cert = der::Tlv::parse_all(&id.certificate.raw).unwrap().children().unwrap();
    let tbs = cert[0].children().unwrap();
    let mut fields: Vec<Vec<u8>> = tbs.iter().filter(|t| t.tag != der::tag::ctx(3)).map(|t| t.raw.to_vec()).collect();
    let usage = der::seq(&[&der::oid("2.5.29.15"), &der::octets(&der::tlv(der::tag::BIT_STRING, &[5, 0x20]))]);
    fields.push(der::explicit(3, &der::seq(&[&usage])));
    let tbs = der::seq(&fields.iter().map(Vec::as_slice).collect::<Vec<_>>());
    let signature = id.key.sign(DigestAlg::Sha256, &tbs).unwrap();
    id.certificate = Certificate::parse(&der::seq(&[&tbs, cert[1].raw, &der::bit_string(&signature)])).unwrap();
    id
}

fn payload() -> envelope::RecipientPayload {
    envelope::RecipientPayload::new(*b"synthetic-seed-12345", 0x12345678)
}

fn decrypt(id: &pkcs12::DigitalId, encrypted: &[u8]) -> Result<Zeroizing<Vec<u8>>, SignError> {
    let key = Pkcs1PrivateDecryptingKey::new(PrivateDecryptingKey::from_pkcs8(id.key.pkcs8()).unwrap()).unwrap();
    let mut plain = Zeroizing::new(vec![0; 512]);
    let used = key.decrypt(encrypted, &mut plain).map_err(|_| SignError::Crypto("test RSA decryption failed".into()))?.len();
    plain.truncate(used);
    Ok(plain)
}

#[test]
fn payload_uses_big_endian_permissions_and_redacts_the_seed() {
    let p = payload();
    assert_eq!(&p.encode()[20..], &[0x12, 0x34, 0x56, 0x78]);
    let decoded = envelope::RecipientPayload::decode(&p.encode()).unwrap();
    assert_eq!(decoded.seed(), p.seed());
    assert_eq!(decoded.permissions, p.permissions);
    assert!(!format!("{p:?}").contains("synthetic"));
    for length in [0, 19, 20, 23, 25, 100] {
        assert!(envelope::RecipientPayload::decode(&vec![0; length]).is_err());
    }
}

#[test]
fn envelope_round_trip_checks_identity_and_eligibility() {
    let id = identity();
    let p = payload();
    let encrypted = envelope::seal(&id.certificate, &p, id.certificate.not_before).unwrap();
    let opened = envelope::open(&encrypted, &id.certificate, |bytes| decrypt(&id, bytes)).unwrap();
    assert_eq!(opened.seed(), p.seed());
    assert_eq!(opened.permissions, p.permissions);
    assert_ne!(encrypted, envelope::seal(&id.certificate, &p, id.certificate.not_before).unwrap());
    let mut wrong = id.certificate.clone();
    wrong.serial.push(0);
    assert!(envelope::open(&encrypted, &wrong, |_| panic!("wrong recipient must not reach the card")).is_err());
    let expired = Time { year: 2099, ..id.certificate.not_before };
    assert!(envelope::seal(&id.certificate, &p, expired).is_err());
    // Expiry never prevents decryption of historical files.
    let original = pkcs12::open(include_bytes!("../data/rsa-aes.p12"), "test").unwrap();
    assert!(envelope::seal(&original.certificate, &p, original.certificate.not_before).is_err());
}

#[test]
fn malformed_envelopes_never_call_the_private_key() {
    let id = identity();
    let encoded = envelope::seal(&id.certificate, &payload(), id.certificate.not_before).unwrap();
    for end in 0..encoded.len() {
        assert!(envelope::open(&encoded[..end], &id.certificate, |_| panic!("truncated envelope reached private key")).is_err());
    }
    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(envelope::open(&trailing, &id.certificate, |_| panic!("trailing data reached private key")).is_err());
    assert!(envelope::open(&encoded, &id.certificate, |_| Ok(Zeroizing::new(vec![0; 16]))).is_err());
}

#[test]
fn duplicate_recipients_and_unsupported_key_transport_do_not_use_the_card() {
    let id = identity();
    let encoded = envelope::seal(&id.certificate, &payload(), id.certificate.not_before).unwrap();
    let root = der::Tlv::parse_all(&encoded).unwrap().children().unwrap();
    let body = root[1].inner().unwrap().children().unwrap();
    let recipients = body[1].children().unwrap();
    let duplicate = der::seq(&[body[0].raw, &der::set_of(&[recipients[0].raw, recipients[0].raw]), body[2].raw]);
    let duplicate = der::seq(&[root[0].raw, &der::explicit(0, &duplicate)]);
    assert!(envelope::open(&duplicate, &id.certificate, |_| panic!("duplicate recipient reached card")).is_err());
    let fields = recipients[0].children().unwrap();
    let wrong_algorithm = der::algorithm("1.2.840.113549.1.1.7", Some(&der::null()));
    let recipient = der::seq(&[fields[0].raw, fields[1].raw, &wrong_algorithm, fields[3].raw]);
    let body = der::seq(&[body[0].raw, &der::set_of(&[&recipient]), body[2].raw]);
    let encoded = der::seq(&[root[0].raw, &der::explicit(0, &body)]);
    assert!(matches!(envelope::open(&encoded, &id.certificate, |_| panic!("unsupported algorithm reached card")), Err(SignError::Unsupported(_))));
}

struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let dir = std::env::temp_dir().join(format!("pdfcraft-envelope-{suffix}"));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn write(&self, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
    fn openssl(&self, args: &[&str]) -> Vec<u8> {
        let result = std::process::Command::new("openssl")
            .args(args)
            .current_dir(&self.0)
            .output()
            .expect("OpenSSL is required for this interoperability test");
        assert!(result.status.success(), "OpenSSL failed: {}", String::from_utf8_lossy(&result.stderr));
        result.stdout
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn openssl_reads_our_envelopes_and_we_read_openssl_envelopes() {
    use base64::Engine;
    if std::process::Command::new("openssl").arg("version").output().is_err() {
        eprintln!("skipped: OpenSSL is not installed; the independent envelope oracle is unavailable");
        return;
    }
    let id = identity();
    let scratch = Scratch::new();
    let pem = format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        base64::engine::general_purpose::STANDARD.encode(&id.certificate.raw)
    );
    scratch.write("cert.pem", pem.as_bytes());
    scratch.write("key.der", id.key.pkcs8());
    let p = payload();
    scratch.write("payload", &p.encode());
    scratch.write("ours.der", &envelope::seal(&id.certificate, &p, id.certificate.not_before).unwrap());
    let plain = scratch
        .openssl(&["cms", "-decrypt", "-binary", "-inform", "DER", "-in", "ours.der", "-recip", "cert.pem", "-inkey", "key.der", "-keyform", "DER"]);
    assert_eq!(plain, *p.encode());
    for cipher in ["-aes128", "-aes256", "-des3"] {
        let encoded = scratch.openssl(&["cms", "-encrypt", "-binary", "-in", "payload", "-outform", "DER", cipher, "cert.pem"]);
        let opened = envelope::open(&encoded, &id.certificate, |bytes| decrypt(&id, bytes)).unwrap();
        assert_eq!(*opened.encode(), *p.encode(), "OpenSSL {cipher}");
    }
    // Streaming CMS uses BER indefinite lengths and constructed content OCTET STRINGs.
    let encoded = scratch.openssl(&["cms", "-encrypt", "-stream", "-binary", "-in", "payload", "-outform", "DER", "-aes256", "cert.pem"]);
    assert_eq!(*envelope::open(&encoded, &id.certificate, |bytes| decrypt(&id, bytes)).unwrap().encode(), *p.encode());
}

/// Run only against a new, disposable SoftHSM store. All PINs below are public test values.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires PDFCRAFT_SOFTHSM_UTIL, PDFCRAFT_PKCS11_MODULE, and a disposable SOFTHSM2_CONF"]
fn software_token_decrypts_an_encryption_only_certificate_envelope() {
    use std::process::Command;
    let module = std::env::var("PDFCRAFT_PKCS11_MODULE").expect("test module");
    let util = std::env::var("PDFCRAFT_SOFTHSM_UTIL").expect("SoftHSM utility");
    assert!(std::env::var("SOFTHSM2_CONF").is_ok(), "an isolated SoftHSM store is required");
    let scratch = Scratch::new();
    let id = identity();
    let label = scratch.0.file_name().unwrap().to_str().unwrap();
    let label = &label[label.len() - 24..]; // The generated ASCII random suffix fits a token label.
    let key_path = scratch.write("key.der", id.key.pkcs8());
    let cert_path = scratch.write("cert.der", &id.certificate.raw);
    let out = Command::new(&util)
        .args(["--module", &module, "--init-token", "--free", "--label", label, "--so-pin", "12345678", "--pin", "1234"])
        .output()
        .unwrap();
    assert!(out.status.success(), "SoftHSM: {}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new("pkcs11-tool")
        .args([
            "--module",
            &module,
            "--token-label",
            label,
            "--login",
            "--pin",
            "1234",
            "--write-object",
            key_path.to_str().unwrap(),
            "--type",
            "privkey",
            "--id",
            "02",
            "--usage-decrypt",
            "--always-auth",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "private key import: {}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new("pkcs11-tool")
        .args([
            "--module",
            &module,
            "--token-label",
            label,
            "--login",
            "--pin",
            "1234",
            "--write-object",
            cert_path.to_str().unwrap(),
            "--type",
            "cert",
            "--id",
            "02",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "certificate import: {}", String::from_utf8_lossy(&out.stderr));
    let identities = pdfcraft_sign::pkcs11::decryption_identities().unwrap();
    let card = identities
        .iter()
        .find(|entry| entry.certificate().raw == id.certificate.raw && entry.token_label() == label)
        .expect("encryption identity is discoverable");
    assert_eq!(card.token_label(), label);
    assert!(card.certificate().signing_problem().is_some(), "encryption does not depend on signing eligibility");
    let expected = payload();
    let encoded = envelope::seal(card.certificate(), &expected, card.certificate().not_before).unwrap();
    let opened = envelope::open(&encoded, card.certificate(), |key| card.decrypt_key(key, Some("1234"))).unwrap();
    assert_eq!(*opened.encode(), *expected.encode());
    assert!(card.decrypt_key(&[0; 1], None).is_err(), "bad ciphertext fails before any authentication");
}

/// End-to-end object-layer PDF encryption using real CMS recipient envelopes.
#[test]
fn certificate_pdf_bytes_round_trip_through_the_object_layer() {
    use pdfcraft_cos::{Algorithm, CosError, Document, EncryptDict, SaveOptions, SecurityHandler, write_full};
    use std::sync::Arc;
    const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Count 1 /Kids [3 0 R] >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >> endobj\n4 0 obj << /Length 18 >> stream\nCertificate secret\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF";
    let id = identity();
    let scratch = Scratch::new();
    scratch.write("key.der", id.key.pkcs8());
    scratch.write("cert.der", &id.certificate.raw);
    let oracle = std::env::var("PDFCRAFT_PYHANKO_PYTHON").ok();
    for algorithm in [Algorithm::Aes128, Algorithm::Aes256] {
        for metadata in [false, true] {
            let payload = envelope::RecipientPayload::new([7; 20], -1);
            let cms = envelope::seal(&id.certificate, &payload, id.certificate.not_before).unwrap();
            let handler = SecurityHandler::create_public_key(algorithm, vec![cms], payload.seed(), metadata).unwrap();
            let mut doc = Document::open(Arc::new(PDF.to_vec())).unwrap();
            doc.set_public_key_encryption(handler, [9; 16]).unwrap();
            let bytes = Arc::new(write_full(&doc, &SaveOptions::default()).unwrap());
            assert!(!String::from_utf8_lossy(&bytes).contains("Certificate secret"));
            let resolver = |params: &EncryptDict, _: &[u8]| {
                let cms = &params.public_key.as_ref().unwrap().recipients[0];
                let plain = envelope::open(cms, &id.certificate, |key| decrypt(&id, key)).map_err(|e| CosError::Security(e.to_string()))?;
                SecurityHandler::open_public_key(params.clone(), plain.seed(), plain.permissions).map_err(|e| CosError::Security(e.to_string()))
            };
            let reopened = Document::open_with_security_resolver(bytes.clone(), &resolver).unwrap();
            let root = reopened.get(reopened.root().unwrap());
            let pages = reopened.get(root.as_dict().unwrap().reference(b"Pages").unwrap());
            let page = reopened.resolve(&pages.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap()[0]);
            let stream_ref = page.as_dict().unwrap().reference(b"Contents").unwrap();
            let stream = reopened.get(stream_ref);
            let pdfcraft_cos::Object::Stream(stream) = stream.as_ref() else { panic!("page content") };
            assert_eq!(stream.decoded().unwrap(), b"Certificate secret");
            let signer = pkcs12::open(include_bytes!("../data/rsa-aes.p12"), "test").unwrap();
            let signed = pdfcraft_sign::sign(&reopened, &signer, &pdfcraft_sign::SignOptions::default()).unwrap();
            assert!(signed.starts_with(bytes.as_slice()), "signing preserves the encrypted original revision");
            let signed_doc = reopened.open_with_same_credentials(Arc::new(signed)).unwrap();
            assert_eq!(signed_doc.permissions(), reopened.permissions());
            if let Some(python) = &oracle {
                scratch.write("encrypted.pdf", &bytes);
                let script = r#"
from pyhanko.pdf_utils.reader import PdfFileReader
from pyhanko.pdf_utils.crypt.pubkey import SimpleEnvelopeKeyDecrypter
with open('encrypted.pdf', 'rb') as f:
    r = PdfFileReader(f)
    result = r.decrypt_pubkey(SimpleEnvelopeKeyDecrypter.load('key.der', 'cert.der'))
    assert result.status.name != 'FAILED', result
    assert r.root['/Pages']['/Kids'][0]['/Contents'].data == b'Certificate secret'
"#;
                let out = std::process::Command::new(python).args(["-c", script]).current_dir(&scratch.0).output().unwrap();
                assert!(out.status.success(), "pyHanko {algorithm:?} metadata={metadata}: {}", String::from_utf8_lossy(&out.stderr));
            }
        }
    }
    if oracle.is_none() {
        eprintln!("pyHanko oracle skipped: set PDFCRAFT_PYHANKO_PYTHON to its interpreter");
    }
}
