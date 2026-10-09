//! Digests, public keys (verification everywhere) and private keys (signing).
//!
//! Verification uses RustCrypto. RSA private-key operations use `aws-lc-rs` on native targets
//! (constant-time; ADR-0009: the `rsa` crate is never used for private keys there). ECDSA
//! signing uses RustCrypto with deterministic nonces (RFC 6979).

use crate::SignError;
use crate::der::{self, Tlv, tag};

pub mod oid {
    pub const SHA1: &str = "1.3.14.3.2.26";
    pub const SHA224: &str = "2.16.840.1.101.3.4.2.4";
    pub const SHA256: &str = "2.16.840.1.101.3.4.2.1";
    pub const SHA384: &str = "2.16.840.1.101.3.4.2.2";
    pub const SHA512: &str = "2.16.840.1.101.3.4.2.3";
    pub const SHA512_224: &str = "2.16.840.1.101.3.4.2.5";
    pub const SHA512_256: &str = "2.16.840.1.101.3.4.2.6";
    pub const SHA3_224: &str = "2.16.840.1.101.3.4.2.7";
    pub const SHA3_256: &str = "2.16.840.1.101.3.4.2.8";
    pub const SHA3_384: &str = "2.16.840.1.101.3.4.2.9";
    pub const SHA3_512: &str = "2.16.840.1.101.3.4.2.10";
    pub const RIPEMD160: &str = "1.3.36.3.2.1";
    pub const RSA: &str = "1.2.840.113549.1.1.1";
    pub const RSA_PSS: &str = "1.2.840.113549.1.1.10";
    pub const MGF1: &str = "1.2.840.113549.1.1.8";
    pub const EC: &str = "1.2.840.10045.2.1";
    pub const P256: &str = "1.2.840.10045.3.1.7";
    pub const P384: &str = "1.3.132.0.34";
    pub const P521: &str = "1.3.132.0.35";
    pub const BRAINPOOL_P256: &str = "1.3.36.3.3.2.8.1.1.7";
    pub const BRAINPOOL_P384: &str = "1.3.36.3.3.2.8.1.1.11";
    pub const BRAINPOOL_P512: &str = "1.3.36.3.3.2.8.1.1.13";
    pub const ED25519: &str = "1.3.101.112";
}

/// Message digests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DigestAlg {
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
    Sha512_224,
    Sha512_256,
    Sha3_224,
    Sha3_256,
    Sha3_384,
    Sha3_512,
    Ripemd160,
}

impl DigestAlg {
    /// Every digest, for tables.
    pub const ALL: [DigestAlg; 12] = [
        DigestAlg::Sha1,
        DigestAlg::Sha224,
        DigestAlg::Sha256,
        DigestAlg::Sha384,
        DigestAlg::Sha512,
        DigestAlg::Sha512_224,
        DigestAlg::Sha512_256,
        DigestAlg::Sha3_224,
        DigestAlg::Sha3_256,
        DigestAlg::Sha3_384,
        DigestAlg::Sha3_512,
        DigestAlg::Ripemd160,
    ];

    pub fn from_oid(o: &str) -> Option<DigestAlg> {
        DigestAlg::ALL.into_iter().find(|d| d.oid() == o)
    }

