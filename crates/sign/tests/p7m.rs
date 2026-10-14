//! CAdES envelope files (issue #1449): `.pdf.p7m` and `.p7s` classification and reading,
//! against fixtures made by OpenSSL 3 (tests/data/README.md).

use pdfcraft_sign::cms;
use pdfcraft_sign::der::{self, Tlv, tag};
use pdfcraft_sign::p7m;
use pdfcraft_sign::pkcs12;

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn test_id() -> pkcs12::DigitalId {
    pkcs12::open(&data("rsa-aes.p12"), "test").unwrap()
}

#[test]
fn sniff_truth_table() {
    for name in ["cades-envelope.pdf.p7m", "cades-detached.p7s", "cades-xml.p7m"] {
        let bytes = data(name);
        assert!(p7m::is_cms_envelope(&bytes), "{name}");
        assert!(p7m::read(&bytes).is_ok(), "{name}");
    }
    // Hostile inputs: the sniff says no, `read` says why, and neither panics.
    let truncated_after_oid: &[u8] = &[0x30, 0x0B, 0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02];
    let plain_data_oid: &[u8] = &[0x30, 0x0B, 0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x01];
    for (bytes, sniffed) in [
        (&[][..], false),
        (&[0x30][..], false),
        // A long-form length of 65280 octets with nothing after the header.
        (&[0x30, 0x82, 0xFF, 0x00][..], false),
        // The header and the OID are intact but the object is cut off right after it.
        (truncated_after_oid, true),
        // An OID, but for plain data rather than SignedData (…1.7.1, not …1.7.2).
        (plain_data_oid, false),
    ] {
        assert_eq!(p7m::is_cms_envelope(bytes), sniffed, "{bytes:?}");
        assert!(p7m::read(bytes).is_err(), "{bytes:?}: an error, never a panic");
    }
}

#[test]
fn sniff_accepts_ber_indefinite_start() {
    // `30 80` is BER's indefinite length: a two-byte header, then the OID element.
    let ber: [u8; 13] = [0x30, 0x80, 0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02];
    assert!(p7m::is_cms_envelope(&ber));
}

#[test]
fn reads_the_committed_envelope() {
    let bytes = data("cades-envelope.pdf.p7m");
    let env = p7m::read(&bytes).unwrap();
    assert_eq!(env.signer_name, "PdfCraft Test");
    let content = env.content.as_deref().expect("an attached envelope carries its content");
    assert!(content.starts_with(b"%PDF-"), "the wrapped document, not metadata: {:?}", &content[..20.min(content.len())]);
    assert_eq!(content, &data("openssl-signed.pdf")[..], "byte-identical to the signed input");
    assert!(env.digest_ok);
    assert_eq!(env.signers, 1);
    assert!(env.signing_time.is_some(), "OpenSSL carries a signing-time attribute");
    assert!(env.quirks.is_empty(), "OpenSSL writes DER: {:?}", env.quirks);
}

#[test]
fn detached_fixture_reads_with_none_content() {
    let bytes = data("cades-detached.p7s");
    assert!(p7m::is_cms_envelope(&bytes));
    let env = p7m::read(&bytes).unwrap();
    assert_eq!(env.content, None, "detached: the document sits outside the envelope");
    assert_eq!(env.signer_name, "PdfCraft Test");
    assert_eq!(env.signers, 1);
    // With no content carried there is nothing here to check the signature against.
    assert!(!env.digest_ok);
}

#[test]
fn xml_envelope_content_is_returned() {
    let bytes = data("cades-xml.p7m");
    let env = p7m::read(&bytes).unwrap();
    assert_eq!(env.content.as_deref(), Some(data("cades-inner.xml").as_slice()));
    assert_eq!(env.signer_name, "PdfCraft Test");
    assert!(env.digest_ok);
    assert_eq!(env.signers, 1);
}

#[test]
fn digest_mismatch_is_reported() {
    let id = test_id();
    let alg = id.key.preferred_digest();
    // Content beginning with a marker unique in the envelope, so the byte to corrupt can be
    // found inside the CMS blob itself.
    const MARKER: &[u8] = b"%PDF-CRAFT-P7M-MARKER";
    let content = [MARKER, b"\n1 0 obj\nendobj\n%%EOF\n".as_slice()].concat();
    let blob = cms::sign_encapsulated(&id.key, &id.certificate, &id.chain, alg, cms::DATA, &content).unwrap();
    assert!(p7m::read(&blob).unwrap().digest_ok, "the envelope as signed is healthy");

    // Flip one byte of the marker inside the blob: the content no longer matches what was
    // signed, and `read` says so instead of failing or crashing.
    let mut tampered = blob.clone();
    let at = tampered.windows(MARKER.len()).position(|w| w == MARKER).expect("the marker is in the content");
    tampered[at + 5] ^= 0x20;
    let env = p7m::read(&tampered).unwrap();
    assert_ne!(env.content.as_deref(), Some(content.as_slice()), "the corrupted byte is in the content");
    assert!(!env.digest_ok);
}

