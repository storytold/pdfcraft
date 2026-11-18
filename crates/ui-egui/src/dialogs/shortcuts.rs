//! Help ▸ Keyboard shortcuts: the framework's shortcuts dialog, read-only, over the catalog the keys run on and the
//! keys the document view handles itself.

use craft::keymap::{Keymap, Platform, Scopes};
use craft::menu::shortcuts::{DialogState, FixedRow, Options, ShortcutsDialog};

use crate::PdfCraftApp;
use crate::commands::{
    ACTUAL_SIZE, COPY, FIND_NEXT, FIND_PREV, FIRST_PAGE, FIT_WIDTH, LAST_PAGE, PAGE_LEVEL, PAGE_NEXT, PAGE_PREV, ROTATE_CCW, ROTATE_CW, SELECT_ALL,
    ZOOM_IN, ZOOM_OUT,
};

/// The open shortcuts window: its state, the keymap it lists and how it behaves.
pub(crate) struct ShortcutsSession {
    state: DialogState,
    keymap: Keymap,
    options: Options,
}

/// The keys the document view handles itself, which no catalog command binds.
fn fixed_rows(ctx: &egui::Context) -> Vec<FixedRow> {
    let key = |s| crate::commands::shortcut_label(ctx, s);
    let pair = |a, b| format!("{} / {}", key(a), key(b));
    // Key names, translated where a catalog has them (not scanned as UI literals).
    let named = |k: &str| tl!(k).to_string();
    // Alt+← / Alt+→ (`canvas::shortcuts`): not a registered command, which has no Alt.
    let alt_arrows = if crate::commands::mac_shortcuts(ctx) {
        "⌥← / ⌥→".to_owned()
    } else {
        let alt = crate::i18n::key_name("Alt");
        format!("{alt}+← / {alt}+→")
    };
    let scroll = crate::i18n::fmt(tl!("Zoom in / out (also pinch or {key}-scroll)"), &[("key", crate::commands::command_modifier_label(ctx))]);
    let first_last = format!("{} / {}, {}", crate::i18n::key_name("Home"), crate::i18n::key_name("End"), pair(FIRST_PAGE, LAST_PAGE));
    let group = tl!("Document view");
    // PLAN: `.chord(Trigger)` where a catalog binding could clash needs the engine `Shortcut` to `Trigger` conversion (pdf-chrome-w9's catalog)
    [
        (pair(FIND_NEXT, FIND_PREV), tl!("Next / previous match").to_string()),
        (key(COPY), tl!("Copy selected text").to_string()),
        (named("Double-click"), tl!("Select a word").to_string()),
        (crate::i18n::key_name("Esc").to_string(), tl!("Clear selection / close find").to_string()),
        (key(ACTUAL_SIZE), tl!("Actual size").to_string()),
        (key(PAGE_LEVEL), tl!("Zoom to page level").to_string()),
        (key(FIT_WIDTH), tl!("Fit to width").to_string()),
        (pair(ZOOM_IN, ZOOM_OUT), scroll),
        (pair(ROTATE_CW, ROTATE_CCW), tl!("Rotate view").to_string()),
        (first_last, tl!("First / last page").to_string()),
        (format!("← / →, {}", pair(PAGE_PREV, PAGE_NEXT)), tl!("Previous / next page").to_string()),
        (alt_arrows, format!("{} / {}", tl!("Previous view"), tl!("Next view"))),
        (named("V"), tl!("Select (V)").to_string()),
        (named("H / Space (hold)"), tl!("Hand (H)").to_string()),
        (crate::i18n::key_name("Delete").to_string(), tl!("Delete selected pages (Organize)").to_string()),
        (key(SELECT_ALL), tl!("Select all pages (Organize)").to_string()),
    ]
    .into_iter()
    .map(|(keys, label)| FixedRow::new(group, label, keys))
    .collect()
}

impl PdfCraftApp {
    /// Opens the shortcuts window (Help ▸ Keyboard shortcuts, `help.shortcuts`). `ctx` words the fixed rows' keys.
    // PLAN: the sheet gives `open_shortcuts()`; the fixed rows' key names need the context, so callers pass it (pdf-hub-w3, pdf-chrome-w10)
    pub(crate) fn open_shortcuts(&mut self, ctx: &egui::Context) {
        let mut options = Options::default();
        options.editable = false;
        options.fixed = fixed_rows(ctx);
        // PLAN: framework gap: `AppCatalog::defs` is `pub(crate)`, so the catalog's rows and scopes cannot reach `Keymap::new`; the dialog lists the fixed rows only until they are public
        let keymap = Keymap::new(Vec::new(), Scopes::new(), Platform::current());
        self.shortcuts = Some(ShortcutsSession { state: DialogState::new(), keymap, options });
    }

    /// Shows the shortcuts window for one pass; closing it drops the session.
    pub(crate) fn show_shortcuts(&mut self, ui: &mut egui::Ui) {
        let Some(session) = self.shortcuts.as_mut() else {
            return;
        };
        let ShortcutsSession { state, keymap, options } = session;
        ShortcutsDialog::new(keymap).options(options).show(ui, state);
        if !state.is_open() {
            self.shortcuts = None;
        }
    }
}
