//! A closed output pipe is normal when a consumer such as `head` exits early (#135).

use std::io::{Read, pipe};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

fn closed_stdout(args: &[&str]) -> Output {
    let (reader, writer) = pipe().unwrap();
    drop(reader);
    Command::new(BIN).args(args).stdout(writer).stderr(Stdio::piped()).spawn().unwrap().wait_with_output().unwrap()
}

#[test]
fn output_commands_exit_cleanly_when_the_reader_has_closed() {
    let base = std::env::temp_dir().join(format!("pdfcraft-cli-stdout-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let pdf = base.join("text.pdf");
    let bytes = pdfcraft_engine::Session::new().create_from_text("Pipe test", "Hello from PdfCraft").unwrap();
    std::fs::write(&pdf, bytes.as_slice()).unwrap();
    let path = pdf.to_str().unwrap();
    let script = base.join("steps.json");
    let saved = base.join("should-not-be-written.pdf");
    let steps = serde_json::json!([
        { "tool": "doc_create", "args": { "from": "blank" } },
        { "tool": "doc_save", "args": { "doc": 1, "path": saved } },
    ]);
    std::fs::write(&script, steps.to_string()).unwrap();
    for args in [
        vec!["tools"],
        vec!["--version"],
        vec!["info", path],
        vec!["text", path],
        vec!["check-one", path],
        vec!["run", "command_list"],
        vec!["run", "--script", script.to_str().unwrap()],
        vec!["split", path, "--every", "1", "--out-dir", base.to_str().unwrap()],
    ] {
        let out = closed_stdout(&args);
        assert!(out.status.success(), "{args:?}: {}: {}", out.status, String::from_utf8_lossy(&out.stderr));
        assert!(out.stderr.is_empty(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    assert!(!saved.exists(), "a closed output pipe stops the script before the next edit");
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn tools_exits_cleanly_when_the_reader_stops_after_one_line() {
    let mut child = Command::new(BIN).arg("tools").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    // Read through the first newline without buffering the rest, then close the pipe.
    let mut byte = [0];
    while stdout.read_exact(&mut byte).is_ok() && byte[0] != b'\n' {}
    drop(stdout);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_closed_stdout_does_not_hide_command_errors() {
    let out = closed_stdout(&["info"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("info: missing file"));
}

#[cfg(target_os = "linux")]
#[test]
fn other_stdout_errors_fail_without_panicking() {
    let full = std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
    let out = Command::new(BIN).arg("--version").stdout(full).stderr(Stdio::piped()).spawn().unwrap().wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("stdout") && !stderr.contains("panicked"), "{stderr}");
}

#[test]
fn ordinary_output_keeps_its_format() {
    let out = Command::new(BIN).arg("tools").output().unwrap();
    assert!(out.status.success() && out.stderr.is_empty());
    let tools: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(tools.as_array().unwrap().iter().any(|t| t["name"] == "doc_open"));
    assert!(out.stdout.ends_with(b"\n"));
}
