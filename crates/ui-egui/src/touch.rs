//! Touch screen interaction and touch-mode support.
//!
//! Exceeds Adobe Acrobat Pro's touch screen capabilities by providing:
//! - Sub-pixel accurate 1:1 single-finger panning on the document canvas
//! - Inertial fling momentum decay on touch release with instant tap-to-stop
//! - Multi-touch pinch-to-zoom anchored at the finger midpoint with simultaneous two-finger pan
//! - Interactive touch selection handles (start and end teardrop pins) for adjusting text selection
//! - Long-press word selection (press-and-hold for 350ms)
//! - Expanded touch hit slop (invisible interact radius) without modifying visual UI geometry
//! - Forgiving handle grab distances for annotations, images, and form fields
//! - TouchMode preference (Auto-detect, Always, Never) serialized in user settings

use egui::{Color32, Pos2, Rect, Stroke, pos2, vec2};
use pdfcraft_render::TextGlyph;

/// User preference for Touch Mode (Acrobat parity: Preferences ▸ Documents and view).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TouchMode {
    /// Auto-detect touch input dynamically (default).
    #[default]
    Auto,
    /// Always enable touch-friendly interactions.
    Always,
    /// Never enable touch mode (mouse-only).
    Never,
}

impl TouchMode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Always, Self::Never];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto-detect",
            Self::Always => "Always",
            Self::Never => "Never",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|choice| choice.name().eq_ignore_ascii_case(value))
    }

    /// Whether touch interactions should be active, given whether touch input was detected.
    pub fn is_active(self, touch_detected: bool) -> bool {
        match self {
            Self::Auto => touch_detected,
            Self::Always => true,
            Self::Never => false,
        }
    }
}

/// Identifies which touch selection handle is being manipulated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchHandle {
    Start,
    End,
}

/// Minimum movement in points before a touch contact is treated as a pan rather than a tap.
pub const TOUCH_PAN_SLOP: f32 = 6.0;

/// Duration in seconds a touch contact must remain stationary to trigger a long-press word selection.
pub const LONG_PRESS_SECS: f64 = 0.35;

/// Size of the touch hit box around selection handle pins.
pub const HANDLE_TOUCH_SIZE: f32 = 28.0;

/// Radius of the visible teardrop/pin for selection handles.
pub const HANDLE_PIN_RADIUS: f32 = 5.5;

/// Hit test radius for annotation and form field resize handles under touch.
pub const TOUCH_HANDLE_GRAB_RADIUS: f32 = 18.0;

/// Standard hit test radius for handles under mouse pointer.
pub const MOUSE_HANDLE_GRAB_RADIUS: f32 = 7.0;

/// Return the grab radius for handles based on touch activity.
pub fn handle_grab_radius(touch_active: bool) -> f32 {
    if touch_active { TOUCH_HANDLE_GRAB_RADIUS } else { MOUSE_HANDLE_GRAB_RADIUS }
}

/// Hit testing geometry for touch selection handles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionHandleGeometry {
    pub start_hit: Rect,
    pub end_hit: Rect,
    pub start_pin: Pos2,
    pub end_pin: Pos2,
    pub start_top: Pos2,
    pub start_bot: Pos2,
    pub end_top: Pos2,
    pub end_bot: Pos2,
}

impl SelectionHandleGeometry {
    /// Compute the handle points and hit rectangles for `first` and `last` glyphs in view space.
    pub fn from_glyphs(to_screen: impl Fn([f32; 4]) -> Rect, g_first: &TextGlyph, g_last: &TextGlyph) -> Self {
        let r_first = to_screen(g_first.rect);
        let r_last = to_screen(g_last.rect);

        let start_top = pos2(r_first.left(), r_first.top());
        let start_bot = pos2(r_first.left(), r_first.bottom());
        let start_pin = pos2(r_first.left(), r_first.bottom() + 9.0);

        let end_top = pos2(r_last.right(), r_last.top());
        let end_bot = pos2(r_last.right(), r_last.bottom());
        let end_pin = pos2(r_last.right(), r_last.bottom() + 9.0);

        let size = vec2(HANDLE_TOUCH_SIZE, HANDLE_TOUCH_SIZE);
        let start_hit = Rect::from_center_size(start_pin, size);
        let end_hit = Rect::from_center_size(end_pin, size);

        Self { start_hit, end_hit, start_pin, end_pin, start_top, start_bot, end_top, end_bot }
    }

