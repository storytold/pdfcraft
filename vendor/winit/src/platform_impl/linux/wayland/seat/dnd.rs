//! PdfCraft patch: receive local files through the core Wayland data-device protocol (#634).
//!
//! Preserve winit 0.30's file events. Unlike SCTK 0.19's data-device delegation, this receiver
//! owns offers until their asynchronous reads finish, even after another drag enters. No clipboard
//! implementation is installed: selection offers on this connection are discarded.

use std::collections::VecDeque;
use std::io;
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ahash::AHashMap;
use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{Interest, Mode, PostAction, RegistrationToken};
use rustix::pipe::{pipe_with, PipeFlags};
use sctk::compositor::SurfaceData;
use sctk::globals::GlobalData;
use sctk::reexports::client::backend::ObjectId;
use sctk::reexports::client::globals::GlobalList;
use sctk::reexports::client::protocol::wl_data_device::{self, WlDataDevice};
use sctk::reexports::client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use sctk::reexports::client::protocol::wl_data_offer::{self, WlDataOffer};
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use sctk::reexports::client::{
    event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum,
};
use tracing::warn;

use crate::dpi::{LogicalPosition, PhysicalPosition};
use crate::event::{Event, WindowEvent};
use crate::platform_impl::wayland::event_loop::sink::EventSink;
use crate::platform_impl::wayland::state::WinitState;
use crate::platform_impl::wayland::window::WindowRequests;
use crate::platform_impl::wayland::{self, DeviceId, WindowId};

const URI_LIST: &str = "text/uri-list";
const MAX_URI_LIST: usize = 1024 * 1024;
const MAX_FILES: usize = 1024;
const MAX_OFFERS: usize = 16;
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30);

fn is_content_surface(surface: &WlSurface) -> bool {
    // Enter/Motion coordinates are relative to the entered surface. Decoration subsurfaces
    // cannot use content coordinates without frame-specific translation, so reject their offers.
    surface
        .data::<SurfaceData>()
        .and_then(|data| data.parent_surface())
        .map_or(true, |parent| parent.id() == surface.id())
}

#[derive(Default)]
pub struct DndState {
    pub manager: Option<WlDataDeviceManager>,
    offers: AHashMap<ObjectId, Offer>,
    ready: VecDeque<ObjectId>,
    cursors: AHashMap<WindowId, WindowEvent>,
    hovered: AHashMap<WindowId, Vec<PathBuf>>,
}

impl DndState {
    pub fn new(globals: &GlobalList, qh: &QueueHandle<WinitState>) -> Self {
        Self { manager: globals.bind(qh, 1..=3, GlobalData).ok(), ..Self::default() }
    }

    fn active(&self, device: &WlDataDevice) -> Option<ObjectId> {
        self.offers.iter().find_map(|(id, offer)| {
            (offer.device == device.id() && offer.window.is_some() && !offer.data.dropped)
                .then(|| id.clone())
        })
    }

    pub(crate) fn record_cursor(&mut self, event: &Event<()>) {
        if let Event::WindowEvent { window_id, event } = event {
            if matches!(event, WindowEvent::CursorMoved { .. } | WindowEvent::CursorLeft { .. }) {
                self.cursors.insert(window_id.0, event.clone());
            }
        }
    }

    pub(crate) fn has_ready_drop(&self, window: WindowId) -> bool {
        self.ready
            .iter()
            .any(|id| self.offers.get(id).is_some_and(|offer| offer.window == Some(window)))
    }

    fn queue_drop(&mut self, id: &ObjectId, requests: &WindowRequests) -> bool {
        let Some(offer) = self.offers.get(id) else { return false };
        if !offer.data.dropped || offer.data.paths.is_none() {
            return false;
        }
        if !self.ready.contains(id) {
            self.ready.push_back(id.clone());
        }
        // If a frame callback is outstanding, its handler only wakes the loop for a redraw request.
        requests.redraw_requested.store(true, Ordering::Relaxed);
        true
    }

    fn take_drop(&mut self, window: WindowId, scale: f64) -> Option<(ObjectId, Option<DropBatch>)> {
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let index = self
            .ready
            .iter()
            .position(|id| self.offers.get(id).is_some_and(|offer| offer.window == Some(window)))?;
        let id = self.ready.get(index)?.clone();
        let offer = self.offers.get_mut(&id)?;
        if !offer.copy_selected() {
            // Return rejected offers too so WinitState runs the full dnd_remove cleanup.
            return Some((id, None));
        }
        let position = offer.position?;
        let paths = offer.data.take_drop()?;
        self.ready.remove(index);
        let device_id = crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(DeviceId));
        let restore =
            self.cursors.get(&window).cloned().unwrap_or(WindowEvent::CursorLeft { device_id });
        Some((id, Some(DropBatch { position: position.to_physical(scale), paths, restore })))
    }

    fn publish_hover(
        &mut self,
        window: WindowId,
        paths: Vec<PathBuf>,
        sink: &mut EventSink,
    ) -> bool {
        let previous = self.hovered.get(&window).map(Vec::as_slice).unwrap_or_default();
        if previous == paths {
            return false;
        }
        if !previous.is_empty() {
            sink.push_window_event(WindowEvent::HoveredFileCancelled, window);
        }
        for path in &paths {
            sink.push_window_event(WindowEvent::HoveredFile(path.clone()), window);
        }
        if paths.is_empty() {
            self.hovered.remove(&window);
        } else {
            self.hovered.insert(window, paths);
        }
        true
    }
}

