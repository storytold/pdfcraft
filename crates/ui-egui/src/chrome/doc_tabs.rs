//! The document tab strips: the main window's and split view's second pane's. A tab is a
//! [`DocView`](crate::DocView) keyed by its view key, since split view can show one document in two
//! tabs; the Combine files tab sits last in the main strip and is never moved.

use craft::behavior::IconId;
use craft::platform::open::CAN_REVEAL;
use craft::ui::buttons::Button;
use craft::ui::contract::{Props as _, Show as _, ShowContainer as _};
use craft::ui::menu::{Menu, MenuItem};
use craft::ui::tabs::{DocumentTab, DocumentTabs, DocumentTabsEvent};

use crate::PdfCraftApp;
use crate::panes::Place;
use crate::tab_context::Action;

/// What a strip tab stands for: a view of a document, or the Combine files tab.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum StripTab {
    /// A document view, by `DocView::view_key`.
    View(u64),
    /// The Combine files tab, always last.
    Combine,
}

/// The strip of `place`: its tabs in pane order, then (main strip only) Combine files.
pub(crate) fn strip(app: &mut PdfCraftApp, ui: &mut egui::Ui, place: Place) {
    let mut tabs: Vec<DocumentTab<StripTab>> = Vec::new();
    for index in app.tabs_in(place) {
        let Some(view) = app.views.get(index) else { continue };
        let Some(doc) = app.session.get(view.id) else { continue };
        let name = crate::canvas::tab_label(app, index).unwrap_or_else(|| doc.name.clone());
        let mut tab = DocumentTab::new(StripTab::View(view.pane.key), name).modified(doc.dirty).closable(true);
        if let Some(path) = &doc.path
            && std::path::Path::new(path).file_name().is_some_and(|file| file.to_string_lossy() != doc.name.as_str())
        {
            tab = tab.tooltip(path.clone());
        }
        tabs.push(tab);
    }
    if place == Place::Main && app.combine_tab.open {
        tabs.push(DocumentTab::new(StripTab::Combine, tl!("Combine files").to_string()).closable(true).movable(false));
    }
    let selected = if place == Place::Main && app.combine_showing() {
        Some(StripTab::Combine)
    } else {
        app.pane_front(place).and_then(|i| app.views.get(i)).map(|v| StripTab::View(v.pane.key))
    };
    let label = if place == Place::Main { tl!("Documents") } else { tl!("Split view documents") };
    let mut strip = DocumentTabs::new(label.to_string(), &tabs).reorderable(true);
    if place == Place::Main {
        strip = strip.trailing(|ui| {
            Button::command("file.open").variant(craft::theme::Variant::GHOST).icon(IconId::new("plus")).show(ui);
        });
    }
    if let Some(selected) = selected {
        strip = strip.selected(selected);
    }
    let output = strip.show(ui);
    let (events, response) = (output.events, output.response);

    for event in events {
        match event {
            DocumentTabsEvent::Selected(StripTab::View(key)) | DocumentTabsEvent::Picked(StripTab::View(key)) => {
                if let Some(i) = app.tab_with_key(key) {
                    app.active = Some(i);
                    app.combine_tab.focused = false;
                }
            }
            DocumentTabsEvent::Selected(StripTab::Combine) | DocumentTabsEvent::Picked(StripTab::Combine) => {
                app.active = None;
                app.combine_tab.focused = true;
            }
            // Double-clicking a saved document's tab renames its file, as in Finder and File Explorer.
            DocumentTabsEvent::Activated(StripTab::View(key)) => {
                let saved = app
                    .tab_with_key(key)
                    .and_then(|i| app.views.get(i))
                    .map(|v| v.id)
                    .filter(|id| app.session.get(*id).is_some_and(|d| d.path.is_some()));
                if let Some(id) = saved
                    && cfg!(not(target_arch = "wasm32"))
                {
                    app.start_rename(id);
                }
            }
            DocumentTabsEvent::CloseRequested(StripTab::View(key)) => app.tab_action(ui.ctx(), Action::Close(key)),
            DocumentTabsEvent::CloseRequested(StripTab::Combine) => app.close_combine_tab(ui.ctx()),
            DocumentTabsEvent::Reordered { documents, before, .. } => {
                for document in documents {
                    let StripTab::View(key) = document else { continue };
                    let Some(from) = app.tab_with_key(key) else { continue };
                    let tabs = app.tabs_in(place);
                    let others: Vec<usize> = tabs.iter().copied().filter(|&i| i != from).collect();
                    let to = match &before {
                        Some(StripTab::View(next)) => {
                            app.tab_with_key(*next).and_then(|n| others.iter().position(|&i| i == n)).unwrap_or(others.len())
                        }
                        _ => others.len(),
                    };
                    app.move_tab_in_strip(from, &tabs, to);
                }
            }
            DocumentTabsEvent::DroppedInto { documents, .. } => {
                for document in documents {
                    if let StripTab::View(key) = document
                        && let Some(i) = app.tab_with_key(key)
                    {
                        app.move_tab_to(i, place);
                    }
                }
            }
            DocumentTabsEvent::ContextMenu(request) => {
                app.panes.tab_menu = match request.key {
                    StripTab::View(key) => Some((place, key)),
                    StripTab::Combine => None,
                };
            }
            _ => {}
        }
    }

    // The menu acts on the tab it was asked for, not the active one.
    let target: Option<u64> = app.panes.tab_menu.filter(|(menu_place, _)| *menu_place == place).map(|(_, key)| key);
    let doc = target.and_then(|key| app.tab_with_key(key)).and_then(|i| app.views.get(i)).map(|v| v.id);
    let path = doc.and_then(|id| app.session.get(id)).and_then(|d| d.path.clone());
    let can_reopen = !app.closed_tabs.is_empty();
    let reveal_label = if cfg!(target_os = "macos") { tl!("Reveal in Finder") } else { tl!("Reveal in File Explorer") }.to_string();
    let mut chosen: Option<Action> = None;
    let menu = Menu::context(tl!("Tab").to_string(), &response).show(ui, |ui, _| {
        let Some(tab) = target else { return };
        if MenuItem::new(tl!("Close Tab").to_string()).show(ui).ran {
            chosen = Some(Action::Close(tab));
        }
        if MenuItem::new(tl!("Reopen Closed Tab").to_string()).enabled(can_reopen).show(ui).ran {
            chosen = Some(Action::Reopen);
        }
        if MenuItem::new(tl!("Split right").to_string()).show(ui).ran {
            chosen = Some(Action::Split { tab, down: false });
        }
        if MenuItem::new(tl!("Split down").to_string()).show(ui).ran {
            chosen = Some(Action::Split { tab, down: true });
        }
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
    if menu.output.closed.is_some() && target.is_some() {
        app.panes.tab_menu = None;
    }
    if let Some(action) = chosen {
        app.tab_action(ui.ctx(), action);
    }
}
