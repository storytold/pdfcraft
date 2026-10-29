use super::{ACCENT, Color32, CornerRadius, DocInfo, DocView, Edit, MAX_OUTLINES, PagePointer, PageXform, Pos2, Rect, Stroke, user_box};
use crate::page_cursor::Layer;

/// A selected page image, and what the pointer is doing to it.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageSelection {
    pub page: usize,
    pub index: usize,
    /// Dragging: the start point and, for a corner, the opposite corner (screen).
    drag: Option<(Pos2, Option<Pos2>)>,
    /// Crop mode (Acrobat's crop handles): the box kept so far.
    pub crop: Option<CropMode>,
    /// The drag is turning the picture by its rotate handle (the `drag` start is then unused).
    turning: bool,
    /// A move or resize applied on release: the frame it had and the one it got, in the page's
    /// 0..1 coordinates, while the page raster still has the picture where it was.
    settled: Option<(Rect, Rect)>,
}

impl ImageSelection {
    /// Cancel an in-progress text, rotation or crop drag and restore the crop it was previewing.
    pub(crate) fn cancel_pointer_gesture(&mut self) {
        self.drag = None;
        self.turning = false;
        if let Some(crop) = &mut self.crop
            && let Some((_, _, before)) = crop.drag.take()
        {
            crop.keep = before;
        }
    }
}

/// Cropping a page image: the handles trim instead of resizing, the part that will go is
/// dimmed, Enter or Done applies and Esc or Cancel leaves it.
#[derive(Clone, Debug, PartialEq)]
pub struct CropMode {
    /// The box to keep (user space).
    pub keep: [f64; 4],
    /// A drag in progress: the grip (0–3 corners, 4–7 edges, 8 the inside), where it started
    /// and the box then.
    drag: Option<(usize, Pos2, [f64; 4])>,
}

impl CropMode {
    pub fn new(keep: [f64; 4]) -> Self {
        Self { keep, drag: None }
    }
}

/// What the pointer and the page are, for the image tools on one page.
#[derive(Clone, Copy)]
pub(crate) struct ImageCx<'a> {
    pub ui: &'a egui::Ui,
    pub resp: &'a egui::Response,
    pub xf: &'a PageXform,
    pub ptr: &'a PagePointer,
    pub page: usize,
    pub info: &'a DocInfo,
    pub allow_interaction: bool,
}

/// The screen boxes of the crop's eight handles: corners, then the top, right, bottom and left
/// edge midpoints.
fn crop_handles(k: Rect) -> [Pos2; 8] {
    [k.left_top(), k.right_top(), k.left_bottom(), k.right_bottom(), k.center_top(), k.right_center(), k.center_bottom(), k.left_center()]
}

