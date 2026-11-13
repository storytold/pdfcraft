//! The exit codes the CLI has today: `ExitCode::SUCCESS` (0) or `ExitCode::FAILURE` (1).
//!
//! Pinned here so that the exit-code contract is a visible, reviewed change when it comes: a
//! failed file, an outside-root path and a usage error all exit 1 today.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// A fresh folder for one test, so the tests can run in parallel and leave nothing behind.
fn workdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-exit-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `batch` with `args`, run from the working directory `dir`.
fn batch(dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN).current_dir(dir).arg("batch").args(args).output().unwrap()
}

/// `run --script --report` from the working directory `dir`: one JSON document for the run.
fn run_report(dir: &Path, script: &Path) -> Output {
    Command::new(BIN).current_dir(dir).args(["run", "--script"]).arg(script).arg("--report").output().unwrap()
}

/// The one JSON document a command prints on stdout.
fn reported(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn batch_exits_0_when_every_file_succeeds() {
    let dir = workdir("batch-ok");
    std::fs::write(dir.join("page.txt"), "Exit code page").unwrap();
    let output = batch(&dir, &["page.txt", "--steps", "convert", "--out-dir", "out"]);
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    let report = reported(&output);
    assert_eq!(report["completed"], 1, "{report}");
    assert_eq!(report["failed"], 0, "{report}");
    assert!(dir.join("out/page.pdf").exists(), "the converted page is written");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn batch_exits_1_when_one_file_fails() {
    let dir = workdir("batch-fail");
    std::fs::write(dir.join("bad.pdf"), "broken").unwrap();
    let output = batch(&dir, &["bad.pdf", "--steps", "convert", "--out-dir", "out"]);
    assert_eq!(output.status.code(), Some(1), "{}", String::from_utf8_lossy(&output.stderr));
    let report = reported(&output);
    assert_eq!(report["failed"], 1, "{report}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn batch_usage_error_exits_1_and_creates_nothing() {
    let dir = workdir("batch-usage");
    std::fs::write(dir.join("page.txt"), "Exit code page").unwrap();
    // Baseline: a usage error exits 1 today. A later change to the exit-code contract will
    // update this assertion on purpose.
    let output = batch(&dir, &["page.txt", "--out-dir", "out"]);
    assert_eq!(output.status.code(), Some(1), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stdout.is_empty(), "a usage error prints to stderr, not a report on stdout");
    assert!(!output.stderr.is_empty(), "a usage error says what is wrong");
    assert!(!dir.join("out").exists(), "a refused batch creates no output");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn batch_outside_root_exits_1_and_creates_nothing() {
    let dir = workdir("batch-outside-root");
    let output = Command::new(BIN)
        .current_dir(&dir)
        .args(["batch", "../outside.pdf", "--steps", "convert", "--out-dir", "out", "--root"])
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", String::from_utf8_lossy(&output.stderr));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("is outside the allowed directory"), "the input is refused by the root: {stderr}");
    assert!(output.stdout.is_empty(), "a refused batch prints no report");
    assert!(!dir.join("out").exists(), "a refused batch creates no output");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn run_script_report_exits_0_when_ok_and_1_when_a_step_fails() {
    let dir = workdir("run-report");
    let script = dir.join("steps.json");
    let blank = serde_json::json!({ "tool": "doc_create", "args": { "from": "blank", "width": 100, "height": 100 } });
    let steps = serde_json::json!([blank.clone(), { "tool": "no_such_tool", "args": {} }]);
    std::fs::write(&script, steps.to_string()).unwrap();

    let failing = run_report(&dir, &script);
    assert_eq!(failing.status.code(), Some(1), "{}", String::from_utf8_lossy(&failing.stderr));
    let report = reported(&failing);
    assert_eq!(report["ok"], false, "{report}");
    assert_eq!(report["steps"][1]["status"], "error", "{report}");

    std::fs::write(&script, serde_json::json!([blank]).to_string()).unwrap();
    let good = run_report(&dir, &script);
    assert_eq!(good.status.code(), Some(0), "{}", String::from_utf8_lossy(&good.stderr));
    let report = reported(&good);
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(report["steps"][0]["status"], "ok", "{report}");
    std::fs::remove_dir_all(&dir).unwrap();
}
