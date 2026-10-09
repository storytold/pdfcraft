//! PKCS #12 digital ID files (RFC 7292): `.p12` / `.pfx`, as Acrobat, OpenSSL, macOS Keychain
//! and Windows export them.
//!
//! Reading supports PBES2 (PBKDF2 with HMAC-SHA-1/256/384/512; AES-128/192/256-CBC or
//! 3DES-CBC) and the legacy PKCS #12 PBE schemes (SHA-1 with 3DES or RC2), and checks the MAC.
//! Files that aren't clean DER still open: trailing whitespace, PEM armour or a bare
//! base64 body (#159).
//! Writing uses PBES2 AES-256-CBC with PBKDF2-HMAC-SHA-256 and an HMAC-SHA-256 MAC, OpenSSL 3's
//! defaults.
//!
//! The envelope is read as BER, not DER: Windows writes `.p12` files with indefinite lengths and
//! the AuthenticatedSafe split into OCTET STRING segments, and nothing in the envelope is signed
//! over its own bytes. The certificates inside it are still read as strict DER, which is how they
//! were signed.

use std::borrow::Cow;

use base64::Engine as _;
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockModeDecrypt, BlockModeEncrypt, InnerIvInit, KeyIvInit};
use hmac::Mac;

use crate::SignError;
use crate::der::{self, Tlv, tag};
use crate::keys::{DigestAlg, PrivateKey};
use crate::x509::Certificate;

const DATA: &str = "1.2.840.113549.1.7.1";
const ENCRYPTED_DATA: &str = "1.2.840.113549.1.7.6";
const KEY_BAG: &str = "1.2.840.113549.1.12.10.1.1";
const SHROUDED_KEY_BAG: &str = "1.2.840.113549.1.12.10.1.2";
const CERT_BAG: &str = "1.2.840.113549.1.12.10.1.3";
const X509_CERT: &str = "1.2.840.113549.1.9.22.1";
const FRIENDLY_NAME: &str = "1.2.840.113549.1.9.20";
const LOCAL_KEY_ID: &str = "1.2.840.113549.1.9.21";
const PBES2: &str = "1.2.840.113549.1.5.13";
const PBKDF2: &str = "1.2.840.113549.1.5.12";
const HMAC_SHA1: &str = "1.2.840.113549.2.7";
const HMAC_SHA256: &str = "1.2.840.113549.2.9";
const HMAC_SHA384: &str = "1.2.840.113549.2.10";
const HMAC_SHA512: &str = "1.2.840.113549.2.11";
const AES128_CBC: &str = "2.16.840.1.101.3.4.1.2";
const AES192_CBC: &str = "2.16.840.1.101.3.4.1.22";
const AES256_CBC: &str = "2.16.840.1.101.3.4.1.42";
const DES_EDE3_CBC: &str = "1.2.840.113549.3.7";
const PBE_SHA_3DES: &str = "1.2.840.113549.1.12.1.3";
const PBE_SHA_2DES: &str = "1.2.840.113549.1.12.1.4";
const PBE_SHA_RC2_128: &str = "1.2.840.113549.1.12.1.5";
const PBE_SHA_RC2_40: &str = "1.2.840.113549.1.12.1.6";

/// What a digital ID file holds: the signing key, its certificate and any others (issuers).
#[derive(Debug)]
pub struct DigitalId {
    pub key: PrivateKey,
    pub certificate: Certificate,
    /// The other certificates in the file (the chain), without the signer's.
    pub chain: Vec<Certificate>,
    pub friendly_name: Option<String>,
}

fn bad(what: &str) -> SignError {
    SignError::Malformed(format!("PKCS #12: {what}"))
}

/// The password as a BMPString with its terminating NUL (RFC 7292 Appendix B.1).
fn bmp(password: &str) -> Vec<u8> {
    let mut v: Vec<u8> = password.encode_utf16().flat_map(u16::to_be_bytes).collect();
    v.extend([0, 0]);
    v
}

