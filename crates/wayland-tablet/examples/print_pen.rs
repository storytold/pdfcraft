//! The Task 0 spike driver: a bare winit window (no egui) plus the sidecar, printing
//! one line per [`PenEvent`] and, on exit, the latency/rate summary the task asked
//! for (events/sec, max inter-event gap, delay against the compositor's own
//! `frame` timestamps).
//!
//! The window is mapped: a solid-color softbuffer buffer is presented on every
//! redraw, so the run shows up in `niri msg windows` and the compositor has a real
//! focused surface to route the pen to.
//!
//! Run on Wayland with a pen (the XP-Pen on niri, per the task):
//!
//! ```text
//! WAYLAND_TABLET_PUMP=main   cargo run -p wayland-tablet --example print_pen
//! WAYLAND_TABLET_PUMP=thread cargo run -p wayland-tablet --example print_pen
//! ```
//!
//! What to watch for (the plan's five questions):
//!
//! 1. spawning at all (a failure here is an answer too),
//! 2. the `delay=` numbers on each pen line (compositor `frame` ms → arrival);
//!    thread mode should stay well under ~10 ms where main mode rides the frame
//!    cadence (that difference is the finding),
//! 3. `surface=` — `proximity_in` carries winit's `wl_surface`; anything but a
//!    crash while those lines appear means no panic,
//! 4. `WINIT ...` lines while the pen is down — pointer emulation is left
//!    untouched and counted (`emulated=` in the summary),
//! 5. run it for five minutes, through focus changes and pen↔eraser switches.
//!
//! `Esc` (or the window close button) ends the run and prints the summary.

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use wayland_tablet::{PenEvent, PumpMode, TabletSidecar};

/// The compositor's `frame` timestamps are CLOCK_MONOTONIC ms (tablet-v2.xml
/// `frame.time`: “the time of the event with millisecond granularity”); this is the
/// same clock, read locally.
/// The run's delay measure, in the protocol clock's own width.
///
/// The `frame` event's `time` is a u32 of the monotonic clock in milliseconds
/// (tablet-v2.xml `frame`, `time` argument; "The time of the event with
/// millisecond granularity"). The subtraction must happen in u32 and wrap where
/// the protocol clock does: in u64 it would not, and a host up longer than 2^32
/// ms (about 49.7 days) would report a multi-day delay for a pen that moved 10 ms
/// ago — 2^32+100 against 90 is 10, not 2^32+10.
fn pen_delay_ms(now_ms: u64, frame_ms: u32) -> u64 {
    u64::from((now_ms as u32).wrapping_sub(frame_ms))
}

#[cfg(test)]
mod tests {
    use super::pen_delay_ms;

    /// The delay wraps where the protocol clock does: across the u32 wrap the
    /// subtraction still lands on the true small delay (2^32+100 against 90 is
    /// 10 ms), and well below the wrap it is the plain difference.
    #[test]
    fn the_delay_wraps_where_the_protocol_clock_does() {
        assert_eq!(pen_delay_ms(4_294_967_296 + 100, 90), 10);
        assert_eq!(pen_delay_ms(100, 90), 10);
        assert_eq!(pen_delay_ms(u64::from(u32::MAX), 0), u64::from(u32::MAX));
    }
}

fn mono_ms() -> u64 {
    let ts = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    // Monotonic clock is never negative; the protocol truncated its ms to u32 and we
    // widen it back.
    u64::try_from(ts.tv_sec).unwrap_or(0) * 1000 + u64::from(u32::try_from(ts.tv_nsec / 1_000_000).unwrap_or(0))
}

/// The run's measurements, everything the summary prints.
#[derive(Default)]
struct Stats {
    count: u64,
    first: Option<u64>,
    last: Option<u64>,
    max_gap_ms: u64,
    max_delay_ms: u64,
    /// Arrival times of recent winit `CursorMoved` events, for pairing pen moves
    /// against winit's own pointer feed (same hardware, two paths).
    cursors: VecDeque<u64>,
    pairs: u64,
    max_cursor_delta_ms: Option<i64>,
    /// `MouseInput` seen while the pen is in proximity: the emulation count
    /// (plan question 4).
    emulated: u64,
    /// Every `MouseInput` winit emitted, both buttons: context for the count above.
    winit_all: u64,
}

impl Stats {
    fn note_pen(&mut self, now: u64, delay_ms: u64) {
        if let Some(last) = self.last {
            self.max_gap_ms = self.max_gap_ms.max(now.saturating_sub(last));
        }
        self.last = Some(now);
        self.first.get_or_insert(now);
        self.count += 1;
        self.max_delay_ms = self.max_delay_ms.max(delay_ms);
    }

