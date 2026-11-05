//! W28: `zwp_tablet_v2` handling, backported from the winit master tablet input
//! (rust-windowing/winit#4318) with this fork's `platform_impl/wayland` module
//! layout and 0.30 event set (phase 1: tablet → mouse emulation, see
//! `emulation.rs`).
//!
//! Wire shape: `zwp_tablet_manager_v2` is bound once per connection, every
//! `wl_seat` gets one `zwp_tablet_seat_v2`, and compositor-created tools
//! (`tool_added`) dispatch their whole event stream
//! (`type`/`proximity_in`/`down`/`motion`/…/`removed`) on the tool object
//! itself. Everything the pen does is translated through the pure state machine
//! in `emulation.rs` and pushed into the same `events_sink` mouse events use —
//! no new application-visible event kinds — so consumers operate unchanged.
//!
//! Pads (`zwp_tablet_pad_v2` and their rings/strips/groups/dials) exist in the
//! protocol; phase 1 parks them: the bindings stay inert, but their
//! event-created children still get valid (empty) object data so a queued
//! `pad_added`/`group` burst cannot fail dispatching.
//!
//! Duplicate-input suppression: compositors that also drive a cursor from the
//! pen send a synthetic `wl_pointer` press/release pair while the tool is in
//! contact. Every seat counts its contacting tools
//! (`WinitSeatState::tablet_contact`) and the pointer handler drops
//! main-surface pointer presses and releases while that count is non-zero — the
//! tool dispatch has already produced the click.
//!
//! Tool metadata: the pointer events carry no tool kind, so the compositor's `type` report
//! (the eraser end among them) is published on the contact channel `crate::tablet` as it is
//! read back from the machine here: exactly one registration per contact, one give-back per
//! end, addressed so pressure reports and ends find their own contact. Pressure reports
//! follow the same pairing onto the channel's per-contact sample window; the application
//! erases what an `Eraser` contact's presses land on and reads the window beside the events
//! its frame translated — both decisions stay application ones.

use tracing::warn;

use sctk::globals::GlobalData;
use sctk::reexports::client::backend::ObjectId;
use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::protocol::wl_seat::WlSeat;
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use sctk::reexports::client::{
    event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum,
};

use crate::dpi::LogicalPosition;
use crate::event::{ElementState, WindowEvent};
use crate::platform_impl::wayland::state::WinitState;
use crate::platform_impl::wayland::{self, DeviceId, WindowId};
use crate::tablet::ContactId;

mod emulation;
mod protocol;

pub(crate) use emulation::{EmulatedPointerEvent, TabletToolEmu, TabletToolKind, TabletToolSignal};
use protocol::tablet_v2::zwp_tablet_manager_v2::ZwpTabletManagerV2;
use protocol::tablet_v2::zwp_tablet_pad_dial_v2::ZwpTabletPadDialV2;
use protocol::tablet_v2::zwp_tablet_pad_group_v2::{self as pad_group_iface, ZwpTabletPadGroupV2};
use protocol::tablet_v2::zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2;
use protocol::tablet_v2::zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2;
use protocol::tablet_v2::zwp_tablet_pad_v2::{self as pad_iface, ZwpTabletPadV2};
pub(crate) use protocol::tablet_v2::zwp_tablet_seat_v2::ZwpTabletSeatV2;
use protocol::tablet_v2::zwp_tablet_seat_v2::{self as seat_iface, Event as TabletSeatEvent};
use protocol::tablet_v2::zwp_tablet_tool_v2::{
    ButtonState, Event as ToolEvent, Type as ToolType, ZwpTabletToolV2,
};
use protocol::tablet_v2::zwp_tablet_v2::ZwpTabletV2;

/// The manager, bound once per connection.
pub struct TabletManagerState {
    manager: ZwpTabletManagerV2,
}

impl TabletManagerState {
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<WinitState>,
    ) -> Result<Self, BindError> {
        // `get_tablet_seat` is the only request both versions share, so holding
        // the manager at v1 keeps the seat API stable as well.
        let manager = globals.bind(queue_handle, 1..=1, GlobalData)?;
        Ok(Self { manager })
    }

    /// Bind one tablet seat for `seat`; stored on that seat's `WinitSeatState`.
    pub(crate) fn get_tablet_seat(
        &self,
        seat: &WlSeat,
        queue_handle: &QueueHandle<WinitState>,
        data: TabletSeatData,
    ) -> ZwpTabletSeatV2 {
        self.manager.get_tablet_seat(seat, queue_handle, data)
    }
}

/// One per-seat tablet binding's data. Kept on `WinitSeatState` the way
/// `relative_pointer` is: created when the seat appears, destroyed only when the
/// seat is removed — W2's lesson about destroying on capability changes holds
/// here as well (a tablet unplugging must not kill the binding).
pub(crate) struct TabletSeatData {
    /// The seat this tablet seat belongs to; tool events route through it.
    pub(crate) seat: WlSeat,
}