    pub fn oid(self) -> &'static str {
        match self {
            DigestAlg::Sha1 => oid::SHA1,
            DigestAlg::Sha224 => oid::SHA224,
            DigestAlg::Sha256 => oid::SHA256,
            DigestAlg::Sha384 => oid::SHA384,
            DigestAlg::Sha512 => oid::SHA512,
            DigestAlg::Sha512_224 => oid::SHA512_224,
            DigestAlg::Sha512_256 => oid::SHA512_256,
            DigestAlg::Sha3_224 => oid::SHA3_224,
            DigestAlg::Sha3_256 => oid::SHA3_256,
            DigestAlg::Sha3_384 => oid::SHA3_384,
            DigestAlg::Sha3_512 => oid::SHA3_512,
            DigestAlg::Ripemd160 => oid::RIPEMD160,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DigestAlg::Sha1 => "SHA-1",
            DigestAlg::Sha224 => "SHA-224",
            DigestAlg::Sha256 => "SHA-256",
            DigestAlg::Sha384 => "SHA-384",
            DigestAlg::Sha512 => "SHA-512",
            DigestAlg::Sha512_224 => "SHA-512/224",
            DigestAlg::Sha512_256 => "SHA-512/256",
            DigestAlg::Sha3_224 => "SHA3-224",
            DigestAlg::Sha3_256 => "SHA3-256",
            DigestAlg::Sha3_384 => "SHA3-384",
            DigestAlg::Sha3_512 => "SHA3-512",
            DigestAlg::Ripemd160 => "RIPEMD-160",
        }
    }

    /// The digest of the concatenation of `parts`.
    pub fn digest(self, parts: &[&[u8]]) -> Vec<u8> {
        fn sha2_run<D: sha2::Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut h = D::new();
            for p in parts {
                h.update(p);
            }
            h.finalize().to_vec()
        }
        fn sha3_run<D: sha3::Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut h = D::new();
            for p in parts {
                h.update(p);
            }
            h.finalize().to_vec()
        }
        match self {
            DigestAlg::Sha1 => sha2_run::<sha1::Sha1>(parts),
            DigestAlg::Sha224 => sha2_run::<sha2::Sha224>(parts),
            DigestAlg::Sha256 => sha2_run::<sha2::Sha256>(parts),
            DigestAlg::Sha384 => sha2_run::<sha2::Sha384>(parts),
            DigestAlg::Sha512 => sha2_run::<sha2::Sha512>(parts),
            DigestAlg::Sha512_224 => sha2_run::<sha2::Sha512_224>(parts),
            DigestAlg::Sha512_256 => sha2_run::<sha2::Sha512_256>(parts),
            DigestAlg::Sha3_224 => sha3_run::<sha3::Sha3_224>(parts),
            DigestAlg::Sha3_256 => sha3_run::<sha3::Sha3_256>(parts),
            DigestAlg::Sha3_384 => sha3_run::<sha3::Sha3_384>(parts),
            DigestAlg::Sha3_512 => sha3_run::<sha3::Sha3_512>(parts),
            DigestAlg::Ripemd160 => {
                use ripemd::Digest;
                let mut h = ripemd::Ripemd160::new();
                for p in parts {
                    h.update(p);
                }
                h.finalize().to_vec()
            }
        }
    }

    /// What to tell the user when a signature that still validates uses this digest, or `None`
    /// for one that is fine. Old documents carry SHA-1 and RIPEMD-160, so they validate, but a
    /// signature panel that showed them like a SHA-256 one would overstate what they prove.
    pub fn weakness(self) -> Option<&'static str> {
        match self {
            DigestAlg::Sha1 => Some(
                "This signature uses SHA-1, a weak hash algorithm: collisions in it have been demonstrated. It still validates because many documents signed years ago use it, but it should not be relied on.",
            ),
            DigestAlg::Ripemd160 => Some(
                "This signature uses RIPEMD-160, a legacy 160-bit hash algorithm that is no longer recommended for signatures. It still validates, but it should not be relied on.",
            ),
            _ => None,
        }
    }

    /// The digest length in bytes.
    pub fn output_len(self) -> usize {
        match self {
            DigestAlg::Sha1 | DigestAlg::Ripemd160 => 20,
            DigestAlg::Sha224 | DigestAlg::Sha512_224 | DigestAlg::Sha3_224 => 28,
            DigestAlg::Sha256 | DigestAlg::Sha512_256 | DigestAlg::Sha3_256 => 32,
            DigestAlg::Sha384 | DigestAlg::Sha3_384 => 48,
            DigestAlg::Sha512 | DigestAlg::Sha3_512 => 64,
        }
    }

    /// The `AlgorithmIdentifier` (no parameters, as RFC 5754 recommends).
    pub fn algorithm(self) -> Vec<u8> {
        der::algorithm(self.oid(), None)
    }
}

/// RSASSA-PSS parameters (RFC 4055 §3.1) besides the message digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PssParams {
    /// The MGF1 hash.
    pub mgf: DigestAlg,
    /// The declared salt length; `None` recovers it from the signature.
    pub salt_len: Option<usize>,
}

/// How a signature value is computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    RsaPkcs1,
    RsaPss(PssParams),
    /// ECDSA over a digest, the signature DER or plain `r ‖ s`.
    Ecdsa,
    /// EdDSA over the message itself (RFC 8419), not over a digest.
    Ed25519,
}

