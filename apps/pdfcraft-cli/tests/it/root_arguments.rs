//! A malformed `--root` must never fall back to an unconfined automation session.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");
const CANARY: &[u8] = b"KEEP EXISTING DESTINATION\n";

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let path = std::env::temp_dir().join(format!("pdfcraft-root-arguments-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("creating argument test directory: {error}"),
            }
        }
        panic!("no unused argument test directory after 128 attempts");
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn malformed_root_fails_before_a_script_can_overwrite_a_file() {
    let dir = TestDir::new();
    let destination = dir.0.join("existing.pdf");
    let script = dir.0.join("steps.json");
    let steps = serde_json::json!([
        { "tool": "doc_create", "args": { "from": "blank", "width": 100, "height": 100 } },
        { "tool": "doc_save", "args": { "doc": 1, "path": destination } },
    ]);
    std::fs::write(&script, steps.to_string()).unwrap();
    std::fs::write(&destination, CANARY).unwrap();

    for (flags, diagnostic) in [
        (vec!["--root"], "--root needs a directory value"),
        (vec!["--root", ""], "--root needs a directory value"),
        (vec!["--root", "--out", "escaped.png"], "--root needs a directory value"),
        (vec!["--out", "--root"], "--root needs a directory value"),
        (vec!["--root", ".", "--root"], "--root may only be supplied once"),
        (vec!["--root", ".", "--root", "."], "--root may only be supplied once"),
        (vec!["--root", ".", "--out", "--root"], "--root may only be supplied once"),
    ] {
        // Both a valid script and a missing one: argument validation precedes script I/O.
        for script_name in ["steps.json", "missing.json"] {
            let out = Command::new(BIN).current_dir(&dir.0).args(["run", "--script", script_name]).args(&flags).output().unwrap();
            assert_eq!(out.status.code(), Some(1), "{script_name} {flags:?} unexpectedly succeeded");
            assert_eq!(String::from_utf8(out.stderr).unwrap().trim(), format!("pdfcraft-cli: run: {diagnostic}"));
            assert!(out.stdout.is_empty(), "a refused command must not execute or report tool results");
            assert_eq!(std::fs::read(&destination).unwrap(), CANARY);
            assert!(!dir.0.join("escaped.png").exists());
        }
    }
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 2);
}

#[test]
fn malformed_root_fails_before_direct_tool_argument_validation() {
    let out = Command::new(BIN).args(["run", "doc_open", "not-key-value", "--root"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8(out.stderr).unwrap().trim(), "pdfcraft-cli: run: --root needs a directory value");
    assert!(out.stdout.is_empty());
}

#[test]
fn a_directory_name_starting_with_a_single_hyphen_remains_valid() {
    let dir = TestDir::new();
    std::fs::create_dir(dir.0.join("-documents")).unwrap();
    let out = Command::new(BIN)
        .current_dir(&dir.0)
        .args(["run", "doc_create", "from=blank", "width=100", "height=100", "--root", "-documents"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(serde_json::from_slice::<serde_json::Value>(&out.stdout).is_ok());
}

#[cfg(feature = "mcp")]
#[test]
fn malformed_root_never_starts_or_initializes_an_mcp_server() {
    use std::io::Write as _;
    use std::process::Stdio;

    let dir = TestDir::new();
    // Even if a directory matches the next option's name, it is not a root operand.
    std::fs::create_dir(dir.0.join("--compact")).unwrap();
    for (flags, diagnostic) in [
        (vec!["--root"], "--root needs a directory value"),
        (vec!["--root", ""], "--root needs a directory value"),
        (vec!["--root", "--compact"], "--root needs a directory value"),
        (vec!["--compact", "--root"], "--root needs a directory value"),
        (vec!["--root", ".", "--root"], "--root may only be supplied once"),
        (vec!["--root", ".", "--root", "./--compact"], "--root may only be supplied once"),
    ] {
        let mut child = Command::new(BIN)
            .current_dir(&dir.0)
            .arg("mcp")
            .args(&flags)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Early rejection may close stdin before this write; that is the intended behavior.
        let _ = child.stdin.take().unwrap().write_all(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{}}\n");
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{flags:?} unexpectedly started the server");
        assert_eq!(String::from_utf8(out.stderr).unwrap().trim(), format!("pdfcraft-cli: mcp: {diagnostic}"));
        assert!(out.stdout.is_empty(), "a refused server must not answer initialize");
    }
}
