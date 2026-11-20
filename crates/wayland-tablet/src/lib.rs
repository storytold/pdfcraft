//! Pen-tablet sidecar for native Wayland.
//!
//! winit 0.30 has no `zwp_tablet_v2` on Wayland, so a pen arrives only as the
//! compositor's `wl_pointer` emulation: no pressure, no tilt, no eraser end. This
//! crate is the spike for the fix without patching winit: it binds the tablet-v2
//! protocol **on the same `wl_display` winit talks on**, on its own event queue, and
//! forwards pen events over a channel.
//!
//! Why the same display: the protocol hands the pen to a *focused surface of this
//! client* — `zwp_tablet_tool_v2.proximity_in` carries the `wl_surface`
//! (tablet-v2.xml, cited in `state.rs`). A second `wl_display` is a second client,
//! and would receive nothing. So [`TabletSidecar::spawn`] takes the display handle's
//! pointer and wraps it with `wayland_backend`'s guest-mode constructor
//! (`Backend::from_foreign_display`, sys backend): the same libwayland connection, a
//! second `Backend`, and the sidecar's own `wl_event_queue` — winit's queue is never
//! touched.
//!
//! Mind the ownership: the guest backend does not own the display (“will not close
//! the connection on drop”), and every proxy this crate destroys itself on [`Drop`].
//!
//! ## Display lifetime contract
//!
//! The display behind the handle passed to [`TabletSidecar::spawn`] /
//! [`TabletSidecar::spawn_with`] must outlive the sidecar and every `Backend` clone
//! obtained through its connection, and the sidecar must be dropped before the
//! display's owner (the windowing library). This is a **known soundness gap the safe
//! signature cannot enforce**: the display reaches this crate only as a raw
//! `*mut wl_display` smuggled through `HasDisplayHandle` — winit owns it and never
//! hands out a Rust reference to borrow — so the signatures say nothing about the
//! lifetime, and a display owner dropped before the sidecar would leave the guest
//! backend pointing at freed memory. The callers uphold the order instead: in
//! ui-egui the sidecar lives in the app struct, which drops before the windowing
//! environment does. (An `unsafe fn` would name the obligation, but the production
//! caller's crate forbids `unsafe_code` end to end; the contract-in-prose form keeps
//! the forbidden-unsafe rule intact.)
//!
//! ```no_run
//! # use wayland_tablet::{TabletSidecar};
//! # fn demo(window: &impl raw_window_handle::HasDisplayHandle) -> Result<(), wayland_tablet::SidecarError> {
//! // The display behind `window` must outlive `sidecar`; drop `sidecar` first
//! // (the module's Display lifetime contract).
//! let (sidecar, events) = TabletSidecar::spawn(window)?;
//! // pump()/thread: see PumpMode; drain `events` from the app's frame loop.
//! let _ = (sidecar, events);
//! Ok(())
//! # }
//! ```

//! ## Task 0 scope
//!
//! This crate is the feasibility spike, no egui integration: `examples/print_pen.rs`
//! drives it under a bare winit window and prints every [`PenEvent`]. The measured
//! questions (latency against winit's pointer events, `proximity_in`'s foreign
//! surface, double delivery, stability) are answered by running that example on
//! hardware — the report in the task thread carries the numbers.

#![deny(clippy::expect_used, clippy::panic, clippy::todo, clippy::unimplemented)]
#![deny(clippy::unwrap_used, clippy::unreachable)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use wayland_client::backend::{Backend, WaylandError};
use wayland_client::globals::{GlobalList, registry_queue_init};
use wayland_client::{Connection, EventQueue};

use state::AppState;

mod state;

pub use state::{PenEvent, PenFrame, ToolKind};

