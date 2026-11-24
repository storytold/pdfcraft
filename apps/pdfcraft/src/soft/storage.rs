//! The settings file (`app.ron`), read and written as eframe does it.
//!
//! eframe's own file storage is private to its GPU integrations. This one keeps the same file: a
//! RON map from keys to strings, so settings saved by either renderer are read by the other.
//!
//! Saving happens on a thread of its own, as in eframe: an autosave while scrolling doesn't wait
//! for the disk (a roaming profile can be slow). One write runs at a time, so saves land in order,
//! and the last one is waited for when the store is dropped.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

/// Reads of a settings file that exists but isn't settings: another window may be writing it.
const READ_ATTEMPTS: u32 = 4;
const READ_RETRY: Duration = Duration::from_millis(50);

pub(crate) struct FileStore {
    path: PathBuf,
    values: HashMap<String, String>,
    /// Changes not yet handed to a write.
    dirty: bool,
    /// The write in progress, if any.
    writing: Option<JoinHandle<Result<(), String>>>,
    /// The file couldn't be read nor copied aside: nothing is saved over it.
    read_only: bool,
}

impl FileStore {
    /// The settings in `path`; none when there is no such file or it can't be read as settings.
    pub(crate) fn open(path: PathBuf) -> Self {
        Self::open_keeping(path, |from, to| std::fs::copy(from, to).map(drop))
    }

    /// As [`Self::open`], with `keep` copying an unreadable file aside.
    fn open_keeping(path: PathBuf, keep: impl FnOnce(&Path, &Path) -> std::io::Result<()>) -> Self {
        let mut read_only = false;
        let values = match read_settled(&path, READ_ATTEMPTS, READ_RETRY) {
            Ok(values) => values,
            Err(e) => {
                if path.exists() {
                    // Kept before anything is saved over it, so what it held can be recovered.
                    let kept = sibling(&path, ".unreadable");
                    match keep(&path, &kept) {
                        Ok(()) => log::warn!("settings in {} were not read: {e}; a copy is in {}", path.display(), kept.display()),
                        Err(copy) => {
                            log::warn!(
                                "settings in {} were not read: {e}; they couldn't be copied aside ({copy}), so nothing is saved over them",
                                path.display()
                            );
                            read_only = true;
                        }
                    }
                }
                HashMap::new()
            }
        };
        Self { path, values, dirty: false, writing: None, read_only }
    }

    /// Waits for the write in progress, and takes its result: a failed write leaves the changes to
    /// be saved again.
    fn settle(&mut self) {
        let Some(writing) = self.writing.take() else { return };
        let result = writing.join().unwrap_or_else(|_| Err("the writer stopped".to_owned()));
        if let Err(e) = result {
            log::warn!("settings were not saved to {}: {e}", self.path.display());
            self.dirty = true;
        }
    }
}

impl Drop for FileStore {
    /// The last save is waited for, and whatever changed after it is written before the app ends.
    fn drop(&mut self) {
        self.settle();
        if self.dirty
            && !self.read_only
            && let Err(e) = write(&self.path, &self.values)
        {
            log::warn!("settings were not saved to {}: {e}", self.path.display());
        }
    }
}

