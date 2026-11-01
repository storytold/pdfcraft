//! `cargo xtask mobile ios|android`: cross-compile `pdfcraft-mobile-ffi`, generate its Swift/
//! Kotlin bindings (library mode: `uniffi-bindgen` reads them back out of the compiled library,
//! so there is no `.udl` file to keep in sync with `crates/mobile-ffi/src/lib.rs`), and drop the
//! result where `apps/pdfcraft-ios`/`apps/pdfcraft-android` expect it.
//!
//! Neither subcommand runs as part of `cargo xtask ci`: there is no shared `craft-common-actions`
//! workflow for mobile builds yet (CLAUDE.md: CI changes don't belong in a feature PR), and the
//! iOS build only runs where Xcode does (macOS). Run these by hand, or from Xcode/Android Studio.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::gates::root;

const CRATE: &str = "pdfcraft-mobile-ffi";
const LIB: &str = "pdfcraft_mobile_ffi";
/// The clang module `uniffi-bindgen` emits for Swift (`<LIB>FFI`, see the generated
/// `.modulemap`). The xcframework and the `Package.swift` binary target must share this name:
/// SPM resolves a binary target's module by matching its headers' module name against the name
/// the target was declared with.
const IOS_FFI_MODULE: &str = "pdfcraft_mobile_ffiFFI";

const IOS_DEVICE: &str = "aarch64-apple-ios";
const IOS_SIM_TARGETS: &[&str] = &["aarch64-apple-ios-sim", "x86_64-apple-ios"];
/// (Rust target triple, Android ABI name used under `jniLibs/`).
const ANDROID_TARGETS: &[(&str, &str)] =
    &[("aarch64-linux-android", "arm64-v8a"), ("armv7-linux-androideabi", "armeabi-v7a"), ("x86_64-linux-android", "x86_64")];

pub fn ios(_args: &[String]) -> Result<()> {
    let root = root();
    require_tool("xcrun", "Xcode command line tools (xcrun, xcodebuild, lipo) — install Xcode")?;

    for target in std::iter::once(IOS_DEVICE).chain(IOS_SIM_TARGETS.iter().copied()) {
        cargo_build_target(&root, target)?;
    }

    let bindings_dir = root.join("target/mobile/ios/bindings");
    fs::create_dir_all(&bindings_dir)?;
    generate_bindings(&root, "swift", &static_lib(&root, IOS_DEVICE), &bindings_dir)?;

    let sim_fat_lib = root.join("target/mobile/ios/libpdfcraft_mobile_ffi-sim.a");
    lipo_create(&IOS_SIM_TARGETS.iter().map(|t| static_lib(&root, t)).collect::<Vec<_>>(), &sim_fat_lib)?;

    let frameworks_dir = root.join("apps/pdfcraft-ios/Frameworks");
    fs::create_dir_all(&frameworks_dir)?;
    let xcframework = frameworks_dir.join(format!("{IOS_FFI_MODULE}.xcframework"));
    if xcframework.exists() {
        fs::remove_dir_all(&xcframework).with_context(|| format!("removing stale {}", xcframework.display()))?;
    }
    run(
        Command::new("xcrun")
            .args(["xcodebuild", "-create-xcframework"])
            .arg("-library")
            .arg(static_lib(&root, IOS_DEVICE))
            .arg("-headers")
            .arg(&bindings_dir)
            .arg("-library")
            .arg(&sim_fat_lib)
            .arg("-headers")
            .arg(&bindings_dir)
            .arg("-output")
            .arg(&xcframework),
        "xcodebuild -create-xcframework",
    )?;

    let sources_dir = root.join("apps/pdfcraft-ios/Sources/PdfCraftFFI");
    fs::create_dir_all(&sources_dir)?;
    fs::copy(bindings_dir.join(format!("{LIB}.swift")), sources_dir.join(format!("{LIB}.swift")))?;

    println!("wrote {}", xcframework.display());
    println!("wrote {}", sources_dir.join(format!("{LIB}.swift")).display());
    Ok(())
}

