//! Password-protected presentations. PowerPoint wraps an encrypted `.pptx` in a Compound File
//! Binary (CFB/OLE2) container holding `EncryptionInfo` and `EncryptedPackage` streams
//! (ECMA-376 / [MS-OFFCRYPTO]) instead of a zip. The container's directory is read by the shared
//! `craft-cfb` crate, which bounds every chain and falls back to a UTF-16 name search when a
//! damaged file's directory cannot be followed.

/// Stream names that mark an encrypted (password-protected) OOXML package.
const ENCRYPTED_STREAMS: [&str; 2] = ["EncryptedPackage", "EncryptionInfo"];

/// Directory entries read at most: 1024 directory sectors of 512 bytes, as before the move to
/// `craft-cfb`, so a hostile directory chain stays cheap.
const MAX_DIR_ENTRIES: usize = 4096;

fn limits() -> craft_cfb::Limits {
    craft_cfb::Limits { max_dir_entries: MAX_DIR_ENTRIES, ..craft_cfb::Limits::default() }
}

/// Whether `bytes` is a compound file holding an encryption stream.
pub(crate) fn is_encrypted(bytes: &[u8]) -> bool {
    let limits = limits();
    ENCRYPTED_STREAMS.iter().any(|name| craft_cfb::has_entry_named(bytes, name, &limits))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use craft_cfb::{ENDOFCHAIN as END_OF_CHAIN, SIGNATURE};

    /// A minimal valid compound file (512-byte sectors: FAT at sector 0, directory at sector 1)
    /// whose directory holds a root entry plus the given stream names.
    fn build(streams: &[&str]) -> Vec<u8> {
        let mut b = vec![0u8; 512 * 3];
        b[..8].copy_from_slice(&SIGNATURE);
        b[24..26].copy_from_slice(&0x3Eu16.to_le_bytes()); // minor version
        b[26..28].copy_from_slice(&3u16.to_le_bytes()); // major version
        b[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes()); // byte order
        b[30..32].copy_from_slice(&9u16.to_le_bytes()); // sector shift
        b[32..34].copy_from_slice(&6u16.to_le_bytes()); // mini sector shift
        b[44..48].copy_from_slice(&1u32.to_le_bytes()); // FAT sectors
        b[48..52].copy_from_slice(&1u32.to_le_bytes()); // first directory sector
        b[60..64].copy_from_slice(&END_OF_CHAIN.to_le_bytes()); // no mini FAT
        b[68..72].copy_from_slice(&END_OF_CHAIN.to_le_bytes()); // no DIFAT sectors
        for i in 0..109 {
            b[76 + i * 4..80 + i * 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        }
        b[76..80].copy_from_slice(&0u32.to_le_bytes()); // FAT lives in sector 0
        // FAT (sector 0, at byte 512): sector 0 is a FAT sector, sector 1 ends its chain.
        for i in 0..128 {
            b[512 + i * 4..516 + i * 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        }
        b[512..516].copy_from_slice(&0xFFFF_FFFDu32.to_le_bytes());
        b[516..520].copy_from_slice(&END_OF_CHAIN.to_le_bytes());
        // Directory (sector 1, at byte 1024).
        for (k, name) in std::iter::once("Root Entry").chain(streams.iter().copied()).take(4).enumerate() {
            let e = 1024 + k * 128;
            let units: Vec<u16> = name.encode_utf16().take(31).collect();
            for (i, u) in units.iter().enumerate() {
                b[e + i * 2..e + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
            }
            b[e + 64..e + 66].copy_from_slice(&((units.len() as u16 + 1) * 2).to_le_bytes());
            b[e + 66] = if k == 0 { 5 } else { 2 };
        }
        b
    }

    #[test]
    fn reads_directory_names() {
        let b = build(&["EncryptionInfo", "EncryptedPackage"]);
        assert_eq!(craft_cfb::directory_names(&b, &limits()), Some(vec!["Root Entry".into(), "EncryptionInfo".into(), "EncryptedPackage".into()]));
        assert!(is_encrypted(&b));
        // A PowerPoint 97-2003 .ppt is a compound file too, without the encryption streams.
        assert!(!is_encrypted(&build(&["PowerPoint Document", "Current User"])));
        assert!(!is_encrypted(b"PK\x03\x04"));
    }

    #[test]
    fn hostile_headers_never_panic() {
        let mut b = build(&["PowerPoint Document"]);
        for at in [30usize, 48, 68, 76, 512, 516, 1024 + 64] {
            for v in [0u32, 1, 7, 0xFFFF_FFFA, 0xFFFF_FFFF, 0x7FFF_FFFF] {
                let mut c = b.clone();
                c[at..at + 4].copy_from_slice(&v.to_le_bytes());
                let _ = is_encrypted(&c);
                for cut in [0usize, 8, 100, 600, 1100] {
                    let _ = is_encrypted(&c[..cut.min(c.len())]);
                }
            }
        }
        // A directory chain that loops back on itself stops at the bound.
        b[516..520].copy_from_slice(&1u32.to_le_bytes());
        assert!(!is_encrypted(&b));
    }

    /// #346: opening a password-protected presentation says so instead of failing as an unknown
    /// file.
    #[test]
    fn a_password_protected_presentation_is_named_as_such() {
        let locked = build(&["EncryptionInfo", "EncryptedPackage"]);
        let err = deckcraft_engine::cmd::file::open_presentation("Locked.pptx", &locked).unwrap_err().to_string();
        assert!(err.contains("Locked.pptx is password-protected"), "{err}");
        let ppt = build(&["PowerPoint Document", "Current User"]);
        let err = deckcraft_engine::cmd::file::open_presentation("Old.ppt", &ppt).unwrap_err().to_string();
        assert!(!err.contains("password"), "{err}");
    }
}
