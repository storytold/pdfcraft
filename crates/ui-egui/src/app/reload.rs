//! Reload a document when its file changes on disk (#431): a LaTeX or Typst build rewriting the
//! PDF that is open shows its new version without reopening it.
//!
//! The framework's [`FileWatch`] looks at the files and reads a changed one once it ends like a
//! whole PDF; this module decides what a new version does to its document. A clean document is
//! reloaded in place, keeping its tab, page, zoom and scroll; one with unsaved changes gets a
//! notice instead. PdfCraft's own saves record the file's version on the watch
//! (`SaveFlow::written`), so they never reload.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft::platform::watch::{Changed, FileStamp, FileWatch, ReadPolicy};
use pdfcraft_engine::DocId;

use crate::PdfCraftApp;

/// How much of the end of a changed file is read first to see whether it is a whole PDF: a file
/// still being written then costs one small read.
const TAIL: usize = 1024;

/// `watch` as the open documents' files are watched: a changed file is read whole once its last
/// KiB holds `%%EOF`. `AppServices::new` applies it to the watch it is given, the system's or a
/// test's, so the read rule lives in one place.
pub(crate) fn document_watch(watch: FileWatch<DocId>) -> FileWatch<DocId> {
    watch.read(ReadPolicy::Whole { tail: TAIL, complete: Arc::new(|end: &[u8]| end.windows(5).any(|w| w == b"%%EOF")) })
}

impl PdfCraftApp {
    /// Each pass: reload what the notice bar's Reload asked for, tell the watch which files are
    /// open (it sends nothing when the list is the one sent last), and act on the new versions
    /// it found. Nothing here reads the frame's input: the reload request and the changes are
    /// taken once from where they wait, so a second pass in a frame finds only what arrived since
    /// the first.
    pub(crate) fn watch_files(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.reload_request.take() {
            self.reload_from_disk(ctx, id);
        }
        // Every open document with a file, while the preference is on. Turning it off stops the
        // watching; the versions recorded stay, so a save still asks about a file changed on disk.
        let mut files: Vec<(DocId, PathBuf)> = Vec::new();
        if self.reload_changed_files {
            for view in &self.views {
                let Some(path) = self.session.get(view.id).and_then(|d| d.path.as_deref()) else { continue };
                // Two tabs of one document watch its file once.
                if !files.iter().any(|(id, _)| *id == view.id) {
                    files.push((view.id, PathBuf::from(path)));
                }
            }
        }
        self.watch.set(files);
        self.watch.tick();
        // A dialog acts on the active document when its OK is pressed: replacing the document
        // under it would apply the dialog to another version (#3917). The changes wait in the
        // watch until it closes, and are checked then: one found for a file no longer watched, or
        // before a save that came meanwhile, is dropped by `take_changes` (#1802).
        if self.holds_reload() {
            return;
        }
        let mut latest: Vec<Changed<DocId>> = Vec::new();
        for change in self.watch.take_changes() {
            if let Some(slot) = latest.iter_mut().find(|c| c.key == change.key) {
                *slot = change;
            } else {
                latest.push(change);
            }
        }
        for change in latest {
            self.file_changed(ctx, change);
        }
    }

    /// A dialog or prompt is open: new versions wait (#3917). The full-screen prompt is not modal
    /// elsewhere, but it is about the version on screen; the changed-on-disk question of a save
    /// holds until the save has taken its answer, since replacing the document first would save
    /// another version than the one the user was asked about.
    fn holds_reload(&self) -> bool {
        self.modal_open() || self.prompts.full_screen.is_some() || self.pending_save.as_ref().is_some_and(|p| p.asking())
    }

