//! W28: pure translation of `zwp_tablet_v2` tool signals into the pointer semantics
//! winit 0.30 already owns, so applications keep drawing through their own pointer
//! pipeline: pen motion becomes `CursorMoved`, contact with the tablet becomes
//! `MouseInput`, and hover focus becomes `CursorEntered`/`CursorLeft`.
//!
//! This module is shared with the regression tests: the application compiles it
//! (via `#[path = …]`, as `scroll.rs` does for W13) against shim modules that
//! substitute the real `crate::dpi` and `crate::event` types. That is why it holds
//! no Wayland types: the compositor-to-signal and event-to-window glue lives in
//! `super::mod` (`TabletManagerState` and the dispatch impls); this file only
//! decides what those events are.
//!
//! Phase 1 (mouse emulation) limitations, kept deliberately:
//!
//! * pressure does not shape the translation (contact stays a plain left click); its reports feed
//!   the per-contact sample window in `crate::tablet` beside the translation; tilt, distance,
//!   rotation, slider and wheel axis updates are dropped;
//! * every tool type produces the same pointer shape; the recorded kind (`TabletToolKind`, the
//!   eraser end among them) is published for the application through `crate::tablet`, which makes
//!   an `Eraser` contact claim the application's eraser for its presses;

//! * tool switches map like a mouse: `BTN_STYLUS` (`0x14b`) left (mostly driver-configured erasers
//!   report it as contact), `BTN_STYLUS2` (`0x14c`) right, `BTN_STYLUS3` (`0x149`) middle, any
//!   other code `MouseButton::Other`;
//! * `frame` events are not used to batch: signals already arrive in protocol order, which is what
//!   a mouse produces.
//!
//! Protocol guarantees (zwp_tablet_v2 v2, stable) this state machine relies on:
//!
//! * `up`, possibly `proximity_out` and `removed` follow contact in that order; releases for every
//!   held button come before `proximity_out`, and `removed` sends `proximity_out` first when the
//!   tool was in proximity;
//! * a tap that starts under contact entering the input region arrives as `proximity_in`, `down`,
//!   `frame` — *without* a `motion` event, so a stationary tap has no position any client could
//!   know: the left press is deferred until the first `motion` and dropped if `up` comes first;
//! * while the tool stays in proximity, focus can move between surfaces by paired
//!   `proximity_in`/`proximity_out`.

use crate::dpi::LogicalPosition;
use crate::event::MouseButton;

/// Tool-switch codes on a tablet tool, forwarded raw from Linux evdev
/// (`input-event-codes.h`) on the `button` event.
const BTN_STYLUS: u32 = 0x14b;
const BTN_STYLUS2: u32 = 0x14c;
const BTN_STYLUS3: u32 = 0x149;

/// The tool type the compositor reported in the initial `type` burst. Recorded
/// for the event loop's device identity; phase 1 draws with every type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TabletToolKind {
    Pen,
    Eraser,
    Brush,
    Pencil,
    Airbrush,
    Finger,
    Mouse,
    Lens,
    #[default]
    Unknown,
}

impl TabletToolKind {}

/// What the dispatch glue observed about one tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TabletToolSignal {
    /// `type` in the initial burst (before `done`).
    Type(TabletToolKind),
    /// `proximity_in`: the tool became focused on a surface.
    Enter,
    /// `proximity_out`: the tool is no longer focused.
    Leave,
    /// `down`: logical contact.
    Contact,
    /// `up`: logical contact ended.
    Lift,
    /// `motion`, in logical surface coordinates.
    Motion(LogicalPosition<f64>),
    /// `pressure`, the raw protocol `uint` (spec: `0`–`65535`). The translation stays a mouse
    /// and reads nothing from it; the dispatch pairs it onto the contact it belongs to
    /// (`crate::tablet`) beside the translation.
    Pressure(u32),
    /// `button`, the raw Linux switch code and the
    /// `zwp_tablet_tool_v2::button_state` value (`0` released, `1` pressed).
    Button { button: u32, state: u32 },
    /// `removed`: the tool object is gone (the glue destroys it).
    Removed,
}