/// Signature algorithm OIDs that name both the scheme and the digest.
const SIGNATURE_OIDS: &[(&str, Scheme, DigestAlg)] = &[
    ("1.2.840.113549.1.1.5", Scheme::RsaPkcs1, DigestAlg::Sha1),
    ("1.2.840.113549.1.1.14", Scheme::RsaPkcs1, DigestAlg::Sha224),
    ("1.2.840.113549.1.1.11", Scheme::RsaPkcs1, DigestAlg::Sha256),
    ("1.2.840.113549.1.1.12", Scheme::RsaPkcs1, DigestAlg::Sha384),
    ("1.2.840.113549.1.1.13", Scheme::RsaPkcs1, DigestAlg::Sha512),
    ("1.2.840.113549.1.1.15", Scheme::RsaPkcs1, DigestAlg::Sha512_224),
    ("1.2.840.113549.1.1.16", Scheme::RsaPkcs1, DigestAlg::Sha512_256),
    ("2.16.840.1.101.3.4.3.13", Scheme::RsaPkcs1, DigestAlg::Sha3_224),
    ("2.16.840.1.101.3.4.3.14", Scheme::RsaPkcs1, DigestAlg::Sha3_256),
    ("2.16.840.1.101.3.4.3.15", Scheme::RsaPkcs1, DigestAlg::Sha3_384),
    ("2.16.840.1.101.3.4.3.16", Scheme::RsaPkcs1, DigestAlg::Sha3_512),
    ("1.3.36.3.3.1.2", Scheme::RsaPkcs1, DigestAlg::Ripemd160),
    ("1.2.840.10045.4.1", Scheme::Ecdsa, DigestAlg::Sha1),
    ("1.2.840.10045.4.3.1", Scheme::Ecdsa, DigestAlg::Sha224),
    ("1.2.840.10045.4.3.2", Scheme::Ecdsa, DigestAlg::Sha256),
    ("1.2.840.10045.4.3.3", Scheme::Ecdsa, DigestAlg::Sha384),
    ("1.2.840.10045.4.3.4", Scheme::Ecdsa, DigestAlg::Sha512),
    ("2.16.840.1.101.3.4.3.9", Scheme::Ecdsa, DigestAlg::Sha3_224),
    ("2.16.840.1.101.3.4.3.10", Scheme::Ecdsa, DigestAlg::Sha3_256),
    ("2.16.840.1.101.3.4.3.11", Scheme::Ecdsa, DigestAlg::Sha3_384),
    ("2.16.840.1.101.3.4.3.12", Scheme::Ecdsa, DigestAlg::Sha3_512),
    // BSI TR-03111 "plain" ECDSA (r ‖ s), as German signature cards write it.
    ("0.4.0.127.0.7.1.1.4.1.1", Scheme::Ecdsa, DigestAlg::Sha1),
    ("0.4.0.127.0.7.1.1.4.1.2", Scheme::Ecdsa, DigestAlg::Sha224),
    ("0.4.0.127.0.7.1.1.4.1.3", Scheme::Ecdsa, DigestAlg::Sha256),
    ("0.4.0.127.0.7.1.1.4.1.4", Scheme::Ecdsa, DigestAlg::Sha384),
    ("0.4.0.127.0.7.1.1.4.1.5", Scheme::Ecdsa, DigestAlg::Sha512),
    ("0.4.0.127.0.7.1.1.4.1.6", Scheme::Ecdsa, DigestAlg::Ripemd160),
];

/// A signature `AlgorithmIdentifier`: the scheme and, when the identifier names one, the digest.
pub fn signature_algorithm(alg: &Tlv<'_>) -> Result<(Scheme, Option<DigestAlg>), SignError> {
    let parts = alg.children()?;
    let o = parts.first().ok_or_else(|| SignError::Malformed("empty algorithm".into()))?.oid()?;
    if let Some((_, scheme, digest)) = SIGNATURE_OIDS.iter().find(|(k, _, _)| *k == o) {
        return Ok((*scheme, Some(*digest)));
    }
    Ok(match o.as_str() {
        oid::RSA => (Scheme::RsaPkcs1, None),
        oid::EC => (Scheme::Ecdsa, None),
        oid::ED25519 => (Scheme::Ed25519, None),
        oid::RSA_PSS => {
            let (hash, params) = pss_params(parts.get(1))?;
            (Scheme::RsaPss(params), Some(hash))
        }
        other => return Err(SignError::Unsupported(format!("signature algorithm {other}"))),
    })
}

/// `RSASSA-PSS-params` (RFC 4055): `[0]` hash (default SHA-1), `[1]` mask generation function
/// (default MGF1 with SHA-1), `[2]` salt length (default 20). Absent parameters mean the defaults.
fn pss_params(params: Option<&Tlv<'_>>) -> Result<(DigestAlg, PssParams), SignError> {
    let hash_of = |alg: &Tlv<'_>| -> Result<DigestAlg, SignError> {
        let o = alg.children()?.first().ok_or_else(|| SignError::Malformed("PSS hash".into()))?.oid()?;
        DigestAlg::from_oid(&o).ok_or_else(|| SignError::Unsupported(format!("digest {o}")))
    };
    let mut hash = DigestAlg::Sha1;
    let mut mgf = DigestAlg::Sha1;
    let mut salt_len = 20usize;
    if let Some(p) = params.filter(|p| p.tag == tag::SEQUENCE) {
        for field in p.children()? {
            match field.tag {
                t if t == tag::ctx(0) => hash = hash_of(&field.inner()?)?,
                t if t == tag::ctx(1) => {
                    let m = field.inner()?.children()?;
                    if m.first().map(|o| o.oid()).transpose()?.as_deref() != Some(oid::MGF1) {
                        return Err(SignError::Unsupported("PSS mask generation function other than MGF1".into()));
                    }
                    mgf = hash_of(m.get(1).ok_or_else(|| SignError::Malformed("MGF1 hash".into()))?)?;
                }
                t if t == tag::ctx(2) => {
                    salt_len = usize::try_from(field.inner()?.u64()?).map_err(|_| SignError::Malformed("PSS salt length".into()))?
                }
                _ => {}
            }
        }
    }
    Ok((hash, PssParams { mgf, salt_len: Some(salt_len) }))
}