/// Who dispatches the sidecar's queue: the app's frame, or a sidecar reader thread.
///
/// The Wayland socket is one fd, read by whoever gets there first — `read()` is
/// collective in wayland-client, and each event queue buffers its own events. So no
/// events are ever *lost* through the other side reading first; what differs is when
/// they get dispatched:
///
/// - [`Self::Main`]: the app calls [`TabletSidecar::pump()`] once per frame. Simplest,
///   no thread; but events read by winit's thread sit in the sidecar's buffer until
///   the next pump — at worst one extra frame of latency.
/// - [`Self::Thread`]: a dedicated reader (`prepare_read` → poll the shared fd with a
///   2 ms timeout → `read()` → `dispatch_pending`) reacts almost immediately after
///   every compositor write, and drains the channel as fast as events land. This is
///   the mode `examples/print_pen.rs` measures against the main-pump mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PumpMode {
    /// Dispatch from the app's frame loop via [`TabletSidecar::pump()`].
    #[default]
    Main,
    /// A dedicated reader thread owns the queue; `pump()` becomes a no-op and the
    /// channel is fed from the thread. The thread stops and is joined on drop.
    Thread,
}

/// How [`TabletSidecar::spawn`] failed.
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    /// Turning the windowing library's handle into a raw one failed.
    #[error("no raw display handle: {0}")]
    DisplayHandle(raw_window_handle::HandleError),
    /// The window's display is not Wayland (X11/Web/macOS): nothing to bind here.
    /// On those platforms the pen question is the windowing library's, not a
    /// sidecar's.
    #[error("the display is not Wayland — no sidecar")]
    NotWayland,
    /// The connection machinery failed before any binding.
    #[error("wayland: {0}")]
    Wayland(#[from] WaylandError),
    /// `registry_queue_init` failed (display error).
    #[error("wayland globals: {0}")]
    Globals(#[from] wayland_client::globals::GlobalError),
    /// A dispatched event was malformed (wayland-client's `DispatchError`).
    #[error("wayland dispatch: {0}")]
    Dispatch(#[from] wayland_client::DispatchError),
    /// No `wl_seat` advertised: tablet-v2 hangs off a seat
    /// (`get_tablet_seat`'s `seat` argument, tablet-v2.xml).
    #[error("no wl_seat on the connection")]
    NoSeat,
    /// A `wl_seat` older than version 5, whose `release` destructor this crate's
    /// cleanup needs (wayland.xml `since=5`).
    #[error("wl_seat older than version 5 (no release destructor)")]
    SeatTooOld,
    /// No `zwp_tablet_manager_v2` advertised: the compositor implements no
    /// tablet-v2, and there is nothing to bind.
    #[error("no zwp_tablet_manager_v2 — compositor has no tablet support")]
    NoTabletManager,
    /// The reader thread could not be spawned.
    #[error("spawning the reader thread: {0}")]
    Thread(#[from] std::io::Error),
}

struct Core {
    state: AppState,
    queue: EventQueue<AppState>,
}

/// The pen sidecar: tablet-v2 bound on the windowing library's `wl_display`.
///
/// Constructed with [`TabletSidecar::spawn`]. Drop stops any reader thread and
/// destroys every proxy this crate created (tools, tablet seat, manager, seat
/// release, registry); the connection itself is left to its owner.
#[must_use = "the sidecar stops when dropped; keep it alive for as long as pen events are wanted"]
pub struct TabletSidecar {
    core: Arc<Mutex<Core>>,
    mode: PumpMode,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    /// The registry from `registry_queue_init`; `GlobalList::destroy` needs it at
    /// drop (it takes self, hence the Option).
    globals: Option<GlobalList>,
    conn: Connection,
    /// A second handle on the guest backend, keeping the foreign-display
    /// association alive as long as `self` does (safety comment in `spawn_with`).
    _backend: Backend,
}

impl TabletSidecar {
    /// Bind tablet-v2 on `display`'s connection (the app's window works directly:
    /// `winit::window::Window` and `eframe`'s handles are `HasDisplayHandle`).
    ///
    /// The display must outlive the sidecar and be dropped after it (the module's
    /// Display lifetime contract — the safe signature cannot enforce it).
    pub fn spawn(display: &impl HasDisplayHandle) -> Result<(Self, Receiver<PenEvent>), SidecarError> {
        Self::spawn_with(display, PumpMode::default())
    }

    /// [`Self::spawn`] with the pump mode pinned (see [`PumpMode`]).
    ///
    /// The display must outlive the sidecar and be dropped after it (the module's
    /// Display lifetime contract — the safe signature cannot enforce it).
    pub fn spawn_with(display: &impl HasDisplayHandle, mode: PumpMode) -> Result<(Self, Receiver<PenEvent>), SidecarError> {
        let handle = match display.display_handle() {
            Ok(handle) => handle,
            Err(err) => return Err(SidecarError::DisplayHandle(err)),
        };
        let RawDisplayHandle::Wayland(wayland) = handle.as_raw() else {
            return Err(SidecarError::NotWayland);
        };
        // SAFETY: `wayland.display` is the `*mut wl_display` of the windowing
        // library's live connection (winit holds it open while the event loop
        // runs). From here on *self* keeps a `Backend` glued to that pointer for
        // its entire lifetime (`_backend` field — then `conn`, whose `Backend`
        // is a clone): the wayland-backend contract for `from_foreign_display` —
        // “ensure the wl_display remains live as long as the Backend (or its
        // clones) exist”. The backend is in guest mode and never disconnects or
        // frees the display; this crate cannot outlive the event loop it was
        // spawned from (documents its own drop order, and the channel is drained
        // by the same app that owns the loop).
        //
        // There is no safe alternative: wayland-backend 0.3 constructs a backend
        // either around a display it owns (`connect_to_env`, `from_fd`) or, only
        // unsafely, by adopting one (`from_foreign_display`, verified in the 0.3
        // source: drop disconnects the display only when it is owned). And a
        // second owned connection cannot carry the feature: tablet events reach
        // the client that owns the surface the tool is over (tablet-v2.xml's
        // `proximity_in` names "the current surface the tablet tool is over",
        // and a wl_surface exists only in its own client's namespace), so the
        // app's own connection must be the one tablet-v2 is bound on.
        #[allow(unsafe_code)]
        let backend = unsafe { Backend::from_foreign_display(wayland.display.as_ptr().cast()) };
        let conn = Connection::from_backend(backend.clone());
        // The sidecar's own queue (!): everything below — registry, seat, tablet
        // seat, tools — dispatches here. winit's queue sees nothing of it.
        let (globals, queue) = registry_queue_init::<AppState>(&conn)?;
        let (tx, rx) = channel::<PenEvent>();
        let mut app = AppState::new(tx);
        let qh = queue.handle();
        app.bind_tables(&globals, &qh)?;
        // One round trip for the startup burst: tool_added/tablet_added announcements
        // and each tool's identification sequence (ending in `done`, tablet-v2.xml).
        // If the pen is already hovering a window, its `proximity_in` lands in this
        // roundtrip too and the first dispatch carries it.
        let mut queue = queue;
        queue.roundtrip(&mut app)?;
        let core = Arc::new(Mutex::new(Core { state: app, queue }));
        let stop = Arc::new(AtomicBool::new(false));
        let reader = match mode {
            PumpMode::Main => None,
            PumpMode::Thread => {
                let thread_stop = Arc::clone(&stop);
                let thread_core = Arc::clone(&core);
                // The thread only ever locks `core` and reads the channel-less
                // shared state; `Core` is behind a mutex, and the queue's dispatch
                // happens only here — no other thread touches it in this mode.
                Some(std::thread::Builder::new().name("wayland-tablet".into()).spawn(move || reader_loop(&thread_core, &thread_stop))?)
            }
        };
        let sidecar = Self { core, mode, stop, reader, globals: Some(globals), conn, _backend: backend };
        Ok((sidecar, rx))
    }

    /// Dispatch the sidecar's queue once and count what it delivered onto the
    /// channel. [`PumpMode::Main`]: call once per frame, from the app's loop
    /// (`egui`'s update, or winit's `about_to_wait`). [`PumpMode::Thread`]: always 0
    /// — the reader thread feeds the channel itself.
    pub fn pump(&self) -> Result<usize, SidecarError> {
        if self.mode != PumpMode::Main {
            return Ok(0);
        }
        // Split the guard's fields apart explicitly: borrowing `queue` and `state`
        // through the `MutexGuard` deref in one call makes the borrowck treat both
        // as a borrow of the whole `Core`.
        let Core { state, queue } = &mut *self.lock_core();
        let dispatched = queue.dispatch_pending(state)?;
        Ok(dispatched)
    }

    /// The sidecar's connection (the same `wl_display` as the app's), for callers
    /// that want to flush or inspect it.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Poison-tolerant lock: a reader thread that panicked must not take the app's
    /// pen path down with it (the never-crash rule; the poisoned state still holds
    /// consistent protocol objects — the mutex content is ours alone).
    fn lock_core(&self) -> MutexGuard<'_, Core> {
        match self.core.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Drop for TabletSidecar {
    fn drop(&mut self) {
        // 1. Stop the reader, if any, first: it must not dispatch onto destroyed
        //    proxies.
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
        // 2. Destroy the bound proxies (pads with their children first, then
        //    tablets, tools, tablet seat, manager, seat release) — the guest
        //    backend's docs warn that after a naked drop the server's late
        //    events for orphaned objects are silently discarded; tearing down
        //    properly is what keeps a foreign connection healthy.
        self.lock_core().state.shutdown();
        // 3. The registry (GlobalList::destroy uses wl_fixes when present; where the
        //    compositor is too old for that, it is a no-op by its own docs — the
        //    guest connection is left with one dead registry object, harmless).
        if let Some(globals) = self.globals.take() {
            globals.destroy();
        }
        // 4. The destroyer's requests ride the shared socket out before we let the
        //    app keep running without us.
        let _ = self.conn.flush();
    }
}

/// The dedicated reader: prepare a read guard (before polling, as its docs insist —
/// the guard is what synchronizes multiple readers of one socket), poll the shared
/// fd briefly, read when ready, and dispatch whatever our queue then holds.
fn reader_loop(core: &Mutex<Core>, stop: &AtomicBool) {
    // 2 ms poll cycle: fast reaction to the pen, small enough that a stopped
    // sidecar is noticed within milliseconds (and not a busy spin: 2 ms of park
    // per iteration).
    const POLL: rustix::event::Timespec = rustix::event::Timespec { tv_sec: 0, tv_nsec: 2_000_000 };
    while !stop.load(Ordering::Relaxed) {
        let mut core = match core.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        // read_events doc: “If this returns None, you should dispatch_pending before
        // trying again” — events are already buffered in our queue.
        let guard = core.queue.prepare_read();
        if let Some(guard) = guard {
            let fd = guard.connection_fd();
            let mut fds = [rustix::event::PollFd::new(&fd, rustix::event::PollFlags::IN)];
            // Reading is collective: whichever thread wins the race consumes the
            // socket's bytes, and each queue (ours and winit's) buffers its own
            // events from them. So a read *here* also unblocks winit's queue, and
            // winit's read feeds ours — nobody starves; whoever polls first decides
            // the latency.
            let ready = rustix::event::poll(&mut fds, Some(&POLL)).is_ok_and(|n| n > 0);
            if ready {
                // No ready, no read: dropping the guard unfenced (its docs: dropping
                // before read is the no-read path other readers need).
                let _ = guard.read();
            }
        }
        // Dispatch what our queue now holds; events reach the app via the state's
        // channel sends inside the Dispatch impls.
        // A protocol error kills the reader (the sidecar is dead): the channel stops
        // delivering, `Drop` joins the thread. The app's own path is untouched.
        let Core { state, queue } = &mut *core;
        if queue.dispatch_pending(state).is_err() {
            stop.store(true, Ordering::Release);
        }
    }
}