/// The PKCS #12 key derivation function (RFC 7292 Appendix B.2).
fn pkcs12_kdf(alg: DigestAlg, password: &[u8], salt: &[u8], id: u8, iterations: u32, n: usize) -> Vec<u8> {
    // Callers only pass the SHA-1/SHA-2 digests PKCS #12 files use (`open` restricts the MAC digest).
    let (u, v) = match alg {
        DigestAlg::Sha1 => (20, 64),
        DigestAlg::Sha224 => (28, 64),
        DigestAlg::Sha256 => (32, 64),
        DigestAlg::Sha384 => (48, 128),
        DigestAlg::Sha512 => (64, 128),
        _ => return Vec::new(),
    };
    let _ = u;
    let fill = |s: &[u8]| -> Vec<u8> {
        if s.is_empty() {
            return Vec::new();
        }
        let len = v * s.len().div_ceil(v);
        s.iter().cycle().take(len).copied().collect()
    };
    let d = vec![id; v];
    let mut i: Vec<u8> = [fill(salt), fill(password)].concat();
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let mut a = alg.digest(&[&d, &i]);
        for _ in 1..iterations.max(1) {
            a = alg.digest(&[&a]);
        }
        let b: Vec<u8> = a.iter().cycle().take(v).copied().collect();
        for chunk in i.chunks_mut(v) {
            // chunk = (chunk + b + 1) mod 2^(8v)
            let mut carry = 1u16;
            for k in (0..v).rev() {
                let s = chunk[k] as u16 + b[k] as u16 + carry;
                chunk[k] = s as u8;
                carry = s >> 8;
            }
        }
        out.extend_from_slice(&a);
    }
    out.truncate(n);
    out
}

#[derive(Clone, Copy)]
enum Cipher {
    Aes128,
    Aes192,
    Aes256,
    TripleDes,
    Rc2(usize),
}

impl Cipher {
    fn key_len(self) -> usize {
        match self {
            Cipher::Aes128 => 16,
            Cipher::Aes192 => 24,
            Cipher::Aes256 => 32,
            Cipher::TripleDes => 24,
            Cipher::Rc2(bits) => bits / 8,
        }
    }

    fn decrypt(self, key: &[u8], iv: &[u8], ct: &[u8]) -> Result<Vec<u8>, SignError> {
        let wrong = || SignError::WrongPassword;
        let r = match self {
            Cipher::Aes128 => cbc::Decryptor::<aes::Aes128>::new_from_slices(key, iv).map_err(|_| wrong())?.decrypt_padded_vec::<Pkcs7>(ct),
            Cipher::Aes192 => cbc::Decryptor::<aes::Aes192>::new_from_slices(key, iv).map_err(|_| wrong())?.decrypt_padded_vec::<Pkcs7>(ct),
            Cipher::Aes256 => cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv).map_err(|_| wrong())?.decrypt_padded_vec::<Pkcs7>(ct),
            Cipher::TripleDes => cbc::Decryptor::<des::TdesEde3>::new_from_slices(key, iv).map_err(|_| wrong())?.decrypt_padded_vec::<Pkcs7>(ct),
            Cipher::Rc2(bits) => {
                let c = rc2::Rc2::new_with_eff_key_len(key, bits);
                cbc::Decryptor::<rc2::Rc2>::inner_iv_slice_init(c, iv).map_err(|_| wrong())?.decrypt_padded_vec::<Pkcs7>(ct)
            }
        };
        r.map_err(|_| wrong())
    }
}

