//! Revocation evidence (RFC 5280 CRLs, RFC 6960 OCSP): parsing, signature verification and
//! status lookup for evidence embedded in a DSS or supplied by the caller. Never fetches:
//! the transport and the choice of evidence stay with the caller.

use crate::der::Tlv;
#[cfg(test)]
use crate::der::tag;
use crate::keys::{self, DigestAlg, PublicKey};
use crate::x509::Certificate;
use crate::{SignError, Time};

/// A DER size cap: real CRLs and OCSP responses are far smaller.
pub(crate) const MAX_EVIDENCE: usize = 2 * 1024 * 1024;

#[cfg(test)]
const ID_PKIX_OCSP_BASIC: &str = "1.3.6.1.5.5.7.48.1.1";
const EKU_OCSP_SIGNING: &str = "1.3.6.1.5.5.7.3.9";

/// What a piece of revocation evidence says about a certificate at a point in time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevocationStatus {
    /// The evidence covers the certificate and does not revoke it.
    Good,
    /// The evidence covers the certificate and revokes it.
    Revoked { at: Time },
    /// The evidence does not cover the certificate, is outside its window, or could not be
    /// verified.
    Unknown,
}

#[cfg(test)]
fn bad(what: impl Into<String>) -> SignError {
    SignError::Malformed(format!("revocation: {}", what.into()))
}

fn strip(b: &[u8]) -> &[u8] {
    match b {
        [0, rest @ ..] if !rest.is_empty() => rest,
        v => v,
    }
}

fn valid_in_window(this: Time, next: Option<Time>, at: Time) -> bool {
    at >= this && next.is_none_or(|n| at <= n)
}

/// Verify `signature` over the raw `signed` bytes with `key`, where the signature's algorithm
/// identifier names the digest.
fn verify_signature(signed: &[u8], alg_raw: &[u8], signature: &[u8], key: &PublicKey) -> bool {
    let Ok(alg) = Tlv::parse_all(alg_raw) else { return false };
    let Ok((scheme, Some(digest))) = keys::signature_algorithm(&alg) else { return false };
    let d = digest.digest(&[signed]);
    key.verify(scheme, digest, &d, signature).unwrap_or(false)
}

// ── X.509 CertificateList (CRL) ─────────────────────────────────────────────────────────────

/// A parsed CRL (RFC 5280 §5.1).
pub struct CertificateList {
    tbs: Vec<u8>,
    sig_alg: Vec<u8>,
    signature: Vec<u8>,
    pub issuer_raw: Vec<u8>,
    pub this_update: Time,
    pub next_update: Option<Time>,
    /// (serial, revocation date), top to bottom.
    pub revoked: Vec<(Vec<u8>, Time)>,
}

impl CertificateList {
    pub fn parse(bytes: &[u8]) -> Result<CertificateList, SignError> {
        let parsed = craft_pkcs7::revocation::CertificateList::parse(bytes).map_err(crate::der::app_error)?.into_parts();
        Ok(Self {
            tbs: parsed.tbs,
            sig_alg: parsed.sig_alg,
            signature: parsed.signature,
            issuer_raw: parsed.issuer_raw,
            this_update: parsed.this_update,
            next_update: parsed.next_update,
            revoked: parsed.revoked,
        })
    }

    // Retained parity oracle: original CRL parser by Justin John (4d0078aeff).
    #[cfg(test)]
    fn parse_legacy(bytes: &[u8]) -> Result<CertificateList, SignError> {
        if bytes.is_empty() || bytes.len() > MAX_EVIDENCE {
            return Err(bad("CRL is empty or exceeds the 2 MiB limit"));
        }
        let parts = Tlv::parse_all(bytes)?.expect(tag::SEQUENCE, "CertificateList")?.children()?;
        let [tbs, alg, sig] = parts.as_slice() else { return Err(bad("CertificateList")) };
        let mut f = tbs.children()?.into_iter().peekable();
        if f.peek().is_some_and(|t| t.tag == tag::INTEGER) {
            f.next(); // version
        }
        let _alg = f.next().ok_or_else(|| bad("tbsCertList"))?;
        let issuer = f.next().ok_or_else(|| bad("tbsCertList"))?.raw.to_vec();
        let this_update = f.next().ok_or_else(|| bad("tbsCertList"))?.time()?;
        let mut next_update = None;
        if f.peek().is_some_and(|t| t.tag == tag::UTC_TIME || t.tag == tag::GENERALIZED_TIME) {
            next_update = f.next().map(|t| t.time()).transpose()?;
        }
        let mut revoked = Vec::new();
        if f.peek().is_some_and(|t| t.tag == tag::SEQUENCE) {
            for entry in f.next().ok_or_else(|| bad("revokedCertificates"))?.children()? {
                let e = entry.children()?;
                let serial = e.first().ok_or_else(|| bad("revoked entry"))?.uint_bytes().to_vec();
                let at = e.get(1).ok_or_else(|| bad("revoked entry"))?.time()?;
                revoked.push((serial, at));
            }
        }
        Ok(CertificateList {
            tbs: tbs.raw.to_vec(),
            sig_alg: alg.raw.to_vec(),
            signature: sig.bits()?.to_vec(),
            issuer_raw: issuer,
            this_update,
            next_update,
            revoked,
        })
    }

