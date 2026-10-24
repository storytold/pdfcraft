//! The macOS menu bar as data: which items it has, when each is enabled or checked, and what
//! choosing one does. No toolkit here, so every platform's tests check it; `native_menu` (macOS
//! only) shows it with muda.
//!
//! The commands are the registry's (`pdfcraft_engine::commands`), the same ones the in-window
//! Menu shows, with live labels and enablement. Items show the registry's shortcuts as key
//! equivalents. AppKit takes such a key before the window sees it, so a key equivalent is handed
//! back to egui as the very key press it was: the keyboard keeps going through egui exactly as
//! before (text fields, form fields and the ⌘W rule in `registry_shortcuts` included), and a
//! menu never takes a key away from the document. Only a click runs the command directly.

use pdfcraft_engine::commands::{self, CommandSpec, Shortcut};

use crate::canvas::{Fit, PageLayout};
use crate::theme::ThemePreference;
use crate::{Dialog, PdfCraftApp, RightPanel, i18n};

/// What an item does when chosen.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    /// Run a registered command, as the in-window menu does.
    Command(&'static str),
    /// Open a recent file by path.
    Recent(String),
    /// Feed egui an input event, as the keyboard would.
    Input(InputKind),
    /// A document-view action the in-window View menu performs directly.
    View(ViewAction),
    Panel(RightPanel),
    /// View ▸ Display theme ▸ Appearance settings…
    Appearance,
    /// A registry shortcut pressed while its item was the key equivalent: fed to egui as that key.
    Key(Shortcut),
    /// PdfCraft ▸ Quit: closes the window the way its close button does, so unsaved changes are
    /// asked about (#1136). AppKit's own Quit (`terminate:`) would end the process at once.
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum InputKind {
    Cut,
    Copy,
    Paste,
    SelectAll,
    /// ⌘Z / ⇧⌘Z in a focused text field (the field keeps its own undo history).
    TextUndo,
    TextRedo,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ViewAction {
    ActualSize,
    FitPage,
    FitWidth,
    FitHeight,
    ZoomIn,
    ZoomOut,
    RotateCw,
    RotateCcw,
    PrevPage,
    NextPage,
    FirstPage,
    LastPage,
}

/// When an item is enabled.
#[derive(Clone, Copy, Debug)]
pub(crate) enum When {
    Always,
    Command(&'static CommandSpec),
    Document,
    /// Undo / Redo: a focused text field's own history, otherwise the command's.
    TextOr(&'static CommandSpec),
    /// File ▸ Close: the document, or with none open the window (macOS ⌘W, #869).
    CloseOrWindow(&'static CommandSpec),
}

/// What an item's check mark follows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Check {
    Layout(PageLayout),
    Continuous,
    Cover,
    Theme(ThemePreference),
    Panel(RightPanel),
}

#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub label: String,
    pub action: Action,
    /// Shown as the key equivalent.
    pub shortcut: Option<Shortcut>,
    pub when: When,
    pub check: Option<Check>,
}

impl Item {
    fn new(label: &str, action: Action, when: When) -> Self {
        Self { label: label.to_owned(), action, shortcut: None, when, check: None }
    }

    fn keyed(mut self, shortcut: Shortcut) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    fn checked(mut self, check: Check) -> Self {
        self.check = Some(check);
        self
    }

    /// A registry command, with its translated label and its shortcut.
    fn command(spec: &'static CommandSpec) -> Self {
        let when = match spec.id {
            "edit.undo" | "edit.redo" => When::TextOr(spec),
            "file.close" => When::CloseOrWindow(spec),
            _ => When::Command(spec),
        };
        let item = Self::new(&i18n::menu_label(spec.id, spec.label), Action::Command(spec.id), when);
        match spec.shortcut {
            Some(s) => item.keyed(s),
            None => item,
        }
    }

    /// What its key equivalent does: the key itself, replayed into egui, except where a click
    /// already is what the key makes (the clipboard events, Quit).
    pub(crate) fn key_action(&self) -> Option<Action> {
        let s = self.shortcut?;
        Some(match self.action {
            Action::Input(_) | Action::Quit => self.action.clone(),
            _ => Action::Key(s),
        })
    }
}

/// Items AppKit provides (its own actions and wording).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum System {
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
    FullScreen,
    BringAllToFront,
}

#[derive(Clone, Debug)]
pub(crate) enum Entry {
    Item(Item),
    Separator,
    Submenu(String, Vec<Entry>),
    /// File ▸ Open Recent: filled from the recent files as they change.
    OpenRecent(String),
    System(System, String),
}

/// A top-level menu. AppKit treats the Window and Help menus specially (the window list, the
/// Help search field).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Role {
    App,
    Plain,
    Window,
    Help,
}

#[derive(Clone, Debug)]
pub(crate) struct Menu {
    pub title: String,
    pub role: Role,
    pub entries: Vec<Entry>,
}

/// Registry commands the bar shows somewhere other than their registry menu, or not at all
/// because macOS puts them elsewhere (Copy is Edit's clipboard Copy, which copies page text too).
const MOVED: [&str; 6] = ["app.preferences", "help.about", "help.check_updates", "view.focus_page_input", "file.open_recent", "edit.copy"];

/// The side panels, in the in-window View ▸ Side panels order.
pub(crate) const PANELS: [(RightPanel, &str); 11] = [
    (RightPanel::Comments, "Comments"),
    (RightPanel::Bookmarks, "Bookmarks"),
    (RightPanel::Pages, "Pages"),
    (RightPanel::Fields, "Fields"),
    (RightPanel::Layers, "Layers"),
    (RightPanel::Attachments, "Attachments"),
    (RightPanel::Signatures, "Signatures"),
    (RightPanel::Accessibility, "Accessibility Checker"),
    (RightPanel::Tags, "Tags"),
    (RightPanel::Search, "Search"),
    (RightPanel::Compare, "Compare"),
];

/// Commands split where the id's group (`file.`, `export.`…) changes. Runs of one-command groups
/// share a section, so a menu doesn't wall single items off between separators.
pub(crate) fn sections<'a>(specs: impl IntoIterator<Item = &'a CommandSpec>) -> Vec<Vec<&'a CommandSpec>> {
    let mut groups: Vec<Vec<&CommandSpec>> = Vec::new();
    for spec in specs {
        let g = spec.id.split('.').next().unwrap_or(spec.id);
        match groups.last_mut() {
            Some(last) if last.first().is_some_and(|p| p.id.split('.').next().unwrap_or(p.id) == g) => last.push(spec),
            _ => groups.push(vec![spec]),
        }
    }
    let mut out: Vec<Vec<&CommandSpec>> = Vec::new();
    let mut singles = false;
    for group in groups {
        let single = group.len() == 1;
        match out.last_mut() {
            Some(last) if single && singles => last.extend(group),
            _ => out.push(group),
        }
        singles = single;
    }
    out
}