/// The pointer events one signal produced, in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EmulatedPointerEvent {
    /// `CursorEntered`: the tool focused a window.
    Entered,
    /// `CursorMoved`, where the position is known.
    Moved(LogicalPosition<f64>),
    /// `CursorLeft`.
    Left,
    /// `MouseInput` press.
    Press(MouseButton),
    /// `MouseInput` release.
    Release(MouseButton),
}

/// One tool's translation state.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct TabletToolEmu {
    ty: Option<TabletToolKind>,

    /// Last known logical position. Kept across focus changes so a remembered
    /// position still lands a deferrable contact where the pen was last seen,
    /// exactly like a mouse press at the last cursor position.
    position: Option<LogicalPosition<f64>>,

    /// Whether the tool is currently focused on the tracked surface.
    focused: bool,

    /// Whether the tool's contact is live per the protocol, whether or not a
    /// press could be emitted for it.
    contacted: bool,

    /// A contact whose press could not be emitted yet (see the module comment
    /// on stationary taps, and on focus switches during contact).
    pending_press: bool,

    /// Believed-pressed buttons still logically down, in press order, contact
    /// inclusive. `Lift`, `Leave` and `Removed` drain the tail so no drag sticks.
    held: Vec<MouseButton>,
}

impl TabletToolEmu {
    /// Feed one signal; return its pointer events in order.
    ///
    /// Every arm delegates so the machine reads as one dispatch table; the
    /// arm bodies live in the helpers below (the vendored rustfmt keeps a
    /// trailing comma on block arms, the repository's formatting does not, so
    /// block arms are avoided here for the file to hold under both).
    pub(crate) fn observe(&mut self, signal: TabletToolSignal) -> Vec<EmulatedPointerEvent> {
        match signal {
            TabletToolSignal::Type(kind) => self.record_type(kind),
            TabletToolSignal::Enter => self.enter(),
            TabletToolSignal::Motion(position) => self.motion(position),
            TabletToolSignal::Contact => self.contact(),
            TabletToolSignal::Pressure(_) => self.pressure_report(),
            TabletToolSignal::Lift => self.lift(),
            TabletToolSignal::Button { button, state } => self.button(button, state),
            TabletToolSignal::Leave => self.leave(),
            TabletToolSignal::Removed => self.removed(),
        }
    }

    /// The type the compositor reported in the initial burst, `Unknown` while nothing has.
    pub(crate) fn kind(&self) -> TabletToolKind {
        self.ty.unwrap_or(TabletToolKind::Unknown)
    }

    /// A pressure report never shapes the translation: it changes no machine state and
    /// produces no pointer event; the dispatch rides its pairing to the sample channel.
    fn pressure_report(&self) -> Vec<EmulatedPointerEvent> {
        Vec::new()
    }

    /// Whether the machine currently counts the tool's protocol contact as live: the anchor the
    /// dispatch pairs every contact's start with its one end on (see `crate::tablet`).
    pub(crate) fn contact_live(&self) -> bool {
        self.contacted
    }

    fn record_type(&mut self, kind: TabletToolKind) -> Vec<EmulatedPointerEvent> {
        self.ty = Some(kind);
        Vec::new()
    }

    fn enter(&mut self) -> Vec<EmulatedPointerEvent> {
        if self.focused {
            // Focus switched between windows without leaving proximity; the
            // glue resyncs the surface, nothing is synthetic here.
            Vec::new()
        } else {
            self.focused = true;
            let mut out = vec![EmulatedPointerEvent::Entered];
            if let Some(position) = self.position {
                out.push(EmulatedPointerEvent::Moved(position));
            }
            if self.pending_press {
                out.push(self.press_contact());
            }
            out
        }
    }

    fn motion(&mut self, position: LogicalPosition<f64>) -> Vec<EmulatedPointerEvent> {
        self.position = Some(position);
        if !self.focused {
            // Movement outside any focus is bookkeeping only.
            return Vec::new();
        }
        let mut out = vec![EmulatedPointerEvent::Moved(position)];
        if self.pending_press {
            out.push(self.press_contact());
        }
        out
    }

