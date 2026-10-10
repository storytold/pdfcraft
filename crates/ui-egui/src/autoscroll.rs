//! Middle-button scrolling for the document viewport: Linux latches auto-scroll, other
//! platforms pan while the button is held. Either way the page tools never see the button,
//! because egui's drag responses accept every pointer button.

use egui::{Context, CursorIcon, Event, Key, PointerButton, Pos2, Stroke, Vec2, vec2};

const DEAD_ZONE: f32 = 15.0;
// Chromium's autoscroll_controller.cc uses distance^2.2 * 0.000008; its
// ui/events/gestures/fixed_velocity_curve.cc multiplies elapsed seconds by 5000.
const SPEED_EXPONENT: f32 = 2.2;
const SPEED_MULTIPLIER: f32 = 0.04;
// Bound hostile coordinates before exponentiation, far beyond ordinary screen distances.
const MAX_DISPLACEMENT: f32 = 1_000_000.0;

#[derive(Default)]
pub(crate) struct AutoScroll {
    anchor: Option<Pos2>,
    organize: bool,
    /// Own a cancelling click through its release, so it cannot also edit page content.
    cancel_button: Option<PointerButton>,
    block_input: bool,
    /// Where the pointer was last frame during a held middle-button pan (not Linux).
    drag: Option<Pos2>,
}

impl AutoScroll {
    pub(crate) fn active(&self) -> bool {
        self.anchor.is_some()
    }

    /// Whether a held middle-button pan is under way (platforms without auto-scroll).
    pub(crate) fn panning(&self) -> bool {
        self.drag.is_some()
    }

