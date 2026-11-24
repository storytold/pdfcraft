//! Excel-compatible password hashes for protection (ECMA-376 Part 1 §18.2.29 and §18.3.1.66
//! attribute sets): the legacy 16-bit hash (`password="CC1A"`) and the iterated, salted hash
//! (`algorithmName`, `hashValue`, `saltValue`, `spinCount`). Protection is a convenience, not
//! security, but verification really checks the password: an unsupported variant is refused,
//! never accepted.

use serde::{Deserialize, Serialize};
use sha2::Digest;

/// Excel's default spin count for new passwords.
pub const DEFAULT_SPIN_COUNT: u32 = 100_000;
/// Largest spin count accepted when verifying (the documented maximum); a file asking for more
/// would only make verification hang.
pub const MAX_SPIN_COUNT: u32 = 10_000_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PasswordHash {
    /// The legacy 16-bit hash.
    Legacy(u16),
    /// An iterated, salted hash as stored in the file: algorithm name, base64 hash and salt.
    Iterated { algorithm: String, hash: String, salt: String, spin_count: u32 },
}

impl PasswordHash {
    /// A SHA-512 hash with Excel's default spin count, for a new password.
    pub fn new_sha512(password: &str, salt: &[u8]) -> PasswordHash {
        let hash = iterated::<sha2::Sha512>(password, salt, DEFAULT_SPIN_COUNT);
        PasswordHash::Iterated {
            algorithm: "SHA-512".into(),
            hash: crate::b64::encode(&hash),
            salt: crate::b64::encode(salt),
            spin_count: DEFAULT_SPIN_COUNT,
        }
    }

    /// Whether `password` matches. `Err` when the hash can't be checked (unsupported algorithm,
    /// broken base64, spin count over [`MAX_SPIN_COUNT`]).
    pub fn verify(&self, password: &str) -> Result<bool, String> {
        match self {
            PasswordHash::Legacy(h) => Ok(legacy_hash(password) == *h),
            PasswordHash::Iterated { algorithm, hash, salt, spin_count } => {
                if algorithm.is_empty() {
                    return Err("the stored password hash is damaged".into());
                }
                if *spin_count > MAX_SPIN_COUNT {
                    return Err(format!("the password uses a spin count of {spin_count}, more than the {MAX_SPIN_COUNT} allowed"));
                }
                let (Some(want), Some(salt)) = (crate::b64::decode(hash), crate::b64::decode(salt)) else {
                    return Err("the stored password hash is damaged".into());
                };
                let got = match algorithm.to_ascii_uppercase().replace('_', "-").as_str() {
                    "SHA-512" => iterated::<sha2::Sha512>(password, &salt, *spin_count),
                    "SHA-384" => iterated::<sha2::Sha384>(password, &salt, *spin_count),
                    "SHA-256" => iterated::<sha2::Sha256>(password, &salt, *spin_count),
                    _ => return Err(format!("passwords hashed with {algorithm} aren't supported")),
                };
                Ok(got == want)
            }
        }
    }
}

/// A 16-byte salt derived from `seed` (callers mix in what varies: names, a counter, the clock).
pub fn salt_from(seed: &[u8]) -> [u8; 16] {
    let h = sha2::Sha512::digest(seed);
    let mut out = [0u8; 16];
    for (o, b) in out.iter_mut().zip(h.iter()) {
        *o = *b;
    }
    out
}

/// The legacy 16-bit password hash: each UTF-16 unit contributes its low byte (its high byte
/// when the low one is zero), folded in from the last character with a 15-bit rotation, then
/// the length and the constant `0xCE4B`.
pub fn legacy_hash(password: &str) -> u16 {
    let bytes: Vec<u16> = password.encode_utf16().map(|u| if u & 0xFF != 0 { u & 0xFF } else { u >> 8 }).collect();
    let rotate = |v: u16| ((v >> 14) & 1) | ((v << 1) & 0x7FFF);
    let mut h: u16 = 0;
    for b in bytes.iter().rev() {
        h = rotate(h) ^ b;
    }
    h = rotate(h) ^ (bytes.len() as u16);
    h ^ 0xCE4B
}

/// H0 = H(salt + UTF-16LE password); Hn = H(Hn-1 + n as 32-bit little-endian), `spin` times.
fn iterated<D: Digest>(password: &str, salt: &[u8], spin: u32) -> Vec<u8> {
    let mut d = D::new();
    d.update(salt);
    for u in password.encode_utf16() {
        d.update(u.to_le_bytes());
    }
    let mut h = d.finalize().to_vec();
    for i in 0..spin {
        let mut d = D::new();
        d.update(&h);
        d.update(i.to_le_bytes());
        h = d.finalize().to_vec();
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_hash_matches_the_published_example() -> Result<(), Box<dyn std::error::Error>> {
        // The hash Excel writes for the password "password" (`password="83AF"`).
        assert_eq!(legacy_hash("password"), 0x83AF);
        assert!(PasswordHash::Legacy(0x83AF).verify("password")?);
        assert!(!PasswordHash::Legacy(0x83AF).verify("Password")?);
        assert_eq!(legacy_hash(""), 0xCE4B);
        Ok(())
    }

    #[test]
    fn iterated_hash_verifies_and_refuses_what_it_cannot_check() -> Result<(), Box<dyn std::error::Error>> {
        let h = PasswordHash::new_sha512("s3cret", b"0123456789abcdef");
        assert!(h.verify("s3cret")?);
        assert!(!h.verify("s3cret ")?);
        assert!(matches!(&h, PasswordHash::Iterated { algorithm, spin_count: DEFAULT_SPIN_COUNT, .. } if algorithm == "SHA-512"));
        let PasswordHash::Iterated { hash, salt, .. } = h else { return Err("not an iterated hash".into()) };
        let other =
            |algorithm: &str, spin_count| PasswordHash::Iterated { algorithm: algorithm.into(), hash: hash.clone(), salt: salt.clone(), spin_count };
        assert!(other("SHA-1", 10).verify("s3cret").is_err());
        assert!(other("MD5", 10).verify("s3cret").is_err());
        assert!(other("SHA-512", MAX_SPIN_COUNT + 1).verify("s3cret").is_err());
        assert!(!other("SHA-256", 2).verify("s3cret")?);
        let broken = PasswordHash::Iterated { algorithm: "SHA-512".into(), hash: "!!".into(), salt: salt.clone(), spin_count: 1 };
        assert!(broken.verify("s3cret").is_err());
        Ok(())
    }
}
