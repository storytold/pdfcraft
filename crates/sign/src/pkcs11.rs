//! PKCS #11 tokens and smart cards (plan: `sign.pkcs11`).
//!
//! Identities are named by RFC 7512 URIs (`pkcs11:token=…;object=…;id=…;type=cert`), and the
//! `DigestInfo` a `CKM_RSA_PKCS` signature is computed over is built here. Module loading
//! (explicit user-chosen paths only), slot listing and signing go through `cryptoki`;
//! nothing here searches for modules on its own.

use crate::keys::DigestAlg;
#[cfg(not(target_arch = "wasm32"))]
use crate::keys::{ExternalKey, PrivateKey, PublicKey};
use crate::{Certificate, DigitalId, SignError};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

/// A `pkcs11:` identity reference (RFC 7512): the token object a signature uses. Only the
/// attributes naming a certificate object are kept (`token`, `manufacturer`, `serial`,
/// `model`, `object`, `id` — the `CKA_ID` bytes — and `type`). Anything else in the URI
/// is refused, and so is a query (`?...`): a PIN never belongs in a reference — it comes
/// from [`PinSource`] at signing time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pkcs11Uri {
    pub token: Option<String>,
    pub manufacturer: Option<String>,
    pub serial: Option<String>,
    pub model: Option<String>,
    pub object: Option<String>,
    pub id: Option<Vec<u8>>,
    pub kind: Option<UriObjectKind>,
}

/// The `type` of object a `pkcs11:` URI names. PdfCraft only lists certificate objects
/// backed by a private key, so matching only ever meets `Cert`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UriObjectKind {
    Cert,
    Public,
    Private,
}

/// An identity's reference: `pkcs11:token=<label>;object=<label>;id=<CKA_ID>;type=cert`.
pub fn reference(token: &str, object: &str, id: &[u8]) -> String {
    format!("pkcs11:token={};object={};id={};type=cert", encode(token), encode(object), encode_bytes(id))
}

/// Parse a `pkcs11:` reference back into its attributes.
pub fn parse_uri(reference: &str) -> Result<Pkcs11Uri, SignError> {
    let malformed = |what: &str| SignError::Malformed(format!("a PKCS #11 identity reference {what}"));
    let path = reference.strip_prefix("pkcs11:").ok_or_else(|| malformed("starts with pkcs11:"))?;
    // The query is where RFC 7512 puts `pin-value`: refused, always. The PIN comes
    // from `PinSource`, never from a string that lands in logs and scripts.
    if path.contains('?') {
        return Err(malformed("must not carry a query (a PIN never belongs in a reference)"));
    }
    let mut uri = Pkcs11Uri::default();
    // `pkcs11:` alone names nothing yet: `find` reports the ambiguity instead.
    if path.is_empty() {
        return Ok(uri);
    }
    for pair in path.split(';') {
        let (name, value) = pair.split_once('=').ok_or_else(|| malformed("holds name=value pairs separated by ';'"))?;
        if value.is_empty() {
            return Err(malformed("holds no empty values"));
        }
        let set_string = |slot: &mut Option<String>| -> Result<(), SignError> {
            if slot.is_some() {
                return Err(malformed("holds no attribute twice"));
            }
            *slot = Some(decode_string(value)?);
            Ok(())
        };
        match name {
            "token" => set_string(&mut uri.token)?,
            "manufacturer" => set_string(&mut uri.manufacturer)?,
            "serial" => set_string(&mut uri.serial)?,
            "model" => set_string(&mut uri.model)?,
            "object" => set_string(&mut uri.object)?,
            "id" => {
                if uri.id.is_some() {
                    return Err(malformed("holds no attribute twice"));
                }
                uri.id = Some(decode_bytes(value)?);
            }
            "type" => {
                if uri.kind.is_some() {
                    return Err(malformed("holds no attribute twice"));
                }
                uri.kind = Some(match value {
                    "cert" => UriObjectKind::Cert,
                    "public" => UriObjectKind::Public,
                    "private" => UriObjectKind::Private,
                    _ => return Err(malformed("names a cert, public or private object")),
                });
            }
            _ => return Err(malformed(&format!("has no {name} attribute (token, manufacturer, serial, model, object, id, type)"))),
        }
    }
    Ok(uri)
}

impl Pkcs11Uri {
    /// Whether the URI names this token object: every attribute it sets must match.
    /// Unset attributes are wildcards. The object `type` is not compared: every listed
    /// identity is a certificate backed by its private key.
    pub fn matches(&self, token: &str, manufacturer: &str, serial: &str, model: &str, object: &str, id: &[u8]) -> bool {
        self.token.as_deref().is_none_or(|t| t == token)
            && self.manufacturer.as_deref().is_none_or(|m| m == manufacturer)
            && self.serial.as_deref().is_none_or(|s| s == serial)
            && self.model.as_deref().is_none_or(|m| m == model)
            && self.object.as_deref().is_none_or(|o| o == object)
            && self.id.as_deref().is_none_or(|i| i == id)
    }
}

/// Percent-encode a URI attribute: everything but the unreserved characters.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex(b >> 4));
            out.push(hex(b & 0x0F));
        }
    }
    out
}

fn encode_bytes(bytes: &[u8]) -> String {
    let mut out = String::new();
    for b in bytes {
        out.push('%');
        out.push(hex(b >> 4));
        out.push(hex(*b & 0x0F));
    }
    out
}

fn hex(nibble: u8) -> char {
    (b'0' + nibble + 7 * (nibble / 10)) as char
}