/// Crop mode for the selected image `i` whose visible frame is `b` (screen). Returns `true`
/// (the pointer and keys are the crop's while it lasts).
fn crop_input(cx: &ImageCx<'_>, view: &mut DocView, i: usize, b: Rect) -> bool {
    let ImageCx { ui, resp, xf, ptr, page, info, allow_interaction } = *cx;
    let painter = ui.painter();
    let Some(mode) = view.image_selection.as_mut().and_then(|s| s.crop.as_mut()) else { return false };
    let hover_pos = ptr.pos;
    let keep_screen = |keep: [f64; 4]| xf.user_rect(info, page, keep.map(|v| v as f32)).intersect(b);
    // A drag in progress moves a grip; the box stays inside the picture and at least 6 px.
    if allow_interaction
        && resp.drag_started()
        && let Some(o) = ptr.origin
    {
        let k = keep_screen(mode.keep);
        let grip = crop_handles(k).iter().position(|h| h.distance(o) < 8.0).or_else(|| k.contains(o).then_some(8));
        if let Some(g) = grip {
            mode.drag = Some((g, o, mode.keep));
        }
    }
    if let Some((grip, start, at_start)) = mode.drag
        && let Some(p) = hover_pos
    {
        let d = p - start;
        let k0 = keep_screen(at_start);
        let (mut min, mut max) = (k0.min, k0.max);
        match grip {
            0 => {
                min.x += d.x;
                min.y += d.y;
            }
            1 => {
                max.x += d.x;
                min.y += d.y;
            }
            2 => {
                min.x += d.x;
                max.y += d.y;
            }
            3 => {
                max.x += d.x;
                max.y += d.y;
            }
            4 => min.y += d.y,
            5 => max.x += d.x,
            6 => max.y += d.y,
            7 => min.x += d.x,
            _ => {
                let shift = egui::vec2(d.x.clamp(b.min.x - k0.min.x, b.max.x - k0.max.x), d.y.clamp(b.min.y - k0.min.y, b.max.y - k0.max.y));
                min += shift;
                max += shift;
            }
        }
        let (min_w, min_h) = (6.0_f32.min(b.width()), 6.0_f32.min(b.height()));
        min.x = min.x.clamp(b.min.x, b.max.x - min_w);
        min.y = min.y.clamp(b.min.y, b.max.y - min_h);
        max.x = max.x.clamp(min.x + min_w, b.max.x);
        max.y = max.y.clamp(min.y + min_h, b.max.y);
        mode.keep = user_box(xf, info, page, Rect::from_min_max(min, max));
        if resp.drag_stopped() || !ptr.primary_down {
            mode.drag = None;
        }
    }
    let k = keep_screen(mode.keep);
    // What goes is dimmed; what stays keeps its frame and handles.
    let dim = Color32::from_black_alpha(96);
    for r in [
        Rect::from_min_max(b.min, Pos2::new(b.max.x, k.min.y)),
        Rect::from_min_max(Pos2::new(b.min.x, k.max.y), b.max),
        Rect::from_min_max(Pos2::new(b.min.x, k.min.y), Pos2::new(k.min.x, k.max.y)),
        Rect::from_min_max(Pos2::new(k.max.x, k.min.y), Pos2::new(b.max.x, k.max.y)),
    ] {
        if r.is_positive() {
            painter.rect_filled(r.intersect(xf.rect), CornerRadius::ZERO, dim);
        }
    }
    painter.rect_stroke(k, CornerRadius::ZERO, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
    for h in crop_handles(k) {
        painter.rect(
            Rect::from_center_size(h, egui::vec2(8.0, 8.0)),
            CornerRadius::ZERO,
            Color32::WHITE,
            Stroke::new(1.0, ACCENT),
            egui::StrokeKind::Middle,
        );
    }
    // Done and Cancel, painted under the box (above it near the bottom of the view).
    let font = egui::FontId::proportional(12.5);
    let pill = |label: &str, right: f32, y: f32| -> Rect {
        let w = painter.layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE).size().x + 20.0;
        Rect::from_min_size(Pos2::new(right - w, y), egui::vec2(w, 22.0))
    };
    let y = if k.max.y + 30.0 < xf.rect.max.y { k.max.y + 6.0 } else { k.min.y - 28.0 };
    let done = pill(tl!("Done"), k.max.x, y);
    let cancel = pill(tl!("Cancel"), done.min.x - 6.0, y);
    let t = crate::theme::Tokens::get(ui.ctx());
    painter.rect_filled(done, CornerRadius::same(11), ACCENT);
    painter.text(done.center(), egui::Align2::CENTER_CENTER, tl!("Done"), font.clone(), Color32::WHITE);
    painter.rect(cancel, CornerRadius::same(11), t.panel, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    painter.text(cancel.center(), egui::Align2::CENTER_CENTER, tl!("Cancel"), font, t.text);
    // Cropping takes over its page's cursor, and the buttons' wherever they are; other pages
    // keep theirs.
    if let Some(p) = hover_pos.filter(|p| xf.rect.contains(*p) || done.contains(*p) || cancel.contains(*p)) {
        let icon = if let Some(g) = crop_handles(k).iter().position(|h| h.distance(p) < 8.0) {
            match g {
                0 | 3 => egui::CursorIcon::ResizeNwSe,
                1 | 2 => egui::CursorIcon::ResizeNeSw,
                4 | 6 => egui::CursorIcon::ResizeVertical,
                _ => egui::CursorIcon::ResizeHorizontal,
            }
        } else if done.contains(p) || cancel.contains(p) {
            egui::CursorIcon::PointingHand
        } else if k.contains(p) {
            egui::CursorIcon::Move
        } else {
            egui::CursorIcon::Default
        };
        view.cursor.claim(Layer::Editor, icon);
    }
    let clicked = |r: Rect| allow_interaction && resp.clicked() && hover_pos.is_some_and(|p| r.contains(p));
    let keys = |key: egui::Key| allow_interaction && ui.input(|inp| inp.key_pressed(key)) && !ui.ctx().egui_wants_keyboard_input();
    if clicked(done) || keys(egui::Key::Enter) {
        let keep = mode.keep;
        if let Some(s) = view.image_selection.as_mut() {
            s.crop = None;
        }
        if keep_screen(keep) != b {
            view.pending_edit = Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::Crop(keep) });
        }
    } else if (clicked(cancel) || keys(egui::Key::Escape))
        && let Some(s) = view.image_selection.as_mut()
    {
        s.crop = None;
    }
    true
}

