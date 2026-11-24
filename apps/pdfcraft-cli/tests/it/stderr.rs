//! Diagnostic stderr failures must not replace a command's real result.
#![cfg(target_os = "linux")]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let path = std::env::temp_dir().join(format!("pdfcraft-stderr-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("creating stderr test directory: {error}"),
            }
        }
        panic!("no unused stderr test directory after 128 attempts");
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Only this test's newly spawned child is ever terminated, including assertion unwinding.
struct OwnedChild(Option<Child>);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn run(command: &mut Command, input: Option<&str>) -> Output {
    command.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut owned = OwnedChild(Some(command.spawn().unwrap()));
    if let Some(input) = input {
        owned.0.as_mut().unwrap().stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    }
    // Every fixture emits only a few short lines, below the pipe capacity.
    let deadline = Instant::now() + Duration::from_secs(30);
    while owned.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "owned CLI child exceeded thirty seconds");
        std::thread::sleep(Duration::from_millis(10));
    }
    owned.0.take().unwrap().wait_with_output().unwrap()
}
fn full() -> Stdio {
    std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap().into()
}
fn render(pdf: &Path, out: &Path, stderr: Stdio) -> Output {
    run(Command::new(BIN).arg("render").arg(pdf).args(["--page", "1", "--dpi", "18", "--out"]).arg(out).stdout(Stdio::piped()).stderr(stderr), None)
}

#[test]
fn failing_diagnostics_preserve_render_success_and_real_text_or_file_errors() {
    let dir = Scratch::new();
    let bytes = pdfcraft_engine::Session::new().create_from_text("Diagnostic test", "Ordinary diagnostic text").unwrap();
    let pdf = dir.0.join("input.pdf");
    std::fs::write(&pdf, bytes.as_slice()).unwrap();
    let image_path = dir.0.join("page.pam");
    let failed_sink = render(&pdf, &image_path, full());
    assert!(failed_sink.status.success(), "completed render must survive failed diagnostics: {}", failed_sink.status);
    assert!(failed_sink.stdout.is_empty());
    let image = std::fs::read(&image_path).unwrap();
    assert!(image.starts_with(b"P7\nWIDTH ") && image.windows(7).any(|part| part == b"ENDHDR\n"));
    let ordinary = render(&pdf, &image_path, Stdio::piped());
    assert!(ordinary.status.success());
    assert!(String::from_utf8_lossy(&ordinary.stderr).contains("rendered page 1 at 18 dpi"));
    assert_eq!(std::fs::read(&image_path).unwrap(), image, "diagnostic destination must not change rendered bytes");

    let invalid = run(Command::new(BIN).arg("text").arg(&pdf).args(["--page", "2"]).stdout(Stdio::piped()).stderr(full()), None);
    assert_eq!(invalid.status.code(), Some(1), "page errors remain normal failures rather than stderr panics");
    assert!(invalid.stdout.is_empty());
    let valid = run(Command::new(BIN).arg("text").arg(&pdf).stdout(Stdio::piped()).stderr(full()), None);
    assert!(valid.status.success());
    assert!(String::from_utf8_lossy(&valid.stdout).contains("Ordinary diagnostic text"));

    // An existing directory with a supported extension reaches the real file writer.
    // Using /dev/full as the render path would only test upstream's extension check.
    let folder = dir.0.join("folder.pam");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("keep"), b"sentinel").unwrap();
    assert_eq!(render(&pdf, &folder, full()).status.code(), Some(1));
    assert_eq!(std::fs::read(folder.join("keep")).unwrap(), b"sentinel");
    assert_eq!(std::fs::read(&image_path).unwrap(), image);
    assert_eq!(std::fs::read(&pdf).unwrap(), bytes.as_slice());
}

#[cfg(feature = "mcp")]
#[test]
fn failing_startup_diagnostics_preserve_mcp_replies_and_protocol_output_errors() {
    let requests = [
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 3, "method": "ordinary-unknown-method" }),
    ];
    let input = requests.iter().map(|request| format!("{request}\n")).collect::<String>();
    let output = run(Command::new(BIN).arg("mcp").stdout(Stdio::piped()).stderr(full()), Some(&input));
    assert!(output.status.success(), "failed startup diagnostics must not stop stdio: {}", output.status);
    let replies: Vec<serde_json::Value> = String::from_utf8(output.stdout).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[0]["id"], 1);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(replies[1]["id"], 2);
    assert_eq!(replies[1]["result"], serde_json::json!({}));
    assert_eq!(replies[2]["id"], 3);
    assert_eq!(replies[2]["error"]["code"], -32601);
    // Upstream's compact mode still reaches stdio even when its expanded diagnostic fails.
    let ping = "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"ping\"}\n";
    for (stderr, diagnostic_expected) in [(full(), false), (Stdio::piped(), true)] {
        let output = run(Command::new(BIN).args(["mcp", "--compact"]).stdout(Stdio::piped()).stderr(stderr), Some(ping));
        assert!(output.status.success());
        let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["id"], 4);
        assert_eq!(reply["result"], serde_json::json!({}));
        if diagnostic_expected {
            assert!(String::from_utf8_lossy(&output.stderr).contains(", compact tool list); close stdin to stop"));
        }
    }
    let output = run(Command::new(BIN).arg("mcp").stdout(full()).stderr(full()), Some("{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"ping\"}\n"));
    assert_eq!(output.status.code(), Some(1), "actual protocol output failure must still fail");
}
