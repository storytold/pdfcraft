//! Horizontal and vertical rulers along the top and left edges of the document area (`view.rulers`).
//!
//! The rulers reserve a thin strip along the top and left of the page area so pages are never covered.
//! Origin `(0, 0)` is the top-left corner of the page under the pointer (or the current page when the
//! pointer is elsewhere), in the page's displayed orientation. Units follow the app's measurement-units
//! preference (`crate::units::Unit`). Ticks use nice 1 / 2 / 5 × 10^n intervals chosen from the active
//! zoom so labels never overlap, and are hard-capped per frame so degenerate inputs cannot stall rendering.

use craft::pages::ui::PlacedPage;
use craft::ui::runtime::env::Env;
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Stroke, pos2};

use crate::units::Unit;

/// Thickness in logical points reserved for each ruler strip (and the corner unit square).
pub const RULER_THICKNESS: f32 = 20.0;

/// Minimum screen spacing in logical points between consecutive major tick labels.
pub const MIN_MAJOR_SPACING_PX: f64 = 56.0;

/// Maximum number of ticks (major + minor) emitted per ruler per frame.
pub const MAX_TICKS: usize = 512;

/// Minimum screen spacing in logical points between minor ticks before minor ticks are omitted.
const MIN_MINOR_SPACING_PX: f64 = 3.0;

/// Whether a tick is a major labelled tick, a half-way subdivision, or a regular minor tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickKind {
    Major,
    Half,
    Minor,
}

/// One positioned tick along a ruler axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tick {
    /// Coordinate in logical screen points along the ruler's axis.
    pub screen: f32,
    /// Coordinate in the active measurement unit relative to the page origin.
    pub value: f64,
    pub kind: TickKind,
}

/// Reserves space for the top and left ruler strips inside `area`.
///
/// Returns `(Some(area), page_rect)` when `area` is finite and larger than [`RULER_THICKNESS`] on
/// both axes; otherwise returns `(None, area)` without producing an inverted rectangle.
pub fn reserve(area: Rect) -> (Option<Rect>, Rect) {
    if !area.is_finite() || area.width() <= RULER_THICKNESS || area.height() <= RULER_THICKNESS {
        return (None, area);
    }
    let page_rect = Rect::from_min_max(pos2(area.min.x + RULER_THICKNESS, area.min.y + RULER_THICKNESS), area.max);
    (Some(area), page_rect)
}

/// Chooses a "nice" major interval (`1`, `2`, or `5` × `10^n`) and minor subdivision count
/// given `px_per_unit` (logical screen pixels per 1 unit) and `min_major_px` (minimum screen
/// pixels between adjacent major labels).
///
/// Returns `None` for non-finite or non-positive inputs, or if the resulting step underflows/overflows.
pub fn nice_interval(px_per_unit: f64, min_major_px: f64) -> Option<(f64, usize)> {
    if !px_per_unit.is_finite() || px_per_unit <= 0.0 || !min_major_px.is_finite() || min_major_px <= 0.0 {
        return None;
    }
    let target = min_major_px / px_per_unit;
    if !target.is_finite() || target <= 0.0 {
        return None;
    }
    let exp = target.log10().floor();
    if !exp.is_finite() {
        return None;
    }
    let base = 10.0_f64.powf(exp);
    if !base.is_finite() || base <= 0.0 {
        return None;
    }
    let frac = target / base;
    let (step, subdivisions) = if frac <= 1.0 {
        (1.0 * base, 10)
    } else if frac <= 2.0 {
        (2.0 * base, 4)
    } else if frac <= 5.0 {
        (5.0 * base, 5)
    } else {
        (10.0 * base, 10)
    };
    if !step.is_finite() || step <= 0.0 {
        return None;
    }
    Some((step, subdivisions))
}

/// Chooses `(major_step, minor_subdivisions)` in `unit` for a scale of `px_per_pt` logical screen
/// pixels per PDF point.
pub fn ruler_step(px_per_pt: f64, unit: Unit) -> Option<(f64, usize)> {
    if !px_per_pt.is_finite() || px_per_pt <= 0.0 {
        return None;
    }
    let per_point = unit.per_point();
    if !per_point.is_finite() || per_point <= 0.0 {
        return None;
    }
    let px_per_unit = px_per_pt / per_point;
    let (step, subdivisions) = nice_interval(px_per_unit, MIN_MAJOR_SPACING_PX)?;
    // Eighths of an inch at 1 in major intervals match standard inch rulers.
    let subdivisions = if unit == Unit::Inches && (step - 1.0).abs() < 1e-6 { 8 } else { subdivisions };
    Some((step, subdivisions))
}

