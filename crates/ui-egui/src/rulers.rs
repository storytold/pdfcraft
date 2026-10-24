//! Canvas rulers along the top and left of the document viewport (`view.rulers`).
//!
//! Rulers provide dynamic measurement markings (pt, in, mm, cm) anchored to the active page
//! origin `(0, 0)` under all view rotations, and show guide indicators that track the cursor
//! position in real time.

use crate::theme::{self, Tokens};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, pos2, vec2};

/// Width and height in points of the horizontal and vertical ruler bars.
pub const RULER_THICKNESS: f32 = 18.0;

/// Calculates the major tick interval and number of minor subdivisions for the ruler.
///
/// Returns a tuple of `(major_step, minor_subdivisions)` in the specified [`crate::units::Unit`],
/// adapted dynamically to `screen_scale` (in logical screen pixels per PDF point) such that major
/// tick labels are comfortably readable and neither too crowded nor too sparse on screen.
pub fn calculate_ruler_step(screen_scale: f32, unit: crate::units::Unit) -> (f64, usize) {
    let s = if screen_scale.is_finite() && screen_scale > 0.001 { screen_scale as f64 } else { 1.0 };
    let per_point = unit.per_point();
    let per_point = if per_point.is_finite() && per_point > 0.0 { per_point } else { 1.0 };

    // Physical screen pixels per 1 unit = (points per unit) * (screen pixels per point).
    let screen_pixels_per_unit = (1.0 / per_point) * s;
    if !screen_pixels_per_unit.is_finite() || screen_pixels_per_unit <= 0.0 {
        return (10.0, 10);
    }

    // Target between 50.0 and 120.0 screen pixels between major numerical tick labels.
    const MIN_SCREEN_STEP: f64 = 50.0;
    let target_units = MIN_SCREEN_STEP / screen_pixels_per_unit;

    // Find the base order of magnitude.
    let exponent = target_units.log10().floor() as i32;
    let power_of_ten = 10.0_f64.powi(exponent);
    let fraction = target_units / power_of_ten;

    let (step, subdivisions) = if fraction <= 1.0 {
        (1.0 * power_of_ten, 10)
    } else if fraction <= 2.0 {
        (2.0 * power_of_ten, 4)
    } else if fraction <= 5.0 {
        (5.0 * power_of_ten, 5)
    } else {
        (10.0 * power_of_ten, 10)
    };

    // For inches at natural scale, use 8 subdivisions (eighths of an inch).
    let subdivisions = if unit == crate::units::Unit::Inches && (step - 1.0).abs() < 1e-4 { 8 } else { subdivisions };

    (step, subdivisions)
}

/// Formats a ruler tick coordinate value as a concise, readable string.
///
/// Integers are formatted without decimal points, while fractional steps
/// format with the minimum number of decimal places needed.
pub fn format_tick_label(val: f64, _step: f64) -> String {
    if val.abs() < 1e-6 {
        return "0".to_string();
    }
    let fract = val.fract().abs();
    if fract < 1e-4 || (1.0 - fract).abs() < 1e-4 {
        format!("{:.0}", val)
    } else {
        let fract10 = (val * 10.0).fract().abs();
        if fract10 < 1e-4 || (1.0 - fract10).abs() < 1e-4 {
            format!("{:.1}", val)
        } else {
            let fract100 = (val * 100.0).fract().abs();
            if fract100 < 1e-4 || (1.0 - fract100).abs() < 1e-4 { format!("{:.2}", val) } else { format!("{:.3}", val) }
        }
    }
}