/// Per-tool state, one entry per known tool on a seat.
///
/// The Wayland side (focused surface) and the pure translation state live
/// together. Dispatching is `&mut self`-exclusive, so nothing locks the way
/// winit 0.31's object-data Mutexes do.
#[derive(Debug, Default)]
pub(crate) struct TabletToolState {
    /// The pure translation state (`emulation.rs`).
    emu: TabletToolEmu,
    /// The surface the tool is focused on; `proximity_in` sets it, routing the
    /// events after that clears it. Everything routes through this surface's
    /// window, top-level or not.
    surface: Option<WlSurface>,
    /// The channel id the tool's live contact registered under: pressure reports
    /// address their samples here, and the contact's end gives the record back.
    /// `None` while the tool merely hovers, and after the end.
    channel_contact: Option<ContactId>,
}

/// Feed one tool signal into its seat's state and push the resulting pointer
/// events.
pub(crate) fn tablet_signal(
    state: &mut WinitState,
    seat_id: &ObjectId,
    tool_id: &ObjectId,
    signal: TabletToolSignal,
) {
    let outputs;
    {
        let seat_state = match state.seats.get_mut(seat_id) {
            Some(seat_state) => seat_state,
            None => {
                warn!("Received tablet event for unknown seat");
                return;
            },
        };

        // The synthetic-pointer suppression counts raw protocol contact, not the
        // translated events: even a contact that cannot draw yet owns the
        // channel for its lifetime.
        match signal {
            TabletToolSignal::Contact => {
                seat_state.tablet_contact = seat_state.tablet_contact.saturating_add(1);
            },
            TabletToolSignal::Lift | TabletToolSignal::Leave | TabletToolSignal::Removed => {
                seat_state.tablet_contact = seat_state.tablet_contact.saturating_sub(1);
            },
            _ => (),
        }

        let tool_state = match seat_state.tablet_tools.get_mut(tool_id) {
            Some(tool_state) => tool_state,
            None => {
                warn!("Received tablet event for unknown tool");
                return;
            },
        };

        // `Leave`/`Removed` must still deliver their `CursorLeft`/releases to the
        // window the pen hovered, so the surface is drained only after the
        // translation state has spoken.
        let routing_surface =
            if matches!(signal, TabletToolSignal::Leave | TabletToolSignal::Removed) {
                tool_state.surface.take()
            } else {
                tool_state.surface.clone()
            };
        // The contact channel pairs each contact with one end, anchored on what the machine
        // itself reports about the tool's contact: a contact the machine takes is registered
        // under a fresh channel id, one it releases gives that record back, and a tool that
        // merely hovers (or whose end came unpaired, against protocol) moves nothing.
        let (contacted_before, kind, channel_contact) =
            (tool_state.emu.contact_live(), tool_state.emu.kind(), tool_state.channel_contact);
        outputs = (tool_state.emu.observe(signal), routing_surface);
        match (contacted_before, tool_state.emu.contact_live()) {
            (false, true) => {
                tool_state.channel_contact =
                    Some(crate::tablet::contact_started(kind == TabletToolKind::Eraser));
            },
            (true, false) => {
                if let Some(id) = channel_contact {
                    crate::tablet::contact_ended(id);
                }
                tool_state.channel_contact = None;
            },
            _ => (),
        }
        // A pressure report needs its contact id, so it addresses the sample window it
        // belongs to; a report whose contact has no record (hovering, or already ended
        // against protocol) finds none and the channel leaves it alone.
        if let (TabletToolSignal::Pressure(pressure), Some(id)) =
            (signal, tool_state.channel_contact)
        {
            // The spec's value is `0`–`65535`; a value above that is a compositor speaking out
            // of spec, and it clamps instead of normalizing into a force above full.
            let force = f64::from(pressure.min(u32::from(u16::MAX))) / f64::from(u16::MAX);
            crate::tablet::contact_pressure(id, crate::event::Force::Normalized(force));
        }
    }

    let (outputs, surface) = outputs;

    // Route everything to the tool's focused window, when there is one left.
    let Some(surface) = surface else { return };
    let Some((window_id, scale_factor)) = tablet_window(state, &surface) else { return };
    let device_id = crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(DeviceId));
    for output in outputs {
        let window_event = match output {
            EmulatedPointerEvent::Entered => WindowEvent::CursorEntered { device_id },
            EmulatedPointerEvent::Moved(position) => {
                WindowEvent::CursorMoved { device_id, position: position.to_physical(scale_factor) }
            },
            EmulatedPointerEvent::Left => WindowEvent::CursorLeft { device_id },
            EmulatedPointerEvent::Press(button) => {
                WindowEvent::MouseInput { device_id, state: ElementState::Pressed, button }
            },
            EmulatedPointerEvent::Release(button) => {
                WindowEvent::MouseInput { device_id, state: ElementState::Released, button }
            },
        };
        state.events_sink.push_window_event(window_event, window_id);
    }
}