/// Winit 0.30's DroppedFile carries no position. Keep its saved cursor until the application's
/// redraw has consumed this batch; then restore the current cursor, including a newer drag.
pub(crate) struct DropBatch {
    position: PhysicalPosition<f64>,
    paths: Vec<PathBuf>,
    restore: WindowEvent,
}

impl DropBatch {
    pub(crate) fn dispatch(self, mut emit: impl FnMut(WindowEvent)) {
        let device_id = crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(DeviceId));
        emit(WindowEvent::CursorMoved { device_id, position: self.position });
        for path in self.paths {
            emit(WindowEvent::DroppedFile(path));
        }
        emit(WindowEvent::RedrawRequested);
        emit(self.restore);
    }
}

struct Offer {
    proxy: WlDataOffer,
    device: ObjectId,
    uri_list: bool,
    source_actions: DndAction,
    action: DndAction,
    window: Option<WindowId>,
    position: Option<LogicalPosition<f64>>,
    data: TransferData,
    reader: Option<RegistrationToken>,
    timeout: Option<RegistrationToken>,
}

impl Offer {
    fn new(proxy: WlDataOffer, device: ObjectId) -> Self {
        Self {
            proxy,
            device,
            uri_list: false,
            source_actions: DndAction::empty(),
            action: DndAction::empty(),
            window: None,
            position: None,
            data: TransferData::default(),
            reader: None,
            timeout: None,
        }
    }

    fn copy_selected(&self) -> bool {
        // Before protocol version 3 all drag-and-drop operations have implicit Copy semantics.
        self.proxy.version() < 3
            || (self.action == DndAction::Copy && self.source_actions.contains(DndAction::Copy))
    }

    fn read_finished(&mut self, handle: &calloop::LoopHandle<'_, WinitState>) {
        // The reader callback removes itself on return; never remove it reentrantly.
        self.reader = None;
        // The deadline bounds the read, not the user's time hovering over a window.
        if let Some(token) = self.timeout.take() {
            handle.remove(token);
        }
    }
}

#[derive(Default)]
struct TransferData {
    bytes: Vec<u8>,
    paths: Option<Vec<PathBuf>>,
    dropped: bool,
}

impl TransferData {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > MAX_URI_LIST.saturating_sub(self.bytes.len()) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "file URI list exceeds 1 MiB"));
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn read_done(&mut self) -> bool {
        self.paths = parse_uri_list(&self.bytes);
        self.bytes = Vec::new();
        self.paths.is_some()
    }

    fn take_drop(&mut self) -> Option<Vec<PathBuf>> {
        if self.dropped {
            self.paths.take()
        } else {
            None
        }
    }
}

