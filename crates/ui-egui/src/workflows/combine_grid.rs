//! Combine files as a grid: a thumbnail of the first page each file contributes, drawn as a
//! stack when it contributes more than one, with its name below. Cards are selected and moved
//! with the same [`RowAction`]s as the table's rows, so both views share the selection, the
//! order and the undo history.
//!
//! Thumbnails render off the UI thread through [`RenderPool`] (its panic guard, size caps and
//! watchdog), one small pool per thumbnail. Only missing ones are asked for, those in view first.
//! At most [`MAX_THREADS`] render threads exist at once, counted as threads, not pools: the pools
//! start no watchdog replacements ([`RenderPool::without_replacements`]), and a pool's threads
//! count until they have actually exited ([`RenderPool::retire`]), so a render the watchdog gave
//! up on, or one whose file left the list, holds its place until its thread is gone. A thread
//! stuck for good in one step that can't be interrupted keeps its place for good: threads can't
//! be killed safely. On the web the pool renders on the UI thread inside `try_recv`, with no
//! watchdog: there, at most one thumbnail per frame, only one waits at a time (its pool holds the
//! parsed document), and a very slow page still stalls that frame (as in the document view).

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, TextureHandle, Vec2, pos2, vec2};
use pdfcraft_render::{RenderConfig, RenderPool, RenderRequest, Retiring};

use crate::PdfCraftApp;
use crate::combine_ui::{CombineFile, ERROR, Lock, RowAction, WARNING};
use crate::icons;
use crate::theme::{self, Tokens};

mod card;
mod thumbs;

use card::Card;
pub(crate) use card::preview;
pub use thumbs::ThumbState;
pub(crate) use thumbs::Thumbs;
#[cfg(test)]
use thumbs::{Entry, Job, Look, MAX_SIDE, MAX_THREADS, Want};
use thumbs::{Key, PREVIEW, Slot};
/// The box a page is fitted in at 100%, and the room the card adds around it (for the hover bar
/// and the stack above it, the name and a note below), which doesn't scale.
const PAPER: Vec2 = vec2(132.0, 172.0);
const AROUND: Vec2 = vec2(52.0, 90.0);
/// From the top of a card to its page box: room for the hover bar, which sits mostly above the
/// page.
const ABOVE: f32 = 32.0;
/// The hover bar's height; a quarter of it overlaps the page.
const BAR_H: f32 = 34.0;
/// The grid's zoom range, and the sizes its buttons and keys step through (the slider, the wheel
/// and a pinch go anywhere between).
pub(crate) const ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.6..=5.0;
const ZOOM_STEPS: [f32; 11] = [0.6, 0.8, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0];

/// The next size up (or down) from `zoom`; the end of the range stays put.
pub(crate) fn zoom_step(zoom: f32, larger: bool) -> f32 {
    let next =
        if larger { ZOOM_STEPS.iter().copied().find(|s| *s > zoom + 0.001) } else { ZOOM_STEPS.iter().rev().copied().find(|s| *s < zoom - 0.001) };
    next.unwrap_or(if larger { *ZOOM_RANGE.end() } else { *ZOOM_RANGE.start() })
}
/// Space above the first row of cards.
const TOP: f32 = 12.0;
/// The most pages one file can be spread out into (each is a card, laid out every frame).
pub(crate) const MAX_EXPAND: usize = 2000;
/// The shape drawn for a page whose size is unknown (US Letter); nothing is rendered for it.
const FALLBACK_PAGE: (f32, f32) = (612.0, 792.0);
/// Icons drawn on the (always white) paper, in either theme: the light theme's muted text, which
/// keeps its contrast on white where the dark theme's doesn't.
const ON_PAPER: Color32 = Color32::from_rgb(0x5E, 0x5E, 0x66);

/// A card being dragged: which file, and the list as it was (see [`RowAction::DropAt`]).
#[derive(Clone, Copy)]
struct GridDrag {
    file: u64,
    revision: u64,
    /// A page card's page (entry id, place among its pages): that page moves, not its file.
    page: Option<(u64, usize)>,
}

