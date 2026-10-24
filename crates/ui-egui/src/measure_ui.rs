//! Measure objects: page-space clicks, live readings and persistent drawing scales.
use crate::{LeftPanel, PdfCraftApp, QuickTool, canvas::DocView, theme::Tokens, widgets};
use egui::{Color32, Pos2, Stroke};
use pdfcraft_engine::{
    Document, Edit, Style,
    measure::{self, Kind, NewMeasurement, PageUnit, Point, Reading, Scale},
};

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
    paths: Option<(usize, u64, Result<measure::snap::Geometry, String>)>,
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
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    doc: &Document,
    view: &mut DocView,
    cx: &crate::comments::PageCx<'_>,
    tool: Tool,
    units: crate::units::Unit,
) {
    let page = cx.page;
    let xf = cx.xf;
    let author = &cx.prefs.author;
    if !doc.allows_annotation() {
        return;
    }
    let Some(info) = doc.info.pages.get(page) else { return };
    let state = &mut view.measure;
    if state.tool != Some(tool) {
        state.cancel();
        state.tool = Some(tool);
    }
    // Keys belong to a focused text field (the label, unit or viewport name) when there is one.
    let typing = ui.ctx().egui_wants_keyboard_input();
    if !typing && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.cancel();
    }
    let screen = |p: Point| {
        let v = info.user_to_view(p[0] as f32, p[1] as f32);
        xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph)
    };
    let pos = cx.ptr.within(xf.rect);
    let mut hover = None;
    let mut snap = None;
    if let Some(pos) = pos {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        let (vx, vy) = xf.screen_to_view(pos);
        let [x, y] = info.view_to_user(vx, vy);
        let mut point = [f64::from(x), f64::from(y)];
        if state.snap_enabled {
            if state.paths.as_ref().is_none_or(|(p, g, _)| *p != page || *g != doc.edit_generation()) {
                state.paths = Some((page, doc.edit_generation(), doc.measurement_paths(page)));
            }
            let pixels_per_unit = xf.zoom().max(1e-6);
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
                    .measurement_to_user(page, [f64::from(vx) + state.sensitivity / f64::from(pixels_per_unit), f64::from(vy)])
                    .map(|p| measure::distance(point, p))
                    .unwrap_or(state.sensitivity / f64::from(pixels_per_unit));
                if state.snaps.intersections && paths.intersection_limited(point, tolerance) {
                    state.error = Some(tl!("This area has too many overlapping paths; intersection snapping is limited.").into());
                }
                if let Ok(Some(hit)) = paths.snap(point, tolerance, state.snaps) {
                    point = hit.point;
                    snap = Some(hit.kind);
                }
            }
        }
        if ui.input(|i| i.modifiers.shift)
            && let Some(last) = state.points.last()
        {
            point = measure::snap::constrain(*last, point);
        }
        hover = Some(point);
        if resp.clicked() {
            if state.page != Some(page) {
                state.cancel();
                state.page = Some(page);
            }
            if state.points.is_empty() {
                match doc.measurement_scale_or(page, point, units.per_point(), units.label()) {
                    Ok(scale) => {
                        state.scale = Some(scale);
                        state.error = None;
                    }
                    Err(e) => {
                        state.error = Some(e);
                        return;
                    }
                }
            }
            let closes =
                tool == Tool::Area && state.points.len() >= 3 && state.points.first().is_some_and(|first| screen(*first).distance(pos) <= 6.0);
            let repeats = state.points.last().is_some_and(|last| measure::distance(*last, point) < 1e-6);
            if !closes && !repeats && state.points.len() < measure::MAX_POINTS {
                state.points.push(point);
            }
            let finishes = closes || matches!(tool, Tool::Distance | Tool::Calibrate) && state.points.len() == 2;
            if finishes {
                finish(state, &mut view.pending_edit, doc, page, tool, author, units);
            }
        }
    }
    if state.page == Some(page) && !state.points.is_empty() {
        if (resp.double_clicked() || !typing && ui.input(|i| i.key_pressed(egui::Key::Enter))) && matches!(tool, Tool::Perimeter | Tool::Area) {
            finish(state, &mut view.pending_edit, doc, page, tool, author, units);
        }
        if !typing && ui.input(|i| i.key_pressed(egui::Key::Backspace)) {
            state.points.pop();
        }
        let mut points = state.points.clone();
        if let Some(p) = hover
            && points.last().is_none_or(|last| measure::distance(*last, p) > 1e-6)
        {
            points.push(p);
        }
        if let Some(scale) = &state.scale {
            match measure::reading(tool.kind(), &points, scale) {
                Ok(reading) => state.reading = Some(reading),
                Err(e) => state.error = Some(e.to_string()),
            }
        }
        let positions: Vec<Pos2> = points.iter().map(|&p| screen(p)).collect();
        let stroke = Stroke::new(1.5, Color32::from_rgb(0, 120, 214));
        for pair in positions.windows(2) {
            if let [a, b] = pair {
                ui.painter().line_segment([*a, *b], stroke);
            }
        }
        if tool == Tool::Area
            && let (Some(a), Some(b)) = (positions.first(), positions.last())
        {
            ui.painter().line_segment([*a, *b], stroke);
        }
        for &p in &positions {
            ui.painter().circle_filled(p, 3.0, stroke.color);
        }
        if let (Some(p), Some(kind)) = (hover, snap) {
            ui.painter().circle_stroke(screen(p), 6.0, Stroke::new(1.0, Color32::from_rgb(46, 158, 92)));
            ui.painter().text(
                screen(p) + egui::vec2(10.0, -10.0),
                egui::Align2::LEFT_BOTTOM,
                tl!(match kind {
                    measure::snap::SnapKind::Endpoint => "Endpoint",
                    measure::snap::SnapKind::Midpoint => "Midpoint",
                    measure::snap::SnapKind::Intersection => "Intersection",
                    measure::snap::SnapKind::Path => "Path",
                }),
                crate::theme::regular(11.0),
                stroke.color,
            );
        }
        if let (Some(p), Some(reading)) = (hover, &state.reading) {
            ui.painter().text(screen(p) + egui::vec2(12.0, 12.0), egui::Align2::LEFT_TOP, &reading.label, crate::theme::semibold(12.0), stroke.color);
        }
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
pub(crate) fn panel(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    egui::ScrollArea::vertical().id_salt("measure-panel").show(ui, |ui| panel_body(app, ui, t));
}
fn panel_body(app: &mut PdfCraftApp, ui: &mut egui::Ui, _t: &Tokens) {
    crate::widgets::begin_dialog(ui);
    let Some((index, id)) = app.active_ids() else {
        ui.label(tl!("Open a PDF to measure it."));
        return;
    };
    let mut tool = None;
    ui.horizontal(|ui| {
        for (label, value) in [("Distance", Tool::Distance), ("Perimeter", Tool::Perimeter), ("Area", Tool::Area)] {
            if ui.selectable_label(app.quick_tool == QuickTool::Measure(value), tl!(label)).clicked() {
                tool = Some(value);
            }
        }
    });
    let Some(doc) = app.session.get(id) else { return };
    let units = app.units;
    let page = app.views[index].current;
    let info = doc.info.pages.get(page).cloned();
    let state = &mut app.views[index].measure;
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
    ui.label(tl!("Click vertices. Enter or double-click finishes a perimeter or area. Click the first point to close an area."));
    ui.label(tl!("Shift constrains to 45 degrees. Backspace removes a vertex. Escape cancels."));
    ui.separator();
    ui.label(tl!("Measurement information"));
    if let Some(reading) = &state.reading {
        ui.label(egui::RichText::new(&reading.label).strong());
        ui.label(format!("X: {:.3}   Y: {:.3}", reading.delta_x, reading.delta_y));
        ui.label(format!("{}: {:.2}°", tl!("Angle"), reading.angle));
        if let Some(scale) = &state.scale {
            ui.label(&scale.ratio);
        }
    } else {
        ui.label(tl!("Click the first point to begin."));
    }
    ui.horizontal(|ui| {
        ui.label(tl!("Label"));
        ui.add(crate::widgets::line(&mut state.label).desired_width(130.0).char_limit(128));
    });
    ui.separator();
    ui.add(widgets::checkbox(&mut state.snap_enabled, tl!("Snap to drawing")));
    if state.snap_enabled {
        ui.horizontal(|ui| {
            ui.add(widgets::checkbox(&mut state.snaps.endpoints, tl!("Endpoints")));
            ui.add(widgets::checkbox(&mut state.snaps.midpoints, tl!("Midpoints")));
        });
        ui.horizontal(|ui| {
            ui.add(widgets::checkbox(&mut state.snaps.intersections, tl!("Intersections")));
            ui.add(widgets::checkbox(&mut state.snaps.paths, tl!("Paths")));
        });
        ui.add(egui::Slider::new(&mut state.sensitivity, 1.0..=20.0).text(tl!("Sensitivity")));
    }
    ui.separator();
    // "1:N" fills the fields below and applies at once, as does picking a common scale.
    let mut chosen = None;
    let fields = scale_fields(state);
    ui.horizontal(|ui| {
        ui.label(tl!("Drawing scale")).on_hover_text(tl!("Applying a scale switches units to m / mm."));
        let typed = ui.add(crate::widgets::line(&mut state.ratio_text).hint_text("1:100").desired_width(56.0).char_limit(16));
        typed.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, tl!("Scale")));
        state.ratio_dirty |= typed.changed();
        if typed.lost_focus() && std::mem::take(&mut state.ratio_dirty) {
            match parse_ratio(&state.ratio_text) {
                Some(n) => chosen = Some(n),
                None => state.error = Some(tl!("Enter a scale such as 1:50.").into()),
            }
        }
        egui::ComboBox::from_id_salt("measure-common-scales").selected_text(tl!("Common scales")).width(20.0).show_ui(ui, |ui| {
            for n in RATIOS {
                if ui.selectable_label(state.ratio_text == format_ratio(n), format_ratio(n)).clicked() {
                    chosen = Some(n);
                }
            }
        });
    });
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(&mut state.drawing_points).range(0.000001..=1e9));
        let page_unit = page_unit_of(state.page_unit, &state.unit, units);
        egui::ComboBox::from_id_salt("measure-page-unit").selected_text(page_unit.label()).width(44.0).show_ui(ui, |ui| {
            for u in PageUnit::ALL {
                if ui.selectable_label(u == page_unit, u.label()).clicked() {
                    state.page_unit = Some(u);
                }
            }
        });
        ui.label("=");
        ui.add(egui::DragValue::new(&mut state.real_distance).range(0.000001..=1e12));
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Unit"));
        ui.add(crate::widgets::line(&mut state.unit).desired_width(64.0).char_limit(12));
        ui.add(egui::DragValue::new(&mut state.precision).range(0..=6).prefix(format!("{} ", tl!("Decimals"))));
    });
    // A hand edit of the fields leaves the 1:N field showing the old ratio: say the new one.
    if scale_fields(state) != fields {
        sync_ratio_text(state, units);
    }
    if ui.button(tl!("Calibrate from two points")).clicked() {
        tool = Some(Tool::Calibrate);
    }
    ui.add(widgets::checkbox(&mut state.whole_page, tl!("Entire page")));
    if !state.whole_page {
        ui.label(tl!("Viewport rectangle in page points (x, y, width, height)"));
        for (i, name) in ["X", "Y", "Width", "Height"].iter().enumerate() {
            if let Some(v) = state.rect.get_mut(i) {
                ui.add(egui::DragValue::new(v).range(0.0..=1e9).prefix(format!("{}: ", tl!(name))));
            }
        }
    }
    ui.horizontal(|ui| {
        ui.label(tl!("Viewport"));
        ui.add(crate::widgets::line(&mut state.name).desired_width(130.0).char_limit(128));
    });
    let apply = ui.add_enabled(doc.allows_annotation(), egui::Button::new(tl!("Apply scale"))).clicked();
    if let Some(error) = &state.error {
        ui.colored_label(Color32::from_rgb(190, 50, 50), error);
    }
    ui.separator();
    let mut select = None;
    match state.listing.as_ref().map(|(_, l)| l) {
        None => {}
        Some(Ok(listing)) => {
            ui.label(format!("{}: {}", tl!("Saved measurements"), listing.measurements.len()));
            if !listing.unsupported.is_empty() {
                let reasons: Vec<String> =
                    listing.unsupported.iter().map(|u| format!("{} {}: {}", tl!("Page"), u.page.saturating_add(1), u.reason)).collect();
                ui.colored_label(Color32::from_rgb(190, 120, 30), format!("{}: {}", tl!("Unsupported measurements"), listing.unsupported.len()))
                    .on_hover_text(reasons.join("\n"));
            }
            egui::ScrollArea::vertical().id_salt("measurement-list").max_height(140.0).show(ui, |ui| {
                for m in &listing.measurements {
                    if ui.button(format!("{} {}: {} {}", tl!("Page"), m.page.saturating_add(1), m.reading.label, m.label)).clicked() {
                        select = Some((m.page, m.index, m.reading.clone(), m.scale.clone()));
                    }
                }
            });
        }
        Some(Err(e)) => {
            ui.colored_label(Color32::from_rgb(190, 50, 50), e);
        }
    }
    let export = ui.button(tl!("Export measurements as CSV")).clicked();
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
                app.views[index].pending_edit = Some(Edit::SetMeasurementScale {
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
        app.views[index].current = page;
        app.views[index].comments.selected = Some((page, annotation));
        app.views[index].measure.reading = Some(reading);
        app.views[index].measure.scale = Some(scale);
        app.quick_tool = QuickTool::Select;
    }
    if let Some(tool) = tool {
        app.views[index].measure.cancel();
        app.quick_tool = QuickTool::Measure(tool);
    }
    if export {
        export_csv(app);
    }
}
pub(crate) fn command(app: &mut PdfCraftApp, id: &str) {
    app.left = LeftPanel::Tool("measure");
    app.left_open = true;
    if let Some((index, _)) = app.active_ids()
        && let Some(tool) = id.strip_prefix("measure.").and_then(Tool::from_name)
    {
        app.views[index].measure.cancel();
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
            #[cfg(not(target_arch = "wasm32"))]
            {
                let write = move |app: &mut PdfCraftApp, path: std::path::PathBuf| match crate::editing::write_atomically(
                    &path.to_string_lossy(),
                    csv.as_bytes(),
                ) {
                    Ok(()) => app.notify(tl!("Saved measurements.")),
                    Err(e) => app.notify(e.to_string()),
                };
                match app.export_dir_override.as_ref().map(|d| std::path::PathBuf::from(d).join("measurements.csv")) {
                    Some(path) => write(app, path),
                    None => {
                        let dialog = rfd::AsyncFileDialog::new().set_file_name("measurements.csv").add_filter("CSV", &["csv"]);
                        app.ask_one(crate::pickers::Ask::Save(dialog), None, write);
                    }
                }
            }
            #[cfg(target_arch = "wasm32")]
            if let Err(e) = crate::editing::download("measurements.csv", csv.as_bytes()) {
                app.notify(&e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Apply scale builds from the panel's fields, then a distance read with it.
    fn measured(state: &MeasureView, from: Point, to: Point) -> f64 {
        let page_unit = page_unit_of(state.page_unit, &state.unit, crate::units::Unit::Points);
        let scale = Scale::ratio([0.0, 0.0], [1.0, 0.0], state.drawing_points, page_unit, state.real_distance, &state.unit, 4).expect("scale");
        measure::reading(Kind::Distance, &[from, to], &scale).expect("reading").value
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
        assert_eq!(format_ratio(50.0), "1:50");
        assert_eq!(format_ratio(2.5), "1:2.5");
        assert_eq!(format_ratio(0.5), "1:0.5");
        // Nothing is rounded away: the field reads the ratio that applies.
        assert_eq!(format_ratio(1.234), "1:1.234");
        assert_eq!(format_ratio(0.001), "1:0.001");
        assert_eq!(parse_ratio(&format_ratio(333.333333)), Some(333.333333));
        for n in RATIOS {
            assert_eq!(parse_ratio(&format_ratio(n)), Some(n));
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
    fn any_starting_unit_becomes_metres() {
        let mut state = MeasureView { unit: "yards?".into(), ..Default::default() };
        assert!(set_ratio(&mut state, 50.0));
        assert_eq!(state.unit, "m");
        // 1 mm of paper at 1:50 is 50 mm.
        let v = measured(&state, [0.0, 0.0], [72.0 / 25.4, 0.0]);
        assert!((v - 0.05).abs() < 1e-9, "{v}");
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

    #[test]
    fn an_unusable_ratio_changes_nothing() {
        let mut state = MeasureView::default();
        let before = (state.drawing_points, state.real_distance, state.unit.clone(), state.page_unit);
        for n in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(!set_ratio(&mut state, n));
        }
        assert_eq!((state.drawing_points, state.real_distance, state.unit.clone(), state.page_unit), before);
    }
}
