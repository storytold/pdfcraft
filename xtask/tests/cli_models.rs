//! Execute the release scripts with synthetic inputs, then unpack their real CLI archives.
//! Compilation, downloads and platform signing tools are replaced by local test shims.

#![cfg(unix)]

use std::path::Path;
use std::process::Command;

#[test]
fn linux_cli_archive_includes_ocr_models_and_their_notices() {
    check_archive("linux");
}

#[cfg(target_os = "macos")]
#[test]
fn macos_cli_archive_includes_ocr_models_and_their_notices() {
    check_archive("macos");
}

fn check_archive(platform: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let output = Command::new("bash").arg(root.join("packaging/tests/cli-models.sh")).arg(platform).output().unwrap();
    assert!(
        output.status.success(),
        "CLI archive regression failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
