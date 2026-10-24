//! Explicit protected service files or an explicitly selected per-user OS vault.
//! A permission failure never falls back to another store or unprotected files.
//! Chunking accommodates Windows Credential Manager's small per-entry blob limit.

use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use zeroize::Zeroizing;

const SERVICE: &str = "org.pdfcraft.hot-folder.sources";
const MAX_SECRET: usize = 64 * 1024;
const CHUNK: usize = 1800;
static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Explicit execution context, never part of exported task JSON. Services use their
/// installer-created protected `secrets` directory; portable workers can select OsVault.
#[derive(Clone, Debug)]
pub enum CredentialStore {
    OsVault,
    ServiceDirectory {
        directory: PathBuf,
    },
    /// Explicit elevated provisioning, bound to an existing private service config.
    /// Workers must run as the service account, never use this administrator context.
    #[cfg(unix)]
    ServiceAdministrator {
        configuration: PathBuf,
        owner_uid: u32,
        owner_gid: u32,
    },
}
impl CredentialStore {
    pub fn for_config(config: &Path, root: Option<&Path>) -> Result<Self, String> {
        let parent = super::confined(config.parent().ok_or("configuration needs a parent folder")?, root)?;
        let directory = parent.join("secrets");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if rustix::process::geteuid().as_raw() == 0 {
                let configuration = super::confined(config, root)?;
                let metadata =
                    fs::symlink_metadata(&configuration).map_err(|_| "administrator provisioning needs an existing service configuration")?;
                if metadata.uid() != 0 {
                    let policy = FilePolicy::Administrator { configuration: configuration.clone(), uid: metadata.uid(), gid: metadata.gid() };
                    policy.validate_context()?;
                    if !directory.exists() {
                        use std::os::unix::fs::DirBuilderExt;
                        fs::DirBuilder::new().mode(0o700).create(&directory).map_err(|_| "cannot create protected service secrets directory")?;
                        policy.adopt(&File::open(&directory).map_err(|_| "cannot inspect new secrets directory")?)?;
                    }
                    policy.check(&directory, true)?;
                    return Ok(Self::ServiceAdministrator { configuration, owner_uid: metadata.uid(), owner_gid: metadata.gid() });
                }
            }
        }
        // Unix: only the service account may provision this directory. Windows: the
        // installer creates its protected ACL before anything can contain a secret.
        #[cfg(unix)]
        if !directory.exists() {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .map_err(|_| "cannot create service secrets directory; provision as the configured service account")?;
        }
        check_private(&directory, true)?;
        Ok(Self::ServiceDirectory { directory })
    }
    fn vault(&self) -> Result<Box<dyn Vault + '_>, String> {
        match self {
            Self::OsVault => Ok(Box::new(OsVault)),
            Self::ServiceDirectory { directory } => Ok(Box::new(FileVault::open(directory, FilePolicy::CurrentUser)?)),
            #[cfg(unix)]
            Self::ServiceAdministrator { configuration, owner_uid, owner_gid } => {
                let directory = configuration.parent().ok_or("service configuration needs a parent")?.join("secrets");
                Ok(Box::new(FileVault::open(
                    &directory,
                    FilePolicy::Administrator { configuration: configuration.clone(), uid: *owner_uid, gid: *owner_gid },
                )?))
            }
        }
    }
}

