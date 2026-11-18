//! PKCS #11 tokens and smart cards, the Italian CNS and CIE among them: identities whose private
//! keys never leave the card. The module is the card issuer's own middleware (bit4id's
//! `libbit4xpki.so` for a CNS, the CIE middleware's `libcie-pkcs11.so`), loaded at run time, so
//! PdfCraft links against no vendor SDK.
//!
//! The PIN is never kept: a caller passes it for one signing operation, the key holds it only in
//! a [`AuthPin`] for as long as that operation runs, and the listing never logs in unless the
//! caller offers a PIN because the card hides its certificates. Nothing is written to the
//! settings, the log or an error message, and a wrong PIN is never retried here — a card locks
//! itself after a few attempts, and only the issuer's own middleware can unlock it.
//!
//! Signing uses the mechanisms cards implement in practice: RSA through `CKM_RSA_PKCS` with the
//! `DigestInfo` built here (or the card's own `CKM_SHA*_RSA_PKCS` when it has no raw PKCS #1),
//! and ECDSA through `CKM_ECDSA` over the digest, whose `r ‖ s` becomes DER. A key marked
//! `CKA_ALWAYS_AUTHENTICATE` (the CIE's, and some CNS cards' signing keys) gets the
//! context-specific login the standard requires before each use; a card with its own PIN pad
//! (`CKF_PROTECTED_AUTHENTICATION_PATH`) is asked without a PIN ever reaching PdfCraft.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error as CryptokiError, RvError};
use cryptoki::mechanism::{Mechanism, MechanismType};
use cryptoki::object::{Attribute, AttributeType, CertificateType, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;
use zeroize::Zeroizing;

use crate::der::{self, Tlv};
use crate::keys::{DigestAlg, ExternalKey, PrivateKey, PublicKey, StoreKey};
use crate::{Certificate, DigitalId, SignError};

/// A signing identity on a token: its certificate, the issuers the token itself holds for it, and
/// where the key is. It carries no PIN; [`Identity::digital_id`] takes one for one operation.
#[derive(Clone, Debug)]
pub struct Identity {
    pub certificate: Certificate,
    pub chain: Vec<Certificate>,
    pub friendly_name: Option<String>,
    /// The token's own label ("CNS", a cardholder's name), for a listing that names the card.
    pub token: String,
    module: PathBuf,
    slot: Slot,
    /// The token's serial number, so the right card is found again after it is replugged and
    /// the slot number moved.
    serial: String,
    /// `CKA_ID` of the private key (or of the certificate when the card hides its keys until
    /// the PIN is entered).
    key_id: Vec<u8>,
    always_authenticate: bool,
    login_required: bool,
    protected_path: bool,
}

impl Identity {
    /// The `pkcs11:<SHA-256 of the certificate>` reference PdfCraft keeps for this identity.
    pub fn reference(&self) -> String {
        reference(&self.certificate)
    }

    /// Whether signing needs a PIN from the user: the card requires a login and has no keypad of
    /// its own. An interface asks for one exactly then, and keeps it only for the signing.
    pub fn needs_pin(&self) -> bool {
        self.login_required && !self.protected_path
    }

    /// The identity as a signing key. `pin` is used for this operation only; the card's own PIN
    /// pad needs none. A card that requires a PIN and is given none refuses here rather than at
    /// the end of a half-done signature.
    pub fn digital_id(self, pin: Option<Zeroizing<String>>) -> Result<DigitalId, SignError> {
        let auth = match (&pin, self.protected_path, self.login_required) {
            // The card's keypad asks for the PIN itself; passing one would be refused.
            (_, true, _) => None,
            // Nothing to log in to (a card without a user PIN, such as a test token).
            (_, false, false) => None,
            (Some(p), false, true) if !p.is_empty() => Some(AuthPin::new(p.as_str().into())),
            _ => return Err(SignError::Crypto(format!("{} needs the card's PIN to sign", self.certificate.display_name()))),
        };
        let public = self.certificate.public_key.clone();
        let key = TokenKey {
            module: self.module,
            slot: self.slot,
            serial: self.serial,
            key_id: self.key_id,
            public: public.clone(),
            always_authenticate: self.always_authenticate,
            protected_path: self.protected_path,
            login: self.login_required && !self.protected_path,
            auth,
        };
        let key = PrivateKey::external(public, Arc::new(key));
        Ok(DigitalId { key, certificate: self.certificate, chain: self.chain, friendly_name: self.friendly_name })
    }
}

/// A token certificate PdfCraft can't sign with, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    /// The subject's common name (else organization, else the DN), or a placeholder when the
    /// certificate couldn't be read at all.
    pub subject: String,
    /// SHA-256 of the certificate as upper-case hex pairs (`Certificate::fingerprint`).
    pub fingerprint: String,
    /// Why it can't sign, as a clause that follows the name ("its ...").
    pub reason: String,
    /// The token holds no private key for it. Not a problem with an identity, so the Sign dialog
    /// doesn't show these.
    pub no_private_key: bool,
}

/// What the configured modules expose: the signing identities, the certificates that can't sign,
/// and anything that went wrong with a module as a whole (a missing library, a card reader the
/// middleware can't open). A module with no card inserted contributes nothing; that is not an
/// error.
#[derive(Default, Debug)]
pub struct Listing {
    pub identities: Vec<Identity>,
    pub unusable: Vec<Unusable>,
    pub problems: Vec<String>,
}

