//! Animation effects, and the system's request to do without them.
//!
//! Windows' Settings ▸ Accessibility ▸ Visual effects ▸ Animation effects, turned off, asks
//! apps not to animate (other systems have a Reduce motion switch for it). The desktop app
//! reads the setting ([`MotionSource`]), and Preferences ▸ Animation effects
//! ([`AnimationEffects`]) follows it unless the user chose On or Off there. With animation
//! effects off, nothing in the window eases:
//!
//! - A wheel notch scrolls its whole distance in the frame it arrives. egui spreads a notch
//!   over about a quarter of a second and has no switch for that, so each frame's wheel input
//!   is read again here, the way egui reads it but for the easing ([`WheelReader`]), and handed
//!   to egui's scroll areas in place of its own.
//! - ⌘/Ctrl-wheel zooms by a notch's whole step at once: egui makes its zoom factor from the
//!   same eased wheel motion. Views read [`zoom_delta`] instead of egui's.
//! - Scrolling to a place (a row revealed by the keys, the Hand tool's drag) takes no time.
//! - egui's other animations (menus fading in, sections opening) take no time: its style's
//!   animation time is zero, which the app's own fades and sweeps ask about too ([`animated`]).
//!
//! How far anything scrolls or zooms doesn't change, only how long it takes.

use egui::style::ScrollAnimation;
use egui::{Event, Modifiers, MouseWheelUnit, TouchPhase, Vec2, vec2};

/// Preferences ▸ Animation effects: whether anything in the window is animated, the wheel's
/// scrolling and zooming included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnimationEffects {
    /// Animated unless the system asks for no animations ([`Motion::reduced`]).
    #[default]
    System,
    On,
    Off,
}

impl AnimationEffects {
    pub const ALL: [Self; 3] = [Self::System, Self::On, Self::Off];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Use system setting",
            Self::On => "On",
            Self::Off => "Off",
        }
    }

    /// The name in saved settings and on the control channel.
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::On => "on",
            Self::Off => "off",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|choice| choice.name().eq_ignore_ascii_case(value))
    }

    /// Whether the window animates, given whether the system asks for no animations.
    pub fn animates(self, system_reduced: bool) -> bool {
        match self {
            Self::System => !system_reduced,
            Self::On => true,
            Self::Off => false,
        }
    }
}

/// Asks the system whether it wants no animations; `None` when it can't say, which leaves the
/// last answer standing. Asked from the UI thread every [`ASK_EVERY`] seconds while the window
/// is in use, so it must answer at once.
pub type MotionSource = Box<dyn Fn() -> Option<bool>>;

/// Seconds between two questions to [`MotionSource`]. Changing the setting takes longer than
/// this away from the window, so the first frame back already follows it.
const ASK_EVERY: f64 = 2.0;

/// What the system says about animations, and what it takes to stop egui's.
pub struct Motion {
    /// The system asks for no animations. The desktop app's [`Self::source`] sets it; false
    /// elsewhere, so tests and screenshots don't depend on the machine.
    pub reduced: bool,
    /// How to ask the system (the desktop app sets it).
    pub source: Option<MotionSource>,
    /// egui time of the last question.
    asked_at: f64,
    /// egui's own animation time and scroll animation, kept while they are changed here.
    egui_times: Option<(f32, ScrollAnimation)>,
    wheel: WheelReader,
    /// This frame's wheel motion, read on its first pass and reused on later passes.
    ///
    /// egui empties `RawInput` between the passes of a multi-pass frame, so reading the wheel
    /// again in a later pass sees nothing. Only the first pass is handed the motion (a scroll
    /// area keeps the offset it moved to, a view keeps the zoom it set); later passes are handed
    /// no scroll and a unit zoom, explicitly.
    frame: Option<(u64, WheelMotion)>,
}

