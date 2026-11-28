use super::PdfCraftApp;

use craft::ui::runtime::frame::FrameInput;
use craft::ui::shell::embed;

impl PdfCraftApp {
    /// The app's part of [`eframe::App::raw_input_hook`], before egui reads the frame's input:
    /// the runtime hands over what it gathered since the last frame (the commands fired) and takes this frame's root values,
    /// command states and host. eframe calls the hook once per frame, before the first pass, so
    /// a second pass of the same frame runs on what this call set and fires nothing twice.
    pub(super) fn prepare_raw_input(&mut self, ctx: &egui::Context, _raw: &mut egui::RawInput) {
        self.set_title_bar_choice(ctx, self.title_bar_choice);
        self.control_before_input(ctx);
        embed::before_input(ctx, |output| {
            for fired in &output.fired {
                self.run_fired(ctx, fired);
            }
            for declined in &output.declined {
                self.run_declined(declined);
            }
            self.sync_host_locale();
            let commands = crate::commands::catalog::command_input(self, ctx);
            FrameInput::new(self.root_env(ctx)).commands(commands).host(self.host.clone())
        });
        self.last_frame_active = self.active_ids().map(|(_, id)| id);
    }

    fn sync_host_locale(&mut self) {
        let Some(catalogs) = self.host.i18n() else { return };
        let language = crate::i18n::Lang::from_pref(&self.language);
        let date_language = self.session.date_language().unwrap_or(language.code());
        if self.host_date_language.as_deref() != Some(date_language) {
            catalogs.set_dates(craft_i18n::DateFormat::new(&craft_i18n::Region::from_tag(date_language)));
            self.host_date_language = Some(date_language.to_string());
        }
        let lang = catalogs.from_code(language.code()).unwrap_or(craft_i18n::Lang::EN);
        if self.host.lang() != lang {
            self.host = std::sync::Arc::new(self.host.with_lang(lang));
        }
    }
}