    fn contact(&mut self) -> Vec<EmulatedPointerEvent> {
        self.contacted = true;
        let mut out = Vec::new();
        if !self.focused {
            // Contact without a focused surface: wait for `Enter`.
            self.pending_press = true;
        } else if self.position.is_some() {
            // The normal path: the pen was seen hovering the surface.
            out.push(self.press_contact());
        } else {
            // A stationary tap entering the input region under contact.
            self.pending_press = true;
        }
        out
    }

    fn lift(&mut self) -> Vec<EmulatedPointerEvent> {
        self.contacted = false;
        self.pending_press = false;
        let mut out = Vec::new();
        if self.held.contains(&MouseButton::Left) {
            out.extend(self.release_tail());
        }
        out
    }

    fn button(&mut self, button: u32, state: u32) -> Vec<EmulatedPointerEvent> {
        let pressed = state == 1;
        let mut out = Vec::new();
        let button = emulate_button(button);
        if pressed {
            if !self.held.contains(&button) {
                self.held.push(button);
                if self.focused {
                    out.push(EmulatedPointerEvent::Press(button));
                }
            }
        } else if self.held.contains(&button) {
            self.held.retain(|held| *held != button);
            if self.focused {
                out.push(EmulatedPointerEvent::Release(button));
            }
        }
        out
    }

    fn leave(&mut self) -> Vec<EmulatedPointerEvent> {
        self.focused = false;
        self.pending_press = false;
        self.contacted = false;
        let mut out = Vec::new();
        // The compositor is supposed to have released held switches before
        // this event; drain anything left so no drag sticks.
        while !self.held.is_empty() {
            out.extend(self.release_tail());
        }
        out.push(EmulatedPointerEvent::Left);
        out
    }

    fn removed(&mut self) -> Vec<EmulatedPointerEvent> {
        // `proximity_out` precedes this event when the tool was in proximity,
        // so no `Left` here; the contact and any switch the compositor forgot
        // still release themselves. The glue destroys the tool object after.
        self.focused = false;
        self.pending_press = false;
        self.contacted = false;
        let mut out = Vec::new();
        while !self.held.is_empty() {
            out.extend(self.release_tail());
        }
        out
    }

    /// Emit the deferred-now-now press and record the button.
    fn press_contact(&mut self) -> EmulatedPointerEvent {
        self.pending_press = false;
        if !self.held.contains(&MouseButton::Left) {
            self.held.push(MouseButton::Left);
        }
        EmulatedPointerEvent::Press(MouseButton::Left)
    }

    /// Release the most recently pressed button (LIFO teardown).
    fn release_tail(&mut self) -> Option<EmulatedPointerEvent> {
        let released = self.held.pop()?;
        Some(EmulatedPointerEvent::Release(released))
    }
}

/// Map a raw tool switch code onto a mouse button the way `MouseInput` names them.
///
/// `BTN_STYLUS2`, the default "right click" switch on active pens, goes right;
/// `BTN_STYLUS3` (tip-programmable on XP-Pen) goes middle; the primary barrel
/// switch reports left, which keeps drivers that emulate a stamp through it
/// drawing continuously; anything else stays `Other` with the evdev code.
fn emulate_button(button: u32) -> MouseButton {
    match button {
        BTN_STYLUS2 => MouseButton::Right,
        BTN_STYLUS3 => MouseButton::Middle,
        BTN_STYLUS => MouseButton::Left,
        other => MouseButton::Other(other.min(u16::MAX as u32) as u16),
    }
}

#[cfg(test)]
// The application regression test includes `emulation.rs` and this file would
// then be reached by two formatters at once; this crate's rustfmt rules apply
// here and nowhere else (see `emulation_tests.rs`'s header comment).
#[rustfmt::skip]
#[path = "emulation_tests.rs"]
mod tests;
