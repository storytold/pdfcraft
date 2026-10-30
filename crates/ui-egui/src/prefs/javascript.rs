//! Preferences ▸ JavaScript: Acrobat JavaScript on or off.

use super::{card, hint};
use crate::theme::Tokens;
use crate::{PdfCraftApp, widgets};

pub(super) fn show(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    card(ui, t, None, |ui| {
        let mut on = app.session.javascript();
        if ui.add(widgets::checkbox(&mut on, tl!("Enable Acrobat JavaScript"))).changed() {
            app.session.set_javascript(on);
        }
        hint(
            ui,
            t,
            tl!(
                "Scripts run in a sandbox without file or network access. A script that asks to open a web page needs your permission first. With JavaScript off, Acrobat's standard format, validate and calculate functions still work."
            ),
        );
    });
}
