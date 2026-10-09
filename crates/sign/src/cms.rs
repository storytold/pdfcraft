//! CMS SignedData (RFC 5652) as PDF signatures use it: detached (`adbe.pkcs7.detached`,
//! `ETSI.CAdES.detached`) or with the digest encapsulated (`adbe.pkcs7.sha1`).

use crate::SignError;
use crate::der::{self, Time, Tlv, tag};
use crate::keys::{DigestAlg, PrivateKey, Scheme, signature_algorithm};
use crate::x509::Certificate;

pub const SIGNED_DATA: &str = "1.2.840.113549.1.7.2";
pub const DATA: &str = "1.2.840.113549.1.7.1";
const CONTENT_TYPE: &str = "1.2.840.113549.1.9.3";
const MESSAGE_DIGEST: &str = "1.2.840.113549.1.9.4";
const SIGNING_TIME: &str = "1.2.840.113549.1.9.5";
const SIGNING_CERT_V2: &str = "1.2.840.113549.1.9.16.2.47";
const SIGNING_CERT: &str = "1.2.840.113549.1.9.16.2.12";
const TIMESTAMP_TOKEN: &str = "1.2.840.113549.1.9.16.2.14";

/// How a signer names its certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignerId {
    IssuerSerial { issuer: Vec<u8>, serial: Vec<u8> },
    KeyId(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignerInfo {
    pub sid: SignerId,
    pub digest: DigestAlg,
    /// The signed attributes as the SET that was signed (`None`: the signature covers the
    /// content itself).
    pub signed_attrs: Option<Vec<u8>>,
    pub message_digest: Option<Vec<u8>>,
    pub content_type: Option<String>,
    pub signing_time: Option<Time>,
    /// ESS signing-certificate(-v2) present (CAdES).
    pub signing_certificate: bool,
    pub scheme: Scheme,
    /// The digest named by the signature algorithm, when it names one.
    pub scheme_digest: Option<DigestAlg>,
    pub signature: Vec<u8>,
    /// An RFC 3161 timestamp token is attached (unsigned attribute).
    pub timestamp: bool,
    /// The attached token's raw CMS bytes, when present.
    pub timestamp_token: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedData {
    pub certificates: Vec<Certificate>,
    pub signer: SignerInfo,
    /// Encapsulated content (`adbe.pkcs7.sha1` carries the document digest here).
    pub content: Option<Vec<u8>>,
    /// Encoding irregularities that were tolerated (BER instead of DER, …), for the details.
    pub quirks: Vec<&'static str>,
}

fn bad(what: &str) -> SignError {
    SignError::Malformed(format!("CMS: {what}"))
}

impl SignedData {
    /// Parse a `ContentInfo` holding SignedData. Bytes after it (a PDF placeholder's zero
    /// padding) are ignored.
    pub fn parse(bytes: &[u8]) -> Result<SignedData, SignError> {
        // CMS from DocuSign, Documenso, Windows and `openssl cms -stream` is BER (indefinite lengths,
        // segmented strings). Only its structure is read this way: certificates and signed
        // attributes are still verified over their own exact bytes.
        let (ci, _) = Tlv::parse_ber(bytes)?;
        let mut quirks = Vec::new();
        if ci.is_indefinite() {
            quirks.push("The signature is BER-encoded (indefinite lengths) rather than DER; it was read leniently.");
        }
        let ci = ci.expect(tag::SEQUENCE, "ContentInfo")?.children()?;
        let [ct, content] = ci.as_slice() else { return Err(bad("ContentInfo")) };
        if ct.oid()? != SIGNED_DATA {
            return Err(bad("not SignedData"));
        }
        let sd = content.expect(tag::ctx(0), "content")?.inner()?.children()?;
        let mut it = sd.into_iter();
        let _version = it.next();
        let _digest_algs = it.next();
        let encap = it.next().ok_or_else(|| bad("encapContentInfo"))?.children()?;
        let content = match encap.get(1) {
            Some(c) if c.tag == tag::ctx(0) => {
                // BER signers may split the content into a constructed OCTET STRING.
                let inner = c.inner()?;
                if inner.tag != tag::OCTET_STRING {
                    quirks.push("The signed content is a constructed (segmented) OCTET STRING, as BER allows; it was read anyway.");
                }
                Some(inner.octets("eContent")?.into_owned())
            }
            _ => None,
        };
        let mut certificates = Vec::new();
        let mut signer_infos = None;
        for t in it {
            match t.tag {
                t2 if t2 == tag::ctx(0) => {
                    for c in t.children()? {
                        // Other certificate formats (attribute certificates) are skipped.
                        if c.tag == tag::SEQUENCE
                            && let Ok(cert) = Certificate::parse(c.raw)
                        {
                            certificates.push(cert);
                        }
                    }
                }
                tag::SET => signer_infos = Some(t),
                _ => {}
            }
        }
        let infos = signer_infos.ok_or_else(|| bad("no signerInfos"))?.children()?;
        let si = infos.first().ok_or_else(|| bad("no signer"))?;
        Ok(SignedData { certificates, signer: parse_signer(si)?, content, quirks })
    }

    /// The signer's certificate among those carried.
    pub fn signer_certificate(&self) -> Option<&Certificate> {
        self.certificates.iter().find(|c| match &self.signer.sid {
            SignerId::IssuerSerial { issuer, serial } => &c.issuer.raw == issuer && strip(&c.serial) == strip(serial),
            SignerId::KeyId(k) => c.subject_key_id.as_ref() == Some(k),
        })
    }

    /// Check the signature value with `cert`'s key. `content_digest` is the digest of the
    /// signed content (the document byte ranges) when there are no signed attributes.
    /// `Err(Unsupported)`: the signer's key algorithm can't be checked here.
    pub fn verify_signature(&self, cert: &Certificate, content_digest: &[u8]) -> Result<bool, SignError> {
        self.verify_signature_noting(cert, content_digest, &mut Vec::new())
    }

    /// [`SignedData::verify_signature`], noting in `notes` each encoding irregularity or
    /// disagreement it tolerated. Only meaningful when it returns `Ok(true)`.
    pub fn verify_signature_noting(&self, cert: &Certificate, content_digest: &[u8], notes: &mut Vec<&'static str>) -> Result<bool, SignError> {
        let s = &self.signer;
        // EdDSA signs the message itself: with signed attributes, their DER encoding.
        if s.scheme == Scheme::Ed25519 {
            let Some(attrs) = &s.signed_attrs else {
                return Err(SignError::Unsupported("Ed25519 signature without signed attributes".into()));
            };
            return cert.public_key.verify_message(s.scheme, attrs, &s.signature);
        }
        // The signature algorithm names the digest it used; a bare `rsaEncryption` / `ecPublicKey`
        // uses the SignerInfo's digestAlgorithm. Some signers disagree, so try both.
        let named = s.scheme_digest.unwrap_or(s.digest);
        let mut algs = vec![named];
        if !algs.contains(&s.digest) {
            algs.push(s.digest);
        }
        for alg in algs {
            let digest = match &s.signed_attrs {
                Some(attrs) => alg.digest(&[attrs]),
                None => content_digest.to_vec(),
            };
            // Notes from an attempt that doesn't verify say nothing about the signature.
            let mut attempt = Vec::new();
            if cert.public_key.verify_noting(s.scheme, alg, &digest, &s.signature, &mut attempt)? {
                notes.extend(attempt);
                // The hash of the content and the hash the signature is made with.
                for weak in [s.digest, alg].into_iter().filter_map(DigestAlg::weakness) {
                    if !notes.contains(&weak) {
                        notes.push(weak);
                    }
                }
                if alg != named {
                    notes.push("The signature algorithm and the SignerInfo's digest algorithm name different hashes; the signature was made with the SignerInfo's.");
                }
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn strip(b: &[u8]) -> &[u8] {
    match b {
        [0, rest @ ..] if !rest.is_empty() => rest,
        _ => b,
    }
}

fn parse_signer(si: &Tlv<'_>) -> Result<SignerInfo, SignError> {
    let f = si.children()?;
    let mut it = f.into_iter();
    let _version = it.next();
    let sid = it.next().ok_or_else(|| bad("sid"))?;
    let sid = if sid.tag == tag::SEQUENCE {
        let p = sid.children()?;
        let [issuer, serial] = p.as_slice() else { return Err(bad("issuerAndSerialNumber")) };
        SignerId::IssuerSerial { issuer: issuer.raw.to_vec(), serial: serial.value.to_vec() }
    } else {
        SignerId::KeyId(sid.value.to_vec())
    };
    let digest_alg = it.next().ok_or_else(|| bad("digestAlgorithm"))?.children()?;
    let d_oid = digest_alg.first().ok_or_else(|| bad("digestAlgorithm"))?.oid()?;
    let digest = DigestAlg::from_oid(&d_oid).ok_or_else(|| SignError::Unsupported(format!("digest {d_oid}")))?;
    let mut next = it.next().ok_or_else(|| bad("signatureAlgorithm"))?;
    let mut signed_attrs = None;
    let (mut message_digest, mut content_type, mut signing_time, mut signing_certificate) = (None, None, None, false);
    if next.tag == tag::ctx(0) {
        // Signed over the DER SET OF, i.e. the same contents with the universal SET tag.
        // (An indefinite-length encoding is re-encoded with a definite length.)
        let mut set = if next.is_indefinite() { der::tlv(tag::SET, next.value) } else { next.raw.to_vec() };
        if let Some(t) = set.first_mut() {
            *t = tag::SET;
        }
        for a in next.children()? {
            let p = a.children()?;
            let Some(o) = p.first().and_then(|o| o.oid().ok()) else { continue };
            let value = p.get(1).and_then(|v| v.children().ok()).and_then(|v| v.into_iter().next());
            match (o.as_str(), value) {
                (MESSAGE_DIGEST, Some(v)) => message_digest = Some(v.value.to_vec()),
                (CONTENT_TYPE, Some(v)) => content_type = v.oid().ok(),
                (SIGNING_TIME, Some(v)) => signing_time = v.time().ok(),
                (SIGNING_CERT_V2 | SIGNING_CERT, _) => signing_certificate = true,
                _ => {}
            }
        }
        signed_attrs = Some(set);
        next = it.next().ok_or_else(|| bad("signatureAlgorithm"))?;
    }
    let (scheme, scheme_digest) = signature_algorithm(&next)?;
    let signature = it.next().ok_or_else(|| bad("signature"))?.octets("signature")?.into_owned();
    let mut timestamp = false;
    let mut timestamp_token = None;
    for t in it {
        if t.tag == tag::ctx(1) {
            for a in t.children()? {
                let kids = a.children()?;
                if kids.first().and_then(|o| o.oid().ok()).as_deref() == Some(TIMESTAMP_TOKEN) {
                    timestamp = true;
                    // The value is a SET OF ContentInfo: the token's own encoding.
                    let set = kids.get(1).and_then(|s| s.children().ok());
                    timestamp_token = set.and_then(|c| c.first().copied()).map(|ci| ci.raw.to_vec());
                }
            }
        }
    }
    Ok(SignerInfo {
        sid,
        digest,
        signed_attrs,
        message_digest,
        content_type,
        signing_time,
        signing_certificate,
        scheme,
        scheme_digest,
        signature,
        timestamp,
        timestamp_token,
    })
}

/// Attach an RFC 3161 `signatureTimeStampToken` unsigned attribute to a detached CMS object.
/// The signed attributes and signature value are preserved byte-for-byte.
pub fn attach_timestamp_token(cms: &[u8], token: &[u8]) -> Result<Vec<u8>, SignError> {
    let (content_info, _) = Tlv::parse(cms)?;
    let ci = content_info.expect(tag::SEQUENCE, "ContentInfo")?.children()?;
    let [content_type, wrapped] = ci.as_slice() else { return Err(bad("ContentInfo")) };
    if content_type.oid()? != SIGNED_DATA {
        return Err(bad("not SignedData"));
    }
    let signed_data = wrapped.expect(tag::ctx(0), "content")?.inner()?;
    let mut sd = signed_data.children()?;
    // signerInfos is the last field of SignedData (RFC 5652 §5.1); searching from the end
    // skips the SET OF digest algorithms.
    let signer_index = sd.iter().rposition(|t| t.tag == tag::SET).ok_or_else(|| bad("no signerInfos"))?;
    let signer_set = sd[signer_index];
    let signer = signer_set.children()?.first().copied().ok_or_else(|| bad("no signer"))?;
    let mut signer_parts: Vec<Vec<u8>> = signer.children()?.into_iter().map(|t| t.raw.to_vec()).collect();
    if signer_parts.len() < 6 {
        return Err(bad("malformed signerInfo"));
    }
    let attr = der::seq(&[&der::oid(TIMESTAMP_TOKEN), &der::set_of(&[token])]);
    let unsigned = der::tlv(tag::ctx(1), &attr);
    signer_parts.push(unsigned);
    let signer_refs: Vec<&[u8]> = signer_parts.iter().map(Vec::as_slice).collect();
    let new_signer = der::seq(&signer_refs);
    let new_signer_set = der::set_of(&[&new_signer]);
    sd[signer_index] = Tlv::parse_all(&new_signer_set)?;
    let sd_refs: Vec<&[u8]> = sd.iter().map(|t| t.raw).collect();
    let new_sd = der::seq(&sd_refs);
    let wrapped_new = der::explicit(0, &new_sd);
    Ok(der::seq(&[&der::oid(SIGNED_DATA), &wrapped_new]))
}
fn attribute(o: &str, value: &[u8]) -> Vec<u8> {
    der::seq(&[&der::oid(o), &der::set_of(&[value])])
}

/// A detached CAdES signature (PAdES B-B): signed attributes content-type, message-digest and
/// signing-certificate-v2 (no signing-time: PAdES takes the time from the signature
/// dictionary's `/M`). `chain` holds further certificates to embed (issuers).
pub fn sign_detached(
    key: &PrivateKey,
    cert: &Certificate,
    chain: &[Certificate],
    alg: DigestAlg,
    content_digest: &[u8],
) -> Result<Vec<u8>, SignError> {
    build(key, cert, chain, alg, DATA, None, content_digest)
}

/// A CMS SignedData that encapsulates `content` under `content_type` (an RFC 3161 token wraps
/// its TSTInfo this way).
pub fn sign_encapsulated(
    key: &PrivateKey,
    cert: &Certificate,
    chain: &[Certificate],
    alg: DigestAlg,
    content_type: &str,
    content: &[u8],
) -> Result<Vec<u8>, SignError> {
    build(key, cert, chain, alg, content_type, Some(content), &alg.digest(&[content]))
}

// The parameters mirror the SignedData fields being assembled.
#[allow(clippy::too_many_arguments)]
fn build(
    key: &PrivateKey,
    cert: &Certificate,
    chain: &[Certificate],
    alg: DigestAlg,
    content_type: &str,
    content: Option<&[u8]>,
    content_digest: &[u8],
) -> Result<Vec<u8>, SignError> {
    let cert_hash = DigestAlg::Sha256.digest(&[&cert.raw]);
    let general_names = der::seq(&[&der::explicit(4, &cert.issuer.raw)]);
    let ess = der::seq(&[&der::seq(&[&der::seq(&[&der::octets(&cert_hash), &der::seq(&[&general_names, &der::uint(&cert.serial)])])])]);
    let attrs =
        [attribute(CONTENT_TYPE, &der::oid(content_type)), attribute(MESSAGE_DIGEST, &der::octets(content_digest)), attribute(SIGNING_CERT_V2, &ess)];
    let refs: Vec<&[u8]> = attrs.iter().map(Vec::as_slice).collect();
    let set = der::set_of(&refs);
    let signature = key.sign(alg, &set)?;
    let mut signed_attrs = set.clone();
    signed_attrs[0] = tag::ctx(0);
    let sid = der::seq(&[&cert.issuer.raw, &der::uint(&cert.serial)]);
    let signer = der::seq(&[&der::int(1), &sid, &alg.algorithm(), &signed_attrs, &key.signature_algorithm(alg), &der::octets(&signature)]);
    let mut certs: Vec<&[u8]> = vec![&cert.raw];
    certs.extend(chain.iter().filter(|c| c.raw != cert.raw).map(|c| c.raw.as_slice()));
    let encap = match content {
        Some(c) => der::seq(&[&der::oid(content_type), &der::explicit(0, &der::octets(c))]),
        None => der::seq(&[&der::oid(content_type)]),
    };
    let signed_data =
        der::seq(&[&der::int(1), &der::set_of(&[&alg.algorithm()]), &encap, &der::tlv(tag::ctx(0), &certs.concat()), &der::set_of(&[&signer])]);
    Ok(der::seq(&[&der::oid(SIGNED_DATA), &der::explicit(0, &signed_data)]))
}