impl ImageSelection {
    pub(crate) fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

/// The page raster's picture of the box `from`, stretched into `to`, over blanks at `blank` (where
/// the screen may still show the picture).
fn stretched_picture(painter: &egui::Painter, xf: &PageXform, tex: egui::TextureId, blank: &[Rect], from: Rect, to: Rect) {
    let clipped = painter.with_clip_rect(painter.clip_rect().intersect(xf.rect));
    for r in blank {
        clipped.rect_filled(*r, CornerRadius::ZERO, Color32::WHITE);
    }
    let mut mesh = egui::Mesh::with_texture(tex);
    for (a, b) in [
        (from.left_top(), to.left_top()),
        (from.right_top(), to.right_top()),
        (from.right_bottom(), to.right_bottom()),
        (from.left_bottom(), to.left_bottom()),
    ] {
        let (u, v) = xf.screen_to_norm(a);
        mesh.vertices.push(egui::epaint::Vertex { pos: b, uv: Pos2::new(u, v), color: Color32::WHITE });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    clipped.add(egui::Shape::mesh(mesh));
}

/// Once a selected corner or the rotate knob is grabbed, text under it cannot take over the gesture. The same
/// applies after the pointer leaves the image/page while dragging.
pub(crate) fn image_owns_pointer(ui: &egui::Ui, xf: &PageXform, ptr: &PagePointer, page: usize, info: &DocInfo, view: &DocView) -> bool {
    let Some(selection) = view.image_selection.as_ref().filter(|s| s.page == page) else { return false };
    if selection.drag.is_some() {
        return true;
    }
    let Some(image) = view.edit_images.get(&page).and_then(|(_, images)| images.get(selection.index)) else { return false };
    let Some(rect) = image.visible_rect else { return false };
    let b = xf.user_rect(info, page, rect.map(|v| v as f32));
    let p = if ptr.primary_down { ptr.origin } else { ptr.pos };
    let knob = rotate_knob(b, ui.clip_rect()).0;
    p.is_some_and(|p| [b.left_top(), b.right_top(), b.left_bottom(), b.right_bottom(), knob].iter().any(|c| c.distance(p) < 8.0))
}

/// What a right-click on a selected image asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ImageAction {
    Replace(usize, usize),
    Save(usize, usize),
    Copy(usize, usize),
    CopyFile(usize, usize),
    /// Copy the picture, then remove it from the page.
    Cut(usize, usize),
    /// Mark the picture for redaction (applied later from Redact a PDF).
    Redact(usize, usize),
    /// Create a link over the picture.
    Link(usize, usize),
    /// Open Recognize Text for the picture's page.
    Recognize(usize),
}

/// Where the rotate knob sits, and the edge point its stem starts from: above the middle of the
/// top edge, or below the bottom edge when the area on screen (`view`) has no room above.
pub(super) fn rotate_knob(b: Rect, view: Rect) -> (Pos2, Pos2) {
    let above = b.center_top() - egui::vec2(0.0, 22.0);
    if above.y - 6.0 >= view.top() { (above, b.center_top()) } else { (b.center_bottom() + egui::vec2(0.0, 22.0), b.center_bottom()) }
}

/// Degrees clockwise (-180..=180) that turn "straight up" from `centre` onto the pointer; `snap`
/// rounds to steps of 15 degrees (Shift). A pointer on the centre turns nothing.
pub(super) fn turn_degrees(centre: Pos2, pointer: Pos2, snap: bool) -> f32 {
    let v = pointer - centre;
    if !(v.x.is_finite() && v.y.is_finite()) || v.length() < 4.0 {
        return 0.0;
    }
    // Screen y points down: up is (0, -1); clockwise is atan2(x, -y).
    let d = v.x.atan2(-v.y).to_degrees();
    if snap { (d / 15.0).round() * 15.0 } else { d }
}

/// Which boxes are outlined on the page, and why.
struct OutlineState {
    selected: Option<usize>,
    hovered_box: Option<usize>,
    /// The picture being dragged, whose outline would trail behind until the release.
    moving: Option<usize>,
    dragging: bool,
    /// Few images: faint outlines for all of them.
    faint_all: bool,
}

/// Outlines the images: all of them faintly when there are few, else the selected and hovered
/// ones; the hovered or selected one is emphasised.
fn draw_outlines(painter: &egui::Painter, boxes: &[Rect], state: OutlineState) {
    let OutlineState { selected, hovered_box, moving, dragging, faint_all } = state;
    if faint_all {
        for (_, b) in boxes.iter().enumerate().filter(|(i, b)| b.is_positive() && moving != Some(*i)) {
            painter.rect_stroke(*b, CornerRadius::ZERO, Stroke::new(0.75, ACCENT.gamma_multiply(0.35)), egui::StrokeKind::Outside);
        }
    }
    if !faint_all {
        for i in selected.filter(|_| moving.is_none()).into_iter().chain(hovered_box.filter(|h| Some(*h) != selected)) {
            if let Some(b) = boxes.get(i).filter(|b| b.is_positive()) {
                painter.rect_stroke(*b, CornerRadius::ZERO, Stroke::new(0.75, ACCENT.gamma_multiply(0.35)), egui::StrokeKind::Outside);
            }
        }
    }
    for i in selected.into_iter().chain(hovered_box).collect::<Vec<_>>() {
        let Some(b) = boxes.get(i).copied().filter(|b| b.is_positive()) else { continue };
        if selected == Some(i) {
            // The selected frame and handles are drawn below (or the drag preview instead).
            if !dragging {
                painter.rect_filled(b, CornerRadius::ZERO, ACCENT.gamma_multiply(0.04));
            }
        } else {
            painter.rect_stroke(b, CornerRadius::ZERO, Stroke::new(1.5, ACCENT.gamma_multiply(0.7)), egui::StrokeKind::Outside);
        }
    }
}

fn draw_handles(painter: &egui::Painter, rect: Rect) {
    for c in [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()] {
        painter.rect(
            Rect::from_center_size(c, egui::vec2(8.0, 8.0)),
            CornerRadius::ZERO,
            Color32::WHITE,
            Stroke::new(1.0, ACCENT),
            egui::StrokeKind::Middle,
        );
    }
}

/// Turning the picture by its rotate handle: the turned outline as a preview (the engine redraws
/// the pixels on release).
fn turn_preview(cx: &ImageCx<'_>, view: &mut DocView, i: usize, b: Rect, knob: Pos2) {
    let ImageCx { ui, resp, ptr, page, .. } = *cx;
    let painter = ui.painter();
    let hover_pos = ptr.pos;
    let shift = ui.input(|i| i.modifiers.shift);
    let degrees = hover_pos.map_or(0.0, |p| turn_degrees(b.center(), p, shift));
    // The turned outline as a preview; the engine redraws the pixels on release.
    let centre = b.center();
    let (sin, cos) = degrees.to_radians().sin_cos();
    let turn = |c: Pos2| {
        let v = c - centre;
        centre + egui::vec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
    };
    let outline = [b.left_top(), b.right_top(), b.right_bottom(), b.left_bottom()].map(turn).to_vec();
    painter.add(egui::Shape::closed_line(outline, Stroke::new(1.5, ACCENT)));
    let galley = painter.layout_no_wrap(format!("{degrees:.0}\u{b0}"), egui::FontId::proportional(12.0), ACCENT);
    let label = Rect::from_center_size(turn(knob) - egui::vec2(0.0, 14.0), galley.size() + egui::vec2(8.0, 4.0));
    painter.rect(label, CornerRadius::same(3), Color32::WHITE, Stroke::new(1.0, ACCENT), egui::StrokeKind::Middle);
    painter.galley(label.center() - galley.size() / 2.0, galley, ACCENT);
    view.cursor.claim(Layer::Drag, egui::CursorIcon::Grabbing);
    if resp.drag_stopped() || !ptr.primary_down {
        if let Some(s) = view.image_selection.as_mut() {
            s.drag = None;
            s.turning = false;
        }
        if degrees.abs() >= 0.05 {
            view.pending_edit = Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::RotateBy(f64::from(degrees)) });
        }
    }
}