fn hmac(alg: DigestAlg, key: &[u8], data: &[u8]) -> Result<Vec<u8>, SignError> {
    fn run<D: hmac::EagerHash>(key: &[u8], data: &[u8]) -> Result<Vec<u8>, SignError>
    where
        hmac::Hmac<D>: hmac::KeyInit + Mac,
    {
        // HMAC takes any key length, so this never fails in practice.
        let mut m = <hmac::Hmac<D> as hmac::KeyInit>::new_from_slice(key).map_err(|_| bad("MAC key"))?;
        m.update(data);
        Ok(m.finalize().into_bytes().to_vec())
    }
    match alg {
        DigestAlg::Sha1 => run::<sha1::Sha1>(key, data),
        DigestAlg::Sha224 => run::<sha2::Sha224>(key, data),
        DigestAlg::Sha256 => run::<sha2::Sha256>(key, data),
        DigestAlg::Sha384 => run::<sha2::Sha384>(key, data),
        DigestAlg::Sha512 => run::<sha2::Sha512>(key, data),
        other => Err(SignError::Unsupported(format!("{} in a PKCS #12 MAC", other.name()))),
    }
}

fn pbkdf2(prf: DigestAlg, password: &[u8], salt: &[u8], rounds: u32, len: usize) -> Result<Vec<u8>, SignError> {
    let mut out = vec![0u8; len];
    match prf {
        DigestAlg::Sha1 => pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, salt, rounds, &mut out),
        DigestAlg::Sha224 => pbkdf2::pbkdf2_hmac::<sha2::Sha224>(password, salt, rounds, &mut out),
        DigestAlg::Sha256 => pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password, salt, rounds, &mut out),
        DigestAlg::Sha384 => pbkdf2::pbkdf2_hmac::<sha2::Sha384>(password, salt, rounds, &mut out),
        DigestAlg::Sha512 => pbkdf2::pbkdf2_hmac::<sha2::Sha512>(password, salt, rounds, &mut out),
        other => return Err(SignError::Unsupported(format!("PBKDF2 with {}", other.name()))),
    }
    Ok(out)
}

/// Decrypt `ct` with the password-based scheme `alg` (an AlgorithmIdentifier).
fn pbe_decrypt(alg: &Tlv<'_>, password: &str, ct: &[u8]) -> Result<Vec<u8>, SignError> {
    let parts = alg.children()?;
    let o = parts.first().ok_or_else(|| bad("algorithm"))?.oid()?;
    let params = parts.get(1).ok_or_else(|| bad("algorithm parameters"))?;
    match o.as_str() {
        PBES2 => {
            let p = params.children()?;
            let [kdf, scheme] = p.as_slice() else { return Err(bad("PBES2 parameters")) };
            let kdf = kdf.children()?;
            if kdf.first().map(|o| o.oid()).transpose()?.as_deref() != Some(PBKDF2) {
                return Err(SignError::Unsupported("PBES2 key derivation other than PBKDF2".into()));
            }
            let kp = kdf.get(1).ok_or_else(|| bad("PBKDF2 parameters"))?.children()?;
            let salt = kp.first().ok_or_else(|| bad("salt"))?.octets("salt")?;
            let rounds = kp.get(1).ok_or_else(|| bad("iterations"))?.u64()? as u32;
            let prf = match kp
                .iter()
                .skip(2)
                .find(|t| t.tag == tag::SEQUENCE)
                .and_then(|a| a.children().ok())
                .and_then(|a| a.first().and_then(|o| o.oid().ok()))
            {
                None => DigestAlg::Sha1,
                Some(o) => match o.as_str() {
                    HMAC_SHA1 => DigestAlg::Sha1,
                    HMAC_SHA256 => DigestAlg::Sha256,
                    HMAC_SHA384 => DigestAlg::Sha384,
                    HMAC_SHA512 => DigestAlg::Sha512,
                    other => return Err(SignError::Unsupported(format!("PBKDF2 PRF {other}"))),
                },
            };
            let sc = scheme.children()?;
            let cipher = match sc.first().map(|o| o.oid()).transpose()?.as_deref() {
                Some(AES128_CBC) => Cipher::Aes128,
                Some(AES192_CBC) => Cipher::Aes192,
                Some(AES256_CBC) => Cipher::Aes256,
                Some(DES_EDE3_CBC) => Cipher::TripleDes,
                other => return Err(SignError::Unsupported(format!("PBES2 cipher {}", other.unwrap_or("?")))),
            };
            let iv = sc.get(1).ok_or_else(|| bad("IV"))?.octets("IV")?;
            let key = pbkdf2(prf, password.as_bytes(), &salt, rounds, cipher.key_len())?;
            cipher.decrypt(&key, &iv, ct)
        }
        PBE_SHA_3DES | PBE_SHA_2DES | PBE_SHA_RC2_128 | PBE_SHA_RC2_40 => {
            let p = params.children()?;
            let salt = p.first().ok_or_else(|| bad("salt"))?.octets("salt")?;
            let rounds = p.get(1).ok_or_else(|| bad("iterations"))?.u64()? as u32;
            let (cipher, key_len, iv_len) = match o.as_str() {
                PBE_SHA_3DES => (Cipher::TripleDes, 24, 8),
                PBE_SHA_2DES => (Cipher::TripleDes, 16, 8),
                PBE_SHA_RC2_128 => (Cipher::Rc2(128), 16, 8),
                _ => (Cipher::Rc2(40), 5, 8),
            };
            let pw = bmp(password);
            let mut key = pkcs12_kdf(DigestAlg::Sha1, &pw, &salt, 1, rounds, key_len);
            if key.len() == 16 && matches!(cipher, Cipher::TripleDes) {
                // Two-key 3DES: K1 K2 K1.
                let k1 = key[..8].to_vec();
                key.extend(k1);
            }
            let iv = pkcs12_kdf(DigestAlg::Sha1, &pw, &salt, 2, rounds, iv_len);
            cipher.decrypt(&key, &iv, ct)
        }
        other => Err(SignError::Unsupported(format!("PKCS #12 encryption {other}"))),
    }
}

