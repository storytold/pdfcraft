//! The sidecar's Wayland state: the registry/seat/tablet object graph, and the frame
//! accumulator that folds each `zwp_tablet_tool_v2` protocol frame into one [`PenEvent`].
//!
//! Every protocol fact below is cited from the tablet-v2 XML shipped inside the
//! `wayland-protocols` crate this build depends on —
//! `protocols/stable/tablet/tablet-v2.xml` in the crate source:
//!
//! - Tablet tool events are grouped by `frame`: “Tablet tool events are grouped by
//!   zwp_tablet_tool_v2.frame events. Any events received before a
//!   zwp_tablet_tool_v2.frame event should be considered part of the same hardware
//!   state change” (`zwp_tablet_tool_v2` interface description). So the wire delivers
//!   no per-event granularity, and one frame is the honest unit: this crate folds
//!   each frame into at least one [`PenEvent`] — one per distinct transition the
//!   frame carries, in a canonical physical order (see `ToolState::emit`).
//! - Pressure is a uint “normalized to a value between 0 and 65535” (`pressure`
//!   event); normalized here to 0..1. Hostile values above 65535 clamp (the
//!   never-crash policy outranks protocol trust).
//! - Tilt is two `fixed` values “in degrees, relative to the z-axis of the tablet”
//!   (`tilt` event); the generated bindings turn `fixed` into `f64`
//!   (wayland-scanner `src/common.rs`, `Type::Fixed => f64`), so the degrees arrive
//!   already converted.
//! - The identification burst (`type`, `hardware_serial`, `hardware_id_wacom`,
//!   `capability`) that follows `tool_added` “is terminated by a
//!   zwp_tablet_tool_v2.done event” — a `done`, *not* a `frame`, so it produces no
//!   [`PenEvent`] (`zwp_tablet_tool_v2` description).
//! - `proximity_in` carries `serial`, the `zwp_tablet_v2` and the *focused surface* of
//!   this client (`proximity_in` event) — the reason the sidecar must live on the
//!   windowing library's own `wl_display`: a second connection would be a different
//!   client and would never receive it.
//! - Nothing here names button codes: the `button` event's `button` argument is a raw
//!   Linux event code by the protocol (“button is an enum, but not a wl_pointer one”
//!   — tablet-v2.xml `button` event; codes are not guessed, they are forwarded raw).
//! - `up` — notably — carries *no* serial: only `proximity_in`, `down` and `button`
//!   have one (tablet-v2.xml `up` event has no arguments at all). Serials are
//!   forwarded from the events that carry them, never invented.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use wayland_client::backend::ObjectId;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_manager_v2::{self, ZwpTabletManagerV2};

use crate::SidecarError;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_dial_v2::{self, ZwpTabletPadDialV2};
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_group_v2::{self, ZwpTabletPadGroupV2};
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_ring_v2::{self, ZwpTabletPadRingV2};
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_strip_v2::{self, ZwpTabletPadStripV2};
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_v2::{self, ZwpTabletPadV2};
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_seat_v2::{self, ZwpTabletSeatV2};
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_tool_v2::{self, ButtonState, Type, ZwpTabletToolV2};
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_v2::{self, ZwpTabletV2};

/// What the tool is: the `type` event's enum, mapped one-to-one
/// (tablet-v2.xml `zwp_tablet_tool_v2.type`: pen 0x140 … lens 0x147).
///
/// An eraser end reports `eraser` — this is what the UI needs to make the eraser end
/// erase and the pen end draw (Acrobat behaviour; the fork saga's “tool type
/// scrambling” hardware caveat lives in `docs/hardware-parity.md`, compositor-side).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolKind {
    Pen,
    Eraser,
    Brush,
    Pencil,
    Airbrush,
    Finger,
    Mouse,
    Lens,
    /// A value this build's XML does not list (`WEnum::Unknown`): forwarded as itself.
    #[default]
    Unknown,
}

/// One protocol frame's pen snapshot: the tool state as the frame left it.
///
/// persistent state (position, pressure, tilt) is whatever the tool reported last —
/// the protocol carries no “reset” between frames, and neither does this.
///
/// `Copy`: a lossless frame hands the same snapshot to every transition it emits
/// (see `ToolState::emit`), and the type is a handful of scalar fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenFrame {
    /// The tool kind (`type` event; `Unknown` before the identification burst lands).
    pub tool: ToolKind,
    /// The focused surface's `wl_surface` id (`proximity_in`'s `surface` argument).
    /// The sidecar does not hold winit's surface proxy; the raw protocol id is what
    /// survives (enough to tell windows apart).
    pub surface: Option<u32>,
    /// Surface-local x (`motion`'s `x`, `fixed` → f64). 0 until the first `motion`.
    pub x: f64,
    /// Surface-local y.
    pub y: f64,
    /// `pressure` normalized 0..1 (protocol's 0..65535 per tablet-v2.xml; clamped).
    pub pressure: f64,
    /// `tilt_x` in degrees, protocol's own units (tablet-v2.xml `tilt` event).
    pub tilt_x: f64,
    /// `tilt_y` in degrees.
    pub tilt_y: f64,
    /// The `frame` event's `time`, in milliseconds (“the time of the event with
    /// millisecond granularity”): the compositor's CLOCK_MONOTONIC, truncated to u32
    /// by the protocol. Delay measurement in `examples/print_pen.rs` deltas this
    /// against the same clock at arrival.
    pub time_ms: u32,
}

impl PenFrame {
    /// The tool state as the frame left it.
    fn snapshot(state: &ToolState) -> Self {
        Self {
            tool: state.kind,
            surface: state.surface,
            x: state.x,
            y: state.y,
            pressure: state.pressure,
            tilt_x: state.tilt_x,
            tilt_y: state.tilt_y,
            time_ms: state.time_ms,
        }
    }
}

