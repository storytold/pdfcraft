//! Bounded acceleration for coarse wheel input in the document viewport.
//!
//! Line-unit events describe notches, not motion over time. A fixed distance per
//! notch makes a fast spin feel slow, so this module derives a bounded multiplier
//! from arrival time and direction. The state is constant-size and leaves isolated
//! notches unchanged.

use egui::Vec2;

/// Idle time after which a burst is forgotten, in seconds.
const RESET_GAP_SECS: f64 = 0.3;
/// Notch spacing at or below which input counts as a continuous spin, in seconds.
const FAST_GAP_SECS: f64 = 0.08;
/// Multiplier gained per fast consecutive notch.
const BOOST_PER_TICK: f32 = 0.4;
/// Hard limit for accelerated motion.
const MAX_BOOST: f32 = 6.0;
/// Ticks that affect the output.
const MAX_TICKS: u32 = 13;
/// Extra ticks retained beyond the ceiling. They absorb a few slow events
/// before the visible multiplier starts to fall.
const HYSTERESIS_TICKS: u32 = 4;
/// Maximum scroll applied from one frame of coarse input, in points per axis.
/// Each event is clamped first; this caps the sum.
pub(crate) const MAX_FRAME_POINTS: f32 = 8192.0;
/// Per-event clamp, in native units (lines or pages). Keeps one hostile event
/// bounded before points conversion.
pub(crate) const MAX_EVENT_UNITS: f32 = 10.0;

/// Constant-size arrival-rate state for line wheel events.
#[derive(Debug, Default)]
pub(crate) struct WheelBoost {
    last: f64,
    burst: u32,
    dir: i8,
}

impl WheelBoost {
    /// Returns the multiplier for one bucket of motion carrying `motion`.
    ///
    /// `motion` is signed in native units; only its sign matters. Zero means
    /// no motion. Non-finite time returns 1.0 and leaves state unchanged;
    /// backwards time resets the burst and returns 1.0.
    pub(crate) fn multiplier(&mut self, now: f64, motion: f32) -> f32 {
        if !now.is_finite() || !motion.is_finite() || motion == 0.0 {
            return 1.0;
        }
        let dir = if motion > 0.0 { 1 } else { -1 };
        let idle = now - self.last;
        if !idle.is_finite() || idle < 0.0 {
            self.last = now;
            self.burst = 0;
            self.dir = dir;
            return 1.0;
        }
        if idle > RESET_GAP_SECS || dir != self.dir {
            self.burst = 0;
        } else if idle <= FAST_GAP_SECS {
            self.burst = self.burst.saturating_add(1).min(MAX_TICKS + HYSTERESIS_TICKS);
        } else {
            // Slow input in the same direction decays rather than extending the burst.
            self.burst = self.burst.saturating_sub(1);
        }
        self.last = now;
        self.dir = dir;
        let ticks = self.burst.min(MAX_TICKS);
        // MAX_TICKS is well within f32's exact integer range.
        (1.0 + ticks as f32 * BOOST_PER_TICK).min(MAX_BOOST)
    }

    /// Drops burst state. Call after a programmatic scroll jump.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Caps one frame of accumulated coarse scroll, per axis. A non-finite axis
/// maps to zero; the other axis is preserved.
pub(crate) fn cap_frame(delta: Vec2) -> Vec2 {
    Vec2::new(
        if delta.x.is_finite() { delta.x.clamp(-MAX_FRAME_POINTS, MAX_FRAME_POINTS) } else { 0.0 },
        if delta.y.is_finite() { delta.y.clamp(-MAX_FRAME_POINTS, MAX_FRAME_POINTS) } else { 0.0 },
    )
}

/// One frame of wheel input, normalized in a single pass. Line motion is in
/// points; page motion remains in native units until the viewport is known.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FrameInput {
    pub precise: Vec2,
    pub line: Vec2,
    pub page: Vec2,
}