struct Bag {
    kind: BagKind,
    local_key_id: Option<Vec<u8>>,
    friendly_name: Option<String>,
}

enum BagKind {
    Key(Vec<u8>),
    Cert(Vec<u8>),
}

fn bags(safe_contents: &[u8], password: &str, out: &mut Vec<Bag>) -> Result<(), SignError> {
    for bag in Tlv::parse_all_ber(safe_contents)?.children()? {
        let f = bag.children()?;
        let Some(id) = f.first().and_then(|o| o.oid().ok()) else { continue };
        let Some(value) = f.get(1).filter(|v| v.tag == tag::ctx(0)).and_then(|v| v.inner().ok()) else { continue };
        let (mut local_key_id, mut friendly_name) = (None, None);
        if let Some(attrs) = f.get(2) {
            for a in attrs.children()? {
                let p = a.children()?;
                let v = p.get(1).and_then(|s| s.children().ok()).and_then(|s| s.into_iter().next());
                match (p.first().and_then(|o| o.oid().ok()).as_deref(), v) {
                    (Some(LOCAL_KEY_ID), Some(v)) => local_key_id = v.octets("local key id").ok().map(Cow::into_owned),
                    (Some(FRIENDLY_NAME), Some(v)) => friendly_name = v.text(),
                    _ => {}
                }
            }
        }
        let kind = match id.as_str() {
            KEY_BAG => BagKind::Key(value.raw.to_vec()),
            SHROUDED_KEY_BAG => {
                let p = value.children()?;
                let [alg, data] = p.as_slice() else { return Err(bad("EncryptedPrivateKeyInfo")) };
                BagKind::Key(pbe_decrypt(alg, password, &data.octets("encrypted private key")?)?)
            }
            CERT_BAG => {
                let p = value.children()?;
                if p.first().and_then(|o| o.oid().ok()).as_deref() != Some(X509_CERT) {
                    continue;
                }
                let Some(c) = p.get(1).and_then(|c| c.inner().ok()).and_then(|c| c.octets("certificate").ok()) else { continue };
                BagKind::Cert(c.into_owned())
            }
            _ => continue,
        };
        out.push(Bag { kind, local_key_id, friendly_name });
    }
    Ok(())
}

