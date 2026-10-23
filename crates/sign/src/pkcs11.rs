//! Digital IDs on PKCS #11 devices: smart cards, USB tokens and cloud certificates whose vendor
//! ships a PKCS #11 module (Certisign desktopID, Soluti birdID, Serpro NeoID). The certificates the
//! module exposes are listed, and signing goes through `C_Sign`, so the private key never leaves
//! the device.
//!
//! Modules are found by looking for the libraries token vendors install in known places (the list
//! Brazilian court signers such as PJeOffice use), plus any library the user adds. Devices with a
//! protected authentication path (a PIN pad, or a cloud certificate that asks for approval on the
//! phone) log in with no PIN; the others take the PIN. A login is kept for the rest of the run, so
//! a device asks once.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use cryptoki::context::{CInitializeArgs, Pkcs11};
use cryptoki::error::{Error as CkError, RvError};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, ObjectClass};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;

use crate::keys::{DigestAlg, ExternalKey, PrivateKey, PublicKey};
use crate::{Certificate, DigitalId, SignError};

/// Library names token vendors install in the usual library folders on macOS.
const MACOS_NAMES: &[&str] = &[
    "libbit4xpki.dylib",
    "libbit4ipki.dylib",
    "libbit4opki.dylib",
    "libASEP11.dylib",
    "libOcsCryptoki.dylib",
    "libgclib.dylib",
    "libwdpkcs.dylib",
    "libsfntpkcs11.dylib",
    "libeTPkcs11.dylib",
    "libeTpkcs11.dylib",
    "libetpkcs11.dylib",
    "libaetpkss.dylib",
    "libdesktopID_Provider.dylib",
    "libdesktopID_Provider_ARM.dylib",
    "opensc-pkcs11.so",
    "libcastle.1.0.0.dylib",
    "libykcs11.dylib",
];

/// Libraries macOS token software installs elsewhere (inside its app or framework).
const MACOS_PATHS: &[&str] = &[
    "/Library/Frameworks/eToken.framework/Versions/Current/libeToken.dylib",
    "/Library/OpenSC/lib/opensc-pkcs11.so",
    "/Library/Application Support/CSSI/libcmP11.dylib",
    "/usr/local/AWP/lib/libOcsCryptok.dylib",
    "/opt/homebrew/lib/libykcs11.dylib",
    "/opt/homebrew/lib/opensc-pkcs11.so",
    "/Applications/tokenadmin.app/Contents/Frameworks/libaetpkss.dylib",
    "/Applications/WatchKey USB Token Admin Tool.app/Contents/MacOS/lib/libWDP11_BR_GOV.dylib",
    "/Applications/NeoID Desktop.app/Contents/Java/tools/macos/libneoidp11.dylib",
    "/Applications/Assistente Desktop birdID.app/Contents/Resources/extraResources/osx/x64/vault-pkcs11.dylib",
    "/Applications/Assistente Desktop birdID.app/Contents/Resources/extraResources/osx/arm/vault-pkcs11-arm.dylib",
];

/// Library names token vendors install in the usual library folders on Linux and FreeBSD.
const UNIX_NAMES: &[&str] = &[
    "libcoolkeypk11.so",
    "libbit4opki.so",
    "libbit4ipki.so",
    "libbit4xpki.so",
    "libASEP11.so",
    "stPKCS11.so",
    "libsiecap11.so",
    "libaetpkss.so",
    "libaetpkss.so.3",
    "libgpkcs11.so",
    "libepsng_p11.so",
    "libneoidp11.so",
    "libeTPkcs11.so",
    "libeToken.so",
    "libcmP11.so",
    "libwdpkcs.so",
    "libscmccid.so",
    "opensc-pkcs11.so",
    "opensc/opensc-pkcs11.so",
    "x86_64-linux-gnu/opensc-pkcs11.so",
    "x86_64-linux-gnu/pkcs11/opensc-pkcs11.so",
    "aarch64-linux-gnu/opensc-pkcs11.so",
    "libykcs11.so",
    "watchdata/ICP/lib/libwdpkcs_icp.so",
    "watchdata/lib/libwdpkcs.so",
];

