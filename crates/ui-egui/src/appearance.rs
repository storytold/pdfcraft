//! Preferences › Appearance: the appearance mode (follow the system, light or dark) above a light
//! and a dark theme card, each with a small picture of PdfCraft in that theme.

use egui::{Rect, RichText, pos2, vec2};

use crate::PdfCraftApp;
use crate::theme::{self, ThemeKind, ThemePreference, Tokens};
use crate::widgets;

/// The themes each card offers. One of each today; a new theme is one more row here.
pub const LIGHT_THEMES: &[(ThemeKind, &str)] = &[(ThemeKind::Light, "Light gray")];
pub const DARK_THEMES: &[(ThemeKind, &str)] = &[(ThemeKind::Dark, "Dark gray")];

fn mode_label(preference: ThemePreference) -> &'static str {
    match preference {
        ThemePreference::System => "Use system setting",
        ThemePreference::Light => "Light",
        ThemePreference::Dark => "Dark",
    }
}

fn mode_command(preference: ThemePreference) -> &'static str {
    match preference {
        ThemePreference::System => "view.theme.system",
        ThemePreference::Light => "view.theme.light",
        ThemePreference::Dark => "view.theme.dark",
    }
}

/// A miniature PdfCraft window in `kind`'s colours: title bar and tabs, the tool bar, the page
/// thumbnails, a page on the pasteboard and the right-hand panel.
fn preview(ui: &mut egui::Ui, kind: ThemeKind, width: f32) {
    let p = Tokens::for_kind(kind);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 118.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, p.chrome);
    let title = Rect::from_min_size(rect.min, vec2(rect.width(), 12.0));
    painter.rect_filled(title, 0.0, p.titlebar);
    // The active tab and the theme button.
    painter.rect_filled(Rect::from_min_size(title.min + vec2(6.0, 3.0), vec2(36.0, 9.0)), 2.0, p.panel);
    painter.rect_filled(Rect::from_min_size(title.min + vec2(9.0, 6.0), vec2(20.0, 2.0)), 1.0, p.text);
    painter.circle_filled(pos2(title.right() - 7.0, title.center().y), 2.5, p.icon);
    // The tool bar, with its first tool active.
    let tools = Rect::from_min_size(pos2(rect.left(), title.bottom()), vec2(rect.width(), 12.0));
    painter.rect_filled(tools, 0.0, p.panel);
    for i in 0..8 {
        let c = if i == 0 { p.accent } else { p.icon };
        painter.rect_filled(Rect::from_min_size(tools.min + vec2(8.0 + i as f32 * 9.0, 3.5), vec2(5.0, 5.0)), 1.0, c);
    }
    let body = Rect::from_min_max(pos2(rect.left(), tools.bottom() + 1.0), rect.max);
    // Page thumbnails on the left, the second one selected.
    let rail = Rect::from_min_size(body.min, vec2((body.width() * 0.16).round(), body.height()));
    painter.rect_filled(rail, 0.0, p.panel);
    for i in 0..3 {
        let thumb = Rect::from_min_size(rail.min + vec2(rail.width() * 0.25, 6.0 + i as f32 * 26.0), vec2(rail.width() * 0.5, 20.0));
        if i == 1 {
            painter.rect_filled(thumb.expand(2.0), 2.0, p.accent_soft);
        }
        painter.rect_filled(thumb, 1.0, egui::Color32::WHITE);
        painter.rect_stroke(thumb, 1.0, egui::Stroke::new(0.5, p.border), egui::StrokeKind::Outside);
    }
    // The right-hand panel: a heading and a few rows.
    let side = Rect::from_min_max(pos2(body.right() - (body.width() * 0.22).round(), body.top()), body.max);
    painter.rect_filled(side, 0.0, p.panel);
    painter.rect_filled(Rect::from_min_size(side.min + vec2(5.0, 5.0), vec2(side.width() * 0.5, 2.5)), 1.0, p.text);
    for i in 0..4 {
        let y = side.top() + 14.0 + i as f32 * 8.0;
        painter.rect_filled(Rect::from_min_size(pos2(side.left() + 5.0, y), vec2(side.width() * 0.7, 2.0)), 1.0, p.text_muted);
    }
    // The page, white in both themes, with a heading and lines of text.
    let paste = Rect::from_min_max(pos2(rail.right(), body.top()), pos2(side.left(), body.bottom()));
    painter.rect_filled(paste, 0.0, p.pasteboard);
    let page_h = paste.height() - 10.0;
    let page = Rect::from_center_size(paste.center(), vec2(page_h * 0.77, page_h));
    painter.rect_filled(page.translate(vec2(1.0, 1.0)), 1.0, p.page_shadow);
    painter.rect_filled(page, 1.0, egui::Color32::WHITE);
    let ink = egui::Color32::from_gray(0x60);
    painter.rect_filled(Rect::from_min_size(page.min + vec2(6.0, 7.0), vec2(page.width() * 0.5, 3.0)), 1.0, egui::Color32::from_gray(0x30));
    for i in 0..8 {
        let w = if i % 4 == 3 { 0.45 } else { 0.8 };
        painter.rect_filled(Rect::from_min_size(page.min + vec2(6.0, 15.0 + i as f32 * 6.0), vec2(page.width() * w - 6.0, 1.5)), 0.5, ink);
    }
}

