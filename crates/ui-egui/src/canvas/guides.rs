//! Guides and the page grid: lines for aligning content that are shown over the pages and never
//! written to the PDF.
//!
//! A guide is dragged out of a ruler onto a page (View ▸ Rulers): onto that page only, or onto
//! every page when Shift is held as it is dropped. It is moved by dragging it and removed by
//! dragging it back onto a ruler or off its page; a right-click on it changes its colour,
//! thickness, label and whether it is on every page. Guides belong to the open document. A guide
//! is a distance in points from one edge of the page as displayed before View ▸ Rotate (top or
//! bottom for a horizontal guide, left or right for a vertical one), so rotating the view turns
//! guides with the page, and a guide measured from the right stays at the right margin on wider
//! pages. Saved sets of guides ([`crate::guide_sets`]) are drawn the same way when shown.
//!
//! The grid is one setting for every page (Preferences ▸ Units and guides): lines every
//! `width` × `height` points from an offset, each cell divided into `subdivisions`. With View ▸
//! Snap to grid on, a guide being dragged snaps to the nearest grid line.
//!
//! Pure geometry is kept apart from painting and input so it can be tested without a frame.

use std::collections::BTreeMap;

use egui::{Color32, CursorIcon, Pos2, Rect, Sense, Stroke, pos2};
use serde::{Deserialize, Serialize};

use super::rulers::{self, RulerInput, RulerLayout, VisiblePageGeometry};
use super::{DocView, PageXform};
use crate::guide_sets::GuideSet;
use crate::page_cursor::Layer;
use crate::page_pointer::PagePointer;

/// At most this many guides on one page, and on every page: each is a strip that takes page
/// input, and thousands would make every frame slow for no use.
pub const MAX_PER_PAGE: usize = 128;
/// How far either side of a guide, in screen points, a press picks it up.
pub const HIT: f32 = 3.0;
/// How close, in screen points, a dragged guide has to come to a grid line to snap to it.
pub const SNAP: f32 = 6.0;
/// Grid lines closer than this on screen are not drawn (the subdivisions first, then the grid).
const MIN_LINE_SPACING: f32 = 4.0;
/// Upper bound on grid lines drawn across one page in one direction.
const MAX_GRID_LINES: usize = 2_000;

/// Which edges of the page a guide runs parallel to, in the page's own displayed orientation
/// (before View ▸ Rotate): a horizontal guide is a fixed distance from the top or bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GuideAxis {
    Horizontal,
    Vertical,
}

/// The edge a guide is measured from: top or left (`Start`), bottom or right (`End`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Edge {
    #[default]
    Start,
    End,
}

/// A guide's own look; what it leaves out comes from the set or from Preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Style {
    pub color: Option<[u8; 4]>,
    /// Line thickness in screen points.
    pub width: Option<f32>,
}

impl Style {
    /// This style, with what it leaves out taken from `fallback`.
    pub fn or(self, fallback: Style) -> Style {
        Style { color: self.color.or(fallback.color), width: self.width.or(fallback.width) }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Guide {
    pub axis: GuideAxis,
    /// Points from the edge in `from`.
    pub at: f32,
    pub from: Edge,
    pub style: Style,
    pub label: Option<String>,
}

impl Guide {
    pub fn new(axis: GuideAxis, at: f32) -> Guide {
        Guide { axis, at, from: Edge::Start, style: Style::default(), label: None }
    }

    /// Points from the top or left edge on a page `size` (width, height) points large.
    pub fn position(&self, size: [f32; 2]) -> f32 {
        let length = self.length(size);
        match self.from {
            Edge::Start => self.at,
            Edge::End => length - self.at,
        }
    }

    fn length(&self, size: [f32; 2]) -> f32 {
        match self.axis {
            GuideAxis::Horizontal => size[1],
            GuideAxis::Vertical => size[0],
        }
    }

    /// Move the guide to `position` points from the top or left edge, still measured from its edge.
    fn set_position(&mut self, position: f32, size: [f32; 2]) {
        self.at = match self.from {
            Edge::Start => position,
            Edge::End => self.length(size) - position,
        };
    }
}

/// Which of a document's guides: one of a page's own, or one of those on every page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GuideRef {
    Page(usize, usize),
    All(usize),
}

impl GuideRef {
    /// This reference after `removed` left its list: `None` when it was this guide, one lower
    /// when it came after it in the same list, unchanged otherwise.
    pub fn after_removing(self, removed: GuideRef) -> Option<GuideRef> {
        match (self, removed) {
            (a, b) if a == b => None,
            (GuideRef::Page(p, i), GuideRef::Page(q, j)) if p == q && i > j => Some(GuideRef::Page(p, i - 1)),
            (GuideRef::All(i), GuideRef::All(j)) if i > j => Some(GuideRef::All(i - 1)),
            (a, _) => Some(a),
        }
    }
}

/// Keep an open Properties window on its guide after `removed` left the guides.
fn props_after_removing(props: &mut OpenProperties, removed: GuideRef) {
    *props = props.and_then(|(r, page)| r.after_removing(removed).map(|r| (r, page)));
}

/// The guides of one document: on every page, and each page's own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Guides {
    pages: BTreeMap<usize, Vec<Guide>>,
    all: Vec<Guide>,
}

impl Guides {
    /// A page's own guides (not those on every page).
    pub fn on(&self, page: usize) -> &[Guide] {
        self.pages.get(&page).map_or(&[], Vec::as_slice)
    }

    /// The guides on every page.
    pub fn all(&self) -> &[Guide] {
        &self.all
    }

    /// Each page's own guides, by page.
    pub fn by_page(&self) -> impl Iterator<Item = (usize, &[Guide])> {
        self.pages.iter().map(|(p, l)| (*p, l.as_slice()))
    }

    /// Every guide shown on `page`, with which one it is.
    pub fn shown_on(&self, page: usize) -> impl Iterator<Item = (GuideRef, &Guide)> {
        self.all
            .iter()
            .enumerate()
            .map(|(i, g)| (GuideRef::All(i), g))
            .chain(self.on(page).iter().enumerate().map(move |(i, g)| (GuideRef::Page(page, i), g)))
    }

    pub fn get(&self, r: GuideRef) -> Option<&Guide> {
        match r {
            GuideRef::Page(p, i) => self.pages.get(&p)?.get(i),
            GuideRef::All(i) => self.all.get(i),
        }
    }

    fn get_mut(&mut self, r: GuideRef) -> Option<&mut Guide> {
        match r {
            GuideRef::Page(p, i) => self.pages.get_mut(&p)?.get_mut(i),
            GuideRef::All(i) => self.all.get_mut(i),
        }
    }

