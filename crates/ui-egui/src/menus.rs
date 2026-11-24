//! The menus: one definition ([`PdfCraftApp::menu_bar`]) for the native macOS menu bar and the
//! in-window bar on Windows, Linux and the web, laid out for each platform by craft-menu.
//!
//! Items run registry commands (`pdfcraft_engine::commands`) by id, with the registry's label and
//! shortcut, so a menu click and its shortcut run the same command. A few items have menu-only
//! ids: the zoom keys the document view handles itself, the side panel choices, the clipboard
//! keys, the recent files and the open documents. Each is a command of the app's catalog
//! (`commands::catalog`), fired once per click, whichever bar it came from.

use craft::keymap::{Key, Mods, Platform};
use craft::menu::{
    Builtin, Chord, CommandRef, Document, Gate, Item, ItemKind, ItemRole, ItemState, Layout, LayoutOptions, Menu, MenuBar, MenuRole, Node, Target,
};
use craft::ui::contract::{Props as _, Show as _};
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

/// Puts the English names on the menu model, for the bar's mnemonics and the Help search.
fn name_in_english(nodes: &mut [Node]) {
    for node in nodes {
        match node {
            Node::Item(item) => item.english = english_label(&item.command),
            Node::Submenu { label, english, children, .. } => {
                *english = TITLES.iter().find(|t| tl!(t) == label.as_str()).map(|t| (*t).to_owned());
                name_in_english(children);
            }
            _ => {}
        }
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
/// menu's gate. A text field only, as production's `text_edit_focused`: egui's keyboard focus alone is
/// also on an open menu's rows or a button, which would hand Edit ▸ Copy and Paste to them.
pub(crate) fn text_editing(ctx: &egui::Context) -> bool {
    craft::ui::runtime::windows::WindowHost::of(ctx).typing()
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
        let mut file = {
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
        let mut help = list(HELP, &mut used);
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
        for nodes in [&mut file, &mut edit, &mut view, &mut sign, &mut help] {
            craft::menu::tidy(nodes);
        }
        let mut bar = MenuBar::new(vec![
            Menu::new(tl!("File"), file).role(MenuRole::File),
            Menu::new(tl!("Edit"), edit).role(MenuRole::Edit),
            Menu::new(tl!("View"), view).role(MenuRole::View),
            Menu::new(tl!("Sign"), sign),
            Menu::new(tl!("Window"), window).role(MenuRole::Window),
            Menu::new(tl!("Help"), help).role(MenuRole::Help),
        ])
        .documents(self.menu_documents());
        for menu in &mut bar.menus {
            menu.english = TITLES.iter().find(|t| tl!(t) == menu.title.as_str()).map(|t| (*t).to_owned());
            name_in_english(&mut menu.children);
        }
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
        let mut bar = craft::ui::contract::Tracked::new(std::mem::take(&mut layout.bar));
        craft::menu::apply_state(&mut bar, |command| {
            let resolved = resolve(ctx, command.clone());
            ItemState::default().enabled(resolved.enabled).checked(resolved.checked)
        });
        layout.bar = bar.into_inner();
        layout
    }

    /// The menu bar drawn in the window (Windows, Linux, the web, and macOS when the native one
    /// couldn't be installed), where the top bar starts: collapsed into a single "Menu" button that
    /// opens the full hierarchical menu tree as a popup.
    pub(crate) fn menu_bar_ui(&mut self, ui: &mut egui::Ui) {
        let out = craft::ui::buttons::Button::new(tl!("Menu"))
            .variant(craft::theme::Variant::GHOST)
            .icon(craft::behavior::IconId::new("panel-left"))
            .show(ui);
        let btn = out.response;
        let popup_id = egui::Popup::default_response_id(&btn);
        ui.ctx().accesskit_node_builder(btn.id, |n| {
            n.set_role(egui::accesskit::Role::Button);
            n.set_label(tl!("Menu"));
            n.set_has_popup(egui::accesskit::HasPopup::Menu);
        });
        handle_menu_button_keys(self, ui.ctx(), popup_id, &btn);
        egui::Popup::menu(&btn).show(|ui| self.menu_popup_ui(ui));
    }

    /// The menus as a vertical popup: each top-level menu is a submenu button, with its items and
    /// submenus rendered recursively. Used by the "Menu" button in the mode bar.
    pub(crate) fn menu_popup_ui(&mut self, ui: &mut egui::Ui) {
        let pass = ui.ctx().cumulative_pass_nr();
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("craft-menu-bar-open"), pass));
        let ctx = ui.ctx().clone();
        let mut layout = craft::menu::layout(&self.menu_bar(&ctx), Target::current(), &layout_options());
        if *self.window_menu_bar != layout.bar {
            *self.window_menu_bar = std::mem::take(&mut layout.bar);
        }
        let mut bar = self.window_menu_bar.clone();
        craft::menu::apply_state(&mut bar, |command| {
            let resolved = resolve(&ctx, command.clone());
            ItemState::default().enabled(resolved.enabled).checked(resolved.checked)
        });
        let bar = bar.into_inner();
        let gate = Gate::default().modal(gated(self)).text_editing(text_editing(&ctx));
        let mut chosen = Vec::new();
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        ui.set_min_width(220.0);

        let pressed_key = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. }
                    if !modifiers.alt && !modifiers.ctrl && !modifiers.command && !modifiers.mac_cmd =>
                {
                    key_letter(*key).map(|c| (c, *key))
                }
                _ => None,
            })
        });

        let has_open_submenu = egui::containers::menu::MenuState::from_ui(ui, |state, _| state.open_item.is_some());

        // An explicit menu target (e.g. from Alt+F) always selects that top-level menu.
        // A plain letter only targets top-level menus when no submenu is open yet;
        // once a submenu is open, plain letters route into the active submenu.
        let mut active_key = None;
        let target = if let Some(t) = self.menu_target.take() {
            Some(t)
        } else if !has_open_submenu {
            pressed_key.map(|(c, _)| c)
        } else {
            active_key = pressed_key;
            None
        };

        let titles: Vec<&str> = bar.menus.iter().map(|m| m.title.as_str()).collect();
        let preferred: Vec<Option<char>> = bar.menus.iter().map(|m| preferred_menu_role_mnemonic(m.role, &m.title)).collect();
        let mnemonics = assign_mnemonics(&titles, &preferred);

        for (menu, mnemonic) in bar.menus.iter().zip(mnemonics) {
            let is_target = target.is_some() && target == mnemonic;
            let next_id = ui.next_auto_id();
            let submenu_id = egui::containers::menu::SubMenu::id_from_widget_id(next_id);
            if is_target {
                if let Some((c, key)) = pressed_key {
                    consume_letter(ui.ctx(), egui::Modifiers::NONE, key, c);
                }
                // A fresh popup has no child MenuState yet, and egui's MenuState::from_id clears
                // open_item when the open submenu was never shown (its state is missing). Mark
                // the child shown to create its state, so Alt+F (the menu target) opens it.
                egui::containers::menu::MenuState::mark_shown(ui.ctx(), submenu_id);
                egui::containers::menu::MenuState::from_ui(ui, |state, _| {
                    state.open_item = Some(submenu_id);
                });
                ui.memory_mut(|mem| {
                    mem.request_focus(next_id);
                    let dir = if ui.style().right_to_left { egui::FocusDirection::Left } else { egui::FocusDirection::Right };
                    mem.move_focus(dir);
                });
            }
            let sub_button = egui::containers::menu::SubMenuButton::new(mnemonic_label(ui, &menu.title, mnemonic, true))
                .config(egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside));
            let (resp_btn, _) = sub_button.ui(ui, |ui| {
                ui.set_min_width(220.0);
                ui.set_max_width(280.0);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                let max_h = (ui.ctx().content_rect().height() - 40.0).max(200.0);
                ui.set_max_height(max_h);
                ui.spacing_mut().scroll.floating = false;
                egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                    render_nodes(ui, &menu.children, &gate, &mut chosen, &mut active_key);
                });
            });
            ctx.accesskit_node_builder(resp_btn.id, |n| {
                n.set_role(egui::accesskit::Role::MenuItem);
                n.set_label(&menu.title);
                n.set_has_popup(egui::accesskit::HasPopup::Menu);
                if let Some(m) = mnemonic {
                    n.set_access_key(m.to_string());
                }
            });
        }
        for command in chosen {
            if command.id == QUIT || matches!(Builtin::of(&command), Some(Builtin::Quit)) {
                self.request_quit(&ctx);
                craft_quit::on_menu_event(&ctx, QUIT);
            } else if matches!(Builtin::of(&command), Some(Builtin::Minimize)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            } else {
                fire(&ctx, command, FireSource::Menu);
            }
        }
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
    // The layout is built every frame from the commands' state; keyed on its bar, the native menus are laid out
    // again only when that changed (or a menu event flipped an item).
    let layout = app.native_bar(ctx);
    let key = layout.bar.clone();
    if let Some(native) = app.native_menu.as_mut() {
        for clash in native.sync_with(&key, move || layout) {
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

/// Handle Alt-tap, F10, and Alt+letter mnemonics (e.g. Alt+F for File) for the mode bar Menu button.
pub(crate) fn handle_menu_button_keys(app: &mut PdfCraftApp, ctx: &egui::Context, popup_id: egui::Id, btn: &egui::Response) {
    let focus_id = egui::Id::unique("mode_bar_menu_focused");
    let mut menu_focused: bool = ctx.data(|d| d.get_temp(focus_id)).unwrap_or(false);

    // A command shortcut (Ctrl+S, F7…) pressed while the keyboard is on the button left the bar
    // (craft_menu::bar::leave_for_shortcut, app.rs): the shortcut runs in that press and the
    // keyboard goes back to the pages. Consume the handoff here, before the local focus state
    // below can put the keyboard back on the bar every frame.
    if ctx.data(|d| d.get_temp::<bool>(egui::Id::unique("craft-menu-bar-leave"))).unwrap_or(false) {
        ctx.data_mut(|d| d.remove::<bool>(egui::Id::unique("craft-menu-bar-leave")));
        menu_focused = false;
        btn.surrender_focus();
    }

    if btn.clicked() {
        menu_focused = false;
    }
    if ctx.input(|i| i.pointer.any_click()) && !btn.hovered() {
        menu_focused = false;
    }

    let f10 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F10));

    // Track Alt-tap (pressing and releasing Alt without another key) to focus the menu button.
    #[derive(Clone, Copy, Default)]
    struct AltTapState {
        alt_tap: bool,
        buttons_held: u32,
    }
    let alt_state_id = egui::Id::unique("mode_bar_alt_tap_state");
    let mut alt_state: AltTapState = ctx.data(|d| d.get_temp(alt_state_id)).unwrap_or_default();
    let mut alt_tapped = false;

    for event in ctx.input(|i| i.events.clone()) {
        match event {
            egui::Event::Key { key: egui::Key::AltLeft | egui::Key::AltRight, pressed, repeat, modifiers, .. } => {
                if pressed && !repeat {
                    alt_state.alt_tap =
                        !modifiers.ctrl && !modifiers.shift && !modifiers.command && !modifiers.mac_cmd && alt_state.buttons_held == 0;
                } else if !pressed && alt_state.alt_tap {
                    alt_tapped = true;
                    alt_state.alt_tap = false;
                }
            }
            egui::Event::PointerButton { pressed, button, .. } => {
                let bit = 1u32 << (button as u8);
                if pressed {
                    alt_state.buttons_held |= bit;
                } else {
                    alt_state.buttons_held &= !bit;
                }
                alt_state.alt_tap = false;
                alt_tapped = false;
            }
            egui::Event::WindowFocused(false) => {
                alt_state.alt_tap = false;
                alt_state.buttons_held = 0;
            }
            egui::Event::Key { pressed: true, .. } | egui::Event::Text(_) | egui::Event::MouseWheel { .. } => {
                alt_state.alt_tap = false;
            }
            _ => {}
        }
    }
    ctx.data_mut(|d| d.insert_temp(alt_state_id, alt_state));

    let is_open = egui::Popup::is_id_open(ctx, popup_id);

    if f10 || alt_tapped {
        if is_open {
            egui::Popup::close_all(ctx);
            menu_focused = false;
            btn.surrender_focus();
        } else {
            menu_focused = !menu_focused;
            if menu_focused {
                btn.request_focus();
            } else {
                btn.surrender_focus();
            }
        }
        ctx.request_repaint();
    }

    if menu_focused {
        if ctx.input(|i| i.key_pressed(egui::Key::Tab)) || ctx.memory(|m| m.focused().is_some_and(|id| id != btn.id)) {
            menu_focused = false;
            btn.surrender_focus();
        } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            menu_focused = false;
            btn.surrender_focus();
            ctx.request_repaint();
        } else if ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Space)
        }) {
            menu_focused = false;
            egui::Popup::open_id(ctx, popup_id);
            ctx.request_repaint();
        } else {
            let plain_letter = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. }
                        if !modifiers.alt && !modifiers.command && !modifiers.ctrl && !modifiers.mac_cmd =>
                    {
                        key_letter(*key).map(|c| (c, *key))
                    }
                    _ => None,
                })
            });
            if let Some((c, key)) = plain_letter {
                ctx.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, key);
                    i.events
                        .retain(|e| !matches!(e, egui::Event::Text(t) if t.chars().count() == 1 && t.chars().all(|ch| ch.to_ascii_uppercase() == c)));
                });
                menu_focused = false;
                egui::Popup::open_id(ctx, popup_id);
                app.menu_target = Some(c);
                ctx.request_repaint();
            }
        }
    }

    // Alt+letter (e.g. Alt+F) opens the Menu popup and selects that submenu.
    // Disabled on macOS so Option-generated text remains available.
    let is_mac = matches!(ctx.os(), egui::os::OperatingSystem::Mac | egui::os::OperatingSystem::IOS);
    let alt_letter = ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key { key, pressed: true, modifiers, .. }
                if !is_mac && modifiers.alt && !modifiers.command && !modifiers.ctrl && !modifiers.mac_cmd =>
            {
                key_letter(*key).map(|c| (c, *key))
            }
            _ => None,
        })
    });
    if let Some((c, key)) = alt_letter {
        let bar = craft::menu::layout(&app.menu_bar(ctx), Target::current(), &layout_options()).bar;
        let mnemonics = top_level_mnemonics(&bar);
        if mnemonics.contains(&c) {
            ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::ALT, key);
                i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t.chars().count() == 1 && t.chars().all(|ch| ch.to_ascii_uppercase() == c)));
            });
            menu_focused = false;
            egui::Popup::open_id(ctx, popup_id);
            app.menu_target = Some(c);
            ctx.request_repaint();
        }
    }

    ctx.data_mut(|d| d.insert_temp(focus_id, menu_focused));
    let pass = ctx.cumulative_pass_nr();
    if is_open {
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("craft-menu-bar-open"), pass));
    }
    if menu_focused {
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("craft-menu-bar-focus"), pass));
    }
}

