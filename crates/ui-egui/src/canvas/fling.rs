//! Touchpad momentum (#531): a two-finger swipe keeps gliding after the fingers lift, and slows
//! to a stop.
//!
//! Wayland compositors leave momentum to the client. libinput reports the swipe as pixel deltas
//! (`MouseWheelUnit::Point`) and the lift as an axis stop, which winit passes on as
//! `TouchPhase::End`; GTK and Firefox glide on from there, and so does the page view.
//!
//! macOS sends its own momentum as a second `Start`…`End`, so gliding there would glide twice,
//! and the web has no touch phases: [`Fling::touchpad`] is off on both. Windows and X11 report no
//! touch phases through winit either, so no `End` ever starts a glide there.
//!
//! A finger on a touch screen dragging with the Hand tool glides too, on every platform: flicked
//! and lifted, the page keeps moving the way a phone's does ([`Fling::drag`], [`Fling::release`]).
//! Nothing in the system glides a drag, so nothing glides it twice, and [`Fling::touchpad`] is not
//! asked. A drag a mouse started still stops where it is let go, and a pinch doesn't glide. Both
//! kinds of glide share the page view's gate (`may_glide` in `canvas.rs`: Animation effects,
//! single-page paging, a dialog), which reaches `read` and `release` as their `glide`.

use egui::{Event, MouseWheelUnit, TouchPhase, Vec2};

/// Seconds of the gesture the lift speed is measured over.
const WINDOW: f64 = 0.1;
/// A lift this long after the last motion is a finger that stopped first: no glide.
const STILL: f64 = 0.05;
/// The glide slows by `e` every this many seconds; it travels the lift speed times this.
const TAU: f32 = 0.35;
/// [`TAU`] for a Hand tool drag: longer, so a flicked page coasts on like a phone's.
const DRAG_TAU: f32 = 0.6;
/// Below this speed (points per second) a lift doesn't glide, and a glide stops.
const START_SPEED: f32 = 150.0;
const STOP_SPEED: f32 = 10.0;
/// Fastest glide, in points per second: a flick the system measured over a hiccup can't throw
/// the document to its end.
const MAX_SPEED: f32 = 8000.0;
/// A pass this long after the last one (the view was hidden, the app stalled) ends a glide.
const STALE: f64 = 0.1;

pub(crate) struct Fling {
    /// Whether a touchpad swipe's lift glides on this platform (see the module docs). A Hand tool
    /// drag doesn't ask: no platform glides one itself.
    touchpad: bool,
    /// The gesture in progress: when each pass's motion arrived, and how far it went.
    samples: Vec<(f64, Vec2)>,
    /// The glide's speed in points per second (positive moves the content down), or zero.
    velocity: Vec2,
    /// When the glide last moved.
    at: f64,
    /// Passes in a row the glide moved the view not at all: it is against an edge.
    stalled: u8,
    /// How quickly the glide in progress slows ([`TAU`] or [`DRAG_TAU`]).
    tau: f32,
    /// A Hand tool drag is being sampled ([`Self::drag`]).
    dragging: bool,
    /// A finger on a touch screen started the drag: only then does it glide when let go.
    touched: bool,
    /// A second finger joined the drag: it was a pinch, which doesn't glide when let go.
    pinched: bool,
}

impl Default for Fling {
    fn default() -> Self {
        Self::new(!cfg!(any(target_os = "macos", target_arch = "wasm32")))
    }
}

impl Fling {
    pub(crate) fn new(touchpad: bool) -> Self {
        Self { touchpad, samples: Vec::new(), velocity: Vec2::ZERO, at: 0.0, stalled: 0, tau: TAU, dragging: false, touched: false, pinched: false }
    }

    /// Whether a Hand tool drag is being sampled.
    pub(crate) fn dragging(&self) -> bool {
        self.dragging
    }

