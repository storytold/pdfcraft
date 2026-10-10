//! The macOS menu bar (issue #80).
//!
//! A macOS application puts its menus in the menu bar at the top of the screen, not in a popup
//! inside the window. This module builds that bar from the same command registry the in-window
//! menu uses (`crate::commands`), so labels, shortcuts and enabled/disabled state stay in step,
//! and turns a picked item into the command it names.
//!
//! AppKit menu code needs Objective-C class declarations, i.e. `unsafe`, which this workspace
//! forbids (AGENTS.md §4). The `muda` crate wraps exactly that behind a safe API. Menus are
//! main-thread only, like the UI, and are dropped with the app.
//!
//! Two deliberate departures from the in-window menu, because this is a *system* menu:
//! - About and Settings… live in the application menu (⌘,) as macOS users expect, so they are
//!   left out of Help and Edit.
//! - Undo and Redo show no key equivalent: `⌘Z`/`⇧⌘Z` must reach a text field while one has the
//!   keyboard, which only egui can do (`registry_shortcuts`), so the accelerator stays with egui
//!   and the menu items run the command when clicked.

use std::cell::RefCell;
use std::collections::HashMap;

use muda::accelerator::Accelerator;
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use pdfcraft_engine::commands::{self, CommandSpec, Shortcut};

use crate::canvas::{Fit, PageLayout};
use crate::theme::ThemePreference;
use crate::{PdfCraftApp, RightPanel};

/// The application menu's title; AppKit replaces it with the bundle's name on screen.
const APP_NAME: &str = "PdfCraft";

/// Open Recent entries: one item per file, whose id carries the path.
const RECENT_PREFIX: &str = "menu.recent:";

/// View-local actions the document view owns (zoom, navigation, page display). They have no
/// registry entry, so they get ids of their own and [`run_view_action`] handles them.
const VIEW_ACTUAL_SIZE: &str = "menu.view.actual_size";
const VIEW_PAGE_LEVEL: &str = "menu.view.page_level";
const VIEW_FIT_WIDTH: &str = "menu.view.fit_width";
const VIEW_FIT_HEIGHT: &str = "menu.view.fit_height";
const VIEW_ZOOM_IN: &str = "menu.view.zoom_in";
const VIEW_ZOOM_OUT: &str = "menu.view.zoom_out";
const VIEW_ROTATE_CW: &str = "menu.view.rotate_cw";
const VIEW_ROTATE_CCW: &str = "menu.view.rotate_ccw";
const VIEW_PREV_VIEW: &str = "menu.view.prev";
const VIEW_NEXT_VIEW: &str = "menu.view.next";

/// The live menu bar. Item handles stay valid as the menu's state is updated each frame; dropping
/// the `_root` takes the whole bar down.
pub(crate) struct NativeMenu {
    _root: Menu,
    /// Registered commands and the view-local items, by menu id (label and enabled state).
    items: HashMap<&'static str, MenuItem>,
    /// View ▸ Display theme radios.
    themes: Vec<(ThemePreference, CheckMenuItem)>,
    /// View ▸ Page display radios.
    layouts: Vec<(PageLayout, CheckMenuItem)>,
    /// View ▸ Page display ▸ Show cover page.
    cover: CheckMenuItem,
    /// View ▸ Side panels.
    panels: Vec<(RightPanel, CheckMenuItem)>,
    /// File ▸ Open Recent, rebuilt when the list changes.
    recent: Submenu,
    recent_signature: RefCell<Vec<String>>,
}

/// The View menu's hand-built parts, gathered while `install` builds the rest.
struct ViewParts {
    menu: Submenu,
    themes: Vec<(ThemePreference, CheckMenuItem)>,
    layouts: Vec<(PageLayout, CheckMenuItem)>,
    cover: CheckMenuItem,
    panels: Vec<(RightPanel, CheckMenuItem)>,
}

