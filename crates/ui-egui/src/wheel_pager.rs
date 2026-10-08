//! Wheel paging in single-page view: when the whole page fits, the mouse wheel and the
//! trackpad turn pages instead of doing nothing.
//!
//! A wheel notch turns one page, however fast the notches come. That needs the raw
//! `MouseWheel` events: egui's smoothed scroll delta spreads one notch over several frames, so
//! notches in quick succession blur together. Smooth input (trackpads, high-resolution wheels)
//! turns one page per gesture and direction once it has moved as far as a notch scrolls. A
//! gesture is a touch from [`TouchPhase::Start`] to `End`, together with the momentum phase
//! the OS sends after it (macOS and Wayland report touch phases), or, without phases, a burst
//! of input with no pause longer than `BURST_GAP_SECS`.

use egui::{MouseWheelUnit, TouchPhase};

/// How far one wheel notch scrolls, in points (egui's native `line_scroll_speed`). Smooth
/// input turns a page once it has moved this far, so a light swipe turns and resting fingers
/// don't; smooth line deltas count this many points per line.
const NOTCH_POINTS: f32 = 40.0;
/// A phase-less point delta at least this big is a wheel notch: egui draws the same line
/// between wheel steps, which it smooths, and trackpad input, which it passes through.
const MIN_NOTCH_POINTS: f32 = 8.0;
/// Phase-less smooth input belongs to one gesture until it pauses this long, in seconds (egui
/// uses the same gap to decide that such a scroll has ended).
const BURST_GAP_SECS: f64 = 0.15;
/// A big point delta this soon (seconds) after another wheel event belongs to a fast trackpad
/// stream, not to a separate notch.
const DENSE_GAP_SECS: f64 = 0.02;
/// The OS starts its momentum phase this soon (seconds) after the finger lifts, so a touch
/// starting sooner continues the swipe rather than starting another.
const MOMENTUM_GAP_SECS: f64 = 0.1;
/// A touch with no events for this long (seconds) has ended unseen: its `End` went elsewhere.
const TOUCH_STALE_SECS: f64 = 1.0;

/// The touch in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Touch {
    Finger,
    /// The momentum phase after a finger touch: still the same swipe.
    Momentum,
}

/// Turns wheel and trackpad input into page turns; see the module docs.
pub(crate) struct WheelPager {
    /// Smooth motion toward the next turn, in points (negative scrolls forward).
    accum: f32,
    /// The way the current gesture already turned (`true` = forward).
    turned: Option<bool>,
    /// The touch in progress, on platforms that report touch phases.
    touch: Option<Touch>,
    /// When the last finger touch ended, so its momentum phase can be recognised.
    finger_up: Option<f64>,
    /// egui time of the last wheel event.
    last_event: f64,
    /// egui time of the last smooth (non-notch) motion.
    last_smooth: f64,
}

impl Default for WheelPager {
    fn default() -> Self {
        // Infinitely long ago, so the first event never looks like part of a stream.
        Self { accum: 0.0, turned: None, touch: None, finger_up: None, last_event: f64::NEG_INFINITY, last_smooth: f64::NEG_INFINITY }
    }
}

impl WheelPager {
    /// Drop partial motion toward a turn (the view moved elsewhere). The gesture goes on: a
    /// swipe that already turned doesn't turn again after a jump.
    pub(crate) fn clear_motion(&mut self) {
        self.accum = 0.0;
    }

    /// Feed one `Event::MouseWheel`: `dy` is its vertical delta (negative scrolls forward) and
    /// `now` is egui time. With `can_turn` false (the page doesn't fit, a dialog is open…) the
    /// gesture is followed but never turns. Returns the way to turn (`true` = forward).
    pub(crate) fn feed(&mut self, unit: MouseWheelUnit, dy: f32, phase: TouchPhase, now: f64, can_turn: bool) -> Option<bool> {
        if !now.is_finite() {
            return None;
        }
        let idle = now - self.last_event;
        self.last_event = now;
        if idle > TOUCH_STALE_SECS {
            self.touch = None;
        }
        match phase {
            TouchPhase::Start => {
                let momentum = self.finger_up.take().is_some_and(|up| now - up <= MOMENTUM_GAP_SECS);
                self.touch = Some(if momentum { Touch::Momentum } else { Touch::Finger });
                if !momentum {
                    self.new_gesture();
                }
                None
            }
            TouchPhase::End | TouchPhase::Cancel => {
                // Only a finger touch is followed by momentum.
                self.finger_up = (self.touch == Some(Touch::Finger)).then_some(now);
                self.touch = None;
                None
            }
            TouchPhase::Move => self.motion(unit, dy, now, idle, can_turn),
        }
    }

    fn new_gesture(&mut self) {
        self.accum = 0.0;
        self.turned = None;
    }

