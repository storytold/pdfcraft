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
            let commands = crate::commands::catalog::command_input(self, ctx);
            FrameInput::new(self.root_env(ctx)).commands(commands).host(self.host.clone())
        });
        self.last_frame_active = self.active_ids().map(|(_, id)| id);
    }
}