/// A registry menu's commands in sections, leaving out the moved ones. `after` puts extra
/// entries right after the command it names.
fn registry(name: &str, mut after: impl FnMut(&str) -> Vec<Entry>) -> Vec<Entry> {
    let mut out = Vec::new();
    for (i, section) in sections(commands::menu(name).filter(|s| !MOVED.contains(&s.id))).into_iter().enumerate() {
        if i > 0 {
            out.push(Entry::Separator);
        }
        for spec in section {
            out.push(Entry::Item(Item::command(spec)));
            out.extend(after(spec.id));
        }
    }
    out
}

fn registered(id: &str) -> Option<Item> {
    commands::command(id).map(Item::command)
}

/// The whole bar in the current interface language: PdfCraft, File, Edit, View, Go, Pages, Window
/// and Help.
pub(crate) fn bar() -> Vec<Menu> {
    let item = |label: &str, action: Action, when: When| Entry::Item(Item::new(label, action, when));
    let system = |s: System, label: &str| Entry::System(s, label.to_owned());

    // PdfCraft
    let mut app = vec![item(&i18n::menu_label("help.about", "About PdfCraft"), Action::Command("help.about"), When::Always)];
    if let Some(updates) = registered("help.check_updates") {
        app.push(Entry::Item(updates));
    }
    app.push(Entry::Separator);
    if let Some(prefs) = registered("app.preferences") {
        app.extend([Entry::Item(prefs), Entry::Separator]);
    }
    app.extend([
        system(System::Services, tl!("Services")),
        Entry::Separator,
        system(System::Hide, tl!("Hide PdfCraft")),
        system(System::HideOthers, tl!("Hide Others")),
        system(System::ShowAll, tl!("Show All")),
        Entry::Separator,
        Entry::Item(Item::new(tl!("Quit PdfCraft"), Action::Quit, When::Always).keyed(Shortcut::cmd("Q"))),
    ]);

    // File, with Open Recent after Open…
    let recent = i18n::menu_label("file.open_recent", "Open Recent");
    let file = registry("File", |id| if id == "file.open" { vec![Entry::OpenRecent(recent.clone())] } else { Vec::new() });

    // Edit: Undo / Redo, the clipboard, then the rest.
    let mut clipboard = vec![Entry::Separator];
    for (label, kind, key) in [
        (tl!("Cut"), InputKind::Cut, "X"),
        (tl!("Copy"), InputKind::Copy, "C"),
        (tl!("Paste"), InputKind::Paste, "V"),
        (tl!("Select All"), InputKind::SelectAll, "A"),
    ] {
        clipboard.push(Entry::Item(Item::new(label, Action::Input(kind), When::Always).keyed(Shortcut::cmd(key))));
    }
    clipboard.push(Entry::Separator);
    let edit = registry("Edit", |id| if id == "edit.redo" { std::mem::take(&mut clipboard) } else { Vec::new() });

    // View: zoom, rotation, page display, the registry's View commands, theme and side panels.
    use crate::commands::{ACTUAL_SIZE, FIT_WIDTH, PAGE_LEVEL, ROTATE_CCW, ROTATE_CW, ZOOM_IN, ZOOM_OUT};
    let view_item = |label: &str, a: ViewAction, key: Option<Shortcut>| {
        let it = Item::new(label, Action::View(a), When::Document);
        Entry::Item(match key {
            Some(k) => it.keyed(k),
            None => it,
        })
    };
    let mut view = vec![
        view_item(tl!("Actual size"), ViewAction::ActualSize, Some(ACTUAL_SIZE)),
        view_item(tl!("Zoom to page level"), ViewAction::FitPage, Some(PAGE_LEVEL)),
        view_item(tl!("Fit to width"), ViewAction::FitWidth, Some(FIT_WIDTH)),
        view_item(tl!("Fit to height"), ViewAction::FitHeight, None),
    ];
    view.extend(registered("view.fit_visible").map(Entry::Item));
    view.extend([
        view_item(tl!("Zoom in"), ViewAction::ZoomIn, Some(ZOOM_IN)),
        view_item(tl!("Zoom out"), ViewAction::ZoomOut, Some(ZOOM_OUT)),
        Entry::Separator,
        view_item(tl!("Rotate view clockwise"), ViewAction::RotateCw, Some(ROTATE_CW)),
        view_item(tl!("Rotate view counterclockwise"), ViewAction::RotateCcw, Some(ROTATE_CCW)),
        Entry::Separator,
    ]);
    for l in PageLayout::ORDER {
        view.extend(registered(l.command()).map(|mut it| {
            it.label = tl!(l.label()).to_owned();
            Entry::Item(it.checked(Check::Layout(l)))
        }));
    }
    view.extend(registered("view.continuous").map(|it| Entry::Item(it.checked(Check::Continuous))));
    view.extend(registered("view.layout.cover").map(|it| Entry::Item(it.checked(Check::Cover))));
    view.push(Entry::Separator);
    view.extend(registry("View", |_| Vec::new()));
    view.push(Entry::Separator);
    let mut theme = Vec::new();
    for (preference, id, label) in [
        (ThemePreference::System, "view.theme.system", tl!("Use system setting")),
        (ThemePreference::Light, "view.theme.light", tl!("Light gray")),
        (ThemePreference::Dark, "view.theme.dark", tl!("Dark gray")),
    ] {
        theme.push(Entry::Item(Item::new(label, Action::Command(id), When::Always).checked(Check::Theme(preference))));
    }
    theme.extend([Entry::Separator, item(tl!("Appearance settings…"), Action::Appearance, When::Always)]);
    view.push(Entry::Submenu(tl!("Display theme").to_owned(), theme));
    let panels = PANELS.iter().map(|&(p, label)| Entry::Item(Item::new(tl!(label), Action::Panel(p), When::Always).checked(Check::Panel(p))));
    view.push(Entry::Submenu(tl!("Side panels").to_owned(), panels.collect()));

    // Go
    use crate::commands::{PAGE_NEXT, PAGE_PREV};
    let mut go = vec![
        view_item(tl!("Previous page"), ViewAction::PrevPage, Some(PAGE_PREV)),
        view_item(tl!("Next page"), ViewAction::NextPage, Some(PAGE_NEXT)),
        view_item(tl!("First page"), ViewAction::FirstPage, None),
        view_item(tl!("Last page"), ViewAction::LastPage, None),
    ];
    go.extend(registered("view.focus_page_input").map(Entry::Item));
    go.push(Entry::Separator);
    go.extend(["view.previous_view", "view.next_view"].into_iter().filter_map(registered).map(Entry::Item));

    // Window
    let window = vec![
        system(System::Minimize, tl!("Minimize")),
        system(System::Zoom, tl!("Zoom")),
        Entry::Separator,
        system(System::FullScreen, tl!("Enter Full Screen")),
        Entry::Separator,
        system(System::BringAllToFront, tl!("Bring All to Front")),
    ];

    let menu = |title: &str, role: Role, entries: Vec<Entry>| Menu { title: title.to_owned(), role, entries };
    vec![
        menu("PdfCraft", Role::App, app),
        menu(tl!("File"), Role::Plain, file),
        menu(tl!("Edit"), Role::Plain, edit),
        menu(tl!("View"), Role::Plain, view),
        menu(tl!("Go"), Role::Plain, go),
        menu(tl!("Pages"), Role::Plain, registry("Pages", |_| Vec::new())),
        menu(tl!("Window"), Role::Window, window),
        menu(tl!("Help"), Role::Help, registry("Help", |_| Vec::new())),
    ]
}

