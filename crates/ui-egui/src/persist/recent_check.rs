//! Recent files whose files are gone leave the list, without the window waiting to find out
//! (#5682).
//!
//! A recent file can be on a network or cloud mount that has stopped answering, where even
//! asking whether the file exists blocks in the kernel, and no timeout can cancel that. So
//! settings keep every recent entry when they load, and a worker asks about them once the window
//! is up; each frame only takes the answers it has so far. An entry the worker hasn't reached
//! yet, or can't tell about (the mount answers with an error), stays listed and can be opened or
//! removed by hand. A worker stuck on a dead mount stays stuck, but nothing waits for it.
//!
//! Opening a file that already has a tab puts it back in the list without reading it, so that
//! entry is asked about the same way (#6323).

use std::sync::{Arc, Mutex, PoisonError};

use crate::PdfCraftApp;

/// The recent files that haven't been checked yet (restored from settings, or put back by
/// focusing an open tab), and the check under way.
#[derive(Debug, Default)]
pub struct RecentCheck {
    /// Paths to check on the next frame that no check is under way.
    unchecked: Vec<String>,
    /// The paths the worker has found gone so far; it adds to them as it goes.
    gone: Option<Arc<Mutex<Vec<String>>>>,
    /// Paths opened while a check was pending: the file is there now, whatever the worker found.
    opened: Vec<String>,
}

impl RecentCheck {
    /// Settings just restored these recent files: check them once the window is up.
    pub(crate) fn restored(&mut self, paths: Vec<String>) {
        self.unchecked = paths;
    }

    /// A tab showing `path` put it back in the list without reading the file: check it is still
    /// there, as for a restored entry. An earlier open no longer vouches for it (the file may
    /// have gone since), so the check this queues decides.
    pub(crate) fn focused(&mut self, path: &str) {
        self.opened.retain(|p| p != path);
        if !self.unchecked.iter().any(|p| p == path) {
            self.unchecked.push(path.to_string());
        }
    }

    /// `path` was just opened: an answer the worker gives about it later is out of date.
    pub(crate) fn opened(&mut self, path: &str) {
        if self.gone.is_some() || !self.unchecked.is_empty() {
            self.opened.push(path.to_string());
        }
    }
}

/// Whether `path` is known to be gone. Any other answer, an error included, keeps the entry.
fn gone(path: &str) -> bool {
    matches!(std::fs::exists(path), Ok(false))
}