/// The settings in `path`, read up to `attempts` times while the file exists but isn't settings:
/// it may be in the middle of being written by another window (eframe writes it in place).
fn read_settled(path: &Path, attempts: u32, pause: Duration) -> Result<HashMap<String, String>, String> {
    let mut result = read(path);
    for _ in 1..attempts {
        if result.is_ok() || !path.exists() {
            break;
        }
        std::thread::sleep(pause);
        result = read(path);
    }
    result
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

/// `path` with `suffix` added to its name.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Where process `process` stages a new `path`. Each process has its own, so two windows saving at
/// once (a second instance that didn't hand its files on) never write into one file and move a
/// mixture over the settings; the last whole file to be moved wins.
fn staged_path(path: &Path, process: u32) -> PathBuf {
    sibling(path, &format!(".{process}.tmp"))
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

    /// Hands the changes to a write on its own thread. While an earlier write is still running, or
    /// no thread can be had, they stay for the next save (or the last one, when the store is
    /// dropped): the window never waits for the disk.
    fn flush(&mut self) {
        if self.writing.as_ref().is_some_and(|writing| !writing.is_finished()) {
            return;
        }
        self.settle();
        if !self.dirty || self.read_only {
            return;
        }
        let (path, values) = (self.path.clone(), self.values.clone());
        match std::thread::Builder::new().name("settings".to_owned()).spawn(move || write(&path, &values)) {
            Ok(writing) => {
                self.writing = Some(writing);
                self.dirty = false;
            }
            Err(e) => log::debug!("settings are saved later: no thread to write them ({e})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::Storage as _;

    /// Saves and waits for the write, as the store does when it is dropped.
    fn save(store: &mut FileStore) {
        store.flush();
        store.settle();
    }

    #[test]
    fn settings_survive_a_round_trip_and_a_file_that_is_not_settings() {
        let folder = std::env::temp_dir().join(format!("pdfcraft-soft-store-{}", std::process::id()));
        let path = folder.join("nested").join("app.ron");
        let mut store = FileStore::open(path.clone());
        assert_eq!(store.get_string("pdfcraft"), None);
        store.set_string("pdfcraft", "{\"a\": \"quoted \\\" and\\nnewline\"}".to_owned());
        store.set_string("window", "(x: 1)".to_owned());
        save(&mut store);
        let mut again = FileStore::open(path.clone());
        assert_eq!(again.get_string("pdfcraft").as_deref(), Some("{\"a\": \"quoted \\\" and\\nnewline\"}"));
        again.remove_string("window");
        save(&mut again);
        assert_eq!(FileStore::open(path.clone()).get_string("window"), None);
        assert!(!staged_path(&path, std::process::id()).exists(), "the staged copy was moved, not left");

        let garbage = "not ron at all {{{";
        std::fs::write(&path, garbage).expect("write");
        let mut broken = FileStore::open(path.clone());
        assert_eq!(broken.get_string("pdfcraft"), None, "unreadable settings start empty, they don't stop the app");
        assert_eq!(
            std::fs::read_to_string(sibling(&path, ".unreadable")).ok().as_deref(),
            Some(garbage),
            "and are kept before anything is saved over them"
        );
        broken.set_string("pdfcraft", "x".to_owned());
        save(&mut broken);
        assert_eq!(FileStore::open(path).get_string("pdfcraft").as_deref(), Some("x"));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn settings_that_can_be_neither_read_nor_copied_aside_are_not_saved_over() {
        let folder = std::env::temp_dir().join(format!("pdfcraft-soft-store-kept-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("folder");
        let path = folder.join("app.ron");
        let garbage = "not ron at all {{{";
        std::fs::write(&path, garbage).expect("write");
        let mut store = FileStore::open_keeping(path.clone(), |_, _| Err(std::io::Error::other("disk full")));
        store.set_string("pdfcraft", "x".to_owned());
        save(&mut store);
        drop(store);
        assert_eq!(std::fs::read_to_string(&path).ok().as_deref(), Some(garbage), "the only copy of what it held is left alone");
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn the_last_changes_are_written_when_the_store_goes() {
        let folder = std::env::temp_dir().join(format!("pdfcraft-soft-store-last-{}", std::process::id()));
        let path = folder.join("app.ron");
        let mut store = FileStore::open(path.clone());
        store.set_string("first", "1".to_owned());
        store.flush();
        // Changed again while the first write may still be running: kept for the last save.
        store.set_string("second", "2".to_owned());
        store.flush();
        drop(store);
        let saved = FileStore::open(path);
        assert_eq!((saved.get_string("first").as_deref(), saved.get_string("second").as_deref()), (Some("1"), Some("2")));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_failed_write_keeps_the_changes_to_be_saved_again() {
        let folder = std::env::temp_dir().join(format!("pdfcraft-soft-store-failed-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("folder");
        // The settings' folder is a file: nothing can be written.
        let blocker = folder.join("blocked");
        std::fs::write(&blocker, "").expect("file");
        let mut store = FileStore::open(blocker.join("app.ron"));
        store.set_string("pdfcraft", "x".to_owned());
        save(&mut store);
        assert!(store.dirty, "still to be saved");
        drop(store);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn settings_being_written_by_another_window_are_read_once_written() {
        let folder = std::env::temp_dir().join(format!("pdfcraft-soft-store-settle-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("folder");
        let path = folder.join("app.ron");
        // Truncated, as eframe leaves the file while it writes it in place.
        std::fs::write(&path, "{\"pdfcraft\": \"x").expect("write");
        let finisher = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                std::fs::write(&path, "{\"pdfcraft\": \"x\"}").expect("write");
            })
        };
        let read = read_settled(&path, 20, Duration::from_millis(25));
        let _ = finisher.join();
        assert_eq!(read.ok().and_then(|values| values.get("pdfcraft").cloned()).as_deref(), Some("x"));
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
