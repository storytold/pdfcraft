//! Autosave and crash recovery.
//!
//! While documents have unsaved changes, the framework's `Autosave` decides when each is due
//! (every `AUTOSAVE_SECS`) and its working file is written through the framework's
//! `RecoveryStore` to the recovery folder. Saving, discarding or closing a document removes its
//! entry. At startup the app offers what a crashed run left, one document at a time; entries with
//! missing or damaged metadata recover without an original path, so saving asks for a
//! destination. A recovered document's original path is only a suggestion: anyone who can write
//! the folder can name any file in a sidecar, so the first Save after recovering asks where,
//! starting at that path, and later saves write where the user chose (#3923).
//!
//! Encrypted documents are stored encrypted (the working file is), so recovery never writes
//! plaintext of a protected document to disk. The web build has no recovery store.

use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::time::SystemTime;

#[cfg(not(target_arch = "wasm32"))]
use craft::platform::autosave::{AutosaveDoc, RecoveryEntry, SettingsMode, adopt_legacy_recovery, recovery_dir};
use craft::ui::dialog::{DialogResult, RecoveredDocument, RecoveryAnswer, RecoveryDialog};

use crate::PdfCraftApp;

/// How often unsaved changes are written to the recovery folder.
pub const AUTOSAVE_SECS: f64 = 60.0;

/// The folder an earlier PdfCraft kept its recovery entries in (the portable data folder's
/// `Recovery`, else `~/Library/Application Support/PdfCraft/Recovery` on macOS,
/// `%LOCALAPPDATA%\PdfCraft\Recovery` on Windows, `$XDG_DATA_HOME/pdfcraft/recovery` elsewhere).
/// What a crash left there is adopted into the current folder.
pub fn default_dir() -> Option<PathBuf> {
    if let Some(dir) = crate::settings::portable_dir() {
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

/// The recovery folder of this machine, after moving what the old folder ([`default_dir`]) holds
/// into it. The app's constructor takes it through its services.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn prepare_platform_recovery_dir() -> Option<PathBuf> {
    let portable = crate::settings::portable_dir();
    let mode = if portable.is_some() { SettingsMode::SelfContained } else { SettingsMode::Platform };
    let dir = recovery_dir("PdfCraft", "pdfcraft", portable.as_deref(), mode, |k| std::env::var_os(k))?;
    if let Some(legacy) = default_dir() {
        let _ = adopt_legacy_recovery(&legacy, &dir);
    }
    Some(dir)
}

/// The OCR Verify screen's per-language user dictionary folder, next to the recovery store:
/// the portable data folder's `Dictionary` in portable mode, otherwise
/// `…/PdfCraft/Dictionary` in the platform's per-user data folder. `None` where files are not
/// kept (the web build).
pub fn default_dictionary_dir() -> Option<PathBuf> {
    if let Some(dir) = crate::settings::portable_dir() {
        return Some(dir.join("Dictionary"));
    }
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support/PdfCraft/Dictionary"))
    } else if cfg!(windows) {
        env("LOCALAPPDATA").map(|d| d.join("PdfCraft").join("Dictionary"))
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share"))).map(|d| d.join("pdfcraft/dictionary"))
    }
}

impl PdfCraftApp {
    /// Write snapshots of every document with unsaved changes now (a frame panicked, or a
    /// snapshot must replace an older one).
    pub fn autosave_now(&mut self) {
        self.autosave.autosave_now();
        self.autosave_pass(web_time::Instant::now());
    }