/// The `pkcs11:<SHA-256 of the certificate DER>` reference kept for an identity.
pub fn reference(certificate: &Certificate) -> String {
    let digest = DigestAlg::Sha256.digest(&[&certificate.raw]);
    format!("pkcs11:{}", digest.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// `PKCS11_MODULES`-style list, split the way the platform splits a path list.
fn env_modules() -> Vec<PathBuf> {
    let Some(value) = std::env::var_os("PDFCRAFT_PKCS11_MODULES") else { return Vec::new() };
    std::env::split_paths(&value).filter(|p| !p.as_os_str().is_empty()).collect()
}

/// Module files to try: `PDFCRAFT_PKCS11_MODULES` when it is set (exactly those, for a caller
/// that knows what it wants), else the middleware the usual issuers and distributions install.
/// Only files that exist are returned unless the caller named them. Add the modules chosen in
/// Preferences to the result.
pub fn default_modules() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        let p = p.canonicalize().unwrap_or(p);
        if p.exists() && !out.contains(&p) {
            out.push(p);
        }
    };
    let named = env_modules();
    if !named.is_empty() {
        for p in named {
            push(p);
        }
        return out;
    }
    // Italian CNS and CIE middleware first: they are what a card in a reader needs.
    for p in [
        "/usr/lib/bit4id/libbit4xpki.so",
        "/usr/lib/libbit4xpki.so",
        "/usr/local/lib/libbit4xpki.so",
        "/usr/lib/libcie-pkcs11.so",
        "/usr/lib/CIE/libcie-pkcs11.so",
        "/usr/local/lib/libcie-pkcs11.so",
    ] {
        push(PathBuf::from(p));
    }
    // Then generic middleware: OpenSC reads many cards, SoftHSM is a software token (tests).
    for dir in [
        "/usr/lib/pkcs11",
        "/usr/lib64/pkcs11",
        "/usr/local/lib/pkcs11",
        "/usr/lib/x86_64-linux-gnu/pkcs11",
        "/usr/lib/aarch64-linux-gnu/pkcs11",
        "/usr/local/lib/softhsm",
        "/opt/homebrew/lib/softhsm",
        "/opt/homebrew/lib/pkcs11",
        "/Library/OpenSC/lib",
    ] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut found: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                // Real card modules: not the p11-kit proxies and trust stores (they load every
                // configured card, which is the user's choice to make per module), not the
                // OpenSC trace wrapper (it needs PKCS11SPY set and is a debugging aid), and not
                // the GNOME keyring (a key store, not a card).
                (name.ends_with(".so") || name.ends_with(".dylib"))
                    && !name.starts_with("p11-kit")
                    && !name.starts_with("pkcs11-spy")
                    && !name.starts_with("gnome-keyring")
            })
            .collect();
        found.sort();
        for p in found {
            push(p);
        }
    }
    #[cfg(target_os = "windows")]
    {
        // Windows' system directory holds the CSP/KSP DLLs a vendor installs for its readers.
        let windir = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        for name in ["bit4xpki.dll", "cie-pkcs11.dll", "opensc-pkcs11.dll", "onepin-opensc-pkcs11.dll"] {
            push(windir.join("System32").join(name));
        }
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            push(PathBuf::from(&pf).join("SoftHSM2").join("lib").join("softhsm2-x64.dll"));
        }
    }
    out
}

/// The loaded module contexts, by path. A module is loaded and initialized once per process:
/// vendor middleware is known to misbehave when it is initialized and finalized repeatedly, and
/// the handle is cheap to keep. A failed load is remembered too, so a broken module is reported
/// once instead of on every keystroke of a dialog.
/// A module as loaded for this process, or why it couldn't be.
type Loaded = Result<Arc<Pkcs11>, String>;

fn context(path: &Path) -> Loaded {
    static MODULES: OnceLock<Mutex<HashMap<PathBuf, Loaded>>> = OnceLock::new();
    let cache = MODULES.get_or_init(|| Mutex::new(HashMap::new()));
    // A panic while holding the lock changes nothing about the module, so a poisoned lock is
    // still safe to use.
    let mut cache = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(known) = cache.get(path) {
        return known.clone();
    }
    let loaded = load(path);
    cache.insert(path.to_path_buf(), loaded.clone());
    loaded
}

fn load(path: &Path) -> Result<Arc<Pkcs11>, String> {
    let pkcs11 = Pkcs11::new(path).map_err(|e| format!("{}: {}", path.display(), describe(&e)))?;
    match pkcs11.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
        Ok(()) => {}
        // The same library, already initialized through another path (a symlinked module
        // directory): usable as it is.
        Err(CryptokiError::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => {}
        Err(e) => return Err(format!("{}: {}", path.display(), describe(&e))),
    }
    Ok(Arc::new(pkcs11))
}

/// The signing identities of every token in the configured modules, without logging in. A token
/// that hides its certificates until the PIN is entered lists nothing here; pass `pin` then.
pub fn identities(modules: &[PathBuf], pin: Option<&str>) -> Listing {
    let mut listing = Listing::default();
    for path in modules {
        let ctx = match context(path) {
            Ok(ctx) => ctx,
            Err(e) => {
                listing.problems.push(e);
                continue;
            }
        };
        let slots = match ctx.get_slots_with_token() {
            Ok(slots) => slots,
            Err(e) => {
                listing.problems.push(format!("{}: {}", path.display(), describe(&e)));
                continue;
            }
        };
        for slot in slots {
            match token_identities(&ctx, path, slot, pin) {
                Ok((mut identities, mut unusable)) => {
                    listing.identities.append(&mut identities);
                    listing.unusable.append(&mut unusable);
                }
                Err(e) => listing.problems.push(format!("{}: {e}", path.display())),
            }
        }
    }
    // Two modules that both read the same card (a vendor's and OpenSC's, say) list the same
    // certificate twice; the first module's view is kept.
    let mut seen = std::collections::HashSet::new();
    listing.identities.retain(|id| seen.insert(id.reference()));
    listing
}