    /// One pass of the Hand tool dragging the page, at `now`: the pointer moved `delta` (positive
    /// moves the content down), with a finger on a touch screen when `touch`, and two or more when
    /// `pinching`. The page view calls it only for that drag. Holding the page stops any glide.
    /// Only a drag a finger started glides when let go (`touch` on its first pass: a finger
    /// touching the screen later doesn't make a mouse drag one), and not one that was ever a
    /// pinch, as a phone's doesn't.
    pub(crate) fn drag(&mut self, now: f64, delta: Vec2, touch: bool, pinching: bool) {
        if !self.dragging {
            self.dragging = true;
            self.touched = touch;
            self.pinched = false;
            self.samples.clear();
        }
        self.pinched |= pinching;
        self.stop();
        if delta != Vec2::ZERO {
            self.push(now, delta);
        }
    }

    /// The Hand tool drag ended at `now`, the pass the finger lifted in: a touch drag glides on at
    /// the speed it was let go at, unless `glide` is false (animations off, paging, a dialog, or
    /// a drag that ended without being let go).
    pub(crate) fn release(&mut self, now: f64, glide: bool) {
        if !std::mem::take(&mut self.dragging) {
            return;
        }
        if glide && self.touched && !self.pinched {
            self.velocity = self.lift_velocity(now);
            self.at = now;
            self.tau = DRAG_TAU;
        }
        self.samples.clear();
    }

    pub(crate) fn gliding(&self) -> bool {
        self.velocity != Vec2::ZERO
    }

    pub(crate) fn stop(&mut self) {
        self.velocity = Vec2::ZERO;
        self.stalled = 0;
    }

    /// Reads one pass's wheel events, at time `now`. Any wheel input stops a glide; a touchpad
    /// gesture's `End` may start one. `glide` is false where the view must not glide this pass
    /// (animations off, single-page paging, a zoom, a dialog).
    pub(crate) fn read(&mut self, events: &[Event], now: f64, glide: bool) {
        for event in events {
            let Event::MouseWheel { unit, delta, phase, modifiers, .. } = event else { continue };
            self.stop();
            // Zooming, a mouse wheel and sideways Shift scrolling never glide.
            if *unit != MouseWheelUnit::Point || modifiers.command || modifiers.ctrl || modifiers.shift {
                self.samples.clear();
                continue;
            }
            match phase {
                TouchPhase::Start => {
                    self.samples.clear();
                    self.push(now, *delta);
                }
                TouchPhase::Move => self.push(now, *delta),
                TouchPhase::End => {
                    if glide && self.touchpad {
                        self.velocity = self.lift_velocity(now);
                        self.at = now;
                        self.tau = TAU;
                    }
                    self.samples.clear();
                }
                TouchPhase::Cancel => self.samples.clear(),
            }
        }
    }

    fn push(&mut self, now: f64, delta: Vec2) {
        if !(delta.x.is_finite() && delta.y.is_finite()) {
            return;
        }
        match self.samples.last_mut() {
            // Several events in one pass are one sample: they share its time.
            Some((at, d)) if *at == now => *d += delta,
            _ => self.samples.push((now, delta)),
        }
        self.samples.retain(|(at, _)| now - at <= WINDOW);
    }

    /// The finger's speed as it lifted at `now`, or zero when it should not glide.
    fn lift_velocity(&self, now: f64) -> Vec2 {
        let recent: Vec<&(f64, Vec2)> = self.samples.iter().filter(|(at, _)| now - at <= WINDOW).collect();
        let (Some(first), Some(last)) = (recent.first(), recent.last()) else { return Vec2::ZERO };
        let span = (last.0 - first.0) as f32;
        if recent.len() < 2 || span <= 0.0 || now - last.0 > STILL {
            return Vec2::ZERO;
        }
        // Each sample is the motion since the pass before it, so the first one's motion
        // happened before the span starts.
        let moved = recent.iter().skip(1).fold(Vec2::ZERO, |sum, (_, d)| sum + *d);
        let v = moved / span;
        if !(v.x.is_finite() && v.y.is_finite()) || v.length() < START_SPEED {
            return Vec2::ZERO;
        }
        if v.length() > MAX_SPEED { v * (MAX_SPEED / v.length()) } else { v }
    }

