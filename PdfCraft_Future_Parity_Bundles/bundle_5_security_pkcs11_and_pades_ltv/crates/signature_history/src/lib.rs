//! PdfCraft Signature History: Multi-revision rollback inspection, signature properties, and FieldMDP locks.
//!
//! Subsystem covering Milestone M9:
//! - [P0] Signatures panel with per-revision tree
//! - [P0] Signature properties (validity summary, signer, time)
//! - [P1] Field locks (FieldMDP)

pub mod revision_tree;
pub mod properties;
pub mod locks;

pub use revision_tree::{DocumentRevision, RevisionHistoryScanner};
pub use properties::{CertificateTrustStatus, SignatureIntegrity, SignaturePropertiesSummary};
pub use locks::{FieldLockDefinition, LockAction};
