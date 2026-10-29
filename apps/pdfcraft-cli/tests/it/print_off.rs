//! Without the `network-print` feature `print --wsd` says so and opens no connection.
#![cfg(not(feature = "network-print"))]

use std::process::Command;

#[test]
fn a_build_without_the_feature_says_so() {
    let o = Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli")).args(["print", "--wsd", "http://127.0.0.1:9/x", "--info"]).output().unwrap();
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("network-print"));
}
