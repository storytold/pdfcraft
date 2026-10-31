//! CMS recipient envelopes for PDF public-key security (RFC 5652 §6, ISO 32000-2 §7.6.5).
//!
//! Each envelope protects a 20-byte seed and four big-endian permission bytes. It does not
//! encrypt the PDF itself. RSA private-key operations belong to the caller (for example a
//! smartcard); this module never exports or imports a private key.

use aws_lc_rs::rsa::{Pkcs1PublicEncryptingKey, PublicEncryptingKey};
use cbc::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use zeroize::Zeroizing;

use crate::der::{self, Tlv, tag};
use crate::{Certificate, PublicKey, SignError, Time};

const ENVELOPED_DATA: &str = "1.2.840.113549.1.7.3";
const DATA: &str = "1.2.840.113549.1.7.1";
const RSA: &str = "1.2.840.113549.1.1.1";
const AES128: &str = "2.16.840.1.101.3.4.1.2";
const AES256: &str = "2.16.840.1.101.3.4.1.42";
const DES3: &str = "1.2.840.113549.3.7";
const MAX_ENVELOPE: usize = 1024 * 1024;

fn bad(message: &str) -> SignError {
    SignError::Malformed(format!("recipient envelope: {message}"))
}

/// The decrypted PDF seed and this recipient's permissions. Debug output hides the seed.
pub struct RecipientPayload {
    seed: Zeroizing<[u8; 20]>,
    pub permissions: i32,
}

impl std::fmt::Debug for RecipientPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecipientPayload").field("permissions", &self.permissions).finish_non_exhaustive()
    }
}

impl RecipientPayload {
    pub fn new(seed: [u8; 20], permissions: i32) -> Self {
        Self { seed: Zeroizing::new(seed), permissions }
    }

    pub fn seed(&self) -> &[u8; 20] {
        &self.seed
    }

