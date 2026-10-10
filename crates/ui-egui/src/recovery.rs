//! Autosave and crash recovery.
//!
//! While documents have unsaved changes, their working file is written every
//! `AUTOSAVE_SECS` to a recovery folder in the user's data directory. Each entry is
//! `<key>.pdf` plus `<key>.json` (name, original path, time). Saving, discarding or closing a
//! document removes its entry, and a clean quit leaves the folder empty. If entries are found at
//! startup, the previous session ended unexpectedly and the app offers to recover them.
//!
//! Encrypted documents are stored encrypted (the working file is), so recovery never writes
//! plaintext of a protected document to disk. The web build has no recovery store yet.

use std::path::{Path, PathBuf};

use crate::PdfCraftApp;

/// How often unsaved changes are written to the recovery folder.
pub const AUTOSAVE_SECS: f64 = 60.0;

/// What a recovery entry records next to the PDF bytes.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecoveryMeta {
    pub key: String,
    pub name: String,
    /// Where the document was saved before (Save writes back there after recovery).
    pub path: Option<String>,
    /// Seconds since the Unix epoch when the snapshot was taken.
    pub saved_at: u64,
    /// The bytes are encrypted; recovering asks for the password.
    pub encrypted: bool,
}

/// The recovery folder.
#[derive(Clone, Debug)]
pub struct RecoveryStore {
    dir: PathBuf,
}

