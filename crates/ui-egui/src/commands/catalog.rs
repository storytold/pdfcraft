//! The app's command catalog and its per-frame state: one [`CommandDef`] for every registry command and
//! menu-only id, and one [`CommandState`] row per command, so the menus, the palette and the shortcuts read one
//! enabled, checked and hint rule.

use std::sync::OnceLock;

use craft::keymap::{Key, Mods};
use craft::menu::Chord;
use craft::ui::runtime::commands::{
    AppCatalog, Binding, CommandDef, CommandDefError, CommandGate, CommandInput, CommandKind, CommandRef, CommandState, CommandStates, ModalRule,
    Scope,
};
use pdfcraft_engine::commands::{self as engine, CommandSpec};

use crate::menus::{self, CUT, DELETE, DOCUMENT, EXTRA, NO_RECENT, PANEL, PANELS, PASTE, QUIT, SELECT_ALL};
use crate::{Mode, PdfCraftApp, QuickTool};
use craft::pages::PageDisplay;
use craft::theme::ModeChoice;

/// Commands only the catalog knows: id, English label and icon. Their handlers are the board's.
const CATALOG_ONLY: &[(&str, &str, &str)] = &[
    ("app.theme", "Switch theme", "moon"),
    ("home.unpin_tool", "Unpin tool", "pin-off"),
    ("form.calculation_order.back", "Back to Prepare a form", "chevron-left"),
    // Labels distinct from the Combine grid's Expand and Collapse, which can show beside them.
    ("tools.expand", "Expand all tools", "maximize-2"),
    ("tools.collapse", "Collapse all tools", "minimize-2"),
    ("tools.all", "Back to all tools", "layout-grid"),
    // The rail's button names the side it moves the rail to.
    ("view.rail.left", "Dock navigation rail on Left", "panel-left"),
    ("view.rail.right", "Dock navigation rail on Right", "panel-right"),
    ("view.fit_toggle", "Toggle fit", "scan"),
    ("bookmark.paste", "Paste bookmarks", "clipboard-paste"),
    ("bookmark.expand_all", "Expand all bookmarks", "chevrons-down"),
    ("bookmark.collapse_all", "Collapse all bookmarks", "chevrons-up"),
    ("bookmark.expand_level", "Expand top-level bookmarks", "chevron-down"),
    ("layers.reset_visibility", "Reset to Initial Visibility", "layers"),
    (PANEL, "Side panel", "panel-right"),
    (DOCUMENT, "Switch to document", "file"),
    ("tool.group", "Tool group", "wrench"),
];

/// Commands whose chords and rows are the framework's clipboard commands.
const FRAMEWORK_EDIT: &[&str] = &["edit.copy", "edit.paste"];

static CATALOG_WIN: OnceLock<Option<AppCatalog>> = OnceLock::new();
static CATALOG_MAC: OnceLock<Option<AppCatalog>> = OnceLock::new();

/// What a command is to the keymap: a toggle or a radio choice for the ones that carry a checked state.
fn kind_of(id: &str) -> CommandKind {
    match id {
        "view.layout.single" | "view.layout.two_up" => CommandKind::Radio { group: Some("layout".into()) },
        "view.theme.system" | "view.theme.light" | "view.theme.dark" => CommandKind::Radio { group: Some("theme".into()) },
        PANEL => CommandKind::Radio { group: Some("panel".into()) },
        "view.continuous" | "view.page_gaps" | "view.layout.cover" | "view.full_screen" | "view.read_mode" | "view.side_panel"
        | "view.tools_pane" | "view.split" => CommandKind::Toggle,
        _ => CommandKind::Command,
    }
}