/// A public key from a `SubjectPublicKeyInfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicKey {
    Rsa {
        n: Vec<u8>,
        e: Vec<u8>,
    },
    P256(Vec<u8>),
    P384(Vec<u8>),
    /// Verification only (no signing).
    P521(Vec<u8>),
    BrainpoolP256(Vec<u8>),
    BrainpoolP384(Vec<u8>),
    BrainpoolP512(Vec<u8>),
    /// The 32-byte public key (RFC 8032). Signs the message itself, see [`Scheme::Ed25519`].
    Ed25519(Vec<u8>),
    /// A key algorithm or curve PdfCraft can't check signatures with. The certificate still
    /// parses (so its name and dates show); verifying with it gives [`SignError::Unsupported`].
    Unsupported {
        what: String,
        spki: Vec<u8>,
    },
}

impl PublicKey {
    /// The `subjectPublicKey` BIT STRING contents — what OCSP's `issuerKeyHash` covers
    /// (RFC 6960 §4.1.1). RSA keys are re-encoded; EC points are the stored bytes.
    pub fn key_bits(&self) -> Vec<u8> {
        match self {
            PublicKey::Rsa { n, e } => der::seq(&[&der::uint(n), &der::uint(e)]),
            PublicKey::P256(bits)
            | PublicKey::P384(bits)
            | PublicKey::P521(bits)
            | PublicKey::BrainpoolP256(bits)
            | PublicKey::BrainpoolP384(bits)
            | PublicKey::BrainpoolP512(bits)
            | PublicKey::Ed25519(bits) => bits.clone(),
            // The BIT STRING of the stored SubjectPublicKeyInfo (empty if it doesn't parse).
            PublicKey::Unsupported { spki, .. } => Tlv::parse(spki)
                .ok()
                .and_then(|(t, _)| t.children().ok())
                .and_then(|c| c.get(1).and_then(|k| k.bits().ok().map(<[u8]>::to_vec)))
                .unwrap_or_default(),
        }
    }

    pub fn from_spki(spki: &Tlv<'_>) -> Result<PublicKey, SignError> {
        let parts = spki.children()?;
        let [alg, key] = parts.as_slice() else { return Err(SignError::Malformed("SubjectPublicKeyInfo".into())) };
        let alg = alg.children()?;
        let o = alg.first().ok_or_else(|| SignError::Malformed("key algorithm".into()))?.oid()?;
        let bits = key.bits()?;
        match o.as_str() {
            oid::ED25519 => Ok(PublicKey::Ed25519(bits.to_vec())),
            oid::RSA => {
                let k = Tlv::parse_all(bits)?.children()?;
                let [n, e] = k.as_slice() else { return Err(SignError::Malformed("RSAPublicKey".into())) };
                Ok(PublicKey::Rsa { n: n.uint_bytes().to_vec(), e: e.uint_bytes().to_vec() })
            }
            oid::EC => match alg.get(1).map(|c| c.oid()).transpose()?.as_deref() {
                Some(oid::P256) => Ok(PublicKey::P256(bits.to_vec())),
                Some(oid::P384) => Ok(PublicKey::P384(bits.to_vec())),
                Some(oid::P521) => Ok(PublicKey::P521(bits.to_vec())),
                Some(oid::BRAINPOOL_P256) => Ok(PublicKey::BrainpoolP256(bits.to_vec())),
                Some(oid::BRAINPOOL_P384) => Ok(PublicKey::BrainpoolP384(bits.to_vec())),
                Some(oid::BRAINPOOL_P512) => Ok(PublicKey::BrainpoolP512(bits.to_vec())),
                other => Ok(PublicKey::Unsupported { what: format!("elliptic curve {}", other.unwrap_or("?")), spki: spki.raw.to_vec() }),
            },
            other => Ok(PublicKey::Unsupported { what: format!("public key algorithm {other}"), spki: spki.raw.to_vec() }),
        }
    }

    /// The `SubjectPublicKeyInfo` encoding.
    pub fn spki(&self) -> Vec<u8> {
        match self {
            PublicKey::Rsa { n, e } => {
                let rsa = der::seq(&[&der::uint(n), &der::uint(e)]);
                der::seq(&[&der::algorithm(oid::RSA, Some(&der::null())), &der::bit_string(&rsa)])
            }
            PublicKey::P256(p) => der::seq(&[&der::seq(&[&der::oid(oid::EC), &der::oid(oid::P256)]), &der::bit_string(p)]),
            PublicKey::P384(p) => der::seq(&[&der::seq(&[&der::oid(oid::EC), &der::oid(oid::P384)]), &der::bit_string(p)]),
            PublicKey::P521(p) => der::seq(&[&der::seq(&[&der::oid(oid::EC), &der::oid(oid::P521)]), &der::bit_string(p)]),
            PublicKey::BrainpoolP256(p) => der::seq(&[&der::seq(&[&der::oid(oid::EC), &der::oid(oid::BRAINPOOL_P256)]), &der::bit_string(p)]),
            PublicKey::BrainpoolP384(p) => der::seq(&[&der::seq(&[&der::oid(oid::EC), &der::oid(oid::BRAINPOOL_P384)]), &der::bit_string(p)]),
            PublicKey::BrainpoolP512(p) => der::seq(&[&der::seq(&[&der::oid(oid::EC), &der::oid(oid::BRAINPOOL_P512)]), &der::bit_string(p)]),
            PublicKey::Ed25519(p) => der::seq(&[&der::seq(&[&der::oid(oid::ED25519)]), &der::bit_string(p)]),
            PublicKey::Unsupported { spki, .. } => spki.clone(),
        }
    }