/// Draws the canvas rulers and cursor tracking indicators for the document view.
///
/// Returns `Some(Unit)` if the user clicked the corner unit badge to cycle measurement units,
/// or `None` if the active unit remains unchanged.
pub fn draw_rulers(
    ui: &mut egui::Ui,
    view: &crate::canvas::DocView,
    viewport: Rect,
    units: crate::units::Unit,
    t: &Tokens,
) -> Option<crate::units::Unit> {
    if !viewport.is_positive() {
        return None;
    }

    let corner_rect = Rect::from_min_size(viewport.min, vec2(RULER_THICKNESS, RULER_THICKNESS));
    let top_rect = Rect::from_min_max(pos2(viewport.min.x + RULER_THICKNESS, viewport.min.y), pos2(viewport.max.x, viewport.min.y + RULER_THICKNESS));
    let left_rect =
        Rect::from_min_max(pos2(viewport.min.x, viewport.min.y + RULER_THICKNESS), pos2(viewport.min.x + RULER_THICKNESS, viewport.max.y));

    // Screen scale: physical screen pixels per PDF page point.
    let screen_scale = (view.zoom * crate::canvas::PT).max(0.001);

    // Compute origin and axis directions using page transformation to support 0, 90, 180, 270 rotations.
    let (origin, dir_x, dir_y) = match view.page_xform(view.current).or_else(|| {
        let pages = view.visible_page_rects();
        pages.first().and_then(|(p, _)| view.page_xform(*p))
    }) {
        Some(xf) => {
            let o = xf.norm_to_screen(0.0, 0.0);
            let u_vec = xf.norm_to_screen(1.0, 0.0) - o;
            let v_vec = xf.norm_to_screen(0.0, 1.0) - o;
            let dx = if u_vec.x.abs() >= v_vec.x.abs() { if u_vec.x >= 0.0 { 1.0 } else { -1.0 } } else { if v_vec.x >= 0.0 { 1.0 } else { -1.0 } };
            let dy = if v_vec.y.abs() >= u_vec.y.abs() { if v_vec.y >= 0.0 { 1.0 } else { -1.0 } } else { if u_vec.y >= 0.0 { 1.0 } else { -1.0 } };
            (o, dx, dy)
        }
        None => (viewport.min, 1.0, 1.0),
    };

    // Draw the corner unit square.
    let next_unit = draw_corner_box(ui, corner_rect, units, t);

    // Draw top horizontal ruler.
    if top_rect.width() > 0.0 {
        draw_top_ruler(ui, top_rect, origin.x, dir_x, screen_scale, units, t);
    }

    // Draw left vertical ruler.
    if left_rect.height() > 0.0 {
        draw_left_ruler(ui, left_rect, origin.y, dir_y, screen_scale, units, t);
    }

    // Draw cursor tracking guides.
    let pointer_pos = ui.input(|i| i.pointer.latest_pos());
    if let Some(pos) = pointer_pos.filter(|p| viewport.contains(*p)) {
        draw_cursor_guides(ui, pos, top_rect, left_rect, viewport, t);
    }

    next_unit
}

/// Draws the corner square where top and left rulers meet, showing the current unit label.
fn draw_corner_box(ui: &mut egui::Ui, rect: Rect, units: crate::units::Unit, t: &Tokens) -> Option<crate::units::Unit> {
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, t.chrome);
    painter.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, t.divider));
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.divider));

    painter.text(rect.center(), Align2::CENTER_CENTER, units.label(), theme::semibold(9.0), t.text_faint);

    let id = ui.id().with("ruler_corner_unit_toggle");
    let resp = ui.interact(rect, id, Sense::click());
    let tip = format!("Unit: {} (click to cycle units)", units.label());
    let resp = resp.on_hover_text(tip);

    if resp.clicked() {
        let next = match units {
            crate::units::Unit::Millimetres => crate::units::Unit::Centimetres,
            crate::units::Unit::Centimetres => crate::units::Unit::Inches,
            crate::units::Unit::Inches => crate::units::Unit::Points,
            crate::units::Unit::Points => crate::units::Unit::Millimetres,
        };
        Some(next)
    } else {
        None
    }
}