/// Render a slice of menu `Node`s into `ui` for the "Menu" button popup, collecting clicked
/// commands. `Standard` items (AppKit-only) are skipped.
fn render_nodes(ui: &mut egui::Ui, nodes: &[Node], gate: &Gate, chosen: &mut Vec<CommandRef>, active_key: &mut Option<(char, egui::Key)>) {
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    let ctx = ui.ctx().clone();
    let root_id = egui::containers::menu::find_menu_root(ui).unique_id;
    egui::containers::menu::MenuState::mark_shown(&ctx, root_id);

    let labels: Vec<&str> = nodes
        .iter()
        .map(|node| match node {
            Node::Item(item) => item.label.as_str(),
            Node::Submenu { label, .. } => label.as_str(),
            _ => "",
        })
        .collect();

    let preferred: Vec<Option<char>> = nodes
        .iter()
        .map(|node| match node {
            Node::Item(item) => preferred_item_mnemonic(item),
            Node::Submenu { label, .. } => extract_explicit_mnemonic(label),
            _ => None,
        })
        .collect();

    let mnemonics = assign_mnemonics(&labels, &preferred);
    let child_open = egui::containers::menu::MenuState::from_ui(ui, |state, _| state.open_item.is_some());

    for (node, mnemonic) in nodes.iter().zip(mnemonics) {
        match node {
            Node::Separator => {
                ui.separator();
            }
            Node::Item(item) => {
                let enabled = item.enabled && gate.allows(item);
                let role = match item.kind {
                    ItemKind::Plain => egui::accesskit::Role::MenuItem,
                    ItemKind::Check(_) => egui::accesskit::Role::MenuItemCheckBox,
                    ItemKind::Radio(_) => egui::accesskit::Role::MenuItemRadio,
                    _ => egui::accesskit::Role::MenuItem,
                };
                let mark = match item.kind {
                    ItemKind::Check(true) | ItemKind::Radio(true) => "\u{2713} ",
                    ItemKind::Check(false) | ItemKind::Radio(false) => "  ",
                    ItemKind::Plain => "",
                    _ => "",
                };
                let mut button = egui::Button::new(mnemonic_item_label(ui, mark, &item.label, mnemonic, true));
                let shortcut_str = item.shortcut.as_ref().map(|c| c.display(shortcut_platform(&ctx)));
                if let Some(ref s) = shortcut_str {
                    button = button.shortcut_text(s.clone());
                }
                let resp = ui.add_enabled(enabled, button);
                ctx.accesskit_node_builder(resp.id, |n| {
                    n.set_role(role);
                    n.set_label(&item.label);
                    if !enabled {
                        n.set_disabled();
                    }
                    if let Some(ref s) = shortcut_str {
                        n.set_value(s.as_str());
                    }
                    if let Some(m) = mnemonic {
                        n.set_access_key(m.to_string());
                    }
                    match item.kind {
                        ItemKind::Check(on) | ItemKind::Radio(on) => {
                            n.set_toggled(if on { egui::accesskit::Toggled::True } else { egui::accesskit::Toggled::False });
                        }
                        _ => {}
                    }
                });

                let is_key_match = !child_open && active_key.is_some_and(|(c, _)| Some(c) == mnemonic);
                if is_key_match && let Some((c, key)) = active_key.take() {
                    consume_letter(&ctx, egui::Modifiers::NONE, key, c);
                    if enabled {
                        chosen.push(item.command.clone());
                        ui.close();
                        egui::Popup::close_all(&ctx);
                    }
                    ctx.request_repaint();
                }

                if resp.clicked() {
                    chosen.push(item.command.clone());
                    ui.close();
                }
            }
            Node::Submenu { label, children, .. } => {
                let enabled = any_enabled(children, gate);
                let is_key_match = !child_open && active_key.is_some_and(|(c, _)| Some(c) == mnemonic);

                let resp = ui
                    .add_enabled_ui(enabled, |ui| {
                        let sub_button = egui::containers::menu::SubMenuButton::new(mnemonic_label(ui, label, mnemonic, true))
                            .config(egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside));
                        let response = ui.add(sub_button.button);
                        let submenu_id = egui::containers::menu::SubMenu::id_from_widget_id(response.id);

                        if is_key_match && let Some((c, key)) = active_key.take() {
                            consume_letter(&ctx, egui::Modifiers::NONE, key, c);
                            if enabled {
                                egui::containers::menu::MenuState::mark_shown(&ctx, submenu_id);
                                egui::containers::menu::MenuState::from_ui(ui, |state, _| {
                                    state.open_item = Some(submenu_id);
                                });
                            }
                            ctx.request_repaint();
                        }

                        let open = egui::containers::menu::MenuState::from_ui(ui, |state, _| state.open_item == Some(submenu_id));

                        // A row scrolled out of view does not open its submenu on hover, but an
                        // already-open submenu (e.g. from keyboard navigation) continues to render.
                        let reach = response.rect.expand2(ui.spacing().item_spacing / 2.0);
                        let hidden = response.rect.is_positive()
                            && ui.ctx().pointer_hover_pos().is_some_and(|p| reach.contains(p) && !ui.clip_rect().contains(p));
                        if !hidden || open {
                            sub_button.sub_menu.show(ui, &response, |ui| {
                                ui.set_min_width(220.0);
                                ui.set_max_width(280.0);
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                let max_h = (ui.ctx().content_rect().height() - 40.0).max(200.0);
                                ui.set_max_height(max_h);
                                ui.spacing_mut().scroll.floating = false;
                                egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                                    render_nodes(ui, children, gate, chosen, active_key);
                                });
                            });
                        }
                        response
                    })
                    .inner;
                ctx.accesskit_node_builder(resp.id, |n| {
                    n.set_role(egui::accesskit::Role::MenuItem);
                    n.set_label(label);
                    n.set_has_popup(egui::accesskit::HasPopup::Menu);
                    if !enabled {
                        n.set_disabled();
                    }
                    if let Some(m) = mnemonic {
                        n.set_access_key(m.to_string());
                    }
                });
            }
            Node::Standard { .. } => {}
            _ => {}
        }
    }
}

