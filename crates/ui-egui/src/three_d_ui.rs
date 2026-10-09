//! Embedded model measurement in real world coordinates. Heavy decoding and
//! preview rasterization use detached snapshots; stale jobs never replace edits.
use crate::{
    PdfCraftApp,
    theme::{self, Tokens},
};
use egui::{Color32, Pos2, Stroke};
use pdfcraft_engine::{
    DocId, Edit,
    measure::{
        three_d::{self, Kind, Measurement, Point, ViewState},
        three_d_camera::Camera,
        three_d_raster::{self, Options, Raster},
        three_d_snap::Mode as SnapMode,
    },
};
use std::sync::{
    Arc,
    mpsc::{self, Receiver, TryRecvError},
};
type Job<T> = Receiver<Result<T, String>>;
fn work<T: Send + 'static>(ctx: &egui::Context, inline: bool, task: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<Job<T>, String> {
    let (sender, receiver) = mpsc::channel();
    let ctx = ctx.clone();
    let run = move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task)).unwrap_or_else(|_| Err("3D operation failed safely".into()));
        let _ = sender.send(result);
        ctx.request_repaint();
    };
    if inline || cfg!(target_arch = "wasm32") {
        run();
    } else {
        std::thread::Builder::new().name("pdfcraft-3d".into()).spawn(run).map_err(|e| e.to_string())?;
    }
    Ok(receiver)
}
fn poll<T>(job: &Job<T>) -> Option<Result<T, String>> {
    match job.try_recv() {
        Ok(value) => Some(value),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(Err("3D worker stopped before completing".into())),
    }
}
#[derive(Clone, PartialEq)]
struct RenderKey {
    generation: Option<u64>,
    camera: Camera,
    size: [u32; 2],
    background: [u8; 3],
    surface: [u8; 3],
}
#[derive(Clone, PartialEq)]
struct PickKey {
    camera: Camera,
    pixel: [f64; 2],
    viewport: [f64; 2],
    generation: Option<u64>,
    mode: SnapMode,
}
struct Pick {
    key: PickKey,
    job: Job<Option<Point>>,
}
struct Load {
    generation: u64,
    view: Option<usize>,
    job: Job<ViewState>,
}
struct Render {
    key: RenderKey,
    job: Job<Raster>,
}
pub struct Viewer {
    pub doc: DocId,
    pub page: usize,
    pub annotation: usize,
    pub selected_view: Option<usize>,
    pub kind: Kind,
    pub snap: SnapMode,
    pub points: Vec<Point>,
    pub camera: Option<Camera>,
    loaded: Option<Arc<ViewState>>,
    generation: Option<u64>,
    load: Option<Load>,
    render: Option<Render>,
    pick: Option<Pick>,
    hover: Option<(PickKey, Option<Point>)>,
    click: Option<PickKey>,
    requested: Option<RenderKey>,
    displayed: Option<RenderKey>,
    texture: Option<egui::TextureHandle>,
    pub precision: u8,
    pub text_position: Option<Point>,
    pub text_size: f64,
    pub extension_length: f64,
    pub color: Point,
    pub placing_text: bool,
    pub flip_text_up: bool,
    pub label: String,
    pub user_text: String,
    pub unit: String,
    pub factor: f64,
    pub diameter: bool,
    pub show_circle: bool,
    pub selected_measurement: Option<usize>,
    pub error: Option<String>,
    saved_page: usize,
    window_rect: Option<egui::Rect>,
    keyboard_owned: bool,
}
impl Viewer {
    pub fn new(doc: DocId, page: usize, annotation: usize) -> Self {
        Self {
            doc,
            page,
            annotation,
            selected_view: None,
            kind: Kind::Linear,
            snap: SnapMode::Auto,
            points: Vec::new(),
            camera: None,
            loaded: None,
            generation: None,
            load: None,
            render: None,
            pick: None,
            hover: None,
            click: None,
            requested: None,
            displayed: None,
            texture: None,
            precision: 3,
            text_position: None,
            text_size: 12.,
            extension_length: 60.,
            color: [0., 0.47, 0.84],
            placing_text: false,
            flip_text_up: false,
            label: String::new(),
            user_text: String::new(),
            unit: String::new(),
            factor: 1.,
            diameter: false,
            show_circle: true,
            selected_measurement: None,
            error: None,
            saved_page: 0,
            window_rect: None,
            keyboard_owned: true,
        }
    }
    fn draft(&self, points: &[Point]) -> Result<Measurement, String> {
        let loaded = self.loaded.as_ref().ok_or("3D model is loading")?;
        let camera = self.camera.as_ref().ok_or("3D camera is loading")?;
        let units = three_d::Units { unit: self.unit.clone(), factor: self.factor, declared: true };
        let old = self.selected_measurement.and_then(|index| loaded.measurements.measurements.iter().find(|m| m.index == index));
        let mut measurement = if points.is_empty() && old.is_some() {
            let mut measurement = old.cloned().ok_or("selected 3D measurement no longer exists")?;
            if let three_d::Geometry::Radial { diameter, .. } = &mut measurement.geometry {
                *diameter = self.diameter;
            }
            let value = measurement.geometry.value().map_err(|e| e.to_string())?;
            measurement.value =
                if self.kind == Kind::Angular { if measurement.degrees { value.to_degrees() } else { value } } else { value * self.factor };
            if self.kind != Kind::Angular {
                measurement.unit = self.unit.clone();
            }
            measurement
        } else {
            Measurement::from_points(self.kind, points, &units, camera.axes()[2].map(|value| -value), self.diameter).map_err(|e| e.to_string())?
        };
        measurement.precision = self.precision;
        if self.flip_text_up {
            measurement.text_y = measurement.text_y.map(|value| -value);
        }
        measurement.text_size = self.text_size;
        measurement.extension_length = self.extension_length;
        measurement.color = self.color;
        if let Some(position) = self.text_position {
            measurement.text_position = position;
        }
        measurement.label = self.label.clone();
        measurement.user_text = self.user_text.clone();
        measurement.show_circle = self.show_circle;
        if let Some(index) = self.selected_measurement
            && let Some(old) = loaded.measurements.measurements.iter().find(|m| m.index == index)
        {
            measurement.source = old.source.clone();
        }
        measurement.dictionary().map_err(|e| e.to_string())?;
        Ok(measurement)
    }
    fn clear(&mut self) {
        self.points.clear();
        self.selected_measurement = None;
        self.error = None;
        self.click = None;
        self.text_position = None;
        self.placing_text = false;
        self.flip_text_up = false;
    }
}
pub(crate) fn open(app: &mut PdfCraftApp, doc: DocId, page: usize, annotation: usize) {
    if let Some((index, _)) = app.active_ids() {
        app.views[index].measure.cancel();
        app.views[index].auto_scroll.cancel();
    }
    app.quick_tool = crate::QuickTool::Select;
    app.three_d = Some(Viewer::new(doc, page, annotation));
}
/// Run before page shortcuts and annotation handlers. A model key must never
/// reach a selected page comment while this window owns the keyboard.
pub(crate) fn keys(app: &mut PdfCraftApp, ctx: &egui::Context) -> bool {
    if app.full_screen || app.dialog.is_some() || app.close_request.is_some() || app.palette_open {
        return false;
    }
    let active = app.active_ids().map(|(_, id)| id);
    let Some(viewer) = app.three_d.as_mut().filter(|v| Some(v.doc) == active) else { return false };
    if ctx.input(|i| i.pointer.any_pressed())
        && let Some(position) = ctx.input(|i| i.pointer.interact_pos())
        && let Some(rect) = viewer.window_rect
    {
        viewer.keyboard_owned = rect.contains(position);
    }
    if !viewer.keyboard_owned {
        return false;
    }
    if ctx.egui_wants_keyboard_input() {
        return true;
    }
    use egui::{Key, Modifiers};
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        viewer.clear();
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Backspace)) {
        viewer.points.pop();
        viewer.selected_measurement = None;
        viewer.click = None;
        viewer.text_position = None;
        viewer.placing_text = false;
    }
    let delete = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete));
    let target = delete
        .then_some(viewer.selected_measurement)
        .flatten()
        .filter(|_| viewer.load.is_none() && viewer.loaded.as_ref().is_some_and(|loaded| loaded.artwork.edit_error.is_none()))
        .map(|index| (viewer.doc, viewer.page, viewer.annotation, viewer.selected_view, index));
    if let Some((doc, page, annotation, view, index)) = target
        && app.session.get(doc).is_some_and(|d| d.allows_annotation())
        && app.apply_edit(Edit::RemoveThreeDMeasurement { page, annotation, view, index })
        && let Some(viewer) = app.three_d.as_mut()
    {
        viewer.clear();
    }
    true
}
enum Action {
    Save(Box<Measurement>),
    Delete(usize),
    Undo,
    Redo,
}
fn refresh(viewer: &mut Viewer, app: &PdfCraftApp, ctx: &egui::Context) {
    if let Some(load) = &viewer.load
        && let Some(result) = poll(&load.job)
    {
        let generation = load.generation;
        let selected = load.view;
        viewer.load = None;
        let current = app.session.get(viewer.doc).map(|d| d.edit_generation());
        if current == Some(generation) && selected == viewer.selected_view {
            match result {
                Ok(loaded) => {
                    if viewer.loaded.is_some() {
                        viewer.clear();
                    }
                    if viewer.loaded.is_none() {
                        viewer.unit = loaded.artwork.units.unit.clone();
                        viewer.factor = loaded.artwork.units.factor;
                    }
                    if viewer.camera.is_none() {
                        viewer.camera = Some(loaded.camera.clone());
                    }
                    viewer.loaded = Some(Arc::new(loaded));
                    viewer.requested = None;
                    viewer.error = None;
                }
                Err(error) => {
                    viewer.error = Some(error);
                    viewer.loaded = None;
                    viewer.camera = None;
                    viewer.texture = None;
                    viewer.displayed = None;
                }
            }
        } else {
            viewer.generation = None;
        }
    }
    let Some(doc) = app.session.get(viewer.doc) else { return };
    let generation = doc.edit_generation();
    if viewer.load.is_none() && viewer.generation != Some(generation) {
        viewer.generation = Some(generation);
        match doc.three_d_loader(viewer.page, viewer.annotation, viewer.selected_view).and_then(|task| work(ctx, app.run_inline, task)) {
            Ok(job) => viewer.load = Some(Load { generation, view: viewer.selected_view, job }),
            Err(error) => viewer.error = Some(error),
        }
    }
}
fn picking(viewer: &mut Viewer, ui: &egui::Ui, response: &egui::Response, rect: egui::Rect, inline: bool, allowed: bool, ready: bool) {
    if viewer.placing_text {
        viewer.click = None;
        viewer.hover = None;
        if ready
            && allowed
            && response.clicked_by(egui::PointerButton::Primary)
            && let Some(position) = response.hover_pos()
        {
            let result = viewer.draft(&viewer.points).and_then(|m| {
                viewer
                    .camera
                    .as_ref()
                    .ok_or_else(|| "3D camera is loading".to_owned())?
                    .plane_point(
                        [f64::from(position.x - rect.min.x), f64::from(position.y - rect.min.y)],
                        [f64::from(rect.width()), f64::from(rect.height())],
                        m.text_position,
                        m.plane,
                    )
                    .map_err(|e| e.to_string())
            });
            match result {
                Ok(point) => {
                    viewer.text_position = Some(point);
                    viewer.placing_text = false;
                    viewer.error = None;
                }
                Err(error) => viewer.error = Some(error),
            }
        }
        return;
    }
    let cursor = if ready && response.hovered() {
        response.hover_pos().and_then(|position| {
            viewer.camera.as_ref().map(|camera| PickKey {
                camera: camera.clone(),
                pixel: [f64::from(position.x - rect.min.x), f64::from(position.y - rect.min.y)],
                viewport: [f64::from(rect.width()), f64::from(rect.height())],
                generation: viewer.generation,
                mode: viewer.snap,
            })
        })
    } else {
        None
    };
    if response.clicked_by(egui::PointerButton::Primary) && allowed && ready && viewer.click.is_none() {
        viewer.click = cursor.clone();
    }
    if let Some(pick) = &viewer.pick
        && let Some(result) = poll(&pick.job)
    {
        let key = pick.key.clone();
        viewer.pick = None;
        if key.generation == viewer.generation {
            match result {
                Ok(point) => {
                    if viewer.click.as_ref() == Some(&key) {
                        viewer.click = None;
                        if let Some(point) = point {
                            let required = if viewer.kind == Kind::Linear { 2 } else { 3 };
                            if viewer.points.len() >= required {
                                viewer.clear();
                            }
                            viewer.points.push(point);
                            viewer.error = None;
                        } else {
                            viewer.error = Some(tl!("Choose a visible point on the model.").into());
                        }
                    }
                    viewer.hover = Some((key, point));
                }
                Err(error) => {
                    viewer.hover = Some((key.clone(), None));
                    viewer.error = Some(error);
                    if viewer.click.as_ref() == Some(&key) {
                        viewer.click = None;
                    }
                }
            }
        } else {
            viewer.click = None;
            viewer.hover = None;
        }
    }
    let requested = viewer.click.as_ref().or(cursor.as_ref());
    if viewer.pick.is_none()
        && let Some(key) = requested
        && (viewer.click.is_some() || viewer.hover.as_ref().is_none_or(|(previous, _)| previous != key))
        && let Some(loaded) = &viewer.loaded
    {
        let loaded = loaded.clone();
        let request = key.clone();
        let key = key.clone();
        match work(ui.ctx(), inline, move || {
            request
                .camera
                .snap(&loaded.artwork.scene, request.pixel, request.viewport, request.mode, 8.)
                .map(|hit| hit.map(|hit| hit.point))
                .map_err(|e| e.to_string())
        }) {
            Ok(job) => viewer.pick = Some(Pick { key, job }),
            Err(error) => viewer.error = Some(error),
        }
    }
    if cursor.is_none() && viewer.click.is_none() {
        viewer.hover = None;
    }
}
fn preview(viewer: &mut Viewer, ui: &mut egui::Ui, t: &Tokens, inline: bool, allowed: bool) {
    let outer_width = ui.available_width().clamp(160., 1400.);
    let height_limit = (ui.ctx().content_rect().height() * 0.4).clamp(200., 500.);
    let outer_height = (outer_width * 0.62).clamp(200., height_limit);
    let (_, outer) = ui.allocate_space(egui::vec2(outer_width, outer_height));
    ui.painter().rect_filled(outer, 6., t.card);
    let Some(camera) = viewer.camera.as_ref() else { return };
    // Preserve the PDF viewport's aspect ratio and projection binding. The
    // surrounding window can resize without stretching the actual model.
    let aspect = (camera.target[0] / camera.target[1]) as f32;
    let size = if aspect > outer_width / outer_height {
        egui::vec2(outer_width, outer_width / aspect)
    } else {
        egui::vec2(outer_height * aspect, outer_height)
    };
    if !size.x.is_finite() || !size.y.is_finite() || size.x < 1. || size.y < 1. {
        viewer.error = Some(tl!("The model viewport is too narrow to display.").into());
        return;
    }
    let rect = egui::Rect::from_center_size(outer.center(), size);
    let width = size.x;
    let height = size.y;
    let response = ui.interact(rect, ui.id().with("3d-canvas"), egui::Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Image, ui.is_enabled(), tl!("3D model canvas")));
    let Some(loaded) = viewer.loaded.clone() else { return };
    let key = RenderKey {
        generation: viewer.generation,
        camera: camera.clone(),
        size: [width.ceil() as u32, height.ceil() as u32],
        background: [t.card.r(), t.card.g(), t.card.b()],
        surface: if t.dark() { [124, 172, 209] } else { [92, 143, 184] },
    };
    if let Some(render) = &viewer.render
        && let Some(result) = poll(&render.job)
    {
        let completed = render.key.clone();
        viewer.render = None;
        if completed == key {
            match result {
                Ok(raster) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied([raster.width as usize, raster.height as usize], &raster.rgba);
                    viewer.texture = Some(ui.ctx().load_texture("3d-preview", image, egui::TextureOptions::LINEAR));
                    viewer.displayed = Some(completed);
                    viewer.error = None;
                }
                Err(error) => viewer.error = Some(error),
            }
        }
    }
    if viewer.render.is_none() && viewer.requested.as_ref() != Some(&key) {
        let loaded = loaded.clone();
        let camera = key.camera.clone();
        let options = Options { width: key.size[0], height: key.size[1], background: key.background, surface: key.surface };
        viewer.requested = Some(key.clone());
        match work(ui.ctx(), inline, move || three_d_raster::render(&loaded.artwork.scene, &camera, options).map_err(|e| e.to_string())) {
            Ok(job) => viewer.render = Some(Render { key: key.clone(), job }),
            Err(error) => viewer.error = Some(error),
        }
    }
    ui.painter().rect_filled(rect, 6., t.card);
    if let Some(texture) = &viewer.texture {
        ui.painter().image(texture.id(), rect, egui::Rect::from_min_max(Pos2::ZERO, egui::pos2(1., 1.)), Color32::WHITE);
    }
    let ready = viewer.displayed.as_ref() == Some(&key) && viewer.load.is_none();
    picking(viewer, ui, &response, rect, inline, allowed, ready);
    let displayed_camera = viewer.displayed.as_ref().map(|key| &key.camera);
    if let Some(camera) = displayed_camera {
        for measurement in loaded.measurements.measurements.iter().skip(viewer.saved_page.saturating_mul(20)).take(20) {
            overlay(ui, rect, camera, measurement, t, viewer.selected_measurement == Some(measurement.index));
        }
        let mut live_points = viewer.points.clone();
        let required = if viewer.kind == Kind::Linear { 2 } else { 3 };
        if !live_points.is_empty()
            && live_points.len() < required
            && let Some((hover, Some(point))) = &viewer.hover
            && hover.camera == key.camera
            && hover.generation == viewer.generation
        {
            live_points.push(*point);
        }
        if let Ok(mut measurement) = viewer.draft(&live_points) {
            if viewer.placing_text
                && let Some(position) = response.hover_pos()
                && let Ok(point) = camera.plane_point(
                    [f64::from(position.x - rect.min.x), f64::from(position.y - rect.min.y)],
                    [f64::from(rect.width()), f64::from(rect.height())],
                    measurement.text_position,
                    measurement.plane,
                )
            {
                measurement.text_position = point;
            }
            overlay(ui, rect, camera, &measurement, t, true);
        }
        for point in &viewer.points {
            if let Ok(Some(p)) = camera.project(*point, [f64::from(rect.width()), f64::from(rect.height())]) {
                let position = rect.min + egui::vec2(p[0] as f32, p[1] as f32);
                ui.painter().circle_filled(position, 6., Color32::BLACK);
                ui.painter().circle_filled(position, 5., Color32::WHITE);
                ui.painter().circle_filled(position, 3.5, t.accent);
            }
        }
    }
    if !ready {
        ui.painter().text(rect.left_top() + egui::vec2(10., 10.), egui::Align2::LEFT_TOP, tl!("Updating 3D view…"), theme::regular(12.), t.text);
    }
    let delta = ui.input(|i| i.pointer.delta());
    let mut operation = Ok(());
    if response.dragged_by(egui::PointerButton::Primary) && !viewer.placing_text {
        if let Some(camera) = &mut viewer.camera {
            operation = camera.orbit(f64::from(-delta.x) * 0.008, f64::from(delta.y) * 0.008);
        }
    } else if response.dragged_by(egui::PointerButton::Middle) || response.dragged_by(egui::PointerButton::Secondary) {
        if let Some(camera) = &mut viewer.camera {
            operation = camera.pan([f64::from(delta.x), f64::from(delta.y)], [f64::from(width), f64::from(height)]);
        }
    } else if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.
            && let Some(camera) = &mut viewer.camera
        {
            operation = camera.zoom((f64::from(scroll) * 0.003).exp().clamp(0.1, 10.));
        }
    }
    if let Err(error) = operation {
        viewer.error = Some(error.to_string());
    }
}
/// Galley meshes hold font-atlas texel coordinates. Convert them after all UI
/// text has been laid out, because later labels may grow the atlas this pass.
#[derive(Default)]
struct CaptionMeshes {
    pending: Vec<Arc<egui::epaint::Mesh>>,
}
impl egui::Plugin for CaptionMeshes {
    fn debug_name(&self) -> &'static str {
        "pdfcraft-model-captions"
    }
    fn output_hook(&mut self, ctx: &egui::Context, output: &mut egui::FullOutput) {
        let atlas = ctx.fonts(|fonts| fonts.font_image_size());
        let pending = std::mem::take(&mut self.pending);
        let ids = pending.iter().map(|mesh| Arc::as_ptr(mesh) as usize).collect::<std::collections::HashSet<_>>();
        for shape in &mut output.shapes {
            normalize_caption_uv(&mut shape.shape, atlas, &ids);
        }
    }
}
fn normalize_caption_uv(shape: &mut egui::Shape, atlas: [usize; 2], ids: &std::collections::HashSet<usize>) {
    match shape {
        egui::Shape::Mesh(mesh) if ids.contains(&(Arc::as_ptr(mesh) as usize)) => {
            for vertex in &mut Arc::make_mut(mesh).vertices {
                vertex.uv.x /= atlas[0].max(1) as f32;
                vertex.uv.y /= atlas[1].max(1) as f32;
            }
        }
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                normalize_caption_uv(shape, atlas, ids);
            }
        }
        _ => {}
    }
}
fn markup_srgb(color: Point) -> [u8; 3] {
    color.map(|value| (value * 255.).round() as u8)
}
fn overlay(ui: &egui::Ui, rect: egui::Rect, camera: &Camera, measurement: &Measurement, t: &Tokens, selected: bool) {
    let painter = ui.painter().with_clip_rect(rect);
    let viewport = [f64::from(rect.width()), f64::from(rect.height())];
    let [red, green, blue] = markup_srgb(measurement.color);
    let color = Color32::from_rgb(red, green, blue);
    let project = |point| camera.project(point, viewport).ok().flatten().map(|p| rect.min + egui::vec2(p[0] as f32, p[1] as f32));
    if let Ok(lines) = measurement.segments() {
        for [a, b] in lines {
            if let (Some(a), Some(b)) = (project(a), project(b)) {
                // The casing stays visible on either dark or light model faces.
                painter.line_segment([a, b], Stroke::new(if selected { 6. } else { 5. }, Color32::BLACK));
                painter.line_segment([a, b], Stroke::new(if selected { 4. } else { 3.5 }, Color32::WHITE));
                painter.line_segment([a, b], Stroke::new(if selected { 2. } else { 1.5 }, color));
            }
        }
    }
    let galley = painter.layout_no_wrap(measurement.caption(), theme::semibold(measurement.text_size as f32), color);
    if let Ok(Some(frame)) = camera.caption_frame(measurement, viewport, f64::from(galley.size().y)) {
        let screen = |right: f32, up: f32| {
            let p = frame.point(f64::from(right), f64::from(up));
            rect.min + egui::vec2(p[0] as f32, p[1] as f32)
        };
        // Preserve the declared markup colour while choosing a readable backplate.
        let linear = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        let luminance = 0.2126 * linear(measurement.color[0]) + 0.7152 * linear(measurement.color[1]) + 0.0722 * linear(measurement.color[2]);
        let background = if luminance > 0.179 { Color32::BLACK } else { Color32::WHITE };
        painter.add(egui::Shape::convex_polygon(
            vec![
                screen(-5., -5.),
                screen(galley.size().x + 5., -5.),
                screen(galley.size().x + 5., galley.size().y + 5.),
                screen(-5., galley.size().y + 5.),
            ],
            background,
            Stroke::new(if selected { 2. } else { 1. }, if selected { t.accent } else { t.border }),
        ));
        ui.ctx().add_plugin(CaptionMeshes::default());
        for row in &galley.rows {
            let mut mesh = row.visuals.mesh.clone();
            for vertex in &mut mesh.vertices {
                let position = row.pos + vertex.pos.to_vec2();
                vertex.pos = screen(position.x - galley.rect.min.x, galley.rect.max.y - position.y);
            }
            let mesh = Arc::new(mesh);
            ui.ctx().with_plugin(|plugin: &mut CaptionMeshes| plugin.pending.push(mesh.clone()));
            painter.add(egui::Shape::Mesh(mesh));
        }
        if measurement.geometry.kind() == Kind::Radial
            && let Some(start) = project(measurement.text_position)
        {
            let end = frame.point(0., f64::from(galley.size().y) * 0.5);
            let end = rect.min + egui::vec2(end[0] as f32, end[1] as f32);
            painter.line_segment([start, end], Stroke::new(5., Color32::BLACK));
            painter.line_segment([start, end], Stroke::new(3.5, Color32::WHITE));
            painter.line_segment([start, end], Stroke::new(1.5, color));
        }
    }
}

