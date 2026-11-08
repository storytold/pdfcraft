//! Explicitly installed OCR resources. Reading status and OCR itself never use the network.
//!
//! Installation is blocking: GUI callers must run it on their worker thread. Both files
//! are downloaded into a private staging directory, size/hash checked, then published by
//! one directory rename. A failed or cancelled download never becomes an installed add-on.

use sha2::{Digest, Sha256};
use std::io::Read;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::Ordering;

pub const ID: &str = "historical";
pub const VERSION: &str = "1";
pub const DIRECTORY: &str = "historical-v1";
const OWNER_FILE: &str = "PDFCRAFT-ADDON.txt";
const OWNER: &[u8] = b"PdfCraft optional OCR add-on\nhistorical-v1\n";
#[cfg(not(target_arch = "wasm32"))]
const LICENCE: &str = include_str!("../../../LICENSE-APACHE");
#[cfg(not(target_arch = "wasm32"))]
const NOTICE: &str = include_str!("../../../docs/historical-ocr-model.txt");

#[derive(Clone, Copy)]
struct Resource {
    file: &'static str,
    #[cfg(not(target_arch = "wasm32"))]
    url: &'static str,
    bytes: u64,
    sha256: &'static str,
}

const RESOURCES: [Resource; 2] = [
    Resource {
        file: "historical-recognition.onnx",
        #[cfg(not(target_arch = "wasm32"))]
        url: "https://github.com/lukashold/pdfcraft/releases/download/ocr-historical-v1/historical-recognition.onnx",
        bytes: 12_974_398,
        sha256: "10904178b6ecf2dffe26d8e3f5873cd396929829e6e87e3c983d88ae2060349a",
    },
    Resource {
        file: "historical-dictionary.json",
        #[cfg(not(target_arch = "wasm32"))]
        url: "https://github.com/lukashold/pdfcraft/releases/download/ocr-historical-v1/historical-dictionary.json",
        bytes: 10_487,
        sha256: "dc2070c479e38ccdce553fbfc96af26e6824333dcc969b8855875acd54f85fde",
    },
];

/// Verified status, suitable for caching in the OCR dialog (hashing is intentionally not per frame).
#[derive(Clone, Debug, serde::Serialize)]
pub struct AddonStatus {
    pub id: &'static str,
    pub title: &'static str,
    pub version: &'static str,
    pub licence: &'static str,
    pub download_bytes: u64,
    pub installed: bool,
    pub supported: bool,
    /// `installed`, `missing`, `damaged`, or `unavailable`.
    pub state: &'static str,
    pub detail: Option<String>,
    pub directory: Option<PathBuf>,
}

fn summary(directory: Option<PathBuf>, state: &'static str, detail: Option<String>) -> AddonStatus {
    AddonStatus {
        id: ID,
        title: "Historical print and handwriting",
        version: VERSION,
        licence: "Apache-2.0",
        download_bytes: RESOURCES.iter().map(|r| r.bytes).sum(),
        installed: state == "installed",
        supported: !cfg!(target_arch = "wasm32"),
        state,
        detail,
        directory,
    }
}

/// Per-user resources, separate from the bundled modern OCR models. No directory is created.
/// `PDFCRAFT_OCR_ADDONS` supplies an explicit resource-root override for managed installations.
pub fn default_dir() -> Result<PathBuf, String> {
    if cfg!(target_arch = "wasm32") {
        return Err("historical OCR add-ons require the desktop application".into());
    }
    let env_path = |name| std::env::var_os(name).filter(|s| !s.is_empty()).map(PathBuf::from);
    let root = if let Some(root) = env_path("PDFCRAFT_OCR_ADDONS") {
        root
    } else if cfg!(target_os = "windows") {
        env_path("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable; cannot locate the OCR add-on folder")?.join("PdfCraft/OcrAddons")
    } else if cfg!(target_os = "macos") {
        env_path("HOME").ok_or("HOME is unavailable; cannot locate the OCR add-on folder")?.join("Library/Application Support/PdfCraft/OcrAddons")
    } else {
        env_path("XDG_DATA_HOME")
            .or_else(|| env_path("HOME").map(|h| h.join(".local/share")))
            .ok_or("the user data folder is unavailable; cannot locate OCR add-ons")?
            .join("pdfcraft/ocr-addons")
    };
    if !root.is_absolute() {
        return Err("the OCR add-on root must be an absolute path".into());
    }
    Ok(root.join(DIRECTORY))
}

/// Return the verified historical model folder, or explain how to install/repair it.
pub fn historical_dir() -> Result<PathBuf, String> {
    let dir = default_dir()?;
    verify(&dir, &RESOURCES)?;
    Ok(dir)
}

