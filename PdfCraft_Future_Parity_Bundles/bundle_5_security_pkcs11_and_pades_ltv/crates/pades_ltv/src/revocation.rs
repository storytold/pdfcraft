//! Revocation Evidence Status (OCSP RFC 6960 and CRL RFC 5280).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertRevocationStatus {
    Good,
    Revoked { epoch_time: u64, reason: u8 },
    Unknown,
}

#[derive(Debug, Clone)]
pub struct OcspResponseRecord {
    pub cert_hash: [u8; 32],
    pub status: CertRevocationStatus,
    pub this_update: u64,
    pub next_update: Option<u64>,
    pub der_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CrlRecord {
    pub issuer: String,
    pub this_update: u64,
    pub next_update: u64,
    pub der_bytes: Vec<u8>,
}
