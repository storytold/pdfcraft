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
fn help_flags_print_generated_help_to_stdout() {
    for args in [&["--help"][..], &["-h"], &["help"]] {
        let out = run(args);
        assert!(out.status.success(), "{args:?} exited {:?}", out.status);
        let stdout = text(&out.stdout);
        assert!(stdout.contains("Usage: pdfcraft-cli <COMMAND>"), "{args:?}: {stdout}");
        for command in ["info", "render", "text", "edit", "combine", "extract", "split", "check", "audit", "batch", "tools", "run", "ui"] {
            assert!(stdout.contains(&format!("\n  {command} ")), "{args:?} {command}: {stdout}");
        }
        assert!(stdout.contains("Exit status: 0 success, 1 the command failed, 2 usage error."), "{args:?}: {stdout}");
        assert!(out.stderr.is_empty(), "{args:?}: {}", text(&out.stderr));
    }
}

#[test]
fn each_command_has_its_own_help() {
    for (args, expected) in [
        (&["render", "--help"][..], "--dpi <DPI>"),
        (&["help", "render"], "--dpi <DPI>"),
        (&["check", "-h"], "--timeout <SECS>"),
        (&["split", "--help"], "--every <N>"),
        (&["edit", "--help"], "--insert-blank <N>"),
        (&["run", "--help"], "--script <FILE>"),
    ] {
        let out = run(args);
        assert!(out.status.success(), "{args:?} exited {:?}", out.status);
        let stdout = text(&out.stdout);
        assert!(stdout.contains(expected), "{args:?}: {stdout}");
        assert!(out.stderr.is_empty(), "{args:?}: {}", text(&out.stderr));
    }
}

#[test]
fn version_prints_the_version_and_links() {
    // `--version` is the long form, with the links; `-V` the short one, as clap prints them.
    let out = run(&["--version"]);
    assert!(out.status.success());
    let stdout = text(&out.stdout);
    assert!(stdout.starts_with("pdfcraft-cli "), "{stdout}");
    assert!(stdout.contains("Discord:") && stdout.contains("Source:"), "{stdout}");
    let out = run(&["-V"]);
    assert!(out.status.success());
    assert_eq!(text(&out.stdout).lines().count(), 1);
    // Only a flag: there is no `version` command.
    let out = run(&["version"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("unrecognized subcommand 'version'"));
}

#[test]
fn no_command_fails_with_usage_on_stderr() {
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(text(&out.stderr).contains("Usage: pdfcraft-cli <COMMAND>"));
}

/// `mcp` is listed only when the build has it (`--no-default-features` leaves it out).
#[test]
fn usage_lists_mcp_only_when_built_with_it() {
    let out = run(&["--help"]);
    let stdout = text(&out.stdout);
    assert_eq!(stdout.contains("\n  mcp "), cfg!(feature = "mcp"), "{stdout}");
    // The other commands are still there.
    assert!(stdout.contains("\n  run "), "{stdout}");
    assert!(stdout.contains("\n  ui "), "{stdout}");
    #[cfg(feature = "mcp")]
    {
        let out = run(&["mcp", "--help"]);
        let stdout = text(&out.stdout);
        assert!(stdout.contains("--compact") && stdout.contains("--automation-read-root <DIR>"), "{stdout}");
    }
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
    assert_eq!(out.status.code(), Some(2));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("unrecognized subcommand 'frobnicate'"), "{stderr}");
    assert!(stderr.contains("--help"), "{stderr}");
}
