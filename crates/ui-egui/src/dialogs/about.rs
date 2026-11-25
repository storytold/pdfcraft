//! Help ▸ About: the framework's hosted About dialog over the app's info and the baked credits.

use craft::ui::patterns::form_dialogs::{AboutInfo, AboutTab, HostedAbout};

use crate::PdfCraftApp;

/// What the About tab shows: the app's name, blurb and version. No brand marks, no web links.
fn info() -> AboutInfo {
    AboutInfo::new(
        crate::i18n::APP_NAME,
        tl!("A clean-room, open-source PDF application written in Rust. MIT OR Apache-2.0."),
        pdfcraft_engine::build_info::version(),
    )
    .built_with("Rendering: hayro (bootstrap) · UI: egui · Icons: Lucide (ISC) · Fonts: Inter, JetBrains Mono, Dancing Script (OFL)")
}

impl PdfCraftApp {
    /// Opens the About window on `tab` (Help ▸ About, `help.about`).
    pub(crate) fn open_about(&mut self, tab: AboutTab) {
        crate::dialogs::host::open(self, HostedAbout::new(info(), crate::credits_data::credits()).tab(tab));
    }

    /// Opens a link the user activated in the About window (the credits list can carry some).
    pub(crate) fn about_pass(&mut self, _ctx: &egui::Context) {
        let Some(url) = self.dialogs.get_mut::<HostedAbout>().and_then(HostedAbout::take_link) else {
            return;
        };
        self.open_url(&url);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn about_shows_no_web_links() {
        assert!(super::info().links.is_empty());
    }
}
