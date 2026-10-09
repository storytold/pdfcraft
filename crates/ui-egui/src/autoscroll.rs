//! Transient button-drag pan for Windows, and Linux's existing auto-scroll.

use egui::{Context, CursorIcon, Event, Key, PointerButton, Pos2, Stroke, Vec2, vec2};

const DEAD_ZONE: f32 = 15.0;
// Chromium's autoscroll_controller.cc uses distance^2.2 * 0.000008; its
// ui/events/gestures/fixed_velocity_curve.cc multiplies elapsed seconds by 5000.
const SPEED_EXPONENT: f32 = 2.2;
const SPEED_MULTIPLIER: f32 = 0.04;
// Bound hostile coordinates before exponentiation, far beyond ordinary screen distances.
const MAX_DISPLACEMENT: f32 = 1_000_000.0;

/// Button capture only; offsets, clipping and bounds belong to the canvas ScrollArea.
#[derive(Default)]
pub(crate) struct DragPan {
    last: Option<Pos2>,
    button: Option<PointerButton>,
    // Releasing Space ends movement, but the held click must never reach editing tools.
    await_primary_release: bool,
    block_input: bool,
}

impl DragPan {
    pub(crate) fn active(&self) -> bool {
        self.last.is_some()
    }

    pub(crate) fn cancel(&mut self) {
        self.last = None;
        self.button = None;
        self.await_primary_release = false;
        self.block_input = false;
    }

    pub(crate) fn blocks_input(&self) -> bool {
        self.block_input
    }

    pub(crate) fn middle_active(&self) -> bool {
        self.active() && self.button == Some(PointerButton::Middle)
    }

    pub(crate) fn update(&mut self, ui: &egui::Ui, viewport: egui::Rect, space: bool) -> Vec2 {
        let ctx = ui.ctx();
        let enabled = ui.is_enabled() && ctx.input(|i| i.focused);
        let events = ctx.input(|i| i.events.clone());
        let viewport = viewport.intersect(ui.clip_rect());
        self.advance_with_space(&events, enabled, space, |pos| viewport.contains(pos) && ctx.layer_id_at(pos) == Some(ui.layer_id()))
    }

    #[cfg(test)]
    fn advance(&mut self, events: &[Event], enabled: bool, can_start: impl Fn(Pos2) -> bool) -> Vec2 {
        self.advance_with_space(events, enabled, false, can_start)
    }

    fn advance_with_space(&mut self, events: &[Event], enabled: bool, space: bool, can_start: impl Fn(Pos2) -> bool) -> Vec2 {
        self.block_input = self.active() || self.await_primary_release;
        if !enabled {
            self.cancel();
            return Vec2::ZERO;
        }
        if !space && self.button == Some(PointerButton::Primary) {
            self.last = None;
            self.button = None;
            self.await_primary_release = true;
        }
        let mut delta = Vec2::ZERO;
        for event in events {
            match event {
                Event::PointerButton { pos, button, pressed: true, .. } => {
                    let supported = *button == PointerButton::Middle || (space && *button == PointerButton::Primary);
                    if supported && !self.active() && !self.await_primary_release && pos.is_finite() && can_start(*pos) {
                        self.last = Some(*pos);
                        self.button = Some(*button);
                        self.block_input = true;
                    }
                }
                Event::PointerMoved(pos) => {
                    if let Some(last) = self.last {
                        if pos.is_finite() {
                            delta += *pos - last;
                            self.last = Some(*pos);
                        } else {
                            self.last = None;
                        }
                    }
                }
                Event::PointerButton { button: PointerButton::Primary, pressed: false, .. } if self.await_primary_release => {
                    self.await_primary_release = false;
                    self.block_input = true;
                }
                Event::PointerButton { pos, button, pressed: false, .. } if self.button == Some(*button) => {
                    if let Some(last) = self.last.take() {
                        if pos.is_finite() {
                            delta += *pos - last;
                        }
                        // Own the release too: egui's generic drag responses accept any button.
                        self.block_input = true;
                    }
                    self.button = None;
                }
                Event::Key { key: Key::Space, pressed: false, .. } if self.button == Some(PointerButton::Primary) => {
                    self.last = None;
                    self.button = None;
                    self.await_primary_release = true;
                }
                Event::PointerGone | Event::WindowFocused(false) | Event::Key { key: Key::Escape, pressed: true, .. } => {
                    self.last = None;
                    self.button = None;
                    self.await_primary_release = false;
                }
                _ => {}
            }
        }
        if delta.is_finite() { delta } else { Vec2::ZERO }
    }
}

