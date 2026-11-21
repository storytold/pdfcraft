//! An `OPENSSL_ia32cap` that asks for a CPU feature the processor lacks stops neither the CLI nor
//! its signing (#2916). aws-lc printed "Fatal Error: HW capability found: …" and aborted before
//! `main`. Built to detect the CPU on first use, it would abort at the first signature instead.

// macOS intentionally does not relaunch: Finder delivers files to the original process.
#![cfg(not(target_os = "macos"))]

use std::process::{Command, Output};

/// CPUID leaf 1, EDX bit 10: reserved, so no x86 processor reports it.
const MISSING_FEATURE: &str = "0x400";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli")).args(args).env("OPENSSL_ia32cap", MISSING_FEATURE).output().unwrap()
}

#[test]
fn a_cpu_override_naming_a_missing_feature_stops_neither_the_cli_nor_signing() {
    let out = run(&["--help"]);
    assert!(out.status.success(), "--help: {:?} {}", out.status, String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("pdfcraft-cli render"));

    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-cpu-override-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A new RSA key: the CLI's first use of aws-lc.
    let id = dir.join("id.p12");
    let path = format!("path={}", id.display());
    let out = run(&["run", "sign_id_create", "name=Test", "password=correct horse", &path, "--root", dir.to_str().unwrap()]);
    assert!(out.status.success(), "sign_id_create: {:?} {}", out.status, String::from_utf8_lossy(&out.stderr));
    assert!(std::fs::metadata(&id).is_ok_and(|m| m.len() > 0), "no digital ID at {}", id.display());
    let _ = std::fs::remove_dir_all(&dir);
}