/// Draws the horizontal top ruler with ticks and numerical labels anchored to the active page origin.
fn draw_top_ruler(ui: &mut egui::Ui, rect: Rect, origin_x: f32, dir_x: f64, screen_scale: f32, units: crate::units::Unit, t: &Tokens) {
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, t.chrome);
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.divider));

    let per_point = if units.per_point().is_finite() && units.per_point() > 0.0 { units.per_point() } else { 1.0 };
    let (step, subdivisions) = calculate_ruler_step(screen_scale, units);
    if step <= 0.0 || subdivisions == 0 {
        return;
    }

    let (val_at_min_x, val_at_max_x) = if dir_x >= 0.0 {
        (
            (((rect.min.x - origin_x) / screen_scale) as f64 * per_point).clamp(-1_000_000.0, 1_000_000.0),
            (((rect.max.x - origin_x) / screen_scale) as f64 * per_point).clamp(-1_000_000.0, 1_000_000.0),
        )
    } else {
        (
            (((origin_x - rect.max.x) / screen_scale) as f64 * per_point).clamp(-1_000_000.0, 1_000_000.0),
            (((origin_x - rect.min.x) / screen_scale) as f64 * per_point).clamp(-1_000_000.0, 1_000_000.0),
        )
    };
    let min_val = val_at_min_x.min(val_at_max_x);
    let max_val = val_at_min_x.max(val_at_max_x);

    let mut tick = (min_val / step).floor() * step;
    let mut count = 0;
    while tick <= max_val + step && count < 500 {
        count += 1;
        let screen_x =
            if dir_x >= 0.0 { origin_x + ((tick / per_point) as f32 * screen_scale) } else { origin_x - ((tick / per_point) as f32 * screen_scale) };
        if screen_x >= rect.min.x - 1.0 && screen_x <= rect.max.x + 1.0 {
            // Major tick line.
            painter.line_segment([pos2(screen_x, rect.max.y - 7.0), pos2(screen_x, rect.max.y)], Stroke::new(1.0, t.text_faint));
            let label = format_tick_label(tick, step);
            painter.text(pos2(screen_x + 2.0, rect.min.y + 1.5), Align2::LEFT_TOP, label, theme::regular(9.0), t.text_faint);
        }

        // Subdivisions between this tick and the next.
        let sub_step = step / (subdivisions as f64);
        for sub in 1..subdivisions {
            let sub_val = tick + (sub as f64) * sub_step;
            let sub_x = if dir_x >= 0.0 {
                origin_x + ((sub_val / per_point) as f32 * screen_scale)
            } else {
                origin_x - ((sub_val / per_point) as f32 * screen_scale)
            };
            if sub_x >= rect.min.x && sub_x <= rect.max.x {
                let h = if sub * 2 == subdivisions { 4.5 } else { 3.0 };
                painter.line_segment([pos2(sub_x, rect.max.y - h), pos2(sub_x, rect.max.y)], Stroke::new(1.0, t.divider));
            }
        }

        tick += step;
    }
}

/// Draws the vertical left ruler with ticks and rotated numerical labels anchored to the active page origin.
fn draw_left_ruler(ui: &mut egui::Ui, rect: Rect, origin_y: f32, dir_y: f64, screen_scale: f32, units: crate::units::Unit, t: &Tokens) {
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, t.chrome);
    painter.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, t.divider));

    let per_point = if units.per_point().is_finite() && units.per_point() > 0.0 { units.per_point() } else { 1.0 };
    let (step, subdivisions) = calculate_ruler_step(screen_scale, units);
    if step <= 0.0 || subdivisions == 0 {
        return;
    }

    let (val_at_min_y, val_at_max_y) = if dir_y >= 0.0 {
        (
            (((rect.min.y - origin_y) / screen_scale) as f64 * per_point).clamp(-1_000_000.0, 1_000_000.0),
            (((rect.max.y - origin_y) / screen_scale) as f64 * per_point).clamp(-1_000_000.0, 1_000_000.0),
        )
    } else {
        (
            (((origin_y - rect.max.y) / screen_scale) as f64 * per_point).clamp(-1_000_000.0, 1_000_000.0),
            (((origin_y - rect.min.y) / screen_scale) as f64 * per_point).clamp(-1_000_000.0, 1_000_000.0),
        )
    };
    let min_val = val_at_min_y.min(val_at_max_y);
    let max_val = val_at_min_y.max(val_at_max_y);

    let mut tick = (min_val / step).floor() * step;
    let mut count = 0;
    while tick <= max_val + step && count < 500 {
        count += 1;
        let screen_y =
            if dir_y >= 0.0 { origin_y + ((tick / per_point) as f32 * screen_scale) } else { origin_y - ((tick / per_point) as f32 * screen_scale) };
        if screen_y >= rect.min.y - 1.0 && screen_y <= rect.max.y + 1.0 {
            // Major tick line.
            painter.line_segment([pos2(rect.max.x - 7.0, screen_y), pos2(rect.max.x, screen_y)], Stroke::new(1.0, t.text_faint));
            let label = format_tick_label(tick, step);
            let galley = ui.painter().layout_no_wrap(label, theme::regular(9.0), t.text_faint);
            let pos = pos2(rect.min.x + 2.0, screen_y - 2.0);
            painter.add(egui::epaint::TextShape::new(pos, galley, t.text_faint).with_angle(-std::f32::consts::FRAC_PI_2));
        }

        // Subdivisions between this tick and the next.
        let sub_step = step / (subdivisions as f64);
        for sub in 1..subdivisions {
            let sub_val = tick + (sub as f64) * sub_step;
            let sub_y = if dir_y >= 0.0 {
                origin_y + ((sub_val / per_point) as f32 * screen_scale)
            } else {
                origin_y - ((sub_val / per_point) as f32 * screen_scale)
            };
            if sub_y >= rect.min.y && sub_y <= rect.max.y {
                let w = if sub * 2 == subdivisions { 4.5 } else { 3.0 };
                painter.line_segment([pos2(rect.max.x - w, sub_y), pos2(rect.max.x, sub_y)], Stroke::new(1.0, t.divider));
            }
        }

        tick += step;
    }
}