/// Folds raw wheel events into frame totals without consulting time or UI state.
/// Invalid scales contribute no line motion.
pub(crate) fn accumulate(events: &[egui::Event], line_points: f32, single_paging: bool) -> FrameInput {
    let line_points = sanitize_scale(line_points);
    let mut out = FrameInput::default();
    for event in events {
        let egui::Event::MouseWheel { unit, delta, modifiers, .. } = event else { continue };
        if modifiers.command || modifiers.ctrl {
            continue; // Zooming, not scrolling.
        }
        if !delta.is_finite() {
            continue;
        }
        match unit {
            egui::MouseWheelUnit::Point if modifiers.shift => {
                // Shift scrolls sideways, matching egui's own wheel handling.
                let sum = delta.x + delta.y;
                if sum.is_finite() {
                    out.precise.x += sum;
                }
            }
            egui::MouseWheelUnit::Point => out.precise += *delta,
            egui::MouseWheelUnit::Line if modifiers.shift => {
                let sum = delta.x + delta.y;
                if sum.is_finite() {
                    out.line.x += sum.clamp(-MAX_EVENT_UNITS, MAX_EVENT_UNITS) * line_points;
                }
            }
            egui::MouseWheelUnit::Line => {
                out.line.x += delta.x.clamp(-MAX_EVENT_UNITS, MAX_EVENT_UNITS) * line_points;
                if !single_paging {
                    out.line.y += delta.y.clamp(-MAX_EVENT_UNITS, MAX_EVENT_UNITS) * line_points;
                }
            }
            egui::MouseWheelUnit::Page if modifiers.shift => {
                let sum = delta.x + delta.y;
                if sum.is_finite() {
                    out.page.x += sum.clamp(-MAX_EVENT_UNITS, MAX_EVENT_UNITS);
                }
            }
            egui::MouseWheelUnit::Page => {
                out.page.x += delta.x.clamp(-MAX_EVENT_UNITS, MAX_EVENT_UNITS);
                out.page.y += delta.y.clamp(-MAX_EVENT_UNITS, MAX_EVENT_UNITS);
            }
        }
    }
    // Bound batched events before acceleration is applied.
    out.precise = cap_frame(out.precise);
    out.line = cap_frame(out.line);
    out.page = cap_frame(out.page);
    out
}

/// Converts the frame total to the extra scroll delta beyond egui's share.
///
/// egui applies the base distance through its own smoothing. This function
/// returns only the accelerated surplus, so an isolated notch adds nothing.
/// Each axis advances once per frame; events batched into one frame cannot
/// accelerate one another. Output is frame-capped.
pub(crate) fn boost_extra(input: FrameInput, boost_x: &mut WheelBoost, boost_y: &mut WheelBoost, now: f64) -> Vec2 {
    let mut extra = Vec2::ZERO;
    if input.line.x != 0.0 && input.line.x.is_finite() {
        extra.x += input.line.x * (boost_x.multiplier(now, input.line.x) - 1.0);
    }
    if input.line.y != 0.0 && input.line.y.is_finite() {
        extra.y += input.line.y * (boost_y.multiplier(now, input.line.y) - 1.0);
    }
    cap_frame(extra)
}

/// The accelerated surplus still to scroll. egui eases a notch's base distance over the frames
/// after it (90% within 0.1 s); the surplus follows the same curve, so a fast spin moves as
/// smoothly as single notches rather than jumping each time one arrives (#5371).
#[derive(Debug, Default)]
pub(crate) struct Surplus {
    pending: Vec2,
    /// When it last stepped, in egui's input time.
    at: f64,
}

