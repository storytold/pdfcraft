//! Digests, public keys (verification everywhere) and private keys (signing).
//!
//! Verification uses RustCrypto. RSA private-key operations use `aws-lc-rs` on native targets
//! (constant-time; ADR-0009: the `rsa` crate is never used for private keys there). ECDSA
//! signing uses RustCrypto with deterministic nonces (RFC 6979).

use crate::SignError;
use crate::der::{self, Tlv, tag};

pub mod oid {
    pub const SHA1: &str = "1.3.14.3.2.26";
    pub const SHA256: &str = "2.16.840.1.101.3.4.2.1";
    pub const SHA384: &str = "2.16.840.1.101.3.4.2.2";
    pub const SHA512: &str = "2.16.840.1.101.3.4.2.3";
    pub const RSA: &str = "1.2.840.113549.1.1.1";
    pub const RSA_SHA1: &str = "1.2.840.113549.1.1.5";
    pub const RSA_PSS: &str = "1.2.840.113549.1.1.10";
    pub const RSA_SHA256: &str = "1.2.840.113549.1.1.11";
    pub const RSA_SHA384: &str = "1.2.840.113549.1.1.12";
    pub const RSA_SHA512: &str = "1.2.840.113549.1.1.13";
    pub const MGF1: &str = "1.2.840.113549.1.1.8";
    pub const EC: &str = "1.2.840.10045.2.1";
    pub const P256: &str = "1.2.840.10045.3.1.7";
    pub const P384: &str = "1.3.132.0.34";
    pub const ECDSA_SHA1: &str = "1.2.840.10045.4.1";
    pub const ECDSA_SHA256: &str = "1.2.840.10045.4.3.2";
    pub const ECDSA_SHA384: &str = "1.2.840.10045.4.3.3";
    pub const ECDSA_SHA512: &str = "1.2.840.10045.4.3.4";
}

/// Message digests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DigestAlg {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl DigestAlg {
    pub fn from_oid(o: &str) -> Option<DigestAlg> {
        Some(match o {
            oid::SHA1 => DigestAlg::Sha1,
            oid::SHA256 => DigestAlg::Sha256,
            oid::SHA384 => DigestAlg::Sha384,
            oid::SHA512 => DigestAlg::Sha512,
            _ => return None,
        })
    }

    pub fn oid(self) -> &'static str {
        match self {
            DigestAlg::Sha1 => oid::SHA1,
            DigestAlg::Sha256 => oid::SHA256,
            DigestAlg::Sha384 => oid::SHA384,
            DigestAlg::Sha512 => oid::SHA512,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DigestAlg::Sha1 => "SHA-1",
            DigestAlg::Sha256 => "SHA-256",
            DigestAlg::Sha384 => "SHA-384",
            DigestAlg::Sha512 => "SHA-512",
        }
    }

    /// The digest of the concatenation of `parts`.
    pub fn digest(self, parts: &[&[u8]]) -> Vec<u8> {
        fn run<D: sha2::Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut h = D::new();
            for p in parts {
                h.update(p);
            }
            h.finalize().to_vec()
        }
        match self {
            DigestAlg::Sha1 => run::<sha1::Sha1>(parts),
            DigestAlg::Sha256 => run::<sha2::Sha256>(parts),
            DigestAlg::Sha384 => run::<sha2::Sha384>(parts),
            DigestAlg::Sha512 => run::<sha2::Sha512>(parts),
        }
    }

    /// The `AlgorithmIdentifier` (no parameters, as RFC 5754 recommends).
    pub fn algorithm(self) -> Vec<u8> {
        der::algorithm(self.oid(), None)
    }
}

/// How a signature value is computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    RsaPkcs1,
    RsaPss,
    Ecdsa,
}