/// The catalog's rows, for the chords of macOS (`mac`) or of the other platforms. A row an earlier one already
/// has the id of is left out, so the catalog never refuses two defs of one id.
pub(crate) fn catalog(mac: bool) -> Result<AppCatalog, CommandDefError> {
    let mut defs: Vec<CommandDef> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut push = |def: CommandDef, chords: Vec<Chord>| {
        if !seen.insert(def.command.id.clone()) {
            return;
        }
        let def = if def.kind == CommandKind::Command {
            let kind = kind_of(&def.command.id);
            def.with_kind(kind)
        } else {
            def
        };
        defs.push(chords.into_iter().fold(def, |def, chord| def.with_default(Binding::new(chord))));
    };
    for spec in engine::COMMANDS.iter().filter(|s| !FRAMEWORK_EDIT.contains(&s.id)) {
        let mut def = CommandDef::new(CommandRef::new(spec.id), spec.label, Scope::app());
        if !spec.icon.is_empty() {
            def = def.with_icon(spec.icon);
        }
        let mut chords: Vec<Chord> = spec.shortcut_for(mac).and_then(menus::chord).into_iter().collect();
        chords.extend(second_chords(spec.id));
        push(def, chords);
    }
    for (id, label, shortcut) in EXTRA.iter().filter(|(id, ..)| !FRAMEWORK_EDIT.contains(id)) {
        push(CommandDef::new(CommandRef::new(*id), *label, Scope::app()), shortcut.and_then(menus::chord).into_iter().collect());
    }
    for (id, label, icon) in CATALOG_ONLY {
        let mut def = CommandDef::new(CommandRef::new(*id), *label, Scope::app()).with_icon(*icon);
        if *id == DOCUMENT {
            // Window ▸ switching tabs stays available under the full-screen prompt (gated still blocks it under other modals).
            def = def.with_modal(ModalRule::Always);
        }
        push(def, Vec::new());
    }
    // The platform catalogs have different bindings, so switching between them rebuilds the keymap.
    AppCatalog::new(defs, u64::from(mac))
}

/// The chords a registry command takes beside its own shortcut: Alt+← / Alt+→ (⌥ on macOS) go back and forward
/// through the view history, as in Acrobat and browsers (#3632), and ⌘Page Up / ⌘Page Down turn a page as ⌘← / ⌘→ do.
fn second_chords(id: &str) -> Vec<Chord> {
    let alt = Mods { alt: true, ..Mods::default() };
    let cmd = Mods { cmd: true, ..Mods::default() };
    match id {
        "view.previous_view" => vec![Chord::new(alt, Key::Left)],
        "view.next_view" => vec![Chord::new(alt, Key::Right)],
        "view.previous_page" => vec![Chord::new(cmd, Key::PageUp)],
        "view.next_page" => vec![Chord::new(cmd, Key::PageDown)],
        _ => Vec::new(),
    }
}

/// The catalog for this platform's chords, built once.
pub(crate) fn shared_catalog(mac: bool) -> Option<AppCatalog> {
    let cell = if mac { &CATALOG_MAC } else { &CATALOG_WIN };
    cell.get_or_init(|| catalog(mac).ok()).clone()
}

/// The command state for this frame: the gate, the catalog and a state row for every command. The rows go into the
/// app's tracked states (`PdfCraftApp::command_states`) only when one changed, so the runtime reads them again only then.
pub(crate) fn command_input(app: &mut PdfCraftApp, ctx: &egui::Context) -> CommandInput {
    let gated = menus::gated(app);
    let mut input = CommandInput::new().gate(CommandGate::new(gated, |gated, _| !*gated));
    if let Some(catalog) = shared_catalog(super::mac_shortcuts(ctx)) {
        input = input.catalog(catalog);
    }
    let mut commands: Vec<CommandRef> = engine::COMMANDS.iter().map(|s| CommandRef::new(s.id)).collect();
    commands.extend(EXTRA.iter().map(|(id, ..)| CommandRef::new(*id)));
    commands.extend((0..app.recent.len()).map(|index| CommandRef::with_params("file.open_recent", serde_json::json!({ "index": index }))));
    commands.extend((0..app.views.len()).map(|index| CommandRef::with_params(DOCUMENT, serde_json::json!({ "index": index }))));
    commands.extend(PANELS.iter().map(|(_, name, _)| CommandRef::with_params(PANEL, serde_json::json!({ "panel": name }))));
    let mut states = CommandStates::new();
    for command in commands {
        let state = state_of(app, ctx, &command, gated);
        states.set(command, state);
    }
    if *app.command_states != states {
        *app.command_states = states;
    }
    input.states(&app.command_states)
}

fn state_of(app: &PdfCraftApp, ctx: &egui::Context, command: &CommandRef, gated: bool) -> CommandState {
    let enabled = enabled(app, ctx, command);
    let mut state = CommandState::new().enabled(enabled);
    if let Some(checked) = checked(app, command) {
        state = state.checked(checked);
    }
    if !enabled
        && !gated
        && let Some(spec) = engine::command(&command.id)
    {
        state = state.hint(app.disabled_reason(spec));
    }
    if menus::panel(command).is_some_and(|panel| has_content(app, panel)) {
        state = state.marked(true);
    }
    state
}

/// Whether a side panel has something to show in the active document.
fn has_content(app: &PdfCraftApp, panel: crate::RightPanel) -> bool {
    let Some((_, id)) = app.active_ids() else { return false };
    let Some(doc) = app.session.get(id) else { return false };
    match panel {
        crate::RightPanel::Bookmarks => !doc.info.outline.is_empty(),
        crate::RightPanel::Layers => !doc.info.layers.is_empty(),
        _ => false,
    }
}