impl Surplus {
    /// Adds the surplus of the frame at `now` and returns the part to scroll now: all of it when
    /// the window doesn't `animate` (egui's notches then move at once too), else egui's eased
    /// share for a frame of `dt` seconds. `line` is the frame's line-wheel motion.
    pub(crate) fn step(&mut self, added: Vec2, line: Vec2, now: f64, dt: f32, animate: bool) -> Vec2 {
        // Drawn every frame while some is left, a view would have scrolled it all by now: one
        // that wasn't (another tab was in front) doesn't jump on its return.
        let gap = now - self.at;
        if !gap.is_finite() || !(0.0..=RESET_GAP_SECS).contains(&gap) {
            self.pending = Vec2::ZERO;
        }
        self.at = now;
        // Turning the wheel back drops what the old direction still owed. The first notch back
        // adds no surplus of its own (its burst starts again at 1×), so its direction is read
        // from the notch itself.
        if line.x * self.pending.x < 0.0 {
            self.pending.x = 0.0;
        }
        if line.y * self.pending.y < 0.0 {
            self.pending.y = 0.0;
        }
        let dt = if dt.is_finite() && dt > 0.0 { dt.min(0.1) } else { 1.0 / 60.0 };
        let share = if animate { egui::emath::exponential_smooth_factor(0.90, 0.1, dt) } else { 1.0 };
        let (x, pending_x) = ease_axis(self.pending.x, added.x, share);
        let (y, pending_y) = ease_axis(self.pending.y, added.y, share);
        self.pending = Vec2::new(pending_x, pending_y);
        Vec2::new(x, y)
    }

    /// Whether some surplus is still to scroll, so the next frames must be drawn.
    pub(crate) fn pending(&self) -> bool {
        self.pending != Vec2::ZERO
    }

    /// Drops what is still to scroll vertically: the pager owns that axis.
    pub(crate) fn stop_vertical(&mut self) {
        self.pending.y = 0.0;
    }

    /// Drops what is still to scroll. Call after a programmatic scroll jump.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// One axis of [`Surplus::step`]: (scroll now, still to scroll). What is still to scroll is held
/// to one frame's cap of wheel input, so a free-spinning wheel can't build up a backlog that goes
/// on scrolling long after it stops. Under a point, the rest goes at once, as egui's does.
fn ease_axis(pending: f32, added: f32, share: f32) -> (f32, f32) {
    let added = if added.is_finite() { added } else { 0.0 };
    let total = (pending + added).clamp(-MAX_FRAME_POINTS, MAX_FRAME_POINTS);
    if total.abs() < 1.0 {
        return (total, 0.0);
    }
    let now = total * share;
    (now, total - now)
}

/// Maps a non-finite or non-positive points-per-unit scale to zero.
fn sanitize_scale(points: f32) -> f32 {
    if points.is_finite() && points > 0.0 { points } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_and_slow_notches_stay_unboosted() {
        let mut b = WheelBoost::default();
        assert_eq!(b.multiplier(0.0, -1.0), 1.0);
        assert_eq!(b.multiplier(0.5, -1.0), 1.0);
        assert_eq!(b.multiplier(1.0, -1.0), 1.0);
    }

    #[test]
    fn fast_spin_accelerates_then_caps() {
        let mut b = WheelBoost::default();
        assert_eq!(b.multiplier(1.0, -1.0), 1.0);
        let mut last = 1.0;
        for (i, t) in (1..=20).map(|i| 1.0 + f64::from(i) * 0.02).enumerate() {
            last = b.multiplier(t, -1.0);
            if i < 2 {
                assert!(last >= 1.0, "early burst must not jump, got {last}");
            }
        }
        assert!(last > 2.0, "a sustained fast spin must accelerate, got {last}");
        assert!(last <= MAX_BOOST, "boost must be capped, got {last}");
        assert_eq!(last, MAX_BOOST, "a long spin should reach the ceiling");
        for i in 1..=20 {
            let m = b.multiplier(1.5 + f64::from(i) * 0.02, -1.0);
            assert!(m <= MAX_BOOST, "boost must stay capped, got {m}");
        }
    }

    #[test]
    fn reversal_and_idle_reset_the_burst() {
        let mut b = WheelBoost::default();
        b.multiplier(1.0, -1.0);
        for i in 1..=6 {
            b.multiplier(1.0 + f64::from(i) * 0.02, -1.0);
        }
        let boosted = b.multiplier(1.0 + 7.0 * 0.02, -1.0);
        assert!(boosted > 1.0, "precondition: burst is boosted, got {boosted}");
        assert_eq!(b.multiplier(1.0 + 8.0 * 0.02, 1.0), 1.0);
        for i in 9..=14 {
            b.multiplier(1.0 + f64::from(i) * 0.02, 1.0);
        }
        assert_eq!(b.multiplier(5.0, 1.0), 1.0);
    }

    #[test]
    fn invalid_input_never_boosts_or_corrupts() {
        let mut b = WheelBoost::default();
        for bad_now in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(b.multiplier(bad_now, -1.0), 1.0);
        }
        for bad_motion in [0.0, -0.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(b.multiplier(1.0, bad_motion), 1.0);
        }
        assert_eq!(b.multiplier(2.0, -1.0), 1.0);
        b.reset();
        assert_eq!(b.multiplier(3.0, -1.0), 1.0);
    }

