//! Recent files whose files are gone leave the list, without the window waiting to find out
//! (#5682).
//!
//! A recent file can be on a network or cloud mount that has stopped answering, where even
//! asking whether the file exists blocks in the kernel, and no timeout can cancel that. So
//! settings keep every recent entry when they load, and a worker asks about them once the window
//! is up; each frame only takes the answers it has so far. An entry the worker hasn't reached
//! yet, or can't tell about (the mount answers with an error), stays listed and can be opened or
//! removed by hand. A worker stuck on a dead mount stays stuck, but nothing waits for it.

use std::sync::{Arc, Mutex, PoisonError};

use crate::PdfCraftApp;

/// The recent files restored from settings that haven't been checked yet, and the check under way.
#[derive(Debug, Default)]
pub struct RecentCheck {
    /// Paths restored from settings, checked on the next frame.
    unchecked: Vec<String>,
    /// The paths the worker has found gone so far; it adds to them as it goes.
    gone: Option<Arc<Mutex<Vec<String>>>>,
}

impl RecentCheck {
    /// Settings just restored these recent files: check them once the window is up.
    pub(crate) fn restored(&mut self, paths: Vec<String>) {
        self.unchecked = paths;
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
        let paths = std::mem::take(&mut self.recent_check.unchecked);
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
            self.recent.retain(|r| !gone.contains(&r.path));
        }
        // The worker has finished (or panicked) and everything it found is taken.
        if finished {
            self.recent_check.gone = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
