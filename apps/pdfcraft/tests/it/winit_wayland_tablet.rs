//! The W28 tablet translation machine ("mouse-emulated pen"), compiled from the audited runtime
//! tree (`vendor/winit-tablet`, the path-patched fork the app builds against — unlike the older
//! `winit_*` modules, which read the retained `vendor/winit` copy) against the winit types the app
//! actually ships, without a compositor. It is not live-input coverage: drawing itself needs a
//! Wayland session with a tablet.
//!
//! What is asserted is the shape `PagePointer` and the rest of the pointer pipeline rely on: a
//! stroke is one left press followed by `CursorMoved`s and one left release, hover produces only
//! cursor events, and contact never invents a second button.
#![cfg(target_os = "linux")]

use crate::event::MouseButton;
use winit::dpi::LogicalPosition;

#[path = "../../../../vendor/winit-tablet/src/platform_impl/linux/wayland/seat/tablet/emulation.rs"]
mod emulation;

use emulation::{EmulatedPointerEvent, TabletToolEmu, TabletToolKind, TabletToolSignal};

fn pos(x: f64, y: f64) -> LogicalPosition<f64> {
    LogicalPosition::new(x, y)
}

/// The events one `EmulatedPointerEvent::Press`/`Release` names, projected onto the
/// `MouseInput` button the app sees.
fn buttons(events: &[EmulatedPointerEvent]) -> Vec<(winit::event::ElementState, MouseButton)> {
    events
        .iter()
        .filter_map(|event| match event {
            EmulatedPointerEvent::Press(button) => Some((winit::event::ElementState::Pressed, *button)),
            EmulatedPointerEvent::Release(button) => Some((winit::event::ElementState::Released, *button)),
            _ => None,
        })
        .collect()
}

/// The positions one signal's `CursorMoved` events name.
fn moves(events: &[EmulatedPointerEvent]) -> Vec<LogicalPosition<f64>> {
    events
        .iter()
        .filter_map(|event| match event {
            EmulatedPointerEvent::Moved(position) => Some(*position),
            _ => None,
        })
        .collect()
}

/// A whole hover stroke: approach, touch, drag, lift, leave. The app's drawing path must see the
/// same events a mouse would produce — one left-click bracket around the moved points — so the
/// existing pointer pipeline needs no tablet handling of its own.
#[test]
fn a_hover_stroke_is_one_left_click_bracket() {
    let mut emu = TabletToolEmu::default();

    // The initial burst names the tool type; the app draws with the active tool whatever it is.
    assert_eq!(emu.observe(TabletToolSignal::Type(TabletToolKind::Eraser)), vec![]);
    assert_eq!(emu.observe(TabletToolSignal::Enter), vec![EmulatedPointerEvent::Entered]);
    assert_eq!(emu.observe(TabletToolSignal::Motion(pos(40.0, 88.0))), vec![EmulatedPointerEvent::Moved(pos(40.0, 88.0))]);

    let contact = emu.observe(TabletToolSignal::Contact);
    assert_eq!(buttons(&contact), vec![(winit::event::ElementState::Pressed, MouseButton::Left)], "contact is a left click, never another button");
    assert_eq!(moves(&contact), vec![], "the press carries no position of its own");

    assert_eq!(moves(&emu.observe(TabletToolSignal::Motion(pos(52.0, 96.0)))), vec![pos(52.0, 96.0)]);
    assert_eq!(moves(&emu.observe(TabletToolSignal::Motion(pos(64.0, 104.0)))), vec![pos(64.0, 104.0)]);

    let lift = emu.observe(TabletToolSignal::Lift);
    assert_eq!(buttons(&lift), vec![(winit::event::ElementState::Released, MouseButton::Left)]);
    assert_eq!(moves(&lift), vec![]);

    assert_eq!(emu.observe(TabletToolSignal::Leave), vec![EmulatedPointerEvent::Left]);
}

/// The stationary tap the protocol allows (`proximity_in`, `down`, `frame`, no `motion`): the
/// press waits for the first motion — the only point where a position is knowable — and a tap
/// that ends before moving never produces one, so `PagePointer` cannot get a click at a fabricated
/// position.
#[test]
fn a_stationary_tap_presses_only_where_a_motion_says() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Enter);

    let down = emu.observe(TabletToolSignal::Contact);
    assert_eq!(buttons(&down), vec![], "no press without a known position");
    assert_eq!(emu.observe(TabletToolSignal::Lift), vec![], "a contact ended unseen never presses");

    // The same machine stays usable for the stroke that follows.
    let stroke = emu.observe(TabletToolSignal::Contact);
    assert_eq!(buttons(&stroke), vec![], "still no position");
    let drag = emu.observe(TabletToolSignal::Motion(pos(8.0, 8.0)));
    assert_eq!(moves(&drag), vec![pos(8.0, 8.0)]);
    assert_eq!(buttons(&drag), vec![(winit::event::ElementState::Pressed, MouseButton::Left)], "the deferred press flushes after the first motion");
    assert_eq!(buttons(&emu.observe(TabletToolSignal::Lift)), vec![(winit::event::ElementState::Released, MouseButton::Left)]);
}

