//! Online evidence for long-term validation: RFC 3161 timestamps over HTTP, and the OCSP
//! responses, CRLs and CA certificates a signer's chain names (PAdES B-T, B-LT and B-LTA).
//!
//! Transport-free like [`crate::timestamp`]: the caller supplies a [`Fetch`] (the desktop app and
//! the CLI do, and only when the user has turned timestamps or LTV on), so this crate never opens
//! a socket. Everything fetched is untrusted: answers are parsed and checked before they are kept,
//! and what couldn't be fetched is reported, never silently skipped.

use crate::cms::SignedData;
use crate::der::{self, Tlv};
use crate::dss::Evidence;
use crate::keys::DigestAlg;
use crate::pdf::{SignOptions, sign, sign_with_timestamp, timestamp_document};
use crate::pkcs12::DigitalId;
use crate::revocation::{CertificateList, OcspResponse, RevocationStatus};
use crate::timestamp::TimestampAuthority;
use crate::x509::Certificate;
use crate::{SignError, Time};

/// HTTP for the evidence: a plain GET and a POST of `body` as `content_type`. Answers are capped
/// by the implementation.
pub trait Fetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, SignError>;
    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<Vec<u8>, SignError>;
}

/// An RFC 3161 authority at `url` (RFC 3161 §3.4: POST `application/timestamp-query`). It keeps
/// the last answer so the token's certificates can join the LTV evidence.
pub struct HttpTsa<'a> {
    pub url: &'a str,
    pub fetch: &'a dyn Fetch,
    last: std::sync::Mutex<Vec<u8>>,
}

impl<'a> HttpTsa<'a> {
    pub fn new(url: &'a str, fetch: &'a dyn Fetch) -> Self {
        Self { url, fetch, last: std::sync::Mutex::new(Vec::new()) }
    }

    /// The certificates in the last token this authority returned.
    fn token_certificates(&self) -> Vec<Certificate> {
        let last = self.last.lock().map(|l| l.clone()).unwrap_or_default();
        // TimeStampResp ::= SEQUENCE { status PKIStatusInfo, timeStampToken ContentInfo OPTIONAL }
        let Some(token) = Tlv::parse_all(&last).ok().and_then(|r| r.children().ok()).and_then(|c| c.get(1).map(|t| t.raw.to_vec())) else {
            return Vec::new();
        };
        SignedData::parse(&token).map(|sd| sd.certificates).unwrap_or_default()
    }
}

impl TimestampAuthority for HttpTsa<'_> {
    fn timestamp(&self, request: &[u8]) -> Result<Vec<u8>, SignError> {
        let answer = self.fetch.post(self.url, "application/timestamp-query", request)?;
        if let Ok(mut last) = self.last.lock() {
            last.clone_from(&answer);
        }
        Ok(answer)
    }
}

/// A DER `OCSPRequest` (RFC 6960 §4.1.1) for `cert` issued by `issuer`, with SHA-1 `CertID`
/// hashes as responders expect.
pub fn ocsp_request(cert: &Certificate, issuer: &Certificate) -> Vec<u8> {
    let h = DigestAlg::Sha1;
    let cert_id = der::seq(&[
        &h.algorithm(),
        &der::octets(&h.digest(&[&cert.issuer.raw])),
        &der::octets(&h.digest(&[&issuer.public_key.key_bits()])),
        &der::uint(&cert.serial),
    ]);
    der::seq(&[&der::seq(&[&der::seq(&[&der::seq(&[&cert_id])])])])
}

/// What [`gather`] found: the evidence to embed, and why any certificate is left without it.
#[derive(Debug, Default)]
pub struct Gathered {
    pub evidence: Evidence,
    pub problems: Vec<String>,
    /// A certificate in the chain is revoked: the signature must not be presented as good.
    pub revoked: bool,
}

/// Certificates followed at most (a real chain has three or four).
const MAX_CERTS: usize = 12;
/// Network requests made at most in one gathering.
const MAX_FETCHES: usize = 24;

