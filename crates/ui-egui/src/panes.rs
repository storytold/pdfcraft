//! Split view (`view.split-window`): the main window's page area in two panes, and which pane
//! shows each tab.
//!
//! Every tab is a [`DocView`](crate::DocView) in `PdfCraftApp::views`, and its [`Place`] says where
//! it shows. The window's tab strip holds the [`Place::Main`] tabs; split view adds a second pane,
//! to the right or below, with a small strip of its own ([`Place::Second`]). Each pane shows the
//! tab of it that was active last, and `PdfCraftApp::active` is the tab of the pane last clicked:
//! menus, keyboard shortcuts and the side panels act on it.
//!
//! One document can show in both panes at once (page 2 beside page 40): two views with one
//! `DocId`. The document, its edits and its undo history are the session's, so an edit made in one
//! view is the other's too ([`PdfCraftApp::settle_panes`] refreshes the other view), while each view
//! keeps its own scroll, zoom and selection. Closing one of them closes only that view.
//!
//! Nothing here is kept across restarts: the second pane's tabs are ordinary tabs, so the last
//! session reopens them in the one strip, unsplit.

use std::collections::{HashMap, HashSet};

use craft::quit::CloseScope;
use craft::ui::canvas::{PanePress, SplitAt, SplitLayout, SplitViews, TabDropTarget};
use craft::ui::contract::Bind; // PLAN: assumed path of `Bind` through the facade
use pdfcraft_engine::DocId;

use crate::PdfCraftApp;
// PLAN: chrome.rs (pdf-chrome-w10) declares `pub(crate) mod doc_tabs;` for this path.
use crate::chrome::doc_tabs::{StripTab, strip};

/// Where a tab is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Place {
    /// The window's tab strip and page area (the left or top pane when split).
    #[default]
    Main,
    /// The second pane of split view (right or bottom).
    Second,
}

/// A tab's place among the panes (`DocView::pane`).
#[derive(Clone, Debug)]
pub(crate) struct ViewPane {
    /// This view among all of the app's views, unique for the app's life: two views of one
    /// document keep their own scroll position and floating bars under it (`DocView::view_key`).
    pub(crate) key: u64,
    /// Where the tab shows.
    pub(crate) place: Place,
    /// Whether `place` was chosen yet (a new tab opens in the pane the user was working in).
    pub(crate) placed: bool,
    /// When this tab was last the active one (each pane shows its latest).
    pub(crate) shown_seq: u64,
}

impl Default for ViewPane {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self { key: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed), place: Place::Main, placed: false, shown_seq: 0 }
    }
}

/// The page area in two panes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Split {
    /// The second pane is below the first (Split down) rather than beside it (Split right).
    pub down: bool,
    /// The first pane's share of the page area.
    pub fraction: f32,
}

impl Split {
    fn new(down: bool) -> Self {
        Self { down, fraction: 0.5 }
    }
}

/// Split view's state: see the module docs.
#[derive(Default)]
pub struct Panes {
    /// Split view, while it is on.
    pub split: Option<Split>,
    /// The latest [`ViewPane::shown_seq`] handed out.
    top_seq: u64,
    /// The pane of the tab active last: a new tab opens there.
    focus: Place,
    /// The tab (by view key) whose context menu is open, and the strip it was opened on.
    pub(crate) tab_menu: Option<(Place, u64)>,
    /// Each open document's edit generation as last seen.
    generations: HashMap<DocId, u64>,
    /// The pane being drawn now (for what belongs to one pane only).
    pub(crate) drawing: Place,
}

impl Panes {}

impl PdfCraftApp {
    /// The tabs shown in `place`, in their strip's order (indexes into `views`).
    pub fn tabs_in(&self, place: Place) -> Vec<usize> {
        self.views.iter().enumerate().filter(|(_, v)| v.pane.place == place).map(|(i, _)| i).collect()
    }

    /// The tab `place` shows: the active tab when it is there, else the one active there last.
    /// `None` for the main pane while Home (or Combine files) shows.
    pub fn pane_front(&self, place: Place) -> Option<usize> {
        if let Some(a) = self.active
            && self.views.get(a).is_some_and(|v| v.pane.place == place)
        {
            return Some(a);
        }
        if place == Place::Main && self.active.is_none() {
            return None;
        }
        self.latest_in(place)
    }

