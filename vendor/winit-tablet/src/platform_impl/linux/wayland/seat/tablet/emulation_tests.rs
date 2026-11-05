//! Unit tests for the tablet translation machine (`emulation.rs`).
//!
//! Kept beside the module it tests; the application's regression test
//! (`apps/pdfcraft/tests/it/winit_wayland_tablet.rs`) includes `emulation.rs`
//! itself, so this file only runs in the winit crate's own test suite and holds
//! to this crate's rustfmt rules, which the vendored tree keeps separate from
//! the application's.

use super::*;

fn press(code: u32) -> TabletToolSignal {
    TabletToolSignal::Button { button: code, state: 1 }
}
fn lift(code: u32) -> TabletToolSignal {
    TabletToolSignal::Button { button: code, state: 0 }
}
fn pos(x: f64, y: f64) -> LogicalPosition<f64> {
    LogicalPosition::new(x, y)
}

#[test]
fn hover_then_draw_maps_to_mouse() {
    let mut emu = TabletToolEmu::default();
    assert_eq!(
        emu.observe(TabletToolSignal::Enter),
        vec![EmulatedPointerEvent::Entered],
        "first focus can know no position yet"
    );
    assert_eq!(emu.observe(TabletToolSignal::Motion(pos(10.0, 20.0))), vec![
        EmulatedPointerEvent::Moved(pos(10.0, 20.0))
    ]);
    assert_eq!(emu.observe(TabletToolSignal::Contact), vec![EmulatedPointerEvent::Press(
        MouseButton::Left
    )]);
    assert_eq!(emu.observe(TabletToolSignal::Motion(pos(11.0, 22.0))), vec![
        EmulatedPointerEvent::Moved(pos(11.0, 22.0))
    ]);
    assert_eq!(emu.observe(TabletToolSignal::Lift), vec![EmulatedPointerEvent::Release(
        MouseButton::Left
    )]);
    assert_eq!(emu.observe(TabletToolSignal::Leave), vec![EmulatedPointerEvent::Left]);
}

#[test]
fn stationary_tap_presses_with_first_motion() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Enter);
    assert_eq!(emu.observe(TabletToolSignal::Contact), vec![], "cannot know a press position yet");
    assert_eq!(
        emu.observe(TabletToolSignal::Motion(pos(5.0, 5.0))),
        vec![
            EmulatedPointerEvent::Moved(pos(5.0, 5.0)),
            EmulatedPointerEvent::Press(MouseButton::Left),
        ],
        "deferred press arrives after the motion"
    );
    assert_eq!(emu.observe(TabletToolSignal::Lift), vec![EmulatedPointerEvent::Release(
        MouseButton::Left
    )]);
}

#[test]
fn lift_before_any_motion_does_not_press() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Enter);
    emu.observe(TabletToolSignal::Contact);
    assert_eq!(emu.observe(TabletToolSignal::Lift), vec![]);
    assert_eq!(emu.position, None, "no position was ever known");
    // And the tool state stays coherent for the next contact.
    assert_eq!(emu.observe(TabletToolSignal::Motion(pos(1.0, 1.0))), vec![
        EmulatedPointerEvent::Moved(pos(1.0, 1.0))
    ]);
    assert_eq!(emu.observe(TabletToolSignal::Contact), vec![EmulatedPointerEvent::Press(
        MouseButton::Left
    )]);
}

#[test]
fn focus_switch_mid_proximity_is_not_a_second_entry() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Enter);
    assert_eq!(emu.observe(TabletToolSignal::Enter), vec![]);
    assert_eq!(emu.observe(TabletToolSignal::Leave), vec![EmulatedPointerEvent::Left]);
    // In proximity the glue keeps the focused surface until `Leave`; a new
    // `Enter` re-enters with the remembered position (nothing was drawn).
    assert_eq!(emu.observe(TabletToolSignal::Enter), vec![EmulatedPointerEvent::Entered]);
}

/// The remembered position keeps a refocusing tap landable: the press goes
/// out at the last seen spot, exactly like a mouse click.
#[test]
fn remembered_position_presses_without_wait() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Enter);
    emu.observe(TabletToolSignal::Motion(pos(50.0, 60.0)));
    emu.observe(TabletToolSignal::Leave);
    assert_eq!(emu.observe(TabletToolSignal::Enter), vec![
        EmulatedPointerEvent::Entered,
        EmulatedPointerEvent::Moved(pos(50.0, 60.0)),
    ]);
    assert_eq!(emu.observe(TabletToolSignal::Contact), vec![EmulatedPointerEvent::Press(
        MouseButton::Left
    )]);
}

#[test]
fn contact_before_focus_presses_on_enter() {
    let mut emu = TabletToolEmu::default();
    assert_eq!(emu.observe(TabletToolSignal::Contact), vec![]);
    assert_eq!(
        emu.observe(TabletToolSignal::Enter),
        vec![EmulatedPointerEvent::Entered, EmulatedPointerEvent::Press(MouseButton::Left)],
        "the focus switch flushes the deferred press"
    );
    assert_eq!(emu.observe(TabletToolSignal::Lift), vec![EmulatedPointerEvent::Release(
        MouseButton::Left
    )]);
}

