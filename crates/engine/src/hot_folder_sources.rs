//! Explicit Hot Folder source adapters. No network access occurs until a worker calls
//! `stage` or a user starts sign-in. Task configuration and journals contain no secrets.
//! Remote originals are acknowledged only after the caller has durably saved every output.

mod credentials;
mod ftp;
mod outlook;
mod project;

pub use credentials::{CredentialStore, credential_status, forget_credentials, save_ftp_password};
pub use outlook::{DeviceSignIn, MailFolder, SignInStatus, outlook_begin_sign_in, outlook_folders, outlook_poll_sign_in};
pub use project::{ProjectManifest, write_project_manifest};

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_ITEMS: usize = 10_000;
const MAX_BATCH: usize = 128;
const MAX_JSON: u64 = 8 * 1024 * 1024;
const STATE_FILE: &str = ".pdfcraft-source-state.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceConfig {
    #[default]
    Local,
    Ftp(FtpSource),
    Outlook(OutlookSource),
    Project(ProjectSource),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceAction {
    #[default]
    Keep,
    /// FTP: existing absolute server directory. Outlook: destination mail-folder ID.
    /// Project: existing local directory. Outlook actions affect the entire message.
    Move {
        destination: String,
    },
    Delete,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FtpSecurity {
    #[default]
    ExplicitTls,
    /// Clear-text FTP is available only when explicitly selected; no TLS downgrade.
    Plain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FtpSource {
    pub host: String,
    pub port: u16,
    pub security: FtpSecurity,
    pub username: String,
    pub credential_id: String,
    pub folder: String,
    pub recursive: bool,
    pub action: SourceAction,
}
impl Default for FtpSource {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 21,
            security: FtpSecurity::ExplicitTls,
            username: String::new(),
            credential_id: String::new(),
            folder: "/".into(),
            recursive: false,
            action: SourceAction::Keep,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutlookSource {
    /// The user's own registered public-client application. There is no bundled client ID.
    pub client_id: String,
    pub tenant: String,
    pub credential_id: String,
    pub folder_id: String,
    pub action: SourceAction,
}
impl Default for OutlookSource {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            tenant: "common".into(),
            credential_id: String::new(),
            folder_id: "inbox".into(),
            action: SourceAction::Keep,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectSource {
    pub manifest: PathBuf,
    pub action: SourceAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedSource {
    pub source_id: String,
    /// Original logical path, suitable for output grouping; never used for staging writes.
    pub relative: PathBuf,
    /// A single safe filename beneath the task's staging inbox.
    pub staged_relative: PathBuf,
    pub sha256: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StageReport {
    pub items: Vec<StagedSource>,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub has_more: bool,
    /// Output already confirmed; caller must recheck its durable output receipt before retrying ack.
    #[serde(default)]
    pub pending_acknowledgements: Vec<StagedSource>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RemoteFile {
    key: String,
    relative: PathBuf,
    revision: String,
    size: u64,
    /// For Outlook: immutable message ID. An action waits for every supported attachment.
    group: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    source: StagedSource,
    remote: RemoteFile,
    ready: bool,
    acknowledged: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Journal {
    config_hash: String,
    records: BTreeMap<String, Record>,
}

pub fn validate(config: &SourceConfig, input: &Path, root: Option<&Path>) -> Result<(), String> {
    let input = confined(input, root)?;
    if !input.is_dir() {
        return Err("source inbox must be an existing directory".into());
    }
    match config {
        SourceConfig::Local => Ok(()),
        SourceConfig::Ftp(source) => ftp::validate(source),
        SourceConfig::Outlook(source) => outlook::validate(source),
        SourceConfig::Project(source) => project::validate(source, &input, root),
    }
}

/// Local originals referenced by a source, for cross-task source/output overlap checks.
/// Completed project entries retain their confined path even after a successful move/delete.
pub fn local_paths(config: &SourceConfig, root: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    guarded(|| match config {
        SourceConfig::Project(source) => project::local_paths(source, root),
        _ => Ok(Vec::new()),
    })
}

trait Backend {
    fn list(&mut self, stop: &Control<'_>) -> Result<Vec<RemoteFile>, String>;
    fn read(&mut self, file: &RemoteFile, writer: &mut dyn Write, stop: &Control<'_>) -> Result<(), String>;
    fn finish(&mut self, files: &[Record], action: &SourceAction, stop: &Control<'_>) -> Result<(), String>;
}

fn backend<'a>(
    config: &'a SourceConfig,
    root: Option<&'a Path>,
    control: &Control<'_>,
    store: &'a CredentialStore,
) -> Result<Box<dyn Backend + 'a>, String> {
    match config {
        SourceConfig::Ftp(source) => Ok(Box::new(ftp::Client::connect(store, source, control)?)),
        SourceConfig::Outlook(source) => Ok(Box::new(outlook::Client::connect(store, source)?)),
        SourceConfig::Project(source) => Ok(Box::new(project::Client::new(source, root)?)),
        SourceConfig::Local => Err("local folders do not use a source adapter".into()),
    }
}

fn action(config: &SourceConfig) -> &SourceAction {
    match config {
        SourceConfig::Ftp(source) => &source.action,
        SourceConfig::Outlook(source) => &source.action,
        SourceConfig::Project(source) => &source.action,
        SourceConfig::Local => &SourceAction::Keep,
    }
}

/// Blocking worker call. Returns both new and previously staged, unacknowledged items.
/// Stage only into this task's inbox; secrets are resolved from the explicitly selected store.
pub fn stage(
    config: &SourceConfig,
    input: &Path,
    root: Option<&Path>,
    should_stop: &dyn Fn() -> bool,
    store: &CredentialStore,
) -> Result<StageReport, String> {
    guarded(|| {
        validate(config, input, root)?;
        if matches!(config, SourceConfig::Local) {
            return Ok(StageReport::default());
        }
        let input = confined(input, root)?;
        let _lock = lock(&input)?;
        let mut journal = read_journal(&input, config)?;
        let control = Control::new(should_stop);
        control.check()?;
        let mut source = backend(config, root, &control, store)?;
        let mut report = stage_with(source.as_mut(), config, &input, &mut journal, &control)?;
        if let SourceConfig::Project(project) = config {
            project::validate_and_order(project, &input, root, &journal, &mut report)?;
        }
        Ok(report)
    })
}

fn stage_with(
    source: &mut dyn Backend,
    config: &SourceConfig,
    input: &Path,
    journal: &mut Journal,
    control: &Control<'_>,
) -> Result<StageReport, String> {
    let files = source.list(control)?;
    if files.len() > MAX_ITEMS {
        return Err("source contains more than 10000 entries; choose a smaller source folder".into());
    }
    let mut unique = BTreeSet::new();
    for file in &files {
        if !unique.insert((&file.group, &file.key)) {
            return Err("source returned duplicate file identifiers; originals retained".into());
        }
    }
    let mut report = StageReport::default();
    let mut total = 0_u64;
    for remote in files {
        control.check()?;
        if !relative_ok(&remote.relative) || remote.size == 0 || remote.size > MAX_FILE {
            return Err("source path is unsafe or source exceeds the 512 MiB per-file limit".into());
        }
        let source_id = hash(format!("{}\n{}\n{}\n{}", journal.config_hash, remote.key, remote.revision, remote.size).as_bytes());
        if !journal.records.contains_key(&source_id)
            && let Some(record) = archived_record(input, &journal.config_hash, &source_id)?
        {
            if record.acknowledged {
                continue;
            }
            return Err("source history contains an unfinished archive record".into());
        }
        if journal.records.get(&source_id).is_some_and(|record| record.acknowledged || record.ready) {
            continue;
        }
        if report.items.len() >= MAX_BATCH || total.saturating_add(remote.size) > MAX_FILE {
            report.warnings.push("source queue continues in the next batch (128 files / 512 MiB)".into());
            report.has_more = true;
            break;
        }
        if let Some(record) = journal.records.get(&source_id) {
            let path = input.join(&record.source.staged_relative);
            if file_hash(&path, control).is_ok_and(|value| value == record.source.sha256) {
                total = total.saturating_add(remote.size);
                report.items.push(record.source.clone());
                continue;
            }
            if path.exists() {
                return Err("staged source changed locally; retain it and use a fresh staging inbox".into());
            }
        }
        if journal.records.len() >= MAX_ITEMS {
            return Err("source queue has 10000 unfinished versions; finish or resolve pending acknowledgements first".into());
        }
        let name = remote.relative.file_name().and_then(|name| name.to_str()).ok_or("source filename is not valid UTF-8")?;
        let staged_relative = PathBuf::from(format!("{}-{}", source_id.get(..24).ok_or("invalid source ID")?, safe_filename(name)));
        let mut temporary = tempfile::Builder::new().prefix(".pdfcraft-source-").tempfile_in(input).map_err(io_error)?;
        source.read(&remote, &mut temporary, control)?;
        let size = temporary.as_file().metadata().map_err(io_error)?.len();
        if size != remote.size {
            return Err("source changed during download (size differs); original retained".into());
        }
        temporary.as_file().sync_all().map_err(io_error)?;
        let sha256 = file_hash(temporary.path(), control)?;
        let target = input.join(&staged_relative);
        match temporary.persist_noclobber(&target) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                if file_hash(&target, control)? != sha256 {
                    return Err("staging filename is occupied by different content; no file was replaced".into());
                }
            }
            Err(error) => return Err(io_error(error.error)),
        }
        let item = StagedSource { source_id: source_id.clone(), relative: remote.relative.clone(), staged_relative, sha256 };
        journal.records.insert(source_id, Record { source: item.clone(), remote, ready: false, acknowledged: false });
        write_journal(input, journal)?;
        total = total.saturating_add(size);
        report.items.push(item);
    }
    // Previously staged inputs remain actionable even when their original was moved by an
    // external actor. A destructive acknowledgement must still verify its remote version.
    for record in journal.records.values().filter(|record| !record.acknowledged && !record.ready) {
        if report.items.iter().any(|item| item.source_id == record.source.source_id) {
            continue;
        }
        if report.items.len() >= MAX_BATCH || total.saturating_add(record.remote.size) > MAX_FILE {
            report.has_more = true;
            break;
        }
        if !report.items.iter().any(|item| item.source_id == record.source.source_id)
            && file_hash(&input.join(&record.source.staged_relative), control).is_ok_and(|value| value == record.source.sha256)
        {
            report.items.push(record.source.clone());
            total = total.saturating_add(record.remote.size);
        }
    }
    report.pending_acknowledgements =
        journal.records.values().filter(|record| record.ready && !record.acknowledged).map(|record| record.source.clone()).collect();
    let _ = config;
    Ok(report)
}

/// Call only after every output has been durably journaled. `confirmed_sha256` must be
/// the hash of the exact staged input used for those outputs. Idempotent on restart.
/// Does not remove staging files: the caller may do so only after this returns Ok.
/// For Outlook, Ok can mean this attachment is ready while another attachment still
/// needs processing; the remote message remains until the whole group is ready.
#[allow(clippy::too_many_arguments)]
pub fn acknowledge(
    config: &SourceConfig,
    input: &Path,
    item: &StagedSource,
    confirmed_sha256: &str,
    root: Option<&Path>,
    should_stop: &dyn Fn() -> bool,
    store: &CredentialStore,
) -> Result<(), String> {
    guarded(|| {
        validate(config, input, root)?;
        let input = confined(input, root)?;
        let _lock = lock(&input)?;
        let mut journal = read_journal(&input, config)?;
        let control = Control::new(should_stop);
        control.check()?;
        if !journal.records.contains_key(&item.source_id)
            && let Some(record) = archived_record(&input, &journal.config_hash, &item.source_id)?
        {
            journal.records.insert(item.source_id.clone(), record);
        }
        let record = journal.records.get_mut(&item.source_id).ok_or("source receipt is absent from the durable journal")?;
        if record.source != *item || confirmed_sha256 != record.source.sha256 {
            return Err("output receipt does not match this source version; original retained".into());
        }
        if record.acknowledged {
            return Ok(());
        }
        if !record.ready && file_hash(&input.join(&record.source.staged_relative), &control)? != confirmed_sha256 {
            return Err("staged input changed after processing; original retained".into());
        }
        record.ready = true;
        let group = record.remote.group.clone();
        write_journal(&input, &mut journal)?;
        let records = journal
            .records
            .values()
            .filter(|record| {
                !record.acknowledged
                    && if let Some(group) = &group { record.remote.group.as_ref() == Some(group) } else { record.source.source_id == item.source_id }
            })
            .cloned()
            .collect::<Vec<_>>();
        if records.iter().any(|record| !record.ready) {
            return Ok(());
        }
        if !matches!(action(config), SourceAction::Keep) {
            backend(config, root, &control, store)?.finish(&records, action(config), &control)?;
        }
        for record in records {
            if let Some(record) = journal.records.get_mut(&record.source.source_id) {
                record.acknowledged = true;
            }
        }
        write_journal(&input, &mut journal)
    })
}

fn guarded<T>(run: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
        .unwrap_or_else(|_| Err("source adapter failed unexpectedly; originals retained".into()))
}

struct Control<'a> {
    stop: &'a dyn Fn() -> bool,
    deadline: Instant,
}
impl<'a> Control<'a> {
    fn new(stop: &'a dyn Fn() -> bool) -> Self {
        Self { stop, deadline: Instant::now() + Duration::from_secs(300) }
    }
    fn check(&self) -> Result<(), String> {
        if (self.stop)() {
            Err("source operation cancelled; originals retained".into())
        } else if Instant::now() >= self.deadline {
            Err("source operation exceeded five minutes; retry a smaller source folder".into())
        } else {
            Ok(())
        }
    }
}
fn io_error(error: impl std::fmt::Display) -> String {
    format!("source I/O failed: {error}")
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}
fn relative_ok(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|part| matches!(part, Component::Normal(_)))
}
/// Reject symlink/junction components before canonicalization, even when their target
/// happens to be inside --root. Missing final components remain allowed for creation.
fn confined(path: &Path, root: Option<&Path>) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(io_error)?;
    for ancestor in absolute.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if is_link(&metadata) => return Err("source paths must not contain symbolic links or junctions".into()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    crate::hot_folder::confined(path, root)
}
fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // All reparse points are refused, including directory junctions.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn supported(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|v| v.to_str()).unwrap_or_default().to_ascii_lowercase().as_str(),
        "pdf" | "png" | "jpg" | "jpeg" | "tif" | "tiff"
    )
}
fn safe_filename(value: &str) -> String {
    let path = Path::new(value);
    let extension = path.extension().and_then(|value| value.to_str()).filter(|_| supported(path));
    let stem = if extension.is_some() { path.file_stem().and_then(|value| value.to_str()).unwrap_or(value) } else { value };
    let mut result = String::new();
    for character in stem.chars() {
        if result.len().saturating_add(character.len_utf8()) > 148 {
            break;
        }
        result.push(if character.is_control() || matches!(character, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            '_'
        } else {
            character
        });
    }
    let mut result = result.trim_matches([' ', '.']).to_owned();
    if result.is_empty() {
        result.push_str("source");
    }
    if let Some(extension) = extension {
        result.push('.');
        result.push_str(&extension.to_ascii_lowercase());
    }
    result
}
fn file_hash(path: &Path, control: &Control<'_>) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if !metadata.is_file() || is_link(&metadata) || metadata.len() > MAX_FILE {
        return Err("source must be a regular non-link file of at most 512 MiB".into());
    }
    let mut file = File::open(path).map_err(io_error)?;
    let mut digest = Sha256::new();
    let mut bytes = [0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        control.check()?;
        let n = file.read(&mut bytes).map_err(io_error)?;
        if n == 0 {
            break;
        }
        size = size.saturating_add(n as u64);
        if size > MAX_FILE {
            return Err("source grew beyond 512 MiB".into());
        }
        digest.update(bytes.get(..n).ok_or("invalid read size")?);
    }
    Ok(digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}
fn copy_limited(reader: &mut dyn Read, writer: &mut dyn Write, limit: u64, control: &Control<'_>) -> Result<u64, String> {
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        control.check()?;
        let n = reader.read(&mut buffer).map_err(|_| "source transfer failed or timed out; partial download was discarded".to_owned())?;
        if n == 0 {
            return Ok(total);
        }
        total = total.saturating_add(n as u64);
        if total > limit {
            return Err("source download exceeds its permitted size; partial download was discarded".into());
        }
        writer.write_all(buffer.get(..n).ok_or("invalid transfer size")?).map_err(io_error)?;
    }
}
fn plain_file(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !is_link(&meta) => Ok(()),
        Ok(_) => Err("source state must be a regular file, never a link".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}
fn lock(input: &Path) -> Result<File, String> {
    let path = input.join(".pdfcraft-source.lock");
    plain_file(&path)?;
    let file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path).map_err(io_error)?;
    file.try_lock().map_err(|_| "another worker is processing this source inbox".to_owned())?;
    Ok(file)
}
fn read_journal(input: &Path, config: &SourceConfig) -> Result<Journal, String> {
    let path = input.join(STATE_FILE);
    plain_file(&path)?;
    let config_hash = hash(&serde_json::to_vec(config).map_err(io_error)?);
    let journal: Journal = if let Some(pending) = crate::hot_folder::journal::pending(&path)? {
        pending
    } else {
        if !path.exists() {
            return Ok(Journal { config_hash, ..Default::default() });
        }
        let bytes = read_bounded(&path, MAX_JSON)?;
        serde_json::from_slice(&bytes).map_err(|_| "source journal is malformed; originals retained".to_owned())?
    };
    if journal.config_hash != config_hash {
        return Err("source settings changed; choose a fresh staging inbox to avoid applying receipts to another source".into());
    }
    if journal.records.len() > MAX_ITEMS {
        return Err("source journal exceeds 10000 records".into());
    }
    for (id, record) in &journal.records {
        validate_record(id, record, &journal.config_hash)?;
    }
    Ok(journal)
}
fn validate_record(id: &str, record: &Record, config_hash: &str) -> Result<(), String> {
    if id.len() != 64
        || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || id != record.source.source_id
        || id != hash(format!("{}\n{}\n{}\n{}", config_hash, record.remote.key, record.remote.revision, record.remote.size).as_bytes())
        || !relative_ok(&record.source.staged_relative)
        || record.source.staged_relative.components().count() != 1
        || record.source.sha256.len() != 64
        || !record.source.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !relative_ok(&record.source.relative)
        || record.source.relative != record.remote.relative
        || record.remote.size == 0
        || record.remote.size > MAX_FILE
    {
        return Err("source journal contains an invalid receipt path or hash".into());
    }
    Ok(())
}

fn archived_record(input: &Path, config_hash: &str, key: &str) -> Result<Option<Record>, String> {
    let record: Option<Record> = crate::hot_folder::journal::read(&input.join(STATE_FILE), "records", key)?;
    if let Some(record) = &record {
        validate_record(key, record, config_hash)?;
    }
    Ok(record)
}

fn archived_relative(input: &Path, config_hash: &str, relative: &Path) -> Result<Option<Record>, String> {
    let record: Option<Record> =
        crate::hot_folder::journal::read(&input.join(STATE_FILE), "paths", &format!("{config_hash}:{}", relative.display()))?;
    if let Some(record) = &record {
        validate_record(&record.source.source_id, record, config_hash)?;
    }
    Ok(record)
}

fn write_journal(input: &Path, journal: &mut Journal) -> Result<(), String> {
    let completed: Vec<_> = journal.records.iter().filter(|(_, record)| record.acknowledged).map(|(key, _)| key.clone()).collect();
    let count = completed.len().saturating_sub(256);
    let target = input.join(STATE_FILE);
    if count > 0 {
        crate::hot_folder::journal::begin(&target, journal)?;
    }
    for key in completed.into_iter().take(count) {
        let record = journal.records.get(&key).ok_or("source history changed during compaction")?;
        crate::hot_folder::journal::write(&input.join(STATE_FILE), "records", &key, record)?;
        crate::hot_folder::journal::write(
            &input.join(STATE_FILE),
            "paths",
            &format!("{}:{}", journal.config_hash, record.remote.relative.display()),
            record,
        )?;
        journal.records.remove(&key);
    }
    let target = input.join(STATE_FILE);
    plain_file(&target)?;
    let bytes = serde_json::to_vec(journal).map_err(io_error)?;
    if bytes.len() as u64 > MAX_JSON {
        return Err("source journal exceeds 8 MiB".into());
    }
    let mut temporary = tempfile::Builder::new().prefix(".pdfcraft-source-state-").tempfile_in(input).map_err(io_error)?;
    temporary.write_all(&bytes).and_then(|_| temporary.as_file().sync_all()).map_err(io_error)?;
    temporary.persist(&target).map_err(|error| io_error(error.error))?;
    sync_directory(input)?;
    crate::hot_folder::journal::finish(&target)
}

/// A synced temporary file does not make its renamed directory entry durable on Unix.
/// Confirm the journal/publication before permitting a destructive source action.
fn sync_directory(_path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(_path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "cannot confirm source directory persistence; no further original-file action was performed".to_owned())?;
    Ok(())
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    plain_file(path)?;
    let mut bytes = Vec::new();
    File::open(path).map_err(io_error)?.take(limit.saturating_add(1)).read_to_end(&mut bytes).map_err(io_error)?;
    if bytes.len() as u64 > limit {
        return Err("source metadata exceeds its size limit".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_names_keep_extensions_and_project_missing_entries_never_silently_disappear() {
        let name = safe_filename(&format!("{}.PDF", "頁".repeat(200)));
        assert!(name.len() <= 160);
        assert!(name.ends_with(".pdf"));
        assert!(relative_ok(Path::new(&name)));
        let (root, config, inbox, originals) = fixture(SourceAction::Keep);
        assert_eq!(local_paths(&config, Some(root.path())).unwrap().len(), 2);
        fs::remove_file(originals.join("chapter/one.pdf")).unwrap();
        assert!(stage(&config, &inbox, Some(root.path()), &|| false, &CredentialStore::OsVault).unwrap_err().contains("missing, unprocessed"));
    }

    #[test]
    fn project_restart_preserves_manifest_order_for_missing_but_staged_originals() {
        let (root, config, inbox, originals) = fixture(SourceAction::Keep);
        let first = stage(&config, &inbox, Some(root.path()), &|| false, &CredentialStore::OsVault).unwrap();
        fs::remove_file(originals.join("chapter/one.pdf")).unwrap();
        let resumed = stage(&config, &inbox, Some(root.path()), &|| false, &CredentialStore::OsVault).unwrap();
        assert_eq!(first.items, resumed.items);
    }

    #[test]
    fn corrupt_receipts_cannot_repoint_a_confirmed_source() {
        let (root, config, inbox, _) = fixture(SourceAction::Delete);
        let report = stage(&config, &inbox, Some(root.path()), &|| false, &CredentialStore::OsVault).unwrap();
        let path = inbox.join(STATE_FILE);
        let mut state: Journal = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        state.records.get_mut(&report.items[0].source_id).unwrap().remote.key.push_str("-different");
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(
            acknowledge(&config, &inbox, &report.items[0], &report.items[0].sha256, Some(root.path()), &|| false, &CredentialStore::OsVault).is_err()
        );
    }
    fn fixture(action: SourceAction) -> (tempfile::TempDir, SourceConfig, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("originals");
        let inbox = root.path().join("inbox");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&inbox).unwrap();
        fs::create_dir(source.join("chapter")).unwrap();
        fs::write(source.join("chapter/one.pdf"), b"synthetic PDF source bytes").unwrap();
        fs::write(source.join("two.png"), b"synthetic image source bytes").unwrap();
        let manifest = source.join("project.pdfcraft.json");
        write_project_manifest(
            &manifest,
            &ProjectManifest { files: vec!["chapter/one.pdf".into(), "two.png".into()], ..Default::default() },
            Some(root.path()),
        )
        .unwrap();
        (root, SourceConfig::Project(ProjectSource { manifest, action }), inbox, source)
    }
    #[test]
    fn project_staging_is_atomic_durable_and_hash_bound_before_deleting_originals() {
        let (root, config, inbox, originals) = fixture(SourceAction::Delete);
        let store = CredentialStore::OsVault;
        let report = stage(&config, &inbox, Some(root.path()), &|| false, &store).unwrap();
        assert_eq!(report.items.len(), 2);
        assert_eq!(report.items[0].relative, PathBuf::from("chapter/one.pdf"));
        assert!(originals.join("chapter/one.pdf").exists());
        let resumed = stage(&config, &inbox, Some(root.path()), &|| false, &store).unwrap();
        assert_eq!(report.items, resumed.items);
        let first = &report.items[0];
        assert!(acknowledge(&config, &inbox, first, "wrong output input hash", Some(root.path()), &|| false, &store).is_err());
        assert!(originals.join("chapter/one.pdf").exists());
        acknowledge(&config, &inbox, first, &first.sha256, Some(root.path()), &|| false, &store).unwrap();
        assert!(!originals.join("chapter/one.pdf").exists());
        acknowledge(&config, &inbox, first, &first.sha256, Some(root.path()), &|| false, &store).unwrap();
        fs::remove_file(inbox.join(&first.staged_relative)).unwrap();
        assert_eq!(stage(&config, &inbox, Some(root.path()), &|| false, &store).unwrap().items.len(), 1);
    }
    #[test]
    fn changed_originals_and_cancelled_operations_are_retained_and_recoverable() {
        let (root, config, inbox, originals) = fixture(SourceAction::Delete);
        let store = CredentialStore::OsVault;
        assert!(stage(&config, &inbox, Some(root.path()), &|| true, &store).is_err());
        assert_eq!(
            fs::read_dir(&inbox).unwrap().filter_map(Result::ok).filter(|item| item.path().extension().is_some_and(|ext| ext == "pdf")).count(),
            0
        );
        let report = stage(&config, &inbox, Some(root.path()), &|| false, &store).unwrap();
        let first = &report.items[0];
        fs::write(originals.join("chapter/one.pdf"), b"changed after OCR").unwrap();
        assert!(acknowledge(&config, &inbox, first, &first.sha256, Some(root.path()), &|| false, &store).is_err());
        assert!(originals.join("chapter/one.pdf").exists());
        let resumed = stage(&config, &inbox, Some(root.path()), &|| false, &store).unwrap();
        assert_eq!(resumed.pending_acknowledgements.len(), 1);
        assert!(!resumed.items.iter().any(|item| item.source_id == first.source_id));
    }
    #[test]
    fn project_archive_is_no_clobber_and_configuration_and_path_escapes_fail() {
        let (root, mut config, inbox, originals) = fixture(SourceAction::Keep);
        let archive = root.path().join("archive");
        fs::create_dir(&archive).unwrap();
        if let SourceConfig::Project(source) = &mut config {
            source.action = SourceAction::Move { destination: archive.to_string_lossy().into_owned() };
        }
        let store = CredentialStore::OsVault;
        let report = stage(&config, &inbox, Some(root.path()), &|| false, &store).unwrap();
        let item = &report.items[0];
        fs::write(archive.join(&item.staged_relative), b"unrelated archive").unwrap();
        assert!(acknowledge(&config, &inbox, item, &item.sha256, Some(root.path()), &|| false, &store).is_err());
        assert!(originals.join("chapter/one.pdf").exists());
        assert_eq!(fs::read(archive.join(&item.staged_relative)).unwrap(), b"unrelated archive");
        if let SourceConfig::Project(source) = &mut config {
            source.action = SourceAction::Delete;
        }
        assert!(stage(&config, &inbox, Some(root.path()), &|| false, &store).is_err());
        assert!(
            write_project_manifest(
                &originals.join("bad.json"),
                &ProjectManifest { files: vec!["../escape.pdf".into()], ..Default::default() },
                Some(root.path())
            )
            .is_err()
        );
        assert!(
            write_project_manifest(
                &originals.join("abbyy.json"),
                &ProjectManifest { format: "ABBYY".into(), files: vec!["two.png".into()], ..Default::default() },
                Some(root.path())
            )
            .is_err()
        );
    }
    struct Fake {
        count: usize,
        fail: bool,
    }
    impl Backend for Fake {
        fn list(&mut self, _: &Control<'_>) -> Result<Vec<RemoteFile>, String> {
            Ok((0..self.count)
                .map(|i| RemoteFile { key: format!("{i}"), relative: format!("{i}.pdf").into(), revision: "1".into(), size: 3, group: None })
                .collect())
        }
        fn read(&mut self, _: &RemoteFile, writer: &mut dyn Write, _: &Control<'_>) -> Result<(), String> {
            writer.write_all(b"abc").unwrap();
            if self.fail { Err("network disconnected".into()) } else { Ok(()) }
        }
        fn finish(&mut self, _: &[Record], _: &SourceAction, _: &Control<'_>) -> Result<(), String> {
            Ok(())
        }
    }
    #[test]
    fn bounded_queues_signal_incomplete_batches_and_failed_downloads_never_publish() {
        let dir = tempfile::tempdir().unwrap();
        let mut journal = Journal { config_hash: "test".into(), ..Default::default() };
        let mut fake = Fake { count: 1, fail: true };
        let control = Control::new(&|| false);
        assert!(stage_with(&mut fake, &SourceConfig::Local, dir.path(), &mut journal, &control).is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        fake.fail = false;
        fake.count = 129;
        let report = stage_with(&mut fake, &SourceConfig::Local, dir.path(), &mut journal, &control).unwrap();
        assert_eq!(report.items.len(), 128);
        assert!(report.has_more);
        for record in journal.records.values_mut() {
            record.ready = true;
        }
        let second = stage_with(&mut fake, &SourceConfig::Local, dir.path(), &mut journal, &control).unwrap();
        assert_eq!(second.items.len(), 1);
        assert!(!second.has_more);
        assert_eq!(second.pending_acknowledgements.len(), 128);
    }
    #[test]
    fn completed_remote_history_rolls_over_and_still_deduplicates_without_downloading() {
        let dir = tempfile::tempdir().unwrap();
        let config = SourceConfig::Local;
        let mut journal = read_journal(dir.path(), &config).unwrap();
        let mut fake = Fake { count: 300, fail: false };
        let control = Control::new(&|| false);
        for _ in 0..3 {
            let report = stage_with(&mut fake, &config, dir.path(), &mut journal, &control).unwrap();
            assert!(!report.items.is_empty());
            for item in report.items {
                let record = journal.records.get_mut(&item.source_id).unwrap();
                record.ready = true;
                record.acknowledged = true;
                fs::remove_file(dir.path().join(&item.staged_relative)).unwrap();
            }
            write_journal(dir.path(), &mut journal).unwrap();
        }
        assert_eq!(journal.records.len(), 256);
        let mut restarted = read_journal(dir.path(), &config).unwrap();
        fake.fail = true; // Any attempted download now fails this assertion.
        assert!(stage_with(&mut fake, &config, dir.path(), &mut restarted, &control).unwrap().items.is_empty());
    }

    #[test]
    fn archived_project_receipts_allow_deleted_originals_but_reject_modified_binding() {
        let (root, config, inbox, _) = fixture(SourceAction::Delete);
        let store = CredentialStore::OsVault;
        let report = stage(&config, &inbox, Some(root.path()), &|| false, &store).unwrap();
        let item = &report.items[0];
        acknowledge(&config, &inbox, item, &item.sha256, Some(root.path()), &|| false, &store).unwrap();
        let mut journal = read_journal(&inbox, &config).unwrap();
        let record = journal.records.remove(&item.source_id).unwrap();
        crate::hot_folder::journal::write(&inbox.join(STATE_FILE), "records", &item.source_id, &record).unwrap();
        crate::hot_folder::journal::write(
            &inbox.join(STATE_FILE),
            "paths",
            &format!("{}:{}", journal.config_hash, record.remote.relative.display()),
            &record,
        )
        .unwrap();
        write_journal(&inbox, &mut journal).unwrap();
        assert_eq!(stage(&config, &inbox, Some(root.path()), &|| false, &store).unwrap().items.len(), 1);
        acknowledge(&config, &inbox, item, &item.sha256, Some(root.path()), &|| false, &store).unwrap();
        let mut forged = record;
        forged.remote.key.push_str("changed");
        crate::hot_folder::journal::write(&inbox.join(STATE_FILE), "records", &item.source_id, &forged).unwrap();
        assert!(acknowledge(&config, &inbox, item, &item.sha256, Some(root.path()), &|| false, &store).is_err());
    }
    #[test]
    fn task_json_and_debug_never_contain_credentials() {
        let config = SourceConfig::Ftp(FtpSource {
            host: "example.test".into(),
            username: "scan-user".into(),
            credential_id: "ftp-scanner".into(),
            ..Default::default()
        });
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("password"));
        assert!(!format!("{config:?}").contains("password"));
        assert!(serde_json::from_str::<SourceConfig>(&json.replace("\"host\"", "\"password\":\"secret\",\"host\"")).is_err());
        assert!(matches!(serde_json::from_str::<SourceConfig>("{\"kind\":\"local\"}").unwrap(), SourceConfig::Local));
    }
}
