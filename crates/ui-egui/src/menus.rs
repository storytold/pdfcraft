//! The menus: one definition ([`PdfCraftApp::menu_bar`]) for the native macOS menu bar and the
//! in-window bar on Windows, Linux and the web, laid out for each platform by craft-menu.
//!
//! Items run registry commands (`pdfcraft_engine::commands`) by id, with the registry's label and
//! shortcut, so a menu click and its shortcut run the same command. A few items have menu-only
//! ids: the zoom keys the document view handles itself, the side panel choices, the clipboard
//! keys, the recent files and the open documents. Each is a command of the app's catalog
//! (`commands::catalog`), fired once per click, whichever bar it came from.

use craft::keymap::{Key, Mods};
use craft::menu::{
    Chord, CommandRef, Document, Gate, Item, ItemRole, ItemState, Labeled, Layout, LayoutOptions, Menu, MenuBar, MenuRole, Node, Target,
};
use craft::ui::runtime::commands::{FireSource, fire, resolve};
use pdfcraft_engine::catalog::{Availability, TOOL_GROUPS};
use pdfcraft_engine::commands::{self, Shortcut};

use crate::{PdfCraftApp, RightPanel};

pub(super) mod tab_context;

/// The app's name in the menus the layouts write (About, Hide, Quit).
pub(crate) const APP_NAME: &str = "PdfCraft";

/// Menu-only commands. The registry's commands keep their own ids.
pub(crate) const CUT: &str = "edit.cut";
pub(crate) const PASTE: &str = "edit.paste";
pub(crate) const DELETE: &str = "edit.delete";
pub(crate) const SELECT_ALL: &str = "edit.select_all";
pub(crate) const QUIT: &str = "app.quit";
pub(crate) const NO_RECENT: &str = "file.no_recent";
/// An open document in the Window menu, `{"index": n}` in the tab strip.
pub(crate) const DOCUMENT: &str = "window.document";
/// A side panel, `{"panel": name}` ([`PANELS`]).
pub(crate) const PANEL: &str = "view.panel";
const ACTUAL_SIZE: &str = "view.zoom.actual_size";
const PAGE_LEVEL: &str = "view.zoom.page_level";
const FIT_WIDTH: &str = "view.zoom.fit_width";
const FIT_HEIGHT: &str = "view.zoom.fit_height";
const ZOOM_IN: &str = "view.zoom.in";
const ZOOM_OUT: &str = "view.zoom.out";
const ROTATE_CW: &str = "view.rotate.cw";
const ROTATE_CCW: &str = "view.rotate.ccw";

/// The menu-only items: id, English label and the shortcut shown (the keys stay with the view). The
/// zoom and rotate ids are registry commands; only Fit to height has no registry row.
pub(crate) const EXTRA: &[(&str, &str, Option<Shortcut>)] = &[
    (QUIT, "Quit", Some(Shortcut::cmd("Q"))),
    (CUT, "Cut", Some(Shortcut::cmd("X"))),
    (PASTE, "Paste", Some(Shortcut::cmd("V"))),
    (DELETE, "Delete", Some(Shortcut { command: false, shift: false, mac_ctrl: false, key: "Delete" })),
    (SELECT_ALL, "Select all", Some(crate::commands::SELECT_ALL)),
    (NO_RECENT, "No recent files", None),
    (FIT_HEIGHT, "Fit to height", None),
];

/// View ▸ Side panels: the panel, its id in [`PANEL`] commands and its English label.
pub(crate) const PANELS: &[(RightPanel, &str, &str)] = &[
    (RightPanel::Comments, "comments", "Comments"),
    (RightPanel::Bookmarks, "bookmarks", "Bookmarks"),
    (RightPanel::Pages, "pages", "Pages"),
    (RightPanel::Fields, "fields", "Fields"),
    (RightPanel::Layers, "layers", "Layers"),
    (RightPanel::Attachments, "attachments", "Attachments"),
    (RightPanel::Signatures, "signatures", "Signatures"),
    (RightPanel::Accessibility, "accessibility", "Accessibility Checker"),
    (RightPanel::Tags, "tags", "Tags"),
    (RightPanel::Search, "search", "Search"),
    (RightPanel::Compare, "compare", "Compare"),
];