/// What stays in the system bar when Preferences put the menus in the window: the app menu, so
/// ⌘Q still asks about unsaved changes (#1136), and the Window menu, which the window has no copy of.
pub(crate) fn system_only(menus: Vec<Menu>) -> Vec<Menu> {
    menus.into_iter().filter(|m| matches!(m.role, Role::App | Role::Window)).collect()
}

/// Every item of `entries`, submenus included, in order.
#[cfg(test)]
pub(crate) fn items(entries: &[Entry]) -> Vec<&Item> {
    let mut out = Vec::new();
    for e in entries {
        match e {
            Entry::Item(it) => out.push(it),
            Entry::Submenu(_, sub) => out.extend(items(sub)),
            _ => {}
        }
    }
    out
}

/// The name of the key (a `keyboard_types::Code`, as muda takes it) a shortcut's key equivalent
/// uses. `None` for keys that stay egui's alone (Delete, function keys…) and for bare keys,
/// which would fire while typing in a field.
pub(crate) fn key_code(s: Shortcut) -> Option<&'static str> {
    if !(s.command || s.mac_ctrl) {
        return None;
    }
    const LETTERS: [&str; 26] = [
        "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK", "KeyL", "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ",
        "KeyR", "KeyS", "KeyT", "KeyU", "KeyV", "KeyW", "KeyX", "KeyY", "KeyZ",
    ];
    const DIGITS: [&str; 10] = ["Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8", "Digit9"];
    Some(match s.key {
        "," => "Comma",
        "Tab" => "Tab",
        // ⌘+ is ⌘= on the keys (as in every Mac app); ⇧⌘+ stays apart as ⇧⌘=.
        "+" | "=" => "Equal",
        "−" | "-" => "Minus",
        "[" => "BracketLeft",
        "]" => "BracketRight",
        "←" => "ArrowLeft",
        "→" => "ArrowRight",
        k => {
            let mut chars = k.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return None };
            let c = c.to_ascii_uppercase();
            if c.is_ascii_uppercase() {
                LETTERS.get(usize::from(c as u8 - b'A')).copied()?
            } else if c.is_ascii_digit() {
                DIGITS.get(usize::from(c as u8 - b'0')).copied()?
            } else {
                return None;
            }
        }
    })
}

