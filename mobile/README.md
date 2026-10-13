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

## Environment Setup

Set these paths in your shell or CI environment:

```bash
# JDK 17 (required for Gradle and Android build tools)
export JAVA_HOME="/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home"

# Android NDK r30
export ANDROID_NDK_HOME="/opt/homebrew/Caskroom/android-ndk/30/AndroidNDK16248370.app/Contents/NDK"

# Android SDK & Platform-Tools (adb)
export ANDROID_HOME="/Users/ritvikpinjala/Library/Android/sdk"
export PATH="$ANDROID_HOME/platform-tools:$PATH"
```

### Android Emulator & Target Device Specs

- **Emulator Device:** Pixel 10 Pro AVD (`emulator-5554`)
- **System Image:** `sdk_gphone16k_arm64` (Android 17 / `emu64a16k`)
- **Architecture:** `arm64-v8a` (16KB page-size aligned)
- **Physical Device:** Not connected during headless CI runs; physical metrics are reported as `"not measured (no physical phone attached)"`.

To run Android tests on the attached emulator:

```bash
cd mobile/android
JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home ./gradlew test lint
ANDROID_SERIAL=emulator-5554 JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home ./gradlew connectedAndroidTest
```

---

## Decision O6: CJK Font Strategy & Binary Size Trade-offs

Per AGENTS.md §1.4, Japanese and CJK fonts live in `storytold/craft-fonts` and are never committed directly. On mobile platforms, bundling full CJK font files heavily impacts download and library size (a typical CJK font face is 15–25 MiB uncompressed, or 8–14 MiB compressed).

### Evaluated Options:

1. **Option A: Static Compilation Embedding (`CRAFT_FONTS_DIR`)**
   - *Mechanism:* Compile `pdfcraft-fonts` with `CRAFT_FONTS_DIR` embedding `CRAFT_FONTS`.
   - *Pros:* Fully offline, 100% deterministic visual rendering across all devices, zero runtime network dependencies.
   - *Cons:* Binary size increases by ~12–18 MiB, causing the stripped `.so` to exceed Budget B10 ($\le 25.0$ MiB for all features or $\le 12.0$ MiB for viewer-only).
   - *Recommended Use:* Dedicated enterprise or desktop distributions.

2. **Option B: Dynamic Download-on-Demand (Play Feature Delivery / Asset Pack)**
   - *Mechanism:* Deliver CJK font resources as an optional downloadable language pack upon first encounter of CJK content.
   - *Pros:* Keeps base app installation small ($\le 12.0$ MiB viewer); guarantees licensed OFL font fidelity once downloaded.
   - *Cons:* Requires network access on first render of Japanese documents; adds download management UI and storage caching logic.
   - *Recommended Use:* Standard mobile consumer release track.

3. **Option C: Platform System Font Fallback (Runtime OS Fallback)**
   - *Mechanism:* Query system-installed CJK fonts via Android `FontMatcher` / `SystemFonts` at runtime.
   - *Pros:* Zero additional download, 0 MiB added to binary size.
   - *Cons:* Visual rendering varies across Android OS versions and vendor skins (Samsung vs Pixel vs Xiaomi); per AGENTS.md §1.4, system fonts may never be embedded back into saved PDFs.
   - *Recommended Use:* Read-only viewing fallback when downloadable packs are absent.


