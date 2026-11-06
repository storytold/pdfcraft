//! Strict checks for a newly returned signature, distinct from lenient historical validation.

use std::collections::HashSet;

use super::{CONTENT_TYPE, DATA, MESSAGE_DIGEST, SIGNING_CERT_V2, SignedData, bad};
use crate::der::{Tlv, tag};
use crate::{Certificate, DigestAlg, SignError};

/// Before the existing CMS reader runs, bound the entire DER tree, including structures
/// inside optional attributes. This path accepts newly produced DER, never padded or BER data.
fn bounded_der(bytes: &[u8]) -> Result<Tlv<'_>, SignError> {
    if bytes.is_empty() || bytes.len() > 1024 * 1024 {
        return Err(bad("the new CMS must contain between 1 byte and 1 MiB"));
    }
    let root = Tlv::parse_all(bytes)?;
    let mut pending = vec![(root, 0usize)];
    let mut nodes = 1usize;
    while let Some((node, depth)) = pending.pop() {
        if depth > 32 {
            return Err(bad("the new CMS is nested too deeply"));
        }
        if node.tag & 0x20 == 0 {
            continue;
        }
        let mut rest = node.value;
        while !rest.is_empty() {
            if nodes >= 8192 {
                return Err(bad("the new CMS has too many elements"));
            }
            let (child, tail) = Tlv::parse(rest)?;
            pending.push((child, depth + 1));
            nodes += 1;
            rest = tail;
        }
    }
    Ok(root)
}

/// RFC 5035 §5.4: the first ESSCertIDv2 binds the signing certificate. If its optional
/// issuerSerial is present it must name the same certificate, not only a matching hash.
fn certificate_reference(value: Tlv<'_>, cert: &Certificate) -> Result<(), SignError> {
    let fields = value.expect(tag::SEQUENCE, "SigningCertificateV2")?.children()?;
    let certs = fields.first().ok_or_else(|| bad("missing ESS certificate references"))?.expect(tag::SEQUENCE, "ESS certs")?.children()?;
    let reference = certs.first().ok_or_else(|| bad("empty ESS certificate references"))?.expect(tag::SEQUENCE, "ESSCertIDv2")?.children()?;
    let mut fields = reference.into_iter();
    let first = fields.next().ok_or_else(|| bad("empty ESSCertIDv2"))?;
    let (alg, hash) = if first.tag == tag::SEQUENCE {
        let algorithm = first.children()?;
        let oid = algorithm.first().ok_or_else(|| bad("missing ESS digest algorithm"))?.oid()?;
        let alg = DigestAlg::from_oid(&oid).ok_or_else(|| bad("unsupported ESS digest algorithm"))?;
        (alg, fields.next().ok_or_else(|| bad("missing ESS certificate hash"))?)
    } else {
        (DigestAlg::Sha256, first)
    };
    if !matches!(alg, DigestAlg::Sha256 | DigestAlg::Sha384 | DigestAlg::Sha512)
        || hash.expect(tag::OCTET_STRING, "ESS certificate hash")?.value != alg.digest(&[&cert.raw])
    {
        return Err(bad("ESS does not bind the signing certificate"));
    }
    if let Some(issuer_serial) = fields.next() {
        let parts = issuer_serial.expect(tag::SEQUENCE, "ESS issuerSerial")?.children()?;
        let [issuer, serial] = parts.as_slice() else { return Err(bad("invalid ESS issuerSerial")) };
        let names = issuer.expect(tag::SEQUENCE, "ESS issuer")?.children()?;
        let issuer_matches = names.iter().any(|n| n.tag == tag::ctx(4) && n.value == cert.issuer.raw);
        let serial = serial.expect(tag::INTEGER, "ESS serial")?.uint_bytes();
        if !issuer_matches || super::strip(serial) != super::strip(&cert.serial) {
            return Err(bad("ESS issuerSerial does not name the signing certificate"));
        }
    }
    if fields.next().is_some() {
        return Err(bad("unexpected ESSCertIDv2 fields"));
    }
    Ok(())
}