/// The egui key a shortcut's key name is.
fn egui_key(name: &str) -> Option<egui::Key> {
    match name {
        "←" => Some(egui::Key::ArrowLeft),
        "→" => Some(egui::Key::ArrowRight),
        n => egui::Key::from_name(n),
    }
}

/// The key press and release a registry shortcut is, as `registry_shortcuts` and the document
/// view match it.
pub(crate) fn shortcut_events(s: Shortcut) -> Vec<egui::Event> {
    let Some(key) = egui_key(s.key) else { return Vec::new() };
    let mut modifiers = egui::Modifiers::NONE;
    if s.command {
        modifiers |= egui::Modifiers::COMMAND;
    }
    if s.shift {
        modifiers |= egui::Modifiers::SHIFT;
    }
    if s.mac_ctrl {
        modifiers |= egui::Modifiers::CTRL;
    }
    [true, false].map(|pressed| egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers }).to_vec()
}

/// The input events the keyboard would have produced for `kind` (as egui-winit makes them).
/// `clipboard` is the system clipboard's text, for Paste.
pub(crate) fn input_events(kind: InputKind, clipboard: impl FnOnce() -> String) -> Vec<egui::Event> {
    let key = |key: egui::Key, modifiers: egui::Modifiers| {
        [true, false].map(|pressed| egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers })
    };
    match kind {
        InputKind::Cut => vec![egui::Event::Cut],
        InputKind::Copy => vec![egui::Event::Copy],
        InputKind::Paste => {
            let text = clipboard().replace("\r\n", "\n");
            if text.is_empty() { Vec::new() } else { vec![egui::Event::Paste(text)] }
        }
        InputKind::SelectAll => key(egui::Key::A, egui::Modifiers::COMMAND).to_vec(),
        InputKind::TextUndo => key(egui::Key::Z, egui::Modifiers::COMMAND).to_vec(),
        InputKind::TextRedo => key(egui::Key::Z, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT).to_vec(),
    }
}