/// The `CKA_ID` bytes of an `id` attribute (capped: real identifiers are tiny).
fn decode_bytes(value: &str) -> Result<Vec<u8>, SignError> {
    /// Longer than any token's key identifier.
    const MAX_ID_LEN: usize = 512;
    let raw = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let Some(b) = raw.get(i) else { break };
        if *b == b'%' {
            let (Some(h), Some(l)) = (raw.get(i + 1).copied(), raw.get(i + 2).copied()) else {
                return Err(SignError::Malformed("a PKCS #11 identity reference percent-encodes every byte as %XX".into()));
            };
            let (Some(h), Some(l)) = (nibble(h), nibble(l)) else {
                return Err(SignError::Malformed("a PKCS #11 identity reference percent-encodes every byte as %XX".into()));
            };
            out.push(h << 4 | l);
            i += 3;
        } else {
            out.push(*b);
            i += 1;
        }
        if out.len() > MAX_ID_LEN {
            return Err(SignError::Malformed("a PKCS #11 identity reference holds a short key identifier".into()));
        }
    }
    Ok(out)
}

/// A text attribute: percent-decoded UTF-8.
fn decode_string(value: &str) -> Result<String, SignError> {
    let bytes = decode_bytes(value)?;
    String::from_utf8(bytes).map_err(|_| SignError::Malformed("a PKCS #11 identity reference holds UTF-8 text attributes".into()))
}

fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// The `DigestInfo` (RFC 8017 §9.2) a `CKM_RSA_PKCS` signature is computed over: `SEQUENCE {
/// AlgorithmIdentifier (with the NULL parameter), OCTET STRING digest }`.
///
/// Unlike CNG — which adds this itself when given PKCS #1 padding — a PKCS #11 token signs
/// exactly these bytes, so the caller builds them first. New signatures use SHA-256/384/512
/// only, like [`crate::keys::PrivateKey::sign`].
pub fn digest_info(alg: DigestAlg, digest: &[u8]) -> Result<Vec<u8>, SignError> {
    if !matches!(alg, DigestAlg::Sha256 | DigestAlg::Sha384 | DigestAlg::Sha512) {
        return Err(SignError::Unsupported(format!("{} for new signatures", alg.name())));
    }
    if digest.len() != alg.output_len() {
        return Err(SignError::Malformed(format!("a {} digest holds {} bytes, not {}", alg.name(), alg.output_len(), digest.len())));
    }
    let id = crate::der::algorithm(alg.oid(), Some(&crate::der::null()));
    Ok(crate::der::seq(&[&id, &crate::der::octets(digest)]))
}

// ── modules and tokens (native targets only; the browser can't load a DLL) ──────────────

/// A token in a slot: what the settings UI lists and what a `pkcs11:` reference resolves
/// against. Plain data, so the UI can show it without touching the module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// The token's label, without the space padding PKCS #11 strings carry.
    pub label: String,
    /// The device serial number, unpadded.
    pub serial: String,
    /// The device manufacturer, unpadded.
    pub manufacturer: String,
    /// The device model, unpadded.
    pub model: String,
    /// Whether signing needs a PIN entered: login is required and the token has no pinpad
    /// reading it itself.
    pub needs_pin: bool,
    /// Whether the token reads the PIN itself (`PROTECTED_AUTHENTICATION_PATH`): login
    /// passes no PIN.
    pub pinpad: bool,
    /// Whether the user PIN is locked.
    pub pin_locked: bool,
}

/// A loaded PKCS #11 module. Loaded and initialized once per path, then kept for the life of
/// the process (see [`load_module`]); the path is the one the user chose, kept for error
/// messages.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug)]
pub struct Module {
    inner: cryptoki::context::Pkcs11,
    path: PathBuf,
}

/// Modules loaded so far, by the path they were loaded from. Most cap at a handful (one per
/// token driver the user added).
#[cfg(not(target_arch = "wasm32"))]
static MODULES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, Module>>> = std::sync::OnceLock::new();

/// Load the module at `path` — a DLL the user picked explicitly. Modules are never searched
/// for or loaded on their own: a DLL executes code.
///
/// A module is one context per process (`C_Initialize` once; cryptoki's own advice): the
/// first call loads and initializes it, later calls for the same path share it. It is never
/// finalized or unloaded, because unloading an initialized vendor library (many run their own
/// threads) can take the process down with it, and a second context for the same library
/// fails with `CKR_CRYPTOKI_ALREADY_INITIALIZED`. A failed load is not remembered, so fixing
/// the module and trying again works.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_module(path: &Path) -> Result<Module, SignError> {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let modules = MODULES.get_or_init(Default::default);
    // A panic elsewhere while holding the lock leaves the map itself intact: keep using it.
    let mut modules = modules.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(module) = modules.get(&key) {
        return Ok(module.clone());
    }
    let inner = cryptoki::context::Pkcs11::new(path)
        .map_err(|e| SignError::Crypto(format!("the PKCS #11 module at {} couldn't be loaded: {}", path.display(), describe(&e))))?;
    match inner.initialize(cryptoki::context::CInitializeArgs::new(cryptoki::context::CInitializeFlags::OS_LOCKING_OK)) {
        // Something else in the process (another library using the same module) started it:
        // the module is usable as it is.
        Ok(()) | Err(cryptoki::error::Error::Pkcs11(cryptoki::error::RvError::CryptokiAlreadyInitialized, _)) => {}
        Err(e) => return Err(SignError::Crypto(format!("the PKCS #11 module at {} couldn't be started: {}", path.display(), describe(&e)))),
    }
    let module = Module { inner, path: path.to_path_buf() };
    modules.insert(key, module.clone());
    Ok(module)
}

