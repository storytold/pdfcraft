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

use egui::{Pos2, Rect, Sense, Stroke, vec2};
use pdfcraft_engine::DocId;

use crate::theme::Tokens;
use crate::{CloseRequest, PdfCraftApp};

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
    /// The document's render queue was last set for both of its views at once.
    pub(crate) shared: bool,
}

impl Default for ViewPane {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self { key: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed), place: Place::Main, placed: false, shown_seq: 0, shared: false }
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

/// How far the divider between the panes may go, as a share of the page area.
const FRACTION: std::ops::RangeInclusive<f32> = 0.15..=0.85;
/// The divider's width, and the height of the second pane's tab strip.
const DIVIDER: f32 = 1.0;
const PANE_STRIP: f32 = 30.0;

/// Where a dragged tab goes if it is let go now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TabDrop {
    /// Its own strip: it moves along it (the strip does that).
    Strip,
    /// The other pane.
    Pane(Place),
    /// The right half of a page area that isn't split: split right with it.
    SplitRight,
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
    /// The main tab strip and the page area, last frame.
    pub(crate) main_strip: Option<Rect>,
    pub(crate) page_rect: Option<Rect>,
    /// Each open document's edit generation as last seen.
    generations: HashMap<DocId, u64>,
    /// The pane being drawn now (for what belongs to one pane only).
    pub(crate) drawing: Place,
    /// Where a dragged tab would land, painted over the page area this frame.
    pub(crate) drop_hint: Option<Rect>,
}

impl Panes {
    /// An egui id for something drawn once per pane (the floating tools bar): the main pane keeps
    /// the plain id.
    pub(crate) fn area_id(&self, name: &'static str) -> egui::Id {
        match self.drawing {
            Place::Main => egui::Id::new(name),
            place => egui::Id::new((name, place)),
        }
    }
}

