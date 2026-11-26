//! The client of a running app's control channel (`pdfcraft --control <PORT>`).
//!
//! The port comes from the user's own flag and the token from `--control-token` or a token file
//! (`--control-token-file`, which the framework refuses when group or others can read it). No
//! file names the port, so a file planted by another user cannot send the token anywhere else.

use craft::automation::args::{EnvNames, FlagNames, control_port};
use craft::automation::client::ControlClient;
use craft::automation::security::client_token;

use super::options::{UI_OPTIONS, flag, positional, validate_options};
use super::{CliError, stdout_line};

/// One request to a running app's control channel (`pdfcraft --control PORT`).
pub(crate) fn ui(args: &[String]) -> Result<(), CliError> {
    validate_options("ui", args, UI_OPTIONS)?;
    let port = flag(args, "--control").ok_or("ui: missing --control PORT (start the app with `pdfcraft --control PORT`)")?;
    let port = control_port(port).map_err(|e| format!("ui: --control {port}: {e}"))?;
    let env = EnvNames::for_app("PDFCRAFT");
    let env_token = std::env::var(&env.token).ok().filter(|v| !v.trim().is_empty());
    let env_file = std::env::var_os(&env.token_file).filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
    // A flag wins over its variable, as for the app's own options.
    let token_flag = flag(args, "--control-token");
    let file_flag = flag(args, "--control-token-file").map(std::path::PathBuf::from);
    let token_file_given = file_flag.clone().or_else(|| if token_flag.is_some() { None } else { env_file });
    let supplied = token_flag.map(str::to_string).or_else(|| if token_file_given.is_some() { None } else { env_token });
    let token = client_token(supplied.as_deref(), token_file_given.as_deref(), &FlagNames::default(), &env).map_err(|e| format!("ui: {e}"))?;

    let pos = positional(args, UI_OPTIONS);
    let method =
        *pos.first().ok_or("ui: missing method (state, inspect, click, move, release, type, key, command, commands, set, open, screenshot, …)")?;
    let method = if method.starts_with("ui.") { method.to_string() } else { format!("ui.{method}") };
    let mut params = serde_json::Map::new();
    for kv in pos.iter().skip(1) {
        let (k, v) = kv.split_once('=').ok_or(format!("ui: expected key=value, got {kv:?}"))?;
        params.insert(k.to_string(), serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.to_string())));
    }

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut client = ControlClient::connect(addr, &token).map_err(|e| format!("can't reach the app on port {port}: {e} (is it still running?)"))?;
    let mut result = client.call(&method, serde_json::Value::Object(params))?;
    if let (Some(out), Some(data)) = (flag(args, "--out"), result.get("base64").and_then(|d| d.as_str())) {
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
        std::fs::write(out, png).map_err(|e| format!("{out}: {e}"))?;
        result["base64"] = serde_json::json!(format!("written to {out}"));
    }
    stdout_line(format_args!("{}", serde_json::to_string_pretty(&result).unwrap_or_default()))?;
    Ok(())
}