impl WinitState {
    fn dnd_cursor(&mut self, id: &ObjectId, x: f64, y: f64) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        let Some(offer) = self.dnd.offers.get_mut(id) else { return };
        // active() excludes dropped offers, so later drags cannot overwrite this snapshot.
        offer.position = Some(LogicalPosition::new(x, y));
        let Some(window_id) = offer.window else {
            return;
        };
        let Some(window) = self.windows.get_mut().get(&window_id) else { return };
        // A poisoned or temporarily held window lock must not kill or stall a file transfer.
        let Ok(window) = window.try_lock() else { return };
        let scale = window.scale_factor();
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let position = LogicalPosition::new(x, y).to_physical(scale);
        drop(window);
        let device_id = crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(DeviceId));
        self.events_sink
            .push_window_event(WindowEvent::CursorMoved { device_id, position }, window_id);
        self.dispatched_events = true;
    }

    fn dnd_hover(&mut self, window: WindowId) {
        if !self.windows.get_mut().contains_key(&window) {
            return;
        }
        let mut hovered = Vec::new();
        for offer in self.dnd.offers.values() {
            if offer.window == Some(window) && !offer.data.dropped && offer.copy_selected() {
                if let Some(paths) = &offer.data.paths {
                    hovered.extend(paths.iter().cloned());
                }
            }
        }
        self.dispatched_events |= self.dnd.publish_hover(window, hovered, &mut self.events_sink);
    }

    fn dnd_remove(&mut self, id: &ObjectId, finish: bool) {
        let Some(mut offer) = self.dnd.offers.remove(id) else { return };
        self.dnd.ready.retain(|ready| ready != id);
        if let Some(token) = offer.reader.take() {
            self.loop_handle.remove(token);
        }
        if let Some(token) = offer.timeout.take() {
            self.loop_handle.remove(token);
        }
        if offer.proxy.is_alive() {
            if finish && offer.data.dropped && offer.copy_selected() && offer.proxy.version() >= 3 {
                offer.proxy.finish();
            }
            offer.proxy.destroy();
        }
        if let Some(window) = offer.window {
            self.dnd_hover(window);
            if !finish
                && !offer.data.dropped
                && self.windows.get_mut().contains_key(&window)
                && !self
                    .dnd
                    .offers
                    .values()
                    .any(|other| other.window == Some(window) && !other.data.dropped)
            {
                // wl_pointer does not report target motion/leave while a drag owns it. Clear our
                // synthetic position on cancellation, including read failures before Leave.
                let device_id =
                    crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(DeviceId));
                self.events_sink.push_window_event(WindowEvent::CursorLeft { device_id }, window);
                self.dispatched_events = true;
            }
        }
    }

    pub(crate) fn dnd_remove_device(&mut self, device: &WlDataDevice) {
        let ids: Vec<_> = self
            .dnd
            .offers
            .iter()
            .filter(|(_, offer)| offer.device == device.id())
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.dnd_remove(&id, false);
        }
    }

    pub(crate) fn dnd_remove_window(&mut self, window: WindowId) {
        self.dnd.cursors.remove(&window);
        self.dnd.hovered.remove(&window);
        let ids: Vec<_> = self
            .dnd
            .offers
            .iter()
            .filter(|(_, offer)| offer.window == Some(window))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.dnd_remove(&id, false);
        }
    }

    fn dnd_deliver(&mut self, id: &ObjectId) {
        let Some(offer) = self.dnd.offers.get_mut(id) else { return };
        if !offer.copy_selected() {
            // Only Copy is supported; never leave a completed, rejected drop without a reader
            // or deadline if the compositor sends an unexpected action change after Drop.
            if offer.data.dropped {
                self.dnd_remove(id, false);
            }
            return;
        }
        let Some(window) = offer.window else { return };
        let Some(requests) = self.window_requests.get_mut().get(&window) else { return };
        self.dispatched_events |= self.dnd.queue_drop(id, requests);
    }

    pub(crate) fn dnd_take_drop(&mut self, window: WindowId, scale: f64) -> Option<DropBatch> {
        if self
            .window_requests
            .get_mut()
            .get(&window)?
            .closed
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return None;
        }
        let (id, batch) = self.dnd.take_drop(window, scale)?;
        if batch.is_some() {
            // DroppedFile clears egui's entire hover list. Republish any newer drag after the batch.
            self.dnd.hovered.remove(&window);
        }
        self.dnd_remove(&id, batch.is_some());
        batch
    }

    fn dnd_read(&mut self, id: &ObjectId) -> io::Result<()> {
        let (read, write) = pipe_with(PipeFlags::CLOEXEC)?;
        // Set ONLY the receiving end nonblocking; a source may use ordinary blocking writes.
        rustix::io::ioctl_fionbio(&read, true)?;
        let key = id.clone();
        let reader = self
            .loop_handle
            .insert_source(
                Generic::new(read, Interest::READ, Mode::Level),
                move |_, pipe, state: &mut WinitState| {
                    if !state.dnd.offers.contains_key(&key) {
                        return Ok(PostAction::Remove);
                    }
                    let mut chunk = [0; 8192];
                    let result = match rustix::io::read(&*pipe, &mut chunk) {
                        Ok(0) => None,
                        Ok(n) => {
                            let result = chunk
                                .get(..n)
                                .ok_or_else(|| {
                                    io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "invalid file transfer read length",
                                    )
                                })
                                .and_then(|bytes| match state.dnd.offers.get_mut(&key) {
                                    Some(offer) => offer.data.append(bytes),
                                    None => Ok(()),
                                });
                            Some(result)
                        },
                        Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {
                            return Ok(PostAction::Continue)
                        },
                        Err(error) => Some(Err(error.into())),
                    };
                    if matches!(result, Some(Ok(()))) {
                        return Ok(PostAction::Continue);
                    }
                    if let Some(offer) = state.dnd.offers.get_mut(&key) {
                        offer.read_finished(&state.loop_handle);
                    }
                    match result {
                        None => {
                            let ready = state.dnd.offers.get_mut(&key).and_then(|offer| {
                                offer.data.read_done().then_some((offer.window, offer.data.dropped))
                            });
                            match ready {
                                Some((_, true)) => state.dnd_deliver(&key),
                                Some((Some(window), false)) => state.dnd_hover(window),
                                _ => state.dnd_remove(&key, false),
                            }
                        },
                        Some(Err(error)) => {
                            warn!("Wayland file drop: {error}");
                            state.dnd_remove(&key, false);
                        },
                        Some(Ok(())) => {},
                    }
                    Ok(PostAction::Remove)
                },
            )
            .map_err(|error| error.error)?;
        if let Some(offer) = self.dnd.offers.get_mut(id) {
            offer.reader = Some(reader);
            offer.proxy.receive(URI_LIST.to_owned(), write.as_fd());
        }
        let key = id.clone();
        let timeout = self
            .loop_handle
            .insert_source(
                Timer::from_duration(TRANSFER_TIMEOUT),
                move |_, _, state: &mut WinitState| {
                    if let Some(offer) = state.dnd.offers.get_mut(&key) {
                        offer.timeout = None;
                        warn!("Wayland file drop: transfer timed out");
                    }
                    state.dnd_remove(&key, false);
                    TimeoutAction::Drop
                },
            )
            .map_err(|error| error.error)?;
        if let Some(offer) = self.dnd.offers.get_mut(id) {
            offer.timeout = Some(timeout);
        }
        Ok(())
    }
}

