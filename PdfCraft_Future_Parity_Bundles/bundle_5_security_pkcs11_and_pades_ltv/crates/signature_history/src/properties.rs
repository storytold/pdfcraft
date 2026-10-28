//! Signature Validity Properties and Cryptographic Summary.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureIntegrity {
    ValidUnmodified,
    ModifiedSinceSigned,
    CorruptedDigest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateTrustStatus {
    Trusted,
    Revoked,
    Expired,
    SelfSignedUntrusted,
}

#[derive(Debug, Clone)]
pub struct SignaturePropertiesSummary {
    pub signer_name: String,
    pub signing_time: String,
    pub sub_filter: String, // e.g. "adbe.pkcs7.detached" or "ETSI.CAdES.detached"
    pub integrity: SignatureIntegrity,
    pub cert_status: CertificateTrustStatus,
    pub is_ltv_enabled: bool,
    pub has_rfc3161_timestamp: bool,
}

impl SignaturePropertiesSummary {
    pub fn is_fully_valid(&self) -> bool {
        self.integrity == SignatureIntegrity::ValidUnmodified
            && self.cert_status == CertificateTrustStatus::Trusted
    }
}
