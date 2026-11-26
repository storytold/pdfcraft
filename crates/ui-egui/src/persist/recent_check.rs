//! Recent files whose files are gone leave the list, without the window waiting to find out
//! (#5682).
//!
//! A recent file can be on a network or cloud mount that has stopped answering, where even
//! asking whether the file exists blocks in the kernel, and no timeout can cancel that. So
//! settings keep every recent entry when they load, and one background job per path asks about
//! them once the window is up; each frame only takes the answers it has so far. An entry the job
//! hasn't reached yet, or can't tell about (the mount answers with an error), stays listed and can
//! be opened or removed by hand. A job stuck on a dead mount stays stuck, but nothing waits for it.

use craft::host::queue::{JobQueue, Priority};

use crate::PdfCraftApp;

/// The recent files restored from settings that haven't been checked yet, and the checks under way.
pub struct RecentCheck {
    /// Paths restored from settings, submitted on the next frame.
    unchecked: Vec<String>,
    /// One job per path; its answer is whether the file is known to be gone.
    checks: JobQueue<String, bool>,
    /// Paths opened while a check was pending: the file is there now, whatever the job found.
    opened: Vec<String>,
}

impl RecentCheck {
    /// A check whose jobs run on `checks`.
    pub(crate) fn new(checks: JobQueue<String, bool>) -> Self {
        Self { unchecked: Vec::new(), checks, opened: Vec::new() }
    }

    /// Settings just restored these recent files: check them once the window is up.
    pub(crate) fn restored(&mut self, paths: Vec<String>) {
        self.unchecked = paths;
    }

    /// Whether a job is waiting, running or has an answer not yet taken.
    fn pending(&self) -> bool {
        let counts = self.checks.counts();
        counts.queued + counts.running + counts.finished > 0
    }

    /// `path` was just opened: an answer a job gives about it later is out of date.
    pub(crate) fn opened(&mut self, path: &str) {
        if self.pending() || !self.unchecked.is_empty() {
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
    /// far (called each frame).
    pub(crate) fn check_restored_recent(&mut self) {
        let paths = std::mem::take(&mut self.recent_check.unchecked);
        // The web build's recent files aren't paths on a file system.
        if !cfg!(target_arch = "wasm32") {
            for path in paths {
                let asked = path.clone();
                self.recent_check.checks.submit(path, Priority::BACKGROUND, move |_| gone(&asked));
            }
        }
        while let Some(answer) = self.recent_check.checks.try_recv() {
            // A file opened since the check began is there, whatever the job found.
            if answer.result && !self.recent_check.opened.contains(&answer.key) {
                self.recent.retain(|r| r.path != answer.key);
            }
        }
        if !self.recent_check.pending() {
            self.recent_check.opened.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use craft::host::queue::QueueOptions;
    use craft::host::{ManualJobs, Wake};

    use super::*;

    /// A file opened after its job found it gone, but before that answer was taken, stays listed
    /// (CodeRabbit on #5692).
    #[test]
    fn a_file_opened_while_the_check_is_pending_stays() {
        let mut app = PdfCraftApp::for_unit_tests();
        // No lanes: a job runs only when the test runs it.
        let queue = JobQueue::new(Arc::new(ManualJobs::new()), Wake::none(), QueueOptions { lanes: 0, ..QueueOptions::default() });
        app.recent_check = RecentCheck::new(queue);
        app.recent = vec![crate::state::RecentFile { name: "a.pdf".into(), path: "/gone/a.pdf".into(), pages: 1, size: 0 }];
        app.recent_check.restored(vec!["/gone/a.pdf".into()]);
        app.check_restored_recent();
        app.recent_check.opened("/gone/a.pdf");
        assert_eq!(app.recent_check.checks.run_inline(10), 1);
        app.check_restored_recent();
        assert_eq!(app.recent.len(), 1, "the reopened file stays");
        assert!(app.recent_check.opened.is_empty());
        // Without the open, the answer removes it.
        app.recent_check.restored(vec!["/gone/a.pdf".into()]);
        app.check_restored_recent();
        app.recent_check.checks.run_inline(10);
        app.check_restored_recent();
        assert!(app.recent.is_empty());
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
