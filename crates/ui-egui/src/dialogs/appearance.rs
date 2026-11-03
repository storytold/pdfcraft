//! Preferences › Appearance: the appearance mode (follow the system, light or dark) above a light
//! and a dark theme card, each with a small picture of PdfCraft in that theme.

use egui::{CornerRadius, Rect, RichText, Stroke, pos2, vec2};

use crate::PdfCraftApp;
use crate::theme::{self, ThemeKind, ThemePreference, Tokens};

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

/// One theme card: its title, Active when it is what shows now, and a preview of the chosen theme.
/// Returns whether the card was clicked and the control's Response.
fn card(ui: &mut egui::Ui, t: &Tokens, title: &str, active: bool, themes: &[(ThemeKind, &str)], query: &str) -> (bool, egui::Response) {
    let mut card_clicked = false;
    let frame_stroke = if active { Stroke::new(1.5, t.accent) } else { Stroke::new(1.0, t.border) };
    let scope_res = ui.scope_builder(egui::UiBuilder::new().id_salt(title).sense(egui::Sense::click()), |ui| {
        egui::Frame::new().fill(t.card).stroke(frame_stroke).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(
            ui,
            |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let font = theme::semibold(12.5);
                    let ranges = crate::dialogs::preferences::find_match_ranges(title, query);
                    let galley = if ranges.is_empty() {
                        crate::bidi::layout_ui(ui, title.to_owned(), font, t.text)
                    } else {
                        let (job, _) = crate::dialogs::preferences::create_highlight_job(title, &ranges, font, t.text);
                        ui.painter().layout_job(job)
                    };
                    let (title_rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
                    ui.painter().galley(title_rect.min, galley, t.text);
                    if active {
                        ui.label(RichText::new(tl!("Active")).color(t.accent_text).small());
                    }
                });
                ui.add_space(4.0);
                let shown = themes.first().map_or(ThemeKind::Light, |(kind, _)| *kind);
                preview(ui, shown, ui.available_width());
            },
        );
    });

    let resp = scope_res.response;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, active, title));

    if resp.has_focus() {
        ui.painter().rect_stroke(resp.rect, CornerRadius::same(6), Stroke::new(2.0, t.accent_text), egui::StrokeKind::Outside);
    }

    let resp = theme::hand(resp);
    let key_select = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::Enter));
    if resp.clicked() || key_select {
        card_clicked = true;
    }
    (card_clicked, resp)
}

/// The Appearance section of Preferences.
#[cfg_attr(not(test), expect(dead_code, reason = "called by unit tests"))]
pub fn preferences_section(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let mut ctx = crate::dialogs::preferences::PrefSearchContext::new("", false);
    preferences_section_with_context(ui, app, t, &mut ctx);
}

/// The Appearance section of Preferences with live search highlighting and scrolling context.
pub(crate) fn preferences_section_with_context(
    ui: &mut egui::Ui,
    app: &mut PdfCraftApp,
    t: &Tokens,
    ctx: &mut crate::dialogs::preferences::PrefSearchContext,
) {
    ctx.label(ui, tl_ctx!("theme", "Appearance"), theme::semibold(13.0), t.text);
    ui.horizontal(|ui| {
        let label = ctx.label(ui, tl!("Appearance mode"), theme::regular(13.0), t.text);
        let combo = craft_ui::ComboBox::from_id_salt("appearance-mode").selected_text(tl!(mode_label(app.theme_preference))).show_ui(ui, |ui| {
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
    let light_title = tl!("Light theme");
    let dark_title = tl!("Dark theme");
    let light_matches = crate::dialogs::preferences::find_match_ranges(light_title, ctx.query).len();
    let dark_matches = crate::dialogs::preferences::find_match_ranges(dark_title, ctx.query).len();
    ctx.total_matches += light_matches + dark_matches;

    ui.columns(2, |cols| {
        let [light, dark] = cols else { return };
        let (light_clicked, light_resp) = card(light, t, light_title, light_active, LIGHT_THEMES, ctx.query);
        if light_clicked {
            app.execute(mode_command(ThemePreference::Light));
        }
        ctx.scroll_if_first_match(&light_resp, light_matches);

        let (dark_clicked, dark_resp) = card(dark, t, dark_title, !light_active, DARK_THEMES, ctx.query);
        if dark_clicked {
            app.execute(mode_command(ThemePreference::Dark));
        }
        ctx.scroll_if_first_match(&dark_resp, dark_matches);
    });
    ui.add_space(4.0);
    // Acrobat's "Highlight existing fields": shared with the toggle in the form bar.
    ctx.checkbox(ui, &mut app.view_defaults.highlight_fields, tl!("Highlight fields"), t);
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

    /// Clicking a theme card picks that theme, not only the mode combo box.
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
        h.get_by_label("Dark theme").click();
        h.run();
        assert_eq!((h.state().theme_preference, h.state().theme), (ThemePreference::Dark, ThemeKind::Dark));
        h.get_by_label("Light theme").click();
        h.run();
        assert_eq!((h.state().theme_preference, h.state().theme), (ThemePreference::Light, ThemeKind::Light));
    }
}