fn any_enabled(nodes: &[Node], gate: &Gate) -> bool {
    nodes.iter().any(|node| match node {
        Node::Item(item) => item.enabled && gate.allows(item),
        Node::Submenu { children, .. } => any_enabled(children, gate),
        _ => false,
    })
}

/// The platform whose key names the shortcut column shows: the build's platform, but on the web
/// the one the browser runs on (`ctx.os()`), as craft-menu's bar does for its own shortcut column.
fn shortcut_platform(ctx: &egui::Context) -> Platform {
    use egui::os::OperatingSystem;
    if cfg!(target_arch = "wasm32") {
        match ctx.os() {
            OperatingSystem::Mac | OperatingSystem::IOS => Platform::Mac,
            OperatingSystem::Windows => Platform::Windows,
            _ => Platform::Linux,
        }
    } else {
        Platform::current()
    }
}

/// Extract the single ASCII character for an alphanumeric Key press, uppercase.
pub(crate) fn key_letter(key: egui::Key) -> Option<char> {
    let name = key.name();
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase()),
        _ => None,
    }
}

/// Consume `key` and its matching `Event::Text` from the egui input state so that
/// the pressed mnemonic letter is not passed to other widgets or text fields.
pub(crate) fn consume_letter(ctx: &egui::Context, mods: egui::Modifiers, key: egui::Key, c: char) {
    ctx.input_mut(|i| {
        i.consume_key(mods, key);
        i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t.chars().count() == 1 && t.chars().all(|ch| ch.to_ascii_uppercase() == c)));
    });
}