fn normalize(bytes: &[u8]) -> Result<Cow<'_, [u8]>, SignError> {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0);
    let end = bytes.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(0, |i| i + 1);
    let trimmed = bytes.get(start..end).unwrap_or(b""); // hostile input: no slicing by hand

    // 1. A DER file starts with a SEQUENCE tag: borrow, no work.
    if trimmed.first() == Some(&0x30) {
        return Ok(Cow::Borrowed(trimmed));
    }
    // 2. PEM armor: base64 body between the BEGIN/END lines.
    if trimmed.starts_with(b"-----BEGIN") {
        let body = pem_body(trimmed)?;
        return Ok(Cow::Owned(decode_base64(body)?));
    }
    // 3. Raw base64 text, armour or not (a real DER PFX always contains bytes
    //    outside the base64 alphabet, so this can't swallow a valid file).
    if trimmed.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/' || *b == b'=' || b.is_ascii_whitespace()) {
        return Ok(Cow::Owned(decode_base64(trimmed)?));
    }
    // 4. Not recognisable: hand the bytes to Tlv::parse_all for a clear error.
    Ok(Cow::Borrowed(trimmed))
}

fn decode_base64(input: &[u8]) -> Result<Vec<u8>, SignError> {
    let compact: Vec<u8> = input.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(&compact).map_err(|e| SignError::Malformed(format!("PKCS #12: base64: {e}")))
}

/// The base64 lines between "-----BEGIN …-----" and "-----END …-----".
fn pem_body(pem: &[u8]) -> Result<&[u8], SignError> {
    let text = std::str::from_utf8(pem).map_err(|_| bad("PEM file is not valid text"))?;
    let begin = text.find("-----BEGIN ").ok_or_else(|| bad("PEM file has no BEGIN marker"))?;
    let body_start = text[begin..].find('\n').map_or(text.len(), |i| begin + i + 1);
    let body_end = text.find("-----END ").filter(|end| *end >= body_start).ok_or_else(|| bad("PEM file has no END marker"))?;
    let body = text.as_bytes().get(body_start..body_end).ok_or_else(|| bad("PEM body"))?;
    if !body.iter().any(|b| !b.is_ascii_whitespace()) {
        return Err(bad("PEM file has an empty body"));
    }
    Ok(body)
}

