use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::auth;
use crate::{PurviewDocument, PurviewError, PurviewLabel, PurviewOpen, PurviewProvider, PurviewRights, PurviewStatus};

static TEMP_ID: AtomicU64 = AtomicU64::new(1);

fn temp_path(extension: &str) -> std::path::PathBuf {
    let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("pdfcraft-purview-{}-{id}.{extension}", std::process::id()))
}

fn write_temp(bytes: &Arc<Vec<u8>>, extension: &str) -> Result<std::path::PathBuf, PurviewError> {
    let path = temp_path(extension);
    std::fs::write(&path, bytes.as_slice()).map_err(|e| PurviewError::Native(format!("couldn't stage the document for Microsoft Purview: {e}")))?;
    Ok(path)
}

fn read_delete(path: &std::path::Path) -> Result<Arc<Vec<u8>>, PurviewError> {
    let bytes = std::fs::read(path).map_err(|e| PurviewError::Native(format!("couldn't read the MIP SDK output: {e}")))?;
    let _ = std::fs::remove_file(path);
    Ok(Arc::new(bytes))
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
    }
    None
}

pub struct NativePurviewProvider {
    app_id: String,
    username_hint: Mutex<Option<String>>,
    rms_access_token: Mutex<Option<String>>,
    sync_access_token: Mutex<Option<String>>,
}

impl NativePurviewProvider {
    pub fn new(app_id: Option<&str>) -> Self {
        let app_id = app_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| std::env::var("PDFCRAFT_PURVIEW_APP_ID").ok())
            .unwrap_or_default();
        Self {
            app_id,
            username_hint: Mutex::new(None),
            rms_access_token: Mutex::new(None),
            sync_access_token: Mutex::new(None),
        }
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
            let working = read_delete(std::path::Path::new(output_path))?;
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
        } else if self.has_tokens() {
            PurviewStatus::Ready
        } else {
            PurviewStatus::SignedOut
        }
    }

    fn sign_in(&self, username_hint: Option<&str>) -> Result<(), PurviewError> {
        if self.app_id.is_empty() {
            return Err(PurviewError::NotConfigured);
        }
        if !self.sdk_ready() || helper_path().is_none() {
            return Err(PurviewError::SdkUnavailable);
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
        .map_err(|error| PurviewError::Native(error.to_string()))?;
        let sync_token = auth::acquire_token(
            &self.app_id,
            &username,
            "https://login.microsoftonline.com/common",
            "https://psor.api.microsoft.com",
            "https://psor.api.microsoft.com/.default",
            "",
        )
        .map_err(|error| PurviewError::Native(error.to_string()))?;
        if let Ok(mut cached) = self.rms_access_token.lock() {
            *cached = Some(rms_token);
        }
        if let Ok(mut cached) = self.sync_access_token.lock() {
            *cached = Some(sync_token);
        }
        Ok(())
    }

    fn sign_out(&self) -> Result<(), PurviewError> {
        if let Ok(mut token) = self.rms_access_token.lock() {
            *token = None;
        }
        if let Ok(mut token) = self.sync_access_token.lock() {
            *token = None;
        }
        Ok(())
    }

    fn inspect(&self, name: &str, bytes: &Arc<Vec<u8>>) -> Result<Option<PurviewOpen>, PurviewError> {
        if !self.sdk_ready() {
            return Ok(None);
        }
        if helper_path().is_none() {
            return Ok(None);
        }
        if self.app_id.is_empty() {
            return Ok(None);
        }
        let extension = std::path::Path::new(name).extension().and_then(|ext| ext.to_str()).unwrap_or("pdf");
        let input = write_temp(bytes, extension)?;
        let output = temp_path(extension);
        let audit = temp_path("audit");
        let rms_token = self.rms_token().unwrap_or_default();
        let sync_token = self.sync_token().unwrap_or_default();
        let username = self.username();
        let request = format!(
            "open\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.app_id,
            escape_tsv(&username),
            escape_tsv(&rms_token),
            escape_tsv(&sync_token),
            escape_tsv(&input.to_string_lossy()),
            escape_tsv(&output.to_string_lossy()),
            escape_tsv(&audit.to_string_lossy())
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
        let request = format!(
            "labels\t{}\t{}\t{}\t{}",
            self.app_id,
            escape_tsv(&username),
            escape_tsv(&rms_token),
            escape_tsv(&sync_token)
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
        let input = write_temp(working_bytes, "pdf")?;
        let output = temp_path("pdf");
        let request = format!(
            "apply-label\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.app_id,
            escape_tsv(&username),
            escape_tsv(&rms_token),
            escape_tsv(&sync_token),
            escape_tsv(&input.to_string_lossy()),
            escape_tsv(&output.to_string_lossy()),
            escape_tsv(label_id),
            escape_tsv(justification.unwrap_or(""))
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

    fn remove_label(&self, handle: u64, original: &Arc<Vec<u8>>, working_bytes: &Arc<Vec<u8>>) -> Result<PurviewOpen, PurviewError> {
        let _ = (handle, original);
        let rms_token = self.rms_token().ok_or(PurviewError::SignInRequired)?;
        let sync_token = self.sync_token().ok_or(PurviewError::SignInRequired)?;
        let username = self.username();
        let input = write_temp(working_bytes, "pdf")?;
        let output = temp_path("pdf");
        let request = format!(
            "remove-label\t{}\t{}\t{}\t{}\t{}\t{}",
            self.app_id,
            escape_tsv(&username),
            escape_tsv(&rms_token),
            escape_tsv(&sync_token),
            escape_tsv(&input.to_string_lossy()),
            escape_tsv(&output.to_string_lossy())
        );
        let line = self.run_helper(&request)?;
        let _ = std::fs::remove_file(&input);
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
        let input = write_temp(working_bytes, "pdf")?;
        let output = temp_path("pdf");
        let request = format!(
            "remove-protection\t{}\t{}\t{}\t{}\t{}\t{}",
            self.app_id,
            escape_tsv(&username),
            escape_tsv(&rms_token),
            escape_tsv(&sync_token),
            escape_tsv(&input.to_string_lossy()),
            escape_tsv(&output.to_string_lossy())
        );
        let line = self.run_helper(&request)?;
        let _ = std::fs::remove_file(&input);
        if line.starts_with("DENIED") {
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
        let original_path = write_temp(original, "pdf")?;
        let working_path = write_temp(working_bytes, "pdf")?;
        let output = temp_path("pdf");
        let request = format!(
            "republish\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.app_id,
            escape_tsv(&username),
            escape_tsv(&rms_token),
            escape_tsv(&sync_token),
            escape_tsv(&original_path.to_string_lossy()),
            escape_tsv(&working_path.to_string_lossy()),
            escape_tsv(&output.to_string_lossy())
        );
        let line = self.run_helper(&request)?;
        let _ = std::fs::remove_file(&original_path);
        let _ = std::fs::remove_file(&working_path);
        if line == "OK" {
            read_delete(&output)
        } else {
            let _ = std::fs::remove_file(&output);
            Err(PurviewError::Native(line))
        }
    }

    fn close(&self, _handle: u64) {}
}