/// Find an identity by its reference or subject common name, with no PIN. A card that hides its
/// certificates until the PIN is entered is searched again with it. Its `chain` holds the
/// issuers the card itself holds, so a signature embeds what the card can prove.
pub fn find(reference_or_name: &str, modules: &[PathBuf], pin: Option<Zeroizing<String>>) -> Result<DigitalId, SignError> {
    let wanted = reference_or_name.trim();
    let name = wanted.strip_prefix("pkcs11:").unwrap_or(wanted);
    let mut listing = identities(modules, None);
    let matching = |listing: &Listing| {
        listing
            .identities
            .iter()
            .position(|id| id.reference() == wanted || id.certificate.display_name() == name || id.certificate.subject.common_name() == Some(name))
    };
    let Some(index) = matching(&listing).or_else(|| {
        let pin = pin.as_deref()?;
        listing = identities(modules, Some(pin));
        matching(&listing)
    }) else {
        let why = if listing.problems.is_empty() { String::new() } else { format!(" ({})", listing.problems.join("; ")) };
        return Err(SignError::Crypto(format!(
            "no PKCS#11 identity {reference_or_name}{why}: is the card in the reader, and is its module configured?"
        )));
    };
    let Some(identity) = listing.identities.into_iter().nth(index) else {
        return Err(SignError::Crypto(format!("no PKCS#11 identity {reference_or_name}")));
    };
    identity.digital_id(pin)
}

fn token_identities(ctx: &Pkcs11, path: &Path, slot: Slot, pin: Option<&str>) -> Result<(Vec<Identity>, Vec<Unusable>), String> {
    let token = ctx.get_token_info(slot).map_err(|e| describe(&e))?;
    // A token nobody initialized holds nothing and can't be logged in to; SoftHSM lists its
    // empty slot as having a token, so this is the difference between "no card" and "no keys".
    if !token.token_initialized() {
        return Ok((Vec::new(), Vec::new()));
    }
    let session = ctx.open_ro_session(slot).map_err(|e| describe(&e))?;
    let auth = pin.filter(|p| !p.is_empty()).map(|p| AuthPin::new(p.into()));
    if let Some(pin) = &auth {
        // A card that doesn't need a login accepts one all the same; one that does needs it
        // before it shows its private keys.
        match session.login(UserType::User, Some(pin)) {
            Ok(()) | Err(CryptokiError::Pkcs11(RvError::UserAlreadyLoggedIn, _)) => {}
            Err(e) => return Err(describe(&e)),
        }
    }
    let certs = session.find_objects(&[Attribute::Class(ObjectClass::CERTIFICATE)]).map_err(|e| describe(&e))?;
    let keys = session.find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY)]).map_err(|e| describe(&e))?;
    let mut parsed: Vec<(Certificate, Vec<u8>)> = Vec::new();
    let mut unusable = Vec::new();
    for handle in certs {
        if attr(&session, handle, AttributeType::CertificateType) != Some(Attribute::CertificateType(CertificateType::X_509)) {
            continue;
        }
        let Some(Attribute::Value(der)) = attr(&session, handle, AttributeType::Value) else { continue };
        let id = match attr(&session, handle, AttributeType::Id) {
            Some(Attribute::Id(id)) => id,
            _ => Vec::new(),
        };
        match Certificate::parse(&der) {
            Ok(certificate) => parsed.push((certificate, id)),
            Err(e) => {
                let subject = crate::x509::subject_of(&der).map(|n| n.display_name()).unwrap_or_else(|| "(unreadable certificate)".into());
                unusable.push(Unusable {
                    fingerprint: DigestAlg::Sha256.digest(&[&der]).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "),
                    subject,
                    reason: format!("it can't be read: {e}"),
                    no_private_key: false,
                });
            }
        }
    }
    // A card that requires a login normally hides every private key from an anonymous session.
    // Seeing none then is not a missing key; the card's certificates are listed and the key is
    // looked up again when signing, after the PIN has been entered.
    let keys_hidden = keys.is_empty() && token.login_required();
    let pool: Vec<Certificate> = parsed.iter().map(|(c, _)| c.clone()).collect();
    let mut identities = Vec::new();
    for (certificate, id) in &parsed {
        let fingerprint = certificate.fingerprint();
        let mut skip = |reason: String, no_private_key: bool| {
            unusable.push(Unusable { subject: certificate.display_name(), fingerprint: fingerprint.clone(), reason, no_private_key });
        };
        let public = &certificate.public_key;
        if public.store_signing_key().is_none() {
            skip(format!("its {} key can't sign through PdfCraft yet (RSA, P-256 and P-384 only)", public.describe()), false);
            continue;
        }
        if let Some(reason) = certificate.signing_problem_clause() {
            skip(reason, false);
            continue;
        }
        let (key_id, always_authenticate) = match match_key(&session, &keys, id, public) {
            Some((key_id, always)) => (key_id, always),
            None if keys_hidden => (id.clone(), false),
            None => {
                skip("the token holds no private key for it".into(), true);
                continue;
            }
        };
        let mut chain: Vec<Certificate> = crate::x509::build_chain(certificate, &pool, None).into_iter().skip(1).cloned().collect();
        chain.retain(|c| c.raw != certificate.raw);
        identities.push(Identity {
            certificate: certificate.clone(),
            chain,
            friendly_name: Some(format!("{} (smart card)", certificate.display_name())),
            token: token.label().to_string(),
            module: path.to_path_buf(),
            slot,
            serial: token.serial_number().to_string(),
            key_id,
            always_authenticate,
            login_required: token.login_required(),
            protected_path: token.protected_authentication_path(),
        });
    }
    Ok((identities, unusable))
}