enum FilePolicy {
    CurrentUser,
    #[cfg(unix)]
    Administrator {
        configuration: PathBuf,
        uid: u32,
        gid: u32,
    },
}
impl FilePolicy {
    fn validate_context(&self) -> Result<(), String> {
        #[cfg(unix)]
        if let Self::Administrator { configuration, uid, gid } = self {
            if rustix::process::geteuid().as_raw() != 0 || *uid == 0 {
                return Err("administrator credential provisioning requires root and a distinct non-root service owner".into());
            }
            super::confined(configuration, None)?;
            check_private_as(configuration, false, Some((*uid, *gid)))?;
            check_private_as(configuration.parent().ok_or("service configuration needs a parent")?, true, Some((*uid, *gid)))?;
        }
        Ok(())
    }
    fn check(&self, path: &Path, directory: bool) -> Result<(), String> {
        self.validate_context()?;
        #[cfg(unix)]
        if let Self::Administrator { uid, gid, .. } = self {
            return check_private_as(path, directory, Some((*uid, *gid)));
        }
        check_private(path, directory)
    }
    fn adopt(&self, _file: &File) -> Result<(), String> {
        self.validate_context()?;
        #[cfg(unix)]
        if let Self::Administrator { uid, gid, .. } = self {
            std::os::unix::fs::fchown(_file, Some(*uid), Some(*gid))
                .map_err(|_| "cannot assign new credential file to the configured service owner")?;
        }
        Ok(())
    }
}
struct FileVault {
    directory: PathBuf,
    policy: FilePolicy,
    _lock: File,
}
impl FileVault {
    fn open(directory: &Path, policy: FilePolicy) -> Result<Self, String> {
        super::confined(directory, None)?;
        policy.check(directory, true)?;
        let path = directory.join(".credentials.lock");
        let mut options = fs::OpenOptions::new();
        options.create_new(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = match options.open(&path) {
            Ok(file) => {
                policy.adopt(&file)?;
                file
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                policy.check(&path, false)?;
                fs::OpenOptions::new().read(true).write(true).open(&path).map_err(|_| "cannot open credential store lock")?
            }
            Err(_) => return Err("cannot create credential store lock".into()),
        };
        policy.check(&path, false)?;
        lock.try_lock().map_err(|_| "another process is refreshing or changing credentials; retry shortly")?;
        Ok(FileVault { directory: directory.to_owned(), policy, _lock: lock })
    }
    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(format!("{}.secret", super::hash(name.as_bytes())))
    }
}
impl Vault for FileVault {
    fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.policy.check(&self.directory, true)?;
        let target = self.path(name);
        if fs::symlink_metadata(&target).is_ok() {
            self.policy.check(&target, false)?;
        }
        let mut temporary =
            tempfile::Builder::new().prefix(".credential-").tempfile_in(&self.directory).map_err(|_| "cannot create protected credential file")?;
        self.policy.adopt(temporary.as_file())?;
        // Verify before writing any secret, including inherited Windows ACLs.
        self.policy.check(temporary.path(), false)?;
        temporary.write_all(bytes).and_then(|_| temporary.as_file().sync_all()).map_err(|_| "cannot persist protected credential")?;
        temporary.persist(target).map_err(|_| "cannot atomically replace protected credential")?;
        Ok(())
    }
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        self.policy.check(&self.directory, true)?;
        let path = self.path(name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("cannot inspect protected credential".into()),
            Ok(_) => {}
        }
        self.policy.check(&path, false)?;
        let mut bytes = Zeroizing::new(Vec::new());
        File::open(path)
            .map_err(|_| "cannot open protected credential")?
            .take((MAX_SECRET + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read protected credential")?;
        if bytes.len() > MAX_SECRET {
            return Err("protected credential exceeds its size limit".into());
        }
        Ok(Some(bytes))
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.policy.check(&self.directory, true)?;
        let path = self.path(name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err("cannot inspect protected credential".into()),
            Ok(_) => {}
        }
        self.policy.check(&path, false)?;
        fs::remove_file(path).map_err(|_| "cannot remove protected credential".into())
    }
}

fn check_private(path: &Path, directory: bool) -> Result<(), String> {
    check_private_as(path, directory, None)
}
fn check_private_as(path: &Path, directory: bool, _expected_owner: Option<(u32, u32)>) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "protected secrets directory is unavailable; provision it as administrator/service account")?;
    if super::is_link(&metadata) || (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err("credential storage must use real directories and files, never links".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (uid, gid) = _expected_owner.unwrap_or_else(|| (rustix::process::geteuid().as_raw(), metadata.gid()));
        if metadata.uid() != uid || metadata.gid() != gid || metadata.mode() & 0o7777 != if directory { 0o700 } else { 0o600 } {
            return Err(
                "credentials require service-account ownership and mode 0700 for directories / 0600 for files; provision as that account".into()
            );
        }
        if !directory && metadata.nlink() != 1 {
            return Err("credential files must not have hard links".into());
        }
    }
    #[cfg(windows)]
    {
        windows_private(path, directory)?;
    }
    Ok(())
}

