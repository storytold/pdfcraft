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
        match app.updates.updater.show_if(ctx, clean) {
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
            match dialog.show(ctx, app.updates.updater.status()) {
                CheckResultResponse::None => {}
                CheckResultResponse::Close => {
                    app.updates.open = false;
                    app.updates.updater.dismiss_status();
                }
                CheckResultResponse::CheckAgain => app.updates.check_requested = true,
                CheckResultResponse::RestartToUpdate => restart_to_update(app, ctx, clean),
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ctx);
}

/// Restart to Update. With unsaved changes the Updates dialog closes and the quit path asks about
/// them; Cancel there leaves the update ready. An open startup prompt keeps its own Restart to
/// Update and save-first note, so the app stays open.
#[cfg(not(target_arch = "wasm32"))]
fn restart_to_update(app: &mut PdfCraftApp, ctx: &egui::Context, clean: bool) {
    if clean {
        app.updates.updater.restart_to_update();
        return;
    }
    app.updates.open = false;
    app.updates.updater.dismiss_status();
    if app.updates.updater.prompt_open() {
        return;
    }
    // Without a prompt this only takes the close a finished installation queued, so a save after
    // Cancel does not close the app.
    let _ = app.updates.updater.show_if(ctx, true);
    app.request_quit(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CloseRequest;
    use eframe::App as _;
    use pdfcraft_engine::{Edit, FieldValue};

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

    #[test]
    fn restart_to_update_with_unsaved_changes_opens_the_save_prompt() {
        let mut app = app(true);
        let ctx = egui::Context::default();
        restart_to_update(&mut app, &ctx, false);
        assert!(!app.updates.open, "the modal Updates dialog closes");
        assert_eq!(app.close_request, Some(CloseRequest::Quit), "the save prompt opens");
        assert!(!app.allow_quit);
        app.resolve_close(&ctx, None);
        assert!(app.close_request.is_none(), "Cancel closes the prompt");
        assert!(!app.allow_quit, "Cancel keeps the app open");
    }

    #[test]
    fn restart_to_update_with_nothing_unsaved_leaves_the_close_to_the_updater() {
        let mut app = app(false);
        restart_to_update(&mut app, &egui::Context::default(), true);
        assert!(app.updates.open, "the dialog stays until the updater closes the app");
        assert!(app.close_request.is_none());
        assert!(!app.allow_quit);
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
        restart_to_update(&mut app, &egui::Context::default(), false);
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
