//! Measure objects: page-space clicks, live readings and persistent drawing scales.
use crate::{LeftPanel, PdfCraftApp, QuickTool, comments::PageMap};
use craft::behavior::{CancelReason, Cursor, CursorIcon, DisplayOrder, ItemSource, Selection, geometry::turn_to_45, kurbo};
use craft::host::{
    Host, PickRequest, Picked, Wake, WriteIntent,
    queue::{JobQueue, Priority, QueueOptions},
};
use craft::theme::variant::Tone;
use craft::ui::{
    buttons::Button,
    canvas::{CanvasTool, ToolAction, ToolClaims, ToolContext, ToolKey, ToolPath, ToolPointer},
    choice::{Checkbox, Segmented},
    contract::{Bind, CollectionEvent, Entry, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _},
    form::{Field, SliderField},
    list::{List, ListRow},
    notices::Notice,
    pickers::{Select, UnitSelect},
    primitives::{Draw, PaintShape, StrokeWidth, TextRun, glyph},
    runtime::env::UnitKind,
    text::{NumberField, TextField},
};
use pdfcraft_engine::{
    Document, Edit, Style,
    measure::{self, Kind, NewMeasurement, PageUnit, Point, Reading, Scale},
};
use pdfcraft_render::DocInfo; // PLAN: assumed `pdfcraft_render::DocInfo` as the type of `Document::info` (pdf-canvas-w9 passes `cx.info`)
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Distance,
    Perimeter,
    Area,
    Calibrate,
}
impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Distance => "distance",
            Self::Perimeter => "perimeter",
            Self::Area => "area",
            Self::Calibrate => "calibrate",
        }
    }
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "distance" => Some(Self::Distance),
            "perimeter" => Some(Self::Perimeter),
            "area" => Some(Self::Area),
            "calibrate" => Some(Self::Calibrate),
            _ => None,
        }
    }
    fn kind(self) -> Kind {
        match self {
            Self::Area => Kind::Area,
            Self::Perimeter => Kind::Perimeter,
            _ => Kind::Distance,
        }
    }
}
/// The snapping geometry of one page read by a queued job: (page, edit generation, geometry or why not).
type PathsRead = (usize, u64, Result<measure::snap::Geometry, String>);

pub struct MeasureView {
    pub points: Vec<Point>,
    pub page: Option<usize>,
    pub tool: Option<Tool>,
    pub reading: Option<Reading>,
    pub scale: Option<Scale>,
    pub error: Option<String>,
    pub snaps: measure::snap::SnapOptions,
    pub snap_enabled: bool,
    pub sensitivity: f64,
    /// The page side of the drawing scale, in its page unit ([`page_unit_of`]).
    pub drawing_points: f64,
    pub real_distance: f64,
    pub unit: String,
    /// The unit picked for the page side of the scale; `None` follows `unit`.
    pub page_unit: Option<PageUnit>,
    /// The "1:N" field of the drawing scale, as typed.
    pub ratio_text: String,
    /// The "1:N" field was edited and not yet applied.
    ratio_dirty: bool,
    pub precision: u8,
    pub name: String,
    pub whole_page: bool,
    pub rect: [f64; 4],
    pub label: String,
    seeded_page: Option<usize>,
    /// Snapping geometry (or why it failed) per (page, edit generation): extraction is not
    /// repeated on every hover frame.
    paths: Option<PathsRead>,
    /// The queue that reads snapping geometry away from the window: a heavy drawing takes
    /// seconds, which would freeze it. The tool builds it from the host on first use.
    reads: Option<JobQueue<(), PathsRead>>,
    /// The (page, edit generation) whose snapping geometry `reads` is reading.
    pending_read: Option<(usize, u64)>,
    /// Saved measurements per edit generation, for the panel.
    listing: Option<(u64, Result<measure::Listing, String>)>,
}
impl Default for MeasureView {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            page: None,
            tool: None,
            reading: None,
            scale: None,
            error: None,
            snaps: Default::default(),
            snap_enabled: true,
            sensitivity: 6.0,
            drawing_points: 1.0,
            real_distance: 1.0,
            unit: "in".into(),
            page_unit: None,
            ratio_text: String::new(),
            ratio_dirty: false,
            precision: 2,
            name: "Drawing scale".into(),
            whole_page: true,
            rect: [0.0, 0.0, 100.0, 100.0],
            label: String::new(),
            seeded_page: None,
            paths: None,
            reads: None,
            pending_read: None,
            listing: None,
        }
    }
}
/// The unit of the page side of a drawing scale: the one picked, else the real-world unit when it
/// is pt, in, mm or cm (a plan at 1:50 in millimetres is 1 mm = 50 mm), else the display unit.
pub(crate) fn page_unit_of(picked: Option<PageUnit>, unit: &str, display: crate::units::Unit) -> PageUnit {
    picked.or_else(|| PageUnit::from_label(unit)).or_else(|| PageUnit::from_label(display.label())).unwrap_or(PageUnit::Pt)
}
/// Drawing scales an architect's rule offers, as the N of 1:N.
pub(crate) const RATIOS: [f64; 10] = [1.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 500.0, 1000.0];
/// "1:200", "1/200", "1 : 200" or a bare "200" (a comma works as the decimal mark), as the N of 1:N.
/// `None` for anything else, for zero, negative or absurd values.
pub(crate) fn parse_ratio(text: &str) -> Option<f64> {
    let text = text.trim().replace(',', ".");
    let n = match text.split_once([':', '/']) {
        Some((one, n)) => n.trim().parse::<f64>().ok()? / one.trim().parse::<f64>().ok()?,
        None => text.parse::<f64>().ok()?,
    };
    RATIO_RANGE.contains(&n).then_some(n)
}
/// The N of 1:N this panel accepts. The lower bound keeps the real distance (N / 1000 m) within the
/// minimum of its field.
const RATIO_RANGE: std::ops::RangeInclusive<f64> = 1e-3..=1e9;
/// "1:200", or "1:2.5" for a ratio that isn't a whole number: the number as it is, so the field shows exactly what applies.
pub(crate) fn format_ratio(n: f64) -> String {
    format!("1:{n}")
}
/// The N of 1:N the scale fields stand for, when they are a plain drawing scale (real distance in m or mm).
fn ratio_of_fields(state: &MeasureView, display: crate::units::Unit) -> Option<f64> {
    let real_mm = match state.unit.trim() {
        "m" => state.real_distance * 1000.0,
        "mm" => state.real_distance,
        _ => return None,
    };
    let paper_mm = state.drawing_points * page_unit_of(state.page_unit, &state.unit, display).points() * 25.4 / 72.0;
    let n = real_mm / paper_mm;
    // Float noise from the unit conversions is rounded away; anything that isn't a clean ratio isn't one.
    let clean = (n * 1e6).round() / 1e6;
    (RATIO_RANGE.contains(&clean) && (clean - n).abs() <= 1e-9 * n).then_some(clean)
}
/// Make the "1:N" field and the common scale picked in its list say what the scale fields now hold: the ratio, or nothing.
fn sync_ratio_text(state: &mut MeasureView, display: crate::units::Unit) {
    state.ratio_text = ratio_of_fields(state, display).map(format_ratio).unwrap_or_default();
    state.ratio_dirty = false;
}
/// What the scale fields hold, to notice a hand edit.
fn scale_fields(state: &MeasureView) -> (f64, f64, String, Option<PageUnit>) {
    (state.drawing_points, state.real_distance, state.unit.clone(), state.page_unit)
}
/// Fill the scale fields for 1:N: 1 mm on the page is N mm, shown in metres. Returns `false`, changing nothing, when `n` is not a usable ratio.
pub(crate) fn set_ratio(state: &mut MeasureView, n: f64) -> bool {
    if !RATIO_RANGE.contains(&n) {
        return false;
    }
    state.unit = "m".into();
    state.page_unit = Some(PageUnit::Mm);
    state.drawing_points = 1.0;
    state.real_distance = n / 1000.0;
    state.ratio_text = format_ratio(n);
    true
}
impl MeasureView {
    pub fn cancel(&mut self) {
        self.points.clear();
        self.page = None;
        self.reading = None;
        self.scale = None;
    }
}
/// Snaps `point` to the page's drawn geometry (endpoints, midpoints, intersections, paths),
/// recording in `state` why it couldn't when the content is too complex or unreadable.
fn snap_to_content(
    state: &mut MeasureView,
    doc: &Document,
    page: usize,
    view_xy: [f32; 2],
    pixels_per_unit: f32,
    point: Point,
) -> Option<(Point, measure::snap::SnapKind)> {
    let reading = tl!("Reading the drawing for snapping…");
    if !load_paths(state, doc, page) {
        state.error = Some(reading.into());
        return None;
    }
    if state.error.as_deref() == Some(reading) {
        state.error = None;
    }
    if let Some((_, _, Err(e))) = &state.paths {
        state.error = Some(e.clone());
    }
    if let Some((_, _, Ok(paths))) = &state.paths {
        if paths.truncated {
            state.error = Some(tl!("This drawing exceeds the snapping limit; only the first paths are used.").into());
        } else if paths.unreadable > 0 {
            state.error = Some(tl!("Some page content couldn't be read; snapping uses the rest.").into());
        }
        let tolerance = doc
            .measurement_to_user(page, [f64::from(view_xy[0]) + state.sensitivity / f64::from(pixels_per_unit), f64::from(view_xy[1])])
            .map(|p| measure::distance(point, p))
            .unwrap_or(state.sensitivity / f64::from(pixels_per_unit));
        if state.snaps.intersections && paths.intersection_limited(point, tolerance) {
            state.error = Some(tl!("This area has too many overlapping paths; intersection snapping is limited.").into());
        }
        if let Ok(Some(hit)) = paths.snap(point, tolerance, state.snaps) {
            return Some((hit.point, hit.kind));
        }
    }
    None
}

