//! The macOS menu bar's items (src/menu_bar.rs), checked on every platform: what each shows,
//! when it is enabled or checked, and what choosing it does.

use pdfcraft_engine::commands;

use crate::menu_bar::{self, Action, Entry, Item, bar, items, sections};
use crate::theme::ThemePreference;
use crate::{Dialog, PdfCraftApp, RightPanel, i18n};

const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";

/// Open system pickers, the print dialog or the network: not for a unit test.
const SYSTEM: &[&str] = &[
    "file.open",
    "file.pin_folder",
    "create.file",
    "create.multiple",
    "create.multiple_separate",
    "create.images",
    "file.save_as",
    "page.insert",
    "page.replace",
    "print.dialog",
    "export.all_images",
    "help.check_updates",
    "file.open_recent",
];

fn with_document() -> PdfCraftApp {
    let mut app = PdfCraftApp::new();
    app.open_bytes("menu.pdf", None, PDF.to_vec()).expect("fixture opens");
    app
}

fn all_items(menus: &[menu_bar::Menu]) -> Vec<&Item> {
    menus.iter().flat_map(|m| items(&m.entries)).collect()
}

fn find<'a>(menus: &'a [menu_bar::Menu], action: &Action) -> &'a Item {
    all_items(menus).into_iter().find(|i| &i.action == action).unwrap_or_else(|| panic!("{action:?} is in the bar"))
}

#[test]
fn every_enabled_menu_command_does_something() {
    let menus = bar();
    let mut idle = Vec::new();
    for item in all_items(&menus) {
        let Action::Command(id) = item.action else { continue };
        if SYSTEM.contains(&id) {
            continue;
        }
        let mut app = with_document();
        let Some(spec) = commands::command(id) else {
            idle.push(format!("{id}: not registered"));
            continue;
        };
        if app.menu_item_state(item, false).enabled && app.command_enabled(spec) && !app.execute(id) {
            idle.push(format!("{id}: enabled, but nothing runs it"));
        }
    }
    assert!(idle.is_empty(), "menu items that do nothing:\n{}", idle.join("\n"));
}

#[test]
fn every_registry_menu_command_is_somewhere_in_the_bar() {
    let menus = bar();
    let shown: Vec<&str> = all_items(&menus)
        .into_iter()
        .filter_map(|i| match i.action {
            Action::Command(id) => Some(id),
            _ => None,
        })
        .collect();
    for name in ["File", "Edit", "View", "Pages", "Help"] {
        for spec in commands::menu(name) {
            // Open Recent is a submenu; Edit ▸ Copy is the clipboard's Copy.
            if !matches!(spec.id, "file.open_recent" | "edit.copy") {
                assert!(shown.contains(&spec.id), "{} ({name}) is missing", spec.id);
            }
        }
    }
}

#[test]
fn no_menu_shows_the_same_item_twice() {
    for m in bar() {
        let labels: Vec<&str> = items(&m.entries).iter().map(|i| i.label.as_str()).collect();
        for (i, l) in labels.iter().enumerate() {
            assert!(!labels.iter().skip(i + 1).any(|other| other == l), "{}: “{l}” twice", m.title);
        }
    }
}

#[test]
fn no_two_items_share_a_key_and_none_takes_a_system_key() {
    let menus = bar();
    let mut seen: Vec<(String, &str)> = Vec::new();
    for item in all_items(&menus) {
        let Some(s) = item.shortcut else { continue };
        let Some(code) = menu_bar::key_code(s) else { continue };
        let key = format!("{}{}{}{code}", s.command, s.shift, s.mac_ctrl);
        if let Some((_, other)) = seen.iter().find(|(k, _)| *k == key) {
            panic!("“{}” and “{other}” both take {s:?}", item.label);
        }
        // ⌘M minimizes, ⌘H hides, ⌘` cycles windows: AppKit's own.
        let system = s.command && !s.shift && !s.mac_ctrl && matches!(code, "KeyM" | "KeyH" | "Backquote");
        assert!(!system, "{} takes the system's {s:?}", item.label);
        seen.push((key, item.label.as_str()));
    }
}

#[test]
fn every_key_equivalent_replays_as_a_key_egui_knows() {
    let menus = bar();
    for item in all_items(&menus) {
        if let Some(Action::Key(s)) = item.key_action() {
            assert_eq!(menu_bar::shortcut_events(s).len(), 2, "{}: {s:?}", item.label);
        }
    }
}

