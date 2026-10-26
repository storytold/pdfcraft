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

/// Track only presses that reached egui and have not yet been released. A pointer release is
/// retained as an ownership marker, never as a timeout event.
fn note_pressed(release: &mut Vec<egui::Event>, events: &[egui::Event]) {
    for event in events {
        match event {
            egui::Event::Key { key, pressed: true, .. }
                if !release.iter().any(|event| matches!(event, egui::Event::Key { key: seen, pressed: false, .. } if seen == key)) =>
            {
                release.push(egui::Event::Key { key: *key, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE });
            }
            egui::Event::PointerButton { pos, button, pressed: true, .. }
                if !release
                    .iter()
                    .any(|event| matches!(event, egui::Event::PointerButton { button: seen, pressed: false, .. } if seen == button)) =>
            {
                release.push(egui::Event::PointerButton { pos: *pos, button: *button, pressed: false, modifiers: egui::Modifiers::NONE });
            }
            egui::Event::ModifiersChanged(modifiers)
                if *modifiers != egui::Modifiers::NONE && !release.contains(&egui::Event::ModifiersChanged(egui::Modifiers::NONE)) =>
            {
                release.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            }
            egui::Event::Key { key, pressed: false, .. } => {
                release.retain(|event| !matches!(event, egui::Event::Key { key: seen, .. } if seen == key));
            }
            egui::Event::PointerButton { button, pressed: false, .. } => {
                release.retain(|event| !matches!(event, egui::Event::PointerButton { button: seen, .. } if seen == button));
            }
            egui::Event::ModifiersChanged(modifiers) if *modifiers == egui::Modifiers::NONE => {
                release.retain(|event| !matches!(event, egui::Event::ModifiersChanged(_)));
            }
            _ => {}
        }
    }
}

/// Cancel expired requests before egui computes interaction. Hidden logic calls this too, so
/// replies do not wait for a visible frame. Never complete an abandoned pointer gesture.
fn expire_input(shared: &mut Shared, now: web_time::Instant) {
    shared.inject.retain_mut(|request| {
        if now.duration_since(request.since) <= timeout() {
            return true;
        }
        shared.expired.push(request.request);
        for event in request.release.drain(..) {
            if matches!(event, egui::Event::PointerButton { .. }) {
                shared.cancel_pointer = true;
                shared.cancel_gestures = true;
            } else {
                shared.cleanup.push(event);
            }
        }
        false
    });
}

/// Synthetic input is bounded like screenshots: a local authenticated client must not be able to
/// queue unbounded batches (one frame each) and hold the control channel's memory for ever.
const MAX_QUEUED_BATCHES: usize = 512;

/// The egui plugin half of the control channel.
pub struct ControlPlugin {
    shared: Arc<Mutex<Shared>>,
}

impl egui::Plugin for ControlPlugin {
    fn debug_name(&self) -> &'static str {
        "pdfcraft-control"
    }

    /// Inject admitted control batches into the current pass, dropping stale requests, applying cancellations
    /// and scheduling repaints.
    fn input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        let Ok(mut s) = self.shared.lock() else { return };
        expire_input(&mut s, web_time::Instant::now());
        if std::mem::take(&mut s.cancel_pointer) {
            // input_hook precedes Context::begin_pass. stop_dragging sets the OLD snapshot's
            // drag_stopped; begin_pass replaces that snapshot without a release/click/drop.
            ctx.input_mut(|i| i.pointer = Default::default());
            ctx.stop_dragging();
            egui::DragAndDrop::clear_payload(ctx);
        }
        input.events.append(&mut s.cleanup);
        s.in_pass = true;
        let epoch = s.epoch;
        let Some(front) = s.inject.front_mut() else { return };
        if !front.started {
            if front.epoch != epoch {
                // The document, dialog or focus changed after this request was admitted, before
                // any of its input landed. Drop only this request; later requests are unrelated.
                let request = front.request;
                s.inject.pop_front();
                s.dropped.push(request);
                if !s.inject.is_empty() {
                    ctx.request_repaint();
                }
                return;
            }
            front.started = true;
        }
        // A non-expired request's own earlier batch may have changed focus or opened a dialog.
        // Let it finish normally; expiry above cancels held pointer state instead of completing
        // an abandoned click/drop.
        let events = front.batches.pop_front();
        if let Some(events) = &events {
            // Remember the inverse of what this request presses as it is applied, so a timeout
            // after this batch can still release exactly those inputs.
            note_pressed(&mut front.release, events);
        }
        let done = front.batches.is_empty();
        if done {
            s.inject.pop_front();
        }
        if let Some(events) = events {
            input.events.extend(events);
        }
        if !s.inject.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Record presented passes and the accesskit tree and focus after a pass, so control replies observe only
    /// drawn frames.
    fn output_hook(&mut self, ctx: &egui::Context, output: &mut egui::FullOutput) {
        let Ok(mut s) = self.shared.lock() else { return };
        s.in_pass = false;
        // A discarded pass is layout work, not a frame the agent can observe. Only count passes
        // whose output is actually presented, so a request cannot be answered early. The last
        // pass of a frame is presented even when its own discard request was refused because
        // `max_passes` is reached; egui still reports that request in the output.
        let max_passes = ctx.options(|o| o.max_passes.get());
        let final_pass = output.platform_output.num_completed_passes >= max_passes;
        if output.platform_output.request_discard_reasons.is_empty() || final_pass {
            s.passes += 1;
        }
        let Some(update) = &output.platform_output.accesskit_update else { return };
        s.nodes = update.nodes.iter().cloned().collect();
        s.root = update.tree.as_ref().map(|t| t.root);
        s.focus = Some(update.focus);
    }
}

/// Screenshot admission limits: bounded requests, per-image bytes and aggregate retained bytes.
const MAX_PENDING_SCREENSHOTS: usize = 4;
const MAX_IMAGE_BYTES: usize = 128 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;
/// One image's encode working copies are bounded too: the RGBA buffer fed to the PNG encoder,
/// the encoded PNG, and the base64 string in the reply. They are made one capture at a time,
/// after the retained image budget above has been checked. Peak encoding memory is therefore at
/// most the retained aggregate plus one crop, one PNG and one base64 string; it is not hidden
/// behind the retained-image limit.
const MAX_ENCODED_BYTES: usize = MAX_IMAGE_BYTES;
const MAX_BASE64_BYTES: usize = 192 * 1024 * 1024;

/// Replace one pending capture's reservation with `bytes`, keeping the other reservations.
///
/// The drain retries captures one at a time: using a single total taken before the loop would
/// let several retries each see room that no longer exists together, and the aggregate would
/// grow past [`MAX_TOTAL_BYTES`].
fn retried_reservation(total: usize, old: usize, bytes: usize) -> Option<usize> {
    let without_this = total.saturating_sub(old);
    let new_total = without_this.saturating_add(bytes);
    (bytes <= MAX_IMAGE_BYTES && new_total <= MAX_TOTAL_BYTES).then_some(new_total)
}

/// The window content a screenshot request was admitted and issued for.
///
/// Only the viewport's local size and scale matter: the window's position on the monitor does not
/// change a single captured pixel, and keyboard focus is not part of the captured content.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    size: egui::Vec2,
    pixels_per_point: f32,
    /// Active document and open dialog/prompt identity, never the focused widget.
    epoch: u64,
}

impl Geometry {
    /// Capture the current viewport size and effective pixels-per-point for the request epoch.
    fn current(ctx: &egui::Context, epoch: u64) -> Self {
        // Viewport info is refreshed by `run_logic`; the last UI pass's `viewport_rect` can be
        // stale while the window is hidden. Fall back to it only when the host cannot say. Only
        // its size is kept: `inner_rect` is in monitor space and its origin may move.
        let (rect, native) = ctx.input(|i| {
            let viewport = i.viewport();
            (viewport.inner_rect.unwrap_or_else(|| i.viewport_rect()), viewport.native_pixels_per_point)
        });
        let native = native.or_else(|| ctx.native_pixels_per_point()).unwrap_or(1.0);
        Self { size: rect.size(), pixels_per_point: native * ctx.zoom_factor(), epoch }
    }

    /// Checked bytes an image of this geometry needs, before the capture is issued.
    fn image_bytes(self) -> Option<usize> {
        let width = (self.size.x * self.pixels_per_point).ceil();
        let height = (self.size.y * self.pixels_per_point).ceil();
        if !(width.is_finite() && height.is_finite()) || width <= 0.0 || height <= 0.0 {
            return None;
        }
        checked_image_bytes(width as usize, height as usize)
    }

    /// Whether `region` is a non-empty rectangle inside this viewport, in logical points.
    fn contains_region(self, region: egui::Rect) -> bool {
        region.min.x.is_finite()
            && region.min.y.is_finite()
            && region.max.x.is_finite()
            && region.max.y.is_finite()
            && region.min.x >= 0.0
            && region.min.y >= 0.0
            && region.max.x > region.min.x
            && region.max.y > region.min.y
            && region.max.x <= self.size.x
            && region.max.y <= self.size.y
    }
}

