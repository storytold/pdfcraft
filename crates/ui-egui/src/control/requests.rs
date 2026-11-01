use super::screenshot::Geometry;
use super::{Action, Control, Host, NodeId, Reply, Value, accesskit, json};

impl Control {
    /// Validate and dispatch one control method, rejecting unknown methods and malformed parameters with
    /// actionable errors.
    pub(super) fn handle(&mut self, ctx: &egui::Context, host: &mut impl Host, method: &str, p: &Value, since: web_time::Instant) -> Handled {
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
            "ui.command" if str_param("id")? == "ui.detach" => {
                let sub = p.get("params").unwrap_or(p);
                let id = parse_doc_or_tab(sub, host, "ui.detach")?;
                Ok(Handled::Now(host.detach(id)))
            }
            "ui.command" if str_param("id")? == "ui.attach" => {
                let sub = p.get("params").unwrap_or(p);
                let id = parse_doc_or_tab(sub, host, "ui.attach")?;
                Ok(Handled::Now(host.attach(id)))
            }
            "ui.command" => Ok(Handled::Now(host.command(str_param("id")?))),
            "ui.detach" | "ui.tab.detach" => {
                let id = parse_doc_or_tab(p, host, method)?;
                Ok(Handled::Now(host.detach(id)))
            }
            "ui.attach" | "ui.tab.attach" => {
                let id = parse_doc_or_tab(p, host, method)?;
                Ok(Handled::Now(host.attach(id)))
            }
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
                "unknown method {other:?} (ui.state, ui.inspect, ui.click, ui.focus, ui.resize, ui.quit, ui.move, ui.drag, ui.type, ui.key, ui.command, ui.commands, ui.set, ui.open, ui.detach, ui.attach, ui.screenshot)"
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

pub(super) enum Handled {
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
pub(super) fn key_batches(key: egui::Key, modifiers: egui::Modifiers) -> Vec<Vec<egui::Event>> {
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

fn parse_doc_or_tab(p: &Value, host: &impl Host, method: &str) -> Result<pdfcraft_engine::DocId, String> {
    only_params(p, method, &["doc", "id", "tab", "index", "params"])?;
    if let Some(doc_val) = p.get("doc") {
        let id_num = doc_val
            .as_u64()
            .or_else(|| doc_val.as_str().and_then(|s| s.parse::<u64>().ok()))
            .ok_or_else(|| format!("{method}: doc id must be an integer"))?;
        return Ok(pdfcraft_engine::DocId(id_num));
    }
    if let Some(tab_val) = p.get("tab").or_else(|| p.get("index")) {
        let tab_num = tab_val
            .as_u64()
            .or_else(|| tab_val.as_str().and_then(|s| s.parse::<u64>().ok()))
            .ok_or_else(|| format!("{method}: tab index must be a non-negative integer"))?;
        let tab_idx = usize::try_from(tab_num).map_err(|_| format!("{method}: tab index out of range"))?;
        return host.doc_id_from_tab(tab_idx).ok_or_else(|| format!("{method}: tab index {tab_idx} out of range"));
    }
    if let Some(id_val) = p.get("id")
        && let Some(id_num) = id_val.as_u64().or_else(|| id_val.as_str().and_then(|s| s.parse::<u64>().ok()))
    {
        return Ok(pdfcraft_engine::DocId(id_num));
    }
    Err(format!("{method}: pass doc id (doc) or tab index (tab)"))
}