pub fn android(_args: &[String]) -> Result<()> {
    let root = root();
    require_tool("cargo-ndk", "`cargo install cargo-ndk`, with $ANDROID_NDK_HOME set to an installed NDK")?;

    let jni_libs = root.join("apps/pdfcraft-android/app/src/main/jniLibs");
    fs::create_dir_all(&jni_libs)?;

    let mut ndk = Command::new("cargo");
    ndk.current_dir(&root).arg("ndk");
    for (target, _) in ANDROID_TARGETS {
        ndk.args(["-t", target]);
    }
    ndk.arg("-o").arg(&jni_libs).args(["build", "--release", "-p", CRATE]);
    run(&mut ndk, "cargo ndk build")?;

    // cargo-ndk already lays out <jni_libs>/<abi>/lib*.so; only the bindings are left to generate,
    // from whichever ABI's .so was just built (library mode reads the library's own metadata, so
    // any one of them does, same as the simulator slice does for iOS above).
    let one_so = jni_libs.join(ANDROID_TARGETS[0].1).join(format!("lib{LIB}.so"));
    let bindings_dir = root.join("target/mobile/android/bindings");
    fs::create_dir_all(&bindings_dir)?;
    generate_bindings(&root, "kotlin", &one_so, &bindings_dir)?;

    let kotlin_out = root.join("apps/pdfcraft-android/app/src/main/java");
    fs::create_dir_all(&kotlin_out)?;
    copy_dir(&bindings_dir, &kotlin_out)?;

    println!("wrote {}", jni_libs.display());
    println!("wrote {}", kotlin_out.join("uniffi").display());
    Ok(())
}

fn cargo_build_target(root: &Path, target: &str) -> Result<()> {
    run(
        Command::new("cargo").current_dir(root).args(["build", "--release", "-p", CRATE, "--target", target]),
        &format!("cargo build --target {target}"),
    )
}

fn static_lib(root: &Path, target: &str) -> PathBuf {
    root.join(format!("target/{target}/release/lib{LIB}.a"))
}

fn lipo_create(inputs: &[PathBuf], output: &Path) -> Result<()> {
    let mut cmd = Command::new("lipo");
    cmd.arg("-create").arg("-output").arg(output);
    for input in inputs {
        cmd.arg(input);
    }
    run(&mut cmd, "lipo -create")
}

/// Builds the `uniffi-bindgen` binary (library mode: `--features cli`) and runs it against
/// `library`'s own embedded metadata — no `.udl` file, so the generated bindings can never drift
/// from `src/lib.rs`.
fn generate_bindings(root: &Path, language: &str, library: &Path, out_dir: &Path) -> Result<()> {
    run(
        Command::new("cargo").current_dir(root).args(["build", "-p", CRATE, "--bin", "uniffi-bindgen", "--features", "cli"]),
        "cargo build --bin uniffi-bindgen",
    )?;
    let bindgen = root.join("target/debug/uniffi-bindgen");
    run(
        Command::new(&bindgen).args(["generate", "--library"]).arg(library).args(["--language", language, "--out-dir"]).arg(out_dir),
        "uniffi-bindgen generate",
    )
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    for entry in fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            fs::create_dir_all(&dest)?;
            copy_dir(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

fn require_tool(name: &str, how_to_install: &str) -> Result<()> {
    let found = Command::new(if cfg!(windows) { "where" } else { "which" }).arg(name).output().map(|o| o.status.success()).unwrap_or(false);
    if found { Ok(()) } else { bail!("`{name}` is not on PATH ({how_to_install})") }
}

fn run(cmd: &mut Command, what: &str) -> Result<()> {
    let status = cmd.status().with_context(|| format!("running {what}"))?;
    if status.success() { Ok(()) } else { bail!("{what}: exited with {status}") }
}