/// A file smaller than this may have its snapping geometry read on the spot.
const SNAP_SMALL_FILE: usize = 8 * 1024 * 1024;

/// Whether the snapping geometry of `page` as it is now is at hand, starting (on the queue) or
/// collecting the read when it isn't.
fn load_paths(state: &mut MeasureView, doc: &Document, page: usize) -> bool {
    let generation = doc.edit_generation();
    let current = |p: usize, g: u64| p == page && g == generation;
    if state.paths.as_ref().is_some_and(|(p, g, _)| current(*p, *g)) {
        return true;
    }
    // One read at a time, each being a whole page: one of another page or edit is let finish, and
    // its answer dropped.
    if state.pending_read.is_some() {
        let Some(done) = state.reads.as_ref().and_then(|queue| queue.try_recv()) else { return false };
        let (read_page, read_generation, answer) = done.result;
        state.pending_read = None;
        if current(read_page, read_generation) {
            state.paths = Some((read_page, read_generation, answer));
            return true;
        }
    }
    start_read(state, doc, page, generation)
}

/// What the window itself reads of `page`: a small file's page, if it is within what a frame can
/// afford. A small file can still hold millions of segments.
fn read_on_the_spot(doc: &Document, page: usize) -> Option<Result<measure::snap::Geometry, String>> {
    if doc.bytes.len() >= SNAP_SMALL_FILE {
        return None;
    }
    let read = doc.measurement_paths_within(page, measure::snap::Limits::WITHIN_A_FRAME);
    (!read.as_ref().is_ok_and(|g| g.truncated)).then_some(read)
}

/// Read `page`'s geometry, on the queue unless it is little. Whether it is at hand already.
fn start_read(state: &mut MeasureView, doc: &Document, page: usize, generation: u64) -> bool {
    if let Some(read) = read_on_the_spot(doc, page) {
        state.paths = Some((page, generation, read));
        return true;
    }
    let Some(queue) = state.reads.as_ref() else {
        state.paths = Some((page, generation, Err("the snapping read has no queue".into())));
        return true;
    };
    let job = doc.measurement_paths_job(page);
    queue.submit((), Priority::VISIBLE, move |_cancel| {
        // The queue delivers nothing for a panicked job, so the guard reports the stop.
        let read = pdfcraft_engine::guard(job).unwrap_or_else(|_| Err("the snapping read stopped unexpectedly".into()));
        (page, generation, read)
    });
    // Without lanes (the web) the owner runs the job.
    #[cfg(target_arch = "wasm32")]
    queue.run_inline(1);
    state.pending_read = Some((page, generation));
    false
}

