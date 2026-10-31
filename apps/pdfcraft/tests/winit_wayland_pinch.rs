#![cfg(target_os = "linux")]

use egui::{Event, RawInput};

mod event {
    pub use winit::event::TouchPhase;
}
#[path = "../../../vendor/winit/src/platform_impl/linux/wayland/seat/pointer/pinch_state.rs"]
mod pinch_state;

use event::TouchPhase;
use pinch_state::PinchState;

#[test]
fn cumulative_wayland_scales_reach_egui_as_exact_multiplicative_zoom() {
    let ctx = egui::Context::default();
    let mut pinch = PinchState::default();
    let begin = pinch.begin(Some(7), 2).unwrap();
    assert_eq!(begin.phase, TouchPhase::Started);
    assert_eq!(begin.delta, 0.0);
    let mut total = 1.0_f32;
    for (frame, scale) in [1.25, 1.5, 2.0, 0.75, 1.0].into_iter().enumerate() {
        let update = pinch.update(scale).unwrap();
        assert_eq!(update.window, 7);
        assert_eq!(update.phase, TouchPhase::Moved);
        // egui-winit translates PinchGesture with exp(delta), not 1 + delta.
        let factor = (update.delta as f32).exp();
        let raw = RawInput { time: Some(frame as f64 / 60.0), events: vec![Event::Zoom(factor)], ..Default::default() };
        ctx.run_ui(raw, |ui| total *= ui.input(|i| i.zoom_delta())).textures_delta.clear();
        assert!((f64::from(total) - scale).abs() < 1e-6, "scale {scale} became {total}");
        assert_eq!(ctx.zoom_factor(), 1.0, "pinch must not scale the app interface");
    }
}

#[test]
fn end_and_cancellation_stop_updates_and_reset_the_next_gesture() {
    for cancelled in [false, true] {
        let mut pinch = PinchState::default();
        pinch.begin(Some(3), 2).unwrap();
        pinch.update(2.0).unwrap();
        let end = pinch.end(cancelled).unwrap();
        assert_eq!(end.phase, if cancelled { TouchPhase::Cancelled } else { TouchPhase::Ended });
        assert_eq!(end.delta, 0.0);
        assert!(pinch.update(3.0).is_none());
        assert!(pinch.end(cancelled).is_none());
        pinch.begin(Some(4), 2).unwrap();
        let update = pinch.update(1.5).unwrap();
        assert_eq!(update.window, 4);
        assert!((update.delta.exp() - 1.5).abs() < 1e-12);
    }
}

#[test]
fn malformed_scales_are_ignored_without_poisoning_the_gesture() {
    let mut pinch = PinchState::default();
    pinch.begin(Some(1), 2).unwrap();
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, 1e-300] {
        assert!(pinch.update(scale).is_none(), "{scale}");
    }
    assert!((pinch.update(2.0).unwrap().delta.exp() - 2.0).abs() < 1e-12);
}

#[test]
fn unsupported_fingers_and_surfaces_clear_previous_gesture_state() {
    let mut pinch = PinchState::default();
    assert!(pinch.update(1.5).is_none());
    assert!(pinch.end(false).is_none());
    for (window, fingers) in [(Some(2), 1), (Some(2), 3), (None, 2)] {
        pinch.begin(Some(1), 2).unwrap();
        assert!(pinch.begin(window, fingers).is_none());
        assert!(pinch.update(1.5).is_none());
        assert!(pinch.end(false).is_none());
    }
}