pub fn status() -> AddonStatus {
    match default_dir() {
        Ok(dir) => status_at(&dir),
        Err(error) => summary(None, "unavailable", Some(error)),
    }
}

/// Verify the two pinned files and ownership marker without downloading anything.
pub fn status_at(dir: &Path) -> AddonStatus {
    if cfg!(target_arch = "wasm32") {
        return summary(None, "unavailable", Some("historical OCR add-ons require the desktop application".into()));
    }
    match verify(dir, &RESOURCES) {
        Ok(()) => summary(Some(dir.to_owned()), "installed", None),
        Err(error) => {
            let state = match std::fs::symlink_metadata(dir) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => "missing",
                _ => "damaged",
            };
            summary(Some(dir.to_owned()), state, Some(error))
        }
    }
}

fn io_error(context: &str, error: impl std::fmt::Display) -> String {
    format!("{context}: {error}")
}

fn checked_parent(dir: &Path) -> Result<PathBuf, String> {
    if !dir.is_absolute()
        || dir.file_name().is_none_or(|name| name != DIRECTORY)
        || dir.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(format!("the add-on path must be an absolute path ending in {DIRECTORY}, without . or .."));
    }
    let parent = dir.parent().ok_or("the add-on folder has no parent")?;
    parent.canonicalize().map_err(|e| io_error("cannot resolve the OCR add-on parent folder", e))
}

fn regular(path: &Path) -> Result<std::fs::Metadata, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| io_error(&format!("cannot read {}", path.display()), e))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!("{} must be a regular file, not a link", path.display()));
    }
    Ok(metadata)
}

fn owned(dir: &Path) -> Result<PathBuf, String> {
    let parent = checked_parent(dir)?;
    let metadata = std::fs::symlink_metadata(dir).map_err(|e| io_error("historical OCR is not installed; install the optional add-on", e))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || dir.canonicalize().map_err(|e| io_error("cannot resolve add-on", e))? != parent.join(DIRECTORY)
    {
        return Err("refusing a linked or redirected OCR add-on folder".into());
    }
    let marker = dir.join(OWNER_FILE);
    if regular(&marker)?.len() != OWNER.len() as u64 {
        return Err("this folder is not a PdfCraft historical OCR installation; it will not be changed".into());
    }
    let mut content = Vec::with_capacity(OWNER.len().saturating_add(1));
    std::fs::File::open(&marker)
        .and_then(|f| f.take((OWNER.len() as u64).saturating_add(1)).read_to_end(&mut content))
        .map_err(|e| io_error("cannot read add-on ownership marker", e))?;
    if content != OWNER {
        return Err("this folder is not a PdfCraft historical OCR installation; it will not be changed".into());
    }
    Ok(parent)
}

fn digest_hex(hash: Sha256) -> String {
    hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_reader(reader: &mut impl Read) -> Result<String, String> {
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        let count = reader.read(&mut buffer).map_err(|e| io_error("cannot verify OCR resource", e))?;
        if count == 0 {
            break;
        }
        hash.update(buffer.get(..count).ok_or("invalid OCR resource read size")?);
    }
    Ok(digest_hex(hash))
}

fn verify(dir: &Path, resources: &[Resource]) -> Result<(), String> {
    owned(dir)?;
    for resource in resources {
        let path = dir.join(resource.file);
        if regular(&path)?.len() != resource.bytes {
            return Err(format!("{} is incomplete or damaged; remove and reinstall the add-on", resource.file));
        }
        let mut file = std::fs::File::open(&path).map_err(|e| io_error("cannot open OCR resource", e))?.take(resource.bytes.saturating_add(1));
        if hash_reader(&mut file)? != resource.sha256 {
            return Err(format!("{} failed SHA-256 verification; remove and reinstall the add-on", resource.file));
        }
    }
    Ok(())
}

pub fn install() -> Result<AddonStatus, String> {
    install_at(&default_dir()?)
}
pub fn install_at(dir: &Path) -> Result<AddonStatus, String> {
    install_at_with_cancel(dir, &AtomicBool::new(false))
}