/// Gather LTV evidence for `certs` (the signer first, then its chain and any timestamp
/// authority's certificates) at time `at`: missing issuers from the Authority Information
/// Access, then, for every certificate below a root, an OCSP response or else a CRL. Only
/// answers that verify against the issuer and cover `at` are kept.
pub fn gather(certs: &[Certificate], at: Time, fetch: &dyn Fetch) -> Gathered {
    let mut out = Gathered::default();
    let mut pool: Vec<Certificate> = Vec::new();
    for c in certs {
        if pool.len() < MAX_CERTS && !pool.iter().any(|p| p.raw == c.raw) {
            pool.push(c.clone());
        }
    }
    let mut fetches = 0usize;
    let mut i = 0;
    while let Some(cert) = pool.get(i).cloned() {
        i += 1;
        if cert.is_self_signed() {
            continue;
        }
        let issuer = match pool.iter().find(|p| p.subject.raw == cert.issuer.raw && cert.signed_by(&p.public_key)) {
            Some(issuer) => issuer.clone(),
            None => match fetch_issuer(&cert, fetch, &mut fetches) {
                Some(issuer) if pool.len() < MAX_CERTS => {
                    pool.push(issuer.clone());
                    issuer
                }
                _ => {
                    out.problems.push(format!("the issuer of {} couldn't be found", cert.display_name()));
                    continue;
                }
            },
        };
        match revocation(&cert, &issuer, at, fetch, &mut fetches, &mut out.evidence) {
            Ok(RevocationStatus::Good) => {}
            Ok(RevocationStatus::Revoked { .. }) => {
                out.revoked = true;
                out.problems.push(format!("{} is revoked", cert.display_name()));
            }
            Ok(RevocationStatus::Unknown) => out.problems.push(format!("no revocation information covers {}", cert.display_name())),
            Err(e) => out.problems.push(format!("{}: {e}", cert.display_name())),
        }
    }
    out.evidence.certs = pool.into_iter().map(|c| c.raw).collect();
    out
}

/// The issuer's certificate from `cert`'s CA-issuers URLs (DER; a PKCS #7 bundle is read too).
fn fetch_issuer(cert: &Certificate, fetch: &dyn Fetch, fetches: &mut usize) -> Option<Certificate> {
    for url in &cert.ca_issuer_urls {
        let Some(bytes) = fetched(fetch, fetches, |f| f.get(url)) else { continue };
        let found = Certificate::parse(&bytes).map(|c| vec![c]).or_else(|_| SignedData::parse(&bytes).map(|sd| sd.certificates)).unwrap_or_default();
        if let Some(issuer) = found.into_iter().find(|c| c.subject.raw == cert.issuer.raw && cert.signed_by(&c.public_key)) {
            return Some(issuer);
        }
    }
    None
}

/// Ask the OCSP responders first, then the CRLs; keep the first answer that decides.
fn revocation(
    cert: &Certificate,
    issuer: &Certificate,
    at: Time,
    fetch: &dyn Fetch,
    fetches: &mut usize,
    evidence: &mut Evidence,
) -> Result<RevocationStatus, SignError> {
    let mut last_error = None;
    for url in &cert.ocsp_urls {
        let request = ocsp_request(cert, issuer);
        let Some(answer) = fetched(fetch, fetches, |f| f.post(url, "application/ocsp-request", &request)) else { continue };
        match OcspResponse::parse(&answer).map(|r| r.check(cert, issuer, at)) {
            Ok(RevocationStatus::Unknown) => {}
            Ok(status) => {
                evidence.ocsps.push(answer);
                return Ok(status);
            }
            Err(e) => last_error = Some(e),
        }
    }
    for url in &cert.crl_urls {
        let Some(answer) = fetched(fetch, fetches, |f| f.get(url)) else { continue };
        match CertificateList::parse(&answer).map(|l| l.check(cert, issuer, at)) {
            Ok(RevocationStatus::Unknown) => {}
            Ok(status) => {
                evidence.crls.push(answer);
                return Ok(status);
            }
            Err(e) => last_error = Some(e),
        }
    }
    match last_error {
        Some(e) => Err(e),
        None => Ok(RevocationStatus::Unknown),
    }
}