#[cfg(windows)]
fn windows_private(path: &Path, directory: bool) -> Result<(), String> {
    use windows_permissions::WindowsSecure;
    use windows_permissions::constants::SecurityInformation;
    let descriptor = path
        .as_os_str()
        .security_descriptor(SecurityInformation::Dacl | SecurityInformation::Owner)
        .map_err(|_| "cannot inspect Windows credential ACL; run provisioning as administrator")?;
    let account = windows_service_account_sid();
    windows_descriptor_private(&descriptor, directory, account.as_deref())
}

#[cfg(windows)]
fn windows_service_account_sid() -> Option<windows_permissions::LocalBox<windows_permissions::Sid>> {
    use windows_service::{service::ServiceAccess, service_manager::*};
    // Trust only SCM's administrator-controlled Log On identity, never a SID in
    // task JSON or an arbitrary owner. Domain lookup runs inside the bounded child.
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?;
    let account = manager.open_service("PdfCraftHotFolder", ServiceAccess::QUERY_CONFIG).ok()?.query_config().ok()?.account_name?;
    let name = account.to_str()?;
    let (_, sid, _) = winsafe::LookupAccountName(None, name.strip_prefix(".\\").unwrap_or(name)).ok()?;
    winsafe::ConvertSidToStringSid(&sid).ok()?.parse().ok()
}

#[cfg(windows)]
fn windows_descriptor_private(
    descriptor: &windows_permissions::SecurityDescriptor,
    directory: bool,
    service_account: Option<&windows_permissions::Sid>,
) -> Result<(), String> {
    use windows_permissions::constants::AceType;
    use windows_permissions::{LocalBox, Sid};
    // NT SERVICE\PdfCraftHotFolder is stable across service-account changes.
    let allowed: Vec<LocalBox<Sid>> = ["S-1-5-18", "S-1-5-32-544", "S-1-5-19", "S-1-5-80-4013549229-3107762393-4025766903-2826249871-1347187068"]
        .iter()
        .map(|sid| sid.parse())
        .collect::<Result<_, _>>()
        .map_err(|_| "cannot construct service ACL identities")?;
    let trusted = |owner: &Sid| allowed.iter().any(|sid| owner == &**sid) || service_account == Some(owner);
    if descriptor.owner().is_none_or(|owner| !trusted(owner)) {
        return Err("credential owner must be SYSTEM, Administrators or the registered service identity".into());
    }
    let acl = descriptor.dacl().ok_or("credential ACL must not grant everyone access")?;
    if acl.len() == 0 || acl.len() > 32 {
        return Err("credential ACL is empty or unexpectedly complex".into());
    }
    for index in 0..acl.len() {
        let ace = acl.get_ace(index).ok_or("credential ACL is malformed")?;
        if ace.ace_type() != AceType::ACCESS_ALLOWED_ACE_TYPE || ace.sid().is_none_or(|owner| !trusted(owner)) {
            return Err(
                "credential ACL may grant access only to SYSTEM, Administrators and the service identities; ordinary Users access is forbidden"
                    .into(),
            );
        }
    }
    if directory {
        let sddl = descriptor.as_sddl().map_err(|_| "cannot inspect credential directory inheritance")?;
        if !sddl.to_string_lossy().contains("D:P") {
            return Err("credential directory must disable inherited ACLs; repair it using the installer".into());
        }
    }
    Ok(())
}

pub(super) fn valid_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 128 || !id.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')) {
        return Err("credential ID must be 1..128 ASCII letters, digits, dots, underscores or hyphens".into());
    }
    Ok(())
}

trait Vault {
    fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String>;
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String>;
    fn remove(&self, name: &str) -> Result<(), String>;
}

struct OsVault;

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
impl Vault for OsVault {
    fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        entry(name)?.set_secret(bytes).map_err(vault_error)
    }
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        match entry(name)?.get_secret() {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(vault_error(error)),
        }
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        match entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(vault_error(error)),
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd", target_os = "openbsd")))]
impl Vault for OsVault {
    fn put(&self, _: &str, _: &[u8]) -> Result<(), String> {
        Err("OS credential storage is unavailable on this platform".into())
    }
    fn get(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        Err("OS credential storage is unavailable on this platform".into())
    }
    fn remove(&self, _: &str) -> Result<(), String> {
        Err("OS credential storage is unavailable on this platform".into())
    }
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
fn entry(name: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, name).map_err(vault_error)
}
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
fn vault_error(_: keyring::Error) -> String {
    // Underlying platform errors can include secret attribute values; do not echo them.
    "OS credential vault is locked, unavailable or access was denied; unlock the current user's vault and retry (no file fallback)".into()
}