    pub(crate) fn cancel(&mut self) {
        self.anchor = None;
        self.drag = None;
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
        if !supported {
            return self.update_drag(ui, viewport);
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

    /// Without auto-scroll, a middle-button drag pans the view 1:1, like the Hand tool. Input is
    /// blocked from the press through the release, so the drag cannot also draw, select or move.
    fn update_drag(&mut self, ui: &egui::Ui, viewport: egui::Rect) -> Vec2 {
        self.anchor = None;
        self.cancel_button = None;
        let ctx = ui.ctx();
        let (pointer, middle_press, middle_down, middle_released, focused) = ctx.input(|i| {
            (
                i.pointer.latest_pos().filter(|p| p.is_finite()),
                // Read the press event itself: later movement in this frame is part of the pan.
                i.events.iter().find_map(|event| match event {
                    Event::PointerButton { pos, button: PointerButton::Middle, pressed: true, .. } if pos.is_finite() => Some(*pos),
                    _ => None,
                }),
                i.pointer.button_down(PointerButton::Middle),
                i.pointer.button_released(PointerButton::Middle),
                i.focused,
            )
        });
        self.block_input = self.drag.is_some() || middle_press.is_some() || middle_down || middle_released;
        if !focused {
            self.drag = None;
            return Vec2::ZERO;
        }
        if let Some(pressed_at) = middle_press
            && self.drag.is_none()
            && !ctx.egui_wants_keyboard_input()
            && viewport.intersect(ui.clip_rect()).contains(pressed_at)
            && ctx.layer_id_at(pressed_at) == Some(ui.layer_id())
        {
            self.drag = Some(pressed_at);
        }
        let (Some(last), Some(pointer)) = (self.drag, pointer) else { return Vec2::ZERO };
        self.drag = middle_down.then_some(pointer);
        // egui's delta moves the content, so the page follows the pointer.
        let delta = pointer - last;
        if delta.is_finite() { delta } else { Vec2::ZERO }
    }

    /// Draw an original geometric marker at the activation point, above page content.
    pub(crate) fn paint(&self, ui: &egui::Ui, viewport: egui::Rect) {
        if self.drag.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            return;
        }
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

    /// Run one frame of the non-Linux path; `check` sees the pan delta inside the frame.
    fn drag_frame(ctx: &Context, scroll: &mut AutoScroll, events: Vec<Event>, focused: bool, check: impl Fn(&AutoScroll, Vec2)) {
        let mut output = ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(400.0, 400.0))), events, focused, ..Default::default() },
            |ui| {
                let delta = scroll.update_for_platform(ui, ui.max_rect(), false, false);
                check(scroll, delta);
            },
        );
        // This input-only test has no renderer to apply the generated font texture.
        output.textures_delta.clear();
    }

    #[test]
    fn middle_drag_pans_one_to_one_and_owns_the_button_until_release() {
        let ctx = Context::default();
        let mut scroll = AutoScroll::default();
        let p = egui::pos2(100.0, 100.0);
        // The first frame lays out the layer that later presses are tested against.
        drag_frame(&ctx, &mut scroll, vec![], true, |_, _| {});
        drag_frame(&ctx, &mut scroll, vec![Event::PointerMoved(p), middle(p, true)], true, |s, d| {
            assert_eq!(d, Vec2::ZERO);
            assert!(s.panning() && s.blocks_input(), "the press itself must not reach the page tools");
            assert!(!s.active(), "no auto-scroll latch outside Linux");
        });
        drag_frame(&ctx, &mut scroll, vec![Event::PointerMoved(p + vec2(-30.0, 40.0))], true, |s, d| {
            assert_eq!(d, vec2(-30.0, 40.0), "the page follows the pointer on both axes");
            assert!(s.blocks_input());
        });
        drag_frame(&ctx, &mut scroll, vec![Event::PointerMoved(p + vec2(-30.0, 50.0)), middle(p + vec2(-30.0, 50.0), false)], true, |s, d| {
            assert_eq!(d, vec2(0.0, 10.0), "movement in the release frame still counts");
            assert!(!s.panning());
            assert!(s.blocks_input(), "the release must not finish a drawing drag");
        });
        drag_frame(&ctx, &mut scroll, vec![Event::PointerMoved(p)], true, |s, d| {
            assert_eq!(d, Vec2::ZERO, "nothing moves once the button is up");
            assert!(!s.blocks_input());
        });
    }

    #[test]
    fn middle_drag_ignores_presses_outside_the_viewport_and_stops_when_focus_is_lost() {
        let ctx = Context::default();
        let mut scroll = AutoScroll::default();
        let outside = egui::pos2(500.0, 100.0);
        drag_frame(&ctx, &mut scroll, vec![], true, |_, _| {});
        drag_frame(&ctx, &mut scroll, vec![Event::PointerMoved(outside), middle(outside, true)], true, |s, _| assert!(!s.panning()));
        drag_frame(&ctx, &mut scroll, vec![Event::PointerMoved(outside + vec2(0.0, 50.0))], true, |_, d| assert_eq!(d, Vec2::ZERO));
        drag_frame(&ctx, &mut scroll, vec![middle(outside, false)], true, |_, _| {});

        let p = egui::pos2(100.0, 100.0);
        drag_frame(&ctx, &mut scroll, vec![Event::PointerMoved(p), middle(p, true)], true, |s, _| assert!(s.panning()));
        drag_frame(&ctx, &mut scroll, vec![Event::PointerMoved(p + vec2(0.0, 50.0))], false, |s, d| {
            assert_eq!(d, Vec2::ZERO);
            assert!(!s.panning(), "losing focus ends the pan");
        });
    }

    #[test]
    fn middle_drag_leaves_escape_to_the_existing_shortcuts() {
        let ctx = Context::default();
        let mut scroll = AutoScroll::default();
        let p = egui::pos2(100.0, 100.0);
        drag_frame(&ctx, &mut scroll, vec![], true, |_, _| {});
        let escape = Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        drag_frame(&ctx, &mut scroll, vec![Event::PointerMoved(p), middle(p, true), escape], true, |s, _| {
            assert!(s.panning());
            assert!(ctx.input(|i| i.key_pressed(Key::Escape)), "Escape must remain available to the existing shortcuts");
        });
        assert!(!scroll.escape(&ctx));
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
