//! Digital IDs on PKCS #11 tokens and smart cards (the CV cryptovision card, a PIV or CAC
//! token, SoftHSM): the private key never leaves the card, which asks for its PIN when used.
//!
//! The PKCS #11 module is loaded at run time (never linked): the paths in
//! `PDFCRAFT_PKCS11_MODULES` (colon-separated), else the p11-kit proxy, which itself loads the
//! modules a system has registered (`/usr/local/etc/pkcs11/`, `/etc/pkcs11/modules/*.module`).
//! Nothing is loaded when neither is present, so the app runs without any module installed.
//!
//! Only RSA, P-256 and P-384 keys with `sha256/384/512`-with-RSA or ECDSA mechanisms are signed
//! with, the same keys the Windows store signs with. Certificates a token carries that can't
//! sign (no private key, another key type, a key usage without digitalSignature/nonRepudiation)
//! are reported in [`Listing::unusable`] with the reason, so a missing identity can be
//! explained instead of silently dropped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error as CkError, RvError};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, ObjectClass};
use cryptoki::session::UserType;
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;

use crate::keys::{DigestAlg, ExternalKey, PrivateKey, PublicKey};
use crate::{Certificate, DigitalId, SignError, x509};

/// A signing identity on a token, with the `pkcs11:<token label>:<SHA-256>` reference that
/// [`find`] resolves and agents pass as `id`.
pub struct Identity {
    pub reference: String,
    pub id: DigitalId,
    token_label: String,
    cka_label: String,
}

/// A token certificate PdfCraft can't sign with, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    /// The subject's common name (else organization, else the DN), a token's label, or a module
    /// path when the module itself couldn't be loaded.
    pub subject: String,
    /// The `pkcs11:` reference, when a certificate was read (else empty).
    pub reference: String,
    /// Why it can't sign, as a clause that follows the name ("its certificate …").
    pub reason: String,
    /// The token holds no private key for it (a certificate someone else issued). Not a problem
    /// with an identity, so the Sign dialog doesn't show these.
    pub no_private_key: bool,
}

/// A token's signing identities and the certificates that can't sign.
#[derive(Default)]
pub struct Listing {
    pub ids: Vec<Identity>,
    pub unusable: Vec<Unusable>,
}

/// The private key on a token: identifies the token and the key object, and signs through the
/// freshly opened, logged-in session at signing time (a key handle is only valid in its own
/// session).
struct TokenKey {
    module: PathBuf,
    slot: Slot,
    cka_id: Vec<u8>,
    public: PublicKey,
    rsa: bool,
    reference: String,
}

impl ExternalKey for TokenKey {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let mechanism = mechanism(self.rsa, alg)?;
        let module = load(&self.module)?;
        let info = module.get_token_info(self.slot).map_err(|e| describe(&self.reference, "the token's details couldn't be read", e))?;
        // A token that writes its PIN on a keypad passes none; any other that needs a login is
        // given the PIN the caller set with [`set_pin`] (never guessed, and only then).
        let session = module.open_ro_session(self.slot).map_err(|e| describe(&self.reference, "the token couldn't be opened", e))?;
        if info.login_required() {
            if info.protected_authentication_path() {
                session.login(UserType::User, None).map_err(|e| login_error(&self.reference, e))?;
            } else {
                let pin = pin_for(&self.reference).ok_or(SignError::PinRequired)?;
                let pin = AuthPin::new(pin.into_boxed_str());
                session.login(UserType::User, Some(&pin)).map_err(|e| login_error(&self.reference, e))?;
            }
        }
        let key = session
            .find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(self.cka_id.clone()), Attribute::Sign(true)])
            .map_err(|e| describe(&self.reference, "the token's private key couldn't be found", e))?
            .into_iter()
            .next()
            .ok_or_else(|| SignError::Crypto(format!("the token at {} no longer holds this private key", self.reference)))?;
        let raw = session.sign(&mechanism, key, msg).map_err(|e| describe(&self.reference, "the token didn't sign", e))?;
        // RSA returns its signature as is; ECDSA returns the fixed-size r ‖ s a CMS wants DER-encoded.
        self.public.store_signature_der(&raw)
    }
}

/// The `pkcs11:<token label>:<SHA-256 of the certificate>` reference PdfCraft keeps for an
/// identity.
pub fn reference(token_label: &str, c: &Certificate) -> String {
    let digest = DigestAlg::Sha256.digest(&[&c.raw]);
    format!("pkcs11:{token_label}:{}", hex(&digest))
}