    /// The freshest cursor arrival within the pairing window, if any.
    fn pair_with_cursor(&mut self, now: u64) -> Option<i64> {
        // 50 ms: a cursor event for the same physical motion should be almost
        // instant; beyond that the two paths are not talking about the same beat.
        const WINDOW_MS: u64 = 50;
        let cursor = self.cursors.iter().rev().find(|&&t| now.saturating_sub(t) <= WINDOW_MS).copied()?;
        self.pairs += 1;
        let delta = now as i64 - cursor as i64;
        self.max_cursor_delta_ms = Some(self.max_cursor_delta_ms.map_or(delta, |d: i64| d.max(delta)));
        Some(delta)
    }
}

/// The solid fill of the mapped window (softbuffer pixels are `0x00RRGGBB`): a slate
/// blue that cannot be confused with a compositor background.
const FILL: u32 = 0x2E_4A_66;

struct App {
    /// The window, shared with softbuffer through `Rc`: winit 0.30's `Window` is not
    /// `Clone`, but raw-window-handle 0.6 has blanket impls for `Rc<H>`, so one `Rc`
    /// is an owned handle both sides keep alive — ours for `request_redraw`,
    /// softbuffer's for its surface.
    window: Option<Rc<Window>>,
    /// The softbuffer surface: presenting a buffer on it is what maps the window on
    /// Wayland (a bare `Window` never attaches a buffer of its own).
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    sidecar: Option<TabletSidecar>,
    events: Option<Receiver<PenEvent>>,
    stats: Stats,
    mode: PumpMode,
    /// Pen-in-proximity, tracked off our own events: the marker for the emulation
    /// count and for telling pen presses from mouse presses in the WINIT log.
    proximate: bool,
    contact: bool,
}

impl App {
    fn new(mode: PumpMode) -> Self {
        Self { window: None, surface: None, sidecar: None, events: None, stats: Stats::default(), mode, proximate: false, contact: false }
    }

    /// One pen event, one line, stats updated.
    fn on_pen(&mut self, ev: PenEvent) {
        let now = mono_ms();
        let delay = pen_delay_ms(now, ev.frame().time_ms);
        self.stats.note_pen(now, delay);
        let frame = ev.frame();
        let at = format!(
            "t={frame_ms} p={pressure:.3} tilt=({tx:+.1},{ty:+.1})deg delay={delay}ms",
            frame_ms = frame.time_ms,
            pressure = frame.pressure,
            tx = frame.tilt_x,
            ty = frame.tilt_y
        );
        match &ev {
            PenEvent::ProximityIn { serial, .. } => {
                self.proximate = true;
                println!("PEN  +in    tool={:?} surface={:?} serial={serial} {at}", frame.tool, frame.surface);
            }
            PenEvent::Down { serial, .. } => {
                self.contact = true;
                println!("PEN  down   tool={:?} x={:.1} y={:.1} serial={serial} {at}", frame.tool, frame.x, frame.y);
            }
            PenEvent::Up { .. } => {
                self.contact = false;
                println!("PEN  up     x={:.1} y={:.1} {at}", frame.x, frame.y);
            }
            PenEvent::Move { .. } => {
                let cursor = self.stats.pair_with_cursor(now);
                match cursor {
                    Some(delta) => println!("PEN  move   x={:.1} y={:.1} {at} cursorΔ={delta:+}ms vs winit", frame.x, frame.y),
                    None => println!("PEN  move   x={:.1} y={:.1} {at} (no cursor pair)", frame.x, frame.y),
                }
            }
            PenEvent::Button { button, pressed, .. } => {
                // Raw Linux event code by the protocol; the hex is presentation only.
                println!("PEN  btn    code={button:#06x} pressed={pressed} {at}");
            }
            PenEvent::ProximityOut { .. } => {
                self.proximate = false;
                println!("PEN  -out   {at}");
            }
        }
    }

    /// The exit summary (plan questions 2 and 4 to numbers).
    fn summary(&self) {
        let (first, last) = (self.stats.first, self.stats.last);
        let seconds = match (first, last) {
            (Some(first), Some(last)) => f64::from(u32::try_from(last.saturating_sub(first)).unwrap_or(0)) / 1000.0,
            _ => 0.0,
        };
        let rate = if seconds > 0.0 { self.stats.count as f64 / seconds } else { 0.0 };
        println!("---- summary (mode={:?}) ----", self.mode);
        println!("pen events          : {}", self.stats.count);
        if seconds > 0.0 {
            println!("span                : {seconds:.1}s, {rate:.0} events/sec");
        }
        println!("max inter-event gap : {}ms", self.stats.max_gap_ms);
        println!("max compositor delay: {}ms (arrival minus `frame` time)", self.stats.max_delay_ms);
        println!("cursor pairs (<=50ms): {} , max |pen−cursor| = {:?}ms", self.stats.pairs, self.stats.max_cursor_delta_ms);
        println!("winit pointer events seen: {} (emulated while pen in proximity: {})", self.stats.winit_all, self.stats.emulated);
        println!("(pen proximate at exit: {} , contact: {})", self.proximate, self.contact);
    }

