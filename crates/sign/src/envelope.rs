//! CMS `EnvelopedData` (RFC 5652) for PDF public-key security (ISO 32000-2 §7.6.5, the
//! `Adobe.PubSec` handler): a small content - the file key's seed and the permissions - wrapped
//! for each recipient's certificate. Key transport with RSA (PKCS #1 v1.5, what Acrobat writes
//! and reads), the content under AES-128-CBC; opening also takes AES-192/256 and 3DES, which
//! other writers use. An elliptic-curve certificate cannot receive: key agreement is not done.

use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use rsa::{BoxedUint, RsaPrivateKey, RsaPublicKey};

use crate::SignError;
use crate::der::{self, Tlv, tag};
use crate::keys::{PrivateKey, PublicKey};
use crate::pkcs12::DigitalId;
use crate::x509::Certificate;

const OID_ENVELOPED_DATA: &str = "1.2.840.113549.1.7.3";
const OID_DATA: &str = "1.2.840.113549.1.7.1";
const OID_RSA: &str = "1.2.840.113549.1.1.1";
const OID_AES128_CBC: &str = "2.16.840.1.101.3.4.1.2";
const OID_AES192_CBC: &str = "2.16.840.1.101.3.4.1.22";
const OID_AES256_CBC: &str = "2.16.840.1.101.3.4.1.42";
const OID_DES_EDE3_CBC: &str = "1.2.840.113549.3.7";

fn bad(what: impl Into<String>) -> SignError {
    SignError::Malformed(format!("enveloped data: {}", what.into()))
}

fn random(n: usize) -> Result<Vec<u8>, SignError> {
    let mut v = vec![0u8; n];
    getrandom::fill(&mut v).map_err(|e| SignError::Crypto(format!("no randomness: {e}")))?;
    Ok(v)
}

/// A fresh 20-byte seed for certificate security's file key (ISO 32000-2 §7.6.5.3), from the
/// operating system's random generator.
pub fn new_seed() -> Result<[u8; 20], SignError> {
    let mut seed = [0u8; 20];
    seed.copy_from_slice(&random(20)?);
    Ok(seed)
}

/// Wrap `content` for `recipient` (an RSA certificate): a DER `ContentInfo` holding the
/// `EnvelopedData`, as the PDF `/Recipients` array carries them.
pub fn envelope(recipient: &Certificate, content: &[u8]) -> Result<Vec<u8>, SignError> {
    let PublicKey::Rsa { n, e } = &recipient.public_key else {
        return Err(SignError::Unsupported(format!(
            "encrypting for {}: certificate encryption needs an RSA certificate (this one has another kind of key)",
            recipient.display_name()
        )));
    };
    let cek = random(16)?;
    let iv = random(16)?;
    let encrypted =
        cbc::Encryptor::<aes::Aes128>::new_from_slices(&cek, &iv).map_err(|_| SignError::Crypto("AES".into()))?.encrypt_padded_vec::<Pkcs7>(content);
    let encrypted_key = rsa_pkcs1_encrypt(n, e, &cek)?;
    // KeyTransRecipientInfo, the recipient named by issuer and serial number.
    let rid = der::seq(&[&recipient.issuer.raw, &der::uint(&recipient.serial)]);
    let ktri = der::seq(&[&der::int(0), &rid, &der::algorithm(OID_RSA, Some(&der::null())), &der::octets(&encrypted_key)]);
    let content_alg = der::algorithm(OID_AES128_CBC, Some(&der::octets(&iv)));
    let eci = der::seq(&[&der::oid(OID_DATA), &content_alg, &der::tlv(0x80, &encrypted)]);
    let enveloped = der::seq(&[&der::int(0), &der::set_of(&[&ktri]), &eci]);
    Ok(der::seq(&[&der::oid(OID_ENVELOPED_DATA), &der::explicit(0, &enveloped)]))
}

