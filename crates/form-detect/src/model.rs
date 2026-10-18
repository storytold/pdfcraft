//! Optional, versioned model storage. Only `install` accesses the network, and callers must
//! obtain the user's download consent first. Inference and status checks stay offline.

use crate::{Error, MODEL_FILE, MODEL_SHA256};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MODEL_BYTES: u64 = 120_762_528;
pub const RELEASE_TAG: &str = "models-ffdetr-576-v1";
pub const DOWNLOAD_URL: &str = "https://github.com/storytold/pdfcraft/releases/download/models-ffdetr-576-v1/ffdetr-576.onnx";
pub const ATTRIBUTION: &str = "FFDetr by Joe Barrow, licensed under Apache-2.0.\nOriginal: https://huggingface.co/jbarrow/FFDetr/tree/56f4e4235e28dcb2953513dc020bb191a2f54cfe\nConverted to self-contained ONNX (576px, float32, opset 17) for PdfCraft.\nCheckpoint SHA-256: f852e1bac18c8f435b82270fc8ff8e2ca4a2cd8869c411fa8f473f16e69585ef\nExport SHA-256: 5e5920c8cbf7287b9fef4d67c0d20728a7155e884ae638ee75926b1bbae79370\n";
#[cfg(not(target_arch = "wasm32"))]
const LICENSE: &str = include_str!("../../../LICENSE-APACHE");

/// Per-user model data, shared across app versions. No directories are created by this call.
pub fn directory() -> Result<PathBuf, Error> {
    let absolute = |key| std::env::var_os(key).map(PathBuf::from).filter(|p| p.is_absolute());
    #[cfg(target_os = "windows")]
    let base = absolute("LOCALAPPDATA").map(|p| p.join("PdfCraft"));
    #[cfg(target_os = "macos")]
    let base = absolute("HOME").map(|p| p.join("Library/Application Support/PdfCraft"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = absolute("XDG_DATA_HOME").or_else(|| absolute("HOME").map(|p| p.join(".local/share"))).map(|p| p.join("pdfcraft"));
    base.map(|p| p.join("models/ffdetr")).ok_or_else(|| Error::Model("cannot find your application data directory".into()))
}

/// Streaming verification bounds memory even if a cache file was replaced or damaged.
pub fn available(dir: &Path) -> Result<bool, Error> {
    let path = dir.join(MODEL_FILE);
    // Check file type before opening: a named pipe must not block a status check forever.
    match std::fs::metadata(&path) {
        Ok(metadata) if !metadata.is_file() => return Ok(false),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    }
    let mut file = std::fs::File::open(&path)?;
    if !file.metadata()?.is_file() {
        return Ok(false);
    }
    let length = file.metadata()?.len();
    #[cfg(feature = "test-model")]
    if length == crate::test_model::bytes().len() as u64 {
        let mut bytes = Vec::new();
        file.by_ref().take(length + 1).read_to_end(&mut bytes)?;
        return Ok(bytes == crate::test_model::bytes());
    }
    if length != MODEL_BYTES {
        return Ok(false);
    }
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        count = count.saturating_add(read as u64);
        if count > MODEL_BYTES {
            return Ok(false);
        }
        hash.update(buffer.get(..read).ok_or_else(|| Error::Model("invalid model read length".into()))?);
    }
    Ok(count == MODEL_BYTES && hex(&hash.finalize()) == MODEL_SHA256)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Download the one supported artifact to a staging file, check its length and SHA-256,
/// and publish it atomically. Cancellation or any failure leaves the previous model intact.
/// Reusing a verified model repairs its licence notices without accessing the network.
/// `progress` returns false to cancel and receives (bytes received, total bytes).
#[cfg(not(target_arch = "wasm32"))]
pub fn install(dir: &Path, mut progress: impl FnMut(u64, u64) -> bool) -> Result<PathBuf, Error> {
    if !progress(0, MODEL_BYTES) {
        return Err(Error::Cancelled);
    }
    if available(dir)? {
        install_notices(dir)?;
        return Ok(dir.join(MODEL_FILE));
    }
    let found = rustls_native_certs::load_native_certs();
    let certs = found.certs.iter().map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned()).collect::<Vec<_>>();
    if certs.is_empty() {
        return Err(Error::Download("no trusted certificates found on this system".into()));
    }
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30 * 60)))
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(20)))
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::new_with_certs(&certs)).build())
        .build()
        .new_agent();
    let mut response = agent
        .get(DOWNLOAD_URL)
        .header("User-Agent", concat!("PdfCraft/", env!("CARGO_PKG_VERSION")))
        .config()
        .http_status_as_error(false)
        .https_only(true)
        .build()
        .call()
        .map_err(|e| Error::Download(format!("couldn't reach the download service ({e}); check your connection and try again")))?;
    let status = response.status().as_u16();
    if status == 404 {
        return Err(Error::Download("the form detection download has not been published yet; please try again after it becomes available".into()));
    }
    if status != 200 {
        return Err(Error::Download(format!("the download service returned HTTP {status}; please try again later")));
    }
    if response.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok()).is_some_and(|n| n != MODEL_BYTES) {
        return Err(Error::Download("the download has an unexpected size; please try again later".into()));
    }
    let reader = response.body_mut().with_config().limit(MODEL_BYTES + 1).reader();
    install_reader(dir, reader, MODEL_BYTES, MODEL_SHA256, &mut progress)
}

