use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::auth;
use crate::{PurviewDocument, PurviewError, PurviewLabel, PurviewOpen, PurviewProvider, PurviewRights, PurviewStatus};

static TEMP_ID: AtomicU64 = AtomicU64::new(1);
const MAX_TEMP_BYTES: u64 = 512 * 1024 * 1024;

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
    if let Some(path) = option_env!("PDFCRAFT_PURVIEW_HELPER") {
        return Some(std::path::PathBuf::from(path));
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(folder) = executable.parent()
    {
        let sibling = folder.join(if cfg!(windows) { "pdfcraft-purview-helper.exe" } else { "pdfcraft-purview-helper" });
        if sibling.is_file() {
            return Some(sibling);
        }
        #[cfg(target_os = "linux")]
        {
            let package_helper = std::path::PathBuf::from("/usr/lib/pdfcraft/pdfcraft-purview-helper");
            if package_helper.is_file() {
                return Some(package_helper);
            }
        }
        #[cfg(target_os = "macos")]
        {
            let bundle_helper = folder.join("pdfcraft-purview-helper");
            if bundle_helper.is_file() {
                return Some(bundle_helper);
            }
        }
    }
    None
}

pub struct NativePurviewProvider {
    app_id: String,
    username_hint: Mutex<Option<String>>,
    auth_status: Mutex<PurviewStatus>,
    rms_access_token: Mutex<Option<String>>,
    sync_access_token: Mutex<Option<String>>,
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
            username_hint: Mutex::new(None),
            auth_status: Mutex::new(PurviewStatus::SignedOut),
            rms_access_token: Mutex::new(None),
            sync_access_token: Mutex::new(None),
            staging_dir: Mutex::new(None),
        }
    }

    fn staging_dir(&self) -> Result<std::path::PathBuf, PurviewError> {
        if let Ok(cache) = self.staging_dir.lock()
            && let Some(path) = cache.clone()
        {
            return Ok(path);
        }
        let staging_id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("pdfcraft-purview-{}-{staging_id}", std::process::id()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::create_dir(&dir).map_err(|e| PurviewError::Native(format!("couldn't create private Microsoft Purview staging: {e}")))?;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| PurviewError::Native(format!("couldn't restrict Microsoft Purview staging: {e}")))?;
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
        if let Ok(mut cache) = self.staging_dir.lock() {
            *cache = Some(dir.clone());
        }
        Ok(dir)
    }

    fn temp_path(&self, extension: &str) -> Result<std::path::PathBuf, PurviewError> {
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        Ok(self.staging_dir()?.join(format!("{}-{id}.{extension}", std::process::id())))
    }

    fn write_temp(&self, bytes: &Arc<Vec<u8>>, extension: &str) -> Result<std::path::PathBuf, PurviewError> {
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

    fn read_delete(&self, path: &std::path::Path) -> Result<Arc<Vec<u8>>, PurviewError> {
        let metadata = std::fs::metadata(path).map_err(|e| PurviewError::Native(format!("couldn't read the MIP SDK output: {e}")))?;
        if metadata.len() > MAX_TEMP_BYTES {
            let _ = std::fs::remove_file(path);
            return Err(PurviewError::Native("the MIP SDK output exceeded the private staging size limit".into()));
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = std::fs::remove_file(path);
                return Err(PurviewError::Native(format!("couldn't read the MIP SDK output: {error}")));
            }
        };
        let _ = std::fs::remove_file(path);
        Ok(Arc::new(bytes))
    }

    fn helper(&self) -> Result<std::path::PathBuf, PurviewError> {
        helper_path().ok_or(PurviewError::SdkUnavailable)
    }

    fn sdk_ready(&self) -> bool {
        option_env!("PDFCRAFT_PURVIEW_SDK_READY").unwrap_or("0") == "1"
    }

    fn rms_token(&self) -> Option<String> {
        self.rms_access_token.lock().ok().and_then(|token| token.clone())
    }

    fn sync_token(&self) -> Option<String> {
        self.sync_access_token.lock().ok().and_then(|token| token.clone())
    }

    fn has_tokens(&self) -> bool {
        self.rms_token().is_some() && self.sync_token().is_some()
    }

    fn username(&self) -> String {
        self.username_hint.lock().ok().and_then(|username| username.clone()).unwrap_or_default()
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

    fn sign_in(&self, username_hint: Option<&str>) -> Result<(), PurviewError> {
        if self.app_id.is_empty() {
            return Err(PurviewError::NotConfigured);
        }
        if !self.sdk_ready() || helper_path().is_none() {
            return Err(PurviewError::SdkUnavailable);
        }
        if let Ok(mut status) = self.auth_status.lock() {
            *status = PurviewStatus::SigningIn;
        }
        if let Ok(mut username) = self.username_hint.lock() {
            *username = username_hint.map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned);
        }
        let username = self.username();
        let rms_token = auth::acquire_token(
            &self.app_id,
            &username,
            "https://login.microsoftonline.com/common",
            "https://api.aadrm.com",
            "https://api.aadrm.com/.default",
            "",
        )
        .map_err(|error| {
            if let Ok(mut status) = self.auth_status.lock() {
                *status = PurviewStatus::SignedOut;
            }
            PurviewError::Native(error.to_string())
        })?;
        let sync_token = auth::acquire_token(
            &self.app_id,
            &username,
            "https://login.microsoftonline.com/common",
            "https://psor.api.microsoft.com",
            "https://psor.api.microsoft.com/.default",
            "",
        )
        .map_err(|error| {
            if let Ok(mut status) = self.auth_status.lock() {
                *status = PurviewStatus::SignedOut;
            }
            PurviewError::Native(error.to_string())
        })?;
        if let Ok(mut cached) = self.rms_access_token.lock() {
            *cached = Some(rms_token);
        }
        if let Ok(mut cached) = self.sync_access_token.lock() {
            *cached = Some(sync_token);
        }
        if let Ok(mut status) = self.auth_status.lock() {
            *status = PurviewStatus::Ready;
        }
        Ok(())
    }

    fn sign_out(&self) -> Result<(), PurviewError> {
        if let Ok(mut status) = self.auth_status.lock() {
            *status = PurviewStatus::SignedOut;
        }
        if let Ok(mut token) = self.rms_access_token.lock() {
            *token = None;
        }
        if let Ok(mut token) = self.sync_access_token.lock() {
            *token = None;
        }
        if let Ok(mut staging) = self.staging_dir.lock()
            && let Some(path) = staging.take()
        {
            let _ = std::fs::remove_dir_all(path);
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
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&audit);
        if line == "NONE" {
            let _ = std::fs::remove_file(&output);
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
        let rms_token = self.rms_token().ok_or(PurviewError::SignInRequired)?;
        let sync_token = self.sync_token().ok_or(PurviewError::SignInRequired)?;
        let username = self.username();
        let input = self.write_temp(working_bytes, "pdf")?;
        let output = self.temp_path("pdf")?;
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
        let _ = std::fs::remove_file(&input);
        if line.starts_with("JUSTIFICATION") {
            let _ = std::fs::remove_file(&output);
            return Err(PurviewError::JustificationRequired);
        }
        let original_output = std::fs::read(&output).ok().map(Arc::new);
        let mut opened = self
            .parse_open(&format!("{line}\t{}", output.to_string_lossy()))?
            .ok_or(PurviewError::Native("the label operation produced no protected document".into()))?;
        if let Some(original_output) = original_output {
            opened.original = original_output;
        }
        Ok(opened)
    }

    fn remove_label(
        &self,
        handle: u64,
        original: &Arc<Vec<u8>>,
        working_bytes: &Arc<Vec<u8>>,
        justification: Option<&str>,
    ) -> Result<PurviewOpen, PurviewError> {
        let _ = (handle, original);
        let rms_token = self.rms_token().ok_or(PurviewError::SignInRequired)?;
        let sync_token = self.sync_token().ok_or(PurviewError::SignInRequired)?;
        let username = self.username();
        let input = self.write_temp(working_bytes, "pdf")?;
        let output = self.temp_path("pdf")?;
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
        let _ = std::fs::remove_file(&input);
        if line == "JUSTIFICATION" {
            let _ = std::fs::remove_file(&output);
            return Err(PurviewError::JustificationRequired);
        }
        let original_output = std::fs::read(&output).ok().map(Arc::new);
        let mut opened = self
            .parse_open(&format!("{line}\t{}", output.to_string_lossy()))?
            .ok_or(PurviewError::Native("the label operation produced no document".into()))?;
        if let Some(original_output) = original_output {
            opened.original = original_output;
        }
        Ok(opened)
    }

    fn remove_protection(&self, handle: u64, original: &Arc<Vec<u8>>, working_bytes: &Arc<Vec<u8>>) -> Result<PurviewOpen, PurviewError> {
        let _ = (handle, original);
        let rms_token = self.rms_token().ok_or(PurviewError::SignInRequired)?;
        let sync_token = self.sync_token().ok_or(PurviewError::SignInRequired)?;
        let username = self.username();
        let input = self.write_temp(working_bytes, "pdf")?;
        let output = self.temp_path("pdf")?;
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
        let _ = std::fs::remove_file(&input);
        if line == "EXPORT_REQUIRED" {
            return Err(PurviewError::ExportRequired);
        }
        let original_output = std::fs::read(&output).ok().map(Arc::new);
        let mut opened = self
            .parse_open(&format!("{line}\t{}", output.to_string_lossy()))?
            .ok_or(PurviewError::Native("the protection operation produced no document".into()))?;
        if let Some(original_output) = original_output {
            opened.original = original_output;
        }
        Ok(opened)
    }

    fn republish(&self, handle: u64, original: &Arc<Vec<u8>>, working_bytes: &Arc<Vec<u8>>) -> Result<Arc<Vec<u8>>, PurviewError> {
        let _ = handle;
        let rms_token = self.rms_token().ok_or(PurviewError::SignInRequired)?;
        let sync_token = self.sync_token().ok_or(PurviewError::SignInRequired)?;
        let username = self.username();
        let original_path = self.write_temp(original, "pdf")?;
        let working_path = self.write_temp(working_bytes, "pdf")?;
        let output = self.temp_path("pdf")?;
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
        let _ = std::fs::remove_file(&original_path);
        let _ = std::fs::remove_file(&working_path);
        if line == "OK" {
            self.read_delete(&output)
        } else {
            let _ = std::fs::remove_file(&output);
            Err(PurviewError::Native(line))
        }
    }

    fn close(&self, _handle: u64) {}
}

impl Drop for NativePurviewProvider {
    fn drop(&mut self) {
        if let Ok(mut staging) = self.staging_dir.lock()
            && let Some(path) = staging.take()
        {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;

    #[test]
    fn parse_open_accepts_control_results_with_the_output_suffix() {
        let provider = NativePurviewProvider::new(Some("e16b2bcd-0a3c-414a-8255-0bcc5f6a7246"));
        assert_eq!(provider.parse_open("NONE\t/tmp/output.pdf").unwrap(), None);
        assert!(matches!(provider.parse_open("SIGNIN\t/tmp/output.pdf"), Err(PurviewError::SignInRequired)));
        assert!(matches!(provider.parse_open("NO_ACCESS\t/tmp/output.pdf"), Err(PurviewError::NoAccess)));
    }
}
