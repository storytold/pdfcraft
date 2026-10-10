//! ECDSA verification on brainpoolP512r1 (RFC 5639), for which no RustCrypto curve crate exists.
//!
//! Verification only, on public data, so plain affine arithmetic over `crypto-bigint` is enough:
//! no secret is involved, and a verification takes a few milliseconds. Checked against
//! signatures made by OpenSSL (`tests/crypto.rs`).

use rsa::BoxedUint;

const P: &str = "aadd9db8dbe9c48b3fd4e6ae33c9fc07cb308db3b3c9d20ed6639cca703308717d4d9b009bc66842aecda12ae6a380e62881ff2f2d82c68528aa6056583a48f3";
const A: &str = "7830a3318b603b89e2327145ac234cc594cbdd8d3df91610a83441caea9863bc2ded5d5aa8253aa10a2ef1c98b9ac8b57f1117a72bf2c7b9e7c1ac4d77fc94ca";
const B: &str = "3df91610a83441caea9863bc2ded5d5aa8253aa10a2ef1c98b9ac8b57f1117a72bf2c7b9e7c1ac4d77fc94cadc083e67984050b75ebae5dd2809bd638016f723";
const GX: &str = "81aee4bdd82ed9645a21322e9c4c6a9385ed9f70b5d916c1b43b62eef4d0098eff3b1f78e2d0d48d50d1687b93b97d5f7c6d5047406a5e688b352209bcb9f822";
const GY: &str = "7dde385d566332ecc0eabfa9cf7822fdf209f70024a57b1aa000c55b881f8111b2dcde494a5f485e5bca4bd88a2763aed1ca2b2fa8f0540678cd1e0f3ad80892";
const N: &str = "aadd9db8dbe9c48b3fd4e6ae33c9fc07cb308db3b3c9d20ed6639cca70330870553e5c414ca92619418661197fac10471db1d381085ddaddb58796829ca90069";

const BITS: u32 = 512;
const BYTES: usize = 64;

type Point = Option<(BoxedUint, BoxedUint)>;

fn big(bytes: &[u8]) -> Option<BoxedUint> {
    let pad = BYTES.checked_sub(bytes.len())?;
    let mut v = vec![0u8; pad];
    v.extend_from_slice(bytes);
    BoxedUint::from_be_slice(&v, BITS).ok()
}