#[cfg(not(target_arch = "wasm32"))]
fn install_reader(
    dir: &Path,
    mut reader: impl Read,
    expected: u64,
    digest: &str,
    progress: &mut impl FnMut(u64, u64) -> bool,
) -> Result<PathBuf, Error> {
    use std::io::Write;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    let mut staging = tempfile::NamedTempFile::new_in(dir)?;
    let mut hash = Sha256::new();
    let mut done = 0u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        if !progress(done, expected) {
            return Err(Error::Cancelled);
        }
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        done =
            done.checked_add(read as u64).filter(|n| *n <= expected).ok_or_else(|| Error::Download("download exceeds its expected size".into()))?;
        let chunk = buffer.get(..read).ok_or_else(|| Error::Download("invalid download read length".into()))?;
        staging.write_all(chunk)?;
        hash.update(chunk);
    }
    if done != expected || hex(&hash.finalize()) != digest {
        return Err(Error::Download("download verification failed; please try again".into()));
    }
    staging.as_file().sync_all()?;
    // All notices are installed before the model, which is the availability marker.
    install_notices(dir)?;
    if !progress(done, expected) {
        return Err(Error::Cancelled);
    }
    let target = dir.join(MODEL_FILE);
    staging.persist(&target).map_err(|e| Error::Io(e.error))?;
    Ok(target)
}

#[cfg(not(target_arch = "wasm32"))]
fn install_notices(dir: &Path) -> Result<(), Error> {
    atomic_text(dir, "LICENSE-APACHE", LICENSE)?;
    atomic_text(dir, "ATTRIBUTION.txt", ATTRIBUTION)
}

#[cfg(not(target_arch = "wasm32"))]
fn atomic_text(dir: &Path, name: &str, text: &str) -> Result<(), Error> {
    use std::io::Write;
    let path = dir.join(name);
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() && metadata.len() == text.len() as u64 => {
            let mut content = Vec::new();
            std::fs::File::open(&path)?.take(metadata.len().saturating_add(1)).read_to_end(&mut content)?;
            if content == text.as_bytes() {
                return Ok(());
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(text.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| Error::Io(e.error))?;
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub fn install(_dir: &Path, _progress: impl FnMut(u64, u64) -> bool) -> Result<PathBuf, Error> {
    Err(Error::Download("form detection downloads are available in the desktop app".into()))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[cfg(unix)]
    #[test]
    fn correct_notices_do_not_need_a_writable_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        atomic_text(dir.path(), "LICENSE-APACHE", LICENSE).unwrap();
        atomic_text(dir.path(), "ATTRIBUTION.txt", ATTRIBUTION).unwrap();
        let permissions = std::fs::metadata(dir.path()).unwrap().permissions();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = atomic_text(dir.path(), "LICENSE-APACHE", LICENSE).and_then(|_| atomic_text(dir.path(), "ATTRIBUTION.txt", ATTRIBUTION));
        std::fs::set_permissions(dir.path(), permissions).unwrap();
        result.unwrap();
    }

    #[test]
    fn download_verifies_then_publishes_and_credits_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"synthetic original model";
        let hash = hex(&Sha256::digest(bytes));
        let path = install_reader(dir.path(), Cursor::new(bytes), bytes.len() as u64, &hash, &mut |_, _| true).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert!(std::fs::read_to_string(dir.path().join("ATTRIBUTION.txt")).unwrap().contains("Joe Barrow"));
        assert_eq!(std::fs::read_to_string(dir.path().join("LICENSE-APACHE")).unwrap(), LICENSE);
    }
    #[test]
    fn corrupt_truncated_oversized_and_cancelled_downloads_keep_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(MODEL_FILE);
        std::fs::write(&target, b"old model").unwrap();
        let bytes = vec![7; 100_000];
        let hash = hex(&Sha256::digest(&bytes));
        for (input, size, cancel) in
            [(&bytes[..], bytes.len() as u64, true), (&bytes[..], 10, false), (&bytes[..10], 100_000, false), (&b"wrong"[..], 5, false)]
        {
            assert!(install_reader(dir.path(), Cursor::new(input), size, &hash, &mut |done, _| !cancel || done == 0).is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"old model");
            assert!(std::fs::read_dir(dir.path()).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".tmp")));
        }
    }
    #[test]
    fn status_does_not_accept_corrupt_files_or_fetch_anything() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!available(dir.path()).unwrap());
        std::fs::write(dir.path().join(MODEL_FILE), b"corrupt").unwrap();
        assert!(!available(dir.path()).unwrap());
        assert!(matches!(install(dir.path(), |_, _| false), Err(Error::Cancelled)));
    }
}
