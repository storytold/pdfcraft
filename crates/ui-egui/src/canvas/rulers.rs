use super::{DocView, PageXform};
use crate::ruler::{self, RulerOrientation, RulerPaintInput};
use crate::{PdfCraftApp, units::Unit};
use egui::{Pos2, Rect, Vec2};

/// Final displayed geometry for one visible page. It is derived from the same snapped transform
/// used to paint the page, so ruler ticks cannot drift from independently rounded page edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct VisiblePageGeometry {
    page_index: usize,
    page_rect: Rect,
    unrotated_size_points: Vec2,
    rotation_degrees: u16,
    screen_points_per_pdf_point_x: f32,
    screen_points_per_pdf_point_y: f32,
}

impl VisiblePageGeometry {
    fn from_xform(page_index: usize, xf: PageXform) -> Self {
        let displayed_width_points = if xf.rot.is_multiple_of(180) { xf.pw } else { xf.ph };
        let displayed_height_points = if xf.rot.is_multiple_of(180) { xf.ph } else { xf.pw };
        Self {
            page_index,
            page_rect: xf.rect,
            unrotated_size_points: egui::vec2(xf.pw, xf.ph),
            rotation_degrees: xf.rot,
            screen_points_per_pdf_point_x: xf.rect.width() / displayed_width_points.max(1.0),
            screen_points_per_pdf_point_y: xf.rect.height() / displayed_height_points.max(1.0),
        }
    }

