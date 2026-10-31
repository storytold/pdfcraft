// Adapted from https://github.com/storytold/pdfcraft/pull/4968 (MIT OR Apache-2.0).
//! In-process provider implementation, reachable only from the worker dispatch.
use std::sync::{Mutex, OnceLock};

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error, RvError};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, KeyType, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;

use crate::keys::{DigestAlg, ExternalKey, PrivateKey, Scheme};
use crate::{Certificate, DigitalId, SignError};
use zeroize::Zeroizing;

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
    // OpenSC install paths vary by distribution and multiarch tuple.
    ["/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu", "/usr/lib64", "/usr/lib"]
        .iter()
        .flat_map(|dir| [format!("{dir}/opensc-pkcs11.so"), format!("{dir}/pkcs11/opensc-pkcs11.so")])
        .collect()
}

// OpenSC is commonly installed at both a real path and a pkcs11/ symlink. Preserve
// provider order, but initialize/enumerate each physical library only once.
fn unique_module_paths(paths: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    paths
        .into_iter()
        .filter(|path| {
            let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
            seen.insert(canonical)
        })
        .collect()
}

/// Loaded modules, initialized once and kept for the process.
fn modules() -> Vec<Pkcs11> {
    type LoadedModules = Vec<(String, Option<Pkcs11>)>;
    static LOADED: OnceLock<Mutex<LoadedModules>> = OnceLock::new();
    let loaded = LOADED.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = match loaded.lock() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for path in unique_module_paths(module_paths()) {
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

/// A session on this slot. Some modules (SoftHSM among them) only take a login on a
/// read-write session, so that is what is tried first; write-protected tokens refuse a
/// RW session and get a read-only one (reads work the same, login then can't).
fn open_session(client: &Pkcs11, slot: Slot) -> Result<Session, SignError> {
    client.open_rw_session(slot).or_else(|_| client.open_ro_session(slot)).map_err(|e| SignError::Crypto(format!("session: {e}")))
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
        // Every parseable certificate: the chain (token_chain) needs the intermediates
        // too, including ones whose key type PdfCraft doesn't sign with.
        out.push((certificate, id));
    }
    out
}

/// The private key on this session matching `ckaid`, with whether it is an RSA key.
fn session_key(session: &Session, ckaid: &[u8]) -> Option<(ObjectHandle, bool)> {
    let objects = session.find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(ckaid.to_vec())]).ok()?;
    if objects.len() != 1 || ckaid.is_empty() {
        return None;
    }
    let object = *objects.first()?;
    let Ok(attributes) = session.get_attributes(object, &[AttributeType::KeyType]) else { return None };
    let key_type = attributes.first().and_then(|a| match a {
        Attribute::KeyType(k) => Some(*k),
        _ => None,
    });
    // An unknown key type is left to the sign call to refuse.
    match key_type {
        None => None,
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
pub(super) fn list() -> Result<Vec<(DigitalId, String)>, SignError> {
    let mut out = Vec::new();
    let mut any_module = false;
    for client in modules() {
        any_module = true;
        let Ok(slots) = client.get_slots_with_token() else { continue };
        for slot in slots {
            let Ok(token) = client.get_token_info(slot) else { continue };
            let token_label = trimmed_label(token.label());
            let login_required = token.login_required();
            // Enumerate without authenticating; hidden private keys are checked at signing.
            let Ok(session) = open_session(&client, slot) else { continue };
            for (certificate, id) in token_certificates(&session) {
                // Signing identities only (RSA, P-256, P-384); Ed25519, P-521 and brainpool
                // keys would list only to fail at sign time with a misleading message.
                // Chain certificates keep all key types (token_chain above).
                if certificate.public_key.store_signing_key().is_none() || certificate.signing_problem().is_some() {
                    continue;
                }
                if session_key(&session, &id).is_none() && !login_required {
                    // No visible key: without login, keys are hidden (CKA_PRIVATE), so a
                    // token that requires login keeps the pairing unproven until the PIN
                    // flow at sign time; a token that doesn't simply has no key.
                    continue;
                }
                let chain = token_chain(&session, &certificate);
                out.push((
                    DigitalId {
                        key: PrivateKey::external(
                            certificate.public_key.clone(),
                            std::sync::Arc::new(Pkcs11Key {
                                module: client.clone(),
                                slot,
                                serial: token.serial_number().to_owned(),
                                ckaid: id.clone(),
                                pin: Mutex::new(None),
                                certificate: certificate.clone(),
                            }),
                        ),
                        certificate,
                        chain,
                        friendly_name: (!token_label.is_empty()).then(|| token_label.clone()),
                    },
                    token.serial_number().to_owned(),
                ));
            }
        }
    }
    if !any_module {
        return Err(SignError::Crypto("no PKCS #11 module was found; install OpenSC or configure PDFCRAFT_PKCS11_MODULE".into()));
    }
    Ok(out)
}

/// The identity a `pkcs11:` reference (or a certificate common name) names, optionally
/// signed in with its PIN for tokens that require it.
pub(super) fn find_with_pin(reference_or_name: &str, pin: Option<&str>, serial: &str) -> Result<DigitalId, SignError> {
    let mut matches = Vec::new();
    for client in modules() {
        let Ok(slots) = client.get_slots_with_token() else { continue };
        for slot in slots {
            let Ok(token) = client.get_token_info(slot) else { continue };
            if token.serial_number() != serial {
                continue;
            }
            let login_required = token.login_required();
            let Ok(session) = open_session(&client, slot) else { continue };
            for (certificate, id) in token_certificates(&session) {
                // A signable identity (RSA, P-256, P-384); references to other certificates
                // resolve to nothing rather than to an identity that can't sign.
                if certificate.public_key.store_signing_key().is_none() || certificate.signing_problem().is_some() {
                    continue;
                }
                let by_name = certificate.subject.common_name() == Some(reference_or_name.trim_start_matches("pkcs11:"));
                if reference(&certificate) != reference_or_name && !by_name {
                    continue;
                }
                // Log in only on the token that holds the certificate: the private keys
                // are hidden (CKA_PRIVATE) until then, and a wrong PIN must not burn
                // failed-login attempts on unrelated tokens. The error propagates, so a
                // wrong PIN doesn't reach the sign call either.
                if session_key(&session, &id).is_none() && !login_required {
                    continue; // The key isn't there and can't be hiding behind a login.
                }
                let chain = token_chain(&session, &certificate);
                matches.push(DigitalId {
                    key: PrivateKey::external(
                        certificate.public_key.clone(),
                        std::sync::Arc::new(Pkcs11Key {
                            module: client.clone(),
                            slot,
                            serial: token.serial_number().to_owned(),
                            ckaid: id.clone(),
                            pin: Mutex::new(pin.map(|p| Zeroizing::new(p.to_owned()))),
                            certificate: certificate.clone(),
                        }),
                    ),
                    certificate,
                    chain,
                    friendly_name: None,
                });
            }
        }
    }
    if matches.len() != 1 {
        return Err(SignError::TokenChanged);
    }
    matches.pop().ok_or(SignError::TokenChanged)
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
    serial: String,
    ckaid: Vec<u8>,
    /// The PIN, when `find_with_pin` was given one; sessions opened at sign time log in
    /// with it when the token requires authentication.
    pin: Mutex<Option<Zeroizing<String>>>,
    certificate: Certificate,
}

impl ExternalKey for Pkcs11Key {
    /// Sign `msg`, hashing it with `alg` (PKCS #1 v1.5 for RSA, DER-encoded ECDSA).
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        // PIN-bearing operations are never retried automatically.
        self.sign_once(alg, msg)
    }
}