/// Barrel switches arrive as raw evdev codes and must land on the mouse buttons the app's
/// right-click (canvas menu) and middle-click (pan) handlers already answer.
#[test]
fn switch_codes_map_onto_mouse_buttons() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Enter);
    emu.observe(TabletToolSignal::Motion(pos(0.0, 0.0)));

    // BTN_STYLUS2 = 0x14c, the default right-click switch; BTN_STYLUS3 = 0x149.
    let right = emu.observe(TabletToolSignal::Button { button: 0x14c, state: 1 });
    assert_eq!(buttons(&right), vec![(winit::event::ElementState::Pressed, MouseButton::Right)]);
    let middle = emu.observe(TabletToolSignal::Button { button: 0x149, state: 1 });
    assert_eq!(buttons(&middle), vec![(winit::event::ElementState::Pressed, MouseButton::Middle)]);

    // Unknown codes stay named, so the app can ignore them explicitly.
    let other = emu.observe(TabletToolSignal::Button { button: 0x91, state: 1 });
    assert_eq!(buttons(&other), vec![(winit::event::ElementState::Pressed, MouseButton::Other(0x91))]);

    // Releasing without focus (pen lifted off the tablet mid-press, driver quirk) must still
    // release: the app cannot keep a ghost drag. The contact click never happened here (no
    // `Contact` was sent), so only the three switches drain.
    let leave = emu.observe(TabletToolSignal::Leave);
    assert_eq!(
        buttons(&leave),
        vec![
            (winit::event::ElementState::Released, MouseButton::Other(0x91)),
            (winit::event::ElementState::Released, MouseButton::Middle),
            (winit::event::ElementState::Released, MouseButton::Right),
        ],
        "everything held releases on focus loss, most recent first"
    );
    assert_eq!(leave.last(), Some(&EmulatedPointerEvent::Left));
}

/// A hover-stroke drag between the windows: motion that reaches the app always carries the
/// compositor's logical position, in order, and only while the tool is focused.
#[test]
fn motion_is_reported_only_while_focused() {
    let mut emu = TabletToolEmu::default();
    assert_eq!(emu.observe(TabletToolSignal::Motion(pos(1.0, 1.0))), vec![], "motion with no focus is bookkeeping");

    // Contact caught mid-flight (focus after contact): the enter flushes the press.
    emu.observe(TabletToolSignal::Contact);
    let enter = emu.observe(TabletToolSignal::Enter);
    assert_eq!(
        enter,
        vec![EmulatedPointerEvent::Entered, EmulatedPointerEvent::Moved(pos(1.0, 1.0)), EmulatedPointerEvent::Press(MouseButton::Left),],
        "enter flushes the deferred press at the remembered position"
    );

    // Focus loss mid-stroke: releases drain before the cursor leaves, and later motions draw
    // nothing.
    let leave = emu.observe(TabletToolSignal::Leave);
    assert_eq!(buttons(&leave), vec![(winit::event::ElementState::Released, MouseButton::Left)]);
    assert_eq!(moves(&emu.observe(TabletToolSignal::Motion(pos(9.0, 9.0)))), vec![], "motions outside focus never draw");
}

/// Phase 1 draws with the active tool whatever it is — every `zwp_tablet_tool_v2` type the
/// compositor can report produces exactly the same pointer shape, so an eraser end (its own
/// tool object) draws too. Pressure-aware tool selection is a phase-2 item.
#[test]
fn every_tool_type_draws_the_same_way() {
    for kind in [
        TabletToolKind::Pen,
        TabletToolKind::Eraser,
        TabletToolKind::Brush,
        TabletToolKind::Pencil,
        TabletToolKind::Airbrush,
        TabletToolKind::Finger,
        TabletToolKind::Mouse,
        TabletToolKind::Lens,
    ] {
        let mut emu = TabletToolEmu::default();
        assert_eq!(emu.observe(TabletToolSignal::Type(kind)), vec![], "recording a type emits nothing");
        emu.observe(TabletToolSignal::Enter);
        emu.observe(TabletToolSignal::Motion(pos(2.0, 3.0)));
        let contact = emu.observe(TabletToolSignal::Contact);
        assert_eq!(buttons(&contact), vec![(winit::event::ElementState::Pressed, MouseButton::Left)], "{kind:?} contacts like a left click");
    }

    // Unknown is the zero/default type (a compositor reporting nothing in the burst).
    assert_eq!(TabletToolKind::default(), TabletToolKind::Unknown);
}

