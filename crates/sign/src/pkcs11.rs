//! Digital IDs on PKCS #11 tokens: smart cards and USB tokens, through the PKCS #11 module
//! (a shared library) their vendor's driver installs, the same public interface Acrobat,
//! LibreOffice and Firefox load. The private key never leaves the token: PdfCraft hands it the
//! data to sign and reads back the signature.
//!
//! - [`identities`] lists the certificates on every token a module sees without logging in, so
//!   listing never asks for a PIN.
//! - [`open`] logs in to the token holding one of them and returns a [`DigitalId`] whose key
//!   signs on the token. The session (and the login) lasts as long as the key.
//!
//! The PIN is a [`Pin`] (a `secrecy` string: zeroized when dropped, redacted in `Debug`). It is
//! passed to the token once, at login, and never stored, logged or put in an error message.
//! Tokens with a protected authentication path (a PIN pad on the reader) take `None` and ask
//! for the PIN themselves.
//!
//! Native targets only: a browser can't load a shared library.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error as CkError, RvError};
use cryptoki::mechanism::{Mechanism, MechanismType};
use cryptoki::object::{Attribute, AttributeType, CertificateType, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;

use crate::keys::{DigestAlg, ExternalKey, PrivateKey, PublicKey};
use crate::{Certificate, DigitalId, SignError};

/// A token PIN: zeroized when dropped and never printed.
pub use cryptoki::types::AuthPin as Pin;

/// At most this many objects are read from one token (a token is untrusted input too).
const MAX_OBJECTS: usize = 256;

/// The token a certificate was found on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenInfo {
    pub label: String,
    pub manufacturer: String,
    pub model: String,
    pub serial: String,
    /// The reader has a PIN pad: log in with `None` and the reader asks for the PIN.
    pub protected_authentication_path: bool,
    /// Too many wrong PINs: the token refuses to log in until it is unblocked.
    pub pin_locked: bool,
    /// The next wrong PIN locks the token.
    pub pin_final_try: bool,
}

/// A certificate on a token, with what is needed to find its private key after logging in.
#[derive(Clone, Debug)]
pub struct TokenCertificate {
    pub token: TokenInfo,
    pub certificate: Certificate,
    /// The object's `CKA_LABEL` (cards name their keys, e.g. a signing and an authentication key).
    pub label: String,
    /// `CKA_ID`, shared by a certificate and its private key.
    pub id: Vec<u8>,
    /// Whether this is an identity to sign with rather than one of the card's CA certificates:
    /// it issued no other certificate on the token, and a keyUsage, when present, allows
    /// nonRepudiation ("content commitment", what signing cards set on the signing
    /// certificate) or digitalSignature. A self-signed personal certificate counts.
    pub for_signing: bool,
}

/// Mark which certificates on one token are identities (see [`TokenCertificate::for_signing`]).
/// Cards that carry an authentication and a signing certificate (common on national eID cards)
/// mark only the signing one nonRepudiation; when one does, a
/// certificate whose keyUsage leaves nonRepudiation out is the other kind and not offered.
fn mark_signers(certs: &mut [TokenCertificate]) {
    const DIGITAL_SIGNATURE: u16 = 0b01;
    const NON_REPUDIATION: u16 = 0b10;
    // A self-signed certificate was issued by no other certificate, whatever its name.
    let issued_another = |c: &Certificate| {
        certs
            .iter()
            .any(|o| o.certificate.raw != c.raw && o.certificate.issuer.raw != o.certificate.subject.raw && o.certificate.issuer.raw == c.subject.raw)
    };
    let has_non_repudiation = |c: &Certificate| c.key_usage.is_some_and(|u| u & NON_REPUDIATION != 0);
    let any_non_repudiation = certs.iter().any(|t| !issued_another(&t.certificate) && has_non_repudiation(&t.certificate));
    let flags: Vec<bool> = certs
        .iter()
        .map(|t| {
            let c = &t.certificate;
            !issued_another(c)
                && match c.key_usage {
                    None => true,
                    Some(_) if any_non_repudiation => has_non_repudiation(c),
                    Some(u) => u & (DIGITAL_SIGNATURE | NON_REPUDIATION) != 0,
                }
        })
        .collect();
    for (t, f) in certs.iter_mut().zip(flags) {
        t.for_signing = f;
    }
}