    #[test]
    fn same_timestamp_events_count_as_a_burst() {
        let mut b = WheelBoost::default();
        assert_eq!(b.multiplier(1.0, -1.0), 1.0);
        for _ in 0..6 {
            b.multiplier(1.0, -1.0);
        }
        assert!(b.multiplier(1.0, -1.0) > 1.0);
    }

    #[test]
    fn fractional_notches_share_the_burst() {
        let mut b = WheelBoost::default();
        b.multiplier(1.0, -0.25);
        for i in 1..=8 {
            b.multiplier(1.0 + f64::from(i) * 0.02, -0.25);
        }
        assert!(b.multiplier(1.0 + 9.0 * 0.02, -0.25) > 1.0);
    }

    #[test]
    fn sleep_wake_resets_the_burst() {
        let mut b = WheelBoost::default();
        b.multiplier(1.0, -1.0);
        for i in 1..=6 {
            b.multiplier(1.0 + f64::from(i) * 0.02, -1.0);
        }
        assert_eq!(b.multiplier(3600.0, -1.0), 1.0);
    }

    #[test]
    fn backwards_time_resets_without_corrupting() {
        let mut b = WheelBoost::default();
        b.multiplier(1.0, -1.0);
        b.multiplier(1.02, -1.0);
        assert_eq!(b.multiplier(0.5, -1.0), 1.0);
        assert_eq!(b.multiplier(0.52, -1.0), 1.4);
    }

    #[test]
    fn unhurried_spin_decays() {
        let mut b = WheelBoost::default();
        b.multiplier(1.0, -1.0);
        for i in 1..=8 {
            b.multiplier(1.0 + f64::from(i) * 0.02, -1.0);
        }
        let peak = b.multiplier(1.0 + 9.0 * 0.02, -1.0);
        assert!(peak > 1.0);
        // Same direction but slower than the fast gap: each event sheds one tick.
        let mut prev = peak;
        for i in 10..=13 {
            let m = b.multiplier(1.0 + f64::from(i) * 0.15, -1.0);
            assert!(m <= prev, "decay must not grow: {m} > {prev}");
            prev = m;
        }
    }

    #[test]
    fn hysteresis_holds_the_ceiling_briefly() {
        let mut b = WheelBoost::default();
        let mut t = 1.0;
        b.multiplier(t, -1.0);
        for _ in 0..20 {
            t += 0.02;
            b.multiplier(t, -1.0);
        }
        assert_eq!(b.multiplier(t + 0.02, -1.0), MAX_BOOST);
        // A couple of unhurried events consume hysteresis before output falls.
        t += 0.15;
        assert_eq!(b.multiplier(t, -1.0), MAX_BOOST);
        t += 0.15;
        assert_eq!(b.multiplier(t, -1.0), MAX_BOOST);
    }

    #[test]
    fn frame_cap_bounds_each_axis() {
        use egui::vec2;
        assert_eq!(cap_frame(vec2(40.0, -40.0)), vec2(40.0, -40.0));
        assert_eq!(cap_frame(vec2(1e9, -1e9)), vec2(MAX_FRAME_POINTS, -MAX_FRAME_POINTS));
        assert_eq!(cap_frame(vec2(f32::NAN, 40.0)), vec2(0.0, 40.0));
        assert_eq!(cap_frame(vec2(40.0, f32::INFINITY)), vec2(40.0, 0.0));
    }

