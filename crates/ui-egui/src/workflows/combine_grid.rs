//! Combine files as a grid: a thumbnail of the first page each file contributes, drawn as a stack when
//! it contributes more than one, with its name below. The grid is the framework's [`TileGrid`] over
//! [`CombineKey`]s and binds the same selection as the table, so both views share the selection, the
//! order and the undo history; this module maps the grid's events onto the [`RowAction`]s of the list.
//!
//! Pictures are [`Thumbnails`](craft::ui::primitives::textures::Thumbnails) over `source::CombineThumbs`:
//! the framework keeps the textures, the lanes and the budgets, and the source renders a page through
//! [`pdfcraft_render::RenderPool`].

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use craft::behavior::{EditEvent, IconId, ReorderKeys, Selection, keyed};
use craft::ui::contract::{Bind, CollectionEvent, Entry, Props as _, ReorderMove, ReorderRule, ShowCollection as _, ShowContainer as _, SpaceAction};
use craft::ui::menu::Menu;
use craft::ui::primitives::textures::{TextureState, ThumbRequest};
use craft::ui::tile_grid::{GridEvent, GridZoom, Tile, TileActionPlace, TileGrid, TileGroup, TilePartEvent, TileZoom};
use pdfcraft_render::RenderConfig;

use crate::PdfCraftApp;
use crate::combine_ui::{CombineFile, CombineKey, CombineTab, RowAction};

mod card;
mod source;

pub(crate) use card::preview;
use card::{Act, Pictures};
use source::CombineThumbs;
pub(crate) use source::OpenDocuments;

/// The most pages one file can be spread out into (each is a tile, laid out every frame).
pub(crate) const MAX_EXPAND: usize = 2000;

/// The pages an expanded file shows, in its range's order.
#[derive(Clone, Debug)]
pub(crate) struct PageList {
    pub(crate) order: Arc<[usize]>,
}

/// The pages an expanded file shows; `None` when it can't be shown as its pages (locked, unreadable, a
/// bad range, no page, or more than [`MAX_EXPAND`]). One page shows as a page tile too: a file split
/// into parts may leave one page in a part.
pub(crate) fn pages_shown(f: &CombineFile) -> Option<PageList> {
    if f.lock.is_some() || f.problem.is_some() {
        return None;
    }
    // A whole file is listed only when it is short enough to be spread out.
    if f.range.trim().is_empty() && f.pages > MAX_EXPAND {
        return None;
    }
    let order = f.pages_taken().ok()?;
    (!order.is_empty() && order.len() <= MAX_EXPAND).then(|| PageList { order: Arc::from(order) })
}

/// What the grid shows, in display order.
struct Rows {
    /// The tiles: a collapsed file's, or each page of an expanded file.
    tiles: Vec<CombineKey>,
    /// Every row of the grid: the tiles, with a header row before each group. An expanded file shows no
    /// tile of its own, so its header is keyed by the file.
    rows: Vec<CombineKey>,
    /// One group per expanded file, over its pages.
    groups: Vec<TileGroup<CombineKey>>,
}

/// The rows of `draft`, the files in `lists` shown as their pages.
fn rows_of(draft: &[CombineFile], lists: &HashMap<u64, PageList>) -> Rows {
    let mut out = Rows { tiles: Vec::with_capacity(draft.len()), rows: Vec::with_capacity(draft.len()), groups: Vec::new() };
    for f in draft {
        match lists.get(&f.id) {
            Some(list) => {
                let detail = crate::i18n::fmt(tl!("Pages: {k} of {n}"), &[("k", &list.order.len().to_string()), ("n", &f.pages.to_string())]);
                out.groups
                    .push(TileGroup::new(CombineKey::File(f.id), out.tiles.len(), list.order.len(), f.name.clone()).detail(detail).selectable());
                out.rows.push(CombineKey::File(f.id));
                for nth in 0..list.order.len() {
                    out.tiles.push(CombineKey::Page(f.id, nth));
                    out.rows.push(CombineKey::Page(f.id, nth));
                }
            }
            None => {
                out.tiles.push(CombineKey::File(f.id));
                out.rows.push(CombineKey::File(f.id));
            }
        }
    }
    out
}

