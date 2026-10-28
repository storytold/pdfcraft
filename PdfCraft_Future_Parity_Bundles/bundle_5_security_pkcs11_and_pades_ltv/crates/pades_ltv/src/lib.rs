//! PdfCraft PAdES LTV: Long-Term Validation, /DSS Document Security Store, and RFC 3161 timestamps.
//!
//! Subsystem covering Milestone M9:
//! - [P1] Add a document timestamp (RFC 3161)
//! - [P1] Long-term validation (DSS/VRI)
//! - [P1] PAdES B-LT and B-LTA conformance
//! - [P1] Revocation checking (OCSP and CRL)

pub mod timestamp;
pub mod revocation;
pub mod dss_store;

pub use timestamp::{Rfc3161Client, TimeStampToken};
pub use revocation::{CertRevocationStatus, CrlRecord, OcspResponseRecord};
pub use dss_store::DocumentSecurityStoreBuilder;