    pub fn count(&self) -> usize {
        self.all.len() + self.pages.values().map(Vec::len).sum::<usize>()
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// Add a guide to `page` (`size` in points, width × height), clamped onto it. `None` when
    /// the position is not finite or the page already has [`MAX_PER_PAGE`] guides.
    pub fn add(&mut self, page: usize, mut guide: Guide, size: [f32; 2]) -> Option<GuideRef> {
        guide.at = clamp_onto(&guide, size)?;
        let list = self.pages.entry(page).or_default();
        if list.len() >= MAX_PER_PAGE {
            return None;
        }
        list.push(guide);
        Some(GuideRef::Page(page, list.len() - 1))
    }

    /// Add a guide shown on every page. Pages differ in size, so it is not clamped; only a
    /// distance that isn't a finite number is refused.
    pub fn add_all(&mut self, guide: Guide) -> Option<GuideRef> {
        if !guide.at.is_finite() || self.all.len() >= MAX_PER_PAGE {
            return None;
        }
        self.all.push(guide);
        Some(GuideRef::All(self.all.len() - 1))
    }

    /// Move a guide to `position` points from the top or left edge of a page `size` large.
    pub fn set_position(&mut self, r: GuideRef, position: f32, size: [f32; 2]) -> bool {
        if !position.is_finite() {
            return false;
        }
        let Some(g) = self.get_mut(r) else { return false };
        let length = g.length(size);
        g.set_position(if length > 0.0 { position.clamp(0.0, length) } else { position }, size);
        true
    }

    pub fn set_style(&mut self, r: GuideRef, style: Style) -> bool {
        self.get_mut(r).map(|g| g.style = style).is_some()
    }

    pub fn set_label(&mut self, r: GuideRef, label: Option<String>) -> bool {
        self.get_mut(r).map(|g| g.label = label.filter(|l| !l.trim().is_empty())).is_some()
    }

    pub fn remove(&mut self, r: GuideRef) -> Option<Guide> {
        match r {
            GuideRef::Page(p, i) => {
                let list = self.pages.get_mut(&p)?;
                let g = (i < list.len()).then(|| list.remove(i));
                if list.is_empty() {
                    self.pages.remove(&p);
                }
                g
            }
            GuideRef::All(i) => (i < self.all.len()).then(|| self.all.remove(i)),
        }
    }

    /// Put a page's guide on every page, or a guide on every page on `page` only.
    pub fn toggle_every_page(&mut self, r: GuideRef, page: usize, size: [f32; 2]) -> Option<GuideRef> {
        let guide = self.get(r)?.clone();
        let moved = match r {
            GuideRef::Page(..) => self.add_all(guide)?,
            GuideRef::All(_) => self.add(page, guide, size)?,
        };
        self.remove(r);
        // Removing shifted later indexes on the same list; the new guide went to the other list.
        Some(moved)
    }

    pub fn clear(&mut self) {
        self.pages.clear();
        self.all.clear();
    }
}

fn clamp_onto(guide: &Guide, size: [f32; 2]) -> Option<f32> {
    let length = guide.length(size);
    (guide.at.is_finite() && length.is_finite() && length > 0.0).then(|| guide.at.clamp(0.0, length))
}

/// The grid, in points, and how the grid and guides look (Preferences ▸ Units and guides).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GridSettings {
    /// Distance between vertical grid lines.
    pub width: f32,
    /// Distance between horizontal grid lines.
    pub height: f32,
    /// Where the first vertical line is, from the page's left edge.
    pub offset_left: f32,
    /// Where the first horizontal line is, from the page's top edge.
    pub offset_top: f32,
    /// Lighter lines dividing each grid cell; 1 draws none.
    pub subdivisions: u8,
    pub grid_color: [u8; 4],
    /// Grid line thickness, in screen points.
    pub grid_line_width: f32,
    /// The colour and thickness of guides that don't have their own.
    pub guide_color: [u8; 4],
    pub guide_line_width: f32,
}

impl Default for GridSettings {
    fn default() -> Self {
        GridSettings {
            width: 72.0,
            height: 72.0,
            offset_left: 0.0,
            offset_top: 0.0,
            subdivisions: 4,
            grid_color: [96, 140, 210, 110],
            grid_line_width: 1.0,
            guide_color: [0, 160, 230, 255],
            guide_line_width: 1.0,
        }
    }
}

/// Shortest grid spacing accepted, in points: a hundredth of a millimetre would ask for millions of lines.
pub const MIN_GRID_SPACING: f32 = 1.0;
pub const MAX_GRID_SPACING: f32 = 14_400.0;
pub const MAX_SUBDIVISIONS: u8 = 20;

impl GridSettings {
    /// The settings with every number in its allowed range: what a hand-edited or older settings
    /// file holds is never trusted.
    pub fn sanitized(self) -> GridSettings {
        use crate::guide_sets::{MAX_WIDTH, MIN_WIDTH};
        let d = GridSettings::default();
        let spacing = |v: f32, fallback: f32| if v.is_finite() { v.clamp(MIN_GRID_SPACING, MAX_GRID_SPACING) } else { fallback };
        let offset = |v: f32| if v.is_finite() { v.clamp(-MAX_GRID_SPACING, MAX_GRID_SPACING) } else { 0.0 };
        let line = |v: f32| if v.is_finite() { v.clamp(MIN_WIDTH, MAX_WIDTH) } else { 1.0 };
        GridSettings {
            width: spacing(self.width, d.width),
            height: spacing(self.height, d.height),
            offset_left: offset(self.offset_left),
            offset_top: offset(self.offset_top),
            subdivisions: self.subdivisions.clamp(1, MAX_SUBDIVISIONS),
            grid_line_width: line(self.grid_line_width),
            guide_line_width: line(self.guide_line_width),
            ..self
        }
    }

    pub fn grid_color32(&self) -> Color32 {
        rgba(self.grid_color)
    }

    /// The look of a guide that has none of its own.
    pub fn guide_style(&self) -> Style {
        Style { color: Some(self.guide_color), width: Some(self.guide_line_width) }
    }

    /// The grid spacing and offset along a guide's axis: horizontal guides snap to the
    /// horizontal lines, which are `height` apart.
    fn along(&self, axis: GuideAxis) -> (f32, f32) {
        match axis {
            GuideAxis::Horizontal => (self.height, self.offset_top),
            GuideAxis::Vertical => (self.width, self.offset_left),
        }
    }

    /// `at` (points from the top or left edge) snapped to the nearest grid line, subdivisions
    /// included, along `axis`, if one is within `tolerance` points; otherwise `at` unchanged.
    pub fn snap(&self, axis: GuideAxis, at: f32, tolerance: f32) -> f32 {
        let s = self.sanitized();
        let (spacing, offset) = s.along(axis);
        let step = spacing / f32::from(s.subdivisions);
        if !(at.is_finite() && tolerance.is_finite() && step > 0.0) {
            return at;
        }
        let nearest = offset + ((at - offset) / step).round() * step;
        if (nearest - at).abs() <= tolerance { nearest } else { at }
    }
}

fn rgba([r, g, b, a]: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn stroke(style: Style) -> Stroke {
    Stroke::new(style.width.unwrap_or(1.0), style.color.map_or(Color32::from_rgb(0, 160, 230), rgba))
}

/// Grid lines across one page dimension of `length` points: (position in points, is a main line).
/// Subdivisions are left out once they would be closer than a few screen points at `scale`
/// (screen points per page point), and the whole grid once its main lines would be.
pub fn grid_lines(length: f32, spacing: f32, offset: f32, subdivisions: u8, scale: f32) -> Vec<(f32, bool)> {
    let subdivisions = subdivisions.clamp(1, MAX_SUBDIVISIONS);
    if !(length.is_finite() && spacing.is_finite() && offset.is_finite() && scale.is_finite())
        || length <= 0.0
        || spacing < MIN_GRID_SPACING
        || scale <= 0.0
    {
        return Vec::new();
    }
    if spacing * scale < MIN_LINE_SPACING {
        return Vec::new();
    }
    let parts = if spacing / f32::from(subdivisions) * scale < MIN_LINE_SPACING { 1 } else { subdivisions };
    let step = spacing / f32::from(parts);
    // The first line at or after the top or left edge.
    let mut k = ((0.0 - offset) / step).ceil();
    let mut out = Vec::new();
    loop {
        let at = offset + k * step;
        if at > length || out.len() >= MAX_GRID_LINES {
            break;
        }
        // A main line every `parts` steps, counted from the offset.
        out.push((at, k.rem_euclid(f32::from(parts)).abs() < 0.5));
        k += 1.0;
    }
    out
}

/// A screen direction: the line a horizontal ruler draws out runs across the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenAxis {
    Horizontal,
    Vertical,
}

