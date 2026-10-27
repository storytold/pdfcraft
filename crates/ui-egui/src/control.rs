//! The UI control channel (M3.9): lets an agent see and drive the running app.
//!
//! **Opt-in only.** Nothing here runs unless the app is started with `--control <file>` (or a
//! test attaches a [`ControlClient`]). The transport ([`serve`]) listens on loopback only, on a
//! random port, and every connection must first present the random token written to `<file>`.
//!
//! How it works:
//! - [`ControlPlugin`], an egui plugin, keeps a copy of the AccessKit tree from every frame's
//!   output (the widget tree with ids, roles, labels, rects and states) and injects queued input
//!   events (AccessKit click actions, pointer events, keys, text) at the start of later frames.
//! - Requests reach the app through a channel; `PdfCraftApp` answers them at the start of a
//!   frame, so they see and change exactly what the user would.
//!
//! Methods (JSON in, JSON out):
//! - `ui.state`: open documents, active document, page, zoom, page errors, mode, panels, dialog,
//!   notice and observed window dimensions in logical points.
//! - `ui.inspect {query?, role?, focused?, limit?}`: widgets in tree order with `id` (a string), `role`, `label`,
//!   `value`, `rect` (points), `enabled`, `toggled`, `selected`, `clickable`, `focusable`, `focused`, `depth`.
//! - `ui.click {id}` | `{label}` | `{x, y, button?}`: click a widget (by its AccessKit action) or a
//!   point (`button`: primary, secondary, or middle).
//! - `ui.focus {id}` | `{label}`: focus one enabled widget through its AccessKit action.
//! - `ui.resize {width, height}`: request a native window size (320–8192 logical points per dimension).
//!   The window manager may constrain it; `ui.state.window` reports the observed result.
//! - `ui.quit {}`: normal native exit, or `quitting: false, needs_confirmation: true` and the
//!   existing save prompt when any document has unsaved edits or typing. No discard parameter.
//! - `ui.move {x, y}`: move the pointer without pressing a button (e.g. latched autoscroll).
//! - `ui.drag {from: [x, y], to: [x, y], steps?, modifiers?, button?}`: press, move and release (drawing
//!   comments, selecting text, moving comments). `ui.state` reports `pages_on_screen` to aim at.
//! - `ui.type {text}`, `ui.key {key, modifiers?}`: keyboard input to the focused widget / app.
//! - `ui.command {id}`: run a registry command (as the menu would). `ui.commands` lists them.
//! - `ui.set {key, value}`: view options (`--page`, `--zoom`, …), plus persistent preferences.
//!   `select` accepts physical page numbers and ranges such as `1,12,16-18`; blank clears the
//!   explicit selection. This is the same bounded parser as the Organize Pages field; invalid
//!   input leaves the previous selection unchanged. `ui.state.active.selected_pages` reports it.
//!   `default-mode=all|read|edit|convert|sign` applies to future PDF opens; `mode` overrides it
//!   for this session without changing the saved preference. `ui.state.default_mode` reports it.
//!   `theme=light|dark|system` changes the saved theme preference; `ui.state.theme_preference`
//!   reports the choice and `ui.state.theme` reports the resolved light/dark colours.
//!   `comment-tool-pinned=true|false` remembers the annotation quick bar's pin across restarts;
//!   `ui.state.comment_tool_pinned` reports the same preference.
//! - `ui.open {path}`: open a file. A file whose tab is already open focuses it
//!   (`"focused": true`) instead of adding a tab; native opens report `pending: true` until
//!   they complete and are listed in `ui.state.pending_opens`; only a failed open replies `Err`.
//! - `ui.screenshot {region?}`: PNG of the window (base64), optionally cropped to a rect.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use egui::accesskit::{self, Action, NodeId};
use serde_json::{Value, json};

mod host;
mod pending;
mod plugin;
mod requests;
mod screenshot;
#[cfg(not(target_arch = "wasm32"))]
mod transport;

pub use plugin::ControlPlugin;
use plugin::expire_input;
use requests::Handled;
#[cfg(test)]
use requests::key_batches;
use screenshot::{Geometry, MAX_IMAGE_BYTES, retried_reservation};
#[cfg(not(target_arch = "wasm32"))]
pub use transport::{Endpoint, serve};

pub type Reply = Result<Value, String>;