/// One pen event, one per distinct transition in a protocol `frame` (see the module
/// docs for why).
///
/// All `serial` values are the protocol's, for `zwp_tablet_tool_v2.set_cursor` whose
/// request needs “serial of the proximity_in event” (tablet-v2.xml). Every event of a
/// frame carries the same end-of-frame snapshot (see `ToolState::emit`).
#[derive(Clone, Debug, PartialEq)]
pub enum PenEvent {
    /// `proximity_in`: the tool entered a surface.
    ProximityIn { frame: PenFrame, serial: u32 },
    /// `down`: tip contact begins.
    Down { frame: PenFrame, serial: u32 },
    /// `up`: tip contact ends (no serial — tablet-v2.xml `up` has no arguments).
    Up { frame: PenFrame },
    /// `motion`, or any other axis-only change in the frame (pressure or tilt without
    /// movement: the frame's snapshot carries the fresh values).
    Move { frame: PenFrame },
    /// `button`: a tool button (barrel button) changed. `button` is the protocol's raw
    /// Linux event code (tablet-v2.xml `button` event); never a guessed enum here.
    Button { frame: PenFrame, button: u32, pressed: bool },
    /// `proximity_out`: the tool left the surface (or jumped surfaces; the next
    /// frame's `proximity_in` announces the destination).
    ProximityOut { frame: PenFrame },
}

impl PenEvent {
    /// The event's serial, where the protocol carries one (see [`PenEvent`]).
    #[must_use]
    pub fn serial(&self) -> Option<u32> {
        match self {
            Self::ProximityIn { serial, .. } | Self::Down { serial, .. } => Some(*serial),
            Self::Up { .. } | Self::Move { .. } | Self::Button { .. } | Self::ProximityOut { .. } => None,
        }
    }

    /// The frame's snapshot.
    #[must_use]
    pub fn frame(&self) -> &PenFrame {
        match self {
            Self::ProximityIn { frame, .. }
            | Self::Down { frame, .. }
            | Self::Up { frame }
            | Self::Move { frame }
            | Self::Button { frame, .. }
            | Self::ProximityOut { frame } => frame,
        }
    }
}

/// The wire-grain grammar the accumulator consumes — thin over the generated event
/// enum, and thin enough for the tests below to drive without a compositor.
///
/// Everything the tests cannot build for real (proxies) stays out: this is a pure
/// state machine on values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RawTool {
    /// `type` (the identification burst).
    Type(ToolKind),
    /// `done` (end of the identification burst) — carries no frame by itself.
    Done,
    /// `removed`.
    Removed,
    /// `proximity_in { serial, tablet, surface }` (the tablet proxy is tracked at the
    /// seat level, not per tool; the surface by its raw protocol id).
    ProximityIn { serial: u32, surface: Option<u32> },
    /// `proximity_out`.
    ProximityOut,
    /// `down`, with the event's serial.
    Down { serial: u32 },
    /// `up` (no serial by the protocol — tablet-v2.xml `up` has no arguments).
    Up,
    /// `motion { x, y }`, degrees-free fixed→f64 as generated.
    Motion { x: f64, y: f64 },
    /// `pressure` as the protocol's own 0..65535.
    Pressure { value: u32 },
    /// `tilt`, degrees as the protocol words them.
    Tilt { x: f64, y: f64 },
    /// `button { button, state }`, the code raw.
    Button { button: u32, pressed: bool },
    /// `frame { time }` — the frame terminator, with the compositor's ms.
    Frame(u32),
}

/// What a frame accumulates. One wire event per field; the frame folds them into
/// one [`PenEvent`] per present transition (see `ToolState::emit`).
///
/// One slot per kind: two transitions of the same kind in a frame collapse to the
/// last — three wire transitions (say down, up, down) become two events — but the
/// end state is still the wire's, because the tool's state fields (`contact`,
/// position, pressure) are applied per wire event, not per slot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Batch {
    /// `proximity_in` with its serial.
    prox_in: Option<u32>,
    prox_out: bool,
    /// `down` with its serial.
    down: Option<u32>,
    up: bool,
    moved: bool,
    /// pressure or tilt changed (no motion in the frame): still worth an event.
    axes: bool,
    /// last `button` change in the frame (two changes of the same button in one
    /// frame are lossy: the batch keeps one slot per kind).
    button: Option<(u32, bool)>,
}

impl Batch {
    const EMPTY: Self = Self { prox_in: None, prox_out: false, down: None, up: false, moved: false, axes: false, button: None };

    fn is_empty(&self) -> bool {
        *self == Self::EMPTY
    }
}

/// One physical tool as the sidecar holds it, keyed by its object id in [`AppState`].
///
/// The proxy is `None` only in tests: [`RawTool`] drives this state machine purely,
/// with no compositor and no connection.
#[derive(Debug)]
pub(crate) struct ToolState {
    proxy: Option<ZwpTabletToolV2>,
    kind: ToolKind,
    /// The focused surface's protocol id (`proximity_in`); the surface is winit's
    /// proxy, and this crate keeps only the id (see [`PenFrame::surface`]).
    surface: Option<u32>,
    x: f64,
    y: f64,
    pressure: f64,
    tilt_x: f64,
    tilt_y: f64,
    /// Current state as the protocol words it: contact state between frames.
    contact: bool,
    batch: Batch,
    time_ms: u32,
}

impl ToolState {
    pub(crate) fn new(proxy: Option<ZwpTabletToolV2>) -> Self {
        Self {
            proxy,
            kind: ToolKind::Unknown,
            surface: None,
            x: 0.0,
            y: 0.0,
            pressure: 0.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            contact: false,
            batch: Batch::EMPTY,
            time_ms: 0,
        }
    }

    /// Feed one wire-grain event; any [`PenEvent`]s it closes land in `out`.
    fn apply(&mut self, ev: RawTool, out: &mut Vec<PenEvent>) {
        match ev {
            RawTool::Type(kind) => self.kind = kind,
            RawTool::Done => {}
            RawTool::ProximityIn { serial, surface } => {
                self.surface = surface;
                self.batch.prox_in = Some(serial);
            }
            RawTool::ProximityOut => {
                self.batch.prox_out = true;
                self.contact = false;
            }
            RawTool::Down { serial } => {
                self.contact = true;
                self.batch.down = Some(serial);
            }
            RawTool::Up => {
                self.contact = false;
                self.batch.up = true;
            }
            RawTool::Motion { x, y } => {
                self.x = x;
                self.y = y;
                self.batch.moved = true;
            }
            RawTool::Pressure { value } => {
                self.pressure = normalize_pressure(value);
                self.batch.axes = true;
            }
            RawTool::Tilt { x, y } => {
                self.tilt_x = x;
                self.tilt_y = y;
                self.batch.axes = true;
            }
            RawTool::Button { button, pressed } => self.batch.button = Some((button, pressed)),
            RawTool::Removed => self.proxy = None,
            RawTool::Frame(time) => {
                if self.batch.is_empty() {
                    // A frame with nothing before it is not observable — often the
                    // identification burst's very first frame (or a compositor's
                    // empty beat). Emitting would spam the app with snapshots.
                    return;
                }
                self.time_ms = time;
                self.emit(out);
            }
        }
    }

