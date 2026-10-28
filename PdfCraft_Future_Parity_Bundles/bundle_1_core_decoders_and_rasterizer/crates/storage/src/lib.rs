//! PdfCraft Storage: Bounded-memory lazy loading, LRU object caching,
//! /EFF attachment encryption, and deterministic document saving.
//!
//! Subsystem covering Milestone M1:
//! - [P0] Lazy loading of very large (GB) files within a memory budget
//! - [P1] Attachment-only encryption (/EFF)
//! - [P2] Deterministic full-save mode

pub mod lru_cache;
pub mod lazy_reader;
pub mod eff_encryption;
pub mod deterministic_save;

pub use lru_cache::{BoundedLruCache, SizedObject};
pub use lazy_reader::{BoundedDocumentReader, RawPdfObject, StorageError, XrefEntry};
pub use eff_encryption::{AttachmentDecryptor, CryptFilterConfig, DocumentSecurityPolicy, EncryptionError};
pub use deterministic_save::DeterministicPdfWriter;