/// Where the screen may still show the picture from before an applied move, and whether the
/// page's whole raster is the old one.
#[derive(Clone, Copy)]
struct Blanked {
    left_at: Option<Rect>,
    raster_old: bool,
}

/// A frame of the picture being moved or resized: the stretched picture, its frame and handles,
/// and on release the edit.
fn drag_preview(
    cx: &ImageCx<'_>,
    images: &[pdfcraft_engine::PageImage],
    view: &mut DocView,
    (i, b): (usize, Rect),
    (start, opposite, p): (Pos2, Option<Pos2>, Pos2),
    blanked: Blanked,
) {
    let ImageCx { ui, resp, xf, ptr, page, info, .. } = *cx;
    let painter = ui.painter();
    let Blanked { left_at, raster_old } = blanked;
    let screen = |r: Rect| Rect::from_two_pos(xf.norm_to_screen(r.min.x, r.min.y), xf.norm_to_screen(r.max.x, r.max.y));
    let preview = match opposite {
        // Resize from the opposite corner, keeping the aspect ratio.
        Some(fixed) => {
            let (w0, h0) = (b.width().max(1.0), b.height().max(1.0));
            // Use the axis with the larger relative pointer change. Taking the maximum
            // absolute size freezes shrinking when the other axis hasn't moved. Measure
            // from the press point so grabbing a handle off-centre cannot cause a jump.
            let horizontal = (p.x - start.x) * (start.x - fixed.x).signum() / w0;
            let vertical = (p.y - start.y) * (start.y - fixed.y).signum() / h0;
            let delta = if horizontal.abs() > vertical.abs() { horizontal } else { vertical };
            let k = (1.0 + delta).max(0.05);
            let (w, h) = (w0 * k, h0 * k);
            let x = if start.x < fixed.x { fixed.x - w } else { fixed.x };
            let y = if start.y < fixed.y { fixed.y - h } else { fixed.y };
            Rect::from_min_size(Pos2::new(x, y), egui::vec2(w, h))
        }
        None => b.translate(p - start),
    };
    // Move/resize the full placement by the visible frame's change. The engine
    // transforms an isolated crop together with its image, preserving its contents.
    let raw = images.get(i).map_or(b, |im| xf.user_rect(info, page, im.rect.map(|v| v as f32)));
    // A zero-width or zero-height visible frame (or a degenerate ratio) keeps the
    // placement's scale instead of producing inf/NaN.
    let ratio = |new: f32, old: f32| if old.abs() > 1e-3 && (new / old).is_finite() { new / old } else { 1.0 };
    let factor = egui::vec2(ratio(preview.width(), b.width()), ratio(preview.height(), b.height()));
    let target = Rect::from_min_max(preview.min + (raw.min - b.min) * factor, preview.min + (raw.max - b.min) * factor);
    let visible = preview.intersect(xf.rect);
    if visible.is_positive() {
        // The pixels follow the frame: the page raster's picture of the image, stretched
        // into the preview over a blank where it was. Re-rendering every frame of a drag
        // would lag; the edit renders it properly on release.
        if preview != b
            && let Some(tex) = view.page_raster(page)
        {
            // Its pixels: where the whole-page raster has it.
            let source = left_at.filter(|_| raster_old).map_or(b, screen);
            let blank: Vec<Rect> = left_at.map(screen).into_iter().chain(std::iter::once(b)).collect();
            stretched_picture(painter, xf, tex, &blank, source, preview);
        }
        painter.rect_filled(visible, CornerRadius::ZERO, ACCENT.gamma_multiply(0.04));
        painter.rect_stroke(visible, CornerRadius::ZERO, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
        draw_handles(painter, preview);
    }
    if resp.drag_stopped() || !ptr.primary_down {
        if let Some(s) = view.image_selection.as_mut() {
            s.drag = None;
            let norm = |r: Rect| {
                let ((u0, v0), (u1, v1)) = (xf.screen_to_norm(r.min), xf.screen_to_norm(r.max));
                Rect::from_two_pos(Pos2::new(u0, v0), Pos2::new(u1, v1))
            };
            s.settled = (preview != b).then(|| (left_at.unwrap_or_else(|| norm(b)), norm(preview)));
        }
        if preview != b {
            view.pending_edit =
                Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::Move(user_box(xf, info, page, target)) });
        }
    }
    view.cursor.claim(
        Layer::Drag,
        match opposite {
            Some(fixed) if (start.x - fixed.x) * (start.y - fixed.y) < 0.0 => egui::CursorIcon::ResizeNeSw,
            Some(_) => egui::CursorIcon::ResizeNwSe,
            None => egui::CursorIcon::Move,
        },
    );
}

