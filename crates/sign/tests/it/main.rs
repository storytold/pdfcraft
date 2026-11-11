//! The crate's integration tests, linked into one test binary.

mod algorithms;
mod ber;
mod chain;
mod cms;
mod crypto;
mod encrypted_signing;
mod pdf;

#[cfg(target_os = "linux")]
mod pkcs11;
mod revision_classification;
mod signed_structure;
mod store_keys;
