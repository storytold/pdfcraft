//! Preferences ▸ Fill & Sign: flattening on save, and the date format Fill & Sign writes.

use super::{card, field_label, hint, wrapped};
use crate::theme::Tokens;
use crate::{PdfCraftApp, widgets};

pub(super) fn show(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    card(ui, t, None, |ui| {
        ui.add(widgets::checkbox(&mut app.flatten_fill_sign_on_save, tl!("Flatten Fill & Sign when saving")));
        hint(ui, t, tl!("Text, marks and signatures become part of the page. Other comments stay editable."));
    });
    card(ui, t, Some(tl!("Date format")), |ui| date_format(ui, app, t));
}

/// Preferences ▸ Date format: a preset, or any pattern typed in, for Fill & Sign dates, and the
/// language of its month and weekday names. A valid pattern applies as it's typed; an invalid
/// one stays in the box with the reason, and the last valid one stays in use.
fn date_format(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    use pdfcraft_engine::dates::{DATE_FORMATS, DATE_LANGUAGES, MAX_DATE_FORMAT_CHARS};
    let sample = |app: &PdfCraftApp, f: &str| format!("{}   {f}", app.date_text(Some(f)).unwrap_or_default());
    {
        let label = field_label(ui, t, tl!("Fill & Sign dates"));
        let current = app.session.date_format().to_string();
        let selected = if DATE_FORMATS.contains(&current.as_str()) { sample(app, &current) } else { tl!("Custom").to_string() };
        craft_ui::ComboBox::from_id_salt("date-format")
            .selected_text(selected)
            .width(240.0)
            .show_ui(ui, |ui| {
                for f in DATE_FORMATS {
                    if ui.selectable_label(current == f, sample(app, f)).clicked() && app.session.set_date_format(f).is_ok() {
                        app.date_format_draft = None;
                        ui.close();
                    }
                }
            })
            .response
            .labelled_by(label.id);
    }
    let label = field_label(ui, t, tl!("Pattern"));
    wrapped(ui, |ui| {
        let mut text = app.date_format_draft.clone().unwrap_or_else(|| app.session.date_format().to_string());
        let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(160.0).char_limit(MAX_DATE_FORMAT_CHARS)).labelled_by(label.id);
        // The box keeps the text as typed (a trailing space included) while it has focus; the
        // setting takes the trimmed pattern whenever it's valid.
        if edit.changed() {
            let _ = app.session.set_date_format(&text);
            app.date_format_draft = Some(text.clone());
        }
        if !edit.has_focus() && pdfcraft_engine::dates::check_date_format(&text).is_ok() {
            app.date_format_draft = None;
        }
        match app.date_text(Some(&text)) {
            Ok(today) => {
                ui.add(egui::Label::new(egui::RichText::new(crate::i18n::fmt(tl!("Today: {date}"), &[("date", &today)])).color(t.text_muted)).wrap())
            }
            // The engine's reason, in English.
            Err(e) => ui.add(egui::Label::new(egui::RichText::new(e).color(egui::Color32::from_rgb(0xD1, 0x3B, 0x3B))).wrap()),
        };
    });
    // Fill & Sign refuses a date it would save as "?"; say so before anyone tries.
    let bad = app.date_text(None).map(|today| pdfcraft_engine::dates::unwritable(&today)).unwrap_or_default();
    if !bad.is_empty() {
        ui.add(
            egui::Label::new(
                egui::RichText::new(crate::i18n::fmt(
                    tl!(
                        "{chars} can't be written into the PDF yet: Fill & Sign text is Western European only. Pick a numeric format such as dd.mm.yyyy."
                    ),
                    &[("chars", &bad)],
                ))
                .color(egui::Color32::from_rgb(0xD1, 0x3B, 0x3B)),
            )
            .wrap(),
        );
    }
    {
        let label = field_label(ui, t, tl!("Language"));
        // Build every label before the combo box mutably borrows `ui`, with the same drawable
        // fallback the interface language picker uses.
        let (follow, selected, current, names) = {
            let drawable = |text: &str| super::picker_text_is_drawable(ui, text);
            let interface = crate::i18n::current().picker_name(drawable);
            let follow = crate::i18n::fmt(tl!("Same as interface ({language})"), &[("language", interface)]);
            let current = app.session.date_language().map(str::to_string);
            let selected = DATE_LANGUAGES
                .iter()
                .find(|l| current.as_deref() == Some(l.code))
                .map_or_else(|| follow.clone(), |l| super::date_language_picker_name(l, drawable).to_string());
            let names = DATE_LANGUAGES.iter().map(|l| (l, super::date_language_picker_name(l, drawable))).collect::<Vec<_>>();
            (follow, selected, current, names)
        };
        craft_ui::ComboBox::from_id_salt("date-language")
            .selected_text(selected)
            .width(240.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), follow).clicked() && app.session.set_date_language(None).is_ok() {
                    ui.close();
                }
                for (l, name) in &names {
                    if ui.selectable_label(current.as_deref() == Some(l.code), *name).clicked() && app.session.set_date_language(Some(l.code)).is_ok()
                    {
                        ui.close();
                    }
                }
            })
            .response
            .labelled_by(label.id);
    }
    hint(
        ui,
        t,
        &crate::i18n::fmt(
            tl!(
                "yyyy or yy year · m or mm month · mmm or mmmm month name · d or dd day · ddd or dddd weekday. H, h, M, s and t are time letters and can't be used on their own: put {escape} before a letter to show it as is."
            ),
            &[("escape", "\\")],
        ),
    );
}
