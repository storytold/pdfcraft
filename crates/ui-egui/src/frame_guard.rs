//! The last-resort guard around each frame (app-agents.md rule 13, #3914). Commands, opens and file use
//! already catch panics; a frame's logic and drawing (dialog buttons, panel actions, canvas
//! gestures, reloads) did not, so a panic anywhere in them ended the app with every open tab and
//! up to a minute of unsaved edits. Now it is reported, the app is put back in a state the next
//! frame can draw, and unsaved documents are written to the recovery folder at once.

use crate::PdfCraftApp;

/// Frames in a row that may panic before the guard lets the panic end the app, as it did before.
/// By then every unsaved document has been snapshotted, so the next start offers to recover it.
const MAX_PANICKING_FRAMES: u32 = 3;

impl PdfCraftApp {
    /// Before `logic`. While the window is hidden eframe runs `logic` without `ui`, so a
    /// `logic` that ran cleanly with no `ui` after it ends the run of panicking frames here.
    pub(crate) fn begin_logic(&mut self) {
        if self.logic_without_ui && !self.frame_panicked {
            self.panicking_frames = 0;
        }
        self.frame_panicked = false;
        self.logic_without_ui = true;
    }

    /// Before `ui`: this frame's `logic` is followed by drawing.
    pub(crate) fn begin_ui(&mut self) {
        self.logic_without_ui = false;
    }

    /// After `ui`: a frame whose logic and drawing both ran cleanly ends the run.
    pub(crate) fn end_ui(&mut self) {
        if !self.frame_panicked {
            self.panicking_frames = 0;
        }
    }

    /// Run one part of the frame (`logic` or `ui`) under the guard.
    pub(crate) fn guarded_frame(&mut self, ctx: &egui::Context, part: impl FnOnce(&mut PdfCraftApp)) {
        if let Err(message) = pdfcraft_engine::guard(|| part(self)) {
            self.after_frame_panic(ctx, &message);
        }
    }

    /// After a frame panicked: keep the work, drop what the frame was showing on top (a dialog,
    /// the command palette), keep `active` pointing at a tab, and say so.
    fn after_frame_panic(&mut self, ctx: &egui::Context, message: &str) {
        log::error!("a frame panicked and was caught: {message}");
        if !self.frame_panicked {
            self.frame_panicked = true;
            self.panicking_frames = self.panicking_frames.saturating_add(1);
        }
        // Now, not at the next autosave tick. Guarded too: the session may be what panicked.
        if let Err(e) = pdfcraft_engine::guard(|| self.autosave_now()) {
            log::error!("snapshotting unsaved documents after a frame panic failed: {e}");
        }
        if self.panicking_frames >= MAX_PANICKING_FRAMES {
            // Every frame panics: the window can't be used or even closed from the app. End it as
            // before this guard; the snapshots above are offered for recovery on the next start.
            std::panic::resume_unwind(Box::new(message.to_string()));
        }
        self.palette_open = false;
        self.keep_active_valid();
        self.notify_fmt("That didn't work: an internal error stopped it ({m}).", &[("m", message)]);
        self.cancel_dialogs(ctx);
        ctx.request_repaint();
    }

    /// Cancels every dialog the dialog host has open, the top one first so a child dialog is
    /// cancelled before the parent it was opened from. Keeps an active unsaved-changes close
    /// prompt alive, matching production's `self.dialog = None`.
    pub(crate) fn cancel_dialogs(&mut self, ctx: &egui::Context) {
        let keep = self.close_guard.is_some().then_some(self.close_guard_dialog).flatten();
        for open in self.dialogs.open_dialogs().into_iter().rev() {
            if Some(open.id) == keep {
                continue;
            }
            self.dialogs.cancel_by_id(ctx, open.id);
        }
    }

    /// The root window's close path, run once per frame after `logic`'s guarded part whether or
    /// not it panicked: a close asked for in a frame whose logic panicked still asks about
    /// unsaved changes, or eframe closes the window and what the recovery snapshot doesn't hold
    /// (a text being typed) is lost. If even that panics, the close is cancelled; the user can
    /// close again. It runs `embed::logic` once, which also runs the windows' own logic.
    pub(crate) fn run_close_path(&mut self, ctx: &egui::Context) {
        if pdfcraft_engine::guard(|| craft::ui::shell::embed::logic(ctx, || self.close_requested(ctx))).is_err() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }

    /// `active` names an open tab, or nothing: a panic part way through closing or replacing a
    /// tab can leave it past the end, and the next frame would index out of bounds unguarded.
    pub(crate) fn keep_active_valid(&mut self) {
        if self.active.is_some_and(|i| i >= self.views.len()) {
            self.active = self.views.len().checked_sub(1);
        }
    }
}

/// Tests: the part of a frame that panics on purpose while `panic_next` names it.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FramePart {
    Logic,
    Ui,
}