    /// The tab of `place` active last (the first one when none has been).
    fn latest_in(&self, place: Place) -> Option<usize> {
        self.views
            .iter()
            .enumerate()
            .filter(|(_, v)| v.pane.place == place)
            .max_by_key(|(i, v)| (v.pane.shown_seq, std::cmp::Reverse(*i)))
            .map(|(i, _)| i)
    }

    /// Where tab `index` shows.
    pub fn place_of(&self, index: usize) -> Option<Place> {
        self.views.get(index).map(|v| v.pane.place)
    }

    /// Whether another tab shows the same document as tab `index`.
    pub(crate) fn has_twin(&self, index: usize) -> bool {
        let Some(id) = self.views.get(index).map(|v| v.id) else { return false };
        self.views.iter().enumerate().any(|(k, v)| k != index && v.id == id)
    }

    /// The tab with view key `key` ([`crate::DocView::view_key`]).
    pub(crate) fn tab_with_key(&self, key: u64) -> Option<usize> {
        self.views.iter().position(|v| v.pane.key == key)
    }

    pub(crate) fn active_key(&self) -> Option<u64> {
        self.active.and_then(|i| self.views.get(i)).map(|v| v.pane.key)
    }

    /// Bring forward the tab `place` shows (a click in that pane).
    pub fn activate_place(&mut self, place: Place) {
        if self.active.and_then(|i| self.views.get(i)).is_some_and(|v| v.pane.place == place) {
            return;
        }
        if let Some(i) = self.latest_in(place) {
            self.active = Some(i);
            self.combine_tab.focused = false;
        }
    }

    /// Keep the panes consistent, once a frame before anything is drawn and before every change
    /// here: new tabs go to the pane the user was working in, a pane left without tabs ends split
    /// view, the views of a document changed through another view catch up, and the active tab
    /// becomes its pane's latest.
    pub(crate) fn settle_panes(&mut self, ctx: &egui::Context) {
        let split = self.panes.split.is_some();
        let focus = if split { self.panes.focus } else { Place::Main };
        for v in &mut self.views {
            if !v.pane.placed {
                v.pane.place = focus;
                v.pane.placed = true;
            }
            if v.pane.place == Place::Second && !split {
                v.pane.place = Place::Main;
            }
        }
        if split {
            let has = |p: Place| self.views.iter().any(|v| v.pane.place == p);
            if !has(Place::Main) || !has(Place::Second) {
                self.unsplit();
            }
        }
        self.sync_views(ctx);
        if let Some(v) = self.active.and_then(|i| self.views.get_mut(i)) {
            self.panes.focus = v.pane.place;
            if v.pane.shown_seq != self.panes.top_seq || self.panes.top_seq == 0 {
                self.panes.top_seq = self.panes.top_seq.saturating_add(1);
                v.pane.shown_seq = self.panes.top_seq;
            }
        }
    }

    /// A document changed through one of its views (an edit, Undo, a reload): its other view
    /// starts afresh at the same place, as after a reload.
    fn sync_views(&mut self, ctx: &egui::Context) {
        let mut seen = HashSet::new();
        for k in 0..self.views.len() {
            let Some(id) = self.views.get(k).map(|v| v.id) else { continue };
            if !seen.insert(id) {
                continue;
            }
            let Some(doc) = self.session.get(id) else { continue };
            let generation = doc.edit_generation();
            let before = self.panes.generations.insert(id, generation);
            if before.is_none_or(|g| g == generation) {
                continue;
            }
            // The view the change came through: the active one, else the first (as the code that
            // made the change finds a document's tab).
            let acting = self.active.filter(|&a| self.views.get(a).is_some_and(|v| v.id == id)).unwrap_or(k);
            for (j, v) in self.views.iter_mut().enumerate() {
                if j != acting && v.id == id {
                    v.follow_document(ctx, &doc.info);
                }
            }
        }
        self.panes.generations.retain(|id, _| seen.contains(id));
    }