    /// How far the glide moves the view in the pass at `now` (zero when not gliding), as the
    /// exact distance a speed decaying with [`TAU`] covers since the last pass.
    pub(crate) fn step(&mut self, now: f64) -> Vec2 {
        if !self.gliding() {
            return Vec2::ZERO;
        }
        let dt = now - self.at;
        if !(0.0..=STALE).contains(&dt) {
            self.stop();
            return Vec2::ZERO;
        }
        self.at = now;
        let decay = (-(dt as f32) / self.tau).exp();
        let moved = self.velocity * self.tau * (1.0 - decay);
        self.velocity *= decay;
        if self.velocity.length() < STOP_SPEED {
            self.stop();
        }
        moved
    }

    /// After a pass the glide moved the view in: whether the offset changed. Two passes that
    /// didn't move it mean an edge, and the glide ends there.
    pub(crate) fn moved(&mut self, moved: bool) {
        if moved {
            self.stalled = 0;
        } else {
            self.stalled = self.stalled.saturating_add(1);
            if self.stalled >= 2 {
                self.stop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Modifiers, vec2};

    fn wheel(dy: f32, phase: TouchPhase) -> Event {
        Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, dy),
            phase,
            modifiers: Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        }
    }

    /// A swipe of `moves` passes of `dy` each, 60 per second, lifted the pass after the last.
    fn swipe(fling: &mut Fling, dy: f32, moves: usize) -> f64 {
        let dt = 1.0 / 60.0;
        let mut now = 0.0;
        fling.read(&[wheel(dy, TouchPhase::Start)], now, true);
        for _ in 0..moves {
            now += dt;
            fling.read(&[wheel(dy, TouchPhase::Move)], now, true);
        }
        now += dt;
        fling.read(&[wheel(0.0, TouchPhase::End)], now, true);
        now
    }

    #[test]
    fn a_flick_glides_on_at_its_lift_speed_and_stops() {
        let mut fling = Fling::new(true);
        let mut now = swipe(&mut fling, -25.0, 6);
        assert!(fling.gliding());
        // 25 points a pass at 60 passes a second.
        assert!((fling.velocity.y + 1500.0).abs() < 1.0, "{:?}", fling.velocity);
        let mut travelled = 0.0;
        let mut passes = 0;
        while fling.gliding() {
            now += 1.0 / 60.0;
            let d = fling.step(now);
            assert!(d.y <= 0.0 && d.x == 0.0, "keeps the swipe's direction: {d:?}");
            travelled += d.y;
            passes += 1;
            assert!(passes < 600, "the glide ends");
        }
        // Speed × TAU, less the tail under STOP_SPEED.
        assert!((-travelled - 1500.0 * TAU).abs() < 10.0, "travelled {travelled}");
        assert_eq!(fling.step(now + 1.0 / 60.0), Vec2::ZERO);
    }

    const DT: f64 = 1.0 / 60.0;

    /// Who is dragging the page in a [`hand_drag`].
    #[derive(Clone, Copy)]
    enum By {
        Finger,
        Mouse,
        /// The mouse, with a finger touching the screen from the drag's third pass on.
        MouseThenFinger,
        /// A finger, with a second one down in the drag's third pass.
        Pinch,
    }

    /// A Hand tool drag of six passes, 30 points up each, 60 per second, from pass `first`.
    /// Returns when the last pass was; the lift is a pass later.
    fn hand_drag(fling: &mut Fling, first: usize, by: By) -> f64 {
        let mut now = 0.0;
        for (k, i) in (first..first + 6).enumerate() {
            now = i as f64 * DT;
            let touch = match by {
                By::Finger | By::Pinch => true,
                By::Mouse => false,
                By::MouseThenFinger => k >= 2,
            };
            fling.drag(now, vec2(0.0, -30.0), touch, matches!(by, By::Pinch) && k == 2);
        }
        now
    }

    /// A [`hand_drag`] from pass 0, lifted a pass after its last, with the page view's gate open
    /// when `glide`.
    fn flick(fling: &mut Fling, by: By, glide: bool) -> f64 {
        let lift = hand_drag(fling, 0, by) + DT;
        fling.release(lift, glide);
        lift
    }

