//! Every command rejects misspelled long options before opening or writing a file.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

#[test]
fn non_edit_commands_reject_unknown_options_before_io() {
    for args in [
        vec!["info", "missing.pdf", "--passwrod", "secret"],
        vec!["text", "missing.pdf", "--pgae", "1"],
        vec!["combine", "a.pdf", "b.pdf", "--ot", "out.pdf"],
        vec!["extract", "missing.pdf", "--pgaes", "1", "--out", "out.pdf"],
        vec!["split", "missing.pdf", "--ever", "2"],
        vec!["render", "missing.pdf", "--dppi", "96", "--out", "out.png"],
        vec!["check-one", "missing.pdf", "--edti"],
        vec!["check", "missing.pdf", "--timout", "2"],
        vec!["audit", "missing.pdf", "--passwrod", "x"],
        vec!["audit", "missing.pdf", "--nonsense", "1"],
        vec!["run", "doc_open", "--roto", "/tmp"],
        vec!["ui", "--contro", "control.json", "state"],
    ] {
        let unknown = args.iter().find(|arg| arg.starts_with("--")).expect("test option");
        let out = Command::new(BIN).args(&args).output().expect("run cli");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
        assert!(stderr.contains("unknown option") && stderr.contains(unknown), "{args:?}: {stderr}");
    }
}

/// photocraft#6944: an option took the next token whatever it was, so a line with a value missing
/// in the middle of it did something else entirely — `extract in.pdf --pages 1 --out --password`
/// wrote the PDF to a file called `--password` and exited 0. The unknown option hidden in a value
/// position passed the check above too, since the walk skipped it along with the option.
///
/// Only the options whose value is a file or folder refuse one: a password or a title can be any
/// string, `--page` included, which `text`'s own tests pin and this does not change.
#[test]
fn a_file_option_refuses_a_value_that_is_another_option() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-values-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    for (option, args) in [
        ("--out", vec!["extract", "missing.pdf", "--pages", "1", "--out", "--password"]),
        ("--out", vec!["combine", "a.pdf", "b.pdf", "--out", "--nonsense"]),
        ("--out", vec!["render", "missing.pdf", "--out", "--dpi", "96"]),
        ("--out-dir", vec!["split", "missing.pdf", "--every", "2", "--out-dir", "--password"]),
        ("--json", vec!["check", "missing.pdf", "--json", "--dpi"]),
        ("--control", vec!["ui", "--control", "--out", "state"]),
        // `edit` parses its own options, in the order given, and had the same gap.
        ("--out", vec!["edit", "missing.pdf", "--out", "--full"]),
    ] {
        let out = Command::new(BIN).current_dir(&dir).args(&args).output().expect("run cli");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
        assert!(stderr.contains(&format!("{option} needs a value")), "{args:?}: {stderr}");
        assert!(stderr.contains("another option"), "{args:?}: {stderr}");
    }
    assert_eq!(std::fs::read_dir(&dir).expect("read dir").count(), 0, "nothing was written");
    let _ = std::fs::remove_dir_all(&dir);
}

/// And a file whose name really does start with `-` is still reachable, the way the shell reaches
/// one and `--root` already documented: `./--name`. The error is then about the input file, which
/// is as far as these lines get here.
#[test]
fn a_file_whose_name_starts_with_dashes_is_still_reachable() {
    // The error is about the file the command could not open, so the test asks which file it
    // names -- not how the OS words "not found", which differs on Windows (#3849 review).
    for (names, args) in [
        ("missing.pdf", vec!["extract", "missing.pdf", "--pages", "1", "--out", "./--dash.pdf"]),
        ("missing.pdf", vec!["edit", "missing.pdf", "--out", "./--dash.pdf"]),
        ("./--dash.pdf", vec!["info", "./--dash.pdf"]),
    ] {
        let out = Command::new(BIN).args(&args).output().expect("run cli");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?}: no such file either way");
        assert!(stderr.contains(names), "{args:?}: the error is about {names}: {stderr}");
        // Not a usage error: the `./--dash.pdf` reached the command as a file name.
        assert!(!stderr.contains("needs a value") && !stderr.contains("unknown option"), "{args:?}: {stderr}");
    }
}

/// The token after an option that takes no value is not its value: `check-one --edit file.pdf`
/// reported "check-one: missing file", the file having been read as `--edit`'s operand.
#[test]
fn a_flag_that_takes_no_value_does_not_swallow_the_file() {
    for args in [vec!["check-one", "--edit", "missing.pdf"], vec!["check-one", "missing.pdf", "--edit"]] {
        let out = Command::new(BIN).args(&args).output().expect("run cli");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?}");
        // Which file the error is about, not how the OS says it is missing (#3849 review).
        assert!(stderr.contains("missing.pdf"), "{args:?}: the error is about the file: {stderr}");
        assert!(!stderr.contains("missing file"), "the file was found, and then not found: {stderr}");
        assert!(!stderr.contains("needs a value"), "{args:?}: {stderr}");
    }
}