    /// Move tab `from` along its strip: `to` is where it lands among the strip's other tabs (`tabs`,
    /// the strip's tabs in order, from first to last). In split view a strip holds only some of the
    /// tabs, so this goes by the tab it lands next to rather than by index in `views`.
    pub(crate) fn move_tab_in_strip(&mut self, from: usize, tabs: &[usize], to: usize) {
        let others: Vec<usize> = tabs.iter().copied().filter(|&i| i != from).collect();
        let is_pinned = self.views.get(from).is_some_and(|v| v.pinned);
        let pinned_others = others.iter().filter(|&&idx| self.views.get(idx).is_some_and(|v| v.pinned)).count();
        let to = if is_pinned { to.min(pinned_others) } else { to.max(pinned_others) };
        let (anchor, before) = match others.get(to) {
            Some(&a) => (a, true),
            None => match others.last() {
                Some(&a) => (a, false),
                None => return,
            },
        };
        if from >= self.views.len() || tabs.iter().position(|&i| i == from) == Some(to) {
            return;
        }
        let Some(anchor) = self.views.get(anchor).map(|v| v.pane.key) else { return };
        let active = self.active_key();
        let old_group = self.views[from].group;
        let view = self.views.remove(from);
        let Some(a) = self.tab_with_key(anchor) else {
            self.views.insert(from.min(self.views.len()), view);
            return;
        };
        let at = if before { a } else { a.saturating_add(1) };
        self.views.insert(at.min(self.views.len()), view);
        self.active = active.and_then(|k| self.tab_with_key(k));

        let new_idx = at.min(self.views.len().saturating_sub(1));
        if self.views[new_idx].pinned {
            self.views[new_idx].group = None;
        } else {
            let place = self.views[new_idx].pane.place;
            let left_group = if new_idx > 0 { self.views.get(new_idx - 1).filter(|v| v.pane.place == place).and_then(|v| v.group) } else { None };
            let right_group = self.views.get(new_idx + 1).filter(|v| v.pane.place == place).and_then(|v| v.group);

            if left_group.is_some() && left_group == right_group {
                self.views[new_idx].group = left_group;
            } else if old_group.is_some() && (left_group == old_group || right_group == old_group) {
                self.views[new_idx].group = old_group;
            } else {
                // Dropped outside its group, or at the edge of another one: not a member of either.
                self.views[new_idx].group = None;
            }
        }

        if let Some(og) = old_group {
            self.cleanup_empty_group(og);
        }
    }

    /// Remove tab `index` but not its document, which another tab still shows. When it was the
    /// active tab, that other tab of the document becomes active.
    fn drop_view(&mut self, index: usize) {
        let Some((key, id)) = self.views.get(index).map(|v| (v.pane.key, v.id)) else { return };
        let active = self.active_key();
        self.views.remove(index);
        self.active = if active == Some(key) { self.views.iter().position(|v| v.id == id) } else { active.and_then(|k| self.tab_with_key(k)) };
    }

    /// Tab `index`'s right-click Split right (or Split down). The tab the main pane shows gets a
    /// second view in the second pane (the same document, twice); another tab moves there (two
    /// documents side by side). A tab of the second pane only turns the split.
    pub fn split_tab(&mut self, ctx: &egui::Context, index: usize, down: bool) {
        self.settle_panes(ctx);
        let Some(place) = self.place_of(index) else { return };
        match place {
            Place::Second => {
                if let Some(s) = self.panes.split.as_mut() {
                    s.down = down;
                }
                self.active = Some(index);
            }
            Place::Main => {
                let alone = self.tabs_in(Place::Main).len() <= 1;
                let shown = self.pane_front(Place::Main) == Some(index) || (self.active.is_none() && self.latest_in(Place::Main) == Some(index));
                match self.panes.split.as_mut() {
                    Some(s) => s.down = down,
                    None => self.panes.split = Some(Split::new(down)),
                }
                if shown || alone {
                    self.show_twin(index, Place::Second);
                } else {
                    self.move_tab_to(index, Place::Second);
                }
            }
        }
    }

    /// Show tab `index`'s document in `place` too (a second view of it), or bring forward the
    /// view of it already there.
    fn show_twin(&mut self, index: usize, place: Place) {
        let Some(view) = self.views.get(index) else { return };
        let id = view.id;
        if let Some(there) = self.views.iter().position(|v| v.id == id && v.pane.place == place) {
            self.active = Some(there);
            return;
        }
        let Some(doc) = self.session.get(id) else { return };
        let mut twin = view.twin(&doc.info);
        twin.pane.place = place;
        twin.pane.placed = true;
        self.views.push(twin);
        self.active = Some(self.views.len() - 1);
    }

    /// View ▸ Split (⌘\): split the current document into two panes, or end split view.
    pub fn toggle_split(&mut self, ctx: &egui::Context) {
        self.settle_panes(ctx);
        if self.panes.split.is_some() {
            self.unsplit();
        } else if let Some(i) = self.active {
            self.split_tab(ctx, i, false);
        }
    }

