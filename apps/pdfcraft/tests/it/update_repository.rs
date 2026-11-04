//! Query the actual executable and exercise the AppImage source resolver without building a package.
use std::ffi::OsString;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn expected() -> String {
    std::env::var("PDFCRAFT_EXPECTED_UPDATE_REPO").unwrap_or_else(|_| "storytold/pdfcraft".to_owned())
}

// Bound the subprocess so a regression cannot leave a GUI running on a CI desktop.
fn run_arguments(args: &[OsString]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pdfcraft"))
        .args(args)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env("PDFCRAFT_UPDATE_REPO", "runtime/ignored")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!("metadata command did not exit: {args:?}: {output:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

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
        assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{}\n", expected()), "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}: {:?}", out.stderr);
    }
}

#[test]
fn earlier_help_and_version_keep_their_normal_behavior() {
    for (first, prefix) in [("--help", "Usage: pdfcraft"), ("--version", "pdfcraft ")] {
        let out = run_arguments(&[first.into(), "--print-update-repository".into()]);
        assert!(out.status.success(), "{first}: {out:?}");
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.starts_with(prefix), "{first}: {text}");
        assert_ne!(text.trim(), expected());
    }
}

#[cfg(target_os = "linux")]
fn resolver(binary: &str) -> Command {
    let mut cmd = Command::new("bash");
    cmd.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/linux/update-repository.sh")).arg(binary);
    for name in ["PDFCRAFT_UPDATE_REPO", "GITHUB_REPOSITORY", "CROSS_TARGET", "EMULATOR", "DISPLAY", "WAYLAND_DISPLAY"] {
        cmd.env_remove(name);
    }
    cmd
}

#[cfg(target_os = "linux")]
#[test]
fn appimage_falls_back_to_the_compiled_source() {
    let out = resolver(env!("CARGO_BIN_EXE_pdfcraft")).output().unwrap();
    assert!(out.status.success(), "{:?}", out);
    assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{}\n", expected()));
    let out = resolver("/nonexistent/pdfcraft").output().unwrap();
    assert!(!out.status.success(), "a missing binary must never silently select official releases");
    let out = resolver("/bin/echo").output().unwrap();
    assert!(!out.status.success(), "an executable without the query option must be rejected before querying");
}

#[cfg(target_os = "linux")]
#[test]
fn appimage_overrides_keep_their_precedence_without_running_the_binary() {
    for (custom, github, want) in [("Custom/PdfCraft", "ci/repo", "Custom/PdfCraft"), ("", "ci/repo", "ci/repo")] {
        let out = resolver("/nonexistent/pdfcraft").env("PDFCRAFT_UPDATE_REPO", custom).env("GITHUB_REPOSITORY", github).output().unwrap();
        assert!(out.status.success(), "{:?}", out);
        assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{want}\n"));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn appimage_rejects_malformed_sources_and_unqueryable_cross_builds() {
    for bad in ["https://github.com/a/b", "a/b|latest|foreign", "a/b\nother", "a/b/c", "a/..", "a--b/c"] {
        let out = resolver("/nonexistent/pdfcraft").env("PDFCRAFT_UPDATE_REPO", bad).output().unwrap();
        assert!(!out.status.success(), "accepted {bad:?}");
    }
    let out = resolver(env!("CARGO_BIN_EXE_pdfcraft")).env("CROSS_TARGET", "aarch64-unknown-linux-gnu").output().unwrap();
    assert!(!out.status.success());
    let out = resolver("/nonexistent/pdfcraft")
        .env("CROSS_TARGET", "aarch64-unknown-linux-gnu")
        .env("PDFCRAFT_UPDATE_REPO", "cross/repo")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "cross/repo\n");
}