    fn motion(&mut self, unit: MouseWheelUnit, dy: f32, now: f64, idle: f64, can_turn: bool) -> Option<bool> {
        if !dy.is_finite() || dy == 0.0 {
            return None;
        }
        let forward = dy < 0.0;
        let smooth_stream = self.touch.is_some() || now - self.last_smooth <= BURST_GAP_SECS;
        let notch = !smooth_stream
            && match unit {
                // macOS reports precise devices in points, so its line deltas are wheel notches,
                // however small its acceleration makes them. Elsewhere a fraction of a line is
                // touchpad motion.
                MouseWheelUnit::Line => cfg!(target_os = "macos") || dy.abs() >= 1.0,
                MouseWheelUnit::Page => true,
                MouseWheelUnit::Point => dy.abs() >= MIN_NOTCH_POINTS && idle >= DENSE_GAP_SECS,
            };
        if notch {
            if !can_turn {
                return None;
            }
            // Every notch turns a page; a smooth tail of the same gesture won't add another.
            self.accum = 0.0;
            self.turned = Some(forward);
            return Some(forward);
        }
        if self.touch.is_none() && idle > BURST_GAP_SECS {
            self.new_gesture();
        }
        self.last_smooth = now;
        if !can_turn {
            // Panning a page that doesn't fit: that motion must not count toward a turn later.
            self.accum = 0.0;
            return None;
        }
        if self.turned == Some(forward) {
            return None;
        }
        let points = if unit == MouseWheelUnit::Point { dy } else { dy * NOTCH_POINTS };
        // One event never counts for more than a turn, which also bounds hostile deltas. Net
        // motion counts, so trackpad jitter cancels out.
        self.accum += points.clamp(-NOTCH_POINTS, NOTCH_POINTS);
        if self.accum.abs() < NOTCH_POINTS {
            return None;
        }
        self.accum = 0.0;
        self.turned = Some(forward);
        Some(forward)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use MouseWheelUnit::{Line, Point};
    use TouchPhase::{End, Move, Start};

    /// Feeds `(unit, dy, phase, seconds)` events and returns the turns, in order.
    fn turns(pager: &mut WheelPager, events: &[(MouseWheelUnit, f32, TouchPhase, f64)]) -> Vec<bool> {
        events.iter().filter_map(|&(unit, dy, phase, at)| pager.feed(unit, dy, phase, at, true)).collect()
    }

    /// A trackpad swipe at 60 fps from `t`: six finger frames of `dy`, then momentum decaying
    /// to nothing, as macOS reports it (each phase its own Start…End).
    fn swipe(t: f64, dy: f32) -> Vec<(MouseWheelUnit, f32, TouchPhase, f64)> {
        let frame = 1.0 / 60.0;
        let mut events = vec![(Point, 0.0, Start, t)];
        let mut at = t;
        for _ in 0..6 {
            at += frame;
            events.push((Point, dy, Move, at));
        }
        events.extend([(Point, 0.0, End, at), (Point, 0.0, Start, at)]);
        let mut d = dy;
        while d.abs() > 0.5 {
            at += frame;
            events.push((Point, d, Move, at));
            d *= 0.92;
        }
        events.push((Point, 0.0, End, at));
        events
    }

    #[test]
    fn every_notch_turns_one_page_however_fast() {
        let mut p = WheelPager::default();
        // A brisk spin: five notches 50 ms apart, then one back.
        let mut events: Vec<_> = (0..5).map(|i| (Line, -1.0, Move, 1.0 + f64::from(i) * 0.05)).collect();
        events.push((Line, 1.0, Move, 1.5));
        assert_eq!(turns(&mut p, &events), [true, true, true, true, true, false]);
        // Pixel-unit wheels (a Logitech mouse on a Mac reports ~14 points a notch).
        assert_eq!(turns(&mut p, &[(Point, -14.0, Move, 3.0), (Point, -14.0, Move, 3.1)]), [true, true]);
    }

    #[test]
    fn a_swipe_turns_one_page_momentum_included() {
        let mut p = WheelPager::default();
        assert_eq!(turns(&mut p, &swipe(1.0, -25.0)), [true]);
        // A slow, steady scroll is one gesture too.
        let mut slow = vec![(Point, 0.0, Start, 3.0)];
        slow.extend((1..=60).map(|i| (Point, -3.0, Move, 3.0 + f64::from(i) / 60.0)));
        slow.push((Point, 0.0, End, 4.0));
        assert_eq!(turns(&mut p, &slow), [true]);
        // Resting fingers that drift a little don't turn.
        let nudge: Vec<_> = [(Point, 0.0, Start, 6.0)].into_iter().chain((1..=5).map(|i| (Point, -2.0, Move, 6.0 + f64::from(i) / 60.0))).collect();
        assert!(turns(&mut p, &nudge).is_empty());
    }

    #[test]
    fn each_new_swipe_turns_again_even_during_momentum() {
        let mut p = WheelPager::default();
        let mut events = swipe(1.0, -25.0);
        // Touching the trackpad again ends the momentum at once (End) and starts a new finger
        // touch: the first swipe's momentum is over, so the second swipe turns.
        let cut = events.iter().position(|e| e.3 > 1.3).unwrap();
        events.truncate(cut);
        let t = events.last().unwrap().3;
        events.push((Point, 0.0, End, t));
        events.extend(swipe(t, -25.0));
        assert_eq!(turns(&mut p, &events), [true, true]);
    }

    #[test]
    fn reversing_mid_gesture_turns_back_but_jitter_does_not() {
        let mut p = WheelPager::default();
        let mut events = vec![(Point, 0.0, Start, 1.0), (Point, -45.0, Move, 1.02)];
        // Jitter right after the turn neither turns back nor re-arms the forward turn.
        events.extend([(Point, 2.0, Move, 1.04), (Point, -20.0, Move, 1.06), (Point, -20.0, Move, 1.08)]);
        // A deliberate push the other way turns back, once.
        events.extend([(Point, 25.0, Move, 1.10), (Point, 25.0, Move, 1.12), (Point, 25.0, Move, 1.14)]);
        assert_eq!(turns(&mut p, &events), [true, false]);
    }

    #[test]
    fn phase_less_streams_turn_once_per_burst() {
        // Web and some touchpads send no touch phases: small dense deltas, possibly big ones
        // in a fast flick, all one gesture until the input pauses.
        let mut p = WheelPager::default();
        let mut burst: Vec<_> = (0..30).map(|i| (Point, -5.0, Move, 1.0 + f64::from(i) * 0.008)).collect();
        burst.extend((0..10).map(|i| (Point, -30.0, Move, 1.3 + f64::from(i) * 0.008)));
        assert_eq!(turns(&mut p, &burst), [true]);
        // A burst that starts big is one gesture too.
        let fast: Vec<_> = (0..20).map(|i| (Point, -30.0, Move, 3.0 + f64::from(i) * 0.008)).collect();
        assert_eq!(turns(&mut p, &fast), [true]);
        // After a pause, the next burst turns again.
        let next: Vec<_> = (0..20).map(|i| (Point, -5.0, Move, 4.0 + f64::from(i) * 0.008)).collect();
        assert_eq!(turns(&mut p, &next), [true]);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn fractional_lines_are_touchpad_motion_off_macos() {
        // Windows and X11 touchpads report many small line deltas.
        let mut p = WheelPager::default();
        let stream: Vec<_> = (0..40).map(|i| (Line, -0.1, Move, 1.0 + f64::from(i) * 0.008)).collect();
        assert_eq!(turns(&mut p, &stream), [true]);
    }

    #[test]
    fn nothing_turns_while_it_cannot_and_that_motion_does_not_count_later() {
        let mut p = WheelPager::default();
        assert_eq!(p.feed(Line, -1.0, Move, 1.0, false), None);
        assert_eq!(p.feed(Point, 0.0, Start, 2.0, false), None);
        assert_eq!(p.feed(Point, -30.0, Move, 2.02, false), None);
        // The page fits now: the same touch has to move a full notch's worth again.
        assert_eq!(p.feed(Point, -30.0, Move, 2.04, true), None);
        assert_eq!(p.feed(Point, -30.0, Move, 2.06, true), Some(true));
    }

    #[test]
    fn a_touch_whose_end_was_missed_expires() {
        let mut p = WheelPager::default();
        assert!(turns(&mut p, &[(Point, 0.0, Start, 1.0)]).is_empty());
        // Seconds later a wheel notch is a notch again, not part of that touch.
        assert_eq!(turns(&mut p, &[(Line, -1.0, Move, 3.0)]), [true]);
    }

    #[test]
    fn hostile_input_never_turns_more_than_once() {
        let mut p = WheelPager::default();
        for dy in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -0.0] {
            assert_eq!(p.feed(Point, dy, Move, 1.0, true), None, "dy={dy}");
        }
        for now in [f64::NAN, f64::INFINITY] {
            assert_eq!(p.feed(Point, -50.0, Move, now, true), None, "now={now}");
        }
        // A huge notch is still one notch; a huge smooth delta is still one turn.
        assert_eq!(turns(&mut p, &[(Point, -1e30, Move, 2.0)]), [true]);
        assert_eq!(turns(&mut p, &[(Point, 0.0, Start, 3.0), (Point, -1e30, Move, 3.01), (Point, -1e30, Move, 3.02)]), [true]);
    }
}