/// An item's state right now.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ItemState {
    pub enabled: bool,
    pub checked: Option<bool>,
    /// The label to show, when it follows the app ("Undo Rotate pages", "Close Window").
    pub label: Option<String>,
}

impl PdfCraftApp {
    /// Whether the menus are in the macOS menu bar, so the window shows Home instead of its Menu
    /// button: the host has the bar, and Preferences didn't move the menus into the window.
    pub(crate) fn menus_in_system_bar(&self) -> bool {
        self.native_menu_bar && self.system_menu_bar
    }

    /// Whether the menu bar's commands must wait: a dialog, a prompt or the command palette has
    /// the keyboard, and nothing may act on the document underneath it.
    pub(crate) fn menu_blocked(&self) -> bool {
        self.modal_open() || self.palette_open
    }

    /// `item`'s enablement, check mark and live label. `text_focused`: a text field has the
    /// keyboard.
    pub(crate) fn menu_item_state(&self, item: &Item, text_focused: bool) -> ItemState {
        let active = self.active_ids().map(|(_, id)| id);
        let view = self.active.and_then(|i| self.views.get(i));
        // Only Undo and Redo name what they act on (`commands::current_edit`); the rest keep their
        // built label, so a frame allocates nothing for them.
        let live = |spec: &CommandSpec| Some(i18n::menu_label(spec.id, &commands::current_label(spec, &self.session, active)));
        let (enabled, label) = match item.when {
            When::Always => (true, None),
            When::Document => (view.is_some(), None),
            When::Command(spec) => (self.command_enabled(spec), None),
            // The field's own history: the plain label, as the document's edits aren't undone.
            When::TextOr(spec) if text_focused => (true, Some(i18n::menu_label(spec.id, spec.label))),
            When::TextOr(spec) => (self.command_enabled(spec), live(spec)),
            When::CloseOrWindow(_) if view.is_none() => (true, Some(tl!("Close Window").to_owned())),
            When::CloseOrWindow(spec) => (self.command_enabled(spec), None),
        };
        // Under a dialog only what edits its fields, and Quit, stay available.
        let enabled = if self.menu_blocked() {
            matches!(item.action, Action::Input(_) | Action::Quit) || (matches!(item.when, When::TextOr(_)) && text_focused)
        } else {
            enabled
        };
        let checked = item.check.map(|c| match c {
            Check::Layout(l) => view.is_some_and(|v| v.layout == l),
            Check::Continuous => view.is_some_and(|v| v.continuous()),
            Check::Cover => view.is_some_and(|v| v.cover_applies() && v.cover),
            Check::Theme(p) => self.theme_preference == p,
            Check::Panel(p) => self.right == Some(p),
        });
        ItemState { enabled, checked, label }
    }