/// Remember a token's PIN for the signing that follows (keyed by the identity's reference). The
/// caller prompts once per signing session; there is no guessing and no default. An empty PIN is
/// a real one and is passed through.
pub fn set_pin(reference: &str, pin: &str) {
    if let Ok(mut pins) = PINS.lock() {
        pins.insert(reference.to_string(), pin.to_string());
    }
}

/// Find an identity by its `pkcs11:` reference, its certificate's CKA_LABEL, or its subject
/// common name.
pub fn find(reference_or_name: &str) -> Result<DigitalId, SignError> {
    let name = reference_or_name.strip_prefix("pkcs11:").unwrap_or(reference_or_name);
    scan()?
        .ids
        .into_iter()
        .find(|i| {
            i.reference == reference_or_name
                || format!("pkcs11:{}:{}", i.token_label, i.cka_label) == reference_or_name
                || i.id.certificate.subject.common_name() == Some(name)
        })
        .map(|i| i.id)
        .ok_or_else(|| SignError::Crypto(format!("no PKCS #11 identity {reference_or_name}")))
}

/// The signing identities every loaded module's tokens hold, and the certificates that can't
/// sign. No PIN is asked for: a token's certificates are public.
pub fn identities() -> Result<Listing, SignError> {
    scan()
}

/// Enumerate the loaded modules' tokens: the `pkcs11:` signing identities (in load order) and
/// the certificates that can't sign.
pub fn scan() -> Result<Listing, SignError> {
    let mut listing = Listing::default();
    for (path, loaded) in modules() {
        let module = match loaded {
            Ok(module) => module,
            Err(reason) => {
                listing.unusable.push(Unusable { subject: path.display().to_string(), reference: String::new(), reason, no_private_key: false });
                continue;
            }
        };
        let slots = match module.get_slots_with_token() {
            Ok(slots) => slots,
            Err(e) => {
                listing.unusable.push(Unusable {
                    subject: path.display().to_string(),
                    reference: String::new(),
                    reason: format!("the module's slots couldn't be read: {e}"),
                    no_private_key: false,
                });
                continue;
            }
        };
        for slot in slots {
            // SoftHSM always reports a slot 0, whose token is uninitialized until one is made;
            // opening a session on it fails. Skip any such token.
            match module.get_token_info(slot) {
                Ok(info) if !info.token_initialized() => continue,
                Ok(_) => {}
                Err(_) => continue,
            }
            match scan_token(&module, slot, &path) {
                Ok((ids, unusable)) => {
                    listing.ids.extend(ids);
                    listing.unusable.extend(unusable);
                }
                // One bad token (pulled out mid-scan) must not hide the others.
                Err(reason) => {
                    let subject = module.get_token_info(slot).map(|i| i.label().trim().to_string()).unwrap_or_else(|_| path.display().to_string());
                    listing.unusable.push(Unusable { subject, reference: String::new(), reason, no_private_key: false });
                }
            }
        }
    }
    Ok(listing)
}

