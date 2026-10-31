//! The command line as a definition: what clap accepts and refuses, and the exit status of each,
//! with no file opened. (Results of the commands are in `tests/it`.)

use crate::cli::EditStep;
use crate::{command, parse, run};

fn go(args: &[&str]) -> (u8, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut argv = vec!["pdfcraft-cli"];
    argv.extend_from_slice(args);
    let status = run(argv, &mut out, &mut err);
    (status, String::from_utf8_lossy(&out).into_owned(), String::from_utf8_lossy(&err).into_owned())
}

/// A usage error: exit 2, nothing on stdout, the complaint on stderr.
fn usage(args: &[&str]) -> String {
    let (status, out, err) = go(args);
    assert_eq!(status, 2, "{args:?}: {err}");
    assert!(out.is_empty(), "{args:?}: {out}");
    err
}

fn accepted(args: &[&str]) {
    let mut argv = vec!["pdfcraft-cli"];
    argv.extend_from_slice(args);
    if let Err(e) = parse(argv) {
        panic!("{args:?}: {e}");
    }
}

#[test]
fn the_definition_is_consistent() {
    command().debug_assert();
}

#[test]
fn a_typo_in_a_flag_is_a_usage_error_for_every_command() {
    for args in [
        &["info", "a.pdf", "--passwrod", "x"][..],
        &["text", "a.pdf", "--pgae", "1"],
        &["combine", "a.pdf", "b.pdf", "--ot", "o.pdf"],
        &["extract", "a.pdf", "--pgaes", "1", "--out", "o.pdf"],
        &["split", "a.pdf", "--ever", "2"],
        &["render", "a.pdf", "--dppi", "96", "--out", "o.png"],
        &["edit", "a.pdf", "--rotat", "1:90", "--out", "o.pdf"],
        &["check", "a.pdf", "--timout", "2"],
        &["check-one", "a.pdf", "--edti"],
        &["audit", "a.pdf", "--nonsense"],
        &["batch", "a", "--steps", "ocr", "--out-dir", "d", "--langauge", "en"],
        &["run", "doc_open", "--roto", "/tmp"],
        &["ui", "--contro", "c.json", "state"],
        &["tools", "--all"],
    ] {
        let err = usage(args);
        assert!(err.contains("unexpected argument"), "{args:?}: {err}");
    }
}

#[test]
fn a_repeated_flag_is_a_usage_error() {
    for args in [
        &["info", "a.pdf", "--password", "a", "--password", "b"][..],
        &["render", "a.pdf", "--page", "1", "--page", "2", "--out", "o.png"],
        &["render", "a.pdf", "--out", "a.png", "--out", "b.png"],
        &["extract", "a.pdf", "--pages", "1", "--pages", "2", "--out", "o.pdf"],
        &["split", "a.pdf", "--every", "2", "--every", "3"],
        &["check", "a", "--dpi", "36", "--dpi", "72"],
        &["check", "a", "--timeout", "5", "--timeout", "6"],
        &["run", "--script", "s.json", "--root", "a", "--root", "b"],
        &["edit", "a.pdf", "--out", "a.pdf", "--out", "b.pdf"],
        &["edit", "a.pdf", "--out", "o.pdf", "--full", "--full"],
        &["batch", "a", "--steps", "ocr", "--out-dir", "x", "--out-dir", "y"],
    ] {
        let err = usage(args);
        assert!(err.contains("cannot be used multiple times"), "{args:?}: {err}");
    }
    // Only the ordered edit options repeat.
    accepted(&["edit", "a.pdf", "--out", "o.pdf", "--rotate", "1:90", "--rotate", "2:180", "--title", "a", "--title", "b"]);
}

