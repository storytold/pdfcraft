//! The menus: one definition ([`PdfCraftApp::menu_bar`]) for the native macOS menu bar and the
//! in-window bar on Windows, Linux and the web, laid out for each platform by craft-menu.
//!
//! Items run registry commands (`pdfcraft_engine::commands`) by id, with the registry's label and
//! shortcut, so a menu click and its shortcut run the same command. A few items have menu-only
//! ids: the zoom keys the document view handles itself, the side panel choices, the clipboard
//! keys, the recent files and the open documents. [`PdfCraftApp::run_menu_command`] runs every
//! one of them, once per click, whichever bar it came from.

use craft_keymap::{Key, Mods};
use craft_menu::{
    Builtin, Chord, CommandRef, Document, Gate, Item, ItemRole, ItemState, Labeled, LayoutOptions, Menu, MenuBar, MenuRole, Node, Target,
};
use pdfcraft_engine::catalog::{Availability, TOOL_GROUPS};
use pdfcraft_engine::commands::{self, Shortcut};

use crate::canvas::{Fit, PageLayout};
use crate::theme::ThemePreference;
use crate::{PdfCraftApp, RightPanel};

/// The app's name in the menus the layouts write (About, Hide, Quit).
pub(crate) const APP_NAME: &str = "PdfCraft";

/// The top bar's width after the menus: the mode tabs, the search box and the buttons.
const ROW_AFTER_MENUS: f32 = 720.0;
/// The narrowest the menu bar gets: one "Menu" title.
const MIN_MENUS_WIDTH: f32 = 80.0;

/// Menu-only commands. The registry's commands keep their own ids.
pub(crate) const CUT: &str = "edit.cut";
pub(crate) const PASTE: &str = "edit.paste";
pub(crate) const DELETE: &str = "edit.delete";
pub(crate) const SELECT_ALL: &str = "edit.select_all";
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

/// The menu-only items: id, English label and the shortcut shown (the keys stay with the view).
const EXTRA: &[(&str, &str, Option<Shortcut>)] = &[
    (CUT, "Cut", Some(Shortcut::cmd("X"))),
    (PASTE, "Paste", Some(Shortcut::cmd("V"))),
    (DELETE, "Delete", Some(Shortcut { command: false, shift: false, mac_ctrl: false, key: "Delete" })),
    (SELECT_ALL, "Select all", Some(crate::commands::SELECT_ALL)),
    (NO_RECENT, "No recent files", None),
    (ACTUAL_SIZE, "Actual size", Some(crate::commands::ACTUAL_SIZE)),
    (PAGE_LEVEL, "Zoom to page level", Some(crate::commands::PAGE_LEVEL)),
    (FIT_WIDTH, "Fit to width", Some(crate::commands::FIT_WIDTH)),
    (FIT_HEIGHT, "Fit to height", None),
    (ZOOM_IN, "Zoom in", Some(crate::commands::ZOOM_IN)),
    (ZOOM_OUT, "Zoom out", Some(crate::commands::ZOOM_OUT)),
    (ROTATE_CW, "Rotate view clockwise", Some(crate::commands::ROTATE_CW)),
    (ROTATE_CCW, "Rotate view counterclockwise", Some(crate::commands::ROTATE_CCW)),
];

/// View ▸ Side panels: the panel, its id in [`PANEL`] commands and its English label.
const PANELS: &[(RightPanel, &str, &str)] = &[
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
    SELECT_ALL,
    SEP,
    "edit.find",
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
const PANES: &[&str] = &["view.side_panel", "view.tools_pane"];
const PANE_COMMANDS: &[&str] = &["comment.list", "form.fields", "ui.dock.reset"];
const MODES: &[&str] = &["view.full_screen", "view.read_mode"];
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
    SEP,
    "bookmark.add",
    "bookmark.from_structure",
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
    }
}

fn translate(english: &'static str) -> String {
    tl!(english).to_owned()
}

/// The layouts' own labels (About, Quit, Window…) through the app's translations.
pub(crate) fn layout_options() -> LayoutOptions<'static> {
    LayoutOptions { label: &translate, ..LayoutOptions::new(APP_NAME) }
}