impl NativeMenu {
    /// Build the menu bar and put it in place (`NSApplication.mainMenu`). Call on the main thread,
    /// after the app is created, once.
    pub(crate) fn install() -> Self {
        let root = Menu::new();
        let mut items: HashMap<&'static str, MenuItem> = HashMap::new();

        // Application menu: About, Settings…, Services, Hide/Show, Quit (the standard shape).
        let app_menu = Submenu::new(APP_NAME, true);
        let about = MenuItem::with_id("help.about", crate::i18n::t("About PdfCraft"), true, None);
        append(&app_menu, &about);
        items.insert("help.about", about);
        append(&app_menu, &PredefinedMenuItem::separator());
        let settings = MenuItem::with_id("app.preferences", crate::i18n::t("Settings…"), true, accelerator(Some(Shortcut::cmd(","))));
        append(&app_menu, &settings);
        items.insert("app.preferences", settings);
        append(&app_menu, &PredefinedMenuItem::separator());
        append(&app_menu, &PredefinedMenuItem::services(None));
        append(&app_menu, &PredefinedMenuItem::separator());
        append(&app_menu, &PredefinedMenuItem::hide(None));
        append(&app_menu, &PredefinedMenuItem::hide_others(None));
        append(&app_menu, &PredefinedMenuItem::show_all(None));
        append(&app_menu, &PredefinedMenuItem::separator());
        append(&app_menu, &PredefinedMenuItem::quit(None));

        // File: the registry's items, with Open Recent as a live submenu in its place.
        let recent = Submenu::new(crate::i18n::t("Open Recent"), true);
        let file = Submenu::new(crate::i18n::t("File"), true);
        for spec in commands::menu("File") {
            if spec.id == "file.open_recent" {
                append(&file, &recent);
            } else {
                add_command(&file, spec, &mut items);
            }
        }

        // Edit: everything but Preferences, which is in the application menu.
        let edit = Submenu::new(crate::i18n::t("Edit"), true);
        for spec in commands::menu("Edit") {
            if spec.id != "app.preferences" {
                add_command(&edit, spec, &mut items);
            }
        }

        let view = build_view(&mut items);

        // Pages: straight from the registry.
        let pages = Submenu::new(crate::i18n::t("Pages"), true);
        for spec in commands::menu("Pages") {
            add_command(&pages, spec, &mut items);
        }

        // Window: the standard items; AppKit adds the list of open windows below them.
        let window = Submenu::new(crate::i18n::t("Window"), true);
        append(&window, &PredefinedMenuItem::minimize(None));
        append(&window, &PredefinedMenuItem::zoom(None));
        append(&window, &PredefinedMenuItem::separator());
        append(&window, &PredefinedMenuItem::bring_all_to_front(None));

        // Help: the registry's items, minus About (in the application menu).
        let help = Submenu::new(crate::i18n::t("Help"), true);
        for spec in commands::menu("Help") {
            if spec.id != "help.about" {
                add_command(&help, spec, &mut items);
            }
        }

        let menus: [&dyn muda::IsMenuItem; 7] = [&app_menu, &file, &edit, &view.menu, &pages, &window, &help];
        if let Err(e) = root.append_items(&menus) {
            log::warn!("menu bar: {e}");
        }
        root.init_for_nsapp();
        // Let AppKit treat these as the Window and Help menus (window list; the Help search box).
        window.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();

        Self {
            _root: root,
            items,
            themes: view.themes,
            layouts: view.layouts,
            cover: view.cover,
            panels: view.panels,
            recent,
            recent_signature: RefCell::new(Vec::new()),
        }
    }

    /// Bring the menu's labels, enabled state and checkmarks in step with the app.
    fn sync(&self, app: &PdfCraftApp) {
        let active = app.active_ids();
        let active_id = active.map(|(_, id)| id);
        for spec in commands::COMMANDS {
            let Some(item) = self.items.get(spec.id) else { continue };
            let label = commands::current_label(spec, &app.session, active_id);
            item.set_text(crate::i18n::menu_label(spec.id, &label));
            item.set_enabled(app.command_enabled(spec));
        }

        let view = app.active.and_then(|i| app.views.get(i));
        let has_doc = view.is_some();
        // View-local items: the zoom and rotate actions need a document; the history items need
        // somewhere to go back or forward to.
        for id in [VIEW_ACTUAL_SIZE, VIEW_PAGE_LEVEL, VIEW_FIT_WIDTH, VIEW_FIT_HEIGHT, VIEW_ZOOM_IN, VIEW_ZOOM_OUT, VIEW_ROTATE_CW, VIEW_ROTATE_CCW] {
            if let Some(item) = self.items.get(id) {
                item.set_enabled(has_doc);
            }
        }
        if let Some(item) = self.items.get(VIEW_PREV_VIEW) {
            item.set_enabled(view.is_some_and(|v| !v.back.is_empty()));
        }
        if let Some(item) = self.items.get(VIEW_NEXT_VIEW) {
            item.set_enabled(view.is_some_and(|v| !v.forward.is_empty()));
        }

        for (layout, item) in &self.layouts {
            item.set_enabled(has_doc);
            item.set_checked(view.is_some_and(|v| v.layout == *layout));
        }
        self.cover.set_enabled(view.is_some_and(crate::DocView::cover_applies));
        self.cover.set_checked(view.is_some_and(|v| v.cover));

        for (preference, item) in &self.themes {
            item.set_checked(app.theme_preference == *preference);
        }
        for (panel, item) in &self.panels {
            item.set_checked(app.right == Some(*panel));
        }

        self.sync_recent(app);
    }