/// `name` shortened in the middle to what `fits`, keeping its extension ("Pilot Boat Ad….pdf").
pub(crate) fn fit_name(name: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(name) {
        return name.to_owned();
    }
    // A short extension after the last dot (a dot is one byte, so these are char boundaries).
    let split = name.rfind('.').filter(|p| *p > 0 && name.len() - p <= 6);
    let (stem, ext) = match split {
        Some(p) => (name.get(..p).unwrap_or(name), name.get(p..).unwrap_or("")),
        None => (name, ""),
    };
    let chars: Vec<char> = stem.chars().collect();
    let with = |n: usize| format!("{}…{ext}", chars.iter().take(n).collect::<String>());
    // The longest start of the stem that fits, found by halving.
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if fits(&with(mid)) { lo = mid } else { hi = mid - 1 }
    }
    with(lo)
}

/// The box a page is fitted in at `zoom` (clamped to [`ZOOM_RANGE`]).
fn paper_box(zoom: f32) -> Vec2 {
    let zoom = if zoom.is_finite() { zoom.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end()) } else { 1.0 };
    PAPER * zoom
}

/// The size to draw a page of `w` × `h` points in `fit`; `None` for a size that isn't one.
fn paper_size(w: f32, h: f32, fit: Vec2) -> Option<Vec2> {
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let s = (fit.x / w).min(fit.y / h);
    s.is_finite().then(|| vec2(w * s, h * s))
}

/// How many cards of `cell` fit across `width` (at least one).
fn columns_in(width: f32, cell: Vec2) -> usize {
    ((width - 16.0) / cell.x).floor().max(1.0) as usize
}

/// What one place in the grid shows: a file's card, or (the file expanded) one of the pages it
/// adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    /// The file, by its place in the list.
    pub(crate) file: usize,
    /// For a page: its place among the pages the file adds, the page (0-based, in the file) and
    /// how many it adds.
    pub(crate) page: Option<(usize, usize, usize)>,
}

/// The pages an expanded file shows: in its range's order, and sorted (to look one up), for the
/// range and page count they were worked out for.
#[derive(Clone, Debug)]
pub(crate) struct PageList {
    of: (String, usize),
    pub(crate) order: Arc<[usize]>,
    sorted: Arc<[usize]>,
}

/// The pages an expanded file shows, worked out again only when its range or page count
/// changes; `None` when it can't be shown as its pages (locked, unreadable, a bad range, no page,
/// or more than [`MAX_EXPAND`]).
pub(crate) fn pages_shown(cache: &mut HashMap<u64, PageList>, f: &CombineFile) -> Option<PageList> {
    if f.lock.is_some() || f.problem.is_some() {
        return None;
    }
    let list = match cache.get(&f.id) {
        Some(list) if list.of.0 == f.range && list.of.1 == f.pages => list.clone(),
        _ => {
            let order = f.pages_taken().ok()?;
            let mut sorted = order.clone();
            sorted.sort_unstable();
            let list = PageList { of: (f.range.clone(), f.pages), order: Arc::from(order), sorted: Arc::from(sorted) };
            cache.insert(f.id, list.clone());
            list
        }
    };
    // (One page shows as a page card too: a file split into parts may leave one page in a part.)
    (!list.order.is_empty() && list.order.len() <= MAX_EXPAND).then_some(list)
}

/// The grid's places, in order: each file's card, or its pages when it is expanded.
fn layout(app: &mut PdfCraftApp) -> Vec<Cell> {
    let tab = &mut app.combine_tab;
    let mut cells = Vec::with_capacity(app.combine_draft.len());
    for (file, f) in app.combine_draft.iter().enumerate() {
        match tab.expanded.contains(&f.id).then(|| pages_shown(&mut tab.page_lists, f)).flatten() {
            Some(list) => {
                let of = list.order.len();
                cells.extend(list.order.iter().enumerate().map(|(nth, &page)| Cell { file, page: Some((nth, page, of)) }));
            }
            None => cells.push(Cell { file, page: None }),
        }
    }
    cells
}