/// The top-level window a focused surface belongs to and its scale factor.
fn tablet_window(state: &WinitState, surface: &WlSurface) -> Option<(WindowId, f64)> {
    let window_id = wayland::make_wid(surface);
    let scale_factor = {
        let mut windows = state.windows.borrow_mut();
        let window = windows.get_mut(&window_id)?;
        let window = window.lock().ok()?;
        window.scale_factor()
    };
    Some((window_id, scale_factor))
}

// The dispatch impls are on `WinitState` directly, the way keyboard and touch
// are; the queue resolves them per proxy/udata pair, no delegation needed.

impl Dispatch<ZwpTabletManagerV2, GlobalData, WinitState> for WinitState {
    fn event(
        _: &mut WinitState,
        _: &ZwpTabletManagerV2,
        _: <ZwpTabletManagerV2 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        // zwp_tablet_manager_v2 defines no events in v1 or v2.
    }
}

impl Dispatch<ZwpTabletSeatV2, TabletSeatData, WinitState> for WinitState {
    event_created_child!(WinitState, ZwpTabletSeatV2, [
        seat_iface::EVT_TABLET_ADDED_OPCODE => (ZwpTabletV2, ()),
        seat_iface::EVT_TOOL_ADDED_OPCODE => (ZwpTabletToolV2, ()),
        seat_iface::EVT_PAD_ADDED_OPCODE => (ZwpTabletPadV2, ()),
    ]);

    fn event(
        state: &mut WinitState,
        _: &ZwpTabletSeatV2,
        event: TabletSeatEvent,
        data: &TabletSeatData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        let seat_id = data.seat.id();
        match event {
            // A new tool enters the registry; its object dispatches all input.
            TabletSeatEvent::ToolAdded { id } => {
                let _ = state.tablet_tool_seats.insert(id.id(), seat_id.clone());
                if let Some(seat_state) = state.seats.get_mut(&seat_id) {
                    let _ = seat_state.tablet_tools.insert(id.id(), TabletToolState::default());
                }
            },
            // Device name/serial metadata stays parked with phase 2.
            TabletSeatEvent::TabletAdded { .. } => (),
            // Pads are parked; their objects still dispatch (inertly).
            TabletSeatEvent::PadAdded { .. } => (),
        }
    }
}

impl Dispatch<ZwpTabletV2, (), WinitState> for WinitState {
    fn event(
        _: &mut WinitState,
        _: &ZwpTabletV2,
        _: <ZwpTabletV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        // Tablet device metadata (name/vid/pid/bustype) stays parked with phase 2.
    }
}

impl Dispatch<ZwpTabletToolV2, (), WinitState> for WinitState {
    fn event(
        state: &mut WinitState,
        tool: &ZwpTabletToolV2,
        event: ToolEvent,
        _: &(),
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        let seat_id = match state.tablet_tool_seats.get(&tool.id()) {
            Some(seat_id) => seat_id.clone(),
            None => return,
        };

        match event {
            ToolEvent::Type { tool_type: WEnum::Value(tool_type) } => {
                let kind = match tool_type {
                    ToolType::Pen => TabletToolKind::Pen,
                    ToolType::Eraser => TabletToolKind::Eraser,
                    ToolType::Brush => TabletToolKind::Brush,
                    ToolType::Pencil => TabletToolKind::Pencil,
                    ToolType::Airbrush => TabletToolKind::Airbrush,
                    ToolType::Finger => TabletToolKind::Finger,
                    ToolType::Mouse => TabletToolKind::Mouse,
                    ToolType::Lens => TabletToolKind::Lens,
                };
                tablet_signal(state, &seat_id, &tool.id(), TabletToolSignal::Type(kind));
            },
            // The identification burst's serials and capability promises don't
            // matter in phase 1: contact and motion drive everything.
            ToolEvent::HardwareSerial { .. }
            | ToolEvent::HardwareIdWacom { .. }
            | ToolEvent::Capability { .. }
            | ToolEvent::Done => (),

            ToolEvent::ProximityIn { surface, .. } => {
                set_tool_surface(state, &seat_id, &tool.id(), surface);
                tablet_signal(state, &seat_id, &tool.id(), TabletToolSignal::Enter);
            },
            ToolEvent::ProximityOut => {
                tablet_signal(state, &seat_id, &tool.id(), TabletToolSignal::Leave);
            },
            ToolEvent::Down { .. } => {
                tablet_signal(state, &seat_id, &tool.id(), TabletToolSignal::Contact);
            },
            ToolEvent::Up => {
                tablet_signal(state, &seat_id, &tool.id(), TabletToolSignal::Lift);
            },
            ToolEvent::Motion { x, y } => {
                tablet_signal(
                    state,
                    &seat_id,
                    &tool.id(),
                    TabletToolSignal::Motion(LogicalPosition::new(x, y)),
                );
            },
            // Axis updates are dropped in the translation; pressure additionally feeds its
            // contact's sample window (see `crate::tablet`).
            ToolEvent::Pressure { pressure } => {
                tablet_signal(state, &seat_id, &tool.id(), TabletToolSignal::Pressure(pressure));
            },
            ToolEvent::Distance { .. }
            | ToolEvent::Tilt { .. }
            | ToolEvent::Rotation { .. }
            | ToolEvent::Slider { .. }
            | ToolEvent::Wheel { .. } => (),
            // No `frame` batching: the protocol order is already the mouse order.
            ToolEvent::Frame { .. } => (),

            ToolEvent::Button { button, state: WEnum::Value(button_state), .. } => {
                let pressed = match button_state {
                    ButtonState::Pressed => 1,
                    ButtonState::Released => 0,
                };
                tablet_signal(state, &seat_id, &tool.id(), TabletToolSignal::Button {
                    button,
                    state: pressed,
                });
            },
            ToolEvent::Button { .. } => (),

            ToolEvent::Removed => {
                tablet_signal(state, &seat_id, &tool.id(), TabletToolSignal::Removed);
                drop_tool(state, &seat_id, &tool.id());
                // The client must destroy the tool object once `removed` lands.
                tool.destroy();
            },
            _ => (),
        }
    }
}

