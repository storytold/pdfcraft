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
}

impl Default for Motion {
    fn default() -> Self {
        Self { reduced: false, source: None, asked_at: f64::NEG_INFINITY, egui_times: None, wheel: WheelReader::default() }
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
            return;
        }
        let options = ctx.options(|o| o.input_options);
        let motion = ctx.input(|i| self.wheel.read(&i.events, i.viewport_rect().height(), &options));
        ctx.input_mut(|i| i.smooth_scroll_delta = motion.scroll);
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|d| d.insert_temp(zoom_id(), (pass, motion.zoom)));
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

fn zoom_id() -> egui::Id {
    egui::Id::new("pdfcraft-motion-wheel-zoom")
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
                Event::MouseWheel { unit, delta: step, phase, modifiers: latest } => match phase {
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
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    fn wheel(unit: MouseWheelUnit, delta: Vec2, phase: TouchPhase, modifiers: Modifiers) -> Event {
        Event::MouseWheel { unit, delta, phase, modifiers }
    }

    fn notch(dy: f32, modifiers: Modifiers) -> Event {
        wheel(MouseWheelUnit::Line, vec2(0.0, dy), TouchPhase::Move, modifiers)
    }

    fn read(events: &[Event]) -> WheelMotion {
        WheelReader::default().read(events, 600.0, &egui::InputOptions::default())
    }

    #[test]
    fn a_step_is_read_whole_in_its_units() {
        let line = egui::InputOptions::default().line_scroll_speed;
        assert_eq!(read(&[notch(-1.0, Modifiers::NONE)]), WheelMotion { scroll: vec2(0.0, -line), zoom: 1.0 });
        assert_eq!(read(&[notch(-1.0, Modifiers::NONE), notch(-2.0, Modifiers::NONE)]).scroll, vec2(0.0, -3.0 * line), "steps of one pass add up");
        let points = wheel(MouseWheelUnit::Point, vec2(3.0, -250.0), TouchPhase::Move, Modifiers::NONE);
        assert_eq!(read(&[points]).scroll, vec2(3.0, -250.0));
        let page = wheel(MouseWheelUnit::Page, vec2(0.0, -1.0), TouchPhase::Move, Modifiers::NONE);
        assert_eq!(read(&[page]).scroll, vec2(0.0, -600.0));
        assert_eq!(read(&[]), WheelMotion { scroll: Vec2::ZERO, zoom: 1.0 }, "and nothing is left for a later pass");
    }

    #[test]
    fn modifiers_turn_the_wheel_sideways_or_into_zoom() {
        let line = egui::InputOptions::default().line_scroll_speed;
        assert_eq!(read(&[notch(-1.0, Modifiers::SHIFT)]).scroll, vec2(-line, 0.0));
        let sideways = wheel(MouseWheelUnit::Point, vec2(-30.0, 0.0), TouchPhase::Move, Modifiers::ALT);
        assert_eq!(read(&[sideways]).scroll, vec2(0.0, -30.0));
        let zoomed = read(&[notch(1.0, Modifiers::COMMAND)]);
        assert_eq!(zoomed.scroll, Vec2::ZERO, "a zooming wheel doesn't scroll");
        let step = (egui::InputOptions::default().scroll_zoom_speed * line).exp();
        assert!((zoomed.zoom - step).abs() < 1e-6, "{zoomed:?}");
        assert!((read(&[notch(-1.0, Modifiers::COMMAND)]).zoom - 1.0 / step).abs() < 1e-6);
        assert!((read(&[Event::Zoom(1.5), Event::Zoom(2.0)]).zoom - 3.0).abs() < 1e-6, "pinch events multiply");
    }

    #[test]
    fn a_touch_gesture_keeps_its_modifiers_and_ends_clean() {
        let mut reader = WheelReader::default();
        let options = egui::InputOptions::default();
        let point = |dy: f32, phase, modifiers| wheel(MouseWheelUnit::Point, vec2(0.0, dy), phase, modifiers);
        // ⌘ held as the fingers land, let go before the momentum: still zooming.
        let began =
            reader.read(&[point(0.0, TouchPhase::Start, Modifiers::COMMAND), point(20.0, TouchPhase::Move, Modifiers::COMMAND)], 600.0, &options);
        assert!(began.zoom > 1.0 && began.scroll == Vec2::ZERO, "{began:?}");
        let momentum = reader.read(&[point(20.0, TouchPhase::Move, Modifiers::NONE)], 600.0, &options);
        assert!(momentum.zoom > 1.0 && momentum.scroll == Vec2::ZERO, "{momentum:?}");
        // The pass it ends in moves nothing, and the next gesture starts without the modifier.
        let ended = reader.read(&[point(20.0, TouchPhase::Move, Modifiers::NONE), point(0.0, TouchPhase::End, Modifiers::NONE)], 600.0, &options);
        assert_eq!(ended, WheelMotion { scroll: Vec2::ZERO, zoom: 1.0 });
        assert_eq!(reader.read(&[point(-20.0, TouchPhase::Move, Modifiers::NONE)], 600.0, &options).scroll, vec2(0.0, -20.0));
    }

    #[test]
    fn numbers_that_are_not_finite_move_nothing() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let scrolled = read(&[notch(bad, Modifiers::NONE), notch(-1.0, Modifiers::NONE)]);
            assert!(scrolled.scroll.x.is_finite() && scrolled.scroll.y.is_finite(), "{bad}: {scrolled:?}");
            assert_eq!(read(&[notch(bad, Modifiers::COMMAND)]).zoom, 1.0, "{bad}");
            assert_eq!(read(&[Event::Zoom(bad)]).zoom, 1.0, "{bad}");
        }
        assert_eq!(read(&[Event::Zoom(0.0)]).zoom, 1.0);
        assert_eq!(read(&[Event::Zoom(-2.0)]).zoom, 1.0);
        // Too far to be a factor at all.
        assert_eq!(read(&[notch(1.0e9, Modifiers::COMMAND)]).zoom, 1.0);
    }

    /// One pass of a bare egui context, a sixtieth of a second after the last.
    fn pass(ctx: &egui::Context, time: &mut f64, events: Vec<Event>, mut seen: impl FnMut(&egui::Context)) {
        *time += 1.0 / 60.0;
        let raw = egui::RawInput { time: Some(*time), events, ..Default::default() };
        // A bare context's first frame uploads the font atlas; nothing paints it here.
        ctx.run_ui(raw, |ui| seen(ui.ctx())).textures_delta.clear();
    }

    /// The reader must stay egui's own reading minus the easing: over the passes egui takes to
    /// ease a step, it hands out as much scroll (and as much zoom) as the reader does at once.
    #[test]
    fn reading_at_once_goes_as_far_as_egui_eases() {
        let options = egui::InputOptions::default();
        for (modifiers, unit, step) in [
            (Modifiers::NONE, MouseWheelUnit::Line, vec2(0.0, -3.0)),
            (Modifiers::NONE, MouseWheelUnit::Point, vec2(40.0, -250.0)),
            (Modifiers::SHIFT, MouseWheelUnit::Line, vec2(0.0, 2.0)),
            (Modifiers::COMMAND, MouseWheelUnit::Line, vec2(0.0, 1.0)),
            (Modifiers::COMMAND, MouseWheelUnit::Point, vec2(0.0, -90.0)),
        ] {
            let event = wheel(unit, step, TouchPhase::Move, modifiers);
            let ctx = egui::Context::default();
            let (mut time, mut scrolled, mut zoomed, mut first) = (0.0, Vec2::ZERO, 1.0_f32, None);
            let mut events = vec![event.clone()];
            for _ in 0..120 {
                pass(&ctx, &mut time, std::mem::take(&mut events), |ctx| {
                    let (scroll, zoom) = ctx.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
                    first.get_or_insert((scroll, zoom));
                    scrolled += scroll;
                    zoomed *= zoom;
                });
            }
            let page_height = ctx.input(|i| i.viewport_rect().height());
            let at_once = WheelReader::default().read(&[event], page_height, &options);
            assert!((scrolled - at_once.scroll).length() < 0.01, "{modifiers:?} {unit:?}: egui scrolled {scrolled:?}, read {:?}", at_once.scroll);
            assert!((zoomed / at_once.zoom - 1.0).abs() < 1e-3, "{modifiers:?} {unit:?}: egui zoomed {zoomed}, read {}", at_once.zoom);
            // What makes the reader necessary: egui's first pass holds part of the step back.
            let first = first.expect("passes ran");
            assert!(
                first != (at_once.scroll, at_once.zoom),
                "{modifiers:?} {unit:?}: egui no longer eases this step ({first:?}); the reader may not be needed"
            );
        }
    }

    #[test]
    fn reduced_motion_hands_egui_the_whole_step_and_stops_its_animations() {
        let ctx = egui::Context::default();
        let mut motion = Motion { reduced: true, ..Default::default() };
        let line = egui::InputOptions::default().line_scroll_speed;
        let mut time = 0.0;
        let mut seen = Vec::new();
        let mut events = vec![notch(-1.0, Modifiers::NONE)];
        for _ in 0..30 {
            pass(&ctx, &mut time, std::mem::take(&mut events), |ctx| {
                motion.begin_pass(ctx, AnimationEffects::System);
                seen.push(ctx.input(|i| i.smooth_scroll_delta));
            });
        }
        assert_eq!(seen.first(), Some(&vec2(0.0, -line)), "the whole notch in the pass it arrives");
        assert!(seen.iter().skip(1).all(|d| *d == Vec2::ZERO), "and none of egui's easing after it: {seen:?}");
        let style = ctx.global_style();
        assert_eq!((style.animation_time, style.scroll_animation), (0.0, egui::style::ScrollAnimation::none()));
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            assert_eq!(ctx.style_of(theme).animation_time, 0.0, "{theme:?}");
        }

        // ⌘/Ctrl with the wheel: the whole zoom step at once, then nothing.
        let step = (egui::InputOptions::default().scroll_zoom_speed * line).exp();
        let mut zooms = Vec::new();
        let mut events = vec![notch(1.0, Modifiers::COMMAND)];
        for _ in 0..30 {
            pass(&ctx, &mut time, std::mem::take(&mut events), |ctx| {
                motion.begin_pass(ctx, AnimationEffects::System);
                zooms.push(zoom_delta(ctx));
            });
        }
        assert!((zooms[0] - step).abs() < 1e-6, "{zooms:?}");
        assert!(zooms.iter().skip(1).all(|z| *z == 1.0), "{zooms:?}");

        // Animations allowed again: egui's own values and its own times.
        motion.reduced = false;
        let mut eased = Vec::new();
        let mut events = vec![notch(1.0, Modifiers::COMMAND)];
        for _ in 0..3 {
            pass(&ctx, &mut time, std::mem::take(&mut events), |ctx| {
                motion.begin_pass(ctx, AnimationEffects::System);
                eased.push((zoom_delta(ctx), ctx.input(|i| i.zoom_delta())));
            });
        }
        assert!(eased.iter().all(|(ours, eguis)| ours == eguis), "{eased:?}");
        assert!(eased[0].0 > 1.0 && eased[0].0 < step, "egui eases the step: {eased:?}");
        let (style, default) = (ctx.global_style(), egui::Style::default());
        assert_eq!((style.animation_time, style.scroll_animation), (default.animation_time, default.scroll_animation));
    }

    #[test]
    fn the_preference_overrides_the_system_for_every_animation() {
        use AnimationEffects::{Off, On, System};
        assert_eq!(AnimationEffects::default(), System, "the system's setting is followed unless the user chooses");
        for choice in AnimationEffects::ALL {
            assert_eq!(AnimationEffects::parse(choice.name()), Some(choice));
        }
        assert_eq!((AnimationEffects::parse("OFF"), AnimationEffects::parse("smooth")), (Some(Off), None));
        let (line, egui_default) = (egui::InputOptions::default().line_scroll_speed, egui::Style::default());
        let egui_times = (egui_default.animation_time, egui_default.scroll_animation);
        // The system's answer, the choice, and whether the window then animates.
        for (reduced, effects, animates) in
            [(false, System, true), (true, System, false), (false, On, true), (true, On, true), (false, Off, false), (true, Off, false)]
        {
            assert_eq!(effects.animates(reduced), animates, "{effects:?}, system reduced: {reduced}");
            let ctx = egui::Context::default();
            let mut motion = Motion { reduced, ..Default::default() };
            let (mut time, mut first) = (0.0, None);
            pass(&ctx, &mut time, vec![notch(-1.0, Modifiers::NONE)], |ctx| {
                motion.begin_pass(ctx, effects);
                first = Some((ctx.input(|i| i.smooth_scroll_delta), animated(ctx)));
            });
            // The wheel, egui's animations and the app's own fades all go the same way.
            let whole_notch = first.map(|(scroll, _)| scroll) == Some(vec2(0.0, -line));
            assert_eq!(
                (whole_notch, first.map(|(_, animated)| animated)),
                (!animates, Some(animates)),
                "{effects:?}, system reduced: {reduced}: {first:?}"
            );
            let style = ctx.global_style();
            let times = if animates { egui_times } else { (0.0, ScrollAnimation::none()) };
            assert_eq!((style.animation_time, style.scroll_animation), times, "{effects:?}, system reduced: {reduced}");
        }
        // A choice made while running applies from the next pass, and choosing back undoes it.
        let ctx = egui::Context::default();
        let mut motion = Motion::default();
        let mut time = 0.0;
        for (effects, times) in [(Off, (0.0, ScrollAnimation::none())), (System, egui_times), (Off, (0.0, ScrollAnimation::none())), (On, egui_times)]
        {
            pass(&ctx, &mut time, Vec::new(), |ctx| motion.begin_pass(ctx, effects));
            let style = ctx.global_style();
            assert_eq!((style.animation_time, style.scroll_animation), times, "{effects:?}");
        }
        // The row's labels are translated: the heading, and On and Off as they suit it.
        let de = crate::i18n::Lang::from_code("de").expect("German ships");
        assert_eq!(crate::i18n::tr(de, "Animation effects"), "Animationseffekte");
        let labels: Vec<&str> =
            AnimationEffects::ALL.into_iter().map(|choice| crate::i18n::tr_ctx(de, "animation effects", choice.label())).collect();
        assert_eq!(labels, ["Systemeinstellung verwenden", "Ein", "Aus"]);
        for lang in crate::i18n::Lang::all() {
            for english in ["Animation effects", "On", "Off"] {
                let shown = if english == "Animation effects" {
                    crate::i18n::tr(lang, english)
                } else {
                    crate::i18n::tr_ctx(lang, "animation effects", english)
                };
                assert!(lang.code() == "en" || shown != english, "{}: {english} is not translated", lang.code());
            }
        }
    }

    /// The chrome's hover wash has a duration of its own (`theme::Press`): it eases in with
    /// animation effects on and is simply there with them off.
    #[test]
    fn the_hover_wash_eases_only_with_animation_effects_on() {
        for (effects, eased) in [(AnimationEffects::On, true), (AnimationEffects::Off, false)] {
            let ctx = egui::Context::default();
            let mut motion = Motion::default();
            let (mut time, mut hover) = (0.0, Vec::new());
            for _ in 0..12 {
                time += 1.0 / 60.0;
                let raw = egui::RawInput { time: Some(time), events: vec![Event::PointerMoved(egui::pos2(20.0, 20.0))], ..Default::default() };
                ctx.run_ui(raw, |ui| {
                    motion.begin_pass(ui.ctx(), effects);
                    let (_, response) = ui.allocate_exact_size(vec2(100.0, 40.0), egui::Sense::click());
                    hover.push(crate::theme::Press::track(ui, &response).hover);
                })
                .textures_delta
                .clear();
            }
            // The first frame the pointer is found over the control.
            let first = hover.iter().copied().find(|amount| *amount > 0.0).expect("the pointer is over the control");
            assert_eq!(first < 1.0, eased, "{effects:?}: {hover:?}");
            assert_eq!(hover.last(), Some(&1.0), "{effects:?}: the wash is whole in the end either way: {hover:?}");
        }
    }

    #[test]
    fn a_zoom_read_in_an_earlier_pass_is_never_used_again() {
        let ctx = egui::Context::default();
        let mut motion = Motion { reduced: true, ..Default::default() };
        let mut time = 0.0;
        pass(&ctx, &mut time, vec![notch(1.0, Modifiers::COMMAND)], |ctx| {
            motion.begin_pass(ctx, AnimationEffects::System);
            assert!(zoom_delta(ctx) > 1.0);
        });
        // A pass that didn't begin here (a host that draws without `logic`): egui's own answer.
        pass(&ctx, &mut time, Vec::new(), |ctx| {
            let eguis = ctx.input(|i| i.zoom_delta());
            assert_eq!(zoom_delta(ctx), eguis);
        });
    }

    #[test]
    fn the_system_is_asked_now_and_then_and_no_answer_changes_nothing() {
        let ctx = egui::Context::default();
        let answer = Rc::new(Cell::new(Some(true)));
        let asked = Rc::new(Cell::new(0_u32));
        let (source_answer, source_asked) = (Rc::clone(&answer), Rc::clone(&asked));
        let mut motion = Motion {
            source: Some(Box::new(move || {
                source_asked.set(source_asked.get() + 1);
                source_answer.get()
            })),
            ..Default::default()
        };
        let mut time = 0.0;
        pass(&ctx, &mut time, Vec::new(), |ctx| motion.begin_pass(ctx, AnimationEffects::System));
        assert!(motion.reduced, "asked in the first pass");
        answer.set(Some(false));
        for _ in 0..30 {
            pass(&ctx, &mut time, Vec::new(), |ctx| motion.begin_pass(ctx, AnimationEffects::System));
        }
        assert!(motion.reduced && asked.get() == 1, "not asked again within half a second ({} times)", asked.get());
        // Back at the window later: the first pass asks.
        time += ASK_EVERY;
        pass(&ctx, &mut time, Vec::new(), |ctx| motion.begin_pass(ctx, AnimationEffects::System));
        assert!(!motion.reduced);
        answer.set(None);
        motion.reduced = true;
        time += ASK_EVERY;
        pass(&ctx, &mut time, Vec::new(), |ctx| motion.begin_pass(ctx, AnimationEffects::System));
        assert!(motion.reduced && asked.get() == 3, "no answer leaves the last one standing");
    }
}