/// photocraft#1108: an argument that is not UTF-8 panicked in `std::env::args` ("called
/// `Result::unwrap()` on an `Err` value"), while collecting the arguments — so it panicked even
/// for `--help`. Such a name cannot be used here, where paths are `str`, so it is a usage error.
#[cfg(unix)]
#[test]
fn an_argument_that_is_not_utf8_is_an_error_not_a_panic() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    for args in [vec![OsStr::new("info"), OsStr::from_bytes(b"/tmp/caf\xe9.pdf")], vec![OsStr::new("--help"), OsStr::from_bytes(b"caf\xe9")]] {
        let out = Command::new(BIN).args(&args).output().expect("run cli");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "an error, not a panic (101): {stderr}");
        assert!(stderr.contains("not valid UTF-8"), "{stderr}");
        assert!(!stderr.contains("panicked"), "{stderr}");
    }
}

fn assert_missing_value(args: &[&str], command: &str, option: &str) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pdfcraft-cli-missing-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&dir).unwrap();
    let out = Command::new(BIN).current_dir(&dir).args(args).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
    assert!(stderr.contains(&format!("{command}: {option} needs a value")), "{args:?}: {stderr}");
    assert!(!stderr.contains("missing.pdf"), "{args:?}: {stderr}");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "{args:?}: wrote files");
    std::fs::remove_dir(&dir).unwrap();
}

macro_rules! missing_value_test {
    ($name:ident, $command:literal, $option:literal, [$($arg:literal),+ $(,)?]) => {
        #[test]
        fn $name() {
            assert_missing_value(&[$($arg),+], $command, $option);
        }
    };
}

missing_value_test!(missing_render_dpi, "render", "--dpi", ["render", "missing.pdf", "--out", "unused.png", "--dpi"]);
missing_value_test!(missing_render_page, "render", "--page", ["render", "missing.pdf", "--out", "unused.png", "--page"]);
missing_value_test!(missing_info_password, "info", "--password", ["info", "missing.pdf", "--password"]);
missing_value_test!(missing_check_timeout, "check", "--timeout", ["check", "missing.pdf", "--timeout"]);
missing_value_test!(missing_split_out_dir, "split", "--out-dir", ["split", "missing.pdf", "--every", "1", "--out-dir"]);
missing_value_test!(missing_split_every, "split", "--every", ["split", "missing.pdf", "--every"]);
missing_value_test!(missing_split_before, "split", "--before", ["split", "missing.pdf", "--before"]);
missing_value_test!(missing_extract_pages, "extract", "--pages", ["extract", "missing.pdf", "--pages"]);
missing_value_test!(missing_repeated_render_dpi, "render", "--dpi", ["render", "missing.pdf", "--out", "unused.png", "--dpi", "36", "--dpi"]);
missing_value_test!(missing_audit_password, "audit", "--password", ["audit", "missing.pdf", "--password"]);
missing_value_test!(missing_text_page, "text", "--page", ["text", "missing.pdf", "--page"]);
missing_value_test!(missing_text_password, "text", "--password", ["text", "missing.pdf", "--password"]);
missing_value_test!(missing_combine_out, "combine", "--out", ["combine", "missing.pdf", "--out"]);
missing_value_test!(missing_extract_out, "extract", "--out", ["extract", "missing.pdf", "--out"]);
missing_value_test!(missing_extract_password, "extract", "--password", ["extract", "missing.pdf", "--password"]);
missing_value_test!(missing_split_password, "split", "--password", ["split", "missing.pdf", "--every", "1", "--password"]);
missing_value_test!(missing_render_out, "render", "--out", ["render", "missing.pdf", "--out"]);
missing_value_test!(missing_render_password, "render", "--password", ["render", "missing.pdf", "--out", "unused.png", "--password"]);
missing_value_test!(missing_check_one_dpi, "check-one", "--dpi", ["check-one", "missing.pdf", "--dpi"]);
missing_value_test!(missing_check_dpi, "check", "--dpi", ["check", "missing.pdf", "--dpi"]);
missing_value_test!(missing_check_json, "check", "--json", ["check", "missing.pdf", "--json"]);
missing_value_test!(missing_run_script, "run", "--script", ["run", "--script"]);
missing_value_test!(missing_run_out, "run", "--out", ["run", "--out"]);
missing_value_test!(missing_ui_control, "ui", "--control", ["ui", "--control"]);
missing_value_test!(missing_ui_out, "ui", "--out", ["ui", "--out"]);
missing_value_test!(missing_batch_out_dir, "batch", "--out-dir", ["batch", "--out-dir"]);
missing_value_test!(missing_batch_steps, "batch", "--steps", ["batch", "--steps"]);
missing_value_test!(missing_batch_ppi, "batch", "--ppi", ["batch", "--ppi"]);
missing_value_test!(missing_batch_quality, "batch", "--quality", ["batch", "--quality"]);
missing_value_test!(missing_batch_dpi, "batch", "--dpi", ["batch", "--dpi"]);
missing_value_test!(missing_batch_language, "batch", "--language", ["batch", "--language"]);
