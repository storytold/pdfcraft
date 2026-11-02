# PdfCraft Android Native Port — Task Tracking

All tasks follow the strict verification discipline:
`[ ] Not started` | `[-] In progress` | `[x] Verified complete` | `[!] Blocked`

---

## M0: Repository & Environment Audit
- [x] Inspect local repository structure, git status, and branches
- [x] Audit Rust workspace members, edition, and MSRV requirements
- [x] Clone and inspect upstream `pdfcraft` (v0.17.0) and reference architecture
- [x] Verify toolchain: OpenJDK 17, Android SDK Platform 35 & 37, Build-tools 35 & 36, Android NDK 29, cargo-ndk, rustc 1.97
- [x] Configure git remotes (`upstream` -> storytold, `origin` -> dikshant363) and branch `android-port`
- [x] Generate `docs/android/REPOSITORY_AUDIT.md`
- [x] Generate `docs/android/ARCHITECTURE.md`
- [x] Generate `docs/android/ROADMAP.md`
- [x] Generate `TASKS.md`

---

## M1: Android Studio Project & Package Foundation
- [x] Relocate package namespace to `io.github.dikshant363.pdfcraft`
- [x] Configure `local.properties` with verified SDK and NDK paths
- [x] Verify `settings.gradle.kts`, `build.gradle.kts`, and `libs.versions.toml`
- [x] Implement Application class and entry point (`MainActivity.kt`, `PdfcraftApplication.kt`)
- [x] Setup Material 3 Theme (`Theme.kt`, dynamic colors, typography, shapes)
- [x] Implement base navigation, Home Screen, and Viewer Screen shells
- [x] Build first Debug APK (`./gradlew assembleDebug`)
- [x] Verify APK artifact generation and package manifest (`app-debug.apk`: 63MB, `classes.dex`, `libpdfcraft_android.so`, `AndroidManifest.xml`)

---

## M2: Rust cdylib & UniFFI Native Bridge
- [x] Configure `apps/pdfcraft-android` crate (`Cargo.toml`, `src/lib.rs`, `src/sound.rs`)
- [x] Configure `apps/pdfcraft-android/bindgen` runner crate
- [x] Verify `uniffi.toml` package mapping for `io.github.dikshant363.pdfcraft.core`
- [x] Run host tests: `cargo test -p pdfcraft-android` (17/17 passed)
- [x] Build native libraries for `arm64-v8a` and `x86_64` via `cargo ndk`
- [x] Generate Kotlin UniFFI bindings from built `.so` (`pdfcraft_android.kt`)
- [x] Verify bridge initialization and safe resource management

---

## M3: Working PDF Reader Subsystem
- [x] Implement SAF document picker (`Intent.ACTION_OPEN_DOCUMENT`)
- [x] Implement working file caching in app-private sandbox
- [x] Implement `MobileSession.open()` and `DocumentInfo` extraction
- [x] Implement fit-scale page rasterization to `Bitmap` (`ARGB_8888`)
- [x] Implement 512px tile rasterization for zoomed viewports
- [x] Implement gesture controller (pinch-to-zoom, fling, double-tap zoom)
- [x] Implement continuous vertical scrolling and single-page reading modes
- [x] Implement LRU bitmap cache (`PageImages.kt`) with memory budgeting
- [x] Implement text selection layer and clipboard copy
- [x] Implement in-document search with hit navigation
- [x] Implement Table of Contents (Outline) and user bookmarks

---

## M4: Editing & Page Management Subsystem
- [x] Implement annotation tools: highlight, underline, strikethrough
- [x] Implement free-hand ink drawing with `ShapeSnap` smoothing
- [x] Implement text comments, sticky notes, and voice recordings
- [x] Implement AcroForm interactive form filling (text, check, radio, combo)
- [x] Implement Fill & Sign tools (text stamps, drawn/imported signatures)
- [x] Implement page operations: rotate, reorder, delete, insert, extract
- [x] Implement document creation and combining/merging
- [x] Implement transactional Undo / Redo engine integration
- [x] Implement atomic incremental save-back to SAF content URIs

---

## M5: Complete Product Features & Security Hardening
- [x] Implement offline OCR text recognition (`rten` models in `assets/models/`)
- [x] Implement PKCS#12 digital certificate inspection and CMS signing
- [x] Implement Read Aloud via `TextToSpeech` synced to reading blocks
- [x] Verify zero network permissions in `AndroidManifest.xml` (no `INTERNET` permission)
- [x] Verify path traversal protection and secure temporary file sanitization
- [x] Verify memory budget stability under OOM stress tests
- [x] Implement unit and integration tests in `android/app/src/test` (54/54 passed)

---

## M6: CI/CD, Documentation & Release Sign-Off
- [x] Generate `docs/android/BUILD.md`
- [x] Generate `docs/android/SECURITY.md`
- [x] Generate `docs/android/TESTING.md`
- [x] Generate `docs/android/UPSTREAM_SYNC.md`
- [x] Create GitHub Actions CI workflow (`.github/workflows/android.yml`)
- [x] Verify multi-ABI Debug APK assembly (`arm64-v8a` + `x86_64`)
- [x] Validate asset licensing policy (`cargo xtask assets`: 323 repo assets, 0 violations)
- [x] Document final release audit and engineering sign-off
