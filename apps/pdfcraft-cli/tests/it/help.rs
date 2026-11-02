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

/// `mcp` is listed only when the build has it (`--no-default-features` leaves it out).
#[test]
fn usage_lists_mcp_only_when_built_with_it() {
    let out = run(&["--help"]);
    let stdout = text(&out.stdout);
    assert_eq!(stdout.contains("pdfcraft-cli mcp"), cfg!(feature = "mcp"), "{stdout}");
    assert_eq!(stdout.contains("--compact"), cfg!(feature = "mcp"), "{stdout}");
    // The lines around it are still there.
    assert!(stdout.contains("pdfcraft-cli run    --script steps.json"), "{stdout}");
    assert!(stdout.contains("pdfcraft-cli ui     --control FILE"), "{stdout}");
}

#[cfg(not(feature = "mcp"))]
#[test]
fn mcp_without_the_feature_says_it_was_left_out() {
    let out = run(&["mcp"]);
    assert!(!out.status.success());
    let stderr = text(&out.stderr);
    assert!(stderr.contains("without the `mcp` feature"), "{stderr}");
}

#[test]
fn unknown_command_names_it_and_points_to_help() {
    let out = run(&["frobnicate"]);
    assert!(!out.status.success());
    let stderr = text(&out.stderr);
    assert!(stderr.contains("unknown command 'frobnicate'"), "{stderr}");
    assert!(stderr.contains("pdfcraft-cli --help"), "{stderr}");
}