fn xform(page: &VisiblePageGeometry) -> PageXform {
    PageXform {
        rect: page.page_rect,
        rot: page.rotation_degrees,
        pw: page.unrotated_size_points.x.max(1.0),
        ph: page.unrotated_size_points.y.max(1.0),
    }
}

pub(super) fn page_size(page: &VisiblePageGeometry) -> [f32; 2] {
    [page.unrotated_size_points.x, page.unrotated_size_points.y]
}

/// Screen points per page point on `page`. The two axes differ only by pixel snapping, so their
/// mean serves for snapping tolerances and for leaving out crowded grid lines.
fn scale(page: &VisiblePageGeometry) -> f32 {
    let s = (page.screen_points_per_pdf_point_x + page.screen_points_per_pdf_point_y) / 2.0;
    if s.is_finite() && s > 0.0 { s } else { 0.0 }
}

/// The page axis a screen line lands on: View ▸ Rotate by a quarter turn swaps them.
pub fn page_axis(screen: ScreenAxis, rotation_degrees: u16) -> GuideAxis {
    let quarter = rotation_degrees % 180 == 90;
    match (screen, quarter) {
        (ScreenAxis::Horizontal, false) | (ScreenAxis::Vertical, true) => GuideAxis::Horizontal,
        _ => GuideAxis::Vertical,
    }
}

/// The guide a screen line through `pointer` makes on `page`, from the top or left edge.
pub(super) fn guide_at(page: &VisiblePageGeometry, screen: ScreenAxis, pointer: Pos2) -> Option<Guide> {
    if !pointer.is_finite() {
        return None;
    }
    let (u, v) = xform(page).screen_to_norm(pointer);
    let axis = page_axis(screen, page.rotation_degrees);
    let at = match axis {
        GuideAxis::Horizontal => v * page.unrotated_size_points.y,
        GuideAxis::Vertical => u * page.unrotated_size_points.x,
    };
    at.is_finite().then(|| Guide::new(axis, at))
}

/// The guide's line on screen, from one page edge to the other.
pub(super) fn screen_segment(page: &VisiblePageGeometry, guide: &Guide) -> [Pos2; 2] {
    let xf = xform(page);
    let at = guide.position(page_size(page));
    match guide.axis {
        GuideAxis::Horizontal => {
            let v = at / xf.ph;
            [xf.norm_to_screen(0.0, v), xf.norm_to_screen(1.0, v)]
        }
        GuideAxis::Vertical => {
            let u = at / xf.pw;
            [xf.norm_to_screen(u, 0.0), xf.norm_to_screen(u, 1.0)]
        }
    }
}

/// Whether the segment runs across the screen (the other way it runs down).
fn runs_across(segment: [Pos2; 2]) -> bool {
    (segment[1].x - segment[0].x).abs() >= (segment[1].y - segment[0].y).abs()
}

/// The strip around a guide's line that picks it up.
pub fn hit_strip(segment: [Pos2; 2]) -> Rect {
    Rect::from_two_pos(segment[0], segment[1]).expand(HIT)
}

/// The strips of every document guide on the visible pages, as (page, guide, strip), clipped to
/// `viewport`. A guide on every page has a strip on each visible page.
pub(super) fn hit_strips(guides: &Guides, pages: &[VisiblePageGeometry], viewport: Rect) -> Vec<(usize, GuideRef, Rect)> {
    let mut out = Vec::new();
    for page in pages.iter().filter(|p| p.page_rect.intersects(viewport)) {
        for (r, g) in guides.shown_on(page.page_index) {
            let strip = hit_strip(screen_segment(page, g)).intersect(viewport);
            if strip.is_positive() {
                out.push((page.page_index, r, strip));
            }
        }
    }
    out
}

/// A guide being dragged: out of a ruler, or an existing one, picked up on `page`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GuideDrag {
    FromRuler(ScreenAxis),
    Existing { guide: GuideRef, page: usize },
}

/// The guide whose Properties window is open, and the page it was opened on.
pub type OpenProperties = Option<(GuideRef, usize)>;

/// A document view's guides, and the drag and Properties window working on them.
#[derive(Clone, Debug, Default)]
pub(crate) struct ViewGuides {
    pub(crate) guides: Guides,
    drag: Option<GuideDrag>,
    props: OpenProperties,
}

impl ViewGuides {
    /// Remove every guide, and end a drag or a Properties window on one.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Replace the guides, as a set copied in or sent by the control channel does.
    pub(crate) fn replace(&mut self, guides: Guides) {
        *self = Self { guides, ..Self::default() };
    }
}

/// The app's guide and grid settings for one frame of one document view.
#[derive(Clone, Copy)]
pub(super) struct Settings<'a> {
    pub(super) show_guides: bool,
    pub(super) show_grid: bool,
    pub(super) snap_to_grid: bool,
    pub(super) grid: GridSettings,
    /// Every set read from the folder, and the names of those shown.
    pub(super) sets: &'a [crate::guide_sets::Loaded],
    pub(super) shown_sets: &'a std::collections::BTreeSet<String>,
    pub(super) unit: crate::units::Unit,
    /// No window of the guides' own (Save guides as a set) is open over the pages.
    pub(super) editable: bool,
}

impl Settings<'_> {
    fn shown(&self) -> Vec<&GuideSet> {
        crate::guide_sets::shown(self.sets, self.shown_sets)
    }
}

/// A choice in the rulers' menu, or a click on their corner, for the app to carry out.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RulerMenuAction {
    Units(crate::units::Unit),
    ToggleGuides,
    ClearGuides,
    ToggleGrid,
    ToggleSnap,
    SaveSet,
    ToggleSet(String),
    ApplySet(String),
}

/// Where the guides are drawn this frame and whether they take input.
#[derive(Clone, Copy)]
pub(super) struct Place {
    /// The document viewport on screen.
    pub(super) viewport: Rect,
    /// The rulers over it, when shown.
    pub(super) rulers: Option<RulerLayout>,
    /// Nothing (a dialog, the palette) is over the pages.
    pub(super) unobstructed: bool,
    /// A dialog is open; passed on to [`PagePointer`].
    pub(super) modal: bool,
}