    fn wheel(unit: egui::MouseWheelUnit, dx: f32, dy: f32) -> egui::Event {
        egui::Event::MouseWheel {
            unit,
            delta: egui::vec2(dx, dy),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        }
    }

    fn shifted(unit: egui::MouseWheelUnit, dx: f32, dy: f32) -> egui::Event {
        egui::Event::MouseWheel {
            unit,
            delta: egui::vec2(dx, dy),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::SHIFT,
            source: egui::MouseWheelSource::Unknown,
        }
    }

    #[test]
    fn isolated_notch_adds_no_extra() {
        use egui::MouseWheelUnit::Line;
        for base in [50.0, 100.0, 200.0, 40.0, 8.0] {
            let input = accumulate(&[wheel(Line, 0.0, -1.0)], base, false);
            assert_eq!(input.line, egui::vec2(0.0, -base));
            let mut bx = WheelBoost::default();
            let mut by = WheelBoost::default();
            assert_eq!(boost_extra(input, &mut bx, &mut by, 1.0), egui::Vec2::ZERO);
        }
    }

    #[test]
    fn fast_spin_adds_a_bounded_extra() {
        use egui::MouseWheelUnit::Line;
        let mut bx = WheelBoost::default();
        let mut by = WheelBoost::default();
        let mut peak = 0.0f32;
        for i in 0..20 {
            let input = accumulate(&[wheel(Line, 0.0, -1.0)], 100.0, false);
            let extra = boost_extra(input, &mut bx, &mut by, 1.0 + f64::from(i) * 0.02);
            assert!(extra.y <= 0.0, "extra follows the spin direction, got {extra:?}");
            peak = peak.min(extra.y);
        }
        assert!(peak < 0.0, "a sustained spin must add an extra");
        assert!(peak >= -MAX_FRAME_POINTS, "extra stays frame-capped, got {peak}");
    }

    #[test]
    fn point_and_page_events_stay_out_of_the_line_boost() {
        use egui::{MouseWheelUnit::Line, MouseWheelUnit::Page, MouseWheelUnit::Point};
        let input = accumulate(&[wheel(Page, 0.0, -1.0), wheel(Point, 0.0, -30.0)], 100.0, false);
        assert_eq!(input.precise, egui::vec2(0.0, -30.0));
        assert_eq!(input.page, egui::vec2(0.0, -1.0));
        assert_eq!(input.line, egui::Vec2::ZERO);
        let input = accumulate(&[wheel(Line, 0.0, -1.0), wheel(Page, 0.0, -1.0)], 100.0, false);
        assert_eq!(input.line, egui::vec2(0.0, -100.0));
        assert_eq!(input.page, egui::vec2(0.0, -1.0));
        let mut bx = WheelBoost::default();
        let mut by = WheelBoost::default();
        assert_eq!(boost_extra(input, &mut bx, &mut by, 1.0), egui::Vec2::ZERO);
    }

    #[test]
    fn single_paging_drops_vertical_keeps_horizontal() {
        use egui::MouseWheelUnit::Line;
        let input = accumulate(&[wheel(Line, 5.0, -1.0)], 100.0, true);
        assert_eq!(input.line, egui::vec2(500.0, 0.0));
    }

    #[test]
    fn shift_merges_into_horizontal() {
        use egui::MouseWheelUnit::{Line, Page};
        let input = accumulate(&[shifted(Line, 1.0, -2.0)], 100.0, false);
        assert_eq!(input.line, egui::vec2(-100.0, 0.0));
        let input = accumulate(&[shifted(Page, 1.0, -2.0)], 100.0, false);
        assert_eq!(input.page, egui::vec2(-1.0, 0.0));
    }

