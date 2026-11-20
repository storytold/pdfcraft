//! Regression fixtures for the shared brainpool verification route (test-only).
use crate::keys::{DigestAlg, PublicKey, Scheme};
const P: &str = "aadd9db8dbe9c48b3fd4e6ae33c9fc07cb308db3b3c9d20ed6639cca703308717d4d9b009bc66842aecda12ae6a380e62881ff2f2d82c68528aa6056583a48f3";
const GX: &str = "81aee4bdd82ed9645a21322e9c4c6a9385ed9f70b5d916c1b43b62eef4d0098eff3b1f78e2d0d48d50d1687b93b97d5f7c6d5047406a5e688b352209bcb9f822";
const GY: &str = "7dde385d566332ecc0eabfa9cf7822fdf209f70024a57b1aa000c55b881f8111b2dcde494a5f485e5bca4bd88a2763aed1ca2b2fa8f0540678cd1e0f3ad80892";
const N: &str = "aadd9db8dbe9c48b3fd4e6ae33c9fc07cb308db3b3c9d20ed6639cca70330870553e5c414ca92619418661197fac10471db1d381085ddaddb58796829ca90069";
fn verify(public: &[u8], digest: &[u8], rs: &[u8]) -> Option<bool> {
    PublicKey::BrainpoolP512(public.to_vec()).verify(Scheme::Ecdsa, DigestAlg::Sha512, digest, rs).ok()
}
#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> [u8; 64] {
        let v: Vec<u8> = (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0)).collect();
        let mut out = [0u8; 64];
        out[64 - v.len()..].copy_from_slice(&v);
        out
    }

    /// Never-crash: public keys and signature values of any size are refused, not panicked on.
    #[test]
    fn hostile_keys_and_signatures_are_refused() {
        let digest = [7u8; 64];
        let key = |x: &[u8; 64], y: &[u8; 64]| [&[4u8][..], x, y].concat();
        let (p, n, gx, gy) = (unhex(P), unhex(N), unhex(GX), unhex(GY));
        // Coordinates that are not field elements, or not on the curve.
        for (x, y) in [([0xFF; 64], [0xFF; 64]), (p, [0; 64]), ([0; 64], p), ([0; 64], [0; 64]), (p, p)] {
            assert_eq!(verify(&key(&x, &y), &digest, &[1u8; 128]), None);
        }
        // The generator, a real point, with an r or s that is zero, n, or all ones.
        for bad in [[0u8; 64], n, [0xFF; 64]] {
            assert_eq!(verify(&key(&gx, &gy), &digest, &[bad.to_vec(), vec![1; 64]].concat()), Some(false));
            assert_eq!(verify(&key(&gx, &gy), &digest, &[vec![1; 64], bad.to_vec()].concat()), Some(false));
        }
    }
}