    /// End split view: the second pane's tabs join the main strip, except second views of
    /// documents the main pane has open already, which close.
    pub fn unsplit(&mut self) {
        self.panes.split = None;
        for i in (0..self.views.len()).rev() {
            let Some((id, place)) = self.views.get(i).map(|v| (v.id, v.pane.place)) else { continue };
            if place != Place::Second {
                continue;
            }
            if self.views.iter().any(|v| v.id == id && v.pane.place == Place::Main) {
                self.drop_view(i);
            } else if let Some(v) = self.views.get_mut(i) {
                v.pane.place = Place::Main;
            }
        }
    }

    /// Move tab `index` to `place` and make it active. A pane that shows its document already
    /// keeps the view it has, and the moving one closes. Returns the tab's index afterwards.
    pub fn move_tab_to(&mut self, index: usize, place: Place) -> Option<usize> {
        let (id, key, from) = self.views.get(index).map(|v| (v.id, v.pane.key, v.pane.place))?;
        if from == place {
            self.active = Some(index);
            return Some(index);
        }
        if place == Place::Second && self.panes.split.is_none() {
            self.panes.split = Some(Split::new(false));
        }
        if let Some(there) = self.views.iter().position(|v| v.id == id && v.pane.place == place && v.pane.key != key) {
            let keep = self.views.get(there).map(|v| v.pane.key);
            self.drop_view(index);
            self.active = keep.and_then(|k| self.tab_with_key(k));
            return self.active;
        }
        if let Some(v) = self.views.get_mut(index) {
            v.pane.place = place;
            v.pane.placed = true;
        }
        self.active = Some(index);
        self.combine_tab.focused = false;
        Some(index)
    }

    /// Quitting with unsaved documents: one tab per document first, so each document is asked
    /// about once (a tab with typing of its own stays, to be asked about).
    fn one_tab_per_document(&mut self) {
        for i in (0..self.views.len()).rev() {
            let Some(id) = self.views.get(i).map(|v| v.id) else { continue };
            if self.views.iter().take(i).any(|v| v.id == id) && !self.has_unsaved_typing(i) {
                self.drop_view(i);
            }
        }
    }

    /// The tabs, one per document (the last session lists each file once).
    pub(crate) fn first_views(&self) -> impl Iterator<Item = &crate::DocView> {
        self.views.iter().enumerate().filter(|(i, v)| !self.views.iter().take(*i).any(|w| w.id == v.id)).map(|(_, v)| v)
    }

    /// Whether closing tab `index` asks about unsaved changes first: the document's changes are
    /// asked about by its last tab only (another tab of it keeps them open); typing a tab holds
    /// of its own, by that tab.
    pub(crate) fn asks_before_closing(&self, index: usize) -> bool {
        if self.has_twin(index) { self.has_unsaved_typing(index) } else { self.has_unsaved_work(index) }
    }

    /// The tab that comes forward when the active tab closes (it was at `index`, in `place`, and
    /// is gone from `views`): the one that took its place in the same strip, else the strip's last,
    /// else the tab the other pane shows.
    pub(crate) fn after_close(&self, place: Place, index: usize) -> Option<usize> {
        let tabs = self.tabs_in(place);
        tabs.iter().copied().find(|&i| i >= index).or_else(|| tabs.last().copied()).or_else(|| match place {
            Place::Main if self.panes.split.is_some() => self.latest_in(Place::Second),
            Place::Main => None,
            Place::Second => self.latest_in(Place::Main),
        })
    }

    /// The tabs on screen now: what each pane shows (and a comparison's other half).
    pub(crate) fn views_on_screen(&self) -> Vec<usize> {
        let mut shown = Vec::new();
        let mut add = |i: Option<usize>| {
            if let Some(i) = i
                && !shown.contains(&i)
            {
                shown.push(i);
            }
        };
        let main = if self.full_screen() { self.active } else { self.pane_front(Place::Main) };
        add(main);
        add(main.and_then(|i| crate::compare_ui::side_by_side(self, i)).map(|(old, _)| old));
        if !self.full_screen() && self.panes.split.is_some() {
            add(self.pane_front(Place::Second));
        }
        shown
    }
}

/// Once a frame, before anything is drawn: the panes settle (and a quit asks about each document
/// once).
pub(crate) fn frame(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if app.close_guard.as_ref().is_some_and(|guard| guard.scope() == CloseScope::Quit) {
        app.one_tab_per_document();
    }
    app.drain_reveals();
    app.settle_panes(ctx);
    // A tab off screen runs no page view this frame, so no pass records its gestures.
    let shown = app.views_on_screen();
    for (i, view) in app.views.iter_mut().enumerate() {
        if !shown.contains(&i) {
            view.left_screen();
        }
    }
}