    /// Decide what is due at `now` and write it. Whatever the timer stops tracking (a saved,
    /// undone-clean or closed document) loses its copy.
    fn autosave_pass(&mut self, now: web_time::Instant) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(store) = self.recovery.clone() else { return };
            let docs: Vec<AutosaveDoc> =
                self.session.autosave_docs().map(|(id, revision, dirty)| AutosaveDoc { id: id.0, revision, dirty }).collect();
            let due = self.autosave.due(now, docs.iter().cloned());
            for id in due.discard {
                if let Err(e) = store.discard(id) {
                    log::warn!("couldn't remove a recovery copy: {e}");
                }
            }
            for id in due.save {
                let revision = docs.iter().find(|d| d.id == id).map_or(0, |d| d.revision);
                let result = match self.session.recovery_snapshot(pdfcraft_engine::DocId(id)) {
                    Some(snap) => {
                        let meta = craft::platform::autosave::RecoveryMeta {
                            name: snap.name,
                            original: snap.path.map(PathBuf::from),
                            extension: "pdf".into(),
                            revision,
                            protected: snap.encrypted,
                        };
                        store.write(id, &meta, &snap.bytes).map_err(|e| e.to_string())
                    }
                    None => Err("the document could not be copied without its password fields".to_string()),
                };
                // A failure leaves the document due again at the next interval.
                if let Some(message) = self.autosave.finished(id, revision, result) {
                    log::warn!("{message}");
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = now;
    }

    /// Drop a document's recovery entry (it was saved, discarded or closed).
    pub(crate) fn forget_recovery(&mut self, doc: pdfcraft_engine::DocId) {
        // Saved (to a destination the user chose), closed or reloaded from its file: the path is
        // no longer only what the sidecar said.
        self.recovered_paths.remove(&doc);
        self.autosave.forget(doc.0);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(store) = &self.recovery
            && let Err(e) = store.discard(doc.0)
        {
            log::warn!("couldn't remove a recovery copy: {e}");
        }
    }

    /// Replace a document's recovery snapshot with its current state now. After an edit that takes
    /// content out (Apply Redactions, Sanitize, Remove Hidden Information), the last snapshot still
    /// holds what went, until the next autosave, and for good if the app is killed before then.
    pub(crate) fn renew_recovery(&mut self, doc: pdfcraft_engine::DocId) {
        self.forget_recovery(doc);
        self.autosave_now();
    }

    /// Autosave when the interval has passed (called by `logic`, which also runs while hidden).
    ///
    /// The deadline is monotonic (`web_time::Instant`): egui's input clock does not advance
    /// during a hidden `run_logic`, so using it would stall autosave or restart its interval.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn autosave_tick(&mut self, ctx: &egui::Context, now: web_time::Instant) {
        if self.recovery.is_none() {
            return;
        }
        self.autosave_pass(now);
        // An idle native window has no input-driven frames: wake it at the timer's deadline,
        // which exists only while a changed document waits for its write.
        if let Some(deadline) = self.autosave.deadline() {
            ctx.request_repaint_after(deadline.saturating_duration_since(now));
        }
    }

    /// The web keeps no recovery folder: nothing is autosaved.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn autosave_tick(&mut self, _ctx: &egui::Context, _now: web_time::Instant) {}

