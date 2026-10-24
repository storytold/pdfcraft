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
/// that the window is not being drawn. While a window is hidden, minimized or fully covered,
/// eframe keeps calling `logic` but runs no egui pass, so input and screenshots wait.
const NOT_DRAWN_TIMEOUT: f64 = 5.0;

/// Wall-clock seconds (egui's own clock stops while the window is not drawn).
fn now_secs() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
    }
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() / 1000.0
    }
}

fn not_drawn(what: &str) -> String {
    format!(
        "{what} did not happen within {NOT_DRAWN_TIMEOUT:.0} s: the window is not being drawn (it may be hidden, minimized or covered); bring it to the front and retry"
    )
}

/// One request from a client, answered on `reply`.
pub struct ControlRequest {
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
        let req = ControlRequest { method: method.into(), params, reply };
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
    /// Event batches to inject, one batch per frame.
    inject: VecDeque<Vec<egui::Event>>,
    /// Real egui passes so far (counted in `output_hook`, which `logic`-only calls never reach).
    passes: u64,
}

/// The egui plugin half of the control channel.
pub struct ControlPlugin {
    shared: Arc<Mutex<Shared>>,
}

impl egui::Plugin for ControlPlugin {
    fn debug_name(&self) -> &'static str {
        "pdfcraft-control"
    }

    fn input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        let Ok(mut s) = self.shared.lock() else { return };
        if let Some(batch) = s.inject.pop_front() {
            input.events.extend(batch);
        }
        if !s.inject.is_empty() {
            ctx.request_repaint();
        }
    }

    fn output_hook(&mut self, _ctx: &egui::Context, output: &mut egui::FullOutput) {
        let Ok(mut s) = self.shared.lock() else { return };
        s.passes += 1;
        let Some(update) = &output.platform_output.accesskit_update else { return };
        s.nodes = update.nodes.iter().cloned().collect();
        s.root = update.tree.as_ref().map(|t| t.root);
        s.focus = Some(update.focus);
    }
}

enum Pending {
    /// Answer after `frames` more frames (the injected input has been handled by then).
    Frames {
        /// Answer once this many egui passes have run (the injected input has been handled).
        until_pass: u64,
        since: f64,
        reply: Sender<Reply>,
        value: Value,
    },
    Screenshot {
        tag: u64,
        region: Option<egui::Rect>,
        reply: Sender<Reply>,
        /// When it was requested (egui time, seconds).
        since: f64,
    },
}

/// The app half: the request queue and requests waiting for later frames.
pub struct Control {
    rx: Receiver<ControlRequest>,
    shared: Arc<Mutex<Shared>>,
    pending: Vec<Pending>,
    next_tag: u64,
}

/// Install the control channel on `ctx`: returns the app half and a client.
pub fn attach(ctx: &egui::Context) -> (Control, ControlClient) {
    let shared = Arc::new(Mutex::new(Shared::default()));
    ctx.add_plugin(ControlPlugin { shared: shared.clone() });
    ctx.enable_accesskit();
    let (tx, rx) = channel();
    (Control { rx, shared, pending: Vec::new(), next_tag: 1 }, ControlClient { tx, ctx: ctx.clone() })
}

/// What a request needs from the app (implemented by `PdfCraftApp`).
pub(crate) trait Host {
    fn state(&self) -> Value;
    fn command(&mut self, id: &str) -> Reply;
    fn dock(&mut self, reset: bool, params: &Value) -> Reply;
    fn commands(&self) -> Value;
    fn set(&mut self, key: &str, value: &str) -> Reply;
    fn open(&mut self, path: &str) -> Reply;
    fn quit(&mut self, ctx: &egui::Context) -> bool;
}

