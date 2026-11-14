//! The menu bar a system file panel gets, as AppKit sees it, on the main thread (`harness = false`):
//! ⌘X, ⌘C, ⌘V and ⌘A reach the panel's text boxes only through it, and the usual menu bar returns
//! when the panel closes, or the one the app installed meanwhile stays.

#[cfg(target_os = "macos")]
#[path = "../src/platform/panel_menu.rs"]
mod panel_menu;

/// ⌘X, ⌘C, ⌘V and ⌘A each have an item in the panel's menu bar that sends the matching edit action,
/// ⌘H hides the app, and there is no Quit item.
#[cfg(target_os = "macos")]
fn assert_panel_shortcuts(app: &objc2_app_kit::NSApplication, usual: &objc2_app_kit::NSMenu) {
    use objc2::sel;

    let bar = app.mainMenu().expect("the panel's menu bar");
    assert!(!std::ptr::eq(&*bar, usual), "the panel has its own menu bar");
    let mut items = Vec::new();
    for menu in (0..bar.numberOfItems()).filter_map(|i| bar.itemAtIndex(i)?.submenu()) {
        items.extend((0..menu.numberOfItems()).filter_map(|i| menu.itemAtIndex(i)));
    }
    for (key, action) in [("x", sel!(cut:)), ("c", sel!(copy:)), ("v", sel!(paste:)), ("a", sel!(selectAll:)), ("h", sel!(hide:))] {
        assert!(
            items.iter().any(|item| item.keyEquivalent().to_string() == key && item.action() == Some(action)),
            "no menu item sends {action:?} on ⌘{key}"
        );
    }
    assert!(!items.iter().any(|item| item.action() == Some(sel!(terminate:))), "no Quit while a panel is up");
}

/// Whether `menu` is the menu bar now.
#[cfg(target_os = "macos")]
fn is_menu_bar(app: &objc2_app_kit::NSApplication, menu: &objc2_app_kit::NSMenu) -> bool {
    app.mainMenu().is_some_and(|bar| std::ptr::eq(&*bar, menu))
}

/// Shows and hides the panel's menu bar against stand-ins for the usual one.
#[cfg(target_os = "macos")]
fn main() {
    // cargo-nextest asks a test binary for its tests (`--list`) before running them.
    if std::env::args().any(|arg| arg == "--list") {
        if !std::env::args().any(|arg| arg == "--ignored") {
            println!("panel_shortcuts: test");
        }
        return;
    }
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSMenu};

    let mtm = MainThreadMarker::new().expect("harness = false runs on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.finishLaunching();
    let usual = NSMenu::new(mtm);
    app.setMainMenu(Some(&usual));
    let mut menu = panel_menu::PanelMenu::default();
    let english = |s: &'static str| s.to_owned();

    menu.sync(true, &english);
    assert!(menu.wanted());
    assert_panel_shortcuts(&app, &usual);
    menu.sync(false, &english);

    // The labels go through the app's translations.
    menu.sync(true, &|s: &'static str| format!("«{s}»"));
    let bar = app.mainMenu().expect("the panel's menu bar");
    let edit = bar.itemAtIndex(1).and_then(|item| item.submenu()).expect("the Edit menu");
    assert_eq!(edit.title().to_string(), "«Edit»");
    let paste = (0..edit.numberOfItems()).filter_map(|i| edit.itemAtIndex(i)).find(|item| item.action() == Some(objc2::sel!(paste:)));
    assert_eq!(paste.map(|item| item.title().to_string()).as_deref(), Some("«Paste»"));
    menu.sync(false, &english);
    assert!(is_menu_bar(&app, &usual), "the usual menu bar is back");

    // craft-menu rebuilds its menu bar while the panel's is up: closing the panel leaves it there.
    menu.sync(true, &english);
    let rebuilt = NSMenu::new(mtm);
    app.setMainMenu(Some(&rebuilt));
    menu.sync(false, &english);
    assert!(is_menu_bar(&app, &rebuilt), "a menu bar installed meanwhile stays");

    // Rebuilt while the panel is still open: the panel's comes back, and the rebuilt one after it.
    menu.sync(true, &english);
    app.setMainMenu(Some(&usual));
    menu.sync(true, &english);
    assert_panel_shortcuts(&app, &usual);
    menu.sync(false, &english);
    assert!(is_menu_bar(&app, &usual), "the menu bar installed meanwhile is the one put back");
    println!("mac_panel_menu_appkit: ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