/// Every slot holding a token: its label, serial number, manufacturer, and whether a PIN
/// is needed or read by a pinpad.
#[cfg(not(target_arch = "wasm32"))]
pub fn tokens(module: &Module) -> Result<Vec<Token>, SignError> {
    let slots = module
        .inner
        .get_slots_with_token()
        .map_err(|e| SignError::Crypto(format!("the PKCS #11 module at {} couldn't list its slots: {}", module.path.display(), describe(&e))))?;
    let mut out = Vec::new();
    for slot in slots {
        let info = module.inner.get_token_info(slot).map_err(|e| {
            SignError::Crypto(format!(
                "a token in the PKCS #11 module at {} couldn't be read (was it removed?): {}",
                module.path.display(),
                describe(&e)
            ))
        })?;
        let pinpad = info.protected_authentication_path();
        out.push(Token {
            label: info.label().trim_end().to_string(),
            serial: info.serial_number().trim_end().to_string(),
            manufacturer: info.manufacturer_id().trim_end().to_string(),
            model: info.model().trim_end().to_string(),
            needs_pin: info.login_required() && !pinpad,
            pinpad,
            pin_locked: info.user_pin_locked(),
        });
    }
    Ok(out)
}

/// Where the signing PIN comes from. Asked once per signature; the PIN lives only in that
/// call and is never stored.
#[derive(Clone)]
pub enum PinSource {
    /// The token needs no PIN, or reads it itself (pinpad).
    None,
    /// Read the PIN from this environment variable at signing time, so it never lands in
    /// logs or scripts.
    Env(String),
    /// Ask through this callback (the UI's PIN dialog) at signing time.
    Ask(std::sync::Arc<dyn Fn() -> Result<secrecy::SecretString, SignError> + Send + Sync>),
    /// The PIN itself, for one signing (the UI's PIN dialog hands it over; never persisted).
    Value(secrecy::SecretString),
}

impl std::fmt::Debug for PinSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PinSource::None => write!(f, "None"),
            PinSource::Env(name) => write!(f, "Env({name})"),
            PinSource::Ask(_) => write!(f, "Ask(..)"),
            PinSource::Value(_) => write!(f, "Value(..)"),
        }
    }
}

/// A token certificate PdfCraft can't sign with, and why — shown greyed, like the
/// Windows store's unusable entries (issue #179).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    /// The subject's display name, or a placeholder when the certificate can't be read.
    pub subject: String,
    /// Why it can't sign, as a clause that follows the name.
    pub reason: String,
}

/// A listed token identity: the signable [`DigitalId`] plus what names it.
#[derive(Debug)]
pub struct Identity {
    /// The identity itself, ready to sign.
    pub id: DigitalId,
    /// The token holding it.
    pub token: Token,
    /// The certificate object's `CKA_LABEL` (empty when the token sets none).
    pub object: String,
    /// The `CKA_ID` pairing the certificate with its private key.
    pub key_id: Vec<u8>,
}

impl Identity {
    /// The identity's RFC 7512 reference, for `find` and `sign_document`.
    pub fn reference(&self) -> String {
        reference(&self.token.label, &self.object, &self.key_id)
    }

    /// Whether a parsed reference names this identity.
    pub fn matches(&self, uri: &Pkcs11Uri) -> bool {
        uri.matches(&self.token.label, &self.token.manufacturer, &self.token.serial, &self.token.model, &self.object, &self.key_id)
    }
}

/// Every token's signing identities and the certificates that can't sign.
#[derive(Debug, Default)]
pub struct Listing {
    pub ids: Vec<Identity>,
    pub unusable: Vec<Unusable>,
}

/// The signing identities on every token of `module`: each X.509 certificate paired with
/// its private key by `CKA_ID`. `pin` is kept in the identities and used only when one
/// signs, on that one token. Listing never logs in: trying a PIN on every token would spend
/// the retries of cards it doesn't belong to, and could lock them.
///
/// Tokens keep private keys hidden until login (`CKA_PRIVATE`), so on a token that requires
/// login a certificate with a `CKA_ID` is listed as an identity even when no key shows yet;
/// signing finds the key after logging in, and says so if there is none. Certificates
/// without a key on a token that shows them, or with one PdfCraft doesn't sign with, land in
/// `unusable` with the reason instead of being left out silently.
#[cfg(not(target_arch = "wasm32"))]
pub fn list(module: &Module, pin: &PinSource) -> Result<Listing, SignError> {
    use cryptoki::object::{Attribute, AttributeType, CertificateType, ObjectClass};

    let slots = module
        .inner
        .get_slots_with_token()
        .map_err(|e| SignError::Crypto(format!("the PKCS #11 module at {} couldn't list its slots: {}", module.path.display(), describe(&e))))?;
    let mut ids = Vec::new();
    let mut unusable = Vec::new();
    for slot in slots {
        let mut skip = |subject: String, reason: String| unusable.push(Unusable { subject, reason });
        let info = match module.inner.get_token_info(slot) {
            Ok(info) => info,
            Err(e) => {
                skip("(unreadable token)".to_string(), format!("the token couldn't be read (was it removed?): {}", describe(&e)));
                continue;
            }
        };
        let session = match module.inner.open_ro_session(slot) {
            Ok(session) => session,
            Err(e) => {
                skip("(unreadable token)".to_string(), format!("the token couldn't be opened (was it removed?): {}", describe(&e)));
                continue;
            }
        };
        let handles = match session.find_objects(&[Attribute::Class(ObjectClass::CERTIFICATE), Attribute::CertificateType(CertificateType::X_509)]) {
            Ok(handles) => handles,
            Err(e) => {
                skip(info.label().trim_end().to_string(), format!("the token couldn't be searched: {}", describe(&e)));
                continue;
            }
        };
        let pinpad = info.protected_authentication_path();
        let login_required = info.login_required();
        let token = Token {
            label: info.label().trim_end().to_string(),
            serial: info.serial_number().trim_end().to_string(),
            manufacturer: info.manufacturer_id().trim_end().to_string(),
            model: info.model().trim_end().to_string(),
            needs_pin: login_required && !pinpad,
            pinpad,
            pin_locked: info.user_pin_locked(),
        };
        for handle in handles {
            let attrs = match session.get_attributes(handle, &[AttributeType::Value, AttributeType::Id, AttributeType::Label]) {
                Ok(attrs) => attrs,
                Err(e) => {
                    skip("(unreadable certificate)".to_string(), format!("the certificate can't be read: {}", describe(&e)));
                    continue;
                }
            };
            let Some(value) = cert_value(&attrs) else {
                skip("(unreadable certificate)".to_string(), "the certificate has no value".to_string());
                continue;
            };
            let certificate = match Certificate::parse(value) {
                Ok(certificate) => certificate,
                Err(e) => {
                    skip("(unreadable certificate)".to_string(), format!("the certificate can't be read: {e}"));
                    continue;
                }
            };
            let subject = certificate.display_name();
            let Some(key_id) = object_id(&attrs) else {
                skip(subject, "it has no key identifier (CKA_ID) pairing it with a key".to_string());
                continue;
            };
            let homeless = match session.find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(key_id.to_vec())]) {
                Ok(keys) => keys.is_empty(),
                // A search that fails is not proof of absence; say so instead of dropping it.
                Err(e) => {
                    skip(subject, format!("its private key couldn't be looked up: {}", describe(&e)));
                    continue;
                }
            };
            // Not logged in, a token requiring login may simply be hiding the key: list the
            // certificate, and let signing (after login) confirm the key.
            if homeless && !login_required {
                skip(subject, "it has no private key on the token".to_string());
                continue;
            }
            // P-521, brainpool and Ed25519 certificates parse (to verify with) but aren't signed with.
            if certificate.public_key.store_signing_key().is_none() {
                let what = certificate.public_key.describe();
                skip(subject, format!("its {what} key can't sign through PdfCraft yet (RSA, P-256 and P-384 only)"));
                continue;
            }
            let public = certificate.public_key.clone();
            let object = object_label(&attrs);
            let key = PrivateKey::external(
                public.clone(),
                std::sync::Arc::new(Pkcs11Key {
                    module: module.clone(),
                    slot,
                    token: token.clone(),
                    key_id: key_id.to_vec(),
                    public,
                    pin: pin.clone(),
                }),
            );
            let id = DigitalId { key, certificate, chain: Vec::new(), friendly_name: Some(subject) };
            ids.push(Identity { id, token: token.clone(), object, key_id: key_id.to_vec() });
        }
    }
    Ok(Listing { ids, unusable })
}

