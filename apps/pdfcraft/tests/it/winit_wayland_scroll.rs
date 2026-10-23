#![cfg(target_os = "linux")]

use egui::{Event, Modifiers, MouseWheelUnit, RawInput, TouchPhase, vec2};
use wayland_client::protocol::wl_pointer::AxisSource;

use crate::event;
#[path = "../../../../vendor/winit/src/platform_impl/linux/wayland/seat/pointer/scroll.rs"]
mod scroll;

fn egui_phase(phase: event::TouchPhase) -> TouchPhase {
    match phase {
        event::TouchPhase::Started => TouchPhase::Start,
        event::TouchPhase::Moved => TouchPhase::Move,
        event::TouchPhase::Ended => TouchPhase::End,
        event::TouchPhase::Cancelled => TouchPhase::Cancel,
    }
}

fn wheel_trace(source: AxisSource, discrete: bool) -> Vec<(f32, egui::Vec2)> {
    let ctx = egui::Context::default();
    let mut previous = event::TouchPhase::Ended;
    let mut seen = Vec::new();
    let ctrl = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    for (frame, modifiers) in [ctrl, ctrl, Modifiers::NONE].into_iter().enumerate() {
        let phase = scroll::phase(previous, Some(source), discrete, false);
        previous = phase;
        let (unit, dy) = if discrete { (MouseWheelUnit::Line, 1.0) } else { (MouseWheelUnit::Point, 24.0) };
        let input = RawInput {
            time: Some(frame as f64 / 60.0),
            events: vec![Event::MouseWheel { unit, delta: vec2(0.0, dy), phase: egui_phase(phase), modifiers }],
            ..Default::default()
        };
        ctx.run_ui(input, |ui| seen.push(ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta)))).textures_delta.clear();
    }
    seen
}

#[test]
fn releasing_ctrl_after_pixel_wheel_zoom_returns_to_scrolling() {
    for source in [AxisSource::Wheel, AxisSource::WheelTilt, AxisSource::Continuous] {
        let seen = wheel_trace(source, false);
        assert!(seen[1].0 > 1.0, "Ctrl must zoom: {source:?}");
        assert_eq!(seen[2].0, 1.0, "releasing Ctrl must stop zooming: {source:?}");
        assert!(seen[2].1.y > 0.0, "the next wheel event must scroll: {source:?}");
    }
}

#[test]
fn discrete_wheels_still_follow_current_modifiers() {
    let seen = wheel_trace(AxisSource::Wheel, true);
    assert!(seen[1].0 > 1.0);
    assert_eq!(seen[2].0, 1.0);
    assert!(seen[2].1.y > 0.0);
}

#[test]
fn finger_gestures_keep_their_modifier_until_the_gesture_ends() {
    // A finger scroll's momentum may arrive after Ctrl is released.
    let seen = wheel_trace(AxisSource::Finger, false);
    assert!(seen[1].0 > 1.0);
    assert!(seen[2].0 > 1.0);
    assert_eq!(seen[2].1, egui::Vec2::ZERO);
    assert_eq!(scroll::phase(event::TouchPhase::Moved, Some(AxisSource::Finger), false, true), event::TouchPhase::Ended);
    assert_eq!(scroll::phase(event::TouchPhase::Ended, Some(AxisSource::Finger), false, false), event::TouchPhase::Started);
}

#[test]
fn unknown_sources_keep_the_existing_phase_policy() {
    assert_eq!(scroll::phase(event::TouchPhase::Ended, None, false, false), event::TouchPhase::Started);
    assert_eq!(scroll::phase(event::TouchPhase::Started, None, false, false), event::TouchPhase::Moved);
    assert_eq!(scroll::phase(event::TouchPhase::Ended, None, true, false), event::TouchPhase::Moved);
    assert_eq!(scroll::phase(event::TouchPhase::Moved, None, false, true), event::TouchPhase::Ended);
}