/// The file gap (0 = before the first file, n = after the last) a drop before place `at` means: a
/// drop inside an expanded file's pages goes to the nearer end of that file.
fn file_gap(cells: &[Cell], at: usize, files: usize) -> usize {
    match cells.get(at) {
        None => files,
        Some(Cell { file, page: Some((nth, _, of)) }) if *nth > 0 => {
            if nth.saturating_mul(2) < *of {
                *file
            } else {
                file.saturating_add(1)
            }
        }
        Some(c) => c.file,
    }
}

/// The grid of files, in the card that holds the list. Returns what the user did.
pub(crate) fn grid(
    app: &mut PdfCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    checks: &[Result<usize, String>],
    selected: &[bool],
    twice: &[bool],
) -> Option<RowAction> {
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let mut action = None;
    let revision = app.combine_tab.revision;
    let drag = egui::DragAndDrop::payload::<GridDrag>(&ctx).map(|d| *d);
    // The card dragged may scroll out of the rows drawn, so its release is watched here.
    let released = drag.is_some() && ctx.input(|i| i.pointer.any_released());
    let n = app.combine_draft.len();
    let places = layout(app);
    // Each file's first place (for the keys and to scroll to it).
    let mut first_place = vec![0usize; n];
    for (at, c) in places.iter().enumerate().rev() {
        if let Some(slot) = first_place.get_mut(c.file) {
            *slot = at;
        }
    }
    app.combine_tab.grid_places = places.iter().map(|c| c.file).collect();
    // Pages counted over the list (an entry that can't be split counting as one, see
    // `moved_pages`): where each place's pages start, for a page dropped before it.
    let counts: Vec<usize> = app
        .combine_draft
        .iter_mut()
        .map(|f| if f.lock.is_none() && f.problem.is_none() { f.selection().ok().filter(|k| *k > 0).unwrap_or(1) } else { 1 })
        .collect();
    let mut page_at = Vec::with_capacity(places.len());
    let mut total_pages = 0usize;
    for c in &places {
        page_at.push(total_pages);
        total_pages = total_pages.saturating_add(if c.page.is_some() { 1 } else { counts.get(c.file).copied().unwrap_or(1) });
    }
    let picked_pages = app.combine_tab.selected_pages.clone();
    let reveal = app.combine_tab.reveal.take();
    let fit = paper_box(app.combine_zoom);
    let cell = fit + AROUND;
    let mut cells: Vec<(usize, Rect)> = Vec::new();
    let viewport = ui.available_rect_before_wrap();
    let pointer = ctx.pointer_latest_pos();
    let files = &mut app.combine_draft;
    let thumbs = &mut app.combine_thumbs;
    let columns = &mut app.combine_tab.grid_columns;

    // Over the grid itself, not over a menu or dialog above it.
    let over_grid = ui.rect_contains_pointer(viewport);
    let mut width = viewport.width();
    let mut focused_card = None;
    // A zoom in the previous frame asks for a new scroll position. 0.37 keeps `State`'s offset
    // private, so the request is applied through the builder and consumed here.
    let pending_offset = ctx.data_mut(|d| d.remove_temp::<Vec2>(pending_offset_id()));
    let mut grid = egui::ScrollArea::vertical().id_salt("combine-grid").auto_shrink([false, false]);
    if let Some(offset) = pending_offset {
        grid = grid.vertical_scroll_offset(offset.y);
    }
    grid.show_viewport(ui, |ui, clip| {
        // Dragging near the top or bottom edge scrolls.
        if drag.is_some()
            && let Some(p) = pointer
            && viewport.contains(p)
        {
            let edge = 40.0;
            let delta = if p.y < viewport.top() + edge {
                12.0
            } else if p.y > viewport.bottom() - edge {
                -12.0
            } else {
                0.0
            };
            if delta != 0.0 {
                ui.scroll_with_delta(vec2(0.0, delta));
                ctx.request_repaint();
            }
        }
        ui.add_space(TOP);
        width = ui.available_width();
        let cols = columns_in(width, cell);
        *columns = cols;
        let rows = places.len().div_ceil(cols);
        let left = ((width - cols as f32 * cell.x) / 2.0).max(0.0);
        let (area, _) = ui.allocate_exact_size(vec2(width, rows as f32 * cell.y), Sense::hover());
        let cell_rect = |i: usize| {
            let (row, col) = (i / cols, i % cols);
            Rect::from_min_size(pos2(area.left() + left + col as f32 * cell.x, area.top() + row as f32 * cell.y), cell)
        };
        if let Some(i) = reveal.and_then(|id| files.iter().position(|f| f.id == id)) {
            ui.scroll_to_rect(cell_rect(first_place.get(i).copied().unwrap_or(0)), None);
        }
        for row in crate::canvas::thumbnail_rows(clip.top() - TOP, clip.bottom() - TOP, cell.y, rows) {
            let first = row.saturating_mul(cols);
            let in_row = places.get(first..first.saturating_add(cols).min(places.len())).unwrap_or_default();
            // An expanded file's pages on this row share a tinted band, so they read as one file.
            let mut run = 0;
            while run < in_row.len() {
                let Some(start) = in_row.get(run).copied() else { break };
                let mut end = run;
                while in_row.get(end.saturating_add(1)).is_some_and(|c| c.file == start.file && c.page.is_some()) {
                    end = end.saturating_add(1);
                }
                if start.page.is_some() {
                    let band = cell_rect(first.saturating_add(run)).union(cell_rect(first.saturating_add(end))).shrink2(vec2(2.0, 4.0));
                    ui.painter().rect_filled(band, CornerRadius::same(10), t.accent_soft.gamma_multiply(0.45));
                }
                run = end.saturating_add(1);
            }
            for (col, place) in in_row.iter().enumerate() {
                let i = first.saturating_add(col);
                let Some(f) = files.get_mut(place.file) else { break };
                let c = cell_rect(i);
                cells.push((i, c));
                let card = Card {
                    i: place.file,
                    page: place.page,
                    rect: c,
                    check: checks.get(place.file).cloned().unwrap_or(Ok(f.pages)),
                    selected: selected.get(place.file).copied().unwrap_or(false)
                        || place.page.is_some_and(|(nth, ..)| picked_pages.contains(&(f.id, nth))),
                    twice: twice.get(place.file).copied().unwrap_or(false),
                    revision,
                    ppp,
                    fit,
                };
                let card_id = card.id(ui, f);
                if let Some(a) = card.show(ui, t, f, thumbs) {
                    action = Some(a);
                }
                if ui.memory(|m| m.has_focus(card_id)) {
                    focused_card = Some((card_id, f.id, place.page.map_or(0, |p| p.0)));
                }
            }
        }
        // While a page card is dragged: the page gap it would go to (any place, between any two
        // pages), drawn at the nearest card's edge, and how many pages go.
        if let Some(d) = drag
            && let Some(page) = d.page
            && let Some(p) = pointer
        {
            let at = crate::canvas::drop_gap(&cells, p);
            let nearest = cells.iter().min_by(|(_, a), (_, b)| a.distance_sq_to_pos(p).total_cmp(&b.distance_sq_to_pos(p)));
            if let Some((_, r)) = nearest {
                let x = if p.x < r.center().x { r.left() + 2.0 } else { r.right() - 2.0 };
                ui.painter().line_segment([pos2(x, r.top() + 10.0), pos2(x, r.bottom() - 30.0)], Stroke::new(3.0, t.accent));
            }
            let moving = if picked_pages.contains(&page) { picked_pages.len() } else { 1 };
            let label = if moving == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &moving.to_string())]) };
            ui.painter().text(p + vec2(14.0, 14.0), Align2::LEFT_TOP, label, theme::medium(12.0), t.accent_text);
            if released
                && viewport.contains(p)
                && let Some(at) = at
            {
                let gap = page_at.get(at).copied().unwrap_or(total_pages);
                action = Some(RowAction::MovePages { page, revision: d.revision, gap });
            }
        } else if let Some(d) = drag
            && let Some(p) = pointer
        {
            // A file's card: the file gap it would go to, drawn as a bar, and how many files go.
            let gap = crate::canvas::drop_gap(&cells, p).map(|at| file_gap(&places, at, n));
            // Drawn at the end of the file before the gap or the start of the one after (an
            // expanded file's pages are never split), whichever is nearer the pointer.
            if let Some(gap) = gap {
                let before = gap
                    .checked_sub(1)
                    .and_then(|f| first_place.get(f.saturating_add(1)).map(|next| next.saturating_sub(1)).or(places.len().checked_sub(1)));
                let after = first_place.get(gap).copied();
                let ends = [before.map(|at| cell_rect(at).right() - 2.0).zip(before), after.map(|at| cell_rect(at).left() + 2.0).zip(after)];
                let nearest = ends.into_iter().flatten().min_by(|(xa, a), (xb, b)| {
                    let da = pos2(*xa, cell_rect(*a).center().y).distance_sq(p);
                    let db = pos2(*xb, cell_rect(*b).center().y).distance_sq(p);
                    da.total_cmp(&db)
                });
                if let Some((x, at)) = nearest {
                    let r = cell_rect(at);
                    ui.painter().line_segment([pos2(x, r.top() + 10.0), pos2(x, r.bottom() - 30.0)], Stroke::new(3.0, t.accent));
                }
            }
            let moving = files
                .iter()
                .position(|f| f.id == d.file)
                .map_or(1, |i| if selected.get(i).copied().unwrap_or(false) { selected.iter().filter(|s| **s).count() } else { 1 });
            let label = if moving == 1 { tl!("1 file").to_string() } else { crate::i18n::fmt(tl!("{n} files"), &[("n", &moving.to_string())]) };
            ui.painter().text(p + vec2(14.0, 14.0), Align2::LEFT_TOP, label, theme::medium(12.0), t.accent_text);
            // Dropped in the grid: there. Anywhere else: nothing moves.
            if released
                && viewport.contains(p)
                && let Some(gap) = gap
            {
                action = Some(RowAction::DropAt { file: d.file, revision: d.revision, gap });
            }
        }
    });

    app.combine_tab.card_focus = focused_card.map(|(id, ..)| id);
    app.combine_tab.card_focus_at = focused_card.map(|(_, file, nth)| (file, nth));

    // Pinch, or Ctrl/⌘ with the wheel, over the grid zooms it.
    let pinch = crate::motion::zoom_delta(&ctx);
    if (pinch - 1.0).abs() > 0.001 && over_grid {
        app.combine_tab.zoom_request = Some(app.combine_zoom * pinch);
    }
    // A zoom asked for this frame (toolbar, keys, pinch) applies now the grid has been laid out:
    // the card under the pointer (or else the first in view) keeps its place on screen. Not while
    // a card is dragged: the grid under it would change.
    if let Some(zoom) = app.combine_tab.zoom_request.take()
        && drag.is_none()
    {
        let before = app.combine_zoom;
        app.set_combine_zoom(zoom);
        if app.combine_zoom != before {
            // The same card all through one gesture (a slider drag, a turn of the wheel): the
            // first in view changes as the cards reflow, and following it would drift. A card is
            // kept by its file and its place among the file's pages.
            let now = ctx.input(|i| i.time);
            let gesture = app.combine_tab.zoom_anchor.filter(|a| now - a.3 < 0.5).and_then(|(id, nth, above, _)| {
                let file = app.combine_draft.iter().position(|f| f.id == id)?;
                Some((first_place.get(file)?.saturating_add(nth).min(places.len().saturating_sub(1)), above))
            });
            // Cards just outside the view are drawn too (under the toolbar, say): the pointer
            // counts only over the grid.
            let under = pointer.filter(|_| over_grid).and_then(|p| cells.iter().find(|(_, r)| r.contains(p)));
            let anchor = gesture
                .or_else(|| under.or_else(|| cells.iter().find(|(_, r)| r.bottom() > viewport.top())).map(|(i, r)| (*i, r.top() - viewport.top())));
            app.combine_tab.zoom_anchor = anchor.and_then(|(i, above)| {
                let place = places.get(i)?;
                Some((app.combine_draft.get(place.file)?.id, place.page.map_or(0, |p| p.0), above, now))
            });
            if let Some((i, above)) = anchor {
                // Where it will be: its row at the new size, as far below the top as it is now;
                // within what can be scrolled to, so zooming out at the bottom shows no gap.
                let cell = paper_box(app.combine_zoom) + AROUND;
                let cols = columns_in(width, cell);
                let content = TOP + places.len().div_ceil(cols) as f32 * cell.y;
                let y = (TOP + (i / cols) as f32 * cell.y - above).min(content - viewport.height()).max(0.0);
                // Applied by the next frame's scroll area, before anything is drawn there: no
                // jump, and no scroll still animating (to a card revealed by the keys, say) can
                // carry it elsewhere. The request is consumed once.
                ctx.data_mut(|d| d.insert_temp(pending_offset_id(), vec2(0.0, y)));
            }
            ctx.request_repaint();
        }
    }

    action
}