/// Extract explicit `(X)` mnemonic suffix from a label, if present.
pub(crate) fn extract_explicit_mnemonic(title: &str) -> Option<char> {
    if let Some(open) = title.rfind('(')
        && let Some(close) = title.rfind(')')
        && close == open + 2
    {
        let ch = title.as_bytes()[open + 1] as char;
        if ch.is_ascii_alphanumeric() {
            return Some(ch.to_ascii_uppercase());
        }
    }
    None
}

/// Determine the preferred mnemonic for a top-level menu role.
pub(crate) fn preferred_menu_role_mnemonic(role: MenuRole, title: &str) -> Option<char> {
    if let Some(c) = extract_explicit_mnemonic(title) {
        return Some(c);
    }
    match role {
        MenuRole::File => Some('F'),
        MenuRole::Edit => Some('E'),
        MenuRole::View => Some('V'),
        MenuRole::Window => Some('W'),
        MenuRole::Help => Some('H'),
        _ => {
            if title == "Sign" || title == "Fill & Sign" || title.starts_with("Sign") {
                Some('S')
            } else {
                None
            }
        }
    }
}

/// Determine the preferred mnemonic for a menu command item.
fn preferred_item_mnemonic(item: &Item) -> Option<char> {
    if let Some(c) = extract_explicit_mnemonic(&item.label) {
        return Some(c);
    }
    let id = item.command.id.as_str();
    if id == QUIT || matches!(Builtin::of(&item.command), Some(Builtin::Quit)) {
        if item.label.chars().any(|c| c.eq_ignore_ascii_case(&'X')) {
            return Some('X');
        } else {
            return Some('Q');
        }
    }
    match id {
        "file.open" => Some('O'),
        "file.close" => Some('C'),
        "file.save" => Some('S'),
        "file.save_as" => Some('A'),
        "file.print" => Some('P'),
        "edit.undo" => Some('U'),
        "edit.redo" => Some('R'),
        "edit.cut" => Some('T'),
        "edit.copy" => Some('C'),
        "edit.paste" => Some('P'),
        "edit.select_all" => Some('A'),
        _ => None,
    }
}

