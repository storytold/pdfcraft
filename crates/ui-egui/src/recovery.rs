//! Autosave and crash recovery.
//!
//! While documents have unsaved changes, their working file is written every
//! `AUTOSAVE_SECS` to a recovery folder in the user's data directory. Each entry is
//! `<key>.pdf` plus `<key>.json` (name, original path, time). Saving, discarding or closing a
//! document removes its entry. At startup the app offers snapshots even when their metadata is
//! missing or damaged; those recover without an original path, so saving asks for a destination.
//!
//! Encrypted documents are stored encrypted (the working file is), so recovery never writes
//! plaintext of a protected document to disk. The web build has no recovery store yet.
//!
//! On Unix the folder is owner-only (`0o700`), so other accounts on the machine can't read the
//! snapshots of documents that aren't encrypted.
//!
//! A running instance holds an exclusively locked `instance-<pid>-<n>.lock` in the folder and
//! names it in the snapshots it writes, so a second instance can tell live autosaves from a
//! crash's leftovers and neither offers nor discards them (#2166). The OS releases the lock
//! when the process dies.

use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

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
    /// The lock file of the instance that wrote the snapshot (see the module doc); missing in
    /// snapshots from before it.
    #[serde(default)]
    pub instance: Option<String>,
}

/// The recovery folder.
#[derive(Clone, Debug)]
pub struct RecoveryStore {
    dir: PathBuf,
    /// This instance's lock, held while it runs and shared by the store's clones (one store,
    /// one instance). Unset until [`claim`](Self::claim); `Some(None)` when claiming failed
    /// (the folder can't be written, or the platform has no file locks).
    lock: Arc<OnceLock<Option<InstanceLock>>>,
}