/// The `pkcs11:<SHA-256 of the certificate>` reference PdfCraft keeps for a token identity.
pub fn reference(c: &Certificate) -> String {
    let d = DigestAlg::Sha256.digest(&[&c.raw]);
    format!("pkcs11:{}", d.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn ck(what: &str) -> impl Fn(CkError) -> SignError + '_ {
    move |e| SignError::Crypto(format!("{what}: {}", describe(&e)))
}

/// A PKCS #11 error in words, without anything the module put in it beyond its return code.
fn describe(e: &CkError) -> String {
    match e {
        CkError::Pkcs11(RvError::PinIncorrect, _) => "the PIN is incorrect".into(),
        CkError::Pkcs11(RvError::PinLocked, _) => "the token is locked after too many wrong PINs".into(),
        CkError::Pkcs11(RvError::PinExpired, _) => "the PIN has expired; change it with the card's own tools".into(),
        CkError::Pkcs11(RvError::FunctionCanceled, _) => "cancelled".into(),
        CkError::Pkcs11(RvError::TokenNotPresent, _) | CkError::Pkcs11(RvError::DeviceRemoved, _) => "the card was removed".into(),
        CkError::Pkcs11(rv, _) => format!("the token refused ({rv})"),
        CkError::LibraryLoading(_) => "the PKCS #11 module couldn't be loaded".into(),
        other => other.to_string(),
    }
}

/// Modules stay loaded and initialized for the life of the process: `C_Initialize` may run
/// once per module, and unloading one while a key from it is alive would pull the code out
/// from under it.
fn module(path: &Path) -> Result<Pkcs11, SignError> {
    static MODULES: OnceLock<Mutex<HashMap<PathBuf, Pkcs11>>> = OnceLock::new();
    let mut modules = MODULES
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| SignError::Crypto("the PKCS #11 module list is unusable after an earlier failure".into()))?;
    if let Some(m) = modules.get(path) {
        return Ok(m.clone());
    }
    if !path.is_file() {
        return Err(SignError::Crypto(format!("{}: no PKCS #11 module there", path.display())));
    }
    let m = Pkcs11::new(path).map_err(|e| SignError::Crypto(format!("{}: {}", path.display(), describe(&e))))?;
    match m.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
        Ok(()) | Err(CkError::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => {}
        Err(e) => return Err(SignError::Crypto(format!("{}: {}", path.display(), describe(&e)))),
    }
    modules.insert(path.to_path_buf(), m.clone());
    Ok(m)
}

fn token_info(m: &Pkcs11, slot: Slot) -> Result<TokenInfo, SignError> {
    let t = m.get_token_info(slot).map_err(ck("token information"))?;
    Ok(TokenInfo {
        label: t.label().trim().to_string(),
        manufacturer: t.manufacturer_id().trim().to_string(),
        model: t.model().trim().to_string(),
        serial: t.serial_number().trim().to_string(),
        protected_authentication_path: t.protected_authentication_path(),
        pin_locked: t.user_pin_locked(),
        pin_final_try: t.user_pin_final_try(),
    })
}

/// The X.509 certificates on the token in `slot`, with their labels and IDs.
fn certificates(session: &Session, token: &TokenInfo) -> Result<Vec<TokenCertificate>, SignError> {
    let found = session
        .find_objects(&[Attribute::Class(ObjectClass::CERTIFICATE), Attribute::CertificateType(CertificateType::X_509)])
        .map_err(ck("the token's certificates couldn't be listed"))?;
    let mut out = Vec::new();
    for h in found.into_iter().take(MAX_OBJECTS) {
        let Ok(attrs) = session.get_attributes(h, &[AttributeType::Value, AttributeType::Id, AttributeType::Label]) else { continue };
        let (mut value, mut id, mut label) = (None, Vec::new(), String::new());
        for a in attrs {
            match a {
                Attribute::Value(v) => value = Some(v),
                Attribute::Id(v) => id = v,
                Attribute::Label(v) => label = String::from_utf8_lossy(&v).trim().to_string(),
                _ => {}
            }
        }
        let Some(Ok(certificate)) = value.map(|v| Certificate::parse(&v)) else { continue };
        out.push(TokenCertificate { token: token.clone(), certificate, label, id, for_signing: false });
    }
    mark_signers(&mut out);
    Ok(out)
}