impl Default for Motion {
    /// Start motion handling with animations enabled and no desktop preference sampled yet.
    fn default() -> Self {
        Self { reduced: false, source: None, asked_at: f64::NEG_INFINITY, egui_times: None, wheel: WheelReader::default(), frame: None }
    }
}

impl Motion {
    /// Call once at the start of every pass, before any ui reads the wheel or draws.
    pub(crate) fn begin_pass(&mut self, ctx: &egui::Context, effects: AnimationEffects) {
        let now = ctx.input(|i| i.time);
        // `>=` is false for a NaN time, which then never asks.
        if now - self.asked_at >= ASK_EVERY
            && let Some(source) = &self.source
        {
            self.asked_at = now;
            if let Some(reduced) = source() {
                self.reduced = reduced;
            }
        }
        let animate = effects.animates(self.reduced);
        self.sync_style(ctx, animate);
        if animate {
            // A gesture begun while easing is egui's to finish.
            self.wheel = WheelReader::default();
            self.frame = None;
            return;
        }
        let options = ctx.options(|o| o.input_options);
        // One frame may run several passes (`request_discard`). Only the first pass has this
        // frame's events, and only it may hand the wheel on: a scroll area applies the delta to
        // its persisted offset, and a view multiplies the zoom into its persisted zoom, so
        // replaying either would apply it twice. egui also rebuilds `smooth_scroll_delta` and
        // its zoom factor from its retained wheel state every pass, so later passes must be
        // given zero scroll and a unit zoom explicitly. A missing zoom entry would not do: it
        // would fall back to egui's rebuilt factor.
        let frame = ctx.cumulative_frame_nr();
        let first_pass = match self.frame {
            Some((read_in, _)) => read_in != frame,
            None => true,
        };
        let motion = match self.frame {
            Some((read_in, motion)) if read_in == frame => motion,
            _ => {
                let motion = ctx.input(|i| self.wheel.read(&i.events, i.viewport_rect().height(), &options));
                self.frame = Some((frame, motion));
                motion
            }
        };
        let (scroll, zoom) = if first_pass { (motion.scroll, motion.zoom) } else { (Vec2::ZERO, 1.0) };
        ctx.input_mut(|i| i.smooth_scroll_delta = scroll);
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|d| d.insert_temp(zoom_id(), (pass, zoom)));
        // Existing kinetic scrolling remains owned by egui; gating wheel input does not
        // suppress a scroll area's per-pass advancement of retained velocity.
    }

    /// egui's animations, and its scrolling to a place, take no time unless the window is to
    /// `animate`; then both get back the times egui had.
    fn sync_style(&mut self, ctx: &egui::Context, animate: bool) {
        if animate && self.egui_times.is_none() {
            return;
        }
        // Checked every pass, so a style replaced since can't bring the animations back.
        let style = ctx.global_style();
        let egui_times = *self.egui_times.get_or_insert((style.animation_time, style.scroll_animation));
        let wanted = if animate { egui_times } else { (0.0, ScrollAnimation::none()) };
        if (style.animation_time, style.scroll_animation) != wanted {
            ctx.all_styles_mut(|s| (s.animation_time, s.scroll_animation) = wanted);
        }
        if animate {
            self.egui_times = None;
        }
    }
}

/// Whether the window animates this pass. The app's own fades and sweeps, which have their own
/// durations, ask this; egui's animations stop by themselves ([`Motion::begin_pass`]).
pub(crate) fn animated(ctx: &egui::Context) -> bool {
    ctx.global_style().animation_time > 0.0
}

/// Return the stable egui id holding the last wheel-zoom input state.
fn zoom_id() -> egui::Id {
    egui::Id::unique("pdfcraft-motion-wheel-zoom")
}