/// Return the byte size of an RGBA image of the given dimensions, or `None` when the multiplication
/// overflows.
fn checked_image_bytes(width: usize, height: usize) -> Option<usize> {
    width.checked_mul(height)?.checked_mul(core::mem::size_of::<egui::Color32>())
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

/// A screenshot pending in the `finish_pending` pass, with ownership of its reply.
struct PendingCapture {
    slot: craft_ui::capture::CaptureSlot,
    callback: Option<egui::ScreenshotCallback>,
    region: Option<egui::Rect>,
    reply: Sender<Reply>,
    since: web_time::Instant,
    geometry: Geometry,
    reserved: usize,
    attempts: u8,
}

impl PendingCapture {
    /// Convert a screenshot request into the pending form that later frames finish.
    fn into_pending(self) -> Pending {
        Pending::Screenshot {
            slot: self.slot,
            callback: self.callback,
            region: self.region,
            reply: self.reply,
            since: self.since,
            geometry: self.geometry,
            reserved: self.reserved,
            attempts: self.attempts,
        }
    }
}

/// The state one `finish_pending` pass needs while resolving pending replies.
struct PendingFinisher {
    now: web_time::Instant,
    passes: u64,
    current: Geometry,
    in_pass: bool,
    dropped: Vec<u64>,
    expired: Vec<u64>,
    reserved_total: usize,
}

impl PendingFinisher {
    /// Answer or keep one queued-input request, preserving its one arrival deadline.
    fn resolve_frames(&self, until_pass: u64, batches: Vec<u64>, since: web_time::Instant, reply: Sender<Reply>, value: Value) -> Option<Pending> {
        if batches.iter().any(|id| self.expired.contains(id)) {
            let _ = reply.send(Err(not_drawn("handling the input")));
            return None;
        }
        if batches.iter().any(|id| self.dropped.contains(id)) {
            let _ = reply.send(Err("the document, dialog or focus changed before the input could be applied; nothing was done".into()));
            return None;
        }
        if self.passes >= until_pass {
            let _ = reply.send(Ok(value));
            return None;
        }
        if !self.in_pass && self.now.duration_since(since) > timeout() {
            let _ = reply.send(Err(not_drawn("handling the input")));
            return None;
        }
        Some(Pending::Frames { until_pass, batches, since, reply, value })
    }

    /// Give one reservation back to the running aggregate budget.
    fn release_reservation(&mut self, reserved: usize) {
        self.reserved_total = self.reserved_total.saturating_sub(reserved);
    }

    /// Resolve one screenshot request, keeping it only while its capture is still pending.
    fn finish_capture(&mut self, ctx: &egui::Context, capture: PendingCapture) -> Option<Pending> {
        // Completion cannot restart or bypass the admission deadline, even when the callback
        // arrived before this delayed tick or a resize would retry it.
        if self.now.duration_since(capture.since) > timeout() {
            capture.slot.cancel();
            self.release_reservation(capture.reserved);
            let _ = capture.reply.send(Err(not_drawn("the screenshot")));
            return None;
        }
        match capture.slot.take() {
            Some(Ok(image)) => self.finish_captured_image(ctx, capture, image),
            Some(Err(error)) => {
                self.release_reservation(capture.reserved);
                let _ = capture.reply.send(Err(format!("the screenshot could not be captured: {error}")));
                None
            }
            None => Some(capture.into_pending()),
        }
    }

    /// Finish a screenshot whose callback produced an image.
    fn finish_captured_image(&mut self, ctx: &egui::Context, capture: PendingCapture, image: Arc<egui::ColorImage>) -> Option<Pending> {
        if self.current.epoch != capture.geometry.epoch {
            // The document/dialog changed: never save a capture of the previous context, and do
            // not retry it into the new one.
            self.release_reservation(capture.reserved);
            let _ = capture.reply.send(Err("the document or dialog changed while the screenshot was being captured; it was not saved".into()));
            return None;
        }
        if self.current.size != capture.geometry.size || self.current.pixels_per_point != capture.geometry.pixels_per_point {
            // A late completion after a resize/DPI change is never encoded: retry once with a
            // fresh slot, else report a defined error.
            return self.retry_captured_image(ctx, capture);
        }
        self.release_reservation(capture.reserved);
        let _ = capture.reply.send(screenshot_png(&image, capture.region, capture.geometry));
        None
    }

    /// Retry one resize/DPI-stale capture with the current geometry.
    fn retry_captured_image(&mut self, ctx: &egui::Context, capture: PendingCapture) -> Option<Pending> {
        if capture.attempts >= 1 {
            self.release_reservation(capture.reserved);
            let _ = capture.reply.send(Err("the window geometry changed while the screenshot was being captured; it was not saved".into()));
            return None;
        }
        if capture.region.as_ref().is_some_and(|region| !self.current.contains_region(*region)) {
            self.release_reservation(capture.reserved);
            let _ = capture.reply.send(Err("the region must lie inside the window".into()));
            return None;
        }
        let Some(bytes) = self.current.image_bytes() else {
            self.release_reservation(capture.reserved);
            let _ = capture.reply.send(Err("the window has no capturable size".into()));
            return None;
        };
        let Some(new_total) = retried_reservation(self.reserved_total, capture.reserved, bytes) else {
            self.release_reservation(capture.reserved);
            let _ = capture.reply.send(Err("the screenshot no longer fits the screenshot budget".into()));
            return None;
        };
        self.reserved_total = new_total;
        let (slot, callback) = craft_ui::capture::CaptureSlot::new(ctx, egui::ViewportId::ROOT, MAX_IMAGE_BYTES);
        Some(Pending::Screenshot {
            slot,
            callback: Some(callback),
            region: capture.region,
            reply: capture.reply,
            since: capture.since,
            geometry: self.current,
            reserved: bytes,
            attempts: capture.attempts + 1,
        })
    }
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

    /// Return the total screenshot bytes reserved by pending captures without overflowing.
    fn reserved_bytes(&self) -> usize {
        self.pending
            .iter()
            .filter_map(|p| if let Pending::Screenshot { reserved, .. } = p { Some(*reserved) } else { None })
            .fold(0usize, usize::saturating_add)
    }

    /// Count the screenshot requests still waiting for a capture.
    fn pending_screenshots(&self) -> usize {
        self.pending.iter().filter(|p| matches!(p, Pending::Screenshot { .. })).count()
    }

    /// Admit one screenshot request within the count, per-image and aggregate limits.
    fn admit_screenshot(
        &mut self,
        ctx: &egui::Context,
        host: &mut impl Host,
        region: Option<egui::Rect>,
        reply: Sender<Reply>,
        since: web_time::Instant,
    ) {
        if self.pending_screenshots() >= MAX_PENDING_SCREENSHOTS {
            let _ = reply.send(Err(format!("too many screenshots are already pending (limit {MAX_PENDING_SCREENSHOTS})")));
            return;
        }
        let geometry = Geometry::current(ctx, host.capture_epoch());
        let Some(bytes) = geometry.image_bytes() else {
            let _ = reply.send(Err("the window has no capturable size".into()));
            return;
        };
        if let Some(region) = region
            && !geometry.contains_region(region)
        {
            let _ = reply.send(Err("the region must lie inside the window".into()));
            return;
        }
        if bytes > MAX_IMAGE_BYTES {
            let _ = reply.send(Err(format!("the screenshot would use {bytes} bytes, over the {MAX_IMAGE_BYTES}-byte image limit")));
            return;
        }
        if self.reserved_bytes().saturating_add(bytes) > MAX_TOTAL_BYTES {
            let _ = reply.send(Err(format!("the screenshot would exceed the {MAX_TOTAL_BYTES}-byte aggregate screenshot budget")));
            return;
        }
        let (slot, callback) = craft_ui::capture::CaptureSlot::new(ctx, egui::ViewportId::ROOT, MAX_IMAGE_BYTES);
        self.pending.push(Pending::Screenshot { slot, callback: Some(callback), region, reply, since, geometry, reserved: bytes, attempts: 0 });
    }

    /// Resolve completed frame and screenshot requests while retaining those that still need a pass or a
    /// retry.
    fn finish_pending(&mut self, ctx: &egui::Context, host: &mut impl Host) {
        let now = web_time::Instant::now();
        let (dropped, expired, in_pass) = self.take_finished_shared(ctx, host, now);
        let mut finisher = PendingFinisher {
            now,
            passes: self.passes(),
            current: Geometry::current(ctx, host.capture_epoch()),
            in_pass,
            dropped,
            expired,
            reserved_total: self.reserved_bytes(),
        };
        let mut keep = Vec::new();
        for pending in std::mem::take(&mut self.pending) {
            let kept = match pending {
                Pending::Frames { until_pass, batches, since, reply, value } => finisher.resolve_frames(until_pass, batches, since, reply, value),
                Pending::Screenshot { slot, callback, region, reply, since, geometry, reserved, attempts } => {
                    finisher.finish_capture(ctx, PendingCapture { slot, callback, region, reply, since, geometry, reserved, attempts })
                }
            };
            if let Some(pending) = kept {
                keep.push(pending);
            }
        }
        self.pending = keep;
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

    /// Queue one request's event batches (one per UI pass) and return its request id.
    ///
    /// The request is stamped once, at admission: all of its batches share the input-target
    /// generation and the caller's single arrival deadline (`since`), so a deferred startup wait
    /// is not restarted here. A poisoned mutex or a full queue is an error, never a silently empty
    /// queue that would let the caller answer `Ok` for input that never happened.
    fn inject(&self, batches: Vec<Vec<egui::Event>>, since: web_time::Instant) -> Result<Vec<u64>, String> {
        let mut s = self.shared.lock().map_err(|_| "control state poisoned".to_string())?;
        let queued: usize = s.inject.iter().map(|request| request.batches.len()).sum();
        if queued.saturating_add(batches.len()) > MAX_QUEUED_BATCHES {
            return Err(format!("too many synthetic input batches are already queued (limit {MAX_QUEUED_BATCHES})"));
        }
        s.next_batch += 1;
        let id = s.next_batch;
        let epoch = s.epoch;
        s.inject.push_back(RequestBatches { request: id, epoch, started: false, since, release: Vec::new(), batches: batches.into() });
        Ok(vec![id])
    }

    /// Queue synthetic input batches and reply only after the requested number of frames has been presented.
    fn after_input(&self, batches: Vec<Vec<egui::Event>>, value: Value, since: web_time::Instant) -> Result<Handled, String> {
        let frames = batches.len() as u32;
        let batches = self.inject(batches, since)?;
        Ok(Handled::AfterInput { batches, frames, value })
    }

    /// Validate and dispatch one control method, rejecting unknown methods and malformed parameters with
    /// actionable errors.
    fn handle(&mut self, ctx: &egui::Context, host: &mut impl Host, method: &str, p: &Value, since: web_time::Instant) -> Handled {
        let str_param = |k: &str| p.get(k).and_then(Value::as_str).ok_or_else(|| format!("{method}: missing string parameter {k}"));
        let r: Result<Handled, String> = (|| match method {
            "ui.state" => {
                let mut state = host.state();
                // Viewport info is fresh in `logic`; the last UI pass's rectangle can be stale.
                let geometry = Geometry::current(ctx, host.capture_epoch());
                let size = geometry.size;
                state["window"] = json!({ "width": size.x, "height": size.y, "pixels_per_point": geometry.pixels_per_point });
                Ok(Handled::Now(Ok(state)))
            }
            "ui.commands" => Ok(Handled::Now(Ok(host.commands()))),
            "ui.panel" | "ui.dock.reset" => Ok(Handled::Now(host.panel(method == "ui.dock.reset", p))),
            "ui.command" if str_param("id")? == "ui.panel" => Ok(Handled::Now(host.panel(false, p.get("params").unwrap_or(p)))),
            "ui.command" => Ok(Handled::Now(host.command(str_param("id")?))),
            "ui.set" => {
                let value = match p.get("value") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                    None => return Err("ui.set: missing parameter value".into()),
                };
                Ok(Handled::Now(host.set(str_param("key")?, &value)))
            }
            "ui.open" => Ok(Handled::Now(host.open(str_param("path")?))),
            "ui.inspect" => Ok(Handled::Now(Ok(self.inspect(p)))),
            "ui.click" => self.click(p, since),
            "ui.focus" => self.focus(p, since),
            "ui.resize" => {
                only_params(p, method, &["width", "height"])?;
                let dimension = |key: &str| -> Result<f32, String> {
                    p.get(key)
                        .and_then(Value::as_f64)
                        .filter(|n| n.is_finite() && (320.0..=8192.0).contains(n))
                        .map(|n| n as f32)
                        .ok_or_else(|| format!("ui.resize: {key} must be between 320 and 8192 logical points"))
                };
                let size = egui::vec2(dimension("width")?, dimension("height")?);
                if cfg!(target_arch = "wasm32") {
                    return Err("ui.resize needs a native window".into());
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                ctx.request_repaint();
                Ok(Handled::AfterFrames(2, json!({ "requested": [size.x, size.y] })))
            }
            "ui.quit" => {
                only_params(p, method, &[])?;
                if cfg!(target_arch = "wasm32") {
                    return Err("ui.quit needs a native window".into());
                }
                let quitting = host.quit(ctx);
                Ok(Handled::Now(Ok(json!({ "quitting": quitting, "needs_confirmation": !quitting }))))
            }
            "ui.drag" => self.drag(p, since),
            "ui.move" => {
                let point = |key: &str| -> Result<f32, String> {
                    p.get(key)
                        .and_then(Value::as_f64)
                        .map(|n| n as f32)
                        .filter(|n| n.is_finite())
                        .ok_or_else(|| format!("ui.move: {key} must be a finite coordinate in points"))
                };
                let pos = egui::pos2(point("x")?, point("y")?);
                self.after_input(vec![vec![egui::Event::PointerMoved(pos)]], json!({ "moved": [pos.x, pos.y] }), since)
            }
            "ui.type" => {
                let text = str_param("text")?.to_string();
                self.after_input(vec![vec![egui::Event::Text(text)]], json!({ "typed": true }), since)
            }
            "ui.key" => {
                let name = str_param("key")?;
                let key = egui::Key::from_name(name)
                    .ok_or_else(|| format!("ui.key: unknown key {name:?} (egui key names: A, Enter, Escape, ArrowDown, F5, …)"))?;
                let modifiers = modifiers(p.get("modifiers"))?;
                self.after_input(key_batches(key, modifiers), json!({ "key": name }), since)
            }
            "ui.screenshot" => {
                let region = match p.get("region") {
                    None | Some(Value::Null) => None,
                    Some(r) => Some(rect_param(r)?),
                };
                ctx.request_repaint();
                Ok(Handled::Screenshot(region))
            }
            other => Err(format!(
                "unknown method {other:?} (ui.state, ui.inspect, ui.click, ui.focus, ui.resize, ui.quit, ui.move, ui.drag, ui.type, ui.key, ui.command, ui.commands, ui.set, ui.open, ui.screenshot)"
            )),
        })();
        r.unwrap_or_else(|e| Handled::Now(Err(e)))
    }

    fn inspect(&self, p: &Value) -> Value {
        let query = p.get("query").and_then(Value::as_str).map(str::to_lowercase);
        let role = p.get("role").and_then(Value::as_str).map(str::to_lowercase);
        let focused = p.get("focused").and_then(Value::as_bool);
        let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(500) as usize;
        let Ok(s) = self.shared.lock() else { return json!({ "widgets": [] }) };
        let mut out = Vec::new();
        let mut total = 0usize;
        let mut stack: Vec<(NodeId, usize)> = s.root.map(|r| vec![(r, 0)]).unwrap_or_default();
        while let Some((id, depth)) = stack.pop() {
            let Some(node) = s.nodes.get(&id) else { continue };
            for c in node.children().iter().rev() {
                stack.push((*c, depth + 1));
            }
            let w = widget(id, node, depth, s.focus == Some(id));
            let text =
                format!("{} {} {}", w["label"].as_str().unwrap_or(""), w["value"].as_str().unwrap_or(""), w["description"].as_str().unwrap_or(""))
                    .to_lowercase();
            let role_ok = role.as_ref().is_none_or(|r| w["role"].as_str().is_some_and(|x| x.to_lowercase() == *r));
            let query_ok = query.as_ref().is_none_or(|q| text.contains(q.as_str()));
            let focus_ok = focused.is_none_or(|want| (s.focus == Some(id)) == want);
            if role_ok && query_ok && focus_ok && id != s.root.unwrap_or(id) {
                total += 1;
                if out.len() < limit {
                    out.push(w);
                }
            }
        }
        json!({ "widgets": out, "count": total, "truncated": total > out.len() })
    }

    /// Press at `from`, move to `to` in `steps` frames, release (drawing, selecting text, moving).
    fn drag(&mut self, p: &Value, since: web_time::Instant) -> Result<Handled, String> {
        let point = |k: &str| -> Result<egui::Pos2, String> {
            match p.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect::<Vec<_>>()).as_deref() {
                Some([x, y]) => Some(egui::pos2(*x as f32, *y as f32))
                    .filter(|pos| pos.is_finite())
                    .ok_or_else(|| format!("ui.drag: {k} must be [x, y], finite coordinates in points")),
                _ => Err(format!("ui.drag: {k} must be [x, y] in points")),
            }
        };
        let (from, to) = (point("from")?, point("to")?);
        if !(to - from).is_finite() {
            return Err("ui.drag: from and to must be a finite distance apart, in points".into());
        }
        let steps = p.get("steps").and_then(Value::as_u64).unwrap_or(8).clamp(1, 200) as usize;
        let modifiers = modifiers(p.get("modifiers"))?;
        let which = pointer_button(p, "ui.drag")?;
        // The events carry the modifiers and so does the input state: gestures that read
        // `ui.input(|i| i.modifiers)` (a Ctrl-drag copy, an Alt-drag column select) see them.
        let held = |mut events: Vec<egui::Event>| {
            if modifiers != egui::Modifiers::NONE {
                events.insert(0, egui::Event::ModifiersChanged(modifiers));
            }
            events
        };
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: which, pressed, modifiers };
        let mut frames = vec![held(vec![egui::Event::PointerMoved(from)]), held(vec![button(from, true)])];
        for k in 1..=steps {
            frames.push(held(vec![egui::Event::PointerMoved(from + (to - from) * (k as f32 / steps as f32))]));
        }
        frames.push(held(vec![button(to, false)]));
        if modifiers != egui::Modifiers::NONE {
            frames.push(vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)]);
        }
        self.after_input(frames, json!({ "dragged": [[from.x, from.y], [to.x, to.y]] }), since)
    }

    /// Click at a point, or on an accesskit widget by id or unique enabled label.
    fn click(&mut self, p: &Value, since: web_time::Instant) -> Result<Handled, String> {
        if let (Some(x), Some(y)) = (p.get("x").and_then(Value::as_f64), p.get("y").and_then(Value::as_f64)) {
            let point = |key: &str, n: f64| {
                Some(n as f32).filter(|n| n.is_finite()).ok_or_else(|| format!("ui.click: {key} must be a finite coordinate in points"))
            };
            let pos = egui::pos2(point("x", x)?, point("y", y)?);
            let which = pointer_button(p, "ui.click")?;
            let button = |pressed| egui::Event::PointerButton { pos, button: which, pressed, modifiers: egui::Modifiers::NONE };
            // Keep hover/layout warmup, then make an instantaneous click even on slow frames.
            let frames = vec![vec![egui::Event::PointerMoved(pos)], vec![button(true), button(false)]];
            return self.after_input(frames, json!({ "clicked": [x, y] }), since);
        }
        let s = self.shared.lock().map_err(|_| "control state poisoned")?;
        let id = match (p.get("id"), p.get("label").and_then(Value::as_str)) {
            (Some(id), _) => {
                let id: u64 =
                    id.as_u64().or_else(|| id.as_str().and_then(|s| s.parse().ok())).ok_or("ui.click: id must be a widget id from ui.inspect")?;
                let id = NodeId(id);
                if !s.nodes.contains_key(&id) {
                    return Err(format!("ui.click: no widget with id {} on screen (run ui.inspect again)", id.0));
                }
                id
            }
            (None, Some(label)) => {
                let want = label.to_lowercase();
                let matches: Vec<NodeId> = s
                    .nodes
                    .iter()
                    .filter(|(_, n)| n.supports_action(Action::Click) && !n.is_disabled() && n.label().is_some_and(|l| l.to_lowercase() == want))
                    .map(|(id, _)| *id)
                    .collect();
                match matches.as_slice() {
                    [one] => *one,
                    [] => return Err(format!("ui.click: no enabled clickable widget labelled {label:?} (try ui.inspect with query)")),
                    many => return Err(format!("ui.click: {} widgets are labelled {label:?}; click one by id", many.len())),
                }
            }
            (None, None) => return Err("ui.click: pass id, label, or x and y".into()),
        };
        let node = &s.nodes[&id];
        if node.is_disabled() {
            return Err(format!("ui.click: widget {} ({}) is disabled", id.0, node.label().unwrap_or("unlabelled")));
        }
        let label = node.label().map(str::to_owned);
        let action = accesskit::ActionRequest { action: Action::Click, target_tree: accesskit::TreeId::ROOT, target_node: id, data: None };
        drop(s);
        let frames = vec![vec![egui::Event::AccessKitActionRequest(action)]];
        self.after_input(frames, json!({ "clicked": id.0.to_string(), "label": label }), since)
    }

    /// Focus an accesskit widget by id or unique enabled label.
    fn focus(&mut self, p: &Value, since: web_time::Instant) -> Result<Handled, String> {
        only_params(p, "ui.focus", &["id", "label"])?;
        let s = self.shared.lock().map_err(|_| "control state poisoned")?;
        let id = match (p.get("id"), p.get("label")) {
            (Some(id), None) => {
                NodeId(id.as_u64().or_else(|| id.as_str().and_then(|s| s.parse().ok())).ok_or("ui.focus: id must be a widget id from ui.inspect")?)
            }
            (None, Some(Value::String(label))) => {
                let matches: Vec<NodeId> = s
                    .nodes
                    .iter()
                    .filter(|(_, n)| n.supports_action(Action::Focus) && !n.is_disabled() && n.label().is_some_and(|l| l.eq_ignore_ascii_case(label)))
                    .map(|(id, _)| *id)
                    .collect();
                match matches.as_slice() {
                    [one] => *one,
                    [] => return Err(format!("ui.focus: no enabled focusable widget labelled {label:?}")),
                    many => return Err(format!("ui.focus: {} widgets are labelled {label:?}; use a widget id", many.len())),
                }
            }
            _ => return Err("ui.focus: pass exactly one widget id or label".into()),
        };
        let node = s.nodes.get(&id).ok_or_else(|| format!("ui.focus: no widget with id {} on screen (run ui.inspect again)", id.0))?;
        if node.is_disabled() || !node.supports_action(Action::Focus) {
            return Err(format!("ui.focus: widget {} is disabled or not focusable", id.0));
        }
        let action = accesskit::ActionRequest { action: Action::Focus, target_tree: accesskit::TreeId::ROOT, target_node: id, data: None };
        drop(s);
        let frames = vec![vec![egui::Event::AccessKitActionRequest(action)]];
        self.after_input(frames, json!({ "focus_requested": id.0.to_string() }), since)
    }
}