    /// "RSA 2048-bit", "ECDSA P-256"…
    pub fn describe(&self) -> String {
        match self {
            PublicKey::Rsa { n, .. } => format!("RSA {}-bit", n.len() * 8),
            PublicKey::P256(_) => "ECDSA P-256".into(),
            PublicKey::P384(_) => "ECDSA P-384".into(),
            PublicKey::P521(_) => "ECDSA P-521".into(),
            PublicKey::BrainpoolP256(_) => "ECDSA brainpoolP256r1".into(),
            PublicKey::BrainpoolP384(_) => "ECDSA brainpoolP384r1".into(),
            PublicKey::BrainpoolP512(_) => "ECDSA brainpoolP512r1".into(),
            PublicKey::Ed25519(_) => "Ed25519".into(),
            PublicKey::Unsupported { what, .. } => format!("unsupported ({what})"),
        }
    }

    /// Check `sig` over a message whose digest (with `alg`) is `digest`. Encodings are read
    /// leniently (see [`crate::rsa_pad`], [`ecdsa_rs`]); the cryptographic check never is.
    /// `Err(Unsupported)` means "can't tell", as opposed to `Ok(false)`: "doesn't match".
    pub fn verify(&self, scheme: Scheme, alg: DigestAlg, digest: &[u8], sig: &[u8]) -> Result<bool, SignError> {
        self.verify_noting(scheme, alg, digest, sig, &mut Vec::new())
    }

    /// [`PublicKey::verify`], noting in `notes` each encoding irregularity it tolerated. Only
    /// meaningful when it returns `Ok(true)`: a signature that doesn't verify may leave notes
    /// about how far it got.
    pub fn verify_noting(&self, scheme: Scheme, alg: DigestAlg, digest: &[u8], sig: &[u8], notes: &mut Vec<&'static str>) -> Result<bool, SignError> {
        match (self, scheme) {
            (PublicKey::Unsupported { what, .. }, _) => Err(SignError::Unsupported(what.clone())),
            (PublicKey::Rsa { n, e }, Scheme::RsaPkcs1) => crate::rsa_pad::pkcs1_verify(n, e, alg, digest, sig, notes),
            (PublicKey::Rsa { n, e }, Scheme::RsaPss(params)) => crate::rsa_pad::pss_verify(n, e, alg, params, digest, sig, notes),
            (PublicKey::Ed25519(_), _) => Err(SignError::Unsupported("Ed25519 signs the message, not a digest".into())),
            (PublicKey::P256(p), Scheme::Ecdsa) => {
                use p256::ecdsa::signature::hazmat::PrehashVerifier;
                let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(p).map_err(|_| SignError::Malformed("P-256 key".into()))?;
                let Some(s) = ecdsa_rs(sig, 32, notes).and_then(|rs| p256::ecdsa::Signature::from_slice(&rs).ok()) else { return Ok(false) };
                Ok(key.verify_prehash(digest, &s).is_ok())
            }
            (PublicKey::P384(p), Scheme::Ecdsa) => {
                use p384::ecdsa::signature::hazmat::PrehashVerifier;
                let key = p384::ecdsa::VerifyingKey::from_sec1_bytes(p).map_err(|_| SignError::Malformed("P-384 key".into()))?;
                let Some(s) = ecdsa_rs(sig, 48, notes).and_then(|rs| p384::ecdsa::Signature::from_slice(&rs).ok()) else { return Ok(false) };
                Ok(key.verify_prehash(digest, &s).is_ok())
            }
            (PublicKey::P521(p), Scheme::Ecdsa) => {
                use p521::ecdsa::signature::hazmat::PrehashVerifier;
                let key = p521::ecdsa::VerifyingKey::from_sec1_bytes(p).map_err(|_| SignError::Malformed("P-521 key".into()))?;
                let Some(s) = ecdsa_rs(sig, 66, notes).and_then(|rs| p521::ecdsa::Signature::from_slice(&rs).ok()) else { return Ok(false) };
                Ok(key.verify_prehash(digest, &s).is_ok())
            }
            (PublicKey::BrainpoolP256(p), Scheme::Ecdsa) => {
                use ecdsa::signature::hazmat::PrehashVerifier;
                type Curve = bp256::BrainpoolP256r1;
                let key = ecdsa::VerifyingKey::<Curve>::from_sec1_bytes(p).map_err(|_| SignError::Malformed("brainpoolP256r1 key".into()))?;
                let Some(s) = ecdsa_rs(sig, 32, notes).and_then(|rs| ecdsa::Signature::<Curve>::from_slice(&rs).ok()) else { return Ok(false) };
                Ok(key.verify_prehash(digest, &s).is_ok())
            }
            (PublicKey::BrainpoolP384(p), Scheme::Ecdsa) => {
                use ecdsa::signature::hazmat::PrehashVerifier;
                type Curve = bp384::BrainpoolP384r1;
                let key = ecdsa::VerifyingKey::<Curve>::from_sec1_bytes(p).map_err(|_| SignError::Malformed("brainpoolP384r1 key".into()))?;
                let Some(s) = ecdsa_rs(sig, 48, notes).and_then(|rs| ecdsa::Signature::<Curve>::from_slice(&rs).ok()) else { return Ok(false) };
                Ok(key.verify_prehash(digest, &s).is_ok())
            }
            (PublicKey::BrainpoolP512(p), Scheme::Ecdsa) => {
                let Some(rs) = ecdsa_rs(sig, 64, notes) else { return Ok(false) };
                crate::ec512::verify(p, digest, &rs).ok_or_else(|| SignError::Malformed("brainpoolP512r1 key or signature".into()))
            }
            _ => Ok(false),
        }
    }

