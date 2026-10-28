//! PdfCraft Public-Key Cryptography: Certificate encryption and recipient envelopes.
//!
//! Subsystem covering Milestone M8:
//! - [P1] Open certificate-encrypted (public-key handler) documents
//! - [P1] Encrypt with certificates for recipients
//! - [P1] Per-recipient permissions for certificate security

pub mod envelope;
pub mod permissions;
pub mod handler;

pub use envelope::{CmsEnvelopedDataParser, RecipientInfo};
pub use permissions::{RecipientPermissions, PERM_PRINT_HIGH_RES, PERM_COPY_TEXT, PERM_MODIFY_CONTENTS, PERM_FILL_FORMS};
pub use handler::PublicKeySecurityHandler;