/// The private key for `certificate`, matched by `CKA_ID` and else by its public key material.
/// Returns the key's `CKA_ID` (so the signer finds it again in its own session) and whether the
/// card wants a context-specific login before each use.
fn match_key(session: &Session, keys: &[ObjectHandle], id: &[u8], public: &PublicKey) -> Option<(Vec<u8>, bool)> {
    let always = |h: ObjectHandle| attr(session, h, AttributeType::AlwaysAuthenticate) == Some(Attribute::AlwaysAuthenticate(true));
    let key_id = |h: ObjectHandle| match attr(session, h, AttributeType::Id) {
        Some(Attribute::Id(id)) => id,
        _ => Vec::new(),
    };
    let found = keys
        .iter()
        .copied()
        .find(|h| !id.is_empty() && key_id(*h) == id)
        .or_else(|| keys.iter().copied().find(|h| key_matches(session, *h, public)))?;
    Some((key_id(found), always(found)))
}

/// Whether a private key's public half is the certificate's: the modulus for RSA, the point for
/// EC. A card whose `CKA_ID`s don't line up (some do not) is still matched exactly.
fn key_matches(session: &Session, handle: ObjectHandle, public: &PublicKey) -> bool {
    match public {
        PublicKey::Rsa { n, .. } => match attr(session, handle, AttributeType::Modulus) {
            Some(Attribute::Modulus(m)) => strip_zeroes(&m) == strip_zeroes(n),
            _ => false,
        },
        PublicKey::P256(point) | PublicKey::P384(point) | PublicKey::P521(point) => match attr(session, handle, AttributeType::EcPoint) {
            // CKA_EC_POINT is a DER OCTET STRING around the SEC1 point.
            Some(Attribute::EcPoint(p)) => Tlv::parse_all(&p)
                .ok()
                .and_then(|t| t.octets("EC point").ok().map(std::borrow::Cow::into_owned))
                .is_some_and(|bytes| strip_zeroes(&bytes) == strip_zeroes(point)),
            _ => false,
        },
        _ => false,
    }
}

fn strip_zeroes(bytes: &[u8]) -> &[u8] {
    bytes.iter().position(|b| *b != 0).and_then(|i| bytes.get(i..)).unwrap_or_default()
}

fn attr(session: &Session, handle: ObjectHandle, kind: AttributeType) -> Option<Attribute> {
    // One attribute at a time: a single unsupported or sensitive attribute makes the whole
    // batch fail, and cards differ in which ones they answer.
    session.get_attributes(handle, &[kind]).ok()?.into_iter().next()
}

/// A private key that lives on a token and signs on request. It holds no object handle (those
/// belong to one session) and finds the key again in the session it opens.
struct TokenKey {
    module: PathBuf,
    slot: Slot,
    serial: String,
    key_id: Vec<u8>,
    public: PublicKey,
    always_authenticate: bool,
    /// The card asks for its PIN through its own keypad (`CKF_PROTECTED_AUTHENTICATION_PATH`).
    protected_path: bool,
    login: bool,
    auth: Option<AuthPin>,
}

impl std::fmt::Debug for TokenKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `AuthPin` redacts itself, but nothing here needs printing either way.
        write!(f, "TokenKey({}, {:?})", self.public.describe(), self.module.file_name().unwrap_or_default())
    }
}

impl ExternalKey for TokenKey {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let ctx = context(&self.module).map_err(SignError::Crypto)?;
        let slot = slot_for(&ctx, self.slot, &self.serial).map_err(|e| failed(&self.module, "", &e))?;
        let session = ctx.open_ro_session(slot).map_err(|e| failed(&self.module, "couldn't open a session on the card", &e))?;
        let signed = self.sign_in(&ctx, slot, &session, alg, msg);
        // A failed logout on a card that is already gone is not news.
        if self.login {
            let _ = session.logout();
        }
        let _ = session.close();
        signed.map_err(|e| match e {
            // Name the module once, the way a card error names the card.
            SignError::Crypto(reason) => SignError::Crypto(format!("{}: {reason}", module_name(&self.module))),
            other => other,
        })
    }
}