/// A signature `AlgorithmIdentifier`: the scheme and, when the identifier names one, the digest.
pub fn signature_algorithm(alg: &Tlv<'_>) -> Result<(Scheme, Option<DigestAlg>), SignError> {
    let parts = alg.children()?;
    let o = parts.first().ok_or_else(|| SignError::Malformed("empty algorithm".into()))?.oid()?;
    Ok(match o.as_str() {
        oid::RSA => (Scheme::RsaPkcs1, None),
        oid::RSA_SHA1 => (Scheme::RsaPkcs1, Some(DigestAlg::Sha1)),
        oid::RSA_SHA256 => (Scheme::RsaPkcs1, Some(DigestAlg::Sha256)),
        oid::RSA_SHA384 => (Scheme::RsaPkcs1, Some(DigestAlg::Sha384)),
        oid::RSA_SHA512 => (Scheme::RsaPkcs1, Some(DigestAlg::Sha512)),
        oid::RSA_PSS => {
            // RSASSA-PSS-params: [0] hashAlgorithm (default SHA-1).
            let hash = parts
                .get(1)
                .and_then(|p| p.children().ok())
                .and_then(|c| c.into_iter().find(|t| t.tag == tag::ctx(0)))
                .and_then(|h| h.inner().ok())
                .and_then(|a| a.children().ok())
                .and_then(|a| a.first().and_then(|o| o.oid().ok()))
                .and_then(|o| DigestAlg::from_oid(&o))
                .unwrap_or(DigestAlg::Sha1);
            (Scheme::RsaPss, Some(hash))
        }
        oid::ECDSA_SHA1 => (Scheme::Ecdsa, Some(DigestAlg::Sha1)),
        oid::ECDSA_SHA256 => (Scheme::Ecdsa, Some(DigestAlg::Sha256)),
        oid::ECDSA_SHA384 => (Scheme::Ecdsa, Some(DigestAlg::Sha384)),
        oid::ECDSA_SHA512 => (Scheme::Ecdsa, Some(DigestAlg::Sha512)),
        oid::EC => (Scheme::Ecdsa, None),
        other => return Err(SignError::Unsupported(format!("signature algorithm {other}"))),
    })
}

/// A public key from a `SubjectPublicKeyInfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicKey {
    Rsa { n: Vec<u8>, e: Vec<u8> },
    P256(Vec<u8>),
    P384(Vec<u8>),
}

impl PublicKey {
    pub fn from_spki(spki: &Tlv<'_>) -> Result<PublicKey, SignError> {
        let parts = spki.children()?;
        let [alg, key] = parts.as_slice() else { return Err(SignError::Malformed("SubjectPublicKeyInfo".into())) };
        let alg = alg.children()?;
        let o = alg.first().ok_or_else(|| SignError::Malformed("key algorithm".into()))?.oid()?;
        let bits = key.bits()?;
        match o.as_str() {
            oid::RSA => {
                let k = Tlv::parse_all(bits)?.children()?;
                let [n, e] = k.as_slice() else { return Err(SignError::Malformed("RSAPublicKey".into())) };
                Ok(PublicKey::Rsa { n: n.uint_bytes().to_vec(), e: e.uint_bytes().to_vec() })
            }
            oid::EC => match alg.get(1).map(|c| c.oid()).transpose()?.as_deref() {
                Some(oid::P256) => Ok(PublicKey::P256(bits.to_vec())),
                Some(oid::P384) => Ok(PublicKey::P384(bits.to_vec())),
                other => Err(SignError::Unsupported(format!("elliptic curve {}", other.unwrap_or("?")))),
            },
            other => Err(SignError::Unsupported(format!("public key algorithm {other}"))),
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
        }
    }

    /// "RSA 2048-bit", "ECDSA P-256"…
    pub fn describe(&self) -> String {
        match self {
            PublicKey::Rsa { n, .. } => format!("RSA {}-bit", n.len() * 8),
            PublicKey::P256(_) => "ECDSA P-256".into(),
            PublicKey::P384(_) => "ECDSA P-384".into(),
        }
    }