/// Paint the grid, the shown sets and the document's guides with their labels over the pages laid
/// out this frame, and handle the rulers' and the guides' input: a drag out of a ruler adds a guide,
/// a drag on a guide moves it (onto a ruler or off its page removes it), a right-click opens the
/// guide's or the rulers' menu, a double-click the guide's Properties, a click on the rulers'
/// corner steps the unit. Call after the page loop, so the guides' widgets are above the page
/// widget, and before the pages' cursor is resolved, so their cursor claims count. The rulers are
/// painted later, over the guides.
pub(super) fn show(ui: &egui::Ui, view: &mut DocView, s: &Settings<'_>, place: Place) -> Option<RulerMenuAction> {
    if view.presentation.is_some() || view.organize {
        view.guides.drag = None;
        return None;
    }
    let pages = rulers::visible_pages(&view.screen_xforms);
    // Guides and the grid lie over the pages and under the rulers.
    let area = place.rulers.map_or(place.viewport, |l| l.page_area(place.viewport));
    let on_screen = on_screen(&pages, area);
    let grid = s.grid.sanitized();
    let defaults = grid.guide_style();
    let sets = s.shown();
    let painter = ui.painter_at(area);
    if s.show_grid {
        paint_grid(&painter, &on_screen, &grid);
    }
    // A shown set that brings its own grid draws it too.
    for set in &sets {
        if let Some(g) = set.grid {
            paint_grid(&painter, &on_screen, &g.sanitized());
        }
    }
    let state = &mut view.guides;
    let interactive = place.unobstructed && s.editable;
    let id = egui::Id::unique(("pdfcraft-guides", view.id.0));
    let mut action = None;
    // The widget holding the drag in progress: its pointer is the drag's.
    let mut owner = None;
    if let Some(layout) = place.rulers.filter(|_| interactive) {
        let input = RulerInput::interact(ui, id, layout);
        let set_list: Vec<(String, bool, Option<u8>)> =
            s.sets.iter().filter_map(|l| l.set.as_ref().ok()).map(|set| (set.name.clone(), s.shown_sets.contains(&set.name), set.shortcut)).collect();
        for (resp, axis, icon) in
            [(&input.horizontal, ScreenAxis::Horizontal, CursorIcon::ResizeRow), (&input.vertical, ScreenAxis::Vertical, CursorIcon::ResizeColumn)]
        {
            if s.show_guides && state.drag.is_none() && resp.drag_started_by(egui::PointerButton::Primary) {
                state.drag = Some(GuideDrag::FromRuler(axis));
            }
            if state.drag == Some(GuideDrag::FromRuler(axis)) && (resp.dragged() || resp.drag_stopped()) {
                owner = Some(resp.clone());
            }
            if s.show_guides && resp.hovered() {
                view.cursor.claim(Layer::Handle, icon);
            }
        }
        if let Some(u) = input.unit_after_click(s.unit) {
            action = Some(RulerMenuAction::Units(u));
        }
        for resp in [&input.horizontal, &input.vertical, &input.corner] {
            resp.context_menu(|ui| {
                if let Some(a) = ruler_menu(ui, s, !state.guides.is_empty(), &set_list) {
                    action = Some(a);
                }
            });
        }
    }
    if !s.show_guides {
        *state = ViewGuides { guides: std::mem::take(&mut state.guides), ..ViewGuides::default() };
        return action;
    }
    for page in &on_screen {
        for set in &sets {
            let style = set.style.or(defaults);
            for g in set.on_page(page.page_index) {
                let seg = screen_segment(page, g);
                let st = stroke(g.style.or(style));
                painter.extend(egui::Shape::dashed_line(&seg, st, 6.0, 3.0));
                paint_label(&painter, seg, g.label.as_deref(), st.color);
            }
        }
        for (_, g) in state.guides.shown_on(page.page_index) {
            let seg = screen_segment(page, g);
            let st = stroke(g.style.or(defaults));
            painter.line_segment(seg, st);
            paint_label(&painter, seg, g.label.as_deref(), st.color);
        }
    }
    let strips = if interactive { hit_strips(&state.guides, &on_screen, area) } else { Vec::new() };
    let mut change = None;
    for &(page, r, strip) in &strips {
        let resp = ui.interact(strip, id.with(("guide", page, r)), Sense::click_and_drag());
        let across = on_screen.iter().find(|p| p.page_index == page).zip(state.guides.get(r)).is_some_and(|(p, g)| runs_across(screen_segment(p, g)));
        if resp.hovered() {
            view.cursor.claim(Layer::Handle, if across { CursorIcon::ResizeRow } else { CursorIcon::ResizeColumn });
        }
        let this = GuideDrag::Existing { guide: r, page };
        if state.drag.is_none() && resp.drag_started_by(egui::PointerButton::Primary) {
            state.drag = Some(this);
        }
        if state.drag == Some(this) && (resp.dragged() || resp.drag_stopped()) {
            owner = Some(resp.clone());
        }
        if resp.double_clicked() {
            state.props = Some((r, page));
        }
        resp.context_menu(|ui| {
            if let Some(a) = guide_menu(ui, matches!(r, GuideRef::All(_))) {
                change = Some((r, page, a));
            }
        });
    }
    if let Some((r, page, a)) = change {
        let size = pages.iter().find(|p| p.page_index == page).map(page_size).unwrap_or([0.0, 0.0]);
        apply(&mut state.guides, &mut state.props, r, page, size, a);
    }
    properties_window(ui.ctx(), &pages, s.unit, &defaults, state);
    let drop = Drop { pages: &pages, area, rulers: place.rulers, modal: place.modal, snap_to_grid: s.snap_to_grid, grid: &grid };
    if let Some(icon) = drag_in_progress(ui, &drop, state, owner.as_ref()) {
        view.cursor.claim(Layer::Drag, icon);
    }
    action
}

/// The pages of `pages` that show in `viewport`.
pub(super) fn on_screen(pages: &[VisiblePageGeometry], viewport: Rect) -> Vec<VisiblePageGeometry> {
    pages.iter().filter(|p| p.page_rect.intersects(viewport)).copied().collect()
}

/// A guide's label at the start of its line, on a small backing so it reads over the page.
fn paint_label(painter: &egui::Painter, seg: [Pos2; 2], label: Option<&str>, color: Color32) {
    let Some(label) = label.filter(|l| !l.trim().is_empty()) else { return };
    let start = if runs_across(seg) {
        if seg[0].x <= seg[1].x { seg[0] } else { seg[1] }
    } else if seg[0].y <= seg[1].y {
        seg[0]
    } else {
        seg[1]
    };
    let font = egui::FontId::proportional(11.0);
    let galley = painter.layout_no_wrap(label.to_string(), font, color.to_opaque());
    let at = start + egui::vec2(4.0, 2.0);
    let back = Rect::from_min_size(at, galley.size()).expand(2.0);
    painter.rect_filled(back, 2.0, Color32::from_white_alpha(220));
    painter.galley(at, galley, color.to_opaque());
}

/// A change to one guide, from its menu or its Properties window.
#[derive(Clone, Debug, PartialEq)]
enum GuideChange {
    Delete,
    EveryPage,
    Style(Style),
    Label(Option<String>),
    Properties,
}

/// Colours offered in a guide's right-click menu: one click, no picker.
const SWATCHES: [[u8; 4]; 8] = [
    [0, 160, 230, 255],
    [230, 0, 92, 255],
    [255, 140, 0, 255],
    [40, 170, 60, 255],
    [140, 70, 210, 255],
    [230, 200, 0, 255],
    [0, 0, 0, 255],
    [128, 128, 128, 255],
];