#[test]
fn flags_that_cannot_go_together_are_usage_errors() {
    for args in [
        &["split", "a.pdf"][..],
        &["split", "a.pdf", "--every", "2", "--before", "3"],
        &["run"],
        &["run", "doc_open", "--script", "s.json"],
        &["run", "--script", "s.json", "--out", "o.png"],
        &["run", "--report", "doc_open"],
        &["run", "--script", "s.json", "doc_open"],
        &["combine", "only.pdf", "--out", "o.pdf"],
        &["combine", "a.pdf", "b.pdf"],
        &["extract", "a.pdf", "--out", "o.pdf"],
        &["render", "a.pdf"],
        &["edit", "a.pdf"],
        &["ui", "state"],
        &["batch", "a", "--out-dir", "d"],
        // An option of a step that --steps leaves out.
        &["batch", "a", "--steps", "convert", "--out-dir", "d", "--quality", "50"],
        &["batch", "a", "--steps", "optimize", "--out-dir", "d", "--language", "en"],
        &["batch", "a", "--steps", "optimize", "--out-dir", "d", "--ocr-all-pages"],
    ] {
        usage(args);
    }
    accepted(&["batch", "a", "--steps", "optimize,ocr", "--out-dir", "d", "--quality", "50", "--language", "en"]);
}

#[test]
fn values_are_checked_by_type_and_range_before_any_work() {
    for args in [
        &["render", "a.pdf", "--out", "o.png", "--page", "0"][..],
        &["render", "a.pdf", "--out", "o.png", "--page", "x"],
        &["render", "a.pdf", "--out", "o.png", "--page", "-1"],
        &["render", "a.pdf", "--out", "o.png", "--dpi", "0"],
        &["render", "a.pdf", "--out", "o.png", "--dpi", "-5"],
        &["render", "a.pdf", "--out", "o.png", "--dpi", "nan"],
        &["render", "a.pdf", "--out", "o.png", "--dpi", "inf"],
        &["render", "a.pdf", "--out", "o.gif"],
        &["render", "a.pdf", "--out", "o"],
        &["text", "a.pdf", "--page", "0"],
        &["check", "a", "--timeout", "0"],
        &["check", "a", "--dpi", "0"],
        &["split", "a.pdf", "--every", "0"],
        &["split", "a.pdf", "--before", "0"],
        &["extract", "a.pdf", "--out", "o.pdf", "--pages", "3-1"],
        &["extract", "a.pdf", "--out", "o.pdf", "--pages", "1-2000000"],
        &["extract", "a.pdf", "--out", "o.pdf", "--pages", "1,,2"],
        &["edit", "a.pdf", "--out", "o.pdf", "--rotate", "1:45"],
        &["edit", "a.pdf", "--out", "o.pdf", "--rotate", "1"],
        &["edit", "a.pdf", "--out", "o.pdf", "--move", "1:0"],
        &["edit", "a.pdf", "--out", "o.pdf", "--insert-blank", "0"],
        &["batch", "a", "--steps", "zip", "--out-dir", "d"],
        &["batch", "a", "--steps", "optimize", "--out-dir", "d", "--quality", "101"],
        &["batch", "a", "--steps", "optimize", "--out-dir", "d", "--quality", "-1"],
        &["batch", "a", "--steps", "optimize", "--out-dir", "d", "--ppi", "8"],
        &["batch", "a", "--steps", "ocr", "--out-dir", "d", "--dpi", "700"],
        &["run", "doc_open", "path"],
        &["render", "a.pdf", "--out", ""],
    ] {
        let err = usage(args);
        assert!(err.contains("invalid value") || err.contains("unexpected argument") || err.contains("a value is required"), "{args:?}: {err}");
    }
    // The ends of the ranges are fine; so are fractions where a number is a measure.
    accepted(&["batch", "a", "--steps", "optimize", "--out-dir", "d", "--quality", "100", "--ppi", "9"]);
    accepted(&["render", "a.pdf", "--out", "o.JPG", "--dpi", "0.5"]);
    accepted(&["extract", "a.pdf", "--out", "o.pdf", "--pages", "1,3,5-8"]);
}

#[test]
fn a_flag_never_takes_the_next_flag_as_its_value() {
    for args in [
        &["extract", "a.pdf", "--pages", "1", "--out", "--password"][..],
        &["combine", "a.pdf", "b.pdf", "--out", "--nonsense"],
        &["split", "a.pdf", "--every", "2", "--out-dir", "--password"],
        &["check", "a", "--json", "--dpi"],
        &["ui", "--control", "--out", "state"],
        &["edit", "a.pdf", "--out", "--full"],
        // A password is a string like any other value: it may not start with `-` either.
        &["info", "a.pdf", "--password", "--page"],
    ] {
        let err = usage(args);
        assert!(err.contains("a value is required") || err.contains("unexpected argument"), "{args:?}: {err}");
    }
}

