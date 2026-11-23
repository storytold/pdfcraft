//! Home tab: recommended tools, pinned tools, recent files and pinned folders (local only, never
//! another app's list), on the framework's start screen.

use std::cell::RefCell;

use craft::behavior::IconId;
use craft::behavior::{DisplayOrder, ItemSource, SizeHint};
use craft::keymap::CommandRef;
use craft::theme::TypeRole;
use craft::theme::variant::{Tone, Variant};
use craft::ui::buttons::{Button, IconButton};
use craft::ui::contract::Gap;
use craft::ui::contract::{ActivateOn, CollectionEvent, Pad, Props as _, Show as _, ShowCollection as _, ShowContainer as _, clicked};
use craft::ui::display::{PictureSize, Text};
use craft::ui::layout_components::{Row, Stack};
use craft::ui::list::ThumbSize;
use craft::ui::notices::Notice;
use craft::ui::runtime::env::Env;
use craft::ui::sections::{HeaderStrength, Section};
use craft::ui::shell::{RecentFile, StartAction, StartPlace, StartScreen};
use craft::ui::style::{DataColor, SurfaceKind};
use craft::ui::tile_grid::{CaptionPlace, GridZoom, Tile, TileGrid, TilePartEvent};
use pdfcraft_engine::catalog::{self, ToolGroup};

use crate::{PdfCraftApp, brand, panels::human_size};

pub mod folders_ui;

const RECOMMENDED: [&str; 5] = ["organize", "comment", "form", "edit", "protect"];