fn text_field(ui: &mut egui::Ui, t: &Tokens, value: &mut String, width: f32, limit: usize) -> egui::Response {
    egui::Frame::new()
        .fill(t.field)
        .stroke(Stroke::new(1., t.border))
        .corner_radius(4.)
        .inner_margin(egui::Margin::symmetric(5, 4))
        .show(ui, |ui| ui.add(egui::TextEdit::singleline(value).frame(egui::Frame::NONE).char_limit(limit).desired_width(width)))
        .inner
}
pub(crate) fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(mut viewer) = app.three_d.take() else { return };
    if app.active_ids().map(|(_, id)| id) != Some(viewer.doc) {
        if app.session.get(viewer.doc).is_some() {
            app.three_d = Some(viewer);
        }
        return;
    }
    refresh(&mut viewer, app, ctx);
    let t = Tokens::get(ctx);
    let document_allowed = app.session.get(viewer.doc).is_some_and(|d| d.allows_annotation());
    let allowed = document_allowed && viewer.loaded.as_ref().is_none_or(|loaded| loaded.artwork.edit_error.is_none());
    let blocked = app.dialog.is_some() || app.close_request.is_some() || app.palette_open;
    let mut open = true;
    let mut action = None;
    let mut select_saved = None;
    let window = egui::Window::new(tl!("Measure 3D model"))
        .id(egui::Id::new("3d-measure-window"))
        .open(&mut open)
        .default_size(egui::vec2(820., 760.))
        .min_size(egui::vec2(380., 400.))
        .vscroll(true)
        .show(ctx, |ui| {
            if blocked {
                ui.disable();
            }
            ui.horizontal_wrapped(|ui| {
                if let Some(loaded) = &viewer.loaded {
                    let shown = viewer
                        .selected_view
                        .and_then(|i| loaded.artwork.views.get(i))
                        .or(loaded.artwork.default_view.as_ref())
                        .map(|v| v.name.clone())
                        .unwrap_or_else(|| tl!("Default view").into());
                    let previous = viewer.selected_view;
                    egui::ComboBox::from_id_salt("3d-view").selected_text(shown).show_ui(ui, |ui| {
                        ui.selectable_value(&mut viewer.selected_view, None, tl!("Default view"));
                        for (i, view) in loaded.artwork.views.iter().enumerate() {
                            ui.selectable_value(&mut viewer.selected_view, Some(i), &view.name);
                        }
                    });
                    if previous != viewer.selected_view {
                        viewer.camera = None;
                        viewer.generation = None;
                        viewer.clear();
                    }
                }
                if ui.button(tl!("Fit model")).clicked()
                    && let Some(loaded) = &viewer.loaded
                {
                    match Camera::fit(&loaded.artwork.scene, viewer.camera.as_ref().map(|c| c.target).unwrap_or([1., 1.])) {
                        Ok(camera) => {
                            viewer.camera = Some(camera);
                            viewer.error = None;
                        }
                        Err(error) => viewer.error = Some(error.to_string()),
                    }
                }
                if ui.button(tl!("Undo")).clicked() {
                    action = Some(Action::Undo);
                }
                if ui.button(tl!("Redo")).clicked() {
                    action = Some(Action::Redo);
                }
            });
            ui.label(
                egui::RichText::new(tl!("Drag to orbit. Right or middle drag pans. Scroll zooms. Click the model to measure."))
                    .small()
                    .color(t.text_muted),
            );
            preview(&mut viewer, ui, &t, app.run_inline, allowed && !blocked);
            ui.horizontal_wrapped(|ui| {
                for (kind, label) in [
                    (Kind::Linear, "Distance"),
                    (Kind::Perpendicular, "Perpendicular"),
                    (Kind::Angular, "Angle"),
                    (Kind::Radial, "Radius / diameter"),
                ] {
                    if ui.selectable_value(&mut viewer.kind, kind, tl!(label)).changed() {
                        viewer.clear();
                    }
                }
                if ui.button(tl!("Clear points")).clicked() {
                    viewer.clear();
                }
            });
            let instruction = match viewer.kind {
                Kind::Linear => "Choose the two endpoints.",
                Kind::Perpendicular => "Choose a point, then the two endpoints of a reference line.",
                Kind::Angular => "Choose an endpoint, the angle vertex, then the other endpoint.",
                Kind::Radial => "Choose three points on the circle.",
            };
            ui.label(tl!(instruction));
            ui.horizontal_wrapped(|ui| {
                ui.label(tl!("Snap"));
                for (mode, label) in [
                    (SnapMode::Auto, "Automatic"),
                    (SnapMode::Vertex, "Vertex snapping"),
                    (SnapMode::Edge, "Edge snapping"),
                    (SnapMode::Surface, "Surfaces"),
                    (SnapMode::Silhouette, "Silhouettes"),
                ] {
                    if ui.selectable_value(&mut viewer.snap, mode, tl!(label)).changed() {
                        viewer.hover = None;
                        viewer.click = None;
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                if viewer.kind == Kind::Radial {
                    ui.checkbox(&mut viewer.diameter, tl!("Diameter"));
                    ui.checkbox(&mut viewer.show_circle, tl!("Show circle"));
                    let label = ui.label(tl!("Extension length (points)"));
                    ui.add(egui::DragValue::new(&mut viewer.extension_length).range(0.0..=4096.0).clamp_existing_to_range(false))
                        .labelled_by(label.id);
                }
                let label = ui.label(tl!("Precision"));
                ui.add(egui::DragValue::new(&mut viewer.precision).range(0..=12)).labelled_by(label.id);
                if viewer.kind != Kind::Angular {
                    let label = ui.label(tl!("Unit"));
                    text_field(ui, &t, &mut viewer.unit, 100., 24).labelled_by(label.id);
                    let label = ui.label(tl!("Units per model unit"));
                    ui.add(egui::DragValue::new(&mut viewer.factor).range(1e-12..=1e12).clamp_existing_to_range(false).speed(0.01).max_decimals(12))
                        .labelled_by(label.id);
                }
            });
            ui.horizontal_wrapped(|ui| {
                let label = ui.label(tl!("Label"));
                text_field(ui, &t, &mut viewer.label, 180., 128).labelled_by(label.id);
                let label = ui.label(tl!("Markup text"));
                text_field(ui, &t, &mut viewer.user_text, 180., 128).labelled_by(label.id);
            });
            let draft = viewer.draft(&viewer.points);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        allowed && viewer.load.is_none() && draft.is_ok(),
                        egui::Button::new(if viewer.placing_text { tl!("Cancel caption placement") } else { tl!("Place caption") }),
                    )
                    .clicked()
                {
                    viewer.placing_text = !viewer.placing_text;
                    viewer.click = None;
                    viewer.hover = None;
                }
                if ui.add_enabled(viewer.text_position.is_some(), egui::Button::new(tl!("Reset caption"))).clicked() {
                    viewer.text_position = None;
                    viewer.placing_text = false;
                }
                if ui.add_enabled(draft.is_ok(), egui::Button::new(tl!("Flip text up"))).clicked() {
                    viewer.flip_text_up = !viewer.flip_text_up;
                }
                let label = ui.label(tl!("Text height (points)"));
                ui.add(egui::DragValue::new(&mut viewer.text_size).range(1. ..=256.).clamp_existing_to_range(false)).labelled_by(label.id);
                let label = ui.label(tl!("Color"));
                let mut color = markup_srgb(viewer.color);
                if ui.color_edit_button_srgb(&mut color).labelled_by(label.id).changed() {
                    viewer.color = color.map(|value| f64::from(value) / 255.);
                }
            });
            if viewer.placing_text {
                ui.label(tl!("Click the preview to place the caption on its measurement plane."));
            }
            if let Ok(measurement) = &draft {
                ui.label(egui::RichText::new(measurement.caption()).font(theme::semibold(22.)).color(t.text));
            } else if !viewer.points.is_empty() {
                let mut live = viewer.points.clone();
                if let Some((hover, Some(point))) = &viewer.hover
                    && viewer.camera.as_ref() == Some(&hover.camera)
                {
                    live.push(*point);
                }
                if let Ok(measurement) = viewer.draft(&live) {
                    ui.label(egui::RichText::new(measurement.caption()).font(theme::semibold(22.)).color(t.text));
                }
                ui.label(crate::i18n::fmt(tl!("Selected points: {n}"), &[("n", &viewer.points.len().to_string())]));
            }
            let required = if viewer.kind == Kind::Linear { 2 } else { 3 };
            if viewer.points.len() == required
                && let Err(error) = &draft
            {
                ui.label(error);
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        allowed && viewer.load.is_none() && draft.is_ok(),
                        egui::Button::new(if viewer.selected_measurement.is_some() { tl!("Update measurement") } else { tl!("Save measurement") }),
                    )
                    .clicked()
                    && let Ok(measurement) = draft
                {
                    action = Some(Action::Save(Box::new(measurement)));
                }
                if !document_allowed {
                    ui.label(tl!("This document does not allow measurement edits."));
                }
            });
            if let Some(error) = viewer.loaded.as_ref().and_then(|loaded| loaded.artwork.edit_error.as_ref()) {
                ui.label(error);
            }
            if let Some(error) = &viewer.error {
                ui.label(egui::RichText::new(error).color(t.text));
                if ui.button(tl!("Retry")).clicked() {
                    viewer.generation = None;
                    viewer.requested = None;
                }
            }
            ui.separator();
            ui.label(egui::RichText::new(tl!("Saved measurements")).strong());
            if let Some(loaded) = &viewer.loaded {
                if loaded.measurements.measurements.is_empty() {
                    ui.label(tl!("No saved measurements in this view."));
                }
                let pages = loaded.measurements.measurements.len().div_ceil(20).max(1);
                viewer.saved_page = viewer.saved_page.min(pages - 1);
                for measurement in loaded.measurements.measurements.iter().skip(viewer.saved_page.saturating_mul(20)).take(20) {
                    ui.push_id(measurement.index, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(measurement.caption());
                            if ui.add_enabled(allowed && viewer.load.is_none(), egui::Button::new(tl!("Edit measurement"))).clicked() {
                                select_saved = Some(measurement.clone());
                            }
                            if ui.add_enabled(allowed && viewer.load.is_none(), egui::Button::new(tl!("Delete measurement"))).clicked() {
                                action = Some(Action::Delete(measurement.index));
                            }
                        });
                    });
                }
                if pages > 1 {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(viewer.saved_page > 0, egui::Button::new(tl!("Previous"))).clicked() {
                            viewer.saved_page = viewer.saved_page.saturating_sub(1);
                        }
                        ui.label(format!("{} / {}", viewer.saved_page + 1, pages));
                        if ui.add_enabled(viewer.saved_page + 1 < pages, egui::Button::new(tl!("Next"))).clicked() {
                            viewer.saved_page += 1;
                        }
                    });
                }
                if !loaded.measurements.unsupported.is_empty() {
                    ui.collapsing(tl!("Unsupported measurements"), |ui| {
                        for item in &loaded.measurements.unsupported {
                            ui.label(&item.reason);
                        }
                    });
                }
            }
        });
    if let Some(window) = window {
        viewer.window_rect = Some(window.response.rect);
    }
    if let Some(measurement) = select_saved {
        viewer.clear();
        viewer.selected_measurement = Some(measurement.index);
        viewer.kind = measurement.geometry.kind();
        viewer.precision = measurement.precision;
        viewer.text_size = measurement.text_size;
        viewer.extension_length = measurement.extension_length;
        viewer.color = measurement.color;
        viewer.unit = measurement.unit.clone();
        if viewer.kind != Kind::Angular {
            viewer.factor = measurement.value / measurement.geometry.value().unwrap_or(1.);
        }
        viewer.label = measurement.label;
        viewer.user_text = measurement.user_text;
        viewer.show_circle = measurement.show_circle;
        if let three_d::Geometry::Radial { diameter, .. } = measurement.geometry {
            viewer.diameter = diameter;
        }
    }
    match action {
        Some(Action::Save(measurement)) => {
            let new = three_d::NewMeasurement {
                page: viewer.page,
                annotation: viewer.annotation,
                view: viewer.selected_view,
                measurement: *measurement,
                camera: viewer.camera.clone(),
            };
            let edit = match viewer.selected_measurement {
                Some(index) => Edit::UpdateThreeDMeasurement { measurement: new, index },
                None => Edit::AddThreeDMeasurement(new),
            };
            if app.apply_edit(edit) {
                viewer.clear();
            }
        }
        Some(Action::Delete(index)) => {
            if app.apply_edit(Edit::RemoveThreeDMeasurement { page: viewer.page, annotation: viewer.annotation, view: viewer.selected_view, index }) {
                viewer.clear();
            }
        }
        Some(Action::Undo) => {
            app.undo();
            viewer.clear();
        }
        Some(Action::Redo) => {
            app.redo();
            viewer.clear();
        }
        None => {}
    }
    if open {
        app.three_d = Some(viewer);
    }
}