enum Handled {
    Now(Reply),
    /// Answer after this many accepted UI passes, without injecting input.
    AfterFrames(u32, Value),
    AfterInput {
        batches: Vec<u64>,
        frames: u32,
        value: Value,
    },
    Screenshot(Option<egui::Rect>),
}

fn widget(id: NodeId, n: &accesskit::Node, depth: usize, focused: bool) -> Value {
    let rect = n.bounds().map(|b| [b.x0, b.y0, b.x1, b.y1]);
    let mut w = json!({
        // A string: 64-bit ids don't survive JSON parsers that use doubles.
        "id": id.0.to_string(),
        "role": format!("{:?}", n.role()),
        "depth": depth,
        "enabled": !n.is_disabled(),
        "clickable": n.supports_action(Action::Click),
        "focusable": n.supports_action(Action::Focus),
    });
    let mut more = serde_json::Map::new();
    let obj = &mut more;
    if let Some(l) = n.label() {
        obj.insert("label".into(), json!(l));
    }
    if let Some(v) = n.value() {
        obj.insert("value".into(), json!(v));
    }
    if let Some(d) = n.description() {
        obj.insert("description".into(), json!(d));
    }
    if let Some(r) = rect {
        obj.insert("rect".into(), json!(r));
    }
    if let Some(t) = n.toggled() {
        obj.insert("toggled".into(), json!(format!("{t:?}").to_lowercase()));
    }
    if let Some(s) = n.is_selected() {
        obj.insert("selected".into(), json!(s));
    }
    if focused {
        obj.insert("focused".into(), json!(true));
    }
    if let Value::Object(m) = &mut w {
        m.extend(more);
    }
    w
}

fn only_params(p: &Value, method: &str, allowed: &[&str]) -> Result<(), String> {
    // The JSON-RPC transport represents omitted params as null.
    if p.is_null() && allowed.is_empty() {
        return Ok(());
    }
    let object = p.as_object().ok_or_else(|| format!("{method}: params must be an object"))?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("{method}: unknown parameter {key:?}"));
    }
    Ok(())
}

fn pointer_button(p: &Value, method: &str) -> Result<egui::PointerButton, String> {
    match p.get("button").and_then(Value::as_str).unwrap_or("primary") {
        "primary" | "left" => Ok(egui::PointerButton::Primary),
        "secondary" | "right" => Ok(egui::PointerButton::Secondary),
        "middle" => Ok(egui::PointerButton::Middle),
        other => Err(format!("{method}: unknown button {other:?} (primary, secondary, middle)")),
    }
}

/// The events for one synthetic key press. egui updates `InputState.modifiers` only from
/// `ModifiersChanged`, not from the `modifiers` field on a key event, so the held set goes in
/// before the press and is released after the key-up. Without this, handlers that read
/// `i.modifiers` (Shift+Tab, ⌘Enter) would see stale state.
fn key_batches(key: egui::Key, modifiers: egui::Modifiers) -> Vec<Vec<egui::Event>> {
    let key_event = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
    let mut press = vec![key_event(true)];
    if modifiers != egui::Modifiers::NONE {
        press.insert(0, egui::Event::ModifiersChanged(modifiers));
    }
    let mut release = vec![key_event(false)];
    if modifiers != egui::Modifiers::NONE {
        release.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    }
    vec![press, release]
}

fn modifiers(v: Option<&Value>) -> Result<egui::Modifiers, String> {
    let mut m = egui::Modifiers::NONE;
    for name in v.and_then(Value::as_array).into_iter().flatten() {
        match name.as_str().unwrap_or("") {
            "command" | "cmd" => m.command = true,
            "shift" => m.shift = true,
            "alt" | "option" => m.alt = true,
            "ctrl" | "control" => m.ctrl = true,
            "mac_cmd" => m.mac_cmd = true,
            other => return Err(format!("ui.key: unknown modifier {other:?} (command, shift, alt, ctrl)")),
        }
    }
    // "command" is ⌘ on macOS and Ctrl elsewhere, as egui expects.
    if m.command {
        if cfg!(target_os = "macos") {
            m.mac_cmd = true;
        } else {
            m.ctrl = true;
        }
    }
    Ok(m)
}

fn rect_param(v: &Value) -> Result<egui::Rect, String> {
    let a: Vec<f32> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).map(|f| f as f32).collect()).unwrap_or_default();
    match a.as_slice() {
        [x0, y0, x1, y1] if x1 > x0 && y1 > y0 => Ok(egui::Rect::from_min_max(egui::pos2(*x0, *y0), egui::pos2(*x1, *y1))),
        _ => Err("region must be [x0, y0, x1, y1] in points".into()),
    }
}

/// Encode (a region of) a screenshot as PNG; the reply carries base64 data and the size.
///
/// The image scale is derived from the returned pixels and the logical viewport the capture was
/// admitted for, never from the current `Context`: egui-wgpu's surface fit may have shrunk the
/// surface (and a resize/DPI change has already been rejected before this call).
fn screenshot_png(image: &egui::ColorImage, region: Option<egui::Rect>, geometry: Geometry) -> Reply {
    let [w, h] = image.size;
    let view = geometry.size;
    if w == 0 || h == 0 {
        return Err("the screenshot has no pixels".into());
    }
    if !(view.x.is_finite() && view.y.is_finite()) || view.x <= 0.0 || view.y <= 0.0 {
        return Err("the window has no capturable size".into());
    }
    if region.is_some_and(|region| !geometry.contains_region(region)) {
        return Err("the region must lie inside the window".into());
    }
    let scale_x = w as f32 / view.x;
    let scale_y = h as f32 / view.y;
    // A fitted surface shrinks both sides by one factor; a different x/y scale means the window
    // changed shape and this image cannot be cropped honestly.
    if (scale_x - scale_y).abs() > 0.01 * scale_x.max(1.0) {
        return Err("the screenshot's aspect ratio does not match the window".into());
    }
    let (x0, y0, x1, y1) = match region {
        Some(r) => {
            let px = |v: f32, scale: f32, max: usize| ((v * scale).round().max(0.0) as usize).min(max);
            (px(r.min.x, scale_x, w), px(r.min.y, scale_y, h), px(r.max.x, scale_x, w), px(r.max.y, scale_y, h))
        }
        None => (0, 0, w, h),
    };
    if x1 <= x0 || y1 <= y0 {
        return Err("the region is outside the window".into());
    }
    let crop_bytes = (x1 - x0)
        .checked_mul(y1 - y0)
        .and_then(|pixels| pixels.checked_mul(core::mem::size_of::<egui::Color32>()))
        .ok_or_else(|| "the screenshot region is too large".to_string())?;
    if crop_bytes > MAX_ENCODED_BYTES {
        return Err(format!("the screenshot region would need {crop_bytes} bytes to encode, over the {MAX_ENCODED_BYTES}-byte encode limit"));
    }
    let mut rgba = Vec::with_capacity(crop_bytes);
    for y in y0..y1 {
        for px in &image.pixels[y * w + x0..y * w + x1] {
            rgba.extend_from_slice(&px.to_srgba_unmultiplied());
        }
    }
    let (cw, ch) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, cw, ch);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    if out.len() > MAX_ENCODED_BYTES {
        return Err(format!("the encoded screenshot is {} bytes, over the {MAX_ENCODED_BYTES}-byte encode limit", out.len()));
    }
    let encoded = out.len().div_ceil(3).checked_mul(4).ok_or_else(|| "the encoded screenshot is too large".to_string())?;
    if encoded > MAX_BASE64_BYTES {
        return Err(format!("the screenshot's base64 reply would be {encoded} bytes, over the {MAX_BASE64_BYTES}-byte limit"));
    }
    use base64::Engine as _;
    Ok(json!({
        "png_base64": base64::engine::general_purpose::STANDARD.encode(out),
        "width": cw,
        "height": ch,
        "pixels_per_point": scale_x,
    }))
}

impl Host for crate::PdfCraftApp {
    /// Cancel any in-progress pointer gesture in every open view.
    fn cancel_pointer_gestures(&mut self) {
        for view in &mut self.views {
            view.cancel_pointer_gestures();
        }
    }

