//! How far a mouse-wheel notch scrolls (#444, #186), and ↓ / ↑ with it (#985).
//!
//! egui scrolls `line_scroll_speed` points per wheel line: 40 on the desktop, 8 on the web.
//! macOS already accelerates line deltas, and browsers there scroll about that far per line,
//! but on Windows and Linux winit reports one line per notch, where browsers scroll about
//! 100 pixels, so 40 felt slow. ⌘/Ctrl-wheel zoom keeps egui's rate, and wheel paging in Single
//! page view still turns one page per notch.
//!
//! Touchpads have a speed of their own. Wayland reports a two-finger swipe in points, and egui
//! moved the view only as far as the fingers went: slow next to the same swipe on X11 (or
//! XWayland), where winit reports it as fractional lines, about one per ten points, which egui
//! multiplies by the line speed. So on Wayland touchpad points are scaled by
//! [`ScrollSpeed::touchpad_factor`] before egui reads them, which also carries into the page
//! view's 1:1 path (#759) and its momentum (#531). macOS accelerates its own deltas, and browsers
//! report mouse wheels in points too, so both keep them as they come at Normal. X11 and Windows
//! report touchpads as lines, which follow the wheel setting.

/// The user's choice, relative to the platform's normal speed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScrollSpeed {
    Slow,
    #[default]
    Normal,
    Fast,
}

/// egui's own `line_scroll_speed`, which its wheel zoom rate is tuned against.
pub(crate) const EGUI_LINE_POINTS: f32 = if cfg!(target_arch = "wasm32") { 8.0 } else { 40.0 };

/// Points one wheel line scrolls at Normal speed.
const NORMAL_LINE_POINTS: f32 = if cfg!(target_arch = "wasm32") {
    // The browser's line deltas, which egui already scales for the web.
    8.0
} else if cfg!(target_os = "macos") {
    40.0
} else {
    100.0
};

/// Points the view moves per point of touchpad travel at Normal speed. 4 is how far egui's
/// scroll areas move for the same swipe under XWayland (ten points a line, 40 points a line).
const NORMAL_TOUCHPAD_FACTOR: f32 = if cfg!(any(target_os = "macos", target_arch = "wasm32")) { 1.0 } else { 4.0 };

impl ScrollSpeed {
    pub const ALL: [Self; 3] = [Self::Slow, Self::Normal, Self::Fast];