    /// Check `sig` over `msg` itself, for the schemes that don't sign a digest (Ed25519,
    /// RFC 8419: with signed attributes, the message is their DER encoding).
    pub fn verify_message(&self, scheme: Scheme, msg: &[u8], sig: &[u8]) -> Result<bool, SignError> {
        match (self, scheme) {
            (PublicKey::Unsupported { what, .. }, _) => Err(SignError::Unsupported(what.clone())),
            (PublicKey::Ed25519(k), Scheme::Ed25519) => {
                let bytes: [u8; 32] = k.as_slice().try_into().map_err(|_| SignError::Malformed("Ed25519 key".into()))?;
                let key = ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|_| SignError::Malformed("Ed25519 key".into()))?;
                let Ok(sig) = ed25519_dalek::Signature::from_slice(sig) else { return Ok(false) };
                Ok(key.verify_strict(msg, &sig).is_ok())
            }
            _ => Ok(false),
        }
    }
}

/// An ECDSA signature as the fixed-width `r ‖ s` the curve crates read: from the standard DER
/// `SEQUENCE { INTEGER r, INTEGER s }` (also with integers that have redundant leading zeros or
/// the sign byte missing), or already raw `r ‖ s` as PKCS #11 tokens return it. `notes` gets an
/// entry when the signature was not canonical DER.
fn ecdsa_rs(sig: &[u8], field_len: usize, notes: &mut Vec<&'static str>) -> Option<Vec<u8>> {
    let pad = |int: &[u8]| -> Option<Vec<u8>> {
        let int = int.get(int.iter().position(|b| *b != 0).unwrap_or(int.len())..).unwrap_or_default();
        let zeros = field_len.checked_sub(int.len())?;
        Some(std::iter::repeat_n(0u8, zeros).chain(int.iter().copied()).collect())
    };
    if let Ok((t, rest)) = Tlv::parse(sig)
        && t.tag == tag::SEQUENCE
        && rest.iter().all(|b| *b == 0)
        && let Ok(parts) = t.children()
        && let [r, s] = parts.as_slice()
        && r.tag == tag::INTEGER
        && s.tag == tag::INTEGER
    {
        // Canonical DER: nothing after the sequence, minimal positive integers.
        let minimal = |i: &Tlv<'_>| i.value.first().is_some_and(|b| b & 0x80 == 0) && !matches!(i.value, [0, b, ..] if b & 0x80 == 0);
        if !rest.is_empty() || !minimal(r) || !minimal(s) {
            notes.push("The ECDSA signature is not in canonical DER (trailing bytes, padding or a missing sign byte); it was read anyway.");
        }
        let mut out = pad(r.value)?;
        out.extend(pad(s.value)?);
        return Some(out);
    }
    (sig.len() == field_len * 2).then(|| {
        notes.push("The ECDSA signature is the raw r and s values instead of a DER sequence; it was read anyway.");
        sig.to_vec()
    })
}

enum Inner {
    #[cfg(not(target_arch = "wasm32"))]
    Rsa(aws_lc_rs::signature::RsaKeyPair),
    #[cfg(target_arch = "wasm32")]
    Rsa,
    P256(p256::ecdsa::SigningKey),
    P384(p384::ecdsa::SigningKey),
    /// A key held elsewhere (the macOS Keychain, a token) that signs on request.
    External(std::sync::Arc<dyn ExternalKey>),
}