/// One token's certificates, split into the identities that can sign and the ones that can't.
fn scan_token(module: &Pkcs11, slot: Slot, path: &Path) -> Result<(Vec<Identity>, Vec<Unusable>), String> {
    let info = module.get_token_info(slot).map_err(|e| format!("the token's details couldn't be read: {e}"))?;
    let label = info.label().trim().to_string();
    let session = module.open_ro_session(slot).map_err(|e| format!("the token couldn't be opened: {e}"))?;
    let objects = session
        .find_objects(&[Attribute::Class(ObjectClass::CERTIFICATE)])
        .map_err(|e| format!("the token's certificates couldn't be listed: {e}"))?;
    // Read every certificate first, so a chain can be built from all of them.
    let mut certs: Vec<(Certificate, Vec<u8>, String)> = Vec::new();
    for object in objects {
        let Ok(attrs) = session.get_attributes(object, &[AttributeType::Value, AttributeType::Id]) else { continue };
        let mut der = None;
        let mut cka_id = Vec::new();
        let mut cka_label = String::new();
        for a in attrs {
            match a {
                Attribute::Value(v) => der = Some(v),
                Attribute::Id(v) => cka_id = v,
                _ => {}
            }
        }
        let Some(der) = der else { continue };
        let Ok(cert) = Certificate::parse(&der) else { continue };
        // A `CKA_LABEL` is optional and may be missing; an empty one just doesn't match.
        if let Ok([Attribute::Label(bytes)]) = session.get_attributes(object, &[AttributeType::Label]).as_deref() {
            cka_label = String::from_utf8_lossy(bytes).trim().trim_end_matches('\0').to_string();
        }
        certs.push((cert, cka_id, cka_label));
    }
    let pool: Vec<Certificate> = certs.iter().map(|(c, _, _)| c.clone()).collect();
    let mut ids = Vec::new();
    let mut unusable = Vec::new();
    for (cert, cka_id, cka_label) in certs {
        let reference = reference(&label, &cert);
        let skip = |unusable: &mut Vec<Unusable>, reason: String, no_private_key: bool| {
            unusable.push(Unusable { subject: cert.display_name(), reference: reference.clone(), reason, no_private_key });
        };
        // A private key object is hidden until login (SoftHSM, cards), so a certificate's
        // key pair is confirmed by the private key *or* its public counterpart, which stays
        // visible pre-login.
        let objects_for =
            |class| session.find_objects(&[Attribute::Class(class), Attribute::Id(cka_id.clone())]).unwrap_or_default().into_iter().next();
        if objects_for(ObjectClass::PRIVATE_KEY).is_none() && objects_for(ObjectClass::PUBLIC_KEY).is_none() {
            skip(&mut unusable, "it has no matching private key on the token".to_string(), true);
            continue;
        }
        // P-521, brainpool and Ed25519 certificates parse (to verify with) but aren't signed with.
        if cert.public_key.store_signing_key().is_none() {
            let what = cert.public_key.describe();
            skip(&mut unusable, format!("its {what} key can't sign through PdfCraft yet (RSA, P-256 and P-384 only)"), false);
            continue;
        }
        // An encryption certificate beside the signing ones: a signature made with it would fail
        // validation, so it is offered greyed, not dropped.
        if cert.key_usage.is_some_and(|u| u & 0b11 == 0) {
            skip(
                &mut unusable,
                "its certificate's key usage doesn't allow digital signatures (digitalSignature or nonRepudiation)".to_string(),
                false,
            );
            continue;
        }
        let chain = x509::build_chain(&cert, &pool, None).into_iter().filter(|c| c.raw != cert.raw).cloned().collect();
        let public = cert.public_key.clone();
        let rsa = matches!(public, PublicKey::Rsa { .. });
        let key = PrivateKey::external(
            public.clone(),
            Arc::new(TokenKey { module: path.to_path_buf(), slot, cka_id, public, rsa, reference: reference.clone() }),
        );
        let friendly_name = Some(format!("{} ({label})", cert.display_name()));
        ids.push(Identity { reference, id: DigitalId { key, certificate: cert, chain, friendly_name }, token_label: label.clone(), cka_label });
    }
    Ok((ids, unusable))
}

/// The PKCS #11 signing mechanism for a key and digest, or why the key can't sign with it.
fn mechanism(rsa: bool, alg: DigestAlg) -> Result<Mechanism<'static>, SignError> {
    Ok(match (rsa, alg) {
        (true, DigestAlg::Sha256) => Mechanism::Sha256RsaPkcs,
        (true, DigestAlg::Sha384) => Mechanism::Sha384RsaPkcs,
        (true, DigestAlg::Sha512) => Mechanism::Sha512RsaPkcs,
        (false, DigestAlg::Sha256) => Mechanism::EcdsaSha256,
        (false, DigestAlg::Sha384) => Mechanism::EcdsaSha384,
        (false, DigestAlg::Sha512) => Mechanism::EcdsaSha512,
        (_, other) => return Err(SignError::Unsupported(format!("{} for new signatures", other.name()))),
    })
}

/// A module path's load result (the error kept as a string so a failure is reported, and
/// cached, once).
type Loaded = Result<Arc<Pkcs11>, String>;

/// Load a module (once per path in the process) and initialize it.
fn load(path: &Path) -> Result<Arc<Pkcs11>, SignError> {
    load_inner(path).map_err(SignError::Crypto)
}

fn load_inner(path: &Path) -> Loaded {
    if let Ok(cache) = LOADED.lock()
        && let Some(found) = cache.get(path)
    {
        return found.clone();
    }
    let result = open(path);
    if let Ok(mut cache) = LOADED.lock() {
        cache.insert(path.to_path_buf(), result.clone());
    }
    result
}

fn open(path: &Path) -> Result<Arc<Pkcs11>, String> {
    let module = Pkcs11::new(path).map_err(|e| format!("{}: the module couldn't be loaded: {e}", path.display()))?;
    match module.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
        // Another handle already initialized this same library in the process (the app never
        // finalizes), which is fine.
        Ok(()) | Err(CkError::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => Ok(Arc::new(module)),
        Err(e) => Err(format!("{}: the module couldn't be initialized: {e}", path.display())),
    }
}