/// Every certificate on every token the module at `module_path` sees (CA certificates included;
/// see [`TokenCertificate::for_signing`]). Doesn't log in, so it never asks for a PIN. A token
/// that can't be read is skipped rather than failing the whole list.
pub fn identities(module_path: &Path) -> Result<Vec<TokenCertificate>, SignError> {
    let m = module(module_path)?;
    let mut out = Vec::new();
    for slot in m.get_slots_with_token().map_err(ck("the card readers couldn't be listed"))? {
        let Ok(token) = token_info(&m, slot) else { continue };
        let Ok(session) = m.open_ro_session(slot) else { continue };
        out.extend(certificates(&session, &token).unwrap_or_default());
    }
    Ok(out)
}

/// The signing-related mechanisms (RSA, ECDSA, hashes) the cards in the module's readers
/// support, by their PKCS #11 names. For diagnostics: what a card can do decides how PdfCraft
/// asks it to sign.
pub fn mechanism_names(module_path: &Path) -> Result<Vec<String>, SignError> {
    let m = module(module_path)?;
    let mut names = Vec::new();
    for slot in m.get_slots_with_token().map_err(ck("the card readers couldn't be listed"))? {
        for mech in m.get_mechanism_list(slot).unwrap_or_default().into_iter().take(MAX_OBJECTS) {
            let name = mech.to_string();
            if (name.contains("RSA") || name.contains("ECDSA") || name.contains("SHA")) && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    Ok(names)
}

/// Log in to the token holding the certificate `reference_or_name` names (its
/// [`reference`], or its subject common name) and return a digital ID that signs there.
/// The certificates on the same token that issued it (the card's CA chain) come along as
/// `chain`. Every call checks the PIN, even while a key from an earlier call is still logged in.
///
/// `pin` is `None` only for a reader with a PIN pad. A wrong PIN returns an error saying so;
/// the token counts it. A token on its final try ([`TokenInfo::pin_final_try`]) is refused
/// before logging in.
pub fn open(module_path: &Path, reference_or_name: &str, pin: Option<Pin>) -> Result<DigitalId, SignError> {
    let m = module(module_path)?;
    let wanted =
        |c: &Certificate| reference(c) == reference_or_name || c.subject.common_name() == Some(reference_or_name.trim_start_matches("pkcs11:"));
    for slot in m.get_slots_with_token().map_err(ck("the card readers couldn't be listed"))? {
        let Ok(token) = token_info(&m, slot) else { continue };
        let Ok(session) = m.open_ro_session(slot) else { continue };
        let Ok(certs) = certificates(&session, &token) else { continue };
        // A name can match an authentication and a signing certificate on the same card.
        let Some(found) = certs.iter().filter(|c| wanted(&c.certificate)).max_by_key(|c| c.for_signing).cloned() else { continue };
        if token.pin_locked {
            return Err(SignError::Crypto(format!("{}: the token is locked after too many wrong PINs", token.label)));
        }
        // Never spend the last try: a mistyped PIN would lock the card.
        if token.pin_final_try {
            return Err(SignError::Crypto(format!(
                "{}: one more wrong PIN would lock this card, so PdfCraft won't try; check or reset the PIN with the card vendor's tools",
                token.label
            )));
        }
        if pin.is_none() && !token.protected_authentication_path {
            return Err(SignError::Crypto(format!("{}: a PIN is needed", token.label)));
        }
        let mut login = session.login(UserType::User, pin.as_ref());
        // The login is shared by every session the app has on the token: one left by an earlier
        // key would let any PIN through. Log out and in again, so this PIN is checked too
        // (keys from earlier calls then need their own `open` again to sign).
        if let Err(CkError::Pkcs11(RvError::UserAlreadyLoggedIn, _)) = login {
            session.logout().map_err(ck("the token's earlier login couldn't be closed"))?;
            login = session.login(UserType::User, pin.as_ref());
        }
        login.map_err(|e| SignError::Crypto(format!("{}: {}", token.label, describe(&e))))?;
        drop(pin);
        let key = private_key(&session, &found)?;
        let mechanisms = m.get_mechanism_list(slot).unwrap_or_default();
        let public = found.certificate.public_key.clone();
        let mechanism = match &public {
            PublicKey::Rsa { .. } if mechanisms.contains(&MechanismType::RSA_PKCS) => KeyMechanism::RsaPkcs,
            PublicKey::Rsa { .. } => KeyMechanism::RsaHashed,
            PublicKey::P256(_) | PublicKey::P384(_) => KeyMechanism::Ecdsa,
            other => return Err(SignError::Unsupported(format!("signing with a {} key on a token", other.describe()))),
        };
        let chain = issuers(&found.certificate, &certs);
        let friendly_name = Some(found.certificate.display_name());
        let key = PrivateKey::external(public.clone(), Arc::new(TokenKey { session: Mutex::new(session), key, public, mechanism }));
        return Ok(DigitalId { key, certificate: found.certificate, chain, friendly_name });
    }
    Err(SignError::Crypto(format!("no token holds the certificate {reference_or_name} (is the card inserted?)")))
}

/// The certificates on the token that issued `leaf`, nearest first: the card's part of the
/// chain. Its other identities (an authentication certificate) and unrelated CAs stay out.
fn issuers(leaf: &Certificate, certs: &[TokenCertificate]) -> Vec<Certificate> {
    let mut chain: Vec<Certificate> = Vec::new();
    let mut current = leaf;
    // Bounded by the number of certificates, so a cycle of names ends.
    // A self-signed certificate ends the chain.
    while chain.len() < certs.len() && current.issuer.raw != current.subject.raw {
        let Some(next) = certs
            .iter()
            .map(|c| &c.certificate)
            .find(|c| c.subject.raw == current.issuer.raw && c.raw != current.raw && c.raw != leaf.raw && !chain.contains(c))
        else {
            break;
        };
        chain.push(next.clone());
        current = next;
    }
    chain
}

/// The private key with the certificate's `CKA_ID` (visible only after logging in).
fn private_key(session: &Session, cert: &TokenCertificate) -> Result<ObjectHandle, SignError> {
    let mut template = vec![Attribute::Class(ObjectClass::PRIVATE_KEY)];
    if !cert.id.is_empty() {
        template.push(Attribute::Id(cert.id.clone()));
    }
    let keys = session.find_objects(&template).map_err(ck("the token's keys couldn't be listed"))?;
    match keys.as_slice() {
        [k] => Ok(*k),
        [] => Err(SignError::Crypto(format!(
            "{}: the certificate {} has no private key on the token",
            cert.token.label,
            cert.certificate.display_name()
        ))),
        _ => Err(SignError::Crypto(format!(
            "{}: more than one private key matches the certificate {}",
            cert.token.label,
            cert.certificate.display_name()
        ))),
    }
}

#[derive(Clone, Copy, Debug)]
enum KeyMechanism {
    /// `CKM_RSA_PKCS` over a `DigestInfo` built here: what every RSA card supports.
    RsaPkcs,
    /// `CKM_SHA*_RSA_PKCS`: the token hashes the message itself.
    RsaHashed,
    /// `CKM_ECDSA` over the digest; the token returns raw `r ‖ s`.
    Ecdsa,
}

struct TokenKey {
    /// A session is used from one thread at a time.
    session: Mutex<Session>,
    key: ObjectHandle,
    public: PublicKey,
    mechanism: KeyMechanism,
}

impl ExternalKey for TokenKey {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let session = self.session.lock().map_err(|_| SignError::Crypto("the token session is unusable after an earlier failure".into()))?;
        let signing = ck("the token didn't sign");
        match self.mechanism {
            KeyMechanism::RsaPkcs => {
                let info = crate::rsa_pad::digest_info(alg, &alg.digest(&[msg]), true);
                session.sign(&Mechanism::RsaPkcs, self.key, &info).map_err(signing)
            }
            KeyMechanism::RsaHashed => {
                let mech = match alg {
                    DigestAlg::Sha384 => Mechanism::Sha384RsaPkcs,
                    DigestAlg::Sha512 => Mechanism::Sha512RsaPkcs,
                    DigestAlg::Sha256 => Mechanism::Sha256RsaPkcs,
                    other => return Err(SignError::Unsupported(format!("{} on a token", other.name()))),
                };
                session.sign(&mech, self.key, msg).map_err(signing)
            }
            KeyMechanism::Ecdsa => {
                let raw = session.sign(&Mechanism::Ecdsa, self.key, &alg.digest(&[msg])).map_err(signing)?;
                match &self.public {
                    PublicKey::P256(_) => p256::ecdsa::Signature::from_slice(&raw)
                        .map(|s| s.to_der().as_bytes().to_vec())
                        .map_err(|e| SignError::Crypto(format!("the token returned an invalid P-256 signature: {e}"))),
                    PublicKey::P384(_) => p384::ecdsa::Signature::from_slice(&raw)
                        .map(|s| s.to_der().as_bytes().to_vec())
                        .map_err(|e| SignError::Crypto(format!("the token returned an invalid P-384 signature: {e}"))),
                    _ => Err(SignError::Unsupported("this key type on a token".into())),
                }
            }
        }
    }
}

/// Where vendors' PKCS #11 modules usually are on this system, for the ones that exist. These
/// are hints for a settings field, not a guarantee: the user picks the module their card's
/// driver installed. Paths come from vendor documentation; only the first Windows one was
/// observed on a real machine.
pub fn candidate_modules() -> Vec<PathBuf> {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["/Library/Application Support/Athena/libASEP11.dylib", "/usr/local/lib/libIDPrimePKCS11.dylib", "/Library/OpenSC/lib/opensc-pkcs11.so"]
    } else if cfg!(target_os = "windows") {
        // PdfCraft is 64-bit, so it needs the 64-bit module: System32 or Program Files on 64-bit
        // Windows (SysWOW64 and Program Files (x86) hold the 32-bit copies). NXP JCOP 3 eID
        // cards ("iGOV") come with "Smart Card Middleware", whose module is idoPKCS.dll (observed
        // on a Windows 11 machine, 2026-10). Older Athena
        // IDProtect cards use asepkcs.dll; Thales IDPrime middleware installs IDPrimePKCS1164.dll
        // under Program Files (SafeNet Authentication Client 10.8 R2 and later) or the older
        // IDGo 800 folder.
        &[
            "C:\\Program Files\\Smart Card Middleware\\bin\\idoPKCS.dll",
            "C:\\Windows\\System32\\asepkcs.dll",
            "C:\\Program Files\\SafeNet\\Authentication\\SAC\\x64\\IDPrimePKCS1164.dll",
            "C:\\Program Files (x86)\\Gemalto\\IDGo 800 PKCS#11\\IDPrimePKCS1164.dll",
            "C:\\Program Files\\OpenSC Project\\OpenSC\\pkcs11\\opensc-pkcs11.dll",
        ]
    } else {
        &[
            "/usr/lib/x64-athena/libASEP11.so",
            "/usr/lib/libASEP11.so",
            "/usr/lib/libIDPrimePKCS11.so",
            "/usr/lib/x86_64-linux-gnu/opensc-pkcs11.so",
            "/usr/lib/opensc-pkcs11.so",
            "/usr/local/lib/opensc-pkcs11.so",
        ]
    };
    candidates.iter().map(PathBuf::from).filter(|p| p.is_file()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_cert(file: &str, key_usage: Option<u16>) -> TokenCertificate {
        let pem = std::fs::read(format!("{}/tests/data/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let mut certificate = crate::x509::load_certificates(&pem).unwrap().remove(0);
        certificate.key_usage = key_usage;
        let token = TokenInfo {
            label: "card".into(),
            manufacturer: String::new(),
            model: String::new(),
            serial: String::new(),
            protected_authentication_path: false,
            pin_locked: false,
            pin_final_try: false,
        };
        TokenCertificate { token, certificate, label: String::new(), id: Vec::new(), for_signing: false }
    }

    #[test]
    fn a_card_offers_its_signing_certificate_not_its_authentication_one() {
        // keyUsage bits: 0 digitalSignature, 1 nonRepudiation, 2 keyEncipherment.
        let mut certs = vec![token_cert("rsa.crt.pem", Some(0b101)), token_cert("rsa.crt.pem", Some(0b10))];
        certs[1].certificate.raw.push(0); // two distinct certificates
        mark_signers(&mut certs);
        assert_eq!(certs.iter().map(|c| c.for_signing).collect::<Vec<_>>(), [false, true]);

        // Without a nonRepudiation certificate on the token, digitalSignature is enough.
        let mut certs = vec![token_cert("rsa.crt.pem", Some(0b101))];
        mark_signers(&mut certs);
        assert!(certs[0].for_signing);
        // A keyUsage that allows neither (encipherment only) never signs; none at all does.
        let mut certs = vec![token_cert("rsa.crt.pem", Some(0b100))];
        mark_signers(&mut certs);
        assert!(!certs[0].for_signing);
        let mut certs = vec![token_cert("rsa.crt.pem", None)];
        mark_signers(&mut certs);
        assert!(certs[0].for_signing);
    }

    #[test]
    fn smart_card_middleware_is_looked_for_first_on_windows() {
        let src = include_str!("pkcs11.rs");
        let windows = src.find("Smart Card Middleware\\\\bin\\\\idoPKCS.dll").unwrap();
        assert!(windows < src.find("System32\\\\asepkcs.dll").unwrap());
    }
}