    /// The input target: active document, open dialog/prompt, palette and find state.
    ///
    /// Synthetic input admitted for one generation is never replayed into another document or
    /// dialog. Only these identities are hashed, never text or document content.
    fn input_epoch(&self) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.capture_epoch().hash(&mut hasher);
        // The focused widget is part of the input target: keys typed for one field must not land
        // in another after focus moves. Only the id value is hashed, never text.
        let focused = self.ctx.as_ref().and_then(|ctx| ctx.memory(|m| m.focused())).map(|id| id.value());
        focused.hash(&mut hasher);
        hasher.finish()
    }

    /// The captured content: active document and open dialog/prompt, never keyboard focus.
    fn capture_epoch(&self) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let doc = self.active.and_then(|i| self.views.get(i)).map(|v| v.id.0);
        doc.hash(&mut hasher);
        format!(
            "{:?}/{:?}/{:?}/{:?}/{:?}",
            self.dialog,
            self.password_prompt.is_some(),
            self.palette_open,
            self.close_request.is_some(),
            self.active.and_then(|i| self.views.get(i)).and_then(|v| v.find.as_ref().map(|_| true)),
        )
        .hash(&mut hasher);
        hasher.finish()
    }

    /// Control requests wait for the first UI pass that installs fonts; a font-gated early
    /// return cannot consume injected input or issue a screenshot.
    fn accepts_input(&self) -> bool {
        self.fonts_ready
    }

    fn panel(&mut self, reset: bool, params: &Value) -> Reply {
        crate::side_panels::command(self, reset, params)
    }

    fn quit(&mut self, ctx: &egui::Context) -> bool {
        self.request_quit(ctx)
    }

    fn state(&self) -> Value {
        let active = self.active.and_then(|i| self.views.get(i));
        let docs: Vec<Value> = self
            .views
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                let d = self.session.get(v.id)?;
                Some(json!({ "tab": i, "doc": v.id.0, "name": d.name, "path": d.path, "pages": d.info.pages.len(), "dirty": d.dirty, "active": self.active == Some(i) }))
            })
            .collect();
        #[cfg(not(target_arch = "wasm32"))]
        let pending_opens: Vec<Value> =
            self.pending_opens.iter().map(|pending| json!({ "token": pending.token, "name": pending.name, "path": pending.path })).collect();
        #[cfg(target_arch = "wasm32")]
        let pending_opens: Vec<Value> = Vec::new();
        json!({
            "side_panels": crate::side_panels::state(self),
            "documents": docs,
            "pending_opens": pending_opens,
            "active": active.map(|v| json!({
                "doc": v.id.0,
                "page": v.current + 1,
                "zoom_percent": (v.zoom * 100.0).round(),
                "fit": format!("{:?}", v.fit),
                "layout": format!("{:?}", v.layout),
                "continuous": v.continuous(),
                "page_gaps": v.show_page_gaps,
                "organize": v.organize,
                "grid_zoom": v.grid_zoom(),
                "thumbnail_zoom": v.thumbnail_zoom(),
                // Pages picked in the organize grid or the Pages panel (empty: the current page).
                "selected_pages": v.selected.iter().map(|p| p + 1).collect::<Vec<_>>(),
                "auto_scrolling": v.auto_scrolling(),
                "raster_memory": v.raster_memory(),
                "viewport": [v.viewport_rect().min.x, v.viewport_rect().min.y, v.viewport_rect().max.x, v.viewport_rect().max.y],
                "find_open": v.find.is_some(),
                "page_errors": v.page_errors().iter().map(|(p, e)| json!({ "page": p + 1, "error": e })).collect::<Vec<_>>(),
                // Where pages are on screen (points), to aim ui.click / ui.drag at page content.
                "pages_on_screen": v.visible_page_rects().iter().map(|(p, r)| json!({ "page": p + 1, "rect": [r.min.x, r.min.y, r.max.x, r.max.y] })).collect::<Vec<_>>(),
                "selected_comment": v.comments.selected.map(|(p, i)| json!({ "page": p + 1, "index": i + 1 })),
                "comment_composer_open": v.comments.composer.is_some(),
            })),
            // The Combine files tab: how its files show, and each grid thumbnail's state.
            "combine": {
                "showing": self.combine_showing(),
                "view": self.combine_view.as_str(),
                "zoom": self.combine_zoom,
                "files": self.combine_draft.iter().map(|f| f.name.clone()).collect::<Vec<_>>(),
                "selected": self.combine_selection(),
                "thumbnails": self.combine_thumbnails().iter().map(|t| format!("{t:?}")).collect::<Vec<_>>(),
                "expanded": self.combine_expanded().iter().filter_map(|i| self.combine_draft.get(*i).map(|f| f.name.clone())).collect::<Vec<_>>(),
                "preview": self.combine_preview().map(|(file, page, render)| json!({ "file": file, "page": page + 1, "render": format!("{render:?}") })),
            },
            "quick_tool": match self.quick_tool {
                crate::QuickTool::Measure(t) => format!("measure-{}", t.name()),
                crate::QuickTool::Select => "select".to_string(),
                crate::QuickTool::Hand => "hand".to_string(),
                crate::QuickTool::Crop => "crop".to_string(),
                crate::QuickTool::EnhanceCamera => "enhance-camera".to_string(),
                crate::QuickTool::Redact => "redact".to_string(),
                crate::QuickTool::AddText => "add-text".to_string(),
                crate::QuickTool::EditText => "edit-text".to_string(),
                crate::QuickTool::Link => "link".to_string(),
                crate::QuickTool::SignArea { certify: false } => "sign".to_string(),
                crate::QuickTool::SignArea { certify: true } => "certify".to_string(),
                crate::QuickTool::MarqueeZoom => "marquee-zoom".to_string(),
                crate::QuickTool::Snapshot => "snapshot".to_string(),
                crate::QuickTool::ColumnSelect => "column-select".to_string(),
                crate::QuickTool::Stamp(k) => format!("stamp-{}", k.name().trim_start_matches("PC").to_ascii_lowercase()),
                crate::QuickTool::CustomStamp(i) => format!("custom-stamp-{i}"),
                crate::QuickTool::Fill(f) => format!("fill-{}", f.command().trim_start_matches("sign.fill.")),
                crate::QuickTool::Field(f) => format!("field-{}", f.command().trim_start_matches("form.add.")),
                crate::QuickTool::Comment(t) => t.command().trim_start_matches("comment.").to_string(),
            },
            "home": self.active.is_none(),
            "mode": format!("{:?}", self.mode),
            "default_mode": self.default_mode,
            "comment_tool_pinned": self.comment_prefs.pinned,
            "left_panel": if self.left_open { json!(format!("{:?}", self.left)) } else { Value::Null },
            "tools_side": if self.side_panels.tools_on_right() { "right" } else { "left" },
            "right_panel": self.right.map(|r| format!("{r:?}")),
            "dialog": self.dialog.map(|d| format!("{d:?}")),
            "palette_open": self.palette_open,
            "theme": format!("{:?}", self.theme),
            "theme_preference": self.theme_preference,
            "language": self.language,
            "notice": self.toast.as_ref().map(|t| t.0.clone()),
            "progress": self.progress_notice.as_ref().map(|p| json!({ "label": p.label, "fraction": p.fraction })),
            "password_prompt": self.password_prompt.is_some(),
            "close_prompt": self.close_request.is_some(),
        })
    }

    fn command(&mut self, id: &str) -> Reply {
        let spec = pdfcraft_engine::commands::command(id).ok_or_else(|| format!("unknown command {id:?} (see ui.commands)"))?;
        if !self.command_enabled(spec) {
            return Err(format!("{id} is disabled right now"));
        }
        self.execute(id);
        Ok(json!({ "ran": id }))
    }

    fn commands(&self) -> Value {
        let mut list: Vec<Value> = pdfcraft_engine::commands::COMMANDS
            .iter()
            .map(|c| json!({ "id": c.id, "label": c.label, "menu": c.menu, "enabled": self.command_enabled(c), "shortcut": c.shortcut.map(|s| s.label(cfg!(target_os = "macos"))) }))
            .collect();
        list.push(json!({"id": "ui.panel", "label": "Side panel", "enabled": true, "params": "panel: tools or side; open: boolean; width: number (clamped to the panel's limits)"}));
        json!({ "commands": list })
    }

    fn set(&mut self, key: &str, value: &str) -> Reply {
        self.set_option(key, value).map(|()| json!({ "set": key, "value": value }))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn open(&mut self, path: &str) -> Reply {
        match self.open_path(path) {
            crate::OpenOutcome::Opened => Ok(json!({ "opened": path, "password_prompt": self.password_prompt.is_some() })),
            crate::OpenOutcome::Focused => Ok(json!({ "opened": path, "focused": true })),
            crate::OpenOutcome::Pending => Ok(json!({ "accepted": path, "pending": true })),
            crate::OpenOutcome::Failed => Err(self.toast.as_ref().map(|t| t.0.clone()).unwrap_or_else(|| format!("couldn't open {path}"))),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn open(&mut self, _path: &str) -> Reply {
        Err("ui.open needs a file system; on the web, drop the file on the page".into())
    }
}

// ---- transport (native only) -----------------------------------------------------------------

/// Where a running app's control channel can be reached (written to the `--control` file).
#[cfg(not(target_arch = "wasm32"))]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
}

/// What a connection may do before it authenticates, so an unauthenticated local client cannot
/// hold a thread for ever or exhaust them (#1806).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct Limits {
    /// How long a connection has to authenticate, counted from its acceptance: an absolute
    /// deadline, so a client feeding one byte at a time cannot extend it.
    auth_deadline: std::time::Duration,
    /// How many connections may wait for authentication at once; further ones are refused.
    max_unauthenticated: usize,
    /// How long a write may block on a client that never reads its replies.
    write_timeout: std::time::Duration,
    /// How long a connection refused after a reply is kept half-open, discarding input, so the
    /// reply is not lost to a reset (see [`close_after_reply`]).
    linger: std::time::Duration,
    /// How many refused connections may linger at once. A lingering connection has given back
    /// its place among [`Self::max_unauthenticated`], so this is what bounds their threads;
    /// beyond it a refused connection is closed at once.
    max_lingering: usize,
}

#[cfg(not(target_arch = "wasm32"))]
const LIMITS: Limits = Limits {
    auth_deadline: std::time::Duration::from_secs(10),
    max_unauthenticated: 8,
    write_timeout: std::time::Duration::from_secs(30),
    linger: std::time::Duration::from_secs(1),
    max_lingering: 8,
};

/// One of a limited number of places, given back when dropped: a place among
/// [`Limits::max_unauthenticated`] (given back when the connection authenticates or is refused,
/// and on every way its thread can end), or among [`Limits::max_lingering`].
#[cfg(not(target_arch = "wasm32"))]
struct Slot(std::sync::Arc<std::sync::atomic::AtomicUsize>);

#[cfg(not(target_arch = "wasm32"))]
impl Slot {
    fn take(waiting: &std::sync::Arc<std::sync::atomic::AtomicUsize>, max: usize) -> Option<Self> {
        use std::sync::atomic::Ordering;
        let mut n = waiting.load(Ordering::Acquire);
        loop {
            if n >= max {
                return None;
            }
            match waiting.compare_exchange_weak(n, n.saturating_add(1), Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Slot(waiting.clone())),
                Err(now) => n = now,
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// The reading half of a connection, with the authentication deadline: every read before it
/// gets the time left as its timeout, and none at all once it has passed.
#[cfg(not(target_arch = "wasm32"))]
struct Reading {
    stream: std::net::TcpStream,
    until: Option<std::time::Instant>,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::io::Read for Reading {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(until) = self.until {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the authentication deadline passed"));
            }
            self.stream.set_read_timeout(Some(left))?;
        }
        self.stream.read(buf)
    }
}

/// Close a connection whose last reply has just been written without losing that reply.
///
/// Closing a socket with input still unread makes the kernel send a reset instead of a FIN, and
/// the reset can discard the reply before the client reads it (Linux does): a client still
/// sending when it is refused would never learn why. So send FIN first, then read and discard
/// whatever arrives until the client closes its side or `linger` passes, and only then close.
///
/// Only [`Limits::max_lingering`] connections linger at once: `place` is one of those places,
/// taken by [`refuse`] before the reply was written, and held for the drain only. Without one
/// the connection is closed at once after the FIN, accepting the risk of a reset rather than
/// holding another thread.
#[cfg(not(target_arch = "wasm32"))]
fn close_after_reply(reading: &mut Reading, place: Option<Slot>, linger: std::time::Duration) {
    use std::io::Read;
    let _ = reading.stream.shutdown(std::net::Shutdown::Write);
    let Some(_place) = place else { return };
    reading.until = Some(std::time::Instant::now() + linger);
    let mut sink = [0u8; 1024];
    // Each read is bounded by the time left, and none is made once `linger` has passed.
    while matches!(reading.read(&mut sink), Ok(n) if n > 0) {}
}

/// A connection is being refused before `auth`: give back its place among those waiting to
/// authenticate, so it does not keep a real client out while it lingers, and take a place to
/// linger in if one is free. Done before the reply is written, so a client that has read the
/// reply sees the places as they will be.
#[cfg(not(target_arch = "wasm32"))]
fn refuse(slot: &mut Option<Slot>, lingering: &std::sync::Arc<std::sync::atomic::AtomicUsize>, limits: Limits) -> Option<Slot> {
    drop(slot.take());
    Slot::take(lingering, limits.max_lingering)
}

/// Listen on `127.0.0.1` (random port) and forward authenticated requests to `client`.
///
/// Protocol: newline-delimited JSON-RPC 2.0. The first request on a connection must be
/// `{"method": "auth", "params": {"token": …}}`; anything else closes the connection, and so does
/// a line longer than [`MAX_AUTH_LINE`] before that, or [`LIMITS`] passing without it.
#[cfg(not(target_arch = "wasm32"))]
pub fn serve(client: ControlClient) -> std::io::Result<Endpoint> {
    serve_with(client, LIMITS)
}

/// How long the accepting thread waits after a failed accept. Accepting can keep failing, for
/// example while the process has no file descriptors left, and retrying at once would keep a
/// core busy until it recovers. The single-instance listener waits the same way.
#[cfg(not(target_arch = "wasm32"))]
const ACCEPT_RETRY: std::time::Duration = std::time::Duration::from_millis(100);

/// The connections `incoming` accepts, calling `wait` after each failed accept.
#[cfg(not(target_arch = "wasm32"))]
fn accepted<T>(incoming: impl Iterator<Item = std::io::Result<T>>, mut wait: impl FnMut()) -> impl Iterator<Item = T> {
    incoming.filter_map(move |stream| stream.map_err(|_| wait()).ok())
}

#[cfg(not(target_arch = "wasm32"))]
fn serve_with(client: ControlClient, limits: Limits) -> std::io::Result<Endpoint> {
    use std::io::{BufReader, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let token = random_token()?;
    let expected = token.clone();
    let waiting = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let lingering = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    std::thread::Builder::new().name("pdfcraft-control".into()).spawn(move || {
        for stream in accepted(listener.incoming(), || std::thread::sleep(ACCEPT_RETRY)) {
            let _ = stream.set_write_timeout(Some(limits.write_timeout));
            // Admitted only while fewer than the limit wait to authenticate: refused on the
            // accepting thread, so a refused connection costs no thread at all.
            let Some(slot) = Slot::take(&waiting, limits.max_unauthenticated) else {
                let mut write = stream;
                let e = rpc_error(Value::Null, -32003, "too many connections waiting to authenticate; try again later");
                let _ = writeln!(write, "{e}").and_then(|_| write.flush());
                continue;
            };
            let client = client.clone();
            let expected = expected.clone();
            let lingering = lingering.clone();
            let _ = std::thread::Builder::new().name("pdfcraft-control-conn".into()).spawn(move || {
                // Held until `auth` succeeds; given back however this thread ends.
                let mut slot = Some(slot);
                let Ok(read) = stream.try_clone() else { return };
                let mut write = stream;
                let mut authed = false;
                let mut reader = BufReader::new(Reading { stream: read, until: Some(std::time::Instant::now() + limits.auth_deadline) });
                let mut buf = Vec::new();
                loop {
                    // Until `auth` succeeds, lines are capped at MAX_AUTH_LINE; afterwards they
                    // are read whole, as before.
                    let max = if authed { usize::MAX } else { MAX_AUTH_LINE };
                    match read_line_limited(&mut reader, &mut buf, max) {
                        Ok(LineRead::Line) => {}
                        // The deadline passed before `auth`: say so and close.
                        Err(e) if !authed && matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
                            let secs = limits.auth_deadline.as_secs_f64();
                            let e = rpc_error(Value::Null, -32004, &format!("not authenticated within {secs} s"));
                            let place = refuse(&mut slot, &lingering, limits);
                            if writeln!(write, "{e}").and_then(|_| write.flush()).is_ok() {
                                close_after_reply(reader.get_mut(), place, limits.linger);
                            }
                            break;
                        }
                        // End of stream or a failed read ends the connection, as before.
                        Ok(LineRead::End) | Err(_) => break,
                        Ok(LineRead::TooLong) => {
                            let e = rpc_error(Value::Null, -32600, &format!("request line longer than {max} bytes before auth"));
                            let place = refuse(&mut slot, &lingering, limits);
                            if writeln!(write, "{e}").and_then(|_| write.flush()).is_ok() {
                                close_after_reply(reader.get_mut(), place, limits.linger);
                            }
                            break;
                        }
                    }
                    // Bytes that aren't UTF-8 end the connection, as `BufRead::lines` did.
                    let Ok(line) = std::str::from_utf8(&buf) else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let (reply, close) = match serde_json::from_str::<Value>(line) {
                        // Before `auth`, like any other first request, this closes the connection.
                        Err(e) => (rpc_error(Value::Null, -32700, &format!("parse error: {e}")), !authed),
                        Ok(msg) => {
                            let id = msg.get("id").cloned().unwrap_or(Value::Null);
                            let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
                            let params = msg.get("params").cloned().unwrap_or(Value::Null);
                            if !authed {
                                let ok =
                                    method == "auth" && params.get("token").and_then(Value::as_str).is_some_and(|t| constant_time_eq(t, &expected));
                                authed = ok;
                                if ok {
                                    // Authenticated: no deadline from here on, and its place is free.
                                    reader.get_mut().until = None;
                                    let _ = reader.get_mut().stream.set_read_timeout(None);
                                    drop(slot.take());
                                    (json!({ "jsonrpc": "2.0", "id": id, "result": { "ok": true } }), false)
                                } else {
                                    (rpc_error(id, -32001, "authenticate first: auth {token}"), true)
                                }
                            } else {
                                let rx = client.send(method, params);
                                match rx.recv_timeout(std::time::Duration::from_secs(30)) {
                                    Ok(Ok(v)) => (json!({ "jsonrpc": "2.0", "id": id, "result": v }), false),
                                    Ok(Err(e)) => (rpc_error(id, -32000, &e), false),
                                    Err(_) => (rpc_error(id, -32002, "the app did not answer within 30 s"), false),
                                }
                            }
                        }
                    };
                    // `close` is only ever set before `auth`.
                    let place = if close { refuse(&mut slot, &lingering, limits) } else { None };
                    if writeln!(write, "{reply}").and_then(|_| write.flush()).is_err() {
                        break;
                    }
                    if close {
                        close_after_reply(reader.get_mut(), place, limits.linger);
                        break;
                    }
                }
            });
        }
    })?;
    Ok(Endpoint { port, token })
}

/// Longest line read before a connection has authenticated. The only acceptable request then is
/// `auth`, about a hundred bytes; a longer line is refused without buffering the rest of it.
#[cfg(not(target_arch = "wasm32"))]
const MAX_AUTH_LINE: usize = 4 * 1024;

/// What [`read_line_limited`] found.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, PartialEq, Eq)]
enum LineRead {
    /// A line, now in the buffer (newline and a trailing `\r` stripped).
    Line,
    /// End of stream.
    End,
    /// The line is longer than the limit; at most limit + 1 bytes of it were read.
    TooLong,
}

/// Read one `\n`-terminated line of at most `max` bytes into `buf`.
#[cfg(not(target_arch = "wasm32"))]
fn read_line_limited(reader: &mut impl std::io::BufRead, buf: &mut Vec<u8>, max: usize) -> std::io::Result<LineRead> {
    use std::io::{BufRead, Read};
    buf.clear();
    // `take` bounds what is buffered for one line: never more than `max` + 1 bytes.
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    if reader.take(limit).read_until(b'\n', buf)? == 0 {
        return Ok(LineRead::End);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(LineRead::Line);
    }
    // No newline: either the limit was passed, or this is a last line before end of stream.
    Ok(if buf.len() > max { LineRead::TooLong } else { LineRead::Line })
}

#[cfg(not(target_arch = "wasm32"))]
fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(not(target_arch = "wasm32"))]
fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 128 random bits from the OS, hex-encoded.
#[cfg(not(target_arch = "wasm32"))]
fn random_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod transport_tests {
    /// A failed accept is followed by a wait before the next one, so a listener that keeps
    /// failing does not spin, and the connections around the failures still come through.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_failed_accept_waits_before_the_next() {
        let failed = || Err(std::io::Error::other("no file descriptors left"));
        let incoming = vec![failed(), Ok(1), failed(), failed(), Ok(2)];
        let mut waits = 0;
        let connections: Vec<i32> = super::accepted(incoming.into_iter(), || waits += 1).collect();
        assert_eq!(connections, [1, 2]);
        assert_eq!(waits, 3, "one wait per failed accept");
    }