    #[test]
    fn a_flicked_finger_glides_on_at_its_lift_speed() {
        // Where touchpad swipes don't glide too: a touch screen drag isn't the system's to glide.
        let mut fling = Fling::new(false);
        hand_drag(&mut fling, 0, By::Finger);
        assert!(fling.dragging());
        fling.release(6.0 * DT, true);
        assert!(!fling.dragging());
        assert!(fling.gliding());
        // 30 points a pass at 60 passes a second.
        assert!((fling.velocity.y + 1800.0).abs() < 1.0, "{:?}", fling.velocity);
    }

    #[test]
    fn a_finger_glide_slows_with_the_drag_decay_and_stops() {
        let mut fling = Fling::new(true);
        let mut now = flick(&mut fling, By::Finger, true);
        let v = fling.velocity.y;
        now += DT;
        let first = fling.step(now);
        let expected = v * DRAG_TAU * (1.0 - (-(DT as f32) / DRAG_TAU).exp());
        assert!((first.y - expected).abs() < 0.01 && first.x == 0.0, "first pass {first:?}, expected {expected}");
        let mut travelled = first.y;
        let mut passes = 0;
        while fling.gliding() {
            now += DT;
            travelled += fling.step(now).y;
            passes += 1;
            assert!(passes < 600, "the glide ends");
        }
        // Speed × DRAG_TAU (1080 points; the touchpad's TAU would give 630), less the tail under
        // STOP_SPEED.
        assert!((-travelled - 1800.0 * DRAG_TAU).abs() < 10.0, "travelled {travelled}");
    }

    #[test]
    fn a_finger_back_on_the_page_holds_a_glide() {
        let mut fling = Fling::new(true);
        let lift = flick(&mut fling, By::Finger, true);
        fling.step(lift + DT);
        assert!(fling.gliding());
        fling.drag(lift + 2.0 * DT, Vec2::ZERO, true, false);
        assert!(!fling.gliding());
        assert_eq!(fling.step(lift + 3.0 * DT), Vec2::ZERO);
    }

    #[test]
    fn a_mouse_drag_stops_where_it_is_let_go() {
        let mut fling = Fling::new(true);
        flick(&mut fling, By::Mouse, true);
        assert!(!fling.gliding());
    }

    #[test]
    fn a_finger_touching_during_a_mouse_drag_does_not_make_it_glide() {
        let mut fling = Fling::new(true);
        flick(&mut fling, By::MouseThenFinger, true);
        assert!(!fling.gliding(), "the mouse started the drag");
    }

    #[test]
    fn a_finger_resting_before_it_lifts_does_not_glide() {
        let mut fling = Fling::new(true);
        hand_drag(&mut fling, 0, By::Finger);
        fling.release(1.0, true);
        assert!(!fling.gliding());
    }

    #[test]
    fn a_pinch_does_not_glide() {
        let mut fling = Fling::new(true);
        flick(&mut fling, By::Pinch, true);
        assert!(!fling.gliding(), "a pinch lifted one finger at a time");
    }

    #[test]
    fn a_closed_gate_does_not_glide() {
        let mut fling = Fling::new(true);
        flick(&mut fling, By::Finger, false);
        assert!(!fling.gliding(), "animations off, paging, a dialog, or a drag not let go");
    }

    #[test]
    fn each_drag_is_judged_by_itself() {
        // A finger after a pinch glides again.
        let mut fling = Fling::new(true);
        flick(&mut fling, By::Pinch, true);
        let lift = hand_drag(&mut fling, 10, By::Finger) + DT;
        fling.release(lift, true);
        assert!(fling.gliding(), "a finger after a pinch");

        // A mouse drag after a finger's glide catches it and doesn't glide itself.
        let mut fling = Fling::new(true);
        flick(&mut fling, By::Finger, true);
        assert!(fling.gliding());
        let lift = hand_drag(&mut fling, 10, By::Mouse) + DT;
        assert!(!fling.gliding(), "the mouse holds the page");
        fling.release(lift, true);
        assert!(!fling.gliding(), "a mouse drag after a finger's glide");
    }

