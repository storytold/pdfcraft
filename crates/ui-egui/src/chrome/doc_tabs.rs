//! The document tab strips: the main window's and split view's second pane's. A tab is a
//! [`DocView`](crate::DocView) keyed by its view key, since split view can show one document in two
//! tabs; group pills sit directly before their group's tabs, and the Combine files tab sits last in
//! the main strip and is never moved.

use std::borrow::Cow;
use std::collections::HashSet;

use craft::behavior::{EditEvent, IconId, SelectionChange, keyed};
use craft::platform::open::CAN_REVEAL;
use craft::ui::buttons::Button;
use craft::ui::contract::overlay::Dismiss;
use craft::ui::contract::{
    Bind, CollectionEvent, DropAt, DropKind, DropPlace, ListDrag, Props as _, ReorderRule, Show as _, ShowCollection as _, ShowContainer as _, Sizing,
};
use craft::ui::layout_components::Row;
use craft::ui::menu::{Menu, MenuItem};
use craft::ui::popover::Popover;
use craft::ui::tabs::{Tab, TabStrip, TabStripOutput};

use crate::PdfCraftApp;
use crate::panes::Place;
use crate::tab_context::Action;
use crate::tab_group::TabGroupId;

/// What a strip item stands for: a group header pill, a view of a document, or the Combine files tab.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum StripTab {
    /// A tab group's header pill, before its tabs.
    Group(TabGroupId),
    /// A document view, by `DocView::view_key`.
    View(u64),
    /// The Combine files tab, always last.
    Combine,
}

struct StripEntry {
    key: StripTab,
    label: String,
    modified: bool,
    closable: bool,
    tooltip: Option<String>,
    icon: Option<IconId>,
}

/// The strip of `place`: its tabs (with group pills) in pane order, then (main strip only) Combine files.
pub(crate) fn strip(app: &mut PdfCraftApp, ui: &mut egui::Ui, place: Place) {
    let (entries, seen_groups, has_combine) = build_strip_entries(app, ui, place);
    let selected = if place == Place::Main && app.combine_showing() {
        Some(StripTab::Combine)
    } else {
        app.pane_front(place).and_then(|i| app.views.get(i)).map(|v| StripTab::View(v.pane.key))
    };
    let strip_label = if place == Place::Main { tl!("Documents") } else { tl!("Split view documents") }.to_string();
    let source = keyed(&entries, |entry: &StripEntry| entry.key.clone()).with_text(|entry: &StripEntry| Cow::Borrowed(entry.label.as_str()));
    let item_count = entries.len();
    let entries_ref = &entries;
    let make_strip = || {
        let mut strip = TabStrip::new(strip_label.clone())
            .takes_drop(DropKind::new(DropPlace::Between, |_: &ListDrag<StripTab>, _| true))
            .can_close(move |key: &StripTab| entries_ref.iter().any(|entry| &entry.key == key && entry.closable))
            .reorder(ReorderRule::new())
            .drag_badge(false)
            .can_drag(|key: &StripTab| matches!(key, StripTab::View(_)))
            .accepts(move |drag: &ListDrag<StripTab>, _, insert| {
                if has_combine && insert >= item_count {
                    return false;
                }
                drag.keys.iter().all(|k| matches!(k, StripTab::View(_)))
            });
        if let Some(selected) = selected.clone() {
            strip = strip.shown(selected);
        }
        strip
    };
    let show_strip = |ui: &mut egui::Ui, strip: TabStrip<'_, StripTab>| {
        strip.show_items(ui, &source, &mut |key, _| {
            let Some(item) = entries_ref.iter().find(|entry| &entry.key == key) else {
                return Tab::new("").closable(false);
            };
            let mut tab = Tab::new(item.label.clone()).closable(item.closable);
            if item.modified {
                tab = tab.modified();
            }
            if let Some(icon) = &item.icon {
                tab = tab.icon(icon.clone());
            }
            if let Some(tooltip) = &item.tooltip {
                tab = tab.tooltip(tooltip.clone());
            }
            tab
        })
    };

    let shown = show_strip_row(ui, place, entries.len(), make_strip, show_strip);
    let Ok(output) = shown else { return };
    let group_rects = paint_group_overlays(app, ui, &output.tab_rects);
    let response = output.response.response.clone();
    handle_strip_output(app, ui, place, &entries, output);
    show_group_editor(app, ui, place, &seen_groups, &group_rects);
    show_tab_context_menu(app, ui, place, &response);
}