/// The English titles of the menus and submenus, for the bar's mnemonics in languages whose
/// labels have no ASCII letter.
const TITLES: &[&str] = &[
    "File",
    "Edit",
    "View",
    "Pages",
    "Sign",
    "Window",
    "Help",
    "Open Recent",
    "Create",
    "Export",
    "Protect",
    "Zoom",
    "Rotate view",
    "Page navigation",
    "Page display",
    "Side panels",
    "Display theme",
    "Tools",
    "Fill & Sign",
];

/// Separator.
const SEP: &str = "---";

const FILE: &[&str] = &[
    "create.blank",
    "file.open",
    "file.open_recent",
    "file.pin_folder",
    SEP,
    "page.combine",
    SEP,
    "file.save",
    "file.save_as",
    "file.revert",
    SEP,
    "file.close",
    "file.close_all",
    SEP,
    "optimize.reduce",
    "optimize.advanced",
    SEP,
    "file.properties",
    "file.word_count",
    SEP,
    "print.dialog",
    QUIT,
];
const CREATE: &[&str] = &["create.file", "create.multiple", "create.multiple_separate", "create.images", "create.clipboard"];
const EXPORT: &[&str] = &["export.docx", "export.pptx", "export.rtf", "export.html", "export.text", SEP, "export.image", "export.all_images"];
const PROTECT: &[&str] = &["protect.password", "protect.remove", "protect.unlock", SEP, "protect.properties"];
const EDIT: &[&str] = &[
    "edit.undo",
    "edit.redo",
    SEP,
    CUT,
    "edit.copy",
    PASTE,
    DELETE,
    SEP,
    "comment.copy",
    "comment.paste",
    SEP,
    SELECT_ALL,
    SEP,
    "edit.find",
    "edit.replace",
    "edit.advanced_search",
    SEP,
    "edit.column_select",
    "edit.snapshot",
    SEP,
    "edit.attach",
    "form.clear",
    SEP,
    "app.preferences",
];
const ZOOM: &[&str] = &[ACTUAL_SIZE, PAGE_LEVEL, FIT_WIDTH, FIT_HEIGHT, "view.fit_visible", SEP, ZOOM_IN, ZOOM_OUT, SEP, "view.marquee_zoom"];
const NAVIGATION: &[&str] = &["view.focus_page_input", SEP, "view.previous_view", "view.next_view"];
const DISPLAY: &[&str] = &[
    "view.layout.single",
    "view.layout.two_up",
    SEP,
    "view.continuous",
    "view.page_gaps",
    "view.layout.cover",
    SEP,
    "view.fit_width_scrolling",
    "view.fit_one_page",
];
const THEMES: &[&str] = &["view.theme.system", "view.theme.light", "view.theme.dark"];
const PANES: &[&str] = &["view.side_panel", "view.tools_pane", "view.split"];
const PANE_COMMANDS: &[&str] = &["comment.list", "form.fields", "ui.dock.reset"];
const MODES: &[&str] = &["view.full_screen", "view.read_mode", crate::viewer_prefs::TOGGLE];
const PAGES: &[&str] = &[
    "page.organize",
    SEP,
    "page.insert_blank",
    "page.insert",
    "page.replace",
    "page.extract",
    "page.split",
    "page.duplicate",
    "page.delete",
    SEP,
    "page.rotate",
    "page.rotate_ccw",
    "page.rotate_dialog",
    "page.reverse",
    SEP,
    "page.crop",
    "page.boxes",
    "page.number",
    "page.properties",
    SEP,
    "bookmark.add",
    "bookmark.from_structure",
    "bookmark.from_text_styles",
];
/// Sign ▸ Fill & Sign, with the short labels of the Fill & Sign toolbar.
const FILL: &[(&str, &str)] = &[
    ("sign.fill.text", "Add text"),
    ("sign.fill.check", "Checkmark"),
    ("sign.fill.cross", "Cross"),
    ("sign.fill.dot", "Dot"),
    ("sign.fill.line", "Line"),
    ("sign.fill.date", "Date"),
    ("sign.fill.signature", "Signature"),
    ("sign.fill.initials", "Initials"),
];
const SIGN: &[&str] = &[
    SEP,
    "sign.fill.signature.change",
    "sign.fill.initials.change",
    "sign.fill.signature.remove",
    "sign.fill.initials.remove",
    SEP,
    "sign.digital",
    "sign.certify",
    "sign.certify_invisible",
    "form.add.signature",
    SEP,
    "sign.validate",
    "sign.panel",
];
const WINDOW: &[&str] = &["view.next_tab", "view.previous_tab"];
const HELP: &[&str] =
    &["help.shortcuts", SEP, "help.app_page", "help.github", "help.website", "help.discord", SEP, "help.check_updates", SEP, "help.about"];