    /// A watched file has a new version: reload its document, or say so when it has unsaved
    /// changes. Stale versions never reach here: [`FileWatch::take_changes`] drops them.
    fn file_changed(&mut self, ctx: &egui::Context, change: Changed<DocId>) {
        let Some(i) = self.views.iter().position(|v| v.id == change.key) else { return };
        // LaunchServices recovery (and Finder metadata edits) changes ctime. The watch already
        // read the stable file: identical saved bytes are not a document change.
        if let Some(bytes) = &change.bytes
            && self.session.get(change.key).is_some_and(|d| d.saved_bytes().as_slice() == bytes.as_slice())
        {
            self.watch.known_as(change.key, &change.path, Some(change.stamp));
            return;
        }
        if self.has_unsaved_work(i) {
            if let Some(view) = self.views.get_mut(i) {
                view.disk_changed = true;
            }
            return;
        }
        match change.bytes {
            Some(bytes) => self.reload_view(ctx, i, bytes),
            // The watch reads every change whole (`document_watch`); one reported by its stamp
            // alone is read here.
            None => self.reload_from_disk(ctx, change.key),
        }
    }

    /// The notice bar's Reload: read the file now and replace the document, unsaved changes and
    /// all.
    fn reload_from_disk(&mut self, ctx: &egui::Context, doc: DocId) {
        let Some(i) = self.views.iter().position(|v| v.id == doc) else { return };
        let Some(path) = self.session.get(doc).and_then(|d| d.path.clone()) else { return };
        let path = Path::new(&path);
        // Taken before reading: a write that lands after the read is found as a new version.
        let stamp = FileStamp::at(path);
        match std::fs::read(path) {
            Ok(bytes) => {
                self.watch.known_as(doc, path, stamp);
                self.reload_view(ctx, i, Arc::new(bytes));
            }
            Err(e) => {
                let name = self.session.get(doc).map(|d| d.name.clone()).unwrap_or_default();
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
            }
        }
    }