    /// Fold the batch, losslessly: one [`PenEvent`] per distinct transition present,
    /// in a canonical physical order — proximity_in > down > button > move > up >
    /// proximity_out — every event carrying the same end-of-frame snapshot (position,
    /// pressure, tilt, surface, time as the frame left them).
    ///
    /// The order is not the wire's: the batch keeps only the last event of each kind
    /// (one slot per field), so the wire's order within a frame is already gone. What
    /// the canonical order preserves is the app's view of a physical sequence: a pen
    /// cannot contact before entering proximity (proximity_in > down), the release
    /// lands where the frame left the pen (move > up), and leaving proximity ends
    /// everything (proximity_out last). One exception, decided by the tool's tip
    /// state: a lift and a re-press in one frame reads Up before Down, because the
    /// wire's last up/down transition is the re-press (see the body's comment).
    /// The old precedence (one event per frame,
    /// down > up > …) lost real input: a click shorter than a frame delivered only
    /// its Down and stuck the app's press, and a barrel-button change riding the tip
    /// frame was dropped.
    fn emit(&mut self, out: &mut Vec<PenEvent>) {
        let frame = PenFrame::snapshot(self);
        let batch = std::mem::replace(&mut self.batch, Batch::EMPTY);
        if let Some(serial) = batch.prox_in {
            out.push(PenEvent::ProximityIn { frame, serial });
        }
        // A lift and a re-press sharing one frame: the canonical order (down > up)
        // would leave the app released while the pen is down. The wire's *last*
        // up/down transition decides the order, and `contact` after the batch
        // applied is exactly that: pressed ⇒ the lift came first, so the app sees
        // Up before the re-press Down. (A prox_out in the same batch would also
        // have cleared `contact`, but out+up+down is no physical sequence — the
        // module doc's note on surface jumps covers the out; a re-press needs a
        // prox_in of its own frame.)
        let up_first = batch.up && batch.down.is_some() && self.contact;
        if up_first {
            out.push(PenEvent::Up { frame });
        }
        if let Some(serial) = batch.down {
            out.push(PenEvent::Down { frame, serial });
        }
        if let Some((button, pressed)) = batch.button {
            out.push(PenEvent::Button { frame, button, pressed });
        }
        if batch.moved || batch.axes {
            out.push(PenEvent::Move { frame });
        }
        if batch.up && !up_first {
            out.push(PenEvent::Up { frame });
        }
        if batch.prox_out {
            out.push(PenEvent::ProximityOut { frame });
        }
    }

    /// Destroy the tool's proxy, if it was ever real (tests run without one).
    pub(crate) fn drop_proxy(&mut self) {
        if let Some(proxy) = self.proxy.as_ref().filter(|proxy| proxy.is_alive()) {
            // zwp_tablet_tool_v2.destroy, a destructor request since the
            // interface's version 1 (tablet-v2.xml `destroy`).
            proxy.destroy();
        }
        self.proxy = None;
    }

    /// Tests drive the pure state machine; no proxy, no compositor.
    #[cfg(test)]
    fn synthetic() -> Self {
        Self {
            proxy: None,
            kind: ToolKind::Unknown,
            surface: None,
            x: 0.0,
            y: 0.0,
            pressure: 0.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            contact: false,
            batch: Batch::EMPTY,
            time_ms: 0,
        }
    }
}

/// The protocol's 0..65535 pressure to 0..1 (tablet-v2.xml `pressure`).
///
/// A hostile report beyond 65535 clamps to 1.0 — never crash, never let a
/// not-normalized value reach the UI.
fn normalize_pressure(value: u32) -> f64 {
    if value > 65535 { 1.0 } else { f64::from(value) / 65535.0 }
}

/// One of a pad group's axis children, as bound. The pad carries no pen input (the
/// pen path is the tool's), so this exists only so the child can be destroyed in
/// order (see [`PadRecord::destroy`]).
enum PadChild {
    Ring(ZwpTabletPadRingV2),
    Strip(ZwpTabletPadStripV2),
    Dial(ZwpTabletPadDialV2),
}

impl PadChild {
    /// The child's own destructor: all of ring/strip/dial have one since version 1
    /// (tablet-v2.xml `zwp_tablet_pad_ring_v2.destroy` and its siblings).
    fn destroy(&self) {
        match self {
            Self::Ring(proxy) => proxy.destroy(),
            Self::Strip(proxy) => proxy.destroy(),
            Self::Dial(proxy) => proxy.destroy(),
        }
    }

    fn is_alive(&self) -> bool {
        match self {
            Self::Ring(proxy) => proxy.is_alive(),
            Self::Strip(proxy) => proxy.is_alive(),
            Self::Dial(proxy) => proxy.is_alive(),
        }
    }
}

/// One bound pad and the objects it offered, the minimum teardown needs: the pad's
/// proxy, its button groups', and the groups' rings/strips/dials. Nothing here is
/// read (pad events carry no pen input); it exists because the pad's `removed`
/// event mandates an ordered destroy (see `PadRecord::destroy`) and because a
/// dropped proxy is an orphan on this guest connection — the same reason the
/// tablets and tools are kept.
struct PadRecord {
    pad: ZwpTabletPadV2,
    /// The pad's button groups (`zwp_tablet_pad_v2.group`, one event per group).
    groups: Vec<ZwpTabletPadGroupV2>,
    /// The groups' axis children, in no order (all are destroyed before any group).
    children: Vec<PadChild>,
}

impl PadRecord {
    fn new(pad: ZwpTabletPadV2) -> Self {
        Self { pad, groups: Vec::new(), children: Vec::new() }
    }

