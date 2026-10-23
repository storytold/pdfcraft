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
//!
//! An empty answer is a Cancel, unless no panel could show at all (on Linux, a desktop portal
//! whose file chooser isn't running, #3988; see [`pdfcraft_platform::file_dialog_probe`]). Then
//! what asked is kept, finished work such as a Reduce File Size copy included, and the user is
//! asked whether to try again once the file chooser works ([`PdfCraftApp::resolve_picker_failed`]).

use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pdfcraft_engine::DocId;
use pdfcraft_platform::file_dialog_probe::Problem;

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
#[derive(Clone, Debug)]
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
    /// The chosen paths; empty when the picker was cancelled or couldn't show.
    paths: Vec<PathBuf>,
    then: Then,
    /// Why no panel could show, when the answer was empty for that reason and not a Cancel.
    why: Option<Problem>,
}

impl std::fmt::Debug for Picked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picked").field("target", &self.target).field("paths", &self.paths).field("why", &self.why).finish_non_exhaustive()
    }
}

/// A request that came while another picker was showing (#2150).
struct Waiting {
    ask: Ask,
    target: Option<PickTarget>,
    then: Then,
}

impl std::fmt::Debug for Waiting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Waiting").field("target", &self.target).finish_non_exhaustive()
    }
}

/// A request whose panel couldn't show, kept until the user tries again or discards it.
pub(crate) struct Failed {
    ask: Ask,
    target: Option<PickTarget>,
    then: Then,
    why: Problem,
}

impl std::fmt::Debug for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Failed").field("target", &self.target).field("why", &self.why).finish_non_exhaustive()
    }
}

