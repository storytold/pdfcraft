//! Digital IDs from PKCS #11 tokens and software modules: a certificate with a private key
//! that never leaves the device, only signs on request. This is the common language of the
//! smart cards and cryptographic modules used on Linux (Rutoken, JaCarta, NSS's software
//! token, …), coordinated system-wide by p11-kit.
//!
//! The module talks to tokens through the [`cryptoki`] wrapper (the way `security-framework`
//! wraps the macOS Keychain and `rustls-cng` wraps the Windows store): enumerate slots →
//! certificates → the private key matching each certificate by its PKCS #11 `CKA_ID`.
//! Signing goes through [`crate::keys::ExternalKey`], like those stores: PdfCraft never sees
//! the key material, it only asks the device to sign. A token that requires authentication
//! is given its PIN through [`find_with_pin`]; sessions the environment has already
//! authenticated sign without one.
//!
//! RSA (PKCS #1 v1.5) and ECDSA keys sign. GOST keys (Rutoken ECP and friends) enumerate
//! but are left out of the list for now: the signature formats above only carry RSA/ECDSA
//! algorithm identifiers, and GOST signing arrives with the GOST integration.

use std::sync::{Mutex, OnceLock};

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error, RvError};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, KeyType, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;

use crate::keys::{DigestAlg, ExternalKey, PrivateKey, PublicKey};
use crate::{Certificate, DigitalId, SignError};

/// Loads a PKCS #11 module and initializes it for multi-threaded use.
fn load(path: &str) -> Result<Pkcs11, SignError> {
    let client = Pkcs11::new(path).map_err(|e| SignError::Crypto(format!("{}: {e}", module_name(path))))?;
    match client.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
        Ok(()) => {}
        // The proxy (or another library in the process) may have initialized the module
        // already; that state is exactly the one we want.
        Err(Error::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => {}
        Err(e) => return Err(SignError::Crypto(format!("{}: {e}", module_name(path)))),
    }
    Ok(client)
}

fn module_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn module_paths() -> Vec<String> {
    if let Ok(list) = std::env::var("PDFCRAFT_PKCS11_MODULE") {
        return list.split(':').filter(|p| !p.is_empty()).map(String::from).collect();
    }
    // The p11-kit proxy is the system-wide aggregate; `p11-kit list-modules` shows what it
    // covers. Its install path varies by multiarch tuple.
    ["/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu", "/usr/lib64", "/usr/lib"]
        .iter()
        .map(|dir| format!("{dir}/p11-kit-proxy.so"))
        .collect()
}

/// Loaded modules, initialized once and kept for the process.
fn modules() -> Vec<Pkcs11> {
    static LOADED: OnceLock<Mutex<Vec<(String, Option<Pkcs11>)>>> = OnceLock::new();
    let loaded = LOADED.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = match loaded.lock() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for path in module_paths() {
        if let Some((_, client)) = guard.iter().find(|(p, _)| *p == path) {
            if let Some(client) = client {
                out.push(client.clone());
            }
            continue;
        }
        let client = load(&path).ok();
        if let Some(client) = &client {
            out.push(client.clone());
        }
        guard.push((path, client));
    }
    out
}

/// The X.509 certificate objects on this session the app could use.
fn token_certificates(session: &Session) -> Vec<(Certificate, Vec<u8>)> {
    let Ok(objects) = session.find_objects(&[Attribute::Class(ObjectClass::CERTIFICATE)]) else { return Vec::new() };
    let mut out = Vec::new();
    for object in objects {
        let Ok(attributes) = session.get_attributes(object, &[AttributeType::Value, AttributeType::Id]) else { continue };
        let mut value: Option<Vec<u8>> = None;
        let mut id = Vec::new();
        for attribute in attributes {
            match attribute {
                Attribute::Value(bytes) => value = Some(bytes),
                Attribute::Id(bytes) => id = bytes,
                _ => {}
            }
        }
        let Some(value) = value else { continue };
        if value.is_empty() {
            continue;
        }
        let Ok(certificate) = Certificate::parse(&value) else { continue };
        // Keys PdfCraft can't represent yet (GOST, Ed25519 on tokens) are left for their
        // stage; an identity must be able to sign.
        if matches!(certificate.public_key, PublicKey::Unsupported { .. }) {
            continue;
        }
        out.push((certificate, id));
    }
    out
}

