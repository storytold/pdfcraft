use std::collections::BTreeSet;

use craft::behavior::IconId;
use craft::theme::variant::{Tone, Variant};
use craft::ui::bars::{Bar, BarKind};
use craft::ui::buttons::{Button, IconButton, SplitButton};
use craft::ui::choice::Segmented;
use craft::ui::contract::{Bind, Entry, Props as _, Show as _, ShowContainer as _, Trigger};
use craft::ui::dialog::Prompt;
use craft::ui::display::Text;
use craft::ui::menu::Menu;
use craft::ui::notices::{Notice, NoticeAction};
use craft::ui::patterns::form_dialogs::{PasswordDialog, PasswordPurpose};
use craft::ui::popover::DropVeil;
use craft::ui::sections::{DialogRole, FooterButton, Panel};
use craft::ui::tile_grid::{ThumbnailSlider, TileZoom};

use super::files::{CombineFile, CombineKey, CombineView, Lock, RowAction};
use super::table::table;
use crate::PdfCraftApp;

/// How the Add files split button adds.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum AddHow {
    Files,
    Folder,
    Subfolders,
}

/// The Combine files page.
pub(crate) fn page(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    // Ranges are checked before anything below reads them.
    let checks: Vec<Result<usize, String>> = app.combine_draft.iter_mut().map(CombineFile::selection).collect();
    // Same-named form fields across the files are renamed when combining: preview it once per
    // list, so the Warnings column can say so before anything is combined.
    app.combine_conflicts.refresh(&app.session, &app.combine_draft);
    let twice = app.combine_tab.duplicate_warnings(&app.combine_draft);
    // A file stays expanded only while it can show as its pages (not locked again by undo, nor
    // edited down to one page or a bad range): it doesn't come back expanded by itself.
    {
        let tab = &mut app.combine_tab;
        let draft = &app.combine_draft;
        tab.expanded.retain(|id| draft.iter().find(|f| f.id == *id).is_some_and(|f| crate::combine_grid::pages_shown(f).is_some()));
    }
    // A page can only remain selected while its tile is visible.
    let shown_pages: BTreeSet<CombineKey> = if app.combine_view == CombineView::Grid {
        app.combine_draft
            .iter()
            .filter(|f| app.combine_tab.expanded.contains(&f.id))
            .flat_map(|f| {
                let count = crate::combine_grid::pages_shown(f).map_or(0, |pages| pages.order.len());
                (0..count).map(move |nth| CombineKey::Page(f.id, nth))
            })
            .collect()
    } else {
        BTreeSet::new()
    };
    let selection = &mut app.combine_tab.selection;
    let visible = |key: &CombineKey| !matches!(key, CombineKey::Page(..)) || shown_pages.contains(key);
    selection.items.retain(visible);
    selection.lead = selection.lead.filter(visible);
    selection.anchor = selection.anchor.filter(visible);
    let n = app.combine_draft.len();
    let selected: Vec<bool> = app.combine_draft.iter().map(|f| app.combine_tab.selection.contains(&CombineKey::File(f.id))).collect();
    let count = selected.iter().filter(|s| **s).count();
    let pages_picked = app.combine_tab.selection.items.iter().filter(|k| matches!(k, CombineKey::Page(..))).count();
    let blocker = blocker(&app.combine_draft, &checks);
    let grid = app.combine_view == CombineView::Grid;

    let mut action: Option<RowAction> = None;
    let mut grid_actions: Vec<RowAction> = Vec::new();
    let mut combine = false;
    let mut dropped = None;

    Panel::new(tl!("Combine files")).titled().fallback_collection(super::collection_id()).show(ui, |ui, _| {
        // Why it can't run is the button's hover text, as in production (no warning line above the grid).
        let why = blocker.clone().unwrap_or_default();
        let mut combine_button = Button::new(tl!("Combine")).variant(Variant::PRIMARY).icon(IconId::new("files")).enabled(blocker.is_none());
        if blocker.is_some() {
            combine_button = combine_button.tooltip(why);
        }
        if combine_button.show(ui).clicked() {
            combine = true;
        }
        let hint = if grid {
            tl!("Files are combined in the order shown. Drag them to change it.")
        } else {
            tl!("Files are combined from top to bottom. Leave Pages empty to take every page.")
        };
        Text::new(hint).show(ui);

        Bar::new(tl!("Combine"), BarKind::Tool).show(ui, |ui, _| {
            add_files(app, ui);
            add_open_documents(app, ui);
            // Moves need a selected row with room to move; the buttons stay put either way.
            let can_up = selected.iter().skip_while(|s| **s).any(|s| *s);
            let can_down = selected.iter().rev().skip_while(|s| **s).any(|s| *s);
            let remove = match (pages_picked, count) {
                (1, _) => tl!("Remove page").to_string(),
                (k, _) if k > 1 => crate::i18n::fmt(tl!("Remove {n} pages"), &[("n", &k.to_string())]),
                (_, 1) => {
                    let name = app.combine_draft.iter().zip(&selected).find(|(_, s)| **s).map_or("", |(f, _)| f.name.as_str());
                    crate::i18n::fmt(tl!("Remove {name}"), &[("name", name)])
                }
                (_, 0) => tl!("Remove").to_string(),
                (_, k) => crate::i18n::fmt(tl!("Remove {n} files"), &[("n", &k.to_string())]),
            };
            if IconButton::new(IconId::new("chevron-up")).label(tl!("Move up")).enabled(can_up).show(ui).clicked() {
                action = Some(RowAction::MoveUp);
            }
            if IconButton::new(IconId::new("chevron-down")).label(tl!("Move down")).enabled(can_down).show(ui).clicked() {
                action = Some(RowAction::MoveDown);
            }
            if IconButton::new(IconId::new("trash-2")).label(remove).enabled(count > 0 || pages_picked > 0).show(ui).clicked() {
                action = Some(RowAction::Remove);
            }
            let locked = app.combine_draft.iter().zip(&selected).any(|(f, s)| *s && f.lock.is_some());
            if Button::new(tl!("Unlock…")).icon(IconId::new("lock-open")).enabled(locked).show(ui).clicked() {
                action = Some(RowAction::Unlock(None));
            }
            IconButton::command(IconId::new("undo-2"), "edit.undo").enabled(app.combine_can_undo(true)).show(ui);
            IconButton::command(IconId::new("redo-2"), "edit.redo").enabled(app.combine_can_undo(false)).show(ui);
            let views = [
                Entry::new(CombineView::Grid, tl!("Grid view")).icon(IconId::new("layout-grid")),
                Entry::new(CombineView::List, tl!("List view")).icon(IconId::new("list")),
            ];
            Segmented::new(tl!("View"), Bind::bound(&mut app.combine_view), &views).show(ui);
            if count > 1 {
                Text::new(crate::i18n::fmt(tl!("{n} selected"), &[("n", &count.to_string())])).show(ui);
            }
            if grid && n > 0 {
                // Every file as its pages, or all as cards again.
                let can_expand = app.combine_draft.iter_mut().any(|f| !app.combine_tab.expanded.contains(&f.id) && expandable(f));
                if IconButton::new(IconId::new("maximize-2")).label(tl!("Expand all")).enabled(can_expand).show(ui).clicked() {
                    action = Some(RowAction::ExpandAll(true));
                }
                let can_collapse = !app.combine_tab.expanded.is_empty();
                if IconButton::new(IconId::new("minimize-2")).label(tl!("Collapse all")).enabled(can_collapse).show(ui).clicked() {
                    action = Some(RowAction::ExpandAll(false));
                }
                ThumbnailSlider::new(tl!("Card size"), Bind::bound(&mut app.combine_zoom), TileZoom::Ladder).show(ui);
            }
        });

        // The list; files dropped over the window join it.
        DropVeil::new(tl!("Drop to add the files")).show_with_files(ui, |ui, files| {
            if let Some(files) = files {
                dropped = Some(files.files.clone());
            }
            if n == 0 {
                // Choose the files, or on the desktop a folder of them.
                let mut empty = Notice::new(Tone::Info, tl!("Add the files to combine"))
                    .lines([tl!("Drop PDFs, images, text files or folders here, or choose them.")])
                    .action(NoticeAction::new(tl!("Add files…")).default_action());
                if cfg!(not(target_arch = "wasm32")) {
                    empty = empty.action(NoticeAction::new(tl!("Add folder…")));
                }
                match empty.show(ui).action {
                    Some(0) => app.combine_dialog(),
                    Some(1) => app.combine_add_folder(false),
                    _ => {}
                }
            } else if grid {
                grid_actions = crate::combine_grid::grid(app, ui, &checks, &twice);
                // PLAN: assumed `combine_grid::preview(app, ui)`, the sheet names it `card::preview` (pdf-workflows-w7)
                crate::combine_grid::preview(app, ui);
            } else {
                if let Some(next) = table(app, ui) {
                    action = Some(next);
                }
                crate::combine_grid::preview(app, ui);
            }
        });

        if n > 0 {
            let pages: usize = checks.iter().filter_map(|c| c.as_ref().ok()).sum();
            // Each file's size once, however many parts it is in.
            let mut counted = BTreeSet::new();
            let size: usize = app.combine_draft.iter().filter(|f| counted.insert(f.group)).map(|f| f.bytes.len()).sum();
            // A file split into parts counts once.
            let n = app.combine_draft.iter().map(|f| f.group).collect::<BTreeSet<u64>>().len();
            let files = if n == 1 { tl!("1 file").to_string() } else { crate::i18n::fmt(tl!("{n} files"), &[("n", &n.to_string())]) };
            let pages = if pages == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &pages.to_string())]) };
            Text::new(format!("{files} · {pages} · {}", crate::panels::human_size(size))).show(ui);
            let bad = app
                .combine_draft
                .iter()
                .zip(&checks)
                .filter(|(f, c)| f.problem.is_some() || c.is_err())
                .map(|(f, _)| f.group)
                .collect::<BTreeSet<u64>>()
                .len();
            if bad > 0 {
                let text = if bad == 1 {
                    tl!("1 file needs attention").to_string()
                } else {
                    crate::i18n::fmt(tl!("{n} files need attention"), &[("n", &bad.to_string())])
                };
                Text::new(text).tone(Tone::Bad).show(ui);
            }
        }
    });

    if let Some(files) = dropped {
        app.drop_into_combine(ui.ctx(), files);
    }
    unlock_dialog(app, ui);
    for action in action.into_iter().chain(grid_actions) {
        app.combine_apply(action);
    }
    if combine && blocker.is_none() {
        if app.combine_draft.iter().any(CombineFile::unreadable) {
            app.combine_tab.confirm_skip = true;
        } else {
            app.combine_staged(ui.ctx());
        }
    }
    skip_prompt(app, ui);
}

