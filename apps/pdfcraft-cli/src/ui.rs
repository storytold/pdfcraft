use std::time::Duration;

use crate::cli::UiArgs;
use crate::error::CliError;
use crate::{Io, stdout_line};

// ---- UI control channel client -----------------------------------------------------------------

/// One request to a running app's control channel (`pdfcraft --control FILE`).
pub(crate) fn ui(args: &UiArgs, io: &mut Io) -> Result<(), CliError> {
    use std::io::{BufRead, BufReader, Write as _};
    let file = args.control.as_path();
    let info = read_control_file(file)?;
    let port = info["port"].as_u64().ok_or_else(|| CliError::Ui(format!("{}: no port", file.display())))?;
    // Refused rather than truncated, which would quietly reach some other port.
    let port = u16::try_from(port).map_err(|_| CliError::Ui(format!("{}: {port} is not a port number", file.display())))?;
    let token = info["token"].as_str().ok_or_else(|| CliError::Ui(format!("{}: no token", file.display())))?;
    let method = args.method.as_str();
    let method = if method.starts_with("ui.") { method.to_string() } else { format!("ui.{method}") };
    let params: serde_json::Map<String, serde_json::Value> = args.args.iter().map(|kv| (kv.key.clone(), kv.value.clone())).collect();
    let stream = std::net::TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| CliError::Ui(format!("can't reach the app on port {port}: {e} (is it still running?)")))?;
    let io_error = |e: std::io::Error| CliError::Ui(e.to_string());
    stream.set_read_timeout(Some(Duration::from_secs(40))).map_err(io_error)?;
    let mut write = stream.try_clone().map_err(io_error)?;
    let mut lines = BufReader::new(stream).lines();
    let mut rpc = |id: u64, method: &str, params: serde_json::Value| -> Result<serde_json::Value, CliError> {
        writeln!(write, "{}", serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).map_err(io_error)?;
        let line = lines.next().ok_or_else(|| CliError::Ui("the app closed the connection".into()))?.map_err(io_error)?;
        let reply: serde_json::Value = serde_json::from_str(&line).map_err(|e| CliError::Ui(e.to_string()))?;
        match reply.get("error") {
            Some(e) => Err(CliError::Ui(e["message"].as_str().unwrap_or("error").to_string())),
            None => Ok(reply["result"].clone()),
        }
    };
    rpc(1, "auth", serde_json::json!({ "token": token }))?;
    let mut result = rpc(2, &method, serde_json::Value::Object(params))?;
    if let (Some(out), Some(data)) = (&args.out, result.get("png_base64").and_then(|d| d.as_str())) {
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| CliError::Ui(e.to_string()))?;
        std::fs::write(out, png).map_err(|e| CliError::file(out, e))?;
        result["png_base64"] = serde_json::json!(format!("written to {}", out.display()));
    }
    stdout_line(io, format_args!("{}", serde_json::to_string_pretty(&result).unwrap_or_default()))
}

/// Most a control file may hold. The app writes under 100 bytes.
const CONTROL_FILE_MAX: u64 = 64 * 1024;

const CONTROL_FILE_ADVICE: &str =
    "Start the app with a control file in a folder only you can write, such as `pdfcraft --control ~/.pdfcraft-control.json`";

/// Read the file written by `pdfcraft --control FILE`.
///
/// It names the port that receives the token and every command, typed text included, so it must
/// be the user's own. In a shared folder such as `/tmp`, another local user can create the file
/// first; the app then can't write it, and the CLI would talk to the other user's listener.
/// Refused: a symbolic link, anything but a regular file, and on Unix a file owned by someone
/// else or open to other users (the app writes it with mode 600).
fn read_control_file(file: &std::path::Path) -> Result<serde_json::Value, CliError> {
    use std::io::Read as _;
    let name = file.display();
    let fail = |message: String| CliError::Ui(format!("{name}: {message}"));
    let at_path = std::fs::symlink_metadata(file).map_err(|e| fail(e.to_string()))?;
    if at_path.file_type().is_symlink() {
        return Err(fail(format!("is a symbolic link, which could point at another user's file. {CONTROL_FILE_ADVICE}")));
    }
    if !at_path.is_file() {
        return Err(fail(format!("not a regular file. {CONTROL_FILE_ADVICE}")));
    }
    let f = std::fs::File::open(file).map_err(|e| fail(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Checked on the open file, so nothing swapped in after `symlink_metadata` gets through.
        let opened = f.metadata().map_err(|e| fail(e.to_string()))?;
        if (opened.dev(), opened.ino()) != (at_path.dev(), at_path.ino()) {
            return Err(fail(format!("was replaced while being opened. {CONTROL_FILE_ADVICE}")));
        }
        if let Some(problem) = control_file_problem(opened.uid(), opened.mode(), current_uid()) {
            return Err(fail(format!("{problem}. {CONTROL_FILE_ADVICE}")));
        }
    }
    let mut text = String::new();
    f.take(CONTROL_FILE_MAX.saturating_add(1)).read_to_string(&mut text).map_err(|e| fail(e.to_string()))?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > CONTROL_FILE_MAX {
        return Err(fail(format!("too large for a control file (over {} KiB)", CONTROL_FILE_MAX / 1024)));
    }
    serde_json::from_str(&text).map_err(|e| fail(e.to_string()))
}

/// Why user `me` can't trust a control file with this owner and mode, if they can't.
#[cfg(any(test, unix))]
fn control_file_problem(owner: u32, mode: u32, me: u32) -> Option<String> {
    if owner != me {
        return Some(format!("owned by another user (uid {owner}; you are uid {me}), who may be the one listening for your commands"));
    }
    let permissions = mode & 0o7777;
    (permissions & 0o077 != 0).then(|| format!("other users have access to it (mode {permissions:o}; the app writes it with mode 600)"))
}

/// The effective user id: the owner of the files this process creates, and so of a control file
/// the app wrote for this user. std has no `geteuid` and this crate forbids `unsafe`; rustix's is
/// safe and can't fail.
#[cfg(unix)]
fn current_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

#[cfg(test)]
mod tests {
    use super::control_file_problem;

    #[test]
    fn a_control_file_must_be_yours_and_private() {
        // Regular file type bits (0o100000) don't matter, only the owner and the permission bits.
        assert_eq!(control_file_problem(1000, 0o100600, 1000), None);
        assert_eq!(control_file_problem(1000, 0o600, 1000), None);
        assert_eq!(control_file_problem(1000, 0o400, 1000), None);
        let other = control_file_problem(1001, 0o600, 1000).unwrap_or_default();
        assert!(other.contains("owned by another user") && other.contains("1001"), "{other}");
        // Root gets no exception: a file a user planted is still theirs.
        assert!(control_file_problem(1000, 0o600, 0).is_some_and(|p| p.contains("owned by another user")));
        for mode in [0o644, 0o640, 0o604, 0o660, 0o606, 0o620, 0o602, 0o610, 0o601, 0o100666] {
            let open = control_file_problem(1000, mode, 1000).unwrap_or_default();
            assert!(open.contains("other users"), "{mode:o}: {open}");
        }
        assert!(control_file_problem(1000, 0o100644, 1000).unwrap_or_default().contains("mode 644"));
    }
}