impl Pkcs11Key {
    fn sign_once(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let Ok(token) = self.module.get_token_info(self.slot) else { return Err(SignError::TokenChanged) };
        if token.serial_number() != self.serial {
            return Err(SignError::TokenChanged);
        }
        // Some modules (SoftHSM among them) only take a login on a read-write session.
        let session = open_session(&self.module, self.slot)?;
        // Recheck the certificate before sending a PIN to a potentially replaced card.
        if !token_certificates(&session).iter().any(|(cert, id)| cert.raw == self.certificate.raw && *id == self.ckaid) {
            return Err(SignError::TokenChanged);
        }
        // Opening a session can cross a hotplug transition. Recheck after the certificate
        // lookup, before obtaining a PIN or using any authentication policy flags.
        let token = self.module.get_token_info(self.slot).map_err(|_| SignError::TokenChanged)?;
        if token.serial_number() != self.serial {
            return Err(SignError::TokenChanged);
        }
        let pin = self.pin.lock().map_err(|_| SignError::Crypto("the token PIN state is unavailable".into()))?.take();
        let protected = token.protected_authentication_path();
        if token.login_required() {
            if protected {
                session.login(UserType::User, None).map_err(|e| SignError::Crypto(format!("token authentication failed: {e}")))?;
            } else {
                let secret = pin.as_deref().ok_or_else(|| SignError::Crypto("the token requires its PIN".into()))?;
                login(&session, secret)?;
            }
        }
        let Some((key, rsa)) = session_key(&session, &self.ckaid) else {
            return Err(SignError::Crypto("the token lost its private key".into()));
        };
        let digest = alg.digest(&[msg]);
        let data: Vec<u8> = if rsa { crate::rsa_pad::digest_info(alg, &digest, true) } else { digest };
        let mechanism = if rsa { Mechanism::RsaPkcs } else { Mechanism::Ecdsa };
        let allowed = session.get_attributes(key, &[AttributeType::Sign]).map_err(|e| SignError::Crypto(format!("read key capabilities: {e}")))?;
        if !allowed.iter().any(|a| matches!(a, Attribute::Sign(true))) {
            return Err(SignError::Crypto("the selected key does not permit signing".into()));
        }
        let always = session
            .get_attributes(key, &[AttributeType::AlwaysAuthenticate])
            .map_err(|e| SignError::Crypto(format!("read key authentication policy: {e}")))?
            .iter()
            .any(|a| matches!(a, Attribute::AlwaysAuthenticate(true)));
        session.sign_init(&mechanism, key).map_err(|e| SignError::Crypto(format!("begin token signature: {e}")))?;
        if always {
            let auth = pin.as_ref().map(|p| AuthPin::new(p.as_str().into()));
            if !protected && auth.is_none() {
                return Err(SignError::Crypto("the signing key requires a PIN for each use".into()));
            }
            session
                .login(UserType::ContextSpecific, if protected { None } else { auth.as_ref() })
                .map_err(|e| SignError::Crypto(format!("signing-key authentication failed: {e}")))?;
        }
        let signature = session.sign_single(&data).map_err(|e| match e {
            Error::Pkcs11(RvError::UserNotLoggedIn, _) => SignError::Crypto("the token requires its PIN to sign".into()),
            other => SignError::Crypto(format!("the token didn't sign ({other})")),
        })?;
        let signature = self.certificate.public_key.store_signature_der(&signature)?;
        let scheme = if rsa { Scheme::RsaPkcs1 } else { Scheme::Ecdsa };
        if !self.certificate.public_key.verify(scheme, alg, &alg.digest(&[msg]), &signature)? {
            return Err(SignError::Crypto("the token signature does not match the selected certificate".into()));
        }
        Ok(signature)
    }
}