#[derive(Serialize, Deserialize)]
struct Pointer {
    generation: String,
    chunks: usize,
    sha256: String,
}
fn pointer(vault: &dyn Vault, id: &str) -> Result<Option<Pointer>, String> {
    let Some(bytes) = vault.get(id)? else {
        return Ok(None);
    };
    if bytes.len() > 1024 {
        return Err("credential manifest is corrupt".into());
    }
    let value: Pointer = serde_json::from_slice(&bytes).map_err(|_| "credential manifest is corrupt".to_owned())?;
    if value.generation.len() != 64
        || !value.generation.bytes().all(|c| c.is_ascii_hexdigit())
        || value.chunks == 0
        || value.chunks > MAX_SECRET.div_ceil(CHUNK)
    {
        return Err("credential manifest is corrupt".into());
    }
    Ok(Some(value))
}
fn chunk_name(id: &str, pointer: &Pointer, index: usize) -> String {
    format!("{id}.{}.{}", pointer.generation, index)
}

fn write(vault: &dyn Vault, id: &str, bytes: &[u8]) -> Result<(), String> {
    valid_id(id)?;
    if bytes.is_empty() || bytes.len() > MAX_SECRET {
        return Err("credential size must be between 1 byte and 64 KiB".into());
    }
    let old = pointer(vault, id)?;
    // Content-addressing plus one atomic pointer update prevents a partial refresh from
    // destroying the last usable credential. The hash is internal to the OS vault.
    let generation = super::hash(bytes);
    let next = Pointer { generation: generation.clone(), chunks: bytes.len().div_ceil(CHUNK), sha256: generation };
    if old.as_ref().is_some_and(|value| value.generation == next.generation) {
        return Ok(());
    }
    let mut stored = 0;
    let result = (|| {
        for (index, chunk) in bytes.chunks(CHUNK).enumerate() {
            vault.put(&chunk_name(id, &next, index), chunk)?;
            stored += 1;
        }
        vault.put(id, &serde_json::to_vec(&next).map_err(|_| "cannot encode credential manifest".to_owned())?)
    })();
    if result.is_err() {
        for index in 0..stored {
            let _ = vault.remove(&chunk_name(id, &next, index));
        }
        return result;
    }
    if let Some(old) = old {
        for index in 0..old.chunks {
            let _ = vault.remove(&chunk_name(id, &old, index));
        }
    }
    Ok(())
}
fn read(vault: &dyn Vault, id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    valid_id(id)?;
    let Some(pointer) = pointer(vault, id)? else {
        return Ok(None);
    };
    let mut output = Zeroizing::new(Vec::new());
    for index in 0..pointer.chunks {
        let bytes = vault.get(&chunk_name(id, &pointer, index))?.ok_or("credential store is incomplete; sign in again")?;
        if bytes.len() > CHUNK || output.len().saturating_add(bytes.len()) > MAX_SECRET {
            return Err("credential size limit exceeded".into());
        }
        output.extend_from_slice(&bytes);
    }
    if super::hash(&output) != pointer.sha256 {
        return Err("credential integrity check failed; sign in again".into());
    }
    Ok(Some(output))
}
fn delete(vault: &dyn Vault, id: &str) -> Result<(), String> {
    valid_id(id)?;
    if let Some(pointer) = pointer(vault, id)? {
        // Remove the public lookup first; an interruption cannot expose a partially removed secret.
        vault.remove(id)?;
        for index in 0..pointer.chunks {
            vault.remove(&chunk_name(id, &pointer, index))?;
        }
    }
    Ok(())
}
pub(super) fn put(store: &CredentialStore, id: &str, bytes: &[u8]) -> Result<(), String> {
    let _lock = LOCK.get_or_init(|| Mutex::new(())).lock().map_err(|_| "credential store lock was interrupted")?;
    write(store.vault()?.as_ref(), id, bytes)
}
pub(super) fn get(store: &CredentialStore, id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    let _lock = LOCK.get_or_init(|| Mutex::new(())).lock().map_err(|_| "credential store lock was interrupted")?;
    read(store.vault()?.as_ref(), id)
}