#[derive(Default)]
pub(crate) struct AutoScroll {
    anchor: Option<Pos2>,
    organize: bool,
    /// Own a cancelling click through its release, so it cannot also edit page content.
    cancel_button: Option<PointerButton>,
    block_input: bool,
}

impl AutoScroll {
    pub(crate) fn active(&self) -> bool {
        self.anchor.is_some()
    }

    pub(crate) fn cancel(&mut self) {
        self.anchor = None;
        self.cancel_button = None;
        self.block_input = false;
    }

    pub(crate) fn blocks_input(&self) -> bool {
        self.block_input
    }

    /// Run before the document's widgets. Starting is restricted to the unobstructed viewport;
    /// once started, moving outside that viewport still controls the speed.
    pub(crate) fn update(&mut self, ui: &egui::Ui, viewport: egui::Rect, organize: bool) -> Vec2 {
        self.update_for_platform(ui, viewport, organize, cfg!(target_os = "linux"))
    }

    fn update_for_platform(&mut self, ui: &egui::Ui, viewport: egui::Rect, organize: bool, supported: bool) -> Vec2 {
        // Leave middle-button events and widget interactions to the existing platform behavior.
        if !supported {
            self.cancel();
            return Vec2::ZERO;
        }
        let ctx = ui.ctx();
        let (pointer, middle_press, middle_down, middle_released, cancel, cancel_button, dt) = ctx.input(|i| {
            let cancel_button = [PointerButton::Primary, PointerButton::Secondary, PointerButton::Extra1, PointerButton::Extra2]
                .into_iter()
                .find(|button| i.pointer.button_pressed(*button));
            (
                i.pointer.hover_pos(),
                // Read the press event itself: later movement in this frame must not move the anchor.
                i.events.iter().find_map(|event| match event {
                    Event::PointerButton { pos, button: PointerButton::Middle, pressed: true, .. } if pos.is_finite() => Some(*pos),
                    _ => None,
                }),
                i.pointer.button_down(PointerButton::Middle),
                i.pointer.button_released(PointerButton::Middle),
                !i.focused
                    || i.key_pressed(Key::Escape)
                    || cancel_button.is_some()
                    || i.events.iter().any(|e| matches!(e, Event::MouseWheel { .. } | Event::Zoom(_))),
                cancel_button,
                i.stable_dt,
            )
        });
        self.block_input = self.active() || self.cancel_button.is_some() || middle_press.is_some() || middle_down || middle_released;
        if let Some(button) = self.cancel_button {
            if !ctx.input(|i| i.pointer.button_down(button)) {
                self.cancel_button = None;
            }
            return Vec2::ZERO;
        }
        if self.active() && (cancel || pointer.is_none() || self.organize != organize || ctx.egui_wants_keyboard_input()) {
            self.cancel();
            self.block_input = true;
            self.cancel_button = cancel_button;
            return Vec2::ZERO;
        }
        if let Some(pressed_at) = middle_press {
            if self.active() {
                self.cancel();
                self.block_input = true;
                self.cancel_button = Some(PointerButton::Middle);
                return Vec2::ZERO;
            } else if !cancel
                && !ctx.egui_wants_keyboard_input()
                && viewport.intersect(ui.clip_rect()).contains(pressed_at)
                && ctx.layer_id_at(pressed_at) == Some(ui.layer_id())
            {
                self.anchor = Some(pressed_at);
                self.organize = organize;
            }
        }
        let (Some(anchor), Some(pointer)) = (self.anchor, pointer) else { return Vec2::ZERO };
        let displacement = pointer.y - anchor.y;
        // Cap the elapsed time too: returning from an idle/hidden window must never jump pages.
        let delta = scroll_delta(displacement, dt);
        if displacement.is_finite() && displacement.abs() > DEAD_ZONE {
            // Continuous redraws let stable_dt use measured frame time. Delayed redraws
            // instead use predicted_dt, which can make speed depend on the actual frame rate.
            ctx.request_repaint();
        }
        vec2(0.0, delta)
    }

