use super::files::{CombineFile, CombineView, Lock, RowAction};
use super::table::{TableEvents, table};
use super::{Align, BTreeSet, CornerRadius, ERROR, Layout, Modifiers, PdfCraftApp, Rect, Stroke, Tokens, icons, theme, vec2, widgets};

/// The Combine files page.
pub(crate) fn page(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let files_hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    // Ranges are checked before anything below reads them.
    let checks: Vec<Result<usize, String>> = app.combine_draft.iter_mut().map(CombineFile::selection).collect();
    // Same-named form fields across the files are renamed when combining: preview it once per
    // list, so the Warnings column can say so before anything is combined.
    app.combine_conflicts.refresh(&app.session, &app.combine_draft);
    let conflicts = app.combine_conflicts.per_file.clone();
    let twice = app.combine_tab.duplicate_warnings(&app.combine_draft);
    let n = app.combine_draft.len();
    let ids: Vec<u64> = app.combine_draft.iter().map(|f| f.id).collect();
    app.combine_tab.selected.retain(|id| ids.contains(id));
    // Pages stay picked only while they show (a grid card of an expanded file), so the trash or
    // Delete never takes a page the user can't see.
    {
        let tab = &mut app.combine_tab;
        if app.combine_view == CombineView::Grid {
            let shown: BTreeSet<(u64, usize)> = app
                .combine_draft
                .iter_mut()
                .filter(|f| tab.expanded.contains(&f.id))
                .flat_map(|f| {
                    let id = f.id;
                    (0..f.selection().unwrap_or(0)).map(move |nth| (id, nth))
                })
                .collect();
            tab.selected_pages.retain(|p| shown.contains(p));
        } else {
            tab.selected_pages.clear();
        }
        if tab.selected_pages.is_empty() {
            tab.page_anchor = None;
        }
    }
    // A file stays expanded only while it can show as its pages (not locked again by undo, nor
    // edited down to one page or a bad range): it doesn't come back expanded by itself.
    {
        let tab = &mut app.combine_tab;
        let draft = &app.combine_draft;
        let lists = &mut tab.page_lists;
        tab.expanded.retain(|id| draft.iter().find(|f| f.id == *id).is_some_and(|f| crate::combine_grid::pages_shown(lists, f).is_some()));
        let expanded = &tab.expanded;
        tab.page_lists.retain(|id, _| expanded.contains(id));
    }
    let selected: Vec<bool> = ids.iter().map(|id| app.combine_tab.selected.contains(id)).collect();
    let count = selected.iter().filter(|s| **s).count();
    let blocker = blocker(&app.combine_draft, &checks);
    let grid = app.combine_view == CombineView::Grid;
    let columns = grid.then_some(app.combine_tab.grid_columns);
    // The keyboard is the list's unless a text field, the password box or the size slider has it.
    let slider_focused = grid && app.combine_tab.zoom_slider.is_some_and(|id| ui.memory(|m| m.has_focus(id)));
    // (Nor while a file is shown large: its window may only open or close this frame.)
    let covered = app.combine_tab.unlock.is_some() || app.combine_tab.preview.is_some() || app.combine_tab.confirm_skip;
    let focus = ui.memory(|m| m.focused());
    let space = focus.is_none() || focus == app.combine_tab.card_focus;
    let any_picked = count > 0 || !app.combine_tab.selected_pages.is_empty();
    let keys = if covered || slider_focused { None } else { keyboard(ui, n, any_picked, columns, space) };
    let mut events = TableEvents { action: keys, ..Default::default() };
    // ⌘+ / ⌘− / ⌘0 size the grid's cards (instead of the whole window's text).
    if grid && n > 0 && !covered {
        app.combine_tab.zoom_request = zoom_keys(ui, app.combine_zoom).or(app.combine_tab.zoom_request);
    }
    // An arrow pressed or a zoom asked for in the list has nothing for the grid to do later.
    if !grid {
        app.combine_tab.reveal = None;
        app.combine_tab.zoom_request = None;
    }
    let mut combine = false;

    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(28, 20)).show(ui, |ui| {
        // Title and Combine.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Combine files")).font(theme::semibold(20.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let response = ui.add_enabled_ui(blocker.is_none(), |ui| widgets::icon_pill(ui, "files", tl!("Combine"), true)).inner;
                if let Some(why) = &blocker {
                    response.clone().on_disabled_hover_text(why);
                }
                if response.clicked() {
                    combine = true;
                }
            });
        });
        let hint = if grid {
            tl!("Files are combined in the order shown. Drag them to change it.")
        } else {
            tl!("Files are combined from top to bottom. Leave Pages empty to take every page.")
        };
        ui.label(egui::RichText::new(hint).color(t.text_faint));
        ui.add_space(12.0);

        // Toolbar: on two lines when the window is narrow.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // Where the line ends (an overflowing line would move it).
            let edge = ui.max_rect().right();
            add_menu(app, ui);
            let open_docs: Vec<(usize, String)> =
                app.views.iter().enumerate().filter_map(|(i, _)| crate::canvas::tab_label(app, i).map(|name| (i, name))).collect();
            let response = ui.add_enabled_ui(!open_docs.is_empty(), |ui| widgets::icon_pill(ui, "file-plus", tl!("Add open documents"), false)).inner;
            response.clone().on_disabled_hover_text(tl!("No documents are open"));
            egui::Popup::menu(&response).show(|ui| {
                if open_docs.len() > 1 && ui.button(tl!("All open documents")).clicked() {
                    app.add_open_documents(&open_docs.iter().map(|(i, _)| *i).collect::<Vec<_>>());
                    ui.close();
                }
                for (i, name) in &open_docs {
                    if ui.button(name.as_str()).clicked() {
                        app.add_open_documents(&[*i]);
                        ui.close();
                    }
                }
            });
            group(ui, &t, edge, 4.0 * 30.0 + 3.0 * 6.0);
            // Moves need a selected row with room to move; the buttons stay put either way.
            let can_up = selected.iter().skip_while(|s| **s).any(|s| *s);
            let can_down = selected.iter().rev().skip_while(|s| **s).any(|s| *s);
            let why = tl!("Select a file first");
            let pages_picked = app.combine_tab.selected_pages.len();
            let remove = match (pages_picked, count) {
                (1, _) => tl!("Remove page").to_string(),
                (k, _) if k > 1 => crate::i18n::fmt(tl!("Remove {n} pages"), &[("n", &k.to_string())]),
                (_, 1) => {
                    let name = app.combine_draft.iter().find(|f| app.combine_tab.selected.contains(&f.id)).map_or("", |f| f.name.as_str());
                    crate::i18n::fmt(tl!("Remove {name}"), &[("name", name)])
                }
                (_, 0) => tl!("Remove").to_string(),
                (_, k) => crate::i18n::fmt(tl!("Remove {n} files"), &[("n", &k.to_string())]),
            };
            if tool(ui, can_up, "chevron-up", tl!("Move up"), why).clicked() {
                events.action = Some(RowAction::MoveUp);
            }
            if tool(ui, can_down, "chevron-down", tl!("Move down"), why).clicked() {
                events.action = Some(RowAction::MoveDown);
            }
            if tool(ui, count > 0 || pages_picked > 0, "trash-2", &remove, why).clicked() {
                events.action = Some(RowAction::Remove);
            }
            let locked = app.combine_draft.iter().any(|f| f.lock.is_some() && app.combine_tab.selected.contains(&f.id));
            let unlock = tool(ui, locked, "lock-open", tl!("Unlock…"), tl!("Select a password-protected file first"));
            if unlock.clicked() {
                events.action = Some(RowAction::Unlock(None, unlock.rect.left_bottom() + vec2(0.0, 6.0)));
            }
            group(ui, &t, edge, 2.0 * 30.0 + 6.0);
            let (undo, redo) = (!app.combine_tab.undo.is_empty(), !app.combine_tab.redo.is_empty());
            if tool(ui, undo, "undo-2", tl!("Undo"), tl!("Nothing to undo")).clicked() {
                app.execute("edit.undo");
            }
            if tool(ui, redo, "redo-2", tl!("Redo"), tl!("Nothing to redo")).clicked() {
                app.execute("edit.redo");
            }
            group(ui, &t, edge, 2.0 * 30.0 + 6.0);
            // Thumbnails or the table; the choice is kept in the settings.
            for (view, icon, label) in [(CombineView::Grid, "layout-grid", tl!("Grid view")), (CombineView::List, "list", tl!("List view"))] {
                if icons::button(ui, icon, 30.0, app.combine_view == view, label).clicked() {
                    app.combine_view = view;
                }
            }
            if count > 1 {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(crate::i18n::fmt(tl!("{n} selected"), &[("n", &count.to_string())])).color(t.text_muted));
            }
            if grid && n > 0 {
                // Every file as its pages, or all as cards again.
                group(ui, &t, edge, 2.0 * 30.0 + 6.0);
                let can_expand = app.combine_draft.iter_mut().any(|f| !app.combine_tab.expanded.contains(&f.id) && expandable(f));
                if ui.add_enabled_ui(can_expand, |ui| icons::button(ui, "maximize-2", 30.0, false, tl!("Expand all"))).inner.clicked() {
                    events.action = Some(RowAction::ExpandAll(true));
                }
                let can_collapse = !app.combine_tab.expanded.is_empty();
                if ui.add_enabled_ui(can_collapse, |ui| icons::button(ui, "minimize-2", 30.0, false, tl!("Collapse all"))).inner.clicked() {
                    events.action = Some(RowAction::ExpandAll(false));
                }
                zoom_controls(app, ui, &t, edge);
            }
        });
        ui.add_space(10.0);

        // The list, in a card that highlights while files are dragged over the window.
        let footer_h = 34.0;
        let card_h = (ui.available_height() - footer_h).max(120.0);
        let stroke = if files_hovering { Stroke::new(2.0, t.accent) } else { Stroke::new(1.0, t.border) };
        egui::Frame::NONE.fill(t.card).stroke(stroke).corner_radius(CornerRadius::same(t.radius)).inner_margin(egui::Margin::symmetric(8, 6)).show(
            ui,
            |ui| {
                ui.set_min_height(card_h - 12.0);
                // The grid scrolls inside the card, leaving room for the totals below it.
                if grid {
                    ui.set_max_height(card_h - 12.0);
                }
                ui.set_width(ui.available_width());
                if n == 0 {
                    empty_state(app, ui, &t, files_hovering);
                // The view as the toolbar left it: a click on List view this frame starts no
                // thumbnail renders.
                } else if app.combine_view == CombineView::Grid {
                    if let Some(action) = crate::combine_grid::grid(app, ui, &t, &checks, &selected, &twice) {
                        events.action = Some(action);
                    }
                } else {
                    let table_events = table(app, ui, &t, &checks, &selected, &conflicts, &twice);
                    events.range_focused |= table_events.range_focused;
                    events.range_changed |= table_events.range_changed;
                    if table_events.action.is_some() {
                        events.action = table_events.action;
                    }
                }
            },
        );

        // A file shown large, over everything.
        crate::combine_grid::preview(app, ui.ctx(), &t);

        // Totals.
        ui.add_space(8.0);
        if n > 0 {
            ui.horizontal(|ui| {
                let pages: usize = checks.iter().filter_map(|c| c.as_ref().ok()).sum();
                // Each file's size once, however many parts it is in.
                let mut counted = BTreeSet::new();
                let size: usize = app.combine_draft.iter().filter(|f| counted.insert(f.group)).map(|f| f.bytes.len()).sum();
                // A file split into parts counts once.
                let n = app.combine_draft.iter().map(|f| f.group).collect::<BTreeSet<u64>>().len();
                let files = if n == 1 { tl!("1 file").to_string() } else { crate::i18n::fmt(tl!("{n} files"), &[("n", &n.to_string())]) };
                let pages = if pages == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &pages.to_string())]) };
                ui.label(egui::RichText::new(format!("{files} · {pages} · {}", crate::panels::human_size(size))).color(t.text_muted));
                let bad = app
                    .combine_draft
                    .iter()
                    .zip(&checks)
                    .filter(|(f, c)| f.problem.is_some() || c.is_err())
                    .map(|(f, _)| f.group)
                    .collect::<BTreeSet<u64>>()
                    .len();
                if bad > 0 {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let text = if bad == 1 {
                            tl!("1 file needs attention").to_string()
                        } else {
                            crate::i18n::fmt(tl!("{n} files need attention"), &[("n", &bad.to_string())])
                        };
                        ui.label(egui::RichText::new(text).color(ERROR));
                    });
                }
            });
        }
    });

    // A Pages field taking the keyboard remembers the list; its first change makes the undo step.
    if events.range_focused {
        app.combine_tab.range_edit = Some(app.combine_snapshot());
    }
    if events.range_changed
        && let Some(before) = app.combine_tab.range_edit.take()
    {
        app.combine_push_undo(before);
    }
    unlock_prompt(app, ui.ctx());
    if let Some(action) = events.action {
        app.combine_apply(action);
    }
    if combine && blocker.is_none() {
        if app.combine_draft.iter().any(CombineFile::unreadable) {
            app.combine_tab.confirm_skip = true;
        } else {
            app.combine_staged();
        }
    }
    skip_prompt(app, ui.ctx());
}

