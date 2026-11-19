//! Home tab: recommended tools, pinned tools, recent files and pinned folders (local only, never
//! another app's list), on the framework's start screen.

use std::cell::RefCell;

use craft::behavior::IconId;
use craft::behavior::{DisplayOrder, ItemSource, SizeHint};
use craft::keymap::CommandRef;
use craft::theme::variant::Tone;
use craft::ui::buttons::{Button, IconButton};
use craft::ui::contract::{CollectionEvent, Props as _, Show as _, ShowCollection as _};
use craft::ui::display::{PictureSize, Text};
use craft::ui::list::ThumbSize;
use craft::ui::notices::Notice;
use craft::ui::runtime::env::Env;
use craft::ui::shell::{RecentFile, StartAction, StartLink, StartPlace, StartScreen};
use craft::ui::style::DataColor;
use craft::ui::tile_grid::{Tile, TileGrid, TilePartEvent};
use pdfcraft_engine::catalog::{self, ToolGroup};

use crate::{PdfCraftApp, brand, panels::human_size};

pub mod folders_ui;

const RECOMMENDED: [&str; 5] = ["organize", "comment", "form", "edit", "protect"];

/// What a section of the page asked for; the page runs it once the screen has been shown.
enum Asked {
    Select(&'static str),
    Unpin(&'static str),
    PinTools,
    CloseCommunityCard,
}

/// Tool tiles keyed by their position in the shown list.
struct Tools(usize);

impl DisplayOrder<usize> for Tools {
    fn len(&self) -> usize {
        self.0
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        (*key < self.0).then_some(*key)
    }
}

impl ItemSource<usize> for Tools {
    fn size_hint(&self, _at: usize) -> SizeHint {
        SizeHint::Aspect(1.0)
    }
}

/// The tile of tool group `g`; `unpinnable` adds the hover action that takes it off Home.
fn tool_tile(g: &'static ToolGroup, unpinnable: bool) -> Tile<'static> {
    let hue = DataColor::new(egui::Color32::from_rgb(g.hue[0], g.hue[1], g.hue[2]));
    let tile = Tile::new(tl!(g.label).to_string()).icon(IconId::new(g.icon)).hue(hue);
    if unpinnable { tile.action(IconId::new("x"), CommandRef::new("home.unpin_tool")) } else { tile }
}

/// A grid of the tools `groups`; a click selects a tool and, with `unpinnable`, the hover action unpins it.
///
/// The start page scrolls as a whole, so the grid takes the height of its rows and owns no scroll.
fn tool_grid(ui: &mut egui::Ui, name: &str, groups: &[&'static ToolGroup], unpinnable: bool, asked: &RefCell<Vec<Asked>>) {
    let out = TileGrid::<usize>::new(name.to_string())
        .natural_height(true)
        .show_items(ui, &Tools(groups.len()), &mut |key, _| tool_tile(groups[*key], unpinnable));
    for event in out.response.events {
        match event {
            CollectionEvent::Activate(key) => {
                if let Some(g) = groups.get(key) {
                    asked.borrow_mut().push(Asked::Select(g.id));
                }
            }
            CollectionEvent::Item(key, TilePartEvent::Action(0)) => {
                if let Some(g) = groups.get(key) {
                    asked.borrow_mut().push(Asked::Unpin(g.id));
                }
            }
            _ => {}
        }
    }
}

/// Draw the Home page: the welcome text, community card, tools, recent files and pinned folders.
pub fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    #[cfg(not(target_arch = "wasm32"))]
    app.refresh_pinned(ui.input(|i| i.time));

    let files: Vec<RecentFile> = app
        .recent
        .iter()
        .map(|r| {
            let pages = crate::i18n::trn(crate::i18n::current(), r.pages as u64, "{n} page", "{n} pages");
            RecentFile::new(r.path.as_str(), r.name.as_str()).note(format!("{pages} · {}", human_size(r.size)))
        })
        .collect();
    let links: Vec<StartLink> = pdfcraft_engine::links::LINKS
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let label = if i == 0 { tl!("Join our Discord") } else { tl!(l.label) };
            StartLink::command(label.to_string(), Some(IconId::new(l.icon)), CommandRef::new(l.command))
        })
        .collect();
    #[cfg(not(target_arch = "wasm32"))]
    let pinned_folders = app.pinned_folders(&Env::of(ui));

    let pinned_tools: Vec<&'static ToolGroup> = app.pinned_tools.iter().filter_map(|id| catalog::group(id)).collect();
    let recommended: Vec<&'static ToolGroup> = RECOMMENDED.iter().filter(|id| !app.is_tool_pinned(id)).filter_map(|id| catalog::group(id)).collect();
    let community_card = !app.community_card_closed;
    let asked = RefCell::new(Vec::new());

    // PLAN: assumed fields `recent_view: RecentView` and `recent_sort: RecentSort` on PdfCraftApp (pdf-hub-w1).
    let (mut view, mut sort) = (app.recent_view, app.recent_sort);
    let mut screen = StartScreen::new(craft::ui::contract::Bind::bound(&mut view), craft::ui::contract::Bind::bound(&mut sort), &files)
        .max_recent(app.recent_limit)
        .offers_new(false)
        .drop_hint(tl!("Files you open in PdfCraft appear here").to_string())
        .links(&links)
        .section(StartPlace::BeforeRecent, |ui, _| {
            let _ = Text::new(tl!("Welcome to PdfCraft").to_string()).show(ui);
            let _ = Text::new(tl!("An open-source PDF workbench — local, private, and scriptable.").to_string()).show(ui);
            if community_card {
                brand::mark(ui, PictureSize::Thumb(ThumbSize::Medium));
                let _ = Text::new(tl!("Join the ArtCraft community").to_string()).show(ui);
                let _ = Text::new(tl!("Get help, share feedback and follow development on Discord.").to_string()).show(ui);
                // Closed for this run only: the card is back on the next launch.
                if IconButton::new(IconId::new("x")).id_salt("community-close").label(tl!("Close")).show(ui).clicked() {
                    asked.borrow_mut().push(Asked::CloseCommunityCard);
                }
            }
            let _ = Text::new(tl!("Recommended tools").to_string()).show(ui);
            tool_grid(ui, tl!("Recommended tools"), &recommended, false, &asked);
            let _ = Text::new(tl!("Pinned Tools").to_string()).show(ui);
            if Button::new(tl!("Pin a tool…").to_string()).id_salt("pin-a-tool").show(ui).clicks > 0 {
                asked.borrow_mut().push(Asked::PinTools);
            }
            if pinned_tools.is_empty() {
                let _ = Notice::new(Tone::Neutral, tl!("Pin your favorite tools here for quick access.").to_string()).show(ui);
            } else {
                tool_grid(ui, tl!("Pinned Tools"), &pinned_tools, true, &asked);
            }
        })
        .section(StartPlace::AfterSections, |ui, _| {
            let _ = Text::new(tl!("Privacy").to_string()).show(ui);
            let _ =
                Text::new(tl!("PdfCraft works offline. No telemetry, no account, and no cloud processing unless you add a provider.").to_string())
                    .show(ui);
        });
    #[cfg(not(target_arch = "wasm32"))]
    {
        screen = screen.pinned(&pinned_folders);
    }
    let actions = screen.show(ui).actions;
    app.recent_view = view;
    app.recent_sort = sort;

    for action in actions {
        match action {
            StartAction::Open => app.open_dialog(),
            StartAction::OpenRecent(path) | StartAction::OpenPinnedFile(path) => app.open_recent(&path.to_string_lossy()),
            StartAction::RemoveRecent(path) => {
                let path = path.to_string_lossy();
                app.recent.retain(|r| r.path != path);
            }
            StartAction::ClearRecent => {
                app.execute(ui.ctx(), "file.clear_recent");
            }
            #[cfg(not(target_arch = "wasm32"))]
            StartAction::RevealRecent(path) | StartAction::OpenPinned(path) => crate::menus::tab_context::reveal_on_jobs(app, path),
            StartAction::PinFolder => app.pin_folder_dialog(),
            StartAction::UnpinFolder(path) => app.unpin_folder(&path.to_string_lossy()),
            StartAction::FoldPinned(path) => app.pinned.toggle_collapsed(&path.to_string_lossy()),
            StartAction::ExpandPinned(path) => app.pinned.toggle_expanded(&path.to_string_lossy()),
            _ => {}
        }
    }
    for ask in asked.into_inner() {
        match ask {
            Asked::Select(id) => app.select_tool(ui.ctx(), id),
            Asked::Unpin(id) => app.unpin_tool(id),
            // PLAN: assumed `PdfCraftApp::open_pin_tools(&mut self)` (pdf-dialogs-w9).
            Asked::PinTools => app.open_pin_tools(),
            Asked::CloseCommunityCard => app.community_card_closed = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_pinned_recommended_tools_appear_once() {
        use egui_kittest::kittest::Queryable;
        let label = catalog::group("edit").map_or("", |g| g.label);
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1500.0, 1200.0)).build_eframe(|cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let info = craft::host::AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" };
                let jobs = std::sync::Arc::new(craft::host::ManualJobs::new());
                let host = craft::host::Host::builder(info).jobs(jobs).build();
                let mut app = PdfCraftApp::for_unit_tests_on(std::sync::Arc::new(host));
                app.set_option(&egui::Context::default(), "language", "en").ok();
                app.pinned_tools = vec!["edit".into()];
                app
            }
        });
        h.run_steps(4);
        assert_eq!(h.get_all_by_role_and_label(egui::accesskit::Role::ListItem, tl!(label)).count(), 1, "one tile for the tool");
    }
}
