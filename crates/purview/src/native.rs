use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};

#[cfg(feature = "mip-auth")]
use crate::auth;
use crate::{PurviewAccount, PurviewDocument, PurviewError, PurviewLabel, PurviewOpen, PurviewProvider, PurviewRights, PurviewStatus};

static TEMP_ID: AtomicU64 = AtomicU64::new(1);
const MAX_TEMP_BYTES: u64 = 512 * 1024 * 1024;

struct StagedPath(std::path::PathBuf);

impl StagedPath {
    fn with_suffix(&self, suffix: &str) -> Self {
        let mut path = self.0.as_os_str().to_os_string();
        path.push(suffix);
        Self(path.into())
    }
}

impl std::ops::Deref for StagedPath {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<std::path::Path> for StagedPath {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for StagedPath {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("couldn't remove a private Microsoft Purview temporary file: {error}");
        }
    }
}

fn unescape(field: &str) -> String {
    let mut out = String::new();
    let mut chars = field.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn escape_tsv(field: &str) -> String {
    field.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

pub(crate) fn looks_like_mip_protection(name: &str, bytes: &[u8]) -> bool {
    let name = name.to_ascii_lowercase();
    if name.ends_with(".pfile") || name.ends_with(".ppdf") || name.ends_with(".irm") {
        return true;
    }
    [
        b"/MicrosoftIRMServices".as_slice(),
        b"MicrosoftIRMServices Protected PDF".as_slice(),
        b"/EncryptedPayload".as_slice(),
        b"/Subtype /MicrosoftIRMServices".as_slice(),
        b"Encrypted using MicrosoftIRMServices".as_slice(),
    ]
    .iter()
    .any(|marker| bytes.windows(marker.len()).any(|window| window.eq_ignore_ascii_case(marker)))
}

fn helper_path() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("PDFCRAFT_PURVIEW_HELPER") {
        return Some(std::path::PathBuf::from(path));
    }
    let executable = std::env::current_exe().ok();
    let system = cfg!(target_os = "linux").then(|| std::path::Path::new("/usr/lib/pdfcraft/pdfcraft-purview-helper"));
    helper_path_from(
        executable.as_deref().and_then(std::path::Path::parent),
        system,
        option_env!("PDFCRAFT_PURVIEW_HELPER").map(std::path::Path::new),
    )
}

fn helper_path_from(
    folder: Option<&std::path::Path>,
    system_helper: Option<&std::path::Path>,
    build_helper: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    if let Some(folder) = folder {
        let sibling = folder.join(if cfg!(windows) { "pdfcraft-purview-helper.exe" } else { "pdfcraft-purview-helper" });
        if sibling.is_file() {
            return Some(sibling);
        }
        #[cfg(target_os = "linux")]
        {
            let package_helper = folder.join("../lib/pdfcraft/pdfcraft-purview-helper");
            if package_helper.is_file() {
                return Some(package_helper);
            }
        }
    }
    system_helper.filter(|path| path.is_file()).or_else(|| build_helper.filter(|path| path.is_file())).map(std::path::Path::to_path_buf)
}

#[derive(Clone)]
struct PurviewAccountSession {
    account: PurviewAccount,
    rms_access_token: String,
    sync_access_token: String,
}

#[cfg(feature = "mip-auth")]
fn matching_account(rms: Option<&PurviewAccount>, sync: Option<&PurviewAccount>) -> Result<PurviewAccount, PurviewError> {
    let (Some(rms), Some(sync)) = (rms, sync) else {
        return Err(PurviewError::Native("Microsoft did not identify the account for both protection and policy. Sign in again.".into()));
    };
    if rms.id != sync.id {
        return Err(PurviewError::Native("Use the same Microsoft account for both protection and policy sign-in.".into()));
    }
    Ok(rms.clone())
}

pub struct NativePurviewProvider {
    app_id: String,
    auth_status: Mutex<PurviewStatus>,
    account_sessions: Mutex<Vec<PurviewAccountSession>>,
    active_account_id: Mutex<Option<String>>,
    account_gate: RwLock<()>,
    staging_dir: Mutex<Option<std::path::PathBuf>>,
}

impl NativePurviewProvider {
    pub fn new(app_id: Option<&str>) -> Self {
        let app_id = app_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| std::env::var("PDFCRAFT_PURVIEW_APP_ID").ok())
            .or_else(|| option_env!("PDFCRAFT_PURVIEW_APP_ID").map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned))
            .unwrap_or_default();
        Self {
            app_id,
            auth_status: Mutex::new(PurviewStatus::SignedOut),
            account_sessions: Mutex::new(Vec::new()),
            active_account_id: Mutex::new(None),
            account_gate: RwLock::new(()),
            staging_dir: Mutex::new(None),
        }
    }

    fn document_operation(&self) -> Result<RwLockReadGuard<'_, ()>, PurviewError> {
        self.account_gate.try_read().map_err(|error| match error {
            TryLockError::WouldBlock => PurviewError::Native("Microsoft account sign-in or switching is in progress; try again shortly".into()),
            TryLockError::Poisoned(_) => PurviewError::Native("Microsoft Purview account state is unavailable".into()),
        })
    }

    fn account_change(&self) -> Result<RwLockWriteGuard<'_, ()>, PurviewError> {
        self.account_gate.try_write().map_err(|error| match error {
            TryLockError::WouldBlock => PurviewError::Native("Microsoft Purview is busy; wait for its current operation to finish".into()),
            TryLockError::Poisoned(_) => PurviewError::Native("Microsoft Purview account state is unavailable".into()),
        })
    }

    fn staging_dir(&self) -> Result<std::path::PathBuf, PurviewError> {
        let mut cache = self.staging_dir.lock().map_err(|_| PurviewError::Native("Microsoft Purview staging is unavailable".into()))?;
        if let Some(path) = cache.as_ref() {
            return Ok(path.clone());
        }
        let staging_id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("pdfcraft-purview-{}-{staging_id}", std::process::id()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&dir)
                .map_err(|e| PurviewError::Native(format!("couldn't create private Microsoft Purview staging: {e}")))?;
        }
        #[cfg(windows)]
        {
            let sid = pdfcraft_platform::owner_only::current_user_sid()
                .map_err(|e| PurviewError::Native(format!("couldn't identify the private Microsoft Purview staging owner: {e}")))?;
            pdfcraft_platform::owner_only::create_private_dir(&dir, &sid)
                .map_err(|e| PurviewError::Native(format!("couldn't create private Microsoft Purview staging: {e}")))?;
        }
        #[cfg(not(any(unix, windows)))]
        {
            std::fs::create_dir(&dir).map_err(|e| PurviewError::Native(format!("couldn't create Microsoft Purview staging: {e}")))?;
        }
        *cache = Some(dir.clone());
        Ok(dir)
    }

    fn temp_path(&self, extension: &str) -> Result<StagedPath, PurviewError> {
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        Ok(StagedPath(self.staging_dir()?.join(format!("{}-{id}.{extension}", std::process::id()))))
    }

    fn write_temp(&self, bytes: &Arc<Vec<u8>>, extension: &str) -> Result<StagedPath, PurviewError> {
        let path = self.temp_path(extension)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(|e| PurviewError::Native(format!("couldn't stage the document for Microsoft Purview: {e}")))?;
        file.write_all(bytes.as_slice()).map_err(|e| PurviewError::Native(format!("couldn't stage the document for Microsoft Purview: {e}")))?;
        #[cfg(windows)]
        {
            let sid = pdfcraft_platform::owner_only::current_user_sid()
                .map_err(|e| PurviewError::Native(format!("couldn't identify the staged Microsoft Purview file owner: {e}")))?;
            pdfcraft_platform::owner_only::restrict_to_owner(&path, &sid)
                .map_err(|e| PurviewError::Native(format!("couldn't restrict the staged Microsoft Purview file: {e}")))?;
        }
        Ok(path)
    }

    fn read_limited(&self, path: &std::path::Path) -> Result<Arc<Vec<u8>>, PurviewError> {
        let file = std::fs::File::open(path).map_err(|e| PurviewError::Native(format!("couldn't read the MIP SDK output: {e}")))?;
        let metadata = file.metadata().map_err(|e| PurviewError::Native(format!("couldn't inspect the MIP SDK output: {e}")))?;
        if metadata.len() > MAX_TEMP_BYTES {
            return Err(PurviewError::Native("the MIP SDK output exceeded the private staging size limit".into()));
        }
        let mut bytes = Vec::new();
        file.take(MAX_TEMP_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| PurviewError::Native(format!("couldn't read the MIP SDK output: {error}")))?;
        if bytes.len() as u64 > MAX_TEMP_BYTES {
            return Err(PurviewError::Native("the MIP SDK output exceeded the private staging size limit".into()));
        }
        Ok(Arc::new(bytes))
    }

    fn read_delete(&self, path: &std::path::Path) -> Result<Arc<Vec<u8>>, PurviewError> {
        let bytes = self.read_limited(path)?;
        std::fs::remove_file(path).map_err(|error| PurviewError::Native(format!("couldn't remove the MIP SDK output: {error}")))?;
        Ok(bytes)
    }

    fn helper(&self) -> Result<std::path::PathBuf, PurviewError> {
        helper_path().ok_or(PurviewError::SdkUnavailable)
    }

    fn sdk_ready(&self) -> bool {
        option_env!("PDFCRAFT_PURVIEW_SDK_READY").unwrap_or("0") == "1"
    }

    fn active_session(&self) -> Option<PurviewAccountSession> {
        let active_id = self.active_account_id.lock().ok()?.clone();
        let sessions = self.account_sessions.lock().ok()?;
        active_id.and_then(|active_id| sessions.iter().find(|session| session.account.id == active_id).cloned()).or_else(|| sessions.first().cloned())
    }

    fn rms_token(&self) -> Option<String> {
        self.active_session().map(|session| session.rms_access_token)
    }

    fn sync_token(&self) -> Option<String> {
        self.active_session().map(|session| session.sync_access_token)
    }

    fn has_tokens(&self) -> bool {
        self.rms_token().is_some() && self.sync_token().is_some()
    }

    fn username(&self) -> String {
        self.active_session()
            .map(|session| {
                if !session.account.principal_name.is_empty() {
                    session.account.principal_name
                } else if !session.account.email.is_empty() {
                    session.account.email
                } else {
                    session.account.display_name
                }
            })
            .unwrap_or_default()
    }

    fn account(&self) -> Option<PurviewAccount> {
        self.active_session().map(|session| session.account)
    }

    fn accounts(&self) -> Vec<PurviewAccount> {
        self.account_sessions.lock().map(|sessions| sessions.iter().map(|session| session.account.clone()).collect()).unwrap_or_default()
    }

    fn run_helper(&self, line: &str) -> Result<String, PurviewError> {
        let mut child = Command::new(self.helper()?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| PurviewError::Native(format!("couldn't start the Microsoft Purview helper: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .map_err(|e| PurviewError::Native(format!("couldn't send the Microsoft Purview request: {e}")))?;
        }
        let output = child.wait_with_output().map_err(|e| PurviewError::Native(format!("couldn't run the Microsoft Purview helper: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(PurviewError::Native(if stderr.trim().is_empty() {
                "the Microsoft Purview helper failed".into()
            } else {
                stderr.trim().to_owned()
            }));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn parse_open(&self, line: &str) -> Result<Option<PurviewOpen>, PurviewError> {
        if line == "NONE" {
            return Ok(None);
        }
        if line == "SIGNIN" {
            return Err(PurviewError::SignInRequired);
        }
        if line == "NO_ACCESS" {
            return Err(PurviewError::NoAccess);
        }
        if let Some((control, _)) = line.split_once('\t') {
            match control {
                "NONE" => return Ok(None),
                "SIGNIN" => return Err(PurviewError::SignInRequired),
                "NO_ACCESS" => return Err(PurviewError::NoAccess),
                "DENIED" => {
                    let message = line.strip_prefix("DENIED\t").unwrap_or("");
                    let message = message.split_once('\t').map(|(message, _)| message).unwrap_or(message);
                    return Err(PurviewError::Native(unescape(message)));
                }
                _ => {}
            }
        }
        if let Some(message) = line.strip_prefix("DENIED\t") {
            return Err(PurviewError::Native(unescape(message)));
        }
        if let Some(fields_line) = line.strip_prefix("OK\t") {
            let fields: Vec<&str> = fields_line.split('\t').collect();
            if fields.len() < 25 {
                return Err(PurviewError::Native("the Microsoft Purview helper returned an incomplete result".into()));
            }
            let handle = 1_u64;
            let protected = fields[0] == "PROTECTED";
            let label = if fields[1].is_empty() {
                None
            } else {
                Some(PurviewLabel {
                    id: unescape(fields[1]),
                    name: unescape(fields[2]),
                    description: unescape(fields[3]),
                    tooltip: unescape(fields[4]),
                    color: if fields[5].is_empty() { None } else { Some(unescape(fields[5])) },
                    parent_id: None,
                    sensitivity: 0,
                    active: true,
                    protection: protected,
                })
            };
            let rights = PurviewRights {
                owner: fields[6] == "1",
                view: fields[7] == "1",
                edit: fields[8] == "1",
                save: fields[9] == "1",
                comment: fields[10] == "1",
                extract: fields[11] == "1",
                export: fields[12] == "1",
                print: fields[13] == "1",
                forward: fields[14] == "1",
                reply: fields[15] == "1",
                reply_all: fields[16] == "1",
                view_rights: fields[17] == "1",
                change_rights: fields[18] == "1",
                object_model: fields[19] == "1",
                document_edit: fields[20] == "1",
            };
            let owner = if fields[21].is_empty() { None } else { Some(unescape(fields[21])) };
            let content_id = if fields[22].is_empty() { None } else { Some(unescape(fields[22])) };
            let labeling_required = fields[23] == "1";
            let default_label_id = if fields[24].is_empty() { None } else { Some(unescape(fields[24])) };
            let downgrade_justification_required = fields.get(25).is_some_and(|value| *value == "1");
            let output_path = fields.get(26).copied().unwrap_or("");
            let working = self.read_delete(std::path::Path::new(output_path))?;
            return Ok(Some(PurviewOpen {
                handle,
                original: Arc::new(Vec::new()),
                bytes: working,
                document: PurviewDocument {
                    protected,
                    label,
                    rights,
                    owner,
                    content_id,
                    labeling_required,
                    default_label_id,
                    downgrade_justification_required,
                },
            }));
        }
        Err(PurviewError::Native("the Microsoft Purview helper returned an unexpected result".into()))
    }

    fn parse_mutation(&self, line: &str, output: &std::path::Path, missing: &str) -> Result<PurviewOpen, PurviewError> {
        if !line.starts_with("OK\t") {
            return self.parse_open(line)?.ok_or_else(|| PurviewError::Native(missing.to_owned()));
        }
        let original = self.read_limited(output)?;
        let mut opened =
            self.parse_open(&format!("{line}\t{}", output.to_string_lossy()))?.ok_or_else(|| PurviewError::Native(missing.to_owned()))?;
        opened.original = original;
        Ok(opened)
    }
}

impl PurviewProvider for NativePurviewProvider {
    fn status(&self) -> PurviewStatus {
        if self.app_id.is_empty() {
            PurviewStatus::NotConfigured
        } else if !self.sdk_ready() || helper_path().is_none() {
            PurviewStatus::SdkUnavailable
        } else {
            self.auth_status
                .lock()
                .ok()
                .map(|status| *status)
                .unwrap_or_else(|| if self.has_tokens() { PurviewStatus::Ready } else { PurviewStatus::SignedOut })
        }
    }

    fn account(&self) -> Option<PurviewAccount> {
        self.account()
    }

    fn accounts(&self) -> Vec<PurviewAccount> {
        self.accounts()
    }

    fn switch_account(&self, account_id: &str) -> Result<(), PurviewError> {
        let _account_change = self.account_change()?;
        let exists = self
            .account_sessions
            .lock()
            .map_err(|_| PurviewError::Native("Microsoft account sessions are unavailable".into()))?
            .iter()
            .any(|session| session.account.id == account_id);
        if !exists {
            return Err(PurviewError::Native("that Microsoft account is not signed in".into()));
        }
        *self.active_account_id.lock().map_err(|_| PurviewError::Native("Microsoft account selection is unavailable".into()))? =
            Some(account_id.to_owned());
        *self.auth_status.lock().map_err(|_| PurviewError::Native("Microsoft account sign-in status is unavailable".into()))? = PurviewStatus::Ready;
        Ok(())
    }

    #[cfg(not(feature = "mip-auth"))]
    fn sign_in(&self, _username_hint: Option<&str>) -> Result<(), PurviewError> {
        if self.app_id.is_empty() { Err(PurviewError::NotConfigured) } else { Err(PurviewError::SdkUnavailable) }
    }

    #[cfg(feature = "mip-auth")]
    fn sign_in(&self, username_hint: Option<&str>) -> Result<(), PurviewError> {
        if self.app_id.is_empty() {
            return Err(PurviewError::NotConfigured);
        }
        if !self.sdk_ready() || helper_path().is_none() {
            return Err(PurviewError::SdkUnavailable);
        }
        let _account_change = self.account_change()?;
        let previous_status = if self.has_tokens() { PurviewStatus::Ready } else { PurviewStatus::SignedOut };
        {
            let mut status = self.auth_status.lock().map_err(|_| PurviewError::Native("Microsoft account sign-in status is unavailable".into()))?;
            if *status == PurviewStatus::SigningIn {
                return Err(PurviewError::Native("Microsoft account sign-in is already in progress".into()));
            }
            *status = PurviewStatus::SigningIn;
        }
        let username_hint = username_hint.map(str::trim).filter(|value| !value.is_empty()).unwrap_or_default();
        let result = (|| {
            let rms_token = auth::acquire_token(
                &self.app_id,
                username_hint,
                "https://login.microsoftonline.com/common",
                "https://api.aadrm.com",
                "https://api.aadrm.com/.default",
                "",
            )
            .map_err(|error| PurviewError::Native(error.to_string()))?;
            let rms_account = rms_token
                .account
                .as_ref()
                .ok_or_else(|| PurviewError::Native("Microsoft did not identify the account for protection. Sign in again.".into()))?;
            let sync_hint = [rms_account.principal_name.as_str(), rms_account.email.as_str(), username_hint]
                .into_iter()
                .find(|hint| !hint.trim().is_empty())
                .unwrap_or_default();
            let sync_token = auth::acquire_token(
                &self.app_id,
                sync_hint,
                "https://login.microsoftonline.com/common",
                "https://psor.api.microsoft.com",
                "https://psor.api.microsoft.com/.default",
                "",
            )
            .map_err(|error| PurviewError::Native(error.to_string()))?;
            let account = matching_account(rms_token.account.as_ref(), sync_token.account.as_ref())?;
            let session = PurviewAccountSession {
                account: account.clone(),
                rms_access_token: rms_token.access_token,
                sync_access_token: sync_token.access_token,
            };
            {
                let mut active =
                    self.active_account_id.lock().map_err(|_| PurviewError::Native("Microsoft account selection is unavailable".into()))?;
                let mut sessions =
                    self.account_sessions.lock().map_err(|_| PurviewError::Native("Microsoft account sessions are unavailable".into()))?;
                if let Some(existing) = sessions.iter_mut().find(|existing| existing.account.id == account.id) {
                    *existing = session;
                } else {
                    sessions.push(session);
                }
                *active = Some(account.id);
            }
            Ok(())
        })();
        let mut status = self.auth_status.lock().map_err(|_| PurviewError::Native("Microsoft account sign-in status is unavailable".into()))?;
        *status = if result.is_ok() { PurviewStatus::Ready } else { previous_status };
        result
    }

    fn sign_out(&self) -> Result<(), PurviewError> {
        let _account_change = self.account_change()?;
        let remaining = {
            let mut active = self.active_account_id.lock().map_err(|_| PurviewError::Native("Microsoft account selection is unavailable".into()))?;
            let mut sessions = self.account_sessions.lock().map_err(|_| PurviewError::Native("Microsoft account sessions are unavailable".into()))?;
            if let Some(active_id) = active.as_ref() {
                sessions.retain(|session| session.account.id != *active_id);
            }
            let remaining = sessions.first().cloned();
            *active = remaining.as_ref().map(|session| session.account.id.clone());
            remaining
        };
        *self.auth_status.lock().map_err(|_| PurviewError::Native("Microsoft account sign-in status is unavailable".into()))? =
            if remaining.is_some() { PurviewStatus::Ready } else { PurviewStatus::SignedOut };
        if remaining.is_none() {
            let mut staging = self.staging_dir.lock().map_err(|_| PurviewError::Native("Microsoft Purview staging is unavailable".into()))?;
            if let Some(path) = staging.as_ref() {
                match std::fs::remove_dir_all(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(PurviewError::Native(format!("signed out, but couldn't remove private Microsoft Purview staging: {error}")));
                    }
                }
                *staging = None;
            }
        }
        Ok(())
    }

    fn inspect(&self, name: &str, bytes: &Arc<Vec<u8>>) -> Result<Option<PurviewOpen>, PurviewError> {
        if !self.sdk_ready() {
            if looks_like_mip_protection(name, bytes.as_slice()) {
                return Err(PurviewError::SdkUnavailable);
            }
            return Ok(None);
        }
        if helper_path().is_none() {
            if looks_like_mip_protection(name, bytes.as_slice()) {
                return Err(PurviewError::SdkUnavailable);
            }
            return Ok(None);
        }
        if self.app_id.is_empty() {
            if looks_like_mip_protection(name, bytes.as_slice()) {
                return Err(PurviewError::NotConfigured);
            }
            return Ok(None);
        }
        let _operation = self.document_operation()?;
        let extension = std::path::Path::new(name).extension().and_then(|ext| ext.to_str()).unwrap_or("pdf");
        let input = self.write_temp(bytes, extension)?;
        let output = self.temp_path(extension)?;
        let audit = self.temp_path("audit")?;
        let cache = self.staging_dir()?;
        let rms_token = self.rms_token().unwrap_or_default();
        let sync_token = self.sync_token().unwrap_or_default();
        let username = self.username();
        let request = format!(
            "open\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.app_id,
            escape_tsv(&username),
            escape_tsv(&rms_token),
            escape_tsv(&sync_token),
            escape_tsv(&input.to_string_lossy()),
            escape_tsv(&output.to_string_lossy()),
            escape_tsv(&audit.to_string_lossy()),
            escape_tsv(&cache.to_string_lossy())
        );
        let line = self.run_helper(&request)?;
        if line == "NONE" {
            return Ok(None);
        }
        let mut opened = self.parse_open(&format!("{line}\t{}", output.to_string_lossy()))?;
        if let Some(opened) = opened.as_mut() {
            opened.original = bytes.clone();
        }
        Ok(opened)
    }

    fn labels(&self) -> Result<Vec<PurviewLabel>, PurviewError> {
        if !self.sdk_ready() {
            return Err(PurviewError::SdkUnavailable);
        }
        let _operation = self.document_operation()?;
        let rms_token = self.rms_token().ok_or(PurviewError::SignInRequired)?;
        let sync_token = self.sync_token().ok_or(PurviewError::SignInRequired)?;
        let username = self.username();
        let cache = self.staging_dir()?;
        let request = format!(
            "labels\t{}\t{}\t{}\t{}\t{}",
            self.app_id,
            escape_tsv(&username),
            escape_tsv(&rms_token),
            escape_tsv(&sync_token),
            escape_tsv(&cache.to_string_lossy())
        );
        let line = self.run_helper(&request)?;
        if line == "SIGNIN" {
            return Err(PurviewError::SignInRequired);
        }
        let mut labels = Vec::new();
        for record in line.split('\n') {
            if record.is_empty() || record == "END" {
                continue;
            }
            if let Some(fields_line) = record.strip_prefix("LABEL\t") {
                let fields: Vec<&str> = fields_line.split('\t').collect();
                if fields.len() >= 8 {
                    labels.push(PurviewLabel {
                        id: unescape(fields[0]),
                        name: unescape(fields[1]),
                        description: unescape(fields[2]),
                        tooltip: unescape(fields[3]),
                        color: if fields[4].is_empty() { None } else { Some(unescape(fields[4])) },
                        parent_id: if fields[5].is_empty() { None } else { Some(unescape(fields[5])) },
                        sensitivity: fields[6].parse().unwrap_or(0),
                        active: fields[7] == "1",
                        protection: fields.get(8).is_some_and(|value| *value == "1"),
                    });
                }
            }
        }
        Ok(labels)
    }

    fn apply_label(
        &self,
        handle: u64,
        original: &Arc<Vec<u8>>,
        working_bytes: &Arc<Vec<u8>>,
        label_id: &str,
        justification: Option<&str>,
    ) -> Result<PurviewOpen, PurviewError> {
        let _ = (handle, original);
        let _operation = self.document_operation()?;
        let rms_token = self.rms_token().ok_or(PurviewError::SignInRequired)?;
        let sync_token = self.sync_token().ok_or(PurviewError::SignInRequired)?;
        let username = self.username();
        let input = self.write_temp(working_bytes, "pdf")?;
        let output = self.temp_path("pdf")?;
        let _input_audit = input.with_suffix(".audit");
        let _output_audit = output.with_suffix(".audit");
        let _working = output.with_suffix(".working");
        let cache = self.staging_dir()?;
        let request = format!(
            "apply-label\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.app_id,
            escape_tsv(&username),
            escape_tsv(&rms_token),
            escape_tsv(&sync_token),
            escape_tsv(&input.to_string_lossy()),
            escape_tsv(&output.to_string_lossy()),
            escape_tsv(label_id),
            escape_tsv(justification.unwrap_or("")),
            escape_tsv(&cache.to_string_lossy())
        );
        let line = self.run_helper(&request)?;
        if line.starts_with("JUSTIFICATION") {
            return Err(PurviewError::JustificationRequired);
        }
        self.parse_mutation(&line, &output, "the label operation produced no protected document")
    }

    fn remove_label(
        &self,
        handle: u64,
        original: &Arc<Vec<u8>>,
        working_bytes: &Arc<Vec<u8>>,
        justification: Option<&str>,
    ) -> Result<PurviewOpen, PurviewError> {
        let _ = (handle, original);
        let _operation = self.document_operation()?;
        let rms_token = self.rms_token().ok_or(PurviewError::SignInRequired)?;
        let sync_token = self.sync_token().ok_or(PurviewError::SignInRequired)?;
        let username = self.username();
        let input = self.write_temp(working_bytes, "pdf")?;
        let output = self.temp_path("pdf")?;
        let _input_audit = input.with_suffix(".audit");
        let _output_audit = output.with_suffix(".audit");
        let cache = self.staging_dir()?;
        let request = format!(
            "remove-label\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.app_id,
            escape_tsv(&username),
            escape_tsv(&rms_token),
            escape_tsv(&sync_token),
            escape_tsv(&input.to_string_lossy()),
            escape_tsv(&output.to_string_lossy()),
            escape_tsv(justification.unwrap_or("")),
            escape_tsv(&cache.to_string_lossy())
        );
        let line = self.run_helper(&request)?;
        if line == "JUSTIFICATION" {
            return Err(PurviewError::JustificationRequired);
        }
        self.parse_mutation(&line, &output, "the label operation produced no document")
    }

    fn remove_protection(&self, handle: u64, original: &Arc<Vec<u8>>, working_bytes: &Arc<Vec<u8>>) -> Result<PurviewOpen, PurviewError> {
        let _ = (handle, original);
        let _operation = self.document_operation()?;
        let rms_token = self.rms_token().ok_or(PurviewError::SignInRequired)?;
        let sync_token = self.sync_token().ok_or(PurviewError::SignInRequired)?;
        let username = self.username();
        let input = self.write_temp(working_bytes, "pdf")?;
        let output = self.temp_path("pdf")?;
        let _input_audit = input.with_suffix(".audit");
        let _output_audit = output.with_suffix(".audit");
        let cache = self.staging_dir()?;
        let request = format!(
            "remove-protection\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.app_id,
            escape_tsv(&username),
            escape_tsv(&rms_token),
            escape_tsv(&sync_token),
            escape_tsv(&input.to_string_lossy()),
            escape_tsv(&output.to_string_lossy()),
            escape_tsv(&cache.to_string_lossy())
        );
        let line = self.run_helper(&request)?;
        if line == "EXPORT_REQUIRED" {
            return Err(PurviewError::ExportRequired);
        }
        self.parse_mutation(&line, &output, "the protection operation produced no document")
    }

    fn republish(&self, handle: u64, original: &Arc<Vec<u8>>, working_bytes: &Arc<Vec<u8>>) -> Result<Arc<Vec<u8>>, PurviewError> {
        let _ = handle;
        let _operation = self.document_operation()?;
        let rms_token = self.rms_token().ok_or(PurviewError::SignInRequired)?;
        let sync_token = self.sync_token().ok_or(PurviewError::SignInRequired)?;
        let username = self.username();
        let original_path = self.write_temp(original, "pdf")?;
        let working_path = self.write_temp(working_bytes, "pdf")?;
        let output = self.temp_path("pdf")?;
        let _original_audit = original_path.with_suffix(".audit");
        let _working_audit = working_path.with_suffix(".audit");
        let cache = self.staging_dir()?;
        let request = format!(
            "republish\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.app_id,
            escape_tsv(&username),
            escape_tsv(&rms_token),
            escape_tsv(&sync_token),
            escape_tsv(&original_path.to_string_lossy()),
            escape_tsv(&working_path.to_string_lossy()),
            escape_tsv(&output.to_string_lossy()),
            escape_tsv(&cache.to_string_lossy())
        );
        let line = self.run_helper(&request)?;
        if line == "OK" { self.read_delete(&output) } else { Err(PurviewError::Native(line)) }
    }

    fn close(&self, _handle: u64) {}
}

impl Drop for NativePurviewProvider {
    fn drop(&mut self) {
        let staging = match self.staging_dir.get_mut() {
            Ok(staging) => staging,
            Err(error) => {
                log::warn!("Microsoft Purview staging state was poisoned; attempting cleanup");
                error.into_inner()
            }
        };
        if let Some(path) = staging.as_ref()
            && let Err(error) = std::fs::remove_dir_all(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("couldn't remove private Microsoft Purview staging: {error}");
        }
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;

    #[test]
    fn installed_helper_takes_precedence_over_a_stale_build_path() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-helper-test-{}-{}", std::process::id(), TEMP_ID.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&dir).unwrap();
        let installed = dir.join(if cfg!(windows) { "pdfcraft-purview-helper.exe" } else { "pdfcraft-purview-helper" });
        let stale = dir.join("missing-build-helper");
        std::fs::write(&installed, b"synthetic helper").unwrap();
        assert_eq!(helper_path_from(Some(&dir), None, Some(&stale)), Some(installed.clone()));
        std::fs::remove_file(&installed).unwrap();
        assert_eq!(helper_path_from(Some(&dir), None, Some(&stale)), None);
        std::fs::write(&stale, b"synthetic build helper").unwrap();
        assert_eq!(helper_path_from(Some(&dir), None, Some(&stale)), Some(stale.clone()));
        std::fs::remove_file(&stale).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn staged_input_is_removed_when_a_request_exits_early() {
        let provider = NativePurviewProvider::new(Some("e16b2bcd-0a3c-414a-8255-0bcc5f6a7246"));
        let path = {
            let staged = provider.write_temp(&Arc::new(b"synthetic document".to_vec()), "pdf").unwrap();
            assert!(staged.exists());
            staged.to_path_buf()
        };
        assert!(!path.exists(), "staged input must not survive an early return");
    }

    #[test]
    fn concurrent_requests_share_one_private_staging_directory() {
        let provider = Arc::new(NativePurviewProvider::new(Some("e16b2bcd-0a3c-414a-8255-0bcc5f6a7246")));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let provider = provider.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    provider.staging_dir().unwrap()
                })
            })
            .collect();
        let paths: Vec<_> = handles.into_iter().map(|handle| handle.join().unwrap()).collect();
        assert!(paths.iter().all(|path| path == &paths[0]));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&paths[0]).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[test]
    #[cfg(all(feature = "mip-sdk", any(target_os = "macos", target_os = "linux")))]
    fn sdk_helper_preserves_backslashes_in_request_fields() {
        let provider = NativePurviewProvider::new(Some("e16b2bcd-0a3c-414a-8255-0bcc5f6a7246"));
        let dir = provider.staging_dir().unwrap();
        let literal = dir.join(r"cache\path");
        std::fs::write(&literal, b"synthetic file").unwrap();
        std::fs::write(dir.join("cachepath"), b"synthetic file").unwrap();

        let sdk = std::env::var_os("MIP_SDK_ROOT").or_else(|| std::env::var_os("PDFCRAFT_MIP_SDK_ROOT")).expect("MIP SDK root");
        let arch = match (std::env::consts::OS, std::env::consts::ARCH) {
            (_, "aarch64") => "arm64",
            ("macos", "x86_64") => "x86_64",
            (_, "x86_64") => "amd64",
            (_, arch) => arch,
        };
        let sdk_bin = std::path::PathBuf::from(sdk).join("bins").join("release").join(arch);
        let library_path = if cfg!(target_os = "macos") { "DYLD_LIBRARY_PATH" } else { "LD_LIBRARY_PATH" };
        let mut child = Command::new(provider.helper().unwrap())
            .env(library_path, sdk_bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let request = format!("labels\tinvalid\tuser\trms\tsync\t{}", escape_tsv(&literal.to_string_lossy()));
        writeln!(child.stdin.take().unwrap(), "{request}").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let response = String::from_utf8(output.stdout).unwrap();
        let denial = unescape(response.strip_prefix("DENIED\t").expect("the existing file cannot be used as an SDK directory"));
        assert!(denial.contains(&*literal.to_string_lossy()), "{denial}");
    }

    #[test]
    fn mutation_keeps_the_committed_output_and_cleans_both_staged_files() {
        let provider = NativePurviewProvider::new(Some("e16b2bcd-0a3c-414a-8255-0bcc5f6a7246"));
        let (output_path, working_path) = {
            let output = provider.temp_path("pdf").unwrap();
            let working = output.with_suffix(".working");
            std::fs::write(&output, b"synthetic protected output").unwrap();
            std::fs::write(&working, b"synthetic working pdf").unwrap();
            let mut fields = vec!["PROTECTED".to_owned(), "label".into(), "Label".into(), String::new(), String::new(), String::new()];
            fields.extend((0..15).map(|_| "0".to_owned()));
            fields.extend(["owner", "content", "0", "", "0"].map(str::to_owned));
            fields.push(working.to_string_lossy().into_owned());
            let result = provider.parse_mutation(&format!("OK\t{}", fields.join("\t")), &output, "missing output").unwrap();
            assert_eq!(result.original.as_slice(), b"synthetic protected output");
            assert_eq!(result.bytes.as_slice(), b"synthetic working pdf");
            (output.to_path_buf(), working.to_path_buf())
        };
        assert!(!output_path.exists());
        assert!(!working_path.exists());
    }

    #[test]
    fn parse_open_accepts_control_results_with_the_output_suffix() {
        let provider = NativePurviewProvider::new(Some("e16b2bcd-0a3c-414a-8255-0bcc5f6a7246"));
        assert_eq!(provider.parse_open("NONE\t/tmp/output.pdf").unwrap(), None);
        assert!(matches!(provider.parse_open("SIGNIN\t/tmp/output.pdf"), Err(PurviewError::SignInRequired)));
        assert!(matches!(provider.parse_open("NO_ACCESS\t/tmp/output.pdf"), Err(PurviewError::NoAccess)));
    }

    #[test]
    fn account_sessions_can_switch_and_sign_out_independently() {
        let provider = NativePurviewProvider::new(Some("e16b2bcd-0a3c-414a-8255-0bcc5f6a7246"));
        let ada = PurviewAccount {
            id: "ada".into(),
            display_name: "Ada Lovelace".into(),
            email: "ada@example.com".into(),
            principal_name: "ada@example.com".into(),
        };
        let grace = PurviewAccount {
            id: "grace".into(),
            display_name: "Grace Hopper".into(),
            email: "grace@example.com".into(),
            principal_name: "grace@example.com".into(),
        };
        if let Ok(mut sessions) = provider.account_sessions.lock() {
            sessions.push(PurviewAccountSession { account: ada.clone(), rms_access_token: "rms-ada".into(), sync_access_token: "sync-ada".into() });
            sessions.push(PurviewAccountSession {
                account: grace.clone(),
                rms_access_token: "rms-grace".into(),
                sync_access_token: "sync-grace".into(),
            });
        }
        if let Ok(mut active) = provider.active_account_id.lock() {
            *active = Some("ada".into());
        }
        assert_eq!(provider.account(), Some(ada));
        assert_eq!(provider.accounts().len(), 2);
        provider.switch_account("grace").unwrap();
        assert_eq!(provider.account(), Some(grace.clone()));
        assert_eq!(provider.rms_token().as_deref(), Some("rms-grace"));
        provider.sign_out().unwrap();
        assert_eq!(provider.accounts().len(), 1);
        assert_eq!(provider.account().map(|account| account.id), Some("ada".to_owned()));
    }

    #[test]
    fn account_changes_cannot_overlap_document_operations() {
        let provider = NativePurviewProvider::new(Some("e16b2bcd-0a3c-414a-8255-0bcc5f6a7246"));
        let operation = provider.document_operation().unwrap();
        assert!(matches!(provider.sign_out(), Err(PurviewError::Native(message)) if message.contains("busy")));
        drop(operation);

        let account_change = provider.account_change().unwrap();
        let bytes = Arc::new(b"synthetic document".to_vec());
        assert!(matches!(provider.republish(1, &bytes, &bytes), Err(PurviewError::Native(message)) if message.contains("in progress")));
        #[cfg(feature = "mip-sdk")]
        assert!(matches!(provider.inspect("doc.pdf", &bytes), Err(PurviewError::Native(message)) if message.contains("in progress")));
        drop(account_change);
        assert!(provider.sign_out().is_ok());
    }

    #[test]
    #[cfg(feature = "mip-auth")]
    fn protection_and_policy_must_identify_the_same_account() {
        let ada = PurviewAccount {
            id: "ada".into(),
            display_name: "Ada Lovelace".into(),
            email: "ada@example.com".into(),
            principal_name: "ada@example.com".into(),
        };
        let grace = PurviewAccount { id: "grace".into(), ..ada.clone() };
        assert_eq!(matching_account(Some(&ada), Some(&ada)).unwrap(), ada);
        assert!(matching_account(Some(&ada), Some(&grace)).is_err());
        assert!(matching_account(Some(&ada), None).is_err());
        assert!(matching_account(None, Some(&ada)).is_err());
    }

    #[test]
    #[cfg(feature = "mip-sdk")]
    fn failed_add_account_keeps_the_existing_account_ready() {
        let provider = NativePurviewProvider::new(Some("invalid-app-id"));
        let ada = PurviewAccount {
            id: "ada".into(),
            display_name: "Ada Lovelace".into(),
            email: "ada@example.com".into(),
            principal_name: "ada@example.com".into(),
        };
        provider.account_sessions.lock().unwrap().push(PurviewAccountSession {
            account: ada.clone(),
            rms_access_token: "rms-ada".into(),
            sync_access_token: "sync-ada".into(),
        });
        *provider.active_account_id.lock().unwrap() = Some(ada.id.clone());
        *provider.auth_status.lock().unwrap() = PurviewStatus::Ready;

        assert!(provider.sign_in(None).is_err());
        assert_eq!(provider.status(), PurviewStatus::Ready);
        assert_eq!(provider.account(), Some(ada));
    }
}
