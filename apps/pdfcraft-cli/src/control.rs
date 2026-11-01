//! The client side of the app's UI control channel (`pdfcraft --control FILE`): `ui` sends it one
//! request, and `mcp --connect FILE` relays MCP tool calls to the window through it.
//!
//! The token goes only to `127.0.0.1` (a control file names a port, never a host), and only after
//! the control file has passed [`read_control_file`]'s checks.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{Value, json};

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
pub fn read_control_file(file: &str) -> Result<Value, String> {
    use std::io::Read as _;
    let at_path = std::fs::symlink_metadata(file).map_err(|e| format!("{file}: {e}"))?;
    if at_path.file_type().is_symlink() {
        return Err(format!("{file}: is a symbolic link, which could point at another user's file. {CONTROL_FILE_ADVICE}"));
    }
    if !at_path.is_file() {
        return Err(format!("{file}: not a regular file. {CONTROL_FILE_ADVICE}"));
    }
    let f = std::fs::File::open(file).map_err(|e| format!("{file}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Checked on the open file, so nothing swapped in after `symlink_metadata` gets through.
        let opened = f.metadata().map_err(|e| format!("{file}: {e}"))?;
        if (opened.dev(), opened.ino()) != (at_path.dev(), at_path.ino()) {
            return Err(format!("{file}: was replaced while being opened. {CONTROL_FILE_ADVICE}"));
        }
        if let Some(problem) = control_file_problem(opened.uid(), opened.mode(), current_uid()) {
            return Err(format!("{file}: {problem}. {CONTROL_FILE_ADVICE}"));
        }
    }
    let mut text = String::new();
    f.take(CONTROL_FILE_MAX.saturating_add(1)).read_to_string(&mut text).map_err(|e| format!("{file}: {e}"))?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > CONTROL_FILE_MAX {
        return Err(format!("{file}: too large for a control file (over {} KiB)", CONTROL_FILE_MAX / 1024));
    }
    serde_json::from_str(&text).map_err(|e| format!("{file}: {e}"))
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

/// The port and token in a checked control file.
fn endpoint(file: &str) -> Result<(u16, String), String> {
    let info = read_control_file(file)?;
    let port = info["port"].as_u64().ok_or(format!("{file}: no port"))?;
    // Refused rather than truncated, which would quietly reach some other port.
    let port = u16::try_from(port).map_err(|_| format!("{file}: {port} is not a port number"))?;
    let token = info["token"].as_str().ok_or(format!("{file}: no token"))?;
    Ok((port, token.to_string()))
}

type Stream = (BufReader<TcpStream>, TcpStream);

/// An authenticated connection to the window behind a control file. It connects on the first
/// call and again after the app restarts, reading the file each time (the app writes a new port
/// and token at every start).
pub struct Connection {
    file: String,
    read_timeout: Duration,
    stream: Option<Stream>,
    next_id: u64,
}

/// A transport failure (never replayed), or the app's own answer.
enum Failure {
    Transport(String),
    Refused(String),
}

impl Connection {
    /// For the control file `file`; a reply must come within `read_timeout`.
    pub fn new(file: &str, read_timeout: Duration) -> Self {
        Self { file: file.to_string(), read_timeout, stream: None, next_id: 1 }
    }

    /// Check the control file now (a bad one is refused at once). `Ok(false)`: there is no file
    /// yet, so the window isn't running.
    pub fn check(&self) -> Result<bool, String> {
        match std::fs::symlink_metadata(&self.file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            _ => endpoint(&self.file).map(|_| true),
        }
    }

    fn connect(&mut self) -> Result<(), String> {
        self.stream = None;
        let file = self.file.clone();
        let (port, token) = match endpoint(&file) {
            Err(_) if std::fs::symlink_metadata(&file).is_err() => {
                return Err(format!("no PdfCraft window: {file} doesn't exist; start the app with `pdfcraft --control {file}`"));
            }
            other => other?,
        };
        let stream = TcpStream::connect(("127.0.0.1", port))
            .map_err(|e| format!("can't reach the app on port {port}: {e} (is it still running? start it with `pdfcraft --control {file}`)"))?;
        stream.set_read_timeout(Some(self.read_timeout)).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(self.read_timeout)).map_err(|e| e.to_string())?;
        let write = stream.try_clone().map_err(|e| e.to_string())?;
        self.stream = Some((BufReader::new(stream), write));
        match self.exchange("auth", json!({ "token": token })) {
            Ok(_) => Ok(()),
            Err(Failure::Refused(e)) => {
                self.stream = None;
                Err(format!("the app refused the token in {file}: {e}"))
            }
            Err(Failure::Transport(e)) => {
                self.stream = None;
                Err(e)
            }
        }
    }

    fn exchange(&mut self, method: &str, params: Value) -> Result<Value, Failure> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let Some((lines, write)) = self.stream.as_mut() else { return Err(Failure::Transport("not connected".into())) };
        writeln!(write, "{}", json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .and_then(|()| write.flush())
            .map_err(|e| Failure::Transport(e.to_string()))?;
        // A rendered page can be large, but a hostile peer cannot grow a line without bound.
        const MAX_REPLY: u64 = 64 * 1024 * 1024;
        let mut line = String::new();
        let read = lines.take(MAX_REPLY + 1).read_line(&mut line).map_err(|e| Failure::Transport(e.to_string()))?;
        if read == 0 {
            return Err(Failure::Transport("the app closed the connection".into()));
        }
        if read as u64 > MAX_REPLY || !line.ends_with('\n') {
            return Err(Failure::Transport("the app sent an oversized or incomplete reply".into()));
        }
        let reply: Value = serde_json::from_str(&line).map_err(|e| Failure::Transport(format!("bad reply from the app: {e}")))?;
        if reply.get("id") != Some(&json!(id)) || reply.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(Failure::Transport("the app sent a reply for another request or protocol".into()));
        }
        if reply.get("error").is_none() && reply.get("result").is_none() {
            return Err(Failure::Transport("the app sent neither result nor error".into()));
        }
        match reply.get("error") {
            Some(e) => Err(Failure::Refused(e["message"].as_str().unwrap_or("error").to_string())),
            None => Ok(reply["result"].clone()),
        }
    }

    /// Call one method. Never replay an uncertain request: the window may already have acted
    /// on it. A later call reconnects and rereads the control file after a transport failure.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if self.stream.is_none() {
            self.connect()?;
        }
        match self.exchange(method, params) {
            Ok(v) => Ok(v),
            Err(Failure::Refused(e)) => Err(e),
            Err(Failure::Transport(e)) => {
                self.stream = None;
                Err(format!(
                    "{method}: {e}; the request was not replayed because it may already have run. Inspect the window state before trying again"
                ))
            }
        }
    }
}

