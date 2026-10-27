//! The crate's integration tests, linked into one test binary.

mod algorithms;
mod ber;
mod chain;
mod cms;
mod crypto;
mod encrypted_signing;
mod pdf;

mod revision_classification;
mod signed_structure;
#[cfg(target_os = "linux")]
mod pkcs11;
 (feat(sign): replace the hand-rolled PKCS #11 FFI with the cryptoki wrapper)
mod store_keys;