fn guide_menu(ui: &mut egui::Ui, every: bool) -> Option<GuideChange> {
    let mut out = None;
    let mut on_every = every;
    if ui.add(crate::widgets::checkbox(&mut on_every, tl!("On every page"))).clicked() {
        out = Some(GuideChange::EveryPage);
        ui.close();
    }
    ui.separator();
    ui.horizontal(|ui| {
        for c in SWATCHES {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::click());
            ui.painter().rect_filled(rect, 3.0, rgba(c));
            if resp.hovered() {
                ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.5, ui.visuals().strong_text_color()), egui::StrokeKind::Outside);
            }
            if resp.clicked() {
                out = Some(GuideChange::Style(Style { color: Some(c), width: None }));
                ui.close();
            }
        }
    });
    if ui.button(tl!("Properties…")).clicked() {
        out = Some(GuideChange::Properties);
        ui.close();
    }
    if ui.button(tl!("Delete guide")).clicked() {
        out = Some(GuideChange::Delete);
        ui.close();
    }
    out
}

/// The rulers' right-click menu: the unit, and the guide, grid and guide-set switches.
fn ruler_menu(ui: &mut egui::Ui, s: &Settings<'_>, any_guides: bool, sets: &[(String, bool, Option<u8>)]) -> Option<RulerMenuAction> {
    let mut action = None;
    for u in crate::units::Unit::ALL {
        if ui.add(crate::widgets::radio(s.unit == u, tl!(u.name()))).clicked() {
            action = Some(RulerMenuAction::Units(u));
            ui.close();
        }
    }
    ui.separator();
    for (on, label, a) in [
        (s.show_guides, tl!("Guides"), RulerMenuAction::ToggleGuides),
        (s.show_grid, tl!("Grid"), RulerMenuAction::ToggleGrid),
        (s.snap_to_grid, tl!("Snap to grid"), RulerMenuAction::ToggleSnap),
    ] {
        let mut checked = on;
        if ui.add(crate::widgets::checkbox(&mut checked, label)).clicked() {
            action = Some(a);
            ui.close();
        }
    }
    if ui.add_enabled(any_guides, egui::Button::new(tl!("Clear guides"))).clicked() {
        action = Some(RulerMenuAction::ClearGuides);
        ui.close();
    }
    ui.separator();
    if ui.add_enabled(any_guides || s.show_grid, egui::Button::new(tl!("Save guides as a set…"))).clicked() {
        action = Some(RulerMenuAction::SaveSet);
        ui.close();
    }
    ui.menu_button(tl!("Guide sets"), |ui| {
        if sets.is_empty() {
            ui.label(tl!("No saved sets yet"));
        }
        for (name, shown, shortcut) in sets {
            ui.horizontal(|ui| {
                let mut on = *shown;
                let text = match shortcut {
                    Some(n) => format!("{name}  ({})", set_shortcut_label(*n, ui.ctx())),
                    None => name.clone(),
                };
                if ui.add(crate::widgets::checkbox(&mut on, text)).clicked() {
                    action = Some(RulerMenuAction::ToggleSet(name.clone()));
                    ui.close();
                }
                if ui
                    .small_button(tl!("Edit in document"))
                    .on_hover_text(tl!("Replace this document's guides with the set's, to move and change them"))
                    .clicked()
                {
                    action = Some(RulerMenuAction::ApplySet(name.clone()));
                    ui.close();
                }
            });
        }
    });
    action
}

/// How the shortcut of guide set `n` is written: Ctrl+Shift+1, or ⇧⌘1 on macOS.
pub(crate) fn set_shortcut_label(n: u8, ctx: &egui::Context) -> String {
    const DIGITS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];
    let digit = DIGITS.get(usize::from(n).wrapping_sub(1)).copied().unwrap_or("?");
    crate::commands::shortcut_label(ctx, pdfcraft_engine::commands::Shortcut::cmd_shift(digit))
}

/// Apply a change; keeps the Properties window on the guide when it moves between lists.
fn apply(guides: &mut Guides, props: &mut OpenProperties, r: GuideRef, page: usize, size: [f32; 2], change: GuideChange) -> bool {
    match change {
        GuideChange::Delete => {
            let removed = guides.remove(r).is_some();
            if removed {
                props_after_removing(props, r);
            }
            removed
        }
        GuideChange::EveryPage => match guides.toggle_every_page(r, page, size) {
            Some(moved) => {
                if props.is_some_and(|(p, _)| p == r) {
                    *props = Some((moved, page));
                } else {
                    props_after_removing(props, r);
                }
                true
            }
            None => false,
        },
        // A swatch keeps the guide's own thickness.
        GuideChange::Style(s) => {
            let width = s.width.or(guides.get(r).and_then(|g| g.style.width));
            guides.set_style(r, Style { width, ..s })
        }
        GuideChange::Label(l) => guides.set_label(r, l),
        GuideChange::Properties => {
            *props = Some((r, page));
            false
        }
    }
}

/// The Properties window of one guide: colour, thickness, label, every page, delete.
fn properties_window(ctx: &egui::Context, pages: &[VisiblePageGeometry], unit: crate::units::Unit, defaults: &Style, state: &mut ViewGuides) -> bool {
    let Some((r, page)) = state.props else { return false };
    let Some(g) = state.guides.get(r).cloned() else {
        state.props = None;
        return false;
    };
    let size = pages.iter().find(|p| p.page_index == page).map(page_size).unwrap_or([0.0, 0.0]);
    let mut open = true;
    let mut change = None;
    let shown = g.style.or(*defaults);
    egui::Window::new(tl!("Guide properties"))
        .id(egui::Id::unique("pdfcraft-guide-properties"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("guide-properties").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(tl!("Color"));
                let mut c = rgba(shown.color.unwrap_or([0, 160, 230, 255]));
                if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::OnlyBlend).changed() {
                    change = Some(GuideChange::Style(Style { color: Some(c.to_srgba_unmultiplied()), ..g.style }));
                }
                ui.end_row();
                ui.label(tl!("Thickness"));
                let mut w = shown.width.unwrap_or(1.0);
                let field = egui::DragValue::new(&mut w)
                    .range(crate::guide_sets::MIN_WIDTH..=crate::guide_sets::MAX_WIDTH)
                    .speed(0.1)
                    .max_decimals(1)
                    .suffix(" pt");
                if ui.add(field).changed() {
                    change = Some(GuideChange::Style(Style { width: Some(w), ..g.style }));
                }
                ui.end_row();
                ui.label(tl!("Label"));
                let mut text = g.label.clone().unwrap_or_default();
                if ui.add(egui::TextEdit::singleline(&mut text).desired_width(180.0)).changed() {
                    change = Some(GuideChange::Label(Some(text.chars().take(120).collect())));
                }
                ui.end_row();
                ui.label(tl!("Position"));
                ui.label(format!("{:.1} {}", g.position(size) * unit.per_point() as f32, unit.label()));
                ui.end_row();
            });
            let mut every = matches!(r, GuideRef::All(_));
            if ui.add(crate::widgets::checkbox(&mut every, tl!("On every page"))).clicked() {
                change = Some(GuideChange::EveryPage);
            }
            ui.horizontal(|ui| {
                if (g.style.color.is_some() || g.style.width.is_some()) && ui.button(tl!("Use the default look")).clicked() {
                    change = Some(GuideChange::Style(Style::default()));
                }
                if ui.button(tl!("Delete guide")).clicked() {
                    change = Some(GuideChange::Delete);
                }
            });
        });
    if !open {
        state.props = None;
    }
    // A plain style change replaces the whole style here (the window shows both fields).
    match change {
        Some(GuideChange::Style(s)) => state.guides.set_style(r, s),
        Some(c) => apply(&mut state.guides, &mut state.props, r, page, size, c),
        None => false,
    }
}