/// Computes major and minor ticks along one ruler axis over `[range_min, range_max]`.
///
/// `origin` is the screen coordinate of unit `0`, `px_per_unit` is logical screen pixels per unit,
/// `major_step` is the distance between major ticks in units, and `subdivisions` is the number of
/// equal segments between major ticks. At most `max_ticks` ticks are returned; degenerate or
/// non-finite inputs yield an empty list without panicking or looping.
pub fn compute_ticks(
    range_min: f32,
    range_max: f32,
    origin: f32,
    px_per_unit: f64,
    major_step: f64,
    subdivisions: usize,
    max_ticks: usize,
) -> Vec<Tick> {
    if !range_min.is_finite()
        || !range_max.is_finite()
        || range_max <= range_min
        || !origin.is_finite()
        || !px_per_unit.is_finite()
        || px_per_unit <= 0.0
        || !major_step.is_finite()
        || major_step <= 0.0
        || subdivisions == 0
        || max_ticks == 0
    {
        return Vec::new();
    }

    let r_min = f64::from(range_min);
    let r_max = f64::from(range_max);
    let orig = f64::from(origin);

    let u_min = (r_min - orig) / px_per_unit;
    let u_max = (r_max - orig) / px_per_unit;
    if !u_min.is_finite() || !u_max.is_finite() {
        return Vec::new();
    }

    let first_k = (u_min / major_step).floor();
    let last_k = (u_max / major_step).ceil();
    if !first_k.is_finite() || !last_k.is_finite() || first_k < -1e12 || last_k > 1e12 {
        return Vec::new();
    }

    let k_start = first_k as i64;
    let k_end = last_k as i64;
    if k_end < k_start {
        return Vec::new();
    }

    let major_px = major_step * px_per_unit;
    let draw_minor = subdivisions > 1 && (major_px / (subdivisions as f64)) >= MIN_MINOR_SPACING_PX;
    let sub_step = major_step / (subdivisions as f64);

    let mut out = Vec::new();
    // Bound major-loop iterations independently so pathological inputs can never spin.
    let max_major_iters = max_ticks.saturating_add(2);
    for (iters, k) in (k_start..=k_end).enumerate() {
        if iters >= max_major_iters || out.len() >= max_ticks {
            break;
        }
        let val = (k as f64) * major_step;
        let screen = orig + val * px_per_unit;
        if screen.is_finite() && screen >= r_min - 0.5 && screen <= r_max + 0.5 {
            out.push(Tick { screen: screen as f32, value: val, kind: TickKind::Major });
        }
        if draw_minor {
            for sub in 1..subdivisions {
                if out.len() >= max_ticks {
                    break;
                }
                let sub_val = val + (sub as f64) * sub_step;
                let sub_screen = orig + sub_val * px_per_unit;
                if sub_screen.is_finite() && sub_screen >= r_min && sub_screen <= r_max {
                    let kind = if subdivisions.is_multiple_of(2) && sub * 2 == subdivisions { TickKind::Half } else { TickKind::Minor };
                    out.push(Tick { screen: sub_screen as f32, value: sub_val, kind });
                }
            }
        }
    }
    out
}

/// Formats a major tick value concisely without trailing zeroes or `-0`.
pub fn format_label(value: f64, step: f64) -> String {
    if !value.is_finite() {
        return String::new();
    }
    let eps = if step.is_finite() && step > 0.0 { (step * 1e-6).max(1e-9) } else { 1e-9 };
    if value.abs() < eps {
        return "0".to_string();
    }
    let decimals = if step.is_finite() && step > 0.0 && step < 1.0 {
        let d = (-step.log10().floor()).clamp(0.0, 4.0) as usize;
        // e.g. step = 0.25 needs 2 decimals even though floor(log10(0.25)) == -1.
        if ((step * 10.0_f64.powi(d as i32)).fract()).abs() > 1e-4 && d < 4 { d + 1 } else { d }
    } else if (value - value.round()).abs() < 1e-4 {
        0
    } else {
        2
    };
    let mut s = format!("{value:.decimals$}");
    if decimals > 0 && s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" { "0".to_string() } else { s }
}

