//! Measure objects: page-space clicks, live readings and persistent drawing scales.
use crate::{LeftPanel, PdfCraftApp, QuickTool, canvas::DocView, theme::Tokens};
use egui::{Color32, Pos2, Stroke};
use pdfcraft_engine::{
    Document, Edit, Style,
    measure::{self, Kind, NewMeasurement, Point, Reading, Scale},
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
    pub warning: Option<String>,
    pub calibration_ready: bool,
    pub scale_open: bool,
    pub format: String,
    pub denominator: u32,
    pub custom_formats: Option<measure::NumberFormats>,
    pub custom_ratio: String,
    pub format_axis: u8,
    pub format_unit: usize,
    pub viewport: Option<usize>,
    pub map_enabled: bool,
    pub map_registration: Option<measure::geo::GeoDefinition>,
    pub map_control: usize,
    pub map_boundary: usize,
    pub map_pick: Option<usize>,
    pub snaps: measure::snap::SnapOptions,
    pub snap_enabled: bool,
    pub sensitivity: f64,
    pub drawing_points: f64,
    pub real_distance: f64,
    pub unit: String,
    pub precision: u8,
    pub name: String,
    pub whole_page: bool,
    pub rect: [f64; 4],
    pub label: String,
    seeded_page: Option<usize>,
    loaded_scale: Option<Scale>,
    scale_seed: Option<(f64, f64, String, u8, String, u32)>,
    saved_page: usize,
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
            warning: None,
            calibration_ready: false,
            scale_open: false,
            format: "decimal".into(),
            denominator: 16,
            custom_formats: None,
            custom_ratio: String::new(),
            format_axis: 0,
            format_unit: 0,
            viewport: None,
            map_enabled: false,
            map_registration: None,
            map_control: 0,
            map_boundary: 0,
            map_pick: None,
            snaps: Default::default(),
            snap_enabled: true,
            sensitivity: 6.0,
            drawing_points: 72.0,
            real_distance: 1.0,
            unit: "in".into(),
            precision: 2,
            name: "Drawing scale".into(),
            whole_page: true,
            rect: [0.0, 0.0, 100.0, 100.0],
            label: String::new(),
            seeded_page: None,
            loaded_scale: None,
            scale_seed: None,
            saved_page: 0,
            paths: None,
            listing: None,
        }
    }
}
impl MeasureView {
    pub fn cancel(&mut self) {
        self.points.clear();
        self.page = None;
        self.reading = None;
        self.scale = None;
    }
}
pub(crate) fn page_input(ui: &egui::Ui, resp: &egui::Response, doc: &Document, view: &mut DocView, cx: &crate::comments::PageCx<'_>, tool: Tool) {
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
        state.map_pick = None;
    }
    let screen = |p: Point| {
        let v = info.user_to_view(p[0] as f32, p[1] as f32);
        xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph)
    };
    let pos = ui.input(|i| i.pointer.hover_pos()).filter(|p| xf.rect.contains(*p));
    let mut hover = None;
    let mut snap = None;
    if let Some(pos) = pos {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        let (vx, vy) = xf.screen_to_view(pos);
        let [x, y] = info.view_to_user(vx, vy);
        let mut point = [f64::from(x), f64::from(y)];
        state.warning = None;
        if state.snap_enabled {
            if state.paths.as_ref().is_none_or(|(p, g, _)| *p != page || *g != doc.edit_generation()) {
                state.paths = Some((page, doc.edit_generation(), doc.measurement_paths(page)));
            }
            let pixels_per_unit = (xf.rect.width() / xf.pw).max(1e-6);
            if let Some((_, _, Err(e))) = &state.paths {
                state.error = Some(e.clone());
            }
            if let Some((_, _, Ok(paths))) = &state.paths {
                if paths.truncated {
                    state.warning = Some(tl!("This drawing exceeds the snapping limit; only the first paths are used.").into());
                } else if paths.unreadable > 0 {
                    state.warning = Some(tl!("Some page content couldn't be read; snapping uses the rest.").into());
                } else if paths.visibility_limited > 0 {
                    state.warning = Some(tl!("Some soft masks or pattern paints need a visibility check before snapping.").into());
                }
                let tolerance = doc
                    .measurement_to_user(page, [f64::from(vx) + state.sensitivity / f64::from(pixels_per_unit), f64::from(vy)])
                    .map(|p| measure::distance(point, p))
                    .unwrap_or(state.sensitivity / f64::from(pixels_per_unit));
                if state.snaps.intersections && paths.intersection_limited(point, tolerance) {
                    state.warning = Some(tl!("This area has too many overlapping paths; intersection snapping is limited.").into());
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
        if tool == Tool::Calibrate && state.map_enabled {
            if resp.clicked()
                && let Some(index) = state.map_pick
            {
                match editor_bbox(state, doc, page) {
                    Ok(bbox) => {
                        let local = [(point[0] - bbox[0]) / (bbox[2] - bbox[0]), (point[1] - bbox[1]) / (bbox[3] - bbox[1])];
                        if local.iter().all(|v| (0.0..=1.0).contains(v)) {
                            if let Some(control) = state.map_registration.as_mut().and_then(|r| r.controls.get_mut(index)) {
                                control.local = local;
                                state.error = None;
                            }
                            state.map_pick = None;
                        } else {
                            state.error = Some(tl!("Choose a control point inside the viewport.").into());
                        }
                    }
                    Err(error) => state.error = Some(error),
                }
            }
            return;
        }
        if resp.clicked() {
            if state.page != Some(page) {
                state.cancel();
                state.page = Some(page);
            }
            if state.points.is_empty() {
                match doc.measurement_scale(page, point) {
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
                finish(state, &mut view.pending_edit, doc, page, tool, author);
            }
        }
    }
    if state.page == Some(page) && !state.points.is_empty() {
        if (resp.double_clicked() || !typing && ui.input(|i| i.key_pressed(egui::Key::Enter))) && matches!(tool, Tool::Perimeter | Tool::Area) {
            finish(state, &mut view.pending_edit, doc, page, tool, author);
        }
        if !typing && ui.input(|i| i.key_pressed(egui::Key::Backspace)) {
            state.points.pop();
            if state.points.is_empty() {
                state.cancel();
            }
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
        let stroke = Stroke::new(1.5, Color32::from_rgb(27, 99, 224));
        // A white underlay keeps the measuring path distinct on dark drawing content.
        let underlay = Stroke::new(3.5, Color32::WHITE);
        for pair in positions.windows(2) {
            if let [a, b] = pair {
                ui.painter().line_segment([*a, *b], underlay);
                ui.painter().line_segment([*a, *b], stroke);
            }
        }
        if tool == Tool::Area
            && let (Some(a), Some(b)) = (positions.first(), positions.last())
        {
            ui.painter().line_segment([*a, *b], underlay);
            ui.painter().line_segment([*a, *b], stroke);
        }
        for &p in &positions {
            ui.painter().circle_filled(p, 4.5, Color32::WHITE);
            ui.painter().circle_filled(p, 3.0, stroke.color);
        }
        if let (Some(p), Some(reading)) = (hover, &state.reading) {
            canvas_label(ui, xf.rect, screen(p) + egui::vec2(14.0, 18.0), &reading.label);
        }
    }
    // Show the snap target before the first click too.
    if let (Some(p), Some(kind)) = (hover, snap) {
        let p = screen(p);
        ui.painter().circle_filled(p, 7.0, Color32::WHITE);
        let stroke = Stroke::new(1.5, Color32::from_rgb(27, 99, 224));
        match kind {
            measure::snap::SnapKind::Endpoint => {
                ui.painter().rect_stroke(egui::Rect::from_center_size(p, egui::vec2(10.0, 10.0)), 0, stroke, egui::StrokeKind::Inside);
            }
            measure::snap::SnapKind::Intersection => {
                for offset in [egui::vec2(5.0, 5.0), egui::vec2(5.0, -5.0)] {
                    ui.painter().line_segment([p - offset, p + offset], stroke);
                }
            }
            _ => {
                ui.painter().circle_stroke(p, 5.0, stroke);
            }
        }
        canvas_label(
            ui,
            xf.rect,
            p + egui::vec2(14.0, -28.0),
            tl!(match kind {
                measure::snap::SnapKind::Endpoint => "Endpoint",
                measure::snap::SnapKind::Midpoint => "Midpoint",
                measure::snap::SnapKind::Intersection => "Intersection",
                measure::snap::SnapKind::Path => "Path",
            }),
        );
    }
}
fn canvas_label(ui: &egui::Ui, page: egui::Rect, position: Pos2, label: &str) {
    let t = Tokens::get(ui.ctx());
    let bounds = page.intersect(ui.clip_rect()).shrink(4.0);
    if !bounds.is_finite() || !bounds.is_positive() || !position.is_finite() {
        return;
    }
    let galley = ui.painter().layout(label.to_string(), crate::theme::semibold(12.0), t.text, bounds.width().max(40.0) - 12.0);
    let size = galley.size() + egui::vec2(12.0, 8.0);
    let position = Pos2::new(
        position.x.clamp(bounds.left(), (bounds.right() - size.x).max(bounds.left())),
        position.y.clamp(bounds.top(), (bounds.bottom() - size.y).max(bounds.top())),
    );
    let rect = egui::Rect::from_min_size(position, size);
    ui.painter().rect_filled(rect, 4, t.panel);
    ui.painter().rect_stroke(rect, 4, Stroke::new(1.0, t.text_faint), egui::StrokeKind::Inside);
    ui.painter().galley(position + egui::vec2(6.0, 4.0), galley, t.text);
}
fn finish(state: &mut MeasureView, pending: &mut Option<Edit>, doc: &Document, page: usize, tool: Tool, author: &str) {
    if tool == Tool::Calibrate {
        if let [a, b] = state.points.as_slice() {
            let a = doc.measurement_to_view(page, *a);
            let b = doc.measurement_to_view(page, *b);
            if let (Ok(a), Ok(b)) = (a, b) {
                state.drawing_points = measure::distance(a, b);
                state.calibration_ready = true;
                state.scale_open = true;
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
fn heading(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.add_space(5.0);
    ui.label(egui::RichText::new(text).font(crate::theme::semibold(13.0)).color(t.text));
    ui.add_space(2.0);
}
fn scale_fields(state: &MeasureView) -> (f64, f64, String, u8, String, u32) {
    (state.drawing_points, state.real_distance, state.unit.clone(), state.precision, state.format.clone(), state.denominator)
}

fn editor_bbox(state: &MeasureView, doc: &Document, page: usize) -> Result<[f64; 4], String> {
    let info = doc.info.pages.get(page).ok_or_else(|| "no such page".to_string())?;
    let rect = if state.whole_page {
        [0.0, 0.0, f64::from(info.width), f64::from(info.height)]
    } else {
        [state.rect[0], state.rect[1], state.rect[0] + state.rect[2], state.rect[1] + state.rect[3]]
    };
    let a = doc.measurement_to_user(page, [rect[0], rect[1]])?;
    let b = doc.measurement_to_user(page, [rect[2], rect[3]])?;
    let bbox = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
    if bbox[2] <= bbox[0] || bbox[3] <= bbox[1] {
        return Err(tl!("Viewport width and height must be positive.").into());
    }
    Ok(bbox)
}
fn blank_registration() -> measure::geo::GeoDefinition {
    measure::geo::GeoDefinition {
        gcs: measure::geo::CoordinateSystem { kind: measure::geo::CoordinateKind::Geographic, epsg: Some(4326), wkt: None },
        dcs: None,
        controls: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|local| measure::geo::ControlPoint { local, position: [0.0, 0.0] }).to_vec(),
        bounds: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        units: ["M".into(), "SQM".into(), "DEG".into()],
        matrix: None,
    }
}
fn coordinate_system_editor(ui: &mut egui::Ui, id: &str, system: &mut measure::geo::CoordinateSystem) {
    use measure::geo::CoordinateKind;
    ui.push_id(id, |ui| {
        egui::ComboBox::from_id_salt("kind")
            .selected_text(tl!(if system.kind == CoordinateKind::Geographic { "Geographic" } else { "Projected" }))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut system.kind, CoordinateKind::Geographic, tl!("Geographic"));
                ui.selectable_value(&mut system.kind, CoordinateKind::Projected, tl!("Projected"));
            });
        let mut wkt = system.wkt.is_some();
        if ui.checkbox(&mut wkt, tl!("Use WKT definition")).changed() {
            if wkt {
                system.epsg = None;
                system.wkt = Some(String::new());
            } else {
                system.wkt = None;
                system.epsg = Some(if system.kind == CoordinateKind::Geographic { 4326 } else { 3857 });
            }
        }
        if let Some(definition) = &mut system.wkt {
            let label = ui.label(tl!("WKT definition"));
            ui.add(egui::TextEdit::multiline(definition).desired_rows(3).desired_width(ui.available_width()).char_limit(16384)).labelled_by(label.id);
        } else {
            let code = system.epsg.get_or_insert(4326);
            let label = ui.label(tl!("EPSG code"));
            ui.add(egui::DragValue::new(code).range(1..=u32::MAX).speed(1.0)).labelled_by(label.id);
        }
    });
}
fn geo_editor(ui: &mut egui::Ui, state: &mut MeasureView, doc: &Document, page: usize, allowed: bool) -> bool {
    let bbox = editor_bbox(state, doc, page).ok();
    let Some(registration) = &mut state.map_registration else { return false };
    let mut picking = false;
    ui.add_enabled_ui(allowed, |ui| {
        ui.label(tl!("Map coordinate system"));
        coordinate_system_editor(ui, "map-gcs", &mut registration.gcs);
        let mut display = registration.dcs.is_some();
        if ui.checkbox(&mut display, tl!("Separate display coordinate system")).changed() {
            registration.dcs = if display { Some(registration.gcs.clone()) } else { None };
        }
        if let Some(dcs) = &mut registration.dcs {
            coordinate_system_editor(ui, "map-dcs", dcs);
        }
        ui.add_space(4.0);
        ui.label(tl!("Control points"));
        state.map_control = state.map_control.min(registration.controls.len().saturating_sub(1));
        egui::ComboBox::from_id_salt("map-control-point").selected_text(format!("{} {}", tl!("Control point"), state.map_control + 1)).show_ui(
            ui,
            |ui| {
                for index in 0..registration.controls.len() {
                    ui.selectable_value(&mut state.map_control, index, format!("{} {}", tl!("Control point"), index + 1));
                }
            },
        );
        if let Some(control) = registration.controls.get_mut(state.map_control) {
            if let Some(bbox) = bbox {
                let user = [bbox[0] + control.local[0] * (bbox[2] - bbox[0]), bbox[1] + control.local[1] * (bbox[3] - bbox[1])];
                if let Ok(mut position) = doc.measurement_to_view(page, user) {
                    let mut changed = false;
                    egui::Grid::new("map-page-position").num_columns(2).show(ui, |ui| {
                        for (value, name) in position.iter_mut().zip(["Page X", "Page Y"]) {
                            let label = ui.label(tl!(name));
                            changed |= ui.add(egui::DragValue::new(value).range(0.0..=1e9).speed(0.1)).labelled_by(label.id).changed();
                            ui.end_row();
                        }
                    });
                    if changed && let Ok(user) = doc.measurement_to_user(page, position) {
                        control.local = [(user[0] - bbox[0]) / (bbox[2] - bbox[0]), (user[1] - bbox[1]) / (bbox[3] - bbox[1])];
                    }
                }
            }
            if ui.button(tl!("Pick page position")).clicked() {
                state.map_pick = Some(state.map_control);
                picking = true;
            }
            egui::Grid::new("map-control-position").num_columns(2).show(ui, |ui| {
                let names = if registration.gcs.kind == measure::geo::CoordinateKind::Geographic {
                    ["Latitude", "Longitude"]
                } else {
                    ["Easting", "Northing"]
                };
                for (value, name) in control.position.iter_mut().zip(names) {
                    let label = ui.label(tl!(name));
                    ui.add(egui::DragValue::new(value).range(-1e12..=1e12).speed(0.0001).max_decimals(9)).labelled_by(label.id);
                    ui.end_row();
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(registration.controls.len() < 128, egui::Button::new(tl!("Add control point"))).clicked() {
                registration.controls.push(measure::geo::ControlPoint { local: [0.5, 0.5], position: [0.0, 0.0] });
                state.map_control = registration.controls.len() - 1;
            }
            if ui.add_enabled(registration.controls.len() > 3, egui::Button::new(tl!("Remove control point"))).clicked() {
                registration.controls.remove(state.map_control);
                state.map_control = state.map_control.min(registration.controls.len() - 1);
                state.map_pick = None;
            }
        });
        ui.label(tl!("Enter known map coordinates for at least three page positions."));
        for (index, name, options) in [
            (
                0,
                "Distance unit",
                vec![("M", "Metres"), ("KM", "Kilometres"), ("FT", "Feet"), ("USFT", "US survey feet"), ("MI", "Miles"), ("NM", "Nautical miles")],
            ),
            (
                1,
                "Area unit",
                vec![
                    ("SQM", "Square metres"),
                    ("HA", "Hectares"),
                    ("SQKM", "Square kilometres"),
                    ("SQFT", "Square US survey feet"),
                    ("A", "Acres"),
                    ("SQMI", "Square miles"),
                ],
            ),
            (2, "Angular unit", vec![("DEG", "Degrees"), ("GRD", "Grads")]),
        ] {
            ui.label(tl!(name));
            egui::ComboBox::from_id_salt(("map-unit", index))
                .selected_text(tl!(options.iter().find(|(code, _)| *code == registration.units[index]).map(|(_, name)| *name).unwrap_or("Unknown")))
                .show_ui(ui, |ui| {
                    for (code, name) in options {
                        ui.selectable_value(&mut registration.units[index], code.into(), tl!(name));
                    }
                });
        }
        let label = ui.label(tl!("Decimals"));
        ui.add(egui::DragValue::new(&mut state.precision).range(0..=6)).labelled_by(label.id);
        ui.collapsing(tl!("Map boundary"), |ui| {
            ui.label(tl!("Boundary coordinates run from 0 to 1 across the viewport."));
            state.map_boundary = state.map_boundary.min(registration.bounds.len().saturating_sub(1));
            egui::ComboBox::from_id_salt("map-boundary-point")
                .selected_text(format!("{} {}", tl!("Boundary point"), state.map_boundary + 1))
                .show_ui(ui, |ui| {
                    for index in 0..registration.bounds.len() {
                        ui.selectable_value(&mut state.map_boundary, index, format!("{} {}", tl!("Boundary point"), index + 1));
                    }
                });
            if let Some(point) = registration.bounds.get_mut(state.map_boundary) {
                ui.horizontal(|ui| {
                    for value in point {
                        ui.add(egui::DragValue::new(value).range(0.0..=1.0).speed(0.001));
                    }
                });
            }
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(registration.bounds.len() < 128, egui::Button::new(tl!("Add boundary point"))).clicked() {
                    registration.bounds.push([0.5, 0.5]);
                    state.map_boundary = registration.bounds.len() - 1;
                }
                if ui.add_enabled(registration.bounds.len() > 3, egui::Button::new(tl!("Remove boundary point"))).clicked() {
                    registration.bounds.remove(state.map_boundary);
                    state.map_boundary = state.map_boundary.min(registration.bounds.len() - 1);
                }
            });
        });
        ui.collapsing(tl!("Projected coordinate matrix"), |ui| {
            let mut enabled = registration.matrix.is_some();
            if ui
                .add_enabled(
                    registration.gcs.kind == measure::geo::CoordinateKind::Projected,
                    egui::Checkbox::new(&mut enabled, tl!("Use projected coordinate matrix")),
                )
                .changed()
            {
                registration.matrix = if enabled { Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]) } else { None };
            }
            if let Some(matrix) = &mut registration.matrix {
                ui.label(tl!("Converts PDF user coordinates to easting, northing and elevation."));
                egui::Grid::new("map-matrix").num_columns(3).show(ui, |ui| {
                    for row in matrix.chunks_mut(3) {
                        for value in row {
                            ui.add(egui::DragValue::new(value).range(-1e12..=1e12).speed(0.001));
                        }
                        ui.end_row();
                    }
                });
            }
        });
    });
    picking
}
fn custom_format_editor(ui: &mut egui::Ui, state: &mut MeasureView, user_units_per_point: f64) {
    if state.custom_formats.is_none() {
        let scale = state.loaded_scale.clone().or_else(|| Scale::new(state.real_distance / state.drawing_points, &state.unit, state.precision).ok());
        if let Some(scale) = scale {
            let conversion = if state.loaded_scale.is_some() { user_units_per_point } else { 1.0 };
            let mut formats = scale.formats.clone().unwrap_or_else(|| measure::NumberFormats {
                x: vec![measure::NumberFormat::decimal(&scale.unit, scale.x, scale.precision)],
                y: if scale.y != scale.x { Some(vec![measure::NumberFormat::decimal(&scale.unit, scale.y, scale.precision)]) } else { None },
                distance: vec![measure::NumberFormat::decimal(&scale.unit, scale.distance_factor, scale.precision)],
                area: vec![measure::NumberFormat::decimal(&scale.area_unit, scale.area_factor, scale.precision)],
                cyx: if scale.y != scale.x { Some(1.0) } else { None },
            });
            if conversion.is_finite() && conversion > 0.0 {
                if let Some(x) = formats.x.first_mut() {
                    x.factor *= conversion;
                }
                if let Some(y) = formats.y.as_mut().and_then(|y| y.first_mut()) {
                    y.factor *= conversion;
                }
            }
            state.custom_ratio = scale.ratio.clone();
            state.custom_formats = Some(formats);
        }
    }
    let Some(formats) = &mut state.custom_formats else { return };
    ui.label(tl!("Advanced unit formats"));
    ui.label(tl!("Units run from largest to smallest. Later factors convert the remainder."));
    let label = ui.label(tl!("Scale description"));
    ui.add(egui::TextEdit::singleline(&mut state.custom_ratio).char_limit(256).desired_width(ui.available_width())).labelled_by(label.id);
    let mut separate_y = formats.y.is_some();
    if ui.checkbox(&mut separate_y, tl!("Separate vertical units")).changed() {
        formats.y = if separate_y { Some(formats.x.clone()) } else { None };
        formats.cyx = if separate_y { Some(1.0) } else { None };
    }
    if formats.y.is_some() {
        let label = ui.label(tl!("Vertical to horizontal conversion"));
        let value = formats.cyx.get_or_insert(1.0);
        ui.add(egui::DragValue::new(value).range(1e-12..=1e12).speed(0.01)).labelled_by(label.id);
    }
    if state.format_axis == 1 && formats.y.is_none() {
        state.format_axis = 0;
    }
    let axis_label = ui.label(tl!("Quantity"));
    egui::ComboBox::from_id_salt("measure-format-axis")
        .selected_text(tl!(match state.format_axis {
            1 => "Vertical coordinates",
            2 => "Distance units",
            3 => "Area units",
            _ => "Horizontal coordinates",
        }))
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (index, label) in [(0, "Horizontal coordinates"), (1, "Vertical coordinates"), (2, "Distance units"), (3, "Area units")] {
                if index != 1 || formats.y.is_some() {
                    ui.selectable_value(&mut state.format_axis, index, tl!(label));
                }
            }
        })
        .response
        .labelled_by(axis_label.id);
    let values = match state.format_axis {
        1 => formats.y.as_mut().unwrap_or(&mut formats.x),
        2 => &mut formats.distance,
        3 => &mut formats.area,
        _ => &mut formats.x,
    };
    state.format_unit = state.format_unit.min(values.len().saturating_sub(1));
    let unit_label = ui.label(tl!("Selected unit"));
    egui::ComboBox::from_id_salt("measure-format-unit")
        .selected_text(format!("{} {}", tl!("Unit"), state.format_unit + 1))
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (index, value) in values.iter().enumerate() {
                ui.selectable_value(&mut state.format_unit, index, format!("{}: {}", index + 1, value.unit));
            }
        })
        .response
        .labelled_by(unit_label.id);
    if let Some(value) = values.get_mut(state.format_unit) {
        egui::Grid::new("measure-full-format-fields").num_columns(2).show(ui, |ui| {
            for (label, text) in [
                ("Unit", &mut value.unit),
                ("Thousands separator", &mut value.thousands),
                ("Decimal separator", &mut value.decimal),
                ("Unit prefix", &mut value.prefix),
                ("Unit suffix", &mut value.suffix),
            ] {
                let label = ui.label(tl!(label));
                ui.add(egui::TextEdit::singleline(text).char_limit(24).desired_width(90.0)).labelled_by(label.id);
                ui.end_row();
            }
            let label = ui.label(tl!("Conversion factor"));
            ui.add(egui::DragValue::new(&mut value.factor).range(1e-12..=1e12).speed(0.001)).labelled_by(label.id);
            ui.end_row();
            let label = ui.label(tl!("Denominator"));
            ui.add(egui::DragValue::new(&mut value.denominator).range(1..=1_000_000_000u32)).labelled_by(label.id);
            ui.end_row();
        });
        let display_label = ui.label(tl!("Unit display format"));
        egui::ComboBox::from_id_salt("measure-format-fraction")
            .selected_text(tl!(match value.fraction {
                measure::Fraction::Decimal => "Decimal",
                measure::Fraction::Fraction => "Fraction",
                measure::Fraction::Round => "Rounded",
                measure::Fraction::Truncate => "Truncated",
            }))
            .show_ui(ui, |ui| {
                for (kind, label) in [
                    (measure::Fraction::Decimal, "Decimal"),
                    (measure::Fraction::Fraction, "Fraction"),
                    (measure::Fraction::Round, "Rounded"),
                    (measure::Fraction::Truncate, "Truncated"),
                ] {
                    ui.selectable_value(&mut value.fraction, kind, tl!(label));
                }
            })
            .response
            .labelled_by(display_label.id);
        ui.checkbox(&mut value.fixed, tl!("Keep trailing zeros or denominator"));
        ui.checkbox(&mut value.unit_first, tl!("Unit before value"));
    }
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(values.len() < 16, egui::Button::new(tl!("Add smaller unit"))).clicked() {
            if let Some(previous) = values.last_mut()
                && !previous.unit_first
                && previous.suffix.is_empty()
            {
                previous.suffix = " ".into();
            }
            values.push(measure::NumberFormat::decimal("in", 12.0, 2));
            state.format_unit = values.len() - 1;
        }
        if ui.add_enabled(values.len() > 1, egui::Button::new(tl!("Remove unit"))).clicked() {
            values.remove(state.format_unit);
            state.format_unit = state.format_unit.saturating_sub(1);
        }
        if ui.add_enabled(state.format_unit > 0, egui::Button::new(tl!("Move unit up"))).clicked() {
            values.swap(state.format_unit, state.format_unit - 1);
            state.format_unit -= 1;
        }
        if ui.add_enabled(state.format_unit + 1 < values.len(), egui::Button::new(tl!("Move unit down"))).clicked() {
            values.swap(state.format_unit, state.format_unit + 1);
            state.format_unit += 1;
        }
    });
    match formats.validate() {
        Ok(()) => {
            if let Ok(scale) = Scale::from_formats(formats.clone(), &state.custom_ratio)
                && let Ok(reading) = measure::reading(Kind::Distance, &[[0.0, 0.0], [72.0, 0.0]], &scale)
            {
                ui.label(format!("{}: {}", tl!("Reference preview"), reading.label));
            }
        }
        Err(error) => {
            ui.label(error.to_string());
        }
    }
}
fn configured_scale(state: &MeasureView) -> Result<Scale, String> {
    if state.format == "custom" {
        let formats = state.custom_formats.clone().ok_or_else(|| tl!("Choose advanced unit formats first.").to_string())?;
        return match &state.loaded_scale {
            Some(old) => old.with_formats(formats, &state.custom_ratio),
            None => Scale::from_formats(formats, &state.custom_ratio),
        }
        .map_err(|e| e.to_string());
    }

    if !state.drawing_points.is_finite() || state.drawing_points <= 0.0 {
        return Err(tl!("Reference length must be positive.").into());
    }
    let mut scale = Scale::new(state.real_distance / state.drawing_points, &state.unit, state.precision).map_err(|e| e.to_string())?;
    scale.ratio = format!("{} pt = {} {}", state.drawing_points, state.real_distance, state.unit);
    if state.format != "decimal" {
        let mut distance = measure::NumberFormat::decimal(&state.unit, 1.0, state.precision);
        if state.format == "fraction" || state.format == "feet-inch" {
            distance.fraction = measure::Fraction::Fraction;
            distance.denominator = state.denominator;
            distance.fixed = false;
        }
        let distances = if state.format == "feet-inch" {
            if state.unit != "ft" {
                return Err(tl!("Feet and inches uses ft as the scale unit.").into());
            }
            let mut foot = measure::NumberFormat::decimal("ft", 1.0, 0);
            foot.suffix = " ".into();
            distance.unit = "in".into();
            distance.factor = 12.0;
            vec![foot, distance]
        } else {
            if state.format == "round" {
                distance.fraction = measure::Fraction::Round;
            }
            if state.format == "truncate" {
                distance.fraction = measure::Fraction::Truncate;
            }
            vec![distance]
        };
        scale = Scale::from_formats(
            measure::NumberFormats {
                x: vec![measure::NumberFormat::decimal(&state.unit, scale.x, state.precision)],
                y: None,
                distance: distances,
                area: vec![measure::NumberFormat::decimal(&scale.area_unit, 1.0, state.precision)],
                cyx: None,
            },
            &scale.ratio,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(scale)
}
fn panel_body(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let Some((index, id)) = app.active_ids() else {
        ui.label(tl!("Open a PDF to measure it."));
        return;
    };
    let Some(doc) = app.session.get(id) else { return };
    let allowed = doc.allows_annotation();
    let page = app.views[index].current;
    let mut open_model = None;
    match doc.three_d_models(page) {
        Ok(models) if !models.is_empty() => {
            ui.label(egui::RichText::new(tl!("3D models on this page")).strong());
            for model in models {
                if ui.button(format!("{} · {}", tl!("Measure 3D"), model.name)).clicked() {
                    open_model = Some(model.annotation);
                }
            }
            ui.separator();
        }
        Err(error) => {
            ui.label(error);
        }
        _ => {}
    }
    let view = &mut app.views[index];
    let selected_annotation = view.comments.selected;
    let state = &mut view.measure;
    let active = match app.quick_tool {
        QuickTool::Measure(tool) => Some(tool),
        _ => None,
    };
    let mut tool = None;
    ui.horizontal(|ui| {
        let width = ((ui.available_width() - 12.0) / 3.0).max(40.0);
        ui.spacing_mut().item_spacing.x = 6.0;
        for (label, value) in [("Distance", Tool::Distance), ("Perimeter", Tool::Perimeter), ("Area", Tool::Area)] {
            let selected = active == Some(value);
            let text = egui::RichText::new(tl!(label)).color(if selected { t.accent_text } else { t.text });
            if ui.add_enabled(allowed, egui::Button::new(text).selected(selected).min_size(egui::vec2(width, 32.0))).clicked() {
                tool = Some(value);
            }
        }
    });
    ui.add_space(5.0);
    let instruction = if state.map_pick.is_some() {
        "Click the control point on the map."
    } else {
        match active {
            Some(Tool::Distance) => "Click two points to measure a distance.",
            Some(Tool::Perimeter) => "Click each vertex. Enter finishes the perimeter.",
            Some(Tool::Area) => "Click the boundary. Enter or the first point closes the area.",
            Some(Tool::Calibrate) => "Click the ends of a known reference length.",
            None => "Choose a tool to start measuring.",
        }
    };
    ui.label(egui::RichText::new(tl!(instruction)).color(t.text_muted));
    ui.collapsing(tl!("Keyboard shortcuts"), |ui| {
        ui.label(tl!("Shift constrains to 45 degrees. Backspace removes a vertex. Escape cancels."));
        ui.label(tl!("Double-click also finishes a perimeter or area."));
    });
    if !allowed {
        ui.label(egui::RichText::new(tl!("This document does not allow measurement edits.")).color(t.text));
    }
    if state.listing.as_ref().is_none_or(|(g, _)| *g != doc.edit_generation()) {
        state.listing = Some((doc.edit_generation(), doc.measurements()));
    }
    if state.page.is_some_and(|drawing_page| drawing_page != page) {
        state.cancel();
    }
    if state.seeded_page != Some(page) {
        state.calibration_ready = false;
        state.seeded_page = Some(page);
        state.viewport = None;
        state.loaded_scale = None;
        state.scale_seed = None;
        state.map_registration = None;
        state.map_enabled = false;
        state.custom_formats = None;
        state.map_pick = None;
        if let (Ok(a), Ok(b)) = (doc.measurement_to_user(page, [0.0, 0.0]), doc.measurement_to_user(page, [72.0, 0.0]))
            && let Ok(scale) = doc.measurement_scale(page, a)
            && let Ok(reading) = measure::reading(Kind::Distance, &[a, b], &scale)
        {
            state.drawing_points = 72.0;
            state.real_distance = reading.value;
            state.unit = scale.unit.clone();
            state.precision = scale.precision;
            state.map_registration = scale.geospatial.as_ref().map(|g| g.definition.clone());
            state.map_enabled = state.map_registration.is_some();
            state.loaded_scale = Some(scale);
            state.scale_seed = Some(scale_fields(state));
        }
    }
    let editing_scale = state.scale_open || active == Some(Tool::Calibrate);
    ui.style_mut().visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, t.text_faint);
    ui.style_mut().visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, t.accent);
    if !editing_scale {
        ui.separator();
        heading(ui, t, tl!("Measurement information"));
        if let Some(reading) = &state.reading {
            ui.label(egui::RichText::new(&reading.label).font(crate::theme::semibold(21.0)).color(t.text));
            egui::Grid::new("measurement-deltas").num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
                for (name, value) in [
                    ("X", reading.delta_x_label.clone()),
                    ("Y", reading.delta_y_label.clone()),
                    (if reading.geospatial.is_some() { "Azimuth" } else { "Angle" }, reading.angle_label.clone()),
                ] {
                    ui.label(egui::RichText::new(tl!(name)).color(t.text_muted));
                    ui.monospace(value);
                    ui.end_row();
                }
            });
            if let Some(geo) = &reading.geospatial
                && let Some(coordinate) = geo.coordinates.last()
            {
                heading(ui, t, tl!("Map coordinates"));
                ui.label(format!("{}: {:.6}°", tl!("Latitude"), coordinate.latitude));
                ui.label(format!("{}: {:.6}°", tl!("Longitude"), coordinate.longitude));
                ui.label(egui::RichText::new(&coordinate.display_system).color(t.text_muted));
                ui.monospace(format!("{:.6}, {:.6} {}", coordinate.display[0], coordinate.display[1], coordinate.display_unit));
                if coordinate.display_approximate {
                    ui.label(tl!("The display datum transformation is approximate."));
                }
                ui.collapsing(tl!("Map accuracy"), |ui| {
                    ui.label(format!("{}: {}", tl!("Registration residual"), geo.registration_residual));
                    ui.label(&coordinate.display_operation);
                    if let Some(accuracy) = coordinate.display_accuracy_meters {
                        ui.label(format!("{}: {} m", tl!("Datum accuracy"), accuracy));
                    }
                });
            }
        } else {
            ui.label(egui::RichText::new(tl!("Click the first point to begin.")).color(t.text_muted));
        }
        let scale = state.scale.clone().or_else(|| doc.measurement_to_user(page, [0.0, 0.0]).ok().and_then(|p| doc.measurement_scale(page, p).ok()));
        if let Some(scale) = scale {
            ui.label(egui::RichText::new(format!("{}: {}", tl!("Drawing scale"), scale.ratio)).color(t.text_muted));
        }
        if !state.points.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{}: {}", tl!("Vertices"), state.points.len()));
                if matches!(active, Some(Tool::Perimeter | Tool::Area))
                    && ui.button(tl!("Finish measurement")).clicked()
                    && let Some(tool) = active
                {
                    finish(state, &mut view.pending_edit, doc, page, tool, &app.comment_prefs.author);
                }
                if ui.button(tl!("Cancel")).clicked() {
                    state.cancel();
                }
            });
        }
        // Give fields a visible boundary within this panel without changing the app-wide style.
        ui.style_mut().visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, t.text_faint);
        ui.style_mut().visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, t.accent);
        let label = ui.label(tl!("Label"));
        ui.add_enabled(allowed, egui::TextEdit::singleline(&mut state.label).desired_width(ui.available_width()).char_limit(128))
            .labelled_by(label.id);
        ui.separator();
        heading(ui, t, tl!("Snap to drawing"));
        ui.checkbox(&mut state.snap_enabled, tl!("Enable snapping"));
        if state.snap_enabled {
            egui::Grid::new("measurement-snap-targets").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.checkbox(&mut state.snaps.endpoints, tl!("Endpoints"));
                ui.checkbox(&mut state.snaps.midpoints, tl!("Midpoints"));
                ui.end_row();
                ui.checkbox(&mut state.snaps.intersections, tl!("Intersections"));
                ui.checkbox(&mut state.snaps.paths, tl!("Paths"));
                ui.end_row();
            });
            ui.label(egui::RichText::new(tl!("Sensitivity (screen pixels)")).color(t.text_muted));
            ui.scope(|ui| {
                ui.visuals_mut().widgets.inactive.bg_fill = t.text_faint;
                ui.visuals_mut().widgets.hovered.bg_fill = t.accent_text;
                ui.spacing_mut().slider_width = (ui.available_width() - 64.0).max(80.0);
                ui.add(egui::Slider::new(&mut state.sensitivity, 1.0..=20.0));
            });
        }
        if let Some(warning) = &state.warning {
            ui.label(egui::RichText::new(warning).color(t.text_muted));
        }
    }
    ui.separator();
    let mut apply = false;
    let mut remove = false;
    let viewports = doc.measurement_viewports(page);
    if editing_scale {
        heading(ui, t, tl!("Drawing scale"));
        if ui.button(tl!("Back to measurement")).clicked() {
            state.scale_open = false;
            if active == Some(Tool::Calibrate) {
                tool = Some(Tool::Distance);
            }
        }
        ui.scope(|ui| {
            ui.label(egui::RichText::new(tl!("Saved measurements keep their original scale.")).color(t.text_muted));
            ui.add_space(4.0);
            if ui.add_enabled(allowed, egui::Checkbox::new(&mut state.map_enabled, tl!("Geospatial map"))).changed()
                && state.map_enabled
                && state.map_registration.is_none()
            {
                state.map_registration = Some(blank_registration());
            }
            if state.map_enabled {
                if geo_editor(ui, state, doc, page, allowed) {
                    tool = Some(Tool::Calibrate);
                    state.cancel();
                }
            } else {
                if ui
                    .add_enabled(allowed, egui::Button::new(tl!("Calibrate from two points")).min_size(egui::vec2(ui.available_width(), 30.0)))
                    .clicked()
                {
                    tool = Some(Tool::Calibrate);
                    state.calibration_ready = false;
                }
                if state.calibration_ready {
                    ui.label(egui::RichText::new(tl!("Reference captured. Enter its real length below.")).color(t.accent_text));
                }
                if state.format != "custom" {
                    egui::Grid::new("measure-reference-fields").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                        let label = ui.label(tl!("Reference length (page points)"));
                        ui.add_enabled(allowed, egui::DragValue::new(&mut state.drawing_points).range(0.000001..=1e9).speed(0.1))
                            .labelled_by(label.id);
                        ui.end_row();
                        let label = ui.label(tl!("Real length"));
                        ui.add_enabled(allowed, egui::DragValue::new(&mut state.real_distance).range(0.000001..=1e12).speed(0.1))
                            .labelled_by(label.id);
                        ui.end_row();
                        let label = ui.label(tl!("Unit"));
                        ui.add_enabled(allowed, egui::TextEdit::singleline(&mut state.unit).desired_width(64.0).char_limit(24)).labelled_by(label.id);
                        ui.end_row();
                    });
                }
                let format_label = ui.label(tl!("Number format"));
                egui::ComboBox::from_id_salt("measure-number-format")
                    .selected_text(tl!(match state.format.as_str() {
                        "custom" => "Advanced unit formats",
                        "fraction" => "Fraction",
                        "feet-inch" => "Feet and inches",
                        "round" => "Rounded",
                        "truncate" => "Truncated",
                        _ => "Decimal",
                    }))
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            ("decimal", "Decimal"),
                            ("fraction", "Fraction"),
                            ("feet-inch", "Feet and inches"),
                            ("round", "Rounded"),
                            ("truncate", "Truncated"),
                            ("custom", "Advanced unit formats"),
                        ] {
                            ui.selectable_value(&mut state.format, value.into(), tl!(label));
                        }
                    })
                    .response
                    .labelled_by(format_label.id);
                if state.format == "custom" {
                    let conversion = doc
                        .measurement_to_user(page, [0.0, 0.0])
                        .and_then(|a| doc.measurement_to_user(page, [1.0, 0.0]).map(|b| measure::distance(a, b)))
                        .unwrap_or(1.0);
                    custom_format_editor(ui, state, conversion);
                }
                if state.format == "decimal" {
                    ui.add(egui::DragValue::new(&mut state.precision).range(0..=6).prefix(format!("{} ", tl!("Decimals"))));
                } else if matches!(state.format.as_str(), "fraction" | "feet-inch") {
                    ui.add(egui::DragValue::new(&mut state.denominator).range(1..=1024).prefix(format!("{} ", tl!("Denominator"))));
                }
            }
            ui.add_space(4.0);
            ui.checkbox(&mut state.whole_page, tl!("Entire page"));
            if !state.whole_page {
                ui.label(tl!("Viewport rectangle in page points (x, y, width, height)"));
                egui::Grid::new("measure-viewport-bounds").num_columns(2).show(ui, |ui| {
                    for (value, label) in state.rect.iter_mut().zip(["X", "Y", "Width", "Height"]) {
                        ui.label(tl!(label));
                        ui.add(egui::DragValue::new(value).range(0.0..=1e9));
                        ui.end_row();
                    }
                });
            }
            let label = ui.label(tl!("Viewport"));
            ui.add_enabled(allowed, egui::TextEdit::singleline(&mut state.name).desired_width(ui.available_width()).char_limit(128))
                .labelled_by(label.id);
            if let Ok(viewports) = &viewports
                && !viewports.is_empty()
            {
                egui::ComboBox::from_id_salt("measure-viewport-selection")
                    .selected_text(
                        state
                            .viewport
                            .and_then(|index| viewports.iter().find(|v| v.index == index))
                            .map(|v| v.name.as_str())
                            .unwrap_or(tl!("New viewport")),
                    )
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut state.viewport, None, tl!("New viewport"));
                        for viewport in viewports {
                            if ui.selectable_value(&mut state.viewport, Some(viewport.index), &viewport.name).clicked() {
                                state.name = viewport.name.clone();
                                state.whole_page = false;
                                if let (Ok(a), Ok(b)) = (
                                    doc.measurement_to_view(page, [viewport.bbox[0], viewport.bbox[1]]),
                                    doc.measurement_to_view(page, [viewport.bbox[2], viewport.bbox[3]]),
                                ) {
                                    state.rect = [a[0].min(b[0]), a[1].min(b[1]), (a[0] - b[0]).abs(), (a[1] - b[1]).abs()];
                                }
                                state.custom_formats = None;
                                state.loaded_scale = None;
                                state.scale_seed = None;
                                state.map_registration = None;
                                state.map_enabled = false;
                                if let Ok(scale) = &viewport.scale {
                                    state.map_registration = scale.geospatial.as_ref().map(|g| g.definition.clone());
                                    state.map_enabled = state.map_registration.is_some();
                                    state.loaded_scale = Some(scale.clone());
                                    state.precision = scale.precision;
                                }
                                if let Ok(scale) = &viewport.scale
                                    && let (Ok(a), Ok(b)) = (doc.measurement_to_user(page, [0.0, 0.0]), doc.measurement_to_user(page, [72.0, 0.0]))
                                    && let Ok(reading) = measure::reading(Kind::Distance, &[a, b], scale)
                                {
                                    state.drawing_points = 72.0;
                                    state.real_distance = reading.value;
                                    state.unit = scale.unit.clone();
                                    state.precision = scale.precision;
                                    state.loaded_scale = Some(scale.clone());
                                    state.scale_seed = Some(scale_fields(state));
                                }
                            }
                        }
                    });
            }
            ui.horizontal_wrapped(|ui| {
                apply = ui
                    .add_enabled(
                        allowed,
                        egui::Button::new(egui::RichText::new(tl!("Apply scale")).color(Color32::WHITE))
                            .fill(Color32::from_rgb(27, 99, 224))
                            .stroke(Stroke::new(1.0, t.accent_text))
                            .min_size(egui::vec2(110.0, 32.0)),
                    )
                    .clicked();
                if state.viewport.is_some() {
                    remove = ui.add_enabled(allowed, egui::Button::new(tl!("Remove viewport"))).clicked();
                }
            });
        });
    } else if ui.add_enabled(allowed, egui::Button::new(tl!("Change drawing scale")).min_size(egui::vec2(ui.available_width(), 30.0))).clicked() {
        state.scale_open = true;
    }
    if let Some(error) = &state.error {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(format!("{}: {error}", tl!("Measurement error"))).color(t.text));
    }
    ui.separator();
    heading(ui, t, tl!("Saved measurements"));
    let mut select = None;
    let mut export = false;
    match state.listing.as_ref().map(|(_, l)| l) {
        Some(Ok(listing)) => {
            if listing.measurements.is_empty() {
                ui.label(egui::RichText::new(tl!("Finished measurements appear here and are saved in the PDF.")).color(t.text_muted));
            }
            let pages = listing.measurements.len().div_ceil(20).max(1);
            state.saved_page = state.saved_page.min(pages - 1);
            for m in listing.measurements.iter().skip(state.saved_page.saturating_mul(20)).take(20) {
                let selected = selected_annotation == Some((m.page, m.index));
                let title = if m.label.is_empty() { m.reading.label.clone() } else { format!("{} · {}", m.label, m.reading.label) };
                if ui
                    .selectable_label(selected, title)
                    .on_hover_text(format!("{} {} · {}", tl!("Page"), m.page.saturating_add(1), m.reading.kind.name()))
                    .clicked()
                {
                    select = Some((m.page, m.index, m.reading.clone(), m.scale.clone()));
                }
                ui.label(
                    egui::RichText::new(format!(
                        "{} {} · {}",
                        tl!("Page"),
                        m.page.saturating_add(1),
                        tl!(match m.reading.kind {
                            Kind::Distance => "Distance",
                            Kind::Perimeter => "Perimeter",
                            Kind::Area => "Area",
                        })
                    ))
                    .small()
                    .color(t.text_muted),
                );
            }
            if pages > 1 {
                ui.horizontal(|ui| {
                    if ui.add_enabled(state.saved_page > 0, egui::Button::new(tl!("Previous"))).clicked() {
                        state.saved_page = state.saved_page.saturating_sub(1);
                    }
                    ui.label(format!("{} / {pages}", state.saved_page + 1));
                    if ui.add_enabled(state.saved_page + 1 < pages, egui::Button::new(tl!("Next"))).clicked() {
                        state.saved_page += 1;
                    }
                });
            }
            if !listing.unsupported.is_empty() {
                ui.collapsing(format!("{} ({})", tl!("Unsupported measurements"), listing.unsupported.len()), |ui| {
                    for u in &listing.unsupported {
                        ui.label(format!("{} {}: {}", tl!("Page"), u.page.saturating_add(1), u.reason));
                    }
                });
            }
            if listing.truncated {
                ui.label(tl!("The measurement list reached its safety limit."));
            }
            export = ui.add_enabled(!listing.measurements.is_empty(), egui::Button::new(tl!("Export measurements as CSV"))).clicked();
        }
        Some(Err(error)) => {
            ui.label(error);
        }
        None => {}
    }
    if apply {
        let preserve = state.format != "custom" && state.scale_seed.as_ref().is_some_and(|seed| *seed == scale_fields(state));
        let selected_scale = if state.map_enabled {
            Scale::new(1.0, "m", state.precision).map_err(|e| e.to_string())
        } else if preserve {
            state.loaded_scale.clone().ok_or_else(|| tl!("The selected scale could not be read.").to_string())
        } else {
            configured_scale(state)
        };
        let result = selected_scale.and_then(|scale| {
            let info = doc.info.pages.get(page).ok_or_else(|| "no such page".to_string())?;
            let rect = if state.whole_page {
                [0.0, 0.0, f64::from(info.width), f64::from(info.height)]
            } else {
                [state.rect[0], state.rect[1], state.rect[0] + state.rect[2], state.rect[1] + state.rect[3]]
            };
            let from = doc.measurement_to_user(page, [rect[0], rect[1]])?;
            let to = doc.measurement_to_user(page, [rect[2], rect[3]])?;
            let a = doc.measurement_to_user(page, [0.0, 0.0])?;
            let b = doc.measurement_to_user(page, [1.0, 0.0])?;
            let conversion = if preserve || state.map_enabled { 1.0 } else { measure::distance(a, b) };
            let mut scale = scale;
            scale.x /= conversion;
            scale.y /= conversion;
            if let Some(formats) = &mut scale.formats
                && let Some(x) = formats.x.first_mut()
            {
                x.factor /= conversion;
            }
            if let Some(y) = scale.formats.as_mut().and_then(|formats| formats.y.as_mut()).and_then(|y| y.first_mut()) {
                y.factor /= conversion;
            }
            let bbox = [from[0].min(to[0]), from[1].min(to[1]), from[0].max(to[0]), from[1].max(to[1])];
            if state.map_enabled {
                let definition = state.map_registration.clone().ok_or_else(|| tl!("Map registration needs control points.").to_string())?;
                let geo = match state.loaded_scale.as_ref().and_then(|s| s.geospatial.as_ref()) {
                    Some(old) => old.updated(definition, bbox),
                    None => measure::geo::GeoScale::new(definition, bbox),
                }
                .map_err(|e| e.to_string())?;
                scale = Scale::from_geo(geo).map_err(|e| e.to_string())?;
                scale.precision = state.precision;
            }
            Ok(match state.viewport {
                Some(index) => Edit::UpdateMeasurementViewport { page, index, bbox, name: state.name.clone(), scale },
                None => Edit::SetMeasurementScale { page, bbox, name: state.name.clone(), scale },
            })
        });
        match result {
            Ok(edit) => {
                state.error = None;
                state.calibration_ready = false;
                state.map_pick = None;
                state.scale_open = false;
                if active == Some(Tool::Calibrate) {
                    tool = Some(Tool::Distance);
                }
                view.pending_edit = Some(edit);
            }
            Err(error) => state.error = Some(error),
        }
    }
    if remove && let Some(viewport) = state.viewport {
        view.pending_edit = Some(Edit::RemoveMeasurementViewport { page, index: viewport });
        view.measure.viewport = None;
    }
    if let Some((page, annotation, reading, scale)) = select {
        view.measure.cancel();
        view.current = page;
        view.comments.selected = Some((page, annotation));
        view.measure.reading = Some(reading);
        view.measure.scale = Some(scale);
        app.quick_tool = QuickTool::Select;
    }
    if let Some(tool) = tool {
        view.measure.cancel();
        view.measure.scale_open = tool == Tool::Calibrate;
        app.quick_tool = QuickTool::Measure(tool);
    }
    if export {
        export_csv(app);
    }
    if let Some(annotation) = open_model {
        crate::three_d_ui::open(app, id, page, annotation);
    }
}

pub(crate) fn command(app: &mut PdfCraftApp, id: &str) {
    app.left = LeftPanel::Tool("measure");
    app.left_open = true;
    if id == "measure.3d"
        && let Some((index, doc)) = app.active_ids()
    {
        let page = app.views[index].current;
        match app.session.get(doc).ok_or_else(|| "no active document".to_string()).and_then(|d| d.three_d_models(page)) {
            Ok(models) => {
                if let Some(model) = models.first() {
                    crate::three_d_ui::open(app, doc, page, model.annotation);
                } else {
                    app.notify(tl!("This page has no 3D models."));
                }
            }
            Err(error) => app.notify_error(error),
        }
    }
    if let Some((index, _)) = app.active_ids()
        && id == "measure.scale"
    {
        app.views[index].measure.scale_open = true;
    }
    if let Some((index, _)) = app.active_ids()
        && let Some(tool) = id.strip_prefix("measure.").and_then(Tool::from_name)
    {
        app.views[index].measure.cancel();
        app.views[index].measure.scale_open = tool == Tool::Calibrate;
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