/// A card's RSA key-management identity, independent of its signing identities.
/// Expired certificates remain eligible for opening historical encrypted files.
/// Device handles and PINs are never serialized.
pub struct RsaDecryptionIdentity {
    module: Pkcs11,
    slot: Slot,
    serial: String,
    ckaid: Vec<u8>,
    certificate: Certificate,
    token_label: String,
}

impl std::fmt::Debug for RsaDecryptionIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RsaDecryptionIdentity")
            .field("token_label", &self.token_label)
            .field("reference", &reference(&self.certificate))
            .finish_non_exhaustive()
    }
}

impl RsaDecryptionIdentity {
    pub fn certificate(&self) -> &Certificate {
        &self.certificate
    }

    /// Public device identity for rejecting a replacement card before using a cached PIN.
    pub fn token_serial(&self) -> &str {
        &self.serial
    }

    pub fn token_label(&self) -> &str {
        &self.token_label
    }

    /// Decrypt one RSA PKCS #1 v1.5 CMS content key on the card. The PIN is borrowed for
    /// this operation only. Removal, replacement, and authentication errors never retry.
    pub fn decrypt_key(&self, encrypted: &[u8], pin: Option<&str>) -> Result<Zeroizing<Vec<u8>>, SignError> {
        super::require_token_serial(&self.serial)?;
        let crate::PublicKey::Rsa { n, .. } = &self.certificate.public_key else {
            return Err(SignError::Unsupported("recipient decryption requires an RSA key".into()));
        };
        if encrypted.len() != n.len() || encrypted.len() > 1024 {
            return Err(SignError::Malformed("recipient RSA ciphertext has an invalid length".into()));
        }
        let token = self.module.get_token_info(self.slot).map_err(|_| SignError::Crypto("the selected token is not present".into()))?;
        if token.serial_number() != self.serial {
            return Err(SignError::Crypto("the selected token was replaced; refresh the identities".into()));
        }
        let session = open_session(&self.module, self.slot)?;
        if !token_certificates(&session).iter().any(|(cert, id)| cert.raw == self.certificate.raw && *id == self.ckaid) {
            return Err(SignError::Crypto("the selected encryption certificate is no longer present".into()));
        }
        // Recheck the post-open device before sending a PIN to its session.
        let token = self.module.get_token_info(self.slot).map_err(|_| SignError::TokenChanged)?;
        if token.serial_number() != self.serial {
            return Err(SignError::TokenChanged);
        }
        let protected = token.protected_authentication_path();
        if token.login_required() {
            if protected {
                session.login(UserType::User, None).map_err(|e| SignError::Crypto(format!("token authentication failed: {e}")))?;
            } else {
                login(&session, pin.ok_or_else(|| SignError::Crypto("the token requires its PIN".into()))?)?;
            }
        }
        let Some((key, true)) = session_key(&session, &self.ckaid) else {
            return Err(SignError::Crypto("the token has no unique RSA private key for this certificate".into()));
        };
        let allowed =
            session.get_attributes(key, &[AttributeType::Decrypt]).map_err(|e| SignError::Crypto(format!("read decryption capability: {e}")))?;
        if !allowed.iter().any(|a| matches!(a, Attribute::Decrypt(true))) {
            return Err(SignError::Crypto("the selected key does not permit decryption".into()));
        }
        let always = session
            .get_attributes(key, &[AttributeType::AlwaysAuthenticate])
            .map_err(|e| SignError::Crypto(format!("read key authentication policy: {e}")))?
            .iter()
            .any(|a| matches!(a, Attribute::AlwaysAuthenticate(true)));
        session.decrypt_init(&Mechanism::RsaPkcs, key).map_err(|e| SignError::Crypto(format!("begin token decryption: {e}")))?;
        if always {
            let auth = pin.map(|p| AuthPin::new(p.into()));
            if !protected && auth.is_none() {
                return Err(SignError::Crypto("the decryption key requires a PIN for each use".into()));
            }
            session
                .login(UserType::ContextSpecific, if protected { None } else { auth.as_ref() })
                .map_err(|e| SignError::Crypto(format!("decryption-key authentication failed: {e}")))?;
        }
        session.decrypt_single(encrypted).map(Zeroizing::new).map_err(|_| SignError::Crypto("the token could not decrypt the recipient key".into()))
    }
}

