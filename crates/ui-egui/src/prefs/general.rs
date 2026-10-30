//! Preferences ▸ General: the interface's language and size, the side of the tools panel, the
//! workspace a PDF opens in, and what happens to files between sessions.

use super::tiles::{self, Picture, Tile};
use super::{card, field_label, hint};
use crate::theme::Tokens;
use crate::{PdfCraftApp, widgets};

pub(super) fn show(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    card(ui, t, None, |ui| {
        let label = field_label(ui, t, tl!("Interface language"));
        language(ui, app, label.id);
        let label = field_label(ui, t, tl!("Interface size"));
        size(ui, app, label.id);
        hint(ui, t, tl!("Auto follows the system's text size. Pages always render at the display's full resolution."));
    });
    card(ui, t, Some(tl!("Tools panel side")), |ui| {
        use crate::side_panels::ToolsSide;
        let mut side = app.side_panels.tools_side;
        tiles::choose(
            ui,
            &mut side,
            &[
                Tile { value: ToolsSide::Left, label: tl!("Left"), picture: Picture::ToolsSide { right: false } },
                Tile { value: ToolsSide::Right, label: tl!("Right"), picture: Picture::ToolsSide { right: true } },
            ],
        );
        app.side_panels.tools_side = side;
    });
    card(ui, t, Some(tl!("Default workspace mode")), |ui| {
        use crate::Mode;
        tiles::choose(
            ui,
            &mut app.default_mode,
            &[
                Tile { value: Mode::AllTools, label: tl!("All tools"), picture: Picture::Icon("layout-grid") },
                Tile { value: Mode::Read, label: tl!("Read"), picture: Picture::Icon("book-open") },
                Tile { value: Mode::Edit, label: tl!("Edit"), picture: Picture::Icon("pencil") },
                Tile { value: Mode::Convert, label: tl!("Convert"), picture: Picture::Icon("file-output") },
                Tile { value: Mode::Sign, label: tl!("E-Sign"), picture: Picture::Icon("signature") },
            ],
        );
        hint(ui, t, tl!("Used when opening PDFs. An explicit launch or control mode takes precedence for the session."));
    });
    card(ui, t, Some(tl!("Documents and view")), |ui| {
        // The web build has no file paths to reopen and no files on disk to watch.
        #[cfg(not(target_arch = "wasm32"))]
        {
            ui.add(widgets::checkbox(&mut app.reopen_last_session, tl!("Reopen the files that were open when PdfCraft last closed")));
            ui.add(widgets::checkbox(&mut app.reload_changed_files, tl!("Reload documents when their files change on disk")));
        }
        // The notice's "Don't show again" turns this off (#4035); the restrictions stay either way.
        ui.add(widgets::checkbox(&mut app.show_secured_notice, tl!("Show a notice when a document is secured")));
    });
}

/// The interface language. A native name is only drawn when these fonts can draw it; otherwise
/// the English name keeps the list readable instead of aborting on a missing glyph (for example
/// Hindi without a Devanagari face, and installed system fonts turned off).
fn language(ui: &mut egui::Ui, app: &mut PdfCraftApp, label: egui::Id) {
    let (selected, picker) = {
        let drawable = |text: &str| super::picker_text_is_drawable(ui, text);
        let selected =
            crate::i18n::Lang::from_code(&app.language).map_or_else(|| tl!("Auto").to_owned(), |language| language.picker_name(drawable).to_owned());
        let picker = crate::i18n::Lang::for_picker_by(|language| language.picker_name(drawable))
            .into_iter()
            .map(|language| (language, language.picker_name(drawable)))
            .collect::<Vec<_>>();
        (selected, picker)
    };
    let before = app.language.clone();
    craft_ui::ComboBox::from_id_salt("interface-language")
        .selected_text(selected)
        .width(220.0)
        .show_ui(ui, |ui| {
            if ui.selectable_value(&mut app.language, crate::i18n::AUTO.to_string(), tl!("Auto")).clicked() {
                ui.close();
            }
            for (language, name) in &picker {
                if ui.selectable_value(&mut app.language, language.code().to_string(), *name).clicked() {
                    ui.close();
                }
            }
        })
        .response
        .labelled_by(label);
    // Relabel the rest of this dialog in the new language right away, not next frame.
    if app.language != before {
        crate::i18n::set_current(crate::i18n::Lang::from_pref(&app.language));
    }
}

/// The interface size. Auto shows the size it draws at: the display's scale times the system's
/// text scale.
fn size(ui: &mut egui::Ui, app: &mut PdfCraftApp, label: egui::Id) {
    use crate::ui_scale;
    let native = ui.ctx().native_pixels_per_point().unwrap_or(1.0);
    let auto_percent = ui_scale::percent(ui_scale::drawn(None, app.system_text_scale, native));
    let auto = crate::i18n::fmt(tl!("Auto ({percent}%)"), &[("percent", &auto_percent.to_string())]);
    let selected = app.ui_scale.map_or_else(|| auto.clone(), |f| format!("{}%", ui_scale::percent(f)));
    craft_ui::ComboBox::from_id_salt("interface-size")
        .selected_text(selected)
        .width(220.0)
        .show_ui(ui, |ui| {
            if ui.selectable_value(&mut app.ui_scale, None, auto).clicked() {
                ui.close();
            }
            for p in ui_scale::PRESETS {
                let factor = f32::from(p) / 100.0;
                if ui.selectable_label(app.ui_scale.is_some_and(|f| ui_scale::percent(f) == u32::from(p)), format!("{p}%")).clicked() {
                    app.ui_scale = Some(factor);
                    ui.close();
                }
            }
        })
        .response
        .labelled_by(label);
}
