//! Corner rotation belongs to one document generation. Only release commits an edit;
//! Escape, focus loss, a modal or another edit cancels the preview.
use crate::canvas::{DocView, PageXform};
use egui::{Color32, Pos2, Rect, Stroke};
use pdfcraft_engine::{Document, Edit, EditableObject, ObjectKind, ObjectTarget};

const BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);
const REACH: f32 = 11.0;

#[derive(Default)]
pub(crate) struct Rotation {
    generation: Option<u64>,
    cache: std::collections::HashMap<usize, Vec<EditableObject>>,
    selected: Option<(usize, ObjectTarget)>,
    gesture: Option<Gesture>,
    pending: Option<(usize, EditableObject)>,
}

struct Gesture {
    page: usize,
    object: ObjectTarget,
    start: Pos2,
    rect: Rect,
    degrees: f32,
    mode: DragMode,
    preview: Rect,
}

#[derive(Clone, Copy)]
enum DragMode {
    Rotate,
    Move,
    Scale { fixed: Pos2 },
}

/// Apply pending text first, then start a gesture only against a current object inventory.
fn start_gesture(view: &mut DocView, doc: &Document, gesture: Gesture) {
    if view.content.hold_blocked() {
        return;
    }
    // Apply a draft first: that edit can renumber the inventory. Never transform stale targets.
    if let Some(edit) = view.content.finish_draft(doc.added()) {
        view.pending_edit = Some(edit);
        return;
    }
    if let Some(e) = view.line_editor.take()
        && e.has_changes()
    {
        let style = e.style();
        view.pending_edit = Some(Edit::EditTextBlock { page: e.page, block: e.block, text: e.text, style });
        return;
    }
    view.block_drag = None;
    view.object_rotation.gesture = Some(gesture);
}

/// Uniform screen-space scaling around the opposite corner, with no jump for an off-center press.
fn scale_preview(rect: Rect, start: Pos2, now: Pos2, fixed: Pos2) -> Rect {
    // Measure from the press, not the handle centre, so an off-centre grab never jumps.
    let horizontal = (now.x - start.x) * (start.x - fixed.x).signum() / rect.width();
    let vertical = (now.y - start.y) * (start.y - fixed.y).signum() / rect.height();
    let delta = if horizontal.abs() > vertical.abs() { horizontal } else { vertical };
    let k = (1.0 + delta).clamp(0.01, 100.0);
    Rect::from_min_max(fixed + (rect.min - fixed) * k, fixed + (rect.max - fixed) * k)
}

/// Paint a square scale grip separately from the curved rotation control.
fn scale_handle(painter: &egui::Painter, at: Pos2) {
    painter.rect(Rect::from_center_size(at, egui::vec2(8.0, 8.0)), 0, Color32::WHITE, Stroke::new(1.0, BLUE), egui::StrokeKind::Middle);
}

impl Rotation {
    /// Cache at most eight pages of geometry; edits invalidate it in `prepare`.
    fn cached_object(&mut self, doc: &Document, page: usize, target: ObjectTarget) -> Option<EditableObject> {
        if self.cache.len() >= 8 && !self.cache.contains_key(&page) {
            self.cache.clear();
        }
        self.cache.entry(page).or_insert_with(|| doc.editable_objects(page).unwrap_or_default()).iter().find(|o| o.target == target).cloned()
    }

    /// Discard the uncommitted gesture without mutating the document.
    pub fn cancel(&mut self) {
        self.gesture = None;
    }
    /// Whether the transform handler owns the current gesture on this page.
    pub fn dragging(&self, page: usize) -> bool {
        self.gesture.as_ref().is_some_and(|g| g.page == page)
    }