/// One request within the budget; a failed one counts too and gives nothing.
fn fetched(fetch: &dyn Fetch, fetches: &mut usize, ask: impl FnOnce(&dyn Fetch) -> Result<Vec<u8>, SignError>) -> Option<Vec<u8>> {
    if *fetches >= MAX_FETCHES {
        return None;
    }
    *fetches += 1;
    ask(fetch).ok()
}

/// Signing online: an optional timestamp authority, and whether to embed LTV evidence.
pub struct Online<'a> {
    /// RFC 3161 server URL for the signature timestamp (B-T), and the document timestamp that
    /// closes an LTV file (B-LTA).
    pub tsa: Option<&'a str>,
    /// Fetch and embed revocation evidence (B-LT).
    pub ltv: bool,
    pub fetch: &'a dyn Fetch,
}

/// The signed file, and what went wrong on the way that didn't stop it (missing evidence).
#[derive(Debug)]
pub struct Signed {
    pub bytes: Vec<u8>,
    pub problems: Vec<String>,
}

/// Sign `doc` (as [`sign`]) and go as far towards PAdES B-LTA as `online` allows: a signature
/// timestamp, then the chain's revocation evidence in a `/DSS`, then a document timestamp over
/// it all. A timestamp or a revoked certificate that fails stops the signing; missing
/// revocation evidence is reported in [`Signed::problems`] and the file is still produced.
pub fn sign_online(doc: &pdfcraft_cos::Document, id: &DigitalId, opts: &SignOptions, online: &Online<'_>) -> Result<Signed, SignError> {
    let tsa = online.tsa.map(|url| HttpTsa::new(url, online.fetch));
    let mut bytes = match &tsa {
        Some(tsa) => sign_with_timestamp(doc, id, opts, tsa)?,
        None => sign(doc, id, opts)?,
    };
    let mut problems = Vec::new();
    if !online.ltv {
        return Ok(Signed { bytes, problems });
    }
    let at = Time::from_pdf(&opts.date).ok_or_else(|| SignError::Malformed(format!("signing date {:?}", opts.date)))?;
    let mut certs = vec![id.certificate.clone()];
    certs.extend(id.chain.iter().cloned());
    if let Some(tsa) = &tsa {
        certs.extend(tsa.token_certificates());
    }
    let gathered = gather(&certs, at, online.fetch);
    if gathered.revoked {
        return Err(SignError::Crypto(format!("a certificate in the signing chain is revoked: {}", gathered.problems.join("; "))));
    }
    problems.extend(gathered.problems);
    bytes = crate::dss::embed(&open(bytes)?, &gathered.evidence)?;
    if let Some(tsa) = &tsa {
        bytes = timestamp_document(&open(bytes)?, tsa, &opts.date)?;
    }
    Ok(Signed { bytes, problems })
}

fn open(bytes: Vec<u8>) -> Result<pdfcraft_cos::Document, SignError> {
    Ok(pdfcraft_cos::Document::open(std::sync::Arc::new(bytes))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::der::tag;

    #[test]
    fn an_ocsp_request_names_the_certificate_by_issuer_hashes_and_serial() {
        let name = crate::x509::Name::build("CA", "", "", "", "");
        let key = crate::keys::PrivateKey::generate_p256().unwrap();
        let t = Time { year: 2026, month: 1, day: 1, hour: 0, minute: 0, second: 0 };
        let ca = Certificate::self_signed(&name, &key, t, 1, &[0x80, 1]).unwrap();
        let req = ocsp_request(&ca, &ca);
        let id = Tlv::parse_all(&req).unwrap().inner().unwrap().inner().unwrap().inner().unwrap().inner().unwrap();
        let fields = id.children().unwrap();
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[1].value, DigestAlg::Sha1.digest(&[&ca.issuer.raw]).as_slice());
        assert_eq!(fields[2].value, DigestAlg::Sha1.digest(&[&ca.public_key.key_bits()]).as_slice());
        assert_eq!(fields[3].tag, tag::INTEGER);
        assert_eq!(fields[3].uint_bytes(), [0x80, 1], "a high-bit serial keeps its sign byte");
    }
}
