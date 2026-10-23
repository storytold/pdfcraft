# PdfCraft Android skeleton

Gradle project that packages the PdfCraft Rust `cdylib` (`libpdfcraft.so`) behind
`android.app.NativeActivity`. The Rust side lives in `apps/pdfcraft/src/lib.rs`; its exported
`android_main(app: AndroidApp)` is wired by `android-activity` (the same backend eframe 0.36 /
winit 0.30 use).

## Prerequisites

- Rust Android targets:
  ```sh
  rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
  ```
- `cargo-ndk`:
  ```sh
  cargo install cargo-ndk
  ```
- Android NDK + SDK. The Gradle task reads `ANDROID_NDK_HOME` (or `ANDROID_NDK_ROOT`), and falls
  back to `~/android-ndk-r23c`. The SDK location goes into `local.properties`:
  ```properties
  sdk.dir=/home/you/Android/Sdk
  ```

## Build

```sh
# From apps/pdfcraft/android/
./gradlew assembleDebug
```

`assembleDebug` runs `cargoNdkBuild` first, which invokes `cargo ndk` for `arm64-v8a`,
`armeabi-v7a` and `x86_64` and drops each `libpdfcraft.so` into `app/src/main/jniLibs/<abi>/`.
The APK lands in `app/build/outputs/apk/debug/`.

To build only the Rust library (without Gradle):

```sh
cargo ndk -t arm64-v8a -o app/src/main/jniLibs build --release --lib
```

## Layout

- `app/src/main/AndroidManifest.xml` — declares `MainActivity` (a `NativeActivity`) and
  `android.app.lib_name = pdfcraft`.
- `app/src/main/java/ai/storyteller/pdfcraft/MainActivity.java` — thin `NativeActivity` subclass.
- `apps/pdfcraft/src/lib.rs` — the `android_main` entry point (the JNI → android-activity bridge).

## Notes

On Android the desktop-only services are absent: file dialogs (`rfd`) and the system clipboard
(`arboard`) have no Android backend, so `pdfcraft-ui-egui` swaps in no-op stand-ins (see
`crates/ui-egui/src/rfd.rs` and `crates/ui-egui/src/arboard.rs`). The editor runs fullscreen and
degrades gracefully — no file dialog, no clipboard, no native menu — exactly like the PhotoCraft
skeleton.