/// Open a `.p12` / `.pfx` file. A wrong password is [`SignError::WrongPassword`].
pub fn open(bytes: &[u8], password: &str) -> Result<DigitalId, SignError> {
    let der = normalize(bytes)?;
    let pfx = Tlv::parse_all_ber(&der)
        .map_err(|e| match e {
            // Keep "not a PKCS #12 file" but carry the exact reason through.
            SignError::Malformed(why) => bad(&format!("not a PKCS #12 file: {why}")),
            e => e,
        })?
        .children()?;
    let auth_safe = pfx.get(1).ok_or_else(|| bad("authSafe"))?.children()?;
    if auth_safe.first().map(|o| o.oid()).transpose()?.as_deref() != Some(DATA) {
        return Err(SignError::Unsupported("public-key protected PKCS #12 files".into()));
    }
    let content = auth_safe.get(1).ok_or_else(|| bad("authSafe content"))?.inner()?.octets("authSafe")?;
    // The MAC proves the password before anything is decrypted. It covers the contents of that
    // OCTET STRING — the segments joined, if the file is segmented, which is what wrote them.
    if let Some(mac) = pfx.get(2) {
        let m = mac.children()?;
        let digest_info = m.first().ok_or_else(|| bad("MacData"))?.children()?;
        let alg = digest_info.first().ok_or_else(|| bad("MAC algorithm"))?.children()?;
        let alg_oid = alg.first().ok_or_else(|| bad("MAC algorithm"))?.oid()?;
        let alg = DigestAlg::from_oid(&alg_oid)
            .filter(|d| matches!(d, DigestAlg::Sha1 | DigestAlg::Sha224 | DigestAlg::Sha256 | DigestAlg::Sha384 | DigestAlg::Sha512))
            .ok_or_else(|| SignError::Unsupported(format!("MAC digest {alg_oid}")))?;
        let expected = digest_info.get(1).ok_or_else(|| bad("MAC"))?.octets("MAC")?;
        let salt = m.get(1).ok_or_else(|| bad("MAC salt"))?.octets("MAC salt")?;
        let iterations = m.get(2).map(|i| i.u64()).transpose()?.unwrap_or(1) as u32;
        let key_len = alg.digest(&[]).len();
        let key = pkcs12_kdf(alg, &bmp(password), &salt, 3, iterations, key_len);
        if hmac(alg, &key, &content)? != *expected {
            return Err(SignError::WrongPassword);
        }
    }
    let mut all = Vec::new();
    for ci in Tlv::parse_all_ber(&content)?.children()? {
        let p = ci.children()?;
        let kind = p.first().ok_or_else(|| bad("ContentInfo"))?.oid()?;
        let body = p.get(1).ok_or_else(|| bad("ContentInfo content"))?.inner()?;
        match kind.as_str() {
            DATA => bags(&body.octets("SafeContents")?, password, &mut all)?,
            ENCRYPTED_DATA => {
                let ed = body.children()?;
                let eci = ed.get(1).ok_or_else(|| bad("EncryptedContentInfo"))?.children()?;
                let alg = eci.get(1).ok_or_else(|| bad("content encryption algorithm"))?;
                let ct: Cow<'_, [u8]> = match eci.get(2) {
                    Some(t) if t.tag == tag::ctx_prim(0) => Cow::Borrowed(t.value),
                    // Constructed form: the OCTET STRING segments, joined.
                    Some(t) if t.tag == tag::ctx(0) => {
                        let mut v = Vec::with_capacity(t.value.len());
                        for s in t.children()? {
                            v.extend_from_slice(&s.octets("encryptedContent")?);
                        }
                        Cow::Owned(v)
                    }
                    _ => return Err(bad("encryptedContent")),
                };
                let plain = pbe_decrypt(alg, password, &ct)?;
                bags(&plain, password, &mut all)?;
            }
            _ => {}
        }
    }
    let (key_bag, pkcs8) = all
        .iter()
        .find_map(|b| match &b.kind {
            BagKind::Key(k) => Some((b, k)),
            _ => None,
        })
        .ok_or_else(|| bad("no private key"))?;
    let key = PrivateKey::from_pkcs8(pkcs8)?;
    let certs: Vec<(Certificate, &Bag)> = all
        .iter()
        .filter_map(|b| match &b.kind {
            BagKind::Cert(c) => Certificate::parse(c).ok().map(|c| (c, b)),
            _ => None,
        })
        .collect();
    // The signer's certificate: the same local key id, else the one with the key's public key.
    let idx = certs
        .iter()
        .position(|(_, b)| key_bag.local_key_id.is_some() && b.local_key_id == key_bag.local_key_id)
        .or_else(|| certs.iter().position(|(c, _)| &c.public_key == key.public_key()))
        .ok_or_else(|| bad("no certificate for the private key"))?;
    let friendly_name = key_bag.friendly_name.clone().or_else(|| certs[idx].1.friendly_name.clone());
    let mut certs: Vec<Certificate> = certs.into_iter().map(|(c, _)| c).collect();
    let certificate = certs.remove(idx);
    if &certificate.public_key != key.public_key() {
        return Err(bad("the certificate does not match the private key"));
    }
    Ok(DigitalId { key, certificate, chain: certs, friendly_name })
}

fn random(n: usize) -> Result<Vec<u8>, SignError> {
    let mut v = vec![0u8; n];
    getrandom::fill(&mut v).map_err(|e| SignError::Crypto(format!("no randomness: {e}")))?;
    Ok(v)
}