/// A registry shortcut as a craft-keymap chord: `command` is ⌘ (Ctrl elsewhere) and `mac_ctrl`
/// the Control key on macOS. `None` for a key craft-keymap doesn't name.
pub(crate) fn chord(s: Shortcut) -> Option<Chord> {
    let key = Key::parse(s.key)?;
    Some(Chord::new(Mods { cmd: s.command, ctrl: s.mac_ctrl, alt: false, shift: s.shift }, key))
}

fn extra(id: &str) -> Option<&'static (&'static str, &'static str, Option<Shortcut>)> {
    EXTRA.iter().find(|(e, ..)| *e == id)
}

/// The English label of an item's command, for the bar's mnemonics.
fn english_label(command: &CommandRef) -> Option<String> {
    if let Some(spec) = commands::command(&command.id) {
        return Some(spec.label.to_owned());
    }
    if let Some((_, label, _)) = extra(&command.id) {
        return Some((*label).to_owned());
    }
    FILL.iter().find(|(id, _)| *id == command.id).map(|(_, label)| (*label).to_owned())
}

fn english(labeled: Labeled<'_>) -> Option<String> {
    match labeled {
        Labeled::Command(command) => english_label(command),
        Labeled::Title(shown) => TITLES.iter().find(|t| tl!(t) == shown).map(|t| (*t).to_owned()),
        // `Labeled` is non-exhaustive: a kind of label the bar adds later has no English name here.
        _ => None,
    }
}

fn translate(english: &'static str) -> String {
    tl!(english).to_owned()
}

/// The layouts' own labels (About, Quit, Window…) through the app's translations.
pub(crate) fn layout_options() -> LayoutOptions<'static> {
    LayoutOptions::new(APP_NAME).label(&translate)
}

/// Whether a dialog, a prompt or the command palette holds the window: the one gate over commands
/// and the menus.
pub(crate) fn gated(app: &PdfCraftApp) -> bool {
    app.modal_open() || app.palette_open
}

/// Whether a text field has the keyboard: the one signal behind the Edit rows' state and the native
/// menu's gate.
pub(crate) fn text_editing(ctx: &egui::Context) -> bool {
    ctx.egui_wants_keyboard_input()
}