/// Images on a page: select, move, resize, right-click. Returns `true` when the pointer was used.
pub(crate) fn image_input(cx: &ImageCx<'_>, images: &[pdfcraft_engine::PageImage], view: &mut DocView, action: &mut Option<ImageAction>) -> bool {
    let ImageCx { ui, resp, xf, ptr, page, info, allow_interaction } = *cx;
    let boxes: Vec<Rect> = images.iter().map(|im| im.visible_rect.map_or(Rect::NOTHING, |r| xf.user_rect(info, page, r.map(|v| v as f32)))).collect();
    let painter = ui.painter();
    let hover_pos = ptr.pos;
    let image_contains = |index: usize, point: Pos2| {
        let (x, y) = xf.screen_to_view(point);
        let Some(p) = info.pages.get(page) else { return false };
        let user = p.view_to_user(x, y);
        images.get(index).is_some_and(|im| im.contains(user.map(f64::from)))
    };
    let selected = view.image_selection.as_ref().filter(|s| s.page == page).map(|s| s.index).filter(|i| *i < boxes.len());
    let dragging = view.image_selection.as_ref().is_some_and(|s| s.drag.is_some());
    // Few images: faint outlines for all of them, as before. Many: only the selected and
    // hovered boxes, so a ~1000-image CAD page stays interactive in Edit text & images.
    // Fully clipped placements (empty visible frames) get no outline.
    let faint_all = boxes.len() <= MAX_OUTLINES;
    // The picture being dragged has its frame at the pointer; its outline where it was would
    // trail behind until the release.
    let moving = selected.filter(|_| dragging);
    let hovered_box = hover_pos.and_then(|p| boxes.iter().enumerate().rposition(|(i, b)| b.is_positive() && b.contains(p) && image_contains(i, p)));
    draw_outlines(painter, &boxes, OutlineState { selected, hovered_box, moving, dragging, faint_all });
    // The selected image: frame, corner handles, dragging.
    if let Some(i) = selected.filter(|i| boxes.get(*i).is_some_and(|b| b.is_positive())) {
        let b = boxes[i];
        if view.image_selection.as_ref().is_some_and(|s| s.crop.is_some()) {
            return crop_input(cx, view, i, b);
        }
        let corners = [b.left_top(), b.right_top(), b.left_bottom(), b.right_bottom()];
        // Until the page is rendered again its raster has an applied move or resize's picture
        // where it was: it stays blanked there and drawn where it went.
        // The whole-page raster and its tiles are replaced separately: the old frame stays blanked
        // while anything still shows the picture there, but its pixels come from the old frame
        // only while the whole-page raster itself is the old one. Once that is fresh it has the
        // picture where it went, and that is where they come from.
        let (out_of_date, raster_old, raster) = (view.page_out_of_date(page), view.page_raster_out_of_date(page), view.page_raster(page));
        let screen = |r: Rect| Rect::from_two_pos(xf.norm_to_screen(r.min.x, r.min.y), xf.norm_to_screen(r.max.x, r.max.y));
        // Where the screen may still show the picture from before an applied move, while the page
        // isn't drawn again yet. Another drag keeps it blanked too.
        let left_at = view.image_selection.as_ref().and_then(|s| s.settled).filter(|_| out_of_date).map(|(from, _)| from);
        if let Some(s) = view.image_selection.as_mut().filter(|s| s.drag.is_none()) {
            match (s.settled, raster) {
                (Some((from, to)), Some(tex)) if out_of_date => {
                    let source = if raster_old { from } else { to };
                    stretched_picture(painter, xf, tex, &[screen(from)], screen(source), screen(to));
                }
                _ => s.settled = None,
            }
        }
        if view.image_selection.as_ref().is_some_and(|s| s.drag.is_none()) {
            painter.rect_stroke(b, CornerRadius::ZERO, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
            draw_handles(painter, b);
        }
        if allow_interaction && let Some(corner) = hover_pos.and_then(|p| corners.iter().position(|c| c.distance(p) < 8.0)) {
            view.cursor.claim(Layer::Handle, if matches!(corner, 0 | 3) { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw });
        }
        // The rotate handle: a knob above the top edge, joined to it by a short stem.
        let (knob, stem) = rotate_knob(b, ui.clip_rect());
        if view.image_selection.as_ref().is_some_and(|s| s.drag.is_none()) {
            painter.line_segment([stem, knob], Stroke::new(1.0, ACCENT));
            painter.circle(knob, 5.0, Color32::WHITE, Stroke::new(1.0, ACCENT));
            if allow_interaction && hover_pos.is_some_and(|p| p.distance(knob) < 8.0) {
                view.cursor.claim(Layer::Handle, egui::CursorIcon::Grab);
            }
        }
        if allow_interaction
            && resp.drag_started()
            && let Some(o) = ptr.origin
            && o.distance(knob) < 8.0
        {
            if let Some(s) = view.image_selection.as_mut() {
                s.drag = Some((o, None));
                s.turning = true;
            }
        } else if allow_interaction
            && resp.drag_started()
            && let Some(o) = ptr.origin
        {
            let corner = corners.iter().position(|c| c.distance(o) < 8.0);
            if corner.is_some() || image_contains(i, o) {
                let opposite = corner.map(|k| corners[3 - k]);
                if let Some(s) = view.image_selection.as_mut() {
                    s.drag = Some((o, opposite));
                }
            }
        }
        if view.image_selection.as_ref().is_some_and(|s| s.turning) {
            turn_preview(cx, view, i, b, knob);
            return true;
        }
        if let Some((start, opposite)) = view.image_selection.as_ref().and_then(|s| s.drag)
            && let Some(p) = hover_pos
        {
            drag_preview(cx, images, view, (i, b), (start, opposite, p), Blanked { left_at, raster_old });
            return true;
        }
        if allow_interaction
            && ui.input(|inp| inp.key_pressed(egui::Key::Delete) || inp.key_pressed(egui::Key::Backspace))
            && !ui.ctx().egui_wants_keyboard_input()
        {
            view.image_selection = None;
            view.pending_edit = Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::Delete });
            return true;
        }
    }
    if !allow_interaction {
        return false;
    }
    // An open menu stays drawn wherever the pointer is: most of its items lie outside the picture
    // (a logo is smaller than its menu), and a menu that is not drawn for a frame cannot be clicked.
    // Only a menu opened by a right-click on the picture: one opened over text or empty page while
    // a picture stays selected is the page's own menu.
    let menu_key = resp.id.with("image-menu");
    if resp.secondary_clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(menu_key, false));
    }
    let mut menu_drawn = false;
    if let Some(sel) = selected
        && resp.context_menu_opened()
        && ui.ctx().data(|d| d.get_temp::<bool>(menu_key)).unwrap_or(false)
    {
        resp.context_menu(|ui| image_menu(ui, page, sel, images, view, action));
        menu_drawn = true;
    }
    let Some(p) = hover_pos.filter(|p| xf.rect.contains(*p)) else { return false };
    let Some(hit) = boxes.iter().enumerate().rposition(|(i, b)| b.contains(p) && image_contains(i, p)) else { return false };
    if selected != Some(hit) {
        painter.rect_stroke(boxes[hit], CornerRadius::ZERO, Stroke::new(1.5, ACCENT.gamma_multiply(0.7)), egui::StrokeKind::Outside);
    }
    let Some(b) = boxes.get(hit).copied() else { return false };
    let corner = [b.left_top(), b.right_top(), b.left_bottom(), b.right_bottom()].iter().position(|c| c.distance(p) < 8.0);
    match (selected == Some(hit), corner) {
        (true, Some(0 | 3)) => view.cursor.claim(Layer::Handle, egui::CursorIcon::ResizeNwSe),
        (true, Some(_)) => view.cursor.claim(Layer::Handle, egui::CursorIcon::ResizeNeSw),
        (true, None) => view.cursor.claim(Layer::Placed, egui::CursorIcon::Move),
        // A click selects it.
        _ => view.cursor.claim(Layer::Placed, egui::CursorIcon::PointingHand),
    }
    if resp.clicked() || resp.secondary_clicked() {
        view.image_selection = Some(ImageSelection { page, index: hit, drag: None, crop: None, turning: false, settled: None });
    }
    if resp.secondary_clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(menu_key, true));
    }
    if selected == Some(hit) && !menu_drawn {
        resp.context_menu(|ui| image_menu(ui, page, hit, images, view, action));
    }
    true
}