/// The page area: the main pane's tab (Home or Combine files while it has none, a comparison's
/// two halves), or split view's two panes.
pub(crate) fn page_area(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    app.panes.drawing = Place::Main;
    let main = app.pane_front(Place::Main);
    let split = app.panes.split.and_then(|s| Some((s, app.pane_front(Place::Second)?)));
    let (layout, mut at) = match split {
        Some((s, _)) if s.down => (SplitLayout::TwoUpHorizontal, SplitAt::lines(Vec::new(), vec![s.fraction])),
        Some((s, _)) => (SplitLayout::TwoUpVertical, SplitAt::lines(vec![s.fraction], Vec::new())),
        None => (SplitLayout::Tile, SplitAt::default()),
    };
    let active = usize::from(app.active.and_then(|i| app.views.get(i)).is_some_and(|v| v.pane.place == Place::Second));
    // The drop rule reads the open views up front: the views closure below borrows the app.
    let open: HashSet<u64> = app.views.iter().map(|v| v.pane.key).collect();
    let views = SplitViews::new(tl!("Pages").to_string(), layout, Bind::bound(&mut at))
        .count(1)
        .active(active)
        .pane_press(PanePress::Interact)
        .takes_tabs::<StripTab>(|documents, _| {
            !documents.is_empty() && documents.iter().all(|d| matches!(d, StripTab::View(key) if open.contains(key)))
        })
        .show_panes(ui, |ui, _, pane| {
            let place = if pane.index == 0 { Place::Main } else { Place::Second };
            if place == Place::Second {
                strip(app, ui, place);
            }
            let room = ui.available_rect_before_wrap();
            let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(room).id_salt("body"));
            body_ui.shrink_clip_rect(room);
            // The strip may have closed or moved a tab just now: look the pane's tab up again (a
            // pane left empty ends split view next frame).
            let front = if place == Place::Second { app.pane_front(place) } else { main.filter(|&i| i < app.views.len()) };
            app.panes.drawing = place;
            match (place, front) {
                (Place::Main, _) => main_pane(app, front, &mut body_ui),
                (Place::Second, Some(index)) => crate::canvas::document_area(app, index, &mut body_ui),
                (Place::Second, None) => {}
            }
            app.panes.drawing = Place::Main;
        });
    let output = views.output;
    // (While Home shows in the main pane, a click there stays on Home.)
    match output.activated {
        Some(0) if main.is_some() => app.activate_place(Place::Main),
        Some(1) => app.activate_place(Place::Second),
        _ => {}
    }
    if let Some(s) = app.panes.split.as_mut()
        && let Some(&fraction) = at.columns.first().or(at.rows.first())
        && fraction.is_finite()
    {
        s.fraction = fraction;
    }
    if let Some(drop) = output.tab_drop {
        for document in drop.documents {
            let StripTab::View(key) = document else { continue };
            let Some(index) = app.tab_with_key(key) else { continue };
            match drop.target {
                TabDropTarget::Pane(pane) => {
                    let place = if pane == 0 { Place::Main } else { Place::Second };
                    if app.place_of(index) != Some(place) {
                        app.move_tab_to(index, place);
                    }
                }
                TabDropTarget::SplitEnd => {
                    // The strip selects a tab as its drag starts. With other tabs in the pane,
                    // dropping it on a new split moves it rather than making a second view of it.
                    if app.place_of(index) == Some(Place::Main) && app.tabs_in(Place::Main).len() > 1 {
                        app.move_tab_to(index, Place::Second);
                    } else {
                        app.split_tab(ui.ctx(), index, false);
                    }
                }
                _ => {}
            }
        }
    }
}

/// What the main pane shows: tab `main`, or Home or Combine files while it has none.
fn main_pane(app: &mut PdfCraftApp, main: Option<usize>, ui: &mut egui::Ui) {
    match main {
        None if app.combine_showing() => crate::combine_ui::page(app, ui),
        None => crate::home::show(app, ui),
        Some(index) => match crate::compare_ui::side_by_side(app, index) {
            Some((old, new)) => crate::compare_ui::compare_area(app, old, new, ui),
            None => crate::canvas::document_area(app, index, ui),
        },
    }
}
