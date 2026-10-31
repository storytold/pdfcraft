use super::{ACCENT, Color32, CornerRadius, DocInfo, DocView, Edit, MAX_OUTLINES, PagePointer, PageXform, Pos2, Rect, Stroke, pos2, user_box};
use crate::canvas::{ImageObb, PainterExt};
use crate::image_engine::{CROP_ACCENT, HandleId, apply_crop, apply_resize, apply_resize_proportional, draw_handle, draw_rule_of_thirds};
use crate::page_cursor::Layer;
use crate::page_router::{Hit, Owner, PicturePart};

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

/// The 8 resize handles on an image (4 corners + 4 edge centers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageHandle {
    TopLeft,
    TopCenter,
    TopRight,
    RightCenter,
    BottomRight,
    BottomCenter,
    BottomLeft,
    LeftCenter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragTarget {
    Move,
    Handle(ImageHandle),
    CropHandle(ImageHandle),
    Rotate,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageDrag {
    pub start: Pos2,
    pub target: DragTarget,
    pub initial_rect: Rect,
    pub initial_matrix: [f64; 6],
}

/// A selected page image, and what the pointer is doing to it.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageSelection {
    pub page: usize,
    pub index: usize,
    /// Dragging in progress.
    pub drag: Option<ImageDrag>,
    /// Crop mode: the box kept so far.
    pub crop: Option<CropMode>,
    pub crop_mode: bool,
    pub crop_rect: Option<Rect>,
    /// The drag is turning the picture by its rotate handle (the `drag` start is then unused).
    pub turning: bool,
    /// A move or resize applied on release: the frame it had and the one it got, in the page's
    /// 0..1 coordinates, while the page raster still has the picture where it was.
    pub settled: Option<(Rect, Rect)>,
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

    pub(crate) fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

/// Cropping a page image: the handles trim instead of resizing, the part that will go is
/// dimmed, Enter or Done applies and Esc or Cancel leaves it.
#[derive(Clone, Debug, PartialEq)]
pub struct CropMode {
    /// The box to keep (user space or normalized UV).
    pub keep: [f64; 4],
    /// A drag in progress: the grip (0–7 handles, 8 inside), where it started and the box then.
    pub(crate) drag: Option<(usize, Pos2, [f64; 4])>,
}

impl CropMode {
    pub fn new(keep: [f64; 4]) -> Self {
        Self { keep, drag: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Matrix(pub(crate) [f64; 6]);

impl Matrix {
    pub(crate) fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    pub(crate) fn invert(&self) -> Option<Self> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        Some(Matrix([ia, ib, ic, id, -(e * ia + f * ic), -(e * ib + f * id)]))
    }

    pub(crate) fn then(&self, then: &Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = then.0;
        Matrix([a * a2 + b * c2, a * b2 + b * d2, c * a2 + d * c2, c * b2 + d * d2, e * a2 + f * c2 + e2, e * b2 + f * d2 + f2])
    }
}

pub(crate) fn user_to_screen(xf: &PageXform, info: &DocInfo, page: usize, x: f64, y: f64) -> Pos2 {
    let Some(p) = info.pages.get(page) else { return Pos2::ZERO };
    let v = p.user_to_view(x as f32, y as f32);
    xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph)
}

fn screen_to_user(xf: &PageXform, info: &DocInfo, page: usize, pt: Pos2) -> (f64, f64) {
    let (vx, vy) = xf.screen_to_view(pt);
    let Some(p) = info.pages.get(page) else { return (0.0, 0.0) };
    let u = p.view_to_user(vx, vy);
    (f64::from(u[0]), f64::from(u[1]))
}

pub(crate) fn quad_contains(quad: [Pos2; 4], p: Pos2) -> bool {
    if !p.x.is_finite() || !p.y.is_finite() || quad.iter().any(|pt| !pt.x.is_finite() || !pt.y.is_finite()) {
        return false;
    }
    let mut sign = 0;
    for i in 0..4 {
        let p1 = quad[i];
        let p2 = quad[(i + 1) % 4];
        let cross = (p2.x - p1.x) * (p.y - p1.y) - (p2.y - p1.y) * (p.x - p1.x);
        if cross.abs() > 1e-4 {
            let s = if cross > 0.0 { 1 } else { -1 };
            if sign == 0 {
                sign = s;
            } else if sign != s {
                return false;
            }
        }
    }
    sign != 0
}

/// The screen positions of the crop's eight handles: corners, then the top, right, bottom and left edge midpoints.
fn crop_handles(k: Rect) -> [Pos2; 8] {
    [k.left_top(), k.right_top(), k.left_bottom(), k.right_bottom(), k.center_top(), k.right_center(), k.center_bottom(), k.left_center()]
}

/// Crop mode for the selected image `i` whose visible frame is `b` (screen). Returns `true`
/// (the pointer and keys are the crop's while it lasts).
#[allow(clippy::cognitive_complexity)]
fn crop_input(cx: &ImageCx<'_>, view: &mut DocView, i: usize, image: &pdfcraft_engine::PageImage) -> bool {
    let ImageCx { ui, resp, xf, ptr, page, info, allow_interaction } = *cx;
    let painter = ui.painter();
    let Some(sel) = view.image_selection.as_mut() else { return false };
    if let Some(rect) = sel.crop_rect
        && let Some(mode) = sel.crop.as_mut()
    {
        mode.keep = user_box(xf, info, page, rect);
        sel.crop_rect = None;
    }
    let Some(mode) = sel.crop.as_mut() else { return false };
    let hover_pos = ptr.pos;
    let full_b = xf.user_rect(info, page, image.rect.map(|v| v as f32));
    let b = if full_b.is_positive() { full_b } else { xf.user_rect(info, page, image.visible_rect.unwrap_or(image.rect).map(|v| v as f32)) };
    let keep_screen = |keep: [f64; 4]| xf.user_rect(info, page, keep.map(|v| v as f32)).intersect(b);
    let k = keep_screen(mode.keep);

    if allow_interaction
        && resp.drag_started()
        && let Some(o) = ptr.origin
    {
        let grip = crop_handles(k).iter().position(|h| h.distance(o) < 10.0).or_else(|| k.contains(o).then_some(8));
        if let Some(g) = grip {
            mode.drag = Some((g, o, mode.keep));
        }
    }

    if let Some((grip, start, at_start)) = mode.drag
        && let Some(p) = hover_pos
    {
        let d = p - start;
        let k0 = keep_screen(at_start);
        if grip < 8 {
            let handle_id = match grip {
                0 => HandleId::TopLeft,
                1 => HandleId::TopRight,
                2 => HandleId::BottomLeft,
                3 => HandleId::BottomRight,
                4 => HandleId::TopMid,
                5 => HandleId::RightMid,
                6 => HandleId::BottomMid,
                _ => HandleId::LeftMid,
            };
            let uv0 = if b.width() > 0.0 && b.height() > 0.0 {
                Rect::from_min_max(
                    pos2(((k0.min.x - b.min.x) / b.width()).clamp(0.0, 1.0), ((k0.min.y - b.min.y) / b.height()).clamp(0.0, 1.0)),
                    pos2(((k0.max.x - b.min.x) / b.width()).clamp(0.0, 1.0), ((k0.max.y - b.min.y) / b.height()).clamp(0.0, 1.0)),
                )
            } else {
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
            };
            let (new_screen, _) = apply_crop(k0, uv0, handle_id, d);
            let (min_w, min_h) = (6.0_f32.min(b.width()), 6.0_f32.min(b.height()));
            let clamped = Rect::from_min_max(
                pos2(new_screen.min.x.clamp(b.min.x, b.max.x - min_w), new_screen.min.y.clamp(b.min.y, b.max.y - min_h)),
                pos2(new_screen.max.x.clamp(b.min.x + min_w, b.max.x), new_screen.max.y.clamp(b.min.y + min_h, b.max.y)),
            );
            mode.keep = user_box(xf, info, page, clamped);
        } else {
            let shift = egui::vec2(d.x.clamp(b.min.x - k0.min.x, b.max.x - k0.max.x), d.y.clamp(b.min.y - k0.min.y, b.max.y - k0.max.y));
            mode.keep = user_box(xf, info, page, Rect::from_min_max(k0.min + shift, k0.max + shift));
        }
        if resp.drag_stopped() || !ptr.primary_down {
            mode.drag = None;
        }
    }

    // Faint ghost outline of the full uncropped image underneath
    painter.rect_stroke(b, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_rgba_unmultiplied(200, 200, 200, 80)), egui::StrokeKind::Outside);

    // Dim outer excluded bands
    let dim = Color32::from_black_alpha(96);
    if k.min.y > b.min.y {
        painter.rect_filled(Rect::from_min_max(b.min, Pos2::new(b.max.x, k.min.y)), CornerRadius::ZERO, dim);
    }
    if k.max.y < b.max.y {
        painter.rect_filled(Rect::from_min_max(Pos2::new(b.min.x, k.max.y), b.max), CornerRadius::ZERO, dim);
    }
    if k.min.x > b.min.x {
        painter.rect_filled(Rect::from_min_max(Pos2::new(b.min.x, k.min.y), Pos2::new(k.min.x, k.max.y)), CornerRadius::ZERO, dim);
    }
    if k.max.x < b.max.x {
        painter.rect_filled(Rect::from_min_max(Pos2::new(k.max.x, k.min.y), Pos2::new(b.max.x, k.max.y)), CornerRadius::ZERO, dim);
    }

    // Rule-of-Thirds Grid inside crop area
    draw_rule_of_thirds(painter, k, Stroke::new(1.0, CROP_ACCENT.gamma_multiply(0.45)));

    // Crop border stroke
    painter.rect_stroke(k, CornerRadius::ZERO, Stroke::new(1.5, CROP_ACCENT), egui::StrokeKind::Outside);

    // Crop handles
    for (idx, h) in crop_handles(k).iter().enumerate() {
        let is_active = mode.drag.as_ref().is_some_and(|d| d.0 == idx);
        let is_hovered = hover_pos.is_some_and(|p| p.distance(*h) < 10.0);
        draw_handle(painter, *h, is_active, is_hovered, true, CROP_ACCENT);
    }

    // Action pills
    let font = egui::FontId::proportional(12.5);
    let done_w = painter.layout_no_wrap(tl!("Done").to_string(), font.clone(), Color32::WHITE).size().x + 22.0;
    let cancel_w = painter.layout_no_wrap(tl!("Cancel").to_string(), font.clone(), Color32::WHITE).size().x + 22.0;
    let is_full_rect = (mode.keep[0] - image.rect[0]).abs() < 1e-3
        && (mode.keep[1] - image.rect[1]).abs() < 1e-3
        && (mode.keep[2] - image.rect[2]).abs() < 1e-3
        && (mode.keep[3] - image.rect[3]).abs() < 1e-3;
    let has_crop = !is_full_rect || image.visible_rect.is_some_and(|v| v != image.rect);
    let reset_w =
        if has_crop { Some(painter.layout_no_wrap(tl!("Remove Crop").to_string(), font.clone(), Color32::WHITE).size().x + 22.0) } else { None };
    let spacing = 6.0;
    let total_w = done_w + spacing + cancel_w + reset_w.map_or(0.0, |w| w + spacing);

    // Position pills below crop box if there's room, otherwise above
    let y = if k.max.y + 32.0 < xf.rect.max.y {
        k.max.y + 6.0
    } else if k.min.y - 28.0 > xf.rect.min.y {
        k.min.y - 28.0
    } else {
        k.max.y + 6.0
    };

    // Right-align pills with k.max.x, clamped inside the page bounds
    let mut right = k.max.x.min(xf.rect.max.x - 8.0);
    if right - total_w < xf.rect.min.x + 8.0 {
        right = (xf.rect.min.x + 8.0 + total_w).min(xf.rect.max.x - 8.0);
    }

    let done = Rect::from_min_size(Pos2::new(right - done_w, y), egui::vec2(done_w, 22.0));
    let cancel = Rect::from_min_size(Pos2::new(done.min.x - spacing - cancel_w, y), egui::vec2(cancel_w, 22.0));
    let reset = reset_w.map(|w| Rect::from_min_size(Pos2::new(cancel.min.x - spacing - w, y), egui::vec2(w, 22.0)));

    let done_id = ui.scope_id().with("crop_pill_done").with(page).with(i);
    let cancel_id = ui.scope_id().with("crop_pill_cancel").with(page).with(i);
    let reset_id = ui.scope_id().with("crop_pill_reset").with(page).with(i);

    let done_resp = ui.interact(done, done_id, egui::Sense::click());
    let cancel_resp = ui.interact(cancel, cancel_id, egui::Sense::click());
    let reset_resp = reset.map(|r| ui.interact(r, reset_id, egui::Sense::click()));

    let t = crate::theme::Tokens::get(ui.ctx());

    // Done button (accent colored)
    let done_bg = if done_resp.is_pointer_button_down_on() {
        ACCENT.gamma_multiply(0.8)
    } else if done_resp.hovered() {
        Color32::from_rgb(40, 145, 255)
    } else {
        ACCENT
    };
    painter.rect_filled(done, CornerRadius::same(11), done_bg);
    painter.text(done.center(), egui::Align2::CENTER_CENTER, tl!("Done"), font.clone(), Color32::WHITE);

    // Cancel button (panel style with border)
    let cancel_bg = if cancel_resp.is_pointer_button_down_on() {
        t.pressed
    } else if cancel_resp.hovered() {
        t.hover
    } else {
        t.card
    };
    let cancel_stroke = Stroke::new(1.0, if cancel_resp.hovered() { ACCENT } else { t.border });
    let cancel_text = if cancel_resp.hovered() { ACCENT } else { t.text };
    painter.rect(cancel, CornerRadius::same(11), cancel_bg, cancel_stroke, egui::StrokeKind::Inside);
    painter.text(cancel.center(), egui::Align2::CENTER_CENTER, tl!("Cancel"), font.clone(), cancel_text);

    // Reset button (panel style with border)
    if let (Some(r), Some(r_resp)) = (reset, reset_resp.as_ref()) {
        let reset_bg = if r_resp.is_pointer_button_down_on() {
            t.pressed
        } else if r_resp.hovered() {
            t.hover
        } else {
            t.card
        };
        let reset_stroke = Stroke::new(1.0, if r_resp.hovered() { ACCENT } else { t.border });
        let reset_text = if r_resp.hovered() { ACCENT } else { t.text };
        painter.rect(r, CornerRadius::same(11), reset_bg, reset_stroke, egui::StrokeKind::Inside);
        painter.text(r.center(), egui::Align2::CENTER_CENTER, tl!("Remove Crop"), font.clone(), reset_text);
    }

    if let Some(p) = hover_pos {
        let icon = if done_resp.hovered()
            || cancel_resp.hovered()
            || reset_resp.as_ref().is_some_and(|r| r.hovered())
            || done.contains(p)
            || cancel.contains(p)
            || reset.is_some_and(|r| r.contains(p))
        {
            egui::CursorIcon::PointingHand
        } else if let Some(g) = crop_handles(k).iter().position(|h| h.distance(p) < 8.0) {
            match g {
                0 => HandleId::TopLeft.cursor_icon(),
                1 => HandleId::TopRight.cursor_icon(),
                2 => HandleId::BottomLeft.cursor_icon(),
                3 => HandleId::BottomRight.cursor_icon(),
                4 => HandleId::TopMid.cursor_icon(),
                5 => HandleId::RightMid.cursor_icon(),
                6 => HandleId::BottomMid.cursor_icon(),
                _ => HandleId::LeftMid.cursor_icon(),
            }
        } else if k.contains(p) {
            if mode.drag.is_some() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab }
        } else {
            egui::CursorIcon::Default
        };
        view.cursor.claim(Layer::Editor, icon);
    }

    let keys = |key: egui::Key| allow_interaction && ui.input(|inp| inp.key_pressed(key)) && !ui.ctx().egui_wants_keyboard_input();
    let done_clicked = allow_interaction && (done_resp.clicked() || (resp.clicked() && hover_pos.is_some_and(|p| done.contains(p))));
    let cancel_clicked = allow_interaction && (cancel_resp.clicked() || (resp.clicked() && hover_pos.is_some_and(|p| cancel.contains(p))));
    let reset_clicked = allow_interaction
        && reset
            .is_some_and(|r| reset_resp.as_ref().is_some_and(|res| res.clicked()) || (resp.clicked() && hover_pos.is_some_and(|p| r.contains(p))));

    if reset_clicked {
        mode.keep = image.rect;
        mode.drag = None;
        if let Some(s) = view.image_selection.as_mut() {
            s.crop_rect = None;
        }
    } else if done_clicked || keys(egui::Key::Enter) {
        let keep = mode.keep;
        if let Some(s) = view.image_selection.as_mut() {
            s.crop = None;
            s.crop_mode = false;
            s.crop_rect = None;
        }
        let is_full = (keep[0] - image.rect[0]).abs() < 1e-3
            && (keep[1] - image.rect[1]).abs() < 1e-3
            && (keep[2] - image.rect[2]).abs() < 1e-3
            && (keep[3] - image.rect[3]).abs() < 1e-3;
        if is_full {
            if image.visible_rect.is_some_and(|v| v != image.rect) {
                view.pending_edit = Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::RemoveCrop });
            }
        } else if image.visible_rect != Some(keep) {
            view.pending_edit = Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::Crop(keep) });
        }
    } else if (cancel_clicked || keys(egui::Key::Escape))
        && let Some(s) = view.image_selection.as_mut()
    {
        s.crop = None;
        s.crop_mode = false;
        s.crop_rect = None;
    }
    true
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
#[allow(dead_code)]
pub(crate) fn image_owns_pointer(_ui: &egui::Ui, xf: &PageXform, ptr: &PagePointer, page: usize, info: &DocInfo, view: &DocView) -> bool {
    let Some(selection) = view.image_selection.as_ref().filter(|s| s.page == page) else { return false };
    if selection.drag.is_some() || selection.crop.is_some() || selection.crop_mode {
        return true;
    }
    let Some(image) = view.edit_images.get(&page).and_then(|(_, images)| images.get(selection.index)) else { return false };
    let obb = ImageObb::from_image(image, xf, info, page);
    let p = if ptr.primary_down { ptr.origin.or(ptr.pos) } else { ptr.pos };
    let Some(p) = p else { return false };

    obb.handles().iter().any(|(_, h)| h.distance(p) < 8.0) || obb.s_rot.distance(p) < 8.0
}