/// Seconds to wait for frames (input to be handled, a screenshot to be taken) before reporting
/// that the window is not being drawn. `App::logic` also runs while a window is hidden,
/// minimized or fully covered, where egui's own input clock does not advance; every deadline
/// here uses a monotonic `web_time::Instant` so hidden logic cannot stall or restart it.
const NOT_DRAWN_TIMEOUT: f64 = 5.0;

/// Return the deadline used to reject control requests that wait for a drawn window.
fn timeout() -> std::time::Duration {
    std::time::Duration::from_secs_f64(NOT_DRAWN_TIMEOUT)
}

fn not_drawn(what: &str) -> String {
    format!(
        "{what} did not happen within {NOT_DRAWN_TIMEOUT:.0} s: the window is not being drawn (it may be hidden, minimized or covered); bring it to the front and retry"
    )
}

/// Return the actionable error text for a control request whose window was not drawn in time.
fn still_starting() -> String {
    format!("the app was still starting after {NOT_DRAWN_TIMEOUT:.0} s; the control request was not applied, retry when the window is drawn")
}

/// The app finished starting, but only after this request's one admission deadline had already
/// passed. Report that instead of claiming the app is still starting.
fn startup_deadline_expired() -> String {
    format!(
        "the app finished starting after this control request's {NOT_DRAWN_TIMEOUT:.0} s deadline; the request was not applied, retry now that the window is drawn"
    )
}

/// One request from a client, answered on `reply`.
pub struct ControlRequest {
    /// Admission time on the sending thread, including time waiting for the next app tick.
    pub since: web_time::Instant,
    pub method: String,
    pub params: Value,
    pub reply: Sender<Reply>,
}

/// Sends requests to the app (from the transport thread or a test).
#[derive(Clone)]
pub struct ControlClient {
    tx: Sender<ControlRequest>,
    ctx: egui::Context,
}

impl ControlClient {
    /// Queue a request; the reply arrives on the returned receiver once the app has handled it.
    pub fn send(&self, method: &str, params: Value) -> Receiver<Reply> {
        let (reply, rx) = channel();
        let req = ControlRequest { since: web_time::Instant::now(), method: method.into(), params, reply };
        if let Err(e) = self.tx.send(req) {
            let _ = e.0.reply.send(Err("the app has closed".into()));
        }
        self.ctx.request_repaint();
        rx
    }
}

/// State shared between the plugin (frame hooks) and the app.
#[derive(Default)]
struct Shared {
    nodes: HashMap<NodeId, accesskit::Node>,
    root: Option<NodeId>,
    focus: Option<NodeId>,
    /// Queued synthetic-input requests, each with one event batch per UI pass.
    inject: VecDeque<RequestBatches>,
    /// Accepted (not discarded) egui passes, counted in `output_hook`.
    passes: u64,
    /// The document/focus generation the app last admitted input for.
    epoch: u64,
    /// Input requests whose queued events became stale before a UI pass consumed them.
    dropped: Vec<u64>,
    next_batch: u64,
    /// A batch admitted before its deadline may finish this pass. Expiry runs before the next
    /// input hook (or in hidden logic), never after egui has already calculated its interaction.
    in_pass: bool,
    expired: Vec<u64>,
    cleanup: Vec<egui::Event>,
    cancel_pointer: bool,
    cancel_gestures: bool,
}

/// One queued synthetic-input request: its batches are injected one per UI pass.
struct RequestBatches {
    request: u64,
    /// The document/dialog/focus generation the request was admitted for.
    epoch: u64,
    /// The first batch was injected, so a focus/dialog change does not reject its own remaining
    /// batches. The admission deadline still cancels a partially delivered gesture.
    started: bool,
    since: web_time::Instant,
    /// Inverse presses still held by this request. Pointer entries are ownership markers for
    /// cancellation only: replaying one on timeout would click or drop the abandoned gesture.
    release: Vec<egui::Event>,
    batches: VecDeque<Vec<egui::Event>>,
}

enum Pending {
    /// Answer after `frames` more accepted frames (the injected input has been handled by then).
    Frames {
        /// Answer once this many accepted UI passes have run.
        until_pass: u64,
        /// The queued batches this request depends on; a stale one fails the request.
        batches: Vec<u64>,
        since: web_time::Instant,
        reply: Sender<Reply>,
        value: Value,
    },
    Screenshot {
        slot: craft_ui::capture::CaptureSlot,
        /// The one-shot callback, kept until the settled pass that issues it.
        callback: Option<egui::ScreenshotCallback>,
        region: Option<egui::Rect>,
        reply: Sender<Reply>,
        since: web_time::Instant,
        geometry: Geometry,
        /// Bytes reserved in the aggregate budget for this request.
        reserved: usize,
        attempts: u8,
    },
}