/// A private key PdfCraft can't read, only ask to sign (OS key stores, tokens).
pub trait ExternalKey: Send + Sync {
    /// Sign `msg`, hashing it with `alg` (PKCS #1 v1.5 for RSA, DER-encoded ECDSA).
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError>;
}

/// A private key that can sign.
pub struct PrivateKey {
    inner: Inner,
    public: PublicKey,
    /// The PKCS#8 `PrivateKeyInfo` it came from (to write it back into a PKCS#12 file).
    pkcs8: Vec<u8>,
}

impl std::fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PrivateKey({})", self.public.describe())
    }
}

impl PrivateKey {
    /// A key held outside PdfCraft's memory (its public half is `public`). It can't be
    /// exported to a PKCS #12 file.
    pub fn external(public: PublicKey, key: std::sync::Arc<dyn ExternalKey>) -> PrivateKey {
        PrivateKey { inner: Inner::External(key), public, pkcs8: Vec::new() }
    }

    /// Whether the key lives outside the app (and so can't be exported).
    pub fn is_external(&self) -> bool {
        matches!(self.inner, Inner::External(_))
    }

    /// From a PKCS#8 `PrivateKeyInfo`.
    pub fn from_pkcs8(info: &[u8]) -> Result<PrivateKey, SignError> {
        let t = Tlv::parse_all(info)?.children()?;
        let [_version, alg, key, ..] = t.as_slice() else { return Err(SignError::Malformed("PrivateKeyInfo".into())) };
        let alg = alg.children()?;
        let o = alg.first().ok_or_else(|| SignError::Malformed("key algorithm".into()))?.oid()?;
        let key = key.expect(tag::OCTET_STRING, "private key")?;
        match o.as_str() {
            oid::RSA => {
                let k = Tlv::parse_all(key.value)?.children()?;
                let (n, e) = match k.as_slice() {
                    [_, n, e, ..] => (n.uint_bytes().to_vec(), e.uint_bytes().to_vec()),
                    _ => return Err(SignError::Malformed("RSAPrivateKey".into())),
                };
                #[cfg(not(target_arch = "wasm32"))]
                let inner = Inner::Rsa(
                    aws_lc_rs::signature::RsaKeyPair::from_pkcs8(info)
                        .or_else(|_| aws_lc_rs::signature::RsaKeyPair::from_der(key.value))
                        .map_err(|e| SignError::Malformed(format!("RSA private key: {e}")))?,
                );
                #[cfg(target_arch = "wasm32")]
                let inner = Inner::Rsa;
                Ok(PrivateKey { inner, public: PublicKey::Rsa { n, e }, pkcs8: info.to_vec() })
            }
            oid::EC => {
                // ECPrivateKey: version, privateKey OCTET STRING, [0] parameters, [1] publicKey.
                let ec = Tlv::parse_all(key.value)?.children()?;
                let d = ec.get(1).ok_or_else(|| SignError::Malformed("ECPrivateKey".into()))?.expect(tag::OCTET_STRING, "EC private key")?.value;
                let curve = match alg.get(1).map(|c| c.oid()).transpose()? {
                    Some(c) => c,
                    None => ec
                        .iter()
                        .find(|t| t.tag == tag::ctx(0))
                        .and_then(|p| p.inner().ok())
                        .and_then(|p| p.oid().ok())
                        .ok_or_else(|| SignError::Malformed("EC curve".into()))?,
                };
                match curve.as_str() {
                    oid::P256 => {
                        let k = p256::ecdsa::SigningKey::from_slice(d).map_err(|_| SignError::Malformed("P-256 key".into()))?;
                        let public = PublicKey::P256(k.verifying_key().to_sec1_point(false).as_bytes().to_vec());
                        Ok(PrivateKey { inner: Inner::P256(k), public, pkcs8: info.to_vec() })
                    }
                    oid::P384 => {
                        let k = p384::ecdsa::SigningKey::from_slice(d).map_err(|_| SignError::Malformed("P-384 key".into()))?;
                        let public = PublicKey::P384(k.verifying_key().to_sec1_point(false).as_bytes().to_vec());
                        Ok(PrivateKey { inner: Inner::P384(k), public, pkcs8: info.to_vec() })
                    }
                    other => Err(SignError::Unsupported(format!("elliptic curve {other}"))),
                }
            }
            other => Err(SignError::Unsupported(format!("private key algorithm {other}"))),
        }
    }

