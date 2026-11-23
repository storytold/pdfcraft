use craft::automation::Reply;
use craft::automation::files::pick_method;
use craft::automation::screenshot::CaptureGeometry;
#[cfg(not(target_arch = "wasm32"))]
use craft::automation::window::window_method;
use craft::ui::runtime::windows::WindowHost;
use egui::ViewportId;
use serde_json::{Value, json};

use crate::PdfCraftApp;

/// Validate and dispatch one of the app's own control methods (and the dock, window and pick tables),
/// rejecting unknown methods and malformed parameters with actionable errors.
pub(super) fn handle(app: &mut PdfCraftApp, ctx: &egui::Context, method: &str, p: &Value) -> Reply {
    let host = WindowHost::of(ctx);
    if let Some(result) = crate::side_panels::dock_method(app, method, p) {
        return Reply::from_result(result);
    }
    // The window table (size, position, minimise) is the desktop's: the page owns the web build's window.
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(result) = window_method(&host, ViewportId::ROOT, method, p) {
        return match result {
            Ok(_) => Reply::AfterInput,
            Err(e) => Reply::from_result(Err(e)),
        };
    }
    if let Some(result) = pick_method(app.host.files(), method, p) {
        return Reply::from_result(result);
    }
    let str_param = |k: &str| p.get(k).and_then(Value::as_str).ok_or_else(|| format!("{method}: missing string parameter {k}"));
    let r: Result<Value, String> = (|| match method {
        "ui.state" => {
            let mut state = app.state(ctx);
            // The viewport's facts are fresh in `logic`; the last UI pass's rectangle can be stale.
            if let Some(facts) = host.screenshot_facts(ViewportId::ROOT) {
                let geometry = CaptureGeometry::from_facts(facts, None);
                let [width, height] = geometry.size_points;
                state["window"] = json!({ "width": width, "height": height, "pixels_per_point": geometry.pixels_per_point });
            }
            Ok(state)
        }
        "ui.commands" => Ok(app.commands()),
        "ui.panel" | "ui.dock.reset" => app.panel(method == "ui.dock.reset", p),
        "ui.command" if str_param("id")? == "ui.panel" => app.panel(false, p.get("params").unwrap_or(p)),
        "ui.command" => app.command(ctx, str_param("id")?),
        "ui.set" => {
            let value = match p.get("value") {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => return Err("ui.set: missing parameter value".into()),
            };
            app.set(ctx, str_param("key")?, &value)
        }
        "ui.open" => app.open(str_param("path")?),
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
            Ok(json!({ "requested": [size.x, size.y] }))
        }
        "ui.quit" => {
            only_params(p, method, &[])?;
            if cfg!(target_arch = "wasm32") {
                return Err("ui.quit needs a native window".into());
            }
            let quitting = app.request_quit(ctx);
            Ok(json!({ "quitting": quitting, "needs_confirmation": !quitting }))
        }
        other => Err(format!(
            "unknown method {other:?} (ui.state, ui.inspect, ui.click, ui.focus, ui.move, ui.release, ui.wheel, ui.key, ui.type, ui.ime, ui.drop, ui.capsLock, ui.dialog.*, ui.pick, ui.window, ui.dock.apply, ui.dock.reset, ui.panel, ui.resize, ui.quit, ui.command, ui.commands, ui.set, ui.open, ui.screenshot)"
        )),
    })();
    Reply::from_result(r)
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
