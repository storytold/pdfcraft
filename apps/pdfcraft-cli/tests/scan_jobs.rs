//! `scan jobs`, `scan job` and the refusals of `scan --continue` read files and touch no scanner, so they
//! run on any machine. The scanning loop itself is tested with a scripted backend in `src/scan_cmd.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(BIN).args(args).output().expect("run cli");
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-scan-jobs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A journal as the command writes it: interrupted by a jam after two of five pages.
fn write_job(dir: &Path, state: &str, version: u32) {
    let page = |n: u32| serde_json::json!({ "number": n, "attempt": 1, "source": "feeder", "format": "bmp", "width": 4, "height": 4, "dpi": 200, "bytes": 8 });
    let text = serde_json::json!({
        "version": version,
        "id": "scan-1-a",
        "started": 1_760_000_000_000_u64,
        "updated": 1_760_000_010_000_u64,
        "request": { "device": "mock:1", "source": "feeder", "dpi": 200, "color": "gray", "pages": 5, "duplex": false, "reverse": false },
        "journal": {
            "version": 1,
            "title": "invoices",
            "device": "mock:1",
            "state": state,
            "expected_pages": 5,
            "attempts_total": 1,
            "pages": [page(1), page(2)],
            "last_error": { "info": { "kind": "jam", "phase": "between_pages", "retryable": false, "resumable": true, "message": "The feeder jammed." }, "safety": "safe", "transient": false },
            "warnings": []
        },
        "view": {},
        "attempts": [],
        "a_field_from_the_future": { "x": 1 }
    });
    std::fs::write(dir.join("scan-job.json"), text.to_string()).unwrap();
}

fn pages(dir: &Path, count: usize) {
    for n in 1..=count {
        std::fs::write(dir.join(format!("page-{n:03}.bmp")), b"x").unwrap();
    }
}

#[test]
fn jobs_lists_one_line_per_journal_and_json_for_machines() {
    let root = folder("list");
    let a = root.join("a");
    std::fs::create_dir_all(&a).unwrap();
    write_job(&a, "interrupted", 1);
    pages(&a, 2);
    let (code, stdout, stderr) = run(&["scan", "jobs", root.to_str().unwrap(), a.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.contains("scan-1-a  interrupted  2/5  1 attempt(s)  jam  2025-10-09T08:53:30Z"), "{stdout}");
    let (code, stdout, _) = run(&["scan", "jobs", a.to_str().unwrap(), "--json"]);
    assert_eq!(code, Some(0));
    let rows: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(rows[0]["state"], "interrupted");
    assert_eq!(rows[0]["exit_code"], 10);
    assert_eq!(rows[0]["page_files"], 2);
    assert!(rows[0]["page_mismatch"].is_null());
    assert!(rows[0].get("journal").is_some() && rows[0].get("request").is_some());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn jobs_says_when_the_page_files_do_not_match_the_journal() {
    let dir = folder("mismatch");
    write_job(&dir, "interrupted", 1);
    pages(&dir, 1);
    let (_, stdout, _) = run(&["scan", "jobs", dir.to_str().unwrap()]);
    assert!(stdout.contains("[1 page file(s) on disk, journal keeps 2]"), "{stdout}");
    let (_, stdout, _) = run(&["scan", "job", dir.to_str().unwrap()]);
    assert!(stdout.contains("WARNING") && stdout.contains("does not verify files"), "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn job_shows_the_detail_and_the_next_step() {
    let dir = folder("detail");
    write_job(&dir, "interrupted", 1);
    pages(&dir, 2);
    let (code, stdout, stderr) = run(&["scan", "job", dir.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.starts_with("invoices: interrupted, 2 of 5 pages, 1 attempt, jam"), "{stdout}");
    assert!(stdout.contains("exit class: needs_attention (exit status 10)"), "{stdout}");
    assert!(stdout.contains("--append|--restart"), "{stdout}");
    let (_, stdout, _) = run(&["scan", "job", dir.to_str().unwrap(), "--json"]);
    let job: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(job["journal"]["expected_pages"], 5);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_newer_journal_version_is_refused_with_a_clear_message() {
    let dir = folder("newer");
    write_job(&dir, "interrupted", 99);
    let (code, _, stderr) = run(&["scan", "job", dir.to_str().unwrap()]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("version 99") && stderr.contains("upgrade"), "{stderr}");
    let (code, _, stderr) = run(&["scan", "--continue", dir.to_str().unwrap(), "--append", "--yes"]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("upgrade"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn continue_refuses_before_any_scanner_is_touched() {
    let dir = folder("continue");
    write_job(&dir, "interrupted", 1);
    pages(&dir, 2);
    let d = dir.to_str().unwrap();
    let (code, stdout, stderr) = run(&["scan", "--continue", d, "--yes"]);
    assert_eq!(code, Some(1));
    assert!(stdout.is_empty() && stderr.contains("--append") && stderr.contains("--restart"), "{stderr}");
    let (_, _, stderr) = run(&["scan", "--continue", d, "--append"]);
    assert!(stderr.contains("--yes is required"), "{stderr}");
    let (_, _, stderr) = run(&["scan", "--continue", d, "--append", "--restart", "--yes"]);
    assert!(stderr.contains("not both"), "{stderr}");
    let (_, _, stderr) = run(&["scan", "--continue", d, "--append", "--yes", "--dpi", "300"]);
    assert!(stderr.contains("settings come from the job file"), "{stderr}");
    // The files are untouched by every refusal.
    assert!(dir.join("page-001.bmp").is_file() && dir.join("page-002.bmp").is_file());

    write_job(&dir, "needs_attention", 1);
    let (code, _, stderr) = run(&["scan", "--continue", d, "--append", "--yes"]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("--i-checked-the-scanner") && stderr.contains("carriage"), "{stderr}");

    write_job(&dir, "interrupted", 1);
    pages(&dir, 3);
    let (_, _, stderr) = run(&["scan", "--continue", d, "--append", "--yes"]);
    assert!(stderr.contains("3 page file(s)"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_fresh_scan_will_not_start_in_a_folder_that_already_has_a_job() {
    let dir = folder("occupied");
    write_job(&dir, "interrupted", 1);
    let (code, stdout, stderr) = run(&["scan", "--out", dir.to_str().unwrap(), "--yes"]);
    // On a platform without a scanner backend the unsupported sentence comes first; either way nothing is scanned.
    assert_eq!(code, Some(1));
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("already holds a scan job") || stderr.contains("only supported on Windows"), "{stderr}");
    assert!(dir.join("scan-job.json").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_usage_lists_the_job_commands_and_exit_codes() {
    let (code, stdout, _) = run(&["--help"]);
    assert_eq!(code, Some(0));
    for needle in [
        "scan   jobs [DIR...] [--json]",
        "scan   job DIR [--json]",
        "scan   --continue DIR",
        "--i-checked-the-scanner",
        "0 completed, 10 needs attention",
        "11 failed, 12 cancelled",
    ] {
        assert!(stdout.contains(needle), "{needle}\n{stdout}");
    }
}