impl PdfCraftApp {
    /// The app's menus for this frame, before the platform layout: every item's label, shortcut,
    /// enabled and checked state as the app is now. A modal dialog doesn't grey anything here;
    /// that is the [`Gate`]'s job ([`PdfCraftApp::menu_gate`]).
    pub fn menu_bar(&self, ctx: &egui::Context) -> MenuBar {
        let mut used: Vec<&str> = Vec::new();
        let list = |ids: &[&'static str], used: &mut Vec<&'static str>| -> Vec<Node> {
            ids.iter()
                .map(|&id| {
                    if id == SEP {
                        return Node::Separator;
                    }
                    used.push(id);
                    self.menu_item(id, None)
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
                    nodes.push(self.recent_menu());
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
                    self.menu_item(id, Some(tl!(label)))
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
        view.push(Node::submenu(tl!("Tools"), self.tools_menu(&used)));
        view.extend(list(&["view.palette"], &mut used));
        let mut bar = MenuBar {
            menus: vec![
                Menu::new(tl!("File"), craft_menu::tidy(file)).role(MenuRole::File),
                Menu::new(tl!("Edit"), craft_menu::tidy(edit)).role(MenuRole::Edit),
                Menu::new(tl!("View"), craft_menu::tidy(view)).role(MenuRole::View),
                Menu::new(tl!("Sign"), craft_menu::tidy(sign)),
                Menu::new(tl!("Window"), window).role(MenuRole::Window),
                Menu::new(tl!("Help"), craft_menu::tidy(help)).role(MenuRole::Help),
            ],
            documents: self.menu_documents(),
        };
        bar.tag_item(&CommandRef::new("help.about"), ItemRole::About);
        bar.tag_item(&CommandRef::new("app.preferences"), ItemRole::Settings);
        craft_menu::apply_state(&mut bar, |command| self.menu_state(ctx, command));
        bar
    }

    /// One item: a registry command with its live label (Undo names the edit) and shortcut, or a
    /// menu-only one. Radio and check items get their kind here, their state from
    /// [`PdfCraftApp::menu_state`].
    fn menu_item(&self, id: &'static str, label: Option<&str>) -> Node {
        let (english, shortcut) = match commands::command(id) {
            Some(spec) => {
                let current = commands::current_label(spec, &self.session, self.active_ids().map(|(_, doc)| doc));
                (crate::i18n::menu_label(id, &current), spec.shortcut.or((id == "edit.copy").then_some(crate::commands::COPY)))
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
    fn recent_menu(&self) -> Node {
        let mut items: Vec<Node> = self
            .recent
            .iter()
            .enumerate()
            .map(|(index, r)| Item::new(CommandRef::with_params("file.open_recent", serde_json::json!({ "index": index })), r.name.clone()).into())
            .collect();
        if items.is_empty() {
            items.push(self.menu_item(NO_RECENT, None));
        } else {
            items.push(Node::Separator);
            items.push(self.menu_item("file.clear_recent", Some(tl!("Clear Recent Files"))));
        }
        let label = commands::command("file.open_recent").map_or("Open Recent", |c| c.label);
        Node::submenu(crate::i18n::menu_label("file.open_recent", label), items)
    }

    /// View ▸ Tools: the All tools groups, each a submenu of its ready tools that no other menu
    /// lists, sections apart.
    fn tools_menu(&self, used: &[&str]) -> Vec<Node> {
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
                            nodes.push(self.menu_item(id, label));
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

    /// What may run from the menus now: a dialog, a prompt or the command palette holds the
    /// window. Edit's text items stay with a focused text field (craft-menu's gate).
    pub fn menu_gate(&self, ctx: &egui::Context) -> Gate {
        Gate { modal: self.modal_open() || self.palette_open, recording_shortcut: false, text_editing: ctx.text_edit_focused() }
    }

    /// Whether a menu item's command can run now, and its checked or selected state.
    pub fn menu_state(&self, ctx: &egui::Context, command: &CommandRef) -> ItemState {
        let view = self.active.and_then(|i| self.views.get(i));
        let organize = view.is_some_and(|v| v.organize);
        let has_doc = view.is_some();
        let enabled_id = |id: &str| commands::command(id).is_some_and(|s| self.command_enabled(s));
        let index = param_index(command);
        let text = ctx.text_edit_focused() && matches!(command.id.as_str(), "edit.undo" | "edit.redo" | "edit.copy" | CUT | PASTE | SELECT_ALL);
        let enabled = text
            || match command.id.as_str() {
                NO_RECENT => false,
                "file.open_recent" => index.is_none_or(|i| i < self.recent.len()),
                "file.clear_recent" => !self.recent.is_empty(),
                DOCUMENT => index.is_some_and(|i| i < self.views.len()),
                // macOS ⌘W closes the window when no document is open (#869).
                "file.close" if !has_doc && crate::commands::mac_shortcuts(ctx) && !cfg!(target_arch = "wasm32") => true,
                "edit.copy" if organize => enabled_id("page.copy"),
                "edit.copy" if view.is_some_and(|v| v.picked_image.is_some()) => {
                    self.active_ids().and_then(|(_, id)| self.session.get(id)).is_some_and(|d| d.allows_copying())
                }
                CUT => organize && enabled_id("page.cut"),
                PASTE => organize && self.page_clipboard.is_some() && enabled_id("page.paste"),
                SELECT_ALL | DELETE => has_doc,
                ACTUAL_SIZE | PAGE_LEVEL | FIT_WIDTH | FIT_HEIGHT | ZOOM_IN | ZOOM_OUT | ROTATE_CW | ROTATE_CCW => has_doc,
                PANEL => panel(command).is_some(),
                id => enabled_id(id),
            };
        let checked = match command.id.as_str() {
            "view.layout.single" => Some(view.is_some_and(|v| v.layout == PageLayout::Single)),
            "view.layout.two_up" => Some(view.is_some_and(|v| v.layout == PageLayout::TwoUp)),
            "view.continuous" => Some(view.is_some_and(|v| v.continuous())),
            "view.page_gaps" => Some(view.is_some_and(|v| v.show_page_gaps)),
            "view.layout.cover" => Some(view.is_some_and(|v| v.cover)),
            "view.full_screen" => Some(self.full_screen),
            "view.read_mode" => Some(self.mode == crate::Mode::Read),
            "view.side_panel" => Some(self.right.is_some()),
            "view.tools_pane" => Some(self.left_open),
            "view.theme.system" => Some(self.theme_preference == ThemePreference::System),
            "view.theme.light" => Some(self.theme_preference == ThemePreference::Light),
            "view.theme.dark" => Some(self.theme_preference == ThemePreference::Dark),
            PANEL => Some(panel(command).is_some_and(|p| self.right == Some(p))),
            _ => None,
        };
        ItemState { enabled, checked }
    }

    /// Run a command chosen in either menu bar. Registry commands go through
    /// [`PdfCraftApp::dispatch_command`], as their shortcuts do; the clipboard items act as
    /// their keys would (⌘C is a Copy event, Delete a key press), so every place that handles the
    /// key handles the click. A command that can't run now (a dialog opened since the menu was
    /// drawn) does nothing.
    pub fn run_menu_command(&mut self, ctx: &egui::Context, command: &CommandRef) {
        // Both menu bars run Quit and Minimize themselves; Hide and Hide Others are AppKit's.
        if Builtin::of(command).is_some() {
            return;
        }
        if !self.menu_gate(ctx).allows_command(command) || !self.menu_state(ctx, command).enabled {
            return;
        }
        let index = param_index(command);
        let key = |key: egui::Key, modifiers: egui::Modifiers| {
            [true, false].map(|pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers })
        };
        match command.id.as_str() {
            "file.open_recent" if index.is_some() => {
                if let Some(path) = index.and_then(|i| self.recent.get(i)).map(|r| r.path.clone()) {
                    self.open_recent(&path);
                }
            }
            DOCUMENT => {
                if let Some(i) = index.filter(|&i| i < self.views.len()) {
                    self.active = Some(i);
                }
            }
            "edit.copy" => self.menu_input.push(egui::Event::Copy),
            CUT => self.menu_input.push(egui::Event::Cut),
            PASTE => ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
            SELECT_ALL => self.menu_input.extend(key(egui::Key::A, egui::Modifiers::COMMAND)),
            DELETE => self.menu_input.extend(key(egui::Key::Delete, egui::Modifiers::NONE)),
            PANEL => self.choose_right_panel(panel(command)),
            ACTUAL_SIZE | PAGE_LEVEL | FIT_WIDTH | FIT_HEIGHT | ZOOM_IN | ZOOM_OUT | ROTATE_CW | ROTATE_CCW => {
                let Some(v) = self.active.and_then(|i| self.views.get_mut(i)) else { return };
                match command.id.as_str() {
                    ACTUAL_SIZE => v.set_zoom(1.0),
                    PAGE_LEVEL => v.fit = Fit::Page,
                    FIT_WIDTH => v.fit = Fit::Width,
                    FIT_HEIGHT => {
                        v.fit = Fit::Height;
                        v.goto = Some((v.current, 0.0));
                    }
                    ZOOM_IN => v.zoom_step(true),
                    ZOOM_OUT => v.zoom_step(false),
                    ROTATE_CW => v.rotate_view(true),
                    _ => v.rotate_view(false),
                }
            }
            id => {
                if let Some(spec) = commands::command(id) {
                    self.dispatch_command(ctx, spec.id);
                }
            }
        }
        ctx.request_repaint();
    }

    /// The menu bar drawn in the window (Windows, Linux, the web, and macOS when the native one
    /// couldn't be installed), where the top bar starts. Runs the commands chosen.
    pub(crate) fn menu_bar_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let bar = craft_menu::layout(&self.menu_bar(&ctx), Target::current(), &layout_options()).bar;
        let gate = self.menu_gate(&ctx);
        // The mode tabs, search and buttons follow the bar in the same row: it takes its titles'
        // width, less when the row is short, where the menus that don't fit move into its "⋯".
        let fit = (ui.available_width() - ROW_AFTER_MENUS).max(MIN_MENUS_WIDTH);
        let chosen = craft_menu::MenuBarUi::new(&bar)
            .gate(gate)
            .label(&translate)
            .english(&english)
            .id(egui::Id::unique("pdfcraft-menu-bar"))
            .shrink_to_fit(fit)
            .show(ui);
        for command in chosen {
            self.run_menu_command(&ctx, &command);
        }
    }

    /// Install the native menu bar (macOS), once, from the eframe app creator. `false` when
    /// AppKit refused it: the in-window bar stays.
    #[cfg(target_os = "macos")]
    pub fn install_native_menu(&mut self, ctx: &egui::Context) -> bool {
        crate::i18n::set_current(crate::i18n::Lang::from_pref(&self.language));
        let bar = craft_menu::mac_layout(&self.menu_bar(ctx), &layout_options()).bar;
        // The Window menu lists the open documents itself, so AppKit's window list stays off.
        self.native_menu = craft_menu::mac::install_with(ctx, &bar, craft_menu::mac::MacOptions::default().window_list(false));
        self.native_menu.is_some()
    }

    /// The native menu's clicks since the last frame, run like in-window ones.
    pub(crate) fn native_menu_clicks(&mut self, ctx: &egui::Context) {
        let gate = self.menu_gate(ctx);
        let Some(native) = self.native_menu.as_mut() else { return };
        native.set_gate(gate);
        for command in native.take_clicks(ctx) {
            self.run_menu_command(ctx, &command);
        }
    }

    /// Bring the native menu in step with the app, after this frame's changes.
    pub(crate) fn sync_native_menu(&mut self, ctx: &egui::Context) {
        if self.native_menu.is_none() {
            return;
        }
        let layout = craft_menu::mac_layout(&self.menu_bar(ctx), &layout_options());
        let gate = self.menu_gate(ctx);
        if let Some(native) = self.native_menu.as_mut() {
            native.set_gate(gate);
            for clash in native.sync(layout) {
                log::info!("menu shortcut {:?} kept by {} over {} ({})", clash.shortcut, clash.command.id, clash.system, clash.resolution);
            }
        }
    }
}

/// A command's `{"index": n}` parameter.
fn param_index(command: &CommandRef) -> Option<usize> {
    command.params.get("index").and_then(serde_json::Value::as_u64).and_then(|i| usize::try_from(i).ok())
}

/// The side panel a [`PANEL`] command names.
fn panel(command: &CommandRef) -> Option<RightPanel> {
    let name = command.params.get("panel").and_then(serde_json::Value::as_str)?;
    PANELS.iter().find(|(_, n, _)| *n == name).map(|(p, ..)| *p)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use craft_menu::{Backend, Dispatch, Event, ItemKind, NativeMenu};

    use super::*;

    const PDF: &[u8] = include_bytes!("../tests/data/form.pdf");

    fn app(docs: &[&str]) -> (PdfCraftApp, egui::Context) {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        for name in docs {
            app.open_bytes(name, None, PDF.to_vec()).unwrap();
        }
        (app, egui::Context::default())
    }

    fn linux(app: &PdfCraftApp, ctx: &egui::Context) -> MenuBar {
        craft_menu::layout(&app.menu_bar(ctx), Target::Linux, &layout_options()).bar
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
            let shortcut = spec.shortcut.or((spec.id == "edit.copy").then_some(crate::commands::COPY));
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
    fn check_and_radio_items_follow_the_app() {
        let (mut app, ctx) = app(&["a.pdf"]);
        let kind = |app: &PdfCraftApp, id: &str| item(&linux(app, &ctx), &CommandRef::new(id)).kind;
        assert_eq!(kind(&app, "view.layout.single"), ItemKind::Radio(true));
        assert_eq!(kind(&app, "view.layout.two_up"), ItemKind::Radio(false));
        let continuous = app.views[0].continuous();
        assert_eq!(kind(&app, "view.continuous"), ItemKind::Check(continuous));
        app.run_menu_command(&ctx, &CommandRef::new("view.layout.two_up"));
        app.run_menu_command(&ctx, &CommandRef::new("view.continuous"));
        assert_eq!(kind(&app, "view.layout.single"), ItemKind::Radio(false));
        assert_eq!(kind(&app, "view.layout.two_up"), ItemKind::Radio(true));
        assert_eq!(kind(&app, "view.continuous"), ItemKind::Check(!continuous));
        let left = app.left_open;
        assert_eq!(kind(&app, "view.tools_pane"), ItemKind::Check(left));
        app.run_menu_command(&ctx, &CommandRef::new("view.tools_pane"));
        assert_eq!(kind(&app, "view.tools_pane"), ItemKind::Check(!left));
        let bookmarks = CommandRef::with_params(PANEL, serde_json::json!({ "panel": "bookmarks" }));
        app.run_menu_command(&ctx, &bookmarks);
        assert_eq!(app.right, Some(RightPanel::Bookmarks));
        assert_eq!(item(&linux(&app, &ctx), &bookmarks).kind, ItemKind::Radio(true));
        assert_eq!(kind(&app, "view.side_panel"), ItemKind::Check(true));
    }

    #[test]
    fn a_dialog_greys_document_commands_and_drops_their_clicks() {
        let (mut app, ctx) = app(&["a.pdf"]);
        let rotate = CommandRef::new("page.rotate");
        assert!(app.menu_gate(&ctx).allows_command(&rotate));
        app.dialog = Some(crate::Dialog::About);
        assert!(!app.menu_gate(&ctx).allows_command(&rotate), "the gate greys the menus under a dialog");
        let doc = app.views[0].id;
        let before = app.session.get(doc).unwrap().info.pages[0].rotation;
        app.run_menu_command(&ctx, &rotate);
        assert_eq!(app.session.get(doc).unwrap().info.pages[0].rotation, before, "a click that arrives anyway does nothing");
        // Quit still asks the window to close, which asks about unsaved changes.
        assert!(app.menu_gate(&ctx).allows_command(&Builtin::Quit.command()));
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
        app.run_menu_command(&ctx, &CommandRef::with_params(DOCUMENT, serde_json::json!({ "index": 0 })));
        assert_eq!(app.active, Some(0));
        // The macOS layout lists them once too.
        let mac = craft_menu::mac_layout(&app.menu_bar(&ctx), &layout_options()).bar;
        let listed = mac.items().iter().filter(|it| it.command.id == DOCUMENT).count();
        assert_eq!(listed, 2);
        app.recent.clear();
        let bar = linux(&app, &ctx);
        assert!(!item(&bar, &CommandRef::new(NO_RECENT)).enabled, "an empty list shows a greyed placeholder");
        assert!(bar.find(&CommandRef::new("file.clear_recent")).is_none());
    }

    /// A native backend that reports what the test queues, through the driver's dispatch as the
    /// macOS backend does.
    #[derive(Clone, Default)]
    struct FakeMenu(Rc<RefCell<(Option<Dispatch>, Vec<Event>)>>);

    impl FakeMenu {
        fn click(&self, command: &CommandRef) {
            let mut state = self.0.borrow_mut();
            let event = state.0.as_ref().map(|d| d.chosen(command, None, false, None));
            state.1.extend(event);
        }
    }

    impl Backend for FakeMenu {
        fn sync(&mut self, _bar: &MenuBar) {}
        fn drain(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.0.borrow_mut().1)
        }
        fn attach(&mut self, dispatch: Dispatch) {
            self.0.borrow_mut().0 = Some(dispatch);
        }
    }

    #[test]
    fn a_native_menu_click_runs_its_command_once() {
        let (mut app, ctx) = app(&["a.pdf"]);
        let fake = FakeMenu::default();
        app.native_menu = Some(NativeMenu::new(Box::new(fake.clone())));
        let doc = app.views[0].id;
        let before = app.session.get(doc).unwrap().info.pages[0].rotation;
        fake.click(&CommandRef::new("page.rotate"));
        let mut raw = egui::RawInput::default();
        app.prepare_raw_input(&ctx, &mut raw);
        app.native_menu_clicks(&ctx);
        app.native_menu_clicks(&ctx);
        assert_eq!(app.session.get(doc).unwrap().info.pages[0].rotation, (before + 90) % 360, "rotated once");
    }
}