/// Whether a command can run now.
fn enabled(app: &PdfCraftApp, ctx: &egui::Context, command: &CommandRef) -> bool {
    let view = app.active.and_then(|i| app.views.get(i));
    let organize = view.is_some_and(|v| v.organize);
    let has_doc = view.is_some();
    let enabled_id = |id: &str| engine::command(id).is_some_and(|s: &CommandSpec| app.command_enabled(s));
    let index = menus::param_index(command);
    // Edit rows stay on while a text field has the keyboard.
    let text = menus::text_editing(ctx) && matches!(command.id.as_str(), "edit.undo" | "edit.redo" | "edit.copy" | CUT | PASTE | SELECT_ALL);
    // Objects selected together in Edit a PDF are what Copy and Cut take, and Paste brings back
    // objects copied there.
    let edit_objects = app.quick_tool == QuickTool::EditText && !organize;
    let doc = app.active_ids().and_then(|(_, id)| app.session.get(id));
    text || match command.id.as_str() {
        QUIT => true,
        NO_RECENT => false,
        "file.open_recent" => index.is_none_or(|i| i < app.recent.len()),
        "file.clear_recent" => !app.recent.is_empty(),
        DOCUMENT => index.is_some_and(|i| i < app.views.len()),
        // macOS ⌘W closes the window when no document is open (#869).
        "file.close" if !has_doc && super::mac_shortcuts(ctx) && !cfg!(target_arch = "wasm32") => true,
        "edit.copy" if organize => enabled_id("page.copy"),
        "edit.copy" if view.is_some_and(|v| v.session.picked_image.is_some()) => doc.is_some_and(|d| d.allows_copying()),
        "edit.copy" if app.is_preparing() && view.is_some_and(|v| v.session.comments.primary().is_some() && v.selected_text().is_none_or(|t| t.is_empty())) => false,
        "edit.copy" | CUT if edit_objects && view.is_some_and(|v| v.session.image_selection.is_some()) => {
            doc.is_some_and(|d| d.allows_copying() && (command.id != CUT || d.allows_modification()))
        }
        "edit.copy" | CUT if edit_objects && view.is_some_and(|v| v.session.objects.count() > 0) => {
            doc.is_some_and(|d| d.allows_copying() && (command.id != CUT || d.allows_modification()))
        }
        PASTE if edit_objects && app.object_clipboard.is_some() => doc.is_some_and(pdfcraft_engine::Document::allows_modification),
        CUT => organize && enabled_id("page.cut"),
        PASTE if organize => app.page_clipboard.is_some() && enabled_id("page.paste"),
        // On the pages Paste takes what the clipboard holds (a picture, comments), which only the paste itself reads:
        // ⌘V and the platform's delivered paste are this command.
        PASTE => has_doc,
        SELECT_ALL | DELETE => has_doc || app.combine_showing() && !app.combine_draft.is_empty(),
        id if id.starts_with("view.zoom.") || id.starts_with("view.rotate.") => has_doc,
        PANEL => menus::panel(command).is_some(),
        id => enabled_id(id),
    }
}

/// The checked or selected state of a toggle or radio command; `None` for any other.
fn checked(app: &PdfCraftApp, command: &CommandRef) -> Option<bool> {
    let view = app.active.and_then(|i| app.views.get(i)).map(|v| &v.view);
    Some(match command.id.as_str() {
        "view.layout.single" => view.is_some_and(|v| v.display == PageDisplay::Single),
        "view.layout.two_up" => view.is_some_and(|v| v.display == PageDisplay::TwoUp),
        "view.continuous" => view.is_some_and(|v| v.continuous),
        "view.page_gaps" => view.is_some_and(|v| v.page_gaps),
        "view.layout.cover" => view.is_some_and(|v| v.cover),
        "view.full_screen" => app.full_screen(),
        "view.read_mode" => app.mode == Mode::Read,
        "view.side_panel" => app.right().is_some(),
        "view.tools_pane" => app.left_open(),
        "view.split" => app.panes.split.is_some(),
        "view.theme.system" => app.theme_preference == ModeChoice::System,
        "view.theme.light" => app.theme_preference == ModeChoice::Light,
        "view.theme.dark" => app.theme_preference == ModeChoice::Dark,
        PANEL => menus::panel(command).is_some_and(|p| app.right() == Some(p)),
        _ => return None,
    })
}