/// Libraries Linux token software installs elsewhere.
const UNIX_PATHS: &[&str] = &[
    "/usr/local/AWP/lib/libOcsCryptoki.so",
    "/usr/local/ngsrv/libepsng_p11.so.1",
    "/usr/lib64/libeToken.so",
    "/usr/lib64/libeTPkcs11.so",
    "/usr/lib64/libaetpkss.so",
    "/usr/local/lib64/libwdpkcs.so",
    "/opt/watchdata/lib64/libwdpkcs.so",
    "/opt/Assistente Desktop birdID/resources/extraResources/linux/x64/vault-pkcs11.so",
];

/// Library folders the `*_NAMES` are looked for in (Unix-like systems).
const UNIX_DIRS: &[&str] = &["/usr/lib", "/usr/local/lib", "/usr/local/lib/pkcs11", "/usr/lib/pkcs11"];

/// Library names token vendors install in `System32` on Windows (64-bit libraries only: a 64-bit
/// PdfCraft can't load the 32-bit copies in `SysWOW64` or `Program Files (x86)`).
const WINDOWS_SYSTEM32: &[&str] = &[
    "incryptoki2.dll",
    "bit4ipki.dll",
    "bit4opki.dll",
    "bit4xpki.dll",
    "SI_PKCS11.dll",
    "IpmPki32.dll",
    "bit4cpki.dll",
    "bit4tpki.dll",
    "CardOS_PKCS11.dll",
    "bit4p11.dll",
    "SSC_PKCS11.dll",
    "inp11lib.dll",
    "cvP11_M4.dll",
    "stPKCS11.dll",
    "SissP11.dll",
    "aepkcs11.dll",
    "eTokenPKCS11.dll",
    "asignp11.dll",
    "EP1PK111.dll",
    "nxpkcs11.dll",
    "micardoPKCS11.dll",
    "p11card.dll",
    "acpkcs211.dll",
    "ep2pk11.dll",
    "ngp11v211.dll",
    "aetpkss1.dll",
    "gclib.dll",
    "eTPkcs11.dll",
    "acospkcs11.dll",
    "dkck201.dll",
    "cryptoki22.dll",
    "slbck.dll",
    "cmP11.dll",
    "WDPKCS.dll",
    "SerproPkcs11.dll",
    "OcsCryptoki.dll",
    "opensc-pkcs11.dll",
    "axaltocm.dll",
    "cs2_pkcs11.dll",
    "dspkcs.dll",
    "psepkcs11.dll",
    "smartp11.dll",
    "AuCryptoki2-0.dll",
    "cknfast.dll",
    "iveacryptoki.dll",
    "siecap11.dll",
    "asepkcs.dll",
    "WDICP_P11_CCID_v34.dll",
    "DXSafePKCS11.dll",
    "vault-pkcs11.dll",
    "eps2003csp11.dll",
    "libykcs11.dll",
];

/// Libraries Windows token software installs under `Program Files`.
const WINDOWS_PROGRAM_FILES: &[&str] = &[
    "Gemalto\\IDGo 800 PKCS11\\IDPrimePKCS1164.dll",
    "Gemplus\\GemSafe Libraries\\BIN\\gclib.dll",
    "Oberthur Technologies\\AuthentIC Webpack\\DLLs\\OCSCryptoki.dll",
    "Oberthur Technologies\\AWP\\DLLs\\OcsCryptoki.dll",
    "CSP Banrisul Multiplo\\PKCS11.dll",
    "Assistente Desktop birdID\\resources\\extraResources\\windows\\x64\\vault-pkcs11.dll",
    "OpenSC Project\\OpenSC\\pkcs11\\opensc-pkcs11.dll",
    "Yubico\\Yubico PIV Tool\\bin\\libykcs11.dll",
];

