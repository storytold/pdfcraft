//! `pdfcraft-cli ui --control FILE`: commands go only to an app whose control file is plausibly
//! the user's own. Another local user can create a file at a shared path such as `/tmp/pc.json`
//! first, naming their own port and token; the CLI must not then send them typed text or trust
//! their replies.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread::JoinHandle;
use std::time::Duration;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

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

/// A stand-in for the app's control listener that records everything a client sends.
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
            if writeln!(write, "{}", serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": { "ok": true } })).is_err() {
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

fn control_json(port: u16) -> String {
    serde_json::json!({ "port": port, "token": TOKEN, "pid": 1 }).to_string()
}

/// Write a control file the way the app does: readable by the owner only.
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

fn ui(control: &Path, method: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli")).arg("ui").arg("--control").arg(control).arg(method).output().unwrap()
}

fn assert_refused(o: &Output, received: &str, expect: &str) {
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(!received.contains(TOKEN), "the token was sent: {received}");
    assert!(!o.status.success(), "expected a refusal; stderr: {stderr}");
    assert!(stderr.contains(expect), "expected {expect:?} in: {stderr}");
}

#[test]
fn a_control_file_of_your_own_still_works() {
    let dir = scratch("own");
    let spy = spy();
    let control = dir.join("pc.json");
    write_private(&control, &control_json(spy.port));
    let o = ui(&control, "state");
    let received = spy.received();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(received.contains(TOKEN) && received.contains("ui.state"), "{received}");
}

/// Checking the owner must not depend on anything but the control file: not on being able to
/// write the temp folder (a sandbox, a read-only container, an odd `TMPDIR`).
#[cfg(unix)]
#[test]
fn a_control_file_of_your_own_works_without_a_writable_temp_folder() {
    let dir = scratch("no-tmp");
    let spy = spy();
    let control = dir.join("pc.json");
    write_private(&control, &control_json(spy.port));
    let missing = dir.join("no-such-temp-folder");
    let o = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli"))
        .env("TMPDIR", &missing)
        .arg("ui")
        .arg("--control")
        .arg(&control)
        .arg("state")
        .output()
        .unwrap();
    let received = spy.received();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(received.contains(TOKEN) && received.contains("ui.state"), "{received}");
    assert!(!missing.exists(), "the CLI created the temp folder");
}

#[test]
fn a_symlinked_control_file_is_refused_before_connecting() {
    let dir = scratch("symlink");
    let spy = spy();
    let target = dir.join("theirs.json");
    write_private(&target, &control_json(spy.port));
    let control = dir.join("pc.json");
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&target, &control);
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&target, &control);
    if let Err(e) = linked {
        eprintln!("skipped: can't create a symlink here ({e})");
        drop(spy.received());
        return;
    }
    let o = ui(&control, "type");
    assert_refused(&o, &spy.received(), "symbolic link");
}

#[test]
fn an_oversized_control_file_is_refused() {
    let dir = scratch("oversized");
    let spy = spy();
    let control = dir.join("pc.json");
    // Valid JSON, padded far beyond anything the app writes.
    write_private(&control, &format!("{}{}", control_json(spy.port), " ".repeat(1 << 20)));
    let o = ui(&control, "state");
    assert_refused(&o, &spy.received(), "too large");
}

#[test]
fn a_port_beyond_65535_is_refused_not_wrapped() {
    let dir = scratch("port");
    let spy = spy();
    let control = dir.join("pc.json");
    // 65536 more than the spy's port: truncated to 16 bits, it would reach the spy.
    let json = serde_json::json!({ "port": u64::from(spy.port) + 65536, "token": TOKEN, "pid": 1 }).to_string();
    write_private(&control, &json);
    let o = ui(&control, "state");
    assert_refused(&o, &spy.received(), "not a port number");
}

#[cfg(unix)]
#[test]
fn a_control_file_other_users_can_read_or_write_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("mode");
    for mode in [0o644, 0o640, 0o602, 0o620] {
        let spy = spy();
        let control = dir.join(format!("pc-{mode:o}.json"));
        write_private(&control, &control_json(spy.port));
        std::fs::set_permissions(&control, std::fs::Permissions::from_mode(mode)).unwrap();
        let o = ui(&control, "state");
        assert_refused(&o, &spy.received(), "other users");
    }
}

/// Needs root to give a file to another user, so it checks something only where the tests run as
/// root, such as a root container. GitHub's Linux and macOS runners don't, so there the ownership
/// rule is covered by the unit test alone.
#[cfg(unix)]
#[test]
fn a_control_file_owned_by_another_user_is_refused() {
    use std::os::unix::fs::MetadataExt;
    let dir = scratch("owner");
    let spy = spy();
    let control = dir.join("pc.json");
    write_private(&control, &control_json(spy.port));
    if std::fs::metadata(&control).unwrap().uid() != 0 {
        eprintln!("skipped: needs root to create a file owned by another user");
        drop(spy.received());
        return;
    }
    std::os::unix::fs::chown(&control, Some(65534), None).unwrap();
    let o = ui(&control, "state");
    assert_refused(&o, &spy.received(), "owned by another user");
}
