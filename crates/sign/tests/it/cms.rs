//! What a CMS's signed attributes may hold. RFC 5652 allows one content-type and one
//! message-digest attribute, each with one value, and the content type must be the type of the
//! encapsulated content. Both content types are object identifiers.
//!
//! These go through `SignedData::parse`, the way a PDF's signature reaches it. Each CMS is built
//! here as the signer builds it, so a case differs from a valid one only where it says.

use pdfcraft_sign::cms::SignedData;
use pdfcraft_sign::der::{self, tag};
use pdfcraft_sign::keys::DigestAlg;
use pdfcraft_sign::{SignError, pkcs12};

const SIGNED_DATA: &str = "1.2.840.113549.1.7.2";
const DATA: &str = "1.2.840.113549.1.7.1";
const TST_INFO: &str = "1.2.840.113549.1.9.16.1.4";
const CONTENT_TYPE: &str = "1.2.840.113549.1.9.3";
const MESSAGE_DIGEST: &str = "1.2.840.113549.1.9.4";

fn identity() -> pkcs12::DigitalId {
    let p12 = std::fs::read(format!("{}/tests/data/rsa-aes.p12", env!("CARGO_MANIFEST_DIR"))).unwrap();
    pkcs12::open(&p12, "test").unwrap()
}

/// A signed attribute: its type and the SET of its values.
fn attribute(oid: &str, values: &[&[u8]]) -> Vec<u8> {
    der::seq(&[&der::oid(oid), &der::set_of(values)])
}

/// A SignedData whose encapsulated content has the type `encap_type` and whose one signer
/// signed the attributes `attrs`.
fn signed_data(id: &pkcs12::DigitalId, encap_type: &str, attrs: &[Vec<u8>]) -> Vec<u8> {
    signed_data_with_encap(id, &der::seq(&[&der::oid(encap_type)]), attrs)
}

/// A SignedData with the encapContentInfo `encap`, as it stands, and whose one signer signed the
/// attributes `attrs`.
fn signed_data_with_encap(id: &pkcs12::DigitalId, encap: &[u8], attrs: &[Vec<u8>]) -> Vec<u8> {
    let alg = DigestAlg::Sha256;
    let refs: Vec<&[u8]> = attrs.iter().map(Vec::as_slice).collect();
    let set = der::set_of(&refs);
    let signature = id.key.sign(alg, &set).unwrap();
    let mut signed_attrs = set;
    signed_attrs[0] = tag::ctx(0);
    let cert = &id.certificate;
    let sid = der::seq(&[&cert.issuer.raw, &der::uint(&cert.serial)]);
    let signer = der::seq(&[&der::int(1), &sid, &alg.algorithm(), &signed_attrs, &id.key.signature_algorithm(alg), &der::octets(&signature)]);
    let signed_data =
        der::seq(&[&der::int(1), &der::set_of(&[&alg.algorithm()]), encap, &der::tlv(tag::ctx(0), &cert.raw), &der::set_of(&[&signer])]);
    der::seq(&[&der::oid(SIGNED_DATA), &der::explicit(0, &signed_data)])
}

/// Why `SignedData::parse` refuses `cms`.
fn refused(cms: &[u8]) -> String {
    match SignedData::parse(cms) {
        Err(SignError::Malformed(why)) => why,
        other => panic!("expected the CMS to be refused, got {other:?}"),
    }
}

