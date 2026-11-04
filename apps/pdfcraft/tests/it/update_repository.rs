//! Query the actual executable and exercise the AppImage source resolver without building a package.
use std::process::Command;

fn expected() -> String {
    std::env::var("PDFCRAFT_EXPECTED_UPDATE_REPO").unwrap_or_else(|_| "storytold/pdfcraft".to_owned())
}

#[test]
fn compiled_repository_is_reported_without_a_display_or_runtime_override() {
    let out = Command::new(env!("CARGO_BIN_EXE_pdfcraft"))
        .arg("--print-update-repository")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("PDFCRAFT_UPDATE_REPO", "runtime/ignored")
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out);
    assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{}\n", expected()));
    assert!(out.stderr.is_empty());
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