fn hex(s: &str) -> Option<BoxedUint> {
    let bytes: Option<Vec<u8>> = (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect();
    big(&bytes?)
}

/// Arithmetic modulo a prime.
struct Field {
    nz: crypto_bigint::NonZero<BoxedUint>,
}

impl Field {
    fn new(p: BoxedUint) -> Option<Field> {
        let nz = Option::from(crypto_bigint::NonZero::new(p))?;
        Some(Field { nz })
    }
    fn add(&self, a: &BoxedUint, b: &BoxedUint) -> BoxedUint {
        a.add_mod(b, &self.nz)
    }
    fn sub(&self, a: &BoxedUint, b: &BoxedUint) -> BoxedUint {
        a.sub_mod(b, &self.nz)
    }
    fn mul(&self, a: &BoxedUint, b: &BoxedUint) -> BoxedUint {
        a.mul_mod(b, &self.nz)
    }
    fn inv(&self, a: &BoxedUint) -> Option<BoxedUint> {
        Option::from(a.invert_mod(&self.nz))
    }
    /// Whether `v` is reduced (`v < p`). `add_mod`, `sub_mod` and friends assert it, and panic on
    /// anything else, so nothing from outside may reach them without this check.
    fn is_reduced(&self, v: &BoxedUint) -> bool {
        v < self.nz.as_ref()
    }
}

struct Curve {
    f: Field,
    a: BoxedUint,
    b: BoxedUint,
    g: (BoxedUint, BoxedUint),
    n: BoxedUint,
}

impl Curve {
    fn brainpool_p512r1() -> Option<Curve> {
        Some(Curve { f: Field::new(hex(P)?)?, a: hex(A)?, b: hex(B)?, g: (hex(GX)?, hex(GY)?), n: hex(N)? })
    }

    fn on_curve(&self, x: &BoxedUint, y: &BoxedUint) -> bool {
        let f = &self.f;
        let lhs = f.mul(y, y);
        let rhs = f.add(&f.add(&f.mul(&f.mul(x, x), x), &f.mul(&self.a, x)), &self.b);
        lhs == rhs
    }

    fn double(&self, p: &Point) -> Point {
        let (x, y) = p.as_ref()?;
        let f = &self.f;
        if bool::from(y.is_zero()) {
            return None;
        }
        let three = big(&[3])?;
        let two = big(&[2])?;
        let num = f.add(&f.mul(&three, &f.mul(x, x)), &self.a);
        let lambda = f.mul(&num, &f.inv(&f.mul(&two, y))?);
        let x3 = f.sub(&f.sub(&f.mul(&lambda, &lambda), x), x);
        let y3 = f.sub(&f.mul(&lambda, &f.sub(x, &x3)), y);
        Some((x3, y3))
    }

    fn add(&self, p: &Point, q: &Point) -> Point {
        let (Some((x1, y1)), Some((x2, y2))) = (p, q) else { return p.clone().or_else(|| q.clone()) };
        let f = &self.f;
        if x1 == x2 {
            return if y1 == y2 { self.double(p) } else { None };
        }
        let lambda = f.mul(&f.sub(y2, y1), &f.inv(&f.sub(x2, x1))?);
        let x3 = f.sub(&f.sub(&f.mul(&lambda, &lambda), x1), x2);
        let y3 = f.sub(&f.mul(&lambda, &f.sub(x1, &x3)), y1);
        Some((x3, y3))
    }

    /// `k · p`, double-and-add from the top bit.
    fn mul(&self, k: &BoxedUint, p: &Point) -> Point {
        let mut acc: Point = None;
        for i in (0..k.bits_vartime()).rev() {
            acc = self.double(&acc);
            if bool::from(k.bit(i)) {
                acc = self.add(&acc, p);
            }
        }
        acc
    }
}

/// Check the ECDSA signature `r ‖ s` (64 bytes each) of `digest` with the SEC1 uncompressed
/// point `public` (`04 ‖ x ‖ y`). `None`: the key is not a valid point or the input is malformed.
pub fn verify(public: &[u8], digest: &[u8], rs: &[u8]) -> Option<bool> {
    let c = Curve::brainpool_p512r1()?;
    let [0x04, xy @ ..] = public else { return None };
    if xy.len() != 2 * BYTES || rs.len() != 2 * BYTES {
        return None;
    }
    let (qx, qy) = (big(xy.get(..BYTES)?)?, big(xy.get(BYTES..)?)?);
    // SEC 1 §2.3.4: coordinates are field elements. The field arithmetic panics on unreduced ones.
    if !c.f.is_reduced(&qx) || !c.f.is_reduced(&qy) || !c.on_curve(&qx, &qy) {
        return None;
    }
    let (r, s) = (big(rs.get(..BYTES)?)?, big(rs.get(BYTES..)?)?);
    let n = crypto_bigint::NonZero::new(c.n.clone());
    let n: crypto_bigint::NonZero<BoxedUint> = Option::from(n)?;
    let zero = |v: &BoxedUint| bool::from(v.is_zero());
    if zero(&r) || zero(&s) || r >= c.n || s >= c.n {
        return Some(false);
    }
    // The leftmost 512 bits of the digest (all of it, for digests up to SHA-512).
    let e = big(digest.get(..digest.len().min(BYTES))?)?.rem_vartime(&n);
    let w: BoxedUint = Option::from(s.invert_mod(&n))?;
    let u1 = e.mul_mod(&w, &n);
    let u2 = r.mul_mod(&w, &n);
    let point = c.add(&c.mul(&u1, &Some(c.g.clone())), &c.mul(&u2, &Some((qx, qy))));
    // The point at infinity is no valid result (X9.62 §7.4.1 step 5): the signature is wrong.
    let Some((x, _)) = point else { return Some(false) };
    Some(x.rem_vartime(&n) == r)
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
