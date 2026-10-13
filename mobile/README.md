# PdfCraft Mobile

This directory contains native platform shells, toolchain definitions, benchmarks, and fixtures for PdfCraft on iOS and Android.

## Toolchain Pins

| Tool | Pinned Version | Notes |
|---|---|---|
| **Rust** | `1.99.0` (edition 2024) | Workspace minimum `rust-version = "1.95"` |
| **Xcode** | `27.0` (Build `27A266a`) | iOS SDK 17.0+ |
| **Android NDK** | `r30` (`/opt/homebrew/share/android-ndk`) | Min SDK: API 26 (`Android 8.0`) |
| **cargo-ndk** | `4.1.2` | NDK build helper for Rust |

### Installed Rust Targets

```bash
rustup target add aarch64-apple-ios
rustup target add aarch64-apple-ios-sim
rustup target add aarch64-linux-android
rustup target add x86_64-linux-android
```

## Build & Check Commands

### Cross-compilation checks

```bash
# iOS device & simulator
cargo check -p pdfcraft-engine -p pdfcraft-render -p pdfcraft-automation --target aarch64-apple-ios
cargo check -p pdfcraft-engine -p pdfcraft-render -p pdfcraft-automation --target aarch64-apple-ios-sim

# Android (arm64-v8a and x86_64) via cargo-ndk
export ANDROID_NDK_HOME="/opt/homebrew/share/android-ndk"
cargo ndk -t arm64-v8a check -p pdfcraft-engine -p pdfcraft-render -p pdfcraft-automation
cargo ndk -t x86_64 check -p pdfcraft-engine -p pdfcraft-render -p pdfcraft-automation
```

## Structure

```text
mobile/
  README.md         Toolchain pins, build, and test instructions
  fixtures/         Synthetic test PDFs only
  benchmarks/       Benchmark scenarios and committed baselines
  ios/
    PdfCraft/       Native SwiftUI application
    PdfCraftKit/    Swift Package (wraps libpdfcraft_ffi via UniFFI)
  android/
    app/            Native Jetpack Compose application
    pdfcraft-kit/   Android library module (AAR with 16KB-aligned JNI libs)
```

## Common Mobile Commands

```bash
# Build XCFramework & update Swift Package
cargo xtask mobile-xcframework

# Run Swift Package test suite
cd mobile/ios/PdfCraftKit && swift test

# Build Android 16KB page-aligned libraries & Kotlin bindings
cargo xtask mobile-android

# Verify mobile native library size against Budget B10 (≤ 25.0 MB)
cargo xtask mobile-size

# Verify API surface contract between FFI and committed snapshots
cargo xtask mobile-bindings --check
```