/// An open, exclusively locked `instance-<pid>-<n>.lock` in the folder, released and removed
/// with the last clone of its store.
#[derive(Debug)]
struct InstanceLock {
    name: String,
    path: PathBuf,
    file: File,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Tells this process's stores apart: tests make several, and a dropped one's lock is free.
static CLAIMS: AtomicU64 = AtomicU64::new(0);

impl RecoveryStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), lock: Arc::default() }
    }

    /// Create the folder if it is missing. On Unix only the owner may open it: snapshots are
    /// unsaved copies of open documents, and with the default mode other accounts on the
    /// machine could read them. A folder an earlier version made is tightened too. A volume
    /// that can't change modes (FAT, some network shares) works as before, with one warning in
    /// the log.
    fn create_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let open_to_others = std::fs::metadata(&self.dir).is_ok_and(|m| m.permissions().mode() & 0o077 != 0);
            if open_to_others && let Err(e) = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700)) {
                static WARNED: std::sync::Once = std::sync::Once::new();
                WARNED.call_once(|| log::warn!("recovery folder {} can't be made owner-only: {e}", self.dir.display()));
            }
        }
        Ok(())
    }

    /// Mark the folder as in use by a running instance (see the module doc). Without a lock
    /// (the folder can't be written, or the platform has no file locks) snapshots are written
    /// and offered as before.
    pub fn claim(&self) {
        self.lock.get_or_init(|| {
            if let Err(e) = self.create_dir() {
                log::warn!("recovery folder {} can't be made: {e}", self.dir.display());
                return None;
            }
            let name = format!("instance-{}-{}.lock", std::process::id(), CLAIMS.fetch_add(1, Ordering::Relaxed));
            let path = self.dir.join(&name);
            let locked = File::options().read(true).write(true).create(true).truncate(false).open(&path).and_then(|file| match file.try_lock() {
                Ok(()) => Ok(file),
                Err(TryLockError::WouldBlock) => Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "held by another instance")),
                Err(TryLockError::Error(e)) => Err(e),
            });
            match locked {
                Ok(file) => Some(InstanceLock { name, path, file }),
                Err(e) => {
                    let _ = std::fs::remove_file(&path);
                    log::warn!("recovery folder {} can't be claimed: {e}", self.dir.display());
                    None
                }
            }
        });
    }

    fn own_lock(&self) -> Option<&InstanceLock> {
        self.lock.get().and_then(Option::as_ref)
    }

    /// The name of this instance's lock file, for the snapshots it writes.
    pub fn instance(&self) -> Option<String> {
        self.own_lock().map(|l| l.name.clone())
    }

    /// Whether `meta` was written by another instance that is still running. A lock file that
    /// is no longer held is a crash's leftover and is removed. The name comes from the sidecar,
    /// which is untrusted, so only a plain `instance-….lock` name in the folder is looked at.
    fn written_by_a_live_instance(&self, meta: &RecoveryMeta) -> bool {
        let Some(name) = meta.instance.as_deref() else { return false };
        if self.own_lock().is_some_and(|l| l.name == name) {
            return false;
        }
        let plain = name.starts_with("instance-")
            && name.ends_with(".lock")
            && !name.contains("..")
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
        if !plain {
            return false;
        }
        let path = self.dir.join(name);
        let Ok(file) = File::options().read(true).write(true).open(&path) else { return false };
        match file.try_lock() {
            Err(TryLockError::WouldBlock) => true,
            Ok(()) => {
                let _ = file.unlock();
                drop(file);
                let _ = std::fs::remove_file(&path);
                false
            }
            // Can't tell (no file locks here): offered, as before.
            Err(TryLockError::Error(_)) => false,
        }
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
        self.create_dir()?;
        crate::editing::write_atomically(&self.dir.join(format!("{}.pdf", meta.key)).to_string_lossy(), bytes)?;
        let json = serde_json::to_vec_pretty(meta).map_err(std::io::Error::other)?;
        crate::editing::write_atomically(&self.dir.join(format!("{}.json", meta.key)).to_string_lossy(), &json)
    }

    pub fn remove(&self, key: &str) {
        let _ = std::fs::remove_file(self.dir.join(format!("{key}.json")));
        let _ = std::fs::remove_file(self.dir.join(format!("{key}.pdf")));
    }

    /// Snapshots, newest first, leaving out what another running instance is autosaving.
    /// Listing never removes a snapshot; missing metadata uses the file name.
    pub fn list(&self) -> Vec<RecoveryMeta> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut out = Vec::new();
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("pdf") || !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let Some(key) = path.file_stem().and_then(std::ffi::OsStr::to_str) else { continue };
            let meta = std::fs::read(path.with_extension("json"))
                .and_then(|b| serde_json::from_slice::<RecoveryMeta>(&b).map_err(std::io::Error::other))
                .and_then(|m| if m.key == key { Ok(m) } else { Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "snapshot key mismatch")) });
            out.push(meta.unwrap_or_else(|e| {
                log::warn!("recovering snapshot without metadata: {}: {e}", path.display());
                let saved_at = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                RecoveryMeta {
                    key: key.to_string(),
                    name: format!("{key}.pdf"),
                    path: None,
                    saved_at,
                    // This is only a display hint; opening the PDF still detects encryption.
                    encrypted: false,
                    instance: None,
                }
            }));
        }
        out.retain(|m| !self.written_by_a_live_instance(m));
        out.sort_by_key(|m| std::cmp::Reverse(m.saved_at));
        out
    }

    /// Rewrite `meta`'s sidecar when it is missing, unreadable or names another key (a snapshot
    /// recovered without metadata), so damaged metadata doesn't stay behind. The PDF is untouched.
    pub fn repair_meta(&self, meta: &RecoveryMeta) {
        let path = self.dir.join(format!("{}.json", meta.key));
        let readable = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<RecoveryMeta>(&b).ok()).is_some_and(|m| m.key == meta.key);
        if readable {
            return;
        }
        let written = serde_json::to_vec_pretty(meta)
            .map_err(std::io::Error::other)
            .and_then(|json| crate::editing::write_atomically(&path.to_string_lossy(), &json));
        if let Err(e) = written {
            log::warn!("couldn't replace recovery metadata {}: {e}", path.display());
        }
    }

    pub fn read(&self, key: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.dir.join(format!("{key}.pdf")))
    }

    /// Remove every entry (clean quit, or "Discard all"), and metadata left without a snapshot.
    pub fn clear(&self) {
        for m in self.list() {
            self.remove(&m.key);
        }
        let Ok(dir) = std::fs::read_dir(&self.dir) else { return };
        for entry in dir.flatten() {
            let path = entry.path();
            // Only sidecars whose PDF is gone: they hold no document content.
            if path.extension().and_then(std::ffi::OsStr::to_str) == Some("json")
                && entry.file_type().is_ok_and(|t| t.is_file())
                && !path.with_extension("pdf").exists()
            {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl PdfCraftApp {
    /// Turn on autosave into `store`, and look for documents a previous session left behind.
    pub fn enable_recovery(&mut self, store: RecoveryStore) {
        store.claim();
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
            let meta =
                RecoveryMeta { key, name: snap.name, path: snap.path, saved_at: now_secs(), encrypted: snap.encrypted, instance: store.instance() };
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
    pub(crate) fn autosave_tick(&mut self, ctx: &egui::Context, now: f64) {
        if self.recovery.is_none() {
            return;
        }
        if now - self.last_autosave >= AUTOSAVE_SECS {
            self.last_autosave = now;
            self.autosave_now();
        }
        // An idle native window has no input-driven frames. Wake it at the existing
        // autosave deadline, without keeping clean or already-snapshotted documents awake.
        if self.session.has_pending_autosave() {
            let remaining = (self.last_autosave + AUTOSAVE_SECS - now).clamp(0.0, AUTOSAVE_SECS);
            if let Ok(delay) = std::time::Duration::try_from_secs_f64(remaining) {
                ctx.request_repaint_after(delay);
            }
        }
    }

    /// Reopen recovered documents (`keys`), as unsaved changes at their original paths.
    pub fn recover(&mut self, keys: &[String]) {
        let Some(store) = self.recovery.clone() else { return };
        for key in keys {
            let Some(meta) = self.recoverable.iter().find(|m| &m.key == key).cloned() else { continue };
            #[cfg(not(target_arch = "wasm32"))]
            if self.ctx.is_some() && !self.run_inline {
                self.startup_superseded = true;
                let result = self.schedule_open(crate::OpenRequest {
                    name: meta.name.clone(),
                    path: None,
                    stamp: None,
                    input: crate::open_worker::Input::Recovery(store.clone(), key.clone()),
                    password: None,
                    activate: true,
                    after_open: None,
                    recovered: Some(meta.clone()),
                    then: None,
                });
                if let Err(error) = result {
                    self.notify_fmt("Couldn't recover {name}: {e}", &[("name", &meta.name), ("e", &error)]);
                }
                continue;
            }
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
        // A snapshot recovered without readable metadata gets a fresh sidecar now, so the
        // damaged one doesn't linger until the next autosave (or forever, if none comes).
        if let Some(store) = &self.recovery {
            let mut meta = meta.clone();
            if let Some(doc) = self.session.get(id) {
                meta.encrypted = doc.info.encrypted;
            }
            store.repair_meta(&meta);
        }
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

#[cfg(test)]
mod idle_tests {
    use super::*;
    use std::time::Duration;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("pdfcraft-idle-recovery-{}-{nonce}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn frame(app: &mut PdfCraftApp, ctx: &egui::Context, now: f64) -> Duration {
        let mut out = ctx.run_ui(egui::RawInput { time: Some(now), ..Default::default() }, |ui| {
            app.autosave_tick(ui.ctx(), now);
        });
        // Headless timer test: acknowledge texture uploads without a GPU backend.
        out.textures_delta.clear();
        out.viewport_output[&egui::ViewportId::ROOT].repaint_delay
    }

    #[test]
    fn idle_dirty_document_requests_its_deadline_and_writes_without_input() {
        let dir = TestDir::new();
        let store = RecoveryStore::new(dir.path());
        let mut app = PdfCraftApp::new();
        app.enable_recovery(store.clone());
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        let id = app.session.open_new("Untitled.pdf", bytes).unwrap();
        let ctx = egui::Context::default();
        let mut delay = Duration::ZERO;
        for _ in 0..4 {
            delay = frame(&mut app, &ctx, 15.0);
        }
        assert!(delay > Duration::ZERO && delay <= Duration::from_secs(45), "idle deadline: {delay:?}");
        assert!(store.list().is_empty());
        frame(&mut app, &ctx, 60.0);
        assert_eq!(store.list().len(), 1, "timer frame writes the snapshot without input");
        assert!(app.session.get(id).unwrap().dirty, "autosave does not mark the document saved");
        assert!(!app.session.has_pending_autosave());
        for _ in 0..4 {
            delay = frame(&mut app, &ctx, 61.0);
        }
        assert_eq!(delay, Duration::MAX, "unchanged snapshot needs no periodic wake");
    }

    #[test]
    fn second_edit_after_autosave_schedules_next_deadline_and_updates_snapshot() {
        let dir = TestDir::new();
        let store = RecoveryStore::new(dir.path());
        let mut app = PdfCraftApp::new();
        app.enable_recovery(store.clone());
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        let id = app.session.open_new("Untitled.pdf", bytes).unwrap();
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, 60.0);
        let entries = store.list();
        assert_eq!(entries.len(), 1);
        let key = entries[0].key.clone();
        let first = store.read(&key).unwrap();
        assert!(!app.session.has_pending_autosave());
        let mut delay = Duration::ZERO;
        for _ in 0..4 {
            delay = frame(&mut app, &ctx, 61.0);
        }
        assert_eq!(delay, Duration::MAX);

        app.session.apply(id, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Second edit".into() }).unwrap();
        assert!(app.session.has_pending_autosave());
        for _ in 0..4 {
            delay = frame(&mut app, &ctx, 75.0);
        }
        assert!(delay > Duration::ZERO && delay <= Duration::from_secs(45), "second edit wakes by 120 seconds: {delay:?}");
        assert_eq!(store.read(&key).unwrap(), first, "editing does not overwrite the snapshot before its deadline");
        frame(&mut app, &ctx, 119.0);
        assert_eq!(store.read(&key).unwrap(), first);
        frame(&mut app, &ctx, 120.0);
        let entries = store.list();
        assert_eq!(entries.len(), 1, "the same document replaces its existing snapshot");
        assert_eq!(entries[0].key, key);
        let second = store.read(&key).unwrap();
        assert_ne!(second, first);
        let mut recovered = pdfcraft_engine::Session::new();
        let restored = recovered.open("Snapshot.pdf", None, second.into(), None).unwrap();
        assert_eq!(recovered.get(restored).unwrap().info_value("Title"), Some("Second edit".into()));
        assert!(app.session.get(id).unwrap().dirty);
        assert!(!app.session.has_pending_autosave());
        for _ in 0..4 {
            delay = frame(&mut app, &ctx, 121.0);
        }
        assert_eq!(delay, Duration::MAX, "the second snapshot also lets the window sleep");
    }

    #[test]
    fn clean_documents_and_disabled_recovery_do_not_schedule_autosave() {
        for recovery_enabled in [false, true] {
            let dir = TestDir::new();
            let mut app = PdfCraftApp::new();
            if recovery_enabled {
                app.enable_recovery(RecoveryStore::new(dir.path()));
            }
            let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
            if recovery_enabled {
                app.session.open("Clean.pdf", None, bytes, None).unwrap();
            } else {
                app.session.open_new("Unsaved.pdf", bytes).unwrap();
            }
            let ctx = egui::Context::default();
            let mut delay = Duration::ZERO;
            for _ in 0..4 {
                delay = frame(&mut app, &ctx, 15.0);
            }
            assert_eq!(delay, Duration::MAX);
        }
    }
}
