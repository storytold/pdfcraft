use serde_json::Value;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

#[test]
fn cli_batch_reports_partial_failure_and_continues_into_subfolders() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-batch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("input/sub")).unwrap();
    std::fs::write(dir.join("input/0.pdf"), b"broken").unwrap();
    std::fs::write(dir.join("input/sub/page.txt"), "Batch text").unwrap();
    let output = Command::new(BIN)
        .args(["batch", "input", "--root"])
        .arg(&dir)
        .args(["--steps", "convert,optimize", "--out-dir", "out", "--recursive"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["completed"], 1, "{report}");
    assert_eq!(report["failed"], 1);
    let converted = dir.join("out/sub/page.pdf");
    assert!(converted.exists());
    let text = Command::new(BIN).arg("text").arg(converted).output().unwrap();
    assert!(text.status.success());
    assert!(String::from_utf8_lossy(&text.stdout).contains("Batch text"));
    let success = Command::new(BIN).current_dir(&dir).args(["batch", "input/sub", "--steps", "convert", "--out-dir", "other"]).output().unwrap();
    assert!(success.status.success(), "{}", String::from_utf8_lossy(&success.stderr));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cli_batch_writes_the_log_file_only_when_asked() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-batch-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("input")).unwrap();
    std::fs::write(dir.join("input/broken.pdf"), b"broken").unwrap();
    let base = ["batch", "input", "--steps", "optimize", "--out-dir", "out"];

    let plain = Command::new(BIN).current_dir(&dir).args(base).output().unwrap();
    assert!(!plain.status.success());
    assert!(!std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.path().extension().is_some_and(|x| x == "log")));

    let log = dir.join("batch.log");
    let logged = Command::new(BIN).current_dir(&dir).args(base).arg("--log").arg(&log).output().unwrap();
    assert!(!logged.status.success(), "a failed batch still exits with an error");
    let text = std::fs::read_to_string(&log).expect("log file written for a failed batch");
    assert!(text.contains("broken.pdf") && text.contains("error"), "{text}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cli_batch_rejects_bad_flags_before_creating_outputs() {
    for args in [
        vec!["batch", "--steps"],
        vec!["batch", "--recursive", "--root"],
        vec!["batch", "--steps", "convert", "--out-dir"],
        vec!["batch", "--steps", "convert", "--steps", "ocr", "--out-dir", "out"],
        vec!["batch", "--unknown"],
    ] {
        let output = Command::new(BIN).args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn cli_batch_writes_the_log_file_only_when_asked_review_regression_1() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-batch-log-regression-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("input")).unwrap();
    std::fs::write(dir.join("input/good.txt"), "Hello world").unwrap();
    std::fs::write(dir.join("input/broken.pdf"), b"broken").unwrap();

    let base = ["batch", "input", "--steps", "convert,optimize", "--out-dir", "out", "--recursive"];

    // Test 1: --log is accepted as a valid option and appears in help
    let help = Command::new(BIN).args(["batch", "--help"]).output().unwrap();
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(help_text.contains("--log"), "CLI help should mention --log option: {help_text}");

    // Test 2: Log write failure still prints batch report to stdout
    let bad_log = dir.join("unwritable_dir");
    std::fs::create_dir_all(&bad_log).unwrap();
    let logged = Command::new(BIN).current_dir(&dir).args(base).arg("--log").arg(&bad_log).output().unwrap();
    let stdout = String::from_utf8_lossy(&logged.stdout);
    assert!(stdout.contains("completed") || stdout.contains("failed"), "Batch report should be printed to stdout even when log write fails: {stdout}");

    // Test 3: Log write doesn't overwrite input files
    let log_as_input = dir.join("input/good.txt");
    let logged2 = Command::new(BIN).current_dir(&dir).args(base).arg("--log").arg(&log_as_input).output().unwrap();
    let content = std::fs::read_to_string(&log_as_input).unwrap();
    assert_eq!(content, "Hello world", "Input file should not be overwritten by log: {content}");

    // Test 4: Dry-run log entries include planned output paths
    let log_file = dir.join("dryrun.log");
    let dry_run = Command::new(BIN).current_dir(&dir).args(base).arg("--log").arg(&log_file).arg("--dry-run").output().unwrap();
    let log_text = std::fs::read_to_string(&log_file).expect("log file written for dry-run");
    assert!(log_text.contains("planned") && log_text.contains("out/good.pdf"), "Dry-run log should include planned output paths: {log_text}");

    std::fs::remove_dir_all(dir).unwrap();
}