    /// Rebuild File ▸ Open Recent when the list changed. Empty means the submenu is disabled.
    fn sync_recent(&self, app: &PdfCraftApp) {
        let signature: Vec<String> = app.recent.iter().map(|r| format!("{}\u{1}{}", r.name, r.path)).collect();
        if *self.recent_signature.borrow() == signature {
            return;
        }
        *self.recent_signature.borrow_mut() = signature;
        while self.recent.remove_at(0).is_some() {}
        self.recent.set_enabled(!app.recent.is_empty());
        for r in &app.recent {
            let item = MenuItem::with_id(format!("{RECENT_PREFIX}{}", r.path), &r.name, true, None);
            append(&self.recent, &item);
        }
        if !app.recent.is_empty() {
            append(&self.recent, &PredefinedMenuItem::separator());
            let clear = MenuItem::with_id("file.clear_recent", crate::i18n::t("Clear Recent Files"), true, None);
            append(&self.recent, &clear);
        }
    }
}

/// Build the View menu: the document view's zoom and navigation, the page display and the panels,
/// around the registry's own View commands (bits of which appear at the top, as in the window).
fn build_view(items: &mut HashMap<&'static str, MenuItem>) -> ViewParts {
    let menu = Submenu::new(crate::i18n::t("View"), true);

    // Zoom (the document view's own keys, written for macOS by `Shortcut`).
    for (id, label, shortcut) in [
        (VIEW_ACTUAL_SIZE, "Actual size", Some(crate::commands::ACTUAL_SIZE)),
        (VIEW_PAGE_LEVEL, "Zoom to page level", Some(crate::commands::PAGE_LEVEL)),
        (VIEW_FIT_WIDTH, "Fit to width", Some(crate::commands::FIT_WIDTH)),
        (VIEW_FIT_HEIGHT, "Fit to height", None),
        (VIEW_ZOOM_IN, "Zoom in", Some(crate::commands::ZOOM_IN)),
        (VIEW_ZOOM_OUT, "Zoom out", Some(crate::commands::ZOOM_OUT)),
        (VIEW_ROTATE_CW, "Rotate view clockwise", Some(crate::commands::ROTATE_CW)),
        (VIEW_ROTATE_CCW, "Rotate view counterclockwise", Some(crate::commands::ROTATE_CCW)),
    ] {
        let item = MenuItem::with_id(id, crate::i18n::t(label), true, accelerator(shortcut));
        append(&menu, &item);
        items.insert(id, item);
    }
    // Fit visible is a registered command (⌘3) shown among the zoom items.
    if let Some(spec) = commands::command("view.fit_visible") {
        add_command(&menu, spec, items);
    }

    // Page navigation.
    append(&menu, &PredefinedMenuItem::separator());
    for (id, label, shortcut) in
        [(VIEW_PREV_VIEW, "Previous view", Some(crate::commands::PREV_VIEW)), (VIEW_NEXT_VIEW, "Next view", Some(crate::commands::NEXT_VIEW))]
    {
        let item = MenuItem::with_id(id, crate::i18n::t(label), true, accelerator(shortcut));
        append(&menu, &item);
        items.insert(id, item);
    }

    // Page display: the layouts and the cover page, as radios.
    append(&menu, &PredefinedMenuItem::separator());
    let mut layouts = Vec::new();
    for layout in PageLayout::ORDER {
        let item = CheckMenuItem::with_id(layout.command(), crate::i18n::t(layout.label()), true, false, None);
        append(&menu, &item);
        layouts.push((layout, item));
    }
    let cover = CheckMenuItem::with_id("view.layout.cover", crate::i18n::t("Show cover page in two-page view"), true, false, None);
    append(&menu, &cover);

    // The registry's own View commands.
    append(&menu, &PredefinedMenuItem::separator());
    for spec in commands::menu("View") {
        add_command(&menu, spec, items);
    }

    // Display theme.
    let theme = Submenu::new(crate::i18n::t("Display theme"), true);
    let mut themes = Vec::new();
    for (preference, command, label) in [
        (ThemePreference::System, "view.theme.system", "Use system setting"),
        (ThemePreference::Light, "view.theme.light", "Light gray"),
        (ThemePreference::Dark, "view.theme.dark", "Dark gray"),
    ] {
        let item = CheckMenuItem::with_id(command, crate::i18n::t(label), true, false, None);
        append(&theme, &item);
        themes.push((preference, item));
    }
    append(&menu, &theme);

    // Side panels.
    let side = Submenu::new(crate::i18n::t("Side panels"), true);
    let mut panels = Vec::new();
    for (panel, label) in [
        (RightPanel::Comments, "Comments"),
        (RightPanel::Bookmarks, "Bookmarks"),
        (RightPanel::Pages, "Pages"),
        (RightPanel::Fields, "Fields"),
        (RightPanel::Layers, "Layers"),
        (RightPanel::Attachments, "Attachments"),
        (RightPanel::Signatures, "Signatures"),
        (RightPanel::Accessibility, "Accessibility Checker"),
        (RightPanel::Search, "Search"),
        (RightPanel::Compare, "Compare"),
    ] {
        let item = CheckMenuItem::with_id(format!("menu.panel.{}", panel_id(panel)), crate::i18n::t(label), true, false, None);
        append(&side, &item);
        panels.push((panel, item));
    }
    append(&menu, &side);

    ViewParts { menu, themes, layouts, cover, panels }
}