/// The selected picture's grips under `p`: its corners and its rotate knob. They are asked after
/// the paragraphs, so text on top of a picture cannot take them; `clip` is the pages' visible
/// area, which the knob stays inside.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_hit(
    clip: Rect,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    images: &[pdfcraft_engine::PageImage],
    view: &DocView,
    p: Pos2,
) -> Option<Hit> {
    let s = view.image_selection.as_ref().filter(|s| s.page == page && s.crop.is_none())?;
    let b = images.get(s.index)?.visible_rect.map(|r| xf.user_rect(info, page, r.map(|v| v as f32)))?;
    let picture = |part| Owner::Picture { index: s.index, part };
    if rotate_knob(b, clip).0.distance(p) < 8.0 {
        return Some(Hit::new(picture(PicturePart::Knob), Layer::Handle, egui::CursorIcon::Grab));
    }
    let corner = [b.left_top(), b.right_top(), b.left_bottom(), b.right_bottom()].iter().position(|c| c.distance(p) < 8.0)?;
    let icon = if matches!(corner, 0 | 3) { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw };
    Some(Hit::new(picture(PicturePart::Corner(corner)), Layer::Handle, icon))
}

/// The picture under `p` (its shape, not only its box): a click selects it, a drag moves the
/// selected one. On the text layer, asked before the paragraphs: editable text on top of a
/// picture takes the press.
pub(crate) fn body_hit(xf: &PageXform, page: usize, info: &DocInfo, images: &[pdfcraft_engine::PageImage], view: &DocView, p: Pos2) -> Option<Hit> {
    if !xf.rect.contains(p) {
        return None;
    }
    let (x, y) = xf.screen_to_view(p);
    let user = info.pages.get(page)?.view_to_user(x, y).map(f64::from);
    let index = images
        .iter()
        .rposition(|im| im.visible_rect.is_some_and(|r| xf.user_rect(info, page, r.map(|v| v as f32)).contains(p)) && im.contains(user))?;
    let selected = view.image_selection.as_ref().is_some_and(|s| s.page == page && s.index == index);
    let icon = if selected { egui::CursorIcon::Move } else { egui::CursorIcon::PointingHand };
    Some(Hit::new(Owner::Picture { index, part: PicturePart::Body }, Layer::Text, icon))
}

