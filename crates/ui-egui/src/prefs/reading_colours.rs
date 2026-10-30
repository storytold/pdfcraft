//! Preferences ▸ Appearance ▸ Reading colours (Acrobat's Replace Document Colors): colour samples
//! for the presets, a live preview and the custom colours (#2627). It sits in Appearance, below
//! the interface theme, but stays independent of it.

use super::{card, hint};
use crate::PdfCraftApp;
use crate::theme::Tokens;

/// The Reading colours card. Solid text and line-art paints are remapped while rendering, and the
/// page clears to the background colour. The preference is app-wide: every open document follows
/// it, and documents open with it.
pub(super) fn show(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    card(ui, t, Some(tl!("Reading colours")), |ui| reading_colours(ui, app, t));
}

fn reading_colours(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let mode = app.reading_colors.mode_name();
    ui.horizontal_wrapped(|ui| {
        for (name, label, colors) in [
            ("off", "Off", pdfcraft_render::ReadingColors::default()),
            ("black-on-white", "Black on white", pdfcraft_render::ReadingColors::BLACK_ON_WHITE),
            ("white-on-black", "White on black", pdfcraft_render::ReadingColors::WHITE_ON_BLACK),
            ("yellow-on-black", "Yellow on black", pdfcraft_render::ReadingColors::YELLOW_ON_BLACK),
            ("green-on-black", "Green on black", pdfcraft_render::ReadingColors::GREEN_ON_BLACK),
        ] {
            let params = colors.params.unwrap_or(pdfcraft_render::ReadingParams {
                text: [0, 0, 0],
                background: [255, 255, 255],
                only_black: false,
                line_art: true,
            });
            ui.vertical(|ui| {
                ui.set_width(104.0);
                let sample = egui::RichText::new("Aa").size(22.0).color(egui::Color32::from_rgb(params.text[0], params.text[1], params.text[2]));
                let button = egui::Button::new(sample)
                    .fill(egui::Color32::from_rgb(params.background[0], params.background[1], params.background[2]))
                    .min_size(egui::vec2(94.0, 44.0))
                    .stroke(egui::Stroke::new(if mode == name { 2.0 } else { 1.0 }, if mode == name { t.accent } else { t.border }));
                let response = ui.add(button);
                response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, mode == name, tl!(label)));
                if response.clicked() {
                    app.set_reading_colors(colors);
                }
                ui.add(egui::Label::new(tl!(label)).wrap());
            });
        }
    });
    if let Some(params) = app.reading_colors.params {
        let text = egui::Color32::from_rgb(params.text[0], params.text[1], params.text[2]);
        let background = egui::Color32::from_rgb(params.background[0], params.background[1], params.background[2]);
        egui::Frame::new().fill(background).inner_margin(16.0).show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 32.0).max(0.0));
            ui.label(egui::RichText::new(tl!("Reading colours")).size(18.0).color(text));
            ui.add(
                egui::Label::new(egui::RichText::new(tl!("Colours are remapped while rendering; the document itself never changes.")).color(text))
                    .wrap(),
            );
        });
    } else {
        ui.label(tl!("Original"));
    }
    egui::CollapsingHeader::new(tl!("Custom")).default_open(mode == "custom").show(ui, |ui| {
        // Opening Custom changes nothing; editing a colour or a flag applies it, starting from
        // the colours already chosen, or black on white.
        let mut colors = if app.reading_colors.params.is_some() { app.reading_colors } else { pdfcraft_render::ReadingColors::BLACK_ON_WHITE };
        let mut changed = false;
        if let Some(params) = colors.params.as_mut() {
            super::wrapped(ui, |ui| {
                let text = ui.label(tl!("Text"));
                changed |= ui.color_edit_button_srgb(&mut params.text).labelled_by(text.id).changed();
                ui.add_space(12.0);
                let background = ui.label(tl!("Page background"));
                changed |= ui.color_edit_button_srgb(&mut params.background).labelled_by(background.id).changed();
            });
            changed |= ui.add(crate::widgets::checkbox(&mut params.only_black, tl!("Only change the colour of black text or line art"))).changed();
            changed |= ui.add(crate::widgets::checkbox(&mut params.line_art, tl!("Change the colour of line art as well as text"))).changed();
        }
        if changed {
            app.set_reading_colors(colors);
        }
    });
    hint(ui, t, tl!("Colours are remapped while rendering; the document itself never changes."));
}
