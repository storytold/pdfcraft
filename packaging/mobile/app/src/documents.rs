use std::io::{Read, Write};
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, FilePath};
use tauri_plugin_fs::{FsExt, OpenOptions};

pub const MAX_DOCUMENT: usize = 64 * 1024 * 1024;

// The maintained fs plugin starts access when opening an iOS file URL. Pair it
// on every return path; exhausting iOS security-scoped handles breaks later imports.
#[cfg(target_os = "ios")]
struct ScopedFile<'a>(&'a tauri::AppHandle, FilePath);
#[cfg(target_os = "ios")]
impl Drop for ScopedFile<'_> {
    fn drop(&mut self) {
        if matches!(&self.1, FilePath::Url(url) if url.scheme() == "file") {
            let _ = self.0.fs().stop_accessing_security_scoped_resource(self.1.clone());
        }
    }
}

pub fn filename(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 240 || name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':')) || matches!(name, "." | "..") {
        return Err("Choose a document name without path separators, at most 240 UTF-8 bytes".into());
    }
    Ok(name.to_owned())
}

fn check_size(bytes: usize) -> Result<(), String> {
    if bytes == 0 || bytes > MAX_DOCUMENT {
        return Err("Mobile documents must contain 1 byte to 64 MiB".into());
    }
    Ok(())
}

pub fn name(path: &FilePath) -> String {
    path.clone()
        .into_path()
        .ok()
        .as_deref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .and_then(|n| filename(n).ok())
        .unwrap_or_else(|| "document.pdf".into())
}

pub fn read(app: &tauri::AppHandle, path: FilePath) -> Result<Vec<u8>, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "ios")]
    let _scope = ScopedFile(app, path.clone());
    let file = app.fs().open(path, options).map_err(|e| format!("Cannot open the selected document: {e}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("Cannot read the selected document: {e}"))?;
    check_size(bytes.len())?;
    Ok(bytes)
}

pub fn export(app: &tauri::AppHandle, name: &str, bytes: &[u8], origin: Option<&str>) -> Result<bool, String> {
    let name = filename(name)?;
    check_size(bytes.len())?;
    // Stage a durable recovery copy before any document-provider write. A provider
    // may not offer atomic rename, unlike a normal desktop filesystem.
    let folder = app.path().app_data_dir().map_err(|e| e.to_string())?.join("pending-exports");
    let recovered = match origin {
        Some(token) => Some(
            app.state::<crate::State>()
                .recoveries
                .lock()
                .map_err(|_| "Recovery storage is unavailable")?
                .get(token)
                .cloned()
                .ok_or("The recovery identity has expired")?,
        ),
        None => None,
    };
    let retained = stage(&folder, recovered.as_deref(), bytes)?;
    let Some(destination) = app.dialog().file().set_file_name(&name).blocking_save_file() else {
        // Cancellation did not write an external document; the editor remains dirty.
        if recovered.is_none() {
            let _ = std::fs::remove_file(&retained);
        }
        return Ok(false);
    };
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(target_os = "ios")]
    let _scope = ScopedFile(app, destination.clone());
    let write = app.fs().open(destination, options).and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()));
    write.map_err(|e| format!("Export failed; your recovery copy remains in app storage: {e}"))?;
    let _ = std::fs::remove_file(retained);
    if let (Some(token), Some(path)) = (origin, recovered) {
        app.state::<crate::State>().recoveries.lock().map_err(|_| "Recovery storage is unavailable")?.remove(token);
        app.state::<crate::State>().recoverable.lock().map_err(|_| "Recovery storage is unavailable")?.remove(&path);
    }
    Ok(true)
}

fn sync_dir(path: &std::path::Path) -> Result<(), String> {
    #[cfg(unix)]
    std::fs::File::open(path).and_then(|file| file.sync_all()).map_err(|e| format!("Cannot make the recovery directory durable: {e}"))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn create_durable_dir(path: &std::path::Path) -> Result<(), String> {
    if path.is_dir() {
        return Ok(());
    }
    let parent = path.parent().ok_or("Recovery folder has no parent")?;
    create_durable_dir(parent)?;
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {}
        Err(error) => return Err(error.to_string()),
    }
    sync_dir(path)?;
    sync_dir(parent)
}

