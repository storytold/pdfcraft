//! macOS: Cut, Copy, Paste and Select All shortcuts in the system file panels.
//!
//! AppKit finds these shortcuts through the menu bar, and PdfCraft's menu bar hands them to the
//! app's own key handling, so the text boxes of an Open or Save panel (Go to Folder, the file name)
//! ignored ⌘V. While a panel is showing, this small menu bar replaces the usual one. It goes away
//! with the panel because the app's own text handling gets these keys from egui. Its application
//! menu has Hide, Hide Others and Show All; Quit, which asks about unsaved changes, comes back with
//! the usual menu bar.

use muda::{Menu, PredefinedMenuItem, Submenu};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSMenu};

/// The panel's menu bar as AppKit holds it, and the one to put back. The `Menu` owns its items.
struct Shown {
    _menu: Menu,
    ours: Option<Retained<NSMenu>>,
    before: Option<Retained<NSMenu>>,
}

/// Swaps the menu bar while a file panel is showing.
#[derive(Default)]
pub struct PanelMenu {
    shown: Option<Shown>,
    wanted: bool,
}

impl std::fmt::Debug for PanelMenu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PanelMenu").field("shown", &self.shown.is_some()).field("wanted", &self.wanted).finish()
    }
}

impl PanelMenu {
    /// Show the panel's menu bar when `panel_open`, else put the usual one back. A menu bar the
    /// app installed since ours (craft-menu rebuilds it when Open Recent or the language changes)
    /// stays: it becomes the one to put back, or is left alone.
    pub fn sync(&mut self, panel_open: bool) {
        self.wanted = panel_open;
        let Some(mtm) = MainThreadMarker::new() else { return };
        let app = NSApplication::sharedApplication(mtm);
        let current = app.mainMenu();
        let ours = |shown: &Shown| same(&current, &shown.ours);
        match (panel_open, self.shown.take()) {
            (true, Some(shown)) if ours(&shown) => self.shown = Some(shown),
            (true, _) => {
                if let Some(menu) = edit_menu() {
                    menu.init_for_nsapp();
                    self.shown = Some(Shown { _menu: menu, ours: app.mainMenu(), before: current });
                }
            }
            (false, Some(shown)) if ours(&shown) => app.setMainMenu(shown.before.as_deref()),
            (false, _) => {}
        }
    }

    /// Whether the last [`Self::sync`] asked for the panel's menu bar.
    #[cfg(test)]
    pub fn wanted(&self) -> bool {
        self.wanted
    }
}

/// Whether two menu bars are the same AppKit object.
fn same(a: &Option<Retained<NSMenu>>, b: &Option<Retained<NSMenu>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::ptr::eq(&**a, &**b),
        _ => false,
    }
}

/// AppKit treats a menu bar's first menu as the application menu, so Edit comes second.
fn edit_menu() -> Option<Menu> {
    let menu = Menu::new();
    let app = Submenu::new("PdfCraft", true);
    app.append_items(&[&PredefinedMenuItem::hide(None), &PredefinedMenuItem::hide_others(None), &PredefinedMenuItem::show_all(None)]).ok()?;
    let edit = Submenu::new("Edit", true);
    edit.append_items(&[
        &PredefinedMenuItem::cut(None),
        &PredefinedMenuItem::copy(None),
        &PredefinedMenuItem::paste(None),
        &PredefinedMenuItem::select_all(None),
    ])
    .ok()?;
    menu.append_items(&[&app, &edit]).ok()?;
    Some(menu)
}