    /// Before authenticating, a line that isn't JSON gets a parse error and closes the
    /// connection, as [`super::serve`] documents for any first request other than `auth`.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn malformed_input_before_auth_closes_the_connection() {
        use std::io::{BufRead, BufReader, ErrorKind, Write};
        let (_control, client) = super::attach(&egui::Context::default());
        let ep = super::serve(client).unwrap();
        let s = std::net::TcpStream::connect(("127.0.0.1", ep.port)).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        writeln!(s.try_clone().unwrap(), "not json").unwrap();
        let mut r = BufReader::new(s);
        let mut first = String::new();
        r.read_line(&mut first).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&first).unwrap();
        assert_eq!(reply["error"]["code"], -32700, "{first}");
        // Closed: end of stream (or a reset), not a connection left waiting for more input.
        let mut next = String::new();
        let closed = match r.read_line(&mut next) {
            Ok(n) => n == 0,
            Err(e) => matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted),
        };
        assert!(closed, "the connection stayed open: {next:?}");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn lines_are_read_up_to_the_limit() {
        use super::{LineRead, read_line_limited};
        let mut buf = Vec::new();
        let mut input: &[u8] = b"aaaaaaaaaa\nbb\r\ncc";
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
        assert_eq!(buf, b"aaaaaaaaaa", "a line exactly at the limit is accepted");
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
        assert_eq!(buf, b"bb", "CRLF is stripped");
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
        assert_eq!(buf, b"cc", "a last line without a newline is still read");
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::End, "then end of stream");
        let long = [b'x'; 100];
        let mut input: &[u8] = &long;
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::TooLong);
        assert_eq!(input.len(), 100 - 11, "at most limit + 1 bytes are read; the rest is left");
    }

    /// Before authenticating, a line longer than [`super::MAX_AUTH_LINE`] is refused as soon as
    /// the limit is passed (no newline needed) and the connection is closed.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn an_overlong_line_before_auth_is_refused() {
        use std::io::{BufRead, BufReader, ErrorKind, Write};
        let (_control, client) = super::attach(&egui::Context::default());
        let ep = super::serve(client).unwrap();
        let s = std::net::TcpStream::connect(("127.0.0.1", ep.port)).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        // Exactly one byte over the limit, without a newline: the server reads it all, so the
        // close below is a clean end of stream.
        s.try_clone().unwrap().write_all(&vec![b'x'; super::MAX_AUTH_LINE + 1]).unwrap();
        let mut r = BufReader::new(s);
        let mut first = String::new();
        r.read_line(&mut first).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&first).unwrap();
        assert_eq!(reply["error"]["code"], -32600, "{first}");
        let mut next = String::new();
        let closed = match r.read_line(&mut next) {
            Ok(n) => n == 0,
            Err(e) => matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted),
        };
        assert!(closed, "the connection stayed open: {next:?}");
    }

    /// Small limits for the tests below: a 300 ms deadline, two places to wait in.
    #[cfg(not(target_arch = "wasm32"))]
    const TEST_LIMITS: super::Limits = super::Limits {
        auth_deadline: std::time::Duration::from_millis(300),
        max_unauthenticated: 2,
        write_timeout: std::time::Duration::from_secs(5),
        linger: std::time::Duration::from_secs(1),
        max_lingering: 2,
    };

    /// The next line the server sends, as JSON, or `None` when it closed the connection instead.
    #[cfg(not(target_arch = "wasm32"))]
    fn next_line(r: &mut std::io::BufReader<std::net::TcpStream>) -> Option<serde_json::Value> {
        use std::io::BufRead;
        let mut line = String::new();
        match r.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(serde_json::from_str(&line).unwrap()),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn connect(port: u16) -> std::net::TcpStream {
        let s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        s
    }

    /// #1806: a connection that never authenticates is closed at the deadline, counted from its
    /// acceptance, and a client feeding a byte at a time cannot push that deadline back.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn an_unauthenticated_connection_is_closed_at_the_deadline() {
        use std::io::{BufReader, Write};
        let (_control, client) = super::attach(&egui::Context::default());
        let ep = super::serve_with(client, TEST_LIMITS).unwrap();
        // Idle.
        let started = std::time::Instant::now();
        let mut r = BufReader::new(connect(ep.port));
        let reply = next_line(&mut r).expect("the server says why before closing");
        assert_eq!(reply["error"]["code"], -32004, "{reply}");
        let took = started.elapsed();
        assert!(took >= std::time::Duration::from_millis(250) && took < std::time::Duration::from_secs(3), "{took:?}");
        assert!(next_line(&mut r).is_none(), "then the connection is closed");
        // Dripping: a byte every 50 ms for as long as the server keeps the connection.
        let started = std::time::Instant::now();
        let s = connect(ep.port);
        let mut w = s.try_clone().unwrap();
        let drip = std::thread::spawn(move || {
            while w.write_all(b"x").is_ok() {
                std::thread::sleep(std::time::Duration::from_millis(50));
                if started.elapsed() > std::time::Duration::from_secs(3) {
                    break;
                }
            }
        });
        let mut r = BufReader::new(s);
        let reply = next_line(&mut r).expect("the server says why before closing");
        assert_eq!(reply["error"]["code"], -32004, "{reply}");
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(2), "the deadline is absolute: {took:?}");
        drop(r);
        drip.join().unwrap();
    }

    /// A client refused while the server still has its input unread gets the reason, not a
    /// reset: closing on top of unread input sends a reset, which can discard the reply before
    /// the client reads it (Linux does). Here a line far over the limit leaves most of itself
    /// unread when the server refuses it.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_client_refused_with_input_unread_gets_the_reason() {
        use std::io::{BufReader, Write};
        let (_control, client) = super::attach(&egui::Context::default());
        let ep = super::serve_with(client, TEST_LIMITS).unwrap();
        let s = connect(ep.port);
        let mut w = s.try_clone().unwrap();
        w.write_all(&vec![b'x'; 16 * super::MAX_AUTH_LINE]).unwrap();
        // Let the server refuse it (and, before the fix, close) before reading the reply.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut r = BufReader::new(s);
        let reply = next_line(&mut r).expect("the server says why before closing");
        assert_eq!(reply["error"]["code"], -32600, "{reply}");
    }

    /// A refused connection gives back its place among those waiting to authenticate before it
    /// lingers, so lingering does not keep real clients out.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_lingering_connection_holds_no_place_to_authenticate() {
        use std::io::{BufReader, Write};
        // A linger far longer than the checks below take, even on a loaded machine.
        let limits = super::Limits { linger: std::time::Duration::from_secs(5), ..TEST_LIMITS };
        let (_control, client) = super::attach(&egui::Context::default());
        let ep = super::serve_with(client, limits).unwrap();
        let auth = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "auth", "params": { "token": ep.token } });
        // Refused with most of its line unread, and left open: it lingers for `linger`.
        let refused = connect(ep.port);
        refused.try_clone().unwrap().write_all(&vec![b'x'; 16 * super::MAX_AUTH_LINE]).unwrap();
        let mut r = BufReader::new(refused.try_clone().unwrap());
        assert_eq!(next_line(&mut r).expect("refused with a reason")["error"]["code"], -32600);
        // Well within the linger, both places are free for new connections.
        let started = std::time::Instant::now();
        let others: Vec<_> = (0..limits.max_unauthenticated).map(|_| connect(ep.port)).collect();
        for s in &others {
            // A refused one has already been closed, so its write may fail; its reply says why.
            let _ = writeln!(s.try_clone().unwrap(), "{auth}");
            let reply = next_line(&mut BufReader::new(s.try_clone().unwrap())).expect("a reply");
            assert_eq!(reply["result"]["ok"], true, "admitted while the refused one lingers: {reply}");
        }
        assert!(started.elapsed() < limits.linger, "checked while it lingered: {:?}", started.elapsed());
        drop(refused);
    }

    /// Only [`super::Limits::max_lingering`] refused connections linger at once: one more still
    /// gets its reply, and is then closed at once instead of lingering.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn only_so_many_refused_connections_linger() {
        use std::io::{BufReader, Write};
        // A linger far longer than the checks below take, even on a loaded machine.
        let limits = super::Limits { linger: std::time::Duration::from_secs(5), max_lingering: 1, ..TEST_LIMITS };
        let (_control, client) = super::attach(&egui::Context::default());
        let ep = super::serve_with(client, limits).unwrap();
        let bad = r#"{"jsonrpc": "2.0", "id": 1, "method": "page_count"}"#;
        // The first refused connection takes the only lingering place and keeps it.
        let first = connect(ep.port);
        writeln!(first.try_clone().unwrap(), "{bad}").unwrap();
        let mut r1 = BufReader::new(first.try_clone().unwrap());
        assert_eq!(next_line(&mut r1).expect("refused with a reason")["error"]["code"], -32001);
        // The second still gets its reason (it has nothing unread, so no reset)...
        let second = connect(ep.port);
        let mut w2 = second.try_clone().unwrap();
        writeln!(w2, "{bad}").unwrap();
        let mut r2 = BufReader::new(second.try_clone().unwrap());
        assert_eq!(next_line(&mut r2).expect("refused with a reason")["error"]["code"], -32001);
        // ...and is closed outright, not lingering: writing to it soon fails, where a lingering
        // connection would read and discard for the whole of `linger`.
        let started = std::time::Instant::now();
        let closed = loop {
            if w2.write_all(b"x").is_err() {
                break true;
            }
            if started.elapsed() > limits.linger {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(closed && started.elapsed() < limits.linger / 2, "it lingered: {:?}", started.elapsed());
        drop(first);
    }

    /// #1806: only so many connections may wait to authenticate at once. One more is refused
    /// on the spot; once a waiting one authenticates or times out, its place is free again.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn only_so_many_connections_may_wait_to_authenticate() {
        use std::io::{BufReader, Write};
        let (_control, client) = super::attach(&egui::Context::default());
        let ep = super::serve_with(client, TEST_LIMITS).unwrap();
        let auth = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "auth", "params": { "token": ep.token } });
        let first = connect(ep.port);
        let second = connect(ep.port);
        // The kernel accepts a connection before the listener admits it: give the listener a
        // moment to admit both, well within their deadline.
        std::thread::sleep(std::time::Duration::from_millis(100));
        // The third finds both places taken.
        let mut r3 = BufReader::new(connect(ep.port));
        let reply = next_line(&mut r3).expect("refused with a reason");
        assert_eq!(reply["error"]["code"], -32003, "{reply}");
        assert!(next_line(&mut r3).is_none(), "and closed");
        // The second authenticates: its place is free, so a new connection is admitted.
        let mut r2 = BufReader::new(second.try_clone().unwrap());
        let mut w2 = second.try_clone().unwrap();
        writeln!(w2, "{auth}").unwrap();
        assert_eq!(next_line(&mut r2).unwrap()["result"]["ok"], true);
        let fourth = connect(ep.port);
        let mut r4 = BufReader::new(fourth.try_clone().unwrap());
        let mut w4 = fourth.try_clone().unwrap();
        writeln!(w4, "{auth}").unwrap();
        assert_eq!(next_line(&mut r4).unwrap()["result"]["ok"], true, "admitted and authenticated");
        // The first was closed at the deadline (its place freed on that way out too): after it,
        // another connection is admitted even though the first never authenticated.
        drop(first);
        std::thread::sleep(TEST_LIMITS.auth_deadline + std::time::Duration::from_millis(100));
        let fifth = connect(ep.port);
        let mut r5 = BufReader::new(fifth.try_clone().unwrap());
        let mut w5 = fifth.try_clone().unwrap();
        writeln!(w5, "{auth}").unwrap();
        assert_eq!(next_line(&mut r5).unwrap()["result"]["ok"], true);
    }

    /// After `auth` the limit no longer applies: a long line is read as before (here it isn't
    /// JSON, so it gets a parse error and the connection stays open).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn long_lines_after_auth_are_still_read() {
        use std::io::{BufRead, BufReader, Write};
        let (_control, client) = super::attach(&egui::Context::default());
        let ep = super::serve(client).unwrap();
        let s = std::net::TcpStream::connect(("127.0.0.1", ep.port)).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let reply = |r: &mut BufReader<std::net::TcpStream>| {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            serde_json::from_str::<serde_json::Value>(&line).unwrap()
        };
        let auth = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "auth", "params": { "token": ep.token } });
        writeln!(w, "{auth}").unwrap();
        assert_eq!(reply(&mut r)["result"]["ok"], true);
        writeln!(w, "{}", "x".repeat(4 * super::MAX_AUTH_LINE)).unwrap();
        assert_eq!(reply(&mut r)["error"]["code"], -32700, "a long line after auth is parsed, not refused");
        writeln!(w, "y").unwrap();
        assert_eq!(reply(&mut r)["error"]["code"], -32700, "and the connection is still open");
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use egui::Plugin as _;
    use egui_kittest::Harness;

    /// A request is answered after accepted frames only: a discarded layout pass is not a frame
    /// the agent can see, so it must not advance the count.
    #[test]
    fn discarded_passes_are_not_counted_as_accepted_frames() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut plugin = ControlPlugin { shared: shared.clone() };
        let ctx = egui::Context::default();
        let mut accepted = egui::FullOutput::default();
        plugin.output_hook(&ctx, &mut accepted);
        let mut discarded = egui::FullOutput::default();
        discarded.platform_output.request_discard_reasons.push(egui::RepaintCause::new_reason("control test discard"));
        plugin.output_hook(&ctx, &mut discarded);
        assert_eq!(shared.lock().unwrap().passes, 1, "only the presented pass counts");
    }

    /// A frame that requests discard on every pass and hits `max_passes` still presents its
    /// last pass: that presented pass must count, or `Pending::Frames` waits for a frame that
    /// never arrives. This runs the real multi-pass loop rather than a hand-built FullOutput.
    #[test]
    fn a_final_capped_pass_is_counted_as_presented() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let ctx = egui::Context::default();
        ctx.add_plugin(ControlPlugin { shared: shared.clone() });
        ctx.options_mut(|o| o.max_passes = 2.try_into().expect("two is non-zero"));
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| ui.ctx().request_discard("control test must discard"));
        output.textures_delta.clear();
        assert_eq!(output.platform_output.num_completed_passes, 2, "the cap was reached");
        assert!(!output.platform_output.request_discard_reasons.is_empty(), "the last pass still requested a discard");
        assert_eq!(shared.lock().unwrap().passes, 1, "the presented final pass counts once");
    }

    /// `max_passes` above two still counts a frame that settles after earlier discarded passes:
    /// egui clears the previous pass's discard reasons at the start of each pass, so the merged
    /// output of a presented final pass is not mistaken for a discarded one.
    #[test]
    fn a_frame_that_settles_before_a_higher_pass_cap_is_counted_once() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let ctx = egui::Context::default();
        ctx.add_plugin(ControlPlugin { shared: shared.clone() });
        ctx.options_mut(|o| o.max_passes = 3.try_into().expect("three is non-zero"));
        let mut pass = 0;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            pass += 1;
            if pass < 3 {
                ui.ctx().request_discard("control test must discard the first two passes");
            }
        });
        output.textures_delta.clear();
        assert_eq!(output.platform_output.num_completed_passes, 3, "the third pass settled at the higher cap");
        assert!(output.platform_output.request_discard_reasons.is_empty(), "earlier discard reasons do not survive the settling pass");
        assert_eq!(shared.lock().unwrap().passes, 1, "the one presented frame counts once");
    }

    /// `ui.key` delivers held modifiers through `ModifiersChanged`, because egui does not derive
    /// `InputState.modifiers` from a key event's own field. Shift+Tab and ⌘Enter handlers read it.
    #[test]
    fn key_commands_deliver_modifier_state() {
        let held = egui::Modifiers { command: true, ctrl: true, ..egui::Modifiers::NONE };
        let batches = key_batches(egui::Key::Enter, held);
        let key = |pressed| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: held };
        assert_eq!(
            batches,
            vec![vec![egui::Event::ModifiersChanged(held), key(true)], vec![key(false), egui::Event::ModifiersChanged(egui::Modifiers::NONE)]]
        );
        let ctx = egui::Context::default();
        let mut press_seen = None;
        ctx.run_ui(egui::RawInput { events: batches[0].clone(), ..Default::default() }, |ui| press_seen = Some(ui.input(|i| i.modifiers)))
            .textures_delta
            .clear();
        let mut release_seen = None;
        ctx.run_ui(egui::RawInput { events: batches[1].clone(), ..Default::default() }, |ui| release_seen = Some(ui.input(|i| i.modifiers)))
            .textures_delta
            .clear();
        assert_eq!(press_seen, Some(held), "the press sees the modifiers it was sent with");
        assert_eq!(release_seen, Some(egui::Modifiers::NONE), "the modifier release follows the key-up");
    }

    /// A batch admitted for one document/focus generation must not apply after that generation
    /// changes; the request is reported as stale instead of replaying the input.
    #[test]
    fn a_stale_synthetic_batch_is_dropped_and_reported() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut plugin = ControlPlugin { shared: shared.clone() };
        {
            let mut s = shared.lock().unwrap();
            s.epoch = 2;
            s.inject.push_back(RequestBatches {
                request: 41,
                epoch: 1,
                started: false,
                since: web_time::Instant::now(),
                release: Vec::new(),
                batches: VecDeque::from([vec![egui::Event::PointerMoved(egui::pos2(1.0, 1.0))]]),
            });
        }
        let mut raw = egui::RawInput::default();
        plugin.input_hook(&egui::Context::default(), &mut raw);
        assert!(raw.events.is_empty(), "stale input must not reach the new document");
        let s = shared.lock().unwrap();
        assert_eq!(s.dropped, vec![41]);
        assert!(s.inject.is_empty());
    }

    /// A stale request is dropped alone. A request admitted later for the current generation
    /// behind it still runs: one agent's outdated input must not fail another's.
    #[test]
    fn a_stale_request_does_not_drop_a_later_current_one() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut plugin = ControlPlugin { shared: shared.clone() };
        {
            let mut s = shared.lock().unwrap();
            s.epoch = 2;
            s.inject.push_back(RequestBatches {
                request: 41,
                epoch: 1,
                started: false,
                since: web_time::Instant::now(),
                release: Vec::new(),
                batches: VecDeque::from([vec![egui::Event::PointerMoved(egui::pos2(1.0, 1.0))]]),
            });
            s.inject.push_back(RequestBatches {
                request: 42,
                epoch: 2,
                started: false,
                since: web_time::Instant::now(),
                release: Vec::new(),
                batches: VecDeque::from([vec![egui::Event::PointerMoved(egui::pos2(2.0, 2.0))]]),
            });
        }
        let ctx = egui::Context::default();
        let mut first = egui::RawInput::default();
        plugin.input_hook(&ctx, &mut first);
        assert!(first.events.is_empty(), "the stale request is not replayed");
        let mut second = egui::RawInput::default();
        plugin.input_hook(&ctx, &mut second);
        assert_eq!(second.events, [egui::Event::PointerMoved(egui::pos2(2.0, 2.0))], "the current request still applies");
        let s = shared.lock().unwrap();
        assert_eq!(s.dropped, vec![41]);
        assert!(s.inject.is_empty());
    }

    /// Once a request has started, the focus/dialog change caused by its own first batch must not
    /// drop its release: a key press without its release leaves `keys_down` stuck.
    #[test]
    fn a_started_request_finishes_its_batches_after_focus_moves() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut plugin = ControlPlugin { shared: shared.clone() };
        {
            let mut s = shared.lock().unwrap();
            s.epoch = 7;
            s.inject.push_back(RequestBatches {
                request: 9,
                epoch: 7,
                started: false,
                since: web_time::Instant::now(),
                release: Vec::new(),
                batches: VecDeque::from([
                    vec![egui::Event::Key {
                        key: egui::Key::Tab,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    vec![egui::Event::Key {
                        key: egui::Key::Tab,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                ]),
            });
        }
        let ctx = egui::Context::default();
        let mut press = egui::RawInput::default();
        plugin.input_hook(&ctx, &mut press);
        assert!(matches!(press.events.as_slice(), [egui::Event::Key { pressed: true, .. }]));
        // The press moved focus: the next pass sees another input-target generation.
        shared.lock().unwrap().epoch = 8;
        let mut release = egui::RawInput::default();
        plugin.input_hook(&ctx, &mut release);
        assert!(matches!(release.events.as_slice(), [egui::Event::Key { pressed: false, .. }]), "the release still arrives");
        let s = shared.lock().unwrap();
        assert!(s.dropped.is_empty(), "a started request is not reported as stale");
        assert!(s.inject.is_empty());
    }

    /// A host for timeout tests: the control layer must not need a real app.
    struct NoopHost;

    impl Host for NoopHost {
        fn state(&self) -> Value {
            json!({})
        }
        fn command(&mut self, _id: &str) -> Reply {
            Err("no commands in this test".into())
        }
        fn panel(&mut self, _reset: bool, _params: &Value) -> Reply {
            Err("no side panels in this test".into())
        }
        fn commands(&self) -> Value {
            json!([])
        }
        fn set(&mut self, _key: &str, _value: &str) -> Reply {
            Err("no settings in this test".into())
        }
        fn open(&mut self, _path: &str) -> Reply {
            Err("no files in this test".into())
        }
        fn quit(&mut self, _ctx: &egui::Context) -> bool {
            false
        }
    }

    /// A host that starts unready (fonts not installed) and counts dispatched state reads.
    /// `state` takes `&self`, so the count sits in a `Cell`.
    struct ReadinessHost {
        ready: bool,
        states: std::cell::Cell<usize>,
        /// Methods the host was asked to run, in dispatch order.
        calls: std::cell::RefCell<Vec<&'static str>>,
    }

    impl ReadinessHost {
        fn new(ready: bool) -> Self {
            Self { ready, states: std::cell::Cell::new(0), calls: std::cell::RefCell::new(Vec::new()) }
        }
    }

    impl Host for ReadinessHost {
        fn state(&self) -> Value {
            self.states.set(self.states.get() + 1);
            self.calls.borrow_mut().push("ui.state");
            json!({ "ready": self.ready })
        }
        fn command(&mut self, _id: &str) -> Reply {
            Err("no commands in this test".into())
        }
        fn panel(&mut self, _reset: bool, _params: &Value) -> Reply {
            Err("no side panels in this test".into())
        }
        fn commands(&self) -> Value {
            self.calls.borrow_mut().push("ui.commands");
            json!([])
        }
        fn set(&mut self, _key: &str, _value: &str) -> Reply {
            Err("no settings in this test".into())
        }
        fn open(&mut self, _path: &str) -> Reply {
            Err("no files in this test".into())
        }
        fn quit(&mut self, _ctx: &egui::Context) -> bool {
            false
        }
        fn accepts_input(&self) -> bool {
            self.ready
        }
    }

    /// The admission timestamp of the one pending request, whichever kind it is.
    fn pending_since(control: &Control) -> Option<web_time::Instant> {
        let first = control.pending.first()?;
        match first {
            Pending::Frames { since, .. } | Pending::Screenshot { since, .. } => Some(*since),
        }
    }

    #[test]
    fn a_request_expired_in_the_channel_is_never_dispatched_or_deferred() {
        for ready in [false, true] {
            let ctx = egui::Context::default();
            let (mut control, client) = attach(&ctx);
            let mut host = ReadinessHost::new(ready);
            let (reply, waiting) = channel();
            let since = web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap();
            client.tx.send(ControlRequest { since, method: "ui.state".into(), params: json!({}), reply }).unwrap();
            control.tick(&ctx, &mut host);
            let error = waiting.try_recv().expect("expired channel request replies on the first tick").expect_err("expired request cannot dispatch");
            assert!(error.contains(if ready { "finished starting" } else { "still starting" }), "{error}");
            assert_eq!(host.states.get(), 0);
            assert!(control.deferred.is_empty() && control.pending.is_empty());
        }
    }

    /// A request held during startup keeps its arrival time. Once the app accepts input, one
    /// older than the control deadline fails instead of being dispatched into a later UI.
    #[test]
    fn a_deferred_request_expires_instead_of_dispatching_after_the_deadline() {
        let ctx = egui::Context::default();
        let (mut control, client) = attach(&ctx);
        let mut host = ReadinessHost::new(false);
        let waiting = client.send("ui.state", json!({}));
        control.tick(&ctx, &mut host);
        assert_eq!(control.deferred.len(), 1, "unready host defers the request");
        assert!(waiting.try_recv().is_err(), "it is not answered while starting");
        let past = web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap();
        for deferred in &mut control.deferred {
            deferred.since = past;
        }
        // It expires even while the app is still not ready, so the client is not left waiting.
        control.tick(&ctx, &mut host);
        let error = waiting.try_recv().expect("expired deferred request receives a reply").expect_err("it must not dispatch");
        assert!(error.contains("still starting"), "{error}");
        assert!(control.deferred.is_empty());
        // And readiness arriving later cannot revive it.
        host.ready = true;
        control.tick(&ctx, &mut host);
        assert_eq!(host.states.get(), 0, "an expired request is never dispatched");
    }

    /// A request deferred only briefly still runs once fonts are ready.
    #[test]
    fn a_fresh_deferred_request_still_dispatches_when_the_app_becomes_ready() {
        let ctx = egui::Context::default();
        let (mut control, client) = attach(&ctx);
        let mut host = ReadinessHost::new(false);
        let waiting = client.send("ui.state", json!({}));
        control.tick(&ctx, &mut host);
        assert!(waiting.try_recv().is_err());
        host.ready = true;
        control.tick(&ctx, &mut host);
        let reply = waiting.try_recv().expect("fresh deferred request replies").expect("it dispatches");
        assert_eq!(reply["ready"], true);
        assert_eq!(host.states.get(), 1, "the fresh deferred request is dispatched once");
    }

    /// A request deferred during startup keeps its one arrival timestamp. Dispatching it into
    /// synthetic input, a deferred frame reply or a screenshot reservation must not start a fresh
    /// 5 s in that phase: every later phase measures the same deadline from arrival.
    #[test]
    fn a_startup_wait_counts_against_one_admission_deadline_in_every_phase() {
        let params = [("ui.type", json!({ "text": "x" })), ("ui.resize", json!({ "width": 800, "height": 600 })), ("ui.screenshot", json!({}))];
        for (method, params) in params {
            // `ui.resize` needs a native window; the other phases are checked on every target.
            if method == "ui.resize" && cfg!(target_arch = "wasm32") {
                continue;
            }
            let ctx = egui::Context::default();
            // A real window size, so an admitted screenshot has capturable geometry.
            ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0))), ..Default::default() },
                |_| {},
            )
            .drop_without_applying_deltas();
            let (mut control, client) = attach(&ctx);
            let mut host = ReadinessHost::new(false);
            let waiting = client.send(method, params);
            control.tick(&ctx, &mut host);
            assert_eq!(control.deferred.len(), 1, "{method}: deferred while starting");
            // Three of the five seconds were spent starting: the request is still inside its
            // deadline, but only just.
            let arrived = web_time::Instant::now().checked_sub(std::time::Duration::from_secs(3)).unwrap();
            for deferred in &mut control.deferred {
                deferred.since = arrived;
            }
            host.ready = true;
            control.tick(&ctx, &mut host);
            assert!(control.deferred.is_empty(), "{method}: dispatched once the app accepts input");
            assert!(waiting.try_recv().is_err(), "{method}: a preserved deadline does not answer early");
            assert_eq!(pending_since(&control), Some(arrived), "{method}: the pending reply keeps the arrival deadline");
            if method == "ui.type" {
                let shared = control.shared.lock().unwrap();
                assert_eq!(shared.inject.front().map(|request| request.since), Some(arrived), "{method}: queued input keeps the arrival deadline");
            }
            // Move that same shared timestamp past its own deadline (five seconds after arrival).
            // Expiry measures the one admission instant, so nothing waits for a fresh five.
            let past = arrived.checked_sub(timeout()).expect("a representable instant");
            for pending in &mut control.pending {
                match pending {
                    Pending::Frames { since, .. } | Pending::Screenshot { since, .. } => *since = past,
                }
            }
            if let Ok(mut shared) = control.shared.lock() {
                for request in &mut shared.inject {
                    request.since = past;
                }
            }
            control.finish_pending(&ctx, &mut host);
            let error = waiting.try_recv().expect("the expired request replies").expect_err("it must not succeed");
            assert!(error.contains("not being drawn"), "{method}: {error}");
            assert!(control.pending.is_empty(), "{method}: the expired request is not retried");
            assert_eq!(control.reserved_bytes(), 0, "{method}: any reservation is released");
        }
    }

    /// One client's requests keep their arrival order: a request deferred during startup runs
    /// before a newer request that was still waiting in the channel when the app became ready.
    #[test]
    fn a_deferred_request_runs_before_a_newer_queued_one() {
        let ctx = egui::Context::default();
        let (mut control, client) = attach(&ctx);
        let mut host = ReadinessHost::new(false);
        let older = client.send("ui.state", json!({}));
        control.tick(&ctx, &mut host);
        assert_eq!(control.deferred.len(), 1, "the older request is deferred while starting");
        let newer = client.send("ui.commands", json!({}));
        host.ready = true;
        control.tick(&ctx, &mut host);
        assert_eq!(*host.calls.borrow(), ["ui.state", "ui.commands"], "the older deferred request is dispatched first");
        assert_eq!(older.try_recv().expect("the older request replies").expect("it dispatches")["ready"], true);
        assert_eq!(newer.try_recv().expect("the newer request replies").expect("it dispatches"), json!([]));
    }

    /// A request whose startup wait used up the deadline is refused once the app is ready, and the
    /// message says the app finished starting instead of claiming it is still starting.
    #[test]
    fn a_request_that_outlasted_startup_reports_that_the_app_finished_starting() {
        let ctx = egui::Context::default();
        let (mut control, client) = attach(&ctx);
        let mut host = ReadinessHost::new(false);
        let waiting = client.send("ui.state", json!({}));
        control.tick(&ctx, &mut host);
        assert_eq!(control.deferred.len(), 1);
        let past = web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap();
        for deferred in &mut control.deferred {
            deferred.since = past;
        }
        host.ready = true;
        control.tick(&ctx, &mut host);
        let error = waiting.try_recv().expect("the expired request replies").expect_err("it is never dispatched");
        assert!(error.contains("finished starting"), "{error}");
        assert!(!error.contains("was still starting"), "the app is ready now: {error}");
        assert_eq!(host.states.get(), 0, "an expired request is never dispatched");
        assert!(control.deferred.is_empty() && control.pending.is_empty());
    }

    /// Keep key/modifier cleanup, but a canceled pointer must never produce a release event.
    #[test]
    fn a_timed_out_started_request_cancels_pointer_and_releases_keys() {
        let ctx = egui::Context::default();
        let (mut control, _client) = attach(&ctx);
        let held = egui::Modifiers { command: true, ctrl: true, ..egui::Modifiers::NONE };
        let pos = egui::pos2(24.0, 32.0);
        let key = |pressed| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: held };
        let ids = control
            .inject(
                vec![
                    vec![
                        egui::Event::ModifiersChanged(held),
                        key(true),
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: held },
                    ],
                    vec![egui::Event::PointerMoved(egui::pos2(40.0, 48.0))],
                    vec![key(false), egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: held }],
                ],
                web_time::Instant::now(),
            )
            .unwrap();
        let (reply, rx) = channel();
        control.pending.push(Pending::Frames { until_pass: u64::MAX, batches: ids, since: web_time::Instant::now(), reply, value: json!({}) });
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        assert!(ctx.input(|i| i.keys_down.contains(&egui::Key::Enter) && i.pointer.primary_down()));
        assert_eq!(ctx.input(|i| i.modifiers), held);
        age_input(&mut control);
        let next = egui::pos2(80.0, 90.0);
        control.inject(vec![vec![egui::Event::PointerMoved(next)]], web_time::Instant::now()).unwrap();
        control.finish_pending(&ctx, &mut NoopHost);
        assert!(rx.try_recv().unwrap().unwrap_err().contains("not being drawn"));
        assert!(control.pending.is_empty());
        ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(!ui.input(|i| i.keys_down.contains(&egui::Key::Enter)));
            assert_eq!(ui.input(|i| i.modifiers), egui::Modifiers::NONE);
            assert_eq!(ui.input(|i| i.pointer.hover_pos()), Some(next), "another live request survives");
            assert!(!ui.input(|i| i.pointer.any_down() || i.pointer.any_released()));
            assert!(ui.ctx().dragged_id().is_none() && ui.ctx().drag_stopped_id().is_none());
        })
        .drop_without_applying_deltas();
        assert!(control.shared.lock().unwrap().inject.is_empty());
        assert!(rx.try_recv().is_err(), "only one timeout reply");
    }

    #[test]
    fn a_deadline_crossing_inside_an_admitted_pass_cancels_before_the_next_interaction() {
        let ctx = egui::Context::default();
        let (mut control, _client) = attach(&ctx);
        let pos = egui::pos2(24.0, 32.0);
        let ids = control
            .inject(
                vec![
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
                    ],
                    vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }],
                ],
                web_time::Instant::now(),
            )
            .unwrap();
        let (reply, rx) = channel();
        control.pending.push(Pending::Frames { until_pass: u64::MAX, batches: ids, since: web_time::Instant::now(), reply, value: json!({}) });
        ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(ui.input(|i| i.pointer.primary_down()), "the press was admitted before expiry");
            age_input(&mut control);
            control.finish_pending(ui.ctx(), &mut NoopHost);
            assert!(rx.try_recv().is_err(), "do not cancel an already-calculated interaction mid-pass");
        })
        .drop_without_applying_deltas();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            control.finish_pending(ui.ctx(), &mut NoopHost);
            assert!(!ui.input(|i| i.pointer.any_down() || i.pointer.any_released()));
            assert!(ui.ctx().drag_stopped_id().is_none());
        })
        .drop_without_applying_deltas();
        assert!(rx.try_recv().unwrap().unwrap_err().contains("not being drawn"));
        assert!(control.shared.lock().unwrap().inject.is_empty());
    }

    /// Deterministic elapsed-time input: age the same admission in both halves, without sleeping
    /// or fabricating any egui pointer/widget state.
    fn age_input(control: &mut Control) {
        let past = web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap();
        for request in &mut control.shared.lock().unwrap().inject {
            request.since = past;
        }
        for pending in &mut control.pending {
            if let Pending::Frames { since, .. } = pending {
                *since = past;
            }
        }
    }

    /// The public gestures this test drives through real UI passes.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Gesture {
        AtomicClick,
        HeldClick,
        Marquee,
    }

    fn gesture_request(gesture: Gesture, from: egui::Pos2, to: egui::Pos2) -> (&'static str, Value) {
        match gesture {
            Gesture::AtomicClick => ("ui.click", json!({"x": from.x, "y": from.y})),
            Gesture::HeldClick => ("ui.drag", json!({"from": [from.x, from.y], "to": [from.x, from.y], "steps": 6})),
            Gesture::Marquee => ("ui.drag", json!({"from": [from.x, from.y], "to": [to.x, to.y], "steps": 6})),
        }
    }

    fn marquee_harness() -> (Harness<'static, crate::PdfCraftApp>, ControlClient) {
        let client_slot: Arc<Mutex<Option<ControlClient>>> = Arc::new(Mutex::new(None));
        let slot = client_slot.clone();
        let h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).with_step_dt(0.01).build_eframe(move |cc| {
            let mut app = crate::PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("synthetic-form.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
            app.set_option("quick", "marquee-zoom").unwrap();
            *slot.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
            app
        });
        let client = client_slot.lock().unwrap().take().unwrap();
        (h, client)
    }

    fn marquee_step(h: &mut Harness<'static, crate::PdfCraftApp>) {
        let ctx = h.ctx.clone();
        let mut raw = std::mem::take(h.input_mut());
        h.state_mut().prepare_raw_input(&ctx, &mut raw);
        *h.input_mut() = raw;
        h.step();
    }

    /// The actual laid-out page rect and a short point gesture inside it.
    fn gesture_target(h: &Harness<'static, crate::PdfCraftApp>) -> (egui::Pos2, egui::Pos2) {
        let page = h.state().views[0].page_screen_rect(0).expect("the actual PDF page was laid out").intersect(h.state().views[0].viewport_rect());
        assert!(page.width() > 100.0 && page.height() > 100.0, "a visible hit target: {page:?}");
        let from = page.center();
        let to = from + egui::vec2(40.0, 50.0);
        assert!(page.contains(to));
        (from, to)
    }

    /// Step until the public request has really reached egui and can still be interrupted.
    fn await_gesture_admission(h: &mut Harness<'static, crate::PdfCraftApp>, gesture: Gesture) -> bool {
        let mut admitted = false;
        for _ in 0..8 {
            marquee_step(h);
            admitted = if gesture == Gesture::AtomicClick {
                let shared = h.state().control.as_ref().unwrap().shared.lock().unwrap();
                shared.inject.front().is_some_and(|request| request.started && request.batches.len() == 1)
            } else {
                h.ctx.input(|i| i.pointer.primary_down()) && (gesture != Gesture::Marquee || h.state().views[0].marquee.is_some())
            };
            if admitted {
                break;
            }
        }
        admitted
    }

    /// The request is queued exactly once, with the expected terminal batch not yet applied.
    fn assert_interruptible_admission(h: &Harness<'static, crate::PdfCraftApp>, gesture: Gesture, from: egui::Pos2) {
        {
            let shared = h.state().control.as_ref().unwrap().shared.lock().unwrap();
            assert_eq!(shared.inject.len(), 1, "only the actual public request is queued");
            let pending = shared.inject.front().unwrap();
            assert!(pending.started);
            let terminal = pending.batches.back().expect("the terminal batch is still pending");
            if gesture == Gesture::AtomicClick {
                assert_eq!(pending.batches.len(), 1);
                assert_eq!(terminal.len(), 2);
                assert!(
                    matches!(terminal[0], egui::Event::PointerButton { pressed: true, button: egui::PointerButton::Primary, pos, .. } if pos == from)
                );
                assert!(
                    matches!(terminal[1], egui::Event::PointerButton { pressed: false, button: egui::PointerButton::Primary, pos, .. } if pos == from)
                );
                assert!(!h.ctx.input(|i| i.pointer.primary_down()), "atomic click has only hovered so far");
            } else {
                assert!(h.ctx.input(|i| i.pointer.primary_down()), "a real pass consumed the press");
                assert_eq!(terminal.len(), 1);
                assert!(matches!(terminal[0], egui::Event::PointerButton { pressed: false, button: egui::PointerButton::Primary, .. }));
            }
        }
        assert!(!h.ctx.input(|i| i.pointer.any_released()), "the terminal release has not run");
        assert!(h.ctx.drag_stopped_id().is_none());
        if gesture == Gesture::Marquee {
            assert!(h.ctx.dragged_id().is_some() && h.state().views[0].marquee.is_some(), "the app has a real drag preview");
        } else {
            assert!(h.state().views[0].marquee.is_none() && h.state().views[0].marquee_done.is_none());
        }
    }

    /// Expire the admission both through hidden logic and through the next input hook, then
    /// assert that cancellation left no gesture, zoom or edit behind.
    fn assert_timed_out_gesture(h: &mut Harness<'static, crate::PdfCraftApp>, reply: &Receiver<Reply>, hidden_logic: bool, zoom: f32, bytes: &[u8]) {
        age_input(h.state_mut().control.as_mut().unwrap());
        if hidden_logic {
            let ctx = h.ctx.clone();
            let app = h.state_mut();
            let mut control = app.control.take().unwrap();
            control.tick(&ctx, app);
            app.control = Some(control);
            assert!(reply.try_recv().unwrap().unwrap_err().contains("not being drawn"));
        }
        marquee_step(h);
        if !hidden_logic {
            assert!(reply.try_recv().unwrap().unwrap_err().contains("not being drawn"));
        }
        for _ in 0..2 {
            marquee_step(h);
        }
        assert!(!h.ctx.input(|i| i.pointer.any_down() || i.pointer.any_released()));
        assert!(h.ctx.dragged_id().is_none() && h.ctx.drag_stopped_id().is_none());
        assert!(h.state().views[0].marquee.is_none() && h.state().views[0].marquee_done.is_none());
        assert_eq!(h.state().views[0].zoom, zoom, "canceling must not turn a drag into a zoom click");
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes.as_slice(), bytes, "no abandoned edit");
        assert!(h.state().control.as_ref().unwrap().shared.lock().unwrap().inject.is_empty());
        assert!(h.state().control.as_ref().unwrap().pending.is_empty());
        assert!(reply.try_recv().is_err(), "the cancellation replies once");
    }

    /// Complete one public gesture and check its real event trace and one-step zoom.
    fn complete_positive_gesture(
        h: &mut Harness<'static, crate::PdfCraftApp>,
        client: &ControlClient,
        gesture: Gesture,
        positive: Gesture,
        bytes: &[u8],
    ) {
        let (from, to) = gesture_target(h);
        let zoom = h.state().views[0].zoom;
        let (method, params) = gesture_request(positive, from, to);
        let reply = client.send(method, params);
        let mut value = None;
        let mut buttons = Vec::new();
        let mut positions = Vec::new();
        for _ in 0..20 {
            marquee_step(h);
            h.ctx.input(|i| {
                for event in &i.events {
                    match event {
                        egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed, pos, .. } => {
                            buttons.push((*pressed, i.time));
                            positions.push(*pos);
                        }
                        egui::Event::PointerMoved(pos) => positions.push(*pos),
                        _ => {}
                    }
                }
            });
            if let Ok(result) = reply.try_recv() {
                value = Some(result.unwrap());
                break;
            }
        }
        assert!(value.is_some(), "a fresh completed request succeeds: {positive:?}, after {gesture:?}");
        assert_eq!(buttons.iter().map(|(pressed, _)| *pressed).collect::<Vec<_>>(), [true, false]);
        assert!(!h.ctx.input(|i| i.pointer.any_down()));
        assert!(h.ctx.dragged_id().is_none());
        if positive == Gesture::Marquee {
            assert!(h.state().views[0].zoom > zoom, "the normal marquee rectangle zooms");
        } else {
            let options = h.ctx.options(|o| o.input_options);
            assert!(buttons[1].1 - buttons[0].1 < options.max_click_duration, "real press/release remain a short click");
            assert!(positions.iter().all(|pos| pos.distance(from) < options.max_click_dist), "real input remains stationary");
            assert!((h.state().views[0].zoom - zoom * 1.5).abs() < 0.001, "a normal click zooms one step");
        }
        assert!(h.state().views[0].marquee.is_none() && h.state().views[0].marquee_done.is_none());
        assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes.as_slice(), bytes, "completed zooms do not edit the document");
        assert!(reply.try_recv().is_err(), "successful request replies once");
    }

    /// Real page hit-testing, real control batches and the production marquee completion path.
    /// Exercise both hidden-logic expiry and expiry first observed by the next input hook.
    #[test]
    fn a_timed_out_marquee_never_zooms_and_completed_clicks_and_drags_still_work() {
        for hidden_logic in [false, true] {
            for gesture in [Gesture::AtomicClick, Gesture::HeldClick, Gesture::Marquee] {
                let (mut h, client) = marquee_harness();
                for _ in 0..5 {
                    marquee_step(&mut h);
                }
                let (from, to) = gesture_target(&h);
                let zoom = h.state().views[0].zoom;
                let bytes = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
                let (method, params) = gesture_request(gesture, from, to);
                let reply = client.send(method, params);
                assert!(await_gesture_admission(&mut h, gesture), "a real interruptible admission: {gesture:?}, hidden={hidden_logic}");
                assert_interruptible_admission(&h, gesture, from);
                assert!(reply.try_recv().is_err(), "the terminal batch has not run");
                assert_eq!(h.state().views[0].zoom, zoom);
                assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes, bytes);
                assert_timed_out_gesture(&mut h, &reply, hidden_logic, zoom, &bytes);

                // Complete the same public gesture, then an atomic click after held input.
                // Real event traces prove the stationary drag is a valid short point click.
                let positives = [gesture, Gesture::AtomicClick];
                let count = if gesture == Gesture::AtomicClick { 1 } else { 2 };
                for positive in positives.into_iter().take(count) {
                    complete_positive_gesture(&mut h, &client, gesture, positive, &bytes);
                }
            }
        }
    }

    /// A screenshot that times out cancels its slot and gives its reservation back, with a
    /// defined error instead of a silent success.
    #[test]
    fn a_timed_out_screenshot_releases_its_slot_and_budget() {
        struct FakeHost;
        impl Host for FakeHost {
            fn state(&self) -> Value {
                json!({})
            }
            fn command(&mut self, _id: &str) -> Reply {
                Err("no commands in this test".into())
            }
            fn panel(&mut self, _reset: bool, _params: &Value) -> Reply {
                Err("no side panels in this test".into())
            }
            fn commands(&self) -> Value {
                json!([])
            }
            fn set(&mut self, _key: &str, _value: &str) -> Reply {
                Err("no settings in this test".into())
            }
            fn open(&mut self, _path: &str) -> Reply {
                Err("no files in this test".into())
            }
            fn quit(&mut self, _ctx: &egui::Context) -> bool {
                false
            }
        }
        let ctx = egui::Context::default();
        let (mut control, _client) = attach(&ctx);
        let (slot, callback) = craft_ui::capture::CaptureSlot::new(&ctx, egui::ViewportId::ROOT, MAX_IMAGE_BYTES);
        let (reply, rx) = channel();
        control.pending.push(Pending::Screenshot {
            slot,
            callback: Some(callback),
            region: None,
            reply,
            since: web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap_or_else(web_time::Instant::now),
            geometry: Geometry { size: egui::vec2(800.0, 600.0), pixels_per_point: 1.0, epoch: 0 },
            reserved: 800 * 600 * 4,
            attempts: 0,
        });
        control.finish_pending(&ctx, &mut FakeHost);
        let error = rx.try_recv().expect("the request is answered").expect_err("a timeout is an error");
        assert!(error.contains("not being drawn"), "{error}");
        assert_eq!(control.reserved_bytes(), 0, "the reservation is released");
        assert!(control.pending.is_empty(), "the cancelled capture is not retried");
    }

    /// A real successful callback must not bypass expiry, including when resize would otherwise
    /// retry it. A non-expired completion still produces a real encoded PNG reply.
    #[test]
    fn completed_screenshots_obey_the_admission_deadline_before_success_or_retry() {
        for (expired, resized) in [(false, false), (true, false), (true, true)] {
            let ctx = egui::Context::default();
            ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64.0, 32.0))), ..Default::default() },
                |_| {},
            )
            .drop_without_applying_deltas();
            let (mut control, _client) = attach(&ctx);
            let geometry = Geometry::current(&ctx, 0);
            let (slot, callback) = craft_ui::capture::CaptureSlot::new(&ctx, egui::ViewportId::ROOT, MAX_IMAGE_BYTES);
            let image = Arc::new(egui::ColorImage::filled([64, 32], egui::Color32::RED));
            let weak_image = Arc::downgrade(&image);
            callback.complete(image);
            assert!(weak_image.upgrade().is_some(), "the completed slot really retains pixels");
            let (reply, rx) = channel();
            control.pending.push(Pending::Screenshot {
                slot,
                callback: None,
                region: None,
                reply,
                since: if expired {
                    web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap()
                } else {
                    web_time::Instant::now()
                },
                geometry,
                reserved: geometry.image_bytes().unwrap(),
                attempts: 0,
            });
            if resized {
                ctx.run_ui(
                    egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(80.0, 40.0))), ..Default::default() },
                    |_| {},
                )
                .drop_without_applying_deltas();
                assert_ne!(Geometry::current(&ctx, 0).size, geometry.size);
            }
            control.finish_pending(&ctx, &mut NoopHost);
            let result = rx.try_recv().expect("one settled reply");
            if expired {
                assert!(result.unwrap_err().contains("not being drawn"), "expiry takes priority over success or resize");
            } else {
                let image = result.expect("the live completed capture still succeeds");
                assert_eq!(image["width"], 64);
                assert_eq!(image["height"], 32);
                use base64::Engine as _;
                let png = base64::engine::general_purpose::STANDARD.decode(image["png_base64"].as_str().expect("actual PNG reply")).unwrap();
                assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
            }
            assert!(control.pending.is_empty(), "an expired capture is never retried");
            assert_eq!(control.reserved_bytes(), 0);
            assert!(weak_image.upgrade().is_none(), "the slot releases its pixels");
            assert!(rx.try_recv().is_err(), "exactly one reply");
        }
    }

    #[test]
    fn expired_unissued_screenshots_are_not_issued_and_late_callbacks_are_harmless() {
        let ctx = egui::Context::default();
        let (mut control, _client) = attach(&ctx);
        let (slot, callback) = craft_ui::capture::CaptureSlot::new(&ctx, egui::ViewportId::ROOT, MAX_IMAGE_BYTES);
        let late_callback = callback.clone();
        let geometry = Geometry { size: egui::vec2(64.0, 32.0), pixels_per_point: 1.0, epoch: 0 };
        let (reply, rx) = channel();
        control.pending.push(Pending::Screenshot {
            slot,
            callback: Some(callback),
            region: None,
            reply,
            since: web_time::Instant::now().checked_sub(std::time::Duration::from_secs(10)).unwrap(),
            geometry,
            reserved: 64 * 32 * 4,
            attempts: 0,
        });
        let output = ctx.run_ui(egui::RawInput::default(), |ui| control.finish_frame(ui.ctx(), &mut NoopHost));
        assert!(
            output
                .viewport_output
                .values()
                .all(|viewport| !viewport.commands.iter().any(|command| matches!(command, egui::ViewportCommand::Screenshot(_))))
        );
        output.drop_without_applying_deltas();
        assert!(rx.try_recv().unwrap().unwrap_err().contains("not being drawn"));
        assert!(control.pending.is_empty());
        assert_eq!(control.reserved_bytes(), 0);
        let image = Arc::new(egui::ColorImage::filled([64, 32], egui::Color32::RED));
        let weak_image = Arc::downgrade(&image);
        late_callback.complete(image);
        assert!(weak_image.upgrade().is_none(), "a late completion cannot resurrect a canceled request");
        control.finish_pending(&ctx, &mut NoopHost);
        assert!(rx.try_recv().is_err(), "no second reply");
    }

    /// One queued batch is consumed by exactly one UI pass.
    #[test]
    fn one_batch_is_consumed_by_one_pass() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut plugin = ControlPlugin { shared: shared.clone() };
        {
            let mut s = shared.lock().unwrap();
            s.epoch = 3;
            s.inject.push_back(RequestBatches {
                request: 1,
                epoch: 3,
                started: false,
                since: web_time::Instant::now(),
                release: Vec::new(),
                batches: VecDeque::from([vec![egui::Event::PointerMoved(egui::pos2(2.0, 3.0))]]),
            });
            s.inject.push_back(RequestBatches {
                request: 2,
                epoch: 3,
                started: false,
                since: web_time::Instant::now(),
                release: Vec::new(),
                batches: VecDeque::from([vec![egui::Event::PointerMoved(egui::pos2(4.0, 5.0))]]),
            });
        }
        let mut first = egui::RawInput::default();
        plugin.input_hook(&egui::Context::default(), &mut first);
        assert_eq!(first.events.len(), 1);
        let mut second = egui::RawInput::default();
        plugin.input_hook(&egui::Context::default(), &mut second);
        assert_eq!(second.events.len(), 1);
        assert_eq!(second.events[0], egui::Event::PointerMoved(egui::pos2(4.0, 5.0)));
        let mut third = egui::RawInput::default();
        plugin.input_hook(&egui::Context::default(), &mut third);
        assert!(third.events.is_empty(), "a consumed batch is never repeated");
    }
}