    /// ISO 32000-2 §7.6.5.4, with the published permission-byte-order correction.
    pub fn encode(&self) -> Zeroizing<Vec<u8>> {
        let mut bytes = Zeroizing::new(Vec::with_capacity(24));
        bytes.extend_from_slice(self.seed.as_ref());
        bytes.extend_from_slice(&self.permissions.to_be_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, SignError> {
        let [seed @ .., a, b, c, d] = bytes else { return Err(bad("expected a 24-byte seed and permissions")) };
        let seed: [u8; 20] = seed.try_into().map_err(|_| bad("expected a 24-byte seed and permissions"))?;
        Ok(Self::new(seed, i32::from_be_bytes([*a, *b, *c, *d])))
    }
}

/// Whether a certificate can identify an RSA encryption key. Expiry is deliberately not
/// checked here: historical encrypted files still need their original key.
pub fn recipient_problem(cert: &Certificate) -> Option<&'static str> {
    if !matches!(cert.public_key, PublicKey::Rsa { .. }) {
        Some("recipient encryption requires an RSA certificate")
    } else if cert.is_ca {
        Some("a CA certificate cannot be a document recipient")
    } else if cert.key_usage.is_some_and(|usage| usage & (1 << 2) == 0) {
        Some("the recipient certificate does not permit key encipherment")
    } else if !cert.unknown_critical.is_empty() {
        Some("the recipient certificate has unsupported critical extensions")
    } else {
        None
    }
}

/// Build one RSA key-transport / AES-256-CBC envelope. No card or PIN is required to encrypt.
/// The caller supplies the time used to reject expired or not-yet-valid recipient certificates.
pub fn seal(cert: &Certificate, payload: &RecipientPayload, at: Time) -> Result<Vec<u8>, SignError> {
    if let Some(problem) = recipient_problem(cert) {
        return Err(SignError::Unsupported(problem.into()));
    }
    if !cert.valid_at(at) {
        return Err(SignError::Crypto("the recipient certificate is not valid at the encryption time".into()));
    }
    let public = PublicEncryptingKey::from_der(&cert.public_key.spki()).map_err(|_| bad("invalid RSA public key"))?;
    if public.key_size_bits() < 2048 {
        return Err(SignError::Unsupported("new recipient envelopes require RSA keys of at least 2048 bits".into()));
    }
    let public = Pkcs1PublicEncryptingKey::new(public).map_err(|_| bad("invalid RSA encryption key"))?;
    let mut key = Zeroizing::new([0u8; 32]);
    let mut iv = [0u8; 16];
    getrandom::fill(key.as_mut()).map_err(|_| SignError::Crypto("no randomness for recipient encryption".into()))?;
    getrandom::fill(&mut iv).map_err(|_| SignError::Crypto("no randomness for recipient encryption".into()))?;
    let mut wrapped = vec![0; public.ciphertext_size()];
    let wrapped = public.encrypt(key.as_ref(), &mut wrapped).map_err(|_| SignError::Crypto("recipient RSA encryption failed".into()))?;
    let encrypted = cbc::Encryptor::<aes::Aes256>::new_from_slices(key.as_ref(), &iv)
        .map_err(|_| bad("AES key or IV"))?
        .encrypt_padded_vec::<Pkcs7>(&payload.encode());
    let sid = der::seq(&[&cert.issuer.raw, &der::uint(&cert.serial)]);
    let recipient = der::seq(&[&der::int(0), &sid, &der::algorithm(RSA, Some(&der::null())), &der::octets(wrapped)]);
    let content = der::seq(&[&der::oid(DATA), &der::algorithm(AES256, Some(&der::octets(&iv))), &der::tlv(tag::ctx_prim(0), &encrypted)]);
    let envelope = der::seq(&[&der::int(0), &der::set_of(&[&recipient]), &content]);
    Ok(der::seq(&[&der::oid(ENVELOPED_DATA), &der::explicit(0, &envelope)]))
}

/// Open an envelope for `cert`. The callback decrypts one RSA PKCS #1 v1.5 encrypted key,
/// and returns secret bytes that are zeroized after use. It is called at most once, only
/// after the complete supported envelope structure and recipient identity are checked.
/// AES-128/256-CBC and 3DES-CBC envelope contents are accepted.
pub fn open(
    bytes: &[u8],
    cert: &Certificate,
    decrypt_key: impl FnOnce(&[u8]) -> Result<Zeroizing<Vec<u8>>, SignError>,
) -> Result<RecipientPayload, SignError> {
    let PreparedEnvelope { encrypted_key, encrypted, iv, oid, key_len } =
        prepare(bytes, cert)?.ok_or_else(|| SignError::Crypto("the envelope has no recipient matching this certificate".into()))?;
    let key = decrypt_key(&encrypted_key)?;
    if key.len() != key_len {
        return Err(bad("decrypted content key length"));
    }
    let failure = || SignError::Crypto("recipient envelope decryption failed".into());
    let plain = match oid.as_str() {
        AES128 => cbc::Decryptor::<aes::Aes128>::new_from_slices(&key, &iv).map_err(|_| failure())?.decrypt_padded_vec::<Pkcs7>(&encrypted),
        AES256 => cbc::Decryptor::<aes::Aes256>::new_from_slices(&key, &iv).map_err(|_| failure())?.decrypt_padded_vec::<Pkcs7>(&encrypted),
        _ => cbc::Decryptor::<des::TdesEde3>::new_from_slices(&key, &iv).map_err(|_| failure())?.decrypt_padded_vec::<Pkcs7>(&encrypted),
    }
    .map_err(|_| failure())?;
    RecipientPayload::decode(&Zeroizing::new(plain))
}

struct PreparedEnvelope {
    encrypted_key: Vec<u8>,
    encrypted: Vec<u8>,
    iv: Vec<u8>,
    oid: String,
    key_len: usize,
}

/// Validate a supported envelope and check its recipient identity without using a private key.
/// Callers can reject missing or ambiguous matches before prompting for a PIN.
pub fn matches_recipient(bytes: &[u8], cert: &Certificate) -> Result<bool, SignError> {
    Ok(prepare(bytes, cert)?.is_some())
}

fn prepare(bytes: &[u8], cert: &Certificate) -> Result<Option<PreparedEnvelope>, SignError> {
    if bytes.len() > MAX_ENVELOPE {
        return Err(bad("size limit exceeded"));
    }
    let root = Tlv::parse_all_ber(bytes)?.expect(tag::SEQUENCE, "CMS ContentInfo")?.children()?;
    let [kind, body] = root.as_slice() else { return Err(bad("ContentInfo fields")) };
    if kind.oid()? != ENVELOPED_DATA {
        return Err(bad("expected EnvelopedData"));
    }
    let body = body.expect(tag::ctx(0), "EnvelopedData wrapper")?.inner()?.expect(tag::SEQUENCE, "EnvelopedData")?.children()?;
    let [version, recipients, content] = body.as_slice() else { return Err(bad("unsupported EnvelopedData fields")) };
    if !matches!(version.u64()?, 0 | 2) {
        return Err(bad("unsupported EnvelopedData version"));
    }
    let recipients = recipients.expect(tag::SET, "recipientInfos")?.children()?;
    if recipients.is_empty() || recipients.len() > 64 {
        return Err(bad("expected 1 to 64 recipients"));
    }
    let mut encrypted_key = None;
    for recipient in recipients {
        if recipient.tag != tag::SEQUENCE {
            continue; // Another recipient can use a different key-management scheme.
        }
        let fields = recipient.children()?;
        let [version, sid, algorithm, wrapped] = fields.as_slice() else { return Err(bad("KeyTransRecipientInfo fields")) };
        let matches = match sid.tag {
            tag::SEQUENCE => {
                let fields = sid.children()?;
                let [issuer, serial] = fields.as_slice() else { return Err(bad("issuer and serial")) };
                version.u64()? == 0 && issuer.raw == cert.issuer.raw && serial.expect(tag::INTEGER, "serial")?.uint_bytes() == cert.serial
            }
            t if t == tag::ctx_prim(0) => version.u64()? == 2 && cert.subject_key_id.as_deref() == Some(sid.value),
            _ => false,
        };
        if !matches {
            continue;
        }
        if encrypted_key.is_some() {
            return Err(bad("duplicate matching recipient"));
        }
        let algorithm = algorithm.expect(tag::SEQUENCE, "key encryption algorithm")?.children()?;
        let (oid, params) = algorithm.split_first().ok_or_else(|| bad("key encryption algorithm"))?;
        if oid.oid()? != RSA || !(params.is_empty() || matches!(params, [p] if p.tag == tag::NULL && p.value.is_empty())) {
            return Err(SignError::Unsupported("recipient envelope requires RSA PKCS #1 v1.5 key transport".into()));
        }
        encrypted_key = Some(wrapped.octets("encrypted key")?);
    }
    let PublicKey::Rsa { n, .. } = &cert.public_key else {
        return Err(SignError::Unsupported("recipient key transport requires an RSA certificate".into()));
    };
    let content = content.expect(tag::SEQUENCE, "encryptedContentInfo")?.children()?;
    let [kind, algorithm, encrypted] = content.as_slice() else { return Err(bad("encryptedContentInfo fields")) };
    if kind.oid()? != DATA {
        return Err(bad("expected data content"));
    }
    let algorithm = algorithm.expect(tag::SEQUENCE, "content encryption algorithm")?.children()?;
    let [oid, iv] = algorithm.as_slice() else { return Err(bad("content encryption parameters")) };
    let oid = oid.oid()?;
    let (key_len, block) = match oid.as_str() {
        AES128 => (16, 16),
        AES256 => (32, 16),
        DES3 => (24, 8),
        _ => return Err(SignError::Unsupported(format!("recipient content encryption {oid}"))),
    };
    let iv = iv.octets("content IV")?;
    let encrypted = match encrypted.tag {
        t if t == tag::ctx_prim(0) => encrypted.value.to_vec(),
        t if t == tag::ctx(0) => {
            let mut joined = Vec::new();
            for piece in encrypted.children()? {
                joined.extend_from_slice(&piece.octets("encrypted content segment")?);
            }
            joined
        }
        _ => return Err(bad("encrypted content tag")),
    };
    if iv.len() != block || encrypted.is_empty() || encrypted.len() % block != 0 {
        return Err(bad("invalid IV or encrypted content length"));
    }
    let Some(encrypted_key) = encrypted_key else { return Ok(None) };
    if encrypted_key.len() != n.len() || encrypted_key.len() > 1024 {
        return Err(bad("RSA encrypted key length"));
    }
    Ok(Some(PreparedEnvelope { encrypted_key: encrypted_key.into_owned(), encrypted, iv: iv.into_owned(), oid, key_len }))
}