/// The private key on this session matching `ckaid`, with whether it is an RSA key.
fn session_key(session: &Session, ckaid: &[u8]) -> Option<(ObjectHandle, bool)> {
    let objects = session.find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(ckaid.to_vec())]).ok()?;
    let object = *objects.first()?;
    let Ok(attributes) = session.get_attributes(object, &[AttributeType::KeyType]) else { return Some((object, false)) };
    let key_type = attributes.first().and_then(|a| match a {
        Attribute::KeyType(k) => Some(*k),
        _ => None,
    });
    // An unknown key type is left to the sign call to refuse.
    match key_type {
        None => Some((object, false)),
        Some(k) if k == KeyType::RSA || k == KeyType::EC => Some((object, k == KeyType::RSA)),
        Some(_) => None,
    }
}

/// The `pkcs11:<SHA-256 of the certificate>` reference PdfCraft keeps for an identity.
pub fn reference(c: &Certificate) -> String {
    let d = DigestAlg::Sha256.digest(&[&c.raw]);
    format!("pkcs11:{}", d.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn trimmed_label(label: &str) -> String {
    label.trim_end_matches(['\0', ' ']).trim().to_string()
}

/// The other certificates on the same token (the issuers), best effort like the Keychain's.
fn token_chain(session: &Session, signer: &Certificate) -> Vec<Certificate> {
    token_certificates(session).into_iter().filter(|(c, _)| c.raw != signer.raw).map(|(c, _)| c).collect()
}

/// The signing identities across all PKCS #11 modules the environment configures: a
/// certificate with a matching private key that may sign. Tokens that need authentication
/// still list their certificates (they are readable); signing asks for the PIN then.
pub fn list() -> Result<Vec<DigitalId>, SignError> {
    let mut out: Vec<DigitalId> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut any_module = false;
    for client in modules() {
        any_module = true;
        let Ok(slots) = client.get_slots_with_token() else { continue };
        for slot in slots {
            let Ok(token) = client.get_token_info(slot) else { continue };
            let token_label = trimmed_label(token.label());
            let login_required = token.login_required();
            // Some modules (SoftHSM among them) only take a login on a read-write session.
            let Ok(session) = client.open_rw_session(slot) else { continue };
            for (certificate, id) in token_certificates(&session) {
                if session_key(&session, &id).is_none() && !login_required {
                    // No visible key: without login, keys are hidden (CKA_PRIVATE), so a
                    // token that requires login keeps the pairing unproven until the PIN
                    // flow at sign time; a token that doesn't simply has no key.
                    continue;
                }
                let key_reference = reference(&certificate);
                if seen.contains(&key_reference) {
                    continue; // The proxy may surface a module's token more than once.
                }
                seen.push(key_reference);
                let chain = token_chain(&session, &certificate);
                out.push(DigitalId {
                    key: PrivateKey::external(
                        certificate.public_key.clone(),
                        std::sync::Arc::new(Pkcs11Key { module: client.clone(), slot, ckaid: id.clone(), pin: Mutex::new(None) }),
                    ),
                    certificate,
                    chain,
                    friendly_name: (!token_label.is_empty()).then(|| token_label.clone()),
                });
            }
        }
    }
    if !any_module {
        return Err(SignError::Crypto("no PKCS #11 module was found (p11-kit or PDFCRAFT_PKCS11_MODULE)".into()));
    }
    Ok(out)
}

/// The identity a `pkcs11:` reference (or a certificate common name) names, optionally
/// signed in with its PIN for tokens that require it.
pub fn find_with_pin(reference_or_name: &str, pin: Option<&str>) -> Result<DigitalId, SignError> {
    for client in modules() {
        let Ok(slots) = client.get_slots_with_token() else { continue };
        for slot in slots {
            let Ok(token) = client.get_token_info(slot) else { continue };
            let login_required = token.login_required();
            let Ok(session) = client.open_rw_session(slot) else { continue };
            if let Some(pin) = pin {
                // Log in before the search when the token wants it: the private keys are
                // hidden (CKA_PRIVATE) until then. A wrong PIN surfaces at the sign call,
                // so a lookup that finds nothing shouldn't mask why.
                if login_required {
                    let _ = login(&session, pin);
                }
            }
            for (certificate, id) in token_certificates(&session) {
                let by_name = certificate.subject.common_name() == Some(reference_or_name.trim_start_matches("pkcs11:"));
                if reference(&certificate) != reference_or_name && !by_name {
                    continue;
                }
                if session_key(&session, &id).is_none() && !login_required {
                    continue; // The key isn't there and can't be hiding behind a login.
                }
                let chain = token_chain(&session, &certificate);
                return Ok(DigitalId {
                    key: PrivateKey::external(
                        certificate.public_key.clone(),
                        std::sync::Arc::new(Pkcs11Key { module: client.clone(), slot, ckaid: id.clone(), pin: Mutex::new(pin.map(String::from)) }),
                    ),
                    certificate,
                    chain,
                    friendly_name: None,
                });
            }
        }
    }
    Err(SignError::Crypto(format!("no PKCS #11 identity {reference_or_name}")))
}

/// The identity a reference (or certificate common name) names, without a PIN.
pub fn find(reference_or_name: &str) -> Result<DigitalId, SignError> {
    find_with_pin(reference_or_name, None)
}

/// Signs in on the session, mapping the PIN errors a UI can act on.
fn login(session: &Session, pin: &str) -> Result<(), SignError> {
    match session.login(UserType::User, Some(&AuthPin::new(pin.into()))) {
        Ok(()) => Ok(()),
        Err(Error::Pkcs11(RvError::UserAlreadyLoggedIn, _)) => Ok(()),
        Err(Error::Pkcs11(RvError::PinIncorrect, _)) => Err(SignError::Crypto("the PIN is incorrect".into())),
        Err(Error::Pkcs11(RvError::PinLocked, _)) => Err(SignError::Crypto("too many failed PIN attempts; the token is locked".into())),
        Err(e) => Err(SignError::Crypto(format!("the token rejected the sign-in ({e})"))),
    }
}

/// A key that stays on the token: PdfCraft asks the device to sign, session by session.
struct Pkcs11Key {
    module: Pkcs11,
    slot: Slot,
    ckaid: Vec<u8>,
    /// The PIN, when `find_with_pin` was given one; sessions opened at sign time log in
    /// with it when the token requires authentication.
    pin: Mutex<Option<String>>,
}

impl ExternalKey for Pkcs11Key {
    /// Sign `msg`, hashing it with `alg` (PKCS #1 v1.5 for RSA, DER-encoded ECDSA).
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        // One retry: a session can go stale (the token was re-plugged, the daemon reset
        // it); a fresh session on the second pass picks the key up again.
        let mut last = Err(SignError::Crypto("the token didn't sign".into()));
        for attempt in 0..2 {
            match self.sign_once(alg, msg) {
                Ok(sig) => return Ok(sig),
                Err(SignError::Crypto(e)) if attempt == 0 && e.starts_with("session") => last = Err(SignError::Crypto(e)),
                Err(e) => return Err(e),
            }
        }
        last
    }
}

impl Pkcs11Key {
    fn sign_once(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let Ok(token) = self.module.get_token_info(self.slot) else { return Err(SignError::Crypto("the token is not present".into())) };
        // Some modules (SoftHSM among them) only take a login on a read-write session.
        let session = self.module.open_rw_session(self.slot).map_err(|e| SignError::Crypto(format!("session: {e}")))?;
        if token.login_required() {
            let pin = self.pin.lock().ok().and_then(|p| p.clone());
            if let Some(pin) = pin {
                login(&session, &pin)?;
            }
            // Without a PIN the sign call below refuses with a message the UI can act on.
        }
        let Some((key, rsa)) = session_key(&session, &self.ckaid) else {
            return Err(SignError::Crypto("the token lost its private key".into()));
        };
        let digest = alg.digest(&[msg]);
        let data: Vec<u8> = if rsa { crate::rsa_pad::digest_info(alg, &digest, true) } else { digest };
        let mechanism = if rsa { Mechanism::RsaPkcs } else { Mechanism::Ecdsa };
        let signature = session.sign(&mechanism, key, &data).map_err(|e| match e {
            Error::Pkcs11(RvError::UserNotLoggedIn, _) => SignError::Crypto("the token requires its PIN to sign".into()),
            other => SignError::Crypto(format!("the token didn't sign ({other})")),
        })?;
        if rsa {
            return Ok(signature);
        }
        // ECDSA signatures come back as raw `r || s`; PdfCraft carries DER.
        ecdsa_raw_to_der(&signature).ok_or_else(|| SignError::Crypto("the token returned a malformed ECDSA signature".into()))
    }
}

/// `r || s` of equal halves → DER `SEQUENCE { r INTEGER, s INTEGER }`.
fn ecdsa_raw_to_der(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.is_empty() || raw.len() % 2 != 0 {
        return None;
    }
    let half = raw.len() / 2;
    let (r, s) = (der_integer(&raw[..half])?, der_integer(&raw[half..])?);
    let len = r.len() + s.len();
    let mut out = Vec::with_capacity(len + 4);
    out.push(0x30);
    if len < 128 {
        out.push(len as u8);
    } else {
        out.push(0x81);
        out.push(len as u8);
    }
    out.extend(r);
    out.extend(s);
    Some(out)
}

fn der_integer(bytes: &[u8]) -> Option<Vec<u8>> {
    let start = bytes.iter().position(|b| *b != 0)?;
    let body = &bytes[start..];
    let needs_pad = body[0] & 0x80 != 0;
    let len = body.len() + usize::from(needs_pad);
    if len >= 128 {
        return None; // A signature half is far shorter than 128 bytes.
    }
    let mut out = Vec::with_capacity(len + 2);
    out.push(0x02);
    out.push(len as u8);
    if needs_pad {
        out.push(0);
    }
    out.extend_from_slice(body);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecdsa_raw_signature_becomes_der() {
        // r = 1, s = 0x81 (needs a zero pad): minimal encodings.
        let raw = [0, 0, 0, 1, 0, 0, 0, 0x81];
        // r = 1 → `02 01 01`; s = 0x81 → `02 02 00 81`; SEQUENCE of the two = 7 bytes.
        let der = ecdsa_raw_to_der(&raw).expect("encodes");
        assert_eq!(der, vec![0x30, 0x07, 0x02, 0x01, 0x01, 0x02, 0x02, 0x00, 0x81]);
    }

    #[test]
    fn ecdsa_raw_signature_rejects_odd_input() {
        assert!(ecdsa_raw_to_der(&[1, 2, 3]).is_none());
        assert!(ecdsa_raw_to_der(&[]).is_none());
    }

    #[test]
    fn reference_is_a_sha256_fingerprint() {
        // The test CA fixture, PEM-wrapped.
        let pem = include_str!("../tests/data/ca.crt.pem");
        let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).expect("PEM decodes");
        let cert = crate::x509::Certificate::parse(&der).expect("test CA parses");
        let r = reference(&cert);
        assert!(r.starts_with("pkcs11:"), "{r}");
        assert_eq!(r.len(), "pkcs11:".len() + 64);
    }
}
