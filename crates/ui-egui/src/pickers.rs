//! Native file pickers that never block the frame.
//!
//! A blocking `rfd::FileDialog` runs `-[NSOpenPanel runModal]` on macOS: a nested run loop inside
//! winit's event handler. An event that arrives while it spins re-enters the handler, winit
//! panics, and because the panic can't unwind out of the AppKit callback the app aborts.
//! `rfd::AsyncFileDialog` returns at once instead, and the app uses the choice on a later frame
//! ([`PdfCraftApp::process_picked`]); a worker thread waits for it.
//!
//! How rfd 0.17 shows the panel (checked against its source):
//! - macOS: creating the future shows the panel, so it must be created on the main thread. With
//!   no parent given, rfd uses the app's main (or first) window and opens the panel as a sheet
//!   on it (`beginSheetModalForWindow`). Without a running app and a window it falls back to the
//!   blocking `runModal`, which is why this only runs inside the running eframe app.
//! - Windows: the panel runs on a thread rfd starts when the future is first polled (here, by
//!   the worker), not inside winit's callback.
//! - Linux and the BSDs: the XDG portal backend (with a Zenity fallback) also works off the UI
//!   thread; the optional GTK3 backend shows the panel on rfd's own GTK thread.
//!
//! Every native picker goes through [`PdfCraftApp::ask`]: open, save, folder and multi-file
//! panels alike. What happens with the choice is a closure that runs on that later frame, so
//! work that depends on the file being written (closing the tab after Save, deleting extracted
//! pages) happens there and nowhere else. One picker shows at a time, and it keeps that claim
//! until its closure starts, so a closure that asks again (merge data, then save the
//! spreadsheet) always gets its second picker.

use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pdfcraft_engine::DocId;

use crate::PdfCraftApp;
use crate::files::{FilePurpose, PickTarget};

/// What the chosen files are for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickFor {
    /// File ▸ Open: open the chosen file in a new tab.
    Open,
    /// Combine, insert, replace or OCR (see [`FilePurpose`]).
    Files(FilePurpose),
}

/// Which native panel to show.
pub(crate) enum Ask {
    /// Choose one existing file.
    File(rfd::AsyncFileDialog),
    /// Choose one or more existing files.
    Files(rfd::AsyncFileDialog),
    /// Choose a folder.
    Folder(rfd::AsyncFileDialog),
    /// Choose where to save a file.
    Save(rfd::AsyncFileDialog),
}

/// What to do with the chosen paths, on a later frame.
type Then = Box<dyn FnOnce(&mut PdfCraftApp, Vec<PathBuf>) + Send>;

/// A finished pick, waiting for the next frame.
struct Picked {
    target: Option<PickTarget>,
    /// The chosen paths; empty when the picker was cancelled.
    paths: Vec<PathBuf>,
    then: Then,
}

impl std::fmt::Debug for Picked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picked").field("target", &self.target).field("paths", &self.paths).finish_non_exhaustive()
    }
}

/// Pickers in flight and the picks they finished.
#[derive(Clone, Debug, Default)]
pub struct Pickers {
    /// A picker is showing, or its pick is waiting for the frame; a second request is refused
    /// until the pick's closure starts.
    showing: Arc<AtomicBool>,
    done: Arc<Mutex<Vec<Picked>>>,
}

/// Clears `showing` if the worker ends without queueing a pick (it panicked).
struct Showing {
    flag: Option<Arc<AtomicBool>>,
    ctx: Option<egui::Context>,
}

impl Showing {
    /// The pick is queued: the frame that uses it clears the flag.
    fn hand_over(mut self) {
        self.flag = None;
    }
}

impl Drop for Showing {
    fn drop(&mut self) {
        if let Some(flag) = &self.flag {
            flag.store(false, Ordering::SeqCst);
            // No picked result will wake the frame to resume opens behind this picker.
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint();
            }
        }
    }
}

impl Pickers {
    /// A picker is showing or its result is waiting for the frame.
    pub(crate) fn pending(&self) -> bool {
        self.showing.load(Ordering::SeqCst)
    }

    /// Wait for `pick` on a worker thread and queue its result for the next frame. On macOS
    /// `pick` must already have shown the picker (rfd starts the panel when the future is
    /// created, on the main thread). Returns false when the worker couldn't start.
    fn spawn<F>(&self, target: Option<PickTarget>, ctx: Option<egui::Context>, pick: F, then: Then) -> bool
    where
        F: Future<Output = Vec<PathBuf>> + Send + 'static,
    {
        self.spawn_worker(target, ctx, pick, then).is_some()
    }