    /// The group this pad owns, for recording the group's axis children — the
    /// group's own events know only the group proxy.
    fn holds_group(&self, group: &ObjectId) -> bool {
        self.groups.iter().any(|g| g.id() == *group)
    }

    /// Destroy the pad and everything it offered, children first. The pad's
    /// `removed` mandates the order (tablet-v2.xml `zwp_tablet_pad_v2.removed`):
    /// "the client must destroy all rings, strips and groups that were offered by
    /// this pad, and issue zwp_tablet_pad_v2.destroy the pad itself" — and the pad's
    /// and group's destructors do not cascade (tablet-v2.xml
    /// `zwp_tablet_pad_v2.destroy`: "Objects created from this object are unaffected
    /// and should be destroyed separately"; the group's says the same).
    fn destroy(&mut self) {
        for child in &self.children {
            if child.is_alive() {
                child.destroy();
            }
        }
        for group in &self.groups {
            if group.is_alive() {
                group.destroy();
            }
        }
        if self.pad.is_alive() {
            self.pad.destroy();
        }
    }
}

/// Everything the sidecar bound, and where events land.
pub(crate) struct AppState {
    tx: Sender<PenEvent>,
    pub(crate) seat: Option<WlSeat>,
    pub(crate) manager: Option<ZwpTabletManagerV2>,
    tablet_seat: Option<ZwpTabletSeatV2>,
    /// All tablets announced by `tablet_added`, kept for cleanup.
    tablets: HashMap<ObjectId, ZwpTabletV2>,
    /// All pads announced by `pad_added`, kept for cleanup (see [`PadRecord`]).
    pads: HashMap<ObjectId, PadRecord>,
    /// All tools announced by `tool_added`, keyed by object id.
    tools: HashMap<ObjectId, ToolState>,
}

impl AppState {
    pub(crate) fn new(tx: Sender<PenEvent>) -> Self {
        Self { tx, seat: None, manager: None, tablet_seat: None, tablets: HashMap::new(), pads: HashMap::new(), tools: HashMap::new() }
    }

    /// Bind the seat and the tablet manager off `globals` (both were spied during
    /// `registry_queue_init`'s roundtrip), then request the tablet seat off the two
    /// (`zwp_tablet_manager_v2.get_tablet_seat`, tablet-v2.xml — “to get access to
    /// the actual tablets, use zwp_tablet_manager_v2.get_tablet_seat”).
    ///
    /// The sidecar is a consumer, not a global arbiter: only the first announced
    /// `wl_seat` is claimed (a multi-seat box's other seats go untouched; spike note).
    pub(crate) fn bind_tables(&mut self, globals: &wayland_client::globals::GlobalList, qh: &QueueHandle<Self>) -> Result<(), crate::SidecarError> {
        use wayland_client::globals::BindError;
        // The seat needs version ≥ 5: `release` (its destructor, used at shutdown) is
        // `since=5` in wayland-client's wayland.xml, and 5..=v11 finds it at any
        // compositor this code runs on; older seats fail loudly, not silently.
        let seat = match globals.bind::<WlSeat, Self, ()>(qh, 5..=WlSeat::interface().version, ()) {
            Ok(seat) => seat,
            Err(BindError::NotPresent) => return Err(SidecarError::NoSeat),
            Err(BindError::UnsupportedVersion) => return Err(SidecarError::SeatTooOld),
        };
        let manager = match globals.bind::<ZwpTabletManagerV2, Self, ()>(qh, 1..=ZwpTabletManagerV2::interface().version, ()) {
            Ok(manager) => manager,
            // “No tablets here” — the manager global is missing (its version can only
            // be 2: tablet-v2.xml `zwp_tablet_manager_v2 version="2"`).
            Err(_) => return Err(SidecarError::NoTabletManager),
        };
        // `get_tablet_seat` is a `new_id` constructor: the seat comes back instantly;
        // its events (tablet_added/tool_added) arrive on this queue at the next
        // dispatch (tablet-v2.xml, `zwp_tablet_manager_v2.get_tablet_seat`).
        let tablet_seat = manager.get_tablet_seat(&seat, qh, ());
        self.seat = Some(seat);
        self.manager = Some(manager);
        self.tablet_seat = Some(tablet_seat);
        Ok(())
    }

    /// The sidecar is shutting down: destroy every real proxy this crate bound, so
    /// dropping leaves no orphan objects on the (foreign, guest-mode) connection.
    ///
    /// Order: each pad with its children first (the pad's `removed` mandates
    /// children before the pad; here at shutdown nothing is mandated, but one order
    /// everywhere), then the tablets, then the tools, then the tablet seat, then the
    /// manager. Called after the reader thread is joined (Thread mode) or on the app
    /// thread (Main mode), so nothing dispatches onto destroyed proxies.
    pub(crate) fn shutdown(&mut self) {
        for (_, mut pad) in self.pads.drain() {
            pad.destroy();
        }
        for (_, tablet) in self.tablets.drain() {
            // zwp_tablet_v2.destroy, a destructor request since the interface's
            // version 1 (tablet-v2.xml `destroy`: "This destroys the client's
            // resource for this tablet object").
            if tablet.is_alive() {
                tablet.destroy();
            }
        }
        for tool in self.tools.values_mut() {
            tool.drop_proxy();
        }
        self.tools.clear();
        if let Some(seat_block) = self.tablet_seat.as_ref().filter(|it| it.is_alive()) {
            // zwp_tablet_seat_v2.destroy, a destructor since version 1
            // (tablet-v2.xml `zwp_tablet_seat_v2.destroy`).
            seat_block.destroy();
        }
        self.tablet_seat = None;
        if let Some(manager) = self.manager.as_ref().filter(|it| it.is_alive()) {
            // zwp_tablet_manager_v2.destroy, destructor since version 1.
            manager.destroy();
        }
        self.manager = None;
        if let Some(seat) = self.seat.as_ref().filter(|it| it.is_alive() && it.version() >= 5) {
            // wl_seat.release is the seat's own destructor, available from
            // version 5 (wayland.xml in wayland-client's source: `release`
            // `since=5`; the bind range in `bind_tables` never goes below 5).
            seat.release();
        }
        self.seat = None;
    }