#[test]
fn a_key_equivalent_is_the_key_registry_shortcuts_matches() {
    let save = commands::command("file.save").and_then(|c| c.shortcut).expect("Save has a shortcut");
    let events = menu_bar::shortcut_events(save);
    let ctx = egui::Context::default();
    let raw = egui::RawInput { events, ..Default::default() };
    let mut hit = false;
    let mut out = ctx.run_ui(raw, |ui| {
        hit = ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S)));
    });
    out.textures_delta.clear();
    assert!(hit);
}

#[test]
fn one_command_groups_share_a_section() {
    let ids = |s: &[Vec<&commands::CommandSpec>]| s.iter().map(|v| v.iter().map(|c| c.id).collect::<Vec<_>>()).collect::<Vec<_>>();
    let view = ids(&sections(commands::menu("View")));
    // No section is a lone command next to another lone command.
    for w in view.windows(2) {
        assert!(!(w[0].len() == 1 && w[1].len() == 1), "{w:?}");
    }
    // Nothing is dropped or reordered.
    let all: Vec<&str> = commands::menu("View").map(|c| c.id).collect();
    assert_eq!(view.concat(), all);
}

#[test]
fn under_a_dialog_only_text_editing_and_quit_stay_available() {
    let menus = bar();
    let mut app = with_document();
    app.dialog = Some(Dialog::Preferences);
    for item in all_items(&menus) {
        let available = matches!(item.action, Action::Input(_) | Action::Quit);
        assert_eq!(app.menu_item_state(item, false).enabled, available, "{}", item.label);
    }
    // A click from a menu opened before the dialog was does nothing underneath it.
    app.run_menu_action(Action::Command("file.close"));
    app.run_menu_action(Action::Panel(RightPanel::Bookmarks));
    assert_eq!(app.views.len(), 1, "the document stays open");
    assert_ne!(app.right, Some(RightPanel::Bookmarks));
}

#[test]
fn undo_and_redo_follow_a_focused_text_field() {
    let menus = bar();
    let app = with_document();
    let undo = find(&menus, &Action::Command("edit.undo"));
    // Nothing to undo in the document.
    assert!(!app.menu_item_state(undo, false).enabled);
    // While typing, ⌘Z is the field's own, so the item must be enabled for AppKit to pass it on.
    let typing = app.menu_item_state(undo, true);
    assert!(typing.enabled);
    assert_eq!(typing.label.as_deref(), Some(i18n::menu_label("edit.undo", "Undo").as_str()));
    assert_eq!(PdfCraftApp::menu_action_for_text(undo.action.clone(), true), Action::Input(menu_bar::InputKind::TextUndo));
}

#[test]
fn close_with_no_document_closes_the_window() {
    let menus = bar();
    let close = find(&menus, &Action::Command("file.close"));
    let state = PdfCraftApp::new().menu_item_state(close, false);
    assert!(state.enabled, "⌘W closes the empty window (#869)");
    assert_eq!(state.label.as_deref(), Some("Close Window"));
    assert!(with_document().menu_item_state(close, false).label.as_deref() != Some("Close Window"));
}

#[test]
fn check_marks_follow_the_app() {
    let menus = bar();
    let mut app = with_document();
    let checked = |app: &PdfCraftApp, a: Action| app.menu_item_state(find(&menus, &a), false).checked;
    app.theme_preference = ThemePreference::Dark;
    assert_eq!(checked(&app, Action::Command("view.theme.dark")), Some(true));
    assert_eq!(checked(&app, Action::Command("view.theme.light")), Some(false));
    let continuous = app.views[0].continuous();
    assert_eq!(checked(&app, Action::Command("view.continuous")), Some(continuous));
    app.execute("view.layout.two_up");
    assert_eq!(checked(&app, Action::Command("view.layout.two_up")), Some(true));
    assert_eq!(checked(&app, Action::Command("view.layout.single")), Some(false));
    app.run_menu_action(Action::Panel(RightPanel::Bookmarks));
    assert_eq!(checked(&app, Action::Panel(RightPanel::Bookmarks)), Some(true));
    assert_eq!(checked(&app, Action::Panel(RightPanel::Comments)), Some(false));
}