    /// Invalidate cached geometry on edits and cancel gestures when input or focus is unavailable.
    pub fn prepare(&mut self, doc: &Document, enabled: bool, ctx: &egui::Context) {
        if !enabled || !ctx.input(|i| i.focused) {
            self.gesture = None;
            self.selected = None;
            self.pending = None;
        }
        if self.generation != Some(doc.edit_generation()) {
            self.cache.clear();
            self.gesture = None;
            self.selected = self.pending.take().and_then(|(page, old)| {
                let objects = doc.editable_objects(page).ok()?;
                // Added and image inventory order is stable; source paragraphs may regroup.
                let mut matches = objects.iter().filter(|o| {
                    o.target.kind == old.target.kind
                        && o.content_type == old.content_type
                        && o.text == old.text
                        && (old.target.kind == ObjectKind::Text || o.target.index == old.target.index)
                });
                let found = matches.next()?;
                if matches.next().is_some() {
                    return None;
                }
                Some((page, found.target))
            });
            self.generation = Some(doc.edit_generation());
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.gesture = None;
            self.selected = None;
        }
    }
}

/// Screen-space corners in clockwise order, starting at the upper left.
fn corners(r: Rect) -> [Pos2; 4] {
    [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()]
}

/// Place rotation controls outside each corner, clear of the scale grips.
fn handles(r: Rect) -> [Pos2; 4] {
    corners(r).map(|p| p + egui::vec2((p.x - r.center().x).signum() * 16.0, (p.y - r.center().y).signum() * 16.0))
}

/// Rotate a screen-space point clockwise about the gesture pivot.
fn turned(p: Pos2, pivot: Pos2, degrees: f32) -> Pos2 {
    let (s, c) = degrees.to_radians().sin_cos();
    let d = p - pivot;
    pivot + egui::vec2(c * d.x - s * d.y, s * d.x + c * d.y)
}

/// Signed pointer angle with optional 15-degree snapping and a stable center dead zone.
fn angle(start: Pos2, now: Pos2, pivot: Pos2, snap: bool) -> f32 {
    let a = start - pivot;
    let b = now - pivot;
    if a.length_sq() < 1.0 || b.length_sq() < 1.0 {
        return 0.0;
    }
    let degrees = (a.x * b.y - a.y * b.x).atan2(a.dot(b)).to_degrees();
    if snap { (degrees / 15.0).round() * 15.0 } else { degrees }
}

/// Draw the curved rotation control with the orientation of its corner.
fn arrow(painter: &egui::Painter, at: Pos2, hot: bool, orientation: f32) {
    painter.circle_filled(at, 10.0, if hot { Color32::from_rgb(220, 235, 255) } else { Color32::WHITE });
    let points: Vec<_> = (0..=16)
        .map(|i| {
            let a = (-150.0 + orientation + i as f32 * 16.0).to_radians();
            at + egui::vec2(a.cos(), a.sin()) * 6.0
        })
        .collect();
    if let Some(end) = points.last().copied() {
        let tip = (106.0 + orientation).to_radians();
        let tangent = egui::vec2(-tip.sin(), tip.cos());
        let normal = egui::vec2(tip.cos(), tip.sin());
        painter.line_segment([end, end - tangent * 4.0 + normal * 3.0], Stroke::new(1.5, BLUE));
        painter.line_segment([end, end - tangent * 4.0 - normal * 3.0], Stroke::new(1.5, BLUE));
    }
    painter.add(egui::Shape::line(points, Stroke::new(1.5, BLUE)));
}

/// Resolve the active editor or Select-tool hit to one current editable object.
fn selected_target(view: &DocView, doc: &Document, page: usize) -> Option<ObjectTarget> {
    view.content
        .selected
        .filter(|(p, _)| *p == page)
        .map(|(_, i)| ObjectTarget { kind: ObjectKind::Added, index: i })
        .or_else(|| {
            view.image_selection
                .as_ref()
                .filter(|s| s.page == page && s.crop.is_none())
                .map(|s| ObjectTarget { kind: ObjectKind::Image, index: s.index })
        })
        .or_else(|| view.line_editor.as_ref().filter(|e| e.page == page).map(|e| ObjectTarget { kind: ObjectKind::Text, index: e.block }))
        .or_else(|| {
            view.picked_image.filter(|p| p.page == page && p.generation == doc.edit_generation()).map(|p| {
                let owner = doc.page_images(page).get(p.index).and_then(|image| image.content_stream);
                doc.added()
                    .iter()
                    .filter(|a| a.page == page)
                    .position(|a| Some(a.obj) == owner)
                    .map_or(ObjectTarget { kind: ObjectKind::Image, index: p.index }, |index| ObjectTarget { kind: ObjectKind::Added, index })
            })
        })
}