impl PdfCraftApp {
    /// The app's menus for this frame, before the platform layout: every item's label, shortcut,
    /// enabled and checked state follow the item's command (`commands::catalog::command_input`). A
    /// modal dialog doesn't grey anything here; that is the command gate's job ([`gated`]).
    pub fn menu_bar(&self, ctx: &egui::Context) -> MenuBar {
        let mac = crate::commands::mac_shortcuts(ctx);
        let mut used: Vec<&str> = Vec::new();
        let list = |ids: &[&'static str], used: &mut Vec<&'static str>| -> Vec<Node> {
            ids.iter()
                .map(|&id| {
                    if id == SEP {
                        return Node::Separator;
                    }
                    used.push(id);
                    self.menu_item(id, None, mac)
                })
                .collect()
        };
        let file = {
            let mut nodes = Vec::new();
            for node in list(FILE, &mut used) {
                // Open Recent is the live recent list; Create, Export and Protect follow Combine,
                // Optimize and Close.
                let recent = matches!(&node, Node::Item(it) if it.command.id == "file.open_recent");
                if recent {
                    nodes.push(self.recent_menu(mac));
                    continue;
                }
                let after = match &node {
                    Node::Item(it) => it.command.id.clone(),
                    _ => String::new(),
                };
                nodes.push(node);
                match after.as_str() {
                    "page.combine" => nodes.push(Node::submenu(tl!("Create"), list(CREATE, &mut used))),
                    "optimize.advanced" => {
                        nodes.push(Node::submenu(tl!("Export"), list(EXPORT, &mut used)));
                        nodes.push(Node::Separator);
                        nodes.push(Node::submenu(tl!("Protect"), list(PROTECT, &mut used)));
                    }
                    _ => {}
                }
            }
            nodes
        };
        // Edit ▸ Pages follows Select All.
        let mut edit = Vec::new();
        for node in list(EDIT, &mut used) {
            let select_all = matches!(&node, Node::Item(it) if it.command.id == SELECT_ALL);
            edit.push(node);
            if select_all {
                edit.push(Node::submenu(tl!("Pages"), list(PAGES, &mut used)));
            }
        }
        let mut sign = vec![Node::submenu(
            tl!("Fill & Sign"),
            FILL.iter()
                .map(|&(id, label)| {
                    used.push(id);
                    self.menu_item(id, Some(tl!(label)), mac)
                })
                .collect(),
        )];
        sign.extend(list(SIGN, &mut used));
        let window = list(WINDOW, &mut used);
        let help = list(HELP, &mut used);
        let mut view = vec![
            Node::submenu(tl!("Zoom"), list(ZOOM, &mut used)),
            Node::submenu(tl!("Rotate view"), list(&[ROTATE_CW, ROTATE_CCW], &mut used)),
            Node::submenu(tl!("Page navigation"), list(NAVIGATION, &mut used)),
            Node::submenu(tl!("Page display"), list(DISPLAY, &mut used)),
            Node::Separator,
        ];
        view.extend(list(PANES, &mut used));
        view.push(Node::submenu(
            tl!("Side panels"),
            PANELS
                .iter()
                .map(|(_, name, label)| {
                    Item::new(CommandRef::with_params(PANEL, serde_json::json!({ "panel": name })), tl!(label)).radio(false).into()
                })
                .collect(),
        ));
        view.extend(list(PANE_COMMANDS, &mut used));
        view.push(Node::Separator);
        view.extend(list(MODES, &mut used));
        view.push(Node::submenu(tl!("Display theme"), list(THEMES, &mut used)));
        view.push(Node::Separator);
        view.push(Node::submenu(tl!("Tools"), self.tools_menu(&used, mac)));
        view.extend(list(&["view.palette"], &mut used));
        let mut bar = MenuBar::new(vec![
            Menu::new(tl!("File"), craft::menu::tidy(file)).role(MenuRole::File),
            Menu::new(tl!("Edit"), craft::menu::tidy(edit)).role(MenuRole::Edit),
            Menu::new(tl!("View"), craft::menu::tidy(view)).role(MenuRole::View),
            Menu::new(tl!("Sign"), craft::menu::tidy(sign)),
            Menu::new(tl!("Window"), window).role(MenuRole::Window),
            Menu::new(tl!("Help"), craft::menu::tidy(help)).role(MenuRole::Help),
        ])
        .documents(self.menu_documents());
        bar.tag_item(&CommandRef::new("help.about"), ItemRole::About);
        bar.tag_item(&CommandRef::new("app.preferences"), ItemRole::Settings);
        bar.tag_item(&CommandRef::new(QUIT), ItemRole::Quit);
        bar
    }

    /// One item: a registry command with its live label (Undo names the edit) and shortcut, or a
    /// menu-only one. Radio items get their kind here, their state from the command.
    fn menu_item(&self, id: &'static str, label: Option<&str>, mac: bool) -> Node {
        let (english, shortcut) = match commands::command(id) {
            Some(spec) => {
                let current = commands::current_label(spec, &self.session, self.active_ids().map(|(_, doc)| doc));
                (crate::i18n::menu_label(id, &current), spec.shortcut_for(mac).or((id == "edit.copy").then_some(crate::commands::COPY)))
            }
            None => match extra(id) {
                Some((_, label, shortcut)) => (tl!(label).to_owned(), *shortcut),
                None => (id.to_owned(), None),
            },
        };
        let mut item = Item::new(id, label.map_or(english, str::to_owned));
        if let Some(c) = shortcut.and_then(chord) {
            item = item.shortcut(c);
        }
        if matches!(id, "view.layout.single" | "view.layout.two_up") || THEMES.contains(&id) {
            item = item.radio(false);
        }
        item.into()
    }

    /// File ▸ Open Recent: the recent files, newest first, then Clear Recent Files.
    fn recent_menu(&self, mac: bool) -> Node {
        let mut items: Vec<Node> = self
            .recent
            .iter()
            .enumerate()
            .map(|(index, r)| Item::new(CommandRef::with_params("file.open_recent", serde_json::json!({ "index": index })), r.name.clone()).into())
            .collect();
        if items.is_empty() {
            items.push(self.menu_item(NO_RECENT, None, mac));
        } else {
            items.push(Node::Separator);
            items.push(self.menu_item("file.clear_recent", Some(tl!("Clear Recent Files")), mac));
        }
        let label = commands::command("file.open_recent").map_or("Open Recent", |c| c.label);
        Node::submenu(crate::i18n::menu_label("file.open_recent", label), items).enabled(!self.recent.is_empty())
    }

    /// View ▸ Tools: the All tools groups, each a submenu of its ready tools that no other menu
    /// lists, sections apart.
    fn tools_menu(&self, used: &[&str], mac: bool) -> Vec<Node> {
        TOOL_GROUPS
            .iter()
            .filter(|g| g.availability == Availability::Ready)
            .map(|group| {
                let mut nodes = Vec::new();
                let mut seen: Vec<&str> = Vec::new();
                for section in group.sections {
                    nodes.push(Node::Separator);
                    for tool in section.items.iter().filter(|t| t.availability == Availability::Ready) {
                        let actions: &[&'static str] = if tool.actions.is_empty() { std::slice::from_ref(&tool.command) } else { tool.actions };
                        for &id in actions {
                            if commands::command(id).is_none() || used.contains(&id) || seen.contains(&id) {
                                continue;
                            }
                            seen.push(id);
                            let label = if tool.actions.is_empty() { Some(tl!(tool.label)) } else { None };
                            nodes.push(self.menu_item(id, label, mac));
                        }
                    }
                }
                Node::submenu(tl!(group.label), nodes)
            })
            .collect()
    }

    /// The open documents, in tab order, for the Window menu.
    fn menu_documents(&self) -> Vec<Document> {
        (0..self.views.len())
            .filter_map(|index| {
                let name = crate::canvas::tab_label(self, index)?;
                Some(Document::new(CommandRef::with_params(DOCUMENT, serde_json::json!({ "index": index })), name).active(self.active == Some(index)))
            })
            .collect()
    }

    /// The native macOS bar's layout with the state AppKit shows: each item's enabled and checked
    /// state is its command's, read from the board. The one reader of command state for the native
    /// bar.
    pub(crate) fn native_bar(&self, ctx: &egui::Context) -> Layout {
        let mut layout = craft::menu::mac_layout(&self.menu_bar(ctx), &layout_options());
        craft::menu::apply_state(&mut layout.bar, |command| {
            let resolved = resolve(ctx, command.clone());
            ItemState::default().enabled(resolved.enabled).checked(resolved.checked)
        });
        layout
    }

    /// The menu bar drawn in the window (Windows, Linux, the web, and macOS when the native one
    /// couldn't be installed), where the top bar starts. A chosen command fires through the board.
    pub(crate) fn menu_bar_ui(&self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let bar = craft::menu::layout(&self.menu_bar(&ctx), Target::current(), &layout_options()).bar;
        let modal = gated(self);
        craft::menu::MenuBarUi::new(&bar).keyboard(!modal).icons(false).compact(true).english(&english).show(ui);
    }

    /// Install the native menu bar (macOS), once, from the eframe app creator. `false` when
    /// AppKit refused it: the in-window bar stays.
    #[cfg(target_os = "macos")]
    pub fn install_native_menu(&mut self, ctx: &egui::Context) -> bool {
        crate::i18n::set_current(crate::i18n::Lang::from_pref(&self.language));
        let bar = self.native_bar(ctx).bar;
        // The Window menu lists the open documents itself, so AppKit's window list stays off.
        self.native_menu = craft::menu::mac::install_with(ctx, &bar, craft::menu::mac::MacOptions::default().window_list(false));
        self.native_menu.is_some()
    }
}

/// The native menu's per-frame turn: its gate, its clicks fired as commands, then its bar brought in
/// step with the app, last, after this frame's changes.
pub(crate) fn frame(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if app.native_menu.is_none() {
        return;
    }
    let gate = Gate::default().modal(gated(app)).text_editing(text_editing(ctx));
    let clicks = app.native_menu.as_mut().map(|native| {
        native.set_gate(gate);
        native.take_clicks(ctx)
    });
    for command in clicks.unwrap_or_default() {
        fire(ctx, command, FireSource::Menu);
    }
    let layout = app.native_bar(ctx);
    if let Some(native) = app.native_menu.as_mut() {
        for clash in native.sync(layout) {
            log::info!("menu shortcut {:?} kept by {} over {} ({})", clash.shortcut, clash.command.id, clash.system, clash.resolution);
        }
    }
}

/// A command's `{"index": n}` parameter.
pub(crate) fn param_index(command: &CommandRef) -> Option<usize> {
    command.params.get("index").and_then(serde_json::Value::as_u64).and_then(|i| usize::try_from(i).ok())
}

/// The side panel a [`PANEL`] command names.
pub(crate) fn panel(command: &CommandRef) -> Option<RightPanel> {
    let name = command.params.get("panel").and_then(serde_json::Value::as_str)?;
    PANELS.iter().find(|(_, n, _)| *n == name).map(|(p, ..)| *p)
}

#[cfg(test)]
mod tests {
    use craft::menu::Builtin;

    use super::*;

    const PDF: &[u8] = include_bytes!("../tests/data/form.pdf");

    fn app(docs: &[&str]) -> (PdfCraftApp, egui::Context) {
        let mut app = PdfCraftApp::for_unit_tests();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        for name in docs {
            app.open_bytes(&egui::Context::default(), name, None, PDF.to_vec()).unwrap();
        }
        (app, egui::Context::default())
    }

    fn linux(app: &PdfCraftApp, ctx: &egui::Context) -> MenuBar {
        craft::menu::layout(&app.menu_bar(ctx), Target::Linux, &layout_options()).bar
    }

    fn item<'b>(bar: &'b MenuBar, command: &CommandRef) -> &'b Item {
        bar.find(command).unwrap_or_else(|| panic!("{} is in the menus", command.key()))
    }