/// What a section of the page asked for; the page runs it once the screen has been shown.
enum Asked {
    Select(&'static str),
    Open,
    Unpin(&'static str),
    PinTools,
    CloseCommunityCard,
    Link(&'static str),
}

/// Recommended actions use the compact cards, followed by the file picker.
fn recommended_grid(ui: &mut egui::Ui, groups: &[&'static ToolGroup], asked: &RefCell<Vec<Asked>>) {
    let out = TileGrid::<usize>::new(tl!("Recommended tools").to_string())
        .activate(ActivateOn::Click)
        .natural_height(true)
        .card()
        .caption(CaptionPlace::None)
        .aspect(2.0)
        .zoom(GridZoom::Fixed(1.05))
        .show_items(ui, &Tools(groups.len() + 1), &mut |key, _| {
            if let Some(g) = groups.get(*key) {
                let description =
                    g.sections.first().map(|s| s.items.iter().take(3).map(|i| tl!(i.label)).collect::<Vec<_>>().join(" · ")).unwrap_or_default();
                tool_tile(g, false).accessible_name(tl!(g.label).to_string()).detail(description).summary_action(tl!("Use now").to_string())
            } else {
                Tile::new(tl!("Open file").to_string()).icon(IconId::new("folder_open")).summary_action("")
            }
        });
    for event in out.response.events {
        if let CollectionEvent::Activate(key) = event {
            if let Some(g) = groups.get(key) {
                asked.borrow_mut().push(Asked::Select(g.id));
            } else if key == groups.len() {
                asked.borrow_mut().push(Asked::Open);
            }
        }
    }
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
    let out = TileGrid::<usize>::new(name.to_string()).activate(ActivateOn::Click).natural_height(true).show_items(
        ui,
        &Tools(groups.len()),
        &mut |key, _| tool_tile(groups[*key], unpinnable),
    );
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
        // Open is in the title bar, as in production.
        .offers_open(false)
        .pinned_title(tl!("Folders").to_string())
        .drop_hint(tl!("Files you open in PdfCraft appear here").to_string())
        .section(StartPlace::BeforeRecent, |ui, _| {
            let _ = Text::new(tl!("Welcome to PdfCraft").to_string()).role(TypeRole::Display).show(ui);
            let _ = Text::new(tl!("An open-source PDF workbench — local, private, and scriptable.").to_string()).show(ui);
            if community_card {
                Stack::new().label(tl!("Join the ArtCraft community")).fill_width().surface(SurfaceKind::Raised).pad(Pad::Regular).show(
                    ui,
                    |ui, _| {
                        Row::new().show(ui, |ui, _| {
                            brand::mark(ui, PictureSize::Thumb(ThumbSize::Medium));
                            let _ = Text::new(tl!("Join the ArtCraft community").to_string()).role(TypeRole::Heading).show(ui);
                            if clicked(&IconButton::new(IconId::new("x")).id_salt("community-close").label(tl!("Close")).show(ui)) {
                                asked.borrow_mut().push(Asked::CloseCommunityCard);
                            }
                        });
                        let _ = Text::new(tl!("Get help, share feedback and follow development on Discord.").to_string()).show(ui);
                        Row::new().wrap(true).show(ui, |ui, _| {
                            for (i, link) in pdfcraft_engine::links::LINKS.iter().enumerate() {
                                let label = if i == 0 { tl!("Join our Discord") } else { tl!(link.label) };
                                let variant = if i == 0 { craft::theme::Variant::PRIMARY } else { craft::theme::Variant::SECONDARY };
                                if clicked(&Button::new(label.to_string()).variant(variant).icon(IconId::new(link.icon)).show(ui)) {
                                    asked.borrow_mut().push(Asked::Link(link.command));
                                }
                            }
                        });
                    },
                );
            }
            Stack::new().label(tl!("Recommended tools")).fill_width().surface(SurfaceKind::Raised).pad(Pad::Regular).show(ui, |ui, _| {
                let _ = Text::new(tl!("Recommended tools").to_string()).role(TypeRole::Heading).show(ui);
                recommended_grid(ui, &recommended, &asked);
            });
            // The heading is the page's, with "Pin a tool…" at its end, as the framework's own sections show theirs.
            let _ = Section::fixed(tl!("Pinned Tools").to_string())
                .strength(HeaderStrength::Heading)
                .quiet_header()
                .header_gap(Gap::Section)
                .controls(|ui, _| {
                    if Button::new(tl!("Pin a tool…").to_string())
                        .id_salt("pin-a-tool")
                        .variant(Variant::GHOST)
                        .icon(IconId::new("pin"))
                        .show(ui)
                        .clicks
                        > 0
                    {
                        asked.borrow_mut().push(Asked::PinTools);
                    }
                })
                .show(ui, |ui, _| {
                    if pinned_tools.is_empty() {
                        let _ = Notice::new(Tone::Neutral, tl!("Pin your favorite tools here for quick access.").to_string()).show(ui);
                    } else {
                        tool_grid(ui, tl!("Pinned Tools"), &pinned_tools, true, &asked);
                    }
                });
        })
        .section(StartPlace::AfterSections, |ui, _| {
            let _ = Text::new(tl!("Privacy").to_uppercase()).role(TypeRole::Label).show(ui);
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
            Asked::Open => app.open_dialog(),
            Asked::Unpin(id) => app.unpin_tool(id),
            // PLAN: assumed `PdfCraftApp::open_pin_tools(&mut self)` (pdf-dialogs-w9).
            Asked::PinTools => app.open_pin_tools(),
            Asked::CloseCommunityCard => app.community_card_closed = true,
            Asked::Link(id) => {
                app.execute(ui.ctx(), id);
            }
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
        // The tiles of Home's two tool grids; the All tools panel beside Home lists the tool too.
        let tiles: usize = [tl!("Recommended tools"), tl!("Pinned Tools")]
            .into_iter()
            .filter_map(|grid| h.query_by_role_and_label(egui::accesskit::Role::ListBox, grid))
            .map(|grid| grid.query_all_by_role_and_label(egui::accesskit::Role::ListItem, tl!(label)).count())
            .sum();
        assert_eq!(tiles, 1, "one tile for the tool");
    }
}