    /// Open `bytes` in place of tab `i`'s document, keeping the tab and the reader's place. A
    /// version that can't be opened leaves the document as it was, and says so. The one place a
    /// reload resets what the view holds about the old version.
    fn reload_view(&mut self, ctx: &egui::Context, i: usize, bytes: Arc<Vec<u8>>) {
        let Some(id) = self.views.get(i).map(|v| v.id) else { return };
        match self.session.reload(id, bytes) {
            Ok(()) => {
                // Whatever was unsaved is gone, so there is nothing left to recover, and no
                // signature placed since the last save is left to ask about locking (#2149).
                self.forget_recovery(id);
                self.placed_signatures.remove(&id);
                if let (Some(view), Some(doc)) = (self.views.get_mut(i), self.session.get(id)) {
                    view.document_replaced(ctx, &doc.info);
                }
                // What the form's scripts said while it opened, as on opening.
                let out = self.session.take_js_output(id);
                self.handle_js(ctx, id, out);
            }
            Err(e) => {
                let name = self.session.get(id).map(|d| d.name.clone()).unwrap_or_default();
                self.notify_fmt("Couldn't reload {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use craft::host::{AppInfo, Host, ManualJobs, NoPickFiles, Wake};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::SaveTarget;

    /// An `n`-page PDF with a proper xref table.
    fn fixture(n: usize) -> Vec<u8> {
        let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
        for _ in 0..n {
            objs.push("<< /Type /Page /Parent 2 0 R >>".into());
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// A fresh temporary folder with `thesis.pdf` in it, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(test: &str, bytes: &[u8]) -> (Self, String) {
            let dir = std::env::temp_dir().join(format!("pdfcraft-watch-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("thesis.pdf").to_string_lossy().into_owned();
            std::fs::write(&path, bytes).unwrap();
            (Self(dir), path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Rewrite `path` as a build does, with a later modification time.
    fn rebuild(path: &str, bytes: &[u8], later: u64) {
        std::fs::write(path, bytes).unwrap();
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(60 * later)).unwrap();
    }

    /// The app with `path` open, on a host whose files go to the disk without a picker and whose
    /// jobs run between frames, watching through an inline watch: each pass looks once.
    struct Rig {
        h: Harness<'static, PdfCraftApp>,
        jobs: Arc<ManualJobs>,
    }

    impl Rig {
        fn new(path: &str) -> Self {
            let jobs = Arc::new(ManualJobs::new());
            let info = AppInfo { id: "pdfcraft", name: "PdfCraft", version: env!("CARGO_PKG_VERSION") };
            let host = Arc::new(Host::builder(info).jobs(jobs.clone()).files(Arc::new(NoPickFiles::default())).build());
            let (intake, _) = craft::platform::open::FileIntake::new(Wake::none());
            let clipboard = craft::platform::clipboard::Clipboard::new(craft::platform::clipboard::EventClipboard::new(|| 0, |_| Ok(0)));
            // PLAN: assumed `AppServices::new(wake, clipboard, recovery_dir, update_source, intake, watch)` (L-207, pdf-hub-w1)
            let services = crate::AppServices::new(Wake::none(), clipboard, None, None, intake, FileWatch::new(Wake::none()).inline());
            let path = path.to_owned();
            let h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
                crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
                {
                    let mut app = PdfCraftApp::with_host(host, services);
                    app.set_option(&cc.egui_ctx, "language", "en").unwrap();
                    app.open_path(&path);
                    app
                }
            });
            let mut rig = Self { h, jobs };
            rig.steps(4);
            rig
        }

        /// `n` frames, running the jobs the app queued before each one.
        fn steps(&mut self, n: usize) {
            for _ in 0..n {
                self.jobs.run_all();
                self.h.run_steps(1);
            }
        }

        fn app(&mut self) -> &mut PdfCraftApp {
            self.h.state_mut()
        }

        fn doc(&self) -> &pdfcraft_engine::Document {
            let app = self.h.state();
            app.session.get(app.views[0].id).unwrap()
        }
    }

    /// #2348 review (CodeRabbit): a reload from disk replaces the document, so a signature
    /// placed since the last save is gone and the next save doesn't ask to lock it. A new
    /// version that can't be opened keeps the document, and the mark with it.
    #[test]
    fn reloading_from_disk_forgets_a_signature_just_placed() {
        let (_dir, path) = TempDir::new("lock-reload", &fixture(1));
        let mut r = Rig::new(&path);
        let id = r.app().views[0].id;
        r.app().placed_signatures.insert(id);

        // A build that left a broken file: found, read, and refused by the engine.
        rebuild(&path, b"not a pdf\n%%EOF\n", 1);
        r.steps(4);
        assert_eq!(r.doc().info.pages.len(), 1, "the broken version was not applied");
        assert!(r.app().placed_signatures.contains(&id), "a failed reload keeps the mark");

        rebuild(&path, &fixture(2), 2);
        r.steps(4);
        assert_eq!(r.doc().info.pages.len(), 2, "the new version was reloaded");
        assert!(!r.app().placed_signatures.contains(&id), "the reloaded document has no placed signature");
    }

    /// #1802: the watch found version B of the file, then the user saved version C over it
    /// before B was applied. B is older than the document now: it is not applied, the undo history
    /// stays, and C stays on disk.
    #[test]
    fn a_version_found_before_a_save_is_not_applied_over_it() {
        let (_dir, path) = TempDir::new("stale-save", &fixture(2));
        let mut r = Rig::new(&path);
        // B lands and the watch finds it (one look sees it, the next sees it settled and reads
        // it) before a pass takes it.
        rebuild(&path, &fixture(3), 1);
        r.app().watch.tick();
        r.app().watch.tick();

        let app = r.app();
        app.views[0].select_pages(&[0]);
        app.apply_edit(&egui::Context::default(), pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        // B is on disk, not the A the document came from: Save asks first (#2166), and the
        // user saves C over it.
        assert!(!app.save_active(&egui::Context::default(), SaveTarget::InPlace), "asks before writing over B");
        r.steps(2);
        assert_eq!(r.doc().info.pages.len(), 2, "B waits while the question is open");
        r.h.get_by_label("Save anyway").click();
        r.steps(4);

        let d = r.doc();
        assert!(!d.dirty, "saved version C");
        assert_eq!(d.info.pages.len(), 2, "the stale version was not applied over the saved one");
        assert!(d.can_undo().is_some(), "the saved document and its history stay");
        assert_eq!(std::fs::read(&path).unwrap(), d.saved_bytes().as_slice(), "C is still on disk");
    }
}
