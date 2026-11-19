//! Query the actual executable and exercise the AppImage source resolver without building a package.
use std::ffi::OsString;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Read an independent runtime expectation, rather than the value emitted by the build script.
fn expected() -> String {
    std::env::var("PDFCRAFT_EXPECTED_UPDATE_REPO").unwrap_or_else(|_| "storytold/pdfcraft".to_owned())
}

/// Query the real executable while clearing display access and setting an ignored runtime source.
fn run_arguments(args: &[OsString]) -> Output {
    run(Command::new(env!("CARGO_BIN_EXE_pdfcraft"))
        .args(args)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env("PDFCRAFT_UPDATE_REPO", "runtime/ignored"))
}

/// Bound and reap every child so a GUI-launch regression cannot hold the test runner open.
fn run(command: &mut Command) -> Output {
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!("metadata command did not exit: {command:?}: {output:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The first metadata flag must exit headlessly and ignore all trailing inputs and runtime overrides.
#[test]
fn compiled_repository_is_reported_without_a_display_or_runtime_override() {
    let cases: Vec<Vec<OsString>> = [
        vec![],
        vec!["--new-window"],
        vec!["missing.pdf"],
        vec!["--new-window", "missing.pdf"],
        vec!["--help"],
        vec!["--version"],
        vec!["a file with spaces.pdf"],
    ]
    .into_iter()
    .map(|args| args.into_iter().map(OsString::from).collect())
    .collect();
    #[cfg(unix)]
    let cases = {
        use std::os::unix::ffi::OsStringExt;
        let mut cases = cases;
        cases.push(vec![OsString::from_vec(b"non-utf8-\xff.pdf".to_vec())]);
        cases
    };
    for trailing in cases {
        let mut args = vec![OsString::from("--print-update-repository")];
        args.extend(trailing);
        let out = run_arguments(&args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{}\n", expected()), "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}: {:?}", out.stderr);
    }
}

/// A later metadata flag must not change the normal help or version output.
#[test]
fn earlier_help_and_version_keep_their_normal_behavior() {
    for (first, prefix) in [("--help", "Usage: pdfcraft"), ("--version", "pdfcraft ")] {
        let out = run_arguments(&[first.into(), "--print-update-repository".into()]);
        assert!(out.status.success(), "{first}: {out:?}");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.starts_with(prefix), "{first}: {text}");
        let plain = run_arguments(&[first.into()]);
        assert_eq!(text.as_bytes(), plain.stdout, "the later query flag must be ignored");
        assert!(!text.lines().any(|line| line == expected()));
    }
}

#[cfg(target_os = "linux")]
/// Resolve packaging metadata in a clean environment without access to a desktop display.
fn resolver(binary: &str) -> Command {
    let mut cmd = Command::new("bash");
    cmd.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/linux/update-repository.sh")).arg(binary);
    for name in ["PDFCRAFT_UPDATE_REPO", "GITHUB_REPOSITORY", "CROSS_TARGET", "EMULATOR", "DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET"] {
        cmd.env_remove(name);
    }
    cmd
}

#[cfg(target_os = "linux")]
/// Fallback must query the binary and refuse missing or incompatible executables.
#[test]
fn appimage_falls_back_to_the_compiled_source() {
    let out = run(&mut resolver(env!("CARGO_BIN_EXE_pdfcraft")));
    assert!(out.status.success(), "{:?}", out);
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{}\n", expected()));
    let out = run(&mut resolver("/nonexistent/pdfcraft"));
    assert_failure(&out, "No such file or directory");
    let out = run(&mut resolver("/bin/echo"));
    assert_failure(&out, "this binary cannot report its update source");
}

#[cfg(target_os = "linux")]
/// Explicit packaging sources take precedence even when there is no executable to query.
#[test]
fn appimage_overrides_keep_their_precedence_without_running_the_binary() {
    for (custom, github, want) in [("Custom/PdfCraft", "ci/repo", "Custom/PdfCraft"), ("", "ci/repo", "ci/repo")] {
        let out = run(resolver("/nonexistent/pdfcraft").env("PDFCRAFT_UPDATE_REPO", custom).env("GITHUB_REPOSITORY", github));
        assert!(out.status.success(), "{:?}", out);
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{want}\n"));
    }
}

#[cfg(target_os = "linux")]
/// Both override variables must reject malformed identities; cross builds need a source or emulator.
#[test]
fn appimage_rejects_malformed_sources_and_unqueryable_cross_builds() {
    let mut bad = vec![
        " ".to_owned(),
        "example".into(),
        "/pdfcraft".into(),
        "example/".into(),
        "a/b/c".into(),
        "https://github.com/a/b".into(),
        "git@github.com:a/b".into(),
        "a@github.com/b".into(),
        "a/b?token=secret".into(),
        "a/b#main".into(),
        "a/b|latest|foreign".into(),
        "a/b\nother".into(),
        "a/b\r".into(),
        "a/b ".into(),
        " a/b".into(),
        "a/..".into(),
        "a/.".into(),
        "../b".into(),
        "a/%2e%2e".into(),
        "a\\b".into(),
        "a/pdƒcraft".into(),
        "-a/b".into(),
        "a-/b".into(),
        "a--b/c".into(),
        "a_b/c".into(),
    ];
    bad.extend([format!("{}/b", "a".repeat(40)), format!("a/{}", "b".repeat(101))]);
    for name in ["PDFCRAFT_UPDATE_REPO", "GITHUB_REPOSITORY"] {
        for value in &bad {
            let out = run(resolver("/nonexistent/pdfcraft").env(name, value));
            assert_failure(&out, "update source must be a GitHub owner/repo");
        }
        // Empty packaging overrides mean unset: query the binary, never silently use official releases.
        let out = run(resolver("/nonexistent/pdfcraft").env(name, ""));
        assert_failure(&out, "No such file or directory");
    }
    let out = run(resolver(env!("CARGO_BIN_EXE_pdfcraft")).env("CROSS_TARGET", "aarch64-unknown-linux-gnu"));
    assert_failure(&out, "cannot query a cross-compiled binary; set PDFCRAFT_UPDATE_REPO or EMULATOR");
    let out = run(resolver("/nonexistent/pdfcraft").env("CROSS_TARGET", "aarch64-unknown-linux-gnu").env("PDFCRAFT_UPDATE_REPO", "cross/repo"));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "cross/repo\n");
}