impl RecoveryStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `Recovery` in the portable data folder in portable mode ([`crate::portable`]), otherwise
    /// the platform's per-user data folder: `~/Library/Application Support/PdfCraft/Recovery`
    /// (macOS), `%LOCALAPPDATA%\PdfCraft\Recovery` (Windows), or
    /// `$XDG_DATA_HOME/pdfcraft/recovery` / `~/.local/share/pdfcraft/recovery` (others).
    pub fn default_dir() -> Option<PathBuf> {
        if let Some(dir) = crate::portable::data_dir() {
            return Some(dir.join("Recovery"));
        }
        let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        if cfg!(target_os = "macos") {
            env("HOME").map(|h| h.join("Library/Application Support/PdfCraft/Recovery"))
        } else if cfg!(windows) {
            env("LOCALAPPDATA").map(|d| d.join("PdfCraft").join("Recovery"))
        } else {
            env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share"))).map(|d| d.join("pdfcraft/recovery"))
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Write (or replace) an entry. The PDF is written before its metadata, both atomically, so
    /// a listed entry always has complete bytes.
    pub fn write(&self, meta: &RecoveryMeta, bytes: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        crate::editing::write_atomically(&self.dir.join(format!("{}.pdf", meta.key)).to_string_lossy(), bytes)?;
        let json = serde_json::to_vec_pretty(meta).map_err(std::io::Error::other)?;
        crate::editing::write_atomically(&self.dir.join(format!("{}.json", meta.key)).to_string_lossy(), &json)
    }

    pub fn remove(&self, key: &str) {
        let _ = std::fs::remove_file(self.dir.join(format!("{key}.json")));
        let _ = std::fs::remove_file(self.dir.join(format!("{key}.pdf")));
    }

    /// Complete entries, newest first. Incomplete leftovers (bytes without metadata) are removed.
    pub fn list(&self) -> Vec<RecoveryMeta> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut out = Vec::new();
        for entry in dir.flatten() {
            let path = entry.path();
            match path.extension().and_then(|e| e.to_str()) {
                Some("json") => {
                    let meta = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<RecoveryMeta>(&b).ok());
                    match meta {
                        Some(m) if self.dir.join(format!("{}.pdf", m.key)).is_file() => out.push(m),
                        _ => {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
                Some("pdf") if !path.with_extension("json").is_file() => {
                    let _ = std::fs::remove_file(&path);
                }
                _ => {}
            }
        }
        out.sort_by_key(|m| std::cmp::Reverse(m.saved_at));
        out
    }

    pub fn read(&self, key: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.dir.join(format!("{key}.pdf")))
    }

    /// Remove every entry (clean quit, or "Discard all").
    pub fn clear(&self) {
        for m in self.list() {
            self.remove(&m.key);
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl PdfCraftApp {
    /// Turn on autosave into `store`, and look for documents a previous session left behind.
    pub fn enable_recovery(&mut self, store: RecoveryStore) {
        self.recoverable = store.list();
        if !self.recoverable.is_empty() {
            self.dialog = Some(crate::Dialog::Recovery);
        }
        self.recovery = Some(store);
    }

    /// Write snapshots of documents changed since the last autosave (called periodically).
    pub fn autosave_now(&mut self) {
        let Some(store) = self.recovery.clone() else { return };
        for snap in self.session.autosave_snapshots() {
            let key = self.recovery_keys.entry(snap.doc).or_insert_with(|| format!("{}-{}-{}", now_secs(), std::process::id(), snap.doc.0)).clone();
            let meta = RecoveryMeta { key, name: snap.name, path: snap.path, saved_at: now_secs(), encrypted: snap.encrypted };
            if let Err(e) = store.write(&meta, &snap.bytes) {
                log::warn!("autosave failed: {e}");
            }
        }
    }

    /// Drop a document's recovery entry (it was saved, discarded or closed).
    pub(crate) fn forget_recovery(&mut self, doc: pdfcraft_engine::DocId) {
        if let (Some(store), Some(key)) = (&self.recovery, self.recovery_keys.remove(&doc)) {
            store.remove(&key);
        }
    }

    /// Autosave when the interval has passed (called every frame).
    pub(crate) fn autosave_tick(&mut self, now: f64) {
        if self.recovery.is_none() {
            return;
        }
        if now - self.last_autosave >= AUTOSAVE_SECS {
            self.last_autosave = now;
            self.autosave_now();
        }
    }

    /// Reopen recovered documents (`keys`), as unsaved changes at their original paths.
    pub fn recover(&mut self, keys: &[String]) {
        let Some(store) = self.recovery.clone() else { return };
        for key in keys {
            let Some(meta) = self.recoverable.iter().find(|m| &m.key == key).cloned() else { continue };
            let bytes = match store.read(key) {
                Ok(b) => b,
                Err(e) => {
                    self.notify_fmt("Couldn't recover {name}: {e}", &[("name", &meta.name), ("e", &e.to_string())]);
                    continue;
                }
            };
            match self.open_bytes(&meta.name, None, bytes) {
                Ok(()) if self.password_prompt.is_none() => self.finish_recovery(&meta),
                // Encrypted: the password prompt opens it, and finishes its recovery once it
                // does. The association travels with the prompt, so cancelling it cannot hand
                // this snapshot's path to the next document that is unlocked (#813).
                Ok(()) => {
                    if let Some(prompt) = self.password_prompt.as_mut() {
                        prompt.recovered = Some(meta.clone());
                    }
                }
                Err(e) => self.notify_fmt("Couldn't recover {name}: {e}", &[("name", &meta.name), ("e", &e.to_string())]),
            }
        }
        self.recoverable.retain(|m| !keys.contains(&m.key));
    }

    /// The most recently opened tab came from `meta`: keep its recovery entry and path.
    pub(crate) fn finish_recovery(&mut self, meta: &RecoveryMeta) {
        let Some(id) = self.views.last().map(|v| v.id) else { return };
        self.session.mark_recovered(id, meta.path.clone());
        self.recovery_keys.insert(id, meta.key.clone());
    }

    /// Delete recovery entries the user chose not to recover.
    pub fn discard_recovered(&mut self, keys: &[String]) {
        if let Some(store) = &self.recovery {
            for k in keys {
                store.remove(k);
            }
        }
        self.recoverable.retain(|m| !keys.contains(&m.key));
    }

    /// Clean shutdown: documents were saved or discarded, so nothing needs recovering.
    pub fn shutdown_recovery(&mut self) {
        if self.first_dirty().is_none()
            && let Some(store) = &self.recovery
        {
            for key in self.recovery_keys.values() {
                store.remove(key);
            }
            self.recovery_keys.clear();
        }
    }
}