    /// Check that NaN, zero, negative or infinite scales and non-wheel events add nothing.
    #[test]
    fn invalid_scales_and_events_contribute_nothing() {
        use egui::MouseWheelUnit::Line;
        for bad in [f32::NAN, 0.0, -10.0, f32::INFINITY] {
            let input = accumulate(&[wheel(Line, 0.0, -1.0)], bad, false);
            assert_eq!(input.line, egui::Vec2::ZERO, "line scale {bad}");
        }
        let zoom = egui::Event::MouseWheel {
            unit: Line,
            delta: egui::vec2(0.0, -1.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::COMMAND,
            source: egui::MouseWheelSource::Unknown,
        };
        let key = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let input = accumulate(&[zoom, key, wheel(Line, 0.0, f32::NAN)], 100.0, false);
        assert_eq!(input, FrameInput::default());
    }

    /// A notch down, as `accumulate` gives it, and no notch.
    const DOWN: egui::Vec2 = egui::vec2(0.0, -100.0);
    const NONE: egui::Vec2 = egui::Vec2::ZERO;

    #[test]
    fn a_surplus_eases_like_egui_and_all_of_it_arrives() {
        let dt = 1.0 / 60.0;
        let mut s = Surplus::default();
        let first = s.step(egui::vec2(0.0, -500.0), DOWN, 0.0, dt, true);
        let share = egui::emath::exponential_smooth_factor(0.90, 0.1, dt);
        assert!((first.y - -500.0 * share).abs() < 0.01, "the first frame takes egui's share: {first:?}");
        let mut moved = first.y;
        for frame in 1..60 {
            moved += s.step(NONE, NONE, f64::from(frame) * f64::from(dt), dt, true).y;
        }
        assert!((moved + 500.0).abs() < 0.01, "the whole surplus arrives: {moved}");
        assert!(!s.pending());
        // Without animation it all goes at once, like egui's notches then.
        assert_eq!(s.step(egui::vec2(0.0, -500.0), DOWN, 1.0, dt, false), egui::vec2(0.0, -500.0));
        assert!(!s.pending());
    }

    #[test]
    fn a_surplus_is_dropped_on_reversal_reset_paging_and_a_long_gap() {
        let dt = 1.0 / 60.0;
        let mut s = Surplus::default();
        s.step(egui::vec2(0.0, -500.0), DOWN, 0.0, dt, true);
        // The first notch back adds no surplus (its burst starts at 1×), yet it drops what the
        // old direction still owed.
        assert_eq!(s.step(NONE, -DOWN, 0.02, dt, true), NONE, "nothing more scrolls down");
        assert!(!s.pending());
        let mut s = Surplus::default();
        s.step(egui::vec2(-300.0, -500.0), egui::vec2(-100.0, -100.0), 0.0, dt, true);
        s.stop_vertical();
        let rest = s.step(NONE, NONE, 0.02, dt, true);
        assert!(rest.x < 0.0 && rest.y == 0.0, "only the horizontal surplus is left: {rest:?}");
        s.reset();
        assert!(!s.pending());
        assert_eq!(s.step(NONE, NONE, 0.04, dt, true), NONE);
        // A view not drawn for a while (another tab in front) doesn't scroll on its return, and
        // neither does one whose clock went back.
        for later in [5.0, 4.0, f64::NAN] {
            let mut s = Surplus::default();
            s.step(egui::vec2(0.0, -500.0), DOWN, 4.5, dt, true);
            assert_eq!(s.step(NONE, NONE, later, dt, true), NONE, "back at {later}");
            assert!(!s.pending());
        }
    }

    #[test]
    fn a_surplus_survives_bad_input() {
        let mut s = Surplus::default();
        let inputs = [(f32::NAN, 0.016), (f32::INFINITY, 0.016), (-500.0, f32::NAN), (-500.0, 0.0), (-500.0, -1.0), (-500.0, f32::INFINITY)];
        for (frame, (added, dt)) in (0_u8..).zip(inputs) {
            let now = s.step(egui::vec2(0.0, added), egui::vec2(f32::NAN, 0.0), f64::from(frame) * 0.016, dt, true);
            assert!(now.is_finite(), "{added} at {dt}: {now:?}");
        }
        // The backlog stays within one frame's cap of wheel input.
        s.step(egui::vec2(0.0, -1.0e9), DOWN, 0.1, 0.016, true);
        assert!(s.step(NONE, NONE, 0.11, 0.1, false).y >= -MAX_FRAME_POINTS, "capped like every frame of wheel input");
    }
}
