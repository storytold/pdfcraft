//! Smart alignment guides and snapping for blocks and editable objects,
//! matching the behaviour in Word, PowerPoint, and Figma.
//!
//! When dragging blocks, edges and centres snap to other elements within a
//! threshold, and faint dashed guide lines in brand red `#b70228` appear.

use egui::{Color32, Painter, Rect, Shape, Stroke, Vec2, pos2, vec2};

/// Faint brand red requested: #b70228
pub const SNAP_COLOR: Color32 = Color32::from_rgb(0xb7, 0x02, 0x28);

/// Default snapping distance threshold in screen pixels.
pub const SNAP_THRESHOLD: f32 = 6.0;

/// Guideline indicating an alignment match.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GuideLine {
    /// A vertical guideline at x coordinate spanning from y_min to y_max.
    Vertical { x: f32, y_min: f32, y_max: f32 },
    /// A horizontal guideline at y coordinate spanning from x_min to x_max.
    Horizontal { y: f32, x_min: f32, x_max: f32 },
}

/// Result of snapping a rectangle against target rectangles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapResult {
    /// Displacement vector to apply to the moving rectangle to snap it.
    pub offset: Vec2,
    /// Alignment guide lines to render.
    pub guides: Vec<GuideLine>,
}

/// Calculate snapping offset and alignment guidelines for `moving` against `targets`.
pub fn snap_rect(moving: Rect, targets: impl IntoIterator<Item = Rect>, threshold: f32) -> SnapResult {
    let target_list: Vec<Rect> = targets.into_iter().filter(|r| r.is_positive()).collect();
    if target_list.is_empty() || !moving.is_positive() {
        return SnapResult::default();
    }

    // 1. Horizontal snap (adjusts X)
    let moving_xs = [moving.min.x, moving.center().x, moving.max.x];
    let mut best_dx: Option<f32> = None;
    let mut min_abs_dx = threshold + 1.0;

    for target in &target_list {
        let target_xs = [target.min.x, target.center().x, target.max.x];
        for &mx in &moving_xs {
            for &tx in &target_xs {
                let dx = tx - mx;
                let abs_dx = dx.abs();
                if abs_dx <= threshold && abs_dx < min_abs_dx {
                    min_abs_dx = abs_dx;
                    best_dx = Some(dx);
                }
            }
        }
    }

    // 2. Vertical snap (adjusts Y)
    let moving_ys = [moving.min.y, moving.center().y, moving.max.y];
    let mut best_dy: Option<f32> = None;
    let mut min_abs_dy = threshold + 1.0;

    for target in &target_list {
        let target_ys = [target.min.y, target.center().y, target.max.y];
        for &my in &moving_ys {
            for &ty in &target_ys {
                let dy = ty - my;
                let abs_dy = dy.abs();
                if abs_dy <= threshold && abs_dy < min_abs_dy {
                    min_abs_dy = abs_dy;
                    best_dy = Some(dy);
                }
            }
        }
    }

    let snap_dx = best_dx.unwrap_or(0.0);
    let snap_dy = best_dy.unwrap_or(0.0);
    let offset = vec2(snap_dx, snap_dy);
    let snapped = moving.translate(offset);

    let mut guides = Vec::new();

    // Construct vertical guidelines if snapped horizontally
    if best_dx.is_some() {
        let snapped_xs = [snapped.min.x, snapped.center().x, snapped.max.x];
        for &sx in &snapped_xs {
            let mut matched_targets = Vec::new();
            for target in &target_list {
                let target_xs = [target.min.x, target.center().x, target.max.x];
                if target_xs.iter().any(|&tx| (tx - sx).abs() < 0.5) {
                    matched_targets.push(*target);
                }
            }
            if !matched_targets.is_empty() {
                let mut y_min = snapped.min.y;
                let mut y_max = snapped.max.y;
                for t in matched_targets {
                    y_min = y_min.min(t.min.y);
                    y_max = y_max.max(t.max.y);
                }
                guides.push(GuideLine::Vertical { x: sx, y_min: y_min - 16.0, y_max: y_max + 16.0 });
            }
        }
    }

    // Construct horizontal guidelines if snapped vertically
    if best_dy.is_some() {
        let snapped_ys = [snapped.min.y, snapped.center().y, snapped.max.y];
        for &sy in &snapped_ys {
            let mut matched_targets = Vec::new();
            for target in &target_list {
                let target_ys = [target.min.y, target.center().y, target.max.y];
                if target_ys.iter().any(|&ty| (ty - sy).abs() < 0.5) {
                    matched_targets.push(*target);
                }
            }
            if !matched_targets.is_empty() {
                let mut x_min = snapped.min.x;
                let mut x_max = snapped.max.x;
                for t in matched_targets {
                    x_min = x_min.min(t.min.x);
                    x_max = x_max.max(t.max.x);
                }
                guides.push(GuideLine::Horizontal { y: sy, x_min: x_min - 16.0, x_max: x_max + 16.0 });
            }
        }
    }

    SnapResult { offset, guides }
}