#[test]
fn signed_attributes_hold_one_content_type_and_one_message_digest() {
    let id = identity();
    let digest = DigestAlg::Sha256.digest(&[b"document"]);
    let (data, tst, md, other_md) = (der::oid(DATA), der::oid(TST_INFO), der::octets(&digest), der::octets(&[0; 32]));
    let content_type = attribute(CONTENT_TYPE, &[&data]);
    let message_digest = attribute(MESSAGE_DIGEST, &[&md]);

    // As a signer writes them: read, and the signature over them verifies.
    let sd = SignedData::parse(&signed_data(&id, DATA, &[content_type.clone(), message_digest.clone()])).unwrap();
    assert_eq!(sd.signer.content_type.as_deref(), Some(DATA));
    assert_eq!(sd.signer.message_digest.as_deref(), Some(digest.as_slice()));
    assert!(sd.verify_signature(&id.certificate, &digest).unwrap());

    // A second attribute of either kind, with the same value or another one, and an attribute
    // with two values or none.
    for (attrs, what) in [
        (vec![content_type.clone(), content_type.clone(), message_digest.clone()], "content type"),
        (vec![content_type.clone(), attribute(CONTENT_TYPE, &[&tst]), message_digest.clone()], "content type"),
        (vec![attribute(CONTENT_TYPE, &[&data, &tst]), message_digest.clone()], "content type"),
        (vec![attribute(CONTENT_TYPE, &[]), message_digest.clone()], "content type"),
        (vec![content_type.clone(), message_digest.clone(), message_digest.clone()], "message digest"),
        (vec![content_type.clone(), message_digest.clone(), attribute(MESSAGE_DIGEST, &[&other_md])], "message digest"),
        (vec![content_type.clone(), attribute(MESSAGE_DIGEST, &[&md, &other_md])], "message digest"),
        (vec![content_type.clone(), attribute(MESSAGE_DIGEST, &[])], "message digest"),
    ] {
        let why = refused(&signed_data(&id, DATA, &attrs));
        assert!(why.contains(&format!("one {what} with one value")), "{why}");
    }
}

#[test]
fn the_signed_content_type_is_the_type_of_the_encapsulated_content() {
    let id = identity();
    let digest = DigestAlg::Sha256.digest(&[b"document"]);
    let message_digest = attribute(MESSAGE_DIGEST, &[&der::octets(&digest)]);
    let signed_as = |oid: &str| attribute(CONTENT_TYPE, &[&der::oid(oid)]);

    // A PDF signature names data in both places, and an RFC 3161 token names TSTInfo in both.
    SignedData::parse(&signed_data(&id, DATA, &[signed_as(DATA), message_digest.clone()])).unwrap();
    SignedData::parse(&signed_data(&id, TST_INFO, &[signed_as(TST_INFO), message_digest.clone()])).unwrap();

    // The signature does not cover eContentType: here it was changed from data to signedData
    // (the last byte of the OID), and the signed attributes still say data.
    let why = refused(&signed_data(&id, SIGNED_DATA, &[signed_as(DATA), message_digest.clone()]));
    assert!(why.contains("not the encapsulated content type"), "{why}");

    // Without a content-type attribute there is nothing to compare here. The type is kept, and a
    // PDF signature's validation refuses one that is not data (tests/it/pdf.rs).
    let sd = SignedData::parse(&signed_data(&id, SIGNED_DATA, std::slice::from_ref(&message_digest))).unwrap();
    assert_eq!(sd.econtent_type, SIGNED_DATA);
}

#[test]
fn content_types_are_object_identifiers() {
    let id = identity();
    let digest = DigestAlg::Sha256.digest(&[b"document"]);
    let message_digest = attribute(MESSAGE_DIGEST, &[&der::octets(&digest)]);
    let content_type = attribute(CONTENT_TYPE, &[&der::oid(DATA)]);
    let not_an_oid = der::octets(DATA.as_bytes());

    // A content-type value that is not an OID would leave nothing to compare.
    let why = refused(&signed_data(&id, DATA, &[attribute(CONTENT_TYPE, &[&not_an_oid]), message_digest.clone()]));
    assert!(why.contains("expected an OID"), "{why}");

    // eContentType is required (RFC 5652 §5.2) and is an OID.
    let why = refused(&signed_data_with_encap(&id, &der::seq(&[&not_an_oid]), &[content_type.clone(), message_digest.clone()]));
    assert!(why.contains("expected an OID"), "{why}");
    let why = refused(&signed_data_with_encap(&id, &der::seq(&[]), &[content_type, message_digest]));
    assert!(why.contains("eContentType"), "{why}");
}
