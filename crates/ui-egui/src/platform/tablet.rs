//! The native Wayland pen, fed to egui (#3650).
//!
//! winit 0.30 binds no tablet-v2, and GNOME/KDE/Hyprland do not emulate pointer events
//! for tablet tools, so a pen on native Wayland does nothing. The sidecar crate
//! (`wayland-tablet`) binds `zwp_tablet_v2` on winit's own `wl_display`, on its own
//! event queue, and sends [`PenEvent`]s over a channel. This module is the egui end:
//!
//! - a reader thread drains the channel, converts each [`PenEvent`] to
//!   [`egui::Event`]s (through [`to_egui`], pure and unit-tested), and stashes them,
//!   calling [`egui::Context::request_repaint`] so an idle app still wakes —
//!   eframe idles with `ControlFlow::Wait`, and a winit wakeup with no winit-level
//!   events runs no egui pass, so the pen would wait for the next mouse move without
//!   that nudge (the same pattern `DocumentCopy` and the desktop-theme thread use);
//! - [`PdfCraftApp::raw_input_hook`] copies the stash into the frame's
//!   [`egui::RawInput`], where egui reads it like any other pointer input.
//!
//! The pen is a plain pointer: motion, tip down/up and the side button map onto
//! egui's primary/secondary button, so every existing tool works. Pressure, tilt and
//! the eraser end are not in egui's input model (the fork's `PointerButton` carries no
//! pressure field); they land in [`PenSnapshot`] instead, for the tool layer to read
//! when a feature wants them ([`PdfCraftApp::pen_snapshot`]). The eraser end is the
//! one eraser state the canvas acts on, and it is resolved from the *drained batch*
//! at drain time (latched from the first eraser event until a `proximity_out`, see
//! [`PenInput`]) — not from the snapshot, which the reader may already have updated
//! past the events the frame is processing.
//!
//! Fallback (Phase 4 of the plan): any failure to start — X11 or XWayland (the
//! display handle is not Wayland), a compositor without tablet-v2, a binding error —
//! is logged once and the app continues exactly as before. Mouse and touch input
//! never pass through here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use raw_window_handle::HasDisplayHandle;
use wayland_tablet::{PenEvent, PumpMode, TabletSidecar, ToolKind};

/// Linux event codes the tablet-v2 `button` event forwards raw (tablet-v2.xml:
/// "button is an enum, but not a wl_pointer one"). The bound one is the Linux
/// input event code for a stylus' barrel button (input-event-codes.h:
/// `BTN_STYLUS` is 0x14b — `BTN_TOOL_PEN` is 0x140, a tool kind, not a button);
/// `BTN_STYLUS2` (0x14c) stays unbound — one barrel button is what the UI binds,
/// as Acrobat's pen side button.
const BTN_STYLUS: u32 = 0x14b;

/// One pen state snapshot for the tool layer: what the last [`PenEvent`] said.
///
/// The eraser end reports [`ToolKind::Eraser`] here — the flag a tool reads to erase
/// with the eraser end and draw with the pen end. Pressure is 0..1 (the sidecar
/// normalizes the protocol's 0..65535 and clamps hostile values); tilt is in degrees,
/// the protocol's own units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PenSnapshot {
    pub tool: ToolKind,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    /// The pen is over one of the app's surfaces (`proximity_in` seen, no
    /// `proximity_out` yet).
    pub proximate: bool,
    /// The tip is down.
    pub contact: bool,
}

/// Whether the pen's eraser end is over the app's surfaces: the canvas maps it to the
/// eraser tool, so the eraser end erases ink whatever tool is selected (Acrobat's
/// behaviour). The pen end, and any state after the pen left the surface, never is.
pub(crate) fn eraser_end_active(snapshot: Option<PenSnapshot>) -> bool {
    matches!(snapshot, Some(snapshot) if snapshot.proximate && snapshot.tool == ToolKind::Eraser)
}

