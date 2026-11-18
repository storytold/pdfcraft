//! `pdfcraft-cli ui --control PORT`: the CLI reaches the app on the port the user names and
//! sends the token it read from the user's own token file.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::JoinHandle;
use std::time::Duration;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A folder for one test, removed when the test ends.
struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(test: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-ui-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// A stand-in for the app's control listener that records everything a client sends and answers
/// each line the way the framework's server does (`id`, `ok`, `result`).
struct Spy {
    port: u16,
    seen: JoinHandle<String>,
}

fn spy() -> Spy {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut write = stream.try_clone().unwrap();
        let mut seen = String::new();
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            let id = serde_json::from_str::<serde_json::Value>(&line).ok().and_then(|v| v.get("id").cloned()).unwrap_or_default();
            seen.push_str(&line);
            seen.push('\n');
            if writeln!(write, "{}", serde_json::json!({ "id": id, "ok": true, "result": {} })).is_err() {
                break;
            }
        }
        seen
    });
    Spy { port, seen }
}

impl Spy {
    /// Everything the client sent. Connects once itself, so the listener wakes up when the client
    /// never connected; that connection sends nothing.
    fn received(self) -> String {
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        self.seen.join().unwrap()
    }
}

/// Write a token file the way the app does: readable by the owner only.
fn write_private(path: &Path, text: &str) {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path).unwrap().write_all(text.as_bytes()).unwrap();
}

#[test]
fn a_token_file_of_your_own_reaches_the_app() {
    let dir = scratch("own");
    let spy = spy();
    let token_file = dir.join("token");
    write_private(&token_file, TOKEN);
    let o = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli"))
        .arg("ui")
        .arg("--control")
        .arg(spy.port.to_string())
        .arg("--control-token-file")
        .arg(&token_file)
        .arg("state")
        .output()
        .unwrap();
    let received = spy.received();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(received.contains(TOKEN) && received.contains("ui.state"), "{received}");
}