/// The scroll position a zoom asked the next frame's grid to start at.
fn pending_offset_id() -> egui::Id {
    egui::Id::unique("pdfcraft-combine-grid-offset")
}

impl PdfCraftApp {
    /// The file shown large, the page on screen (0-based, in the file) and its render, while it
    /// shows (tests and the control channel).
    pub fn combine_preview(&self) -> Option<(String, usize, ThumbState)> {
        let p = self.combine_tab.preview.as_ref()?;
        let f = self.combine_draft.iter().find(|f| f.id == p.file)?;
        Some((f.name.clone(), p.page?, self.combine_thumbs.state(PREVIEW)))
    }

    /// Size the Combine grid's cards (1.0 = the usual); out-of-range values are clamped and
    /// nonsense is ignored.
    pub fn set_combine_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() {
            self.combine_zoom = zoom.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end());
        }
    }

    /// Every frame, whatever shows: take finished thumbnails, start those the grid asked for (only
    /// while it is drawn), and let go of what no longer matches the list, so renders under way
    /// finish and release their threads in the list view, on Home or in a document tab too.
    pub(crate) fn combine_thumbs_frame(&mut self, ctx: &egui::Context) {
        // What each thumbnail must match now (cards out of view included): each file's first page,
        // and an expanded file's pages (looked up in its sorted list, never listed one by one).
        // (By file: the parts a file is split into share its thumbnails.)
        struct Shown {
            source: usize,
            auth: u64,
            pages: Vec<usize>,
        }
        let mut shown: HashMap<u64, Shown> = HashMap::with_capacity(self.combine_draft.len());
        let tab = &mut self.combine_tab;
        for f in self.combine_draft.iter_mut() {
            let Ok(first) = f.first_page() else { continue };
            let sorted = tab.expanded.contains(&f.id).then(|| pages_shown(&mut tab.page_lists, f)).flatten().map(|l| l.sorted);
            let entry = shown.entry(f.group).or_insert_with(|| Shown { source: Arc::as_ptr(&f.bytes) as usize, auth: f.auth, pages: Vec::new() });
            entry.pages.push(first);
            entry.pages.extend(sorted.iter().flat_map(|s| s.iter().copied()));
        }
        for s in shown.values_mut() {
            s.pages.sort_unstable();
            s.pages.dedup();
        }
        // The page shown large, while it shows: leaving the tab closes it.
        if !self.combine_showing() {
            self.combine_tab.preview = None;
        }
        let preview = self.combine_tab.preview.as_ref().and_then(|p| {
            let page = p.page?;
            let f = self.combine_draft.iter().find(|f| f.id == p.file)?;
            Some(Key { source: Arc::as_ptr(&f.bytes) as usize, auth: f.auth, page, px: [0, 0] })
        });
        let current = |slot: Slot| -> Option<Key> {
            if slot == PREVIEW {
                return preview;
            }
            let (id, page) = slot;
            let s = shown.get(&id)?;
            s.pages.binary_search(&page).is_ok().then_some(Key { source: s.source, auth: s.auth, page, px: [0, 0] })
        };
        self.combine_thumbs.finish(ctx, &current);
    }

    /// The single pages selected: each one's entry (by place in the list) and page (0-based, in
    /// the file), in list order (tests and the control channel).
    pub fn combine_selected_pages(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (i, f) in self.combine_draft.iter().enumerate() {
            let pages = f.pages_taken().unwrap_or_default();
            for (nth, page) in pages.iter().enumerate() {
                if self.combine_tab.selected_pages.contains(&(f.id, nth)) {
                    out.push((i, *page));
                }
            }
        }
        out
    }

    /// Each entry's name and the pages it takes (0-based, in order): a file split into parts is
    /// several entries (tests and the control channel).
    pub fn combine_parts_shown(&self) -> Vec<(String, Vec<usize>)> {
        self.combine_draft.iter().map(|f| (f.name.clone(), f.pages_taken().unwrap_or_default())).collect()
    }

    /// The files shown as their pages in the grid, by place in the list (tests and the control
    /// channel).
    pub fn combine_expanded(&self) -> Vec<usize> {
        self.combine_draft.iter().enumerate().filter(|(_, f)| self.combine_tab.expanded.contains(&f.id)).map(|(i, _)| i).collect()
    }

    /// An expanded file's page cards: each page (0-based, in the file) and its thumbnail; empty
    /// for a file shown as its card (tests and the control channel).
    pub fn combine_page_thumbnails(&self, file: usize) -> Vec<(usize, ThumbState)> {
        let Some(f) = self.combine_draft.get(file).filter(|f| self.combine_tab.expanded.contains(&f.id)) else { return Vec::new() };
        let pages = f.pages_taken().unwrap_or_default();
        pages.into_iter().map(|p| (p, self.combine_thumbs.state((f.group, p)))).collect()
    }

    /// Each listed file's grid thumbnail, in list order (tests and the control channel).
    pub fn combine_thumbnails(&self) -> Vec<ThumbState> {
        self.combine_draft.iter().map(|f| f.first_taken().map_or(ThumbState::None, |p| self.combine_thumbs.state((f.group, p)))).collect()
    }

    /// Thumbnail renders under way (tests: never more than a few).
    pub fn combine_thumbnails_rendering(&self) -> usize {
        self.combine_thumbs.jobs.len()
    }

    /// Render threads for thumbnails that exist now (counted from the threads themselves),
    /// including those of finished or abandoned renders that haven't exited yet (tests: never
    /// more than three).
    pub fn combine_thumbnail_threads(&mut self) -> usize {
        self.combine_thumbs.threads()
    }

    /// Whether no thumbnail render is under way and every render thread has exited (tests).
    pub fn combine_thumbnail_threads_exited(&self) -> bool {
        self.combine_thumbs.jobs.is_empty() && self.combine_thumbs.retiring.iter().all(Retiring::exited)
    }

    /// Thumbnail textures held (tests: none once the tab is closed).
    pub fn combine_thumbnail_textures(&self) -> usize {
        self.combine_thumbs.entries.values().filter(|e| e.tex.is_some()).count()
    }
}

/// The render configuration of a Combine thumbnail. Thumbnails are only shown on screen, so
/// installed CJK faces may draw fonts a PDF doesn't embed, as in the viewer; the combined file
/// itself is written from the original bytes and never from these rasters (AGENTS.md §1.4).
fn thumbnail_config(password: Option<Arc<str>>) -> RenderConfig {
    RenderConfig { password, ..Default::default() }.on_screen()
}

#[cfg(test)]
mod tests;