fn build_strip_entries(app: &mut PdfCraftApp, ui: &egui::Ui, place: Place) -> (Vec<StripEntry>, HashSet<TabGroupId>, bool) {
    let mut entries: Vec<StripEntry> = Vec::new();
    let mut seen_groups: HashSet<TabGroupId> = HashSet::new();

    for index in app.tabs_in(place) {
        let Some(view) = app.views.get(index) else { continue };
        if let Some(gid) = view.group {
            if seen_groups.insert(gid)
                && let Some(g) = app.group(gid)
            {
                let title = g.title.trim();
                let group_label = if title.is_empty() { tl!("Tab group").to_string() } else { title.to_string() };
                let tip = if g.collapsed {
                    crate::i18n::fmt(tl!("{name}: Click to expand"), &[("name", &group_label)])
                } else {
                    crate::i18n::fmt(tl!("{name}: Click to collapse, right-click to edit"), &[("name", &group_label)])
                };
                entries.push(StripEntry {
                    key: StripTab::Group(gid),
                    label: group_label,
                    modified: false,
                    closable: false,
                    tooltip: Some(tip),
                    icon: Some(IconId::new(if g.collapsed { "chevron-right" } else { "chevron-down" })),
                });
            }
            if app.group(gid).is_some_and(|g| g.collapsed) {
                continue;
            }
        }
        let Some(doc) = app.session.get(view.id) else { continue };
        let base_name = crate::canvas::tab_label(app, index).unwrap_or_else(|| doc.name.clone());
        let name = if view.pinned { crate::i18n::fmt(tl!("{name} (pinned)"), &[("name", &base_name)]) } else { base_name };
        let tooltip = doc
            .path
            .as_ref()
            .filter(|path| std::path::Path::new(path).file_name().is_some_and(|file| file.to_string_lossy() != doc.name.as_str()))
            .cloned();
        entries.push(StripEntry {
            key: StripTab::View(view.pane.key),
            label: name,
            modified: doc.dirty,
            closable: !view.pinned,
            tooltip,
            icon: view.pinned.then(|| IconId::new("pin")),
        });
    }

    // Advance group collapse/expand indicator animation for groups shown in this strip.
    for &gid in &seen_groups {
        if let Some(g) = app.group_mut(gid) {
            g.open_progress = ui.ctx().animate_bool_with_time(ui.scope_id().with(("tab_group_open", g.id)), !g.collapsed, 0.15);
        }
    }

    let has_combine = place == Place::Main && app.combine_tab.open;
    if has_combine {
        entries.push(StripEntry {
            key: StripTab::Combine,
            label: tl!("Combine files").to_string(),
            modified: false,
            closable: true,
            tooltip: None,
            icon: None,
        });
    }

    (entries, seen_groups, has_combine)
}

fn show_strip_row<'a>(
    ui: &mut egui::Ui,
    place: Place,
    item_count: usize,
    mut make_strip: impl FnMut() -> TabStrip<'a, StripTab>,
    mut show_strip: impl FnMut(&mut egui::Ui, TabStrip<'a, StripTab>) -> TabStripOutput<StripTab>,
) -> Result<TabStripOutput<StripTab>, egui::Response> {
    if place != Place::Main {
        return Ok(show_strip(ui, make_strip()));
    }
    let mut shown = None;
    let row = Row::new().sizing(Sizing::FillWidth).show(ui, |ui, _| {
        let strip = show_strip(ui, make_strip());
        let memory = ui.scope_id().with("document-tabs-trailing-width");
        let held = ui.ctx().data_mut(|d| d.get_temp::<f32>(memory).unwrap_or(0.0));
        let slot = ui.allocate_exact_size(egui::vec2(held, strip.response.response.rect.height()), egui::Sense::hover()).0;
        let all_show = strip.tab_rects.len() == item_count;
        let after = match strip.tab_rects.last() {
            Some((_, rect)) if all_show => Some(rect.right() + ui.spacing().item_spacing.x),
            None => Some(strip.response.response.rect.left()),
            _ => None,
        };
        let (room, layout) = match after {
            Some(at) if !ui.layout().prefer_right_to_left() => {
                let end = slot.right().min(ui.clip_rect().right().max(slot.left()));
                let left = at.min(slot.left()).min(end - held).max(strip.response.response.rect.left());
                (
                    egui::Rect::from_min_max(egui::pos2(left, slot.top()), egui::pos2(slot.right().max(left + 400.0), slot.bottom())),
                    egui::Layout::left_to_right(egui::Align::Center),
                )
            }
            _ => {
                let end = slot.right().min(ui.clip_rect().right().max(slot.left()));
                (
                    egui::Rect::from_min_max(egui::pos2(end - slot.width(), slot.top()), egui::pos2(end, slot.bottom())),
                    egui::Layout::right_to_left(egui::Align::Center),
                )
            }
        };
        let builder = egui::UiBuilder::new().max_rect(room).layout(layout);
        let drawn = ui
            .scope_builder(builder, |ui| {
                Button::command("file.open").variant(craft::theme::Variant::GHOST).icon(IconId::new("plus")).show(ui);
            })
            .response
            .rect
            .width();
        if (drawn - held).abs() > 0.5 {
            ui.ctx().data_mut(|d| d.insert_temp(memory, drawn));
            ui.ctx().request_repaint();
        }
        shown = Some(strip);
    });
    shown.ok_or(row.output)
}