#[cfg(test)]
mod screenshot_geometry_tests {
    use super::*;

    #[test]
    fn image_bytes_are_checked_against_overflow_and_limits() {
        assert_eq!(checked_image_bytes(2, 3), Some(24));
        assert_eq!(checked_image_bytes(usize::MAX, 2), None);
        let big = Geometry { size: egui::vec2(10_000.0, 10_000.0), pixels_per_point: 8.0, epoch: 0 };
        assert!(big.image_bytes().is_some_and(|bytes| bytes > MAX_IMAGE_BYTES));
    }

    /// The crop is derived from the returned pixels and the admitted logical rect, not from a
    /// current `pixels_per_point`: a fitted surface may return a smaller image.
    #[test]
    fn region_crop_uses_the_capture_geometry() {
        let image = egui::ColorImage::filled([200, 100], egui::Color32::RED);
        let geometry = Geometry { size: egui::vec2(100.0, 50.0), pixels_per_point: 2.0, epoch: 7 };
        let reply =
            screenshot_png(&image, Some(egui::Rect::from_min_max(egui::pos2(10.0, 5.0), egui::pos2(60.0, 30.0))), geometry).expect("a valid crop");
        assert_eq!(reply["width"], 100);
        assert_eq!(reply["height"], 50);
        assert_eq!(reply["pixels_per_point"], 2.0);
    }