/// Blocking explicit download; callers should use a worker and may cancel between read chunks.
#[cfg(not(target_arch = "wasm32"))]
pub fn install_at_with_cancel(dir: &Path, cancel: &AtomicBool) -> Result<AddonStatus, String> {
    cancelled(cancel)?;
    let found = rustls_native_certs::load_native_certs();
    let certs: Vec<ureq::tls::Certificate<'static>> = found.certs.iter().map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned()).collect();
    if certs.is_empty() {
        return Err("no system TLS certificates are available for the add-on download".into());
    }
    let agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(std::time::Duration::from_secs(180)))
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::new_with_certs(&certs)).build())
        .build()
        .new_agent();
    install_with(dir, &RESOURCES, cancel, |resource, destination| {
        let mut response = agent
            .get(resource.url)
            .header("User-Agent", concat!("PdfCraft/", env!("CARGO_PKG_VERSION")))
            .call()
            .map_err(|e| io_error("could not download the optional OCR add-on; check the connection and retry", e))?;
        write_verified(&mut response.body_mut().as_reader(), destination, resource, cancel)
    })?;
    Ok(status_at(dir))
}

#[cfg(target_arch = "wasm32")]
pub fn install_at_with_cancel(_dir: &Path, _cancel: &AtomicBool) -> Result<AddonStatus, String> {
    Err("historical OCR add-on downloads require the desktop application".into())
}

#[cfg(not(target_arch = "wasm32"))]
fn lock_parent(parent: &Path) -> Result<std::fs::File, String> {
    let path = parent.join(".historical-v1.lock");
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            regular(&path)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error("cannot inspect add-on lock", e)),
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| io_error("cannot open add-on lock", e))?;
    file.try_lock().map_err(|e| io_error("another process is changing the OCR add-on; try again when it finishes", e))?;
    Ok(file)
}

#[cfg(not(target_arch = "wasm32"))]
fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) { Err("the OCR add-on download was cancelled; nothing was installed".into()) } else { Ok(()) }
}

#[cfg(not(target_arch = "wasm32"))]
fn install_with(
    dir: &Path,
    resources: &[Resource],
    cancel: &AtomicBool,
    mut fetch: impl FnMut(&Resource, &Path) -> Result<(), String>,
) -> Result<(), String> {
    cancelled(cancel)?;
    // Validate the lexical boundary before creating any directory.
    if !dir.is_absolute()
        || dir.file_name().is_none_or(|name| name != DIRECTORY)
        || dir.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(format!("expected an absolute add-on path ending in {DIRECTORY}"));
    }
    let parent = dir.parent().ok_or("missing OCR add-on parent")?;
    std::fs::create_dir_all(parent).map_err(|e| io_error("cannot create the OCR add-on folder", e))?;
    let parent = checked_parent(dir)?;
    let _lock = lock_parent(&parent)?;
    if std::fs::symlink_metadata(dir).is_ok() {
        return verify(dir, resources);
    }
    let stage = tempfile::Builder::new()
        .prefix(".historical-v1-")
        .tempdir_in(&parent)
        .map_err(|e| io_error("cannot create add-on download staging folder", e))?;
    for resource in resources {
        cancelled(cancel)?;
        fetch(resource, &stage.path().join(resource.file))?;
    }
    for (name, bytes) in [(OWNER_FILE, OWNER), ("LICENSE.txt", LICENCE.as_bytes()), ("NOTICE.txt", NOTICE.as_bytes())] {
        std::fs::write(stage.path().join(name), bytes).map_err(|e| io_error("cannot write OCR add-on attribution", e))?;
    }
    cancelled(cancel)?;
    // The destination is a fixed, previously absent child of this canonical parent.
    std::fs::rename(stage.path(), parent.join(DIRECTORY)).map_err(|e| io_error("cannot publish the complete OCR add-on", e))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn write_verified(reader: &mut impl Read, destination: &Path, resource: &Resource, cancel: &AtomicBool) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|e| io_error("cannot write downloaded OCR resource", e))?;
    let mut hash = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        cancelled(cancel)?;
        let count = reader.read(&mut buffer).map_err(|e| io_error("OCR download interrupted; check the connection and retry", e))?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count as u64);
        if total > resource.bytes {
            return Err(format!("{} exceeds its expected download size; refusing it", resource.file));
        }
        let bytes = buffer.get(..count).ok_or("invalid OCR download read size")?;
        file.write_all(bytes).map_err(|e| io_error("cannot save the OCR download; check available disk space", e))?;
        hash.update(bytes);
    }
    if total != resource.bytes || digest_hex(hash) != resource.sha256 {
        return Err(format!("{} failed its size/SHA-256 check; nothing was installed", resource.file));
    }
    file.sync_all().map_err(|e| io_error("cannot finish writing the OCR add-on", e))
}

pub fn remove() -> Result<(), String> {
    remove_at(&default_dir()?)
}

