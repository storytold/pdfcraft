//! Help ▸ Check for updates and the check at start, through `craft_update`: the flow itself is its
//! `Updater`; this module supplies the suite and the quit path, and shows the result in
//! `craft_update`'s `CheckResultDialog`.

#[cfg(not(target_arch = "wasm32"))]
use craft_update::updater::{Action, CheckSource, Status, Updater};
#[cfg(not(target_arch = "wasm32"))]
use craft_update::{CheckResultDialog, CheckResultResponse};

use crate::PdfCraftApp;

/// Where every PdfCraft release is listed.
pub const RELEASES_PAGE: &str = "https://github.com/storytold/pdfcraft/releases";

/// Stands in for the network in tests.
#[cfg(not(target_arch = "wasm32"))]
pub type UpdateSource = CheckSource;

pub(crate) struct Updates {
    #[cfg(not(target_arch = "wasm32"))]
    updater: Updater,
    /// The Updates dialog is showing.
    pub(crate) open: bool,
    /// Ask for a newer release at start, unless Preferences says not to; the desktop app turns
    /// it on, so tests never reach the network.
    #[cfg(not(target_arch = "wasm32"))]
    startup_check: bool,
    #[cfg(not(target_arch = "wasm32"))]
    check_requested: bool,
    /// How many times `on_exit` called [`Updates::exiting`].
    #[cfg(test)]
    exits: u32,
    /// Stands in for the close that a finished installation queues in the `Updater`.
    #[cfg(test)]
    close_queued: bool,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            updater: Updater::new(&crate::UPDATE_APP, env!("CARGO_PKG_VERSION")),
            open: false,
            #[cfg(not(target_arch = "wasm32"))]
            startup_check: false,
            #[cfg(not(target_arch = "wasm32"))]
            check_requested: false,
            #[cfg(test)]
            exits: 0,
            #[cfg(test)]
            close_queued: false,
        }
    }
}

impl Updates {
    /// Starts the helper of an update that waits for the app to exit. Later calls do nothing.
    pub(crate) fn exiting(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.updater.start_pending_update();
        #[cfg(test)]
        {
            self.exits += 1;
        }
    }

    /// Draws the startup prompt and returns what it asks for. `can_start` is false while unsaved
    /// work holds back a ready update.
    #[cfg(not(target_arch = "wasm32"))]
    fn show_prompt(&mut self, ctx: &egui::Context, can_start: bool) -> Action {
        #[cfg(test)]
        if can_start && std::mem::take(&mut self.close_queued) {
            return Action::Close;
        }
        self.updater.show_if(ctx, can_start)
    }

    /// Asks the next frame that may close to close the app, so that a ready update installs.
    #[cfg(not(target_arch = "wasm32"))]
    fn queue_close(&mut self) {
        self.updater.restart_to_update();
        #[cfg(test)]
        {
            self.close_queued = true;
        }
    }

    /// Takes the close that a finished installation queued, so a save after Cancel does not close
    /// the app. `show_prompt` draws an open startup prompt, so this is for when none is open.
    #[cfg(not(target_arch = "wasm32"))]
    fn take_queued_close(&mut self, ctx: &egui::Context) {
        let _ = self.show_prompt(ctx, true);
    }
}

impl PdfCraftApp {
    /// Whether the app asks for a newer release on its first frame (off until the desktop app turns it on).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_startup_update_check(&mut self, on: bool) {
        self.updates.startup_check = on;
    }

    /// Replaces the network with `source` (tests).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_update_source(&mut self, source: UpdateSource) {
        self.updates.updater = Updater::new(&crate::UPDATE_APP, env!("CARGO_PKG_VERSION")).with_source(source);
    }

    /// Help ▸ Check for updates: ask for the latest release and show the outcome.
    pub fn check_for_updates(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.updates.open = true;
            self.updates.check_requested = true;
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.open_url(RELEASES_PAGE);
        }
    }

    /// Starts the checks and picks up their answers (each frame).
    pub(crate) fn poll_updates(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(ctx) = self.ctx.clone() else { return };
            // Settled on the first frame: a preference turned on later waits for the next start.
            let at_start = std::mem::take(&mut self.updates.startup_check) && self.update_check_at_start;
            let updates = &mut self.updates;
            if std::mem::take(&mut updates.check_requested) {
                updates.updater.check_now(&ctx);
            } else if at_start {
                updates.updater.check_at_start(&ctx);
            }
            updates.updater.tick(&ctx);
        }
    }
}