/// Most files the prompt names; the rest are counted.
const SKIP_PROMPT_NAMES: usize = 8;

/// "Leave out files that can't be read?": names them and why, then combines the others, or not.
fn skip_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if !app.combine_tab.confirm_skip {
        return;
    }
    let t = Tokens::get(ctx);
    let mut left_out: Vec<(&str, &str)> = Vec::new();
    let mut seen = BTreeSet::new();
    for f in app.combine_draft.iter().filter(|f| f.unreadable() && seen.insert(f.group)) {
        left_out.push((f.name.as_str(), f.problem.as_deref().unwrap_or_default()));
    }
    let mut choice: Option<bool> = None;
    let modal = egui::Modal::new(egui::Id::unique("combine-skip")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.horizontal(|ui| {
            ui.add(icons::image("triangle-alert", 22.0, ERROR));
            ui.add(egui::Label::new(egui::RichText::new(tl!("Leave out files that can't be read?")).font(theme::semibold(16.0))).wrap());
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(tl!("These files can't be read, so they won't be in the combined PDF:")).color(t.text_muted));
        ui.add_space(6.0);
        for (name, why) in left_out.iter().take(SKIP_PROMPT_NAMES) {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("• {name}")).strong());
                ui.label(egui::RichText::new(*why).color(t.text_muted));
            });
        }
        if left_out.len() > SKIP_PROMPT_NAMES {
            let more = (left_out.len() - SKIP_PROMPT_NAMES).to_string();
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("and {n} more"), &[("n", &more)])).color(t.text_muted));
        }
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Combine the rest"), true).clicked() {
                choice = Some(true);
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                choice = Some(false);
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(false);
    }
    if let Some(go) = choice {
        app.combine_tab.confirm_skip = false;
        if go {
            app.combine_staged();
        }
    }
}