    fn send(&self, out: Vec<PenEvent>) {
        for ev in out {
            // The receiving app dying is not the sidecar's crash: a background
            // pen pipeline outliving the window just drops its events.
            let _ = self.tx.send(ev);
        }
    }
}

/// The registry: `registry_queue_init` populated the list itself and forwards any
/// post-roundtrip globals here — the sidecar bound what it needed at spawn, so this
/// is deliberately passive (tightening later is a Task 1 question).
impl Dispatch<WlRegistry, GlobalListContents> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

/// The seat: the sidecar never asks the seat anything beyond its identity — wl_pointer
/// emulation from winit is exactly what stays theirs (Task 0 question 4 observes it,
/// it does not interfere here).
impl Dispatch<WlSeat, ()> for AppState {
    fn event(_state: &mut Self, _proxy: &WlSeat, _event: wl_seat::Event, _data: &(), _conn: &Connection, _qh: &QueueHandle<Self>) {}
}

/// The manager interface has no events at all (tablet-v2.xml: requests only); this
/// impl exists because `GlobalList::bind` wires the bound object's (empty) data onto
/// this queue's registry of `Dispatch` impls.
impl Dispatch<ZwpTabletManagerV2, ()> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpTabletManagerV2,
        _event: zwp_tablet_manager_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTabletPadV2, ()> for AppState {
    // The pad's `group` event carries `new_id` (tablet-v2.xml `zwp_tablet_pad_v2.group`:
    // "pad_group" — the announce of the pad's button group). Like the seat's `*_added`
    // events above, the child's user data is initialized through
    // `Dispatch::event_created_child`, whose default panics — so this specialization is
    // required, not optional (same reason as the seat impl above): a tablet whose pad
    // is announced would abort on the pad's first `group`.
    event_created_child!(AppState, ZwpTabletPadV2, [
        zwp_tablet_pad_v2::EVT_GROUP_OPCODE => (ZwpTabletPadGroupV2, ()),
    ]);

    // Pads stay unsupported: the pad's events carry no pen input and are ignored,
    // but the pad, its groups and their axis children are all recorded, so the
    // mandated teardown order can be honoured on `removed` and at shutdown.
    fn event(state: &mut Self, proxy: &ZwpTabletPadV2, event: zwp_tablet_pad_v2::Event, _data: &(), _conn: &Connection, _qh: &QueueHandle<Self>) {
        match event {
            // The pad's button group arrives as `new_id` (tablet-v2.xml
            // `zwp_tablet_pad_v2.group`); recorded for teardown.
            zwp_tablet_pad_v2::Event::Group { pad_group } => {
                if let Some(pad) = state.pads.get_mut(&proxy.id()) {
                    pad.groups.push(pad_group);
                }
            }
            // The removal mandates an ordered destroy (tablet-v2.xml
            // `zwp_tablet_pad_v2.removed`): "the client must destroy all rings,
            // strips and groups that were offered by this pad, and issue
            // zwp_tablet_pad_v2.destroy the pad itself."
            zwp_tablet_pad_v2::Event::Removed => {
                if let Some(mut pad) = state.pads.remove(&proxy.id()) {
                    pad.destroy();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpTabletPadGroupV2, ()> for AppState {
    // The group's `ring`/`strip`/`dial` events each carry `new_id` (tablet-v2.xml
    // `zwp_tablet_pad_group_v2.ring`/`strip`/`dial`) — same `event_created_child`
    // requirement as above. `dial` exists since version 2 (the XML's `since="2"`); the
    // opcode constant is generated regardless of the bound version, and covering an
    // event that a version-1 compositor never sends costs nothing.
    event_created_child!(AppState, ZwpTabletPadGroupV2, [
        zwp_tablet_pad_group_v2::EVT_RING_OPCODE => (ZwpTabletPadRingV2, ()),
        zwp_tablet_pad_group_v2::EVT_STRIP_OPCODE => (ZwpTabletPadStripV2, ()),
        zwp_tablet_pad_group_v2::EVT_DIAL_OPCODE => (ZwpTabletPadDialV2, ()),
    ]);

    fn event(
        state: &mut Self,
        proxy: &ZwpTabletPadGroupV2,
        event: zwp_tablet_pad_group_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // The group's axis children are recorded onto the pad that announced the
        // group, so teardown destroys them before the group (see
        // `PadRecord::destroy`). A group whose pad record is already gone (pad
        // `removed` destroys everything it offered at once) has nowhere to record —
        // and no teardown left to do: that destroy covered it.
        let child = match event {
            zwp_tablet_pad_group_v2::Event::Ring { ring } => PadChild::Ring(ring),
            zwp_tablet_pad_group_v2::Event::Strip { strip } => PadChild::Strip(strip),
            zwp_tablet_pad_group_v2::Event::Dial { dial } => PadChild::Dial(dial),
            _ => return,
        };
        if let Some(pad) = state.pads.values_mut().find(|pad| pad.holds_group(&proxy.id())) {
            pad.children.push(child);
        }
    }
}

impl Dispatch<ZwpTabletPadRingV2, ()> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpTabletPadRingV2,
        _event: zwp_tablet_pad_ring_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTabletPadStripV2, ()> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpTabletPadStripV2,
        _event: zwp_tablet_pad_strip_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTabletPadDialV2, ()> for AppState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpTabletPadDialV2,
        _event: zwp_tablet_pad_dial_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTabletSeatV2, ()> for AppState {
    // The seat's three events all carry `new_id` arguments (tablet-v2.xml
    // `tablet_added`/`tool_added`/`pad_added`), and wayland-client initializes each
    // new object's user data *before* dispatching that event — through
    // `Dispatch::event_created_child`. The default panics
    // (wayland-client-0.31 `src/event_queue.rs`: "Missing event_created_child
    // specialization for event opcode 0 of zwp_tablet_seat_v2"), so this
    // specialization is required, not optional: without it the first
    // `tablet_added` aborts the process inside the backend's non-unwinding
    // dispatcher. The generated opcode constants are cited verbatim
    // (wayland-scanner names them `EVT_<EVENT>_OPCODE`,
    // wayland-scanner-0.31 `src/common.rs:140`), and the child proxies get the
    // same `()` user data their `Dispatch` impls below use.
    event_created_child!(AppState, ZwpTabletSeatV2, [
        zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE => (ZwpTabletV2, ()),
        zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE => (ZwpTabletToolV2, ()),
        zwp_tablet_seat_v2::EVT_PAD_ADDED_OPCODE => (ZwpTabletPadV2, ()),
    ]);

    fn event(state: &mut Self, _proxy: &ZwpTabletSeatV2, event: zwp_tablet_seat_v2::Event, _data: &(), _conn: &Connection, _qh: &QueueHandle<Self>) {
        match event {
            // tablet_added carries the new zwp_tablet_v2 as `new_id` (tablet-v2.xml);
            // the bindings materialize it with this queue's data.
            zwp_tablet_seat_v2::Event::TabletAdded { id } => {
                state.tablets.insert(id.id(), id);
            }
            zwp_tablet_seat_v2::Event::ToolAdded { id } => {
                state.tools.insert(id.id(), ToolState::new(Some(id)));
            }
            // Pads (the pad ring/strip buttons of some tablets) carry no pen input,
            // but they are protocol objects on this guest connection: kept in a
            // [`PadRecord`], so their `removed` and the sidecar's shutdown can
            // destroy them in the mandated order instead of orphaning them.
            zwp_tablet_seat_v2::Event::PadAdded { id } => {
                state.pads.insert(id.id(), PadRecord::new(id));
            }
            // Non-exhaustive by generation; anything else later is ignored here —
            // seat-level objects only announce devices.
            _ => {}
        }
    }
}

impl Dispatch<ZwpTabletV2, ()> for AppState {
    fn event(state: &mut Self, proxy: &ZwpTabletV2, event: zwp_tablet_v2::Event, _data: &(), _conn: &Connection, _qh: &QueueHandle<Self>) {
        // The tablet's description (name, path, geometry, identification) is not read
        // in the spike; only removal matters (the object's tool needs no cleanup here —
        // its own `removed` arrives separately on the tool proxy, tablet-v2.xml
        // `zwp_tablet_v2.removed`: "When a tablet is removed, some tools may be
        // removed").
        if matches!(event, zwp_tablet_v2::Event::Removed) {
            // The removal mandates the destroy (tablet-v2.xml `zwp_tablet_v2.removed`):
            // "When this event is received, the client must zwp_tablet_v2.destroy the
            // object." Dropping the proxy alone would orphan it on this guest
            // connection.
            if proxy.is_alive() {
                proxy.destroy();
            }
            state.tablets.remove(&proxy.id());
        }
    }
}

impl Dispatch<ZwpTabletToolV2, ()> for AppState {
    fn event(state: &mut Self, proxy: &ZwpTabletToolV2, event: zwp_tablet_tool_v2::Event, _data: &(), _conn: &Connection, _qh: &QueueHandle<Self>) {
        let Some(tool) = state.tools.get_mut(&proxy.id()) else { return };
        let mut out = Vec::new();
        match event {
            zwp_tablet_tool_v2::Event::Type { tool_type } => {
                let kind = match tool_type {
                    WEnum::Value(value) => match value {
                        Type::Pen => ToolKind::Pen,
                        Type::Eraser => ToolKind::Eraser,
                        Type::Brush => ToolKind::Brush,
                        Type::Pencil => ToolKind::Pencil,
                        Type::Airbrush => ToolKind::Airbrush,
                        Type::Finger => ToolKind::Finger,
                        Type::Mouse => ToolKind::Mouse,
                        Type::Lens => ToolKind::Lens,
                        // Non-exhaustive enum; a value a future XML adds shows as
                        // `Unknown`, like an out-of-range wire value would.
                        _ => ToolKind::Unknown,
                    },
                    WEnum::Unknown(_) => ToolKind::Unknown,
                };
                tool.apply(RawTool::Type(kind), &mut out);
            }
            // Identification burst metadata the spike does not read. `done`
            // terminates it (tablet-v2.xml, quoted in the module docs) — not a
            // frame, nothing to emit.
            zwp_tablet_tool_v2::Event::HardwareSerial { .. } | zwp_tablet_tool_v2::Event::HardwareIdWacom { .. } => {}
            zwp_tablet_tool_v2::Event::Done => tool.apply(RawTool::Done, &mut out),
            zwp_tablet_tool_v2::Event::Capability { .. } => {}
            zwp_tablet_tool_v2::Event::Removed => {
                // The compositor removed the tool. The removal mandates the destroy
                // (tablet-v2.xml `zwp_tablet_tool_v2.removed`): "When this event is
                // received, the client must zwp_tablet_tool_v2.destroy the object" —
                // the object is gone server-side, but the client's resource is not:
                // dropping it without the request would orphan it on this guest
                // connection. `drop_proxy` sends it (and only it; destroying twice
                // would indeed be an error), before `apply` clears the proxy.
                tool.drop_proxy();
                tool.apply(RawTool::Removed, &mut out);
                state.tools.remove(&proxy.id());
            }
            zwp_tablet_tool_v2::Event::ProximityIn { serial, surface, .. } => {
                // The `wl_surface` is winit's object (question 3 of the plan: does
                // materializing it panic? — running with it here is the evidence);
                // only its raw protocol id is kept.
                let surface = surface.id().protocol_id();
                tool.apply(RawTool::ProximityIn { serial, surface: Some(surface) }, &mut out);
            }
            zwp_tablet_tool_v2::Event::ProximityOut => tool.apply(RawTool::ProximityOut, &mut out),
            zwp_tablet_tool_v2::Event::Down { serial } => {
                tool.apply(RawTool::Down { serial }, &mut out);
            }
            // `up` carries no serial (tablet-v2.xml `up`: no arguments).
            zwp_tablet_tool_v2::Event::Up => tool.apply(RawTool::Up, &mut out),
            zwp_tablet_tool_v2::Event::Motion { x, y } => {
                tool.apply(RawTool::Motion { x, y }, &mut out);
            }
            zwp_tablet_tool_v2::Event::Pressure { pressure } => {
                tool.apply(RawTool::Pressure { value: pressure }, &mut out);
            }
            zwp_tablet_tool_v2::Event::Distance { .. } | zwp_tablet_tool_v2::Event::Rotation { .. } => {}
            zwp_tablet_tool_v2::Event::Slider { .. } | zwp_tablet_tool_v2::Event::Wheel { .. } => {}
            zwp_tablet_tool_v2::Event::Tilt { tilt_x, tilt_y } => {
                tool.apply(RawTool::Tilt { x: tilt_x, y: tilt_y }, &mut out);
            }
            zwp_tablet_tool_v2::Event::Button { button, state: button_state, .. } => {
                // The state enum only has the two values (tablet-v2.xml `button_state`);
                // an out-of-range value is dropped with the frame — nothing to say.
                // (The generated enum is non-exhaustive: unknown values arrive.)
                let pressed = match button_state {
                    WEnum::Value(value) => match value {
                        ButtonState::Pressed => true,
                        ButtonState::Released => false,
                        _ => return,
                    },
                    WEnum::Unknown(_) => return,
                };
                tool.apply(RawTool::Button { button, pressed }, &mut out);
            }
            zwp_tablet_tool_v2::Event::Frame { time } => {
                tool.apply(RawTool::Frame(time), &mut out);
            }
            // Non-exhaustive by generation: a protocol addition this XML does not
            // know about lands here (the bound version keeps it empty in practice).
            _ => {}
        }
        state.send(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the pure state machine and collect what closed.
    fn run(state: &mut ToolState, events: &[RawTool]) -> Vec<PenEvent> {
        let mut out = Vec::new();
        for ev in events {
            state.apply(*ev, &mut out);
        }
        out
    }

    fn proximity() -> RawTool {
        RawTool::ProximityIn { serial: 42, surface: Some(7) }
    }

    /// The canonical stroke frame: prox_in, motion, pressure, down — the lossless
    /// fold emits the entry, the press and the move, in that order, each carrying
    /// the frame's state and the frame's own timestamp.
    #[test]
    fn a_stroke_frame_delivers_the_entry_the_press_and_the_move() {
        let mut tool = ToolState::synthetic();
        run(
            &mut tool,
            &[
                RawTool::Type(ToolKind::Pen),
                proximity(),
                RawTool::Motion { x: 12.5, y: 3.25 },
                RawTool::Pressure { value: 32768 },
                RawTool::Down { serial: 43 },
            ],
        );
        let out = run(&mut tool, &[RawTool::Frame(1000)]);
        assert_eq!(out.len(), 3, "entry, press, move — one per transition: {out:?}");
        assert!(matches!(&out[0], PenEvent::ProximityIn { .. }), "{out:?}");
        assert!(matches!(&out[1], PenEvent::Down { .. }), "the press after the entry: {out:?}");
        assert!(matches!(&out[2], PenEvent::Move { .. }), "{out:?}");
        let frame = out[1].frame();
        assert_eq!(frame.tool, ToolKind::Pen);
        assert_eq!(frame.surface, Some(7));
        assert_eq!(frame.x, 12.5);
        assert_eq!(frame.y, 3.25);
        assert_eq!(frame.pressure, f64::from(32768) / 65535.0, "normalized 0..1 from the protocol's 0..65535 (tablet-v2.xml `pressure`)");
        assert_eq!(frame.time_ms, 1000, "the `frame` event's own time");
        assert_eq!(out[1].serial(), Some(43), "the `down` event's serial");
    }

    /// Continual pressure while stationary (no motion) still reports: the axes flag
    /// keeps the frame observable.
    #[test]
    fn a_pressure_only_frame_still_reports() {
        let mut tool = ToolState::synthetic();
        let out = run(
            &mut tool,
            &[
                proximity(),
                RawTool::Down { serial: 1 },
                RawTool::Frame(10),
                // A frame later: the pen presses harder, unmoved.
                RawTool::Pressure { value: 65535 },
                RawTool::Frame(30),
            ],
        );
        assert_eq!(out.len(), 3, "entry, press, then the pressure-only frame: {out:?}");
        assert!(matches!(&out[1], PenEvent::Down { .. }), "{out:?}");
        assert!(matches!(&out[2], PenEvent::Move { .. }), "an axis-only frame lands as Move: {out:?}",);
        assert_eq!(out[2].frame().pressure, 1.0);
        assert_eq!(out[2].frame().time_ms, 30);
    }

    /// Lifting and leaving: up and proximity_out keep their own events.
    #[test]
    fn an_up_frame_makes_one_up() {
        let mut tool = ToolState::synthetic();
        run(&mut tool, &[proximity(), RawTool::Motion { x: 5.0, y: 6.0 }, RawTool::Down { serial: 2 }, RawTool::Frame(100)]);
        let out = run(&mut tool, &[RawTool::Up, RawTool::Frame(150)]);
        assert_eq!(out.len(), 1, "{out:?}");
        // `up` carries no serial (tablet-v2.xml `up`: no arguments) — the event is
        // the lift, unadorned.
        assert!(matches!(&out[0], PenEvent::Up { .. }), "{out:?}");
        let out = run(&mut tool, &[RawTool::ProximityOut, RawTool::Frame(200)]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(matches!(&out[0], PenEvent::ProximityOut { .. }), "{out:?}");
    }

    /// Hover with no contact: the entry frame is the `proximity_in`; the following
    /// frames are Moves, pressure 0, the surface remembered.
    #[test]
    fn a_hover_frame_is_a_move() {
        let mut tool = ToolState::synthetic();
        let out = run(&mut tool, &[proximity(), RawTool::Frame(10)]);
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], PenEvent::ProximityIn { .. }), "the entry frame is the +in: {out:?}");
        let out = run(&mut tool, &[RawTool::Motion { x: 1.0, y: 2.0 }, RawTool::Frame(50)]);
        assert_eq!(out.len(), 1);
        let PenEvent::Move { frame } = &out[0] else {
            panic!("hover is a Move: {out:?}");
        };
        assert_eq!(frame.pressure, 0.0);
        assert_eq!(frame.surface, Some(7));
    }

    /// A frame that carries nothing (the identification burst: `type`, `done`, and the
    /// burst's trailing beat) emits nothing.
    #[test]
    fn an_identification_burst_emits_nothing() {
        let mut tool = ToolState::synthetic();
        let out = run(&mut tool, &[RawTool::Type(ToolKind::Eraser), RawTool::Done, RawTool::Frame(0)]);
        assert_eq!(out, Vec::new(), "the burst ends with `done`, not `frame` (module docs); nothing observable");
        assert_eq!(tool.kind, ToolKind::Eraser);
    }

    /// The tilt/pressure numbers pass through in protocol units (degrees / 0..1).
    #[test]
    fn tilt_and_pressure_units() {
        let mut tool = ToolState::synthetic();
        let out = run(&mut tool, &[proximity(), RawTool::Tilt { x: -45.0, y: 30.0 }, RawTool::Pressure { value: 1000 }, RawTool::Frame(7)]);
        let frame = out[0].frame();
        assert_eq!(frame.tilt_x, -45.0, "degrees, as the protocol words them (tablet-v2.xml `tilt`)");
        assert_eq!(frame.tilt_y, 30.0);
        assert_eq!(frame.pressure, f64::from(1000) / 65535.0);
    }

    /// The protocol's pressure range: 0 empty, 65535 full, hostile clamped.
    #[test]
    fn pressure_clamps_beyond_the_protocol_range() {
        assert_eq!(normalize_pressure(0), 0.0);
        assert_eq!(normalize_pressure(65535), 1.0);
        assert_eq!(normalize_pressure(u32::MAX), 1.0, "clamped, never > 1, never a crash");
    }

    /// prox_in and down in one frame (a stroke starting the instant the pen lands):
    /// both arrive, entry first, and the Down's serial is the down's.
    #[test]
    fn prox_in_and_down_in_one_frame() {
        let mut tool = ToolState::synthetic();
        let out = run(&mut tool, &[RawTool::ProximityIn { serial: 9, surface: Some(3) }, RawTool::Down { serial: 10 }, RawTool::Frame(5)]);
        assert_eq!(out.len(), 2, "entry and press are distinct transitions: {out:?}");
        assert!(matches!(&out[0], PenEvent::ProximityIn { frame, .. } if frame.surface == Some(3)), "{out:?}");
        assert!(matches!(&out[1], PenEvent::Down { frame, .. } if frame.surface == Some(3)), "{out:?}");
        assert_eq!(out[1].serial(), Some(10));
    }

    /// A click shorter than a frame: down and up in one batch. Both arrive, in
    /// order — the press is not stuck and the click is not lost.
    #[test]
    fn a_click_shorter_than_a_frame_delivers_down_then_up() {
        let mut tool = ToolState::synthetic();
        run(&mut tool, &[proximity(), RawTool::Frame(10)]);
        let out = run(&mut tool, &[RawTool::Down { serial: 11 }, RawTool::Motion { x: 4.0, y: 4.0 }, RawTool::Up, RawTool::Frame(12)]);
        assert_eq!(out.len(), 3, "press, move, lift: {out:?}");
        assert!(matches!(&out[0], PenEvent::Down { serial: 11, .. }), "{out:?}");
        assert!(matches!(&out[1], PenEvent::Move { .. }), "the release lands where the frame left the pen: {out:?}");
        assert!(matches!(&out[2], PenEvent::Up { .. }), "{out:?}");
        // Pure tap, no motion in the frame: just the press and the lift.
        let out = run(&mut tool, &[RawTool::Down { serial: 12 }, RawTool::Up, RawTool::Frame(13)]);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(matches!(&out[0], PenEvent::Down { serial: 12, .. }), "{out:?}");
        assert!(matches!(&out[1], PenEvent::Up { .. }), "{out:?}");
    }

    /// A lift and a re-press inside one frame, with the tip already down entering
    /// it: the app must see Up before the re-press Down — the fixed canonical order
    /// would leave the app released while the pen is down.
    #[test]
    fn a_repress_frame_delivers_up_then_down() {
        let mut tool = ToolState::synthetic();
        run(&mut tool, &[proximity(), RawTool::Down { serial: 30 }, RawTool::Frame(10)]);
        let out = run(&mut tool, &[RawTool::Up, RawTool::Down { serial: 31 }, RawTool::Frame(11)]);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(matches!(&out[0], PenEvent::Up { .. }), "the lift first: {out:?}");
        assert!(matches!(&out[1], PenEvent::Down { serial: 31, .. }), "then the re-press: {out:?}");
        assert!(tool.contact, "the frame ends pressed");
    }

    /// A barrel-button change riding the same frame as the tip survives next to it
    /// (the old precedence dropped the button when down or up shared the frame).
    #[test]
    fn a_barrel_button_rides_the_tip_frame() {
        let mut tool = ToolState::synthetic();
        run(&mut tool, &[proximity(), RawTool::Frame(10)]);
        // Tip down and barrel press in one frame: both arrive, down first.
        let out = run(&mut tool, &[RawTool::Down { serial: 20 }, RawTool::Button { button: 0x14b, pressed: true }, RawTool::Frame(12)]);
        assert_eq!(out.len(), 2, "tip down and barrel press share the frame: {out:?}");
        assert!(matches!(&out[0], PenEvent::Down { serial: 20, .. }), "{out:?}");
        assert!(matches!(&out[1], PenEvent::Button { button: 0x14b, pressed: true, .. }), "{out:?}");
        // Same frame as the lift: the canonical order puts the button change before
        // the up, so the release is seen while the app still knows the press.
        let out = run(&mut tool, &[RawTool::Up, RawTool::Button { button: 0x14b, pressed: false }, RawTool::Frame(13)]);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(matches!(&out[0], PenEvent::Button { pressed: false, .. }), "button before the lift, in the canonical order: {out:?}");
        assert!(matches!(&out[1], PenEvent::Up { .. }), "{out:?}");
    }

    /// A tool that changes kind mid-life (the compositor's actual behaviour per
    /// tablet-v2.xml `type`: the same object can flip ends between frames): the next
    /// observable frame reports the new kind.
    #[test]
    fn a_kind_flip_arrives_with_the_next_frame() {
        let mut tool = ToolState::synthetic();
        run(&mut tool, &[RawTool::Type(ToolKind::Pen), proximity(), RawTool::Frame(1)]);
        let out = run(&mut tool, &[RawTool::Type(ToolKind::Eraser), RawTool::Motion { x: 1.0, y: 1.0 }, RawTool::Frame(2)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].frame().tool, ToolKind::Eraser, "the next frame reports the new kind");
    }
}