/// Open an envelope with `id`: the content when the envelope names this identity (by issuer
/// and serial, or by subject key identifier) and the key opens it; `None` when it is for
/// someone else.
pub fn open(cms: &[u8], id: &DigitalId) -> Result<Option<Vec<u8>>, SignError> {
    let info = Tlv::parse_all_ber(cms).map_err(|_| bad("not DER"))?;
    let parts = info.expect(tag::SEQUENCE, "ContentInfo")?.children()?;
    let (Some(oid), Some(body)) = (parts.first(), parts.get(1)) else { return Err(bad("ContentInfo")) };
    if oid.oid()? != OID_ENVELOPED_DATA {
        return Err(bad("not EnvelopedData"));
    }
    let enveloped = body.inner()?.expect(tag::SEQUENCE, "EnvelopedData")?.children()?;
    // version, [originatorInfo], recipientInfos, encryptedContentInfo, [unprotectedAttrs]
    let mut it = enveloped.iter();
    it.next().ok_or_else(|| bad("version"))?;
    let mut next = it.next().ok_or_else(|| bad("recipientInfos"))?;
    if next.tag == 0xA0 {
        next = it.next().ok_or_else(|| bad("recipientInfos"))?;
    }
    let recipients = next.children()?;
    let eci = it.next().ok_or_else(|| bad("encryptedContentInfo"))?.children()?;
    let (Some(_data_oid), Some(alg), Some(encrypted)) = (eci.first(), eci.get(1), eci.get(2)) else { return Err(bad("encryptedContentInfo")) };
    let alg_parts = alg.children()?;
    let content_oid = alg_parts.first().ok_or_else(|| bad("content algorithm"))?.oid()?;
    let iv = alg_parts.get(1).ok_or_else(|| bad("content IV"))?.octets("IV")?.into_owned();
    let encrypted = if encrypted.tag == 0x80 { encrypted.value.to_vec() } else { encrypted.octets("encryptedContent")?.into_owned() };

    let mine = |rid: &Tlv<'_>| -> Result<bool, SignError> {
        if rid.tag == tag::SEQUENCE {
            let p = rid.children()?;
            let (Some(issuer), Some(serial)) = (p.first(), p.get(1)) else { return Ok(false) };
            Ok(issuer.raw == id.certificate.issuer.raw.as_slice() && serial.uint_bytes() == der_uint_value(&id.certificate.serial))
        } else if rid.tag == 0x80 {
            Ok(id.certificate.subject_key_id.as_deref() == Some(rid.value))
        } else {
            Ok(false)
        }
    };
    for r in &recipients {
        if r.tag != tag::SEQUENCE {
            continue; // not key transport (key agreement, passwords…): not for an RSA identity
        }
        let p = r.children()?;
        let (Some(_version), Some(rid), Some(kea), Some(ek)) = (p.first(), p.get(1), p.get(2), p.get(3)) else { continue };
        if !mine(rid)? {
            continue;
        }
        let kea_oid = kea.children()?.first().ok_or_else(|| bad("key encryption algorithm"))?.oid()?;
        if kea_oid != OID_RSA {
            return Err(SignError::Unsupported(format!("key encryption {kea_oid} (only RSA PKCS #1 v1.5 is read)")));
        }
        let cek = rsa_pkcs1_decrypt(&id.key, &ek.octets("encryptedKey")?)?;
        let plain = match content_oid.as_str() {
            OID_AES128_CBC => aes_decrypt::<aes::Aes128>(&cek, &iv, &encrypted)?,
            OID_AES192_CBC => aes_decrypt::<aes::Aes192>(&cek, &iv, &encrypted)?,
            OID_AES256_CBC => aes_decrypt::<aes::Aes256>(&cek, &iv, &encrypted)?,
            OID_DES_EDE3_CBC => cbc::Decryptor::<des::TdesEde3>::new_from_slices(&cek, &iv)
                .map_err(|_| bad("3DES key"))?
                .decrypt_padded_vec::<Pkcs7>(&encrypted)
                .map_err(|_| bad("the content does not decrypt"))?,
            other => return Err(SignError::Unsupported(format!("content encryption {other}"))),
        };
        return Ok(Some(plain));
    }
    Ok(None)
}