#[test]
fn a_clicked_command_waits_for_a_form_field_being_typed_in() {
    // As ⌘S does (#166): the field's text goes into the document before the save runs.
    let mut app = with_document();
    app.views[0].forms.focus = Some(crate::forms_ui::Focus {
        name: "name".into(),
        widget: 0,
        text: "typed".into(),
        picked: Vec::new(),
        request_focus: false,
        select_all: false,
        press: None,
        calendar: None,
        calendar_rect: None,
        calendar_text: None,
    });
    app.run_menu_action(Action::Command("file.save"));
    let doc = app.views[0].id;
    assert_eq!(app.deferred_commands, vec![("file.save", Some(doc))]);
}

#[test]
fn every_label_only_the_bar_uses_is_in_every_catalog() {
    let english = i18n::Lang::from_code("en");
    let labels = [
        "Services",
        "Hide PdfCraft",
        "Hide Others",
        "Show All",
        "Quit PdfCraft",
        "Window",
        "Minimize",
        "Zoom",
        "Enter Full Screen",
        "Bring All to Front",
        "Go",
        "First page",
        "Last page",
        "Close Window",
        "Cut",
        "Copy",
        "Paste",
        "Select All",
    ];
    let mut missing = Vec::new();
    for lang in i18n::Lang::all().filter(|l| Some(*l) != english) {
        for label in labels {
            if !i18n::has(lang, label) {
                missing.push(format!("{}: {label}", lang.code()));
            }
        }
    }
    assert!(missing.is_empty(), "untranslated menu bar labels:\n{}", missing.join("\n"));
}

#[test]
fn the_mac_bundle_declares_every_interface_language() {
    // Without it AppKit shows its own items (Window ▸ Minimize All…) in English.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/macos/Info.plist.in");
    let plist = std::fs::read_to_string(path).expect("Info.plist template");
    let declared = plist.split("<key>CFBundleLocalizations</key>").nth(1).and_then(|rest| rest.split("</array>").next()).unwrap_or_default();
    for lang in i18n::Lang::all() {
        // Apple writes the region and script with capitals: pt-BR, zh-Hans.
        let apple = match lang.code() {
            "pt-br" => "pt-BR",
            "zh-hans" => "zh-Hans",
            "zh-hant" => "zh-Hant",
            code => code,
        };
        assert!(declared.contains(&format!("<string>{apple}</string>")), "{apple} is missing from CFBundleLocalizations");
    }
}

#[test]
fn the_bar_has_the_mac_menus_in_order() {
    let titles: Vec<String> = bar().into_iter().map(|m| m.title).collect();
    assert_eq!(titles, ["PdfCraft", "File", "Edit", "View", "Go", "Pages", "Window", "Help"]);
    // Check for updates is in the app menu, where Mac users look for it (#80).
    let menus = bar();
    let app_menu = menus.first().expect("app menu");
    assert!(items(&app_menu.entries).iter().any(|i| i.action == Action::Command("help.check_updates")));
    assert!(app_menu.entries.iter().any(|e| matches!(e, Entry::System(menu_bar::System::Services, _))));
}

#[test]
fn menus_in_the_window_leave_the_app_and_window_menus_in_the_bar() {
    let titles: Vec<String> = menu_bar::system_only(bar()).into_iter().map(|m| m.title).collect();
    assert_eq!(titles, ["PdfCraft", "Window"]);
    // Quit stays up there, so ⌘Q still asks about unsaved changes (#1136).
    let menus = menu_bar::system_only(bar());
    let app_menu = menus.first().expect("app menu");
    assert!(items(&app_menu.entries).iter().any(|i| i.action == Action::Quit));
}

#[test]
fn the_menu_bar_preference_moves_the_menus_and_is_remembered() {
    let mut app = PdfCraftApp::new();
    assert!(!app.menus_in_system_bar(), "hosts without the bar keep the window's Menu");
    app.native_menu_bar = true;
    assert!(app.menus_in_system_bar(), "on by default where the bar exists");
    app.system_menu_bar = false;
    assert!(!app.menus_in_system_bar());

    let mut restored = PdfCraftApp::new();
    restored.restore(&app.persist());
    assert!(!restored.system_menu_bar);
}
