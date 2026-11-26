//! Help ▸ Check for updates and the check at start, through `craft_update`: the flow itself is its
//! `Updater`; this module feeds it the app's unsaved-work facts and carries out what it asks for,
//! and shows the outcome of a check in `UpdateCheckDialog` on the app's `DialogHost`.

// PLAN: `craft::update` needs the `update` feature of the craft facade in crates/ui-egui/Cargo.toml (pdf-hub-w1, L-206).
#[cfg(not(target_arch = "wasm32"))]
use craft::ui::dialog::DialogResult;
#[cfg(not(target_arch = "wasm32"))]
use craft::update::stack_dialogs::{CheckChoice, UpdateCheckDialog};
#[cfg(not(target_arch = "wasm32"))]
use craft::update::updater::{Action, Status};

use crate::PdfCraftApp;

/// Where every PdfCraft release is listed.
pub const RELEASES_PAGE: &str = "https://github.com/storytold/pdfcraft/releases";

impl PdfCraftApp {
    /// Whether the app asks for a newer release on its first frame (off until the desktop app turns it on).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_startup_update_check(&mut self, on: bool) {
        self.startup_check = on;
    }

    /// Help ▸ Check for updates: ask for the latest release. The result dialog opens while the
    /// updater's status is not idle ([`Self::update_logic`]).
    pub fn check_for_updates(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.updater.check_now();
        #[cfg(target_arch = "wasm32")]
        self.open_url(RELEASES_PAGE);
    }

    /// One frame of the update flow, before the dialog host runs: starts the checks, drives the
    /// startup prompt, shows the result of a check and carries out what they ask for.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn update_logic(&mut self, ctx: &egui::Context) {
        self.updater.tick(ctx);
        // Settled on the first frame: a preference turned on later waits for the next start.
        if std::mem::take(&mut self.startup_check) && self.update_check_at_start {
            self.updater.check_at_start();
        }
        // Answer any closed check-result dialog before `show_update_check`, so dismissing or
        // restarting does not immediately reopen the dialog while `status` is still non-idle.
        let dirty: Vec<usize> = (0..self.views.len()).filter(|&i| self.has_unsaved_work(i)).collect();
        self.answer_update_check(ctx, dirty.is_empty());
        self.show_update_check();

        // The restart helper is scheduled before the app closes, so no update starts over unsaved work.
        let can_save = dirty.iter().all(|&i| self.saves_in_place(i));
        match self.updater.run_with_save(&mut self.dialogs, dirty.is_empty(), can_save) {
            Action::OpenUrl(url) => self.open_url(&url),
            Action::Close => {
                self.request_quit(ctx);
            }
            // `None`, and any later action this app doesn't know: nothing to do this frame.
            _ => {}
        }
        if self.updater.take_save_request() {
            let mut saved = true;
            for &i in &dirty {
                saved &= self.save_view(ctx, i, crate::editing::SaveTarget::InPlace);
            }
            if !saved {
                // The save path has told the user why.
                self.updater.save_stopped(None);
            }
        }
    }

    /// Without an updater the web build has nothing to run.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn update_logic(&mut self, _ctx: &egui::Context) {}

    /// Whether tab `index` saves in place without a question: it has a path that is no mail
    /// temporary copy and none read back from a recovery sidecar.
    #[cfg(not(target_arch = "wasm32"))]
    fn saves_in_place(&self, index: usize) -> bool {
        let Some(view) = self.views.get(index) else { return false };
        let path = self.session.get(view.id).and_then(|d| d.path.as_deref());
        path.is_some_and(|p| !craft::dialogs::save::is_mail_temp_copy(std::path::Path::new(p))) && !self.recovered_paths.contains(&view.id)
    }

    /// Keeps the check-result dialog on the host while the updater has something to report, and
    /// feeds it.
    #[cfg(not(target_arch = "wasm32"))]
    fn show_update_check(&mut self) {
        if !self.dialogs.is_open::<UpdateCheckDialog>() && !matches!(self.updater.status(), Status::Idle) {
            self.dialogs.open(UpdateCheckDialog::new(env!("CARGO_PKG_VERSION")));
        }
        let (status, installing, message) = (self.updater.status().clone(), self.updater.is_installing(), self.updater.status_message());
        if let Some(dialog) = self.dialogs.get_mut::<UpdateCheckDialog>() {
            dialog.set_status(status);
            dialog.set_installing(installing);
            dialog.set_message(message);
        }
    }

    /// Carries out the answer to the check-result dialog. With unsaved changes Restart to Update
    /// dismisses the result and asks about them through the quit path; Cancel there leaves the
    /// update ready.
    #[cfg(not(target_arch = "wasm32"))]
    fn answer_update_check(&mut self, ctx: &egui::Context, clean: bool) {
        let Some(DialogResult::Ok(choice)) = self.dialogs.take::<UpdateCheckDialog>() else {
            return;
        };
        match choice {
            CheckChoice::Close => self.updater.dismiss_status(),
            CheckChoice::CheckNow | CheckChoice::CheckAgain => self.updater.check_now(),
            CheckChoice::Update => {
                self.updater.request_install(clean);
            }
            CheckChoice::Download => self.open_url(RELEASES_PAGE),
            CheckChoice::RestartToUpdate if clean => self.updater.restart_to_update(),
            CheckChoice::RestartToUpdate => {
                self.updater.dismiss_status();
                self.request_quit(ctx);
            }
            _ => {}
        }
    }
}