/// One pen event's converted egui events, plus the identity the drain needs: the
/// frame's tool kind (every event of a frame shares it — `PenFrame` is `Copy`), and
/// whether the event left the surface, which ends the eraser latch. The eraser end
/// is resolved from these at drain time, not from the reader's later snapshot: the
/// reader may already have processed the pen leaving by the time the canvas reads
/// the frame's input, and the frame's own events are what it must resolve from.
struct Stashed {
    kind: ToolKind,
    prox_out: bool,
    events: Vec<egui::Event>,
}

/// The pen's egui-side input state: the event stash, the reader's snapshot, and the
/// eraser resolution (see [`PenInput::take_events`]). Split from [`Tablet`] so tests
/// can drive the drain without a Wayland display.
#[derive(Default)]
struct PenInput {
    /// Converted egui events not yet handed to the frame's `RawInput`. The reader
    /// pushes, [`PenInput::take_events`] drains.
    stash: Arc<Mutex<Vec<Stashed>>>,
    /// The latest pen state, from the same events. The reader writes; the snapshot
    /// fallback of [`PenInput::eraser_end_for_frame`] reads.
    snapshot: Arc<Mutex<PenSnapshot>>,
    /// The eraser end's identity, latched at drain time: on from the first delivered
    /// eraser event, cleared when a `proximity_out` is delivered (so it cannot stick
    /// after the pen leaves). Touched by the drain only (see `take_events`); atomic
    /// so `Tablet` stays `Sync` beside the reader thread's handles.
    eraser_latch: AtomicBool,
    /// The drained batch's eraser resolution for the frame whose input is being read
    /// (see `take_events`); `eraser_end_for_frame` reads it, falling back to the
    /// snapshot when it is false — hover with no events keeps using the snapshot.
    frame_eraser: AtomicBool,
}

/// The receiving frame's zoom (`ctx.zoom_factor()`) guarded the way
/// `drag_pointer::in_window` guards it: a zoom that is not a positive finite number
/// leaves the coordinates alone.
///
/// The zoom factor, not `pixels_per_point`: tablet-v2's `motion` is surface-local, the
/// compositor's *logical* space — the native scale is already out of it — and egui's
/// points are winit's physical pixels over `pixels_per_point`, which is the zoom factor
/// times the native scale (`egui_winit::pixels_per_point`). Surface-local into points is
/// the zoom factor alone: dividing by `pixels_per_point` would divide the native scale
/// in a second time, and on a HiDPI display the pen would land at half the mouse.
fn frame_zoom(ctx: &egui::Context) -> Option<f32> {
    let zoom = ctx.zoom_factor();
    (zoom.is_finite() && zoom > 0.0).then_some(zoom)
}

/// Surface coordinates into egui points, at drain time: every position the events
/// carry is in the compositor's surface space, and the frame's zoom is what turns it
/// into points (see [`to_egui`]).
fn to_points(events: &mut [egui::Event], zoom: Option<f32>) {
    let Some(zoom) = zoom else { return };
    for event in events {
        match event {
            egui::Event::PointerMoved(pos) | egui::Event::PointerButton { pos, .. } => {
                *pos = egui::pos2(pos.x / zoom, pos.y / zoom);
            }
            _ => {}
        }
    }
}