    /// Schedule a repaint: `resumed`, `Resized` and `Focused` land here, as does the
    /// loop's own keep-alive nudge in `about_to_wait`.
    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Present the mapped buffer: resize the softbuffer surface to the window's
    /// physical size, fill it with [`FILL`], commit. On Wayland this commit is what
    /// maps the window; on every later redraw it just keeps the color current.
    fn draw(&mut self) {
        let (Some(window), Some(surface)) = (self.window.as_deref(), self.surface.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        // 0×0 (minimized or pre-layout) has no nonzero size to resize to; the next
        // `Resized`/`Focused` redraw draws it at the real size.
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if let Err(err) = surface.resize(width, height) {
            eprintln!("softbuffer resize: {err}");
            return;
        }
        match surface.buffer_mut() {
            Ok(mut buffer) => {
                buffer.fill(FILL);
                if let Err(err) = buffer.present() {
                    eprintln!("softbuffer present: {err}");
                }
            }
            Err(err) => eprintln!("softbuffer buffer: {err}"),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.sidecar.is_some() {
            return; // resumed fires again on some platforms; the sidecar is spawn-once
        }
        let window = match event_loop.create_window(Window::default_attributes().with_title("print_pen")) {
            Ok(window) => window,
            Err(err) => {
                eprintln!("window: {err}");
                event_loop.exit();
                return;
            }
        };
        // Q1 itself: a second Backend on winit's wl_display. Any error printed here
        // is an answer, not a failure of the run.
        match TabletSidecar::spawn_with(&window, self.mode) {
            Ok((sidecar, events)) => {
                println!("sidecar spawned (mode={:?}; exit with Esc or close)", self.mode);
                self.sidecar = Some(sidecar);
                self.events = Some(events);
                // Map the window: one `Rc<Window>` for softbuffer's context and
                // surface, one for us. `Context::new` takes the display by value and
                // `Surface::new` the window — both traits hold for `Rc<Window>`.
                let window = Rc::new(window);
                let context = match softbuffer::Context::new(window.clone()) {
                    Ok(context) => context,
                    Err(err) => {
                        eprintln!("softbuffer context: {err}");
                        event_loop.exit();
                        return;
                    }
                };
                match softbuffer::Surface::new(&context, window.clone()) {
                    Ok(surface) => {
                        self.surface = Some(surface);
                        self.window = Some(window.clone());
                        window.request_redraw();
                    }
                    Err(err) => {
                        eprintln!("softbuffer surface: {err}");
                        event_loop.exit();
                    }
                }
            }
            Err(err) => {
                eprintln!("sidecar spawn failed: {err}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(_) => self.request_redraw(),
            WindowEvent::Focused(_) => self.request_redraw(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && event.logical_key == Key::Named(NamedKey::Escape) => {
                event_loop.exit();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let now = mono_ms();
                self.stats.cursors.push_back(now);
                if self.stats.cursors.len() > 64 {
                    self.stats.cursors.pop_front();
                }
                println!("WINIT CursorMoved x={:.0} y={:.0}", position.x, position.y);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // The pen ends as pointer primaries in winit (that is the design of
                // the emulated side); count them while the pen is proximate, so the
                // double-delivery question has a number.
                self.stats.winit_all += 1;
                if self.proximate {
                    self.stats.emulated += 1;
                }
                println!("WINIT MouseInput button={button:?} state={state:?}");
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            return;
        }
        // Main mode: dispatch here, once per frame. Thread mode: the reader fills
        // the channel; this just drains.
        if let Some(err) = self.sidecar.as_ref().and_then(|sidecar| sidecar.pump().err()) {
            eprintln!("sidecar pump: {err}");
        }
        // Drain the channel snapshot-style: `self.events` out first, so the borrow
        // of the side of `self` the pen path writes to ends with the drain. (A
        // disconnected channel just means the sidecar is gone; keep going.)
        if let Some(events) = self.events.take() {
            loop {
                match events.try_recv() {
                    Ok(ev) => self.on_pen(ev),
                    Err(TryRecvError::Empty) => {
                        self.events = Some(events);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        eprintln!("sidecar channel gone (sidecar dropped?)");
                        break;
                    }
                }
            }
        }
        // Keep the loop breathing: redraws throttle to the compositor's cadence, and
        // in main mode that cadence *is* the pump cadence (part of the measured
        // difference between the two PumpModes).
        self.request_redraw();
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.summary();
    }
}

/// `WAYLAND_TABLET_PUMP=thread` → dedicated reader thread; anything else → main.
fn pump_mode() -> PumpMode {
    match std::env::var("WAYLAND_TABLET_PUMP").as_deref() {
        Ok("thread") => PumpMode::Thread,
        _ => PumpMode::Main,
    }
}

fn main() {
    let mode = pump_mode();
    eprintln!("print_pen: WAYLAND_TABLET_PUMP resolves to {mode:?}");
    let event_loop = match EventLoop::new() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            eprintln!("event loop: {err}");
            std::process::exit(1);
        }
    };
    let mut app = App::new(mode);
    if let Err(err) = event_loop.run_app(&mut app) {
        eprintln!("app: {err}");
        std::process::exit(1);
    }
}