impl TokenKey {
    fn sign_in(&self, ctx: &Pkcs11, slot: Slot, session: &Session, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let mut logged_in = false;
        if self.login {
            match session.login(UserType::User, self.auth.as_ref()) {
                Ok(()) => logged_in = true,
                // A token that keeps one login across sessions (some do) is fine as it is.
                Err(CryptokiError::Pkcs11(RvError::UserAlreadyLoggedIn, _)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        let key = find_key(session, &self.key_id, &self.public)?;
        // The flag could not be read while the key was hidden behind the login; now it can.
        let always = self.always_authenticate || attr(session, key, AttributeType::AlwaysAuthenticate) == Some(Attribute::AlwaysAuthenticate(true));
        if always && (self.auth.is_some() || self.protected_path) {
            session.login(UserType::ContextSpecific, self.auth.as_ref())?;
        }
        let available = ctx.get_mechanism_list(slot).unwrap_or_default();
        // A CIE (and some CNS cards) wants the PIN again for every operation even when
        // `CKA_ALWAYS_AUTHENTICATE` isn't readable. The PIN just proved correct above, so this
        // asks the card again with a good one and cannot burn an attempt.
        let raw = match sign_with(session, &self.public, alg, msg, key, &available) {
            Ok(raw) => raw,
            Err(CryptokiError::Pkcs11(RvError::UserNotLoggedIn, _)) if logged_in => {
                if self.auth.is_none() && !self.protected_path {
                    return Err(SignError::Crypto(describe(&CryptokiError::Pkcs11(RvError::UserNotLoggedIn, cryptoki::context::Function::Sign))));
                }
                session.login(UserType::ContextSpecific, self.auth.as_ref())?;
                sign_with(session, &self.public, alg, msg, key, &available)?
            }
            Err(e) => return Err(e.into()),
        };
        match &self.public {
            PublicKey::Rsa { .. } => Ok(raw),
            PublicKey::P256(_) | PublicKey::P384(_) => ecdsa_der(&self.public, &raw),
            other => Err(SignError::Unsupported(format!("signing with {} keys on a token", other.describe()))),
        }
    }
}

/// One `C_Sign`, without converting its result: the caller decides whether an EC `r ‖ s` becomes
/// DER, and whether a "not logged in" answer is worth a context-specific login and a retry.
fn sign_with(
    session: &Session,
    public: &PublicKey,
    alg: DigestAlg,
    msg: &[u8],
    key: ObjectHandle,
    available: &[MechanismType],
) -> Result<Vec<u8>, CryptokiError> {
    match public {
        PublicKey::Rsa { .. } => {
            let digest = alg.digest(&[msg]);
            match rsa_mechanism(alg, available) {
                RsaMechanism::RawPkcs1 => session.sign(&Mechanism::RsaPkcs, key, &digest_info(alg, &digest)),
                RsaMechanism::HashInside(alg) => session.sign(&hash_mechanism(alg), key, msg),
                RsaMechanism::Unsupported => Err(CryptokiError::Pkcs11(RvError::MechanismInvalid, cryptoki::context::Function::SignInit)),
            }
        }
        _ => session.sign(&Mechanism::Ecdsa, key, &alg.digest(&[msg])),
    }
}

/// How a card will make an RSA PKCS #1 v1.5 signature for a digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RsaMechanism {
    /// `CKM_RSA_PKCS` over the `DigestInfo` the caller builds.
    RawPkcs1,
    /// The card hashes and wraps the message itself (`CKM_SHA*_RSA_PKCS`).
    HashInside(DigestAlg),
    /// Neither is offered.
    Unsupported,
}

/// The first mechanism this card offers, preferring `CKM_RSA_PKCS`: it is the one every card
/// that signs RSA implements, and it keeps the `DigestInfo` under PdfCraft's control.
fn rsa_mechanism(alg: DigestAlg, available: &[MechanismType]) -> RsaMechanism {
    if available.is_empty() || available.contains(&MechanismType::RSA_PKCS) {
        return RsaMechanism::RawPkcs1;
    }
    let kind = match alg {
        DigestAlg::Sha256 => MechanismType::SHA256_RSA_PKCS,
        DigestAlg::Sha384 => MechanismType::SHA384_RSA_PKCS,
        DigestAlg::Sha512 => MechanismType::SHA512_RSA_PKCS,
        _ => return RsaMechanism::Unsupported,
    };
    if available.contains(&kind) { RsaMechanism::HashInside(alg) } else { RsaMechanism::Unsupported }
}

/// The mechanism for a card that hashes the message itself.
fn hash_mechanism(alg: DigestAlg) -> Mechanism<'static> {
    match alg {
        DigestAlg::Sha384 => Mechanism::Sha384RsaPkcs,
        DigestAlg::Sha512 => Mechanism::Sha512RsaPkcs,
        _ => Mechanism::Sha256RsaPkcs,
    }
}

/// `DigestInfo ::= SEQUENCE { digestAlgorithm AlgorithmIdentifier, digest OCTET STRING }`
/// (RFC 8017 §9.2), with the NULL parameters the standard requires.
fn digest_info(alg: DigestAlg, digest: &[u8]) -> Vec<u8> {
    let algorithm = der::algorithm(alg.oid(), Some(&der::null()));
    der::seq(&[&algorithm, &der::octets(digest)])
}

/// `CKM_ECDSA` returns `r ‖ s` at the key's size (some cards strip leading zeroes); a CMS
/// signature carries DER.
fn ecdsa_der(public: &PublicKey, raw: &[u8]) -> Result<Vec<u8>, SignError> {
    let Some(StoreKey::Ecdsa(bits)) = public.store_signing_key() else {
        return Err(SignError::Unsupported(format!("signing with {} keys", public.describe())));
    };
    let half = usize::try_from(bits).map_err(|_| SignError::Malformed("ECDSA key size".into()))?.div_ceil(8);
    if raw.is_empty() || !raw.len().is_multiple_of(2) || raw.len() > half.saturating_mul(2) {
        return Err(SignError::Crypto(format!("the card returned an invalid ECDSA signature ({} bytes)", raw.len())));
    }
    let (r, s) = raw.split_at(raw.len() / 2);
    if r.len() == half && s.len() == half {
        return public.store_signature_der(raw);
    }
    // Right-align each half in the key's width; a value wider than the width can't be a
    // signature this key made.
    let mut fixed = vec![0u8; half * 2];
    let r = strip_zeroes(r);
    let s = strip_zeroes(s);
    if r.len() > half || s.len() > half {
        return Err(SignError::Crypto("the card returned an ECDSA signature wider than its key".into()));
    }
    // Both ranges exist: each half is at most the key's width (checked just above).
    if let Some(chunk) = fixed.get_mut(half - r.len()..half) {
        chunk.copy_from_slice(r);
    }
    if let Some(chunk) = fixed.get_mut(half * 2 - s.len()..half * 2) {
        chunk.copy_from_slice(s);
    }
    public.store_signature_der(&fixed)
}

/// The slot holding the same token as before. Slots are renumbered when a reader is replugged,
/// so the serial number is what identifies the card; the stored slot is the fallback.
fn slot_for(ctx: &Pkcs11, slot: Slot, serial: &str) -> Result<Slot, CryptokiError> {
    let slots = ctx.get_slots_with_token()?;
    if let Some(found) = slots.iter().copied().find(|s| ctx.get_token_info(*s).is_ok_and(|t| t.serial_number() == serial)) {
        return Ok(found);
    }
    if slots.contains(&slot) {
        return Ok(slot);
    }
    Err(CryptokiError::Pkcs11(RvError::TokenNotPresent, cryptoki::context::Function::OpenSession))
}

fn find_key(session: &Session, key_id: &[u8], public: &PublicKey) -> Result<ObjectHandle, CryptokiError> {
    if !key_id.is_empty()
        && let Ok(found) = session.find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(key_id.to_vec())])
        && let Some(handle) = found.first()
    {
        return Ok(*handle);
    }
    for handle in session.find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY)]).unwrap_or_default() {
        if key_matches(session, handle, public) {
            return Ok(handle);
        }
    }
    Err(CryptokiError::Pkcs11(RvError::KeyHandleInvalid, cryptoki::context::Function::SignInit))
}