impl PenInput {
    /// Move the events the reader has converted since the last frame into `raw`, and
    /// resolve the eraser end from the drained batch. The zoom is the receiving
    /// frame's: the surface coordinates the events carry become egui points here,
    /// with this frame's zoom — not in the reader thread, whose zoom may be stale by
    /// the time this frame drains (see [`to_egui`]).
    ///
    /// Called from `raw_input_hook`, before egui reads this frame's input.
    fn take_events(&self, raw: &mut egui::RawInput, zoom: Option<f32>) {
        let pending = self.stash.lock().map(|mut stash| std::mem::take(&mut *stash));
        match pending {
            Ok(pending) => {
                // The latch in delivery order: an eraser-kind frame latches it, a
                // `proximity_out` clears it — so a latch held across empty frames
                // (tip down, no events) survives, and one whose pen left cannot
                // stick. The frame's resolution rides the drained batch itself:
                // any eraser-kind event in it, or the latch it started with and
                // kept. Relaxed ordering: both flags are this thread's state; the
                // reader never touches them.
                //
                // The leave-and-flip edge (an eraser `proximity_out` and a pen-end
                // `proximity_in` in one drain): the leave's own frame kind is the
                // eraser, so this frame still resolves the eraser — the leave is
                // part of what it processes — and the next drain resolves the pen
                // end. Either way the latch was cleared by the leave, so nothing
                // sticks.
                let mut latched = self.eraser_latch.load(Ordering::Relaxed);
                let mut any_eraser = false;
                for Stashed { kind, prox_out, mut events } in pending {
                    to_points(&mut events, zoom);
                    raw.events.extend(events);
                    if kind == ToolKind::Eraser {
                        any_eraser = true;
                        latched = true;
                    }
                    if prox_out {
                        latched = false;
                    }
                }
                self.eraser_latch.store(latched, Ordering::Relaxed);
                self.frame_eraser.store(any_eraser || latched, Ordering::Relaxed);
            }
            // A poisoned lock means the reader panicked; the app's pen path dies with
            // it, never the app (the never-crash rule) — the stash is dropped, and a
            // latch from before the panic is cleared with it: nothing will ever end
            // it now, and a stuck eraser end would erase every pen stroke.
            Err(_) => {
                self.eraser_latch.store(false, Ordering::Relaxed);
                self.frame_eraser.store(false, Ordering::Relaxed);
            }
        }
    }

    /// Whether the pen's eraser end is active for this frame's input: the drained
    /// batch's resolution (any eraser-kind event in it, or a latch the drain kept —
    /// a tip held down delivers no events between frames), falling back to the
    /// reader's snapshot when neither — hover with no events keeps using the
    /// snapshot.
    fn eraser_end_for_frame(&self) -> bool {
        self.frame_eraser.load(Ordering::Relaxed) || eraser_end_active(Some(self.snapshot()))
    }

    /// The latest pen state, for the tool layer (pressure, tilt, eraser flag).
    fn snapshot(&self) -> PenSnapshot {
        self.snapshot.lock().map(|guard| *guard).unwrap_or_default()
    }
}

/// The sidecar and everything its events need to reach egui.
///
/// Dropping it stops the sidecar (which stops and joins its reader), and the reader
/// thread here exits when the channel disconnects.
pub struct Tablet {
    #[allow(dead_code)] // ownership: the sidecar must outlive the reader thread below
    sidecar: TabletSidecar,
    input: PenInput,
}

impl Tablet {
    /// Bind the tablet sidecar on `display` and start its egui reader.
    ///
    /// Every failure is the caller's one log line (the fallback rule): this returns
    /// the error and starts nothing. `ctx` is only asked for repaints and the current
    /// modifier state; both are thread-safe. The sidecar borrows the display behind
    /// `display` for as long as it lives (the sidecar crate's Display lifetime
    /// contract): the returned [`Tablet`] must drop before the display's owner does.
    pub fn start(ctx: &egui::Context, display: &impl HasDisplayHandle) -> Result<Self, wayland_tablet::SidecarError> {
        // `Thread`: the sidecar's own reader dispatches its queue off the app's frame,
        // so event latency does not ride the compositor's frame cadence (the spike
        // measured the difference; `pump()` is a no-op in this mode).
        let (sidecar, events) = TabletSidecar::spawn_with(display, PumpMode::Thread)?;
        // Display lifetime (the sidecar crate's contract): the sidecar borrows the
        // display behind `display` until it drops. The `Tablet` returned here carries
        // it; `enable_wayland_pen` stores that in the app struct, which drops with the
        // app — before eframe's windowing environment, which owns winit's display
        // connection, goes away. If the sidecar ever moves elsewhere, keep that order:
        // sidecar first, display owner last.
        let input = PenInput::default();
        let reader_stash = Arc::clone(&input.stash);
        let reader_snapshot = Arc::clone(&input.snapshot);
        let reader_ctx = ctx.clone();
        // Exits when the sidecar is dropped (the channel disconnects); a send error on
        // `request_repaint` cannot happen for a live app.
        std::thread::Builder::new()
            .name("wayland-tablet-egui".into())
            .spawn(move || reader_loop(events, reader_ctx, reader_stash, reader_snapshot))?;
        Ok(Self { sidecar, input })
    }