    /// A new RSA key (Acrobat's default for a self-signed digital ID is 2048-bit RSA).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn generate_rsa(bits: usize) -> Result<PrivateKey, SignError> {
        use aws_lc_rs::encoding::AsDer;
        use aws_lc_rs::rsa::KeySize;
        let size = match bits {
            2048 => KeySize::Rsa2048,
            3072 => KeySize::Rsa3072,
            4096 => KeySize::Rsa4096,
            _ => return Err(SignError::Unsupported(format!("{bits}-bit RSA keys"))),
        };
        let kp = aws_lc_rs::signature::RsaKeyPair::generate(size).map_err(|_| SignError::Crypto("RSA key generation failed".into()))?;
        let der = kp.as_der().map_err(|_| SignError::Crypto("RSA key encoding failed".into()))?;
        PrivateKey::from_pkcs8(der.as_ref())
    }

    /// RSA keys are generated only on native targets (ADR-0009).
    #[cfg(target_arch = "wasm32")]
    pub fn generate_rsa(_bits: usize) -> Result<PrivateKey, SignError> {
        Err(SignError::Unsupported("creating RSA keys in the browser (use a P-256 key)".into()))
    }

    /// A new P-256 key.
    pub fn generate_p256() -> Result<PrivateKey, SignError> {
        loop {
            let mut d = [0u8; 32];
            getrandom::fill(&mut d).map_err(|e| SignError::Crypto(format!("no randomness: {e}")))?;
            if let Ok(k) = p256::ecdsa::SigningKey::from_slice(&d) {
                let point = k.verifying_key().to_sec1_point(false).as_bytes().to_vec();
                // PKCS#8 with an ECPrivateKey (RFC 5915) carrying the public key.
                let ec = der::seq(&[&der::int(1), &der::octets(&d), &der::explicit(1, &der::bit_string(&point))]);
                let info = der::seq(&[&der::int(0), &der::seq(&[&der::oid(oid::EC), &der::oid(oid::P256)]), &der::octets(&ec)]);
                return PrivateKey::from_pkcs8(&info);
            }
        }
    }

    pub fn public_key(&self) -> &PublicKey {
        &self.public
    }

    pub fn pkcs8(&self) -> &[u8] {
        &self.pkcs8
    }

    /// The signature `AlgorithmIdentifier` this key writes with `alg`.
    pub fn signature_algorithm(&self, alg: DigestAlg) -> Vec<u8> {
        let rsa = matches!(self.public, PublicKey::Rsa { .. });
        let scheme = if rsa { Scheme::RsaPkcs1 } else { Scheme::Ecdsa };
        // `sign` only writes SHA-256/384/512, which the table has for both schemes.
        let oid = SIGNATURE_OIDS
            .iter()
            .find(|(_, s, d)| *s == scheme && *d == alg)
            .map_or(if rsa { "1.2.840.113549.1.1.11" } else { "1.2.840.10045.4.3.2" }, |(o, _, _)| *o);
        der::algorithm(oid, rsa.then(der::null).as_deref())
    }

    /// The digest a signature with this key should use: SHA-256, or SHA-384 for P-384.
    pub fn preferred_digest(&self) -> DigestAlg {
        match self.public {
            PublicKey::P384(_) => DigestAlg::Sha384,
            _ => DigestAlg::Sha256,
        }
    }

    /// Sign `msg` (hashed with `alg`). New signatures never use SHA-1.
    pub fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        if !matches!(alg, DigestAlg::Sha256 | DigestAlg::Sha384 | DigestAlg::Sha512) {
            return Err(SignError::Unsupported(format!("{} for new signatures", alg.name())));
        }
        match &self.inner {
            #[cfg(not(target_arch = "wasm32"))]
            Inner::Rsa(kp) => {
                use aws_lc_rs::signature::{RSA_PKCS1_SHA256, RSA_PKCS1_SHA384, RSA_PKCS1_SHA512};
                let enc: &'static dyn aws_lc_rs::signature::RsaEncoding = match alg {
                    DigestAlg::Sha384 => &RSA_PKCS1_SHA384,
                    DigestAlg::Sha512 => &RSA_PKCS1_SHA512,
                    _ => &RSA_PKCS1_SHA256,
                };
                let mut sig = vec![0u8; kp.public_modulus_len()];
                kp.sign(enc, &aws_lc_rs::rand::SystemRandom::new(), msg, &mut sig).map_err(|_| SignError::Crypto("RSA signing failed".into()))?;
                Ok(sig)
            }
            #[cfg(target_arch = "wasm32")]
            Inner::Rsa => Err(SignError::Unsupported("RSA signing in the browser".into())),
            Inner::P256(k) => {
                use p256::ecdsa::signature::hazmat::PrehashSigner;
                let s: p256::ecdsa::Signature = k.sign_prehash(&alg.digest(&[msg])).map_err(|e| SignError::Crypto(e.to_string()))?;
                Ok(s.to_der().as_bytes().to_vec())
            }
            Inner::External(k) => k.sign(alg, msg),
            Inner::P384(k) => {
                use p384::ecdsa::signature::hazmat::PrehashSigner;
                let s: p384::ecdsa::Signature = k.sign_prehash(&alg.digest(&[msg])).map_err(|e| SignError::Crypto(e.to_string()))?;
                Ok(s.to_der().as_bytes().to_vec())
            }
        }
    }
}