/// A module failure as a sentence: the module file first, then what went wrong.
fn failed(path: &Path, what: &str, e: &CryptokiError) -> SignError {
    let name = module_name(path);
    let reason = describe(e);
    SignError::Crypto(if what.is_empty() { format!("{name}: {reason}") } else { format!("{name}: {what}: {reason}") })
}

fn module_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// A PKCS #11 error in words a signer can act on. Only the ones that happen in practice get a
/// sentence of their own; the rest keep the standard's short name so a report can be matched
/// against the spec.
fn describe(e: &CryptokiError) -> String {
    let CryptokiError::Pkcs11(rv, _) = e else {
        return match e {
            CryptokiError::LibraryLoading(e) => format!("the module couldn't be loaded ({e})"),
            CryptokiError::NullFunctionPointer | CryptokiError::MissingSymbol(_) => {
                "the file isn't a PKCS #11 module, or is an older version than PdfCraft can use".to_string()
            }
            other => format!("{other}"),
        };
    };
    match rv {
        RvError::PinIncorrect => "the PIN is incorrect".to_string(),
        RvError::PinLocked => {
            "the card's PIN is locked after too many wrong attempts; unlock it with the card's own middleware (the PIN unblock code or PUK), or ask the issuer".to_string()
        }
        RvError::PinExpired => "the card's PIN has expired; change it with the card's own middleware".to_string(),
        RvError::PinInvalid => "the PIN has characters the card doesn't accept".to_string(),
        RvError::PinLenRange => "the PIN isn't the length the card expects".to_string(),
        RvError::UserPinNotInitialized => "the card has no PIN yet; set one with the card's own middleware".to_string(),
        RvError::UserNotLoggedIn => "the card expects its PIN before this".to_string(),
        RvError::UserAlreadyLoggedIn => "the card is already logged in".to_string(),
        RvError::DeviceRemoved => "the card was removed; put it back in the reader".to_string(),
        RvError::DeviceError => "the card reader reported an error; replug the reader".to_string(),
        RvError::TokenNotPresent => "there is no card in the reader".to_string(),
        RvError::TokenNotRecognized => "the reader doesn't recognize the card".to_string(),
        RvError::SessionCount => "the card has no free session (another program is holding all of them)".to_string(),
        RvError::FunctionCanceled => "the PIN entry was cancelled".to_string(),
        RvError::KeyHandleInvalid => "the card no longer holds that key".to_string(),
        RvError::FunctionNotSupported | RvError::MechanismInvalid | RvError::MechanismParamInvalid => {
            "the card doesn't support this signature algorithm".to_string()
        }
        RvError::DataLenRange | RvError::SignatureLenRange => "the data is the wrong size for the card's key".to_string(),
        other => format!("the card's module reported {other:?}"),
    }
}

impl From<CryptokiError> for SignError {
    fn from(e: CryptokiError) -> Self {
        SignError::Crypto(describe(&e))
    }
}

#[cfg(test)]
mod software_token_tests {
    //! The whole path against a software token, with no hardware: SoftHSM2 (BSD-2-Clause, not
    //! shipped) generates a key, PdfCraft mints a certificate with it through `C_Sign`, lists it,
    //! signs a message and verifies the result. Run with:
    //!
    //! ```text
    //! PDFCRAFT_SOFTHSM_MODULE=/usr/lib/softhsm/libsofthsm2.so cargo test -p pdfcraft-sign --lib \
    //!     -- --ignored --nocapture
    //! ```
    //!
    //! The token must have a user PIN (`PDFCRAFT_SOFTHSM_PIN`, default 1234) and free room; the
    //! test removes the objects it creates, whose labels start with `pdfcraft-test-`.

    use super::*;
    use crate::keys::{Scheme, oid};
    use cryptoki::object::KeyType;

    const LABEL: &str = "pdfcraft-test";

    fn setup() -> Option<(PathBuf, String)> {
        let module = std::env::var_os("PDFCRAFT_SOFTHSM_MODULE")?;
        let pin = std::env::var("PDFCRAFT_SOFTHSM_PIN").unwrap_or_else(|_| "1234".into());
        Some((PathBuf::from(module), pin))
    }

    fn login(session: &Session, pin: &str) {
        // Signing logs the token out when it is done (a card left unlocked is a card anyone can
        // use), and the token's login is shared by every session on it, this one included.
        match session.login(UserType::User, Some(&AuthPin::new(pin.into()))) {
            Ok(()) | Err(CryptokiError::Pkcs11(RvError::UserAlreadyLoggedIn, _)) => {}
            Err(e) => panic!("the software token's PIN: {e:?}"),
        }
    }

    /// Remove what an earlier run left behind, so the listing counts only this run's objects.
    fn clean(session: &Session) {
        for class in [ObjectClass::CERTIFICATE, ObjectClass::PRIVATE_KEY, ObjectClass::PUBLIC_KEY] {
            for handle in session.find_objects(&[Attribute::Class(class)]).unwrap_or_default() {
                let label = match attr(session, handle, AttributeType::Label) {
                    Some(Attribute::Label(label)) => String::from_utf8_lossy(&label).into_owned(),
                    _ => String::new(),
                };
                if label.starts_with(LABEL) {
                    let _ = session.destroy_object(handle);
                }
            }
        }
    }