/// The startup prompt, and the Updates dialog that answers Help ▸ Check for updates.
pub(crate) fn dialog(app: &mut PdfCraftApp, ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // The restart helper is scheduled before the app closes, so no update starts over unsaved work.
        let clean = app.first_dirty().is_none();
        match app.updates.show_prompt(ctx, clean) {
            Action::None => {}
            Action::OpenUrl(url) => app.open_url(&url),
            Action::Close => {
                app.request_quit(ctx);
            }
        }
        // A requested check that found a release is answered by the prompt, not by this dialog.
        if app.updates.open && !app.updates.check_requested && matches!(app.updates.updater.status(), Status::Idle) {
            app.updates.open = false;
        }
        if app.updates.open {
            let dialog = CheckResultDialog::new(&crate::UPDATE_APP, env!("CARGO_PKG_VERSION"));
            let response = dialog.show(ctx, app.updates.updater.status());
            answer_result(app, ctx, response);
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ctx);
}

/// Carries out the answer to the Updates dialog.
#[cfg(not(target_arch = "wasm32"))]
fn answer_result(app: &mut PdfCraftApp, ctx: &egui::Context, response: CheckResultResponse) {
    match response {
        CheckResultResponse::None => {}
        CheckResultResponse::Close => {
            app.updates.open = false;
            app.updates.updater.dismiss_status();
        }
        CheckResultResponse::CheckAgain => app.updates.check_requested = true,
        CheckResultResponse::RestartToUpdate => restart_to_update(app, ctx),
    }
}

