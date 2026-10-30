//! Preferences ▸ Commenting: the author of new comments (Acrobat: Preferences ▸ Identity) and
//! hover previews.

use super::{card, field_label};
use crate::theme::Tokens;
use crate::{PdfCraftApp, widgets};

pub(super) fn show(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    card(ui, t, Some(tl!("Identity")), |ui| {
        let label = field_label(ui, t, tl!("Name on new comments"));
        ui.add(widgets::line(&mut app.comment_prefs.author).desired_width(ui.available_width().min(320.0)).char_limit(crate::MAX_AUTHOR_CHARS))
            .labelled_by(label.id);
    });
    card(ui, t, None, |ui| {
        ui.add(widgets::checkbox(&mut app.comment_prefs.hide_empty_highlight_hover, tl!("Hide hover previews for highlights without comment text")));
    });
}