    /// The software token the test works on.
    struct Token {
        module: PathBuf,
        slot: Slot,
        serial: String,
        pin: String,
    }

    /// A key pair on the token, and a certificate for it made by the token itself.
    fn make_identity(token: &Token, session: &Session, ec: bool, id: &[u8]) {
        let Token { module, slot, serial, pin } = token;
        let label = if ec { format!("{LABEL}-ec") } else { format!("{LABEL}-rsa") };
        let label_bytes = label.as_bytes().to_vec();
        let mut public = vec![
            Attribute::Class(ObjectClass::PUBLIC_KEY),
            Attribute::Token(true),
            Attribute::Verify(true),
            Attribute::Id(id.to_vec()),
            Attribute::Label(label_bytes.clone()),
        ];
        let mut private = vec![
            Attribute::Class(ObjectClass::PRIVATE_KEY),
            Attribute::Token(true),
            Attribute::Private(true),
            Attribute::Sign(true),
            Attribute::Sensitive(true),
            Attribute::Id(id.to_vec()),
            Attribute::Label(label_bytes),
        ];
        let mechanism = if ec {
            public.push(Attribute::EcParams(der::oid(oid::P256)));
            public.push(Attribute::KeyType(KeyType::EC));
            private.push(Attribute::KeyType(KeyType::EC));
            Mechanism::EccKeyPairGen
        } else {
            public.push(Attribute::KeyType(KeyType::RSA));
            public.push(Attribute::ModulusBits(2048.into()));
            public.push(Attribute::PublicExponent(vec![1, 0, 1]));
            private.push(Attribute::KeyType(KeyType::RSA));
            Mechanism::RsaPkcsKeyPairGen
        };
        let (public_handle, private_handle) = session.generate_key_pair(&mechanism, &public, &private).expect("key generation");
        let key_public = if ec {
            let point = match attr(session, public_handle, AttributeType::EcPoint) {
                Some(Attribute::EcPoint(point)) => point,
                other => panic!("EC point: {other:?}"),
            };
            let point = Tlv::parse_all(&point).expect("DER OCTET STRING").octets("EC point").expect("point bytes").into_owned();
            PublicKey::P256(point)
        } else {
            let modulus = match attr(session, public_handle, AttributeType::Modulus) {
                Some(Attribute::Modulus(m)) => m,
                other => panic!("modulus: {other:?}"),
            };
            let exponent = match attr(session, public_handle, AttributeType::PublicExponent) {
                Some(Attribute::PublicExponent(e)) => e,
                other => panic!("exponent: {other:?}"),
            };
            PublicKey::Rsa { n: modulus, e: exponent }
        };
        let public = key_public.clone();
        let key = PrivateKey::external(
            key_public,
            Arc::new(TokenKey {
                module: module.clone(),
                slot: *slot,
                serial: serial.clone(),
                key_id: id.to_vec(),
                public,
                always_authenticate: false,
                protected_path: false,
                login: true,
                auth: Some(AuthPin::new(pin.as_str().into())),
            }),
        );
        // The certificate is signed by the token itself, through the same path a document is:
        // if the mechanism or the DigestInfo were wrong, this fails rather than the listing.
        let name = crate::Name::build(&label, "", "PdfCraft tests", "", "IT");
        let certificate = Certificate::self_signed(&name, &key, crate::Time::from_unix(1_700_000_000), 1, id).expect("a certificate");
        // Minting the certificate signed through the token, which logged it out afterwards.
        login(session, pin);
        session
            .create_object(&[
                Attribute::Class(ObjectClass::CERTIFICATE),
                Attribute::CertificateType(CertificateType::X_509),
                Attribute::Token(true),
                Attribute::Private(false),
                Attribute::Value(certificate.raw.clone()),
                Attribute::Subject(name.raw.clone()),
                Attribute::Id(id.to_vec()),
                Attribute::Label(certificate.subject.display_name().into_bytes()),
            ])
            .expect("the certificate object");
        let _ = private_handle;
    }