/// Write a digital ID as a `.p12` protected by `password`.
pub fn write(id: &DigitalId, password: &str) -> Result<Vec<u8>, SignError> {
    const ITER: u32 = 2048;
    let encrypt = |plain: &[u8]| -> Result<Vec<u8>, SignError> {
        let (salt, iv) = (random(16)?, random(16)?);
        let key = pbkdf2(DigestAlg::Sha256, password.as_bytes(), &salt, ITER, 32)?;
        let ct = cbc::Encryptor::<aes::Aes256>::new_from_slices(&key, &iv)
            .map_err(|_| SignError::Crypto("AES".into()))?
            .encrypt_padded_vec::<Pkcs7>(plain);
        let kdf = der::seq(&[
            &der::oid(PBKDF2),
            &der::seq(&[&der::octets(&salt), &der::int(ITER as u64), &der::seq(&[&der::oid(HMAC_SHA256), &der::null()])]),
        ]);
        let alg = der::seq(&[&der::oid(PBES2), &der::seq(&[&kdf, &der::seq(&[&der::oid(AES256_CBC), &der::octets(&iv)])])]);
        Ok(der::seq(&[&alg, &der::octets(&ct)]))
    };
    let local_id = DigestAlg::Sha1.digest(&[&id.certificate.raw]);
    let mut attrs: Vec<Vec<u8>> = vec![der::seq(&[&der::oid(LOCAL_KEY_ID), &der::set_of(&[&der::octets(&local_id)])])];
    if let Some(n) = &id.friendly_name {
        let v: Vec<u8> = n.encode_utf16().flat_map(u16::to_be_bytes).collect();
        attrs.push(der::seq(&[&der::oid(FRIENDLY_NAME), &der::set_of(&[&der::tlv(tag::BMP_STRING, &v)])]));
    }
    let attr_refs: Vec<&[u8]> = attrs.iter().map(Vec::as_slice).collect();
    let attr_set = der::set_of(&attr_refs);
    let cert_bag = |c: &Certificate, with_attrs: bool| {
        let value = der::explicit(0, &der::seq(&[&der::oid(X509_CERT), &der::explicit(0, &der::octets(&c.raw))]));
        if with_attrs { der::seq(&[&der::oid(CERT_BAG), &value, &attr_set]) } else { der::seq(&[&der::oid(CERT_BAG), &value]) }
    };
    let mut cert_bags = vec![cert_bag(&id.certificate, true)];
    cert_bags.extend(id.chain.iter().map(|c| cert_bag(c, false)));
    let certs_plain = der::seq(&cert_bags.iter().map(Vec::as_slice).collect::<Vec<_>>());
    let enc_certs = encrypt(&certs_plain)?;
    // EncryptedData: version 0, EncryptedContentInfo { data, algorithm, [0] IMPLICIT ct }.
    let enc_parts = Tlv::parse_all(&enc_certs)?.children()?;
    let eci = der::seq(&[&der::oid(DATA), enc_parts[0].raw, &der::tlv(tag::ctx_prim(0), enc_parts[1].value)]);
    let encrypted_data = der::seq(&[&der::int(0), &eci]);
    let ci_certs = der::seq(&[&der::oid(ENCRYPTED_DATA), &der::explicit(0, &encrypted_data)]);
    let key_bag = der::seq(&[&der::oid(SHROUDED_KEY_BAG), &der::explicit(0, &encrypt(id.key.pkcs8())?), &attr_set]);
    let ci_key = der::seq(&[&der::oid(DATA), &der::explicit(0, &der::octets(&der::seq(&[&key_bag])))]);
    let auth_safe = der::seq(&[&ci_certs, &ci_key]);
    let salt = random(16)?;
    let mac_key = pkcs12_kdf(DigestAlg::Sha256, &bmp(password), &salt, 3, ITER, 32);
    let mac = hmac(DigestAlg::Sha256, &mac_key, &auth_safe)?;
    let mac_data = der::seq(&[&der::seq(&[&DigestAlg::Sha256.algorithm(), &der::octets(&mac)]), &der::octets(&salt), &der::int(ITER as u64)]);
    Ok(der::seq(&[&der::int(3), &der::seq(&[&der::oid(DATA), &der::explicit(0, &der::octets(&auth_safe))]), &mac_data]))
}