/// What a right-click on a selected image asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImageAction {
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
    Paste(usize, Option<[f64; 2]>),
    DrawFreehand,
    AddComment(usize, usize),
    Crop,
}

/// Where the rotate knob sits, and the edge point its stem starts from: above the middle of the
/// top edge, or below the bottom edge when the area on screen (`view`) has no room above.
pub(super) fn rotate_knob(b: Rect, view: Rect) -> (Pos2, Pos2) {
    let above = b.center_top() - egui::vec2(0.0, 22.0);
    if above.y - 6.0 >= view.top() { (above, b.center_top()) } else { (b.center_bottom() + egui::vec2(0.0, 22.0), b.center_bottom()) }
}

/// Degrees clockwise (-180..=180) that turn from `start` onto `pointer` around `centre`; `snap`
/// rounds to steps of 15 degrees (Shift). A pointer on the centre turns nothing.
pub(super) fn turn_degrees_delta(centre: Pos2, start: Pos2, pointer: Pos2, snap: bool) -> f32 {
    let v_start = start - centre;
    let v_curr = pointer - centre;
    if !(v_curr.x.is_finite() && v_curr.y.is_finite())
        || v_curr.length() < 4.0
        || !(v_start.x.is_finite() && v_start.y.is_finite())
        || v_start.length() < 4.0
    {
        return 0.0;
    }
    // Screen y points down: up is (0, -1); clockwise is atan2(x, -y).
    let theta_start = v_start.x.atan2(-v_start.y).to_degrees();
    let theta_curr = v_curr.x.atan2(-v_curr.y).to_degrees();
    let mut d = theta_curr - theta_start;
    d = ((d + 180.0).rem_euclid(360.0)) - 180.0;
    if snap { (d / 15.0).round() * 15.0 } else { d }
}

