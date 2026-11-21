//! Bounded acceleration for coarse wheel input in the document viewport.
//!
//! Coarse line events use a bounded arrival-rate multiplier and one motion curve
//! for their whole distance. Isolated notches retain their distance. Fractional line
//! components stay direct on their own axes; explicitly continuous sources stay direct
//! on both axes. Fractional values alone do not identify the physical device.
//! Retained motion uses constant-size state.

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
/// Per-axis bound for a normalized bucket, accelerated surplus or emitted motion step.
pub(crate) const MAX_FRAME_POINTS: f32 = 8192.0;
/// Per-axis motion input limit after combining base, surplus and Page distance.
const MAX_MOTION_INPUT_POINTS: f32 = 2.0 * MAX_FRAME_POINTS;
/// Retained motion permits a transient burst beyond one output frame.
const MAX_PENDING_POINTS: f32 = 2.0 * MAX_FRAME_POINTS;
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

/// Caps a normalized bucket or output step, per axis. A non-finite axis
/// maps to zero; the other axis is preserved.
pub(crate) fn cap_frame(delta: Vec2) -> Vec2 {
    cap_points(delta, MAX_FRAME_POINTS)
}

/// Caps finite point distances independently; invalid axes contribute no distance.
fn cap_points(delta: Vec2, limit: f32) -> Vec2 {
    Vec2::new(
        if delta.x.is_finite() { delta.x.clamp(-limit, limit) } else { 0.0 },
        if delta.y.is_finite() { delta.y.clamp(-limit, limit) } else { 0.0 },
    )
}

/// Eases the whole coarse-wheel distance, including its unaccelerated part.
///
/// The caller owns input once per egui frame, cancels old motion on direct-input
/// axes before stepping, and requests another frame while motion is pending.
/// Each axis admits up to 16384 input points and retains up to 16384 pending
/// points, emitting at most 8192 per frame. Input beyond those finite capacities
/// is dropped; isolated base-plus-surplus bursts are preserved when there is room.
#[derive(Debug, Default)]
pub(crate) struct Motion {
    pending: Vec2,
    last: Option<f64>,
}

impl Motion {
    /// Forgets owed distance and its clock when input ownership or navigation changes.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Drops older coarse motion on axes carrying direct input. This must happen
    /// before adding this frame's coarse input, so mixed input is retained.
    pub(crate) fn cancel_axes(&mut self, direct: Vec2) {
        if direct.x != 0.0 {
            self.pending.x = 0.0;
        }
        if direct.y != 0.0 {
            self.pending.y = 0.0;
        }
    }

    /// Whether another frame is needed to deliver admitted coarse movement.
    pub(crate) fn has_pending(&self) -> bool {
        self.pending != Vec2::ZERO
    }

    /// Consumes a bounded frame bucket. Reversal cancels the older direction on
    /// that axis. Disabling animation drains existing and new input immediately,
    /// except for overflow beyond the frame cap. A stale clock cancels the old
    /// tail; an invalid clock also limits fresh input to one output frame without
    /// retaining it. Pending plus fresh input cannot exceed queue and output capacity.
    pub(crate) fn step(&mut self, input: Vec2, now: f64, dt: f32, animate: bool) -> Vec2 {
        let input = cap_points(input, MAX_MOTION_INPUT_POINTS);
        let stale = !now.is_finite() || self.last.is_some_and(|last| now < last || now - last > RESET_GAP_SECS);
        if stale {
            self.pending = Vec2::ZERO;
        }
        self.last = now.is_finite().then_some(now);
        if !now.is_finite() {
            self.pending = Vec2::ZERO;
            return cap_frame(input);
        }
        // Use a valid elapsed frame interval rather than letting invalid or very
        // large intervals erase the distinction between animated and direct input.
        let dt = if dt.is_finite() && dt > 0.0 { dt.min(0.1) } else { 1.0 / 60.0 };
        let fraction = if animate { 1.0 - 0.1_f32.powf(dt / 0.1) } else { 1.0 };
        Vec2::new(Self::step_axis(&mut self.pending.x, input.x, fraction), Self::step_axis(&mut self.pending.y, input.y, fraction))
    }