impl Dispatch<WlDataDeviceManager, GlobalData> for WinitState {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDevice, ()> for WinitState {
    event_created_child!(WinitState, WlDataDevice, [0 => (WlDataOffer, ())]);

    fn event(
        state: &mut Self,
        device: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                if state.dnd.offers.len() >= MAX_OFFERS {
                    warn!("Wayland file drop: too many pending offers");
                    id.destroy();
                    return;
                }
                state.dnd.offers.insert(id.id(), Offer::new(id, device.id()));
            },
            wl_data_device::Event::Enter { serial, surface, x, y, id } => {
                if let Some(old) = state.dnd.active(device) {
                    state.dnd_remove(&old, false);
                }
                let Some(proxy) = id else { return };
                let key = proxy.id();
                let window = wayland::make_wid(&surface);
                let Some(offer) = state.dnd.offers.get_mut(&key) else { return };
                if offer.device != device.id()
                    || !offer.uri_list
                    || (proxy.version() >= 3 && !offer.source_actions.contains(DndAction::Copy))
                    || !is_content_surface(&surface)
                    || !state.windows.get_mut().contains_key(&window)
                {
                    proxy.accept(serial, None);
                    state.dnd_remove(&key, false);
                    return;
                }
                offer.window = Some(window);
                proxy.accept(serial, Some(URI_LIST.to_owned()));
                if proxy.version() >= 3 {
                    proxy.set_actions(DndAction::Copy, DndAction::Copy);
                }
                if let Err(error) = state.dnd_read(&key) {
                    warn!("Wayland file drop: cannot receive URI list: {error}");
                    state.dnd_remove(&key, false);
                    return;
                }
                state.dnd_cursor(&key, x, y);
            },
            wl_data_device::Event::Motion { x, y, .. } => {
                if let Some(id) = state.dnd.active(device) {
                    state.dnd_cursor(&id, x, y);
                }
            },
            wl_data_device::Event::Leave => {
                if let Some(id) = state.dnd.active(device) {
                    state.dnd_remove(&id, false);
                }
            },
            wl_data_device::Event::Drop => {
                if let Some(id) = state.dnd.active(device) {
                    if let Some(offer) = state.dnd.offers.get_mut(&id) {
                        if !offer.copy_selected() {
                            state.dnd_remove(&id, false);
                            return;
                        }
                        offer.data.dropped = true;
                    }
                    state.dnd_deliver(&id);
                }
            },
            wl_data_device::Event::Selection { id: Some(offer) } => {
                state.dnd_remove(&offer.id(), false)
            },
            _ => {},
        }
    }
}

impl Dispatch<WlDataOffer, ()> for WinitState {
    fn event(
        state: &mut Self,
        proxy: &WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(offer) = state.dnd.offers.get_mut(&proxy.id()) else { return };
        match event {
            wl_data_offer::Event::Offer { mime_type } => offer.uri_list |= mime_type == URI_LIST,
            wl_data_offer::Event::SourceActions { source_actions } => {
                offer.source_actions = match source_actions {
                    WEnum::Value(actions) => actions,
                    WEnum::Unknown(_) => DndAction::empty(),
                };
            },
            wl_data_offer::Event::Action { dnd_action } => {
                offer.action = match dnd_action {
                    WEnum::Value(action) => action,
                    WEnum::Unknown(_) => DndAction::empty(),
                };
                if let Some(window) = offer.window {
                    state.dnd_hover(window);
                }
            },
            _ => {},
        }
        // A completed transfer may now be waiting for a redraw when its final offer events arrive.
        state.dnd_deliver(&proxy.id());
    }
}

/// Decode local file URIs without interpreting filename bytes as UTF-8 or trimming their spaces.
fn parse_uri_list(data: &[u8]) -> Option<Vec<PathBuf>> {
    if data.len() > MAX_URI_LIST {
        return None;
    }
    let mut paths = Vec::new();
    for line in data.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() || line.starts_with(b"#") {
            continue;
        }
        if let Some(path) = file_uri(line) {
            if paths.len() == MAX_FILES {
                return None;
            }
            paths.push(path);
        }
    }
    (!paths.is_empty()).then_some(paths)
}