/// "Add files…" with a menu of the other ways to add: folders (desktop).
#[cfg(not(target_arch = "wasm32"))]
fn add_files(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let entries = [
        Entry::new(AddHow::Files, tl!("Add files…")).icon(IconId::new("file-plus")),
        Entry::new(AddHow::Folder, tl!("Add folder…")).icon(IconId::new("folder")),
        Entry::new(AddHow::Subfolders, tl!("Add folder and subfolders…")).icon(IconId::new("folder-open")),
    ];
    let out = SplitButton::new(tl!("Add files…"), &entries).arrow_label(tl!("More ways to add files")).icon(IconId::new("plus")).show(ui);
    if out.response.clicked() {
        app.combine_dialog();
    }
    match out.chosen {
        Some(AddHow::Files) => app.combine_dialog(),
        Some(AddHow::Folder) => app.combine_add_folder(false),
        Some(AddHow::Subfolders) => app.combine_add_folder(true),
        None => {}
    }
}

/// "Add files…" alone: a browser has no folders to add.
#[cfg(target_arch = "wasm32")]
fn add_files(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    if Button::new(tl!("Add files…")).icon(IconId::new("plus")).show(ui).clicked() {
        app.combine_dialog();
    }
}

/// The open documents as files to combine: the main part adds every one, the menu one document.
fn add_open_documents(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let open: Vec<(usize, String)> =
        app.views.iter().enumerate().filter_map(|(i, _)| crate::canvas::tab_label(app, i).map(|name| (i, name))).collect();
    // `None` is every open document.
    let mut entries: Vec<Entry<Option<usize>>> = vec![Entry::new(None, tl!("All open documents"))];
    entries.extend(open.iter().map(|(i, name)| Entry::new(Some(*i), name.clone())));
    let opener = Button::new(tl!("Add open documents")).icon(IconId::new("file-plus")).enabled(!open.is_empty()).show(ui);
    let picked = Menu::new(tl!("Add open documents")).opened_by(&opener, Trigger::Click).pick(ui, &entries).picked;
    match picked {
        Some(Some(i)) => app.add_open_documents(&[i]),
        Some(None) => app.add_open_documents(&open.iter().map(|(i, _)| *i).collect::<Vec<_>>()),
        None => {}
    }
}