    pub fn label(self) -> &'static str {
        match self {
            Self::Slow => "Slow",
            Self::Normal => "Normal",
            Self::Fast => "Fast",
        }
    }

    /// This speed relative to Normal.
    pub fn factor(self) -> f32 {
        match self {
            Self::Slow => 0.5,
            Self::Normal => 1.0,
            Self::Fast => 2.0,
        }
    }

    /// Points one wheel line scrolls.
    pub fn line_points(self) -> f32 {
        NORMAL_LINE_POINTS * self.factor()
    }

    /// Makes egui scroll this far per wheel line, from this frame's input on (call it before egui
    /// reads `raw`). egui scales line deltas before turning ⌘/Ctrl-wheel motion into zoom, so
    /// those events are scaled back to keep zooming at egui's own rate.
    pub(crate) fn apply(self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.set_line_speed(ctx);
        let points = self.line_points();
        let zoom_scale = EGUI_LINE_POINTS / points;
        for event in &mut raw.events {
            if let egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta, modifiers, .. } = event
                && modifiers.command
            {
                *delta *= zoom_scale;
            }
        }
    }

    /// Points the view moves per point a touchpad reports.
    pub fn touchpad_factor(self) -> f32 {
        NORMAL_TOUCHPAD_FACTOR * self.factor()
    }

    /// Scales this frame's touchpad (point) scrolling by [`Self::touchpad_factor`] (call it
    /// before egui reads `raw`). ⌘/Ctrl-wheel zooms at egui's own rate, so it is left alone.
    pub(crate) fn apply_touchpad(self, raw: &mut egui::RawInput) {
        let factor = self.touchpad_factor();
        for event in &mut raw.events {
            if let egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, modifiers, .. } = event
                && !modifiers.command
            {
                *delta *= factor;
            }
        }
    }

    /// Makes egui scroll this far per wheel line (from the next frame when called during one).
    pub(crate) fn set_line_speed(self, ctx: &egui::Context) {
        let points = self.line_points();
        if ctx.options(|o| o.input_options.line_scroll_speed) != points {
            ctx.options_mut(|o| o.input_options.line_scroll_speed = points);
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.label().eq_ignore_ascii_case(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(unit: egui::MouseWheelUnit, dy: f32, command: bool) -> egui::Event {
        let modifiers = egui::Modifiers { command, ..egui::Modifiers::NONE };
        egui::Event::MouseWheel { unit, delta: egui::vec2(0.0, dy), phase: egui::TouchPhase::Move, modifiers }
    }

    #[test]
    fn zoom_keeps_egui_rate_and_scrolling_follows_the_choice() {
        let ctx = egui::Context::default();
        for speed in ScrollSpeed::ALL {
            let mut raw = egui::RawInput {
                events: vec![
                    wheel(egui::MouseWheelUnit::Line, 1.0, true),
                    wheel(egui::MouseWheelUnit::Line, 1.0, false),
                    wheel(egui::MouseWheelUnit::Point, 30.0, true),
                ],
                ..Default::default()
            };
            speed.apply(&ctx, &mut raw);
            assert_eq!(ctx.options(|o| o.input_options.line_scroll_speed), speed.line_points());
            let dy: Vec<f32> =
                raw.events.iter().filter_map(|e| if let egui::Event::MouseWheel { delta, .. } = e { Some(delta.y) } else { None }).collect();
            // ⌘/Ctrl lines scaled back so egui's zoom sees EGUI_LINE_POINTS per line; plain lines
            // and point deltas untouched.
            assert!((dy[0] * speed.line_points() - EGUI_LINE_POINTS).abs() < 1e-3, "{speed:?}: {dy:?}");
            assert_eq!((dy[1], dy[2]), (1.0, 30.0));
        }
        assert_eq!(ScrollSpeed::parse("FAST"), Some(ScrollSpeed::Fast));
        assert_eq!(ScrollSpeed::parse("warp"), None);
    }

    #[test]
    fn touchpad_points_follow_the_touchpad_choice_and_nothing_else_does() {
        assert_eq!(ScrollSpeed::Normal.touchpad_factor(), NORMAL_TOUCHPAD_FACTOR);
        assert_eq!(ScrollSpeed::Slow.touchpad_factor() * 4.0, ScrollSpeed::Fast.touchpad_factor());
        for speed in ScrollSpeed::ALL {
            let mut raw = egui::RawInput {
                events: vec![
                    wheel(egui::MouseWheelUnit::Point, -10.0, false),
                    wheel(egui::MouseWheelUnit::Point, 30.0, true),
                    wheel(egui::MouseWheelUnit::Line, 1.0, false),
                    wheel(egui::MouseWheelUnit::Point, f32::NAN, false),
                    egui::Event::PointerMoved(egui::pos2(5.0, 5.0)),
                ],
                ..Default::default()
            };
            speed.apply_touchpad(&mut raw);
            let dy: Vec<f32> =
                raw.events.iter().filter_map(|e| if let egui::Event::MouseWheel { delta, .. } = e { Some(delta.y) } else { None }).collect();
            // Plain touchpad points scaled; ⌘/Ctrl zoom and wheel lines untouched; a junk delta
            // goes on as it came, no worse than before.
            assert_eq!(dy[0], -10.0 * speed.touchpad_factor(), "{speed:?}");
            assert_eq!((dy[1], dy[2]), (30.0, 1.0), "{speed:?}");
            assert!(dy[3].is_nan());
            assert_eq!(raw.events.len(), 5);
        }
    }
}