/// A request that arrived while the app could not yet run a real UI pass (fonts not ready).
struct DeferredRequest {
    request: ControlRequest,
    /// When this request reached the control queue. One admission deadline runs from here through
    /// startup, input injection, pending frames and screenshot issuance: dispatching it later must
    /// not start a fresh 5 s in each phase, and a request that used up the deadline while the app
    /// was starting is never applied against a later UI state.
    since: web_time::Instant,
}

/// The app half: the request queue and requests waiting for later frames.
pub struct Control {
    rx: Receiver<ControlRequest>,
    shared: Arc<Mutex<Shared>>,
    pending: Vec<Pending>,
    /// Requests admitted while the app cannot yet run a real UI pass (fonts not ready).
    deferred: VecDeque<DeferredRequest>,
}

/// Install the control channel on `ctx`: returns the app half and a client.
pub fn attach(ctx: &egui::Context) -> (Control, ControlClient) {
    let shared = Arc::new(Mutex::new(Shared::default()));
    ctx.add_plugin(ControlPlugin { shared: shared.clone() });
    ctx.enable_accesskit();
    let (tx, rx) = channel();
    (Control { rx, shared, pending: Vec::new(), deferred: VecDeque::new() }, ControlClient { tx, ctx: ctx.clone() })
}

/// What a request needs from the app (implemented by `PdfCraftApp`).
pub(crate) trait Host {
    /// Return the UI-state snapshot reported by `ui.state`.
    fn state(&self) -> Value;
    /// Run one registered command by id and return its reply.
    fn command(&mut self, id: &str) -> Reply;
    /// Apply or reset the side panel layout from a control request.
    fn panel(&mut self, reset: bool, params: &Value) -> Reply;
    /// List the commands available to the control channel.
    fn commands(&self) -> Value;
    /// Set one application option from a control request.
    fn set(&mut self, key: &str, value: &str) -> Reply;
    /// Open a document path requested through the control channel.
    fn open(&mut self, path: &str) -> Reply;
    /// Request application exit; returns whether it closed without asking for confirmation.
    fn quit(&mut self, ctx: &egui::Context) -> bool;

    /// A generation that changes when the input target (document, dialog, focus) does.
    ///
    /// Synthetic input admitted for one generation must not be replayed into another.
    fn input_epoch(&self) -> u64 {
        0
    }

    /// A generation for the captured content (document and dialog identity only).
    ///
    /// Screenshots do not depend on keyboard focus, so a field gaining focus must not discard a
    /// pending capture. Hosts that do not distinguish the two reuse the input generation.
    fn capture_epoch(&self) -> u64 {
        self.input_epoch()
    }

    /// Whether the app can apply a real UI pass yet (fonts installed, no early return).
    fn accepts_input(&self) -> bool {
        true
    }

    /// Drop transient app-owned grabs before widgets observe the canceled pointer as up.
    fn cancel_pointer_gestures(&mut self) {}
}