/// Degrees clockwise (-180..=180) that turn "straight up" from `centre` onto the pointer; `snap`
/// rounds to steps of 15 degrees (Shift). A pointer on the centre turns nothing.
#[allow(dead_code)]
pub(super) fn turn_degrees(centre: Pos2, pointer: Pos2, snap: bool) -> f32 {
    turn_degrees_delta(centre, centre - egui::vec2(0.0, 10.0), pointer, snap)
}

#[doc(hidden)]
pub fn image_menu_id() -> egui::Id {
    egui::Id::unique("image-menu")
}

/// Images on a page: select, move, resize, right-click. Returns `true` when the pointer was used.
#[allow(clippy::cognitive_complexity)]
pub(crate) fn image_input(cx: &ImageCx<'_>, images: &[pdfcraft_engine::PageImage], view: &mut DocView, action: &mut Option<ImageAction>) -> bool {
    let ImageCx { ui, resp, xf, ptr, page, info, allow_interaction } = *cx;
    let _ = &action;
    let boxes: Vec<Rect> = images.iter().map(|im| crate::canvas::image_screen_rect(im, xf, info, page)).collect();
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
    let faint_all = boxes.len() <= MAX_OUTLINES;
    let moving = selected.filter(|_| dragging);
    if faint_all {
        for (idx, im) in images.iter().enumerate() {
            if moving == Some(idx) {
                continue;
            }
            if let Some(b) = boxes.get(idx).filter(|b| b.is_positive()) {
                let obb = ImageObb::from_image(im, xf, info, page);
                if obb.is_rotated {
                    let pts = obb.corners();
                    for k in 0..4 {
                        painter.line_segment([pts[k], pts[(k + 1) % 4]], Stroke::new(0.75, ACCENT.gamma_multiply(0.35)));
                    }
                } else {
                    painter.rect_stroke(*b, CornerRadius::ZERO, Stroke::new(0.75, ACCENT.gamma_multiply(0.35)), egui::StrokeKind::Outside);
                }
            }
        }
    }
    let hovered_box = hover_pos.and_then(|p| {
        boxes.iter().enumerate().rposition(|(i, b)| {
            b.is_positive()
                && images.get(i).is_some_and(|img| b.contains(p) || quad_contains(ImageObb::from_image(img, xf, info, page).corners(), p))
                && image_contains(i, p)
        })
    });
    if !faint_all {
        for i in selected.filter(|_| moving.is_none()).into_iter().chain(hovered_box.filter(|h| Some(*h) != selected)) {
            if let (Some(b), Some(img)) = (boxes.get(i).filter(|b| b.is_positive()), images.get(i)) {
                let obb = ImageObb::from_image(img, xf, info, page);
                if obb.is_rotated {
                    let pts = obb.corners();
                    for k in 0..4 {
                        painter.line_segment([pts[k], pts[(k + 1) % 4]], Stroke::new(0.75, ACCENT.gamma_multiply(0.35)));
                    }
                } else {
                    painter.rect_stroke(*b, CornerRadius::ZERO, Stroke::new(0.75, ACCENT.gamma_multiply(0.35)), egui::StrokeKind::Outside);
                }
            }
        }
    }
    for i in selected.into_iter().chain(hovered_box).collect::<Vec<_>>() {
        let (Some(b), Some(img)) = (boxes.get(i).copied().filter(|b| b.is_positive()), images.get(i)) else { continue };
        let obb = ImageObb::from_image(img, xf, info, page);
        if selected == Some(i) {
            if !dragging {
                if obb.is_rotated {
                    painter.polygon(obb.corners().to_vec(), ACCENT.gamma_multiply(0.04), Stroke::NONE);
                } else {
                    painter.rect_filled(b, CornerRadius::ZERO, ACCENT.gamma_multiply(0.04));
                }
            }
        } else if obb.is_rotated {
            painter.polygon(obb.corners().to_vec(), Color32::TRANSPARENT, Stroke::new(1.5, ACCENT.gamma_multiply(0.7)));
        } else {
            painter.rect_stroke(b, CornerRadius::ZERO, Stroke::new(1.5, ACCENT.gamma_multiply(0.7)), egui::StrokeKind::Outside);
        }
    }
    // The selected image: frame, corner handles, dragging.
    if let Some(i) = selected.filter(|i| boxes.get(*i).is_some_and(|b| b.is_positive())) {
        let Some(b) = boxes.get(i).copied() else { return false };
        let Some(image) = images.get(i) else { return false };
        let obb = ImageObb::from_image(image, xf, info, page);
        let (out_of_date, raster_old, raster) = (view.page_out_of_date(page), view.page_raster_out_of_date(page), view.page_raster(page));
        let screen = |r: Rect| Rect::from_two_pos(xf.norm_to_screen(r.min.x, r.min.y), xf.norm_to_screen(r.max.x, r.max.y));
        let left_at = view.image_selection.as_ref().and_then(|s| s.settled).filter(|_| out_of_date).map(|(from, _)| from);
        if let Some(s) = view.image_selection.as_mut().filter(|s| s.drag.is_none()) {
            match (s.settled, raster) {
                (Some((from, to)), Some(tex)) if out_of_date => {
                    let source = if raster_old { from } else { to };
                    stretched_picture(painter, xf, tex, &[screen(from)], screen(source), screen(to));
                }
                _ if !out_of_date => s.settled = None,
                _ => {}
            }
        }

        if view.image_selection.as_ref().is_some_and(|s| s.crop.is_some() || s.crop_mode) {
            if let Some(s) = view.image_selection.as_mut()
                && s.crop.is_none()
            {
                let rect = images.get(i).and_then(|im| im.visible_rect).unwrap_or(images.get(i).map_or([0.0; 4], |im| im.rect));
                s.crop = Some(CropMode::new(rect));
            }
            return crop_input(cx, view, i, image);
        }
        let (knob, stem) = rotate_knob(b, ui.clip_rect());
        if view.image_selection.as_ref().is_some_and(|s| s.drag.is_none()) {
            if obb.is_rotated {
                painter.polygon(obb.corners().to_vec(), Color32::TRANSPARENT, Stroke::new(1.5, ACCENT));
                painter.line_segment([obb.s_tc, obb.s_rot], Stroke::new(1.2, ACCENT));
                painter.circle(obb.s_rot, 4.5, Color32::WHITE, Stroke::new(1.0, ACCENT));
            } else {
                painter.rect_stroke(b, CornerRadius::ZERO, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
                painter.line_segment([stem, knob], Stroke::new(1.0, ACCENT));
                painter.circle(knob, 5.0, Color32::WHITE, Stroke::new(1.0, ACCENT));
            }
            for (_, h) in obb.handles() {
                let is_hovered = hover_pos.is_some_and(|p| p.distance(h) < 10.0);
                draw_handle(painter, h, false, is_hovered, false, ACCENT);
            }
        }

        let mut handle_drag = None;
        for (h, h_pos, handle_id) in obb.handles_with_ids(ui.scope_id().with("img_handle")) {
            let handle_rect = Rect::from_center_size(h_pos, egui::vec2(18.0, 18.0));
            let handle_resp = ui.interact(handle_rect, handle_id, egui::Sense::drag());
            if allow_interaction && handle_resp.drag_started() {
                handle_drag = Some(h);
            }
        }
        let rot_id = ui.scope_id().with("img_handle").with(8);
        let rot_rect = Rect::from_center_size(if obb.is_rotated { obb.s_rot } else { knob }, egui::vec2(12.0, 12.0));
        let rot_resp = ui.interact(rot_rect, rot_id, egui::Sense::drag());
        let rot_drag = allow_interaction && rot_resp.drag_started();

        if allow_interaction && let Some(p) = hover_pos {
            if obb.s_rot.distance(p) < 8.0 || knob.distance(p) < 8.0 {
                view.cursor.claim(Layer::Handle, egui::CursorIcon::Grab);
            } else if let Some((h, _)) = obb.handles().iter().find(|(_, pos)| pos.distance(p) < 8.0) {
                let icon = match h {
                    ImageHandle::TopLeft | ImageHandle::BottomRight => {
                        if obb.is_rotated {
                            let v = obb.handle_pos(*h) - obb.screen_center;
                            if v.x * v.y > 0.0 { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw }
                        } else {
                            egui::CursorIcon::ResizeNwSe
                        }
                    }
                    ImageHandle::TopRight | ImageHandle::BottomLeft => {
                        if obb.is_rotated {
                            let v = obb.handle_pos(*h) - obb.screen_center;
                            if v.x * v.y > 0.0 { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw }
                        } else {
                            egui::CursorIcon::ResizeNeSw
                        }
                    }
                    ImageHandle::TopCenter | ImageHandle::BottomCenter => {
                        if obb.is_rotated && (obb.v_axis.0.abs() > obb.v_axis.1.abs()) {
                            egui::CursorIcon::ResizeHorizontal
                        } else {
                            egui::CursorIcon::ResizeVertical
                        }
                    }
                    ImageHandle::RightCenter | ImageHandle::LeftCenter => {
                        if obb.is_rotated && (obb.u_axis.1.abs() > obb.u_axis.0.abs()) {
                            egui::CursorIcon::ResizeVertical
                        } else {
                            egui::CursorIcon::ResizeHorizontal
                        }
                    }
                };
                view.cursor.claim(Layer::Handle, icon);
            }
        }

        let origin = ptr.origin;
        if allow_interaction {
            if let Some(h) = handle_drag {
                if let Some(s) = view.image_selection.as_mut() {
                    let o = origin.unwrap_or_else(|| obb.handle_pos(h));
                    s.drag = Some(ImageDrag { start: o, target: DragTarget::Handle(h), initial_rect: b, initial_matrix: image.matrix });
                }
            } else if rot_drag {
                if let Some(s) = view.image_selection.as_mut() {
                    let o = origin.unwrap_or(if obb.is_rotated { obb.s_rot } else { knob });
                    s.drag = Some(ImageDrag { start: o, target: DragTarget::Rotate, initial_rect: b, initial_matrix: image.matrix });
                    s.turning = true;
                }
            } else if resp.drag_started()
                && let Some(o) = origin
            {
                if obb.s_rot.distance(o) < 8.0 || knob.distance(o) < 8.0 {
                    if let Some(s) = view.image_selection.as_mut() {
                        s.drag = Some(ImageDrag { start: o, target: DragTarget::Rotate, initial_rect: b, initial_matrix: image.matrix });
                        s.turning = true;
                    }
                } else if let Some((h, _)) = obb.handles().iter().find(|(_, pos)| pos.distance(o) < 8.0) {
                    if let Some(s) = view.image_selection.as_mut() {
                        s.drag = Some(ImageDrag { start: o, target: DragTarget::Handle(*h), initial_rect: b, initial_matrix: image.matrix });
                    }
                } else if (image_contains(i, o) || quad_contains(obb.corners(), o))
                    && let Some(s) = view.image_selection.as_mut()
                {
                    s.drag = Some(ImageDrag { start: o, target: DragTarget::Move, initial_rect: b, initial_matrix: image.matrix });
                }
            }
        }

        if view.image_selection.as_ref().is_some_and(|s| s.turning) {
            let shift = ui.input(|i| i.modifiers.shift);
            let centre = obb.screen_center;
            let rot_knob = if obb.is_rotated { obb.s_rot } else { knob };
            let drag_start = view.image_selection.as_ref().and_then(|s| s.drag).map_or(rot_knob, |d| d.start);
            let degrees = hover_pos.map_or(0.0, |p| turn_degrees_delta(centre, drag_start, p, shift));
            let (sin, cos) = degrees.to_radians().sin_cos();
            let turn = |c: Pos2| {
                let v = c - centre;
                centre + egui::vec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
            };
            let outline = obb.corners().map(turn).to_vec();
            painter.add(egui::Shape::closed_line(outline, Stroke::new(1.5, ACCENT)));
            let galley = painter.layout_no_wrap(format!("{degrees:.0}\u{b0}"), egui::FontId::proportional(12.0), ACCENT);
            let label = Rect::from_center_size(turn(rot_knob) - egui::vec2(0.0, 14.0), galley.size() + egui::vec2(8.0, 4.0));
            painter.rect(label, CornerRadius::same(3), Color32::WHITE, Stroke::new(1.0, ACCENT), egui::StrokeKind::Middle);
            painter.galley(label.center() - galley.size() / 2.0, galley, ACCENT);
            view.cursor.claim(Layer::Drag, egui::CursorIcon::Grabbing);
            if resp.drag_stopped() || !ptr.primary_down {
                if let Some(s) = view.image_selection.as_mut() {
                    s.drag = None;
                    s.turning = false;
                }
                if degrees.abs() >= 0.05 {
                    view.pending_edit =
                        Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::RotateBy(f64::from(degrees)) });
                }
            }
            return true;
        }

        if let Some(drag) = view.image_selection.as_ref().and_then(|s| s.drag)
            && let Some(p) = hover_pos
        {
            if !obb.is_rotated && !matches!(drag.target, DragTarget::Rotate) {
                let preview = match drag.target {
                    DragTarget::Handle(handle) => {
                        let handle_id = HandleId::from(handle);
                        let delta = p - drag.start;
                        let is_corner = matches!(handle_id, HandleId::TopLeft | HandleId::TopRight | HandleId::BottomLeft | HandleId::BottomRight);
                        let shift = ui.input(|inp| inp.modifiers.shift);
                        if is_corner && !shift {
                            apply_resize_proportional(drag.initial_rect, handle_id, delta)
                        } else {
                            apply_resize(drag.initial_rect, handle_id, delta)
                        }
                    }
                    DragTarget::Move => drag.initial_rect.translate(p - drag.start),
                    _ => drag.initial_rect,
                };

                let raw = images.get(i).map_or(b, |im| xf.user_rect(info, page, im.rect.map(|v| v as f32)));
                let ratio = |new: f32, old: f32| if old.abs() > 1e-3 && (new / old).is_finite() { new / old } else { 1.0 };
                let factor = egui::vec2(ratio(preview.width(), b.width()), ratio(preview.height(), b.height()));
                let f_min = egui::vec2((raw.min.x - b.min.x) * factor.x, (raw.min.y - b.min.y) * factor.y);
                let f_max = egui::vec2((raw.max.x - b.min.x) * factor.x, (raw.max.y - b.min.y) * factor.y);
                let target = Rect::from_min_max(preview.min + f_min, preview.min + f_max);
                let visible = preview.intersect(xf.rect);

                if visible.is_positive() {
                    if preview != b
                        && let Some(tex) = raster
                    {
                        let source = left_at.filter(|_| raster_old).map_or(b, screen);
                        let blank: Vec<Rect> = left_at.map(screen).into_iter().chain(std::iter::once(b)).collect();
                        stretched_picture(painter, xf, tex, &blank, source, preview);
                    }
                    painter.rect_filled(visible, CornerRadius::ZERO, ACCENT.gamma_multiply(0.04));
                    painter.rect_stroke(visible, CornerRadius::ZERO, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
                    for h in [
                        preview.left_top(),
                        preview.right_top(),
                        preview.left_bottom(),
                        preview.right_bottom(),
                        preview.center_top(),
                        preview.right_center(),
                        preview.center_bottom(),
                        preview.left_center(),
                    ] {
                        draw_handle(painter, h, false, false, false, ACCENT);
                    }
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

                let icon = match drag.target {
                    DragTarget::Handle(handle) => HandleId::from(handle).cursor_icon(),
                    DragTarget::Move => egui::CursorIcon::Grabbing,
                    _ => egui::CursorIcon::Move,
                };
                view.cursor.claim(Layer::Drag, icon);
                return true;
            }
            let m_init = Matrix(drag.initial_matrix);
            let [a0, b0, c0, d0, e0, f0] = drag.initial_matrix;
            let w0 = (a0 * a0 + b0 * b0).sqrt().max(1e-6);
            let h0 = (c0 * c0 + d0 * d0).sqrt().max(1e-6);
            let u_axis = (a0 / w0, b0 / w0);
            let v_axis = (c0 / h0, d0 / h0);
            let center_user = (e0 + 0.5 * (a0 + c0), f0 + 0.5 * (b0 + d0));

            let (sx, sy) = screen_to_user(xf, info, page, drag.start);
            let (px, py) = screen_to_user(xf, info, page, p);

            let (m_new, cursor_icon) = match drag.target {
                DragTarget::Rotate => {
                    let v0 = (sx - center_user.0, sy - center_user.1);
                    let v1 = (px - center_user.0, py - center_user.1);
                    let theta0 = v0.1.atan2(v0.0);
                    let theta1 = v1.1.atan2(v1.0);
                    let mut delta = theta1 - theta0;
                    if ui.input(|i| i.modifiers.shift) {
                        let step = std::f64::consts::PI / 12.0;
                        delta = (delta / step).round() * step;
                    }
                    let (cos_d, sin_d) = (delta.cos(), delta.sin());
                    let t_rot = [
                        cos_d,
                        sin_d,
                        -sin_d,
                        cos_d,
                        center_user.0 * (1.0 - cos_d) + center_user.1 * sin_d,
                        center_user.1 * (1.0 - cos_d) - center_user.0 * sin_d,
                    ];
                    (m_init.then(&Matrix(t_rot)), egui::CursorIcon::Grab)
                }
                DragTarget::Move => {
                    let (dx, dy) = (px - sx, py - sy);
                    (Matrix([a0, b0, c0, d0, e0 + dx, f0 + dy]), egui::CursorIcon::Move)
                }
                DragTarget::Handle(handle) => {
                    let p00 = (e0, f0);
                    let p10 = (e0 + a0, f0 + b0);
                    let p11 = (e0 + a0 + c0, f0 + b0 + d0);
                    let p01 = (e0 + c0, f0 + d0);
                    let p_curr = (px, py);

                    let (new_w, new_h, p00_new, icon) = match handle {
                        ImageHandle::TopRight => {
                            let anchor = p00;
                            let d = (p_curr.0 - anchor.0, p_curr.1 - anchor.1);
                            let du = d.0 * u_axis.0 + d.1 * u_axis.1;
                            let dv = d.0 * v_axis.0 + d.1 * v_axis.1;
                            let su = du / w0;
                            let sv = dv / h0;
                            let s = if (su - 1.0).abs() >= (sv - 1.0).abs() { su } else { sv };
                            let s = s.max(5.0 / w0).max(5.0 / h0);
                            (w0 * s, h0 * s, anchor, egui::CursorIcon::ResizeNeSw)
                        }
                        ImageHandle::TopLeft => {
                            let anchor = p10;
                            let d = (p_curr.0 - anchor.0, p_curr.1 - anchor.1);
                            let du = d.0 * (-u_axis.0) + d.1 * (-u_axis.1);
                            let dv = d.0 * v_axis.0 + d.1 * v_axis.1;
                            let su = du / w0;
                            let sv = dv / h0;
                            let s = if (su - 1.0).abs() >= (sv - 1.0).abs() { su } else { sv };
                            let s = s.max(5.0 / w0).max(5.0 / h0);
                            let nw = w0 * s;
                            let nh = h0 * s;
                            (nw, nh, (anchor.0 - nw * u_axis.0, anchor.1 - nw * u_axis.1), egui::CursorIcon::ResizeNwSe)
                        }
                        ImageHandle::BottomLeft => {
                            let anchor = p11;
                            let d = (p_curr.0 - anchor.0, p_curr.1 - anchor.1);
                            let du = d.0 * (-u_axis.0) + d.1 * (-u_axis.1);
                            let dv = d.0 * (-v_axis.0) + d.1 * (-v_axis.1);
                            let su = du / w0;
                            let sv = dv / h0;
                            let s = if (su - 1.0).abs() >= (sv - 1.0).abs() { su } else { sv };
                            let s = s.max(5.0 / w0).max(5.0 / h0);
                            let nw = w0 * s;
                            let nh = h0 * s;
                            (
                                nw,
                                nh,
                                (anchor.0 - nw * u_axis.0 - nh * v_axis.0, anchor.1 - nw * u_axis.1 - nh * v_axis.1),
                                egui::CursorIcon::ResizeNeSw,
                            )
                        }
                        ImageHandle::BottomRight => {
                            let anchor = p01;
                            let d = (p_curr.0 - anchor.0, p_curr.1 - anchor.1);
                            let du = d.0 * u_axis.0 + d.1 * u_axis.1;
                            let dv = d.0 * (-v_axis.0) + d.1 * (-v_axis.1);
                            let su = du / w0;
                            let sv = dv / h0;
                            let s = if (su - 1.0).abs() >= (sv - 1.0).abs() { su } else { sv };
                            let s = s.max(5.0 / w0).max(5.0 / h0);
                            let nw = w0 * s;
                            let nh = h0 * s;
                            (nw, nh, (anchor.0 - nh * v_axis.0, anchor.1 - nh * v_axis.1), egui::CursorIcon::ResizeNwSe)
                        }
                        ImageHandle::RightCenter => {
                            let anchor = p00;
                            let d = (p_curr.0 - anchor.0, p_curr.1 - anchor.1);
                            let du = (d.0 * u_axis.0 + d.1 * u_axis.1).max(5.0);
                            (du, h0, anchor, egui::CursorIcon::ResizeHorizontal)
                        }
                        ImageHandle::LeftCenter => {
                            let anchor = p10;
                            let d = (p_curr.0 - anchor.0, p_curr.1 - anchor.1);
                            let du = (d.0 * (-u_axis.0) + d.1 * (-u_axis.1)).max(5.0);
                            (du, h0, (anchor.0 - du * u_axis.0, anchor.1 - du * u_axis.1), egui::CursorIcon::ResizeHorizontal)
                        }
                        ImageHandle::TopCenter => {
                            let anchor = p00;
                            let d = (p_curr.0 - anchor.0, p_curr.1 - anchor.1);
                            let dv = (d.0 * v_axis.0 + d.1 * v_axis.1).max(5.0);
                            (w0, dv, anchor, egui::CursorIcon::ResizeVertical)
                        }
                        ImageHandle::BottomCenter => {
                            let anchor = p01;
                            let d = (p_curr.0 - anchor.0, p_curr.1 - anchor.1);
                            let dv = (d.0 * (-v_axis.0) + d.1 * (-v_axis.1)).max(5.0);
                            (w0, dv, (anchor.0 - dv * v_axis.0, anchor.1 - dv * v_axis.1), egui::CursorIcon::ResizeVertical)
                        }
                    };
                    let mat = Matrix([new_w * u_axis.0, new_w * u_axis.1, new_h * v_axis.0, new_h * v_axis.1, p00_new.0, p00_new.1]);
                    (mat, icon)
                }
                _ => (m_init, egui::CursorIcon::Default),
            };

            let preview_obb = ImageObb::new(m_new.0, xf, info, page);
            if let Some(tex) = view.page_raster(page) {
                let clipped = painter.with_clip_rect(painter.clip_rect().intersect(xf.rect));
                let mut mesh = egui::Mesh::with_texture(tex);
                let orig_corners = obb.corners();
                let new_corners = preview_obb.corners();
                for (from, to) in [
                    (orig_corners[3], new_corners[3]),
                    (orig_corners[2], new_corners[2]),
                    (orig_corners[1], new_corners[1]),
                    (orig_corners[0], new_corners[0]),
                ] {
                    let (u, v) = xf.screen_to_norm(from);
                    mesh.vertices.push(egui::epaint::Vertex { pos: to, uv: Pos2::new(u, v), color: Color32::WHITE });
                }
                mesh.add_triangle(0, 1, 2);
                mesh.add_triangle(0, 2, 3);
                clipped.add(egui::Shape::mesh(mesh));
            }

            if preview_obb.is_rotated {
                let pts = preview_obb.corners();
                painter.polygon(pts.to_vec(), ACCENT.gamma_multiply(0.04), Stroke::new(1.5, ACCENT));
            } else {
                let p_rect = Rect::from_two_pos(preview_obb.s01, preview_obb.s10);
                let visible = p_rect.intersect(xf.rect);
                if visible.is_positive() {
                    painter.rect_filled(visible, CornerRadius::ZERO, ACCENT.gamma_multiply(0.04));
                    painter.rect_stroke(visible, CornerRadius::ZERO, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
                }
            }
            for (_, h) in preview_obb.handles() {
                draw_handle(painter, h, false, false, false, ACCENT);
            }

            if resp.drag_stopped() || !ptr.primary_down {
                if let Some(s) = view.image_selection.as_mut() {
                    s.drag = None;
                }
                if m_new.0 != drag.initial_matrix {
                    if matches!(drag.target, DragTarget::Move) {
                        let (dx, dy) = (px - sx, py - sy);
                        let new_rect = [image.rect[0] + dx, image.rect[1] + dy, image.rect[2] + dx, image.rect[3] + dy];
                        view.pending_edit = Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::Move(new_rect) });
                    } else if let Some(inv) = Matrix(drag.initial_matrix).invert() {
                        let t = inv.then(&m_new);
                        view.pending_edit = Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::Transform(t.0) });
                    }
                }
            }
            view.cursor.claim(Layer::Drag, cursor_icon);
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
    let menu_key = image_menu_id();
    let Some(p) = hover_pos.filter(|p| xf.rect.contains(*p)) else { return false };
    let Some(hit) = boxes.iter().enumerate().rposition(|(i, b)| {
        b.is_positive()
            && images.get(i).is_some_and(|img| b.contains(p) || quad_contains(ImageObb::from_image(img, xf, info, page).corners(), p))
            && image_contains(i, p)
    }) else {
        if resp.secondary_clicked() || resp.clicked() {
            view.image_selection = None;
            ui.ctx().data_mut(|d| d.insert_persisted(menu_key, false));
        }
        return false;
    };
    let Some(hit_img) = images.get(hit) else { return false };
    let hit_obb = ImageObb::from_image(hit_img, xf, info, page);
    if selected != Some(hit) {
        if hit_obb.is_rotated {
            let pts = hit_obb.corners();
            for k in 0..4 {
                painter.line_segment([pts[k], pts[(k + 1) % 4]], Stroke::new(1.5, ACCENT.gamma_multiply(0.7)));
            }
        } else if let Some(b) = boxes.get(hit).copied() {
            painter.rect_stroke(b, CornerRadius::ZERO, Stroke::new(1.5, ACCENT.gamma_multiply(0.7)), egui::StrokeKind::Outside);
        }
    }
    let corner = hit_obb.handles().iter().position(|(_, c)| c.distance(p) < 10.0);
    let cursor = match (selected == Some(hit), corner) {
        (true, Some(0 | 4)) => egui::CursorIcon::ResizeNwSe,
        (true, Some(2 | 6)) => egui::CursorIcon::ResizeNeSw,
        (true, Some(1 | 5)) => egui::CursorIcon::ResizeVertical,
        (true, Some(3 | 7)) => egui::CursorIcon::ResizeHorizontal,
        (true, None) => egui::CursorIcon::Grab,
        _ => egui::CursorIcon::PointingHand,
    };
    view.cursor.claim(Layer::Text, cursor);
    let clicked = match view.router.owner(page) {
        Some(Owner::Picture { index, .. }) if resp.clicked() => Some(index),
        _ => (resp.clicked() || resp.secondary_clicked()).then_some(hit),
    };
    if let Some(index) = clicked {
        view.image_selection =
            Some(ImageSelection { page, index, drag: None, crop: None, crop_mode: false, crop_rect: None, turning: false, settled: None });
    }
    if resp.secondary_clicked() {
        ui.ctx().data_mut(|d| d.insert_persisted(menu_key, true));
    }
    true
}

