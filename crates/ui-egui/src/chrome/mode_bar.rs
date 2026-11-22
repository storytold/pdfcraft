//! The top slot: the menus, the mode strip, the document buttons and the command search.

use std::borrow::Cow;

use craft::behavior::{DisplayOrder, IconId, ItemSource, SelectionChange};
use craft::keymap::CommandRef;
use craft::menu::Target;
use craft::ui::bars::{Bar, BarKind};
use craft::ui::buttons::{CommandSearch, IconButton};
use craft::ui::contract::{Bind, CollectionEvent, Props as _, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::layout_components::Row;
use craft::ui::menu::HelpEntry;
use craft::ui::patterns::panels::CommandPalette;
use craft::ui::tabs::{Tab, TabKind, TabStrip};

use pdfcraft_engine::catalog::{Availability, TOOL_GROUPS};

use crate::{Mode, PdfCraftApp};

/// The modes the strip offers. Read is a View command, not a tab.
const MODES: [(Mode, &str); 4] = [(Mode::AllTools, "All tools"), (Mode::Edit, "Edit"), (Mode::Convert, "Convert"), (Mode::Sign, "E-Sign")];

/// The mode labels, keyed by position.
struct ModeTabs;

impl DisplayOrder<usize> for ModeTabs {
    fn len(&self) -> usize {
        MODES.len()
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        (*key < MODES.len()).then_some(*key)
    }

    fn text(&self, at: usize) -> Option<Cow<'_, str>> {
        MODES.get(at).map(|(_, label)| Cow::Borrowed(tl!(*label)))
    }
}

impl ItemSource<usize> for ModeTabs {}

/// Draw the menus, the mode strip and the buttons after it, then the command palette over them.
pub(super) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let hidden = app.hidden_interface();
    let menus = app.native_menu.is_none() && !hidden.menubar;
    Row::new().show(ui, |ui, _| {
        if menus {
            app.menu_bar_ui(ui);
        }
        // Production replaces the search field with its icon when the field cannot keep its minimum width.
        let compact_search = ui.available_width() < 140.0 + 116.0;
        // Below room for the current mode, the search icon and the three document buttons, the buttons give way
        // (their commands stay in the menus and on their shortcuts) so the search never lands on the strip.
        let document_buttons = ui.available_width() >= 200.0;
        // The document may hide the tools (`crate::viewer_prefs`): the mode strip, the buttons and the search.
        let shown = MODES.iter().position(|(mode, _)| *mode == app.mode);
        Bar::new(tl!("Modes"), BarKind::Tool)
            .trailing(|ui, _| {
                if hidden.toolbar {
                    return;
                }
                if compact_search {
                    IconButton::command(IconId::new("search"), "view.palette").label(tl!("Find tools and commands")).show(ui);
                } else {
                    CommandSearch::new("view.palette", tl!("Find tools and commands")).show(ui);
                }
                // Production puts the search first and the document buttons at the right edge.
                if document_buttons {
                    IconButton::command(IconId::new("info"), "file.properties").show(ui);
                    IconButton::command(IconId::new("save"), "file.save").show(ui);
                    IconButton::command(IconId::new("printer"), "print.dialog").show(ui);
                }
            })
            .show(ui, |ui, _| {
                if hidden.toolbar {
                    return;
                }
                // The bar lays its parts out along an unbounded row, so the strip takes its natural length (16.3).
                let strip = TabStrip::new(tl!("Modes")).kind(TabKind::Text).natural_height(true);
                let strip = match shown {
                    Some(at) => strip.shown(at),
                    None => strip,
                };
                let out = strip.show_items(ui, &ModeTabs, &mut |key, _| Tab::new(tl!(MODES[*key % MODES.len()].1)));
                for event in out.response.events {
                    if let CollectionEvent::Selection(SelectionChange::Set(keys)) = event
                        && let Some((mode, _)) = keys.last().and_then(|at| MODES.get(*at))
                    {
                        app.switch_mode(ui.ctx(), *mode, true);
                    }
                }
            });
    });
    if app.palette_open {
        let ctx = ui.ctx().clone();
        let entries = palette_entries(app, &ctx);
        CommandPalette::new(tl!("Find tools and commands"), &entries, Bind::bound(&mut app.palette_open)).show(ui);
    }
}

/// What the palette searches: the menus' commands, each ready tool group, and the catalogue items
/// that are not commands of their own.
fn palette_entries(app: &PdfCraftApp, ctx: &egui::Context) -> Vec<HelpEntry> {
    let mut entries = app.menu_bar(ctx).help_entries(Target::current(), &crate::menus::layout_options(), None);
    for group in TOOL_GROUPS {
        if group.availability == Availability::Ready {
            // PLAN: assumed `tool.group` reads a `group` parameter (pdf-chrome-w9)
            let select = CommandRef::with_params("tool.group", serde_json::json!({ "group": group.id }));
            entries.push(HelpEntry::new(select, tl!(group.label)).with_path([tl!("Tool")]));
        }
        for section in group.sections {
            for item in section.items {
                if item.availability == Availability::Ready && pdfcraft_engine::commands::command(item.command).is_none() {
                    entries.push(HelpEntry::new(item.command, tl!(item.label)).with_path([tl!(group.label)]));
                }
            }
        }
    }
    entries
}