    /// Move the events the reader has converted since the last frame into `raw`, and
    /// resolve the eraser end from the drained batch. See [`PenInput::take_events`];
    /// the zoom is this frame's, read off `ctx`.
    pub fn take_events(&self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.input.take_events(raw, frame_zoom(ctx));
    }

    /// The latest pen state, for the tool layer (pressure, tilt, eraser flag).
    pub fn snapshot(&self) -> PenSnapshot {
        self.input.snapshot()
    }

    /// Whether the pen's eraser end is active for this frame's input: resolved from
    /// the drained batch (see [`Self::take_events`]), falling back to the snapshot
    /// for hover with no events. See [`PenInput::eraser_end_for_frame`].
    pub fn eraser_end_for_frame(&self) -> bool {
        self.input.eraser_end_for_frame()
    }
}

/// What the reader stashes for one pen event: its converted egui events, plus the
/// identity the drain resolves the eraser from — the frame's tool kind (every event
/// of a frame shares it — `PenFrame` is `Copy` — and the canvas must resolve the
/// eraser from the delivered batch, not the reader's later snapshot) and whether
/// the event left the surface, which ends the eraser latch. Also the snapshot
/// update the event carries; the reader applies it, the tests drop it.
fn stash_event(ev: &PenEvent, modifiers: egui::Modifiers) -> (Stashed, SnapshotUpdate) {
    let (events, update) = to_egui(ev, modifiers);
    (Stashed { kind: ev.frame().tool, prox_out: matches!(ev, PenEvent::ProximityOut { .. }), events }, update)
}

/// The reader: [`PenEvent`]s from the sidecar's channel into egui events, one repaint
/// per batch. `recv` blocks until the sidecar is dropped, which ends the thread.
fn reader_loop(events: Receiver<PenEvent>, ctx: egui::Context, stash: Arc<Mutex<Vec<Stashed>>>, snapshot: Arc<Mutex<PenSnapshot>>) {
    for ev in events {
        let (stashed, update) = stash_event(&ev, current_modifiers(&ctx));
        // Lock poisoning would mean this thread panicked; the lock is only touched
        // here and in the frame's take, both panic-free, so this cannot happen — and
        // if it did, into_inner keeps the app's pen path alive (never-crash rule).
        if let Ok(mut stash) = stash.lock() {
            stash.push(stashed);
        }
        if let Ok(mut snapshot) = snapshot.lock() {
            update.apply(&mut snapshot);
        }
        ctx.request_repaint();
    }
    // Channel closed: the sidecar was dropped (app shutdown), not an error.
    log::debug!("wayland pen sidecar channel closed; pen input ends");
}

/// The frame's modifier state, read from any thread (`Context::input` takes the
/// input-state lock); the modifiers a pen press carries are whatever the keyboard held.
fn current_modifiers(ctx: &egui::Context) -> egui::Modifiers {
    ctx.input(|input| input.modifiers)
}

