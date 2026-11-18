//! Image manipulation engine for non-destructive crop, anchor-based resize, and movement.
//!
//! Contributor-original clean-room design with zero-panic safety and lockstep pixel-scale invariance.

use egui::{Color32, Pos2, Rect, Stroke, Vec2, pos2};

use crate::edit_text_ui::ImageHandle;

// ============================================================================
// 1. MATHEMATICAL CORE: ANCHOR-BASED RESIZE & CROP
// ============================================================================

/// Minimum allowed dimensions in pixels to prevent panics, zero divisions, or inverted rects.
pub const MIN_RECT_SIZE: f32 = 10.0;

/// **Anchor-Based Resize System**
///
/// Recalculates the screen bounding box by locking the diagonally opposite point/edge as
/// a fixed anchor.
///
/// Math breakdown:
/// - Corner handles: The diagonally opposite corner is fixed in both X and Y.
///   Width and height are calculated relative to that anchor, clamped to >= `MIN_RECT_SIZE`,
///   and the new bounding box is constructed from `anchor` and the clamped dragged coordinate.
/// - Mid-edge handles: The opposite edge coordinate is fixed in 1D. The perpendicular axis is
///   completely unaffected.
pub fn apply_resize(image_rect: Rect, handle: ImageHandle, delta: Vec2) -> Rect {
    if !image_rect.is_finite() || !delta.x.is_finite() || !delta.y.is_finite() {
        return image_rect;
    }
    let mut min = image_rect.min;
    let mut max = image_rect.max;

    match handle {
        // Dragging TopLeft: Fixed Anchor = BottomRight (max.x, max.y)
        ImageHandle::TopLeft => {
            let anchor = max;
            let target = min + delta;
            let new_w = (anchor.x - target.x).max(MIN_RECT_SIZE);
            let new_h = (anchor.y - target.y).max(MIN_RECT_SIZE);
            min.x = anchor.x - new_w;
            min.y = anchor.y - new_h;
        }

        // Dragging TopCenter: Fixed Anchor = Bottom edge (max.y)
        ImageHandle::TopCenter => {
            let anchor_y = max.y;
            let target_y = min.y + delta.y;
            let new_h = (anchor_y - target_y).max(MIN_RECT_SIZE);
            min.y = anchor_y - new_h;
        }

        // Dragging TopRight: Fixed Anchor = BottomLeft (min.x, max.y)
        ImageHandle::TopRight => {
            let anchor_x = min.x;
            let anchor_y = max.y;
            let target_x = max.x + delta.x;
            let target_y = min.y + delta.y;
            let new_w = (target_x - anchor_x).max(MIN_RECT_SIZE);
            let new_h = (anchor_y - target_y).max(MIN_RECT_SIZE);
            max.x = anchor_x + new_w;
            min.y = anchor_y - new_h;
        }

        // Dragging RightCenter: Fixed Anchor = Left edge (min.x)
        ImageHandle::RightCenter => {
            let anchor_x = min.x;
            let target_x = max.x + delta.x;
            let new_w = (target_x - anchor_x).max(MIN_RECT_SIZE);
            max.x = anchor_x + new_w;
        }

        // Dragging BottomRight: Fixed Anchor = TopLeft (min.x, min.y)
        ImageHandle::BottomRight => {
            let anchor = min;
            let target = max + delta;
            let new_w = (target.x - anchor.x).max(MIN_RECT_SIZE);
            let new_h = (target.y - anchor.y).max(MIN_RECT_SIZE);
            max.x = anchor.x + new_w;
            max.y = anchor.y + new_h;
        }

        // Dragging BottomCenter: Fixed Anchor = Top edge (min.y)
        ImageHandle::BottomCenter => {
            let anchor_y = min.y;
            let target_y = max.y + delta.y;
            let new_h = (target_y - anchor_y).max(MIN_RECT_SIZE);
            max.y = anchor_y + new_h;
        }

        // Dragging BottomLeft: Fixed Anchor = TopRight (max.x, min.y)
        ImageHandle::BottomLeft => {
            let anchor_x = max.x;
            let anchor_y = min.y;
            let target_x = min.x + delta.x;
            let target_y = max.y + delta.y;
            let new_w = (anchor_x - target_x).max(MIN_RECT_SIZE);
            let new_h = (target_y - anchor_y).max(MIN_RECT_SIZE);
            min.x = anchor_x - new_w;
            max.y = anchor_y + new_h;
        }

        // Dragging LeftCenter: Fixed Anchor = Right edge (max.x)
        ImageHandle::LeftCenter => {
            let anchor_x = max.x;
            let target_x = min.x + delta.x;
            let new_w = (anchor_x - target_x).max(MIN_RECT_SIZE);
            min.x = anchor_x - new_w;
        }
    }

    Rect::from_min_max(min, max)
}