/// Computes the displayed top-left origin and `(px_per_pt_x, px_per_pt_y)` for a placed page.
///
/// The page is measured in its displayed orientation (swapping width and height under 90°/270°
/// view rotation). Returns `None` if `page_rect` is zero-sized or non-finite, or if no positive
/// finite scale can be determined.
pub fn page_origin_and_scale(page_rect: Rect, page_size_pt: (f32, f32), view_rotation: u16, fallback_zoom: f64) -> Option<(Pos2, f64, f64)> {
    if !page_rect.is_finite() || page_rect.width() <= 0.0 || page_rect.height() <= 0.0 {
        return None;
    }
    let (disp_w, disp_h) = if view_rotation % 180 == 90 { (page_size_pt.1, page_size_pt.0) } else { page_size_pt };
    if disp_w.is_finite() && disp_w > 0.0 && disp_h.is_finite() && disp_h > 0.0 {
        let sx = f64::from(page_rect.width()) / f64::from(disp_w);
        let sy = f64::from(page_rect.height()) / f64::from(disp_h);
        if sx.is_finite() && sx > 0.0 && sy.is_finite() && sy > 0.0 {
            return Some((page_rect.left_top(), sx, sy));
        }
    }
    if fallback_zoom.is_finite() && fallback_zoom > 0.0 {
        let s = fallback_zoom * f64::from(super::PT);
        if s.is_finite() && s > 0.0 {
            return Some((page_rect.left_top(), s, s));
        }
    }
    None
}