/// Assign unique uppercase ASCII mnemonics for a set of items or titles in a menu level,
/// avoiding duplicate assignments.
pub(crate) fn assign_mnemonics(labels: &[&str], preferred: &[Option<char>]) -> Vec<Option<char>> {
    let mut result: Vec<Option<char>> = vec![None; labels.len()];
    let mut used: Vec<char> = Vec::new();

    // Pass 1: Match preferred characters that exist in the label (or explicit (X) suffix).
    for (i, label) in labels.iter().enumerate() {
        if label.is_empty() {
            continue;
        }
        let explicit = extract_explicit_mnemonic(label);
        let pref = explicit.or_else(|| preferred.get(i).copied().flatten()).map(|c| c.to_ascii_uppercase());
        if let Some(p) = pref {
            let has = label.chars().any(|c| c.to_ascii_uppercase() == p);
            if has && !used.contains(&p) {
                result[i] = Some(p);
                used.push(p);
            }
        }
    }

    // Pass 2: For items without a mnemonic:
    // If the label has no ASCII characters at all, fall back to preferred if unused.
    // Otherwise try word starts first, then any ASCII alphanumeric character in the label.
    for (i, label) in labels.iter().enumerate() {
        if label.is_empty() || result[i].is_some() {
            continue;
        }
        let explicit = extract_explicit_mnemonic(label);
        let pref = explicit.or_else(|| preferred.get(i).copied().flatten()).map(|c| c.to_ascii_uppercase());

        let has_ascii = label.chars().any(|c| c.is_ascii_alphanumeric());
        if !has_ascii
            && let Some(p) = pref
            && !used.contains(&p)
        {
            result[i] = Some(p);
            used.push(p);
            continue;
        }

        let starts = label.split(|c: char| !c.is_alphanumeric()).filter_map(|w| w.chars().next());
        let rest = label.chars();
        let pick = starts.chain(rest).filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).find(|c| !used.contains(c));

        if let Some(c) = pick {
            result[i] = Some(c);
            used.push(c);
        } else if let Some(p) = pref {
            if !used.contains(&p) {
                result[i] = Some(p);
                used.push(p);
            }
        } else if let Some(unused) = ('A'..='Z').chain('0'..='9').find(|c| !used.contains(c)) {
            result[i] = Some(unused);
            used.push(unused);
        }
    }

    result
}