/// Paint alignment guidelines with faint dashed lines in red `#b70228`.
pub fn paint_guides(painter: &Painter, guides: &[GuideLine], clip: Option<Rect>) {
    if guides.is_empty() {
        return;
    }
    // Faint dashed line in brand red #b70228
    let stroke = Stroke::new(1.0, SNAP_COLOR.gamma_multiply(0.65));
    for guide in guides {
        match *guide {
            GuideLine::Vertical { x, mut y_min, mut y_max } => {
                if let Some(c) = clip {
                    y_min = y_min.clamp(c.min.y, c.max.y);
                    y_max = y_max.clamp(c.min.y, c.max.y);
                }
                if y_max > y_min + 1.0 {
                    let p1 = pos2(x, y_min);
                    let p2 = pos2(x, y_max);
                    painter.extend(Shape::dashed_line(&[p1, p2], stroke, 5.0, 4.0));
                }
            }
            GuideLine::Horizontal { y, mut x_min, mut x_max } => {
                if let Some(c) = clip {
                    x_min = x_min.clamp(c.min.x, c.max.x);
                    x_max = x_max.clamp(c.min.x, c.max.x);
                }
                if x_max > x_min + 1.0 {
                    let p1 = pos2(x_min, y);
                    let p2 = pos2(x_max, y);
                    painter.extend(Shape::dashed_line(&[p1, p2], stroke, 5.0, 4.0));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_left_to_left_edge() {
        let moving = Rect::from_min_size(pos2(102.0, 50.0), vec2(50.0, 30.0));
        let target = Rect::from_min_size(pos2(100.0, 200.0), vec2(60.0, 40.0));
        let res = snap_rect(moving, vec![target], 5.0);
        assert_eq!(res.offset.x, -2.0);
        assert_eq!(res.offset.y, 0.0);
        assert!(!res.guides.is_empty());
        assert!(matches!(res.guides[0], GuideLine::Vertical { x, .. } if (x - 100.0).abs() < 0.1));
    }

    #[test]
    fn snaps_center_to_center() {
        // moving center x = 152.0 (127.0 + 50/2), target center x = 150.0 (120.0 + 60/2)
        let moving = Rect::from_min_size(pos2(127.0, 50.0), vec2(50.0, 30.0));
        let target = Rect::from_min_size(pos2(120.0, 200.0), vec2(60.0, 40.0));
        let res = snap_rect(moving, vec![target], 5.0);
        assert_eq!(res.offset.x, -2.0);
        assert_eq!(res.offset.y, 0.0);
        assert!(matches!(res.guides[0], GuideLine::Vertical { x, .. } if (x - 150.0).abs() < 0.1));
    }

    #[test]
    fn snaps_top_to_top_edge() {
        let moving = Rect::from_min_size(pos2(50.0, 103.0), vec2(50.0, 30.0));
        let target = Rect::from_min_size(pos2(200.0, 100.0), vec2(60.0, 100.0));
        let res = snap_rect(moving, vec![target], 5.0);
        assert_eq!(res.offset.x, 0.0);
        assert_eq!(res.offset.y, -3.0);
        assert!(matches!(res.guides[0], GuideLine::Horizontal { y, .. } if (y - 100.0).abs() < 0.1));
    }

    #[test]
    fn ignores_beyond_threshold() {
        let moving = Rect::from_min_size(pos2(120.0, 50.0), vec2(50.0, 30.0));
        let target = Rect::from_min_size(pos2(100.0, 200.0), vec2(60.0, 40.0));
        let res = snap_rect(moving, vec![target], 5.0);
        assert_eq!(res.offset, Vec2::ZERO);
        assert!(res.guides.is_empty());
    }
}
