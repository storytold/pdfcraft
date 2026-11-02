# PdfCraft for Android

**A native Android PDF reader and editor built with Kotlin, Jetpack Compose, and PdfCraft's Rust engine.**

This repository is an independent Android-focused fork of [storytold/pdfcraft](https://github.com/storytold/pdfcraft). The Android application lives in `apps/pdfcraft-android/android`; its Rust/UniFFI bridge lives in `apps/pdfcraft-android`.

<details>
<summary>Project status</summary>

**Status: beta / active development.** The Android application and native bridge are present on the `android-port` branch. The repository's default branch is currently `main`, which tracks the upstream desktop project. Select `android-port` from the branch selector to browse the Android implementation.

No production release is published yet. CI build artifacts, when available, are development builds and should not be treated as a signed Play Store release.

</details>

## What is included

- Native Kotlin UI using Jetpack Compose and Material 3.
- Existing PdfCraft Rust engine reused through UniFFI; no WebView and no duplicate PDF engine.
- PDF reading, navigation, search, text selection, bookmarks and reading modes.
- Annotations, voice comments, form filling, Fill & Sign and page organization.
- Offline-first document workflows using Android's Storage Access Framework.
- Native rendering, background read-aloud and Android sharing/open-with integration.

See [the Android application guide](apps/pdfcraft-android/README.md) for the feature inventory, architecture, limitations and build instructions.

## Build locally

### Requirements

- Rust **1.95 or newer**, with Android targets.
- JDK 17.
- Android SDK and NDK **29.0.14206865**.
- `cargo-ndk` 4.1.2.

Install the Rust targets and cargo-ndk:

```sh
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
```

Build and test:

```sh
cd apps/pdfcraft-android/android
./gradlew testDebugUnitTest
./gradlew assembleDebug
```

The debug APK is written to:

```
apps/pdfcraft-android/android/app/build/outputs/apk/debug/app-debug.apk
```

To build a release APK and Android App Bundle for local validation:

```sh
./gradlew assembleRelease bundleRelease
```

A locally generated release build is not a Play Store release. Configure and protect a real release signing key before distribution.

## Repository layout

| Path | Purpose |
|---|---|
| `apps/pdfcraft-android/android` | Native Android Studio / Gradle application |
| `apps/pdfcraft-android/src` | Rust mobile session and UniFFI API |
| `apps/pdfcraft-android/bindgen` | Kotlin binding generator |
| `crates/engine`, `crates/render`, `crates/automation`, etc. | Shared Rust PDF engine |
| `.github/workflows/android.yml` | Android build and test workflow |
| `docs/audit/` | Engineering audit, defect register and verification evidence |

## Upstream and contributions

- Original project: [storytold/pdfcraft](https://github.com/storytold/pdfcraft)
- Android implementation: [apps/pdfcraft-android](https://github.com/dikshant363/pdfcraft-android/tree/android-port/apps/pdfcraft-android)
- Contribution and audit notes: [docs/audit/](docs/audit/)

Android-specific work is maintained separately from upstream-compatible Rust contributions. Upstream changes are proposed through focused pull requests; this fork does not imply that the upstream maintainers have accepted or adopted the Android application.

## Licence

See the repository's `LICENSE`, `LICENSE-APACHE`, `LICENSE-MIT`, and `ATTRIBUTION.toml` files for project and third-party licence details. Third-party assets retain their respective licences and attributions.
