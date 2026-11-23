//! How far a mouse-wheel notch scrolls (#444, #186), and ↓ / ↑ with it (#985).
//!
//! egui scrolls `line_scroll_speed` points per wheel line: 40 on the desktop, 8 on the web.
//! macOS already accelerates line deltas, and browsers there scroll about that far per line,
//! but on Windows and Linux winit reports one line per notch, where browsers scroll about
//! 100 pixels, so 40 felt slow. Wheel input reported in points (most trackpads) keeps its own
//! speed, ⌘/Ctrl-wheel zoom keeps egui's rate, and wheel paging in Single page view still turns
//! one page per notch.
//!
//! Touchpad input reported in points is the exception on Linux. A Wayland compositor sends the
//! raw finger distance (about 5 px per event at a comfortable swipe), which winit passes on 1:1.
//! Under X11 the same gesture arrives as wheel lines and is scaled by [`NORMAL_LINE_POINTS`], so
//! the same swipe scrolled several times as far and the Wayland window felt slow. Point input
//! now gets a gain there too, which follows the same Slow / Normal / Fast choice.

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

/// How much touchpad motion reported in points is multiplied at Normal speed. Only Linux asks for
/// any: Windows and macOS already deliver touchpad deltas that feel right.
const NORMAL_POINT_GAIN: f32 = if cfg!(target_os = "linux") { 3.0 } else { 1.0 };

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

    /// What touchpad motion reported in points is multiplied by: 1 everywhere but Linux, where it
    /// follows the choice (Slow 1.5, Normal 3, Fast 6).
    pub fn point_gain(self) -> f32 {
        if cfg!(target_os = "linux") { NORMAL_POINT_GAIN * self.factor() } else { 1.0 }
    }

    /// Makes egui scroll this far per wheel line, from this frame's input on (call it before egui
    /// reads `raw`). egui scales line deltas before turning ⌘/Ctrl-wheel motion into zoom, so
    /// those events are scaled back to keep zooming at egui's own rate.
    pub(crate) fn apply(self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.set_line_speed(ctx);
        let points = self.line_points();
        let zoom_scale = EGUI_LINE_POINTS / points;
        let point_gain = self.point_gain();
        for event in &mut raw.events {
            let egui::Event::MouseWheel { unit, delta, modifiers, .. } = event else { continue };
            match unit {
                egui::MouseWheelUnit::Line if modifiers.command => *delta *= zoom_scale,
                // Zooming (Ctrl/⌘ with the wheel) is tuned against the unscaled motion.
                egui::MouseWheelUnit::Point if point_gain != 1.0 && !modifiers.command && !modifiers.ctrl => {
                    *delta *= point_gain;
                }
                _ => {}
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
        egui::Event::MouseWheel {
            unit,
            delta: egui::vec2(0.0, dy),
            phase: egui::TouchPhase::Move,
            modifiers,
            source: egui::MouseWheelSource::Unknown,
        }
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

    fn point_event(dy: f32, command: bool, ctrl: bool) -> egui::Event {
        let modifiers = egui::Modifiers { command, ctrl, ..egui::Modifiers::NONE };
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, dy),
            phase: egui::TouchPhase::Move,
            modifiers,
            source: egui::MouseWheelSource::Unknown,
        }
    }

    fn point_dy(speed: ScrollSpeed, command: bool, ctrl: bool) -> f32 {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput { events: vec![point_event(5.0, command, ctrl)], ..Default::default() };
        speed.apply(&ctx, &mut raw);
        raw.events.iter().find_map(|e| if let egui::Event::MouseWheel { delta, .. } = e { Some(delta.y) } else { None }).expect("the wheel event")
    }

    #[test]
    fn touchpad_points_follow_the_choice_on_linux_only() {
        let expected = |gain: f32| if cfg!(target_os = "linux") { 5.0 * gain } else { 5.0 };
        assert_eq!(point_dy(ScrollSpeed::Slow, false, false), expected(1.5));
        assert_eq!(point_dy(ScrollSpeed::Normal, false, false), expected(3.0));
        assert_eq!(point_dy(ScrollSpeed::Fast, false, false), expected(6.0));
    }

    #[test]
    fn ctrl_and_command_wheel_zoom_is_never_scaled() {
        for speed in ScrollSpeed::ALL {
            assert_eq!(point_dy(speed, true, false), 5.0, "{speed:?} command");
            assert_eq!(point_dy(speed, false, true), 5.0, "{speed:?} ctrl");
        }
    }
}
