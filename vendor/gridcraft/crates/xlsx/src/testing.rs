//! Test support: builds password-encrypted workbooks in code, so tests here and in the crates
//! above (engine, UI) never need an encrypted file on disk. Compiled for this crate's tests and,
//! elsewhere, only with the `test-support` feature (a dev-dependency feature, never shipped).
//! The encryption is `craft-offcrypto`'s and the compound file `craft-cfb`'s.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

/// The spin count `encrypt_workbook` uses: Excel's default.
pub const SPIN_COUNT: u32 = 100_000;

/// `package` (normally an `.xlsx` zip) as Excel stores it with a password: Agile encryption
/// (AES-256, SHA-512) inside a compound file.
pub fn encrypt_workbook(package: &[u8], password: &str) -> Vec<u8> {
    craft_offcrypto::encrypt_with_spin_count(package, password, SPIN_COUNT).unwrap()
}

/// The same with Standard encryption (AES-128, SHA-1), as Excel 2007 wrote it.
pub fn encrypt_workbook_standard(package: &[u8], password: &str) -> Vec<u8> {
    craft_offcrypto::encrypt_standard(package, password, 128).unwrap()
}

/// A version 3 compound file (512-byte sectors) holding `streams` under the root: streams under
/// 4096 bytes in the mini stream, larger ones in regular sectors.
pub fn compound_file(streams: &[(&str, &[u8])]) -> Vec<u8> {
    craft_cfb::write_streams(craft_cfb::Version::V3, streams).unwrap()
}
