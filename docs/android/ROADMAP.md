# PdfCraft Android Native Port — Execution Roadmap

**Document:** `docs/android/ROADMAP.md`  
**Date:** November 2026  
**Status:** Canonical Implementation Roadmap  
**Target Package:** `io.github.dikshant363.pdfcraft`  
**Fork Strategy:** Independent Fork (`https://github.com/dikshant363/pdfcraft-android`)  
**Target Branch:** `android-port`  

---

## Milestone M0: Repository & Toolchain Audit (Complete)
- [x] Full filesystem, Git, and Rust workspace audit.
- [x] Cross-analysis between upstream `pdfcraft` (v0.17.0) and reference `artcraft-android` (v0.14.0).
- [x] Complete toolchain provisioning: OpenJDK 17, Android SDK 35, Build-tools 35.0.0, Android NDK 29.0.14206865, cargo-ndk 4.1.2, Rust targets (`aarch64-linux-android`, `x86_64-linux-android`).
- [x] Generation of `REPOSITORY_AUDIT.md` and `ARCHITECTURE.md`.

---

## Milestone M1: Core Android Application Setup & Verification
- [ ] Establish package identity `io.github.dikshant363.pdfcraft` across `settings.gradle.kts`, `build.gradle.kts`, `AndroidManifest.xml`, and Kotlin source trees.
- [ ] Configure Android Gradle Plugin (AGP 9.2.x), Kotlin (2.3.x / Compose Compiler), Material 3, and SDK targets (`minSdk = 31`, `targetSdk = 36`, `compileSdk = 37`).
- [ ] Wire application entry points: `PdfcraftApplication`, `MainActivity`, `AppRoot`, `AppViewModel`.
- [ ] Material 3 Theme setup: dynamic color palettes on Android 12+, crisp fallback dark and light themes, typography with Inter font, edge-to-edge drawing.
- [ ] Build first APK shell and verify launchability and resource inflation.

---

## Milestone M2: Rust cdylib & UniFFI Bridge Compilation
- [ ] Configure `apps/pdfcraft-android` crate with `crate-type = ["cdylib", "lib"]`.
- [ ] Synchronize `uniffi.toml` for `io.github.dikshant363.pdfcraft.core`.
- [ ] Configure `apps/pdfcraft-android/bindgen` with `uniffi = "=0.32.2"`.
- [ ] Verify `cargo check -p pdfcraft-android` and host unit tests (`cargo test -p pdfcraft-android`).
- [ ] Implement Gradle tasks `CargoNdkBuild` and `UniffiBindgen` for `arm64-v8a` and `x86_64`.
- [ ] Verify native library loading (`System.loadLibrary("pdfcraft_android")`) and `engine_version()` invocation.

---

## Milestone M3: High-Performance Native PDF Reader
- [ ] Storage Access Framework (SAF) integration: opening PDFs via `Intent.ACTION_OPEN_DOCUMENT` and `content://` URIs.
- [ ] Native rendering pipeline: fit-screen rendering + dynamic 512px tile rasterization with `hayro`.
- [ ] Viewport gesture handling: smooth pinch-to-zoom, double-tap zoom, fling scrolling in continuous vertical layout and single-page layout.
- [ ] Memory-budgeted LRU bitmap cache (`PageImages`) with instant cancellation of off-screen render requests.
- [ ] Text layer indexing: hit testing, text selection with native handles, full-document search with highlighted result rectangles.
- [ ] Document outline & table of contents (`OutlineEntry`), bookmarks, and page thumbnails sidebar.
- [ ] Night mode (inverted rendering) and Reading mode (distraction-free single column).

---

## Milestone M4: Document Editing & Page Management
- [ ] Annotation system: highlight, underline, strikethrough, sticky notes, free-hand ink with shape smoothing (`ShapeSnap`), free-text comments.
- [ ] AcroForm filling: interactive text inputs, checkboxes, radio groups, choice dropdowns, JavaScript calculation execution.
- [ ] Fill & Sign: quick text placement, checkmarks/crossmarks, vector signature drawing with persistent local signature storage.
- [ ] Page organization: page rotation (90°, 180°, 270°), reordering, deletion, extraction, and insertion.
- [ ] Document operations: PDF compression/optimization, password encryption, merging/combining multiple files, splitting.
- [ ] Transactional undo/redo stack reflecting Rust engine state.
- [ ] Incremental save-back to original `content://` URI or Save As to user-selected destination.

---

## Milestone M5: Complete Offline Product Hardening
- [ ] Optical Character Recognition (OCR): offline text recognition using bundled `rten` models (`assets/models/`).
- [ ] Digital ID (.p12 / .pfx) certificate inspection and CMS digital signing.
- [ ] Read Aloud accessibility engine via Android `TextToSpeech` synced to `ReadingBlock` glyph positions.
- [ ] Comprehensive edge-case verification: corrupt PDFs, password-protected PDFs, massive scanned documents, extreme zoom levels.
- [ ] Memory profiling and leak verification across rapid activity recreation and orientation changes.

---

## Milestone M6: Production CI/CD & Release Deliverables
- [ ] GitHub Actions workflows for Rust formatting, Clippy, Android unit tests, and Gradle assemble.
- [ ] Release APK and Android App Bundle (AAB) builds with ProGuard / R8 code shrinking and resource optimization.
- [ ] Cryptographic SHA-256 checksums and release metadata generation.
- [ ] Complete documentation: `BUILD.md`, `SECURITY.md`, `TESTING.md`, `UPSTREAM_SYNC.md`.