#[test]
fn switch_buttons_map_like_a_mouse() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Enter);
    emu.observe(TabletToolSignal::Motion(pos(1.0, 1.0)));
    assert_eq!(emu.observe(press(BTN_STYLUS2)), vec![EmulatedPointerEvent::Press(
        MouseButton::Right
    )]);
    assert_eq!(emu.observe(lift(BTN_STYLUS2)), vec![EmulatedPointerEvent::Release(
        MouseButton::Right
    )]);
    assert_eq!(emu.observe(press(BTN_STYLUS3)), vec![EmulatedPointerEvent::Press(
        MouseButton::Middle
    )]);
    assert_eq!(emu.observe(press(0x91)), vec![EmulatedPointerEvent::Press(MouseButton::Other(
        0x91
    ))]);
    assert_eq!(
        emu.observe(press(BTN_STYLUS3)),
        vec![],
        "a held switch does not re-press while already down"
    );
    // Re-presses are distinct when the switch was lifted in between; other
    // switches stay held independently.
    assert_eq!(emu.observe(lift(BTN_STYLUS3)), vec![EmulatedPointerEvent::Release(
        MouseButton::Middle
    )]);
    assert_eq!(emu.observe(press(BTN_STYLUS2)), vec![EmulatedPointerEvent::Press(
        MouseButton::Right
    )]);
    assert_eq!(emu.observe(lift(0x91)), vec![EmulatedPointerEvent::Release(MouseButton::Other(
        0x91
    ))]);
    assert_eq!(emu.observe(lift(BTN_STYLUS3)), vec![], "the middle switch was already released");
}

#[test]
fn leave_drains_switches_and_contact() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Enter);
    emu.observe(TabletToolSignal::Motion(pos(0.0, 0.0)));
    assert_eq!(emu.observe(TabletToolSignal::Contact), vec![EmulatedPointerEvent::Press(
        MouseButton::Left
    )]);
    assert_eq!(emu.observe(press(BTN_STYLUS2)), vec![EmulatedPointerEvent::Press(
        MouseButton::Right
    )]);
    assert_eq!(emu.observe(TabletToolSignal::Leave), vec![
        EmulatedPointerEvent::Release(MouseButton::Right),
        EmulatedPointerEvent::Release(MouseButton::Left),
        EmulatedPointerEvent::Left,
    ]);
    assert_eq!(emu.held, vec![], "no drag sticks past the focus loss");
    assert!(!emu.contacted);
}

#[test]
fn removed_releases_without_left() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Enter);
    // The pen draws (contact flushes through a motion) when it is ripped
    // away; only the contact then releases, since `proximity_out` is
    // protocol-ordered before `removed` (so no `Left`).
    emu.observe(TabletToolSignal::Motion(pos(2.0, 2.0)));
    emu.observe(TabletToolSignal::Contact);
    assert_eq!(emu.observe(TabletToolSignal::Removed), vec![EmulatedPointerEvent::Release(
        MouseButton::Left
    )]);
    assert!(!emu.contacted);
    assert_eq!(emu.held, vec![]);
}

#[test]
fn removed_readies_contact_that_never_pressed() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Enter);
    // A stationary tap with no known position: no press was ever emitted,
    // and neither `removed` nor the seat removal may invent one.
    emu.observe(TabletToolSignal::Contact);
    assert_eq!(emu.observe(TabletToolSignal::Removed), vec![]);
    assert!(!emu.contacted);
    assert_eq!(emu.held, vec![]);
}

#[test]
fn pressure_reports_shape_nothing() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Type(TabletToolKind::Pen));
    emu.observe(TabletToolSignal::Enter);
    emu.observe(TabletToolSignal::Motion(pos(1.0, 1.0)));
    assert_eq!(
        emu.observe(TabletToolSignal::Pressure(40_000)),
        vec![],
        "a pressure report while hovering testifies nothing and draws nothing"
    );

    emu.observe(TabletToolSignal::Contact);
    let live = emu.observe(TabletToolSignal::Motion(pos(2.0, 2.0)));
    assert_eq!(
        emu.observe(TabletToolSignal::Pressure(60_000)),
        vec![],
        "a pressure report mid-stroke produces no pointer event of its own"
    );
    assert_eq!(
        live,
        vec![EmulatedPointerEvent::Moved(pos(2.0, 2.0))],
        "the stroke's reported press did not wait on pressure"
    );

    assert_eq!(emu.observe(TabletToolSignal::Lift), vec![
        EmulatedPointerEvent::Release(MouseButton::Left)
    ]);
    assert_eq!(emu.observe(TabletToolSignal::Pressure(65_535)), vec![], "a report after the lift draws nothing");
}

#[test]
fn tool_types_map_the_protocol_values() {
    let mut emu = TabletToolEmu::default();
    assert_eq!(emu.observe(TabletToolSignal::Type(TabletToolKind::Eraser)), vec![]);
    assert_eq!(emu.ty, Some(TabletToolKind::Eraser));
    assert_eq!(emu.observe(TabletToolSignal::Type(TabletToolKind::Pen)), vec![]);
    assert_eq!(emu.ty, Some(TabletToolKind::Pen));
}

#[test]
fn motions_out_of_focus_never_draw() {
    let mut emu = TabletToolEmu::default();
    assert_eq!(emu.observe(TabletToolSignal::Motion(pos(3.0, 4.0))), vec![]);
    assert_eq!(
        emu.observe(TabletToolSignal::Enter),
        vec![EmulatedPointerEvent::Entered, EmulatedPointerEvent::Moved(pos(3.0, 4.0)),],
        "the remembered motion surfaces with the focus"
    );
    assert_eq!(emu.observe(TabletToolSignal::Motion(pos(5.0, 6.0))), vec![
        EmulatedPointerEvent::Moved(pos(5.0, 6.0))
    ]);
}