/// Where a grid reorder lands, in the list's own indexes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Landing {
    /// Files before entry `gap` of the list (0 = before the first, the list's length = after the last).
    Files { gap: usize },
    /// Pages before item `gap` of every page of the list, collapsed files' pages included.
    Pages { gap: usize },
}

/// Where `mv` lands. `ReorderMove::to` counts the grid's `rows` that remain once the moved keys are taken
/// out, header rows included, so the target is the row now at `to`. `ids` are the entries of the list and
/// `items` how many page items each contributes.
fn landing(rows: &[CombineKey], mv: &ReorderMove<CombineKey>, ids: &[u64], items: &[usize]) -> Landing {
    let target = rows.iter().filter(|k| !mv.keys.contains(k)).nth(mv.to);
    let entry = |id: u64| ids.iter().position(|i| *i == id);
    let total = || items.iter().fold(0usize, |sum, n| sum.saturating_add(*n));
    // The first item of entry `at`.
    let start = |at: usize| items.iter().take(at).fold(0usize, |sum, n| sum.saturating_add(*n));
    match mv.key {
        CombineKey::File(_) => {
            let gap = match target {
                Some(CombineKey::File(id) | CombineKey::Page(id, _)) => entry(*id),
                None => None,
            };
            Landing::Files { gap: gap.unwrap_or(ids.len()) }
        }
        CombineKey::Page(..) => {
            let gap = match target {
                Some(CombineKey::Page(id, nth)) => entry(*id).map(|at| start(at).saturating_add(*nth)),
                // A tile or a header of a file: that entry's first item.
                Some(CombineKey::File(id)) => entry(*id).map(start),
                None => None,
            };
            Landing::Pages { gap: gap.unwrap_or_else(total) }
        }
    }
}

/// What the grid reads of the list while its events are mapped.
struct View<'a> {
    draft: &'a [CombineFile],
    /// Each entry's place in the list, by id.
    index: HashMap<u64, usize>,
    /// The pages of the files shown as their pages.
    lists: &'a HashMap<u64, PageList>,
    /// What each entry's range takes, in list order.
    checks: &'a [Result<usize, String>],
}

impl View<'_> {
    fn file(&self, id: u64) -> Option<(usize, &CombineFile)> {
        let at = *self.index.get(&id)?;
        self.draft.get(at).map(|f| (at, f))
    }

    /// The menu row of `act` on `key`: a file's Remove names the file, as production's menu does.
    fn act_label(&self, act: Act, key: &CombineKey) -> String {
        match (act, key) {
            (Act::Remove, CombineKey::File(id)) => match self.file(*id) {
                Some((_, file)) => crate::i18n::fmt(tl!("Remove {name}"), &[("name", &file.name)]),
                None => act.label().to_owned(),
            },
            _ => act.label().to_owned(),
        }
    }

    /// The actions the tile (or header) of `key` offers, in the order its band lists them.
    fn acts(&self, key: &CombineKey) -> Vec<Act> {
        match *key {
            CombineKey::File(id) => {
                let Some((at, f)) = self.file(id) else { return Vec::new() };
                if self.lists.contains_key(&id) {
                    return vec![Act::Collapse, Act::Remove];
                }
                let check = self.checks.get(at).cloned().unwrap_or(Ok(f.pages));
                card::file_acts(f, &check)
            }
            CombineKey::Page(id, nth) => {
                let Some((_, f)) = self.file(id) else { return Vec::new() };
                let Some(page) = self.lists.get(&id).and_then(|l| l.order.get(nth)) else { return Vec::new() };
                card::page_acts(f, *page)
            }
        }
    }

    /// How many page items each entry of the list contributes: its pages, or one when it moves whole (a
    /// locked file, a bad range).
    fn items(&self) -> Vec<usize> {
        self.draft
            .iter()
            .map(|f| {
                let pages = (f.lock.is_none() && f.problem.is_none()).then(|| f.pages_taken().ok()).flatten().filter(|p| !p.is_empty());
                pages.map_or(1, |p| p.len())
            })
            .collect()
    }
}