    /// Check `sig` over a message whose digest (with `alg`) is `digest`.
    pub fn verify(&self, scheme: Scheme, alg: DigestAlg, digest: &[u8], sig: &[u8]) -> Result<bool, SignError> {
        match (self, scheme) {
            (PublicKey::Rsa { n, e }, Scheme::RsaPkcs1 | Scheme::RsaPss) => {
                use rsa::{BoxedUint, Pkcs1v15Sign, RsaPublicKey, pss::Pss};
                let key = RsaPublicKey::new(BoxedUint::from_be_slice_vartime(n), BoxedUint::from_be_slice_vartime(e))
                    .map_err(|e| SignError::Malformed(format!("RSA key: {e}")))?;
                let ok = match (scheme, alg) {
                    (Scheme::RsaPkcs1, DigestAlg::Sha1) => key.verify(Pkcs1v15Sign::new::<sha1::Sha1>(), digest, sig),
                    (Scheme::RsaPkcs1, DigestAlg::Sha256) => key.verify(Pkcs1v15Sign::new::<sha2::Sha256>(), digest, sig),
                    (Scheme::RsaPkcs1, DigestAlg::Sha384) => key.verify(Pkcs1v15Sign::new::<sha2::Sha384>(), digest, sig),
                    (Scheme::RsaPkcs1, DigestAlg::Sha512) => key.verify(Pkcs1v15Sign::new::<sha2::Sha512>(), digest, sig),
                    (_, DigestAlg::Sha1) => key.verify(Pss::<sha1::Sha1>::new(), digest, sig),
                    (_, DigestAlg::Sha256) => key.verify(Pss::<sha2::Sha256>::new(), digest, sig),
                    (_, DigestAlg::Sha384) => key.verify(Pss::<sha2::Sha384>::new(), digest, sig),
                    (_, DigestAlg::Sha512) => key.verify(Pss::<sha2::Sha512>::new(), digest, sig),
                };
                Ok(ok.is_ok())
            }
            (PublicKey::P256(p), Scheme::Ecdsa) => {
                use p256::ecdsa::signature::hazmat::PrehashVerifier;
                let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(p).map_err(|_| SignError::Malformed("P-256 key".into()))?;
                let Ok(s) = p256::ecdsa::Signature::from_der(sig) else { return Ok(false) };
                Ok(key.verify_prehash(digest, &s).is_ok())
            }
            (PublicKey::P384(p), Scheme::Ecdsa) => {
                use p384::ecdsa::signature::hazmat::PrehashVerifier;
                let key = p384::ecdsa::VerifyingKey::from_sec1_bytes(p).map_err(|_| SignError::Malformed("P-384 key".into()))?;
                let Ok(s) = p384::ecdsa::Signature::from_der(sig) else { return Ok(false) };
                Ok(key.verify_prehash(digest, &s).is_ok())
            }
            _ => Ok(false),
        }
    }
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
        match (&self.public, alg) {
            (PublicKey::Rsa { .. }, DigestAlg::Sha1) => der::algorithm(oid::RSA_SHA1, Some(&der::null())),
            (PublicKey::Rsa { .. }, DigestAlg::Sha256) => der::algorithm(oid::RSA_SHA256, Some(&der::null())),
            (PublicKey::Rsa { .. }, DigestAlg::Sha384) => der::algorithm(oid::RSA_SHA384, Some(&der::null())),
            (PublicKey::Rsa { .. }, DigestAlg::Sha512) => der::algorithm(oid::RSA_SHA512, Some(&der::null())),
            (_, DigestAlg::Sha1) => der::algorithm(oid::ECDSA_SHA1, None),
            (_, DigestAlg::Sha256) => der::algorithm(oid::ECDSA_SHA256, None),
            (_, DigestAlg::Sha384) => der::algorithm(oid::ECDSA_SHA384, None),
            (_, DigestAlg::Sha512) => der::algorithm(oid::ECDSA_SHA512, None),
        }
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
        if alg == DigestAlg::Sha1 {
            return Err(SignError::Unsupported("SHA-1 for new signatures".into()));
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