/// This verifies structure and certificate binding; the caller additionally checks the PDF
/// digest and the signature value against its immutable snapshot. No trust is inferred here.
pub(crate) fn checked_detached(bytes: &[u8], baseline: bool, has_pdf_reason: bool) -> Result<SignedData, SignError> {
    let ci = bounded_der(bytes)?.expect(tag::SEQUENCE, "ContentInfo")?.children()?;
    let [_, content] = ci.as_slice() else { return Err(bad("invalid ContentInfo")) };
    let fields = content.expect(tag::ctx(0), "SignedData")?.inner()?.children()?;
    let signers = fields.last().ok_or_else(|| bad("missing signerInfos"))?.expect(tag::SET, "signerInfos")?.children()?;
    if signers.len() != 1 {
        return Err(bad("a new PDF signature must have exactly one signer"));
    }
    let sd = SignedData::parse(bytes)?;
    let algorithms = fields.get(1).ok_or_else(|| bad("missing digestAlgorithms"))?.expect(tag::SET, "digestAlgorithms")?.children()?;
    let [algorithm] = algorithms.as_slice() else { return Err(bad("a new single-signer CMS needs one digest algorithm")) };
    let algorithm = algorithm.expect(tag::SEQUENCE, "digest algorithm")?.children()?;
    let digest_oid = algorithm.first().ok_or_else(|| bad("missing digest algorithm identifier"))?.oid()?;
    if digest_oid != sd.signer.digest.oid() || algorithm.len() > 2 || algorithm.get(1).is_some_and(|p| p.tag != tag::NULL || !p.value.is_empty()) {
        return Err(bad("digestAlgorithms does not match the signer digest"));
    }
    if sd.content.is_some() || sd.econtent_type != DATA || sd.signer.content_type.as_deref() != Some(DATA) {
        return Err(bad("the returned CMS must be detached and sign id-data"));
    }
    if !matches!(sd.signer.digest, DigestAlg::Sha256 | DigestAlg::Sha384 | DigestAlg::Sha512)
        || sd.signer.scheme_digest.is_some_and(|d| d != sd.signer.digest)
    {
        return Err(bad("the new signature must use a consistent SHA-256, SHA-384 or SHA-512 algorithm"));
    }
    let cert = sd.signer_certificate().ok_or_else(|| bad("missing signing certificate"))?;
    if let Some(problem) = cert.signing_problem_clause() {
        return Err(SignError::Crypto(format!("the returned signing certificate {problem}")));
    }
    let attrs = sd.signer.signed_attrs.as_deref().ok_or_else(|| bad("missing signed attributes"))?;
    let mut seen = HashSet::new();
    for attr in Tlv::parse_all(attrs)?.expect(tag::SET, "signed attributes")?.children()? {
        let fields = attr.expect(tag::SEQUENCE, "signed attribute")?.children()?;
        let [oid, values] = fields.as_slice() else { return Err(bad("invalid signed attribute")) };
        let oid = oid.oid()?;
        if !seen.insert(oid.clone()) {
            return Err(bad("duplicate signed attribute"));
        }
        let values = values.expect(tag::SET, "attribute values")?.children()?;
        if values.len() != 1 {
            return Err(bad("a new signature attribute must have exactly one value"));
        }
        if oid == SIGNING_CERT_V2 {
            let value = *values.first().ok_or_else(|| bad("missing ESS reference"))?;
            certificate_reference(value, cert)?;
        }
        // ETSI EN 319 142-1 V1.2.1 table 1: allowed signed attributes of baseline
        // signatures. signingTime and contentHints are deliberately not in this list.
        if baseline
            && !matches!(
                oid.as_str(),
                CONTENT_TYPE | MESSAGE_DIGEST | SIGNING_CERT_V2
                    | "1.2.840.113549.1.9.16.2.50" // signer-attributes-v2
                    | "1.2.840.113549.1.9.16.2.20" // content-time-stamp
                    | "1.2.840.113549.1.9.16.2.15" // signature-policy-identifier
                    | "1.2.840.113549.1.9.16.2.16" // commitment-type-indication
            )
        {
            return Err(bad("the returned signed attributes do not match the requested PAdES baseline profile"));
        }
    }
    if !seen.contains(CONTENT_TYPE) || !seen.contains(MESSAGE_DIGEST) || !seen.contains(SIGNING_CERT_V2) {
        return Err(bad("a new PDF signature needs content-type, message-digest and signing-certificate-v2"));
    }
    if baseline && has_pdf_reason && (seen.contains("1.2.840.113549.1.9.16.2.15") || seen.contains("1.2.840.113549.1.9.16.2.16")) {
        return Err(bad("the returned PAdES attributes conflict with the prepared PDF reason"));
    }
    Ok(sd)
}
