use egui::{Align, Layout};

use crate::PdfCraftApp;
use crate::theme::Tokens;

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, _tokens: &Tokens) -> (bool, bool) {
    ui.vertical(|ui| {
        ui.add_space(4.0);
        ui.label(tl!("Microsoft Purview sensitivity label"));
        ui.add_space(8.0);

        if app.purview_labels.is_empty() {
            ui.label(tl!("No sensitivity labels are available. Sign in with a Microsoft 365 account, then try again."));
        } else {
            let mut selected = app.purview_label_id.clone();
            for label in &app.purview_labels {
                if !label.active {
                    continue;
                }
                let text = if label.description.is_empty() { label.name.clone() } else { format!("{} — {}", label.name, label.description) };
                ui.radio_value(&mut selected, label.id.clone(), text);
            }
            app.purview_label_id = selected;
        }

        ui.add_space(8.0);
        ui.label(tl!("Justification (required for policy downgrades or removals)"));
        ui.text_edit_multiline(&mut app.purview_justification);
    });

    let (cancel, apply) = ui
        .with_layout(Layout::right_to_left(Align::Center), |ui| {
            let cancel = ui.button(tl!("Cancel")).clicked();
            let apply = ui.add_enabled(!app.purview_label_id.is_empty(), egui::Button::new(tl!("Apply label"))).clicked();
            (cancel, apply)
        })
        .inner;
    if apply && let Some((_, id)) = app.active_ids() {
        let justification = if app.purview_justification.trim().is_empty() { None } else { Some(app.purview_justification.as_str()) };
        match app.session.purview_apply_label(id, &app.purview_label_id, justification) {
            Ok(()) => app.notify_tr("Microsoft Purview label applied"),
            Err(error) => app.notify_fmt("Couldn't apply the Microsoft Purview label: {e}", &[("e", &error.to_string())]),
        }
    }
    (apply, cancel)
}
