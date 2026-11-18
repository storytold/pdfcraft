//! Window composition: the shell's title bar, mode bar, view rail, dock and page area.

use craft::behavior::IconId;
use craft::keymap::CommandRef;
use craft::menu::ThemeChoice;
use craft::theme::ModeChoice;
use craft::ui::buttons::{Button, IconButton};
use craft::ui::contract::{Bind, Outcome as _, Props as _, Show as _, ShowContainer as _};
use craft::ui::menu::{Menu, MenuItem};
use craft::ui::runtime::env::WindowKey;
use craft::ui::shell::{AppShell, Slot, Title, TitleBar};

use crate::panes::Place;
use crate::{PdfCraftApp, side_panels};

mod mode_bar;
pub(crate) mod doc_tabs;
pub(crate) mod view_rail;

/// Compose the window: title bar, mode bar, view rail and the dock, whose document panel holds the
/// pages.
///
/// The shell carries out every screen-mode change; the mode it ends the pass with goes back to the
/// app, which does the presentation work in one place.
pub(crate) fn shell(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let hidden = app.hidden_interface();
    let modes_visible = !hidden.mode_bar(app.native_menu.is_some());
    let window_ui = !hidden.window_ui;
    let out = AppShell::new(WindowKey::main())
        .screen_mode(Bind::given(app.screen_mode))
        // A presentation shows the page and nothing else, so no reveal brings the bars back.
        .allow_reveal(false)
        .title_bar(|ui, _, app: &mut PdfCraftApp| title_bar(app, ui))
        .top(Slot::new("modes").visible(modes_visible), |ui, _, app: &mut PdfCraftApp| mode_bar::show(app, ui))
        .end(Slot::new("view-rail").visible(window_ui), |ui, _, app: &mut PdfCraftApp| view_rail::show(app, ui))
        .body(|ui, _, app: &mut PdfCraftApp| side_panels::dock(app, ui))
        .show(ui, app);
    // PLAN: assumed `set_screen_mode(&mut self, ScreenMode)` (pdf-shell-w8)
    app.set_screen_mode(out.screen_mode);
}

/// The title bar: Home, the document tabs and Open, then the community, shortcuts, theme and
/// Preferences buttons. The window's caption buttons are the bar's own.
fn title_bar(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let title = match app.active_ids().and_then(|(_, id)| app.session.get(id)) {
        Some(doc) if doc.dirty => Title::unsaved(doc.name.clone()),
        Some(doc) => Title::new(doc.name.clone()),
        None => Title::new("PdfCraft"),
    };
    let theme_icon = match app.theme_preference {
        ModeChoice::System => "monitor",
        ModeChoice::Light => "sun",
        ModeChoice::Dark => "moon",
    };
    let out = TitleBar::new().show_titled(
        ui,
        title,
        |ui, _| {
            let home = app.active.is_none() && !app.combine_showing();
            if IconButton::new(IconId::new("house")).label(tl!("Home")).selected(home).show(ui).clicked() {
                app.active = None;
                app.combine_tab.focused = false;
            }
            doc_tabs::strip(app, ui, Place::Main);
            Button::command("file.open").icon(IconId::new("plus")).show(ui);
        },
        |ui, _| {
            IconButton::command(IconId::new("messages-square"), "help.discord").show(ui);
            IconButton::command(IconId::new("circle-help"), "help.shortcuts").show(ui);
            let theme_button = IconButton::command(IconId::new(theme_icon), ThemeChoice::NextMode.command()).show(ui);
            let opener = theme_button.response().clone();
            Menu::context(tl!("Appearance"), &opener).show(ui, |ui, _| {
                let appearance = CommandRef::with_params("app.preferences", serde_json::json!({ "tab": "appearance" }));
                MenuItem::command(appearance).label(tl!("Appearance settings…")).show(ui);
            });
            IconButton::command(IconId::new("settings"), "app.preferences").show(ui);
        },
    );
    if out.close_requested {
        app.request_quit(&ctx);
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