/// Every configured module with its load result, in order. A path that fails stays in the list
/// so the reason is reported once.
fn modules() -> Vec<(PathBuf, Loaded)> {
    module_paths().into_iter().map(|path| (path.clone(), load_inner(&path))).collect()
}

/// The module paths to load: `PDFCRAFT_PKCS11_MODULES` (verbatim, so a missing one is reported),
/// else an existing p11-kit proxy (which auto-loads the system's registered modules).
fn module_paths() -> Vec<PathBuf> {
    module_paths_from(std::env::var("PDFCRAFT_PKCS11_MODULES").ok().as_deref())
}

/// [`module_paths`] with the environment value passed in, so it can be tested without touching
/// the process environment.
fn module_paths_from(modules: Option<&str>) -> Vec<PathBuf> {
    if let Some(list) = modules {
        let paths: Vec<PathBuf> = list.split(':').filter(|p| !p.is_empty()).map(PathBuf::from).collect();
        if !paths.is_empty() {
            return paths;
        }
    }
    const PROXIES: &[&str] = &[
        "/usr/lib/x86_64-linux-gnu/p11-kit-proxy.so",
        "/usr/lib64/p11-kit-proxy.so",
        "/usr/lib/p11-kit-proxy.so",
        "/usr/local/lib/p11-kit-proxy.so",
        "/opt/homebrew/lib/p11-kit-proxy.so",
    ];
    PROXIES.iter().map(PathBuf::from).find(|p| p.exists()).into_iter().collect()
}

/// PINs remembered for the next signing, by identity reference.
static PINS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Modules already loaded (and their failures), so a library is loaded and initialized once.
static LOADED: LazyLock<Mutex<HashMap<PathBuf, Loaded>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn pin_for(reference: &str) -> Option<String> {
    PINS.lock().ok()?.get(reference).cloned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn describe(reference: &str, what: &str, e: CkError) -> SignError {
    SignError::Crypto(format!("{what} ({}): {e}", reference))
}

/// A login failure with the PIN-specific ones named.
fn login_error(reference: &str, e: CkError) -> SignError {
    match e {
        CkError::Pkcs11(RvError::PinIncorrect, _) => SignError::WrongPin,
        CkError::Pkcs11(RvError::PinLocked, _) => SignError::Crypto(format!("the token at {reference} is locked (too many wrong PINs)")),
        other => SignError::Crypto(format!("the token at {reference} refused the login: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mechanism_follows_the_key_type() {
        use cryptoki::mechanism::MechanismType;
        let mt = |rsa, alg| mechanism(rsa, alg).unwrap().mechanism_type();
        assert_eq!(mt(true, DigestAlg::Sha256), MechanismType::SHA256_RSA_PKCS);
        assert_eq!(mt(true, DigestAlg::Sha384), MechanismType::SHA384_RSA_PKCS);
        assert_eq!(mt(true, DigestAlg::Sha512), MechanismType::SHA512_RSA_PKCS);
        assert_eq!(mt(false, DigestAlg::Sha256), MechanismType::ECDSA_SHA256);
        assert_eq!(mt(false, DigestAlg::Sha384), MechanismType::ECDSA_SHA384);
        assert_eq!(mt(false, DigestAlg::Sha512), MechanismType::ECDSA_SHA512);
        assert!(matches!(mechanism(true, DigestAlg::Sha1), Err(SignError::Unsupported(_))));
    }

    #[test]
    fn module_paths_follow_the_environment() {
        assert_eq!(module_paths_from(Some("/a.so:/b.so")), vec![PathBuf::from("/a.so"), PathBuf::from("/b.so")]);
        assert_eq!(module_paths_from(Some("/only.so")), vec![PathBuf::from("/only.so")]);
        // A leading or trailing colon is not an empty path.
        assert_eq!(module_paths_from(Some(":/only.so:")), vec![PathBuf::from("/only.so")]);
        // An empty setting falls back to an existing proxy (or nothing).
        let empty = module_paths_from(Some(""));
        assert!(empty.iter().all(|p| p.extension().is_some_and(|e| e == "so")));
    }

    fn cert_with(der: &[u8]) -> Certificate {
        Certificate::parse(der).unwrap()
    }

    #[test]
    fn reference_carries_the_token_label_and_fingerprint() {
        let der = std::fs::read(format!("{}/tests/data/rsa-aes.p12", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let id = crate::pkcs12::open(&der, "test").unwrap();
        let r = reference("My Token", &id.certificate);
        assert!(r.starts_with("pkcs11:My Token:"));
        assert_eq!(r.rsplit(':').next().unwrap().len(), 64);
        let _ = cert_with(&id.certificate.raw);
    }
}