/// Pressure rides beside the translation (phase 2): a report produces no pointer event of its
/// own and never delays or reshapes the press a stroke is made of, in hover, mid-stroke and
/// after the lift alike. The application reads it from the channel (`winit::tablet`), whose
/// pairing test lives in the fork; this file pins the machine-side half of that contract.
#[test]
fn pressure_reports_shape_nothing() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Type(TabletToolKind::Pen));
    emu.observe(TabletToolSignal::Enter);
    assert_eq!(emu.observe(TabletToolSignal::Pressure(40_000)), vec![], "hover does not draw");

    emu.observe(TabletToolSignal::Motion(pos(10.0, 10.0)));
    let contact = emu.observe(TabletToolSignal::Contact);
    assert_eq!(buttons(&contact), vec![(winit::event::ElementState::Pressed, MouseButton::Left)], "the press is not held back by pressure");
    assert_eq!(emu.observe(TabletToolSignal::Pressure(60_000)), vec![]);
    assert_eq!(moves(&emu.observe(TabletToolSignal::Motion(pos(20.0, 20.0)))), vec![pos(20.0, 20.0)], "motion keeps flowing after pressure");
    assert_eq!(buttons(&emu.observe(TabletToolSignal::Lift)), vec![(winit::event::ElementState::Released, MouseButton::Left)]);
    assert_eq!(emu.observe(TabletToolSignal::Pressure(65_535)), vec![], "nothing draws after the lift");
    assert_eq!(buttons(&emu.observe(TabletToolSignal::Removed)), vec![], "a removal mid-pressure invents nothing");
}

/// The eraser end is not a barrel switch and not a pen: the compositor reports it as its own
/// tool object typed `Eraser`, and the machine keeps that kind behind the same pointer shape.
/// The application erases with it through the contact channel (`winit::tablet`), so nothing in
/// the pointer shape may change — a right/middle or `Other` button for the eraser would leak
/// tool ownership into mouse semantics.
#[test]
fn the_eraser_end_is_identified_and_presses_like_the_pen() {
    let mut emu = TabletToolEmu::default();
    assert_eq!(emu.observe(TabletToolSignal::Type(TabletToolKind::Eraser)), vec![]);
    assert_eq!(emu.kind(), TabletToolKind::Eraser, "the eraser end's tool type is recorded, not discarded");
    assert!(!emu.contact_live(), "the eraser end's tool object is not in contact before its own first");

    emu.observe(TabletToolSignal::Enter);
    emu.observe(TabletToolSignal::Motion(pos(3.0, 3.0)));
    let contact = emu.observe(TabletToolSignal::Contact);
    assert_eq!(contact, vec![EmulatedPointerEvent::Press(MouseButton::Left)], "an eraser contact is still a plain left click of the pointer shape");
    assert!(emu.contact_live(), "the machine anchors the channel's pairing on its own contact record");
}

/// Switching between the pen and the eraser needs no restart: each end is its own tool object
/// with its own type, and both stay usable down and back up in turn.
#[test]
fn pen_and_eraser_switch_over_without_a_reset() {
    // The pen strokes, lifts, and stays in proximity (its object is not destroyed).
    let mut pen = TabletToolEmu::default();
    assert_eq!(pen.observe(TabletToolSignal::Type(TabletToolKind::Pen)), vec![]);
    pen.observe(TabletToolSignal::Enter);
    pen.observe(TabletToolSignal::Motion(pos(1.0, 1.0)));
    pen.observe(TabletToolSignal::Contact);
    pen.observe(TabletToolSignal::Motion(pos(2.0, 2.0)));
    assert_eq!(buttons(&pen.observe(TabletToolSignal::Lift)), vec![(winit::event::ElementState::Released, MouseButton::Left)]);
    assert_eq!(pen.kind(), TabletToolKind::Pen);

    // The flipped pen enters proximity as the eraser's own object: it erases through the
    // channel the app asks beside these events, while the pointer shape stays pen-like.
    let mut eraser = TabletToolEmu::default();
    assert_eq!(eraser.observe(TabletToolSignal::Type(TabletToolKind::Eraser)), vec![]);
    eraser.observe(TabletToolSignal::Enter);
    eraser.observe(TabletToolSignal::Motion(pos(5.0, 5.0)));
    assert_eq!(buttons(&eraser.observe(TabletToolSignal::Contact)), vec![(winit::event::ElementState::Pressed, MouseButton::Left)]);
    assert_eq!(eraser.kind(), TabletToolKind::Eraser);
    assert_eq!(buttons(&eraser.observe(TabletToolSignal::Lift)), vec![(winit::event::ElementState::Released, MouseButton::Left)]);

    // Flipped back: the pen's own object never saw the eraser's contact, and it records the
    // tool it always had.
    assert_eq!(pen.kind(), TabletToolKind::Pen);
    assert!(!pen.contact_live(), "the pen's contact ended with its lift; the channel gives it back");
}

/// A removal mid-erase must free everything: the machine's pairing anchor reads contact over,
/// the held buttons drain, and the channel's attribution ends with them.
#[test]
fn a_removal_during_eraser_contact_ends_cleanly() {
    let mut emu = TabletToolEmu::default();
    emu.observe(TabletToolSignal::Type(TabletToolKind::Eraser));
    emu.observe(TabletToolSignal::Enter);
    emu.observe(TabletToolSignal::Motion(pos(4.0, 4.0)));
    emu.observe(TabletToolSignal::Contact);

    let removed = emu.observe(TabletToolSignal::Removed);
    assert_eq!(
        removed,
        vec![EmulatedPointerEvent::Release(MouseButton::Left)],
        "the erase's press releases; the compositor sends proximity_out first"
    );
    assert!(!emu.contact_live(), "the channel's eraser attribution ends with the removal");
}