    /// [`Self::spawn`], returning the worker so tests can wait for it instead of polling.
    fn spawn_worker<F>(&self, target: Option<PickTarget>, ctx: Option<egui::Context>, pick: F, then: Then) -> Option<std::thread::JoinHandle<()>>
    where
        F: Future<Output = Vec<PathBuf>> + Send + 'static,
    {
        let showing = Showing { flag: Some(self.showing.clone()), ctx: ctx.clone() };
        let done = self.done.clone();
        std::thread::Builder::new()
            .name("file-picker".into())
            .spawn(move || {
                let paths = pollster::block_on(pick);
                done.lock().unwrap_or_else(PoisonError::into_inner).push(Picked { target, paths, then });
                showing.hand_over();
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            })
            .ok()
    }

    /// Queue a pick that is already known (tests and automation). Like a real pick, it keeps
    /// the flag until the frame uses it.
    fn deliver(&self, target: Option<PickTarget>, paths: Vec<PathBuf>, then: Then) {
        self.done.lock().unwrap_or_else(PoisonError::into_inner).push(Picked { target, paths, then });
    }

    fn take(&self) -> Vec<Picked> {
        std::mem::take(&mut *self.done.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl PdfCraftApp {
    /// Show a native picker for `pick_for` without blocking the frame. The choice is used on a
    /// later frame by [`Self::process_picked`].
    pub(crate) fn pick(&mut self, pick_for: PickFor, dialog: rfd::AsyncFileDialog, multiple: bool) {
        // Insert and Replace edit the active document: remember which one.
        let target = match pick_for {
            PickFor::Files(purpose) if purpose.edits_active_document() => self.active_ids().map(|(_, id)| id),
            _ => None,
        };
        let ask = if multiple { Ask::Files(dialog) } else { Ask::File(dialog) };
        self.ask(ask, target, move |app, paths| match pick_for {
            PickFor::Open => paths.iter().for_each(|p| app.open_path(&p.to_string_lossy())),
            PickFor::Files(purpose) => app.use_paths(purpose, &paths),
        });
    }

    /// Show `ask` without blocking the frame, and call `then` with the chosen paths on a later
    /// frame ([`Self::process_picked`]). Returns false, after telling the user, when no picker
    /// could be shown: another one is still open, or its worker couldn't start.
    ///
    /// `then` is not called when the picker is cancelled, or when `target` is given and by then
    /// that document is no longer the active one or has been edited (the user is told). Use
    /// `target` whenever `then` edits the active document or refers to its pages or objects by
    /// position.
    ///
    /// `pick_override` answers instead of a native panel (tests and automation); the answer
    /// still arrives on a later frame, like a real pick.
    pub(crate) fn ask(&mut self, ask: Ask, target: Option<DocId>, then: impl FnOnce(&mut PdfCraftApp, Vec<PathBuf>) + Send + 'static) -> bool {
        if self.pickers.showing.swap(true, Ordering::SeqCst) {
            self.notify_tr("Another file dialog is still open. Finish with it first.");
            return false;
        }
        let target = self.pick_target(target);
        let then: Then = Box::new(then);
        if let Some(paths) = self.pick_override.clone() {
            self.pickers.deliver(target, paths.into_iter().map(PathBuf::from).collect(), then);
            return true;
        }
        // On macOS creating the future shows the panel; it must happen here, on the main thread.
        let ctx = self.ctx.clone();
        let one = |h: Option<rfd::FileHandle>| h.map(|h| h.path().to_path_buf()).into_iter().collect::<Vec<_>>();
        let started = match ask {
            Ask::File(d) => {
                let pick = d.pick_file();
                self.pickers.spawn(target, ctx, async move { one(pick.await) }, then)
            }
            Ask::Folder(d) => {
                let pick = d.pick_folder();
                self.pickers.spawn(target, ctx, async move { one(pick.await) }, then)
            }
            Ask::Save(d) => {
                let pick = d.save_file();
                self.pickers.spawn(target, ctx, async move { one(pick.await) }, then)
            }
            Ask::Files(d) => {
                let pick = d.pick_files();
                self.pickers.spawn(
                    target,
                    ctx,
                    async move { pick.await.unwrap_or_default().into_iter().map(|h| h.path().to_path_buf()).collect() },
                    then,
                )
            }
        };
        if !started {
            self.pickers.showing.store(false, Ordering::SeqCst);
            self.notify_tr("Couldn't show the file picker. Please try again.");
        }
        started
    }

    /// [`Self::ask`] for one path: `then` gets the first chosen path.
    pub(crate) fn ask_one(&mut self, ask: Ask, target: Option<DocId>, then: impl FnOnce(&mut PdfCraftApp, PathBuf) + Send + 'static) -> bool {
        self.ask(ask, target, move |app, paths| {
            if let Some(path) = paths.into_iter().next() {
                then(app, path);
            }
        })
    }

    /// Use the picks that finished since the last frame. Called every frame.
    pub(crate) fn process_picked(&mut self) {
        for Picked { target, paths, then } in self.pickers.take() {
            // The pick is in hand: the next picker may show, including one `then` asks for.
            self.pickers.showing.store(false, Ordering::SeqCst);
            // Opens were polled before this pick was consumed. Wake their retained replies
            // once, including after cancellation, without polling while a picker waits.
            if !self.pending_opens.is_empty()
                && let Some(ctx) = &self.ctx
            {
                ctx.request_repaint();
            }
            if paths.is_empty() {
                continue;
            }
            if !self.still_pick_target(target) {
                continue;
            }
            // Last-resort guard (AGENTS.md §4), as for commands: this work used to run inside
            // the command that asked, and a panic in it must not take the app down.
            if let Err(m) = pdfcraft_engine::guard(|| then(self, paths)) {
                self.notify_fmt("That didn't work: an internal error stopped it ({m}).", &[("m", m.as_str())]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(into: &Arc<Mutex<Vec<PathBuf>>>) -> Then {
        let into = into.clone();
        Box::new(move |_, paths| into.lock().unwrap().extend(paths))
    }

    #[test]
    fn worker_queues_the_pick_and_keeps_the_flag_until_the_frame_uses_it() {
        let mut app = PdfCraftApp::new();
        app.pickers.showing.store(true, Ordering::SeqCst);
        let got = Arc::new(Mutex::new(Vec::new()));
        let worker = app.pickers.spawn_worker(None, None, async { vec![PathBuf::from("a.pdf")] }, record(&got)).expect("worker starts");
        // Wait for the worker itself: polling with a deadline was flaky on loaded CI runners.
        assert!(worker.join().is_ok());
        assert!(app.pickers.showing.load(Ordering::SeqCst), "the queued pick still owns the flag");
        app.process_picked();
        assert!(!app.pickers.showing.load(Ordering::SeqCst));
        assert_eq!(*got.lock().unwrap(), vec![PathBuf::from("a.pdf")]);
    }

    #[test]
    fn a_panicking_worker_releases_picker_and_requests_repaint() {
        let pickers = Pickers::default();
        pickers.showing.store(true, Ordering::SeqCst);
        let ctx = egui::Context::default();
        // Consume the context's initial repaint demand so only the failed worker can wake it.
        for _ in 0..2 {
            let _ = ctx.run_logic(&egui::RawInput::default(), |_| {});
        }
        assert!(!ctx.has_requested_repaint());
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requested = wakes.clone();
        ctx.set_request_repaint_callback(move |_| {
            requested.fetch_add(1, Ordering::SeqCst);
        });
        fn fail() -> Vec<PathBuf> {
            panic!("picker failed")
        }
        let got = Arc::new(Mutex::new(Vec::new()));
        let worker = pickers.spawn_worker(None, Some(ctx.clone()), async { fail() }, record(&got)).expect("worker starts");
        // join returns once the panic has unwound and the drop guard has run.
        assert!(worker.join().is_err());
        assert!(!pickers.showing.load(Ordering::SeqCst));
        assert!(pickers.take().is_empty());
        assert!(ctx.has_requested_repaint(), "retained opens must get another frame after a picker worker fails");
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "an abandoned picker requests one wake");
    }

    #[test]
    fn the_follow_up_runs_on_a_later_frame_and_not_when_cancelled() {
        let mut app = PdfCraftApp::new();
        let got = Arc::new(Mutex::new(Vec::new()));
        app.pickers.deliver(None, vec![], record(&got));
        app.pickers.deliver(None, vec![PathBuf::from("b.pdf")], record(&got));
        assert!(got.lock().unwrap().is_empty(), "nothing runs until the frame uses the picks");
        app.process_picked();
        assert_eq!(*got.lock().unwrap(), vec![PathBuf::from("b.pdf")], "a cancelled pick runs nothing");
    }

    #[test]
    fn cancellation_requests_a_frame_for_a_retained_open_reply() {
        let mut app = PdfCraftApp::new();
        let ctx = egui::Context::default();
        app.ctx = Some(ctx.clone());
        for _ in 0..2 {
            let _ = ctx.run_logic(&egui::RawInput::default(), |_| {});
        }
        assert!(!ctx.has_requested_repaint());
        let (reply, receiver) = std::sync::mpsc::channel();
        reply
            .send(crate::open_worker::ResultMessage { token: 1, kind: crate::open_worker::ResultKind::ReadFailed("controlled failure".into()) })
            .unwrap();
        app.pending_opens.push(crate::PendingOpen {
            token: 1,
            receiver,
            name: "waiting.pdf".into(),
            path: None,
            stamp: None,
            activate: true,
            after_open: None,
            recovered: None,
            then: None,
        });
        let got = Arc::new(Mutex::new(Vec::new()));
        app.pickers.showing.store(true, Ordering::SeqCst);
        app.pickers.deliver(None, vec![], record(&got));
        app.poll_pending_opens();
        assert_eq!(app.pending_opens.len(), 1, "the picker keeps the ready reply queued");
        app.process_picked();
        assert!(!app.pickers.showing.load(Ordering::SeqCst));
        assert!(got.lock().unwrap().is_empty(), "cancellation runs no callback");
        assert!(ctx.has_requested_repaint(), "cancellation must wake the earlier blocked open poll");
        app.poll_pending_opens();
        assert!(app.pending_opens.is_empty(), "the retained ready reply is consumed on the next poll");
    }

    #[test]
    fn a_second_picker_is_refused_while_one_is_showing() {
        let mut app = PdfCraftApp::new();
        app.pick_override = Some(vec!["c.pdf".into()]);
        app.pickers.showing.store(true, Ordering::SeqCst);
        let got = Arc::new(Mutex::new(Vec::new()));
        let into = got.clone();
        assert!(!app.ask(Ask::File(rfd::AsyncFileDialog::new()), None, move |_, paths| into.lock().unwrap().extend(paths)));
        app.process_picked();
        assert!(got.lock().unwrap().is_empty());
        assert!(app.pickers.showing.load(Ordering::SeqCst), "the showing picker still owns the flag");
        assert!(app.toast.as_ref().is_some_and(|(m, _)| m.contains("Another file dialog")), "the user is told");
    }

    #[test]
    fn a_follow_up_that_asks_again_gets_its_picker_even_if_another_request_came_first() {
        let mut app = PdfCraftApp::new();
        app.pick_override = Some(vec!["first.fdf".into()]);
        let got = Arc::new(Mutex::new(Vec::new()));
        let into = got.clone();
        assert!(app.ask(Ask::Files(rfd::AsyncFileDialog::new()), None, move |app, _| {
            // Merge data: the spreadsheet's save picker, asked from the first pick's closure.
            assert!(app.ask(Ask::Save(rfd::AsyncFileDialog::new()), None, move |_, paths| into.lock().unwrap().extend(paths)));
        }));
        // Another command asks for a picker before the frame uses the first pick: refused.
        assert!(!app.ask(Ask::File(rfd::AsyncFileDialog::new()), None, |_, _| panic!("must not run")));
        app.process_picked();
        app.process_picked();
        assert_eq!(*got.lock().unwrap(), vec![PathBuf::from("first.fdf")], "the chained picker answered");
    }

    #[test]
    fn a_panicking_follow_up_is_reported_and_the_next_pick_still_runs() {
        let mut app = PdfCraftApp::new();
        let got = Arc::new(Mutex::new(Vec::new()));
        app.pickers.deliver(None, vec![PathBuf::from("x.pdf")], Box::new(|_, _| panic!("follow-up failed")));
        app.pickers.deliver(None, vec![PathBuf::from("y.pdf")], record(&got));
        app.process_picked();
        assert_eq!(*got.lock().unwrap(), vec![PathBuf::from("y.pdf")]);
        let toast = app.toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
        assert!(toast.contains("internal error"), "the user is told: {toast:?}");
        assert!(!app.pickers.showing.load(Ordering::SeqCst));
    }
}