#[test]
fn signerless_certificate_gives_digest_ok_false() {
    let id = test_id();
    let alg = id.key.preferred_digest();
    let content = b"%PDF-an unremarkable document\n".to_vec();
    let blob = cms::sign_encapsulated(&id.key, &id.certificate, &id.chain, alg, cms::DATA, &content).unwrap();
    let env = p7m::read(&blob).unwrap();
    assert_eq!(env.signer_name, "Test Signer RSA");
    assert!(env.digest_ok);

    // Without the certificates the signer cannot be identified and nothing is vouched for,
    // but the envelope still reads: its content is intact.
    let stripped = drop_certificates(&blob);
    let env = p7m::read(&stripped).unwrap();
    assert_eq!(env.signer_name, "the signer's certificate was not carried");
    assert!(!env.digest_ok);
    assert_eq!(env.content.as_deref(), Some(content.as_slice()));
}

#[test]
fn counts_signerinfos() {
    let id = test_id();
    let alg = id.key.preferred_digest();
    let blob = cms::sign_encapsulated(&id.key, &id.certificate, &id.chain, alg, cms::DATA, b"%PDF-one signer\n").unwrap();
    assert_eq!(p7m::read(&blob).unwrap().signers, 1);
    // A second SignerInfo in the SET is counted, though only the first one is read.
    assert_eq!(p7m::read(&with_signers(&blob, 2)).unwrap().signers, 2);
    assert_eq!(p7m::read(&data("cades-envelope.pdf.p7m")).unwrap().signers, 1);
}

/// The same SignedData with its certificates `[0]` emptied, as when a signer carries no
/// certificate for itself.
fn drop_certificates(envelope: &[u8]) -> Vec<u8> {
    let ci = Tlv::parse_all(envelope).unwrap();
    let parts = ci.children().unwrap();
    let [oid, content] = parts.as_slice() else { panic!("a ContentInfo") };
    let fields: Vec<Vec<u8>> = content.inner().unwrap().children().unwrap().into_iter().map(|t| t.raw.to_vec()).collect();
    // The certificates are the only context-tagged field of SignedData (RFC 5652 §5.1).
    let fields: Vec<Vec<u8>> = fields.into_iter().map(|f| if f.first() == Some(&tag::ctx(0)) { der::tlv(tag::ctx(0), &[]) } else { f }).collect();
    let refs: Vec<&[u8]> = fields.iter().map(Vec::as_slice).collect();
    let signed_data = der::seq(&refs);
    der::seq(&[oid.raw, &der::explicit(0, &signed_data)])
}

/// The same SignedData with its SignerInfo duplicated until the signerInfos SET holds `total`.
fn with_signers(envelope: &[u8], total: usize) -> Vec<u8> {
    let ci = Tlv::parse_all(envelope).unwrap();
    let parts = ci.children().unwrap();
    let [oid, content] = parts.as_slice() else { panic!("a ContentInfo") };
    let mut fields: Vec<Vec<u8>> = content.inner().unwrap().children().unwrap().into_iter().map(|t| t.raw.to_vec()).collect();
    // signerInfos is the last field of SignedData (RFC 5652 §5.1).
    let set = Tlv::parse_all(fields.last().unwrap()).unwrap();
    assert_eq!(set.tag, tag::SET, "the last field is the signerInfos SET");
    let mut signers: Vec<Vec<u8>> = set.children().unwrap().into_iter().map(|t| t.raw.to_vec()).collect();
    while signers.len() < total {
        signers.push(signers[0].clone());
    }
    let refs: Vec<&[u8]> = signers.iter().map(Vec::as_slice).collect();
    fields.pop();
    fields.push(der::set_of(&refs));
    let refs: Vec<&[u8]> = fields.iter().map(Vec::as_slice).collect();
    let signed_data = der::seq(&refs);
    der::seq(&[oid.raw, &der::explicit(0, &signed_data)])
}