impl Control {
    /// Handle new requests and finish waiting ones. Call at the start of every frame.
    pub(crate) fn tick(&mut self, ctx: &egui::Context, host: &mut impl Host) {
        self.finish_pending(ctx);
        while let Ok(req) = self.rx.try_recv() {
            match self.handle(ctx, host, &req.method, &req.params) {
                Handled::Now(r) => {
                    let _ = req.reply.send(r);
                }
                Handled::AfterFrames(frames, value) => {
                    let until_pass = self.passes() + u64::from(frames);
                    self.pending.push(Pending::Frames { until_pass, since: now_secs(), reply: req.reply, value });
                }
                Handled::Screenshot(region) => {
                    let tag = self.next_tag;
                    self.next_tag += 1;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(tag)));
                    self.pending.push(Pending::Screenshot { tag, region, reply: req.reply, since: now_secs() });
                }
            }
        }
        if !self.pending.is_empty() {
            ctx.request_repaint();
        }
    }

    fn finish_pending(&mut self, ctx: &egui::Context) {
        let shots: Vec<(u64, Arc<egui::ColorImage>)> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { user_data, image, .. } => {
                        user_data.data.as_ref().and_then(|d| d.downcast_ref::<u64>()).map(|tag| (*tag, image.clone()))
                    }
                    _ => None,
                })
                .collect()
        });
        let ppp = ctx.pixels_per_point();
        let now = now_secs();
        let passes = self.passes();
        let mut keep = Vec::new();
        for p in self.pending.drain(..) {
            match p {
                Pending::Frames { until_pass, reply, value, .. } if passes >= until_pass => {
                    let _ = reply.send(Ok(value));
                }
                Pending::Frames { since, reply, .. } if now - since > NOT_DRAWN_TIMEOUT => {
                    // The agent is told it failed, so it must not happen later either.
                    if let Ok(mut sh) = self.shared.lock() {
                        sh.inject.clear();
                    }
                    let _ = reply.send(Err(not_drawn("handling the input")));
                }
                p @ Pending::Frames { .. } => keep.push(p),
                Pending::Screenshot { tag, region, reply, since } => match shots.iter().find(|(t, _)| *t == tag) {
                    Some((_, image)) => {
                        let _ = reply.send(screenshot_png(image, region, ppp));
                    }
                    // The platform skips painting windows that are hidden, minimized or fully
                    // covered, so no screenshot ever arrives: say so instead of hanging.
                    None if now - since > NOT_DRAWN_TIMEOUT => {
                        let _ = reply.send(Err(not_drawn("the screenshot")));
                    }
                    None => keep.push(Pending::Screenshot { tag, region, reply, since }),
                },
            }
        }
        self.pending = keep;
    }

    fn passes(&self) -> u64 {
        self.shared.lock().map(|s| s.passes).unwrap_or(0)
    }

    fn inject(&self, batches: Vec<Vec<egui::Event>>) -> u32 {
        let n = batches.len() as u32;
        if let Ok(mut s) = self.shared.lock() {
            s.inject.extend(batches);
        }
        // The last batch is handled during the frame it is injected into; answer one frame later.
        n + 1
    }

    fn handle(&mut self, ctx: &egui::Context, host: &mut impl Host, method: &str, p: &Value) -> Handled {
        let str_param = |k: &str| p.get(k).and_then(Value::as_str).ok_or_else(|| format!("{method}: missing string parameter {k}"));
        let r: Result<Handled, String> = (|| match method {
            "ui.state" => {
                let mut state = host.state();
                let size = ctx.input(|i| i.viewport_rect().size());
                state["window"] =
                    json!({ "width": size.x, "height": size.y, "pixels_per_point": ctx.pixels_per_point(), "focused": ctx.input(|i| i.focused) });
                Ok(Handled::Now(Ok(state)))
            }
            "ui.commands" => Ok(Handled::Now(Ok(host.commands()))),
            "ui.dock" | "ui.dock.reset" => Ok(Handled::Now(host.dock(method == "ui.dock.reset", p))),
            "ui.command" if str_param("id")? == "ui.dock" => Ok(Handled::Now(host.dock(false, p.get("params").unwrap_or(p)))),
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
            "ui.click" => self.click(p),
            "ui.focus" => self.focus(p),
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
            "ui.drag" => self.drag(p),
            "ui.move" => {
                let point = |key: &str| -> Result<f32, String> {
                    p.get(key)
                        .and_then(Value::as_f64)
                        .map(|n| n as f32)
                        .filter(|n| n.is_finite())
                        .ok_or_else(|| format!("ui.move: {key} must be a finite coordinate in points"))
                };
                let pos = egui::pos2(point("x")?, point("y")?);
                Ok(Handled::AfterFrames(self.inject(vec![vec![egui::Event::PointerMoved(pos)]]), json!({ "moved": [pos.x, pos.y] })))
            }
            "ui.type" => {
                let text = str_param("text")?.to_string();
                Ok(Handled::AfterFrames(self.inject(vec![vec![egui::Event::Text(text)]]), json!({ "typed": true })))
            }
            "ui.key" => {
                let name = str_param("key")?;
                let key = egui::Key::from_name(name)
                    .ok_or_else(|| format!("ui.key: unknown key {name:?} (egui key names: A, Enter, Escape, ArrowDown, F5, …)"))?;
                let modifiers = modifiers(p.get("modifiers"))?;
                let ev = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
                Ok(Handled::AfterFrames(self.inject(vec![vec![ev(true)], vec![ev(false)]]), json!({ "key": name })))
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
    fn drag(&mut self, p: &Value) -> Result<Handled, String> {
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
        let n = self.inject(frames);
        Ok(Handled::AfterFrames(n, json!({ "dragged": [[from.x, from.y], [to.x, to.y]] })))
    }

    fn click(&mut self, p: &Value) -> Result<Handled, String> {
        if let (Some(x), Some(y)) = (p.get("x").and_then(Value::as_f64), p.get("y").and_then(Value::as_f64)) {
            let point = |key: &str, n: f64| {
                Some(n as f32).filter(|n| n.is_finite()).ok_or_else(|| format!("ui.click: {key} must be a finite coordinate in points"))
            };
            let pos = egui::pos2(point("x", x)?, point("y", y)?);
            let which = pointer_button(p, "ui.click")?;
            let button = |pressed| egui::Event::PointerButton { pos, button: which, pressed, modifiers: egui::Modifiers::NONE };
            // Keep hover/layout warmup, then make an instantaneous click even on slow frames.
            let frames = self.inject(vec![vec![egui::Event::PointerMoved(pos)], vec![button(true), button(false)]]);
            return Ok(Handled::AfterFrames(frames, json!({ "clicked": [x, y] })));
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
        let frames = self.inject(vec![vec![egui::Event::AccessKitActionRequest(action)]]);
        Ok(Handled::AfterFrames(frames, json!({ "clicked": id.0.to_string(), "label": label })))
    }

    fn focus(&mut self, p: &Value) -> Result<Handled, String> {
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
        let frames = self.inject(vec![vec![egui::Event::AccessKitActionRequest(action)]]);
        Ok(Handled::AfterFrames(frames, json!({ "focus_requested": id.0.to_string() })))
    }
}

enum Handled {
    Now(Reply),
    AfterFrames(u32, Value),
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
fn screenshot_png(image: &egui::ColorImage, region: Option<egui::Rect>, ppp: f32) -> Reply {
    let [w, h] = image.size;
    let (x0, y0, x1, y1) = match region {
        Some(r) => {
            let px = |v: f32, max: usize| ((v * ppp).round().max(0.0) as usize).min(max);
            (px(r.min.x, w), px(r.min.y, h), px(r.max.x, w), px(r.max.y, h))
        }
        None => (0, 0, w, h),
    };
    if x1 <= x0 || y1 <= y0 {
        return Err("the region is outside the window".into());
    }
    let mut rgba = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
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
    use base64::Engine as _;
    Ok(json!({ "png_base64": base64::engine::general_purpose::STANDARD.encode(out), "width": cw, "height": ch, "pixels_per_point": ppp }))
}

impl Host for crate::PdfCraftApp {
    fn dock(&mut self, reset: bool, params: &Value) -> Reply {
        crate::docking::command(self, reset, params)
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
        #[cfg(not(target_arch = "wasm32"))]
        let javascript_menu = self.link_script.inspect();
        #[cfg(target_arch = "wasm32")]
        let javascript_menu = serde_json::Value::Null;
        json!({
            "javascript_menu": javascript_menu,
            "docking": self.docking,
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
            "left_panel": if self.left_open { json!(format!("{:?}", self.left)) } else { Value::Null },
            "tools_side": if self.docking.tools_on_right() { "right" } else { "left" },
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
        list.push(json!({"id": "ui.dock", "label": "Arrange Panel", "enabled": true, "params": "action, or operation/panel; move: target/zone/before; float/moveFloating: rect; resizeSplit: path/size; setStackOpen: open; resizeStack: height"}));
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