    /// A retry replaces one reservation in a running aggregate. Three 50 MiB captures that each
    /// retry to 100 MiB must not all be admitted against a stale 150 MiB snapshot.
    #[test]
    fn retries_keep_one_running_aggregate_budget() {
        const MIB: usize = 1024 * 1024;
        let first = retried_reservation(3 * 50 * MIB, 50 * MIB, 100 * MIB).expect("the first retry fits");
        let second = retried_reservation(first, 50 * MIB, 100 * MIB).expect("the second retry fits");
        assert_eq!(second, 250 * MIB);
        assert_eq!(retried_reservation(second, 50 * MIB, 100 * MIB), None, "the third would pass 256 MiB");
        assert_eq!(retried_reservation(0, 0, MAX_IMAGE_BYTES + 1), None, "a single image over the per-image limit never fits");
    }

    #[test]
    fn an_image_with_another_aspect_ratio_is_rejected() {
        let image = egui::ColorImage::filled([200, 100], egui::Color32::RED);
        let geometry = Geometry { size: egui::vec2(100.0, 60.0), pixels_per_point: 2.0, epoch: 0 };
        assert!(screenshot_png(&image, None, geometry).is_err());
    }

    #[test]
    fn an_outside_region_is_rejected_not_panicking() {
        let image = egui::ColorImage::filled([20, 10], egui::Color32::RED);
        let geometry = Geometry { size: egui::vec2(10.0, 5.0), pixels_per_point: 2.0, epoch: 0 };
        assert!(screenshot_png(&image, Some(egui::Rect::from_min_max(egui::pos2(11.0, 6.0), egui::pos2(15.0, 9.0))), geometry).is_err());
    }