    #[test]
    fn a_slow_drag_or_a_pause_before_lifting_does_not_glide() {
        let mut fling = Fling::new(true);
        swipe(&mut fling, -1.0, 6);
        assert!(!fling.gliding(), "60 points a second is below the start speed");

        let mut fling = Fling::new(true);
        let dt = 1.0 / 60.0;
        fling.read(&[wheel(-25.0, TouchPhase::Start)], 0.0, true);
        fling.read(&[wheel(-25.0, TouchPhase::Move)], dt, true);
        fling.read(&[wheel(0.0, TouchPhase::End)], dt + 0.2, true);
        assert!(!fling.gliding(), "the finger rested before it lifted");
    }

    /// Check that new finger or wheel input stops an in-progress glide.
    #[test]
    fn new_input_stops_a_glide() {
        let mut fling = Fling::new(true);
        let now = swipe(&mut fling, -25.0, 6);
        fling.read(&[wheel(-2.0, TouchPhase::Start)], now + 0.1, true);
        assert!(!fling.gliding(), "fingers back on the touchpad");

        let mut fling = Fling::new(true);
        let now = swipe(&mut fling, -25.0, 6);
        let notch = Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: vec2(0.0, -1.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        };
        fling.read(&[notch], now + 0.1, true);
        assert!(!fling.gliding(), "a wheel notch");
    }

    /// Check that disabled flinging, a false glide flag, and a command-modified zoom gesture prevent glide.
    #[test]
    fn no_glide_where_it_is_off_or_not_wanted() {
        let mut off = Fling::new(false);
        swipe(&mut off, -25.0, 6);
        assert!(!off.gliding(), "macOS and the web glide by themselves or not at all");

        let mut fling = Fling::new(true);
        let dt = 1.0 / 60.0;
        fling.read(&[wheel(-25.0, TouchPhase::Start)], 0.0, true);
        fling.read(&[wheel(-25.0, TouchPhase::Move)], dt, true);
        fling.read(&[wheel(-25.0, TouchPhase::Move)], 2.0 * dt, true);
        fling.read(&[wheel(0.0, TouchPhase::End)], 3.0 * dt, false);
        assert!(!fling.gliding(), "animations off, paging or a dialog");

        let mut zoom = Fling::new(true);
        let ctrl = |phase| Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, -25.0),
            phase,
            modifiers: Modifiers::COMMAND,
            source: egui::MouseWheelSource::Unknown,
        };
        zoom.read(&[ctrl(TouchPhase::Start)], 0.0, true);
        zoom.read(&[ctrl(TouchPhase::Move)], dt, true);
        zoom.read(&[ctrl(TouchPhase::Move)], 2.0 * dt, true);
        zoom.read(&[ctrl(TouchPhase::End)], 3.0 * dt, true);
        assert!(!zoom.gliding(), "a pinch or ⌘/Ctrl swipe zooms");
    }

    #[test]
    fn an_edge_a_stall_or_a_wild_speed_ends_or_caps_the_glide() {
        let mut fling = Fling::new(true);
        let now = swipe(&mut fling, -25.0, 6);
        fling.step(now + 1.0 / 60.0);
        fling.moved(false);
        assert!(fling.gliding(), "one pass may not show the move yet");
        fling.moved(false);
        assert!(!fling.gliding(), "against the edge");

        let mut fling = Fling::new(true);
        let now = swipe(&mut fling, -25.0, 6);
        assert_eq!(fling.step(now + 1.0), Vec2::ZERO, "a second-long gap (the tab was hidden)");
        assert!(!fling.gliding());

        let mut fling = Fling::new(true);
        swipe(&mut fling, -5000.0, 6);
        assert!((fling.velocity.length() - MAX_SPEED).abs() < 1.0, "{:?}", fling.velocity);

        let mut fling = Fling::new(true);
        swipe(&mut fling, f32::NAN, 6);
        assert!(!fling.gliding(), "a speed that isn't a number");
    }
}