fn paint_group_overlays(app: &PdfCraftApp, ui: &egui::Ui, tab_rects: &[(StripTab, egui::Rect)]) -> Vec<(TabGroupId, egui::Rect)> {
    let mut group_rects: Vec<(TabGroupId, egui::Rect)> = Vec::new();
    let mut prev: Option<(egui::Rect, Option<TabGroupId>)> = None;
    for (item, rect) in tab_rects {
        match item {
            StripTab::Group(gid) => {
                group_rects.push((*gid, *rect));
                if let Some(g) = app.group(*gid) {
                    let hovered = ui.rect_contains_pointer(*rect);
                    crate::tab_group::paint_group_pill(ui, g, *rect, hovered);
                }
                prev = Some((*rect, Some(*gid)));
            }
            StripTab::View(key) => {
                let view_group = app.tab_with_key(*key).and_then(|i| app.views.get(i)).and_then(|v| v.group);
                if let Some(gid) = view_group
                    && let Some(g) = app.group(gid)
                {
                    let color = g.color.color32();
                    let bar = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 2.5));
                    ui.painter().rect_filled(bar, egui::CornerRadius::ZERO, color);
                    if let Some((prev_rect, Some(prev_gid))) = prev
                        && prev_gid == gid
                    {
                        let (gap_left, gap_right) =
                            if prev_rect.right() <= rect.left() { (prev_rect.right(), rect.left()) } else { (rect.right(), prev_rect.left()) };
                        if gap_right > gap_left && gap_right - gap_left <= 16.0 {
                            let bridge = egui::Rect::from_min_max(egui::pos2(gap_left, rect.top()), egui::pos2(gap_right, rect.top() + 2.5));
                            ui.painter().rect_filled(bridge, egui::CornerRadius::ZERO, color);
                        }
                    }
                }
                prev = Some((*rect, view_group));
            }
            StripTab::Combine => {
                prev = Some((*rect, None));
            }
        }
    }
    group_rects
}

fn select_strip_tab(app: &mut PdfCraftApp, tab: StripTab) {
    match tab {
        StripTab::Group(gid) => app.toggle_group_collapsed(gid),
        StripTab::View(k) => {
            if let Some(i) = app.tab_with_key(k) {
                app.active = Some(i);
                app.combine_tab.focused = false;
            }
        }
        StripTab::Combine => {
            app.active = None;
            app.combine_tab.focused = true;
        }
    }
}