/// Record the tool's focused surface before an `Enter` is translated.
fn set_tool_surface(
    state: &mut WinitState,
    seat_id: &ObjectId,
    tool_id: &ObjectId,
    surface: WlSurface,
) {
    if let Some(seat_state) = state.seats.get_mut(seat_id) {
        if let Some(tool_state) = seat_state.tablet_tools.get_mut(tool_id) {
            tool_state.surface = Some(surface);
        }
    }
}

/// Drop a tool's registry entries; called after `removed`.
fn drop_tool(state: &mut WinitState, seat_id: &ObjectId, tool_id: &ObjectId) {
    let _ = state.tablet_tool_seats.remove(tool_id);
    if let Some(seat_state) = state.seats.get_mut(seat_id) {
        let _ = seat_state.tablet_tools.remove(tool_id);
    }
}

// —— Inert pad-family bindings: they exist so no queued pad event ever finds a
// missing dispatch target; all of their input stays parked with phase 2. ——

impl Dispatch<ZwpTabletPadV2, (), WinitState> for WinitState {
    event_created_child!(WinitState, ZwpTabletPadV2, [
        pad_iface::EVT_GROUP_OPCODE => (ZwpTabletPadGroupV2, ()),
    ]);

    fn event(
        _: &mut WinitState,
        _: &ZwpTabletPadV2,
        _: <ZwpTabletPadV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        // Pad inputs stay parked with phase 2.
    }
}

impl Dispatch<ZwpTabletPadGroupV2, (), WinitState> for WinitState {
    event_created_child!(WinitState, ZwpTabletPadGroupV2, [
        pad_group_iface::EVT_RING_OPCODE => (ZwpTabletPadRingV2, ()),
        pad_group_iface::EVT_STRIP_OPCODE => (ZwpTabletPadStripV2, ()),
        pad_group_iface::EVT_DIAL_OPCODE => (ZwpTabletPadDialV2, ()),
    ]);

    fn event(
        _: &mut WinitState,
        _: &ZwpTabletPadGroupV2,
        _: <ZwpTabletPadGroupV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        // Pad inputs stay parked with phase 2.
    }
}

impl Dispatch<ZwpTabletPadRingV2, (), WinitState> for WinitState {
    fn event(
        _: &mut WinitState,
        _: &ZwpTabletPadRingV2,
        _: <ZwpTabletPadRingV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        // Pad inputs stay parked with phase 2.
    }
}

impl Dispatch<ZwpTabletPadStripV2, (), WinitState> for WinitState {
    fn event(
        _: &mut WinitState,
        _: &ZwpTabletPadStripV2,
        _: <ZwpTabletPadStripV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        // Pad inputs stay parked with phase 2.
    }
}

impl Dispatch<ZwpTabletPadDialV2, (), WinitState> for WinitState {
    fn event(
        _: &mut WinitState,
        _: &ZwpTabletPadDialV2,
        _: <ZwpTabletPadDialV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        // Pad inputs stay parked with phase 2.
    }
}