/// What one [`PenEvent`] becomes in egui, plus the snapshot update it carries.
///
/// Positions: tablet-v2's `motion` coordinates are surface-local in the compositor's
/// global space — exactly what `wl_pointer.motion` reports (both are "surface-local"
/// per tablet-v2.xml and wayland.xml) — and that is not egui's point space. The
/// division happens at drain time ([`PenInput::take_events`]) with the zoom factor the
/// receiving frame holds (`egui::Context::zoom_factor`): egui's points are winit's
/// physical pixels over `pixels_per_point`, and `pixels_per_point` is the zoom factor
/// times the native scale (`egui_winit::pixels_per_point`) — so surface-local, which is
/// already logical, divides by the zoom factor alone. Not in the reader thread: the
/// zoom can change between the reader's conversion and this frame's drain. So the
/// events here carry the surface coordinates as they came.
fn to_egui(ev: &PenEvent, modifiers: egui::Modifiers) -> (Vec<egui::Event>, SnapshotUpdate) {
    let mut out = Vec::new();
    let mut update = SnapshotUpdate::default();
    let frame = ev.frame();
    let at = egui::Pos2::new(frame.x as f32, frame.y as f32);
    match ev {
        // Entry and every move place the pointer first, so `pointer.latest_pos` is
        // current even when the frame carries the press too.
        PenEvent::ProximityIn { .. } => {
            out.push(egui::Event::PointerMoved(at));
            update.proximate = Some(true);
            update.kind = Some(frame.tool);
        }
        PenEvent::Move { .. } => out.push(egui::Event::PointerMoved(at)),
        PenEvent::Down { .. } => {
            out.push(egui::Event::PointerMoved(at));
            out.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers });
            update.contact = Some(true);
            update.kind = Some(frame.tool);
        }
        PenEvent::Up { .. } => {
            out.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers });
            update.contact = Some(false);
        }
        PenEvent::Button { button, pressed, .. } => {
            // The barrel button acts as the secondary button (what Acrobat's pen side
            // button does). Any other code is dropped with a note: tablet-v2 forwards
            // raw Linux event codes and this build binds only the stylus button
            // (`BTN_STYLUS`); `BTN_STYLUS2` stays unbound.
            if *button == BTN_STYLUS {
                out.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: *pressed, modifiers });
            } else {
                log::trace!("unbound pen button code {button:#06x} ({pressed}) dropped");
            }
        }
        PenEvent::ProximityOut { .. } => {
            // The pen left the surface: egui's pointer goes with it, like a mouse
            // leaving the window.
            out.push(egui::Event::PointerGone);
            update.proximate = Some(false);
            update.contact = Some(false);
        }
    }
    // Pressure and tilt ride every event that carries a frame snapshot.
    update.pressure = Some(frame.pressure as f32);
    update.tilt = Some((frame.tilt_x as f32, frame.tilt_y as f32));
    (out, update)
}

/// The fields one [`PenEvent`] refreshes; `None` keeps the previous value.
#[derive(Default)]
struct SnapshotUpdate {
    kind: Option<ToolKind>,
    pressure: Option<f32>,
    tilt: Option<(f32, f32)>,
    proximate: Option<bool>,
    contact: Option<bool>,
}