/// **Anchor-Based Proportional Resize System**
///
/// Recalculates the screen bounding box by locking the diagonally opposite point as
/// a fixed anchor while preserving the aspect ratio.
///
/// For corner handles, uses the axis with the larger relative change to scale both
/// width and height uniformly from the locked anchor point, clamped to `>= MIN_RECT_SIZE`.
/// For mid-edge handles, delegates to `apply_resize`.
pub fn apply_resize_proportional(image_rect: Rect, handle: ImageHandle, delta: Vec2) -> Rect {
    if !image_rect.is_finite() || !delta.x.is_finite() || !delta.y.is_finite() {
        return image_rect;
    }
    let w0 = image_rect.width().max(1.0);
    let h0 = image_rect.height().max(1.0);

    match handle {
        ImageHandle::TopLeft => {
            let horizontal = -delta.x / w0;
            let vertical = -delta.y / h0;
            let d = if horizontal.abs() > vertical.abs() { horizontal } else { vertical };
            let k = (1.0 + d).max(MIN_RECT_SIZE / w0).max(MIN_RECT_SIZE / h0);
            let (w, h) = (w0 * k, h0 * k);
            Rect::from_min_max(Pos2::new(image_rect.max.x - w, image_rect.max.y - h), image_rect.max)
        }
        ImageHandle::TopRight => {
            let horizontal = delta.x / w0;
            let vertical = -delta.y / h0;
            let d = if horizontal.abs() > vertical.abs() { horizontal } else { vertical };
            let k = (1.0 + d).max(MIN_RECT_SIZE / w0).max(MIN_RECT_SIZE / h0);
            let (w, h) = (w0 * k, h0 * k);
            Rect::from_min_max(Pos2::new(image_rect.min.x, image_rect.max.y - h), Pos2::new(image_rect.min.x + w, image_rect.max.y))
        }
        ImageHandle::BottomLeft => {
            let horizontal = -delta.x / w0;
            let vertical = delta.y / h0;
            let d = if horizontal.abs() > vertical.abs() { horizontal } else { vertical };
            let k = (1.0 + d).max(MIN_RECT_SIZE / w0).max(MIN_RECT_SIZE / h0);
            let (w, h) = (w0 * k, h0 * k);
            Rect::from_min_max(Pos2::new(image_rect.max.x - w, image_rect.min.y), Pos2::new(image_rect.max.x, image_rect.min.y + h))
        }
        ImageHandle::BottomRight => {
            let horizontal = delta.x / w0;
            let vertical = delta.y / h0;
            let d = if horizontal.abs() > vertical.abs() { horizontal } else { vertical };
            let k = (1.0 + d).max(MIN_RECT_SIZE / w0).max(MIN_RECT_SIZE / h0);
            let (w, h) = (w0 * k, h0 * k);
            Rect::from_min_max(image_rect.min, Pos2::new(image_rect.min.x + w, image_rect.min.y + h))
        }
        mid => apply_resize(image_rect, mid, delta),
    }
}

