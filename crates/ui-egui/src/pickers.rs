//! Native file pickers that never block the frame.
//!
//! A blocking `rfd::FileDialog` runs `-[NSOpenPanel runModal]` on macOS: a nested run loop inside
//! winit's event handler. An event that arrives while it spins re-enters the handler, winit
//! panics, and because the panic can't unwind out of the AppKit callback the app aborts.
//! `rfd::AsyncFileDialog` shows the panel without a nested run loop and returns at once instead.
//! A worker thread waits for the choice, and the app uses it on a later frame
//! ([`PdfCraftApp::process_picked`]). The panel has no parent window yet, like every other
//! picker in the app, so it floats rather than opening as a sheet.

use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pdfcraft_engine::DocId;

use crate::PdfCraftApp;
use crate::files::FilePurpose;

/// What the chosen files are for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickFor {
    /// File ▸ Open: open the chosen file in a new tab.
    Open,
    /// Combine, insert, replace or OCR (see [`FilePurpose`]).
    Files(FilePurpose),
}

/// A finished pick, waiting for the next frame.
#[derive(Debug)]
struct Picked {
    pick_for: PickFor,
    /// The document the pick acts on, for purposes that edit the active document.
    target: Option<DocId>,
    /// The chosen files; empty when the picker was cancelled.
    paths: Vec<PathBuf>,
}

/// Pickers in flight and the picks they finished.
#[derive(Clone, Debug, Default)]
pub struct Pickers {
    /// A picker is showing; a second request is ignored until it closes.
    showing: Arc<AtomicBool>,
    done: Arc<Mutex<Vec<Picked>>>,
}

/// Clears `showing` when the worker ends, including when it panics.
struct Showing(Arc<AtomicBool>);

impl Drop for Showing {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Pickers {
    /// Wait for `pick` on a worker thread and queue its result for the next frame. `pick` must
    /// already have shown the picker (rfd starts the panel when the future is created, on the
    /// main thread). Returns false when the worker couldn't start.
    fn spawn<F>(&self, pick_for: PickFor, target: Option<DocId>, ctx: Option<egui::Context>, pick: F) -> bool
    where
        F: Future<Output = Vec<PathBuf>> + Send + 'static,
    {
        let showing = Showing(self.showing.clone());
        let done = self.done.clone();
        let started = std::thread::Builder::new().name("file-picker".into()).spawn(move || {
            let _showing = showing;
            let paths = pollster::block_on(pick);
            done.lock().unwrap_or_else(PoisonError::into_inner).push(Picked { pick_for, target, paths });
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        started.is_ok()
    }

    /// Queue a pick that is already known (tests and automation).
    fn deliver(&self, pick_for: PickFor, target: Option<DocId>, paths: Vec<PathBuf>) {
        self.done.lock().unwrap_or_else(PoisonError::into_inner).push(Picked { pick_for, target, paths });
    }

    fn take(&self) -> Vec<Picked> {
        std::mem::take(&mut *self.done.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl PdfCraftApp {
    /// Show a native picker for `pick_for` without blocking the frame. The choice is used on a
    /// later frame by [`Self::process_picked`].
    pub(crate) fn pick(&mut self, pick_for: PickFor, dialog: rfd::AsyncFileDialog, multiple: bool) {
        if self.pickers.showing.swap(true, Ordering::SeqCst) {
            return;
        }
        // Insert and Replace edit the active document: remember which one.
        let target = match pick_for {
            PickFor::Files(FilePurpose::InsertPages | FilePurpose::ReplacePages) => self.active_ids().map(|(_, id)| id),
            _ => None,
        };
        if let Some(paths) = self.pick_override.clone() {
            self.pickers.showing.store(false, Ordering::SeqCst);
            self.pickers.deliver(pick_for, target, paths.into_iter().map(PathBuf::from).collect());
            return;
        }
        // Creating the future shows the panel; it must happen here, on the main thread.
        let started = if multiple {
            let pick = dialog.pick_files();
            self.pickers.spawn(pick_for, target, self.ctx.clone(), async move {
                pick.await.unwrap_or_default().into_iter().map(|h| h.path().to_path_buf()).collect()
            })
        } else {
            let pick = dialog.pick_file();
            self.pickers.spawn(pick_for, target, self.ctx.clone(), async move { pick.await.map(|h| h.path().to_path_buf()).into_iter().collect() })
        };
        if !started {
            self.pickers.showing.store(false, Ordering::SeqCst);
            self.notify("Couldn't show the file picker. Please try again.");
        }
    }

    /// Use the picks that finished since the last frame. Called every frame.
    pub(crate) fn process_picked(&mut self) {
        for Picked { pick_for, target, paths } in self.pickers.take() {
            if paths.is_empty() {
                continue;
            }
            if target.is_some() && self.active_ids().map(|(_, id)| id) != target {
                self.notify("The document changed while you were choosing a file, so nothing was added.");
                continue;
            }
            match pick_for {
                PickFor::Open => paths.iter().for_each(|p| self.open_path(&p.to_string_lossy())),
                PickFor::Files(purpose) => self.use_paths(purpose, &paths),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_queues_the_pick_and_clears_showing() {
        let pickers = Pickers::default();
        pickers.showing.store(true, Ordering::SeqCst);
        assert!(pickers.spawn(PickFor::Open, None, None, async { vec![PathBuf::from("a.pdf")] }));
        for _ in 0..500 {
            if !pickers.showing.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!pickers.showing.load(Ordering::SeqCst));
        let picked = pickers.take();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].paths, vec![PathBuf::from("a.pdf")]);
    }

    #[test]
    fn a_panicking_worker_clears_showing_and_queues_nothing() {
        let pickers = Pickers::default();
        pickers.showing.store(true, Ordering::SeqCst);
        fn fail() -> Vec<PathBuf> {
            panic!("picker failed")
        }
        assert!(pickers.spawn(PickFor::Open, None, None, async { fail() }));
        for _ in 0..500 {
            if !pickers.showing.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!pickers.showing.load(Ordering::SeqCst));
        assert!(pickers.take().is_empty());
    }
}