/// A row of a tile's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TileRow {
    Act(Act),
    Unlock,
}

/// The list action for `act` on the tile of `key`.
fn act_action(act: Act, key: CombineKey) -> RowAction {
    match (act, key) {
        (Act::Preview, CombineKey::File(id)) => RowAction::Preview(id),
        (Act::Preview, CombineKey::Page(id, nth)) => RowAction::PreviewAt(id, nth),
        (Act::Expand, CombineKey::File(id) | CombineKey::Page(id, _)) => RowAction::Expand(id, true),
        (Act::Collapse, CombineKey::File(id) | CombineKey::Page(id, _)) => RowAction::Expand(id, false),
        (Act::Remove, CombineKey::File(id)) => RowAction::RemoveFile(id),
        (Act::Remove, CombineKey::Page(id, nth)) => RowAction::RemovePages(Some((id, nth))),
    }
}

/// What Delete removes: the selection, whichever tile the keyboard is on; nothing while nothing is selected (a page
/// let go of when its file collapsed or an undo took it, files picked through automation).
fn delete_action(selection: &Selection<CombineKey>) -> Option<RowAction> {
    Some(match selection.items.first()? {
        CombineKey::File(_) => RowAction::Remove,
        CombineKey::Page(..) => RowAction::RemovePages(None),
    })
}

/// The grid of files, in the card that holds the list. `checks` is what each entry's range takes and
/// `twice` whether its file is in the list again, both in list order. Returns what the user did, in order.
// PLAN: the signature is `(app, ui, checks, twice) -> Vec<RowAction>`: the table's colours and selection
// flags are gone, and one pass can report several actions (owner of the caller: pdf-workflows-w6).
pub(crate) fn grid(app: &mut PdfCraftApp, ui: &mut egui::Ui, checks: &[Result<usize, String>], twice: &[bool]) -> Vec<RowAction> {
    let ctx = ui.ctx().clone();
    let lists = app.combine_page_lists();
    let rows = rows_of(&app.combine_draft, &lists);
    let source = CombineThumbs::new(&app.combine_draft, &lists, app.combine_tab.open_documents.clone());
    let thumbs = source::thumbnails(&ctx);
    let pictures = Pictures { thumbs: &thumbs, source: &source };
    let view =
        View { draft: &app.combine_draft, index: app.combine_draft.iter().enumerate().map(|(at, f)| (f.id, at)).collect(), lists: &lists, checks };
    let tiles = keyed(&rows.tiles, |key: &CombineKey| *key);

    let shown = TileGrid::<CombineKey>::new(tl!("Files").to_string())
        .id(crate::combine_ui::collection_id())
        .card()
        .detail_line()
        .selection(Bind::bound(&mut app.combine_tab.selection))
        .zoom(GridZoom::Bound(Bind::bound(&mut app.combine_zoom), TileZoom::Ladder))
        .groups(&rows.groups)
        .collapsible()
        .reorder(ReorderRule::new().keys(ReorderKeys::AltArrows))
        .deletable(true)
        .action_place(TileActionPlace::Band)
        .space(SpaceAction::Preview)
        .show_items(ui, &tiles, &mut |key, _| {
            let tile = match *key {
                CombineKey::File(id) => view.file(id).map(|(at, f)| {
                    let check = checks.get(at).cloned().unwrap_or(Ok(f.pages));
                    card::file_tile(f, &check, twice.get(at).copied().unwrap_or(false), pictures)
                }),
                CombineKey::Page(id, nth) => view.file(id).and_then(|(_, f)| {
                    let page = *lists.get(&id)?.order.get(nth)?;
                    Some(card::page_tile(f, nth, page, pictures))
                }),
            };
            tile.unwrap_or_else(|| Tile::new(String::new()))
        });

    let mut actions = Vec::new();
    let mut selection_changed = false;
    for event in shown.response.events {
        match event {
            CollectionEvent::Selection(_) => selection_changed = true,
            CollectionEvent::Reorder(EditEvent::Commit { after: mv, .. } | EditEvent::Set { after: mv, .. }) => {
                let revision = app.combine_tab.revision;
                let ids: Vec<u64> = app.combine_draft.iter().map(|f| f.id).collect();
                actions.extend(match (mv.key, landing(&rows.rows, &mv, &ids, &view.items())) {
                    (CombineKey::File(file), Landing::Files { gap }) => Some(RowAction::DropAt { file, revision, gap }),
                    (CombineKey::Page(id, nth), Landing::Pages { gap }) => Some(RowAction::MovePages { page: (id, nth), revision, gap }),
                    _ => None,
                });
            }
            CollectionEvent::Delete(_) => actions.extend(delete_action(&app.combine_tab.selection)),
            CollectionEvent::Preview(CombineKey::File(id)) => actions.push(RowAction::Preview(id)),
            CollectionEvent::Preview(CombineKey::Page(id, nth)) => actions.push(RowAction::PreviewAt(id, nth)),
            CollectionEvent::Item(key, TilePartEvent::Action(at)) => {
                actions.extend(view.acts(&key).get(at).map(|act| act_action(*act, key)));
            }
            _ => {}
        }
    }
    // A right-click offers the tile's own actions, and Unlock… on a locked file, for the tile it was opened on.
    if let Some(key) = shown.response.menu.as_ref().map(|request| request.key) {
        let mut entries: Vec<Entry<TileRow>> =
            view.acts(&key).into_iter().map(|act| Entry::new(TileRow::Act(act), view.act_label(act, &key)).icon(act.icon())).collect();
        let locked = match key {
            CombineKey::File(id) => view.file(id).is_some_and(|(_, f)| f.lock.is_some()),
            CombineKey::Page(..) => false,
        };
        if locked {
            entries.push(Entry::new(TileRow::Unlock, tl!("Unlock…")).icon(IconId::new("lock-open")));
        }
        match Menu::context(tl!("File"), &shown.response.response).pick(ui, &entries).picked {
            Some(TileRow::Act(act)) => actions.push(act_action(act, key)),
            Some(TileRow::Unlock) => {
                if let CombineKey::File(id) = key {
                    actions.push(RowAction::Unlock(Some(id)));
                }
            }
            None => {}
        }
    }
    for event in shown.events {
        match event {
            GridEvent::GroupOpen { group, open, .. } => {
                if let Some(CombineKey::File(id)) = rows.groups.get(group).map(|g| g.key) {
                    actions.push(RowAction::Expand(id, open));
                }
            }
            _ => {}
        }
    }
    // The pick that was just made decides which kind of tile stays selected.
    if selection_changed {
        app.combine_keep_one_kind();
    }
    actions
}