#[cfg(test)]
mod caption_mesh_tests {
    use super::*;
    #[test]
    fn picker_and_markup_use_the_same_srgb_components() {
        assert_eq!(markup_srgb([0., 0.47, 0.84]), [0, 120, 214]);
        assert_eq!(markup_srgb([1., 0.5, 0.]), [255, 128, 0]);
    }
    #[test]
    fn final_atlas_dimensions_normalize_only_owned_caption_meshes() {
        let mesh = |uv| {
            Arc::new(egui::epaint::Mesh {
                vertices: vec![egui::epaint::Vertex { pos: Pos2::new(17., 29.), uv, color: Color32::RED }],
                ..Default::default()
            })
        };
        let caption = mesh(Pos2::new(128., 64.));
        let unrelated = mesh(Pos2::new(0.5, 0.25));
        let ids = std::collections::HashSet::from([Arc::as_ptr(&caption) as usize]);
        let mut shapes = egui::Shape::Vec(vec![egui::Shape::Mesh(caption), egui::Shape::Mesh(unrelated.clone())]);
        normalize_caption_uv(&mut shapes, [512, 256], &ids);
        let egui::Shape::Vec(shapes) = shapes else { panic!("shape container") };
        let egui::Shape::Mesh(caption) = &shapes[0] else { panic!("caption mesh") };
        assert_eq!(caption.vertices[0].uv, Pos2::new(0.25, 0.25));
        assert_eq!(caption.vertices[0].pos, Pos2::new(17., 29.));
        assert_eq!(caption.vertices[0].color, Color32::RED);
        let egui::Shape::Mesh(actual) = &shapes[1] else { panic!("other mesh") };
        assert!(Arc::ptr_eq(actual, &unrelated));
        assert_eq!(actual.vertices[0].uv, Pos2::new(0.5, 0.25));
    }
}