/// Whether a click may place a point: with snapping on, not while the drawing is being read, as
/// the point would land unsnapped.
fn takes_clicks(state: &MeasureView) -> bool {
    !state.snap_enabled || state.pending_read.is_none()
}

/// The Measure tools on a page: a click places a vertex, Enter or a double click finishes a
/// perimeter or area, Shift turns the next vertex onto a 45-degree ray.
pub(crate) struct MeasureTool<'a> {
    view: &'a mut MeasureView,
    doc: &'a Document,
    pages: &'a PageMap<'a>,
    info: &'a DocInfo,
    tool: Tool,
    units: crate::units::Unit,
    author: String,
    /// The pointer's page, point (user space) and snap kind of the last hover or press.
    hover: Option<(usize, Point, Option<measure::snap::SnapKind>)>,
    edit: Option<Edit>,
}

impl<'a> MeasureTool<'a> {
    /// The tool for `tool` over `view`. A different tool than the last drops the measurement in
    /// progress. `host` and `wake` build the snapping read queue on first use.
    pub(crate) fn new(
        view: &'a mut MeasureView,
        doc: &'a Document,
        pages: &'a PageMap<'a>,
        info: &'a DocInfo,
        tool: Tool,
        units: crate::units::Unit,
        author: &str,
        host: &Host,
        wake: Wake,
    ) -> Self {
        if view.tool != Some(tool) {
            view.cancel();
            view.tool = Some(tool);
        }
        view.reads
            .get_or_insert_with(|| JobQueue::new(host.jobs_shared().clone(), wake, QueueOptions { name: "measure-snap", ..QueueOptions::default() }));
        Self { view, doc, pages, info, tool, units, author: author.into(), hover: None, edit: None }
    }

    /// The measurement the tool finished this pass, for the page body to apply.
    pub(crate) fn take_edit(&mut self) -> Option<Edit> {
        self.edit.take()
    }

    /// The page, user-space point and snap kind under `pos`, snapped to the drawing and turned
    /// onto a 45-degree ray from the last vertex while Shift is held.
    fn locate(&mut self, pos: egui::Pos2, cx: &ToolContext<'_>) -> Option<(usize, Point, Option<measure::snap::SnapKind>)> {
        let space = cx.space()?;
        let screen = kurbo::Point::new(f64::from(pos.x), f64::from(pos.y));
        let page = self.pages.page_at(screen)?;
        let displayed = self.pages.to_page(page, screen)?;
        let mut point = self.pages.to_user(page, displayed);
        let mut snap = None;
        if self.view.snap_enabled {
            let [vx, vy] = self.doc.measurement_to_view(page, point).ok()?;
            if let Some((snapped, kind)) =
                snap_to_content(self.view, self.doc, page, [vx as f32, vy as f32], (space.view.zoom as f32).max(1e-6), point)
            {
                point = snapped;
                snap = Some(kind);
            }
        }
        if cx.mods.shift
            && self.view.page == Some(page)
            && let Some(last) = self.view.points.last()
        {
            let turned = turn_to_45(kurbo::Point::new(last[0], last[1]), kurbo::Point::new(point[0], point[1]));
            point = [turned.x, turned.y];
        }
        Some((page, point, snap))
    }

    /// The vertices placed, and the pointer's point as the next one when `with_hover`.
    fn vertices(&self, with_hover: bool) -> Vec<Point> {
        let mut points = self.view.points.clone();
        if with_hover
            && let Some((page, p, _)) = self.hover
            && self.view.page == Some(page)
            && points.last().is_none_or(|last| measure::distance(*last, p) > 1e-6)
        {
            points.push(p);
        }
        points
    }

    /// The live reading of the vertices with the pointer's point as the next one.
    fn refresh_reading(&mut self) {
        if self.view.page.is_none() || self.view.points.is_empty() {
            return;
        }
        let points = self.vertices(true);
        if let Some(scale) = &self.view.scale {
            match measure::reading(self.tool.kind(), &points, scale) {
                Ok(reading) => self.view.reading = Some(reading),
                Err(e) => self.view.error = Some(e.to_string()),
            }
        }
    }

    fn finish(&mut self, page: usize) {
        finish(self.view, &mut self.edit, self.doc, page, self.tool, &self.author, self.units);
    }
}

