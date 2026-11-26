//! Pointer input in the frames the platform delivers it in.
//!
//! `Harness::step` runs one frame for every queued event, each 0.25 s after the last, so input
//! sent through `Harness::event` (or `drag_at` and `drop_at`) never shares a frame. Real input
//! does: egui-winit turns each touch (and on Windows each pen stroke, which winit reports as
//! touch) into a `Touch` event plus pointer events in one frame, and sends `PointerGone` with
//! every release, so `hover_pos()` is `None` on the frame egui reports the tap. A slow frame
//! can also hold a whole click. These helpers push each frame's events into
//! `Harness::input_mut` and step once.
use egui::{Event, Modifiers, PointerButton, Pos2, TouchDeviceId, TouchId, TouchPhase};
use egui_kittest::Harness;

/// Runs one frame with all of `events` in it, in order.
pub fn one_frame<S>(h: &mut Harness<'_, S>, events: impl IntoIterator<Item = Event>) {
    h.input_mut().events.extend(events);
    h.step();
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

fn touch(phase: TouchPhase, pos: Pos2) -> Event {
    Event::Touch { device_id: TouchDeviceId(1), id: TouchId(1), phase, pos, force: None }
}

/// A finger tap at `p`, or a pen tap on Windows: the touch starts (touch, move, press in one
/// frame), and a frame later it ends (touch, release, `PointerGone` in one frame).
pub fn touch_tap<S>(h: &mut Harness<'_, S>, p: Pos2) {
    one_frame(h, [touch(TouchPhase::Start, p), Event::PointerMoved(p), button(p, true)]);
    h.run_steps(1);
    one_frame(h, [touch(TouchPhase::End, p), button(p, false), Event::PointerGone]);
    h.run_steps(4);
}

/// A double-click at `p`: two clicks, each a press and release in one frame, in consecutive
/// frames (0.25 s apart, within egui's 0.3 s double-click delay).
pub fn double_click<S>(h: &mut Harness<'_, S>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    for _ in 0..2 {
        one_frame(h, [button(p, true), button(p, false)]);
    }
    h.run_steps(2);
}

/// Drops `files` on the window as the platform does: a frame with them held over the window, then
/// the frame of the drop, which no longer holds them (egui-winit clears the hovered files on a
/// drop). A drop with no hover before it starts no drag session, so nothing would take it.
pub fn drop_files<S>(h: &mut Harness<'_, S>, files: impl IntoIterator<Item = egui::DroppedFileHandle>) {
    let files: Vec<egui::DroppedFileHandle> = files.into_iter().collect();
    let held = files.iter().map(|file| {
        let path = file.path();
        egui::HoveredFile { path: (!path.as_os_str().is_empty()).then(|| path.to_path_buf()), mime: String::new() }
    });
    h.input_mut().hovered_files = held.collect();
    h.step();
    h.input_mut().hovered_files.clear();
    h.input_mut().dropped_files.extend(files);
    h.step();
}