    /// A region that a shrink now cuts off is refused, not silently clamped to a smaller crop.
    #[test]
    fn a_region_cut_by_a_shrink_is_rejected_instead_of_cropped() {
        let image = egui::ColorImage::filled([20, 10], egui::Color32::RED);
        let shrunk = Geometry { size: egui::vec2(10.0, 5.0), pixels_per_point: 2.0, epoch: 0 };
        let region = egui::Rect::from_min_max(egui::pos2(8.0, 4.0), egui::pos2(15.0, 9.0));
        assert!(!shrunk.contains_region(region));
        let error = screenshot_png(&image, Some(region), shrunk).expect_err("a cut region must not become a smaller image");
        assert!(error.contains("inside the window"), "{error}");
    }

    /// Screenshot identity is the captured content, not the window's monitor-space origin or the
    /// focused widget: only the capture epoch and the viewport-local size/scale matter.
    #[test]
    fn capture_geometry_compares_size_scale_and_capture_epoch_only() {
        let admitted = Geometry { size: egui::vec2(800.0, 600.0), pixels_per_point: 2.0, epoch: 5 };
        assert_eq!(admitted, Geometry { size: egui::vec2(800.0, 600.0), pixels_per_point: 2.0, epoch: 5 });
        assert_ne!(admitted, Geometry { size: egui::vec2(801.0, 600.0), pixels_per_point: 2.0, epoch: 5 });
        assert_ne!(admitted, Geometry { size: egui::vec2(800.0, 600.0), pixels_per_point: 1.5, epoch: 5 });
        assert_ne!(admitted, Geometry { size: egui::vec2(800.0, 600.0), pixels_per_point: 2.0, epoch: 6 });
        assert!(admitted.contains_region(egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0))));
        assert!(!admitted.contains_region(egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(801.0, 600.0))));
        assert!(!admitted.contains_region(egui::Rect::from_min_max(egui::pos2(-1.0, 0.0), egui::pos2(10.0, 10.0))));
    }
}

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