#[test]
fn a_value_that_starts_with_a_dash_is_written_with_an_equals_sign_or_a_dot_slash() {
    accepted(&["extract", "a.pdf", "--pages", "1", "--out=--dash.pdf"]);
    accepted(&["extract", "a.pdf", "--pages", "1", "--out", "./--dash.pdf"]);
    accepted(&["info", "a.pdf", "--password=--page"]);
    accepted(&["info", "./--dash.pdf"]);
    accepted(&["mcp", "--automation-read-root=-folder"]);
}

#[test]
fn after_a_double_dash_everything_is_a_file() {
    let parsed = parse(["pdfcraft-cli", "combine", "--out", "o.pdf", "--", "--a.pdf", "-b.pdf"]);
    let parsed = parsed.expect("parses");
    let crate::Cmd::Combine(args) = parsed.cli.command else { panic!("combine") };
    assert_eq!(args.inputs, [std::path::PathBuf::from("--a.pdf"), "-b.pdf".into()]);
    accepted(&["info", "--", "-x.pdf"]);
}

#[test]
fn version_is_a_flag_and_not_a_command() {
    for flag in ["--version", "-V"] {
        let (status, out, err) = go(&[flag]);
        assert_eq!(status, 0, "{err}");
        assert!(out.starts_with("pdfcraft-cli "), "{out}");
        assert!(err.is_empty());
    }
    let err = usage(&["version"]);
    assert!(err.contains("unrecognized subcommand"), "{err}");
}

#[test]
fn help_goes_to_stdout_with_status_zero_and_a_missing_command_is_a_usage_error() {
    for args in [&["--help"][..], &["-h"], &["help"], &["help", "render"], &["render", "--help"], &["check", "-h"], &["edit", "--help"]] {
        let (status, out, err) = go(args);
        assert_eq!(status, 0, "{args:?}: {err}");
        assert!(out.contains("Usage: pdfcraft-cli"), "{args:?}: {out}");
        assert!(err.is_empty(), "{args:?}: {err}");
    }
    let (_, out, _) = go(&["--help"]);
    for command in ["info", "render", "text", "edit", "combine", "extract", "split", "check", "audit", "batch", "tools", "run", "ui"] {
        assert!(out.contains(&format!("\n  {command} ")), "{command}: {out}");
    }
    assert!(!out.contains("check-one"), "an internal command is not listed: {out}");
    let err = usage(&[]);
    assert!(err.contains("Usage: pdfcraft-cli"), "{err}");
    let err = usage(&["frobnicate"]);
    assert!(err.contains("unrecognized subcommand 'frobnicate'"), "{err}");
}

#[test]
fn edit_options_run_in_the_order_they_were_given() {
    let parsed = parse([
        "pdfcraft-cli",
        "edit",
        "in.pdf",
        "--out",
        "o.pdf",
        "--delete",
        "2",
        "--rotate",
        "1,3:-90",
        "--title",
        "T",
        "--move",
        "5:1",
        "--insert-blank",
        "2",
        "--rotate",
        "4:180",
        "--author",
        "A",
    ])
    .expect("parses");
    let crate::Cmd::Edit(args) = &parsed.cli.command else { panic!("edit") };
    let sub = parsed.matches.subcommand_matches("edit").expect("edit matches");
    let steps = args.steps(sub);
    let kinds: Vec<&str> = steps
        .iter()
        .map(|s| match s {
            EditStep::Delete(_) => "delete",
            EditStep::Rotate(_) => "rotate",
            EditStep::Title(_) => "title",
            EditStep::Move(_) => "move",
            EditStep::InsertBlank(_) => "blank",
            EditStep::Author(_) => "author",
        })
        .collect();
    assert_eq!(kinds, ["delete", "rotate", "title", "move", "blank", "rotate", "author"]);
    assert_eq!(steps[1], EditStep::Rotate(crate::cli::Rotate { pages: vec![0, 2], degrees: -90 }));
    assert_eq!(steps[4], EditStep::InsertBlank(1));
}