    #[test]
    #[ignore = "needs a SoftHSM2 token; set PDFCRAFT_SOFTHSM_MODULE"]
    fn a_software_token_lists_and_signs() {
        let Some((module, pin)) = setup() else {
            eprintln!("PDFCRAFT_SOFTHSM_MODULE not set; skipping");
            return;
        };
        let ctx = context(&module).expect("the module loads");
        let wanted = std::env::var("PDFCRAFT_SOFTHSM_TOKEN").ok();
        let slot = ctx
            .get_slots_with_token()
            .expect("slots")
            .into_iter()
            .find(|slot| {
                // SoftHSM reports its uninitialized slot as having a token too: only a
                // token that was initialized can hold keys.
                ctx.get_token_info(*slot).is_ok_and(|t| t.token_initialized() && wanted.as_deref().is_none_or(|label| t.label() == label))
            })
            .expect("an initialized software token");
        let serial = ctx.get_token_info(slot).expect("token info").serial_number().to_string();
        {
            let session = ctx.open_rw_session(slot).expect("a read-write session");
            login(&session, &pin);
            clean(&session);
            let token = Token { module: module.clone(), slot, serial, pin: pin.clone() };
            make_identity(&token, &session, false, b"01");
            login(&session, &pin);
            make_identity(&token, &session, true, b"02");
            let _ = session.logout();
        }
        let listing = identities(std::slice::from_ref(&module), None);
        assert!(listing.problems.is_empty(), "{:?}", listing.problems);
        assert_eq!(listing.identities.len(), 2, "one RSA and one P-256 identity");
        // The same two with the PIN offered while listing: the keys become visible and are
        // matched exactly, which is how a card that hides its certificates is read.
        let unlocked = identities(std::slice::from_ref(&module), Some(&pin));
        assert!(unlocked.problems.is_empty(), "{:?}", unlocked.problems);
        assert_eq!(unlocked.identities.len(), 2, "{:?}", unlocked.unusable);
        assert_eq!(unlocked.unusable.len(), 0);
        for identity in listing.identities {
            // A token with a PIN is listed without logging in, and asks for the PIN to sign.
            assert!(identity.needs_pin());
            let reference = identity.reference();
            let expected = identity.certificate.public_key.describe();
            let id = find(&reference, std::slice::from_ref(&module), Some(Zeroizing::new(pin.clone()))).expect("found by reference");
            assert_eq!(id.certificate.public_key.describe(), expected);
            let msg = b"signed through a token";
            let alg = id.key.preferred_digest();
            let signature = id.key.sign(alg, msg).expect("the token signs");
            let scheme = if expected.starts_with("RSA") { Scheme::RsaPkcs1 } else { Scheme::Ecdsa };
            let verified = id.certificate.public_key.verify(scheme, alg, &alg.digest(&[msg]), &signature).expect("a checkable signature");
            assert!(verified, "{expected}: the token's signature doesn't verify");
        }
        // A wrong PIN is reported as such, before anything is signed, and without a retry.
        let reference = identities(std::slice::from_ref(&module), None).identities[0].reference();
        let wrong = find(&reference, std::slice::from_ref(&module), Some(Zeroizing::new("9999".into()))).expect("resolved");
        let alg = wrong.key.preferred_digest();
        let refused = wrong.key.sign(alg, b"a document").expect_err("a wrong PIN doesn't sign");
        assert!(refused.to_string().contains("incorrect"), "{refused}");
        {
            let session = ctx.open_rw_session(slot).expect("a read-write session");
            login(&session, &pin);
            clean(&session);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_info_is_the_der_rfc_8017_asks_for() {
        let digest = [0x11u8; 32];
        let info = digest_info(DigestAlg::Sha256, &digest);
        let expected =
            [&[0x30, 0x31, 0x30, 0x0d, 0x06, 0x09][..], &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20][..], &digest]
                .concat();
        assert_eq!(info, expected);
        // The NULL parameter is there, as the standard requires for these digests.
        assert!(info.windows(2).any(|w| w == [0x05, 0x00]));
    }

    #[test]
    fn a_raw_ecdsa_signature_becomes_der_even_with_stripped_zeroes() {
        let point = vec![4u8; 65];
        let key = PublicKey::P256(point);
        let mut fixed = vec![0x7fu8; 64];
        fixed[0] = 0;
        fixed[32] = 0;
        let der = ecdsa_der(&key, &fixed).expect("fixed size");
        assert_eq!(der, key.store_signature_der(&fixed).unwrap());
        // Stripped leading zeroes are right-aligned to the key's width.
        let stripped = [&[0x01u8, 0x02][..], &[0x03, 0x04][..]].concat();
        let padded = ecdsa_der(&key, &stripped).expect("short halves");
        let mut expected = vec![0u8; 64];
        expected[30] = 0x01;
        expected[31] = 0x02;
        expected[62] = 0x03;
        expected[63] = 0x04;
        assert_eq!(padded, key.store_signature_der(&expected).unwrap());
    }

    #[test]
    fn impossible_ecdsa_lengths_are_refused() {
        let key = PublicKey::P256(vec![4u8; 65]);
        assert!(ecdsa_der(&key, &[]).is_err());
        assert!(ecdsa_der(&key, &[1u8; 63]).is_err());
        assert!(ecdsa_der(&key, &[1u8; 66]).is_err());
        assert!(ecdsa_der(&PublicKey::Ed25519(vec![0; 32]), &[1u8; 64]).is_err());
    }

    #[test]
    fn rsa_prefers_raw_pkcs1_and_falls_back_to_the_cards_own_hashing() {
        assert_eq!(rsa_mechanism(DigestAlg::Sha256, &[]), RsaMechanism::RawPkcs1);
        assert_eq!(rsa_mechanism(DigestAlg::Sha256, &[MechanismType::RSA_PKCS]), RsaMechanism::RawPkcs1);
        assert_eq!(rsa_mechanism(DigestAlg::Sha256, &[MechanismType::SHA256_RSA_PKCS]), RsaMechanism::HashInside(DigestAlg::Sha256));
        assert_eq!(
            rsa_mechanism(DigestAlg::Sha384, &[MechanismType::SHA384_RSA_PKCS, MechanismType::SHA256_RSA_PKCS]),
            RsaMechanism::HashInside(DigestAlg::Sha384)
        );
        assert_eq!(rsa_mechanism(DigestAlg::Sha512, &[MechanismType::SHA256_RSA_PKCS]), RsaMechanism::Unsupported);
    }

    #[test]
    fn references_and_errors_are_stable_and_readable() {
        let key = PrivateKey::generate_p256().expect("a P-256 key");
        let name = crate::Name::build("Test Signer", "", "", "", "IT");
        let cert = Certificate::self_signed(&name, &key, crate::Time::from_unix(0), 1, &[1, 2, 3]).expect("a certificate");
        let reference = reference(&cert);
        assert!(reference.starts_with("pkcs11:"));
        assert_eq!(reference.len(), "pkcs11:".len() + 64);
        assert!(reference["pkcs11:".len()..].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

        let locked = CryptokiError::Pkcs11(RvError::PinLocked, cryptoki::context::Function::Login);
        assert!(describe(&locked).contains("locked"));
        assert!(describe(&CryptokiError::Pkcs11(RvError::PinIncorrect, cryptoki::context::Function::Login)).contains("incorrect"));
        assert_eq!(
            describe(&CryptokiError::Pkcs11(RvError::DeviceRemoved, cryptoki::context::Function::Sign)),
            "the card was removed; put it back in the reader"
        );
        // The PIN never appears in a message, whatever the module said.
        assert!(!describe(&locked).contains("1234"));
    }
}