/// Most files the prompt names; the rest are counted.
const SKIP_PROMPT_NAMES: usize = 8;

/// "Leave out files that can't be read?": names them and why, then combines the others, or not.
fn skip_prompt(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    if !app.combine_tab.confirm_skip {
        return;
    }
    let mut left_out: Vec<(&str, &str)> = Vec::new();
    let mut seen = BTreeSet::new();
    for f in app.combine_draft.iter().filter(|f| f.unreadable() && seen.insert(f.group)) {
        left_out.push((f.name.as_str(), f.problem.as_deref().unwrap_or_default()));
    }
    let mut lines: Vec<String> = left_out.iter().take(SKIP_PROMPT_NAMES).map(|(name, why)| format!("• {name}: {why}")).collect();
    if left_out.len() > SKIP_PROMPT_NAMES {
        let more = (left_out.len() - SKIP_PROMPT_NAMES).to_string();
        lines.push(crate::i18n::fmt(tl!("and {n} more"), &[("n", &more)]));
    }
    let out = Prompt::new(tl!("Leave out files that can't be read?"), tl!("These files can't be read, so they won't be in the combined PDF:"))
        .opened(Bind::bound(&mut app.combine_tab.confirm_skip))
        .lines(lines.into_iter().map(Into::into))
        .answers([FooterButton::new(tl!("Combine the rest"), DialogRole::Accept), FooterButton::new(tl!("Cancel"), DialogRole::Cancel)])
        .show(ui);
    if out.closed.is_some() {
        app.combine_tab.confirm_skip = false;
        if out.picked == Some(0) {
            app.combine_staged(ui.ctx());
        }
    }
}