/// Trim a screen rectangle and its UVs together, keeping the opposite edge fixed.
/// The pixel scale stays unchanged, and each edge stops at the texture boundary.
pub fn apply_crop(image_rect: Rect, uv_rect: Rect, handle: ImageHandle, delta: Vec2) -> (Rect, Rect) {
    if !delta.is_finite() || !image_rect.is_finite() || !uv_rect.is_finite() || !image_rect.is_positive() || !uv_rect.is_positive() {
        return (image_rect, uv_rect);
    }
    let scale = image_rect.size() / uv_rect.size();
    if !scale.is_finite() || scale.x <= 0.0 || scale.y <= 0.0 {
        return (image_rect, uv_rect);
    }
    let minimum = (egui::vec2(MIN_RECT_SIZE, MIN_RECT_SIZE) / scale).min(uv_rect.size() * 0.95);
    let mut uv = uv_rect;
    if matches!(handle, ImageHandle::TopLeft | ImageHandle::LeftCenter | ImageHandle::BottomLeft) {
        uv.min.x = (uv.min.x + delta.x / scale.x).clamp(0.0, (uv.max.x - minimum.x).max(0.0));
    } else if matches!(handle, ImageHandle::TopRight | ImageHandle::RightCenter | ImageHandle::BottomRight) {
        uv.max.x = (uv.max.x + delta.x / scale.x).clamp((uv.min.x + minimum.x).min(1.0), 1.0);
    }
    if matches!(handle, ImageHandle::TopLeft | ImageHandle::TopCenter | ImageHandle::TopRight) {
        uv.min.y = (uv.min.y + delta.y / scale.y).clamp(0.0, (uv.max.y - minimum.y).max(0.0));
    } else if matches!(handle, ImageHandle::BottomLeft | ImageHandle::BottomCenter | ImageHandle::BottomRight) {
        uv.max.y = (uv.max.y + delta.y / scale.y).clamp((uv.min.y + minimum.y).min(1.0), 1.0);
    }
    // Reconstruct from the fixed opposite edge, rather than adding a nearly full-width
    // shift to the moved edge. Calculate in f64 so each screen coordinate rounds only
    // once to f32; small crops far from the origin otherwise lose multiple ulps.
    let span = |min: f32, max: f32| f64::from(max) - f64::from(min);
    let width = span(image_rect.min.x, image_rect.max.x) * span(uv.min.x, uv.max.x) / span(uv_rect.min.x, uv_rect.max.x);
    let height = span(image_rect.min.y, image_rect.max.y) * span(uv.min.y, uv.max.y) / span(uv_rect.min.y, uv_rect.max.y);
    let mut screen = image_rect;
    if uv.min.x != uv_rect.min.x {
        screen.min.x = (f64::from(image_rect.max.x) - width) as f32;
    } else if uv.max.x != uv_rect.max.x {
        screen.max.x = (f64::from(image_rect.min.x) + width) as f32;
    }
    if uv.min.y != uv_rect.min.y {
        screen.min.y = (f64::from(image_rect.max.y) - height) as f32;
    } else if uv.max.y != uv_rect.max.y {
        screen.max.y = (f64::from(image_rect.min.y) + height) as f32;
    }
    if !screen.is_finite() || !screen.is_positive() {
        return (image_rect, uv_rect);
    }
    (screen, uv)
}

// ============================================================================
// 2. VISUAL RENDERING HELPERS
// ============================================================================

/// Vibrant amber/orange color for Crop Mode.
pub const CROP_ACCENT: Color32 = Color32::from_rgb(230, 126, 34);

/// Renders the Rule-of-Thirds composition grid inside the given crop rectangle.
pub fn draw_rule_of_thirds(painter: &egui::Painter, rect: Rect, stroke: Stroke) {
    let w = rect.width();
    let h = rect.height();
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let x1 = rect.min.x + w / 3.0;
    let x2 = rect.min.x + (2.0 * w) / 3.0;
    painter.line_segment([pos2(x1, rect.min.y), pos2(x1, rect.max.y)], stroke);
    painter.line_segment([pos2(x2, rect.min.y), pos2(x2, rect.max.y)], stroke);

    let y1 = rect.min.y + h / 3.0;
    let y2 = rect.min.y + (2.0 * h) / 3.0;
    painter.line_segment([pos2(rect.min.x, y1), pos2(rect.max.x, y1)], stroke);
    painter.line_segment([pos2(rect.min.x, y2), pos2(rect.max.x, y2)], stroke);
}

/// Renders a circular handle with shadow, fill, and accent stroke using the image tool accent.
pub fn draw_handle(painter: &egui::Painter, center: Pos2, is_active: bool, is_hovered: bool, crop_mode: bool, accent_color: Color32) {
    let base_radius = 6.5;
    let circle_radius = if is_active {
        base_radius + 2.0
    } else if is_hovered {
        base_radius + 1.2
    } else {
        base_radius
    };

    let fill_color = if crop_mode {
        if is_active || is_hovered { Color32::from_rgb(255, 230, 150) } else { Color32::from_rgb(255, 255, 255) }
    } else if is_active || is_hovered {
        Color32::from_rgb(210, 235, 255)
    } else {
        Color32::from_rgb(255, 255, 255)
    };

    // Draw handle shadow/glow
    painter.circle_filled(center, circle_radius + 1.5, Color32::from_rgba_unmultiplied(0, 0, 0, 80));

    // Draw handle circle
    painter.circle_filled(center, circle_radius, fill_color);
    painter.circle_stroke(center, circle_radius, Stroke::new(1.8, accent_color));
}