/// Enumerate RSA key-management certificates without logging into any card. Private keys
/// hidden until login are checked when `decrypt_key` executes, never assumed absent.
pub fn decryption_identities() -> Result<Vec<RsaDecryptionIdentity>, SignError> {
    discover_available(
        modules(),
        |module| module.get_slots_with_token().map_err(|e| SignError::Crypto(format!("list token slots: {e}"))),
        |module, slot| {
            let token = module.get_token_info(slot).map_err(|e| SignError::Crypto(format!("read token information: {e}")))?;
            if !token.token_initialized() {
                return Ok(Vec::new());
            }
            let session = open_session(module, slot)?;
            Ok(token_certificates(&session)
                .into_iter()
                .filter_map(|(certificate, ckaid)| {
                    if ckaid.is_empty() || crate::envelope::recipient_problem(&certificate).is_some() {
                        return None;
                    }
                    Some(RsaDecryptionIdentity {
                        module: module.clone(),
                        slot,
                        serial: token.serial_number().to_owned(),
                        ckaid,
                        certificate,
                        token_label: trimmed_label(token.label()),
                    })
                })
                .collect())
        },
    )
}

// Discovery never authenticates. One provider or reader going away must not hide
// identities on the remaining readers. Keep the no-provider diagnostic distinct.
fn discover_available<C, S, I>(
    clients: Vec<C>,
    mut slots: impl FnMut(&C) -> Result<Vec<S>, SignError>,
    mut identities: impl FnMut(&C, S) -> Result<Vec<I>, SignError>,
) -> Result<Vec<I>, SignError> {
    if clients.is_empty() {
        return Err(SignError::Crypto("no PKCS #11 module was found; install OpenSC or configure PDFCRAFT_PKCS11_MODULE".into()));
    }
    let mut result = Vec::new();
    for client in clients {
        let Ok(available) = slots(&client) else { continue };
        for slot in available {
            if let Ok(found) = identities(&client, slot) {
                result.extend(found);
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decryption_discovery_keeps_healthy_cards_after_reader_and_provider_failures() {
        use std::cell::RefCell;
        let visited = RefCell::new(Vec::new());
        let ids = discover_available(
            vec![0, 1, 2],
            |module| if *module == 0 { Err(SignError::Crypto("provider unavailable".into())) } else { Ok(vec![0, 1, 2]) },
            |module, slot| {
                visited.borrow_mut().push((*module, slot));
                match slot {
                    0 => Err(SignError::Crypto("token information unavailable".into())),
                    1 => Err(SignError::Crypto("session refused".into())),
                    _ => Ok(vec![format!("healthy-{module}")]),
                }
            },
        )
        .unwrap();
        assert_eq!(ids, ["healthy-1", "healthy-2"]);
        assert_eq!(*visited.borrow(), [(1, 0), (1, 1), (1, 2), (2, 0), (2, 1), (2, 2)]);
        assert!(discover_available::<(), (), ()>(vec![], |_| Ok(vec![]), |_, _| Ok(vec![])).unwrap_err().to_string().contains("no PKCS #11 module"));
        assert!(discover_available::<(), (), ()>(vec![()], |_| Ok(vec![]), |_, _| Ok(vec![])).unwrap().is_empty());
    }

    #[test]
    fn module_path_aliases_do_not_duplicate_a_card() {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pdfcraft-pkcs11-alias-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("pkcs11")).unwrap();
        let actual = dir.join("module.so");
        let alias = dir.join("pkcs11/module.so");
        std::fs::write(&actual, b"synthetic path only; never loaded").unwrap();
        std::os::unix::fs::symlink(&actual, &alias).unwrap();
        let paths = unique_module_paths(vec![
            actual.to_string_lossy().into_owned(),
            alias.to_string_lossy().into_owned(),
            actual.to_string_lossy().into_owned(),
        ]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(paths.len(), 1, "one provider must not enumerate the same card through two library aliases");
        assert_eq!(std::path::Path::new(&paths[0]), actual);
    }

    #[test]
    fn reference_is_a_sha256_fingerprint() {
        // The test CA fixture, PEM-wrapped.
        let pem = include_str!("../../tests/data/ca.crt.pem");
        let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).expect("PEM decodes");
        let cert = crate::x509::Certificate::parse(&der).expect("test CA parses");
        let r = reference(&cert);
        assert!(r.starts_with("pkcs11:"), "{r}");
        assert_eq!(r.len(), "pkcs11:".len() + 64);
    }
}