/// Unlock…'s password box, floating below what opened it. Enter unlocks, Escape or a click
/// elsewhere closes it.
fn unlock_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(mut prompt) = app.combine_tab.unlock.take() else { return };
    let t = Tokens::get(ctx);
    let files: Vec<&CombineFile> = app.combine_draft.iter().filter(|f| prompt.rows.contains(&f.id)).collect();
    let title = match files.as_slice() {
        [one] => crate::i18n::fmt(tl!("Unlock {name}"), &[("name", &one.name)]),
        many => crate::i18n::fmt(tl!("Unlock {n} files"), &[("n", &many.len().to_string())]),
    };
    let explain = if files.iter().all(|f| f.lock == Some(Lock::Permissions)) {
        tl!("Enter the permissions password to allow combining")
    } else {
        tl!("Enter the password that opens it")
    };
    let batch = files.len() > 1;
    let (mut submit, mut cancel) = (false, false);
    let area = egui::Area::new(egui::Id::unique("combine-unlock")).order(egui::Order::Foreground).fixed_pos(prompt.at).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
            ui.set_width(340.0);
            ui.horizontal(|ui| {
                ui.add(icons::image("lock", 16.0, t.icon));
                ui.add(egui::Label::new(egui::RichText::new(&title).font(theme::semibold(14.0))).truncate());
            });
            ui.add_space(4.0);
            ui.label(egui::RichText::new(explain).color(t.text_muted));
            if batch {
                ui.label(egui::RichText::new(tl!("The same password is tried on each file.")).small().color(t.text_faint));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                crate::widgets::begin_dialog(ui);
                let field = ui.add(crate::widgets::line(&mut prompt.input.0).password(!prompt.show).hint_text(tl!("Password")).desired_width(290.0));
                if !prompt.focused {
                    field.request_focus();
                    prompt.focused = true;
                }
                if field.changed() {
                    prompt.error = None;
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                let (icon, tip) = if prompt.show { ("eye-off", tl!("Hide password")) } else { ("eye", tl!("Show password")) };
                if icons::button(ui, icon, 26.0, false, tip).clicked() {
                    prompt.show = !prompt.show;
                }
            });
            if let Some(e) = &prompt.error {
                ui.label(egui::RichText::new(e).color(ERROR));
            }
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tl!("The password is only used to read the file; it isn't saved.")).small().color(t.text_faint));
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled_ui(!prompt.input.0.is_empty(), |ui| widgets::pill_button(ui, tl!("Unlock"), true)).inner.clicked() {
                    submit = true;
                }
                if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                    cancel = true;
                }
            });
        });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || area.response.clicked_elsewhere() {
        cancel = true;
    }
    if submit && !prompt.input.0.is_empty() {
        let rows = prompt.rows.clone();
        if app.combine_unlock(&rows, &prompt.input.0) {
            return;
        }
        prompt.error = Some(tl!("Wrong password").to_string());
        prompt.focused = false;
    }
    if !cancel {
        app.combine_tab.unlock = Some(prompt);
    }
}

