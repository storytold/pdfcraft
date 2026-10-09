//! `--help` prints the full usage to stdout and succeeds; a missing or unknown command fails with
//! the usage (or a pointer to `--help`) on stderr.

use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

fn run(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn help_flags_print_full_usage_to_stdout() {
    for args in [&["--help"][..], &["-h"], &["help"], &["render", "--help"], &["check", "-h"]] {
        let out = run(args);
        assert!(out.status.success(), "{args:?} exited {:?}", out.status);
        let stdout = text(&out.stdout);
        // Per-command options, not just the command list.
        assert!(stdout.contains("pdfcraft-cli render <file.pdf> --page N"), "{args:?}: {stdout}");
        assert!(stdout.contains("--every N"), "{args:?}: {stdout}");
        assert!(out.stderr.is_empty(), "{args:?}: {}", text(&out.stderr));
    }
}

#[test]
fn no_command_fails_with_usage_on_stderr() {
    let out = run(&[]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(text(&out.stderr).contains("pdfcraft-cli render <file.pdf> --page N"));
}

#[test]
fn unknown_command_names_it_and_points_to_help() {
    let out = run(&["frobnicate"]);
    assert!(!out.status.success());
    let stderr = text(&out.stderr);
    assert!(stderr.contains("unknown command 'frobnicate'"), "{stderr}");
    assert!(stderr.contains("pdfcraft-cli --help"), "{stderr}");
}
