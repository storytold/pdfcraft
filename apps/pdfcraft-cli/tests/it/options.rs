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

/// A file `check-one` cannot read is a record on stdout with its own status and the reason, which
/// `check` relays, not an empty stdout that reads as a crash.
#[test]
fn check_one_reports_an_unreadable_file_as_a_record() {
    let out = Command::new(BIN).args(["check-one", "missing.pdf"]).output().expect("run cli");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let record: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout:?}"));
    assert_eq!(record["status"], "read-error");
    assert!(record["detail"].as_str().is_some_and(|d| d.contains("missing.pdf")), "{record}");
}

mod missing_value_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct ValueDir(PathBuf);

    impl ValueDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            for _ in 0..128 {
                let serial = NEXT.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!("pdfcraft-cli-missing-values-{}-{serial}", std::process::id()));
                match std::fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("creating missing-value test directory: {error}"),
                }
            }
            panic!("no unused missing-value test directory after 128 attempts");
        }

        fn write_pdf(&self) -> Vec<u8> {
            let bytes = pdfcraft_engine::Session::new().create_from_text("Missing operand", "Keep this document").unwrap();
            std::fs::write(self.0.join("input.pdf"), bytes.as_slice()).unwrap();
            bytes.as_ref().clone()
        }
    }

    impl Drop for ValueDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn render_refuses_a_missing_numeric_operand_instead_of_writing_with_defaults() {
        let dir = ValueDir::new();
        let bytes = dir.write_pdf();
        let output = dir.0.join("page.png");
        for flags in [vec!["--page"], vec!["--dpi"], vec!["--page", "1", "--page"], vec!["--dpi", "18", "--dpi"]] {
            let option = flags.last().unwrap();
            let out = Command::new(BIN).current_dir(&dir.0).args(["render", "input.pdf", "--out", "page.png"]).args(&flags).output().unwrap();
            assert_eq!(out.status.code(), Some(1), "{flags:?}: {out:?}");
            assert!(out.stdout.is_empty(), "{flags:?}: no render result may be printed");
            assert_eq!(String::from_utf8(out.stderr).unwrap().trim(), format!("pdfcraft-cli: render: {option} needs a value"));
            assert!(!output.exists(), "{flags:?}: no output may be written");
            assert_eq!(std::fs::read(dir.0.join("input.pdf")).unwrap(), bytes);
        }
        // Omitting an option still uses its default; only a supplied option needs an operand.
        let out = Command::new(BIN).current_dir(&dir.0).args(["render", "input.pdf", "--out", "page.png"]).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(output.exists());
    }

    #[test]
    fn commands_refuse_a_missing_operand_before_reading_or_scanning() {
        let dir = ValueDir::new();
        for (command, option) in [("info", "--password"), ("audit", "--password"), ("check-one", "--dpi"), ("check", "--dpi"), ("check", "--timeout")]
        {
            let out = Command::new(BIN).current_dir(&dir.0).args([command, "missing.pdf", option]).output().unwrap();
            assert_eq!(out.status.code(), Some(1), "{command} {option}: {out:?}");
            assert!(out.stdout.is_empty(), "{command} {option}: no file record may be printed");
            assert_eq!(String::from_utf8(out.stderr).unwrap().trim(), format!("pdfcraft-cli: {command}: {option} needs a value"));
        }
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0, "nothing may be written");
    }

    #[test]
    fn supplied_string_operands_can_be_empty_or_look_like_options() {
        let dir = ValueDir::new();
        let bytes = dir.write_pdf();
        for password in ["", "--page"] {
            let out = Command::new(BIN).current_dir(&dir.0).args(["info", "input.pdf", "--password", password]).output().unwrap();
            assert!(out.status.success(), "{password:?}: {out:?}");
            assert!(out.stderr.is_empty(), "{password:?}: {out:?}");
            let info: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(info["pages"], 1);
            assert_eq!(std::fs::read(dir.0.join("input.pdf")).unwrap(), bytes);
        }
    }
}