fn separator(ui: &mut egui::Ui, t: &Tokens) {
    ui.add_space(4.0);
    // As tall as the buttons on this line (the toolbar can take two).
    let top = ui.cursor().top();
    ui.painter().vline(ui.cursor().left(), (top + 4.0)..=(top + 26.0), Stroke::new(1.0, t.divider));
    ui.add_space(6.0);
}

/// Before a group of toolbar buttons `width` wide: a divider, or a new line when the group
/// doesn't fit before `edge` (the toolbar wraps between groups, never inside one).
fn group(ui: &mut egui::Ui, t: &Tokens, edge: f32, width: f32) {
    if edge - ui.cursor().min.x < width + 10.0 {
        ui.end_row();
    } else {
        separator(ui, t);
    }
}

/// A toolbar icon button, disabled with a reason.
fn tool(ui: &mut egui::Ui, enabled: bool, icon: &str, label: &str, disabled_why: &str) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| icons::button(ui, icon, 30.0, false, label)).inner.on_disabled_hover_text(disabled_why)
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

/// While no text field has the keyboard: Up/Down select (Shift extends), Alt+Up/Down move the
/// selection, Delete removes it, Ctrl/⌘+A selects every file. In the grid (`columns` per row)
/// Left/Right step one file and Up/Down one row.
pub(crate) fn keyboard(ui: &egui::Ui, n: usize, any_selected: bool, grid_columns: Option<usize>, space: bool) -> Option<RowAction> {
    // A focused card or button still leaves the arrows to the list; a text field, an open menu
    // or a dialog doesn't.
    let ctx = ui.ctx();
    if n == 0 || ctx.text_edit_focused() || egui::Popup::is_any_open(ctx) || ctx.memory(|m| m.top_modal_layer().is_some()) {
        return None;
    }
    use egui::Key;
    ui.ctx().input_mut(|i| {
        if i.consume_key(Modifiers::COMMAND, Key::A) {
            return Some(RowAction::SelectAll);
        }
        // As Quick Look: the file the keyboard is on, large (the way there without a pointer). A
        // focused button keeps its Space.
        if any_selected && space && i.consume_key(Modifiers::NONE, Key::Space) {
            return Some(RowAction::PreviewSelected);
        }
        if any_selected {
            if i.consume_key(Modifiers::ALT, Key::ArrowUp) {
                return Some(RowAction::MoveUp);
            }
            if i.consume_key(Modifiers::ALT, Key::ArrowDown) {
                return Some(RowAction::MoveDown);
            }
            if i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace) {
                return Some(RowAction::Remove);
            }
        }
        let row = grid_columns.unwrap_or(1).max(1);
        let mut keys = vec![(false, row, Key::ArrowUp), (true, row, Key::ArrowDown)];
        if grid_columns.is_some() {
            keys.extend([(false, 1, Key::ArrowLeft), (true, 1, Key::ArrowRight)]);
        }
        for (down, by, key) in keys {
            // Shift first: a plain arrow also matches with Shift held (egui ignores an extra
            // Shift), which would step instead of extending.
            if i.consume_key(Modifiers::SHIFT, key) {
                return Some(RowAction::Step { down, by, extend: true });
            }
            if i.consume_key(Modifiers::NONE, key) {
                return Some(RowAction::Step { down, by, extend: false });
            }
        }
        None
    })
}