#[cfg(target_os = "linux")]
/// A refusal must emit no repository and identify the actual reason for failing.
fn assert_failure(out: &Output, message: &str) {
    assert!(!out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "a failure must not select any repository: {out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains(message), "{out:?}");
}

#[cfg(target_os = "linux")]
/// Record both calls so directly running the native fixture cannot masquerade as emulator support.
#[test]
fn appimage_queries_cross_builds_through_the_emulator() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let emulator = dir.path().join("emulator.sh");
    let calls = dir.path().join("calls.txt");
    std::fs::write(&emulator, "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PDFCRAFT_TEST_EMULATOR_LOG\"\nexec \"$@\"\n").unwrap();
    std::fs::set_permissions(&emulator, std::fs::Permissions::from_mode(0o700)).unwrap();
    let out = run(resolver(env!("CARGO_BIN_EXE_pdfcraft"))
        .env("CROSS_TARGET", "aarch64-unknown-linux-gnu")
        .env("EMULATOR", &emulator)
        .env("PDFCRAFT_TEST_EMULATOR_LOG", &calls));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{}\n", expected()));
    assert!(out.stderr.is_empty(), "{out:?}");
    let binary = env!("CARGO_BIN_EXE_pdfcraft");
    assert_eq!(std::fs::read_to_string(calls).unwrap(), format!("{binary}\n--help\n{binary}\n--print-update-repository\n"));
}