impl CanvasTool for MeasureTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) -> ToolAction {
        if !self.doc.allows_annotation() {
            return ToolAction::Ignored;
        }
        let Some((page, point, snap)) = self.locate(pointer.pos, cx) else { return ToolAction::Done };
        self.hover = Some((page, point, snap));
        if !takes_clicks(self.view) {
            return ToolAction::Done;
        }
        if self.view.page != Some(page) {
            self.view.cancel();
            self.view.page = Some(page);
        }
        if self.view.points.is_empty() {
            match self.doc.measurement_scale_or(page, point, self.units.per_point(), self.units.label()) {
                Ok(scale) => {
                    self.view.scale = Some(scale);
                    self.view.error = None;
                }
                Err(e) => {
                    self.view.error = Some(e);
                    return ToolAction::Done;
                }
            }
        }
        // A press within the click distance of the first vertex closes an area.
        let closes = self.tool == Tool::Area
            && self.view.points.len() >= 3
            && self
                .view
                .points
                .first()
                .and_then(|first| self.pages.to_screen(page, self.pages.to_doc(page, *first)))
                .is_some_and(|first| first.distance(pointer.pos) <= cx.slop);
        let repeats = self.view.points.last().is_some_and(|last| measure::distance(*last, point) < 1e-6);
        if !closes && !repeats && self.view.points.len() < measure::MAX_POINTS {
            self.view.points.push(point);
        }
        let finishes = closes || matches!(self.tool, Tool::Distance | Tool::Calibrate) && self.view.points.len() == 2;
        if finishes || pointer.clicks == 2 && matches!(self.tool, Tool::Perimeter | Tool::Area) && !self.view.points.is_empty() {
            self.finish(page);
        }
        self.refresh_reading();
        ToolAction::Done
    }

    fn drag(&mut self, _pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {}

    fn release(&mut self, _pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {}

    fn hover(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        self.hover = self.locate(pointer.pos, cx);
        self.refresh_reading();
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {
        self.view.cancel();
        self.hover = None;
    }

    fn claims(&self) -> ToolClaims {
        // Backspace removes a vertex only while there is one; otherwise it stays the app's.
        if self.view.points.is_empty() { ToolClaims::NONE } else { ToolClaims::NONE.deletes() }
    }

    fn key(&mut self, key: &ToolKey, _cx: &mut ToolContext<'_>) -> bool {
        match key {
            ToolKey::Submit => match self.view.page {
                Some(page) if matches!(self.tool, Tool::Perimeter | Tool::Area) && !self.view.points.is_empty() => {
                    self.finish(page);
                    true
                }
                _ => false,
            },
            ToolKey::Delete { forward: false, .. } => {
                let popped = self.view.points.pop().is_some();
                self.refresh_reading();
                popped
            }
            ToolKey::Cancel => {
                self.view.cancel();
                self.hover = None;
                true
            }
            _ => false,
        }
    }

    fn paint(&self, draw: &Draw<'_>, cx: &ToolContext<'_>) {
        let (Some(style), Some(page)) = (cx.style(), self.view.page) else { return };
        if self.view.points.is_empty() {
            return;
        }
        let accent = style.color(style.tokens.color.accent.solid);
        let good = style.color(style.tokens.color.status.good.solid);
        let screen = |p: Point| self.pages.to_screen(page, self.pages.to_doc(page, p));
        let positions: Vec<egui::Pos2> = self.vertices(cx.pointer.is_some()).iter().filter_map(|&p| screen(p)).collect();
        draw.stroke(&PaintShape::Path(positions.clone()), StrokeWidth::Medium, accent);
        if self.tool == Tool::Area
            && let (Some(a), Some(b)) = (positions.first(), positions.last())
        {
            draw.stroke(&PaintShape::Path(vec![*b, *a]), StrokeWidth::Medium, accent);
        }
        let side = draw.width(StrokeWidth::Heavy);
        for &p in &positions {
            draw.fill(&glyph::dot(p, side), accent);
        }
        let Some((hover_page, hover_point, snap)) = self.hover.filter(|(p, ..)| *p == page) else { return };
        let _ = hover_page;
        let Some(at) = screen(hover_point) else { return };
        let label = |text: String, at: egui::Pos2, color| {
            let rect = egui::Rect::from_min_size(at, egui::vec2((cx.rect.right() - at.x).max(0.0), cx.slop * 2.0));
            draw.text(rect, &TextRun::new(text).color(color));
        };
        if let Some(kind) = snap {
            draw.stroke(&PaintShape::Circle(at, cx.slop), StrokeWidth::Thin, good);
            let name = tl!(match kind {
                measure::snap::SnapKind::Endpoint => "Endpoint",
                measure::snap::SnapKind::Midpoint => "Midpoint",
                measure::snap::SnapKind::Intersection => "Intersection",
                measure::snap::SnapKind::Path => "Path",
            });
            label(name.to_string(), at + egui::vec2(cx.slop, -cx.slop * 3.0), good);
        }
        if let Some(reading) = &self.view.reading {
            label(reading.label.clone(), at + egui::vec2(cx.slop, cx.slop), accent);
        }
    }

    fn cursor(&self, _cx: &ToolContext<'_>) -> Cursor {
        Cursor::System(CursorIcon::Crosshair)
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Measure panel's Apply scale and Export commands")
    }
}

fn finish(state: &mut MeasureView, pending: &mut Option<Edit>, doc: &Document, page: usize, tool: Tool, author: &str, units: crate::units::Unit) {
    if tool == Tool::Calibrate {
        if let [a, b] = state.points.as_slice() {
            let a = doc.measurement_to_view(page, *a);
            let b = doc.measurement_to_view(page, *b);
            if let (Ok(a), Ok(b)) = (a, b) {
                state.drawing_points = measure::distance(a, b) / page_unit_of(state.page_unit, &state.unit, units).points();
            }
        }
        state.cancel();
        return;
    }
    let minimum = if tool == Tool::Area { 3 } else { 2 };
    if state.points.len() < minimum {
        state.error = Some(crate::i18n::fmt(tl!("This measurement needs at least {n} points."), &[("n", &minimum.to_string())]));
        return;
    }
    if let Some(scale) = state.scale.clone() {
        *pending = Some(Edit::AddMeasurement(NewMeasurement {
            page,
            kind: tool.kind(),
            points: state.points.clone(),
            scale,
            style: Style { color: [0.0, 0.47, 0.84], ..Style::default() },
            label: state.label.clone(),
            author: author.into(),
        }));
        state.cancel();
    }
}

/// The unit select's entry for a page unit.
fn unit_kind(unit: PageUnit) -> UnitKind {
    match unit {
        PageUnit::Pt => UnitKind::Points,
        PageUnit::In => UnitKind::Inches,
        PageUnit::Mm => UnitKind::Millimetres,
        PageUnit::Cm => UnitKind::Centimetres,
    }
}

/// The page unit a unit select entry stands for.
fn page_unit_of_kind(kind: UnitKind) -> Option<PageUnit> {
    PageUnit::ALL.into_iter().find(|u| unit_kind(*u) == kind)
}

/// Draw the measurement panel: tool selection, scale calibration, snapping and the current reading.
pub(crate) fn panel(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let Some((index, id)) = app.active_ids() else {
        Notice::new(Tone::Info, tl!("Open a PDF to measure it.")).show(ui);
        return;
    };
    let current = match app.quick_tool {
        QuickTool::Measure(t) => Some(t),
        _ => None,
    };
    let mut tool = None;
    let tools = [Entry::new(Tool::Distance, tl!("Distance")), Entry::new(Tool::Perimeter, tl!("Perimeter")), Entry::new(Tool::Area, tl!("Area"))];
    let picked = Segmented::new(tl!("Measure"), Bind::given(current.unwrap_or(Tool::Distance)), &tools).show(ui);
    if let Some(t) = picked.value().copied()
        && Some(t) != current
    {
        tool = Some(t);
    }
    let Some(doc) = app.session.get(id) else { return };
    let units = app.units;
    let page = app.views[index].current();
    let info = doc.info.pages.get(page).cloned();
    let state = &mut app.views[index].session.measure;
    if state.listing.as_ref().is_none_or(|(g, _)| *g != doc.edit_generation()) {
        state.listing = Some((doc.edit_generation(), doc.measurements()));
    }
    if state.seeded_page != Some(page) {
        state.seeded_page = Some(page);
        // One page unit (the one the scale's unit gives) and what it measures.
        if let Ok(a) = doc.measurement_to_user(page, [0.0, 0.0])
            && let Ok(scale) = doc.measurement_scale_or(page, a, units.per_point(), units.label())
            && let Ok(b) = doc.measurement_to_user(page, [page_unit_of(None, &scale.unit, units).points(), 0.0])
            && let Ok(reading) = measure::reading(Kind::Distance, &[a, b], &scale)
        {
            state.drawing_points = 1.0;
            state.real_distance = reading.value;
            state.unit = scale.unit;
            state.page_unit = None;
            state.precision = scale.precision;
        }
        sync_ratio_text(state, units);
    }
    Notice::new(Tone::Neutral, tl!("Click vertices. Enter or double-click finishes a perimeter or area. Click the first point to close an area."))
        .lines([tl!("Shift constrains to 45 degrees. Backspace removes a vertex. Escape cancels.")])
        .show(ui);
    if let Some(reading) = &state.reading {
        let mut lines = vec![format!("X: {:.3}   Y: {:.3}", reading.delta_x, reading.delta_y), format!("{}: {:.2}°", tl!("Angle"), reading.angle)];
        if let Some(scale) = &state.scale {
            lines.push(scale.ratio.clone());
        }
        Notice::new(Tone::Neutral, reading.label.clone()).lines(lines).show(ui);
    } else {
        Notice::new(Tone::Neutral, tl!("Click the first point to begin.")).show(ui);
    }
    Field::new(tl!("Label")).id_salt("measure-label").show(ui, |ui, _| {
        TextField::new(tl!("Label"), Bind::bound(&mut state.label)).show(ui);
    });
    Checkbox::new(Bind::bound(&mut state.snap_enabled)).label(tl!("Snap to drawing")).show(ui);
    if state.snap_enabled {
        Checkbox::new(Bind::bound(&mut state.snaps.endpoints)).label(tl!("Endpoints")).show(ui);
        Checkbox::new(Bind::bound(&mut state.snaps.midpoints)).label(tl!("Midpoints")).show(ui);
        Checkbox::new(Bind::bound(&mut state.snaps.intersections)).label(tl!("Intersections")).show(ui);
        Checkbox::new(Bind::bound(&mut state.snaps.paths)).label(tl!("Paths")).show(ui);
        SliderField::new(tl!("Sensitivity"), Bind::bound(&mut state.sensitivity), 1.0..=20.0).show(ui);
    }
    // "1:N" fills the fields below and applies at once, as does picking a common scale.
    let mut chosen = None;
    let fields = scale_fields(state);
    let before = state.ratio_text.clone();
    let ratio_text = &mut state.ratio_text;
    let typed = Field::new(tl!("Drawing scale"))
        .id_salt("measure-ratio")
        .show(ui, |ui, _| TextField::new(tl!("Scale"), Bind::bound(ratio_text)).submits().show(ui));
    state.ratio_dirty |= state.ratio_text != before;
    if typed.inner.as_ref().is_some_and(|t| t.submitted || t.response.lost_focus()) && std::mem::take(&mut state.ratio_dirty) {
        match parse_ratio(&state.ratio_text) {
            Some(n) => chosen = Some(n),
            None => state.error = Some(tl!("Enter a scale such as 1:50.").into()),
        }
    }
    let ratios: Vec<Entry<String>> = RATIOS.iter().map(|&n| Entry::new(format_ratio(n), format_ratio(n))).collect();
    let common = Field::new(tl!("Common scales"))
        .id_salt("measure-common")
        .show(ui, |ui, _| Select::new(tl!("Common scales"), Bind::given(state.ratio_text.clone()), &ratios).show(ui));
    if let Some(text) = common.inner.as_ref().and_then(|c| c.value())
        && let Some(n) = parse_ratio(text)
        && *text != state.ratio_text
    {
        chosen = Some(n);
    }
    let page_unit = page_unit_of(state.page_unit, &state.unit, units);
    let unit_entries: Vec<Entry<UnitKind>> = PageUnit::ALL.iter().map(|&u| Entry::new(unit_kind(u), u.label())).collect();
    let mut drawing = Some(state.drawing_points);
    let mut picked_unit = unit_kind(page_unit);
    Field::new(tl!("Page length")).id_salt("measure-page-length").show(ui, |ui, _| {
        UnitSelect::new(tl!("Page length"), Bind::bound(&mut drawing), Bind::bound(&mut picked_unit), &unit_entries).range(0.000001..=1e9).show(ui);
    });
    state.drawing_points = drawing.unwrap_or(state.drawing_points);
    if picked_unit != unit_kind(page_unit) {
        state.page_unit = page_unit_of_kind(picked_unit);
    }
    let mut real = Some(state.real_distance);
    Field::new(tl!("Real distance")).id_salt("measure-real").show(ui, |ui, _| {
        NumberField::new(tl!("Real distance"), Bind::bound(&mut real)).range(0.000001..=1e9).show(ui);
    });
    state.real_distance = real.unwrap_or(state.real_distance);
    Field::new(tl!("Unit")).id_salt("measure-unit").show(ui, |ui, _| {
        TextField::new(tl!("Unit"), Bind::bound(&mut state.unit)).show(ui);
    });
    let mut precision = Some(f64::from(state.precision));
    Field::new(tl!("Decimals")).id_salt("measure-decimals").show(ui, |ui, _| {
        NumberField::new(tl!("Decimals"), Bind::bound(&mut precision)).range(0.0..=6.0).step(1.0).show(ui);
    });
    state.precision = precision.map_or(state.precision, |p| p.round().clamp(0.0, 6.0) as u8);
    // A hand edit of the fields leaves the 1:N field showing the old ratio: say the new one.
    if scale_fields(state) != fields {
        sync_ratio_text(state, units);
    }
    if Button::new(tl!("Calibrate from two points")).show(ui).clicked() {
        tool = Some(Tool::Calibrate);
    }
    Checkbox::new(Bind::bound(&mut state.whole_page)).label(tl!("Entire page")).show(ui);
    if !state.whole_page {
        Notice::new(Tone::Neutral, tl!("Viewport rectangle in page points (x, y, width, height)")).show(ui);
        for (i, name) in [tl!("X"), tl!("Y"), tl!("Width"), tl!("Height")].into_iter().enumerate() {
            if let Some(v) = state.rect.get_mut(i) {
                let mut value = Some(*v);
                NumberField::new(name, Bind::bound(&mut value)).range(0.0..=1e9).show(ui);
                *v = value.unwrap_or(*v);
            }
        }
    }
    Field::new(tl!("Viewport")).id_salt("measure-viewport").show(ui, |ui, _| {
        TextField::new(tl!("Viewport"), Bind::bound(&mut state.name)).show(ui);
    });
    let apply = Button::new(tl!("Apply scale")).enabled(doc.allows_annotation()).show(ui).clicked();
    if let Some(error) = &state.error {
        Notice::new(Tone::Bad, error.clone()).show(ui);
    }
    let mut select = None;
    match state.listing.as_ref().map(|(_, l)| l) {
        None => {}
        Some(Ok(listing)) => {
            Notice::new(Tone::Neutral, crate::i18n::fmt(tl!("Saved measurements: {n}"), &[("n", &listing.measurements.len().to_string())])).show(ui);
            if !listing.unsupported.is_empty() {
                let reasons: Vec<String> = listing
                    .unsupported
                    .iter()
                    .map(|u| {
                        crate::i18n::fmt(tl!("Page {page}: {reason}"), &[("page", &u.page.saturating_add(1).to_string()), ("reason", &u.reason)])
                    })
                    .collect();
                Notice::new(Tone::Warn, crate::i18n::fmt(tl!("Unsupported measurements: {n}"), &[("n", &listing.unsupported.len().to_string())]))
                    .lines(reasons)
                    .show(ui);
            }
            if !listing.measurements.is_empty() {
                let source = MeasurementRows(listing.measurements.len());
                let mut selection = Selection::default();
                let out = List::new(tl!("Saved measurements")).select(SelectMode::Single).selection(Bind::bound(&mut selection)).show_items(
                    ui,
                    &source,
                    &mut |&at, icx| {
                        let text = listing
                            .measurements
                            .get(at)
                            .map(|m| {
                                crate::i18n::fmt(
                                    tl!("Page {page}: {reading} {label}"),
                                    &[("page", &m.page.saturating_add(1).to_string()), ("reading", &m.reading.label), ("label", &m.label)],
                                )
                            })
                            .unwrap_or_default();
                        ListRow::new(text).striped(icx.at % 2 == 1)
                    },
                );
                // Only the pass's own pick selects; a drained event list is empty on a second pass.
                if out.events.iter().any(|e| !matches!(e, CollectionEvent::Column(_)))
                    && let Some(m) = selection.lead.and_then(|at| listing.measurements.get(at))
                {
                    select = Some((m.page, m.index, m.reading.clone(), m.scale.clone()));
                }
            }
        }
        Some(Err(e)) => {
            Notice::new(Tone::Bad, e.clone()).show(ui);
        }
    }
    let export = Button::new(tl!("Export measurements as CSV")).show(ui).clicked();
    if let Some(n) = chosen
        && set_ratio(state, n)
    {
        state.error = None;
    }
    // Apply scale, or a 1:N typed or picked: the fields are applied to the document at once.
    if (apply || chosen.is_some())
        && doc.allows_annotation()
        && let Some(info) = info
    {
        let result =
            doc.measurement_to_user(page, [0.0, 0.0]).and_then(|a| doc.measurement_to_user(page, [1.0, 0.0]).map(|b| (a, b))).and_then(|(a, b)| {
                let page_unit = page_unit_of(state.page_unit, &state.unit, units);
                Scale::ratio(a, b, state.drawing_points, page_unit, state.real_distance, &state.unit, state.precision).map_err(|e| e.to_string())
            });
        match result {
            Ok(scale) => {
                let [x, y, w, h] = if state.whole_page { [0.0, 0.0, f64::from(info.width), f64::from(info.height)] } else { state.rect };
                let a = info.view_to_user(x as f32, y as f32);
                let b = info.view_to_user((x + w) as f32, (y + h) as f32);
                app.views[index].session.pending_edit = Some(Edit::SetMeasurementScale {
                    page,
                    bbox: [f64::from(a[0].min(b[0])), f64::from(a[1].min(b[1])), f64::from(a[0].max(b[0])), f64::from(a[1].max(b[1]))],
                    name: state.name.clone(),
                    scale,
                });
            }
            Err(e) => state.error = Some(e.to_string()),
        }
    }
    if let Some((page, annotation, reading, scale)) = select {
        app.views[index].go_to_page(page);
        app.views[index].session.comments.select_only((page, annotation));
        app.views[index].session.measure.reading = Some(reading);
        app.views[index].session.measure.scale = Some(scale);
        app.quick_tool = QuickTool::Select;
    }
    if let Some(tool) = tool {
        app.views[index].session.measure.cancel();
        app.quick_tool = QuickTool::Measure(tool);
    }
    if export {
        export_csv(app);
    }
}
/// The saved measurements as the list's item source, keyed by position.
struct MeasurementRows(usize);

impl DisplayOrder<usize> for MeasurementRows {
    fn len(&self) -> usize {
        self.0
    }

    fn key(&self, at: usize) -> usize {
        at
    }
}

impl ItemSource<usize> for MeasurementRows {}

pub(crate) fn command(app: &mut PdfCraftApp, id: &str) {
    app.left = LeftPanel::Tool("measure");
    app.set_left_open(true);
    if let Some((index, _)) = app.active_ids()
        && let Some(tool) = id.strip_prefix("measure.").and_then(Tool::from_name)
    {
        app.views[index].session.measure.cancel();
        app.quick_tool = QuickTool::Measure(tool);
    }
    if id == "measure.export" {
        export_csv(app);
    }
}
fn export_csv(app: &mut PdfCraftApp) {
    let Some((_, id)) = app.active_ids() else { return };
    let result = app.session.get(id).ok_or_else(|| "no such document".to_string()).and_then(|d| d.measurements());
    match result {
        Err(e) => app.notify(&e),
        Ok(listing) => {
            let csv = measure::csv(&listing.measurements);
            if !listing.unsupported.is_empty() {
                app.notify(crate::i18n::fmt(
                    tl!("{n} measurements use an unsupported format and are left out."),
                    &[("n", &listing.unsupported.len().to_string())],
                ));
            }
            let request = PickRequest::save("measurements").file_name("measurements.csv").filter("CSV", &["csv"]);
            app.pick_file(request, None, move |app: &mut PdfCraftApp, _: &egui::Context, picked: Picked| {
                let Some(path) = picked.first_path() else { return };
                // A browser answers with a name, not a path to write: the files service hands the bytes over as a download.
                let written = match picked {
                    Picked::Paths(_) => {
                        craft::atomic::atomic_write(&path, csv.as_bytes()).map(|()| tl!("Saved measurements.")).map_err(|e| e.to_string())
                    }
                    _ => app
                        .host
                        .files()
                        .write(&path, csv.as_bytes(), WriteIntent::SaveAs)
                        .map(|_| tl!("Downloaded measurements."))
                        .map_err(|e| e.to_string()),
                };
                match written {
                    Ok(done) => app.notify(done),
                    Err(e) => app.notify(e),
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft::host::{Jobs, ManualJobs};

    /// A snapping read queue that runs a job only when the test says so.
    fn manual_reads() -> JobQueue<(), PathsRead> {
        let jobs: Arc<dyn Jobs> = Arc::new(ManualJobs::new());
        JobQueue::new(jobs, Wake::none(), QueueOptions { lanes: 0, name: "measure-snap", ..QueueOptions::default() })
    }

    /// What Apply scale builds from the panel's fields, then a distance read with it.
    fn measured(state: &MeasureView, from: Point, to: Point) -> f64 {
        let page_unit = page_unit_of(state.page_unit, &state.unit, crate::units::Unit::Points);
        let scale = Scale::ratio([0.0, 0.0], [1.0, 0.0], state.drawing_points, page_unit, state.real_distance, &state.unit, 4).expect("scale");
        measure::reading(Kind::Distance, &[from, to], &scale).expect("reading").value
    }

    /// A one-page PDF of `lines` short lines, written out uncompressed.
    fn lined_pdf(lines: usize) -> Vec<u8> {
        padded_lined_pdf(lines, 0)
    }

    /// [`lined_pdf`], with `padding` more bytes in the file that aren't on the page.
    fn padded_lined_pdf(lines: usize, padding: usize) -> Vec<u8> {
        let content = "10 10 m 200 200 l S\n".repeat(lines);
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut pdf = b"%PDF-1.7\n".to_vec();
        pdf.extend_from_slice(format!("%{}\n", "x".repeat(padding)).as_bytes());
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        pdf
    }

    /// A big file's geometry is read on the queue, which the window doesn't wait for; a small
    /// file's is there at once.
    #[test]
    fn only_a_small_file_is_read_for_snapping_on_the_window_thread() {
        let mut session = pdfcraft_engine::Session::new();

        let small = session.open_new("small.pdf", Arc::new(lined_pdf(10))).unwrap();
        let mut state = MeasureView { reads: Some(manual_reads()), ..MeasureView::default() };
        assert!(load_paths(&mut state, session.get(small).unwrap(), 0), "there at once");
        assert!(state.pending_read.is_none());

        // However little is on its page.
        let big = padded_lined_pdf(10, SNAP_SMALL_FILE);
        assert!(big.len() >= SNAP_SMALL_FILE);
        let big = session.open_new("big.pdf", Arc::new(big)).unwrap();
        let doc = session.get(big).unwrap();
        let mut state = MeasureView { reads: Some(manual_reads()), ..MeasureView::default() };
        assert!(!load_paths(&mut state, doc, 0), "started, and not waited for");
        assert_eq!(state.pending_read.map(|(p, _)| p), Some(0));
        assert_eq!(state.reads.as_ref().map(|q| q.run_inline(1)), Some(1));
        assert!(load_paths(&mut state, doc, 0));
        assert!(state.pending_read.is_none());
        let geometry = state.paths.as_ref().and_then(|(_, _, g)| g.as_ref().ok()).expect("read");
        assert!(geometry.segment_count() > 0);
    }

    /// A small file can hold more than a frame can read: it goes to the queue, and comes whole.
    #[test]
    fn a_small_file_past_a_frames_limits_is_read_on_a_thread() {
        let lines = measure::snap::Limits::WITHIN_A_FRAME.segments + 1;
        let pdf = lined_pdf(lines);
        assert!(pdf.len() < SNAP_SMALL_FILE);
        let mut session = pdfcraft_engine::Session::new();
        let id = session.open_new("small.pdf", Arc::new(pdf)).unwrap();
        let doc = session.get(id).unwrap();
        let mut state = MeasureView { reads: Some(manual_reads()), ..MeasureView::default() };
        assert!(!load_paths(&mut state, doc, 0), "not read on the window's thread");
        assert_eq!(state.reads.as_ref().map(|q| q.run_inline(1)), Some(1));
        assert!(load_paths(&mut state, doc, 0));
        let geometry = state.paths.as_ref().and_then(|(_, _, g)| g.as_ref().ok()).expect("read");
        assert_eq!(geometry.segment_count(), lines);
        assert!(!geometry.truncated);
    }

    /// A read of another page is let finish before the next starts, and only then is it dropped.
    #[test]
    fn only_one_snapping_read_runs_at_a_time() {
        let mut session = pdfcraft_engine::Session::new();
        let id = session.open_new("big.pdf", Arc::new(lined_pdf(SNAP_SMALL_FILE / 20 + 1))).unwrap();
        let doc = session.get(id).unwrap();
        let reads = manual_reads();
        reads.submit((), Priority::VISIBLE, |_| (9, 9, Ok(measure::snap::Geometry::default())));
        let mut state = MeasureView { reads: Some(reads), pending_read: Some((9, 9)), ..MeasureView::default() };
        assert!(!load_paths(&mut state, doc, 0));
        assert_eq!(state.pending_read.map(|(p, _)| p), Some(9), "waits for it");
        assert_eq!(state.reads.as_ref().map(|q| q.run_inline(1)), Some(1));
        assert!(!load_paths(&mut state, doc, 0));
        assert_eq!(state.pending_read.map(|(p, _)| p), Some(0), "then reads this page");
        assert!(state.paths.is_none(), "the other page's answer is dropped");
    }

    #[test]
    fn the_reading_message_goes_when_the_drawing_is_read() {
        let mut session = pdfcraft_engine::Session::new();
        let id = session.open_new("small.pdf", Arc::new(lined_pdf(10))).unwrap();
        let doc = session.get(id).unwrap();
        let mut state = MeasureView { error: Some(tl!("Reading the drawing for snapping…").into()), ..MeasureView::default() };
        let _ = snap_to_content(&mut state, doc, 0, [5.0, 5.0], 1.0, [5.0, 5.0]);
        assert_eq!(state.error, None);
    }

    #[test]
    fn clicks_wait_for_the_drawing_only_with_snapping_on() {
        let mut state = MeasureView { pending_read: Some((0, 0)), ..MeasureView::default() };
        assert!(!takes_clicks(&state));
        state.snap_enabled = false;
        assert!(takes_clicks(&state));
        state.snap_enabled = true;
        state.pending_read = None;
        assert!(takes_clicks(&state));
    }

    #[test]
    fn typed_ratios_are_read_leniently() {
        assert_eq!(parse_ratio("1:200"), Some(200.0));
        assert_eq!(parse_ratio(" 1 : 50 "), Some(50.0));
        assert_eq!(parse_ratio("1/20"), Some(20.0));
        assert_eq!(parse_ratio("250"), Some(250.0));
        assert_eq!(parse_ratio("1:2,5"), Some(2.5));
        assert_eq!(parse_ratio("2:1"), Some(0.5));
        assert_eq!(parse_ratio("1:0.001"), Some(1e-3));
    }

    #[test]
    fn invalid_ratios_are_refused() {
        for bad in [
            "", " ", ":", "1:", ":50", "1:0", "0", "-5", "1:-5", "0:5", "abc", "1:abc", "1:50:2", "1:inf", "NaN", "1e12", "1:1e12", "1:0.0001",
            "1:1e-6",
        ] {
            assert_eq!(parse_ratio(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn ratios_are_written_as_people_say_them() {
        // Nothing is rounded away: the field reads the ratio that applies.
        for n in RATIOS.into_iter().chain([2.5, 0.5, 1.234, 0.001, 333.333333]) {
            assert_eq!(parse_ratio(&format_ratio(n)), Some(n), "1:{n}");
        }
    }

    #[test]
    fn every_offered_ratio_reads_paper_length_times_n_in_metres_whatever_the_unit_was() {
        for unit in ["mm", "cm", "m", "km", "in", "ft"] {
            for n in RATIOS {
                let mut state = MeasureView { unit: unit.into(), ..Default::default() };
                assert!(set_ratio(&mut state, n));
                assert_eq!(state.unit, "m");
                // 72 pt is one inch of paper: 25.4 mm x N, shown in metres.
                let v = measured(&state, [0.0, 0.0], [0.0, 72.0]);
                let expected = 25.4 * n / 1000.0;
                assert!((v - expected).abs() < 1e-9 * expected.max(1.0), "1:{n} in {unit} gave {v}, wanted {expected}");
            }
        }
    }

    #[test]
    fn the_smallest_ratio_keeps_the_real_distance_within_its_field() {
        let mut state = MeasureView::default();
        assert!(set_ratio(&mut state, parse_ratio("1:0.001").expect("ratio")));
        // The real-distance field accepts 0.000001 and no less.
        assert!(state.real_distance >= 0.000001, "{}", state.real_distance);
        assert!(!set_ratio(&mut state, 1e-4));
    }

    #[test]
    fn the_ratio_field_follows_the_scale_fields() {
        let display = crate::units::Unit::Points;
        let mut state = MeasureView::default();
        for n in [50.0, 2.5, 1.234, 0.001, 1e9] {
            assert!(set_ratio(&mut state, n));
            sync_ratio_text(&mut state, display);
            assert_eq!(state.ratio_text, format_ratio(n), "1:{n}");
        }
        // A hand edit that is still a drawing scale reads as its ratio, in another page unit too.
        assert!(set_ratio(&mut state, 50.0));
        state.page_unit = Some(PageUnit::Cm);
        sync_ratio_text(&mut state, display);
        assert_eq!(state.ratio_text, "1:5");
        state.page_unit = Some(PageUnit::Mm);
        state.real_distance = 0.1;
        sync_ratio_text(&mut state, display);
        assert_eq!(state.ratio_text, "1:100");
        // One that isn't (another unit, or no ratio at all) empties it.
        state.unit = "ft".into();
        sync_ratio_text(&mut state, display);
        assert_eq!(state.ratio_text, "");
        state.unit = "m".into();
        state.real_distance = 0.1 / 3.0;
        sync_ratio_text(&mut state, display);
        assert_eq!(state.ratio_text, "");
        state.drawing_points = 0.0;
        sync_ratio_text(&mut state, display);
        assert_eq!(state.ratio_text, "");
    }
}