impl Control {
    /// Handle new requests and finish waiting ones. Call at the start of every frame (`logic`).
    pub(crate) fn tick(&mut self, ctx: &egui::Context, host: &mut impl Host) {
        self.set_epoch(host.input_epoch());
        self.finish_pending(ctx, host);
        let ready = host.accepts_input();
        if !ready {
            // Expiry applies while the app is still starting too: a request that waited through
            // the startup gate must fail at the same deadline instead of running whenever fonts
            // arrive.
            let now = web_time::Instant::now();
            while self.deferred.front().is_some_and(|deferred| now.duration_since(deferred.since) > timeout()) {
                if let Some(deferred) = self.deferred.pop_front() {
                    let _ = deferred.request.reply.send(Err(still_starting()));
                }
            }
        } else {
            // Deferred requests reached the queue before anything still in the channel, so they
            // are dispatched first: one client's requests keep their arrival order. Each carries
            // its own admission timestamp, so startup time counts against its deadline.
            while let Some(deferred) = self.deferred.pop_front() {
                if web_time::Instant::now().duration_since(deferred.since) > timeout() {
                    let _ = deferred.request.reply.send(Err(startup_deadline_expired()));
                } else {
                    self.dispatch(ctx, host, deferred.request, deferred.since);
                }
            }
        }
        while let Ok(req) = self.rx.try_recv() {
            // One admission timestamp per request: queued input, pending frames and an admitted
            // screenshot all measure their deadline from the sending thread, including time
            // waiting in this channel, never from a later phase.
            let since = req.since;
            if web_time::Instant::now().duration_since(since) > timeout() {
                let _ = req.reply.send(Err(if ready { startup_deadline_expired() } else { still_starting() }));
                continue;
            }
            if !ready {
                if self.deferred.len() < 32 {
                    self.deferred.push_back(DeferredRequest { request: req, since });
                } else {
                    let _ = req.reply.send(Err("the app is still starting; too many control requests queued".into()));
                }
                continue;
            }
            self.dispatch(ctx, host, req, since);
        }
        if !self.pending.is_empty() || !self.deferred.is_empty() {
            ctx.request_repaint();
            // A hidden or occluded window runs `logic` without a UI pass. Ask for one so queued
            // input and screenshots can be applied; the screenshot command itself also paints.
            //
            // The shared fork's web host does not implement `RequestPaintWhileHidden` yet: a
            // hidden browser tab still times out rather than falsely succeeding. Recorded as a
            // pending fork prerequisite, not qualified here.
            #[cfg(not(target_arch = "wasm32"))]
            if ctx.input(|i| i.viewport().visible()) != Some(true) {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            }
        }
    }

    /// Refresh the input-target generation before a pass consumes queued synthetic input.
    ///
    /// `raw_input_hook` runs before the plugin's `input_hook`, so a dialog opened by the
    /// previous pass already invalidates a request that has not started yet, instead of a pass
    /// later. Only that request is rejected; requests already started finish their batches.
    pub(crate) fn set_epoch(&self, epoch: u64) {
        if let Ok(mut s) = self.shared.lock() {
            s.epoch = epoch;
        }
    }