    /// Delivers one axis while bounding both the returned step and remaining distance.
    fn step_axis(pending: &mut f32, input: f32, fraction: f32) -> f32 {
        if input != 0.0 && input.signum() != pending.signum() {
            *pending = 0.0;
        }
        let capacity = MAX_PENDING_POINTS + MAX_FRAME_POINTS;
        let total = (*pending + input).clamp(-capacity, capacity);
        // Emit enough overflow to keep both output and pending bounded, without
        // discarding distance after the finite queue's admission boundary.
        let magnitude = total.abs();
        let emitted = if magnitude <= 0.01 { magnitude } else { (magnitude * fraction).max(magnitude - MAX_PENDING_POINTS).min(MAX_FRAME_POINTS) };
        let output = total.signum() * emitted;
        *pending = total - output;
        output
    }
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
        let egui::Event::MouseWheel { unit, delta, modifiers, source, .. } = event else { continue };
        if modifiers.command || modifiers.ctrl {
            continue; // Zooming, not scrolling.
        }
        if !delta.is_finite() {
            continue;
        }
        // Fractional native line components use a distance-preserving routing policy for
        // high-resolution input, not evidence of a particular physical device.
        // Inspect before clamping or axis folding, which can hide fractions.
        let continuous = matches!(source, egui::MouseWheelSource::Trackpad | egui::MouseWheelSource::Momentum);
        let direct_x = continuous || delta.x.fract() != 0.0 || (modifiers.shift && delta.y.fract() != 0.0);
        let direct_y = continuous || delta.y.fract() != 0.0;
        let delta = if modifiers.shift { Vec2::new(delta.x + delta.y, 0.0) } else { *delta };
        if !delta.is_finite() {
            continue;
        }
        match unit {
            egui::MouseWheelUnit::Point => out.precise += delta,
            egui::MouseWheelUnit::Line => {
                let mut points = delta.clamp(Vec2::splat(-MAX_EVENT_UNITS), Vec2::splat(MAX_EVENT_UNITS)) * line_points;
                if single_paging {
                    points.y = 0.0;
                }
                if direct_x {
                    out.precise.x += points.x;
                } else {
                    out.line.x += points.x;
                }
                if direct_y {
                    out.precise.y += points.y;
                } else {
                    out.line.y += points.y;
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

/// Converts the frame total to the accelerated surplus.
///
/// The caller adds this to the base distance before passing the whole coarse
/// bucket to `Motion`, so an isolated notch adds no surplus.
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

/// Maps a non-finite or non-positive points-per-unit scale to zero.
fn sanitize_scale(points: f32) -> f32 {
    if points.is_finite() && points > 0.0 { points } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Easing preserves distance and terminates at each tested refresh rate.
    #[test]
    fn whole_distance_eases_and_settles_at_each_frame_rate() {
        for hz in [30, 60, 144] {
            let dt = 1.0 / hz as f32;
            let mut motion = Motion::default();
            let first = motion.step(egui::vec2(0.0, -600.0), 1.0, dt, true);
            assert!(first.y < 0.0 && first.y > -600.0);
            let mut total = first.y;
            for frame in 1..=hz * 2 {
                let delta = motion.step(Vec2::ZERO, 1.0 + f64::from(frame) / f64::from(hz), dt, true);
                assert!(delta.y <= 0.0);
                total += delta.y;
            }
            assert!((total + 600.0).abs() < 0.002, "{hz} Hz: {total}");
            assert!(!motion.has_pending());
        }
    }

    /// Timing follows elapsed seconds rather than a fixed number of frames.
    #[test]
    fn equal_elapsed_time_consumes_the_same_share_at_different_cadences() {
        let expected = 600.0 * (1.0 - 0.1_f32.powf(5.0 / 3.0));
        for hz in [30, 60, 144] {
            let mut motion = Motion::default();
            let mut total = 0.0;
            for frame in 0..hz / 6 {
                let input = if frame == 0 { egui::vec2(600.0, 0.0) } else { Vec2::ZERO };
                total += motion.step(input, 1.0 + f64::from(frame) / f64::from(hz), 1.0 / hz as f32, true).x;
            }
            assert!((total - expected).abs() < 0.01, "{hz} Hz: {total} versus {expected}");
        }
    }

    /// Reversing one axis preserves the other axis's admitted distance.
    #[test]
    fn motion_reversal_cancels_only_the_reversed_axis() {
        let mut motion = Motion::default();
        motion.step(egui::vec2(120.0, -600.0), 1.0, 0.01, true);
        let retained_x = motion.pending.x;
        let delta = motion.step(egui::vec2(0.0, 100.0), 1.01, 0.01, true);
        assert!(delta.x > 0.0 && delta.y > 0.0);
        assert!((delta.x + motion.pending.x - retained_x).abs() < 0.001);
        assert!((delta.y + motion.pending.y - 100.0).abs() < 0.001);
    }

    /// An isolated base-plus-surplus burst can settle without losing its requested distance.
    #[test]
    fn transient_burst_above_one_output_frame_preserves_distance() {
        for distance in [9600.0, 2.0 * MAX_FRAME_POINTS] {
            let mut motion = Motion::default();
            let mut total = 0.0;
            for frame in 0..120 {
                let input = if frame == 0 { egui::vec2(0.0, -distance) } else { Vec2::ZERO };
                let output = motion.step(input, 1.0 + f64::from(frame) / 60.0, 1.0 / 60.0, true);
                assert!(output.is_finite() && output.y.abs() <= MAX_FRAME_POINTS);
                total += output.y;
            }
            assert!((total + distance).abs() < 0.02, "requested {distance}, delivered {total}");
            assert!(!motion.has_pending());
        }
    }

    /// Direct input interrupts older motion, while a current mixed event still contributes.
    #[test]
    fn precise_axes_cancel_old_tail_without_dropping_current_coarse_input() {
        let mut motion = Motion::default();
        motion.step(egui::vec2(200.0, -500.0), 1.0, 0.01, true);
        let retained_x = motion.pending.x;
        motion.cancel_axes(egui::vec2(0.0, 0.25));
        let delta = motion.step(egui::vec2(0.0, -80.0), 1.01, 0.01, true);
        assert!((delta.x + motion.pending.x - retained_x).abs() < 0.001);
        assert!((delta.y + motion.pending.y + 80.0).abs() < 0.001);
    }

    /// Saturation limits per-frame work without silently losing admitted movement.
    #[test]
    fn saturated_motion_bounds_output_and_queue_without_losing_distance() {
        let mut motion = Motion::default();
        let mut admitted = 0.0;
        let mut emitted = 0.0;
        for frame in 0..80 {
            let input = if frame < 20 { MAX_FRAME_POINTS } else { 0.0 };
            admitted += input;
            let delta = motion.step(egui::vec2(input, 0.0), 1.0 + f64::from(frame) * 0.01, 0.01, true);
            assert!((0.0..=MAX_FRAME_POINTS).contains(&delta.x));
            assert!((0.0..=MAX_PENDING_POINTS).contains(&motion.pending.x));
            emitted += delta.x;
            assert!((admitted - emitted - motion.pending.x).abs() < 0.1);
        }
    }

    /// Sustained arrival above service capacity drops excess only at the finite admission boundary.
    #[test]
    fn sustained_double_frame_input_keeps_a_finite_queue_and_output() {
        let mut motion = Motion::default();
        let mut requested = 0.0;
        let mut admitted = 0.0;
        let mut emitted = 0.0;
        for frame in 0..160 {
            let input = if frame < 20 { MAX_MOTION_INPUT_POINTS } else { 0.0 };
            let previous = motion.pending.x;
            let total = (previous + input).min(MAX_PENDING_POINTS + MAX_FRAME_POINTS);
            requested += input;
            admitted += total - previous;
            let output = motion.step(egui::vec2(input, -input), 1.0 + f64::from(frame) * 0.01, 0.01, true);
            emitted += output.x;
            assert!((0.0..=MAX_FRAME_POINTS).contains(&output.x));
            assert!((0.0..=MAX_PENDING_POINTS).contains(&motion.pending.x));
            assert_eq!(output.y, -output.x);
            assert_eq!(motion.pending.y, -motion.pending.x);
            assert!((output.x + motion.pending.x - total).abs() < 0.003);
            assert!((admitted - emitted - motion.pending.x).abs() < 0.15);
        }
        assert!(requested > admitted, "a sustained overload must not create an unlimited queue");
        assert!((emitted - admitted).abs() < 0.15);
        assert!(!motion.has_pending());
    }

    /// Input admission permits two output frames, while each emitted step stays independently bounded.
    #[test]
    fn oversized_motion_input_is_limited_before_entering_the_queue() {
        for animate in [false, true] {
            let mut motion = Motion::default();
            let mut delivered = 0.0;
            for frame in 0..120 {
                let input = if frame == 0 { egui::vec2(4.0 * MAX_FRAME_POINTS, 0.0) } else { Vec2::ZERO };
                let output = motion.step(input, 1.0 + f64::from(frame) / 60.0, 1.0 / 60.0, animate);
                assert!(output.x <= MAX_FRAME_POINTS);
                assert!(motion.pending.x <= MAX_PENDING_POINTS);
                delivered += output.x;
            }
            assert!((delivered - MAX_MOTION_INPUT_POINTS).abs() < 0.02, "animations {animate}: {delivered}");
            assert!(!motion.has_pending());
        }
    }

    /// A non-finite clock neither resumes old movement nor emits an oversized fresh step.
    #[test]
    fn invalid_clock_limits_a_fresh_burst_without_retaining_it() {
        for now in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut motion = Motion::default();
            motion.step(egui::vec2(100.0, 100.0), 1.0, 0.01, true);
            let output = motion.step(egui::vec2(MAX_MOTION_INPUT_POINTS, -MAX_MOTION_INPUT_POINTS), now, 0.01, true);
            assert_eq!(output, egui::vec2(MAX_FRAME_POINTS, -MAX_FRAME_POINTS));
            assert!(!motion.has_pending());
            assert_eq!(motion.step(Vec2::ZERO, 2.0, 0.01, true), Vec2::ZERO);
        }
    }

    /// Turning animation off drains owed movement under the same output cap.
    #[test]
    fn disabling_animation_preserves_owed_distance_with_a_bounded_flush() {
        let mut motion = Motion::default();
        let first = motion.step(egui::vec2(MAX_FRAME_POINTS, 0.0), 1.0, 0.01, true);
        let second = motion.step(egui::vec2(MAX_FRAME_POINTS, 0.0), 1.01, 0.01, false);
        assert_eq!(second.x, MAX_FRAME_POINTS);
        let last = motion.step(Vec2::ZERO, 1.02, 0.01, false);
        assert!((first.x + second.x + last.x - 2.0 * MAX_FRAME_POINTS).abs() < 0.002);
        assert!(!motion.has_pending());
        assert_eq!(motion.step(egui::vec2(0.0, -100.0), 1.03, 0.01, false), egui::vec2(0.0, -100.0));
        assert!(!motion.has_pending());
    }

    /// Invalid input or a discontinuous clock cannot retain an unbounded or stale tail.
    #[test]
    fn reset_clock_gaps_and_invalid_values_cannot_reanimate_old_motion() {
        for next in [0.5, 2.0, f64::NAN, f64::INFINITY] {
            let mut motion = Motion::default();
            motion.step(egui::vec2(100.0, -200.0), 1.0, 0.01, true);
            assert_eq!(motion.step(Vec2::ZERO, next, 0.01, true), Vec2::ZERO);
            assert!(!motion.has_pending());
        }
        for dt in [f32::NAN, f32::INFINITY, -1.0, 0.0, 1e9] {
            let mut motion = Motion::default();
            let delta = motion.step(egui::vec2(100.0, f32::NAN), 1.0, dt, true);
            assert!(delta.x.is_finite() && delta.x > 0.0 && delta.x < 100.0);
            assert_eq!(delta.y, 0.0);
            motion.reset();
            assert!(!motion.has_pending());
        }
    }

    /// Normalization cannot hide fractional motion by clamping or combining its axes.
    #[test]
    fn fractional_lines_are_direct_before_clamping_or_axis_folding() {
        use egui::MouseWheelUnit::Line;
        let input = accumulate(&[wheel(Line, 0.0, -10.5), shifted(Line, 0.5, 0.5)], 100.0, false);
        assert_eq!(input.precise, egui::vec2(100.0, -1000.0));
        assert_eq!(input.line, Vec2::ZERO);
        let events: [egui::Event; 4] = core::array::from_fn(|_| wheel(Line, 0.0, -0.25));
        let input = accumulate(&events, 100.0, false);
        assert_eq!(input.precise.y, -100.0);
    }

    /// Source hints and partial-notch distances each independently select direct movement.
    #[test]
    fn known_continuous_sources_are_direct_even_for_integral_lines() {
        for source in [egui::MouseWheelSource::Trackpad, egui::MouseWheelSource::Momentum] {
            let mut event = wheel(egui::MouseWheelUnit::Line, 0.0, -1.0);
            if let egui::Event::MouseWheel { source: slot, .. } = &mut event {
                *slot = source;
            }
            let input = accumulate(&[event], 100.0, false);
            assert_eq!(input.precise, egui::vec2(0.0, -100.0));
            assert_eq!(input.line, Vec2::ZERO);
            let diagonal = egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.25, -1.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
                source,
            };
            let input = accumulate(&[diagonal], 100.0, false);
            assert_eq!(input.precise, egui::vec2(25.0, -100.0));
            assert_eq!(input.line, Vec2::ZERO);
        }
        let mut event = wheel(egui::MouseWheelUnit::Line, 0.0, -0.25);
        if let egui::Event::MouseWheel { source, .. } = &mut event {
            *source = egui::MouseWheelSource::Wheel;
        }
        assert_eq!(accumulate(&[event], 100.0, false).precise, egui::vec2(0.0, -25.0));
    }

    /// A fractional horizontal component does not change a vertical notch's distance policy.
    #[test]
    fn diagonal_fraction_keeps_the_other_axis_coarse() {
        use egui::MouseWheelUnit::Line;
        for source in [egui::MouseWheelSource::Unknown, egui::MouseWheelSource::Wheel] {
            let mut diagonal = wheel(Line, 0.25, -1.0);
            if let egui::Event::MouseWheel { source: slot, .. } = &mut diagonal {
                *slot = source;
            }
            let mut bx = WheelBoost::default();
            let mut by = WheelBoost::default();
            let mut reference_bx = WheelBoost::default();
            let mut reference_by = WheelBoost::default();
            let mut motion = Motion::default();
            let mut reference_motion = Motion::default();
            for frame in 0..90 {
                let (input, reference) = if frame < 12 {
                    let input = accumulate(std::slice::from_ref(&diagonal), 100.0, false);
                    assert_eq!(input.precise, egui::vec2(25.0, 0.0), "{source:?}: only the fractional axis is direct");
                    assert_eq!(input.line, egui::vec2(0.0, -100.0));
                    (input, accumulate(&[wheel(Line, 0.0, -1.0)], 100.0, false))
                } else {
                    (FrameInput::default(), FrameInput::default())
                };
                let now = 1.0 + f64::from(frame) / 60.0;
                let coarse = input.line + boost_extra(input, &mut bx, &mut by, now);
                let reference_coarse = reference.line + boost_extra(reference, &mut reference_bx, &mut reference_by, now);
                motion.cancel_axes(input.precise);
                reference_motion.cancel_axes(reference.precise);
                assert_eq!(coarse, reference_coarse, "{source:?}: the vertical burst remains unchanged");
                assert_eq!(motion.step(coarse, now, 1.0 / 60.0, true), reference_motion.step(reference_coarse, now, 1.0 / 60.0, true));
            }
            if let egui::Event::MouseWheel { delta, .. } = &mut diagonal {
                *delta = egui::vec2(1.0, -0.25);
            }
            let transposed = accumulate(&[diagonal], 100.0, false);
            assert_eq!(transposed.precise, egui::vec2(0.0, -25.0));
            assert_eq!(transposed.line, egui::vec2(100.0, 0.0));
        }
    }

    /// Coarse and direct events in one frame keep their respective distance policies.
    #[test]
    fn mixed_coarse_and_direct_events_retain_both_buckets() {
        use egui::MouseWheelUnit::{Line, Point};
        let input = accumulate(&[wheel(Line, 0.0, -1.0), wheel(Line, 0.0, -0.25), wheel(Point, 2.0, -3.0)], 100.0, false);
        assert_eq!(input.line, egui::vec2(0.0, -100.0));
        assert_eq!(input.precise, egui::vec2(2.0, -28.0));
        let input = accumulate(&[wheel(Line, 2.0, -0.25)], 100.0, true);
        assert_eq!(input.precise, Vec2::ZERO);
        assert_eq!(input.line, egui::vec2(200.0, 0.0));
    }

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
}
