//! RFC 3161 timestamp queries and tokens.
//!
//! Transport-free by design: callers encode a request, send it through an explicitly
//! configured authority at the application boundary, and validate the returned DER response
//! before the token is attached to a CMS signature. [`respond`] is the TSA-side signer, used
//! by the deterministic test authority and by a locally configured stamping identity.

use crate::cms::SignedData;
use crate::der::{self, Tlv, tag};
use crate::keys::{DigestAlg, PrivateKey};
use crate::x509::Certificate;
use crate::{SignError, Time};

const TST_INFO: &str = "1.2.840.113549.1.9.16.1.4";
const MAX_RESPONSE: usize = 4 * 1024 * 1024;
const MAX_TOKEN: usize = 2 * 1024 * 1024;
const MAX_REQUEST: usize = 1024 * 1024;

/// RFC 3161 request data. `nonce` is optional but recommended for online requests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimestampQuery {
    pub digest: DigestAlg,
    pub imprint: Vec<u8>,
    pub policy: Option<String>,
    pub nonce: Option<u64>,
    pub certificate_request: bool,
}

impl TimestampQuery {
    pub fn new(digest: DigestAlg, imprint: Vec<u8>) -> Result<Self, SignError> {
        if imprint.len() != digest.digest(&[&[]]).len() {
            return Err(SignError::Malformed("RFC 3161 imprint length does not match its digest algorithm".into()));
        }
        Ok(Self { digest, imprint, policy: None, nonce: None, certificate_request: true })
    }

    /// Encode a DER `TimeStampReq` (RFC 3161 §2.4.1).
    pub fn encode(&self) -> Result<Vec<u8>, SignError> {
        let alg = self.digest.algorithm();
        let imprint = der::seq(&[&alg, &der::octets(&self.imprint)]);
        let mut parts: Vec<Vec<u8>> = vec![der::int(1), imprint];
        if let Some(policy) = &self.policy {
            parts.push(der::try_oid(policy)?);
        }
        if let Some(nonce) = self.nonce {
            parts.push(der::int(nonce));
        }
        if self.certificate_request {
            parts.push(der::boolean(true));
        }
        let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        Ok(der::seq(&refs))
    }
}

/// A validated RFC 3161 time-stamp token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimestampToken {
    pub raw: Vec<u8>,
    pub digest: DigestAlg,
    pub imprint: Vec<u8>,
    pub policy: String,
    pub serial: Vec<u8>,
    pub gen_time: Time,
}

impl TimestampToken {
    /// The TSA's certificate, from the token's embedded certificates.
    pub fn signer_certificate(&self) -> Option<Certificate> {
        let token = SignedData::parse(&self.raw).ok()?;
        token.signer_certificate().cloned()
    }
}

/// A caller-supplied transport. The sign crate never creates sockets.
pub trait TimestampAuthority {
    fn timestamp(&self, request: &[u8]) -> Result<Vec<u8>, SignError>;
}

/// The certificates carried inside a token's CMS (for trust anchoring at validation time).
pub fn token_certs(raw: &[u8]) -> Vec<Certificate> {
    SignedData::parse(raw).map(|sd| sd.certificates).unwrap_or_default()
}

fn bad(message: impl Into<String>) -> SignError {
    SignError::Malformed(format!("RFC 3161: {}", message.into()))
}

/// Decode a `TimeStampReq` as [`TimestampQuery::encode`] wrote it.
pub fn parse_request(bytes: &[u8]) -> Result<TimestampQuery, SignError> {
    if bytes.is_empty() || bytes.len() > MAX_REQUEST {
        return Err(bad("request is empty or exceeds the 1 MiB limit"));
    }
    let fields = Tlv::parse_all(bytes)?.expect(tag::SEQUENCE, "TimeStampReq")?.children()?;
    let mut it = fields.into_iter();
    let _version = it.next().ok_or_else(|| bad("missing version"))?.u64()?;
    let imprint = it.next().ok_or_else(|| bad("missing message imprint"))?.children()?;
    let alg_oid =
        imprint.first().ok_or_else(|| bad("missing imprint algorithm"))?.children()?.first().ok_or_else(|| bad("missing algorithm OID"))?.oid()?;
    let digest = DigestAlg::from_oid(&alg_oid).ok_or_else(|| bad("unsupported imprint digest algorithm"))?;
    let hash = imprint.get(1).ok_or_else(|| bad("missing imprint digest"))?.expect(tag::OCTET_STRING, "imprint digest")?.value.to_vec();
    let mut q = TimestampQuery::new(digest, hash)?;
    for f in it {
        match f.tag {
            tag::OID => q.policy = Some(f.oid()?),
            tag::INTEGER => q.nonce = Some(f.u64()?),
            tag::BOOLEAN => q.certificate_request = f.value.first() == Some(&0xFF),
            _ => {}
        }
    }
    Ok(q)
}

