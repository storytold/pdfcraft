//! Image manipulation engine for non-destructive crop, anchor-based resize, and movement.
//!
//! Ported and adapted from `Egui_image_resize_sandbox` with clean-room guarantees,
//! zero-panic safety, and lockstep pixel-scale invariance.

use egui::{Color32, CursorIcon, Pos2, Rect, Stroke, Vec2, pos2, vec2};

use crate::edit_text_ui::ImageHandle;

// ============================================================================
// 1. HANDLE DEFINITION & GEOMETRIC ANCHORS
// ============================================================================

/// The 8 anchor handles for image resizing and cropping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandleId {
    TopLeft,
    TopMid,
    TopRight,
    RightMid,
    BottomRight,
    BottomMid,
    BottomLeft,
    LeftMid,
}

impl HandleId {
    /// Ordered list of all 8 handles for iteration.
    pub const ALL: [HandleId; 8] = [
        HandleId::TopLeft,
        HandleId::TopMid,
        HandleId::TopRight,
        HandleId::RightMid,
        HandleId::BottomRight,
        HandleId::BottomMid,
        HandleId::BottomLeft,
        HandleId::LeftMid,
    ];

    /// Returns a human-readable display name.
    pub fn name(&self) -> &'static str {
        match self {
            HandleId::TopLeft => "Top-Left",
            HandleId::TopMid => "Top-Center",
            HandleId::TopRight => "Top-Right",
            HandleId::RightMid => "Right-Center",
            HandleId::BottomRight => "Bottom-Right",
            HandleId::BottomMid => "Bottom-Center",
            HandleId::BottomLeft => "Bottom-Left",
            HandleId::LeftMid => "Left-Center",
        }
    }

    /// Evaluates the handle's current 2D screen position on a given bounding box.
    pub fn position(&self, rect: Rect) -> Pos2 {
        match self {
            HandleId::TopLeft => rect.left_top(),
            HandleId::TopMid => pos2(rect.center().x, rect.top()),
            HandleId::TopRight => rect.right_top(),
            HandleId::RightMid => pos2(rect.right(), rect.center().y),
            HandleId::BottomRight => rect.right_bottom(),
            HandleId::BottomMid => pos2(rect.center().x, rect.bottom()),
            HandleId::BottomLeft => rect.left_bottom(),
            HandleId::LeftMid => pos2(rect.left(), rect.center().y),
        }
    }

    /// Mouse cursor to display when hovering or dragging this handle.
    pub fn cursor_icon(&self) -> CursorIcon {
        match self {
            HandleId::TopLeft | HandleId::BottomRight => CursorIcon::ResizeNwSe,
            HandleId::TopRight | HandleId::BottomLeft => CursorIcon::ResizeNeSw,
            HandleId::TopMid | HandleId::BottomMid => CursorIcon::ResizeVertical,
            HandleId::LeftMid | HandleId::RightMid => CursorIcon::ResizeHorizontal,
        }
    }

    /// Returns a description of the fixed anchor locked during dragging.
    pub fn anchor_description(&self) -> &'static str {
        match self {
            HandleId::TopLeft => "Bottom-Right point locked",
            HandleId::TopMid => "Bottom edge locked",
            HandleId::TopRight => "Bottom-Left point locked",
            HandleId::RightMid => "Left edge locked",
            HandleId::BottomRight => "Top-Left point locked",
            HandleId::BottomMid => "Top edge locked",
            HandleId::BottomLeft => "Top-Right point locked",
            HandleId::LeftMid => "Right edge locked",
        }
    }
}

impl From<HandleId> for ImageHandle {
    fn from(h: HandleId) -> Self {
        match h {
            HandleId::TopLeft => ImageHandle::TopLeft,
            HandleId::TopMid => ImageHandle::TopCenter,
            HandleId::TopRight => ImageHandle::TopRight,
            HandleId::RightMid => ImageHandle::RightCenter,
            HandleId::BottomRight => ImageHandle::BottomRight,
            HandleId::BottomMid => ImageHandle::BottomCenter,
            HandleId::BottomLeft => ImageHandle::BottomLeft,
            HandleId::LeftMid => ImageHandle::LeftCenter,
        }
    }
}

impl From<ImageHandle> for HandleId {
    fn from(h: ImageHandle) -> Self {
        match h {
            ImageHandle::TopLeft => HandleId::TopLeft,
            ImageHandle::TopCenter => HandleId::TopMid,
            ImageHandle::TopRight => HandleId::TopRight,
            ImageHandle::RightCenter => HandleId::RightMid,
            ImageHandle::BottomRight => HandleId::BottomRight,
            ImageHandle::BottomCenter => HandleId::BottomMid,
            ImageHandle::BottomLeft => HandleId::BottomLeft,
            ImageHandle::LeftCenter => HandleId::LeftMid,
        }
    }
}