fn aes_decrypt<C>(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, SignError>
where
    C: cbc::cipher::BlockCipherDecrypt + cbc::cipher::KeyInit,
{
    cbc::Decryptor::<C>::new_from_slices(key, iv)
        .map_err(|_| bad("AES key"))?
        .decrypt_padded_vec::<Pkcs7>(data)
        .map_err(|_| bad("the content does not decrypt"))
}

/// The bytes a DER INTEGER of this unsigned value carries (no sign byte, no leading zeros).
fn der_uint_value(v: &[u8]) -> &[u8] {
    let start = v.iter().position(|b| *b != 0).unwrap_or(v.len().saturating_sub(1));
    &v[start..]
}

/// RSAES-PKCS1-v1_5 (RFC 8017 §7.2.1): `00 02 <random non-zero padding> 00 <message>`.
fn rsa_pkcs1_encrypt(n: &[u8], e: &[u8], message: &[u8]) -> Result<Vec<u8>, SignError> {
    let key = RsaPublicKey::new(BoxedUint::from_be_slice_vartime(n), BoxedUint::from_be_slice_vartime(e))
        .map_err(|e| SignError::Crypto(format!("RSA public key: {e}")))?;
    let k = n.iter().skip_while(|b| **b == 0).count();
    if message.len() + 11 > k {
        return Err(SignError::Crypto("the message is too long for the key".into()));
    }
    let mut em = vec![0u8, 2];
    let mut pad = Vec::with_capacity(k);
    while pad.len() < k - message.len() - 3 {
        for b in random(k)? {
            if b != 0 && pad.len() < k - message.len() - 3 {
                pad.push(b);
            }
        }
    }
    em.extend_from_slice(&pad);
    em.push(0);
    em.extend_from_slice(message);
    let c = rsa::hazmat::rsa_encrypt(&key, &BoxedUint::from_be_slice_vartime(&em)).map_err(|e| SignError::Crypto(format!("RSA: {e}")))?;
    let mut out = c.to_be_bytes().to_vec();
    // Fixed width: the modulus length.
    while out.len() > k && out[0] == 0 {
        out.remove(0);
    }
    while out.len() < k {
        out.insert(0, 0);
    }
    Ok(out)
}

/// A random source the decryption never draws on (no blinding is asked for).
struct NoRng;

impl rand_core::TryRng for NoRng {
    type Error = std::convert::Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Ok(0)
    }
    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        Ok(0)
    }
    fn try_fill_bytes(&mut self, _dst: &mut [u8]) -> Result<(), Self::Error> {
        Ok(())
    }
}
impl rand_core::TryCryptoRng for NoRng {}

/// RSAES-PKCS1-v1_5 decryption with an RSA private key read from its PKCS #8 encoding.
fn rsa_pkcs1_decrypt(key: &PrivateKey, encrypted: &[u8]) -> Result<Vec<u8>, SignError> {
    let private = rsa_private_key(key)?;
    let c = BoxedUint::from_be_slice_vartime(encrypted);
    let PublicKey::Rsa { n, .. } = key.public_key() else { return Err(SignError::Unsupported("RSA".into())) };
    let k = n.iter().skip_while(|b| **b == 0).count();
    let m = rsa::hazmat::rsa_decrypt(None::<&mut NoRng>, &private, &c).map_err(|_| SignError::Crypto("RSA decryption failed".into()))?;
    let mut em = m.to_be_bytes().to_vec();
    while em.len() > k && em[0] == 0 {
        em.remove(0);
    }
    while em.len() < k {
        em.insert(0, 0);
    }
    // 00 02 PS 00 M
    if em.len() < 11 || em[0] != 0 || em[1] != 2 {
        return Err(SignError::Crypto("the wrapped key does not unwrap with this certificate's key".into()));
    }
    let sep = em[2..].iter().position(|b| *b == 0).ok_or_else(|| SignError::Crypto("the wrapped key is malformed".into()))?;
    if sep < 8 {
        return Err(SignError::Crypto("the wrapped key is malformed".into()));
    }
    Ok(em[2 + sep + 1..].to_vec())
}