impl SnapshotUpdate {
    fn apply(self, snapshot: &mut PenSnapshot) {
        let Self { kind, pressure, tilt, proximate, contact } = self;
        if let Some(kind) = kind {
            snapshot.tool = kind;
        }
        if let Some(pressure) = pressure {
            snapshot.pressure = pressure;
        }
        if let Some((tilt_x, tilt_y)) = tilt {
            snapshot.tilt_x = tilt_x;
            snapshot.tilt_y = tilt_y;
        }
        if let Some(proximate) = proximate {
            snapshot.proximate = proximate;
        }
        if let Some(contact) = contact {
            snapshot.contact = contact;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wayland_tablet::PenFrame;

    /// A frame snapshot like the sidecar sends: position, pressure, tilt, tool.
    fn frame(tool: ToolKind, x: f64, y: f64) -> PenFrame {
        PenFrame { tool, surface: Some(7), x, y, pressure: 0.5, tilt_x: -10.0, tilt_y: 20.0, time_ms: 1000 }
    }

    fn no_mods() -> egui::Modifiers {
        egui::Modifiers::default()
    }

    /// Entry and moves place the pointer, at the frame's own coordinates (surface
    /// logical space is egui's point space; see `to_egui`).
    #[test]
    fn entry_and_move_place_the_pointer() {
        let (events, _) = to_egui(&PenEvent::ProximityIn { frame: frame(ToolKind::Pen, 12.5, 3.25), serial: 1 }, no_mods());
        assert_eq!(events, vec![egui::Event::PointerMoved(egui::pos2(12.5, 3.25))], "{events:?}");
        let (events, _) = to_egui(&PenEvent::Move { frame: frame(ToolKind::Pen, 20.0, 8.0) }, no_mods());
        assert_eq!(events, vec![egui::Event::PointerMoved(egui::pos2(20.0, 8.0))], "{events:?}");
    }

    /// Tip down: the press lands at the frame's position (after the pointer is placed
    /// there), and the release goes to the same button.
    #[test]
    fn tip_down_and_up_are_primary_press_and_release() {
        let (events, _) = to_egui(&PenEvent::Down { frame: frame(ToolKind::Pen, 4.0, 5.0), serial: 2 }, no_mods());
        assert_eq!(
            events,
            vec![
                egui::Event::PointerMoved(egui::pos2(4.0, 5.0)),
                egui::Event::PointerButton { pos: egui::pos2(4.0, 5.0), button: egui::PointerButton::Primary, pressed: true, modifiers: no_mods() },
            ],
            "{events:?}"
        );
        let (events, _) = to_egui(&PenEvent::Up { frame: frame(ToolKind::Pen, 4.0, 5.0) }, no_mods());
        assert_eq!(
            events,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(4.0, 5.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: no_mods()
            }],
            "{events:?}"
        );
    }

    /// The frame's modifier state rides the button events it is given.
    #[test]
    fn button_events_carry_the_given_modifiers() {
        let modifiers = egui::Modifiers { shift: true, ..no_mods() };
        let (events, _) = to_egui(&PenEvent::Down { frame: frame(ToolKind::Pen, 0.0, 0.0), serial: 3 }, modifiers);
        let Some(egui::Event::PointerButton { modifiers: carried, .. }) = events.last() else {
            panic!("a button event: {events:?}");
        };
        assert_eq!(*carried, modifiers);
    }

    /// The stylus barrel button is the secondary button, pressed and released.
    #[test]
    fn barrel_button_is_secondary() {
        for pressed in [true, false] {
            let (events, _) = to_egui(&PenEvent::Button { frame: frame(ToolKind::Pen, 6.0, 7.0), button: BTN_STYLUS, pressed }, no_mods());
            assert_eq!(
                events,
                vec![egui::Event::PointerButton { pos: egui::pos2(6.0, 7.0), button: egui::PointerButton::Secondary, pressed, modifiers: no_mods() }],
                "{events:?}"
            );
        }
    }

    /// Any other barrel button code is dropped: tablet-v2 forwards raw Linux event
    /// codes, and only the stylus button is bound.
    #[test]
    fn other_button_codes_are_dropped() {
        let (events, _) = to_egui(&PenEvent::Button { frame: frame(ToolKind::Pen, 0.0, 0.0), button: 0x141, pressed: true }, no_mods());
        assert_eq!(events, Vec::<egui::Event>::new(), "{events:?}");
    }

    /// The real `BTN_STYLUS` code, fed raw (not through the constant): 0x14b is the
    /// kernel's stylus barrel button (`input-event-codes.h`; 0x140 is
    /// `BTN_TOOL_PEN`, a tool kind) and reaches egui as the secondary button. With
    /// the code the constant used to carry by mistake (0x140), this feeds an unbound
    /// code and nothing comes out.
    #[test]
    fn the_raw_stylus_code_is_the_secondary_button() {
        for pressed in [true, false] {
            let (events, _) = to_egui(&PenEvent::Button { frame: frame(ToolKind::Pen, 6.0, 7.0), button: 0x14b, pressed }, no_mods());
            assert_eq!(
                events,
                vec![egui::Event::PointerButton { pos: egui::pos2(6.0, 7.0), button: egui::PointerButton::Secondary, pressed, modifiers: no_mods() }],
                "{events:?}"
            );
        }
    }

    /// The pen leaving the surface clears egui's pointer, like a mouse leaving.
    #[test]
    fn leaving_the_surface_clears_the_pointer() {
        let (events, _) = to_egui(&PenEvent::ProximityOut { frame: frame(ToolKind::Pen, 0.0, 0.0) }, no_mods());
        assert_eq!(events, vec![egui::Event::PointerGone], "{events:?}");
    }

    /// The eraser end over the surface is the override the canvas maps to the eraser
    /// tool; the pen end, and the eraser end after leaving the surface, never are.
    #[test]
    fn the_eraser_end_is_active_only_while_proximate() {
        let eraser = PenSnapshot { tool: ToolKind::Eraser, proximate: true, ..Default::default() };
        assert!(eraser_end_active(Some(eraser)));
        let pen = PenSnapshot { tool: ToolKind::Pen, proximate: true, ..Default::default() };
        assert!(!eraser_end_active(Some(pen)));
        let left = PenSnapshot { tool: ToolKind::Eraser, ..Default::default() };
        assert!(!eraser_end_active(Some(left)));
        assert!(!eraser_end_active(None));
    }

    /// A drained pen event, exactly as the reader stashes it: through the shared
    /// builder (`stash_event`), so the tests and the reader cannot diverge.
    fn stashed(ev: &PenEvent) -> Stashed {
        stash_event(ev, no_mods()).0
    }

    fn drain(input: &PenInput) -> egui::RawInput {
        drain_at(input, None)
    }

    /// The drain with the frame's zoom (`None`: 1x, the surface coordinates as points).
    fn drain_at(input: &PenInput, zoom: Option<f32>) -> egui::RawInput {
        let mut raw = egui::RawInput::default();
        input.take_events(&mut raw, zoom);
        raw
    }

    /// The drain turns the surface coordinates the reader stashed into egui points
    /// with the receiving frame's zoom: at 2x, (200, 100) surface is (100, 50) points;
    /// drained at 1x, the same batch comes through unchanged. The zoom is this frame's,
    /// not the reader thread's: it can change between the two.
    #[test]
    fn the_drain_divides_surface_coordinates_by_the_frames_zoom() {
        let input = PenInput::default();
        {
            let mut stash = input.stash.lock().unwrap();
            stash.push(stashed(&PenEvent::Move { frame: frame(ToolKind::Pen, 200.0, 100.0) }));
        }
        let raw = drain_at(&input, Some(2.0));
        assert_eq!(raw.events, vec![egui::Event::PointerMoved(egui::pos2(100.0, 50.0))], "{:?}", raw.events);
        {
            let mut stash = input.stash.lock().unwrap();
            stash.push(stashed(&PenEvent::Move { frame: frame(ToolKind::Pen, 200.0, 100.0) }));
        }
        let raw = drain_at(&input, None);
        assert_eq!(raw.events, vec![egui::Event::PointerMoved(egui::pos2(200.0, 100.0))], "{:?}", raw.events);
    }

    /// Native scale 2 with zoom 1: the surface coordinates are logical already, so the
    /// conversion divides by the zoom factor alone (`frame_zoom`, which reads
    /// `ctx.zoom_factor()`) — (200, 100) stays (200, 100). Dividing by
    /// `pixels_per_point` (the zoom factor times the native scale) would halve it, and
    /// the pen would land at half the mouse on a HiDPI display.
    #[test]
    fn native_scale_2_with_zoom_1_leaves_the_surface_position_unchanged() {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.viewports.entry(raw.viewport_id).and_modify(|v| v.native_pixels_per_point = Some(2.0));
        ctx.run_ui(raw, |_| {}).drop_without_applying_deltas();
        assert_eq!(ctx.native_pixels_per_point(), Some(2.0));
        assert_eq!(ctx.zoom_factor(), 1.0);
        assert_eq!(ctx.pixels_per_point(), 2.0);
        assert_eq!(frame_zoom(&ctx), Some(1.0), "zoom 1: the surface coordinates pass through");

        let input = PenInput::default();
        {
            let mut stash = input.stash.lock().unwrap();
            stash.push(stashed(&PenEvent::Move { frame: frame(ToolKind::Pen, 200.0, 100.0) }));
        }
        let drained = drain_at(&input, frame_zoom(&ctx));
        assert_eq!(drained.events, vec![egui::Event::PointerMoved(egui::pos2(200.0, 100.0))], "{:?}", drained.events);
    }

    /// The eraser tap whose whole life fits between two frames — enter, press,
    /// release and leave drained in one batch: the batch itself resolves as the
    /// eraser end (the reader's snapshot may already be past it — the reader is not
    /// running here, so the fallback snapshot is the untouched default), and the
    /// latch ends at the leave, so the next frame's empty drain is the pen.
    #[test]
    fn an_eraser_tap_between_frames_resolves_from_the_drained_batch() {
        let input = PenInput::default();
        {
            let mut stash = input.stash.lock().unwrap();
            stash.push(stashed(&PenEvent::ProximityIn { frame: frame(ToolKind::Eraser, 4.0, 5.0), serial: 1 }));
            stash.push(stashed(&PenEvent::Down { frame: frame(ToolKind::Eraser, 4.0, 5.0), serial: 2 }));
            stash.push(stashed(&PenEvent::Up { frame: frame(ToolKind::Eraser, 4.0, 5.0) }));
            stash.push(stashed(&PenEvent::ProximityOut { frame: frame(ToolKind::Eraser, 0.0, 0.0) }));
        }
        drain(&input);
        assert!(input.eraser_end_for_frame(), "the drained batch resolves as the eraser end: the release click erases");
        drain(&input);
        assert!(!input.eraser_end_for_frame(), "the latch cleared after the proximity_out");
    }

    /// The latch's life: pen-end events never set it; the first eraser event does;
    /// it holds across the empty drains of frames that only draw; the leave's batch
    /// still resolves eraser (its kind rides the frame), and after it the latch is
    /// cleared — it cannot stick.
    #[test]
    fn the_eraser_latch_holds_while_the_tip_is_down_and_ends_at_the_leave() {
        let input = PenInput::default();
        {
            let mut stash = input.stash.lock().unwrap();
            stash.push(stashed(&PenEvent::ProximityIn { frame: frame(ToolKind::Pen, 1.0, 1.0), serial: 1 }));
            stash.push(stashed(&PenEvent::Down { frame: frame(ToolKind::Pen, 1.0, 1.0), serial: 2 }));
        }
        drain(&input);
        assert!(!input.eraser_end_for_frame(), "pen-end events never latch");
        {
            let mut stash = input.stash.lock().unwrap();
            stash.push(stashed(&PenEvent::ProximityIn { frame: frame(ToolKind::Eraser, 2.0, 2.0), serial: 3 }));
            stash.push(stashed(&PenEvent::Down { frame: frame(ToolKind::Eraser, 2.0, 2.0), serial: 4 }));
        }
        drain(&input);
        assert!(input.eraser_end_for_frame(), "latched from the first eraser event");
        drain(&input);
        assert!(input.eraser_end_for_frame(), "an empty drain keeps the latch: a tip held down delivers no events");
        {
            let mut stash = input.stash.lock().unwrap();
            stash.push(stashed(&PenEvent::ProximityOut { frame: frame(ToolKind::Eraser, 0.0, 0.0) }));
        }
        drain(&input);
        assert!(input.eraser_end_for_frame(), "the leave frame's batch still carries the eraser kind");
        drain(&input);
        assert!(!input.eraser_end_for_frame(), "cleared after the proximity_out — it cannot stick");
    }

    /// The snapshot update: tool kind, pressure, tilt, proximity and contact, across
    /// the events that carry them.
    #[test]
    fn snapshots_track_the_pen_state() {
        let mut snapshot = PenSnapshot::default();
        let (_, update) = to_egui(&PenEvent::ProximityIn { frame: frame(ToolKind::Pen, 1.0, 1.0), serial: 4 }, no_mods());
        update.apply(&mut snapshot);
        assert_eq!(
            snapshot,
            PenSnapshot { tool: ToolKind::Pen, pressure: 0.5, tilt_x: -10.0, tilt_y: 20.0, proximate: true, contact: false },
            "{snapshot:?}"
        );

        // An eraser end touching down: the kind flips, contact sets — the flag the
        // tool layer reads.
        let (_, update) = to_egui(&PenEvent::Down { frame: frame(ToolKind::Eraser, 1.0, 1.0), serial: 5 }, no_mods());
        update.apply(&mut snapshot);
        assert_eq!(snapshot.tool, ToolKind::Eraser, "{snapshot:?}");
        assert!(snapshot.contact, "{snapshot:?}");

        // Leaving clears contact and proximity, keeps the last tool and axes.
        let (_, update) = to_egui(&PenEvent::ProximityOut { frame: frame(ToolKind::Eraser, 0.0, 0.0) }, no_mods());
        update.apply(&mut snapshot);
        assert!(!snapshot.proximate && !snapshot.contact, "{snapshot:?}");
        assert_eq!(snapshot.tool, ToolKind::Eraser, "{snapshot:?}");
    }
}