    /// Issue admitted captures after a settled UI pass. Call at the end of `App::ui`.
    pub(crate) fn finish_frame(&mut self, ctx: &egui::Context, host: &mut impl Host) {
        if !host.accepts_input() || ctx.will_discard() {
            return;
        }
        let current = Geometry::current(ctx, host.capture_epoch());
        let mut reserved_total = self.reserved_bytes();
        let mut kept = Vec::new();
        for pending in self.pending.drain(..) {
            let mut entry = pending;
            if let Pending::Screenshot { slot, callback, reply, region, geometry, reserved, attempts, since } = &mut entry
                && callback.is_some()
            {
                if web_time::Instant::now().duration_since(*since) > timeout() {
                    slot.cancel();
                    reserved_total = reserved_total.saturating_sub(*reserved);
                    let _ = reply.send(Err(not_drawn("the screenshot")));
                    continue;
                }
                if geometry.epoch != current.epoch {
                    slot.cancel();
                    reserved_total = reserved_total.saturating_sub(*reserved);
                    let _ = reply.send(Err("the document or dialog changed before the screenshot could be issued; it was not saved".into()));
                    continue;
                }
                if *geometry != current {
                    // Resize, DPI or zoom between admission and issuance: retry once with the
                    // real geometry rather than cropping with stale numbers.
                    if *attempts >= 1 {
                        slot.cancel();
                        reserved_total = reserved_total.saturating_sub(*reserved);
                        let _ = reply.send(Err("the window geometry changed while the screenshot was being prepared; it was not saved".into()));
                        continue;
                    }
                    if region.as_ref().is_some_and(|region| !current.contains_region(*region)) {
                        slot.cancel();
                        reserved_total = reserved_total.saturating_sub(*reserved);
                        let _ = reply.send(Err("the region must lie inside the window".into()));
                        continue;
                    }
                    let Some(bytes) = current.image_bytes() else {
                        slot.cancel();
                        reserved_total = reserved_total.saturating_sub(*reserved);
                        let _ = reply.send(Err("the window has no capturable size".into()));
                        continue;
                    };
                    let Some(new_total) = retried_reservation(reserved_total, *reserved, bytes) else {
                        slot.cancel();
                        reserved_total = reserved_total.saturating_sub(*reserved);
                        let _ = reply.send(Err("the screenshot no longer fits the screenshot budget".into()));
                        continue;
                    };
                    reserved_total = new_total;
                    slot.cancel();
                    let (new_slot, new_callback) = craft_ui::capture::CaptureSlot::new(ctx, egui::ViewportId::ROOT, MAX_IMAGE_BYTES);
                    *slot = new_slot;
                    *callback = Some(new_callback);
                    *geometry = current;
                    *reserved = bytes;
                    *attempts += 1;
                }
                if let Some(callback) = callback.take() {
                    // The screenshot command itself requests a paint, including while hidden.
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(callback));
                }
            }
            kept.push(entry);
        }
        self.pending = kept;
        if !self.pending.is_empty() {
            #[cfg(not(target_arch = "wasm32"))]
            if ctx.input(|i| i.viewport().visible()) != Some(true) {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            }
        }
    }

    /// `since` is the request's arrival time: the one admission deadline it was admitted under.
    fn dispatch(&mut self, ctx: &egui::Context, host: &mut impl Host, req: ControlRequest, since: web_time::Instant) {
        match self.handle(ctx, host, &req.method, &req.params, since) {
            Handled::Now(r) => {
                let _ = req.reply.send(r);
            }
            Handled::AfterFrames(frames, value) => {
                let until_pass = self.passes() + u64::from(frames);
                self.pending.push(Pending::Frames { until_pass, batches: Vec::new(), since, reply: req.reply, value });
            }
            Handled::AfterInput { batches, frames, value } => {
                let until_pass = self.passes() + u64::from(frames) + 1;
                self.pending.push(Pending::Frames { until_pass, batches, since, reply: req.reply, value });
            }
            Handled::Screenshot(region) => self.admit_screenshot(ctx, host, region, req.reply, since),
        }
    }

    /// Expire queued input and collect the batches a pending reply must react to.
    fn take_finished_shared(&self, ctx: &egui::Context, host: &mut impl Host, now: web_time::Instant) -> (Vec<u64>, Vec<u64>, bool) {
        let (dropped, expired, in_pass) = self
            .shared
            .lock()
            .map(|mut s| {
                if !s.in_pass {
                    expire_input(&mut s, now);
                }
                if std::mem::take(&mut s.cancel_gestures) {
                    host.cancel_pointer_gestures();
                }
                (std::mem::take(&mut s.dropped), std::mem::take(&mut s.expired), s.in_pass)
            })
            .unwrap_or_default();
        if !expired.is_empty() {
            ctx.request_repaint(); // Run the queued key/modifier cleanup and pointer cancellation.
        }
        (dropped, expired, in_pass)
    }

    fn passes(&self) -> u64 {
        self.shared.lock().map(|s| s.passes).unwrap_or(0)
    }
}

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod pointer_tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn coordinate_clicks_activate_once_with_slow_frames() {
        struct Probe {
            control: Option<Control>,
            client: Option<ControlClient>,
            host: crate::PdfCraftApp,
            primary: usize,
            secondary: usize,
            disabled: usize,
            discard: bool,
            discarded: bool,
            events: Vec<(f64, egui::PointerButton, bool, egui::Pos2)>,
        }
        fn request(h: &mut Harness<'_, Probe>, c: &ControlClient, params: Value) -> Reply {
            let rx = c.send("ui.click", params);
            for _ in 0..8 {
                let before = h.ctx.input(|i| i.time);
                h.step();
                assert!(h.ctx.input(|i| i.time) > before, "simulated input time advances");
                if let Ok(reply) = rx.try_recv() {
                    return reply;
                }
            }
            panic!("coordinate click did not reply within eight steps");
        }
        let max_duration = egui::Context::default().options(|o| o.input_options.max_click_duration);
        let slow_dt = (max_duration + 0.25) as f32;
        assert!(f64::from(slow_dt) > max_duration);
        for discard in [false, true] {
            // This probe draws its own UI with the harness's fonts, not PdfCraftApp::ui.
            // Exercise the readiness gate explicitly before allowing this host's input.
            let state = Probe {
                control: None,
                client: None,
                host: crate::PdfCraftApp::new(),
                primary: 0,
                secondary: 0,
                disabled: 0,
                discard,
                discarded: false,
                events: Vec::new(),
            };
            let mut h = Harness::builder().with_size(egui::vec2(320.0, 200.0)).with_step_dt(slow_dt).build_ui_state(
                |ui, state: &mut Probe| {
                    if state.control.is_none() {
                        let (control, client) = attach(ui.ctx());
                        state.control = Some(control);
                        state.client = Some(client);
                        ui.ctx().options_mut(|o| o.max_passes = if state.discard { 2 } else { 1 }.try_into().unwrap());
                    }
                    state.control.as_mut().unwrap().tick(ui.ctx(), &mut state.host);
                    ui.input(|i| {
                        for event in &i.events {
                            if let egui::Event::PointerButton { pos, button, pressed, .. } = event {
                                assert!(state.events.len() < 32, "bounded pointer trace");
                                state.events.push((i.time, *button, *pressed, *pos));
                            }
                        }
                    });
                    let response = ui.button("Click probe");
                    if response.clicked() {
                        state.primary += 1;
                        if state.discard && !state.discarded {
                            state.discarded = true;
                            ui.ctx().request_discard("control click must not replay across passes");
                        }
                    }
                    if response.secondary_clicked() {
                        state.secondary += 1;
                    }
                    let disabled = ui.add_enabled(false, egui::Button::new("Disabled probe"));
                    if disabled.clicked() || disabled.secondary_clicked() {
                        state.disabled += 1;
                    }
                },
                state,
            );
            h.run_steps(3);
            let c = h.state().client.as_ref().unwrap().clone();
            let probe_pos = h.get_by_label("Click probe").rect().center();
            let waiting = c.send("ui.click", json!({"x":probe_pos.x,"y":probe_pos.y}));
            h.run_steps(3);
            assert!(waiting.try_recv().is_err(), "unready host must defer control input");
            assert_eq!(h.state().primary, 0);
            assert!(h.state().events.is_empty());
            h.state_mut().host.fonts_ready = true;
            let mut ready_reply = None;
            for _ in 0..8 {
                h.step();
                if let Ok(reply) = waiting.try_recv() {
                    ready_reply = Some(reply);
                    break;
                }
            }
            assert!(ready_reply.expect("ready host completes deferred click").is_ok());
            assert_eq!(h.state().primary, 1);
            h.state_mut().primary = 0;
            h.state_mut().events.clear();
            h.state_mut().discarded = false;
            assert_eq!(h.ctx.options(|o| o.input_options.max_click_duration), max_duration);
            let pos = h.get_by_label("Click probe").rect().center();
            assert_eq!((h.state().primary, h.state().secondary, h.state().disabled), (0, 0, 0));
            let reply = request(&mut h, &c, json!({"x":pos.x,"y":pos.y})).unwrap();
            assert_eq!(reply["clicked"], json!([pos.x, pos.y]));
            println!("slow control click discard={discard} duration_limit={max_duration} step_dt={slow_dt} pointer_events={:?}", h.state().events);
            let events = &h.state().events;
            assert_eq!(
                events.iter().map(|(_, button, pressed, point)| (*button, *pressed, *point)).collect::<Vec<_>>(),
                [(egui::PointerButton::Primary, true, pos), (egui::PointerButton::Primary, false, pos)]
            );
            let elapsed = events[1].0 - events[0].0;
            assert!(elapsed == 0.0 || elapsed >= f64::from(slow_dt) - 1e-6, "the pair is atomic or crosses a simulated slow frame");
            assert_eq!(h.state().primary, 1, "a coordinate click activates the real button once on slow frames");
            h.run_steps(3);
            assert_eq!(h.state().primary, 1, "later passes must not replay the click");
            assert_eq!(h.state().discarded, discard);
            request(&mut h, &c, json!({"x":pos.x,"y":pos.y,"button":"secondary"})).unwrap();
            assert_eq!((h.state().primary, h.state().secondary), (1, 1));
            let disabled = h.get_by_label("Disabled probe").rect().center();
            request(&mut h, &c, json!({"x":disabled.x,"y":disabled.y})).unwrap();
            assert_eq!((h.state().primary, h.state().secondary, h.state().disabled), (1, 1, 0));
            let disabled_id = h
                .state()
                .control
                .as_ref()
                .unwrap()
                .shared
                .lock()
                .unwrap()
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some("Disabled probe"))
                .unwrap()
                .0
                .0;
            assert!(request(&mut h, &c, json!({"id":disabled_id})).unwrap_err().contains("disabled"));
            assert!(h.state().host.views.is_empty(), "pointer probes create no document edits");
            assert_eq!(h.ctx.options(|o| o.input_options.max_click_duration), max_duration);
        }
    }
}