/// Where this system's token vendors install their PKCS #11 libraries. Not filtered: most of
/// these don't exist on a given machine.
pub fn known_locations() -> Vec<PathBuf> {
    if cfg!(target_os = "windows") {
        let windir = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from("C:\\Windows"), PathBuf::from);
        let system32 = windir.join("System32");
        let program_files = std::env::var_os("ProgramFiles").map_or_else(|| PathBuf::from("C:\\Program Files"), PathBuf::from);
        WINDOWS_SYSTEM32.iter().map(|n| system32.join(n)).chain(WINDOWS_PROGRAM_FILES.iter().map(|p| program_files.join(p))).collect()
    } else {
        let (names, paths) = if cfg!(target_os = "macos") { (MACOS_NAMES, MACOS_PATHS) } else { (UNIX_NAMES, UNIX_PATHS) };
        UNIX_DIRS.iter().flat_map(|dir| names.iter().map(move |n| Path::new(dir).join(n))).chain(paths.iter().map(PathBuf::from)).collect()
    }
}

/// The modules to read: `PDFCRAFT_PKCS11_MODULES` (a path list, as in `PATH`) when set, else the
/// [`known_locations`] and the libraries the user added. Existing files only, each once (a library
/// installed under two names, or linked from a folder, is loaded once).
pub fn modules(added: &[PathBuf]) -> Vec<PathBuf> {
    let listed: Vec<PathBuf> = match std::env::var_os("PDFCRAFT_PKCS11_MODULES") {
        Some(v) => std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect(),
        None => known_locations().into_iter().chain(added.iter().cloned()).collect(),
    };
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for path in listed {
        if !path.is_file() {
            continue;
        }
        let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.contains(&real) {
            seen.push(real);
            out.push(path);
        }
    }
    out
}

/// Each module is loaded and initialised once per process and never finalised: its sessions and
/// login live as long as PdfCraft.
fn context(module: &Path) -> Result<Pkcs11, SignError> {
    static LOADED: OnceLock<Mutex<HashMap<PathBuf, Pkcs11>>> = OnceLock::new();
    let mut loaded = LOADED.get_or_init(Default::default).lock().map_err(|_| SignError::Crypto("PKCS #11 lock poisoned".into()))?;
    if let Some(p) = loaded.get(module) {
        return Ok(p.clone());
    }
    let p = Pkcs11::new(module).map_err(|e| SignError::Crypto(format!("{}: {e}", module.display())))?;
    match p.initialize(CInitializeArgs::OsThreads) {
        Ok(()) | Err(CkError::AlreadyInitialized) | Err(CkError::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => {}
        Err(e) => return Err(SignError::Crypto(format!("{}: {e}", module.display()))),
    }
    loaded.insert(module.to_path_buf(), p.clone());
    Ok(p)
}

/// Where a device certificate's key lives.
#[derive(Clone)]
struct Location {
    module: PathBuf,
    slot: Slot,
    /// `CKA_ID` shared by the certificate and its private key.
    id: Vec<u8>,
    /// The device authenticates by itself (no PIN from PdfCraft).
    protected: bool,
    token: String,
}

type Sessions = HashMap<(PathBuf, u64), Arc<Mutex<Session>>>;

/// The logged-in session of each device, by module and slot.
fn sessions() -> &'static Mutex<Sessions> {
    static S: OnceLock<Mutex<Sessions>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// The device's session, logged in: an earlier one if there is one, or a new one (which may make
/// the device ask for approval, and take as long as that takes).
fn logged_in(loc: &Location, pin: Option<&str>) -> Result<Arc<Mutex<Session>>, SignError> {
    // One login at a time: a signature that comes while an early login waits for the phone waits
    // with it and then uses its session, instead of asking a second time.
    static LOGIN: Mutex<()> = Mutex::new(());
    let _one = LOGIN.lock().map_err(|_| SignError::Crypto("PKCS #11 lock poisoned".into()))?;
    let key = (loc.module.clone(), loc.slot.id());
    if let Some(s) = sessions().lock().map_err(|_| SignError::Crypto("PKCS #11 lock poisoned".into()))?.get(&key) {
        return Ok(s.clone());
    }
    let p = context(&loc.module)?;
    let session = p.open_ro_session(loc.slot).map_err(|e| SignError::Crypto(format!("{}: {e}", loc.token)))?;
    let pin = match (loc.protected, pin.filter(|p| !p.is_empty())) {
        (true, _) => None,
        (false, Some(p)) => Some(AuthPin::new(p.into())),
        (false, None) => return Err(SignError::WrongPassword),
    };
    match session.login(UserType::User, pin.as_ref()) {
        Ok(()) | Err(CkError::Pkcs11(RvError::UserAlreadyLoggedIn, _)) => {}
        Err(CkError::Pkcs11(RvError::PinIncorrect | RvError::PinLenRange | RvError::ArgumentsBad, _)) if !loc.protected => {
            return Err(SignError::WrongPassword);
        }
        Err(e) => return Err(SignError::Crypto(format!("{} didn't log in: {e}", loc.token))),
    }
    let session = Arc::new(Mutex::new(session));
    sessions().lock().map_err(|_| SignError::Crypto("PKCS #11 lock poisoned".into()))?.insert(key, session.clone());
    Ok(session)
}

struct TokenKey {
    loc: Location,
    pin: Option<String>,
}

impl ExternalKey for TokenKey {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let mechanism = match alg {
            DigestAlg::Sha1 => Mechanism::Sha1RsaPkcs,
            DigestAlg::Sha384 => Mechanism::Sha384RsaPkcs,
            DigestAlg::Sha512 => Mechanism::Sha512RsaPkcs,
            _ => Mechanism::Sha256RsaPkcs,
        };
        let attempt = || -> Result<Vec<u8>, SignError> {
            let session = logged_in(&self.loc, self.pin.as_deref())?;
            let session = session.lock().map_err(|_| SignError::Crypto("PKCS #11 lock poisoned".into()))?;
            let key = session
                .find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(self.loc.id.clone())])
                .map_err(|e| SignError::Crypto(format!("{}: {e}", self.loc.token)))?
                .into_iter()
                .next()
                .ok_or_else(|| SignError::Crypto(format!("{} has no private key for this certificate", self.loc.token)))?;
            session.sign(&mechanism, key, msg).map_err(|e| SignError::Crypto(format!("{} didn't sign: {e}", self.loc.token)))
        };
        attempt().or_else(|e| {
            if matches!(e, SignError::WrongPassword) {
                return Err(e);
            }
            // A session the device dropped (a cloud session that expired) is opened again, once.
            if let Ok(mut s) = sessions().lock() {
                s.remove(&(self.loc.module.clone(), self.loc.slot.id()));
            }
            attempt().map_err(|_| e)
        })
    }
}