#[cfg(feature = "mcp")]
impl pdfcraft_automation::window::Window for Connection {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        Connection::call(self, method, params)
    }

    fn describe(&self) -> String {
        format!("the PdfCraft window of control file {}", self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::control_file_problem;

    #[test]
    fn a_lost_reply_is_not_replayed_and_the_next_call_reconnects() {
        use super::*;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let path = std::env::temp_dir().join(format!("pc-no-replay-{}.json", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        writeln!(options.open(&path).unwrap(), "{}", json!({"port":port,"token":"synthetic"})).unwrap();
        let server = std::thread::spawn(move || {
            let mut methods = Vec::new();
            for connection in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut output = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines() {
                    let msg: Value = serde_json::from_str(&line.unwrap()).unwrap();
                    let method = msg["method"].as_str().unwrap();
                    if method != "auth" {
                        methods.push(method.to_string());
                    }
                    if connection == 0 && method == "tool.call" {
                        break;
                    } // applied, then EOF instead of reply
                    writeln!(output, "{}", json!({"jsonrpc":"2.0","id":msg["id"],"result":{"ok":true}})).unwrap();
                    if connection == 1 && method != "auth" {
                        break;
                    }
                }
            }
            methods
        });
        let mut connection = Connection::new(path.to_str().unwrap(), Duration::from_secs(5));
        assert!(connection.call("ui.state", json!({})).is_ok());
        let e = connection.call("tool.call", json!({"name":"page_insert_blank","arguments":{"doc":1,"at":1}})).unwrap_err();
        assert!(e.contains("not replayed"), "{e}");
        assert!(connection.call("ui.state", json!({})).is_ok());
        assert_eq!(server.join().unwrap(), ["ui.state", "tool.call", "ui.state"]);
        std::fs::remove_file(path).unwrap();
    }

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