    /// Check `cert` against this CRL at `at`. The CRL must be issued by `cert`'s issuer, its
    /// signature must verify with the issuer's key, and `at` must fall inside its window.
    pub fn check(&self, cert: &Certificate, issuer: &Certificate, at: Time) -> RevocationStatus {
        if self.issuer_raw != issuer.subject.raw || issuer.subject.raw != cert.issuer.raw {
            return RevocationStatus::Unknown;
        }
        if !verify_signature(&self.tbs, &self.sig_alg, &self.signature, &issuer.public_key) {
            return RevocationStatus::Unknown;
        }
        if !valid_in_window(self.this_update, self.next_update, at) {
            return RevocationStatus::Unknown;
        }
        match self.revoked.iter().find(|(serial, _)| strip(serial) == strip(&cert.serial)) {
            Some((_, t)) => RevocationStatus::Revoked { at: *t },
            None => RevocationStatus::Good,
        }
    }
}

// ── OCSP BasicOCSPResponse ──────────────────────────────────────────────────────────────────

/// One `CertID` (RFC 6960 §4.1.1): hashes of the issuer's name and key, and the serial.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertId {
    pub hash_alg: DigestAlg,
    pub issuer_name_hash: Vec<u8>,
    pub issuer_key_hash: Vec<u8>,
    pub serial: Vec<u8>,
}