/// The right-click context menu for a selected image.
#[allow(clippy::too_many_arguments)]
pub(crate) fn image_context_menu(
    ui: &mut egui::Ui,
    view: &mut DocView,
    images: &[pdfcraft_engine::PageImage],
    page: usize,
    hit: usize,
    can_paste: bool,
    action: &mut Option<ImageAction>,
    at: Option<[f64; 2]>,
) {
    let Some(img) = images.get(hit) else { return };
    use pdfcraft_engine::ImageEdit as E;
    let raster = !img.is_form;

    let close = |ui: &mut egui::Ui| {
        ui.ctx().data_mut(|d| d.insert_persisted(image_menu_id(), false));
        egui::Popup::close_all(ui.ctx());
        ui.close();
    };

    // Top horizontal icon bar: Comment, Crop, Rotate CCW, Rotate CW
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if crate::icons::button(ui, "message-square", 24.0, false, tl!("Add a sticky note here")).clicked() {
            *action = Some(ImageAction::AddComment(page, hit));
            close(ui);
        }
        if raster && crate::icons::button(ui, "crop", 24.0, false, tl!("Crop")).clicked() {
            *action = Some(ImageAction::Crop);
            if let Some(s) = view.image_selection.as_mut() {
                let rect = img.visible_rect.unwrap_or(img.rect);
                s.crop = Some(CropMode::new(rect));
                s.crop_mode = true;
                s.crop_rect = None;
            }
            close(ui);
        }
        if crate::icons::button(ui, "rotate-ccw", 24.0, false, tl!("Rotate Counterclockwise")).clicked() {
            view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Rotate(3) });
            close(ui);
        }
        if crate::icons::button(ui, "rotate-cw", 24.0, false, tl!("Rotate Clockwise")).clicked() {
            view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Rotate(1) });
            close(ui);
        }
    });

    ui.separator();

    if ui.button(tl!("Cut Image")).clicked() {
        *action = Some(ImageAction::Cut(page, hit));
        close(ui);
    }
    if ui.button(tl!("Copy Image")).clicked() {
        *action = Some(ImageAction::Copy(page, hit));
        close(ui);
    }
    if ui.add_enabled(can_paste, egui::Button::new(tl!("Paste"))).clicked() {
        *action = Some(ImageAction::Paste(page, at));
        close(ui);
    }

    ui.separator();

    let is_cropped = img.visible_rect.is_some() && img.visible_rect != Some(img.rect);
    if raster && ui.add_enabled(is_cropped, egui::Button::new(tl!("Remove crop"))).clicked() {
        view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::RemoveCrop });
        close(ui);
    }

    if ui.button(tl!("Flip horizontal")).clicked() {
        view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Flip { horizontal: true } });
        close(ui);
    }
    if ui.button(tl!("Flip vertical")).clicked() {
        view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Flip { horizontal: false } });
        close(ui);
    }

    ui.menu_button(tl!("Arrange image"), |ui| {
        if ui.button(tl!("Bring to front")).clicked() {
            view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Arrange(pdfcraft_engine::ArrangeOrder::BringToFront) });
            close(ui);
        }
        if ui.button(tl!("Send to back")).clicked() {
            view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Arrange(pdfcraft_engine::ArrangeOrder::SendToBack) });
            close(ui);
        }
        if ui.button(tl!("Bring forward")).clicked() {
            view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Arrange(pdfcraft_engine::ArrangeOrder::BringForward) });
            close(ui);
        }
        if ui.button(tl!("Send backward")).clicked() {
            view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Arrange(pdfcraft_engine::ArrangeOrder::SendBackward) });
            close(ui);
        }
    });

    if ui.add_enabled(raster, egui::Button::new(tl!("Replace image"))).clicked() {
        *action = Some(ImageAction::Replace(page, hit));
        close(ui);
    }
    if ui.add_enabled(raster, egui::Button::new(tl!("Save Image As…"))).clicked() {
        *action = Some(ImageAction::Save(page, hit));
        close(ui);
    }

    ui.separator();

    if ui.button(tl!("Draw freehand")).clicked() {
        *action = Some(ImageAction::DrawFreehand);
        close(ui);
    }
    if ui.button(tl!("Redact Image")).clicked() {
        *action = Some(ImageAction::Redact(page, hit));
        close(ui);
    }
    if ui.add_enabled(raster, egui::Button::new(tl!("Export image"))).clicked() {
        *action = Some(ImageAction::Save(page, hit));
        close(ui);
    }

    ui.separator();

    if ui.button(tl!("Create Link…")).clicked() {
        *action = Some(ImageAction::Link(page, hit));
        close(ui);
    }

    ui.menu_button(tl!("Edit image using"), |ui| {
        ui.add_enabled(false, egui::Button::new(tl!("Default image editor")));
    });

    if ui.button(tl!("Recognize Text…")).clicked() {
        *action = Some(ImageAction::Recognize(page));
        close(ui);
    }

    ui.separator();

    if ui.button(tl!("Delete")).clicked() {
        view.image_selection = None;
        view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Delete });
        close(ui);
    }
}
