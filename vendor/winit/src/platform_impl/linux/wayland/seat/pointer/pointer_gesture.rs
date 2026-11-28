//! PdfCraft patch (#1095): pinch support adapted from rust-windowing/winit#4338.
//! Only pinch is backported; scale deltas use the logarithmic units egui-winit consumes.

use std::sync::Mutex;

use sctk::compositor::SurfaceData;
use sctk::globals::GlobalData;
use wayland_client::globals::{BindError, GlobalList};
use wayland_client::{delegate_dispatch, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gesture_pinch_v1::{Event, ZwpPointerGesturePinchV1};
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gestures_v1::ZwpPointerGesturesV1;

use crate::event::WindowEvent;
use crate::platform_impl::wayland::state::WinitState;
use crate::platform_impl::wayland::{make_wid, DeviceId, WindowId};

use super::pinch_state::{PinchEvent, PinchState};

pub struct PointerGesturesState(pub ZwpPointerGesturesV1);

impl PointerGesturesState {
    pub fn new(globals: &GlobalList, queue_handle: &QueueHandle<WinitState>) -> Result<Self, BindError> {
        globals.bind(queue_handle, 1..=1, GlobalData).map(Self)
    }
}

#[derive(Debug, Default)]
pub struct PointerGestureData(Mutex<PinchState<WindowId>>);

fn emit(state: &mut WinitState, event: Option<PinchEvent<WindowId>>) {
    if let Some(event) = event {
        // The window may have closed while a gesture was in progress.
        if state.windows.get_mut().contains_key(&event.window) {
            state.events_sink.push_window_event(
                WindowEvent::PinchGesture {
                    device_id: crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(DeviceId)),
                    delta: event.delta,
                    phase: event.phase,
                },
                event.window,
            );
        }
    }
}

impl PointerGestureData {
    pub fn cancel(&self, state: &mut WinitState) {
        if let Ok(mut pinch) = self.0.lock() {
            emit(state, pinch.end(true));
        }
    }
}

impl Dispatch<ZwpPointerGesturesV1, GlobalData, WinitState> for PointerGesturesState {
    fn event(
        _: &mut WinitState,
        _: &ZwpPointerGesturesV1,
        _: <ZwpPointerGesturesV1 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        // This interface has no events.
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, PointerGestureData, WinitState> for PointerGesturesState {
    fn event(
        state: &mut WinitState,
        _: &ZwpPointerGesturePinchV1,
        event: Event,
        data: &PointerGestureData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        let Ok(mut pinch) = data.0.lock() else { return };
        let event = match event {
            Event::Begin { surface, fingers, .. } => {
                // Decoration/subsurface gestures must not zoom the document.
                let window = surface.data::<SurfaceData>().filter(|data| data.parent_surface().is_none()).map(|_| make_wid(&surface));
                pinch.begin(window, fingers)
            }
            Event::Update { scale, .. } => pinch.update(scale),
            Event::End { cancelled, .. } => pinch.end(cancelled != 0),
            _ => None,
        };
        emit(state, event);
    }
}

delegate_dispatch!(WinitState: [ZwpPointerGesturesV1: GlobalData] => PointerGesturesState);
delegate_dispatch!(WinitState: [ZwpPointerGesturePinchV1: PointerGestureData] => PointerGesturesState);