#[test]
fn run_takes_key_values_as_json_when_they_parse() {
    let parsed = parse(["pdfcraft-cli", "run", "page_rotate", "pages=[1,3]", "degrees=-90", "name=a b"]).expect("parses");
    let crate::Cmd::Run(args) = parsed.cli.command else { panic!("run") };
    assert_eq!(args.tool.as_deref(), Some("page_rotate"));
    let values: Vec<_> = args.args.iter().map(|kv| (kv.key.as_str(), kv.value.clone())).collect();
    assert_eq!(values, [("pages", serde_json::json!([1, 3])), ("degrees", serde_json::json!(-90)), ("name", serde_json::json!("a b"))]);
}

#[cfg(unix)]
#[test]
fn an_argument_that_is_not_utf8_is_a_usage_error() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let status = run([OsString::from("pdfcraft-cli"), OsString::from("run"), OsString::from_vec(b"caf\xe9".to_vec())], &mut out, &mut err);
    assert_eq!(status, 2, "{}", String::from_utf8_lossy(&err));
    assert!(out.is_empty());
}

#[cfg(unix)]
#[test]
fn a_file_name_that_is_not_utf8_is_a_path_and_not_a_panic() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let bad = OsString::from_vec(b"/nonexistent/caf\xe9.pdf".to_vec());
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let status = run([OsString::from("pdfcraft-cli"), OsString::from("info"), bad], &mut out, &mut err);
    assert_eq!(status, 1, "a runtime failure, not a usage error: {}", String::from_utf8_lossy(&err));
    assert!(String::from_utf8_lossy(&err).starts_with("error: "));
}

#[cfg(not(feature = "mcp"))]
#[test]
fn mcp_without_the_feature_says_it_was_left_out() {
    let (status, _, err) = go(&["mcp", "--root", "x"]);
    assert_eq!(status, 1);
    assert!(err.contains("without the `mcp` feature"), "{err}");
}

/// A writer that fails the way a full disk or a closed pipe does.
struct Failing(std::io::ErrorKind);

impl std::io::Write for Failing {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(self.0.into())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(self.0.into())
    }
}

/// Codex review: help and `--version` are written by `run` itself, and a failed write there was
/// dropped, so `pdfcraft-cli --version > /dev/full` exited 0.
#[test]
fn a_failed_write_of_help_or_the_version_is_an_error_but_a_closed_pipe_is_not() {
    for args in [&["--version"][..], &["--help"], &["render", "--help"]] {
        let mut err = Vec::new();
        let status = run(std::iter::once("pdfcraft-cli").chain(args.iter().copied()), &mut Failing(std::io::ErrorKind::StorageFull), &mut err);
        assert_eq!(status, 1, "{args:?}");
        assert!(String::from_utf8_lossy(&err).starts_with("error: stdout: "), "{args:?}: {}", String::from_utf8_lossy(&err));
        let mut err = Vec::new();
        let status = run(std::iter::once("pdfcraft-cli").chain(args.iter().copied()), &mut Failing(std::io::ErrorKind::BrokenPipe), &mut err);
        assert_eq!(status, 0, "{args:?}");
        assert!(err.is_empty());
    }
}

/// Codex review: a batch path that is not UTF-8 was turned into a different name by a lossy
/// conversion before it reached the tool.
#[cfg(unix)]
#[test]
fn a_path_the_tools_cannot_take_is_refused_and_not_renamed() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let bad = || OsString::from_vec(b"caf\xe9".to_vec());
    let cases: Vec<Vec<OsString>> = vec![
        vec!["batch".into(), bad(), "--steps".into(), "ocr".into(), "--out-dir".into(), "d".into()],
        vec!["batch".into(), "a".into(), "--steps".into(), "ocr".into(), "--out-dir".into(), bad()],
        vec!["run".into(), "page_render".into(), "--out".into(), bad()],
    ];
    for case in cases {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let argv: Vec<OsString> = std::iter::once("pdfcraft-cli".into()).chain(case.clone()).collect();
        let status = run(argv, &mut out, &mut err);
        assert_eq!(status, 2, "{case:?}: {}", String::from_utf8_lossy(&err));
        assert!(String::from_utf8_lossy(&err).contains("UTF-8"), "{}", String::from_utf8_lossy(&err));
    }
}