fn published(folder: &std::path::Path, clean_partial: bool) -> Result<Vec<std::path::PathBuf>, String> {
    let mut files = Vec::new();
    for (count, entry) in std::fs::read_dir(folder).map_err(|e| e.to_string())?.enumerate() {
        if count >= 1024 {
            return Err("Recovery storage contains too many entries".into());
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        if name.starts_with("export-") && name.ends_with(".pdf") {
            files.push(entry.path());
        } else if clean_partial && name.starts_with(".writing-") {
            std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        }
    }
    Ok(files)
}

fn stage(folder: &std::path::Path, recovered: Option<&std::path::Path>, bytes: &[u8]) -> Result<std::path::PathBuf, String> {
    check_size(bytes.len())?;
    create_durable_dir(folder)?;
    if recovered.is_none() && published(folder, false)?.len() >= 8 {
        return Err("Eight pending exports are retained in app storage; recover them before starting another export".into());
    }
    // A process killed during write leaves only an unpublished .writing-* file.
    let mut draft = tempfile::Builder::new().prefix(".writing-").tempfile_in(folder).map_err(|e| e.to_string())?;
    draft.write_all(bytes).and_then(|()| draft.as_file().sync_all()).map_err(|e| e.to_string())?;
    let retained = if let Some(path) = recovered {
        // Only a path registered by this host, never a path supplied by the webview.
        let file = draft.persist(path).map_err(|e| e.to_string())?;
        drop(file);
        path.to_path_buf()
    } else {
        let random = draft.path().file_name().and_then(|name| name.to_str()).ok_or("Recovery file has no name")?.trim_start_matches(".writing-");
        let path = folder.join(format!("export-{random}.pdf"));
        let file = draft.persist_noclobber(&path).map_err(|e| e.to_string())?;
        drop(file);
        path
    };
    // Publish the complete, synced payload before opening/truncating a provider file.
    sync_dir(folder)?;
    Ok(retained)
}

/// Reopen failed PDF exports on next launch. Only regular files from our private
/// bounded spool are accepted; a foreign symlink must never grant import access.
pub fn recover(app: &tauri::AppHandle) -> Result<(), String> {
    let folder = app.path().app_data_dir().map_err(|e| e.to_string())?.join("pending-exports");
    if !folder.exists() {
        return Ok(());
    }
    let state = app.state::<crate::State>();
    let mut recovery = state.recoverable.lock().map_err(|_| "Recovery storage is unavailable")?;
    let mut queue = state.incoming.lock().map_err(|_| "The incoming-document queue is unavailable")?;
    for path in published(&folder, true)?.into_iter().take(8) {
        recovery.insert(path.clone());
        queue.push_back(FilePath::Path(path));
    }
    sync_dir(&folder)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_urls_preserve_the_decoded_unicode_document_name() {
        let file = std::env::temp_dir().join("scan résumé.pdf");
        let url = tauri::Url::from_file_path(&file).unwrap();
        assert_eq!(name(&FilePath::Url(url)), "scan résumé.pdf");
    }
    #[test]
    fn a_killed_partial_write_does_not_consume_recovery_capacity() {
        let folder = tempfile::tempdir().unwrap();
        for n in 0..8 {
            std::fs::write(folder.path().join(format!(".writing-crash-{n}")), b"partial").unwrap();
        }
        assert!(published(folder.path(), false).unwrap().is_empty());
        let saved = stage(folder.path(), None, b"complete").unwrap();
        assert_eq!(published(folder.path(), true).unwrap(), vec![saved]);
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }
    #[test]
    fn pending_export_is_durable_and_recoverable_even_at_the_storage_limit() {
        let folder = tempfile::tempdir().unwrap();
        let first = stage(folder.path(), None, b"first").unwrap();
        for _ in 1..8 {
            stage(folder.path(), None, b"pending").unwrap();
        }
        assert!(stage(folder.path(), None, b"ninth").is_err());
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        let recovered = stage(folder.path(), Some(&first), b"newer edits").unwrap();
        assert_eq!(recovered, first);
        assert_eq!(std::fs::read(&recovered).unwrap(), b"newer edits");
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 8);
        std::fs::remove_file(recovered).unwrap();
        assert!(stage(folder.path(), None, b"after successful save").is_ok());
    }
    #[test]
    fn external_document_names_cannot_escape_app_storage_or_expand_without_bound() {
        for invalid in ["", " ", "..", "../secret.pdf", "a\\b.pdf", "content:secret", "a\0.pdf"] {
            assert!(filename(invalid).is_err());
        }
        assert!(filename(&"x".repeat(241)).is_err());
        assert_eq!(filename("日本語.pdf").unwrap(), "日本語.pdf");
    }
    #[test]
    fn mobile_document_allocations_are_bounded() {
        assert!(check_size(0).is_err());
        assert!(check_size(MAX_DOCUMENT + 1).is_err());
        assert!(check_size(MAX_DOCUMENT).is_ok());
    }
}