    /// Opens the offer for the first entry a previous run left, once nothing else (a password
    /// prompt of an entry just recovered) is in front, and acts on the answer. Called once per
    /// frame with the dialogs' other results.
    pub(crate) fn recovery_pass(&mut self, _ctx: &egui::Context) {
        if let Some(result) = self.dialogs.take::<RecoveryDialog>() {
            #[cfg(not(target_arch = "wasm32"))]
            match result {
                DialogResult::Ok(choice) => {
                    let keys: Vec<String> = if choice.apply_to_all {
                        self.recovered.iter().map(|e| e.key.clone()).collect()
                    } else {
                        self.recovered.first().map(|e| e.key.clone()).into_iter().collect()
                    };
                    match choice.answer {
                        RecoveryAnswer::Recover => self.recover(&keys),
                        RecoveryAnswer::Discard => self.discard_recovered(&keys),
                        // An answer this app doesn't know keeps the copy, like Escape.
                        _ => self.skip_offer(),
                    }
                }
                // Escape keeps the copy for the next launch and stops offering it in this run.
                _ => self.skip_offer(),
            }
            #[cfg(target_arch = "wasm32")]
            let _ = result;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.password_prompt.is_none()
            && !self.dialogs.is_open::<RecoveryDialog>()
            && let Some(entry) = self.recovered.first()
        {
            let age = SystemTime::now().duration_since(entry.saved).unwrap_or_default();
            let mut document = RecoveredDocument::new(entry.key.clone(), entry.meta.name.clone(), age);
            if entry.meta.protected {
                document = document.protected();
            }
            let dialog = RecoveryDialog::new(document).remaining(self.recovered.len());
            let _ = self.dialogs.open(dialog);
        }
    }

    /// Stops offering the first entry in this run; its copy stays for the next launch.
    #[cfg(not(target_arch = "wasm32"))]
    fn skip_offer(&mut self) {
        if !self.recovered.is_empty() {
            self.recovered.remove(0);
        }
    }

    /// Reopen recovered documents (`keys`), as unsaved changes at their original paths.
    pub fn recover(&mut self, keys: &[String]) {
        #[cfg(not(target_arch = "wasm32"))]
        for key in keys {
            let Some(at) = self.recovered.iter().position(|e| &e.key == key) else { continue };
            let entry = self.recovered.remove(at);
            self.startup_superseded = true;
            self.schedule_open(crate::OpenRequest {
                name: entry.meta.name.clone(),
                path: None,
                stamp: None,
                input: crate::open_worker::Input::Recovery(entry.path.clone()),
                password: None,
                activate: true,
                after_open: None,
                recovered: Some(entry.clone()),
                then: None,
                priority: craft::host::queue::Priority::VISIBLE,
                target_group: None,
            });
        }
        #[cfg(target_arch = "wasm32")]
        let _ = keys;
    }

    /// The most recently opened tab came from `entry`: keep its recovery entry and path. The
    /// store replaces the old copy with this document's autosaves, and a sidecar that was
    /// missing or damaged is written again now, over the same bytes.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn finish_recovery(&mut self, entry: &RecoveryEntry) {
        let Some(id) = self.views.last().map(|v| v.id) else { return };
        let path = entry.meta.original.as_ref().map(|p| p.to_string_lossy().into_owned());
        self.session.mark_recovered(id, path.clone());
        if path.is_some() {
            self.recovered_paths.insert(id);
        }
        let revision = self.session.autosave_docs().find(|d| d.0 == id).map_or(0, |d| d.1);
        // The copy is already on disk: the timer waits for the next edit.
        self.autosave.mark_autosaved(id.0, revision);
        let Some(store) = self.recovery.clone() else { return };
        store.adopt(id.0, entry);
        let mut meta = entry.meta.clone();
        meta.revision = revision;
        meta.protected |= self.session.get(id).is_some_and(|d| d.info.encrypted);
        match std::fs::read(&entry.path) {
            Ok(bytes) => {
                if let Err(e) = store.write(id.0, &meta, &bytes) {
                    log::warn!("couldn't replace recovery metadata of {}: {e}", entry.path.display());
                }
            }
            Err(e) => log::warn!("couldn't read recovery copy {}: {e}", entry.path.display()),
        }
    }