fn file_uri(uri: &[u8]) -> Option<PathBuf> {
    let mut parts = uri.splitn(2, |byte| *byte == b':');
    if !parts.next()?.eq_ignore_ascii_case(b"file") {
        return None;
    }
    let mut path = parts.next()?;
    if let Some(rest) = path.strip_prefix(b"//") {
        let slash = rest.iter().position(|byte| *byte == b'/')?;
        let host = rest.get(..slash)?;
        if !host.is_empty() && !host.eq_ignore_ascii_case(b"localhost") {
            return None;
        }
        path = rest.get(slash..)?;
    }
    if !path.starts_with(b"/") || path.iter().any(|byte| matches!(byte, 0..=31 | 127 | b'?' | b'#'))
    {
        return None;
    }
    let mut decoded = Vec::new();
    decoded.try_reserve(path.len()).ok()?;
    let mut bytes = path.iter().copied();
    while let Some(byte) = bytes.next() {
        let byte = if byte == b'%' {
            let high = (bytes.next()? as char).to_digit(16)?;
            let low = (bytes.next()? as char).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()?
        } else {
            byte
        };
        if byte == 0 {
            return None;
        }
        decoded.push(byte);
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sctk::reexports::client::protocol::wl_registry::WlRegistry;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::AtomicBool;

    use crate::platform_impl::wayland::window::state::FrameCallbackState;

    // Allocate distinct typed proxy identities on a socket whose peer never dispatches. This
    // exercises the real offer map without a compositor, a WinitState, or a new test dependency.
    impl Dispatch<WlRegistry, ()> for WinitState {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as Proxy>::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    #[test]
    fn completed_deferred_drop_requests_the_frame_callback_wakeup() {
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let connection = Connection::from_socket(socket).unwrap();
        let queue = connection.new_event_queue::<WinitState>();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let device: WlDataDevice = registry.bind(1, 3, &qh, ());
        let proxy: WlDataOffer = registry.bind(2, 3, &qh, ());
        let id = proxy.id();
        let window = WindowId(1);
        let mut offer = Offer::new(proxy, device.id());
        offer.window = Some(window);
        offer.position = Some(LogicalPosition::new(400.0, 300.0));
        offer.action = DndAction::Copy;
        offer.source_actions = DndAction::Copy;
        offer.data.dropped = true;
        offer.data.append(b"file:///tmp/deferred.pdf\n").unwrap();
        let mut dnd = DndState::default();
        dnd.offers.insert(id.clone(), offer);
        let requests = WindowRequests {
            closed: AtomicBool::new(false),
            redraw_requested: AtomicBool::new(false),
        };
        assert!(!dnd.queue_drop(&id, &requests), "the source has not closed its pipe yet");
        assert!(!requests.redraw_requested.load(Ordering::Relaxed));
        assert!(dnd.offers.get_mut(&id).unwrap().data.read_done());
        assert!(dnd.queue_drop(&id, &requests));
        assert!(dnd.has_ready_drop(window));

        // The completion wakes this iteration, but an outstanding frame callback prevents drawing.
        let mut frame = FrameCallbackState::Requested;
        assert!(matches!(frame, FrameCallbackState::Requested));
        // single_iteration resets this before waiting again.
        let mut dispatched_events = false;
        // CompositorHandler::frame uses this exact flag to wake the loop; no input event follows.
        if requests.redraw_requested.load(Ordering::Relaxed) {
            dispatched_events = true;
        }
        frame = FrameCallbackState::Received;
        assert!(dispatched_events, "the frame callback must wake delivery without another input");
        assert!(matches!(frame, FrameCallbackState::Received));
        assert!(requests.take_redraw_requested());
        let batch = dnd.take_drop(window, 1.0).unwrap().1.unwrap();
        let mut delivered = Vec::new();
        batch.dispatch(|event| {
            if let WindowEvent::DroppedFile(path) = event {
                delivered.push(path);
            }
        });
        assert_eq!(delivered, [PathBuf::from("/tmp/deferred.pdf")]);
        assert!(!dnd.has_ready_drop(window));
    }

    #[test]
    fn ready_drop_losing_copy_selection_is_removed() {
        use sctk::reexports::client::globals::registry_queue_init;
        use sctk::reexports::client::protocol::{wl_compositor::WlCompositor, wl_shm::WlShm};
        use sctk::reexports::protocols::xdg::shell::client::xdg_wm_base::XdgWmBase;
        use std::sync::Arc;
        use wayland_backend::server::{self, Backend, GlobalHandler};

        // Advertise only the globals needed to construct the real WinitState. Bind requests
        // stay queued on the client; the fixture dispatches only initial registry discovery.
        struct RegistryOnly;
        impl GlobalHandler<()> for RegistryOnly {
            fn bind(
                self: Arc<Self>,
                _: &server::Handle,
                _: &mut (),
                _: server::ClientId,
                _: server::GlobalId,
                _: server::ObjectId,
            ) -> Arc<dyn server::ObjectData<()>> {
                unreachable!("this fixture only dispatches registry discovery");
            }
        }
        let (socket, peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut backend = Backend::<()>::new().unwrap();
        let client = backend.handle().insert_client(peer, Arc::new(())).unwrap();
        for interface in [WlCompositor::interface(), WlShm::interface(), XdgWmBase::interface()] {
            backend.handle().create_global(interface, 1, Arc::new(RegistryOnly));
        }
        let connection = Connection::from_socket(socket).unwrap();
        let init_connection = connection.clone();
        let init = std::thread::spawn(move || registry_queue_init::<WinitState>(&init_connection));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !init.is_finished() {
            assert!(std::time::Instant::now() < deadline, "registry discovery timed out");
            backend.dispatch_all_clients(&mut ()).unwrap();
            backend.flush(Some(client.clone())).unwrap();
            std::thread::yield_now();
        }
        let (globals, queue) = init.join().unwrap().unwrap();
        let qh = queue.handle();
        let event_loop = calloop::EventLoop::<WinitState>::try_new().unwrap();
        let mut state = WinitState::new(&globals, &qh, event_loop.handle()).unwrap();
        let registry = globals.registry();
        let device: WlDataDevice = registry.bind(1, 3, &qh, ());
        let proxy: WlDataOffer = registry.bind(2, 3, &qh, ());
        let id = proxy.id();
        let window = WindowId(1);
        let mut offer = Offer::new(proxy.clone(), device.id());
        offer.window = Some(window);
        offer.position = Some(LogicalPosition::new(400.0, 300.0));
        offer.action = DndAction::Copy;
        offer.source_actions = DndAction::Copy;
        offer.data.dropped = true;
        offer.data.append(b"file:///tmp/rejected.pdf\n").unwrap();
        assert!(offer.data.read_done());
        state.dnd.offers.insert(id.clone(), offer);
        let requests = Arc::new(WindowRequests {
            closed: AtomicBool::new(false),
            redraw_requested: AtomicBool::new(false),
        });
        state.window_requests.get_mut().insert(window, requests.clone());
        assert!(state.dnd.queue_drop(&id, &requests));
        assert!(state.dnd.has_ready_drop(window));
        assert!(requests.take_redraw_requested());

        state.dnd.offers.get_mut(&id).unwrap().action = DndAction::Move;
        assert!(
            state.dnd_take_drop(window, 1.0).is_none(),
            "recheck Copy before deferred delivery"
        );
        assert!(state.dnd.ready.is_empty(), "a rejected drop must not remain queued");
        assert!(!state.dnd.offers.contains_key(&id), "a rejected offer must be removed");
        assert!(!proxy.is_alive(), "the rejected offer's protocol object must be destroyed");
        assert!(
            !state.dnd.has_ready_drop(window),
            "a rejected drop must not trigger further redraws"
        );
        assert!(state.dnd_take_drop(window, 1.0).is_none());
        assert!(!requests.take_redraw_requested());
    }

    #[test]
    fn deferred_drops_keep_their_own_position_until_their_frame_and_restore_the_cursor() {
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let connection = Connection::from_socket(socket).unwrap();
        let queue = connection.new_event_queue::<WinitState>();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let device: WlDataDevice = registry.bind(1, 3, &qh, ());
        let first: WlDataOffer = registry.bind(2, 3, &qh, ());
        let second: WlDataOffer = registry.bind(3, 3, &qh, ());
        let window = WindowId(1);
        let mut dnd = DndState::default();
        for (proxy, position, uri) in [
            (first, LogicalPosition::new(400.0, 300.0), b"file:///tmp/canvas.pdf\n".as_slice()),
            (second, LogicalPosition::new(80.0, 200.0), b"file:///tmp/panel.pdf\n".as_slice()),
        ] {
            let id = proxy.id();
            let mut offer = Offer::new(proxy, device.id());
            offer.window = Some(window);
            offer.position = Some(position);
            offer.action = DndAction::Copy;
            offer.source_actions = DndAction::Copy;
            offer.data.dropped = true;
            offer.data.append(uri).unwrap();
            dnd.ready.push_back(id.clone());
            dnd.offers.insert(id, offer);
        }
        let device_id = crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(DeviceId));
        let current = PhysicalPosition::new(50.0, 600.0);
        dnd.record_cursor(&Event::WindowEvent {
            window_id: crate::window::WindowId(window),
            event: WindowEvent::CursorMoved { device_id, position: current },
        });
        // The movement has already been delivered in an earlier loop iteration. EOF arrives later.
        assert!(dnd.take_drop(window, 2.0).is_none(), "never deliver before EOF");
        for offer in dnd.offers.values_mut() {
            assert!(offer.data.read_done());
        }
        let mut cursor = Some(current);
        let mut files = Vec::new();
        let mut frames = Vec::new();
        for expected in [PhysicalPosition::new(800.0, 600.0), PhysicalPosition::new(160.0, 400.0)] {
            let batch = dnd.take_drop(window, 2.0).unwrap().1.unwrap();
            batch.dispatch(|event| match event {
                WindowEvent::CursorMoved { position, .. } => cursor = Some(position),
                WindowEvent::CursorLeft { .. } => cursor = None,
                WindowEvent::DroppedFile(path) => files.push(path),
                WindowEvent::RedrawRequested => {
                    frames.push((cursor, std::mem::take(&mut files)));
                },
                _ => {},
            });
            assert_eq!(frames.last().unwrap().0, Some(expected));
            assert_eq!(cursor, Some(current), "a newer drag's cursor must be restored");
        }
        assert_eq!(frames[0].1, [PathBuf::from("/tmp/canvas.pdf")]);
        assert_eq!(frames[1].1, [PathBuf::from("/tmp/panel.pdf")]);
        assert!(!dnd.has_ready_drop(window));
        assert!(dnd.take_drop(window, 2.0).is_none(), "delivery happens only once");
    }

    #[test]
    fn deferred_drop_restores_cursor_leave_and_keeps_multi_file_drops_in_one_frame() {
        let device_id = crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(DeviceId));
        let mut events = Vec::new();
        DropBatch {
            position: PhysicalPosition::new(200.0, 100.0),
            paths: vec![PathBuf::from("/tmp/a.pdf"), PathBuf::from("/tmp/b.pdf")],
            restore: WindowEvent::CursorLeft { device_id },
        }
        .dispatch(|event| events.push(event));
        assert!(matches!(
            events.as_slice(),
            [
                WindowEvent::CursorMoved { .. },
                WindowEvent::DroppedFile(_),
                WindowEvent::DroppedFile(_),
                WindowEvent::RedrawRequested,
                WindowEvent::CursorLeft { .. }
            ]
        ));
    }

    #[test]
    fn hover_notifications_change_only_when_published_paths_change() {
        let mut dnd = DndState::default();
        let window = WindowId(1);
        let mut sink = EventSink::new();
        let paths = vec![PathBuf::from("/tmp/a.pdf"), PathBuf::from("/tmp/b.pdf")];
        assert!(!dnd.publish_hover(window, Vec::new(), &mut sink));
        assert!(dnd.publish_hover(window, paths.clone(), &mut sink));
        assert!(!dnd.publish_hover(window, paths, &mut sink));
        assert!(dnd.publish_hover(window, Vec::new(), &mut sink));
        assert!(!dnd.publish_hover(window, Vec::new(), &mut sink));
        assert!(matches!(
            sink.window_events.as_slice(),
            [
                Event::WindowEvent { event: WindowEvent::HoveredFile(_), .. },
                Event::WindowEvent { event: WindowEvent::HoveredFile(_), .. },
                Event::WindowEvent { event: WindowEvent::HoveredFileCancelled, .. }
            ]
        ));
    }

    #[test]
    fn dropping_an_older_offer_republishes_a_newer_hover_without_a_spurious_cancel() {
        let mut dnd = DndState::default();
        let window = WindowId(1);
        let mut sink = EventSink::new();
        let paths = vec![PathBuf::from("/tmp/new.pdf")];
        assert!(dnd.publish_hover(window, paths.clone(), &mut sink));
        sink.window_events.clear();
        // DroppedFile clears egui's hover list; dnd_take_drop forgets that published state.
        dnd.hovered.remove(&window);
        assert!(dnd.publish_hover(window, paths, &mut sink));
        assert!(matches!(
            sink.window_events.as_slice(),
            [Event::WindowEvent { event: WindowEvent::HoveredFile(_), .. }]
        ));
    }

    #[test]
    fn decoration_surfaces_are_rejected_without_reusing_their_local_coordinates() {
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let connection = Connection::from_socket(socket).unwrap();
        let queue = connection.new_event_queue::<WinitState>();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let content: WlSurface = registry.bind(1, 6, &qh, SurfaceData::new(None, 1));
        let decoration: WlSurface =
            registry.bind(2, 6, &qh, SurfaceData::new(Some(content.clone()), 1));
        assert!(is_content_surface(&content), "content coordinates can be delivered unchanged");
        assert!(
            !is_content_surface(&decoration),
            "decoration coordinates must never be delivered as content coordinates"
        );
    }

    #[test]
    fn leaving_a_new_drag_preserves_an_older_drop_waiting_for_eof() {
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let connection = Connection::from_socket(socket).unwrap();
        let queue = connection.new_event_queue::<WinitState>();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let device: WlDataDevice = registry.bind(1, 3, &qh, ());
        let first: WlDataOffer = registry.bind(2, 3, &qh, ());
        let second: WlDataOffer = registry.bind(3, 3, &qh, ());
        let first_id = first.id();
        let second_id = second.id();
        assert_ne!(first_id, second_id);
        let mut old = Offer::new(first, device.id());
        assert!(!old.copy_selected());
        old.action = DndAction::Move;
        old.source_actions = DndAction::Copy | DndAction::Move;
        assert!(!old.copy_selected(), "never deliver a negotiated Move");
        old.action = DndAction::Copy;
        assert!(old.copy_selected());
        old.window = Some(WindowId(1));
        old.data.dropped = true;
        old.data.append(b"file:///tmp/old").unwrap();
        let mut state = DndState::default();
        state.offers.insert(first_id.clone(), old);
        assert_eq!(state.active(&device), None, "a dropped transfer is no longer the active drag");
        let mut new = Offer::new(second, device.id());
        new.window = Some(WindowId(1));
        new.data.append(b"file:///tmp/new.pdf\n").unwrap();
        assert!(new.data.read_done());
        state.offers.insert(second_id.clone(), new);
        assert_eq!(state.active(&device), Some(second_id.clone()));
        // Leave selects only the current hover, retaining the previous drop and its own bytes.
        state.offers.remove(&state.active(&device).unwrap());
        assert_eq!(state.active(&device), None);
        let old = state.offers.get_mut(&first_id).unwrap();
        old.data.append(b".pdf\n").unwrap();
        assert!(old.data.read_done());
        assert_eq!(old.data.take_drop(), Some(vec![PathBuf::from("/tmp/old.pdf")]));
        assert!(old.data.take_drop().is_none());
    }

    #[test]
    fn completed_hover_disarms_its_read_deadline() {
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let connection = Connection::from_socket(socket).unwrap();
        let proxy = WlDataOffer::inert(connection.backend().downgrade());
        let mut offer = Offer::new(proxy, ObjectId::null());
        let event_loop = calloop::EventLoop::<WinitState>::try_new().unwrap();
        let handle = event_loop.handle();
        let token =
            handle.insert_source(Timer::immediate(), |_, _, _| TimeoutAction::Drop).unwrap();
        offer.timeout = Some(token);
        offer.data.append(b"file:///tmp/hover.pdf\n").unwrap();
        assert!(offer.data.read_done());
        offer.read_finished(&handle);
        assert!(offer.timeout.is_none());
        assert!(
            handle.disable(&token).is_err(),
            "the timer must be unregistered, not just forgotten"
        );
        assert!(offer.data.paths.is_some(), "a completed hover remains valid until drop or leave");
        assert!(offer.data.take_drop().is_none(), "hovering must not open the file");
    }

    #[test]
    fn local_files_keep_order_spaces_unicode_and_percent_bytes() {
        let paths = parse_uri_list(b"# files\r\nfile:///tmp/one.pdf\r\nfile://LOCALHOST/tmp/drop%20%C3%BC%20b.pdf\nFILE:/tmp/%2520%23%3F.pdf\nfile:///tmp/last.pdf \r\n").unwrap();
        assert_eq!(
            paths,
            ["/tmp/one.pdf", "/tmp/drop ü b.pdf", "/tmp/%20#?.pdf", "/tmp/last.pdf "]
                .map(PathBuf::from)
        );
        assert_eq!(
            parse_uri_list("file:///tmp/生 ü.pdf\n".as_bytes()),
            Some(vec![PathBuf::from("/tmp/生 ü.pdf")])
        );
    }

    #[test]
    fn encoded_delimiters_are_filename_bytes_not_list_or_uri_syntax() {
        assert_eq!(
            parse_uri_list(b"file:///tmp/%0A%0D%23%3F%25.pdf\r\n"),
            Some(vec![PathBuf::from("/tmp/\n\r#?%.pdf")])
        );
    }

    #[test]
    fn non_utf8_filename_bytes_are_not_replaced() {
        let paths = parse_uri_list(b"file:///tmp/%FF.pdf\nfile:///tmp/raw\xfe.pdf\n").unwrap();
        assert_eq!(paths[0].as_os_str().as_bytes(), b"/tmp/\xff.pdf");
        assert_eq!(paths[1].as_os_str().as_bytes(), b"/tmp/raw\xfe.pdf");
    }

    #[test]
    fn malformed_remote_and_non_file_entries_are_ignored() {
        let invalid = b"https://example.org/a.pdf\nfile://remote/tmp/a.pdf\nfile://user@localhost/tmp/a.pdf\nfile:relative.pdf\nfile:///tmp/%00.pdf\nfile:///tmp/bad%2\nfile:///tmp/bad%GG\nfile:///tmp/a?query\nfile:///tmp/a#fragment\nfile:///tmp/raw\0.pdf\n";
        assert!(parse_uri_list(invalid).is_none());
        let mut mixed = invalid.to_vec();
        mixed.extend_from_slice(b"file:///tmp/valid.pdf\n");
        assert_eq!(parse_uri_list(&mixed), Some(vec![PathBuf::from("/tmp/valid.pdf")]));
    }

    #[test]
    fn uri_bytes_and_file_counts_are_bounded() {
        let mut transfer = TransferData::default();
        transfer.append(&vec![b'#'; MAX_URI_LIST]).unwrap();
        assert!(transfer.append(b"x").is_err());
        assert_eq!(transfer.bytes.len(), MAX_URI_LIST);
        assert!(parse_uri_list(&vec![b'x'; MAX_URI_LIST + 1]).is_none());
        let list = b"file:///tmp/a.pdf\n".repeat(MAX_FILES);
        assert_eq!(parse_uri_list(&list).unwrap().len(), MAX_FILES);
        assert!(parse_uri_list(&b"file:///tmp/a.pdf\n".repeat(MAX_FILES + 1)).is_none());
    }

    #[test]
    fn a_drop_waits_for_eof_and_can_be_taken_only_once() {
        let mut transfer = TransferData { dropped: true, ..Default::default() };
        transfer.append(b"file:///tmp/drop%20").unwrap();
        assert!(transfer.take_drop().is_none());
        transfer.append(b"%C3%BC.pdf\n").unwrap();
        assert!(transfer.read_done());
        assert!(transfer.bytes.is_empty());
        assert_eq!(transfer.take_drop(), Some(vec![PathBuf::from("/tmp/drop ü.pdf")]));
        assert!(transfer.take_drop().is_none());
    }

    #[test]
    fn reading_during_hover_does_not_open_a_file() {
        let mut transfer = TransferData::default();
        transfer.append(b"file:///tmp/a.pdf\n").unwrap();
        assert!(transfer.read_done());
        assert!(transfer.take_drop().is_none());
        transfer.dropped = true;
        assert!(transfer.take_drop().is_some());
    }

    #[test]
    fn nonblocking_pipe_reports_would_block_then_data_then_eof() {
        let (read, write) = pipe_with(PipeFlags::CLOEXEC).unwrap();
        rustix::io::ioctl_fionbio(&read, true).unwrap();
        let mut bytes = [0; 4];
        assert_eq!(rustix::io::read(&read, &mut bytes), Err(rustix::io::Errno::AGAIN));
        rustix::io::write(&write, b"data").unwrap();
        assert_eq!(rustix::io::read(&read, &mut bytes).unwrap(), 4);
        assert_eq!(&bytes, b"data");
        drop(write);
        assert_eq!(rustix::io::read(&read, &mut bytes).unwrap(), 0);
    }
}