/// Parse and validate a token (a bare CMS `ContentInfo`): TSTInfo content, bounded size, the
/// TSA's CMS signature and its message digest — but not the imprint, which callers compare
/// against the signed data they hold.
pub fn parse_token(raw: &[u8]) -> Result<TimestampToken, SignError> {
    if raw.is_empty() || raw.len() > MAX_TOKEN {
        return Err(bad("token is empty or exceeds the 2 MiB limit"));
    }
    let token = SignedData::parse(raw)?;
    if token.signer.content_type.as_deref() != Some(TST_INFO) {
        return Err(bad("token content is not TSTInfo"));
    }
    let info = token.content.as_deref().ok_or_else(|| bad("token has no TSTInfo content"))?;
    let fields = Tlv::parse_all(info)?.expect(tag::SEQUENCE, "TSTInfo")?.children()?;
    let policy = fields.get(1).ok_or_else(|| bad("TSTInfo has no policy"))?.oid()?;
    let imprint_fields = fields.get(2).ok_or_else(|| bad("TSTInfo has no message imprint"))?.children()?;
    let alg_fields = imprint_fields.first().ok_or_else(|| bad("message imprint has no algorithm"))?.children()?;
    let digest = alg_fields
        .first()
        .ok_or_else(|| bad("message imprint has no digest algorithm"))?
        .oid()
        .and_then(|o| DigestAlg::from_oid(&o).ok_or_else(|| bad("unsupported imprint digest algorithm")))?;
    let imprint = imprint_fields
        .get(1)
        .ok_or_else(|| bad("message imprint has no digest"))?
        .expect(tag::OCTET_STRING, "message imprint digest")?
        .value
        .to_vec();
    let serial = fields.get(3).ok_or_else(|| bad("TSTInfo has no serial number"))?.uint_bytes().to_vec();
    let gen_time = fields.get(4).ok_or_else(|| bad("TSTInfo has no generation time"))?.time()?;
    // The signed messageDigest uses the SignerInfo's digest algorithm, which need not be the
    // imprint's (RFC 5652 §5.4).
    let content_digest = token.signer.digest.digest(&[info]);
    if token.signer.message_digest.as_deref() != Some(content_digest.as_slice()) {
        return Err(bad("token signed message digest does not match TSTInfo"));
    }
    let cert = token.signer_certificate().ok_or_else(|| bad("TSA certificate is missing"))?;
    if !token.verify_signature(cert, &content_digest) {
        return Err(bad("TSA signature is invalid"));
    }
    Ok(TimestampToken { raw: raw.to_vec(), digest, imprint, policy, serial, gen_time })
}

/// Parse and validate a `TimeStampResp`, including the CMS signature and message imprint.
pub fn parse_response(bytes: &[u8], query: &TimestampQuery) -> Result<TimestampToken, SignError> {
    if bytes.is_empty() || bytes.len() > MAX_RESPONSE {
        return Err(bad("response is empty or exceeds the 4 MiB limit"));
    }
    let fields = Tlv::parse_all(bytes)?.expect(tag::SEQUENCE, "TimeStampResp")?.children()?;
    let status_info = fields.first().ok_or_else(|| bad("missing status"))?.children()?;
    let status = status_info.first().ok_or_else(|| bad("missing status value"))?.u64()?;
    if status != 0 && status != 1 {
        return Err(bad(format!("TSA rejected the request (status {status})")));
    }
    let token = fields.get(1).ok_or_else(|| bad("granted response has no time-stamp token"))?;
    let token = parse_token(token.raw)?;
    if token.digest != query.digest || token.imprint != query.imprint {
        return Err(bad("message imprint does not match the requested digest"));
    }
    Ok(token)
}

/// The TSA side: sign a `TimeStampResp` for `query` at `gen_time`. Used by deterministic test
/// authorities and a locally configured stamping identity, never implied to be a remote TSA.
#[allow(clippy::too_many_arguments)]
pub fn respond(
    key: &PrivateKey,
    cert: &Certificate,
    chain: &[Certificate],
    alg: DigestAlg,
    query: &TimestampQuery,
    policy: &str,
    gen_time: Time,
    serial: u64,
) -> Result<Vec<u8>, SignError> {
    let mut info = vec![
        der::int(1),
        der::try_oid(policy)?,
        der::seq(&[&query.digest.algorithm(), &der::octets(&query.imprint)]),
        der::int(serial),
        gen_time.encode(),
    ];
    if let Some(nonce) = query.nonce {
        info.push(der::int(nonce));
    }
    let refs: Vec<&[u8]> = info.iter().map(Vec::as_slice).collect();
    let tst_info = der::seq(&refs);
    let token = crate::cms::sign_encapsulated(key, cert, chain, alg, TST_INFO, &tst_info)?;
    let status = der::seq(&[&der::int(0)]);
    Ok(der::seq(&[&status, &token]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_with_options() {
        let imprint = DigestAlg::Sha256.digest(&[b"document"]);
        let mut q = TimestampQuery::new(DigestAlg::Sha256, imprint.clone()).unwrap();
        q.policy = Some("1.2.3.4".into());
        q.nonce = Some(42);
        let encoded = q.encode().unwrap();
        let parsed = parse_request(&encoded).unwrap();
        assert_eq!(parsed, q);
        // The encoded fields themselves.
        let fields = Tlv::parse_all(&encoded).unwrap().children().unwrap();
        assert_eq!(fields[0].u64().unwrap(), 1);
        assert_eq!(fields[1].children().unwrap()[1].value, imprint.as_slice());
        assert_eq!(fields[2].oid().unwrap(), "1.2.3.4");
        assert_eq!(fields[3].u64().unwrap(), 42);
        assert_eq!(fields[4].value, &[0xff]);
    }

    #[test]
    fn rejects_oversized_response_before_parsing() {
        let q = TimestampQuery::new(DigestAlg::Sha256, vec![0; 32]).unwrap();
        assert!(parse_response(&vec![0; MAX_RESPONSE + 1], &q).is_err());
        assert!(parse_request(&vec![0; MAX_REQUEST + 1]).is_err());
    }
}