impl PdfCraftApp {
    /// The pages of each file shown as its pages in the grid, by entry id.
    fn combine_page_lists(&self) -> HashMap<u64, PageList> {
        let expanded = &self.combine_tab.expanded;
        self.combine_draft.iter().filter(|f| expanded.contains(&f.id)).filter_map(|f| pages_shown(f).map(|list| (f.id, list))).collect()
    }

    /// The thumbnail source for the list as it is now.
    fn combine_thumb_source(&self) -> CombineThumbs {
        CombineThumbs::new(&self.combine_draft, &self.combine_page_lists(), self.combine_tab.open_documents.clone())
    }

    /// What `key` shows now at the size `request` asks, read without starting a render.
    fn combine_thumb_state(&self, ctx: &egui::Context, source: &CombineThumbs, key: CombineKey, request: &ThumbRequest) -> TextureState {
        source.state_of(ctx, &key, request)
    }

    /// The file shown large and the page on screen (0-based, in the file), while it shows (tests and the
    /// control channel).
    pub fn combine_preview(&self) -> Option<(String, usize)> {
        let p = self.combine_tab.preview.as_ref()?;
        let f = self.combine_draft.iter().find(|f| f.id == p.file)?;
        Some((f.name.clone(), p.page?))
    }

    /// Size the Combine grid's cards (1.0 = the usual); out-of-range values are clamped and nonsense is
    /// ignored.
    pub fn set_combine_zoom(&mut self, zoom: f64) {
        if zoom.is_finite() {
            let range = TileZoom::Ladder.range();
            self.combine_zoom = zoom.clamp(*range.start(), *range.end());
        }
    }