// ============================================================================
// 2. MATHEMATICAL CORE: ANCHOR-BASED RESIZE & CROP
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
pub fn apply_resize(image_rect: Rect, handle: HandleId, delta: Vec2) -> Rect {
    if !delta.x.is_finite() || !delta.y.is_finite() {
        return image_rect;
    }
    let mut min = image_rect.min;
    let mut max = image_rect.max;

    match handle {
        // Dragging TopLeft: Fixed Anchor = BottomRight (max.x, max.y)
        HandleId::TopLeft => {
            let anchor = max;
            let target = min + delta;
            let new_w = (anchor.x - target.x).max(MIN_RECT_SIZE);
            let new_h = (anchor.y - target.y).max(MIN_RECT_SIZE);
            min.x = anchor.x - new_w;
            min.y = anchor.y - new_h;
        }

        // Dragging TopMid: Fixed Anchor = Bottom edge (max.y)
        HandleId::TopMid => {
            let anchor_y = max.y;
            let target_y = min.y + delta.y;
            let new_h = (anchor_y - target_y).max(MIN_RECT_SIZE);
            min.y = anchor_y - new_h;
        }

        // Dragging TopRight: Fixed Anchor = BottomLeft (min.x, max.y)
        HandleId::TopRight => {
            let anchor_x = min.x;
            let anchor_y = max.y;
            let target_x = max.x + delta.x;
            let target_y = min.y + delta.y;
            let new_w = (target_x - anchor_x).max(MIN_RECT_SIZE);
            let new_h = (anchor_y - target_y).max(MIN_RECT_SIZE);
            max.x = anchor_x + new_w;
            min.y = anchor_y - new_h;
        }

        // Dragging RightMid: Fixed Anchor = Left edge (min.x)
        HandleId::RightMid => {
            let anchor_x = min.x;
            let target_x = max.x + delta.x;
            let new_w = (target_x - anchor_x).max(MIN_RECT_SIZE);
            max.x = anchor_x + new_w;
        }

        // Dragging BottomRight: Fixed Anchor = TopLeft (min.x, min.y)
        HandleId::BottomRight => {
            let anchor = min;
            let target = max + delta;
            let new_w = (target.x - anchor.x).max(MIN_RECT_SIZE);
            let new_h = (target.y - anchor.y).max(MIN_RECT_SIZE);
            max.x = anchor.x + new_w;
            max.y = anchor.y + new_h;
        }

        // Dragging BottomMid: Fixed Anchor = Top edge (min.y)
        HandleId::BottomMid => {
            let anchor_y = min.y;
            let target_y = max.y + delta.y;
            let new_h = (target_y - anchor_y).max(MIN_RECT_SIZE);
            max.y = anchor_y + new_h;
        }

        // Dragging BottomLeft: Fixed Anchor = TopRight (max.x, min.y)
        HandleId::BottomLeft => {
            let anchor_x = max.x;
            let anchor_y = min.y;
            let target_x = min.x + delta.x;
            let target_y = max.y + delta.y;
            let new_w = (anchor_x - target_x).max(MIN_RECT_SIZE);
            let new_h = (target_y - anchor_y).max(MIN_RECT_SIZE);
            min.x = anchor_x - new_w;
            max.y = anchor_y + new_h;
        }

        // Dragging LeftMid: Fixed Anchor = Right edge (max.x)
        HandleId::LeftMid => {
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
pub fn apply_resize_proportional(image_rect: Rect, handle: HandleId, delta: Vec2) -> Rect {
    if !delta.x.is_finite() || !delta.y.is_finite() {
        return image_rect;
    }
    let w0 = image_rect.width().max(1.0);
    let h0 = image_rect.height().max(1.0);

    match handle {
        HandleId::TopLeft => {
            let horizontal = -delta.x / w0;
            let vertical = -delta.y / h0;
            let d = if horizontal.abs() > vertical.abs() { horizontal } else { vertical };
            let k = (1.0 + d).max(MIN_RECT_SIZE / w0).max(MIN_RECT_SIZE / h0);
            let (w, h) = (w0 * k, h0 * k);
            Rect::from_min_max(Pos2::new(image_rect.max.x - w, image_rect.max.y - h), image_rect.max)
        }
        HandleId::TopRight => {
            let horizontal = delta.x / w0;
            let vertical = -delta.y / h0;
            let d = if horizontal.abs() > vertical.abs() { horizontal } else { vertical };
            let k = (1.0 + d).max(MIN_RECT_SIZE / w0).max(MIN_RECT_SIZE / h0);
            let (w, h) = (w0 * k, h0 * k);
            Rect::from_min_max(Pos2::new(image_rect.min.x, image_rect.max.y - h), Pos2::new(image_rect.min.x + w, image_rect.max.y))
        }
        HandleId::BottomLeft => {
            let horizontal = -delta.x / w0;
            let vertical = delta.y / h0;
            let d = if horizontal.abs() > vertical.abs() { horizontal } else { vertical };
            let k = (1.0 + d).max(MIN_RECT_SIZE / w0).max(MIN_RECT_SIZE / h0);
            let (w, h) = (w0 * k, h0 * k);
            Rect::from_min_max(Pos2::new(image_rect.max.x - w, image_rect.min.y), Pos2::new(image_rect.max.x, image_rect.min.y + h))
        }
        HandleId::BottomRight => {
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

/// **Non-Destructive Crop System**
///
/// In Crop Mode:
/// 1. The handles adjust the normalized texture coordinates `uv_rect` ([0.0..1.0]).
/// 2. The screen `image_rect` shrinks/expands proportionally in lockstep.
/// 3. The pixel scale (`screen_pixels / uv_span`) remains invariant, ensuring that
///    the visible image content NEVER stretches, jumps, or distorts.
/// 4. The diagonally opposite point/edge is strictly locked as the fixed anchor.
///
/// Returns: `(new_image_rect, new_uv_rect)`
pub fn apply_crop(image_rect: Rect, uv_rect: Rect, handle: HandleId, delta: Vec2) -> (Rect, Rect) {
    if !delta.x.is_finite() || !delta.y.is_finite() {
        return (image_rect, uv_rect);
    }
    let w_screen = image_rect.width();
    let h_screen = image_rect.height();
    let w_uv = uv_rect.width();
    let h_uv = uv_rect.height();

    // Guard against degraded or non-finite zero rects
    if w_screen <= 0.0
        || h_screen <= 0.0
        || w_uv <= 0.0
        || h_uv <= 0.0
        || !w_screen.is_finite()
        || !h_screen.is_finite()
        || !w_uv.is_finite()
        || !h_uv.is_finite()
    {
        return (image_rect, uv_rect);
    }

    // Invariant ratios: screen pixels per unit UV and UV per screen pixel
    let px_per_uv_x = w_screen / w_uv;
    let px_per_uv_y = h_screen / h_uv;
    let uv_per_px_x = w_uv / w_screen;
    let uv_per_px_y = h_uv / h_screen;

    // Minimum UV span corresponding to MIN_RECT_SIZE pixels
    let min_uv_w = (MIN_RECT_SIZE * uv_per_px_x).min(w_uv * 0.95);
    let min_uv_h = (MIN_RECT_SIZE * uv_per_px_y).min(h_uv * 0.95);

    let mut new_screen = image_rect;
    let mut new_uv = uv_rect;

    // Identify which geometric edges are affected by this handle
    let affects_left = matches!(handle, HandleId::TopLeft | HandleId::LeftMid | HandleId::BottomLeft);
    let affects_right = matches!(handle, HandleId::TopRight | HandleId::RightMid | HandleId::BottomRight);
    let affects_top = matches!(handle, HandleId::TopLeft | HandleId::TopMid | HandleId::TopRight);
    let affects_bottom = matches!(handle, HandleId::BottomLeft | HandleId::BottomMid | HandleId::BottomRight);

    // ------------------------------------------------------------------------
    // HORIZONTAL (X) CROP CALCULATION
    // ------------------------------------------------------------------------
    if affects_left {
        // Dragging Left: Fixed Anchor is the Right edge
        let screen_anchor_x = image_rect.max.x;
        let uv_anchor_u = uv_rect.max.x;

        // Apply pointer delta in UV space
        let target_u_min = uv_rect.min.x + delta.x * uv_per_px_x;

        // Clamp: Cannot uncrop past 0.0 (texture edge), nor exceed anchor - min_uv_w
        let clamped_u_min = target_u_min.clamp(0.0, (uv_anchor_u - min_uv_w).max(0.0));

        // Recalculate screen bounding box from anchor using invariant scale
        let new_uv_w = uv_anchor_u - clamped_u_min;
        let new_screen_w = new_uv_w * px_per_uv_x;

        new_uv.min.x = clamped_u_min;
        new_screen.min.x = screen_anchor_x - new_screen_w;
        new_screen.max.x = screen_anchor_x; // Explicitly locked
    } else if affects_right {
        // Dragging Right: Fixed Anchor is the Left edge
        let screen_anchor_x = image_rect.min.x;
        let uv_anchor_u = uv_rect.min.x;

        let target_u_max = uv_rect.max.x + delta.x * uv_per_px_x;

        // Clamp: Cannot crop narrower than anchor + min_uv_w, nor exceed 1.0 (texture edge)
        let clamped_u_max = target_u_max.clamp((uv_anchor_u + min_uv_w).min(1.0), 1.0);

        let new_uv_w = clamped_u_max - uv_anchor_u;
        let new_screen_w = new_uv_w * px_per_uv_x;

        new_uv.max.x = clamped_u_max;
        new_screen.min.x = screen_anchor_x; // Explicitly locked
        new_screen.max.x = screen_anchor_x + new_screen_w;
    }

    // ------------------------------------------------------------------------
    // VERTICAL (Y) CROP CALCULATION
    // ------------------------------------------------------------------------
    if affects_top {
        // Dragging Top: Fixed Anchor is the Bottom edge
        let screen_anchor_y = image_rect.max.y;
        let uv_anchor_v = uv_rect.max.y;

        let target_v_min = uv_rect.min.y + delta.y * uv_per_px_y;

        // Clamp: Cannot uncrop past 0.0, nor exceed anchor - min_uv_h
        let clamped_v_min = target_v_min.clamp(0.0, (uv_anchor_v - min_uv_h).max(0.0));

        let new_uv_h = uv_anchor_v - clamped_v_min;
        let new_screen_h = new_uv_h * px_per_uv_y;

        new_uv.min.y = clamped_v_min;
        new_screen.min.y = screen_anchor_y - new_screen_h;
        new_screen.max.y = screen_anchor_y; // Explicitly locked
    } else if affects_bottom {
        // Dragging Bottom: Fixed Anchor is the Top edge
        let screen_anchor_y = image_rect.min.y;
        let uv_anchor_v = uv_rect.min.y;

        let target_v_max = uv_rect.max.y + delta.y * uv_per_px_y;

        // Clamp: Cannot crop smaller than anchor + min_uv_h, nor exceed 1.0
        let clamped_v_max = target_v_max.clamp((uv_anchor_v + min_uv_h).min(1.0), 1.0);

        let new_uv_h = clamped_v_max - uv_anchor_v;
        let new_screen_h = new_uv_h * px_per_uv_y;

        new_uv.max.y = clamped_v_max;
        new_screen.min.y = screen_anchor_y; // Explicitly locked
        new_screen.max.y = screen_anchor_y + new_screen_h;
    }

    (new_screen, new_uv)
}

/// Resets the crop boundary back to [0.0..1.0] and expands `image_rect`
/// to reveal the full image at the current screen pixel scale without shifting existing pixels.
pub fn reset_crop(image_rect: Rect, uv_rect: Rect) -> (Rect, Rect) {
    if uv_rect.width() <= 0.0 || uv_rect.height() <= 0.0 || !uv_rect.width().is_finite() || !uv_rect.height().is_finite() {
        return (image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
    }

    let px_per_uv_x = image_rect.width() / uv_rect.width();
    let px_per_uv_y = image_rect.height() / uv_rect.height();

    let full_min_x = image_rect.min.x - uv_rect.min.x * px_per_uv_x;
    let full_min_y = image_rect.min.y - uv_rect.min.y * px_per_uv_y;
    let full_w = px_per_uv_x;
    let full_h = px_per_uv_y;

    let new_rect = Rect::from_min_size(pos2(full_min_x, full_min_y), vec2(full_w, full_h));
    let new_uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    (new_rect, new_uv)
}

// ============================================================================
// 3. VISUAL RENDERING HELPERS
// ============================================================================

/// Vibrant amber/orange color for Crop Mode.
pub const CROP_ACCENT: Color32 = Color32::from_rgb(230, 126, 34);

/// Crisp blue color for Resize Mode.
pub const RESIZE_ACCENT: Color32 = Color32::from_rgb(52, 152, 219);

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

/// Renders a circular handle with shadow, fill, and accent stroke matching the sandbox.
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
// 4. UNIT TESTS: MATHEMATICAL VERIFICATION
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resize_bottom_right_anchor() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        let res = apply_resize(r, HandleId::BottomRight, vec2(50.0, 25.0));
        assert_eq!(res.min, pos2(100.0, 100.0), "TopLeft anchor must remain fixed");
        assert_eq!(res.max, pos2(350.0, 225.0), "BottomRight must expand by delta");
    }

    #[test]
    fn test_resize_top_left_anchor() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        let res = apply_resize(r, HandleId::TopLeft, vec2(20.0, 15.0));
        assert_eq!(res.max, pos2(300.0, 200.0), "BottomRight anchor must remain fixed");
        assert_eq!(res.min, pos2(120.0, 115.0), "TopLeft must move by delta");
    }

    #[test]
    fn test_resize_clamping_prevents_inversion() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(200.0, 200.0));
        let res = apply_resize(r, HandleId::TopLeft, vec2(500.0, 500.0));
        assert_eq!(res.max, pos2(200.0, 200.0));
        assert_eq!(res.min, pos2(200.0 - MIN_RECT_SIZE, 200.0 - MIN_RECT_SIZE));
        assert!(res.width() >= MIN_RECT_SIZE);
        assert!(res.height() >= MIN_RECT_SIZE);
    }

    #[test]
    fn test_resize_top_right_anchor() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        let res = apply_resize(r, HandleId::TopRight, vec2(10.0, -10.0));
        assert_eq!(res.min.x, 100.0, "min.x (BL) anchor fixed");
        assert_eq!(res.max.y, 200.0, "max.y (BL) anchor fixed");
        assert_eq!(res.max.x, 310.0);
        assert_eq!(res.min.y, 90.0);
    }

    #[test]
    fn test_resize_bottom_left_anchor() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        let res = apply_resize(r, HandleId::BottomLeft, vec2(-15.0, 20.0));
        assert_eq!(res.max.x, 300.0, "max.x (TR) anchor fixed");
        assert_eq!(res.min.y, 100.0, "min.y (TR) anchor fixed");
        assert_eq!(res.min.x, 85.0);
        assert_eq!(res.max.y, 220.0);
    }

    #[test]
    fn test_resize_mid_edges() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));

        let tm = apply_resize(r, HandleId::TopMid, vec2(50.0, -30.0));
        assert_eq!(tm.min.x, 100.0, "X min unaffected");
        assert_eq!(tm.max.x, 300.0, "X max unaffected");
        assert_eq!(tm.max.y, 200.0, "Y max anchor locked");
        assert_eq!(tm.min.y, 70.0);

        let rm = apply_resize(r, HandleId::RightMid, vec2(40.0, 50.0));
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

        let (new_screen, new_uv) = apply_crop(screen, uv, HandleId::RightMid, vec2(-80.0, 0.0));

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

        let (new_screen, new_uv) = apply_crop(screen, uv, HandleId::LeftMid, vec2(50.0, 0.0));

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

        let (new_screen, new_uv) = apply_crop(screen, uv, HandleId::LeftMid, vec2(-100.0, 0.0));
        assert_eq!(new_uv.min.x, 0.0, "UV min cannot go below 0.0");
        assert_eq!(new_screen.min.x, 100.0, "Screen min clamped at initial position");

        let (new_screen2, new_uv2) = apply_crop(screen, uv, HandleId::RightMid, vec2(100.0, 0.0));
        assert_eq!(new_uv2.max.x, 1.0, "UV max cannot exceed 1.0");
        assert_eq!(new_screen2.max.x, 500.0, "Screen max clamped at initial position");
    }

    #[test]
    fn test_reset_crop() {
        let screen = Rect::from_min_max(pos2(150.0, 100.0), pos2(450.0, 350.0)); // 300 x 250
        let uv = Rect::from_min_max(pos2(0.25, 0.0), pos2(0.75, 1.0)); // 0.5 x 1.0

        let (full_rect, full_uv) = reset_crop(screen, uv);
        assert_eq!(full_uv, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
        assert_eq!(full_rect.width(), 600.0);
        assert_eq!(full_rect.height(), 250.0);
        assert_eq!(full_rect.min.x, 0.0);
    }

    #[test]
    fn test_apply_resize_proportional_horizontal_only_scales_both_dimensions() {
        let rect = Rect::from_min_max(pos2(100.0, 100.0), pos2(200.0, 200.0)); // 100 x 100
        // TopRight corner moved horizontally by -25px (from x=200 to x=175)
        let resized = apply_resize_proportional(rect, HandleId::TopRight, vec2(-25.0, 0.0));
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
        let resized = apply_resize_proportional(rect, HandleId::BottomRight, vec2(20.0, 40.0));
        assert_eq!(resized.min, pos2(50.0, 50.0), "TopLeft anchor strictly locked");
        assert_eq!(resized.width(), 120.0);
        assert_eq!(resized.height(), 240.0);
    }
}