pub fn credential_status(store: &CredentialStore, id: &str) -> Result<bool, String> {
    Ok(get(store, id)?.is_some())
}
pub fn forget_credentials(store: &CredentialStore, id: &str) -> Result<(), String> {
    let _lock = LOCK.get_or_init(|| Mutex::new(())).lock().map_err(|_| "credential store lock was interrupted")?;
    delete(store.vault()?.as_ref(), id)
}
pub fn save_ftp_password(store: &CredentialStore, id: &str, password: &str) -> Result<(), String> {
    if password.contains(['\r', '\n', '\0']) || password.len() > 4096 {
        return Err("FTP password contains a line break or exceeds 4096 bytes".into());
    }
    // Prefix distinguishes FTP credentials from OAuth state if an ID is accidentally reused.
    let payload = Zeroizing::new(format!("ftp-password-v1:{password}"));
    put(store, id, payload.as_bytes())
}
pub(super) fn ftp_password(store: &CredentialStore, id: &str) -> Result<Zeroizing<String>, String> {
    let bytes = get(store, id)?.ok_or("FTP password is not stored; connect the source explicitly first")?;
    let value = std::str::from_utf8(&bytes).map_err(|_| "FTP credential is corrupt")?;
    let password = value.strip_prefix("ftp-password-v1:").ok_or("credential ID belongs to another source type")?;
    Ok(Zeroizing::new(password.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Memory {
        values: RefCell<BTreeMap<String, Vec<u8>>>,
        deny: Cell<bool>,
        writes_left: Cell<Option<usize>>,
    }
    impl Vault for Memory {
        fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
            if self.deny.get() || self.writes_left.get() == Some(0) {
                return Err("permission denied".into());
            }
            if let Some(left) = self.writes_left.get() {
                self.writes_left.set(Some(left - 1));
            }
            self.values.borrow_mut().insert(name.into(), bytes.to_vec());
            Ok(())
        }
        fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
            if self.deny.get() {
                return Err("permission denied".into());
            }
            Ok(self.values.borrow().get(name).cloned().map(Zeroizing::new))
        }
        fn remove(&self, name: &str) -> Result<(), String> {
            if self.deny.get() {
                return Err("permission denied".into());
            }
            self.values.borrow_mut().remove(name);
            Ok(())
        }
    }
    #[test]
    fn secret_chunks_round_trip_refresh_atomically_and_delete() {
        let vault = Memory::default();
        let bytes = vec![42; 9000];
        write(&vault, "test", &bytes).unwrap();
        assert_eq!(*read(&vault, "test").unwrap().unwrap(), bytes);
        vault.writes_left.set(Some(1));
        assert!(write(&vault, "test", &vec![9; 12000]).is_err());
        assert_eq!(*read(&vault, "test").unwrap().unwrap(), bytes);
        vault.writes_left.set(None);
        write(&vault, "test", b"refreshed token").unwrap();
        assert_eq!(*read(&vault, "test").unwrap().unwrap(), b"refreshed token");
        delete(&vault, "test").unwrap();
        assert!(vault.values.borrow().is_empty());
    }
    #[test]
    fn permission_denial_fails_closed_without_any_fallback() {
        let vault = Memory::default();
        vault.deny.set(true);
        assert!(write(&vault, "test", b"secret").is_err());
        assert!(read(&vault, "test").is_err());
        assert!(delete(&vault, "test").is_err());
        assert!(vault.values.borrow().is_empty());
        assert!(write(&vault, "../escape", b"secret").is_err());
    }
    #[test]
    fn broken_secret_chunk_is_detected() {
        let vault = Memory::default();
        write(&vault, "test", b"secret").unwrap();
        let pointer = pointer(&vault, "test").unwrap().unwrap();
        vault.values.borrow_mut().insert(chunk_name("test", &pointer, 0), b"changed".to_vec());
        assert!(read(&vault, "test").is_err());
    }
    #[cfg(windows)]
    #[test]
    fn windows_service_acl_accepts_only_protected_service_identities() {
        use windows_permissions::{LocalBox, SecurityDescriptor};
        let good: LocalBox<SecurityDescriptor> = "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;LS)".parse().unwrap();
        assert!(windows_descriptor_private(&good, true, None).is_ok());
        let service_sid = "S-1-5-80-4013549229-3107762393-4025766903-2826249871-1347187068";
        let named_sid: windows_permissions::LocalBox<windows_permissions::Sid> = "S-1-5-21-100-200-300-1001".parse().unwrap();
        let named: LocalBox<SecurityDescriptor> =
            format!("O:{named_sid}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{service_sid})").parse().unwrap();
        assert!(windows_descriptor_private(&named, false, Some(&named_sid)).is_ok());
        assert!(windows_descriptor_private(&named, false, None).is_err());
        for sddl in [
            "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;LS)(A;OICI;GR;;;BU)",
            "O:BAD:(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;LS)",
            "O:BUD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;LS)",
            "O:BAD:P(A;OICI;FA;;;WD)",
            "O:BA",
        ] {
            let descriptor: LocalBox<SecurityDescriptor> = sddl.parse().unwrap();
            assert!(windows_descriptor_private(&descriptor, true, None).is_err(), "{sddl}");
        }
        let temp = tempfile::tempdir().unwrap();
        let store = CredentialStore::ServiceDirectory { directory: temp.path().to_owned() };
        assert!(save_ftp_password(&store, "test", "must not reach an ordinary temp directory").is_err());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
    #[cfg(unix)]
    #[test]
    fn unix_service_store_checks_modes_and_owner_and_round_trips_without_keyring() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let store = CredentialStore::ServiceDirectory { directory: temp.path().to_owned() };
        save_ftp_password(&store, "test", "test-only password").unwrap();
        assert_eq!(*ftp_password(&store, "test").unwrap(), "test-only password");
        for file in fs::read_dir(temp.path()).unwrap() {
            assert_eq!(file.unwrap().metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(credential_status(&store, "test").is_err());
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        forget_credentials(&store, "test").unwrap();
        assert!(!credential_status(&store, "test").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn administrator_context_rejects_unbound_or_nonprivate_service_config() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let config = temp.path().join("tasks.json");
        fs::write(&config, b"{}").unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        let store = CredentialStore::ServiceAdministrator { configuration: config.clone(), owner_uid: 65534, owner_gid: 65534 };
        assert!(save_ftp_password(&store, "test", "must not be written").is_err());
        assert!(!temp.path().join("secrets").exists());
        let zero = CredentialStore::ServiceAdministrator { configuration: config, owner_uid: 0, owner_gid: 0 };
        assert!(save_ftp_password(&zero, "test", "must not be written").is_err());
    }

    /// Run the compiled test executable via sudo, not cargo as root: this touches
    /// only a fresh temporary directory and never connects to an account/keyring.
    #[cfg(unix)]
    #[test]
    #[ignore = "requires root to test provisioning for a distinct synthetic service UID"]
    fn unix_admin_provisioning_preserves_service_uid_gid_and_private_modes() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, fchown};
        assert_eq!(rustix::process::geteuid().as_raw(), 0, "run only this ignored test executable with sudo");
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let config = temp.path().join("tasks.json");
        fs::write(&config, b"{\"version\":1,\"tasks\":[]}").unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        fchown(File::open(&config).unwrap(), Some(65534), Some(65534)).unwrap();
        fchown(File::open(temp.path()).unwrap(), Some(65534), Some(65534)).unwrap();
        let store = CredentialStore::for_config(&config, Some(temp.path())).unwrap();
        assert!(matches!(store, CredentialStore::ServiceAdministrator { owner_uid: 65534, owner_gid: 65534, .. }));
        put(&store, "test", &vec![7; 9000]).unwrap();
        assert_eq!(*get(&store, "test").unwrap().unwrap(), vec![7; 9000]);
        let directory = temp.path().join("secrets");
        assert_eq!(fs::metadata(&directory).unwrap().uid(), 65534);
        for path in fs::read_dir(&directory).unwrap() {
            let meta = path.unwrap().metadata().unwrap();
            assert_eq!((meta.uid(), meta.gid(), meta.mode() & 0o7777), (65534, 65534, 0o600));
        }
        fs::set_permissions(&config, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(get(&store, "test").is_err());
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        forget_credentials(&store, "test").unwrap();
        assert!(!credential_status(&store, "test").unwrap());
    }
}
