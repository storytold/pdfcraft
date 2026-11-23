//! The settings file (`app.ron`), read and written as eframe does it.
//!
//! eframe's own file storage is private to its GPU integrations. This one keeps the same file: a
//! RON map from keys to strings, so settings saved by either renderer are read by the other.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub(crate) struct FileStore {
    path: PathBuf,
    values: HashMap<String, String>,
    dirty: bool,
}

impl FileStore {
    /// The settings in `path`; none when there is no such file or it can't be read as settings.
    pub(crate) fn open(path: PathBuf) -> Self {
        let values = match read(&path) {
            Ok(values) => values,
            Err(e) => {
                if path.exists() {
                    log::warn!("settings in {} were not read: {e}", path.display());
                }
                HashMap::new()
            }
        };
        Self { path, values, dirty: false }
    }
}

fn read(path: &Path) -> Result<HashMap<String, String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    ron::from_str(&text).map_err(|e| e.to_string())
}

/// Writes `values` beside `path` and moves the new file over it, so a failed write (a full disk)
/// leaves the old settings.
fn write(path: &Path, values: &HashMap<String, String>) -> Result<(), String> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    }
    let text = ron::ser::to_string_pretty(values, ron::ser::PrettyConfig::default()).map_err(|e| e.to_string())?;
    let staged = staged_path(path, std::process::id());
    let written = std::fs::File::create(&staged).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    });
    let moved = written.and_then(|()| std::fs::rename(&staged, path));
    if moved.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    moved.map_err(|e| e.to_string())
}

/// Where process `process` stages a new `path`. Each process has its own, so two windows saving at
/// once (a second instance that didn't hand its files on) never write into one file and move a
/// mixture over the settings; the last whole file to be moved wins.
fn staged_path(path: &Path, process: u32) -> PathBuf {
    let mut staged = path.as_os_str().to_owned();
    staged.push(format!(".{process}.tmp"));
    PathBuf::from(staged)
}

impl eframe::Storage for FileStore {
    fn get_string(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }

    fn set_string(&mut self, key: &str, value: String) {
        if self.values.get(key) != Some(&value) {
            self.values.insert(key.to_owned(), value);
            self.dirty = true;
        }
    }

    fn remove_string(&mut self, key: &str) {
        self.dirty |= self.values.remove(key).is_some();
    }

    fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        match write(&self.path, &self.values) {
            Ok(()) => self.dirty = false,
            // Still dirty: the next save tries again.
            Err(e) => log::warn!("settings were not saved to {}: {e}", self.path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::Storage as _;

    #[test]
    fn settings_survive_a_round_trip_and_a_file_that_is_not_settings() {
        let folder = std::env::temp_dir().join(format!("pdfcraft-soft-store-{}", std::process::id()));
        let path = folder.join("nested").join("app.ron");
        let mut store = FileStore::open(path.clone());
        assert_eq!(store.get_string("pdfcraft"), None);
        store.set_string("pdfcraft", "{\"a\": \"quoted \\\" and\\nnewline\"}".to_owned());
        store.set_string("window", "(x: 1)".to_owned());
        store.flush();
        let mut again = FileStore::open(path.clone());
        assert_eq!(again.get_string("pdfcraft").as_deref(), Some("{\"a\": \"quoted \\\" and\\nnewline\"}"));
        again.remove_string("window");
        again.flush();
        assert_eq!(FileStore::open(path.clone()).get_string("window"), None);
        assert!(!staged_path(&path, std::process::id()).exists(), "the staged copy was moved, not left");

        std::fs::write(&path, "not ron at all {{{").expect("write");
        let mut broken = FileStore::open(path.clone());
        assert_eq!(broken.get_string("pdfcraft"), None, "unreadable settings start empty, they don't stop the app");
        broken.set_string("pdfcraft", "x".to_owned());
        broken.flush();
        assert_eq!(FileStore::open(path).get_string("pdfcraft").as_deref(), Some("x"));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn two_processes_stage_their_settings_in_different_files() {
        let path = Path::new("PdfCraftData").join("app.ron");
        let (one, other) = (staged_path(&path, 100), staged_path(&path, 200));
        assert_ne!(one, other);
        assert_eq!(one.parent(), path.parent(), "staged beside the settings, so moving it is a rename");
    }
}