/// Where a dragged guide can land this frame.
struct Drop<'a> {
    pages: &'a [VisiblePageGeometry],
    /// The page area, without the rulers.
    area: Rect,
    /// Releasing over a ruler removes the guide.
    rulers: Option<RulerLayout>,
    modal: bool,
    snap_to_grid: bool,
    grid: &'a GridSettings,
}

/// Where the dragged guide lands when released at `pointer`: the page and the guide, from the top
/// or left edge and snapped to the grid if asked. `None` over a ruler or off the pages (a page's
/// own guide also off its own page), where a release removes the guide.
fn landing<'a>(d: &Drop<'a>, guides: &Guides, current: GuideDrag, pointer: Pos2) -> (ScreenAxis, Option<(&'a VisiblePageGeometry, Guide)>) {
    let over_ruler = d.rulers.is_some_and(|l| [l.corner, l.horizontal, l.vertical].iter().any(|r| r.contains(pointer)));
    let (screen_axis, target) = match current {
        GuideDrag::FromRuler(axis) => (axis, d.pages.iter().find(|p| p.page_rect.contains(pointer))),
        GuideDrag::Existing { guide, page } => {
            let picked = d.pages.iter().find(|p| p.page_index == page);
            let axis = picked.zip(guides.get(guide)).map_or(ScreenAxis::Horizontal, |(p, g)| {
                if runs_across(screen_segment(p, g)) { ScreenAxis::Horizontal } else { ScreenAxis::Vertical }
            });
            // A guide on every page may be dropped on any visible page; a page's own stays on it.
            let target = match guide {
                GuideRef::All(_) => d.pages.iter().find(|p| p.page_rect.contains(pointer)),
                GuideRef::Page(..) => picked.filter(|p| p.page_rect.contains(pointer)),
            };
            (axis, target)
        }
    };
    let landing = target.filter(|_| !over_ruler && d.area.contains(pointer)).and_then(|page| {
        let mut g = guide_at(page, screen_axis, pointer)?;
        if d.snap_to_grid {
            let tolerance = SNAP / scale(page).max(f32::EPSILON);
            g.at = d.grid.snap(g.axis, g.at, tolerance);
        }
        Some((page, g))
    });
    (screen_axis, landing)
}

/// The drag in progress, whether it began on a ruler or on a guide: its preview, and on release the
/// guide added, moved or removed. The pointer is read through [`PagePointer`] from the widget
/// holding the drag (`owner`); without one (its strip scrolled away, the rulers hidden) the drag
/// ends with no change. Returns the cursor the drag shows.
fn drag_in_progress(ui: &egui::Ui, d: &Drop<'_>, state: &mut ViewGuides, owner: Option<&egui::Response>) -> Option<CursorIcon> {
    let current = state.drag?;
    let Some(owner) = owner else {
        state.drag = None;
        return None;
    };
    let ptr = PagePointer::read_excluding(ui, owner, d.modal, &[]);
    let released = owner.drag_stopped() || !ptr.primary_down;
    let Some(pointer) = ptr.pos else {
        if released {
            state.drag = None;
        }
        return None;
    };
    let (screen_axis, landing) = landing(d, &state.guides, current, pointer);
    let painter = ui.painter_at(d.area);
    let preview = stroke(d.grid.guide_style()).color.gamma_multiply(0.8);
    match &landing {
        Some((page, g)) => {
            painter.line_segment(screen_segment(page, g), Stroke::new(1.0, preview));
        }
        None => {
            // Off the pages (or back on a ruler) the guide would be removed: show it across the
            // whole view, dashed, so the user can see it is not on a page.
            let (a, b) = match screen_axis {
                ScreenAxis::Horizontal => (pos2(d.area.left(), pointer.y), pos2(d.area.right(), pointer.y)),
                ScreenAxis::Vertical => (pos2(pointer.x, d.area.top()), pos2(pointer.x, d.area.bottom())),
            };
            painter.extend(egui::Shape::dashed_line(&[a, b], Stroke::new(1.0, preview), 4.0, 4.0));
        }
    }
    let icon = if screen_axis == ScreenAxis::Horizontal { CursorIcon::ResizeRow } else { CursorIcon::ResizeColumn };
    if released {
        state.drag = None;
        // Shift on release puts a new guide on every page.
        let shift = ui.input(|i| i.modifiers.shift);
        drop_guide(&mut state.guides, &mut state.props, current, landing.map(|(page, g)| (page.page_index, page_size(page), g)), shift);
    }
    Some(icon)
}

/// Carry out a released drag: add the new guide, move the guide picked up, or remove it when it
/// lands nowhere (`landing` is the page, its size and the guide there). Returns whether the
/// guides changed.
fn drop_guide(guides: &mut Guides, props: &mut OpenProperties, current: GuideDrag, landing: Option<(usize, [f32; 2], Guide)>, shift: bool) -> bool {
    match (current, landing) {
        (GuideDrag::FromRuler(_), Some((_, _, g))) if shift => guides.add_all(g).is_some(),
        (GuideDrag::FromRuler(_), Some((page, size, g))) => guides.add(page, g, size).is_some(),
        (GuideDrag::FromRuler(_), None) => false,
        (GuideDrag::Existing { guide, .. }, Some((_, size, g))) => guides.set_position(guide, g.at, size),
        (GuideDrag::Existing { guide, .. }, None) => {
            let removed = guides.remove(guide).is_some();
            if removed {
                props_after_removing(props, guide);
            }
            removed
        }
    }
}

fn paint_grid(painter: &egui::Painter, pages: &[VisiblePageGeometry], grid: &GridSettings) {
    let main = Stroke::new(grid.grid_line_width, grid.grid_color32());
    let sub = Stroke::new(grid.grid_line_width, grid.grid_color32().gamma_multiply(0.45));
    for page in pages {
        let [pw, ph] = page_size(page);
        let scale = scale(page);
        for (axis, length, spacing, offset) in
            [(GuideAxis::Horizontal, ph, grid.height, grid.offset_top), (GuideAxis::Vertical, pw, grid.width, grid.offset_left)]
        {
            for (at, is_main) in grid_lines(length, spacing, offset, grid.subdivisions, scale) {
                painter.line_segment(screen_segment(page, &Guide::new(axis, at)), if is_main { main } else { sub });
            }
        }
    }
}

/// Ctrl+Shift+1 … 9 (⇧⌘ on macOS): the guide set with that shortcut number, consumed from this
/// frame's input. The digit is read from the physical key as well, because Shift turns it into
/// `!`, `"`, `§` … on many layouts.
pub(crate) fn set_shortcut(ctx: &egui::Context) -> Option<u8> {
    use egui::{Event, Key};
    const DIGITS: [Key; 9] = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
    ctx.input_mut(|i| {
        let mut hit = None;
        i.events.retain(|e| {
            if let Event::Key { key, physical_key, pressed: true, modifiers, .. } = e
                && modifiers.command
                && modifiers.shift
                && !modifiers.alt
                && hit.is_none()
                && let Some(n) = DIGITS.iter().position(|d| d == key || Some(*d) == *physical_key)
            {
                hit = u8::try_from(n + 1).ok();
                return false;
            }
            true
        });
        hit
    })
}

/// A document view's guides for the control channel: in the guide-set schema (so an agent can
/// read, edit and send them back), and where each lies on screen on the pages drawn last frame.
pub(crate) fn state_json(view: &DocView, unit: crate::units::Unit) -> serde_json::Value {
    let pages = on_screen(&rulers::visible_pages(&view.screen_xforms), view.viewport_screen);
    pages_json(&view.guides.guides, &pages, unit)
}

fn pages_json(guides: &Guides, pages: &[VisiblePageGeometry], unit: crate::units::Unit) -> serde_json::Value {
    let set = crate::guide_sets::from_document("document", guides, unit, None, None);
    let mut on_screen = Vec::new();
    for page in pages {
        for (r, g) in guides.shown_on(page.page_index) {
            let [a, b] = screen_segment(page, g);
            on_screen.push(serde_json::json!({
                "page": page.page_index + 1,
                "every_page": matches!(r, GuideRef::All(_)),
                "axis": if g.axis == GuideAxis::Horizontal { "horizontal" } else { "vertical" },
                "from_top_or_left_pt": g.position(page_size(page)),
                "screen": [[a.x, a.y], [b.x, b.y]],
                "label": g.label,
            }));
        }
    }
    serde_json::json!({ "count": guides.count(), "set": crate::guide_sets::to_json(&set), "on_screen": on_screen })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    fn page(rot: u16) -> VisiblePageGeometry {
        // A 200 × 100 pt page shown at 2 screen points per point from (10, 20).
        let (w, h) = if rot.is_multiple_of(180) { (400.0, 200.0) } else { (200.0, 400.0) };
        VisiblePageGeometry {
            page_index: 3,
            page_rect: Rect::from_min_size(pos2(10.0, 20.0), vec2(w, h)),
            unrotated_size_points: vec2(200.0, 100.0),
            rotation_degrees: rot,
            screen_points_per_pdf_point_x: 2.0,
            screen_points_per_pdf_point_y: 2.0,
        }
    }

    #[test]
    fn a_line_from_the_top_ruler_is_a_horizontal_guide_at_the_page_distance() {
        let p = page(0);
        let g = guide_at(&p, ScreenAxis::Horizontal, pos2(100.0, 20.0 + 60.0)).unwrap();
        assert_eq!(g.axis, GuideAxis::Horizontal);
        assert!((g.at - 30.0).abs() < 1e-4, "{g:?}");
        let seg = screen_segment(&p, &g);
        assert!(seg[0].distance(pos2(10.0, 80.0)) < 1e-3 && seg[1].distance(pos2(410.0, 80.0)) < 1e-3, "{seg:?}");
        let v = guide_at(&p, ScreenAxis::Vertical, pos2(10.0 + 50.0, 100.0)).unwrap();
        assert_eq!(v.axis, GuideAxis::Vertical);
        assert!((v.at - 25.0).abs() < 1e-4, "{v:?}");
    }

    /// View ▸ Rotate turns guides with the page: a quarter turn shows a guide that is a distance
    /// from the page's left edge across the screen.
    #[test]
    fn rotating_the_view_swaps_the_axes_and_keeps_the_guide_on_the_page() {
        for rot in [90, 270] {
            let p = page(rot);
            let g = guide_at(&p, ScreenAxis::Horizontal, pos2(100.0, 120.0)).unwrap();
            assert_eq!(g.axis, GuideAxis::Vertical, "rot {rot}");
            let seg = screen_segment(&p, &g);
            assert!((seg[0].y - 120.0).abs() < 1e-3 && (seg[1].y - 120.0).abs() < 1e-3, "rot {rot}: {seg:?}");
            assert!(runs_across(seg));
        }
        let g = guide_at(&page(180), ScreenAxis::Horizontal, pos2(100.0, 80.0)).unwrap();
        assert_eq!(g, Guide::new(GuideAxis::Horizontal, 70.0), "upside down, 60 screen points below the top is 30 above the bottom");
    }

    /// A guide measured from the right edge keeps its margin on a wider page, and moving it keeps
    /// it measured from that edge.
    #[test]
    fn a_guide_from_the_far_edge_follows_the_page_size() {
        let g = Guide { from: Edge::End, ..Guide::new(GuideAxis::Vertical, 20.0) };
        assert_eq!(g.position([200.0, 100.0]), 180.0);
        assert_eq!(g.position([600.0, 100.0]), 580.0);
        let mut gs = Guides::default();
        let r = gs.add_all(g).unwrap();
        assert!(gs.set_position(r, 150.0, [200.0, 100.0]));
        assert_eq!(gs.get(r).map(|g| (g.from, g.at)), Some((Edge::End, 50.0)));
    }

    #[test]
    fn guides_are_clamped_onto_their_page_and_capped() {
        let size = [200.0, 100.0];
        let mut gs = Guides::default();
        assert_eq!(gs.add(0, Guide::new(GuideAxis::Horizontal, 500.0), size), Some(GuideRef::Page(0, 0)));
        assert_eq!(gs.on(0)[0].at, 100.0);
        assert_eq!(gs.add(0, Guide::new(GuideAxis::Vertical, f32::NAN), size), None);
        assert_eq!(gs.add(0, Guide::new(GuideAxis::Vertical, 1.0), [0.0, 0.0]), None, "an empty page takes no guide");
        for i in 1..MAX_PER_PAGE {
            assert!(gs.add(0, Guide::new(GuideAxis::Vertical, i as f32), size).is_some());
        }
        assert_eq!(gs.add(0, Guide::new(GuideAxis::Vertical, 5.0), size), None);
        assert!(gs.set_position(GuideRef::Page(0, 1), -4.0, size));
        assert_eq!(gs.on(0)[1].at, 0.0);
        assert!(!gs.set_position(GuideRef::Page(0, 9_999), 1.0, size));
        assert!(!gs.set_position(GuideRef::Page(0, 1), f32::NAN, size));
        assert!(gs.remove(GuideRef::Page(0, 9_999)).is_none());
        assert!(gs.add_all(Guide::new(GuideAxis::Vertical, f32::INFINITY)).is_none());
        gs.clear();
        assert!(gs.is_empty() && gs.on(0).is_empty());
    }

    #[test]
    fn a_guide_moves_between_one_page_and_every_page() {
        let size = [200.0, 100.0];
        let mut gs = Guides::default();
        let r = gs.add(2, Guide::new(GuideAxis::Horizontal, 10.0), size).unwrap();
        let every = gs.toggle_every_page(r, 2, size).unwrap();
        assert_eq!(every, GuideRef::All(0));
        assert!(gs.on(2).is_empty() && gs.all().len() == 1);
        assert_eq!(gs.shown_on(7).count(), 1, "shown on any page");
        let back = gs.toggle_every_page(every, 5, size).unwrap();
        assert_eq!(back, GuideRef::Page(5, 0));
        assert!(gs.all().is_empty());
        assert!(gs.set_style(back, Style { color: Some([1, 2, 3, 255]), width: Some(2.0) }));
        assert!(gs.set_label(back, Some("  ".into())));
        assert_eq!(gs.get(back).unwrap().label, None, "a blank label is no label");
    }

    #[test]
    fn removing_the_last_guide_forgets_the_page() {
        let mut gs = Guides::default();
        gs.add(2, Guide::new(GuideAxis::Horizontal, 1.0), [10.0, 10.0]);
        gs.add(5, Guide::new(GuideAxis::Vertical, 2.0), [10.0, 10.0]);
        assert!(gs.remove(GuideRef::Page(2, 0)).is_some());
        assert!(!gs.pages.contains_key(&2));
        assert_eq!(gs.on(5), &[Guide::new(GuideAxis::Vertical, 2.0)]);
        assert_eq!(gs.count(), 1);
    }

    #[test]
    fn grid_lines_start_at_the_offset_and_mark_main_lines() {
        let lines = grid_lines(100.0, 40.0, 10.0, 2, 1.0);
        assert_eq!(lines.iter().map(|l| l.0).collect::<Vec<_>>(), [10.0, 30.0, 50.0, 70.0, 90.0]);
        assert_eq!(lines.iter().map(|l| l.1).collect::<Vec<_>>(), [true, false, true, false, true]);
        let lines = grid_lines(50.0, 20.0, 45.0, 1, 1.0);
        assert_eq!(lines.iter().map(|l| l.0).collect::<Vec<_>>(), [5.0, 25.0, 45.0], "an offset beyond the first cell still draws from the edge");
    }

    #[test]
    fn crowded_grid_lines_are_left_out_and_bad_input_draws_nothing() {
        assert!(grid_lines(100.0, 10.0, 0.0, 4, 1.0).iter().all(|l| l.1), "subdivisions 2.5 screen points apart are dropped");
        assert!(grid_lines(100.0, 2.0, 0.0, 1, 1.0).is_empty(), "main lines 2 screen points apart");
        assert!(grid_lines(f32::NAN, 10.0, 0.0, 1, 1.0).is_empty());
        assert!(grid_lines(100.0, 0.0, 0.0, 1, 1.0).is_empty());
        assert!(grid_lines(100.0, 10.0, 0.0, 1, 0.0).is_empty());
        assert!(grid_lines(1e9, 1.0, 0.0, 1, 100.0).len() <= MAX_GRID_LINES);
    }

    #[test]
    fn snapping_reaches_the_nearest_grid_or_subdivision_line_within_the_tolerance() {
        let g = GridSettings { width: 72.0, height: 36.0, offset_left: 0.0, offset_top: 6.0, subdivisions: 2, ..Default::default() };
        assert_eq!(g.snap(GuideAxis::Vertical, 70.0, 3.0), 72.0);
        assert_eq!(g.snap(GuideAxis::Vertical, 37.0, 3.0), 36.0, "a subdivision line");
        assert_eq!(g.snap(GuideAxis::Vertical, 50.0, 3.0), 50.0, "too far from any line");
        assert_eq!(g.snap(GuideAxis::Horizontal, 25.0, 2.0), 24.0, "horizontal lines from the top offset, 18 apart");
        assert_eq!(g.snap(GuideAxis::Horizontal, f32::INFINITY, 2.0), f32::INFINITY);
    }

    #[test]
    fn stored_grid_settings_are_brought_into_range() {
        let bad = GridSettings {
            width: 0.0,
            height: f32::NAN,
            offset_left: 1e9,
            offset_top: f32::NEG_INFINITY,
            subdivisions: 0,
            grid_line_width: 100.0,
            guide_line_width: f32::NAN,
            ..Default::default()
        };
        let s = bad.sanitized();
        assert_eq!((s.width, s.height), (MIN_GRID_SPACING, GridSettings::default().height));
        assert_eq!((s.offset_left, s.offset_top), (MAX_GRID_SPACING, 0.0));
        assert_eq!(s.subdivisions, 1);
        assert_eq!((s.grid_line_width, s.guide_line_width), (crate::guide_sets::MAX_WIDTH, 1.0));
        let parsed: GridSettings = serde_json::from_str(r#"{"width": 36}"#).unwrap();
        assert_eq!((parsed.width, parsed.height), (36.0, 72.0), "missing fields keep their defaults");
    }

    #[test]
    fn hit_strips_cover_each_visible_guide_and_every_page_guides() {
        let p = page(0);
        let size = [200.0, 100.0];
        let mut gs = Guides::default();
        gs.add(3, Guide::new(GuideAxis::Horizontal, 30.0), size);
        gs.add(4, Guide::new(GuideAxis::Vertical, 25.0), size);
        gs.add_all(Guide::new(GuideAxis::Vertical, 25.0));
        let strips = hit_strips(&gs, &[p], Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0)));
        assert_eq!(strips.iter().map(|s| s.1).collect::<Vec<_>>(), [GuideRef::All(0), GuideRef::Page(3, 0)], "page 4 is not visible");
        assert!(strips[1].2.contains(pos2(200.0, 80.0 + HIT - 0.5)));
        assert!(!strips[1].2.contains(pos2(200.0, 80.0 + HIT + 1.0)));
        let json = pages_json(&gs, &[p], crate::units::Unit::Points);
        assert_eq!(json["count"], 3);
        assert_eq!(json["on_screen"].as_array().unwrap().len(), 2);
        assert_eq!(json["on_screen"][1]["screen"], serde_json::json!([[10.0, 80.0], [410.0, 80.0]]));
    }

    #[test]
    fn deleting_another_guide_keeps_the_properties_window_on_its_guide() {
        let size = [200.0, 100.0];
        let mut gs = Guides::default();
        for at in [10.0, 20.0, 30.0] {
            gs.add(0, Guide::new(GuideAxis::Horizontal, at), size);
        }
        gs.add_all(Guide::new(GuideAxis::Vertical, 5.0));
        gs.add_all(Guide::new(GuideAxis::Vertical, 15.0));
        let mut props = Some((GuideRef::Page(0, 2), 0));
        assert!(apply(&mut gs, &mut props, GuideRef::Page(0, 1), 0, size, GuideChange::Delete));
        assert_eq!(props, Some((GuideRef::Page(0, 1), 0)));
        assert_eq!(gs.get(props.unwrap().0).unwrap().at, 30.0, "still the guide at 30");
        // Moving the first page guide to every page shifts it again; a guide on every page is not.
        let mut on_all = Some((GuideRef::All(1), 0));
        assert!(apply(&mut gs, &mut props, GuideRef::Page(0, 0), 0, size, GuideChange::EveryPage));
        assert_eq!(props, Some((GuideRef::Page(0, 0), 0)));
        assert!(apply(&mut gs, &mut on_all, GuideRef::All(0), 0, size, GuideChange::Delete));
        assert_eq!(on_all, Some((GuideRef::All(0), 0)));
        assert!(apply(&mut gs, &mut on_all, GuideRef::All(0), 0, size, GuideChange::Delete));
        assert_eq!(on_all, None, "its own guide deleted closes it");
        assert_eq!(GuideRef::Page(1, 3).after_removing(GuideRef::Page(0, 0)), Some(GuideRef::Page(1, 3)), "another page's list");
    }

    #[test]
    fn pages_off_screen_get_no_guide_strips() {
        let mut gs = Guides::default();
        gs.add_all(Guide::new(GuideAxis::Vertical, 25.0));
        let near = page(0);
        let far = VisiblePageGeometry { page_index: 9, page_rect: near.page_rect.translate(vec2(0.0, 5_000.0)), ..near };
        let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0));
        assert_eq!(on_screen(&[near, far], viewport), [near]);
        assert_eq!(hit_strips(&gs, &[near, far], viewport).len(), 1);
    }
}