    fn visible_area(self, viewport: Rect) -> f32 {
        if self.page_rect.is_positive() && viewport.is_positive() {
            let visible = self.page_rect.intersect(viewport);
            visible.width().max(0.0) * visible.height().max(0.0)
        } else {
            0.0
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct CanvasGeometry {
    viewport_rect: Rect,
    pages: Vec<VisiblePageGeometry>,
    active_page_index: Option<usize>,
}

impl CanvasGeometry {
    fn from_view(view: &DocView) -> Self {
        let viewport_rect = view.viewport_screen;
        let pages: Vec<_> = view.screen_xforms.iter().map(|(page, xf)| VisiblePageGeometry::from_xform(*page, *xf)).collect();
        let active_page_index = active_page_geometry(viewport_rect, &pages).map(|page| page.page_index);
        Self { viewport_rect, pages, active_page_index }
    }

    fn active_page(&self) -> Option<&VisiblePageGeometry> {
        let active = self.active_page_index?;
        self.pages.iter().find(|page| page.page_index == active)
    }
}

fn active_page_geometry(viewport: Rect, pages: &[VisiblePageGeometry]) -> Option<&VisiblePageGeometry> {
    if !viewport.is_positive() {
        return None;
    }
    if let Some(page) = pages.iter().find(|page| page.page_rect.contains(viewport.center())) {
        return Some(page);
    }
    pages
        .iter()
        .enumerate()
        .max_by(|(ia, a), (ib, b)| a.visible_area(viewport).total_cmp(&b.visible_area(viewport)).then_with(|| ib.cmp(ia)))
        .map(|(_, page)| page)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RulerLayout {
    corner: Rect,
    horizontal: Rect,
    vertical: Rect,
}

impl RulerLayout {
    fn new(outer: Rect) -> Self {
        let horizontal_height = ruler::HORIZONTAL_RULER_HEIGHT.max(0.0).min(outer.height());
        let vertical_width = ruler::VERTICAL_RULER_WIDTH.max(0.0).min(outer.width());
        let x = outer.left() + vertical_width;
        let y = outer.top() + horizontal_height;
        Self {
            corner: Rect::from_min_max(outer.min, egui::pos2(x, y)),
            horizontal: Rect::from_min_max(egui::pos2(x, outer.top()), egui::pos2(outer.right(), y)),
            vertical: Rect::from_min_max(egui::pos2(outer.left(), y), egui::pos2(x, outer.bottom())),
        }
    }
}

/// Ruler chrome that excludes page gestures for an ordinary document viewport.
pub(super) fn blocked_rects(enabled: bool, presentation: bool, organize: bool, viewport: Rect) -> Vec<Rect> {
    if !enabled || presentation || organize || !viewport.is_positive() {
        return Vec::new();
    }
    let layout = RulerLayout::new(viewport);
    vec![layout.corner, layout.horizontal, layout.vertical]
}

/// The part of `viewport` left for page interaction after its top and left ruler bands.
pub(super) fn page_interaction_rect(enabled: bool, presentation: bool, organize: bool, viewport: Rect) -> Rect {
    if !enabled || presentation || organize || !viewport.is_positive() {
        return viewport;
    }
    let layout = RulerLayout::new(viewport);
    Rect::from_min_max(layout.corner.max, viewport.max)
}

pub(super) fn paint(app: &PdfCraftApp, index: usize, ui: &egui::Ui) {
    let Some(view) = app.views.get(index) else { return };
    if !app.show_rulers || view.presentation.is_some() || view.organize {
        return;
    }
    let pointer = ui.ctx().pointer_hover_pos();
    let geometry = CanvasGeometry::from_view(view);
    if geometry.pages.is_empty() || !geometry.viewport_rect.is_positive() {
        return;
    }
    let Some(axes) = ruler_axes(&geometry) else { return };
    let layout = RulerLayout::new(geometry.viewport_rect);
    paint_rulers(ui, layout, axes, geometry.viewport_rect, app.units, pointer);
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RulerAxes {
    horizontal_origin: f64,
    horizontal_start: f64,
    horizontal_length: f64,
    vertical_origin: f64,
    vertical_length: f64,
    scale_x: f64,
    scale_y: f64,
}

fn paint_rulers(ui: &egui::Ui, layout: RulerLayout, axes: RulerAxes, viewport: Rect, units: Unit, pointer: Option<Pos2>) {
    // Preserve the feature branch's deterministic ruler policy. At small rendered sizes the
    // rulers describe the viewport (a facing spread starts at its left edge). Once an axis fills
    // 72% of its ruler, it becomes page-relative; a facing spread then uses its fixed spine as
    // zero, with negative coordinates on the left and positive coordinates on the right.
    let horizontal_relative = axes.horizontal_length * axes.scale_x >= f64::from(layout.horizontal.width()) * 0.72;
    let vertical_relative = axes.vertical_length * axes.scale_y >= f64::from(layout.vertical.height()) * 0.72;
    let (mut horizontal_origin, horizontal_length) =
        horizontal_ruler_axis(axes.horizontal_origin, axes.horizontal_start, axes.horizontal_length, axes.scale_x, horizontal_relative);
    horizontal_origin = snap_ruler_coordinate(horizontal_origin, ui.ctx().pixels_per_point());
    let (vertical_origin, vertical_length) = if vertical_relative {
        (snap_ruler_coordinate(axes.vertical_origin, ui.ctx().pixels_per_point()), axes.vertical_length)
    } else {
        (f64::from(layout.vertical.top()), f64::from(layout.vertical.height()) / axes.scale_y.max(f64::EPSILON))
    };
    let pointer = pointer.filter(|point| viewport.contains(*point));
    ruler::paint_corner(ui, layout.corner, units);
    ruler::paint_ruler(
        ui,
        RulerPaintInput {
            orientation: RulerOrientation::Horizontal,
            rect: layout.horizontal,
            page_origin: horizontal_origin,
            page_length_pdf_points: horizontal_length,
            screen_points_per_pdf_point: axes.scale_x,
            unit: units,
            pointer_screen_position: pointer.map(|point| f64::from(point.x)),
        },
    );
    ruler::paint_ruler(
        ui,
        RulerPaintInput {
            orientation: RulerOrientation::Vertical,
            rect: layout.vertical,
            page_origin: vertical_origin,
            page_length_pdf_points: vertical_length,
            screen_points_per_pdf_point: axes.scale_y,
            unit: units,
            pointer_screen_position: pointer.map(|point| f64::from(point.y)),
        },
    );
}

/// Selects the feature branch's horizontal ruler origin without involving the pointer.
fn horizontal_ruler_axis(page_origin: f64, page_start: f64, page_length: f64, scale: f64, page_relative: bool) -> (f64, f64) {
    if page_start < 0.0 && !page_relative { (page_origin + page_start * scale, page_length) } else { (page_origin, page_length) }
}

/// Snaps a ruler coordinate to the same physical-pixel grid used by page rectangles.
fn snap_ruler_coordinate(value: f64, pixels_per_point: f32) -> f64 {
    let pixels_per_point = f64::from(pixels_per_point);
    if value.is_finite() && pixels_per_point.is_finite() && pixels_per_point > 0.0 {
        (value * pixels_per_point).round() / pixels_per_point
    } else {
        value
    }
}

/// Selects the active page axes, pairs facing pages on the same row, and derives
/// ruler scale from rendered page widths while excluding the screen-only gutter.
fn ruler_axes(geometry: &CanvasGeometry) -> Option<RulerAxes> {
    let page = geometry.active_page()?;
    let rotated = !page.rotation_degrees.is_multiple_of(180);
    let page_width_points = if rotated { page.unrotated_size_points.y } else { page.unrotated_size_points.x };
    let page_height_points = if rotated { page.unrotated_size_points.x } else { page.unrotated_size_points.y };
    let same_row = |other: &&VisiblePageGeometry| {
        let overlap = (page.page_rect.bottom().min(other.page_rect.bottom()) - page.page_rect.top().max(other.page_rect.top())).max(0.0);
        let side_by_side = other.page_rect.right() <= page.page_rect.left() + 0.5 || other.page_rect.left() >= page.page_rect.right() - 0.5;
        other.page_index != page.page_index && overlap > 0.5 && side_by_side
    };
    let partner = geometry.pages.iter().filter(same_row).min_by(|a, b| {
        let gap = |other: &VisiblePageGeometry| {
            if other.page_rect.center().x < page.page_rect.center().x {
                (page.page_rect.left() - other.page_rect.right()).abs()
            } else {
                (other.page_rect.left() - page.page_rect.right()).abs()
            }
        };
        gap(a).total_cmp(&gap(b))
    });
    let (horizontal_origin, horizontal_start, horizontal_length, rendered_page_width) =
        partner.map_or((page.page_rect.left(), 0.0, page_width_points, page.page_rect.width()), |other| {
            let other_rotated = !other.rotation_degrees.is_multiple_of(180);
            let other_width_points = if other_rotated { other.unrotated_size_points.y } else { other.unrotated_size_points.x };
            let rendered_page_width = page.page_rect.width() + other.page_rect.width();
            if other.page_rect.center().x < page.page_rect.center().x {
                (
                    (other.page_rect.right() + page.page_rect.left()) / 2.0,
                    -other_width_points,
                    other_width_points + page_width_points,
                    rendered_page_width,
                )
            } else {
                (
                    (page.page_rect.right() + other.page_rect.left()) / 2.0,
                    -page_width_points,
                    page_width_points + other_width_points,
                    rendered_page_width,
                )
            }
        });
    let horizontal_length = f64::from(horizontal_length);
    let rendered_page_width = f64::from(rendered_page_width);
    // The facing-page gutter has no PDF-point extent and must not change ruler scale. Pages are
    // snapped independently, so derive one aggregate scale from only their final rendered widths.
    let scale_x = if horizontal_length.is_finite() && horizontal_length > 0.0 && rendered_page_width.is_finite() {
        rendered_page_width / horizontal_length
    } else {
        f64::from(page.screen_points_per_pdf_point_x)
    };
    Some(RulerAxes {
        horizontal_origin: f64::from(horizontal_origin),
        horizontal_start: f64::from(horizontal_start),
        horizontal_length,
        vertical_origin: f64::from(page.page_rect.top()),
        vertical_length: f64::from(page_height_points),
        scale_x,
        scale_y: f64::from(page.screen_points_per_pdf_point_y),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, pos2, vec2};

    fn page(index: usize, rect: Rect, rot: u16, size: Vec2) -> VisiblePageGeometry {
        let xf = PageXform { rect, rot, pw: size.x, ph: size.y };
        VisiblePageGeometry::from_xform(index, xf)
    }

    #[test]
    fn independently_rounded_axes_keep_distinct_scales() {
        let viewport = Rect::from_min_size(Pos2::ZERO, vec2(1_000.0, 900.0));
        let p = page(0, Rect::from_min_size(pos2(100.0, 200.0), vec2(300.0, 401.0)), 0, vec2(600.0, 800.0));
        let geometry = CanvasGeometry { viewport_rect: viewport, pages: vec![p], active_page_index: Some(0) };
        let axes = ruler_axes(&geometry).expect("visible page axes");
        assert!((axes.scale_x - 0.5).abs() < f64::from(f32::EPSILON));
        assert!((axes.scale_y - 0.50125).abs() < f64::from(f32::EPSILON));
    }

    #[test]
    fn mixed_facing_pages_use_one_deterministic_ruler_page() {
        // The viewport centre is in the gutter, and both pages have equal visible area. The lower
        // page index wins independently of pointer position, page size, rotation, or axis scale.
        let viewport = Rect::from_min_max(Pos2::ZERO, pos2(802.0, 700.0));
        let left = page(0, Rect::from_min_size(pos2(100.0, 100.0), vec2(300.0, 400.0)), 0, vec2(600.0, 800.0));
        let right = page(1, Rect::from_min_size(pos2(402.0, 100.0), vec2(300.0, 400.0)), 90, vec2(800.0, 400.0));
        let pages = [left, right];

        let active = active_page_geometry(viewport, &pages).expect("a deterministic ruler page");
        assert_eq!(active.page_index, 0);
        let geometry = CanvasGeometry { viewport_rect: viewport, pages: pages.to_vec(), active_page_index: Some(active.page_index) };
        let axes = ruler_axes(&geometry).expect("facing-page axes");
        assert_eq!((axes.horizontal_start, axes.horizontal_length), (-600.0, 1_000.0));
        assert_eq!(axes.horizontal_origin, 401.0);
        assert_eq!(horizontal_ruler_axis(401.0, -600.0, 1_000.0, 0.5, false), (101.0, 1_000.0));
        assert_eq!(horizontal_ruler_axis(401.0, -600.0, 1_000.0, 0.5, true), (401.0, 1_000.0));
    }

    #[test]
    fn facing_spread_scale_excludes_the_gutter() {
        let viewport = Rect::from_min_size(Pos2::ZERO, vec2(1_000.0, 900.0));
        let left = page(0, Rect::from_min_size(pos2(100.0, 100.0), vec2(299.0, 400.0)), 0, vec2(600.0, 800.0));

        let axes_for_gap = |gap: f32| {
            let right = page(1, Rect::from_min_size(pos2(399.0 + gap, 100.0), vec2(201.0, 400.0)), 0, vec2(400.0, 800.0));
            let rendered_width = f64::from(right.page_rect.right() - left.page_rect.left());
            let geometry = CanvasGeometry { viewport_rect: viewport, pages: vec![left, right], active_page_index: Some(0) };
            (ruler_axes(&geometry).expect("facing-page axes"), rendered_width)
        };
        let (axes, rendered_width) = axes_for_gap(2.0);
        let (wider_gap_axes, wider_gap_rendered_width) = axes_for_gap(12.0);

        assert_eq!(axes.horizontal_length, 1_000.0);
        assert_eq!(rendered_width, 502.0);
        assert!((axes.scale_x - 0.5).abs() < f64::EPSILON);
        assert!((wider_gap_axes.scale_x - axes.scale_x).abs() < f64::EPSILON);
        assert_eq!(wider_gap_rendered_width, 512.0);

        // The gap is screen chrome, not document width. Increasing only the gap
        // must not switch the horizontal ruler into page-relative mode.
        let ruler_width = 700.0_f64;
        let relative_threshold = ruler_width * 0.72;
        assert!(axes.horizontal_length * axes.scale_x < relative_threshold);
        assert!(wider_gap_axes.horizontal_length * wider_gap_axes.scale_x < relative_threshold);
        assert!(wider_gap_rendered_width >= relative_threshold);
        let left_edge = axes.horizontal_origin + axes.horizontal_start * axes.scale_x;
        let right_edge = left_edge + axes.horizontal_length * axes.scale_x;
        assert!((left_edge - 100.0).abs() < 1e-9);
        assert!((right_edge - 600.0).abs() < 1e-9);
    }

    #[test]
    fn layout_overlays_the_viewport_without_resizing_it() {
        let viewport = Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 600.0));
        let layout = RulerLayout::new(viewport);
        assert_eq!(layout.corner.min, viewport.min);
        assert_eq!(layout.horizontal.right(), viewport.right());
        assert_eq!(layout.vertical.bottom(), viewport.bottom());
    }

    #[test]
    fn blocked_rects_match_the_painted_bands_only_when_eligible() {
        let viewport = Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 600.0));
        let layout = RulerLayout::new(viewport);
        assert_eq!(blocked_rects(true, false, false, viewport), vec![layout.corner, layout.horizontal, layout.vertical]);
        assert!(blocked_rects(false, false, false, viewport).is_empty());
        assert!(blocked_rects(true, true, false, viewport).is_empty());
        assert!(blocked_rects(true, false, true, viewport).is_empty());
        assert!(blocked_rects(true, false, false, Rect::NOTHING).is_empty());
    }

    #[test]
    fn page_interaction_rect_excludes_ruler_bands_only_when_eligible() {
        let viewport = Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 600.0));
        let layout = RulerLayout::new(viewport);
        assert_eq!(page_interaction_rect(true, false, false, viewport), Rect::from_min_max(layout.corner.max, viewport.max));
        assert_eq!(page_interaction_rect(false, false, false, viewport), viewport);
        assert_eq!(page_interaction_rect(true, true, false, viewport), viewport);
        assert_eq!(page_interaction_rect(true, false, true, viewport), viewport);
        assert_eq!(page_interaction_rect(true, false, false, Rect::NOTHING), Rect::NOTHING);
    }
}