impl CertId {
    /// The `CertID` naming `cert` as issued by `issuer`, using `hash_alg`.
    pub fn build(cert: &Certificate, issuer: &Certificate, hash_alg: DigestAlg) -> CertId {
        CertId {
            hash_alg,
            issuer_name_hash: hash_alg.digest(&[&cert.issuer.raw]),
            issuer_key_hash: hash_alg.digest(&[&issuer.public_key.key_bits()]),
            serial: cert.serial.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CertStatus {
    Good,
    Revoked(Time),
    Unknown,
}

/// One SingleResponse: the status of one certificate at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SingleResponse {
    pub cert_id: CertId,
    status: CertStatus,
    pub this_update: Time,
    pub next_update: Option<Time>,
}

enum ResponderId {
    ByName(Vec<u8>),
    ByKey(Vec<u8>),
}

/// A parsed OCSP response (RFC 6960 §4.2.1): the responder's identity, per-certificate
/// responses, and the delegated responder's certificate when it is embedded.
pub struct OcspResponse {
    tbs: Vec<u8>,
    sig_alg: Vec<u8>,
    signature: Vec<u8>,
    responder_id: ResponderId,
    pub responses: Vec<SingleResponse>,
    responder_cert: Option<Certificate>,
}

impl OcspResponse {
    pub fn parse(bytes: &[u8]) -> Result<OcspResponse, SignError> {
        use craft_pkcs7::revocation as shared;
        let parsed = shared::OcspResponse::parse(bytes).map_err(crate::der::app_error)?.into_parts();
        let responder_id = match parsed.responder_id {
            shared::ResponderId::ByName(v) => ResponderId::ByName(v),
            shared::ResponderId::ByKey(v) => ResponderId::ByKey(v),
        };
        let responses = parsed
            .responses
            .into_iter()
            .map(|response| {
                let (id, status, this_update, next_update) = response.into_parts();
                let hash_alg = match id.hash_alg {
                    craft_pkcs7::DigestAlg::Sha1 => DigestAlg::Sha1,
                    craft_pkcs7::DigestAlg::Sha224 => DigestAlg::Sha224,
                    craft_pkcs7::DigestAlg::Sha256 => DigestAlg::Sha256,
                    craft_pkcs7::DigestAlg::Sha384 => DigestAlg::Sha384,
                    craft_pkcs7::DigestAlg::Sha512 => DigestAlg::Sha512,
                    craft_pkcs7::DigestAlg::Sha512_224 => DigestAlg::Sha512_224,
                    craft_pkcs7::DigestAlg::Sha512_256 => DigestAlg::Sha512_256,
                    craft_pkcs7::DigestAlg::Sha3_224 => DigestAlg::Sha3_224,
                    craft_pkcs7::DigestAlg::Sha3_256 => DigestAlg::Sha3_256,
                    craft_pkcs7::DigestAlg::Sha3_384 => DigestAlg::Sha3_384,
                    craft_pkcs7::DigestAlg::Sha3_512 => DigestAlg::Sha3_512,
                    craft_pkcs7::DigestAlg::Ripemd160 => DigestAlg::Ripemd160,
                };
                let cert_id = CertId { hash_alg, issuer_name_hash: id.issuer_name_hash, issuer_key_hash: id.issuer_key_hash, serial: id.serial };
                let status = match status {
                    shared::CertStatus::Good => CertStatus::Good,
                    shared::CertStatus::Revoked(at) => CertStatus::Revoked(at),
                    shared::CertStatus::Unknown => CertStatus::Unknown,
                };
                SingleResponse { cert_id, status, this_update, next_update }
            })
            .collect();
        Ok(Self {
            tbs: parsed.tbs,
            sig_alg: parsed.sig_alg,
            signature: parsed.signature,
            responder_id,
            responses,
            responder_cert: parsed.responder_cert.map(Certificate::from_shared),
        })
    }

    #[cfg(test)]
    fn parse_legacy(bytes: &[u8]) -> Result<OcspResponse, SignError> {
        if bytes.is_empty() || bytes.len() > MAX_EVIDENCE {
            return Err(bad("OCSP response is empty or exceeds the 2 MiB limit"));
        }
        let resp = Tlv::parse_all(bytes)?.expect(tag::SEQUENCE, "OCSPResponse")?.children()?;
        let status = resp.first().ok_or_else(|| bad("missing responseStatus"))?.enumerated()?;
        if status != 0 {
            return Err(bad(format!("responder reported an error (status {status})")));
        }
        let response_bytes =
            resp.get(1).ok_or_else(|| bad("successful response has no responseBytes"))?.expect(tag::ctx(0), "responseBytes")?.inner()?.children()?;
        let [type_oid, wrapped] = response_bytes.as_slice() else { return Err(bad("responseBytes")) };
        if type_oid.oid()? != ID_PKIX_OCSP_BASIC {
            return Err(bad("unsupported response type"));
        }
        let basic = Tlv::parse_all(wrapped.value)?.expect(tag::SEQUENCE, "BasicOCSPResponse")?.children()?;
        let mut it = basic.into_iter();
        let tbs = it.next().ok_or_else(|| bad("BasicOCSPResponse"))?;
        let alg = it.next().ok_or_else(|| bad("BasicOCSPResponse"))?;
        let signature = it.next().ok_or_else(|| bad("BasicOCSPResponse"))?.expect(tag::BIT_STRING, "signature")?.bits()?.to_vec();
        let mut responder_cert = None;
        for t in it {
            if t.tag == tag::ctx(0) {
                for c in t.children()? {
                    if c.tag == tag::SEQUENCE
                        && let Ok(cert) = Certificate::parse(c.raw)
                    {
                        responder_cert.get_or_insert(cert);
                    }
                }
            }
        }
        let fields = tbs.children()?;
        let mut f = fields.into_iter().peekable();
        if f.peek().is_some_and(|t| t.tag == tag::ctx(0)) {
            f.next(); // version
        }
        let rid = f.next().ok_or_else(|| bad("ResponseData"))?;
        let responder_id = match rid.tag {
            t if t == tag::ctx(1) => ResponderId::ByName(rid.value.to_vec()),
            t if t == tag::ctx(2) => ResponderId::ByKey(Tlv::parse_all(rid.value)?.expect(tag::OCTET_STRING, "byKey responderID")?.value.to_vec()),
            _ => return Err(bad("responderID")),
        };
        let _produced_at = f.next().ok_or_else(|| bad("ResponseData"))?.time()?;
        let mut responses = Vec::new();
        for single in f.next().ok_or_else(|| bad("ResponseData"))?.children()? {
            let s = single.children()?;
            let cert_id_fields = s.first().ok_or_else(|| bad("SingleResponse"))?.children()?;
            let [hash_alg, name_hash, key_hash, serial] = cert_id_fields.as_slice() else { return Err(bad("CertID")) };
            let alg_oid = hash_alg.children()?.first().ok_or_else(|| bad("CertID"))?.oid()?;
            let Some(hash_alg) = DigestAlg::from_oid(&alg_oid) else { return Err(bad("unsupported CertID hash")) };
            let cert_id = CertId {
                hash_alg,
                issuer_name_hash: name_hash.value.to_vec(),
                issuer_key_hash: key_hash.value.to_vec(),
                serial: serial.uint_bytes().to_vec(),
            };
            let status = match s.get(1).map(|t| t.tag) {
                // RFC 6960 §4.2.1: CertStatus is CHOICE with IMPLICIT tags — good [0] NULL,
                // revoked [1] RevokedInfo (the value is the RevokedInfo's own contents),
                // unknown [2] UnknownInfo.
                Some(t) if t == tag::ctx_prim(0) => CertStatus::Good,
                Some(t) if t == tag::ctx(1) => {
                    let info = s.get(1).unwrap_or(&single);
                    let (date, _) = Tlv::parse(info.value)?;
                    CertStatus::Revoked(date.time()?)
                }
                Some(t) if t == tag::ctx_prim(2) => CertStatus::Unknown,
                _ => return Err(bad("certStatus")),
            };
            let this_update = s.get(2).ok_or_else(|| bad("SingleResponse"))?.time()?;
            let next_update = s.get(3).filter(|t| t.tag == tag::ctx(0)).map(|t| t.inner().and_then(|g| g.time())).transpose()?;
            responses.push(SingleResponse { cert_id, status, this_update, next_update });
        }
        Ok(OcspResponse { tbs: tbs.raw.to_vec(), sig_alg: alg.raw.to_vec(), signature, responder_id, responses, responder_cert })
    }

    /// Check `cert` (issued by `issuer`) at `at`. The responder must be the issuer itself, or
    /// a delegated responder with the OCSP-signing EKU whose certificate is issued by the
    /// issuer; the response signature and CertID must match, and `at` must fall inside the
    /// response window.
    pub fn check(&self, cert: &Certificate, issuer: &Certificate, at: Time) -> RevocationStatus {
        // Who signed the response? The issuer identified by responderID, or a delegated,
        // embedded responder identified the same way. Which of the two it is comes from that
        // choice, never from the embedded certificate's names, which anyone can copy.
        let (responder, delegated): (Option<&Certificate>, bool) = match &self.responder_id {
            ResponderId::ByName(name) => {
                if *name == issuer.subject.raw {
                    (Some(issuer), false)
                } else {
                    (self.responder_cert.as_ref().filter(|c| *name == c.subject.raw), true)
                }
            }
            ResponderId::ByKey(hash) => {
                if *hash == DigestAlg::Sha1.digest(&[&issuer.public_key.key_bits()]) {
                    (Some(issuer), false)
                } else {
                    (self.responder_cert.as_ref().filter(|c| *hash == DigestAlg::Sha1.digest(&[&c.public_key.key_bits()])), true)
                }
            }
        };
        let Some(responder) = responder else { return RevocationStatus::Unknown };
        // A delegated responder must carry the OCSP-signing EKU and be issued by the issuer.
        if delegated {
            if responder.issuer.raw != issuer.subject.raw || !responder.signed_by(&issuer.public_key) {
                return RevocationStatus::Unknown;
            }
            if !responder.extended_key_usage.as_ref().is_some_and(|e| e.iter().any(|o| o == EKU_OCSP_SIGNING)) {
                return RevocationStatus::Unknown;
            }
        }
        if !verify_signature(&self.tbs, &self.sig_alg, &self.signature, &responder.public_key) {
            return RevocationStatus::Unknown;
        }
        // Find the SingleResponse for this certificate: the CertID hashes must recompute.
        let single = self.responses.iter().find(|r| {
            let name_hash = r.cert_id.hash_alg.digest(&[&cert.issuer.raw]);
            let key_hash = r.cert_id.hash_alg.digest(&[&issuer.public_key.key_bits()]);
            name_hash == r.cert_id.issuer_name_hash && key_hash == r.cert_id.issuer_key_hash && strip(&r.cert_id.serial) == strip(&cert.serial)
        });
        let Some(single) = single else { return RevocationStatus::Unknown };
        if !valid_in_window(single.this_update, single.next_update, at) {
            return RevocationStatus::Unknown;
        }
        match single.status {
            CertStatus::Good => RevocationStatus::Good,
            CertStatus::Revoked(at) => RevocationStatus::Revoked { at },
            CertStatus::Unknown => RevocationStatus::Unknown,
        }
    }
}

#[cfg(test)]
mod shared_ocsp_tests {
    use super::*;
    use crate::der;

    fn compare(bytes: &[u8]) {
        match (OcspResponse::parse(bytes), OcspResponse::parse_legacy(bytes)) {
            (Ok(a), Ok(b)) => {
                assert_eq!(
                    (a.tbs, a.sig_alg, a.signature, a.responses, a.responder_cert),
                    (b.tbs, b.sig_alg, b.signature, b.responses, b.responder_cert)
                );
                match (a.responder_id, b.responder_id) {
                    (ResponderId::ByName(a), ResponderId::ByName(b)) | (ResponderId::ByKey(a), ResponderId::ByKey(b)) => assert_eq!(a, b),
                    _ => panic!("responder identifier changed"),
                }
            }
            (Err(a), Err(b)) => assert_eq!(format!("{a:?}"), format!("{b:?}")),
            _ => panic!("shared and retained OCSP parsers disagree"),
        }
    }

    #[test]
    fn shared_ocsp_matches_status_digest_identity_and_error_policy() {
        let date = Time { year: 2026, month: 6, day: 15, hour: 12, minute: 0, second: 0 };
        let cert = crate::pkcs12::open(include_bytes!("../tests/data/ec-p256.p12"), "test").unwrap().certificate;
        let statuses = [der::tlv(tag::ctx_prim(0), &[]), der::tlv(tag::ctx(1), &date.encode()), der::tlv(tag::ctx_prim(2), &[])];
        let identities = [der::tlv(tag::ctx(1), &cert.subject.raw), der::tlv(tag::ctx(2), &der::octets(&[1, 2, 3]))];
        for hash in DigestAlg::ALL {
            for status in &statuses {
                for identity in &identities {
                    let id = der::seq(&[&hash.algorithm(), &der::octets(&[1]), &der::octets(&[2]), &der::uint(&[0x80])]);
                    let single = der::seq(&[&id, status, &date.encode(), &der::explicit(0, &date.encode())]);
                    let tbs = der::seq(&[identity, &date.encode(), &der::seq(&[&single])]);
                    // Preserve the existing parser's accepted direct certificate sequence form.
                    let basic = der::seq(&[&tbs, &hash.algorithm(), &der::bit_string(&[1, 2, 3]), &der::explicit(0, &cert.raw)]);
                    let response = der::seq(&[
                        &der::tlv(tag::ENUMERATED, &[0]),
                        &der::explicit(0, &der::seq(&[&der::oid(ID_PKIX_OCSP_BASIC), &der::octets(&basic)])),
                    ]);
                    compare(&response);
                    for end in [0, 1, response.len() / 2, response.len() - 1] {
                        compare(&response[..end]);
                    }
                    // Mutate each byte in one representative record, keeping test cost bounded.
                    if hash == DigestAlg::Sha256 && status == &statuses[0] && identity == &identities[0] {
                        for i in 0..response.len() {
                            let mut mutated = response.clone();
                            mutated[i] ^= 0x80;
                            compare(&mutated);
                        }
                    }
                }
            }
        }
        compare(&vec![0; MAX_EVIDENCE]);
        compare(&vec![0; MAX_EVIDENCE + 1]);
        compare(&der::seq(&[&der::tlv(tag::ENUMERATED, &[1])]));
    }
}

#[cfg(test)]
mod shared_crl_tests {
    use super::*;
    use crate::der;

    #[test]
    fn shared_crl_parser_matches_retained_app_reader() {
        let date = Time { year: 2026, month: 6, day: 15, hour: 12, minute: 0, second: 0 };
        let algorithm = DigestAlg::Sha256.algorithm();
        let tbs = der::seq(&[
            &der::int(1),
            &algorithm,
            &der::seq(&[]),
            &date.encode(),
            &date.encode(),
            &der::seq(&[&der::seq(&[&der::uint(&[0x80]), &date.encode()])]),
        ]);
        let valid = der::seq(&[&tbs, &algorithm, &der::bit_string(&[1, 2, 3])]);
        let compare = |bytes: &[u8]| match (CertificateList::parse(bytes), CertificateList::parse_legacy(bytes)) {
            (Ok(a), Ok(b)) => {
                assert_eq!(
                    (a.tbs, a.sig_alg, a.signature, a.issuer_raw, a.this_update, a.next_update, a.revoked),
                    (b.tbs, b.sig_alg, b.signature, b.issuer_raw, b.this_update, b.next_update, b.revoked)
                );
            }
            (Err(a), Err(b)) => assert_eq!(format!("{a:?}"), format!("{b:?}")),
            _ => panic!("shared and retained CRL parsers disagree"),
        };
        for end in 0..=valid.len() {
            compare(&valid[..end]);
        }
        for i in 0..valid.len() {
            let mut mutated = valid.clone();
            mutated[i] ^= 0x80;
            compare(&mutated);
        }
        compare(&vec![0; MAX_EVIDENCE]);
        compare(&vec![0; MAX_EVIDENCE + 1]);
    }
}