/// Drain the menu events that arrived since the last frame, run what they name, then keep the
/// menu in step. Called from the update loop while the native menu is installed.
pub(crate) fn poll(app: &mut PdfCraftApp) {
    let mut picked: Vec<String> = Vec::new();
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        picked.push(event.id.0.clone());
    }
    if !picked.is_empty() {
        let active = app.active_ids();
        let form_typing = active.and_then(|(i, _)| app.views.get(i)).is_some_and(|v| v.forms.focus.is_some());
        for id in picked {
            match commands::command(&id) {
                // A registered command. While a form field has the keyboard, let it take the
                // frame's typing first so ⌘S saves what is on screen (#166), exactly as
                // `registry_shortcuts` does.
                Some(spec) if form_typing => {
                    app.deferred_commands.push((spec.id, active.map(|(_, doc)| doc)));
                    if let Some(ctx) = &app.ctx {
                        ctx.request_repaint();
                    }
                }
                Some(spec) => {
                    app.execute(spec.id);
                }
                // A view-local action, an Open Recent entry or a side panel.
                None => {
                    run_view_action(app, &id);
                }
            }
        }
    }
    if let Some(menu) = &app.native_menu {
        menu.sync(app);
    }
}

/// Run a menu id that is not a registered command: an Open Recent entry, a side panel, or one of
/// the document view's own actions. Returns whether it was handled.
fn run_view_action(app: &mut PdfCraftApp, id: &str) -> bool {
    if let Some(path) = id.strip_prefix(RECENT_PREFIX) {
        app.open_recent(path);
        return true;
    }
    if let Some(name) = id.strip_prefix("menu.panel.") {
        if let Some(panel) = panel_from_id(name) {
            app.choose_right_panel(Some(panel));
            return true;
        }
        return false;
    }
    let Some(i) = app.active else { return false };
    let Some(view) = app.views.get_mut(i) else { return false };
    match id {
        VIEW_ACTUAL_SIZE => view.set_zoom(1.0),
        VIEW_PAGE_LEVEL => view.fit = Fit::Page,
        VIEW_FIT_WIDTH => view.fit = Fit::Width,
        VIEW_FIT_HEIGHT => {
            view.fit = Fit::Height;
            let current = view.current;
            view.goto = Some((current, 0.0));
        }
        VIEW_ZOOM_IN => view.zoom_step(true),
        VIEW_ZOOM_OUT => view.zoom_step(false),
        VIEW_ROTATE_CW => view.rotate_view(true),
        VIEW_ROTATE_CCW => view.rotate_view(false),
        VIEW_PREV_VIEW => {
            view.view_history(false);
        }
        VIEW_NEXT_VIEW => {
            view.view_history(true);
        }
        _ => return false,
    }
    if let Some(ctx) = &app.ctx {
        ctx.request_repaint();
    }
    true
}