/// Draws real-time cursor indicator lines on the rulers and crosshair alignment guides on the canvas.
fn draw_cursor_guides(ui: &mut egui::Ui, pos: Pos2, top_rect: Rect, left_rect: Rect, viewport: Rect, t: &Tokens) {
    let painter = ui.painter();
    let guide_color = Color32::from_rgba_unmultiplied(26, 115, 232, 60);

    // Indicator on top ruler.
    if pos.x >= top_rect.min.x && pos.x <= top_rect.max.x {
        painter.line_segment([pos2(pos.x, top_rect.min.y), pos2(pos.x, top_rect.max.y)], Stroke::new(1.0, t.accent));
        // Small downward triangle at bottom of ruler.
        let p1 = pos2(pos.x - 3.0, top_rect.max.y - 4.0);
        let p2 = pos2(pos.x + 3.0, top_rect.max.y - 4.0);
        let p3 = pos2(pos.x, top_rect.max.y);
        painter.add(egui::epaint::PathShape::convex_polygon(vec![p1, p2, p3], t.accent, Stroke::NONE));

        // Hairline extending down across the document canvas.
        painter.line_segment([pos2(pos.x, top_rect.max.y), pos2(pos.x, viewport.max.y)], Stroke::new(1.0, guide_color));
    }

    // Indicator on left ruler.
    if pos.y >= left_rect.min.y && pos.y <= left_rect.max.y {
        painter.line_segment([pos2(left_rect.min.x, pos.y), pos2(left_rect.max.x, pos.y)], Stroke::new(1.0, t.accent));
        // Small rightward triangle at right edge of ruler.
        let p1 = pos2(left_rect.max.x - 4.0, pos.y - 3.0);
        let p2 = pos2(left_rect.max.x - 4.0, pos.y + 3.0);
        let p3 = pos2(left_rect.max.x, pos.y);
        painter.add(egui::epaint::PathShape::convex_polygon(vec![p1, p2, p3], t.accent, Stroke::NONE));

        // Hairline extending right across the document canvas.
        painter.line_segment([pos2(left_rect.max.x, pos.y), pos2(viewport.max.x, pos.y)], Stroke::new(1.0, guide_color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::PT;
    use crate::units::Unit;

    #[test]
    fn ruler_step_calculation_and_tick_formatting() {
        // Points at 100% zoom (1.0 * PT = 96/72 px/pt) -> target ~50 / (96/72) = 37.5 pt -> nice step 50.0 pt with 5 subdivisions
        let (step_pt, sub_pt) = calculate_ruler_step(1.0 * PT, Unit::Points);
        assert!((step_pt - 50.0).abs() < 1e-4, "expected 50 pt step, got {step_pt}");
        assert_eq!(sub_pt, 5);

        // Inches at 100% zoom (72 pt/in * 96/72 = 96 px/in) -> target ~50/96 = 0.52 in -> nice step 1.0 in with 8 subdivisions
        let (step_in, sub_in) = calculate_ruler_step(1.0 * PT, Unit::Inches);
        assert!((step_in - 1.0).abs() < 1e-4, "expected 1.0 in step, got {step_in}");
        assert_eq!(sub_in, 8);

        // Millimetres at 100% zoom (2.8346 pt/mm * 96/72 = 3.7795 px/mm) -> target ~50 / 3.7795 = 13.2 mm -> nice step 20.0 mm with 4 subdivisions
        let (step_mm, sub_mm) = calculate_ruler_step(1.0 * PT, Unit::Millimetres);
        assert!((step_mm - 20.0).abs() < 1e-4, "expected 20.0 mm step, got {step_mm}");
        assert_eq!(sub_mm, 4);

        // Degenerate inputs must not panic.
        let (step_zero, _) = calculate_ruler_step(0.0, Unit::Points);
        assert!(step_zero > 0.0);
        let (step_nan, _) = calculate_ruler_step(f32::NAN, Unit::Points);
        assert!(step_nan > 0.0);

        // Tick formatting
        assert_eq!(format_tick_label(0.0, 10.0), "0");
        assert_eq!(format_tick_label(100.0, 50.0), "100");
        assert_eq!(format_tick_label(-50.0, 50.0), "-50");
        assert_eq!(format_tick_label(0.5, 0.5), "0.5");
        assert_eq!(format_tick_label(1.25, 0.25), "1.25");
    }
}