    /// Undo / Redo go to a focused text field, as ⌘Z does there.
    pub(crate) fn menu_action_for_text(action: Action, text_focused: bool) -> Action {
        match action {
            Action::Command("edit.undo") if text_focused => Action::Input(InputKind::TextUndo),
            Action::Command("edit.redo") if text_focused => Action::Input(InputKind::TextRedo),
            a => a,
        }
    }

    /// Run one chosen item (everything but input events, which go into the frame's input). A
    /// click can come from a menu opened before a dialog was: it does nothing under one.
    pub(crate) fn run_menu_action(&mut self, action: Action) {
        if self.menu_blocked() && !matches!(action, Action::Quit) {
            return;
        }
        match action {
            Action::Command(id) => self.menu_command(id),
            Action::Recent(path) => self.open_recent(&path),
            Action::Panel(p) => self.choose_right_panel(Some(p)),
            Action::Appearance => self.dialog = Some(Dialog::Preferences),
            Action::View(a) => {
                let Some(v) = self.active.and_then(|i| self.views.get_mut(i)) else { return };
                match a {
                    ViewAction::ActualSize => v.set_zoom(1.0),
                    ViewAction::FitPage => v.fit = Fit::Page,
                    ViewAction::FitWidth => v.fit = Fit::Width,
                    ViewAction::FitHeight => {
                        v.fit = Fit::Height;
                        v.goto = Some((v.current, 0.0));
                    }
                    ViewAction::ZoomIn => v.zoom_step(true),
                    ViewAction::ZoomOut => v.zoom_step(false),
                    ViewAction::RotateCw => v.rotate_view(true),
                    ViewAction::RotateCcw => v.rotate_view(false),
                    ViewAction::PrevPage => v.step_page(false),
                    ViewAction::NextPage => v.step_page(true),
                    ViewAction::FirstPage => v.go_to_page(0),
                    ViewAction::LastPage => v.go_to_page(usize::MAX),
                }
            }
            Action::Quit => {
                if let Some(ctx) = &self.ctx {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Action::Input(_) | Action::Key(_) => {}
        }
    }

    /// A clicked command, with the keyboard's rules: while a form field is being typed in, it
    /// waits a frame so the field's text is in the document first (#166); File ▸ Close with no
    /// document closes the window (#869).
    fn menu_command(&mut self, id: &'static str) {
        let active = self.active_ids();
        if active.and_then(|(i, _)| self.views.get(i)).is_some_and(|v| v.forms.focus.is_some()) {
            self.deferred_commands.push((id, active.map(|(_, doc)| doc)));
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint();
            }
        } else if id == "file.close" && active.is_none() {
            if let Some(ctx) = &self.ctx {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        } else {
            self.execute(id);
        }
    }
}