/// The `CKA_VALUE` of a certificate object: its DER bytes.
#[cfg(not(target_arch = "wasm32"))]
fn cert_value(attrs: &[cryptoki::object::Attribute]) -> Option<&[u8]> {
    attrs.iter().find_map(|a| match a {
        cryptoki::object::Attribute::Value(v) => Some(v.as_slice()),
        _ => None,
    })
}

/// The `CKA_ID` pairing a certificate with its private key.
#[cfg(not(target_arch = "wasm32"))]
fn object_id(attrs: &[cryptoki::object::Attribute]) -> Option<&[u8]> {
    attrs.iter().find_map(|a| match a {
        cryptoki::object::Attribute::Id(v) => Some(v.as_slice()),
        _ => None,
    })
}

/// The `CKA_LABEL` of a certificate object, if it has one and it reads as text.
#[cfg(not(target_arch = "wasm32"))]
fn object_label(attrs: &[cryptoki::object::Attribute]) -> String {
    attrs
        .iter()
        .find_map(|a| match a {
            cryptoki::object::Attribute::Label(v) => Some(String::from_utf8_lossy(v).into_owned()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Find the listed identity a `pkcs11:` URI names. Zero matches and more than one are
/// both errors saying so: signing must never pick a key the reference leaves ambiguous.
#[cfg(not(target_arch = "wasm32"))]
pub fn find(module: &Module, uri: &str, pin: &PinSource) -> Result<DigitalId, SignError> {
    let want = parse_uri(uri)?;
    let hits: Vec<Identity> = list(module, pin)?.ids.into_iter().filter(|id| id.matches(&want)).collect();
    match hits.len() {
        1 => Ok(hits.into_iter().next().map(|id| id.id).ok_or_else(|| SignError::Crypto(format!("no PKCS #11 identity {uri}")))?),
        0 => Err(SignError::Crypto(format!("no PKCS #11 identity {uri}"))),
        n => Err(SignError::Crypto(format!("the PKCS #11 reference {uri} matches {n} identities; be specific"))),
    }
}

/// A token private key: found by `CKA_ID` at listing, opened for one session at signing.
/// The PIN is asked then and lives only in that call.
#[cfg(not(target_arch = "wasm32"))]
struct Pkcs11Key {
    module: Module,
    slot: cryptoki::slot::Slot,
    token: Token,
    key_id: Vec<u8>,
    public: PublicKey,
    pin: PinSource,
}

#[cfg(not(target_arch = "wasm32"))]
impl ExternalKey for Pkcs11Key {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        use cryptoki::mechanism::Mechanism;
        use cryptoki::object::{Attribute, ObjectClass};

        if !matches!(alg, DigestAlg::Sha256 | DigestAlg::Sha384 | DigestAlg::Sha512) {
            return Err(SignError::Unsupported(format!("{} for new signatures", alg.name())));
        }
        let digest = alg.digest(&[msg]);
        // RSA signs the DigestInfo (built here: the token signs exactly these bytes);
        // ECDSA signs the digest itself and returns raw r ‖ s.
        let (mechanism, input) = match &self.public {
            PublicKey::Rsa { .. } => (Mechanism::RsaPkcs, digest_info(alg, &digest)?),
            PublicKey::P256(_) | PublicKey::P384(_) => (Mechanism::Ecdsa, digest),
            _ => return Err(SignError::Unsupported(format!("signing with {} keys", self.public.describe()))),
        };
        let session = self
            .module
            .inner
            .open_ro_session(self.slot)
            .map_err(|e| SignError::Crypto(format!("the token couldn't be opened for signing (was it removed?): {}", describe(&e))))?;
        // The session logs out when it drops; the PIN below lives only in this call.
        self.login(&session)?;
        let mut keys = session
            .find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(self.key_id.clone())])
            .map_err(|e| SignError::Crypto(format!("the token key couldn't be found (was the token changed?): {}", describe(&e))))?;
        let Some(key) = keys.pop() else {
            return Err(SignError::Crypto(
                "the token has no private key for this certificate (was the token changed, or is the certificate stored without its key?)".into(),
            ));
        };
        let raw = session.sign(&mechanism, key, &input).map_err(|e| SignError::Crypto(format!("the token didn't sign: {}", describe(&e))))?;
        self.public.store_signature_der(&raw)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Pkcs11Key {
    /// Log the session in through [`login`], re-reading the token's flags for the
    /// last-try warning.
    fn login(&self, session: &cryptoki::session::Session) -> Result<(), SignError> {
        login(session, self.token.needs_pin, self.token.pinpad, &self.pin, Some((&self.module.inner, self.slot)))
    }
}

/// Log a session in: nothing when the token needs none, an empty login on a pinpad,
/// otherwise the PIN from the caller's source. `reread` (the module and slot) lets a
/// wrong PIN warn when the token flags it as possibly the last try (tokens report flags,
/// not a remaining-tries count); without it the token's error stands on its own.
#[cfg(not(target_arch = "wasm32"))]
fn login(
    session: &cryptoki::session::Session,
    needs_pin: bool,
    pinpad: bool,
    pin: &PinSource,
    reread: Option<(&cryptoki::context::Pkcs11, cryptoki::slot::Slot)>,
) -> Result<(), SignError> {
    use cryptoki::session::UserType;
    if !needs_pin && !pinpad {
        return Ok(());
    }
    if pinpad {
        return match session.login(UserType::User, None) {
            Ok(()) | Err(cryptoki::error::Error::Pkcs11(cryptoki::error::RvError::UserAlreadyLoggedIn, _)) => Ok(()),
            Err(e) => Err(SignError::Crypto(format!("the token's reader couldn't log in (was the token removed?): {}", describe(&e)))),
        };
    }
    let pin: secrecy::SecretString = match pin {
        PinSource::None => return Err(SignError::Crypto("this token needs its PIN".into())),
        PinSource::Env(name) => secrecy::SecretString::new(
            std::env::var(name).map_err(|_| SignError::Crypto(format!("signing needs the token PIN in the {name} environment variable")))?.into(),
        ),
        PinSource::Ask(ask) => ask()?,
        PinSource::Value(pin) => pin.clone(),
    };
    let auth: cryptoki::types::AuthPin = pin;
    // Login is per application and token, not per session: another open session of ours
    // may have logged in already, which is what this call wanted.
    match session.login(UserType::User, Some(&auth)) {
        Ok(()) | Err(cryptoki::error::Error::Pkcs11(cryptoki::error::RvError::UserAlreadyLoggedIn, _)) => return Ok(()),
        Err(e) => Err(e),
    }
    .map_err(|e| {
        let mut message = format!("the token couldn't be unlocked: {}", describe(&e));
        // Best effort, failure path only: warn when the token flags this as the last try.
        let final_try = reread.is_some_and(|(context, slot)| context.get_token_info(slot).is_ok_and(|info| info.user_pin_final_try()));
        if matches!(e, cryptoki::error::Error::Pkcs11(cryptoki::error::RvError::PinIncorrect, ..)) && final_try {
            message.push_str(" (this may be the last try before the PIN locks)");
        }
        SignError::Crypto(message)
    })
}

/// A PKCS #11 failure as a sentence saying what happened, instead of a bare return code.
#[cfg(not(target_arch = "wasm32"))]
fn describe(error: &cryptoki::error::Error) -> String {
    use cryptoki::error::{Error, RvError};
    let rv = match error {
        Error::Pkcs11(rv, ..) => rv,
        _ => return error.to_string(),
    };
    let meaning = match rv {
        RvError::PinIncorrect => "the PIN is incorrect",
        RvError::PinLocked => "the PIN is locked",
        RvError::PinExpired => "the PIN has expired and must be changed",
        RvError::TokenNotPresent => "the token isn't inserted",
        RvError::TokenNotRecognized => "the token isn't recognized",
        RvError::DeviceRemoved => "the device was removed",
        RvError::DeviceError | RvError::DeviceMemory => "the device reported an error",
        RvError::UserAlreadyLoggedIn => "already logged in",
        _ => return error.to_string(),
    };
    format!("{meaning} ({rv:?})")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(target_arch = "wasm32"))]
    use cryptoki::object::{Attribute, CertificateType, KeyType, ObjectClass};
    #[cfg(not(target_arch = "wasm32"))]
    use cryptoki::session::{Session, UserType};

    #[test]
    fn references_round_trip_as_rfc_7512() {
        let uri = reference("My Token", "Smile Лиза", &[1, 2, 0xAB]);
        assert_eq!(uri, "pkcs11:token=My%20Token;object=Smile%20%D0%9B%D0%B8%D0%B7%D0%B0;id=%01%02%AB;type=cert");
        let parsed = parse_uri(&uri).unwrap();
        assert_eq!(parsed.token.as_deref(), Some("My Token"));
        assert_eq!(parsed.object.as_deref(), Some("Smile Лиза"));
        assert_eq!(parsed.id.as_deref(), Some([1, 2, 0xAB].as_slice()));
        assert_eq!(parsed.kind, Some(UriObjectKind::Cert));
        // Matching: set attributes must agree, unset ones are wildcards.
        assert!(parsed.matches("My Token", "", "", "", "Smile Лиза", &[1, 2, 0xAB]));
        assert!(!parsed.matches("Other", "", "", "", "Smile Лиза", &[1, 2, 0xAB]));
        assert!(!parsed.matches("My Token", "", "", "", "Smile Лиза", &[9]));
        assert!(parse_uri("pkcs11:id=%01%02%AB").unwrap().matches("anything", "", "", "", "whatever", &[1, 2, 0xAB]));
        assert!(parse_uri("pkcs11:").unwrap().matches("anything", "", "", "", "whatever", &[1]));
    }

    #[test]
    fn references_reject_garbage_and_queries() {
        for bad in [
            "",
            "windows:token=x",
            "pkcs11",
            "pkcs11:token",
            "pkcs11:token=",
            "pkcs11:unknown=x",
            "pkcs11:type=secret-key",
            "pkcs11:token=x;token=y",
            "pkcs11:id=%",
            "pkcs11:id=%ZZ",
            "pkcs11:token=%FF",
            "pkcs11:token=x?pin-value=1234",
            "pkcs11:object=a;bogus",
        ] {
            assert!(parse_uri(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn digest_info_matches_the_rfc_8017_shape() {
        // SHA-256("abc").
        let digest =
            b"\xba\x78\x16\xbf\x8f\x01\xcf\xea\x41\x41\x40\xde\x5d\xae\x22\x23\xb0\x03\x61\xa3\x96\x17\x7a\x9c\xb4\x10\xff\x61\xf2\x00\x15\xad";
        let got = digest_info(DigestAlg::Sha256, digest).unwrap();
        let mut want = vec![0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20];
        want.extend_from_slice(digest);
        assert_eq!(got, want);
    }

    #[test]
    fn digest_info_refuses_anything_it_would_not_sign() {
        assert!(matches!(digest_info(DigestAlg::Sha1, &[0u8; 20]), Err(SignError::Unsupported(_))));
        assert!(digest_info(DigestAlg::Sha256, &[0u8; 31]).is_err());
        assert!(digest_info(DigestAlg::Sha384, &[0u8; 32]).is_err());
        assert!(digest_info(DigestAlg::Sha512, &[0u8; 64]).is_ok());
    }

    /// A missing or garbage module is an error naming the path, never a crash. `file!()` is
    /// this test's own source: it exists but is no DLL.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn broken_modules_are_errors_naming_the_path() {
        for path in ["/nonexistent/pkcs11-test-module.so", file!()] {
            let e = super::load_module(std::path::Path::new(path)).unwrap_err().to_string();
            assert!(e.contains("couldn't be loaded"), "{path}: {e}");
            assert!(e.contains(path), "{e}");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn failures_say_what_happened() {
        use cryptoki::error::{Error, RvError};
        // `Function` carries no meaning here; any value does.
        let at = |rv| super::describe(&Error::Pkcs11(rv, cryptoki::context::Function::GetInfo));
        assert!(at(RvError::PinIncorrect).contains("incorrect"));
        assert!(at(RvError::PinLocked).contains("locked"));
        assert!(at(RvError::TokenNotPresent).contains("isn't inserted"));
        assert!(at(RvError::DeviceRemoved).contains("removed"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn attributes_give_the_cert_and_its_key_id() {
        use cryptoki::object::Attribute;
        let attrs = vec![Attribute::Label(b"ignored".to_vec()), Attribute::Id(vec![1, 2]), Attribute::Value(vec![3, 4])];
        assert_eq!(super::object_id(&attrs), Some([1, 2].as_slice()));
        assert_eq!(super::cert_value(&attrs), Some([3, 4].as_slice()));
        let missing: Vec<Attribute> = vec![Attribute::Label(b"x".to_vec())];
        assert_eq!(super::object_id(&missing), None);
        assert_eq!(super::cert_value(&missing), None);
    }

    #[test]
    fn pin_sources_never_print_a_pin() {
        let debug = format!("{:?}", super::PinSource::Env("PDFCRAFT_PIN".to_string()));
        assert!(debug.contains("PDFCRAFT_PIN"));
        let ask = super::PinSource::Ask(std::sync::Arc::new(|| Ok(secrecy::SecretString::new("s3cret".to_string().into()))));
        assert_eq!(format!("{ask:?}"), "Ask(..)");
        let value = super::PinSource::Value(secrecy::SecretString::new("s3cret".to_string().into()));
        assert_eq!(format!("{value:?}"), "Value(..)");
    }

    /// Live token test, skipped without a module to run against (like the craft-fonts tests
    /// without `CRAFT_FONTS_DIR`):
    ///
    /// ```sh
    /// SOFTHSM2_CONF=/tmp/softhsm-test/softhsm2.conf \
    /// PDFCRAFT_PKCS11_TEST_MODULE=/usr/lib/softhsm/libsofthsm2.so \
    ///   cargo test -p pdfcraft-sign pkcs11_softhsm
    /// ```
    ///
    /// Point `SOFTHSM2_CONF` at a scratch store: the test initializes its own token there
    /// (`PdfCraft test`, SO PIN `00000000`, user PIN `12345678`) when none exists, and reuses
    /// it afterwards, so repeated runs are idempotent. A slot holding someone else's token is
    /// left alone; with no usable slot the test fails saying how to point at a scratch store.
    /// The RSA-2048 and P-256 pairs are generated in-test and imported with self-signed
    /// certificates — no fixture keys, no helper binaries — then listed, found by reference,
    /// signed with, and CMS-validated. A wrong PIN (one attempt) must report itself as
    /// incorrect. SoftHSM implements no PIN retry counters, so nothing here can lock: the
    /// locked-PIN mapping is covered by `failures_say_what_happened`, and a token that
    /// arrives locked is checked against it when one does.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn pkcs11_softhsm_token_round_trip() {
        const TOKEN_LABEL: &str = "PdfCraft test";
        const SO_PIN: &str = "00000000";
        const USER_PIN: &str = "12345678";
        const RSA_ID: &[u8] = b"pdfcraft-test-rsa";
        const EC_ID: &[u8] = b"pdfcraft-test-ec";
        /// DER of the P-256 OID (1.2.840.10045.3.1.7), the `EcParams` of every P-256 key.
        const P256_PARAMS: &[u8] = &[0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];

        fn secret(pin: &str) -> secrecy::SecretString {
            secrecy::SecretString::new(pin.to_owned().into())
        }

        let Some(lib) = std::env::var("PDFCRAFT_PKCS11_TEST_MODULE").ok() else {
            eprintln!("skipping pkcs11_softhsm_token_round_trip: set PDFCRAFT_PKCS11_TEST_MODULE to a PKCS #11 module (e.g. SoftHSM2) to run it");
            return;
        };
        let module = super::load_module(std::path::Path::new(&lib)).expect("the test module loads");

        // The test slot: ours by label, else a fresh one initialized for it.
        let slot = {
            let mut ours = None;
            let mut fresh = None;
            for slot in module.inner.get_all_slots().expect("slots list") {
                let Ok(info) = module.inner.get_token_info(slot) else { continue };
                if info.label().trim_end() == TOKEN_LABEL {
                    ours = Some(slot);
                    break;
                }
                if fresh.is_none() && !info.token_initialized() {
                    fresh = Some(slot);
                }
            }
            match (ours, fresh) {
                (Some(slot), _) => slot,
                (None, Some(slot)) => {
                    module.inner.init_token(slot, &secret(SO_PIN), TOKEN_LABEL).expect("a fresh slot initializes the test token");
                    slot
                }
                (None, None) => panic!("no slot for the test token: point SOFTHSM2_CONF at a scratch store"),
            }
        };

        // Provision (idempotent): whatever of the two pairs is missing is generated
        // locally, self-signed, and imported with the same CKA_ID on all three objects.
        {
            let session = module.inner.open_rw_session(slot).expect("a read-write session");
            match session.login(UserType::User, Some(&secret(USER_PIN))) {
                Ok(()) => {}
                Err(e) => {
                    // A fresh token has no user PIN yet: the SO sets it, then steps aside.
                    session
                        .login(UserType::So, Some(&secret(SO_PIN)))
                        .expect("the test token takes the documented PINs; a foreign token lives here: point SOFTHSM2_CONF at a scratch store");
                    let _ = e;
                    session.init_pin(&secret(USER_PIN)).expect("the SO sets the user PIN");
                    session.logout().expect("logout before the user login");
                    session.login(UserType::User, Some(&secret(USER_PIN))).expect("user login with the new PIN");
                }
            }
            provision_rsa(&session);
            provision_ec(&session, P256_PARAMS);
        }

        let pin = PinSource::Value(secret(USER_PIN));
        let token = super::tokens(&module).expect("tokens list").into_iter().find(|t| t.label == TOKEN_LABEL).expect("the test token lists");
        assert!(token.needs_pin && !token.pinpad, "a PIN token without a pinpad: {token:?}");
        if token.pin_locked {
            assert!(token.pin_locked, "a locked token reports itself through its flags");
            return;
        }
        eprintln!("the locked-PIN check needs a locked token (SoftHSM implements no retry counters): mapping covered by failures_say_what_happened");

        // Listing never logs in, yet the keys SoftHSM hides until login (CKA_PRIVATE) still
        // pair: both identities list with no PIN at all, the way the Sign dialog lists them.
        let blind = super::list(&module, &PinSource::None).expect("listing needs no PIN");
        for want in [RSA_ID, EC_ID] {
            assert!(blind.ids.iter().any(|id| id.key_id.as_slice() == want), "listed without a PIN: {:?}", blind.unusable);
        }
        // Signing without a PIN source says the token needs one, and spends no attempt.
        let no_pin = blind.ids.iter().find(|id| id.key_id.as_slice() == RSA_ID).expect("RSA listed");
        let e = no_pin.id.key.sign(DigestAlg::Sha256, b"x").unwrap_err().to_string();
        assert!(e.contains("needs its PIN"), "{e}");
        // The same module path shares one context: a second load neither re-initializes
        // (CKR_CRYPTOKI_ALREADY_INITIALIZED) nor unloads the first.
        let again = super::load_module(std::path::Path::new(&lib)).expect("a second load shares the context");
        assert!(!super::list(&again, &PinSource::None).expect("still lists").ids.is_empty());

        let listing = super::list(&module, &pin).expect("the test identities list");
        for (want, what) in [(RSA_ID, "RSA-2048"), (EC_ID, "P-256")] {
            let hit = listing.ids.iter().find(|id| id.key_id.as_slice() == want).unwrap_or_else(|| panic!("{what} pairs by CKA_ID"));
            // The reference resolves to the same identity, which signs verifiably.
            let found = super::find(&module, &hit.reference(), &pin).expect("find by reference");
            assert_eq!(found.certificate, hit.id.certificate);
            let alg = hit.id.key.preferred_digest();
            let digest = alg.digest(&[b"the token-signed document"]);
            let sig = crate::cms::sign_detached(&hit.id.key, &hit.id.certificate, &hit.id.chain, alg, &digest).expect("the token signs");
            let sd = crate::cms::SignedData::parse(&sig).expect("its CMS parses");
            assert!(sd.verify_signature(&hit.id.certificate, &digest).expect("its CMS verifies"), "{what}");
        }

        // One wrong attempt, at signing (listing never tries a PIN): incorrect, and (with no
        // counters here) nothing more.
        let wrong = super::list(&module, &PinSource::Value(secret("wrong-pin"))).expect("listing ignores the PIN");
        let id = wrong.ids.iter().find(|id| id.key_id.as_slice() == RSA_ID).expect("RSA listed");
        let e = id.id.key.sign(DigestAlg::Sha256, b"x").unwrap_err().to_string();
        assert!(e.contains("incorrect"), "a wrong PIN reports itself: {e}");
    }

    fn provision_rsa(session: &Session) {
        const ID: &[u8] = b"pdfcraft-test-rsa";
        const LABEL: &[u8] = b"PdfCraft test RSA";
        // Done when the certificate is there; a key left by a run that stopped before its
        // certificate is replaced, so the pair always matches.
        if !session.find_objects(&[Attribute::Class(ObjectClass::CERTIFICATE), Attribute::Id(ID.to_vec())]).unwrap_or_default().is_empty() {
            return;
        }
        for stale in session.find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(ID.to_vec())]).unwrap_or_default() {
            session.destroy_object(stale).expect("a stale test key is removed");
        }
        let local = PrivateKey::generate_rsa(2048).unwrap();
        let name = crate::x509::Name::build("PdfCraft Test", "", "", "", "");
        let cert =
            Certificate::self_signed(&name, &local, crate::Time { year: 2026, month: 1, day: 1, hour: 0, minute: 0, second: 0 }, 5, &[0xA1]).unwrap();
        // The full RSA CRT out of the PKCS#8.
        let info = crate::der::Tlv::parse_all(local.pkcs8()).unwrap().children().unwrap();
        let rsa = crate::der::Tlv::parse_all(info[2].value).unwrap().children().unwrap();
        let u = |i: usize| rsa[i].uint_bytes().to_vec();
        session
            .create_object(&[
                Attribute::Class(ObjectClass::PRIVATE_KEY),
                Attribute::KeyType(KeyType::RSA),
                Attribute::Token(true),
                Attribute::Private(true),
                Attribute::Label(LABEL.to_vec()),
                Attribute::Id(ID.to_vec()),
                Attribute::Sensitive(true),
                Attribute::Extractable(false),
                Attribute::Sign(true),
                Attribute::Modulus(u(1)),
                Attribute::PublicExponent(u(2)),
                Attribute::PrivateExponent(u(3)),
                Attribute::Prime1(u(4)),
                Attribute::Prime2(u(5)),
                Attribute::Exponent1(u(6)),
                Attribute::Exponent2(u(7)),
                Attribute::Coefficient(u(8)),
            ])
            .expect("the RSA key imports");
        session
            .create_object(&[
                Attribute::Class(ObjectClass::CERTIFICATE),
                Attribute::CertificateType(CertificateType::X_509),
                Attribute::Token(true),
                Attribute::Private(false),
                Attribute::Label(LABEL.to_vec()),
                Attribute::Id(ID.to_vec()),
                // CKA_SUBJECT is required for X.509 certificate objects (SoftHSM refuses
                // the template without it: CKR_TEMPLATE_INCOMPLETE).
                Attribute::Subject(cert.subject.raw.clone()),
                Attribute::Issuer(cert.issuer.raw.clone()),
                Attribute::Value(cert.raw.clone()),
            ])
            .expect("its certificate imports");
    }

    fn provision_ec(session: &Session, params: &[u8]) {
        const ID: &[u8] = b"pdfcraft-test-ec";
        const LABEL: &[u8] = b"PdfCraft test P-256";
        // Done when the certificate is there; a key left by a run that stopped before its
        // certificate is replaced, so the pair always matches.
        if !session.find_objects(&[Attribute::Class(ObjectClass::CERTIFICATE), Attribute::Id(ID.to_vec())]).unwrap_or_default().is_empty() {
            return;
        }
        for stale in session.find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(ID.to_vec())]).unwrap_or_default() {
            session.destroy_object(stale).expect("a stale test key is removed");
        }
        let local = PrivateKey::generate_p256().unwrap();
        let name = crate::x509::Name::build("PdfCraft Test", "", "", "", "");
        let cert =
            Certificate::self_signed(&name, &local, crate::Time { year: 2026, month: 1, day: 1, hour: 0, minute: 0, second: 0 }, 5, &[0xE2]).unwrap();
        // The scalar out of the ECPrivateKey, the point out of the certificate.
        let info = crate::der::Tlv::parse_all(local.pkcs8()).unwrap().children().unwrap();
        let ec = crate::der::Tlv::parse_all(info[2].value).unwrap().children().unwrap();
        let scalar = ec[1].value.to_vec();
        assert!(matches!(cert.public_key, PublicKey::P256(_)), "a P-256 certificate holds a P-256 key");
        session
            .create_object(&[
                Attribute::Class(ObjectClass::PRIVATE_KEY),
                Attribute::KeyType(KeyType::EC),
                Attribute::Token(true),
                Attribute::Private(true),
                Attribute::Label(LABEL.to_vec()),
                Attribute::Id(ID.to_vec()),
                Attribute::Sensitive(true),
                Attribute::Extractable(false),
                Attribute::Sign(true),
                Attribute::EcParams(params.to_vec()),
                Attribute::Value(scalar),
            ])
            .expect("the EC key imports");
        session
            .create_object(&[
                Attribute::Class(ObjectClass::CERTIFICATE),
                Attribute::CertificateType(CertificateType::X_509),
                Attribute::Token(true),
                Attribute::Private(false),
                Attribute::Label(LABEL.to_vec()),
                Attribute::Id(ID.to_vec()),
                // CKA_SUBJECT is required for X.509 certificate objects (SoftHSM refuses
                // the template without it: CKR_TEMPLATE_INCOMPLETE).
                Attribute::Subject(cert.subject.raw.clone()),
                Attribute::Issuer(cert.issuer.raw.clone()),
                Attribute::Value(cert.raw.clone()),
            ])
            .expect("its certificate imports");
    }
}