/// The RSA private key's numbers from its PKCS #8 `PrivateKeyInfo` (RFC 5208 + RFC 8017 A.1.2).
fn rsa_private_key(key: &PrivateKey) -> Result<RsaPrivateKey, SignError> {
    if !matches!(key.public_key(), PublicKey::Rsa { .. }) {
        return Err(SignError::Unsupported("opening a certificate-protected document needs an RSA key".into()));
    }
    let info = Tlv::parse_all(key.pkcs8()).map_err(|_| bad("PKCS #8"))?.expect(tag::SEQUENCE, "PrivateKeyInfo")?.children()?;
    let inner = info.get(2).ok_or_else(|| bad("PrivateKeyInfo"))?.octets("privateKey")?.into_owned();
    let rsa_key = Tlv::parse_all(&inner).map_err(|_| bad("RSAPrivateKey"))?.expect(tag::SEQUENCE, "RSAPrivateKey")?.children()?;
    let num = |i: usize, what: &str| -> Result<BoxedUint, SignError> {
        Ok(BoxedUint::from_be_slice_vartime(rsa_key.get(i).ok_or_else(|| bad(what))?.uint_bytes()))
    };
    let (n, e, d, p, q) = (num(1, "n")?, num(2, "e")?, num(3, "d")?, num(4, "p")?, num(5, "q")?);
    RsaPrivateKey::from_components(n, e, d, vec![p, q]).map_err(|e| SignError::Crypto(format!("RSA private key: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn an_envelope_opens_only_for_its_recipient() {
        let rsa_id = crate::pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
        let other = crate::pkcs12::open(&data("chain.p12"), "test").unwrap(); // Ada: another RSA key
        let ec_id = crate::pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
        let content = b"seed-seed-seed-seed-\xff\xff\xff\xfc";
        let cms = envelope(&rsa_id.certificate, content).unwrap();
        assert_eq!(open(&cms, &rsa_id).unwrap().as_deref(), Some(&content[..]));
        // Someone else's RSA key: not named, so not theirs; an EC identity cannot open any.
        assert_eq!(open(&cms, &other).unwrap(), None);
        assert!(envelope(&ec_id.certificate, content).is_err(), "no key transport for EC");
        // A damaged wrapped key fails loudly (the PKCS #1 padding no longer checks); damaged
        // content comes out as other bytes, which the file key derived from it then exposes.
        let mut broken = cms.clone();
        let at = broken.len() - 100; // inside the RSA-wrapped key, which precedes the content
        broken[at] ^= 0x55;
        assert!(open(&broken, &rsa_id).is_err());
        let mut damaged = cms.clone();
        let at = damaged.len() - 20; // inside the AES content
        damaged[at] ^= 0x55;
        match open(&damaged, &rsa_id) {
            Ok(Some(bytes)) => assert_ne!(bytes, content),
            Ok(None) => panic!("still addressed to the recipient"),
            Err(_) => {} // the padding no longer checks
        }
        // Two recipients in one file: each opens their own.
        let cms_other = envelope(&other.certificate, content).unwrap();
        assert_eq!(open(&cms_other, &other).unwrap().as_deref(), Some(&content[..]));
        assert_eq!(open(&cms_other, &rsa_id).unwrap(), None);
    }

    #[test]
    fn pkcs1_padding_round_trips_at_the_key_size() {
        let id = crate::pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
        let PublicKey::Rsa { n, e } = id.certificate.public_key.clone() else { panic!("RSA") };
        let msg = [7u8; 32];
        let c = rsa_pkcs1_encrypt(&n, &e, &msg).unwrap();
        assert_eq!(c.len(), n.iter().skip_while(|b| **b == 0).count());
        assert_eq!(rsa_pkcs1_decrypt(&id.key, &c).unwrap(), msg);
        assert!(rsa_pkcs1_encrypt(&n, &e, &[0u8; 300]).is_err(), "too long for the key");
    }
}