/// One theme card: its title, Active when it is what shows now, a preview of the chosen theme
/// and a radio row per theme. Returns true when the card or one of its radio rows was clicked.
fn card(ui: &mut egui::Ui, t: &Tokens, title: &str, active: bool, themes: &[(ThemeKind, &str)]) -> bool {
    let inner = egui::Frame::new()
        .fill(t.card)
        .stroke(egui::Stroke::new(1.0, if active { t.accent } else { t.border }))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).font(theme::semibold(12.5)).color(t.text));
                if active {
                    ui.label(RichText::new(tl!("Active")).color(t.accent_text).small());
                }
            });
            ui.add_space(4.0);
            let shown = themes.first().map_or(ThemeKind::Light, |(kind, _)| *kind);
            preview(ui, shown, ui.available_width());
            ui.add_space(4.0);
            let mut clicked = false;
            for (kind, label) in themes {
                // One theme per card today, so it is always the chosen one.
                clicked |= ui.add(widgets::radio(*kind == shown, tl!(label))).clicked();
            }
            clicked
        });
    // The whole card picks its theme, not just the radio row.
    let whole = inner.response.interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    inner.inner || whole.clicked()
}

/// The Appearance section of Preferences.
pub fn preferences_section(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    ui.label(RichText::new(tl_ctx!("theme", "Appearance")).font(theme::semibold(13.0)));
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Appearance mode"));
        let combo = egui::ComboBox::from_id_salt("appearance-mode").selected_text(tl!(mode_label(app.theme_preference))).show_ui(ui, |ui| {
            for preference in [ThemePreference::System, ThemePreference::Light, ThemePreference::Dark] {
                if ui.selectable_label(app.theme_preference == preference, tl!(mode_label(preference))).clicked() {
                    app.execute(mode_command(preference));
                    ui.close();
                }
            }
        });
        // The visible label names the combo box for assistive technology and tests.
        combo.response.labelled_by(label.id);
    });
    ui.add_space(4.0);
    let light_active = app.theme == ThemeKind::Light;
    ui.columns(2, |cols| {
        let [light, dark] = cols else { return };
        if card(light, t, tl!("Light theme"), light_active, LIGHT_THEMES) {
            app.execute(mode_command(ThemePreference::Light));
        }
        if card(dark, t, tl!("Dark theme"), !light_active, DARK_THEMES) {
            app.execute(mode_command(ThemePreference::Dark));
        }
    });
    ui.add_space(4.0);
    // Acrobat's "Highlight existing fields": shared with the toggle in the form bar.
    ui.add(crate::widgets::checkbox(&mut app.view_defaults.highlight_fields, tl!("Highlight fields")));
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// The mode is chosen in Preferences and both theme cards show, the one on screen marked
    /// Active.
    #[test]
    fn preferences_choose_the_appearance_mode_and_show_both_themes() {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let mut fonts = false;
        let mut h = Harness::builder().with_size(egui::vec2(640.0, 400.0)).build_ui_state(
            move |ui, app: &mut PdfCraftApp| {
                // The app's fonts (the semibold headings) take effect from the next frame.
                if !fonts {
                    ui.ctx().set_fonts(theme::font_definitions());
                    fonts = true;
                    return;
                }
                let t = Tokens::get(ui.ctx());
                preferences_section(ui, app, &t);
            },
            app,
        );
        h.run();
        assert!(h.query_by_label("Light theme").is_some() && h.query_by_label("Dark theme").is_some());
        assert!(h.query_by_label("Light gray").is_some() && h.query_by_label("Dark gray").is_some());
        assert_eq!(h.query_all_by_label("Active").count(), 1);
        if let Ok(path) = std::env::var("PDFCRAFT_APPEARANCE_SHOT") {
            h.render().unwrap().save(path).unwrap();
        }

        // The field-highlight preference is on for new settings, and the box turns it off.
        assert!(h.state().view_defaults.highlight_fields);
        h.get_by_label("Highlight fields").click();
        h.run();
        assert!(!h.state().view_defaults.highlight_fields);

        let combo = egui::accesskit::Role::ComboBox;
        h.get_by_role(combo).click();
        h.run();
        h.get_by_label("Dark").click();
        h.run();
        assert_eq!(h.state().theme_preference, ThemePreference::Dark);
        h.get_by_role(combo).click();
        h.run();
        h.get_by_label("Use system setting").click();
        h.run();
        assert_eq!(h.state().theme_preference, ThemePreference::System);
    }

    /// Clicking a theme card, or its radio row, picks that theme, not only the mode combo box.
    #[test]
    fn clicking_a_theme_card_picks_that_theme() {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let mut fonts = false;
        let mut h = Harness::builder().with_size(egui::vec2(640.0, 400.0)).build_ui_state(
            move |ui, app: &mut PdfCraftApp| {
                if !fonts {
                    ui.ctx().set_fonts(theme::font_definitions());
                    fonts = true;
                    return;
                }
                let t = Tokens::get(ui.ctx());
                preferences_section(ui, app, &t);
            },
            app,
        );
        h.run();
        h.get_by_label("Dark gray").click();
        h.run();
        assert_eq!((h.state().theme_preference, h.state().theme), (ThemePreference::Dark, ThemeKind::Dark));
        // The card's title is not a widget of its own: the click lands on the card.
        h.get_by_label("Light theme").click();
        h.run();
        assert_eq!((h.state().theme_preference, h.state().theme), (ThemePreference::Light, ThemeKind::Light));
    }
}
