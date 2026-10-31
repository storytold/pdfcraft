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
        assert_eq!(out.status.code(), Some(2), "{args:?} unexpectedly succeeded: {stderr}");
        assert!(stderr.contains("unexpected argument") && stderr.contains(unknown), "{args:?}: {stderr}");
    }
}

/// photocraft#6944: an option took the next token whatever it was, so a line with a value missing
/// in the middle of it did something else entirely — `extract in.pdf --pages 1 --out --password`
/// wrote the PDF to a file called `--password` and exited 0. The unknown option hidden in a value
/// position passed the check above too, since the walk skipped it along with the option.
///
/// RFC-0012 §1.4 extends that to every value flag: a password or a title that starts with `-` is
/// written `--password=--page`, so a flag never takes the next flag as its value.
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
        assert_eq!(out.status.code(), Some(2), "{args:?} unexpectedly succeeded: {stderr}");
        // clap names the missing value, or the flag it found where the value should be.
        assert!(stderr.contains(&format!("a value is required for '{option} ")) || stderr.contains("unexpected argument"), "{args:?}: {stderr}");
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
        assert!(!stderr.contains("a value is required") && !stderr.contains("unexpected argument"), "{args:?}: {stderr}");
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
        assert!(!stderr.contains("a value is required"), "{args:?}: {stderr}");
    }
}

/// photocraft#1108: an argument that is not UTF-8 panicked in `std::env::args`, while collecting
/// the arguments, so even `--help` panicked. A file name is a path now and reaches the command
/// (which fails to open it: a runtime error); anywhere a string is needed it is a usage error.
#[cfg(unix)]
#[test]
fn an_argument_that_is_not_utf8_is_an_error_not_a_panic() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let out = Command::new(BIN).args([OsStr::new("info"), OsStr::from_bytes(b"/tmp/caf\xe9.pdf")]).output().expect("run cli");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "a failure to open the file, not a panic (101): {stderr}");
    assert!(stderr.starts_with("error: "), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");

    for args in [vec![OsStr::new("run"), OsStr::from_bytes(b"caf\xe9")], vec![OsStr::new("--help"), OsStr::from_bytes(b"caf\xe9")]] {
        let out = Command::new(BIN).args(&args).output().expect("run cli");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.code() == Some(2) || out.status.code() == Some(0), "{args:?}: {stderr}");
        assert!(!stderr.contains("panicked"), "{stderr}");
    }
    // A name that is not UTF-8 works as a path: it is the file `info` reports.
    let out = Command::new(BIN).args([OsStr::new("info"), OsStr::from_bytes(b"/tmp/caf\xe9.pdf")]).output().expect("run cli");
    assert!(String::from_utf8_lossy(&out.stderr).contains("caf"), "the file is named");
}