    /// Draw an original geometric marker at the activation point, above page content.
    pub(crate) fn paint(&self, ui: &egui::Ui, viewport: egui::Rect) {
        let Some(anchor) = self.anchor else { return };
        let painter = ui.painter().with_clip_rect(viewport);
        let ink = ui.visuals().text_color();
        painter.circle(anchor, 13.0, ui.visuals().window_fill(), Stroke::new(1.0, ink));
        painter.circle_filled(anchor, 2.0, ink);
        for direction in [-1.0, 1.0] {
            painter.add(egui::Shape::convex_polygon(
                vec![anchor + vec2(-4.0, direction * 6.0), anchor + vec2(4.0, direction * 6.0), anchor + vec2(0.0, direction * 10.0)],
                ink,
                Stroke::NONE,
            ));
        }
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }

    /// Escape belongs to autoscroll first, leaving selection/find/full-screen intact.
    pub(crate) fn escape(&mut self, ctx: &Context) -> bool {
        if self.active() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            self.cancel();
            true
        } else {
            false
        }
    }
}

fn scroll_delta(displacement: f32, dt: f32) -> f32 {
    if !displacement.is_finite() || !dt.is_finite() {
        return 0.0;
    }
    let distance = displacement.abs();
    if distance <= DEAD_ZONE {
        return 0.0;
    }
    // Chromium uses the full distance outside the dead zone, without subtracting its radius.
    let speed = distance.min(MAX_DISPLACEMENT).powf(SPEED_EXPONENT) * SPEED_MULTIPLIER;
    // egui's delta moves content, the opposite of the scroll offset.
    -displacement.signum() * speed * dt.clamp(0.0, 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn middle(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Middle, pressed, modifiers: egui::Modifiers::NONE }
    }

    fn primary(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn space_drag_uses_shared_motion_and_owns_the_click_until_release() {
        let mut pan = DragPan::default();
        let p = egui::pos2(100.0, 100.0);
        pan.advance_with_space(&[primary(p, true)], true, true, |_| true);
        assert!(pan.active());
        assert!(!pan.middle_active());
        assert_eq!(pan.advance_with_space(&[Event::PointerMoved(p + vec2(-30.0, 25.0))], true, true, |_| false), vec2(-30.0, 25.0));
        assert_eq!(pan.advance_with_space(&[Event::PointerMoved(p)], true, false, |_| true), Vec2::ZERO);
        assert!(!pan.active());
        assert!(pan.blocks_input());
        pan.advance_with_space(&[], true, false, |_| true);
        assert!(pan.blocks_input(), "a held left click cannot become an editing drag after Space is released");
        pan.advance_with_space(&[primary(p, false)], true, false, |_| true);
        assert!(pan.blocks_input());
        pan.advance_with_space(&[], true, false, |_| true);
        assert!(!pan.blocks_input());
    }

    #[test]
    fn space_drag_requires_a_fresh_inside_press_and_stops_on_mouse_release() {
        let mut pan = DragPan::default();
        let p = egui::pos2(100.0, 100.0);
        for (space, inside) in [(false, true), (true, false)] {
            pan.advance_with_space(&[primary(p, true)], true, space, |_| inside);
            pan.advance_with_space(&[Event::PointerMoved(p)], true, true, |_| true);
            assert!(!pan.active());
        }
        pan.advance_with_space(&[primary(p, true)], true, true, |_| true);
        let end = p + vec2(25.0, -10.0);
        assert_eq!(pan.advance_with_space(&[Event::PointerMoved(end), primary(end, false), Event::PointerMoved(p)], true, true, |_| true), end - p);
        assert!(!pan.active());
        for event in [Event::PointerGone, Event::WindowFocused(false)] {
            pan.advance_with_space(&[primary(p, true)], true, true, |_| true);
            pan.advance_with_space(&[event], true, true, |_| true);
            assert!(!pan.active());
        }
    }

    #[test]
    fn drag_pan_tracks_both_axes_one_to_one_and_stops_on_release() {
        let mut pan = DragPan::default();
        let p = egui::pos2(100.0, 100.0);
        assert_eq!(pan.advance(&[middle(p, true)], true, |_| true), Vec2::ZERO);
        for movement in [vec2(40.0, 0.0), vec2(0.0, -30.0), vec2(-25.0, 20.0)] {
            let next = pan.last.unwrap() + movement;
            assert_eq!(pan.advance(&[Event::PointerMoved(next)], true, |_| false), movement);
            assert!(pan.active());
        }
        let end = pan.last.unwrap();
        assert_eq!(pan.advance(&[middle(end, false), Event::PointerMoved(end + vec2(50.0, 50.0))], true, |_| true), Vec2::ZERO);
        assert!(!pan.active());
        assert!(pan.blocks_input(), "the release must not activate editing tools");
        assert_eq!(pan.advance(&[], true, |_| true), Vec2::ZERO);
        assert!(!pan.blocks_input());
    }

    #[test]
    fn drag_pan_handles_press_move_release_in_one_frame_without_latching() {
        let mut pan = DragPan::default();
        let p = egui::pos2(100.0, 100.0);
        let end = p + vec2(-30.0, -50.0);
        assert_eq!(pan.advance(&[middle(p, true), Event::PointerMoved(end), middle(end, false)], true, |_| true), end - p);
        assert!(!pan.active());
        assert!(pan.blocks_input());
        pan.advance(&[middle(p, true), middle(p, false)], true, |_| true);
        assert!(!pan.active(), "clicking alone must never auto-scroll");
    }

    #[test]
    fn drag_pan_cannot_start_outside_viewport_or_resume_after_cancellation() {
        let mut pan = DragPan::default();
        let p = egui::pos2(100.0, 100.0);
        pan.advance(&[middle(p, true)], true, |_| false);
        assert_eq!(pan.advance(&[Event::PointerMoved(p + vec2(10.0, 20.0))], true, |_| true), Vec2::ZERO);
        assert!(!pan.active(), "entering the viewport while held must not start a pan");
        for event in [Event::PointerGone, Event::WindowFocused(false)] {
            pan.advance(&[middle(p, true)], true, |_| true);
            pan.advance(&[event], true, |_| true);
            assert!(!pan.active());
        }
        pan.advance(&[middle(p, true)], true, |_| true);
        pan.advance(&[], false, |_| true);
        assert!(!pan.active());
        assert_eq!(pan.advance(&[Event::PointerMoved(p)], true, |_| true), Vec2::ZERO);
    }

    #[test]
    fn unsupported_platform_leaves_middle_button_and_escape_input_untouched() {
        for previously_active in [false, true] {
            let ctx = Context::default();
            let pos = egui::pos2(100.0, 100.0);
            let mut scroll = AutoScroll { anchor: previously_active.then_some(pos), block_input: previously_active, ..Default::default() };
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(400.0, 400.0))),
                    events: vec![
                        Event::PointerMoved(pos),
                        Event::PointerButton { pos, button: PointerButton::Middle, pressed: true, modifiers: egui::Modifiers::NONE },
                        Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE },
                    ],
                    ..Default::default()
                },
                |ui| {
                    assert_eq!(scroll.update_for_platform(ui, ui.max_rect(), false, false), Vec2::ZERO);
                    assert!(!scroll.active(), "unsupported platforms must not start or retain custom scrolling");
                    assert!(!scroll.blocks_input(), "middle-button input must reach the existing widgets");
                    scroll.paint(ui, ui.max_rect());
                    assert_eq!(ctx.output(|o| o.cursor_icon), CursorIcon::Default, "no custom marker or cursor");
                    assert!(!scroll.escape(&ctx));
                    ctx.input(|i| {
                        assert!(i.pointer.button_pressed(PointerButton::Middle));
                        assert!(i.key_pressed(Key::Escape), "Escape must remain available to the existing shortcuts");
                    });
                },
            );
            // This input-only test has no renderer to apply the generated font texture.
            output.textures_delta.clear();
        }
    }

    #[test]
    fn chromium_curve_is_gentle_near_the_anchor_and_accelerates_farther_away() {
        // Reference speeds in screen points/second from Chromium's distance exponent (2.2),
        // controller multiplier (0.000008), and fixed-velocity animation multiplier (5000).
        for (distance, expected_speed) in [(25.0, 48.0), (50.0, 219.0), (100.0, 1005.0), (200.0, 4617.0)] {
            let speed = -scroll_delta(distance, 0.01) / 0.01;
            assert!((speed - expected_speed).abs() < 1.0, "distance={distance}, speed={speed}, expected={expected_speed}");
        }
        for distance in [-15.0, 0.0, 15.0] {
            assert_eq!(scroll_delta(distance, 0.01), 0.0);
        }
        assert!(scroll_delta(15.1, 0.01) < 0.0);
    }

    #[test]
    fn fractional_motion_covers_the_same_distance_at_different_frame_rates() {
        for distance in [16.0, 50.0, 200.0] {
            let expected = scroll_delta(distance, 0.01) * 100.0;
            for frames in [30, 60, 120, 144] {
                let delta = scroll_delta(distance, 1.0 / frames as f32);
                let travelled: f32 = (0..frames).map(|_| delta).sum();
                assert!((travelled - expected).abs() < expected.abs() * 0.00001);
            }
        }
        assert!(scroll_delta(16.0, 1.0 / 144.0).abs() < 1.0);
    }

    #[test]
    fn speed_has_a_dead_zone_is_symmetric_and_rejects_invalid_input() {
        for y in [-15.0, -1.0, 0.0, 1.0, 15.0] {
            assert_eq!(scroll_delta(y, 0.016), 0.0);
        }
        assert!(scroll_delta(30.0, 0.016) < 0.0);
        for (near, far) in [(16.0, 30.0), (30.0, 100.0), (100.0, 200.0)] {
            assert!(scroll_delta(far, 0.016).abs() > scroll_delta(near, 0.016).abs());
        }
        assert_eq!(scroll_delta(30.0, 0.016), -scroll_delta(-30.0, 0.016));
        assert!(scroll_delta(200.0, 0.01).abs() / 0.01 > 3200.0, "ordinary distances have no linear-curve speed ceiling");
        assert_eq!(scroll_delta(1000.0, 10.0), scroll_delta(1000.0, 0.05));
        assert_eq!(scroll_delta(1000.0, -1.0), 0.0);
        assert_eq!(scroll_delta(f32::MAX, 0.016), scroll_delta(MAX_DISPLACEMENT, 0.016));
        assert!(scroll_delta(f32::MAX, 0.016).is_finite());
        assert_eq!(scroll_delta(-f32::MAX, 0.016), -scroll_delta(f32::MAX, 0.016));
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(scroll_delta(invalid, 0.016), 0.0);
            assert_eq!(scroll_delta(1000.0, invalid), 0.0);
        }
    }
}