/// Add a registered command's item to `parent`, with its live label and (for text-safe commands)
/// its macOS key equivalent. The handle is kept for the per-frame state sync.
fn add_command(parent: &Submenu, spec: &'static CommandSpec, items: &mut HashMap<&'static str, MenuItem>) {
    // Undo and Redo keep their accelerators to egui, which knows whether a text field has the
    // keyboard (see `registry_shortcuts`); a native key equivalent would take ⌘Z from it.
    let accel = if spec.in_text { accelerator(spec.shortcut) } else { None };
    let item = MenuItem::with_id(spec.id, crate::i18n::menu_label(spec.id, spec.label), true, accel);
    append(parent, &item);
    items.insert(spec.id, item);
}

/// Append `item`, reporting (not crashing on) a failure to build the menu.
fn append(parent: &Submenu, item: &dyn muda::IsMenuItem) {
    if let Err(e) = parent.append(item) {
        log::warn!("menu: {e}");
    }
}

/// How `shortcut` is written as a macOS key equivalent, or `None` when AppKit has no equivalent
/// for the key (`+`, `−`, arrows: the document view keeps handling those keys).
fn accelerator(shortcut: Option<Shortcut>) -> Option<Accelerator> {
    let shortcut = shortcut?;
    let mut parts: Vec<&str> = Vec::new();
    if shortcut.command {
        parts.push("Cmd");
    }
    if shortcut.mac_ctrl {
        parts.push("Ctrl");
    }
    if shortcut.shift {
        parts.push("Shift");
    }
    parts.push(shortcut.key);
    let text = parts.join("+");
    match text.parse::<Accelerator>() {
        Ok(accelerator) => Some(accelerator),
        Err(e) => {
            log::debug!("menu accelerator for {text:?}: {e}");
            None
        }
    }
}

/// A stable id fragment for a right-hand panel.
fn panel_id(panel: RightPanel) -> &'static str {
    match panel {
        RightPanel::Comments => "comments",
        RightPanel::Bookmarks => "bookmarks",
        RightPanel::Pages => "pages",
        RightPanel::Fields => "fields",
        RightPanel::Layers => "layers",
        RightPanel::Attachments => "attachments",
        RightPanel::Signatures => "signatures",
        RightPanel::Accessibility => "accessibility",
        RightPanel::Search => "search",
        RightPanel::Compare => "compare",
    }
}

/// The panel a [`panel_id`] fragment stands for.
fn panel_from_id(id: &str) -> Option<RightPanel> {
    match id {
        "comments" => Some(RightPanel::Comments),
        "bookmarks" => Some(RightPanel::Bookmarks),
        "pages" => Some(RightPanel::Pages),
        "fields" => Some(RightPanel::Fields),
        "layers" => Some(RightPanel::Layers),
        "attachments" => Some(RightPanel::Attachments),
        "signatures" => Some(RightPanel::Signatures),
        "accessibility" => Some(RightPanel::Accessibility),
        "search" => Some(RightPanel::Search),
        "compare" => Some(RightPanel::Compare),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every panel round-trips through its menu id.
    #[test]
    fn panels_round_trip() {
        for panel in [
            RightPanel::Comments,
            RightPanel::Bookmarks,
            RightPanel::Pages,
            RightPanel::Fields,
            RightPanel::Layers,
            RightPanel::Attachments,
            RightPanel::Signatures,
            RightPanel::Accessibility,
            RightPanel::Search,
            RightPanel::Compare,
        ] {
            assert_eq!(panel_from_id(panel_id(panel)), Some(panel));
            assert!(panel_from_id(panel_id(panel)).is_some());
        }
        assert_eq!(panel_from_id("nope"), None);
    }

    /// Letters, digits and punctuation the registry uses become key equivalents.
    #[test]
    fn registry_shortcuts_become_accelerators() {
        assert!(accelerator(Some(Shortcut::cmd("S"))).is_some());
        assert!(accelerator(Some(Shortcut::cmd_shift("S"))).is_some());
        assert!(accelerator(Some(Shortcut::cmd(","))).is_some());
    }

    /// Keys the document view owns (`+`, `−`, arrows) have no AppKit equivalent; the item is
    /// still shown, and the view keeps handling those keys.
    #[test]
    fn view_local_keys_have_no_accelerator() {
        assert!(accelerator(Some(Shortcut::cmd("+"))).is_none());
        assert!(accelerator(Some(Shortcut::cmd("−"))).is_none());
        assert!(accelerator(Some(Shortcut::cmd("←"))).is_none());
    }

    /// Commands without a shortcut and Undo/Redo (whose ⌘Z stays with text fields) show none.
    #[test]
    fn no_shortcut_means_no_accelerator() {
        assert!(accelerator(None).is_none());
    }
}