/// Pickers in flight and the picks they finished.
#[derive(Debug, Default)]
pub struct Pickers {
    /// Requests that show, in order, once the picker before them is answered.
    waiting: std::collections::VecDeque<Waiting>,
    /// A copy of the native picker showing now, to show again if it turns out it couldn't.
    shown: Option<Ask>,
    /// The request whose panel couldn't show, while the user is asked what to do with it.
    pub(crate) failed: Option<Failed>,
    /// A picker is showing, or its pick is waiting for the frame; a second request waits
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
        self.spawn_worker(target, ctx, pick, then, pdfcraft_platform::file_dialog_probe::why_empty).is_some()
    }

    /// [`Self::spawn`], returning the worker so tests can wait for it instead of polling.
    /// `why_empty` explains an empty answer (it may take a few D-Bus calls, so it runs here).
    fn spawn_worker<F>(
        &self,
        target: Option<PickTarget>,
        ctx: Option<egui::Context>,
        pick: F,
        then: Then,
        why_empty: fn() -> Option<Problem>,
    ) -> Option<std::thread::JoinHandle<()>>
    where
        F: Future<Output = Vec<PathBuf>> + Send + 'static,
    {
        let showing = Showing { flag: Some(self.showing.clone()), ctx: ctx.clone() };
        let done = self.done.clone();
        std::thread::Builder::new()
            .name("file-picker".into())
            .spawn(move || {
                let paths = pollster::block_on(pick);
                let why = if paths.is_empty() { why_empty() } else { None };
                done.lock().unwrap_or_else(PoisonError::into_inner).push(Picked { target, paths, then, why });
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
        self.done.lock().unwrap_or_else(PoisonError::into_inner).push(Picked { target, paths, then, why: None });
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
            PickFor::Open => paths.iter().for_each(|p| {
                app.open_path(&p.to_string_lossy());
            }),
            PickFor::Files(purpose) => app.use_paths(purpose, &paths),
        });
    }

    /// Show `ask` without blocking the frame, and call `then` with the chosen paths on a later
    /// frame ([`Self::process_picked`]). While another picker is open, `ask` waits and shows
    /// when that one is answered (the user is told). Returns false, after telling the user,
    /// when no picker could be shown because its worker couldn't start.
    ///
    /// `then` is not called when the picker is cancelled, or when `target` is given and by then
    /// that document is no longer the active one or has been edited (the user is told). Use
    /// `target` whenever `then` edits the active document or refers to its pages or objects by
    /// position.
    ///
    /// `pick_override` answers instead of a native panel (tests and automation); the answer
    /// still arrives on a later frame, like a real pick.
    pub(crate) fn ask(&mut self, ask: Ask, target: Option<DocId>, then: impl FnOnce(&mut PdfCraftApp, Vec<PathBuf>) + Send + 'static) -> bool {
        let target = self.pick_target(target);
        let then: Then = Box::new(then);
        if self.pickers.showing.swap(true, Ordering::SeqCst) {
            // Refusing would drop what asked, such as a finished Reduce File Size copy.
            self.pickers.waiting.push_back(Waiting { ask, target, then });
            self.notify_tr("Another file dialog is still open. This one will open when it closes.");
            return true;
        }
        self.show_picker(ask, target, then)
    }

    /// [`Self::ask`] once `showing` is claimed for it.
    fn show_picker(&mut self, ask: Ask, target: Option<PickTarget>, then: Then) -> bool {
        if let Some(paths) = self.pick_override.clone() {
            self.pickers.deliver(target, paths.into_iter().map(PathBuf::from).collect(), then);
            return true;
        }
        self.pickers.shown = Some(ask.clone());
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
            self.pickers.shown = None;
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
        for Picked { target, paths, then, why } in self.pickers.take() {
            // The pick is in hand: the next picker may show, including one `then` asks for.
            self.pickers.showing.store(false, Ordering::SeqCst);
            let shown = self.pickers.shown.take();
            // Opens were polled before this pick was consumed. Wake their retained replies
            // once, including after cancellation, without polling while a picker waits.
            if !self.pending_opens.is_empty()
                && let Some(ctx) = &self.ctx
            {
                ctx.request_repaint();
            }
            if paths.is_empty() {
                // No panel could show (#3988): keep what asked rather than take it for a Cancel.
                if let (Some(why), Some(ask)) = (why, shown) {
                    if self.pickers.failed.is_some() {
                        // One question at a time; this one is asked again after it.
                        self.pickers.waiting.push_back(Waiting { ask, target, then });
                    } else {
                        self.pickers.failed = Some(Failed { ask, target, then, why });
                    }
                }
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
        // After the closures, so a picker one of them asked for comes before the waiting ones. Not
        // while a panel that couldn't show is waiting for an answer: the next one couldn't either.
        if !self.pickers.waiting.is_empty()
            && self.pickers.failed.is_none()
            && !self.pickers.showing.swap(true, Ordering::SeqCst)
            && let Some(Waiting { ask, target, then }) = self.pickers.waiting.pop_front()
        {
            self.show_picker(ask, target, then);
        }
    }

    /// Why the last file dialog couldn't show, while the user is asked whether to try again
    /// (`dialogs::picker_failed_prompt`).
    pub(crate) fn picker_failed(&self) -> Option<&Problem> {
        self.pickers.failed.as_ref().map(|f| &f.why)
    }

    /// The user's answer to [`Self::picker_failed`]: show the same dialog again, its result
    /// going where it was going, or drop what asked.
    pub(crate) fn resolve_picker_failed(&mut self, retry: bool) {
        let Some(Failed { ask, target, then, .. }) = self.pickers.failed.take() else { return };
        if !retry {
            return;
        }
        if self.pickers.showing.swap(true, Ordering::SeqCst) {
            self.pickers.waiting.push_front(Waiting { ask, target, then });
        } else {
            self.show_picker(ask, target, then);
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
        let worker = app.pickers.spawn_worker(None, None, async { vec![PathBuf::from("a.pdf")] }, record(&got), || None).expect("worker starts");
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
        let worker = pickers.spawn_worker(None, Some(ctx.clone()), async { fail() }, record(&got), || None).expect("worker starts");
        // join returns once the panic has unwound and the drop guard has run.
        assert!(worker.join().is_err());
        assert!(!pickers.showing.load(Ordering::SeqCst));
        assert!(pickers.take().is_empty());
        assert!(ctx.has_requested_repaint(), "retained opens must get another frame after a picker worker fails");
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "an abandoned picker requests one wake");
    }

    fn backend_down() -> Option<Problem> {
        Some(Problem::BackendDown(vec!["xdg-desktop-portal-gtk".into()]))
    }

    /// A Save panel answered empty by a worker whose probe says `why_empty`.
    fn answer_empty(app: &mut PdfCraftApp, then: Then, why_empty: fn() -> Option<Problem>) {
        app.pickers.showing.store(true, Ordering::SeqCst);
        app.pickers.shown = Some(Ask::Save(rfd::AsyncFileDialog::new().set_file_name("reduced.pdf")));
        let worker = app.pickers.spawn_worker(None, None, async { Vec::new() }, then, why_empty).expect("worker starts");
        assert!(worker.join().is_ok());
        app.process_picked();
    }

    #[test]
    fn a_panel_that_could_not_show_keeps_what_asked_until_tried_again() {
        let mut app = PdfCraftApp::new();
        let got = Arc::new(Mutex::new(Vec::new()));
        answer_empty(&mut app, record(&got), backend_down);
        assert_eq!(app.picker_failed(), backend_down().as_ref(), "the user is told why");
        assert!(!app.pickers.showing.load(Ordering::SeqCst), "the question doesn't hold the picker");
        assert!(got.lock().unwrap().is_empty());
        // The file chooser works again: Try again shows the same panel and the kept work uses it.
        app.pick_override = Some(vec!["reduced.pdf".into()]);
        app.resolve_picker_failed(true);
        assert_eq!(app.picker_failed(), None);
        app.process_picked();
        assert_eq!(*got.lock().unwrap(), vec![PathBuf::from("reduced.pdf")]);
    }

    #[test]
    fn discarding_a_panel_that_could_not_show_drops_what_asked() {
        let mut app = PdfCraftApp::new();
        let got = Arc::new(Mutex::new(Vec::new()));
        answer_empty(&mut app, record(&got), backend_down);
        assert_eq!(Arc::strong_count(&got), 2, "the kept closure holds its work");
        app.resolve_picker_failed(false);
        assert_eq!(app.picker_failed(), None);
        assert_eq!(Arc::strong_count(&got), 1, "discarded");
        app.pick_override = Some(vec!["x.pdf".into()]);
        app.process_picked();
        assert!(got.lock().unwrap().is_empty());
    }

    #[test]
    fn an_empty_answer_on_a_working_desktop_is_still_a_cancel() {
        let mut app = PdfCraftApp::new();
        let got = Arc::new(Mutex::new(Vec::new()));
        answer_empty(&mut app, record(&got), || None);
        assert_eq!(app.picker_failed(), None);
        assert_eq!(Arc::strong_count(&got), 1, "a cancelled request is dropped, as before");
    }

    #[test]
    fn a_second_panel_that_could_not_show_waits_behind_the_first() {
        let mut app = PdfCraftApp::new();
        let got = Arc::new(Mutex::new(Vec::new()));
        let first = got.clone();
        answer_empty(&mut app, Box::new(move |_, p| first.lock().unwrap().push(("first", p))), backend_down);
        let second = got.clone();
        answer_empty(&mut app, Box::new(move |_, p| second.lock().unwrap().push(("second", p))), backend_down);
        assert_eq!(Arc::strong_count(&got), 3, "neither request is lost");
        app.pick_override = Some(vec!["a.pdf".into()]);
        app.process_picked();
        assert!(got.lock().unwrap().is_empty(), "nothing shows while the first question is open");
        app.resolve_picker_failed(true);
        app.process_picked();
        app.process_picked();
        let order: Vec<_> = got.lock().unwrap().iter().map(|(who, _)| *who).collect();
        assert_eq!(order, ["first", "second"]);
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
    fn a_second_picker_waits_for_the_one_showing_and_opens_when_it_closes() {
        let mut app = PdfCraftApp::new();
        app.pick_override = Some(vec!["c.pdf".into()]);
        let got = Arc::new(Mutex::new(Vec::new()));
        let first = got.clone();
        assert!(app.ask(Ask::Save(rfd::AsyncFileDialog::new()), None, move |_, _| first.lock().unwrap().push("first")));
        let second = got.clone();
        assert!(app.ask(Ask::Folder(rfd::AsyncFileDialog::new()), None, move |_, _| second.lock().unwrap().push("second")));
        assert!(app.toast.as_ref().is_some_and(|(m, _)| m.contains("will open when it closes")), "the user is told it will follow");
        app.process_picked();
        assert_eq!(*got.lock().unwrap(), vec!["first"], "the waiting picker shows only once the first one is answered");
        assert!(app.pickers.showing.load(Ordering::SeqCst), "the waiting picker owns the flag now");
        app.process_picked();
        assert_eq!(*got.lock().unwrap(), vec!["first", "second"]);
        assert!(!app.pickers.showing.load(Ordering::SeqCst));
    }

    #[test]
    fn a_waiting_picker_still_opens_when_the_one_showing_is_cancelled() {
        let mut app = PdfCraftApp::new();
        app.pick_override = Some(vec!["c.pdf".into()]);
        app.pickers.showing.store(true, Ordering::SeqCst);
        let got = Arc::new(Mutex::new(Vec::new()));
        let into = got.clone();
        assert!(app.ask(Ask::File(rfd::AsyncFileDialog::new()), None, move |_, paths| into.lock().unwrap().extend(paths)));
        app.process_picked();
        assert!(got.lock().unwrap().is_empty(), "the showing picker still owns the flag");
        app.pickers.deliver(None, vec![], Box::new(|_, _| {}));
        app.process_picked();
        app.process_picked();
        assert_eq!(*got.lock().unwrap(), vec![PathBuf::from("c.pdf")]);
    }

    /// A one-page document showing `text`, with an unencoded content stream long enough that
    /// Reduce File Size makes a smaller copy (it saves none otherwise).
    fn text_pdf(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 12 Tf 20 250 Td ({text}) Tj ET\n{}", "q Q\n".repeat(500));
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 300] >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
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

    /// #2150: Reduce File Size finishing under an open Save As panel used to drop its bytes.
    #[test]
    fn a_reduced_copy_finished_under_another_dialog_is_saved_once_it_closes() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ui-pickers-{}-reduce", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("notes (reduced).pdf");
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let source = text_pdf("Minutes of the harbour board");
        app.open_bytes("notes.pdf", None, source.clone()).unwrap();
        let (_, doc) = app.active_ids().unwrap();
        let (reduced, _) = app.session.optimize_job(doc, &pdfcraft_engine::optimize::Settings::default(), &[]).unwrap().run(|_| true).unwrap();
        assert!(reduced.len() < source.len(), "a copy that isn't smaller would not be saved at all");
        app.pick_override = Some(vec![out.to_string_lossy().into_owned()]);
        // Save As is still open on another tab when the optimizer finishes.
        app.pickers.showing.store(true, Ordering::SeqCst);
        app.save_optimized(doc, crate::optimize_ui::OptimizeKind::Reduce, Ok((reduced.clone(), String::new())));
        app.pickers.deliver(None, vec![], Box::new(|_, _| {}));
        app.process_picked();
        app.process_picked();
        let written = std::fs::read(&out).unwrap_or_default();
        assert!(written == *reduced, "the reduced copy is saved where the user chose, byte for byte");
        let mut again = pdfcraft_engine::Session::new();
        let id = again.open("notes (reduced).pdf", None, Arc::new(written), None).unwrap();
        let text = pdfcraft_engine::export::Exporter::new(again.get(id).unwrap()).text(0).unwrap();
        assert!(text.contains("Minutes of the harbour board"), "the saved copy reopens with its text: {text:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_follow_up_that_asks_again_gets_its_picker_before_a_request_that_came_first() {
        let mut app = PdfCraftApp::new();
        app.pick_override = Some(vec!["first.fdf".into()]);
        let got = Arc::new(Mutex::new(Vec::new()));
        let into = got.clone();
        assert!(app.ask(Ask::Files(rfd::AsyncFileDialog::new()), None, move |app, _| {
            // Merge data: the spreadsheet's save picker, asked from the first pick's closure.
            assert!(app.ask(Ask::Save(rfd::AsyncFileDialog::new()), None, move |_, _| into.lock().unwrap().push("chained")));
        }));
        // Another command asks for a picker before the frame uses the first pick: it waits.
        let other = got.clone();
        assert!(app.ask(Ask::File(rfd::AsyncFileDialog::new()), None, move |_, _| other.lock().unwrap().push("other")));
        for _ in 0..4 {
            app.process_picked();
        }
        assert_eq!(*got.lock().unwrap(), vec!["chained", "other"], "the chained picker answers first");
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