/// Unlock…'s password dialog, shown while `unlock` holds the rows it asks about. A wrong password keeps it open.
fn unlock_dialog(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let Some(ask) = &app.combine_tab.unlock else { return };
    let rows = ask.rows.clone();
    let error = ask.error.clone();
    let files: Vec<&CombineFile> = app.combine_draft.iter().filter(|f| rows.contains(&f.id)).collect();
    let permissions = files.iter().all(|f| f.lock == Some(Lock::Permissions));
    let purpose = match (files.as_slice(), permissions) {
        ([one], true) => PasswordPurpose::permissions(one.name.clone()),
        ([one], false) => PasswordPurpose::open(one.name.clone()),
        (many, true) => PasswordPurpose::permissions_several(many.len()),
        (many, false) => PasswordPurpose::open_several(many.len()),
    };
    let mut dialog = PasswordDialog::new(purpose).tag(rows.clone()).note(tl!("The password is only used to read the file; it isn't saved."));
    if let Some(error) = error {
        dialog = dialog.error(error.into());
    }
    let mut open = true;
    let out = dialog.opened(Bind::bound(&mut open)).show(ui);
    if let Some(mut answer) = out.answer {
        let rows = answer.take_tag::<Vec<u64>>().unwrap_or(rows);
        // It closes the dialog on success and sets the error on a wrong password.
        app.combine_unlock(&rows, answer.secret.expose());
    } else if out.closed.is_some() {
        app.combine_tab.unlock = None;
    }
}

/// Whether a file can be shown as its pages: readable, unlocked, and adding more than one.
pub(super) fn expandable(f: &mut CombineFile) -> bool {
    f.lock.is_none() && f.problem.is_none() && f.selection().is_ok_and(|k| k > 1 && k <= crate::combine_grid::MAX_EXPAND)
}

/// Why Combine can't run yet, if so.
fn blocker(files: &[CombineFile], checks: &[Result<usize, String>]) -> Option<String> {
    // (A file split into parts is still one file.) Files that can't be read are left out, so
    // they don't count.
    if files.iter().filter(|f| !f.unreadable()).map(|f| f.group).collect::<BTreeSet<u64>>().len() < 2 {
        return Some(match files.iter().find(|f| f.unreadable()) {
            Some(f) => {
                crate::i18n::fmt(tl!("{name} can't be combined: {why}"), &[("name", &f.name), ("why", f.problem.as_deref().unwrap_or_default())])
            }
            None => tl!("Add at least two files").to_string(),
        });
    }
    for (f, check) in files.iter().zip(checks) {
        if f.unreadable() {
            continue;
        }
        if let Some(p) = &f.problem {
            return Some(crate::i18n::fmt(tl!("{name} can't be combined: {why}"), &[("name", &f.name), ("why", p)]));
        }
        if let Err(e) = check {
            return Some(crate::i18n::fmt(tl!("Check the pages of {name}: {why}"), &[("name", &f.name), ("why", e)]));
        }
    }
    None
}