/// Top-level mnemonics for the menus in `bar`.
pub(crate) fn top_level_mnemonics(bar: &MenuBar) -> Vec<char> {
    let titles: Vec<&str> = bar.menus.iter().map(|m| m.title.as_str()).collect();
    let preferred: Vec<Option<char>> = bar.menus.iter().map(|m| preferred_menu_role_mnemonic(m.role, &m.title)).collect();
    assign_mnemonics(&titles, &preferred).into_iter().flatten().collect()
}

pub(crate) fn mnemonic_label(ui: &egui::Ui, label: &str, mnemonic: Option<char>, show_underline: bool) -> egui::WidgetText {
    mnemonic_item_label(ui, "", label, mnemonic, show_underline)
}

pub(crate) fn mnemonic_item_label(ui: &egui::Ui, mark: &str, label: &str, mnemonic: Option<char>, show_underline: bool) -> egui::WidgetText {
    let text_color = ui.visuals().text_color();
    let font_id = egui::TextStyle::Button.resolve(ui.style());

    let Some(m) = mnemonic else {
        return egui::WidgetText::from(format!("{mark}{label}"));
    };

    let char_match = label.char_indices().find(|(_, c)| c.to_ascii_uppercase() == m);
    let (display_text, match_idx, match_len) = match char_match {
        Some((i, c)) => (label.to_owned(), Some(i), c.len_utf8()),
        None => {
            let formatted = format!("{label} ({m})");
            let idx = formatted.rfind(m);
            (formatted, idx, 1)
        }
    };

    if !show_underline {
        return egui::WidgetText::from(format!("{mark}{display_text}"));
    }

    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;

    let format_normal = egui::TextFormat { font_id: font_id.clone(), color: text_color, ..Default::default() };
    let format_underlined = egui::TextFormat { font_id, color: text_color, underline: egui::Stroke::new(1.0, text_color), ..Default::default() };

    if !mark.is_empty() {
        job.append(mark, 0.0, format_normal.clone());
    }

    if let Some(i) = match_idx {
        if i > 0 {
            job.append(&display_text[..i], 0.0, format_normal.clone());
        }
        job.append(&display_text[i..i + match_len], 0.0, format_underlined);
        if i + match_len < display_text.len() {
            job.append(&display_text[i + match_len..], 0.0, format_normal);
        }
    } else {
        job.append(&display_text, 0.0, format_normal);
    }

    egui::WidgetText::from(job)
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

    #[test]
    fn french_file_and_window_have_distinct_sibling_mnemonics() {
        // Colliding first letter 'F', where Fenêtre's preferred role 'W' is absent from the title:
        let titles = ["Fichier", "Fenêtre"];
        let preferred = [Some('F'), Some('W')];
        let mnemonics = assign_mnemonics(&titles, &preferred);
        assert_eq!(mnemonics[0], Some('F'), "Fichier receives its preferred mnemonic 'F'");
        assert_ne!(mnemonics[1], Some('F'), "Fenêtre must not collide with Fichier's 'F'");
        assert!(mnemonics[1].is_some(), "Fenêtre receives an alternative unique mnemonic");
        assert_ne!(mnemonics[0], mnemonics[1], "Mnemonics within the menu set must be distinct");

        // French top-level menu set:
        let fr_titles = ["Fichier", "Édition", "Affichage", "Signer", "Fenêtre", "Aide"];
        let fr_pref = [Some('F'), Some('E'), Some('V'), Some('S'), Some('W'), Some('H')];
        let fr_mnemonics = assign_mnemonics(&fr_titles, &fr_pref);
        let mut seen = std::collections::HashSet::new();
        for (title, m) in fr_titles.iter().zip(fr_mnemonics.iter()) {
            let ch = m.unwrap_or_else(|| panic!("{title} has an assigned mnemonic"));
            assert!(seen.insert(ch), "Mnemonic '{ch}' for '{title}' must be unique across the menu set");
        }
        assert_eq!(fr_mnemonics[0], Some('F'), "Fichier retains 'F'");
        assert_ne!(fr_mnemonics[4], Some('F'), "Fenêtre does not collide on 'F'");
    }

    #[test]
    fn item_mnemonics_are_assigned_uniquely_within_a_submenu() {
        let labels = ["Save", "Save As", "Sign", "Exit"];
        let preferred = [Some('S'), Some('A'), Some('S'), Some('X')];
        let mnemonics = assign_mnemonics(&labels, &preferred);
        let mut seen = std::collections::HashSet::new();
        for (label, m) in labels.iter().zip(mnemonics.iter()) {
            let ch = m.unwrap_or_else(|| panic!("{label} has an assigned mnemonic"));
            assert!(seen.insert(ch), "Mnemonic '{ch}' for '{label}' must be unique within the submenu");
        }
        assert_eq!(mnemonics[0], Some('S'));
        assert_eq!(mnemonics[1], Some('A'));
        assert_eq!(mnemonics[3], Some('X'));
    }

    #[test]
    fn clipped_submenu_row_does_not_open_on_hover_unless_already_open() {
        let ctx = egui::Context::default();
        let submenu = Node::submenu("Sub", vec![Item::new(CommandRef::new("cmd.test"), "Child Item").into()]);
        let nodes = vec![submenu];
        let gate = Gate::default();

        // Pass 1: Pointer hover over the row's rectangle (y=150), but clip rect is y in 0..100.
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::PointerMoved(egui::pos2(50.0, 150.0)));
        let out = ctx.run_ui(raw, |ui| {
            ui.set_clip_rect(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0)));
            ui.add_space(130.0);
            let mut chosen = Vec::new();
            render_nodes(ui, &nodes, &gate, &mut chosen, &mut None);
        });
        out.drop_without_applying_deltas();

        // The submenu must NOT have opened because the row was clipped out of view.
        let mut open_item = None;
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_clip_rect(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0)));
            ui.add_space(130.0);
            open_item = egui::containers::menu::MenuState::from_ui(ui, |state, _| state.open_item);
            let mut chosen = Vec::new();
            render_nodes(ui, &nodes, &gate, &mut chosen, &mut None);
        });
        out.drop_without_applying_deltas();
        assert_eq!(open_item, None, "Clipped submenu must not open on hover");

        // Pass 2: Keyboard navigation opens the submenu (mnemonic 'S' for Sub) even when clipped.
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_clip_rect(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0)));
            ui.add_space(130.0);
            let mut chosen = Vec::new();
            render_nodes(ui, &nodes, &gate, &mut chosen, &mut Some(('S', egui::Key::S)));
        });
        out.drop_without_applying_deltas();

        let mut open_after_key = None;
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_clip_rect(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0)));
            ui.add_space(130.0);
            let mut chosen = Vec::new();
            render_nodes(ui, &nodes, &gate, &mut chosen, &mut None);
            open_after_key = egui::containers::menu::MenuState::from_ui(ui, |state, _| state.open_item);
        });
        out.drop_without_applying_deltas();
        assert!(open_after_key.is_some(), "Keyboard-opened submenu must remain open and render even when clipped");
    }
}