/// Converts an RGBA theme token value (`craft::theme::Rgba.0`) into an `egui::Color32`.
fn token_color([r, g, b, a]: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Draws the top and left rulers and corner unit indicator inside the reserved strips of `outer`
/// outside `page_rect`.
///
/// Never claims the cursor or takes presses. Draws short cursor marks on both rulers only while
/// `pointer` is over a placed page.
#[allow(clippy::too_many_arguments)]
pub(super) fn show(
    ui: &egui::Ui,
    outer: Rect,
    page_rect: Rect,
    placed: &[PlacedPage],
    page_sizes: &[(f32, f32)],
    current_page: usize,
    view_rotation: u16,
    view_zoom: f64,
    units: Unit,
    pointer: Option<Pos2>,
) {
    if !outer.is_finite() || !page_rect.is_finite() || !outer.is_positive() {
        return;
    }

    let env = Env::of(ui);
    let colors = &env.tokens.color;
    let bg = token_color(colors.surface.raised.0);
    let border = token_color(colors.line.control_edge.0);
    let tick_major = token_color(colors.text.muted.0);
    let tick_minor = token_color(colors.line.control_edge.0);
    let label_ink = token_color(colors.text.normal.0);
    let accent = token_color(colors.accent.solid.0);
    let thin = env.tokens.stroke.thin.max(1.0);

    let mut font: FontId = egui::TextStyle::Small.resolve(ui.style());
    font.size = font.size.min(9.5);

    let corner_rect = Rect::from_min_max(outer.min, page_rect.min);
    let top_rect = Rect::from_min_max(pos2(page_rect.min.x, outer.min.y), pos2(outer.max.x, page_rect.min.y));
    let left_rect = Rect::from_min_max(pos2(outer.min.x, page_rect.min.y), pos2(page_rect.min.x, outer.max.y));

    // Anchor page: page under the pointer if any, otherwise the current page, otherwise the first placed page.
    let pointer_page = pointer.and_then(|pos| placed.iter().find(|p| p.rect.is_finite() && p.rect.contains(pos)));
    let cursor_on_page = pointer.filter(|_| pointer_page.is_some());
    let anchor = pointer_page.or_else(|| placed.iter().find(|p| p.page == current_page)).or_else(|| placed.first());

    let origin_scale = anchor
        .and_then(|p| {
            let size = page_sizes.get(p.page).copied().unwrap_or((0.0, 0.0));
            page_origin_and_scale(p.rect, size, view_rotation, view_zoom)
        })
        .or_else(|| {
            if view_zoom.is_finite() && view_zoom > 0.0 {
                let s = view_zoom * f64::from(super::PT);
                (s.is_finite() && s > 0.0).then_some((page_rect.min, s, s))
            } else {
                None
            }
        });

    let per_point = units.per_point();
    let per_point_ok = per_point.is_finite() && per_point > 0.0;

    // Corner unit badge (display only; does not take clicks or claim the cursor).
    {
        let painter = ui.painter().with_clip_rect(corner_rect);
        painter.rect_filled(corner_rect, CornerRadius::ZERO, bg);
        painter.line_segment([corner_rect.right_top(), corner_rect.right_bottom()], Stroke::new(thin, border));
        painter.line_segment([corner_rect.left_bottom(), corner_rect.right_bottom()], Stroke::new(thin, border));
        painter.text(corner_rect.center(), Align2::CENTER_CENTER, units.label(), font.clone(), label_ink);
    }

    // Top horizontal ruler.
    if top_rect.width() > 0.0 && top_rect.height() > 0.0 {
        let painter = ui.painter().with_clip_rect(top_rect);
        painter.rect_filled(top_rect, CornerRadius::ZERO, bg);
        painter.line_segment([top_rect.left_bottom(), top_rect.right_bottom()], Stroke::new(thin, border));

        if per_point_ok
            && let Some((origin, sx, _)) = origin_scale
            && let Some((step, subs)) = ruler_step(sx, units)
        {
            let px_per_unit = sx / per_point;
            let ticks = compute_ticks(top_rect.min.x, top_rect.max.x, origin.x, px_per_unit, step, subs, MAX_TICKS);
            for tick in ticks {
                match tick.kind {
                    TickKind::Major => {
                        painter.line_segment(
                            [pos2(tick.screen, top_rect.max.y - 7.0), pos2(tick.screen, top_rect.max.y)],
                            Stroke::new(thin, tick_major),
                        );
                        let label = format_label(tick.value, step);
                        painter.text(pos2(tick.screen + 2.0, top_rect.min.y + 1.5), Align2::LEFT_TOP, label, font.clone(), label_ink);
                    }
                    TickKind::Half => {
                        painter.line_segment(
                            [pos2(tick.screen, top_rect.max.y - 5.0), pos2(tick.screen, top_rect.max.y)],
                            Stroke::new(thin, tick_minor),
                        );
                    }
                    TickKind::Minor => {
                        painter.line_segment(
                            [pos2(tick.screen, top_rect.max.y - 3.5), pos2(tick.screen, top_rect.max.y)],
                            Stroke::new(thin, tick_minor),
                        );
                    }
                }
            }
        }

        if let Some(pos) = cursor_on_page
            && pos.x >= top_rect.min.x
            && pos.x <= top_rect.max.x
        {
            painter.line_segment([pos2(pos.x, top_rect.min.y), pos2(pos.x, top_rect.max.y)], Stroke::new(1.5, accent));
            let tip = vec![pos2(pos.x - 3.0, top_rect.max.y - 4.0), pos2(pos.x + 3.0, top_rect.max.y - 4.0), pos2(pos.x, top_rect.max.y)];
            painter.add(egui::epaint::PathShape::convex_polygon(tip, accent, Stroke::NONE));
        }
    }

    // Left vertical ruler.
    if left_rect.width() > 0.0 && left_rect.height() > 0.0 {
        let painter = ui.painter().with_clip_rect(left_rect);
        painter.rect_filled(left_rect, CornerRadius::ZERO, bg);
        painter.line_segment([left_rect.right_top(), left_rect.right_bottom()], Stroke::new(thin, border));

        if per_point_ok
            && let Some((origin, _, sy)) = origin_scale
            && let Some((step, subs)) = ruler_step(sy, units)
        {
            let px_per_unit = sy / per_point;
            let ticks = compute_ticks(left_rect.min.y, left_rect.max.y, origin.y, px_per_unit, step, subs, MAX_TICKS);
            for tick in ticks {
                match tick.kind {
                    TickKind::Major => {
                        painter.line_segment(
                            [pos2(left_rect.max.x - 7.0, tick.screen), pos2(left_rect.max.x, tick.screen)],
                            Stroke::new(thin, tick_major),
                        );
                        let label = format_label(tick.value, step);
                        let galley = painter.layout_no_wrap(label, font.clone(), label_ink);
                        let pos = pos2(left_rect.min.x + 2.0, tick.screen + 2.0 + galley.size().x);
                        painter.add(egui::epaint::TextShape::new(pos, galley, label_ink).with_angle(-std::f32::consts::FRAC_PI_2));
                    }
                    TickKind::Half => {
                        painter.line_segment(
                            [pos2(left_rect.max.x - 5.0, tick.screen), pos2(left_rect.max.x, tick.screen)],
                            Stroke::new(thin, tick_minor),
                        );
                    }
                    TickKind::Minor => {
                        painter.line_segment(
                            [pos2(left_rect.max.x - 3.5, tick.screen), pos2(left_rect.max.x, tick.screen)],
                            Stroke::new(thin, tick_minor),
                        );
                    }
                }
            }
        }

        if let Some(pos) = cursor_on_page
            && pos.y >= left_rect.min.y
            && pos.y <= left_rect.max.y
        {
            painter.line_segment([pos2(left_rect.min.x, pos.y), pos2(left_rect.max.x, pos.y)], Stroke::new(1.5, accent));
            let tip = vec![pos2(left_rect.max.x - 4.0, pos.y - 3.0), pos2(left_rect.max.x - 4.0, pos.y + 3.0), pos2(left_rect.max.x, pos.y)];
            painter.add(egui::epaint::PathShape::convex_polygon(tip, accent, Stroke::NONE));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::PT;
    use egui::vec2;

    #[test]
    fn nice_interval_picks_1_2_5_multiples_across_zooms_and_units() {
        for unit in Unit::ALL {
            for zoom in [0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 16.0, 64.0] {
                let px_per_pt = zoom * f64::from(PT);
                let (step, subs) = ruler_step(px_per_pt, unit).expect("finite positive zoom yields a step");
                assert!(step > 0.0 && step.is_finite());
                assert!(subs >= 4);
                let px_per_unit = px_per_pt / unit.per_point();
                let major_px = step * px_per_unit;
                // Labels never overlap (at least MIN_MAJOR_SPACING_PX apart) and never get absurdly sparse.
                assert!(major_px >= MIN_MAJOR_SPACING_PX - 1e-6, "unit={unit:?} zoom={zoom} major_px={major_px}");
                assert!(major_px <= MIN_MAJOR_SPACING_PX * 2.51, "unit={unit:?} zoom={zoom} major_px={major_px}");
                // Leading digit of step is 1, 2, or 5.
                let mantissa = step / 10.0_f64.powf(step.log10().floor());
                assert!(
                    (mantissa - 1.0).abs() < 1e-6 || (mantissa - 2.0).abs() < 1e-6 || (mantissa - 5.0).abs() < 1e-6,
                    "step {step} mantissa {mantissa} is not 1/2/5"
                );
            }
        }
        // Higher zoom yields smaller or equal major interval.
        for unit in Unit::ALL {
            let (far, _) = ruler_step(0.25 * f64::from(PT), unit).unwrap();
            let (near, _) = ruler_step(4.0 * f64::from(PT), unit).unwrap();
            assert!(near < far, "{unit:?}: zoomed in ({near}) should have finer step than zoomed out ({far})");
        }
    }

    #[test]
    fn tick_positions_anchor_to_page_offset() {
        let origin = 120.0_f32;
        let px_per_unit = 10.0_f64;
        let ticks = compute_ticks(20.0, 320.0, origin, px_per_unit, 10.0, 5, MAX_TICKS);
        let majors: Vec<Tick> = ticks.iter().copied().filter(|t| t.kind == TickKind::Major).collect();
        // Origin 0 lands exactly at screen coordinate 120.0.
        let zero = majors.iter().find(|t| t.value.abs() < 1e-9).expect("zero tick present");
        assert!((zero.screen - origin).abs() < 1e-4);
        // Every major tick satisfies screen == origin + value * px_per_unit and stays inside the ruler strip.
        assert!(majors.len() >= 3);
        for t in &majors {
            let expected = origin + (t.value * px_per_unit) as f32;
            assert!((t.screen - expected).abs() < 1e-3, "tick {t:?} expected {expected}");
            assert!(t.screen >= 19.5 && t.screen <= 320.5);
        }
        // Shifting origin right by 35 px shifts every tick's screen position by +35 px for the same value.
        let shifted = compute_ticks(20.0, 320.0, origin + 35.0, px_per_unit, 10.0, 5, MAX_TICKS);
        let shifted_zero = shifted.iter().find(|t| t.kind == TickKind::Major && t.value.abs() < 1e-9).unwrap();
        assert!((shifted_zero.screen - (origin + 35.0)).abs() < 1e-4);
    }

    #[test]
    fn label_formatting_is_clean_and_has_no_negative_zero() {
        assert_eq!(format_label(0.0, 10.0), "0");
        assert_eq!(format_label(-0.0, 10.0), "0");
        assert_eq!(format_label(-1e-12, 1.0), "0");
        assert_eq!(format_label(100.0, 50.0), "100");
        assert_eq!(format_label(-50.0, 50.0), "-50");
        assert_eq!(format_label(0.5, 0.5), "0.5");
        assert_eq!(format_label(1.0, 0.5), "1");
        assert_eq!(format_label(1.25, 0.25), "1.25");
        assert_eq!(format_label(0.05, 0.05), "0.05");
        assert_eq!(format_label(f64::NAN, 1.0), "");
        assert_eq!(format_label(f64::INFINITY, 1.0), "");
    }

    #[test]
    fn tick_cap_bounds_output_on_dense_ranges() {
        let cap = 32;
        let ticks = compute_ticks(0.0, 100_000.0, 0.0, 10.0, 1.0, 10, cap);
        assert_eq!(ticks.len(), cap);
        let ticks_default = compute_ticks(0.0, 1_000_000.0, 0.0, 10.0, 1.0, 10, MAX_TICKS);
        assert!(ticks_default.len() <= MAX_TICKS);
    }

    #[test]
    fn degenerate_inputs_do_not_panic_or_loop() {
        // Bad zooms: 0, negative, NaN, infinity, huge.
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e308, 1e-308] {
            let _ = nice_interval(bad, MIN_MAJOR_SPACING_PX);
            let _ = ruler_step(bad, Unit::Points);
            let ticks = compute_ticks(0.0, 500.0, 10.0, bad, 10.0, 5, MAX_TICKS);
            assert!(ticks.len() <= MAX_TICKS);
        }
        // Bad ranges and steps.
        assert!(compute_ticks(100.0, 100.0, 0.0, 1.0, 10.0, 5, MAX_TICKS).is_empty());
        assert!(compute_ticks(200.0, 100.0, 0.0, 1.0, 10.0, 5, MAX_TICKS).is_empty());
        assert!(compute_ticks(f32::NAN, 100.0, 0.0, 1.0, 10.0, 5, MAX_TICKS).is_empty());
        assert!(compute_ticks(0.0, 100.0, f32::NAN, 1.0, 10.0, 5, MAX_TICKS).is_empty());
        assert!(compute_ticks(0.0, 100.0, 0.0, 1.0, 0.0, 5, MAX_TICKS).is_empty());
        assert!(compute_ticks(0.0, 100.0, 0.0, 1.0, f64::NAN, 5, MAX_TICKS).is_empty());
        assert!(compute_ticks(0.0, 100.0, 0.0, 1.0, 10.0, 0, MAX_TICKS).is_empty());

        // Zero-size and tiny areas for reserve().
        let zero = Rect::from_min_size(pos2(10.0, 20.0), vec2(0.0, 0.0));
        assert_eq!(reserve(zero), (None, zero));
        let tiny = Rect::from_min_size(pos2(0.0, 0.0), vec2(RULER_THICKNESS, 200.0));
        assert_eq!(reserve(tiny), (None, tiny));

        // Zero-size page and rotated pages for page_origin_and_scale().
        assert!(page_origin_and_scale(zero, (0.0, 0.0), 0, 1.0).is_none());
        let page = Rect::from_min_size(pos2(40.0, 50.0), vec2(400.0, 200.0));
        assert!(page_origin_and_scale(page, (0.0, 0.0), 0, 0.0).is_none());
        assert!(page_origin_and_scale(page, (f32::NAN, 100.0), 0, f64::NAN).is_none());
        // Rotated 90°: displayed width is ph (800), displayed height is pw (400).
        let (origin, sx, sy) = page_origin_and_scale(page, (400.0, 800.0), 90, 1.0).unwrap();
        assert_eq!(origin, page.left_top());
        assert!((sx - 0.5).abs() < 1e-6);
        assert!((sy - 0.5).abs() < 1e-6);
    }
}