/// This pass's zoom factor from a pinch or ⌘/Ctrl with the wheel (1.0 for none): egui's
/// `zoom_delta`, without the easing while animation effects are off.
pub(crate) fn zoom_delta(ctx: &egui::Context) -> f32 {
    let read: Option<(u64, f32)> = ctx.data(|d| d.get_temp(zoom_id()));
    let pass = ctx.cumulative_pass_nr();
    ctx.input(|i| match read {
        // Only what [`Motion::begin_pass`] read in this very pass: a factor left from an earlier
        // one would zoom again every frame. Fingers on a touch screen are measured, never eased.
        Some((at, zoom)) if at == pass && i.multi_touch().is_none() => zoom,
        _ => i.zoom_delta(),
    })
}

/// One pass's wheel input.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WheelMotion {
    /// Points to scroll by, as `InputState::smooth_scroll_delta` gives them (positive moves the
    /// content right and down).
    scroll: Vec2,
    /// The zoom factor from pinch events and ⌘/Ctrl with the wheel.
    zoom: f32,
}

/// Reads wheel events as egui's `WheelState` does (egui 0.36, `input_state/wheel_state.rs`):
/// the same units, modifiers and touch phases, but every step lands whole in its own pass,
/// where egui keeps part of a coarse one for later passes. Compare the two when updating egui;
/// `tests::reading_at_once_goes_as_far_as_egui_eases` fails if they come apart.
#[derive(Default)]
struct WheelReader {
    /// The modifiers of the touch gesture in progress (`TouchPhase::Start` to `End`). They hold
    /// for the whole gesture, as in egui: its momentum arrives after the keys are let go.
    touch: Option<Modifiers>,
}

impl WheelReader {
    /// `page_height` is what one `MouseWheelUnit::Page` scrolls: the window's height in points.
    fn read(&mut self, events: &[Event], page_height: f32, options: &egui::InputOptions) -> WheelMotion {
        let mut delta = Vec2::ZERO;
        let mut zoom = 1.0_f32;
        let mut modifiers = self.touch.unwrap_or_default();
        for event in events {
            match event {
                Event::MouseWheel { unit, delta: step, phase, modifiers: latest, .. } => match phase {
                    TouchPhase::Start => {
                        self.touch = Some(*latest);
                        modifiers = *latest;
                    }
                    TouchPhase::Move => {
                        modifiers = match &mut self.touch {
                            Some(held) => {
                                *held |= *latest;
                                *held
                            }
                            None => *latest,
                        };
                        let mut step = match unit {
                            MouseWheelUnit::Point => *step,
                            MouseWheelUnit::Line => options.line_scroll_speed * *step,
                            MouseWheelUnit::Page => page_height * *step,
                        };
                        let horizontal = modifiers.matches_any(options.horizontal_scroll_modifier);
                        let vertical = modifiers.matches_any(options.vertical_scroll_modifier);
                        if horizontal && !vertical {
                            step = vec2(step.x + step.y, 0.0);
                        }
                        if !horizontal && vertical {
                            step = vec2(0.0, step.x + step.y);
                        }
                        // The system's numbers: one that isn't finite would end up in a scroll offset.
                        if step.x.is_finite() && step.y.is_finite() {
                            delta += step;
                        }
                    }
                    // egui drops what the gesture moved earlier in the pass it ends in.
                    TouchPhase::End | TouchPhase::Cancel => {
                        self.touch = None;
                        modifiers = Modifiers::NONE;
                        delta = Vec2::ZERO;
                    }
                },
                Event::Zoom(factor) => zoom *= *factor,
                _ => {}
            }
        }
        // A pass is all zoom or all scroll, by the modifiers it ends with, as in egui.
        let mut scroll = delta;
        if modifiers.matches_any(options.zoom_modifier) {
            zoom *= (options.scroll_zoom_speed * (delta.x + delta.y)).exp();
            scroll = Vec2::ZERO;
        }
        if !(zoom.is_finite() && zoom > 0.0) {
            zoom = 1.0;
        }
        WheelMotion { scroll, zoom }
    }
}

#[cfg(test)]
mod tests;