/// The `pkcs11:<SHA-256 of the certificate>` reference kept for a device identity.
pub fn reference(c: &Certificate) -> String {
    let d = DigestAlg::Sha256.digest(&[&c.raw]);
    format!("pkcs11:{}", d.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// The certificates one module's devices hold, with where their keys are.
fn module_listing(module: &Path) -> Result<Vec<(Certificate, Location)>, SignError> {
    let p = context(module)?;
    let slots = p.get_slots_with_token().map_err(|e| SignError::Crypto(format!("{}: {e}", module.display())))?;
    let mut out = Vec::new();
    for slot in slots {
        let Ok(info) = p.get_token_info(slot) else { continue };
        let Ok(session) = p.open_ro_session(slot) else { continue };
        let Ok(certs) = session.find_objects(&[Attribute::Class(ObjectClass::CERTIFICATE)]) else { continue };
        for handle in certs {
            let Ok(attrs) = session.get_attributes(handle, &[AttributeType::Id, AttributeType::Value]) else { continue };
            let (mut id, mut der) = (None, None);
            for a in attrs {
                match a {
                    Attribute::Id(v) => id = Some(v),
                    Attribute::Value(v) => der = Some(v),
                    _ => {}
                }
            }
            let (Some(id), Some(der)) = (id, der) else { continue };
            let Ok(certificate) = Certificate::parse(&der) else { continue };
            // The mechanisms used here sign with RSA, and only certificates allowed to sign
            // (key usage digitalSignature or nonRepudiation) are offered.
            if !matches!(certificate.public_key, PublicKey::Rsa { .. }) || certificate.key_usage.is_some_and(|u| u & 0b11 == 0) {
                continue;
            }
            let token = info.label().trim().to_string();
            out.push((certificate, Location { module: module.to_path_buf(), slot, id, protected: info.protected_authentication_path(), token }));
        }
    }
    Ok(out)
}

/// Every module's certificates. A module that doesn't load (another architecture, a broken
/// install) or a slot that can't be read only means fewer identities; the reason goes to the log.
fn listing(added: &[PathBuf]) -> Vec<(Certificate, Location)> {
    let mut out = Vec::new();
    for module in modules(added) {
        match module_listing(&module) {
            Ok(found) => out.extend(found),
            Err(e) => log::warn!("PKCS #11 module skipped: {e}"),
        }
    }
    out
}

/// A device identity as the Sign dialog shows it.
pub struct TokenId {
    pub certificate: Certificate,
    pub reference: String,
    /// The device's label (e.g. "desktopID", "SafeNet eToken").
    pub token: String,
    /// The device asks PdfCraft for a PIN (otherwise it authenticates by itself).
    pub needs_pin: bool,
}

fn token_ids(found: Vec<(Certificate, Location)>) -> Vec<TokenId> {
    let mut out: Vec<TokenId> = Vec::new();
    for (certificate, loc) in found {
        let reference = reference(&certificate);
        if out.iter().any(|o| o.reference == reference) {
            continue;
        }
        out.push(TokenId { reference, token: loc.token.clone(), needs_pin: !loc.protected, certificate });
    }
    out
}

/// The signing certificates on every device of [`modules`].
pub fn identities(added: &[PathBuf]) -> Vec<TokenId> {
    token_ids(listing(added))
}

/// The signing certificates on the devices of one module, for checking a library the user picks:
/// an error says why it can't be used (not a PKCS #11 library, another architecture).
pub fn module_identities(module: &Path) -> Result<Vec<TokenId>, SignError> {
    module_listing(module).map(token_ids)
}

fn locate(reference: &str, added: &[PathBuf]) -> Result<(Certificate, Location), SignError> {
    listing(added)
        .into_iter()
        .find(|(c, _)| self::reference(c) == reference)
        .ok_or_else(|| SignError::Crypto(format!("no device certificate {reference} (is the device connected?)")))
}

/// The identity a reference names, signing on its device. `pin` is for devices without a
/// protected authentication path.
pub fn find(reference: &str, pin: Option<&str>, added: &[PathBuf]) -> Result<DigitalId, SignError> {
    let (certificate, loc) = locate(reference, added)?;
    let name = format!("{} ({})", certificate.display_name(), loc.token);
    let key = PrivateKey::external(certificate.public_key.clone(), Arc::new(TokenKey { loc, pin: pin.map(str::to_string) }));
    Ok(DigitalId { key, certificate, chain: Vec::new(), friendly_name: Some(name) })
}

/// Log in to the device of `reference` ahead of signing, so a device that asks for approval asks
/// while the signature is set up. Blocks until the device answers: call it off the UI thread.
pub fn prepare(reference: &str, pin: Option<&str>, added: &[PathBuf]) -> Result<(), SignError> {
    let (_, loc) = locate(reference, added)?;
    logged_in(&loc, pin).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_locations_are_absolute_and_unique() {
        let all = known_locations();
        assert!(!all.is_empty());
        for p in &all {
            assert!(p.is_absolute(), "{}", p.display());
            assert_eq!(all.iter().filter(|q| *q == p).count(), 1, "{} listed twice", p.display());
        }
    }

    #[test]
    fn modules_are_existing_files_each_once() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-pkcs11-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let lib = dir.join("fake-pkcs11.so");
        let _ = std::fs::write(&lib, b"not a library");
        let found = modules(&[lib.clone(), lib.clone(), dir.join("missing.so")]);
        assert_eq!(found.iter().filter(|p| **p == lib).count(), 1);
        assert!(!found.iter().any(|p| p.ends_with("missing.so")));
        // Not a PKCS #11 library: an error, never a panic.
        assert!(module_identities(&lib).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