/// The right-click menu of the picture `hit` (its rotate, flip, replace, copy, cut, redact, link,
/// recognize and delete items).
fn image_menu(
    ui: &mut egui::Ui,
    page: usize,
    hit: usize,
    images: &[pdfcraft_engine::PageImage],
    view: &mut DocView,
    action: &mut Option<ImageAction>,
) {
    use pdfcraft_engine::ImageEdit as E;
    let items: [(&str, Option<E>); 4] = [
        (tl!("Rotate Clockwise"), Some(E::Rotate(1))),
        (tl!("Rotate Counterclockwise"), Some(E::Rotate(3))),
        (tl!("Flip Horizontal"), Some(E::Flip { horizontal: true })),
        (tl!("Flip Vertical"), Some(E::Flip { horizontal: false })),
    ];
    for (label, change) in items {
        if ui.button(label).clicked() {
            view.pending_edit = change.map(|c| Edit::EditPageImage { page, index: hit, change: c });
            ui.close();
        }
    }
    // Crop: the handles trim instead of resizing until Done.
    if ui.button(tl!("Crop Image")).clicked() {
        if let (Some(s), Some(im)) = (view.image_selection.as_mut(), images.get(hit)) {
            s.crop = Some(CropMode::new(im.visible_rect.unwrap_or(im.rect)));
        }
        ui.close();
    }
    let cropped = images.get(hit).is_some_and(|im| im.visible_rect.is_some_and(|v| v.iter().zip(im.rect.iter()).any(|(a, b)| (a - b).abs() > 0.01)));
    if ui.add_enabled(cropped, egui::Button::new(tl!("Remove Crop"))).clicked() {
        if let Some(im) = images.get(hit) {
            view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Crop(im.rect) });
        }
        ui.close();
    }
    let raster = images.get(hit).is_some_and(|image| !image.is_form);
    if ui.add_enabled(raster, egui::Button::new(tl!("Replace Image…"))).clicked() {
        *action = Some(ImageAction::Replace(page, hit));
        ui.close();
    }
    if ui.add_enabled(raster, egui::Button::new(tl!("Save Image As…"))).clicked() {
        *action = Some(ImageAction::Save(page, hit));
        ui.close();
    }
    // Cutting and copying also find the picture inside grouped Form artwork.
    if ui.button(tl!("Cut Image")).clicked() {
        *action = Some(ImageAction::Cut(page, hit));
        ui.close();
    }
    if ui.button(tl!("Copy Image")).clicked() {
        *action = Some(ImageAction::Copy(page, hit));
        ui.close();
    }
    if ui.button(tl!("Copy Image in Original Format")).clicked() {
        *action = Some(ImageAction::CopyFile(page, hit));
        ui.close();
    }
    ui.separator();
    if ui.button(tl!("Redact Image")).clicked() {
        *action = Some(ImageAction::Redact(page, hit));
        ui.close();
    }
    if ui.button(tl!("Create Link…")).clicked() {
        *action = Some(ImageAction::Link(page, hit));
        ui.close();
    }
    if ui.button(tl!("Recognize Text…")).clicked() {
        *action = Some(ImageAction::Recognize(page));
        ui.close();
    }
    ui.separator();
    if ui.button(tl!("Delete")).clicked() {
        view.image_selection = None;
        view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Delete });
        ui.close();
    }
}