/// ⌘+ (or ⌘=), ⌘− and ⌘0, while no text field, menu or dialog has the keyboard: the grid zoom
/// they ask for, from `zoom`.
fn zoom_keys(ui: &egui::Ui, zoom: f32) -> Option<f32> {
    use crate::combine_grid::zoom_step;
    use egui::{Key, KeyboardShortcut};
    let ctx = ui.ctx();
    if ctx.text_edit_focused() || egui::Popup::is_any_open(ctx) || ctx.memory(|m| m.top_modal_layer().is_some()) {
        return None;
    }
    // All of them are taken (several can arrive in one slow frame), so none is left for egui to
    // zoom the whole window with.
    let (larger, smaller, reset) = ctx.input_mut(|i| {
        let mut pressed = |k| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, k));
        (pressed(Key::Plus) | pressed(Key::Equals), pressed(Key::Minus), pressed(Key::Num0))
    });
    match (larger, smaller, reset) {
        (_, _, true) => Some(1.0),
        (true, false, _) => Some(zoom_step(zoom, true)),
        (false, true, _) => Some(zoom_step(zoom, false)),
        _ => None,
    }
}

/// The grid's card size, at the right of the toolbar (right to left: larger, the percentage, the
/// slider, smaller), as the page grid's.
fn zoom_controls(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, edge: f32) {
    use crate::combine_grid::{ZOOM_RANGE, zoom_step};
    let zoom = app.combine_zoom;
    // Two buttons, the percentage, the slider and the spaces between: on a line of their own when
    // they don't fit beside the rest.
    let needed = 30.0 + 44.0 + 96.0 + 30.0 + 4.0 * ui.spacing().item_spacing.x + 8.0;
    if edge - ui.cursor().min.x < needed {
        ui.end_row();
    }
    // Right-aligned on the line, up to its end.
    let line = Rect::from_min_max(ui.cursor().min, egui::pos2(edge, ui.cursor().min.y + 30.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(line).layout(Layout::right_to_left(Align::Center)), |ui| {
        let larger = ui.add_enabled_ui(zoom < *ZOOM_RANGE.end(), |ui| icons::button(ui, "zoom-in", 30.0, false, tl!("Larger pages"))).inner;
        if larger.clicked() {
            app.combine_tab.zoom_request = Some(zoom_step(zoom, true));
        }
        let percent =
            egui::Button::new(egui::RichText::new(format!("{:.0}%", zoom * 100.0)).font(theme::medium(12.0)).color(t.text_muted)).frame(false);
        let percent = ui.add_sized([44.0, 30.0], percent);
        percent.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Reset page size")));
        if percent.on_hover_text(tl!("Reset page size")).clicked() {
            app.combine_tab.zoom_request = Some(1.0);
        }
        let mut value = zoom;
        ui.spacing_mut().slider_width = 96.0;
        let slider = ui.add(egui::Slider::new(&mut value, ZOOM_RANGE).show_value(false));
        // Read out as a percentage, as shown beside it.
        slider.widget_info(|| egui::WidgetInfo::slider(true, f64::from(value * 100.0).round(), tl!("Page size")));
        app.combine_tab.zoom_slider = Some(slider.id);
        if slider.on_hover_text(tl!("Page size")).changed() {
            app.combine_tab.zoom_request = Some(value);
        }
        let smaller = ui.add_enabled_ui(zoom > *ZOOM_RANGE.start(), |ui| icons::button(ui, "zoom-out", 30.0, false, tl!("Smaller pages"))).inner;
        if smaller.clicked() {
            app.combine_tab.zoom_request = Some(zoom_step(zoom, false));
        }
    });
}

fn empty_state(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, files_hovering: bool) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() / 2.0 - 70.0).max(16.0));
        ui.add(icons::image("files", 40.0, if files_hovering { t.accent } else { t.text_faint }));
        ui.add_space(10.0);
        ui.label(egui::RichText::new(tl!("Add the files to combine")).font(theme::medium(15.0)));
        ui.add_space(4.0);
        ui.label(egui::RichText::new(tl!("Drop PDFs, images, text files or folders here, or choose them.")).color(t.text_faint));
        ui.add_space(12.0);
        let folders = cfg!(not(target_arch = "wasm32"));
        // Centre the buttons: measure them first.
        let font = theme::medium(12.5);
        let width = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font.clone(), t.text).size().x);
        let total = width(tl!("Add files…")) + 26.0 + if folders { width(tl!("Add folder…")) + 46.0 + 8.0 } else { 0.0 };
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
            ui.spacing_mut().item_spacing.x = 8.0;
            if widgets::pill_button(ui, tl!("Add files…"), true).clicked() {
                app.combine_dialog();
            }
            if folders && widgets::icon_pill(ui, "folder", tl!("Add folder…"), false).clicked() {
                app.combine_add_folder(false);
            }
        });
    });
}