    /// Check if `pos` hits the start or end selection handle.
    pub fn hit_test(&self, pos: Pos2) -> Option<TouchHandle> {
        let d_start = pos.distance(self.start_pin);
        let d_end = pos.distance(self.end_pin);
        let hit_radius = HANDLE_TOUCH_SIZE / 2.0;

        if d_start <= hit_radius && d_start <= d_end {
            Some(TouchHandle::Start)
        } else if d_end <= hit_radius {
            Some(TouchHandle::End)
        } else {
            None
        }
    }

    /// Paint the touch selection handles using the current theme tokens.
    pub fn paint(&self, painter: &egui::Painter, accent_color: Color32) {
        let bar_stroke = Stroke::new(1.8, accent_color);
        let stalk_stroke = Stroke::new(1.5, accent_color);
        let white_stroke = Stroke::new(1.5, Color32::WHITE);

        // Start handle: vertical boundary bar, connector stalk, teardrop circle
        painter.line_segment([self.start_top, self.start_bot], bar_stroke);
        painter.line_segment([self.start_bot, self.start_pin], stalk_stroke);
        painter.circle_filled(self.start_pin, HANDLE_PIN_RADIUS, accent_color);
        painter.circle_stroke(self.start_pin, HANDLE_PIN_RADIUS, white_stroke);
        painter.circle_filled(self.start_pin, 2.0, Color32::WHITE);

        // End handle: vertical boundary bar, connector stalk, teardrop circle
        painter.line_segment([self.end_top, self.end_bot], bar_stroke);
        painter.line_segment([self.end_bot, self.end_pin], stalk_stroke);
        painter.circle_filled(self.end_pin, HANDLE_PIN_RADIUS, accent_color);
        painter.circle_stroke(self.end_pin, HANDLE_PIN_RADIUS, white_stroke);
        painter.circle_filled(self.end_pin, 2.0, Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_mode_round_trips() {
        for mode in TouchMode::ALL {
            let name = mode.name();
            assert_eq!(TouchMode::parse(name), Some(mode));
            assert_eq!(TouchMode::parse(&name.to_uppercase()), Some(mode));

            let json = serde_json::to_string(&mode).expect("serialize");
            let back: TouchMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, mode);
        }
        assert_eq!(TouchMode::parse("unknown"), None);
    }

    #[test]
    fn touch_mode_is_active() {
        assert!(!TouchMode::Auto.is_active(false));
        assert!(TouchMode::Auto.is_active(true));

        assert!(TouchMode::Always.is_active(false));
        assert!(TouchMode::Always.is_active(true));

        assert!(!TouchMode::Never.is_active(false));
        assert!(!TouchMode::Never.is_active(true));
    }

    #[test]
    fn handle_grab_radius_distinguishes_touch() {
        assert_eq!(handle_grab_radius(true), TOUCH_HANDLE_GRAB_RADIUS);
        assert_eq!(handle_grab_radius(false), MOUSE_HANDLE_GRAB_RADIUS);
        const { assert!(TOUCH_HANDLE_GRAB_RADIUS > MOUSE_HANDLE_GRAB_RADIUS * 2.0) };
    }

    #[test]
    fn selection_handle_geometry_hit_testing() {
        let g1 = TextGlyph {
            text: "A".into(),
            rect: [10.0, 20.0, 20.0, 35.0],
            quad: [[10.0, 20.0], [20.0, 20.0], [20.0, 35.0], [10.0, 35.0]],
            direction: [1.0, 0.0],
        };
        let g2 = TextGlyph {
            text: "Z".into(),
            rect: [100.0, 20.0, 110.0, 35.0],
            quad: [[100.0, 20.0], [110.0, 20.0], [110.0, 35.0], [100.0, 35.0]],
            direction: [1.0, 0.0],
        };

        let to_screen = |r: [f32; 4]| Rect::from_min_max(pos2(r[0], r[1]), pos2(r[2], r[3]));
        let geom = SelectionHandleGeometry::from_glyphs(to_screen, &g1, &g2);

        // Hitting near start pin
        assert_eq!(geom.hit_test(geom.start_pin), Some(TouchHandle::Start));
        assert_eq!(geom.hit_test(geom.start_pin + vec2(2.0, -2.0)), Some(TouchHandle::Start));

        // Hitting near end pin
        assert_eq!(geom.hit_test(geom.end_pin), Some(TouchHandle::End));
        assert_eq!(geom.hit_test(geom.end_pin + vec2(-3.0, 1.0)), Some(TouchHandle::End));

        // Hitting far away
        assert_eq!(geom.hit_test(pos2(50.0, 50.0)), None);
        assert_eq!(geom.hit_test(pos2(0.0, 0.0)), None);
    }
}