#[cfg(test)]
impl PdfCraftApp {
    pub(crate) fn injected_panic(&mut self, part: FramePart) {
        if self.panic_next == Some(part) {
            panic!("injected {part:?} panic");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft::ui::dialog::UnsavedChangesDialog;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use pdfcraft_engine::{Edit, Session};

    /// A recovery folder of its own, removed on drop. The app opens the store in it.
    struct Store(std::path::PathBuf);

    impl Store {
        fn new(tag: &str) -> Store {
            let dir = std::env::temp_dir().join(format!("pdfcraft-frame-guard-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Store(dir)
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Snapshots written to the store's folder.
    fn snapshots(store: &Store) -> usize {
        std::fs::read_dir(&store.0).map(|d| d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "pdf")).count()).unwrap_or(0)
    }

    fn pdf(text: &str) -> Vec<u8> {
        Session::new().create_from_text("t", text).unwrap().to_vec()
    }

    /// An app built through `with_host`, with recovery in `dir` when given.
    fn app(dir: Option<&std::path::Path>) -> PdfCraftApp {
        let wake = craft::host::Wake::none();
        let (intake, _) = craft::platform::open::FileIntake::new(wake.clone());
        let clipboard = craft::platform::clipboard::Clipboard::new(craft::platform::clipboard::EventClipboard::new(|| 0, |_| Ok(0)));
        let watch = craft::platform::watch::FileWatch::new(wake.clone());
        let services = crate::AppServices::new(wake, clipboard, dir.map(std::path::Path::to_path_buf), None, intake, watch);
        let info = craft::host::AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" };
        PdfCraftApp::with_host(std::sync::Arc::new(craft::host::Host::builder(info).build()), services)
    }

    /// Two tabs, the second with an unsaved edit, and the About dialog open when `about`.
    fn harness(store: &Store, about: bool) -> Harness<'static, PdfCraftApp> {
        let dir = store.0.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let mut app = app(Some(&dir));
                app.set_option(&cc.egui_ctx, "language", "en").unwrap();
                app.open_bytes(&egui::Context::default(), "a.pdf", None, pdf("First")).unwrap();
                app.open_bytes(&egui::Context::default(), "b.pdf", None, pdf("Second")).unwrap();
                app
            }
        });
        h.run_steps(4);
        let ctx_h = h.ctx.clone();
        assert!(h.state_mut().apply_edit(&ctx_h, Edit::RotatePages { pages: vec![0], degrees: 90 }));
        if about {
            let ctx = h.ctx.clone();
            h.state_mut().set_option(&ctx, "dialog", "about").unwrap();
        }
        h.run_steps(1);
        h
    }

    #[test]
    fn a_panic_while_drawing_keeps_the_app_its_tabs_and_their_edits() {
        let store = Store::new("panic");
        let mut h = harness(&store, true);
        // A panic in logic is caught like one in drawing.
        h.state_mut().panic_next = Some(FramePart::Logic);
        h.run_steps(1);
        h.state_mut().panic_next = None;
        h.run_steps(2);
        assert_eq!(h.state().views.len(), 2);
        h.get_by_label_contains("injected Logic panic");
        assert_eq!(snapshots(&store), 1);

        let ctx = h.ctx.clone();
        h.state_mut().set_option(&ctx, "dialog", "about").unwrap();
        h.run_steps(1);
        assert!(!h.state().dialogs.open_dialogs().is_empty(), "the dialog is showing");
        h.state_mut().panic_next = Some(FramePart::Ui);
        h.run_steps(1);
        h.state_mut().panic_next = None;
        h.run_steps(2);
        let app = h.state();
        assert_eq!(app.views.len(), 2, "every tab is still open");
        assert_eq!(app.active, Some(1));
        assert!(app.dialogs.open_dialogs().is_empty(), "the dialog that was showing is dropped");
        assert_eq!(app.panicking_frames, 0, "a frame that draws resets the count");
        // The notice card and its text both carry the message.
        assert!(h.query_all_by_label_contains("injected Ui panic").next().is_some(), "the panic is reported");
        assert_eq!(snapshots(&store), 1, "the edited tab was snapshotted for recovery at once");
    }

    #[test]
    fn closing_the_window_in_a_frame_whose_logic_panics_still_asks_about_unsaved_changes() {
        // The panic comes before the quit guard runs: without the question eframe closes the
        // window, and what isn't in the recovery snapshot (a text being typed) is lost.
        let store = Store::new("quit");
        let mut h = harness(&store, false);
        h.state_mut().panic_next = Some(FramePart::Logic);
        h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        h.run_steps(1);
        h.state_mut().panic_next = None;
        h.run_steps(2);
        assert!(h.state().dialogs.is_open::<UnsavedChangesDialog>(), "the save question is asked");
    }

    #[test]
    fn frames_that_keep_panicking_end_the_app_after_snapshotting_its_work() {
        let store = Store::new("again");
        let mut h = harness(&store, true);
        h.state_mut().panic_next = Some(FramePart::Ui);
        // The guard lets the third panicking frame through, as the app ended before it.
        let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.run_steps(MAX_PANICKING_FRAMES as usize)));
        assert!(ended.is_err(), "an app that can't draw a single frame isn't kept running");
        assert_eq!(snapshots(&store), 1, "its unsaved tab is offered for recovery on the next start");
    }

    #[test]
    fn hidden_frames_that_run_cleanly_reset_the_count() {
        // eframe runs `logic` without `ui` while the window is hidden: panics separated by
        // clean hidden frames are not "in a row" and must not end the app.
        let mut app = app(None);
        let ctx = egui::Context::default();
        for _ in 0..MAX_PANICKING_FRAMES * 2 {
            app.begin_logic();
            app.guarded_frame(&ctx, |_| panic!("hidden logic panic"));
            app.begin_logic();
            app.guarded_frame(&ctx, |_| {});
        }
        app.begin_logic();
        assert_eq!(app.panicking_frames, 0, "a clean hidden frame resets the count");
    }

    #[test]
    fn hidden_frames_that_keep_panicking_still_end_the_app() {
        let mut app = app(None);
        let ctx = egui::Context::default();
        let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for _ in 0..MAX_PANICKING_FRAMES {
                app.begin_logic();
                app.guarded_frame(&ctx, |_| panic!("hidden logic panic"));
            }
        }));
        assert!(ended.is_err(), "panics in every hidden frame still reach the limit");
    }
}