/// Independent handle widgets do not share the page/menu response ID or resize hit area.
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    doc: &Document,
    view: &mut DocView,
    select_tool: bool,
    editing_content: bool,
) -> bool {
    // Preserve the existing move renderer and its release/settling state once it owns a drag.
    if view.content.dragging(page) {
        return false;
    }
    if view.image_selection.as_ref().is_some_and(|s| s.page == page && s.crop.is_some()) {
        view.object_rotation.gesture = None;
        view.object_rotation.selected = None;
        return false;
    }
    if view.objects.count() > 1 || !ui.input(|i| i.focused) {
        return false;
    }
    let single = selected_target(view, doc, page);
    if let Some(target) = single {
        view.object_rotation.selected = Some((page, target));
    }
    let Some((_, target)) = view.object_rotation.selected.filter(|(p, _)| *p == page) else { return false };
    let Some(object) = view.object_rotation.cached_object(doc, page, target) else { return false };
    let bounds = xf.user_rect(&doc.info, page, object.rect.map(|v| v as f32));
    if !bounds.is_finite() || bounds.width() < 1.0 || bounds.height() < 1.0 {
        return false;
    }
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let dragging = view.object_rotation.gesture.as_ref().is_some_and(|g| g.page == page);
    let mut owns = dragging;
    if !dragging {
        // The text editor, image selection and added-content tools already paint a frame.
        if single.is_none() || select_tool && view.picked_image.is_none() {
            ui.painter().rect_stroke(bounds, 0, Stroke::new(1.0, BLUE), egui::StrokeKind::Outside);
        }
        for (corner, at) in handles(bounds).into_iter().enumerate() {
            let hit = ui.interact(
                Rect::from_center_size(at, egui::vec2(REACH * 2.0, REACH * 2.0)),
                resp.id.with(("rotate-corner", target.kind.as_str(), target.index, corner)),
                egui::Sense::drag(),
            );
            arrow(ui.painter(), at, hit.hovered() || hit.dragged(), corner as f32 * 90.0);
            hit.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, crate::i18n::fmt(tl!("Rotate corner {n}"), &[("n", &(corner + 1).to_string())]))
            });
            if hit.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            hit.clone().on_hover_text(tl!("Drag to rotate. Hold Shift for 15° steps; Esc cancels."));
            let pressed = hit.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed());
            owns |= hit.hovered() || hit.is_pointer_button_down_on();
            if pressed && let Some(start) = pointer {
                start_gesture(
                    view,
                    doc,
                    Gesture { page, object: target, start, rect: bounds, degrees: 0.0, mode: DragMode::Rotate, preview: bounds },
                );
            }
        }
    }
    // Existing edit-mode image grips keep their pixel preview. Text corners scale glyphs
    // instead of merely rewrapping the paragraph. Select-tool objects use these same gestures.
    let scale_here = select_tool || single.is_none() || object.content_type == "text";
    if !dragging && view.object_rotation.gesture.is_none() && scale_here {
        for (corner, at) in corners(bounds).into_iter().enumerate() {
            let hit = ui.interact(
                Rect::from_center_size(at, egui::vec2(16.0, 16.0)),
                resp.id.with(("scale-corner", target.kind.as_str(), target.index, corner)),
                egui::Sense::drag(),
            );
            scale_handle(ui.painter(), at);
            hit.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, crate::i18n::fmt(tl!("Scale corner {n}"), &[("n", &(corner + 1).to_string())]))
            });
            if hit.hovered() {
                ui.ctx().set_cursor_icon(if corner % 2 == 0 { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw });
            }
            owns |= hit.hovered() || hit.is_pointer_button_down_on();
            if hit.is_pointer_button_down_on()
                && ui.input(|i| i.pointer.primary_pressed())
                && let Some(start) = pointer
            {
                let fixed = corners(bounds)[(corner + 2) % 4];
                start_gesture(
                    view,
                    doc,
                    Gesture { page, object: target, start, rect: bounds, degrees: 0.0, mode: DragMode::Scale { fixed }, preview: bounds },
                );
            }
        }
    }
    // Only intercept primary drags of an already selected object; right-click remains the
    // page/image menu's response, and unselected text keeps ordinary text selection.
    // Unrotated added content already has a live pixel preview in Edit a PDF.
    // Keep that handler in charge of body drags; our separate corner widgets still own transforms.
    let existing_move = editing_content
        && target.kind == ObjectKind::Added
        && doc.added().iter().filter(|a| a.page == page).nth(target.index).is_some_and(|a| a.rotation == 0.0);
    if !owns && !dragging && !existing_move && (select_tool || single.is_none()) {
        let inset = (bounds.width().min(bounds.height()) * 0.2).min(8.0);
        let inside = |p: Pos2| bounds.shrink(inset).contains(p);
        if pointer.is_some_and(inside) && !ui.input(|i| i.pointer.secondary_down()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
        }
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(start) = ui.input(|i| i.pointer.press_origin()).filter(|p| inside(*p))
        {
            owns = true;
            start_gesture(view, doc, Gesture { page, object: target, start, rect: bounds, degrees: 0.0, mode: DragMode::Move, preview: bounds });
        }
    }
    if let Some(g) = view.object_rotation.gesture.as_mut().filter(|g| g.page == page) {
        owns = true;
        if let Some(now) = pointer {
            match g.mode {
                DragMode::Rotate => g.degrees = angle(g.start, now, g.rect.center(), ui.input(|i| i.modifiers.shift)),
                DragMode::Move => g.preview = g.rect.translate(now - g.start),
                DragMode::Scale { fixed } => g.preview = scale_preview(g.rect, g.start, now, fixed),
            }
        }
        let pivot = g.rect.center();
        let quad = corners(g.preview).map(|p| turned(p, pivot, g.degrees));
        // Geometry-only preview never samples unrelated artwork from the page raster.
        ui.painter().add(egui::Shape::closed_line(quad.to_vec(), Stroke::new(1.5, BLUE)));
        for (corner, at) in handles(g.preview).into_iter().enumerate() {
            arrow(ui.painter(), turned(at, pivot, g.degrees), true, corner as f32 * 90.0 + g.degrees);
        }
        for at in quad {
            scale_handle(ui.painter(), at);
        }
        if matches!(g.mode, DragMode::Rotate) {
            ui.painter().circle_stroke(pivot, 5.0, Stroke::new(1.0, BLUE));
            ui.painter().line_segment([pivot - egui::vec2(8.0, 0.0), pivot + egui::vec2(8.0, 0.0)], Stroke::new(1.0, BLUE));
            ui.painter().line_segment([pivot - egui::vec2(0.0, 8.0), pivot + egui::vec2(0.0, 8.0)], Stroke::new(1.0, BLUE));
            ui.painter().text(
                pivot + egui::vec2(0.0, 16.0),
                egui::Align2::CENTER_TOP,
                format!("{:.1}°", g.degrees),
                egui::FontId::proportional(13.0),
                BLUE,
            );
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if !ui.input(|i| i.pointer.primary_down())
            && let Some(g) = view.object_rotation.gesture.take()
        {
            let edit = match g.mode {
                DragMode::Rotate if g.degrees.abs() > 0.01 => Some(Edit::RotateObject { page, object: g.object, degrees: f64::from(g.degrees) }),
                DragMode::Move if g.preview.min.distance(g.rect.min) > 0.1 => {
                    let before = xf.screen_to_view(g.rect.min);
                    let after = xf.screen_to_view(g.preview.min);
                    doc.object_move_offset(page, [f64::from(after.0 - before.0), f64::from(after.1 - before.1)])
                        .ok()
                        .map(|offset| Edit::MoveObjects { page, objects: vec![g.object], offset })
                }
                DragMode::Scale { fixed } if (g.preview.width() - g.rect.width()).abs() > 0.1 => {
                    let p = xf.screen_to_view(fixed);
                    doc.info.pages.get(page).map(|info| Edit::ScaleObject {
                        page,
                        object: g.object,
                        factor: f64::from(g.preview.width() / g.rect.width()),
                        pivot: info.view_to_user(p.0, p.1).map(f64::from),
                    })
                }
                _ => None,
            };
            if edit.is_some() {
                view.object_rotation.pending = Some((page, object));
                view.pending_edit = edit;
            }
        }
    } else if resp.clicked() && pointer.is_some_and(|p| !bounds.contains(p)) && !owns {
        view.object_rotation.selected = None;
    }
    owns
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Angle deltas stay continuous across the seam and respect snapping and the pivot dead zone.
    #[test]
    fn angles_cross_the_seam_and_snap_without_jumping() {
        let p = Pos2::ZERO;
        let start = Pos2::new(100.0, 0.0);
        assert!((angle(start, Pos2::new(0.0, 100.0), p, false) - 90.0).abs() < 0.001);
        assert_eq!(angle(start, turned(start, p, 22.0), p, true), 15.0);
        assert!((angle(turned(start, p, 179.0), turned(start, p, -179.0), p, false) - 2.0).abs() < 0.001);
        assert_eq!(angle(start, p, p, false), 0.0);
    }
    /// Rotation targets remain outside the neighboring scale-grip hit areas.
    #[test]
    fn all_four_rotation_targets_are_clear_of_resize_corners() {
        let r = Rect::from_min_size(Pos2::ZERO, egui::vec2(200.0, 40.0));
        for (handle, corner) in handles(r).into_iter().zip(corners(r)) {
            assert!(handle.distance(corner) > REACH + 8.0);
        }
    }
    /// Main's control cancellation must discard a pending transform before button cleanup.
    #[test]
    fn control_cancellation_discards_corner_rotation() {
        let mut app = crate::PdfCraftApp::new();
        app.run_inline = true;
        let bytes = app.session.create_blank(300.0, 400.0, 1).unwrap();
        app.open_bytes("rotation-cancel.pdf", None, bytes.as_ref().clone()).unwrap();
        assert!(app.apply_edit(Edit::AddText {
            page: 0,
            text: pdfcraft_engine::AddedText { text: "Cancel rotation".into(), rect: [20.0, 30.0, 180.0, 60.0], ..Default::default() }
        }));
        let id = app.views[0].id;
        let generation = app.session.get(id).unwrap().edit_generation();
        let rect = Rect::from_min_max(Pos2::new(20.0, 30.0), Pos2::new(180.0, 60.0));
        app.views[0].object_rotation.gesture = Some(Gesture {
            page: 0,
            object: ObjectTarget { kind: ObjectKind::Added, index: 0 },
            start: rect.left_top(),
            rect,
            degrees: 45.0,
            mode: DragMode::Rotate,
            preview: rect,
        });
        crate::control::Host::cancel_pointer_gestures(&mut app);
        assert!(app.views[0].object_rotation.gesture.is_none());
        assert_eq!(app.session.get(id).unwrap().edit_generation(), generation);
        assert_eq!(app.session.get(id).unwrap().added()[0].rotation, 0.0);
    }
}