/// Restart to Update. With unsaved changes the Updates dialog closes and the quit path asks about
/// them; Cancel there leaves the update ready. An open startup prompt keeps its own Restart to
/// Update and save-first note, so the app stays open.
#[cfg(not(target_arch = "wasm32"))]
fn restart_to_update(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if app.first_dirty().is_none() {
        app.updates.queue_close();
        return;
    }
    app.updates.open = false;
    app.updates.updater.dismiss_status();
    if app.updates.updater.prompt_open() {
        return;
    }
    app.updates.take_queued_close(ctx);
    app.request_quit(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CloseRequest;
    use crate::editing::SaveTarget;
    use eframe::App as _;
    use pdfcraft_engine::{Edit, FieldValue};
    use std::path::PathBuf;
    use std::sync::Arc;

    /// An app with one open document holding a form, and with `dirty` an unsaved change to it.
    fn app(dirty: bool) -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.open_bytes("source.pdf", None, include_bytes!("../../tests/data/form.pdf").to_vec()).unwrap();
        if dirty {
            assert!(app.apply_edit(Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Changed".into()) }));
            assert!(app.first_dirty().is_some());
        }
        app.updates.open = true;
        app
    }

    /// An app with an unsaved change to a document in a new temporary folder, which Save writes
    /// without asking where. Returns the document's path too.
    fn app_with_file(test: &str) -> (PdfCraftApp, PathBuf) {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ui-updates-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("source.pdf");
        let bytes = include_bytes!("../../tests/data/form.pdf").to_vec();
        std::fs::write(&path, &bytes).unwrap();
        let mut app = PdfCraftApp::new();
        app.open_bytes("source.pdf", Some(path.to_string_lossy().into_owned()), bytes).unwrap();
        assert!(app.apply_edit(Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Changed".into()) }));
        app.updates.open = true;
        (app, path)
    }

    #[test]
    fn restart_to_update_with_unsaved_changes_opens_the_save_prompt() {
        let mut app = app(true);
        let ctx = egui::Context::default();
        answer_result(&mut app, &ctx, CheckResultResponse::RestartToUpdate);
        assert!(!app.updates.open, "the modal Updates dialog closes");
        assert_eq!(app.close_request, Some(CloseRequest::Quit), "the save prompt opens");
        assert!(!app.allow_quit);
        app.resolve_close(&ctx, None);
        assert!(app.close_request.is_none(), "Cancel closes the prompt");
        assert!(!app.allow_quit, "Cancel keeps the app open");
    }

    #[test]
    fn restart_to_update_with_unsaved_changes_dismisses_the_check_result() {
        let mut app = app(true);
        app.set_update_source(Arc::new(|| Ok(None)));
        let ctx = egui::Context::default();
        app.updates.updater.check_now(&ctx);
        assert!(!matches!(app.updates.updater.status(), Status::Idle));
        answer_result(&mut app, &ctx, CheckResultResponse::RestartToUpdate);
        assert!(matches!(app.updates.updater.status(), Status::Idle));
    }

    #[test]
    fn saving_at_the_restart_prompt_writes_the_file_and_quits() {
        let (mut app, path) = app_with_file("save");
        let ctx = egui::Context::default();
        answer_result(&mut app, &ctx, CheckResultResponse::RestartToUpdate);
        app.resolve_close(&ctx, Some(true));
        assert!(app.allow_quit, "the app quits once the document is saved");

        let mut reopened = PdfCraftApp::new();
        reopened.open_bytes("source.pdf", Some(path.to_string_lossy().into_owned()), std::fs::read(&path).unwrap()).unwrap();
        let doc = reopened.session.get(reopened.views[0].id).unwrap();
        let name = doc.form.iter().find(|f| f.name == "name").and_then(|f| f.value.first().cloned());
        assert_eq!(name.as_deref(), Some("Changed"), "the change is in the file");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn saving_after_canceling_the_restart_keeps_the_app_open() {
        let (mut app, path) = app_with_file("cancel");
        app.updates.close_queued = true;
        let ctx = egui::Context::default();
        answer_result(&mut app, &ctx, CheckResultResponse::RestartToUpdate);
        app.resolve_close(&ctx, None);
        assert!(app.save_active(SaveTarget::InPlace));
        assert!(app.first_dirty().is_none());
        dialog(&mut app, &ctx);
        assert!(!app.allow_quit, "the save does not close the app");
        assert!(app.close_request.is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn restart_to_update_with_nothing_unsaved_leaves_the_close_to_the_updater() {
        let mut app = app(false);
        let ctx = egui::Context::default();
        answer_result(&mut app, &ctx, CheckResultResponse::RestartToUpdate);
        assert!(app.updates.open, "the dialog stays until the updater closes the app");
        assert!(app.close_request.is_none());
        assert!(!app.allow_quit);
        dialog(&mut app, &ctx);
        assert!(app.allow_quit, "the next frame closes the app");
    }

    #[test]
    fn restart_to_update_with_the_startup_prompt_open_leaves_the_app_open() {
        let mut app = app(true);
        app.updates.updater.offer(craft_update::Release {
            tag: "v99.0.0".into(),
            version: craft_update::parse_version("99.0.0"),
            name: "PdfCraft 99.0.0".into(),
            html_url: "https://github.com/storytold/pdfcraft/releases/tag/v99.0.0".into(),
            ..Default::default()
        });
        assert!(app.updates.updater.prompt_open());
        answer_result(&mut app, &egui::Context::default(), CheckResultResponse::RestartToUpdate);
        assert!(!app.updates.open, "the result dialog closes");
        assert!(app.updates.updater.prompt_open(), "the prompt stays");
        assert!(app.close_request.is_none(), "the prompt carries the restart");
    }

    #[test]
    fn on_exit_calls_the_update_exit_hook_once() {
        let mut app = PdfCraftApp::new();
        app.on_exit(None);
        assert_eq!(app.updates.exits, 1);
    }
}