    /// Delete recovery entries the user chose not to recover.
    pub fn discard_recovered(&mut self, keys: &[String]) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(store) = &self.recovery {
                for entry in self.recovered.iter().filter(|e| keys.contains(&e.key)) {
                    if let Err(e) = store.remove(entry) {
                        log::warn!("couldn't remove recovery entry {}: {e}", entry.key);
                    }
                }
            }
            self.recovered.retain(|e| !keys.contains(&e.key));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = keys;
    }

    /// Clean shutdown: documents were saved or discarded, so nothing needs recovering.
    pub fn shutdown_recovery(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.first_dirty().is_none()
            && let Some(store) = &self.recovery
        {
            for view in &self.views {
                if let Err(e) = store.discard(view.id.0) {
                    log::warn!("couldn't remove a recovery copy: {e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use craft::host::{AppInfo, Host, ManualJobs, ScriptedFiles};

    pub(super) struct TestDir(PathBuf);

    impl TestDir {
        /// A new folder of this test's own. The tests run on parallel threads of one process, and
        /// two can read the same clock time (it moves in microseconds on macOS), so a counter
        /// tells their folders apart; the time keeps runs of the process apart.
        pub(super) fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("pdfcraft-idle-recovery-{}-{nonce}-{n}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        /// The folder.
        pub(super) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        /// Remove the folder and what the test left in it.
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An app whose services name `recovery` as the recovery folder (none: recovery is off), and
    /// the host's scripted files and manual jobs.
    pub(super) fn app_in(recovery: Option<&Path>) -> (PdfCraftApp, Arc<ScriptedFiles>, Arc<ManualJobs>) {
        let files = Arc::new(ScriptedFiles::new());
        let jobs = Arc::new(ManualJobs::new());
        let info = AppInfo { id: "pdfcraft", name: "PdfCraft", version: env!("CARGO_PKG_VERSION") };
        let host = Host::builder(info).jobs(jobs.clone()).files(files.clone()).build();
        let wake = craft::host::Wake::none();
        let clipboard = craft::platform::clipboard::Clipboard::new(craft::platform::clipboard::SystemClipboard::new());
        let (intake, _) = craft::platform::open::FileIntake::new(wake.clone());
        let watch = craft::platform::watch::FileWatch::new(wake.clone());
        let services = crate::AppServices::new(wake, clipboard, recovery.map(Path::to_path_buf), None, intake, watch);
        (PdfCraftApp::with_host(Arc::new(host), services), files, jobs)
    }

    /// The copies in `dir`.
    fn copies(dir: &Path) -> Vec<PathBuf> {
        let read = std::fs::read_dir(dir).unwrap();
        read.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "pdf")).collect()
    }

    fn frame(app: &mut PdfCraftApp, ctx: &egui::Context, now: web_time::Instant) -> Duration {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.autosave_tick(ui.ctx(), now);
        });
        // Headless timer test: acknowledge texture uploads without a GPU backend.
        out.textures_delta.clear();
        out.viewport_output[&egui::ViewportId::ROOT].repaint_delay
    }

    /// #3923 (review on #4327): a copy that is not a regular file is refused before it is
    /// opened. The store lists a FIFO in the recovery folder as an entry, and reading it would
    /// hold the recovery until something wrote to it.
    #[cfg(unix)]
    #[test]
    fn a_snapshot_that_is_not_a_regular_file_is_refused_at_once() {
        use craft::platform::autosave::RecoveryStore;

        let dir = TestDir::new();
        let folder = dir.path().join("recovery");
        let meta = |name: &str| craft::platform::autosave::RecoveryMeta {
            name: name.into(),
            original: None,
            extension: "pdf".into(),
            revision: 1,
            protected: false,
        };
        let first = RecoveryStore::open(&folder).unwrap();
        first.write(1, &meta("fifo.pdf"), b"%PDF-1.7\n").unwrap();
        first.write(2, &meta("folder.pdf"), b"%PDF-1.7\n").unwrap();
        let fifo = folder.join(format!("{}.pdf", first.key(1)));
        let planted = folder.join(format!("{}.pdf", first.key(2)));
        std::fs::remove_file(&fifo).unwrap();
        match std::process::Command::new("mkfifo").arg(&fifo).status() {
            Ok(s) if s.success() => {}
            _ => return, // No mkfifo here: nothing to plant.
        }
        std::fs::remove_file(&planted).unwrap();
        std::fs::create_dir(&planted).unwrap();
        drop(first);
        let entries = RecoveryStore::open(&folder).unwrap().leftovers();
        assert_eq!(entries.len(), 2);
        for entry in entries {
            let (tx, rx) = std::sync::mpsc::channel();
            let path = entry.path.clone();
            std::thread::spawn(move || {
                let _ = tx.send(crate::open_worker::read_recovery_copy(&path).map(|b| b.len()));
            });
            let read = rx.recv_timeout(Duration::from_secs(5)).expect("the read returned instead of waiting on the FIFO");
            assert_eq!(read.map_err(|e| e.kind()), Err(std::io::ErrorKind::InvalidInput), "{}", entry.path.display());
        }
    }

    /// Check that an edit after autosave is not written before its deadline, is written at it
    /// into the same entry, and lets the window sleep after.
    #[test]
    fn second_edit_after_autosave_schedules_next_deadline_and_updates_snapshot() {
        let dir = TestDir::new();
        let (mut app, ..) = app_in(Some(&dir.path().join("recovery")));
        let folder = dir.path().join("recovery");
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        let id = app.session.open_new("Untitled.pdf", bytes).unwrap();
        let ctx = egui::Context::default();
        let base = web_time::Instant::now();
        frame(&mut app, &ctx, base);
        frame(&mut app, &ctx, base + Duration::from_secs(60));
        let entries = copies(&folder);
        assert_eq!(entries.len(), 1);
        let copy = entries[0].clone();
        let first = std::fs::read(&copy).unwrap();
        let mut delay = Duration::ZERO;
        for _ in 0..4 {
            delay = frame(&mut app, &ctx, base + Duration::from_secs(61));
        }
        assert_eq!(delay, Duration::MAX);

        app.session.apply(id, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Second edit".into() }).unwrap();
        for _ in 0..4 {
            delay = frame(&mut app, &ctx, base + Duration::from_secs(75));
        }
        assert!(delay > Duration::ZERO && delay <= Duration::from_secs(45), "second edit wakes by 120 seconds: {delay:?}");
        assert_eq!(std::fs::read(&copy).unwrap(), first, "editing does not overwrite the snapshot before its deadline");
        frame(&mut app, &ctx, base + Duration::from_secs(119));
        assert_eq!(std::fs::read(&copy).unwrap(), first);
        frame(&mut app, &ctx, base + Duration::from_secs(120));
        assert_eq!(copies(&folder), std::slice::from_ref(&copy), "the same document replaces its existing snapshot");
        let second = std::fs::read(&copy).unwrap();
        assert_ne!(second, first);
        let mut recovered = pdfcraft_engine::Session::new();
        let restored = recovered.open("Snapshot.pdf", None, second.into(), None).unwrap();
        assert_eq!(recovered.get(restored).unwrap().info_value("Title"), Some("Second edit".into()));
        assert!(app.session.get(id).unwrap().dirty);
        for _ in 0..4 {
            delay = frame(&mut app, &ctx, base + Duration::from_secs(121));
        }
        assert_eq!(delay, Duration::MAX, "the second snapshot also lets the window sleep");
    }

    /// Check that a clean document and a document with recovery disabled never schedule an autosave.
    #[test]
    fn clean_documents_and_disabled_recovery_do_not_schedule_autosave() {
        for recovery_enabled in [false, true] {
            let dir = TestDir::new();
            let folder = dir.path().join("recovery");
            let (mut app, ..) = app_in(recovery_enabled.then_some(folder.as_path()));
            let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
            if recovery_enabled {
                app.session.open("Clean.pdf", None, bytes, None).unwrap();
            } else {
                app.session.open_new("Unsaved.pdf", bytes).unwrap();
            }
            let ctx = egui::Context::default();
            let base = web_time::Instant::now();
            let mut delay = Duration::ZERO;
            for _ in 0..4 {
                delay = frame(&mut app, &ctx, base + Duration::from_secs(15));
            }
            assert_eq!(delay, Duration::MAX);
        }
    }
}

#[cfg(test)]
mod sidecar_tests {
    use super::idle_tests::{TestDir, app_in};

    use crate::SaveTarget;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use std::time::Duration;

    /// #3923: a sidecar names a file the user can write (here, one that is not the document's).
    /// After Recover, Save asks where instead of writing that file; the file the user picks is
    /// the document's file from then on.
    #[test]
    fn the_first_save_after_recovering_asks_instead_of_writing_the_sidecars_path() {
        let _ctx = &egui::Context::default();
        let ctx = &egui::Context::default();
        let dir = TestDir::new();
        let folder = dir.path().join("recovery");
        let named = dir.path().join("named.pdf");
        std::fs::write(&named, b"someone else's file").unwrap();
        {
            let (mut crashed, ..) = app_in(Some(&folder));
            let bytes = crashed.session.create_blank(200.0, 300.0, 1).unwrap();
            crashed.open_bytes(ctx, "work.pdf", Some(named.to_string_lossy().into_owned()), bytes.to_vec()).unwrap();
            assert!(crashed.apply_edit(ctx, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
            crashed.autosave_now();
        }

        let (mut app, files, jobs) = app_in(Some(&folder));
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        assert_eq!(app.recovered.len(), 1);
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            app
        });
        h.run_steps(4);
        h.get_by_label("Recover").click();
        for _ in 0..200 {
            jobs.run_all();
            h.step();
            if h.state().views.len() == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(h.state().views.len(), 1);
        let chosen = dir.path().join("chosen.pdf");
        files.queue(craft::host::Picked::Paths(vec![chosen.clone()]));

        assert!(!h.state_mut().save_active(ctx, SaveTarget::InPlace), "Save asks instead of writing the sidecar's path");
        for _ in 0..20 {
            jobs.run_all();
            h.step();
        }
        // Saves go through the host's files, which hold what is written in memory.
        assert!(files.file(&named).is_none(), "the named file is untouched");
        assert!(files.file(&chosen).expect("the save wrote the chosen file").starts_with(b"%PDF"), "the document went where the user chose");

        // The chosen file is the document's file now: the next Save writes it without asking.
        let ctx_h = h.ctx.clone();
        assert!(h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        assert!(h.state_mut().save_active(ctx, SaveTarget::InPlace));
    }
}
