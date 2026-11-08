use super::options::{UI_OPTIONS, flag, positional, validate_options};
use super::{CliError, Duration, stdout_line};

// ---- UI control channel client -----------------------------------------------------------------

/// One request to a running app's control channel (`pdfcraft --control FILE`).
pub(crate) fn ui(args: &[String]) -> Result<(), CliError> {
    validate_options("ui", args, UI_OPTIONS)?;
    let file = flag(args, "--control").ok_or("ui: missing --control FILE (start the app with `pdfcraft --control FILE`)")?;
    let mut window = super::control::Connection::new(file, Duration::from_secs(40));
    // The file is checked before anything else, as it always was.
    window.check()?;
    let pos = positional(args, UI_OPTIONS);
    let method =
        *pos.first().ok_or("ui: missing method (state, inspect, click, drag, type, key, command, commands, set, open, screenshot, tool.call)")?;
    // `state` means `ui.state`; a full name such as `tool.call` is sent as it is.
    let method = if method.contains('.') { method.to_string() } else { format!("ui.{method}") };
    let mut params = serde_json::Map::new();
    for kv in pos.iter().skip(1) {
        let (k, v) = kv.split_once('=').ok_or(format!("ui: expected key=value, got {kv:?}"))?;
        params.insert(k.to_string(), serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.to_string())));
    }
    let mut result = window.call(&method, serde_json::Value::Object(params))?;
    if let (Some(out), Some(data)) = (flag(args, "--out"), result.get("png_base64").and_then(|d| d.as_str())) {
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
        std::fs::write(out, png).map_err(|e| format!("{out}: {e}"))?;
        result["png_base64"] = serde_json::json!(format!("written to {out}"));
    }
    stdout_line(format_args!("{}", serde_json::to_string_pretty(&result).unwrap_or_default()))?;
    Ok(())
}