impl PdfCraftApp {
    /// Start checking the recent files restored from settings, and drop the ones found gone so
    /// far (called each frame). The check runs on a worker thread, inline in tests
    /// ([`PdfCraftApp::run_inline`]).
    pub(crate) fn check_restored_recent(&mut self) {
        // One check at a time: paths queued while a worker runs wait for it to finish, so its
        // answers aren't dropped with its slot.
        let paths = if self.recent_check.gone.is_none() { std::mem::take(&mut self.recent_check.unchecked) } else { Vec::new() };
        // The web build's recent files aren't paths on a file system.
        if !paths.is_empty() && !cfg!(target_arch = "wasm32") {
            let slot = Arc::new(Mutex::new(Vec::new()));
            let out = slot.clone();
            let ctx = self.ctx.clone();
            let work = move || {
                for path in paths {
                    if gone(&path) {
                        out.lock().unwrap_or_else(PoisonError::into_inner).push(path);
                        if let Some(ctx) = &ctx {
                            ctx.request_repaint();
                        }
                    }
                }
            };
            if self.run_inline {
                work();
            } else if std::thread::Builder::new().name("pdfcraft-recent".into()).spawn(work).is_err() {
                // No worker: the entries stay listed, as if the check hadn't reached them.
                return;
            }
            self.recent_check.gone = Some(slot);
        }
        let Some(slot) = &self.recent_check.gone else { return };
        // Asked before taking, so a path the worker adds after this is taken on a later frame.
        let finished = Arc::strong_count(slot) == 1;
        let gone = std::mem::take(&mut *slot.lock().unwrap_or_else(PoisonError::into_inner));
        if !gone.is_empty() {
            // A file opened since the check began is there, whatever the worker found earlier,
            // and one queued for a later check is left for that check to decide.
            let RecentCheck { opened, unchecked, .. } = &self.recent_check;
            self.recent.retain(|r| !gone.contains(&r.path) || opened.contains(&r.path) || unchecked.contains(&r.path));
        }
        // The worker has finished (or panicked) and everything it found is taken.
        if finished {
            self.recent_check.gone = None;
            self.recent_check.opened.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file opened after the worker found it gone, but before that answer was taken, stays
    /// listed (CodeRabbit on #5692).
    #[test]
    fn a_file_opened_while_the_check_is_pending_stays() {
        let mut app = PdfCraftApp::new();
        app.recent = vec![crate::state::RecentFile { name: "a.pdf".into(), path: "/gone/a.pdf".into(), pages: 1, size: 0 }];
        // The worker's answer is waiting: it found the file gone.
        app.recent_check.gone = Some(Arc::new(Mutex::new(vec!["/gone/a.pdf".to_string()])));
        app.recent_check.opened("/gone/a.pdf");
        app.check_restored_recent();
        assert_eq!(app.recent.len(), 1, "the reopened file stays");
        assert!(app.recent_check.gone.is_none() && app.recent_check.opened.is_empty());
        // Without the open, the answer removes it.
        app.recent_check.gone = Some(Arc::new(Mutex::new(vec!["/gone/a.pdf".to_string()])));
        app.check_restored_recent();
        assert!(app.recent.is_empty());
    }

    /// Focusing a tab after its file was opened and then deleted ends that open's exemption:
    /// the check the focus queued removes the row (CodeRabbit on #6323).
    #[test]
    fn a_focus_after_an_open_lets_the_check_decide() {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        let row = |path: &str| crate::state::RecentFile { name: "x.pdf".into(), path: path.into(), pages: 1, size: 0 };
        app.recent = vec![row("/gone/b.pdf")];
        // Another focus is waiting for its check when b.pdf is opened; b.pdf then goes and its
        // tab is focused, all before the next frame.
        app.recent_check.focused("/gone/a.pdf");
        app.recent_check.opened("/gone/b.pdf");
        app.recent_check.focused("/gone/b.pdf");
        app.check_restored_recent();
        assert!(app.recent.is_empty(), "the earlier open does not keep a file that has gone since");
        // An open after the focus still vouches for the file.
        app.check_restored_recent();
        app.recent = vec![row("/gone/b.pdf")];
        app.recent_check.focused("/gone/b.pdf");
        app.recent_check.opened("/gone/b.pdf");
        app.check_restored_recent();
        assert_eq!(app.recent.len(), 1);
    }

    /// An answer from a check already under way does not remove a row that a later focus put
    /// back and queued again: the queued check decides (CodeRabbit on #6323).
    #[test]
    fn an_older_answer_leaves_a_requeued_row_to_its_own_check() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-recent-requeue-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let back = dir.join("back.pdf").to_string_lossy().into_owned();
        let lost = dir.join("lost.pdf").to_string_lossy().into_owned();
        std::fs::write(&back, b"%PDF-1.7").unwrap();
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.recent = [&back, &lost].map(|p| crate::state::RecentFile { name: "x.pdf".into(), path: p.clone(), pages: 1, size: 0 }).into();
        // The worker found both gone; back.pdf was recreated and both tabs focused before the
        // answer was taken.
        app.recent_check.gone = Some(Arc::new(Mutex::new(vec![back.clone(), lost.clone()])));
        app.recent_check.focused(&back);
        app.recent_check.focused(&lost);
        app.check_restored_recent();
        assert_eq!(app.recent.len(), 2, "the older answer waits for the queued check");
        app.check_restored_recent();
        assert_eq!(
            app.recent.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            [back.as_str()],
            "the queued check keeps what is there and drops what is not"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_a_path_known_to_be_missing_is_gone() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-recent-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let here = dir.join("here.pdf");
        std::fs::write(&here, b"%PDF-1.7").unwrap();
        assert!(!gone(&here.to_string_lossy()));
        assert!(gone(&dir.join("missing.pdf").to_string_lossy()));
        // A path that can't be asked about (here: one with a NUL) is not known to be gone.
        assert!(!gone("bad\0path.pdf"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
