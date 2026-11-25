//! Window composition: the shell's title bar, mode bar, view rail, dock and page area.

use craft::behavior::IconId;
use craft::keymap::CommandRef;
use craft::menu::ThemeChoice;
use craft::theme::{ModeChoice, TypeRole};
use craft::ui::buttons::{Button, IconButton};
use craft::ui::contract::{Bind, Outcome as _, Props as _, Show as _, ShowContainer as _};
use craft::ui::display::Text;
use craft::ui::menu::{Menu, MenuItem};
use craft::ui::popover::DropVeil;
use craft::ui::runtime::env::{Env, WindowKey};
use craft::ui::shell::{AppShell, Slot, TitleBar};
use egui::{Color32, CornerRadius, Sense, Stroke, vec2};

use crate::canvas::PageBackground;
use crate::panes::Place;
use crate::{PdfCraftApp, side_panels};

pub(crate) mod doc_tabs;
mod mode_bar;
pub(crate) mod view_rail;

pub(crate) const HOME_ICON: &str = "home";

/// Compose the window: title bar, mode bar, view rail and the dock, whose document panel holds the
/// pages.
///
/// The shell carries out every screen-mode change; the mode it ends the pass with goes back to the
/// app, which does the presentation work in one place.
pub(crate) fn shell(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    // Production's dialogs end with their buttons: no close x in the title row.
    craft::ui::dialog::set_close_button(ui.ctx(), false);
    let hidden = app.hidden_interface();
    let modes_visible = !hidden.mode_bar(app.native_menu.is_some());
    let window_ui = !hidden.window_ui;
    let fallback_rail = window_ui && side_panels::rail_anchor(app).is_none();
    let out = AppShell::new(WindowKey::main())
        .toast_alignment(egui::Align2::CENTER_BOTTOM)
        .screen_mode(Bind::given(app.screen_mode))
        // A presentation shows the page and nothing else, so no reveal brings the bars back.
        .allow_reveal(false)
        // macOS with a right-to-left system language puts the traffic lights at the right: the title bar mirrors
        // with them, Home beside the lights and the tabs reading from the right (production 4448ee940).
        .title_follows_lights(true)
        .title_bar(|ui, _, app: &mut PdfCraftApp| title_bar(app, ui))
        .top(Slot::new("modes").visible(modes_visible), |ui, _, app: &mut PdfCraftApp| mode_bar::show(app, ui))
        .start(
            Slot::new("view-rail-start").visible(fallback_rail && app.rail_side == craft::behavior::DockSide::Start),
            |ui, _, app: &mut PdfCraftApp| view_rail::show(app, ui),
        )
        .end(Slot::new("view-rail").visible(fallback_rail && app.rail_side == craft::behavior::DockSide::End), |ui, _, app: &mut PdfCraftApp| {
            view_rail::show(app, ui)
        })
        .body(|ui, _, app: &mut PdfCraftApp| window_body(app, ui))
        .show(ui, app);
    // PLAN: assumed `set_screen_mode(&mut self, ScreenMode)` (pdf-shell-w8)
    app.set_screen_mode(out.screen_mode);
}

/// The dock, under the veil that shows files held over the window. Files dropped on the window open as
/// documents; the Combine files tab takes its own through its veil.
fn window_body(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let mut dropped = None;
    let organize = app.active_ids().is_some_and(|(index, id)| {
        app.session.get(id).is_some_and(|doc| doc.allows_assembly()) && app.views.get(index).is_some_and(|view| view.organize)
    });
    let veil = DropVeil::new(tl!("Drop to open the files"));
    let veil = if organize { veil.positional() } else { veil };
    let _ = veil.show_with_files(ui, |ui, files| {
        if let Some(files) = files {
            dropped = Some(files.files.clone());
        }
        side_panels::dock(app, ui);
    });
    if let Some(files) = dropped
        && !app.combine_showing()
    {
        // While the page grid shows, it takes files dropped anywhere in the window: one let go off the grid goes
        // in after the last page, as there is no gap under the pointer to say where.
        let grid = app.active_ids().filter(|(_, id)| app.session.get(*id).is_some_and(|doc| doc.allows_assembly())).map(|(index, _)| index);
        match grid.and_then(|index| app.views.get_mut(index)).filter(|view| view.organize) {
            Some(view) => view.dropped_files = Some((usize::MAX, files)),
            None => {
                let ctx = ui.ctx().clone();
                app.open_dropped(&ctx, files);
            }
        }
    }
}