/// The two panes and the divider between them, in `area`.
fn layout(area: Rect, split: Split) -> (Rect, Rect, Rect) {
    let f = if split.fraction.is_finite() { split.fraction.clamp(*FRACTION.start(), *FRACTION.end()) } else { 0.5 };
    if split.down {
        let y = (area.top() + area.height() * f).round();
        (
            Rect::from_min_max(area.min, egui::pos2(area.right(), y)),
            Rect::from_min_max(egui::pos2(area.left(), y), egui::pos2(area.right(), y + DIVIDER)),
            Rect::from_min_max(egui::pos2(area.left(), y + DIVIDER), area.max),
        )
    } else {
        let x = (area.left() + area.width() * f).round();
        (
            Rect::from_min_max(area.min, egui::pos2(x, area.bottom())),
            Rect::from_min_max(egui::pos2(x, area.top()), egui::pos2(x + DIVIDER, area.bottom())),
            Rect::from_min_max(egui::pos2(x + DIVIDER, area.top()), area.max),
        )
    }
}

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

    fn active_key(&self) -> Option<u64> {
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
    pub(crate) fn settle_panes(&mut self) {
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
        self.sync_views();
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
    fn sync_views(&mut self) {
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
                    v.follow_document(&doc.info);
                }
            }
        }
        self.panes.generations.retain(|id, _| seen.contains(id));
    }

    /// Move tab `from` along its strip: `to` is where it lands among the strip's tabs (`slots`, the
    /// strip's tabs in order, from first to last; `chrome::drop_index`). In split view a strip
    /// holds only some of the tabs, so this goes by the tab it lands next to rather than by index
    /// in `views`.
    pub(crate) fn move_tab_in_strip(&mut self, from: usize, slots: &[(usize, Rect)], to: usize) {
        let others: Vec<usize> = slots.iter().map(|(i, _)| *i).filter(|&i| i != from).collect();
        let (anchor, before) = match others.get(to) {
            Some(&a) => (a, true),
            None => match others.last() {
                Some(&a) => (a, false),
                None => return,
            },
        };
        if from >= self.views.len() || slots.iter().position(|(i, _)| *i == from) == Some(to) {
            return;
        }
        let Some(anchor) = self.views.get(anchor).map(|v| v.pane.key) else { return };
        let active = self.active_key();
        let view = self.views.remove(from);
        let Some(a) = self.tab_with_key(anchor) else {
            self.views.insert(from.min(self.views.len()), view);
            return;
        };
        let at = if before { a } else { a.saturating_add(1) };
        self.views.insert(at.min(self.views.len()), view);
        self.active = active.and_then(|k| self.tab_with_key(k));
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
    pub fn split_tab(&mut self, index: usize, down: bool) {
        self.settle_panes();
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
    pub fn toggle_split(&mut self) {
        self.settle_panes();
        if self.panes.split.is_some() {
            self.unsplit();
        } else if let Some(i) = self.active {
            self.split_tab(i, false);
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

    /// Quitting or File ▸ Close all with unsaved documents: one tab per document first, so each
    /// document is asked about once (a tab with typing of its own stays, to be asked about).
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

    /// Where a tab dragged from `from`'s strip lands if let go at `pos`: the other pane, the right
    /// half of a page area that isn't split, or else its own strip (as before split view, the
    /// tab moves along the strip wherever it is let go).
    pub(crate) fn drop_target(&self, pos: Pos2, from: Place) -> TabDrop {
        if from != Place::Main && self.panes.main_strip.is_some_and(|r| r.contains(pos)) {
            return TabDrop::Pane(Place::Main);
        }
        let Some(page) = self.panes.page_rect.filter(|r| r.contains(pos)) else { return TabDrop::Strip };
        match self.panes.split {
            Some(split) => {
                let (first, _, second) = layout(page, split);
                if second.contains(pos) && from != Place::Second {
                    TabDrop::Pane(Place::Second)
                } else if first.contains(pos) && from != Place::Main {
                    TabDrop::Pane(Place::Main)
                } else {
                    TabDrop::Strip
                }
            }
            None if pos.x >= page.center().x => TabDrop::SplitRight,
            None => TabDrop::Strip,
        }
    }

    /// The area a [`TabDrop`] would fill, for the hint drawn while dragging.
    pub(crate) fn drop_hint_rect(&self, drop: TabDrop) -> Option<Rect> {
        let page = self.panes.page_rect?;
        match drop {
            TabDrop::Pane(p) => {
                let split = self.panes.split?;
                let (first, _, second) = layout(page, split);
                Some(if p == Place::Second { second } else { first })
            }
            TabDrop::SplitRight => Some(Rect::from_min_max(egui::pos2(page.center().x, page.top()), page.max)),
            TabDrop::Strip => None,
        }
    }

    /// Let tab `index` go at `drop`.
    pub(crate) fn drop_tab(&mut self, index: usize, drop: TabDrop) {
        match drop {
            TabDrop::Strip => {}
            TabDrop::Pane(place) => {
                self.move_tab_to(index, place);
            }
            TabDrop::SplitRight => self.split_tab(index, false),
        }
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
        let main = if self.full_screen { self.active } else { self.pane_front(Place::Main) };
        add(main);
        add(main.and_then(|i| crate::compare_ui::side_by_side(self, i)).map(|(old, _)| old));
        if !self.full_screen && self.panes.split.is_some() {
            add(self.pane_front(Place::Second));
        }
        shown
    }
}

/// Once a frame, before anything is drawn: the panes settle (and a quit or Close all asks about
/// each document once).
pub(crate) fn frame(app: &mut PdfCraftApp) {
    if matches!(app.close_request, Some(CloseRequest::Quit | CloseRequest::All)) {
        app.one_tab_per_document();
    }
    app.settle_panes();
}

/// The page area: the main pane's tab (Home or Combine files while it has none, a comparison's
/// two halves), or split view's two panes.
pub(crate) fn page_area(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    app.panes.page_rect = Some(ui.max_rect());
    app.panes.drawing = Place::Main;
    if let Some(hint) = app.panes.drop_hint.take() {
        let t = Tokens::get(ui.ctx());
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("pane-drop-hint")));
        painter.rect(hint.shrink(4.0), 6.0, t.accent.gamma_multiply(0.12), Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
    }
    let main = app.pane_front(Place::Main);
    match app.panes.split.and_then(|s| Some((s, app.pane_front(Place::Second)?))) {
        Some((split, second)) => split_area(app, split, main, second, ui),
        None => main_pane(app, main, ui),
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

/// Whether a press this frame began in `rect` on the page area itself, or on one of the floating
/// bars of the view drawn there (not on a dialog, menu or panel over it).
fn pressed_in(ui: &egui::Ui, rect: Rect, place: Place, view: Option<&crate::DocView>) -> bool {
    let Some(pos) = ui.input(|i| if i.pointer.any_pressed() { i.pointer.press_origin() } else { None }) else { return false };
    if !rect.contains(pos) {
        return false;
    }
    match ui.ctx().layer_id_at(pos) {
        None => true,
        Some(layer) => {
            let quick = if place == Place::Main { egui::Id::new("quick-bar") } else { egui::Id::new(("quick-bar", place)) };
            layer == ui.layer_id()
                || [quick, egui::Id::new("camera-bar")].contains(&layer.id)
                || view.is_some_and(|v| egui::Id::new(("find-bar", v.view_key())) == layer.id)
        }
    }
}

/// Split view: the main pane, the divider and the second pane with its strip. A press in a pane
/// makes it the active one before it is drawn, so the click acts there.
fn split_area(app: &mut PdfCraftApp, split: Split, main: Option<usize>, second: usize, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let area = ui.max_rect();
    let (first, divider, other) = layout(area, split);
    if !app.modal_open() && !app.palette_open {
        // (While Home shows in the main pane, a click there stays on Home.)
        if main.is_some() && pressed_in(ui, first, Place::Main, main.and_then(|i| app.views.get(i))) {
            app.activate_place(Place::Main);
        } else if pressed_in(ui, other, Place::Second, app.views.get(second)) {
            app.activate_place(Place::Second);
        }
    }
    // The divider, dragged to share the area differently.
    let grab = if split.down { divider.expand2(vec2(0.0, 3.0)) } else { divider.expand2(vec2(3.0, 0.0)) };
    let response = ui.interact(grab, ui.id().with("pane-divider"), Sense::drag());
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(if split.down { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
    }
    if response.dragged()
        && let Some(p) = ui.ctx().pointer_latest_pos()
        && let Some(s) = app.panes.split.as_mut()
    {
        let f = if split.down { (p.y - area.top()) / area.height().max(1.0) } else { (p.x - area.left()) / area.width().max(1.0) };
        if f.is_finite() {
            s.fraction = f.clamp(*FRACTION.start(), *FRACTION.end());
        }
    }
    ui.painter().rect_filled(divider, 0.0, if response.dragged() || response.hovered() { t.accent } else { t.divider });
    let active = app.active.and_then(|i| app.views.get(i)).map(|v| v.pane.place);
    pane(app, ui, first, Place::Main, main, active == Some(Place::Main));
    pane(app, ui, other, Place::Second, Some(second), active == Some(Place::Second));
}

/// One pane of split view: the second has its own tab strip above its page area.
fn pane(app: &mut PdfCraftApp, ui: &mut egui::Ui, rect: Rect, place: Place, front: Option<usize>, active: bool) {
    let t = Tokens::get(ui.ctx());
    let salt = if place == Place::Second { "second-pane" } else { "main-pane" };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt(salt));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    let body = if place == Place::Second {
        let strip = Rect::from_min_size(rect.min, vec2(rect.width(), PANE_STRIP));
        crate::chrome::pane_tab_strip(app, &mut child, strip, place);
        Rect::from_min_max(egui::pos2(rect.left(), strip.bottom()), rect.max)
    } else {
        rect
    };
    // The strip may have closed or moved a tab just now: look the pane's tab up again (a pane
    // left empty ends split view next frame).
    let front = if place == Place::Second { app.pane_front(place) } else { front.filter(|&i| i < app.views.len()) };
    let mut body_ui = child.new_child(egui::UiBuilder::new().max_rect(body).id_salt("body"));
    body_ui.set_clip_rect(body.intersect(ui.clip_rect()));
    app.panes.drawing = place;
    match (place, front) {
        (Place::Main, _) => main_pane(app, front, &mut body_ui),
        (Place::Second, Some(index)) => crate::canvas::document_area(app, index, &mut body_ui),
        (Place::Second, None) => {}
    }
    app.panes.drawing = Place::Main;
    // The active pane is marked along its top edge.
    if active {
        let edge = Rect::from_min_size(rect.min, vec2(rect.width(), 2.0));
        ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new(("active-pane", salt)))).rect_filled(edge, 0.0, t.accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panes_share_the_area_and_the_divider_stays_inside() {
        let area = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(1000.0, 600.0));
        let (a, d, b) = layout(area, Split { down: false, fraction: 0.5 });
        assert_eq!(a.right(), d.left());
        assert_eq!(d.right(), b.left());
        assert_eq!(b.right(), 1000.0);
        let (a, _, b) = layout(area, Split { down: true, fraction: 2.0 });
        assert!(a.height() <= 600.0 * 0.85 + 1.0 && b.height() > 0.0, "a fraction past the end is held back");
        let (a, _, _) = layout(area, Split { down: false, fraction: f32::NAN });
        assert_eq!(a.width(), 500.0, "a fraction that isn't a number shares evenly");
    }
}
