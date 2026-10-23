//! `scan check` guards that answer before any scanner is listed or opened, so they are safe to run
//! on any machine. The device paths are covered with a fake backend in `pdfcraft-automation`.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// Run the CLI with `args`: whether it succeeded, its stdout and its stderr.
fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(BIN).args(args).output().expect("run cli");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn acquire_without_a_device_is_refused_and_prints_nothing() {
    let (ok, stdout, stderr) = run(&["scan", "check", "--acquire"]);
    assert!(!ok);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("--acquire needs --device"), "{stderr}");
}

#[test]
fn out_without_acquire_is_refused() {
    let (ok, _, stderr) = run(&["scan", "check", "--out", "pages"]);
    assert!(!ok);
    assert!(stderr.contains("--out only applies together with --acquire"), "{stderr}");
}

#[test]
fn unknown_and_misspelled_options_are_refused() {
    for args in [["scan", "check", "--aquire"], ["scan", "check", "--yes"], ["scan", "check", "--dump"]] {
        let (ok, stdout, stderr) = run(&args);
        assert!(!ok, "{args:?}");
        assert!(stdout.is_empty(), "{stdout}");
        assert!(stderr.contains("unknown option") || stderr.contains("scan check takes only"), "{args:?}: {stderr}");
    }
    let (ok, _, stderr) = run(&["scan", "check", "--device"]);
    assert!(!ok);
    assert!(stderr.contains("--device needs a value"), "{stderr}");
}

#[test]
fn the_usage_lists_scan_check() {
    let (ok, stdout, _) = run(&["--help"]);
    assert!(ok);
    assert!(stdout.contains("pdfcraft-cli scan   check"), "{stdout}");
    assert!(stdout.contains("--acquire"), "{stdout}");
}

#[cfg(not(windows))]
#[test]
fn off_windows_the_check_says_scanning_needs_windows() {
    for args in [vec!["scan", "check"], vec!["scan", "check", "--details", "--json"], vec!["scan", "check", "--device", "x", "--acquire"]] {
        let (ok, stdout, stderr) = run(&args);
        assert!(!ok, "{args:?}");
        assert!(stdout.is_empty(), "{stdout}");
        assert!(stderr.contains("scanning is only supported on Windows (WIA)"), "{args:?}: {stderr}");
    }
}