    #[test]
    fn every_menu_command_is_listed_once_with_its_shortcut() {
        let (app, ctx) = app(&["a.pdf"]);
        let bar = linux(&app, &ctx);
        let items = bar.items();
        let mut keys: Vec<String> = items.iter().map(|it| it.command.key()).collect();
        keys.sort();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "no command is listed twice");
        for spec in commands::COMMANDS.iter().filter(|s| s.menu.is_some()) {
            if spec.id == "file.open_recent" {
                continue; // A submenu of the recent files.
            }
            let it = item(&bar, &CommandRef::new(spec.id));
            let shortcut = spec.shortcut_for(crate::commands::mac_shortcuts(&ctx)).or((spec.id == "edit.copy").then_some(crate::commands::COPY));
            assert_eq!(it.shortcut, shortcut.and_then(chord), "{} has its registry shortcut", spec.id);
        }
        // Every item runs a registry command or one of the menu's own.
        for it in items {
            let id = it.command.id.as_str();
            assert!(
                commands::command(id).is_some() || extra(id).is_some() || [PANEL, DOCUMENT].contains(&id) || Builtin::of(&it.command).is_some(),
                "{id} runs something"
            );
        }
    }

    #[test]
    fn recent_files_and_open_documents_are_listed_once_each() {
        let (mut app, ctx) = app(&["a.pdf", "b.pdf"]);
        app.recent = ["one.pdf", "two.pdf"].map(|n| crate::RecentFile { name: n.into(), path: format!("/nowhere/{n}"), pages: 1, size: 0 }).into();
        let bar = linux(&app, &ctx);
        for (i, name) in ["one.pdf", "two.pdf"].into_iter().enumerate() {
            assert_eq!(item(&bar, &CommandRef::with_params("file.open_recent", serde_json::json!({ "index": i }))).label, name);
        }
        let window = bar.menu(MenuRole::Window).unwrap();
        let documents: Vec<&str> = window
            .children
            .iter()
            .filter_map(|n| match n {
                Node::Item(it) if it.command.id == DOCUMENT => Some(it.label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(documents, ["a.pdf", "b.pdf"], "each open document once, in tab order");
        // The macOS layout lists them once too.
        let mac = craft::menu::mac_layout(&app.menu_bar(&ctx), &layout_options()).bar;
        let listed = mac.items().iter().filter(|it| it.command.id == DOCUMENT).count();
        assert_eq!(listed, 2);
        app.recent.clear();
        let bar = linux(&app, &ctx);
        assert!(bar.find(&CommandRef::new(NO_RECENT)).is_some(), "an empty list shows a placeholder");
        assert!(bar.find(&CommandRef::new("file.clear_recent")).is_none());
    }
}
