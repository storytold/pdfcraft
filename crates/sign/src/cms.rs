//! CMS `SignedData` (RFC 5652) as PDF signatures use it: detached (`adbe.pkcs7.detached`,
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
    /// ESS signing-certificate(-v2) present (`CAdES`).
    pub signing_certificate: bool,
    pub scheme: Scheme,
    /// The digest named by the signature algorithm, when it names one.
    pub scheme_digest: Option<DigestAlg>,
    pub signature: Vec<u8>,
    /// An RFC 3161 timestamp token is attached (unsigned attribute).
    pub timestamp: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedData {
    pub certificates: Vec<Certificate>,
    pub signer: SignerInfo,
    /// Encapsulated content (`adbe.pkcs7.sha1` carries the document digest here).
    pub content: Option<Vec<u8>>,
}

fn bad(what: &str) -> SignError {
    SignError::Malformed(format!("CMS: {what}"))
}

impl SignedData {
    /// Parse a `ContentInfo` holding `SignedData`. Bytes after it (a PDF placeholder's zero
    /// padding) are ignored.
    ///
    /// # Errors
    ///
    /// When `bytes` is not a `ContentInfo` holding `SignedData`, or any part of it — the
    /// `encapContentInfo`, certificates, `signerInfos`, digest or signature algorithm — is
    /// malformed or names an unsupported algorithm.
    pub fn parse(bytes: &[u8]) -> Result<SignedData, SignError> {
        let (ci, _) = Tlv::parse(bytes)?;
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
                let inner = c.inner()?;
                // Constructed OCTET STRINGs (BER) are not expected in DER-signed PDFs.
                Some(inner.expect(tag::OCTET_STRING, "eContent")?.value.to_vec())
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
        Ok(SignedData { certificates, signer: parse_signer(si)?, content })
    }

    /// The signer's certificate among those carried.
    #[must_use]
    pub fn signer_certificate(&self) -> Option<&Certificate> {
        self.certificates.iter().find(|c| match &self.signer.sid {
            SignerId::IssuerSerial { issuer, serial } => &c.issuer.raw == issuer && strip(&c.serial) == strip(serial),
            SignerId::KeyId(k) => c.subject_key_id.as_ref() == Some(k),
        })
    }

    /// Check the signature value with `cert`'s key. `content_digest` is the digest of the
    /// signed content (the document byte ranges) when there are no signed attributes.
    #[must_use]
    pub fn verify_signature(&self, cert: &Certificate, content_digest: &[u8]) -> bool {
        let s = &self.signer;
        let alg = s.scheme_digest.unwrap_or(s.digest);
        let digest = match &s.signed_attrs {
            Some(attrs) => alg.digest(&[attrs]),
            None => content_digest.to_vec(),
        };
        cert.public_key.verify(s.scheme, alg, &digest, &s.signature).unwrap_or(false)
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
        let mut set = next.raw.to_vec();
        set[0] = tag::SET;
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
    let signature = it.next().ok_or_else(|| bad("signature"))?.expect(tag::OCTET_STRING, "signature")?.value.to_vec();
    let mut timestamp = false;
    for t in it {
        if t.tag == tag::ctx(1) {
            for a in t.children()? {
                if a.children()?.first().and_then(|o| o.oid().ok()).as_deref() == Some(TIMESTAMP_TOKEN) {
                    timestamp = true;
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
    })
}

fn attribute(o: &str, value: &[u8]) -> Vec<u8> {
    der::seq(&[&der::oid(o), &der::set_of(&[value])])
}

/// A detached `CAdES` signature (`PAdES` B-B): signed attributes content-type, message-digest and
/// signing-certificate-v2 (no signing-time: `PAdES` takes the time from the signature
/// dictionary's `/M`). `chain` holds further certificates to embed (issuers).
///
/// # Errors
///
/// When `key` refuses or fails to sign: SHA-1 for a new signature, RSA signing in the browser,
/// or an error from the platform crypto or an external key.
pub fn sign_detached(
    key: &PrivateKey,
    cert: &Certificate,
    chain: &[Certificate],
    alg: DigestAlg,
    content_digest: &[u8],
) -> Result<Vec<u8>, SignError> {
    let cert_hash = DigestAlg::Sha256.digest(&[&cert.raw]);
    let general_names = der::seq(&[&der::explicit(4, &cert.issuer.raw)]);
    let ess = der::seq(&[&der::seq(&[&der::seq(&[&der::octets(&cert_hash), &der::seq(&[&general_names, &der::uint(&cert.serial)])])])]);
    let attrs = [attribute(CONTENT_TYPE, &der::oid(DATA)), attribute(MESSAGE_DIGEST, &der::octets(content_digest)), attribute(SIGNING_CERT_V2, &ess)];
    let refs: Vec<&[u8]> = attrs.iter().map(Vec::as_slice).collect();
    let set = der::set_of(&refs);
    let signature = key.sign(alg, &set)?;
    let mut signed_attrs = set.clone();
    signed_attrs[0] = tag::ctx(0);
    let sid = der::seq(&[&cert.issuer.raw, &der::uint(&cert.serial)]);
    let signer = der::seq(&[&der::int(1), &sid, &alg.algorithm(), &signed_attrs, &key.signature_algorithm(alg), &der::octets(&signature)]);
    let mut certs: Vec<&[u8]> = vec![&cert.raw];
    certs.extend(chain.iter().filter(|c| c.raw != cert.raw).map(|c| c.raw.as_slice()));
    let signed_data = der::seq(&[
        &der::int(1),
        &der::set_of(&[&alg.algorithm()]),
        &der::seq(&[&der::oid(DATA)]),
        &der::tlv(tag::ctx(0), &certs.concat()),
        &der::set_of(&[&signer]),
    ]);
    Ok(der::seq(&[&der::oid(SIGNED_DATA), &der::explicit(0, &signed_data)]))
}