fn handle_strip_output(app: &mut PdfCraftApp, ui: &egui::Ui, place: Place, entries: &[StripEntry], output: TabStripOutput<StripTab>) {
    for event in output.response.events {
        match event {
            CollectionEvent::Selection(SelectionChange::Set(keys)) => {
                for key in keys {
                    select_strip_tab(app, key);
                }
            }
            // Double-clicking a saved document's tab renames its file, as in Finder and File Explorer.
            CollectionEvent::Activate(StripTab::View(key)) => {
                let saved = app
                    .tab_with_key(key)
                    .and_then(|i| app.views.get(i))
                    .map(|v| v.id)
                    .filter(|id| app.session.get(*id).is_some_and(|d| d.path.is_some()));
                if let Some(id) = saved
                    && cfg!(not(target_arch = "wasm32"))
                {
                    app.start_rename(ui.ctx(), id);
                }
            }
            CollectionEvent::Reorder(edit) => {
                if let (Some(moved), true) = (edit.value(), matches!(edit, EditEvent::Set { .. } | EditEvent::Commit { .. })) {
                    let remaining: Vec<&StripTab> = entries.iter().map(|e| &e.key).filter(|id| !moved.keys.contains(id)).collect();
                    let before = remaining.get(moved.to).cloned().cloned();
                    for document in &moved.keys {
                        let StripTab::View(key) = *document else { continue };
                        let Some(from) = app.tab_with_key(key) else { continue };
                        let tabs = app.tabs_in(place);
                        let others: Vec<usize> = tabs.iter().copied().filter(|&i| i != from).collect();
                        let to = match &before {
                            Some(StripTab::View(next)) => {
                                app.tab_with_key(*next).and_then(|n| others.iter().position(|&i| i == n)).unwrap_or(others.len())
                            }
                            Some(StripTab::Group(gid)) => {
                                others.iter().position(|&i| app.views.get(i).is_some_and(|v| v.group == Some(*gid))).unwrap_or(others.len())
                            }
                            Some(StripTab::Combine) | None => others.len(),
                        };
                        app.move_tab_in_strip(from, &tabs, to);
                    }
                }
            }
            CollectionEvent::Dropped { at: DropAt::Gap { key: Some(_), .. }, payload, .. } => {
                if let Some(drag) = payload.get::<ListDrag<StripTab>>() {
                    for document in &drag.keys {
                        if let StripTab::View(key) = document
                            && let Some(i) = app.tab_with_key(*key)
                        {
                            app.move_tab_to(i, place);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(picked) = output.picked {
        select_strip_tab(app, picked);
    }

    for closed in output.closed {
        match closed {
            StripTab::View(key) => app.tab_action(ui.ctx(), Action::Close(key)),
            StripTab::Combine => app.close_combine_tab(ui.ctx()),
            StripTab::Group(_) => {}
        }
    }

    if let Some(request) = output.menu {
        match request.key {
            StripTab::View(key) => {
                app.panes.tab_menu = Some((place, key));
                app.editing_group = None;
            }
            StripTab::Group(gid) => {
                app.panes.tab_menu = None;
                if let Some(g) = app.group(gid) {
                    app.group_title_draft = g.title.clone();
                    app.editing_group = Some(gid);
                }
            }
            StripTab::Combine => {
                app.panes.tab_menu = None;
                app.editing_group = None;
            }
        }
    }
}

fn show_group_editor(
    app: &mut PdfCraftApp,
    ui: &mut egui::Ui,
    place: Place,
    seen_groups: &HashSet<TabGroupId>,
    group_rects: &[(TabGroupId, egui::Rect)],
) {
    let Some(gid) = app.editing_group else { return };
    if app.group(gid).is_none() || !seen_groups.contains(&gid) {
        return;
    }
    let mut open = true;
    let mut close_popup = false;
    let mut popover =
        Popover::new(tl!("Tab group").to_string(), Bind::bound(&mut open)).dismiss(Dismiss::new().focus_left(false)).close_on_enter(true);
    if let Some(pill_rect) = group_rects.iter().find(|(g, _)| *g == gid).map(|(_, r)| *r) {
        popover = popover.at_point(pill_rect.left_bottom() + egui::vec2(0.0, 4.0));
    } else {
        popover = popover.at_pointer();
    }
    popover.show(ui, |ui, _| {
        crate::tab_group::group_editor_popup(app, ui, place, gid, &mut close_popup);
    });
    if !open || close_popup {
        app.editing_group = None;
    }
}

fn show_tab_context_menu(app: &mut PdfCraftApp, ui: &mut egui::Ui, place: Place, response: &egui::Response) {
    // The menu acts on the tab it was asked for, not the active one.
    let Some(tab) = app.panes.tab_menu.filter(|(menu_place, _)| *menu_place == place).map(|(_, key)| key) else {
        return;
    };
    let view_idx = app.tab_with_key(tab);
    let view = view_idx.and_then(|i| app.views.get(i));
    let doc = view.map(|v| v.id);
    let path = doc.and_then(|id| app.session.get(id)).and_then(|d| d.path.clone());
    let can_reopen = !app.closed_tabs.is_empty();
    let is_pinned = view.is_some_and(|v| v.pinned);
    let current_group = view.and_then(|v| v.group);
    let tabs_in_place = app.tabs_in(place);
    let has_other_closable = view_idx.is_some_and(|idx| tabs_in_place.iter().any(|&i| i != idx && app.views.get(i).is_some_and(|v| !v.pinned)));
    let has_tabs_to_right = view_idx.is_some_and(|idx| {
        tabs_in_place
            .iter()
            .position(|&i| i == idx)
            .is_some_and(|pos| tabs_in_place[pos + 1..].iter().any(|&i| app.views.get(i).is_some_and(|v| !v.pinned)))
    });
    let reveal_label = if cfg!(target_os = "macos") { tl!("Reveal in Finder") } else { tl!("Reveal in File Explorer") }.to_string();
    let mut open = true;
    let mut chosen: Option<Action> = None;
    let menu = Menu::context(tl!("Tab").to_string(), response).open(Bind::bound(&mut open)).show(ui, |ui, _| {
        let group_label = |g: &crate::tab_group::TabGroup| {
            let label = if g.title.is_empty() { tl!(g.color.name()) } else { g.title.as_str() };
            format!("● {label}")
        };

        // Grouping options (Edge standard)
        if let Some(current_gid) = current_group {
            if MenuItem::new(tl!("Remove from group").to_string()).show(ui).ran {
                chosen = Some(Action::RemoveFromGroup(tab));
            }
            if app.tab_groups.len() > 1 {
                Menu::submenu_row_enabled(ui, tl!("Move to another group").to_string(), true, |ui, _| {
                    for g in &app.tab_groups {
                        if g.id != current_gid && MenuItem::new(group_label(g)).show(ui).ran {
                            chosen = Some(Action::AddToGroup { tab, group_id: g.id });
                        }
                    }
                });
            }
        } else {
            if MenuItem::new(tl!("Add tab to new group").to_string()).show(ui).ran {
                chosen = Some(Action::AddToNewGroup(tab));
            }
            if !app.tab_groups.is_empty() {
                Menu::submenu_row_enabled(ui, tl!("Add tab to group").to_string(), true, |ui, _| {
                    if MenuItem::new(tl!("New group").to_string()).show(ui).ran {
                        chosen = Some(Action::AddToNewGroup(tab));
                    }
                    ui.separator();
                    for g in &app.tab_groups {
                        if MenuItem::new(group_label(g)).show(ui).ran {
                            chosen = Some(Action::AddToGroup { tab, group_id: g.id });
                        }
                    }
                });
            }
        }

        ui.separator();

        // Pinning (Edge standard)
        let pin_label = if is_pinned { tl!("Unpin tab") } else { tl!("Pin tab") }.to_string();
        if MenuItem::new(pin_label).show(ui).ran {
            chosen = Some(Action::TogglePin(tab));
        }

        // Duplicate tab (Edge standard)
        if MenuItem::new(tl!("Duplicate tab").to_string()).show(ui).ran {
            chosen = Some(Action::Duplicate(tab));
        }

        ui.separator();

        if MenuItem::new(tl!("Close Tab").to_string()).show(ui).ran {
            chosen = Some(Action::Close(tab));
        }
        if MenuItem::new(tl!("Close other tabs").to_string()).enabled(has_other_closable).show(ui).ran {
            chosen = Some(Action::CloseOthers(tab));
        }
        if MenuItem::new(tl!("Close tabs to the right").to_string()).enabled(has_tabs_to_right).show(ui).ran {
            chosen = Some(Action::CloseToRight(tab));
        }
        if MenuItem::new(tl!("Reopen Closed Tab").to_string()).enabled(can_reopen).show(ui).ran {
            chosen = Some(Action::Reopen);
        }
        ui.separator();
        if MenuItem::new(tl!("Split right").to_string()).show(ui).ran {
            chosen = Some(Action::Split { tab, down: false });
        }
        if MenuItem::new(tl!("Split down").to_string()).show(ui).ran {
            chosen = Some(Action::Split { tab, down: true });
        }
        ui.separator();
        if MenuItem::new(reveal_label.clone()).enabled(CAN_REVEAL && path.is_some()).show(ui).ran
            && let Some(id) = doc
        {
            chosen = Some(Action::Reveal(id));
        }
        if MenuItem::new(tl!("Rename…").to_string()).enabled(CAN_REVEAL && path.is_some()).show(ui).ran
            && let Some(id) = doc
        {
            chosen = Some(Action::Rename(id));
        }
    });
    if !open || menu.output.closed.is_some() {
        app.panes.tab_menu = None;
    }
    if let Some(action) = chosen {
        app.tab_action(ui.ctx(), action);
    }
}