/// The title bar: Home, the document tabs and Open, then the community, shortcuts, theme and
/// Preferences buttons. The window's caption buttons are the bar's own.
fn title_bar(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let (theme_icon, theme_label) = match app.theme_preference {
        ModeChoice::System => ("monitor", tl!("Use system setting")),
        ModeChoice::Light => ("sun", tl!("Light gray")),
        ModeChoice::Dark => ("moon", tl!("Dark gray")),
    };
    // The button names the choice it shows; a click steps to the next one.
    let theme_label = format!("{}: {theme_label}", tl!("Display theme"));
    let out = TitleBar::new()
        .trailing(|ui, _| {
            Button::command("help.discord").label("Discord").icon(IconId::new("messages-square")).tooltip(tl!("Join the ArtCraft Discord")).show(ui);
            IconButton::command(IconId::new("circle-help"), "help.shortcuts").show(ui);
            let theme_button = IconButton::command(IconId::new(theme_icon), ThemeChoice::NextMode.command()).label(theme_label.clone()).show(ui);
            let opener = theme_button.response().clone();
            Menu::context(tl!("Appearance"), &opener).show(ui, |ui, _| {
                let appearance = CommandRef::with_params("app.preferences", serde_json::json!({ "tab": "appearance" }));
                MenuItem::command(appearance).label(tl!("Appearance settings…")).show(ui);
            });
            IconButton::command(IconId::new("settings"), "app.preferences").show(ui);
        })
        .show(ui, |ui, _| {
            let home = app.active.is_none() && !app.combine_showing();
            if IconButton::new(IconId::new(HOME_ICON)).label(tl!("Home")).selected(home).show(ui).clicked() {
                app.active = None;
                app.combine_tab.focused = false;
            }
            doc_tabs::strip(app, ui, Place::Main);
        })
        .output;
    if out.close_requested {
        app.request_quit(&ctx);
    }

    if app.views.is_empty() {
        app.reading_bg.picker_open = false;
    }

    if app.reading_bg.picker_open {
        let reading_bg = &mut app.reading_bg;
        let mut open = reading_bg.picker_open;
        let opener_id = reading_bg.picker_opener;
        let focus_first = std::mem::take(&mut reading_bg.picker_focus_pending);
        let win_id = egui::Id::unique("reading-bg-tint-window");
        let close_on_esc = ui.ctx().input(|i| i.key_pressed(egui::Key::Escape));
        if close_on_esc {
            open = false;
        } else {
            egui::Window::new(tl!("Page Background & Tint"))
                .id(win_id)
                .open(&mut open)
                .resizable(false)
                .collapsible(false)
                .default_width(260.0)
                .show(ui.ctx(), |ui| {
                    Text::new(tl!("Tint Strength").to_string()).role(TypeRole::Label).show(ui);
                    let mut percent = (reading_bg.tint_strength * 100.0).round();
                    let slider = ui.add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix("%").step_by(1.0));
                    if focus_first {
                        slider.request_focus();
                    }
                    if slider.changed() {
                        reading_bg.tint_strength = (percent / 100.0).clamp(0.0, 1.0);
                        if reading_bg.mode == PageBackground::Default {
                            reading_bg.mode = PageBackground::Dark;
                        }
                        ui.ctx().request_repaint();
                    }
                    ui.add_space(8.0);
                    Text::new(tl!("Preset Swatches").to_string()).role(TypeRole::Label).show(ui);
                    let presets = [
                        (tl!("White"), Color32::WHITE),
                        (tl!("Warm Sepia"), Color32::from_rgb(250, 240, 215)),
                        (tl!("Grey"), Color32::from_rgb(160, 160, 160)),
                        (tl!("Dark Grey"), Color32::from_rgb(0x1E, 0x1E, 0x1E)),
                        (tl!("Pure Black"), Color32::BLACK),
                    ];
                    let accent = Env::of(ui).tokens.color.accent.solid;
                    let accent_c32 = Color32::from_rgba_unmultiplied(accent.0[0], accent.0[1], accent.0[2], accent.0[3]);
                    ui.horizontal(|ui| {
                        for (name, color) in presets {
                            let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                            let is_selected = reading_bg.color == color;
                            let stroke = if is_selected { Stroke::new(2.0, accent_c32) } else { Stroke::new(1.0, Color32::from_gray(100)) };
                            ui.painter().rect(rect, CornerRadius::same(4), color, stroke, egui::StrokeKind::Inside);
                            let name_str = name.to_string();
                            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, is_selected, name_str.clone()));
                            if resp.on_hover_text(name).clicked() {
                                reading_bg.color = color;
                                if reading_bg.mode == PageBackground::Default {
                                    reading_bg.mode = PageBackground::Dark;
                                }
                                ui.ctx().request_repaint();
                            }
                        }
                    });
                    ui.add_space(8.0);
                    Text::new(tl!("Custom Color").to_string()).role(TypeRole::Label).show(ui);
                    if egui::color_picker::color_picker_color32(ui, &mut reading_bg.color, egui::color_picker::Alpha::Opaque) {
                        if reading_bg.mode == PageBackground::Default {
                            reading_bg.mode = PageBackground::Dark;
                        }
                        ui.ctx().request_repaint();
                    }
                });
        }
        if !open
            && reading_bg.picker_open
            && let Some(opener) = opener_id
        {
            ui.ctx().memory_mut(|m| m.request_focus(opener));
            reading_bg.picker_opener = None;
        }
        app.reading_bg.picker_open = open;
    }
}

/// What the rail shows under the page box: the page count, or, when the current page's label is
/// not its physical number (front matter numbered i, ii, …), its position as "3/180" (#2834).
fn page_position(current: usize, count: usize, label: &str) -> String {
    let number = current.saturating_add(1).to_string();
    if label.is_empty() || label == number || current >= count { count.to_string() } else { format!("{number}/{count}") }
}

#[cfg(test)]
mod page_position_tests {
    use super::page_position;

    #[test]
    fn position_shows_only_when_the_label_is_not_the_page_number() {
        assert_eq!(page_position(0, 180, "i"), "1/180");
        assert_eq!(page_position(2, 180, "1"), "3/180");
        assert_eq!(page_position(2, 180, "3"), "180");
        assert_eq!(page_position(2, 180, ""), "180");
        assert_eq!(page_position(0, 0, "i"), "0");
        assert_eq!(page_position(usize::MAX, 3, "x"), "3");
    }
}
