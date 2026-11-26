//! The client of a running app's control channel (`pdfcraft --control <PORT>`).
//!
//! The port comes from the user's own flag and the token from `--control-token` or a token file
//! (`--control-token-file`, which the framework refuses when group or others can read it). No
//! file names the port, so a file planted by another user cannot send the token anywhere else.

use craft::automation::args::{EnvNames, FlagNames, control_port};
use craft::automation::client::ControlClient;
use craft::automation::security::client_token;

use crate::cli::UiArgs;
use crate::error::CliError;
use crate::{Io, stdout_line};

/// One request to a running app's control channel (`pdfcraft --control PORT`).
pub(crate) fn ui(args: &UiArgs, io: &mut Io) -> Result<(), CliError> {
    let port = control_port(&args.control).map_err(|e| CliError::Ui(format!("--control {}: {e}", args.control)))?;
    let env = EnvNames::for_app("PDFCRAFT");
    let env_token = std::env::var(&env.token).ok().filter(|v| !v.trim().is_empty());
    let env_file = std::env::var_os(&env.token_file).filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
    // A flag wins over its variable, as for the app's own options.
    let token_flag = args.control_token.as_deref();
    let file_flag = args.control_token_file.clone();
    let token_file_given = file_flag.or_else(|| if token_flag.is_some() { None } else { env_file });
    let supplied = token_flag.map(str::to_string).or_else(|| if token_file_given.is_some() { None } else { env_token });
    let token = client_token(supplied.as_deref(), token_file_given.as_deref(), &FlagNames::default(), &env).map_err(|e| CliError::Ui(e.to_string()))?;

    let method = args.method.as_str();
    let method = if method.starts_with("ui.") { method.to_string() } else { format!("ui.{method}") };
    let params: serde_json::Map<String, serde_json::Value> = args.args.iter().map(|kv| (kv.key.clone(), kv.value.clone())).collect();

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut client = ControlClient::connect(addr, &token)
        .map_err(|e| CliError::Ui(format!("can't reach the app on port {port}: {e} (is it still running?)")))?;
    let mut result = client.call(&method, serde_json::Value::Object(params)).map_err(|e| CliError::Ui(e.to_string()))?;
    if let (Some(out), Some(data)) = (&args.out, result.get("base64").and_then(|d| d.as_str())) {
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| CliError::Ui(e.to_string()))?;
        std::fs::write(out, png).map_err(|e| CliError::file(out, e))?;
        result["base64"] = serde_json::json!(format!("written to {}", out.display()));
    }
    stdout_line(io, format_args!("{}", serde_json::to_string_pretty(&result).unwrap_or_default()))
}
