//! The last-resort guard around each frame (AGENTS.md §4, #3914). Commands, opens and file use
//! already catch panics; a frame's logic and drawing (dialog buttons, panel actions, canvas
//! gestures, reloads) did not, so a panic anywhere in them ended the app with every open tab and
//! up to a minute of unsaved edits. Now it is reported, the app is put back in a state the next
//! frame can draw, and unsaved documents are written to the recovery folder at once.

use crate::PdfCraftApp;

/// Frames in a row that may panic before the guard lets the panic end the app, as it did before.
/// By then every unsaved document has been snapshotted, so the next start offers to recover it.
const MAX_PANICKING_FRAMES: u32 = 3;

impl PdfCraftApp {
    /// Run one part of the frame (`logic` or `ui`) under the guard.
    pub(crate) fn guarded_frame(&mut self, part: impl FnOnce(&mut PdfCraftApp)) {
        if let Err(message) = pdfcraft_engine::guard(|| part(self)) {
            self.after_frame_panic(&message);
        }
    }

    /// After a frame panicked: keep the work, drop what the frame was showing on top (a dialog,
    /// the command palette), keep `active` pointing at a tab, and say so.
    fn after_frame_panic(&mut self, message: &str) {
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
        self.dialog = None;
        self.palette_open = false;
        self.keep_active_valid();
        self.notify_fmt("That didn't work: an internal error stopped it ({m}).", &[("m", message)]);
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
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
    use crate::{Dialog, RecoveryStore};
    use egui_kittest::Harness;
    use pdfcraft_engine::{Edit, Session};

    /// A recovery folder of its own, removed on drop.
    struct Store(RecoveryStore);

    impl Store {
        fn new(tag: &str) -> Store {
            let dir = std::env::temp_dir().join(format!("pdfcraft-frame-guard-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Store(RecoveryStore::new(dir))
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.dir());
        }
    }

    /// Snapshots written to the store. `RecoveryStore::list` leaves out a running instance's own.
    fn snapshots(store: &Store) -> usize {
        std::fs::read_dir(store.0.dir()).map(|d| d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "pdf")).count()).unwrap_or(0)
    }

    fn pdf(text: &str) -> Vec<u8> {
        Session::new().create_from_text("t", text).unwrap().to_vec()
    }

    /// Two tabs, the second with an unsaved edit, and the About dialog open.
    fn harness(store: &Store) -> Harness<'static, PdfCraftApp> {
        let recovery = store.0.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.enable_recovery(recovery);
            app.open_bytes("a.pdf", None, pdf("First")).unwrap();
            app.open_bytes("b.pdf", None, pdf("Second")).unwrap();
            app
        });
        h.run_steps(4);
        assert!(h.state_mut().apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
        h.state_mut().dialog = Some(Dialog::About);
        h.run_steps(1);
        h
    }

    fn toast(h: &Harness<'static, PdfCraftApp>) -> String {
        h.state().toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
    }

    #[test]
    fn a_panic_while_drawing_keeps_the_app_its_tabs_and_their_edits() {
        let store = Store::new("ui");
        let mut h = harness(&store);
        h.state_mut().panic_next = Some(FramePart::Ui);
        h.run_steps(1);
        h.state_mut().panic_next = None;
        h.run_steps(2);
        let app = h.state();
        assert_eq!(app.views.len(), 2, "every tab is still open");
        assert_eq!(app.active, Some(1));
        assert!(app.dialog.is_none(), "the dialog that was showing is dropped");
        assert_eq!(app.panicking_frames, 0, "a frame that draws resets the count");
        assert!(toast(&h).starts_with("That didn't work: an internal error stopped it (injected Ui panic)"), "{}", toast(&h));
        assert_eq!(snapshots(&store), 1, "the edited tab was snapshotted for recovery at once");
    }

    #[test]
    fn a_panic_in_a_frames_logic_is_caught_too() {
        let store = Store::new("logic");
        let mut h = harness(&store);
        h.state_mut().panic_next = Some(FramePart::Logic);
        h.run_steps(1);
        h.state_mut().panic_next = None;
        h.run_steps(2);
        assert_eq!(h.state().views.len(), 2);
        assert!(toast(&h).contains("injected Logic panic"), "{}", toast(&h));
        assert_eq!(snapshots(&store), 1);
    }

    #[test]
    fn frames_that_keep_panicking_end_the_app_after_snapshotting_its_work() {
        let store = Store::new("again");
        let mut h = harness(&store);
        h.state_mut().panic_next = Some(FramePart::Ui);
        // The guard lets the third panicking frame through, as the app ended before it.
        let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.run_steps(MAX_PANICKING_FRAMES as usize)));
        assert!(ended.is_err(), "an app that can't draw a single frame isn't kept running");
        assert_eq!(snapshots(&store), 1, "its unsaved tab is offered for recovery on the next start");
    }

    #[test]
    fn active_stays_on_an_open_tab() {
        let mut app = PdfCraftApp::new();
        app.active = Some(3);
        app.keep_active_valid();
        assert_eq!(app.active, None, "no tabs: nothing is active");
        app.open_bytes("a.pdf", None, pdf("Only")).unwrap();
        app.active = Some(5);
        app.keep_active_valid();
        assert_eq!(app.active, Some(0));
    }
}