/// "Add files…" with a menu of the other ways to add: folders (desktop).
fn add_menu(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    if !cfg!(not(target_arch = "wasm32")) {
        if widgets::icon_pill(ui, "plus", tl!("Add files…"), false).clicked() {
            app.combine_dialog();
        }
        return;
    }
    let (main, more) = widgets::split_pill(ui, "plus", tl!("Add files…"), tl!("More ways to add files"));
    if main.clicked() {
        app.combine_dialog();
    }
    egui::Popup::menu(&more).show(|ui| {
        ui.set_min_width(220.0);
        let item = |ui: &mut egui::Ui, icon: &str, label: &str| {
            ui.add(egui::Button::image_and_text(icons::image(icon, 15.0, ui.visuals().text_color()), label))
        };
        if item(ui, "file-plus", tl!("Add files…")).clicked() {
            app.combine_dialog();
            ui.close();
        }
        if item(ui, "folder", tl!("Add folder…")).on_hover_text(tl!("Every PDF, image and text file in the folder")).clicked() {
            app.combine_add_folder(false);
            ui.close();
        }
        if item(ui, "folder-open", tl!("Add folder and subfolders…"))
            .on_hover_text(tl!("Every PDF, image and text file in the folder and the folders inside it"))
            .clicked()
        {
            app.combine_add_folder(true);
            ui.close();
        }
    });
}