/// Remove only this application's known files. Unexpected files, links or directories are
/// refused, and no recursive delete is used. Damaged model contents can still be uninstalled.
#[cfg(not(target_arch = "wasm32"))]
pub fn remove_at(dir: &Path) -> Result<(), String> {
    let parent = checked_parent(dir)?;
    let _lock = lock_parent(&parent)?;
    let canonical_parent = owned(dir)?;
    let expected = [RESOURCES[0].file, RESOURCES[1].file, "LICENSE.txt", "NOTICE.txt", OWNER_FILE];
    for entry in std::fs::read_dir(dir).map_err(|e| io_error("cannot inspect OCR add-on", e))? {
        let entry = entry.map_err(|e| io_error("cannot inspect OCR add-on file", e))?;
        if !expected.iter().any(|name| entry.file_name() == *name) {
            return Err("the OCR add-on folder contains unrelated files; nothing was removed".into());
        }
        regular(&entry.path())?;
    }
    for name in expected {
        if dir.canonicalize().map_err(|e| io_error("cannot resolve OCR add-on", e))? != canonical_parent.join(DIRECTORY) {
            return Err("the OCR add-on path changed; removal stopped".into());
        }
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error("cannot remove OCR add-on file", e)),
        }
    }
    std::fs::remove_dir(dir).map_err(|e| io_error("cannot remove the empty OCR add-on directory", e))
}

#[cfg(target_arch = "wasm32")]
pub fn remove_at(_dir: &Path) -> Result<(), String> {
    Err("historical OCR add-ons require the desktop application".into())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    const TEST: Resource = Resource {
        file: "historical-recognition.onnx",
        url: "https://example.invalid/test",
        bytes: 3,
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    };
    fn fetch(r: &Resource, path: &Path) -> Result<(), String> {
        write_verified(&mut &b"abc"[..], path, r, &AtomicBool::new(false))
    }

    #[test]
    fn install_is_complete_and_corruption_is_detected() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(DIRECTORY);
        install_with(&dir, &[TEST], &AtomicBool::new(false), fetch).unwrap();
        verify(&dir, &[TEST]).unwrap();
        std::fs::write(dir.join(TEST.file), b"bad").unwrap();
        assert!(verify(&dir, &[TEST]).unwrap_err().contains("SHA-256"));
        remove_at(&dir).unwrap();
        assert!(!dir.exists());
    }
    #[test]
    fn interrupted_or_bad_second_file_never_installs() {
        for broken in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let dir = root.path().join(DIRECTORY);
            let second = Resource { file: "historical-dictionary.json", ..TEST };
            let result = install_with(&dir, &[TEST, second], &AtomicBool::new(false), |r, p| {
                if r.file == second.file {
                    if broken { write_verified(&mut &b"bad"[..], p, r, &AtomicBool::new(false)) } else { Err("offline".into()) }
                } else {
                    fetch(r, p)
                }
            });
            assert!(result.is_err());
            assert!(!dir.exists());
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1, "only the persistent OS lock file remains");
        }
    }
    #[test]
    fn cancellation_and_oversized_download_leave_no_installation() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(DIRECTORY);
        let cancel = AtomicBool::new(false);
        let result = install_with(&dir, &[TEST], &cancel, |r, p| {
            fetch(r, p)?;
            cancel.store(true, Ordering::Relaxed);
            Ok(())
        });
        assert!(result.unwrap_err().contains("cancelled"));
        assert!(!dir.exists());
        assert!(write_verified(&mut &b"abcd"[..], &root.path().join("oversize"), &TEST, &AtomicBool::new(false)).unwrap_err().contains("exceeds"));
    }
    #[test]
    fn removal_refuses_unrelated_files_and_unowned_folders() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(DIRECTORY);
        std::fs::create_dir(&dir).unwrap();
        assert!(remove_at(&dir).is_err());
        std::fs::remove_dir(&dir).unwrap();
        install_with(&dir, &[TEST], &AtomicBool::new(false), fetch).unwrap();
        std::fs::write(dir.join("my-document.txt"), b"keep").unwrap();
        assert!(remove_at(&dir).unwrap_err().contains("unrelated"));
        assert!(dir.join(TEST.file).exists());
        assert!(remove_at(root.path()).is_err());
    }
    #[test]
    fn lock_prevents_concurrent_mutation_without_stale_lock_failures() {
        let root = tempfile::tempdir().unwrap();
        let lock = lock_parent(root.path()).unwrap();
        assert!(lock_parent(root.path()).is_err());
        drop(lock);
        assert!(lock_parent(root.path()).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn redirected_directory_is_not_read_or_removed() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let dir = root.path().join(DIRECTORY);
        std::os::unix::fs::symlink(other.path(), &dir).unwrap();
        assert!(owned(&dir).is_err());
        assert!(remove_at(&dir).is_err());
        assert!(other.path().exists());
    }
}