// ============================================================================
// 3. UNIT TESTS: MATHEMATICAL VERIFICATION
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    // Verify scale in screen space: dividing an endpoint's rounding error by a tiny
    // retained UV span would magnify it and make a fixed scale tolerance invalid.
    fn crop_scale_preserved(before: Rect, uv_before: Rect, after: Rect, uv_after: Rect) -> bool {
        let axis = |lo: f32, hi: f32, u0: f32, u1: f32, new_lo: f32, new_hi: f32, v0: f32, v1: f32| {
            let span = f64::from(hi) - f64::from(lo);
            let expected = span * (f64::from(v1) - f64::from(v0)) / (f64::from(u1) - f64::from(u0));
            let actual = f64::from(new_hi) - f64::from(new_lo);
            // One moved endpoint rounds once to f32. EPSILON * coordinate magnitude
            // bounds one ulp (conservatively twice the nearest-rounding error).
            // The f64 term also covers the two arithmetic operations above.
            let magnitude = f64::from(new_lo).abs().max(f64::from(new_hi).abs()).max(expected.abs());
            let bound = f64::from(f32::EPSILON) * magnitude + 4.0 * f64::EPSILON * span.abs();
            (actual - expected).abs() <= bound
        };
        axis(before.min.x, before.max.x, uv_before.min.x, uv_before.max.x, after.min.x, after.max.x, uv_after.min.x, uv_after.max.x)
            && axis(before.min.y, before.max.y, uv_before.min.y, uv_before.max.y, after.min.y, after.max.y, uv_after.min.y, uv_after.max.y)
    }

    #[test]
    fn crop_scale_at_large_screen_offset() {
        let screen = Rect::from_min_size(pos2(813.7157, 0.0), vec2(495.0725, 50.0));
        let uv = Rect::from_min_size(pos2(0.114936605, 0.0), vec2(0.32074833, 0.3));
        let (cropped, kept) = apply_crop(screen, uv, ImageHandle::BottomLeft, vec2(1255.676, 0.0));
        let width = (f64::from(screen.max.x) - f64::from(screen.min.x)) * (f64::from(kept.max.x) - f64::from(kept.min.x))
            / (f64::from(uv.max.x) - f64::from(uv.min.x));
        assert_eq!(cropped.min.x, (f64::from(screen.max.x) - width) as f32, "moved edge rounds once from the fixed anchor");
        assert_eq!(cropped.max.x, screen.max.x, "opposite horizontal edge stays fixed");
        assert_eq!(cropped.min.y, screen.min.y, "opposite vertical edge stays fixed");
        assert!(crop_scale_preserved(screen, uv, cropped, kept));
        let stretched = Rect::from_min_max(cropped.min - vec2(0.1, 0.0), cropped.max);
        assert!(!crop_scale_preserved(screen, uv, stretched, kept), "a visible stretch must exceed the rounding bound");
    }

    #[test]
    fn test_resize_bottom_right_anchor() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        let res = apply_resize(r, ImageHandle::BottomRight, vec2(50.0, 25.0));
        assert_eq!(res.min, pos2(100.0, 100.0), "TopLeft anchor must remain fixed");
        assert_eq!(res.max, pos2(350.0, 225.0), "BottomRight must expand by delta");
    }

    #[test]
    fn test_resize_top_left_anchor() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        let res = apply_resize(r, ImageHandle::TopLeft, vec2(20.0, 15.0));
        assert_eq!(res.max, pos2(300.0, 200.0), "BottomRight anchor must remain fixed");
        assert_eq!(res.min, pos2(120.0, 115.0), "TopLeft must move by delta");
    }

    #[test]
    fn test_resize_clamping_prevents_inversion() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(200.0, 200.0));
        let res = apply_resize(r, ImageHandle::TopLeft, vec2(500.0, 500.0));
        assert_eq!(res.max, pos2(200.0, 200.0));
        assert_eq!(res.min, pos2(200.0 - MIN_RECT_SIZE, 200.0 - MIN_RECT_SIZE));
        assert!(res.width() >= MIN_RECT_SIZE);
        assert!(res.height() >= MIN_RECT_SIZE);
    }

    #[test]
    fn test_resize_top_right_anchor() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        let res = apply_resize(r, ImageHandle::TopRight, vec2(10.0, -10.0));
        assert_eq!(res.min.x, 100.0, "min.x (BL) anchor fixed");
        assert_eq!(res.max.y, 200.0, "max.y (BL) anchor fixed");
        assert_eq!(res.max.x, 310.0);
        assert_eq!(res.min.y, 90.0);
    }

    #[test]
    fn test_resize_bottom_left_anchor() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        let res = apply_resize(r, ImageHandle::BottomLeft, vec2(-15.0, 20.0));
        assert_eq!(res.max.x, 300.0, "max.x (TR) anchor fixed");
        assert_eq!(res.min.y, 100.0, "min.y (TR) anchor fixed");
        assert_eq!(res.min.x, 85.0);
        assert_eq!(res.max.y, 220.0);
    }

    #[test]
    fn test_resize_mid_edges() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));

        let tm = apply_resize(r, ImageHandle::TopCenter, vec2(50.0, -30.0));
        assert_eq!(tm.min.x, 100.0, "X min unaffected");
        assert_eq!(tm.max.x, 300.0, "X max unaffected");
        assert_eq!(tm.max.y, 200.0, "Y max anchor locked");
        assert_eq!(tm.min.y, 70.0);

        let rm = apply_resize(r, ImageHandle::RightCenter, vec2(40.0, 50.0));
        assert_eq!(rm.min.x, 100.0, "X min anchor locked");
        assert_eq!(rm.max.x, 340.0);
        assert_eq!(rm.min.y, 100.0, "Y unaffected");
        assert_eq!(rm.max.y, 200.0, "Y unaffected");
    }

    #[test]
    fn test_crop_lockstep_ratio_preservation() {
        let screen = Rect::from_min_max(pos2(100.0, 100.0), pos2(500.0, 400.0));
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

        let initial_px_per_uv_x = screen.width() / uv.width();
        let initial_px_per_uv_y = screen.height() / uv.height();

        let (new_screen, new_uv) = apply_crop(screen, uv, ImageHandle::RightCenter, vec2(-80.0, 0.0));

        assert_eq!(new_screen.min.x, 100.0, "Left edge anchored");
        assert_eq!(new_screen.max.x, 420.0, "Right edge moved in by 80px");
        assert_eq!(new_screen.width(), 320.0);

        assert_eq!(new_uv.min.x, 0.0);
        assert!((new_uv.max.x - 0.8).abs() < 1e-4);

        let new_px_per_uv_x = new_screen.width() / new_uv.width();
        assert!((new_px_per_uv_x - initial_px_per_uv_x).abs() < 1e-4, "Pixel scale must remain invariant in crop mode");

        let new_px_per_uv_y = new_screen.height() / new_uv.height();
        assert!((new_px_per_uv_y - initial_px_per_uv_y).abs() < 1e-4, "Pixel scale Y must remain invariant");
    }

    #[test]
    fn test_crop_left_edge_anchoring() {
        let screen = Rect::from_min_max(pos2(100.0, 100.0), pos2(500.0, 400.0));
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

        let (new_screen, new_uv) = apply_crop(screen, uv, ImageHandle::LeftCenter, vec2(50.0, 0.0));

        assert_eq!(new_screen.max.x, 500.0, "Right edge must remain strictly anchored");
        assert_eq!(new_screen.min.x, 150.0, "Left edge must move by 50px");
        assert_eq!(new_screen.width(), 350.0);

        assert!((new_uv.min.x - 0.125).abs() < 1e-4);
        assert_eq!(new_uv.max.x, 1.0);
    }

    #[test]
    fn test_crop_uncrop_clamped_at_zero_and_one() {
        let screen = Rect::from_min_max(pos2(100.0, 100.0), pos2(500.0, 400.0));
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

        let (new_screen, new_uv) = apply_crop(screen, uv, ImageHandle::LeftCenter, vec2(-100.0, 0.0));
        assert_eq!(new_uv.min.x, 0.0, "UV min cannot go below 0.0");
        assert_eq!(new_screen.min.x, 100.0, "Screen min clamped at initial position");

        let (new_screen2, new_uv2) = apply_crop(screen, uv, ImageHandle::RightCenter, vec2(100.0, 0.0));
        assert_eq!(new_uv2.max.x, 1.0, "UV max cannot exceed 1.0");
        assert_eq!(new_screen2.max.x, 500.0, "Screen max clamped at initial position");
    }

    #[test]
    fn test_apply_resize_proportional_horizontal_only_scales_both_dimensions() {
        let rect = Rect::from_min_max(pos2(100.0, 100.0), pos2(200.0, 200.0)); // 100 x 100
        // TopRight corner moved horizontally by -25px (from x=200 to x=175)
        let resized = apply_resize_proportional(rect, ImageHandle::TopRight, vec2(-25.0, 0.0));
        assert_eq!(resized.min.x, 100.0, "Left anchor preserved");
        assert_eq!(resized.max.y, 200.0, "Bottom anchor preserved");
        assert_eq!(resized.width(), 75.0, "Width scaled to 75%");
        assert_eq!(resized.height(), 75.0, "Height scaled to 75% proportionally");
        assert_eq!(resized.max.x, 175.0);
        assert_eq!(resized.min.y, 125.0);
    }

    #[test]
    fn test_apply_resize_proportional_bottom_right_anchor_locked() {
        let rect = Rect::from_min_max(pos2(50.0, 50.0), pos2(150.0, 250.0)); // 100 x 200
        // BottomRight moved down/right by (20px, 40px)
        let resized = apply_resize_proportional(rect, ImageHandle::BottomRight, vec2(20.0, 40.0));
        assert_eq!(resized.min, pos2(50.0, 50.0), "TopLeft anchor strictly locked");
        assert_eq!(resized.width(), 120.0);
        assert_eq!(resized.height(), 240.0);
    }

    #[cfg(test)]
    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_handle() -> impl Strategy<Value = ImageHandle> {
            prop_oneof![
                Just(ImageHandle::TopLeft),
                Just(ImageHandle::TopCenter),
                Just(ImageHandle::TopRight),
                Just(ImageHandle::RightCenter),
                Just(ImageHandle::BottomRight),
                Just(ImageHandle::BottomCenter),
                Just(ImageHandle::BottomLeft),
                Just(ImageHandle::LeftCenter),
            ]
        }

        proptest! {
            #[test]
            fn prop_apply_resize_never_inverts_and_respects_min_size(
                min_x in -5000.0f32..5000.0,
                min_y in -5000.0f32..5000.0,
                w in 10.0f32..2000.0,
                h in 10.0f32..2000.0,
                handle in arb_handle(),
                dx in -10000.0f32..10000.0,
                dy in -10000.0f32..10000.0,
            ) {
                let rect = Rect::from_min_size(pos2(min_x, min_y), vec2(w, h));
                let res = apply_resize(rect, handle, vec2(dx, dy));
                prop_assert!(res.is_finite());
                prop_assert!(res.width() >= MIN_RECT_SIZE - 1e-3);
                prop_assert!(res.height() >= MIN_RECT_SIZE - 1e-3);
                prop_assert!(res.min.x <= res.max.x);
                prop_assert!(res.min.y <= res.max.y);
            }

            #[test]
            fn prop_apply_resize_proportional_preserves_aspect_ratio(
                min_x in -2000.0f32..2000.0,
                min_y in -2000.0f32..2000.0,
                w in 20.0f32..1000.0,
                h in 20.0f32..1000.0,
                handle in arb_handle(),
                dx in -5000.0f32..5000.0,
                dy in -5000.0f32..5000.0,
            ) {
                let rect = Rect::from_min_size(pos2(min_x, min_y), vec2(w, h));
                let res = apply_resize_proportional(rect, handle, vec2(dx, dy));
                prop_assert!(res.is_finite());
                prop_assert!(res.width() >= MIN_RECT_SIZE - 1e-3);
                prop_assert!(res.height() >= MIN_RECT_SIZE - 1e-3);

                let is_corner = matches!(handle, ImageHandle::TopLeft | ImageHandle::TopRight | ImageHandle::BottomLeft | ImageHandle::BottomRight);
                if is_corner {
                    let orig_ratio = w / h;
                    let new_ratio = res.width() / res.height();
                    prop_assert!((orig_ratio - new_ratio).abs() <= 1e-3 * orig_ratio.max(1.0), "orig: {orig_ratio}, new: {new_ratio}");
                }
            }

            #[test]
            fn prop_apply_crop_uv_clamped(
                min_x in 0.0f32..1000.0,
                min_y in 0.0f32..1000.0,
                w in 50.0f32..500.0,
                h in 50.0f32..500.0,
                uv_u0 in 0.0f32..0.4,
                uv_v0 in 0.0f32..0.4,
                uv_w in 0.3f32..0.5,
                uv_h in 0.3f32..0.5,
                handle in arb_handle(),
                dx in -2000.0f32..2000.0,
                dy in -2000.0f32..2000.0,
            ) {
                let screen = Rect::from_min_size(pos2(min_x, min_y), vec2(w, h));
                let uv = Rect::from_min_size(pos2(uv_u0, uv_v0), vec2(uv_w, uv_h));
                let (new_screen, new_uv) = apply_crop(screen, uv, handle, vec2(dx, dy));

                prop_assert!(new_screen.is_finite());
                prop_assert!(new_uv.is_finite());
                prop_assert!(new_uv.min.x >= 0.0 - 1e-5);
                prop_assert!(new_uv.min.y >= 0.0 - 1e-5);
                prop_assert!(new_uv.max.x <= 1.0 + 1e-5);
                prop_assert!(new_uv.max.y <= 1.0 + 1e-5);
                prop_assert!(new_uv.width() > 0.0);
                prop_assert!(new_uv.height() > 0.0);

                if !matches!(handle, ImageHandle::TopLeft | ImageHandle::LeftCenter | ImageHandle::BottomLeft) {
                    prop_assert_eq!(new_screen.min.x, screen.min.x, "stationary left edge moved");
                }
                if !matches!(handle, ImageHandle::TopRight | ImageHandle::RightCenter | ImageHandle::BottomRight) {
                    prop_assert_eq!(new_screen.max.x, screen.max.x, "stationary right edge moved");
                }
                if !matches!(handle, ImageHandle::TopLeft | ImageHandle::TopCenter | ImageHandle::TopRight) {
                    prop_assert_eq!(new_screen.min.y, screen.min.y, "stationary top edge moved");
                }
                if !matches!(handle, ImageHandle::BottomLeft | ImageHandle::BottomCenter | ImageHandle::BottomRight) {
                    prop_assert_eq!(new_screen.max.y, screen.max.y, "stationary bottom edge moved");
                }

                prop_assert!(crop_scale_preserved(screen, uv, new_screen, new_uv),
                    "screen/UV scale changed beyond endpoint rounding: {screen:?} {uv:?} => {new_screen:?} {new_uv:?}");
            }

            #[test]
            fn prop_quad_contains_never_panics(
                q0_x in -1000.0f32..1000.0,
                q0_y in -1000.0f32..1000.0,
                q1_x in -1000.0f32..1000.0,
                q1_y in -1000.0f32..1000.0,
                q2_x in -1000.0f32..1000.0,
                q2_y in -1000.0f32..1000.0,
                q3_x in -1000.0f32..1000.0,
                q3_y in -1000.0f32..1000.0,
                px in -2000.0f32..2000.0,
                py in -2000.0f32..2000.0,
            ) {
                let quad = [pos2(q0_x, q0_y), pos2(q1_x, q1_y), pos2(q2_x, q2_y), pos2(q3_x, q3_y)];
                let p = pos2(px, py);
                let _ = crate::edit_text_ui::quad_contains(quad, p);
            }
        }

        #[test]
        fn degenerate_quad_does_not_contain_points() {
            let zero_quad = [pos2(0.0, 0.0), pos2(0.0, 0.0), pos2(0.0, 0.0), pos2(0.0, 0.0)];
            assert!(!crate::edit_text_ui::quad_contains(zero_quad, pos2(10.0, 10.0)));
            assert!(!crate::edit_text_ui::quad_contains(zero_quad, pos2(0.0, 0.0)));
        }
    }
}