    /// Closes the preview: each frame the tab isn't showing ends it, so coming back does not reopen it.
    pub(crate) fn close_combine_preview(&mut self) {
        self.combine_tab.preview = None;
    }

    /// The single pages selected: each one's entry (by place in the list) and page (0-based, in the
    /// file), in list order (tests and the control channel).
    pub fn combine_selected_pages(&self) -> Vec<(usize, usize)> {
        let picked: BTreeSet<(u64, usize)> = self
            .combine_tab
            .selection
            .items
            .iter()
            .filter_map(|key| match key {
                CombineKey::Page(id, nth) => Some((*id, *nth)),
                CombineKey::File(_) => None,
            })
            .collect();
        let mut out = Vec::new();
        for (i, f) in self.combine_draft.iter().enumerate() {
            if !picked.iter().any(|(id, _)| *id == f.id) {
                continue;
            }
            for (nth, page) in f.pages_taken().unwrap_or_default().iter().enumerate() {
                if picked.contains(&(f.id, nth)) {
                    out.push((i, *page));
                }
            }
        }
        out
    }

    /// Each entry's name and the pages it takes (0-based, in order): a file split into parts is several
    /// entries (tests and the control channel).
    pub fn combine_parts_shown(&self) -> Vec<(String, Vec<usize>)> {
        self.combine_draft.iter().map(|f| (f.name.clone(), f.pages_taken().unwrap_or_default())).collect()
    }

    /// The files shown as their pages in the grid, by place in the list (tests and the control channel).
    pub fn combine_expanded(&self) -> Vec<usize> {
        self.combine_draft.iter().enumerate().filter(|(_, f)| self.combine_tab.expanded.contains(&f.id)).map(|(i, _)| i).collect()
    }

    /// An expanded file's page tiles: each page (0-based, in the file) and its thumbnail; empty for a file
    /// shown as its tile (tests and the control channel).
    pub fn combine_page_thumbnails(&self, ctx: &egui::Context, file: usize, request: &ThumbRequest) -> Vec<(usize, TextureState)> {
        let Some(f) = self.combine_draft.get(file).filter(|f| self.combine_tab.expanded.contains(&f.id)) else { return Vec::new() };
        let source = self.combine_thumb_source();
        let pages = f.pages_taken().unwrap_or_default();
        pages
            .into_iter()
            .enumerate()
            .map(|(nth, page)| (page, self.combine_thumb_state(ctx, &source, CombineKey::Page(f.id, nth), request)))
            .collect()
    }

    /// Each listed file's grid thumbnail, in list order (tests and the control channel).
    pub fn combine_thumbnails(&self, ctx: &egui::Context, request: &ThumbRequest) -> Vec<TextureState> {
        let source = self.combine_thumb_source();
        self.combine_draft.iter().map(|f| self.combine_thumb_state(ctx, &source, CombineKey::File(f.id), request)).collect()
    }
}

<<<<<<< HEAD
/// Lets go of every thumbnail and open document of a Combine tab that closed.
pub(crate) fn release_thumbnails(ctx: &egui::Context, tab: &CombineTab) {
    source::thumbnails(ctx).retain(ctx, |_| false);
    source::lock(&tab.open_documents).clear();
}

/// The render configuration of a Combine thumbnail. Thumbnails are only shown on screen, so installed CJK
/// faces may draw fonts a PDF doesn't embed, as in the viewer; the combined file itself is written from
/// the original bytes and never from these rasters (AGENTS.md §1.4).
=======
/// The render configuration of a Combine thumbnail. Thumbnails are only shown on screen, so
/// installed CJK faces may draw fonts a PDF doesn't embed, as in the viewer; the combined file
/// itself is written from the original bytes and never from these rasters (docs/development.md "Fonts and attribution").
>>>>>>> upstream/main
fn thumbnail_config(password: Option<Arc<str>>) -> RenderConfig {
    RenderConfig { password, ..Default::default() }.on_screen()
}

#[cfg(test)]
mod tests;
