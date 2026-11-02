# PdfCraft Android Native Port — Repository Audit

**Document:** `docs/android/REPOSITORY_AUDIT.md`  
**Date:** November 2026  
**Auditor:** Principal Mobile Systems Architect & Senior Rust Engineer  
**Upstream Version:** PdfCraft v0.17.0 (`storytold/pdfcraft` @ commit `0cba87ae`)  
**Target Package:** `io.github.dikshant363.pdfcraft`  
**Target Repo:** `https://github.com/dikshant363/pdfcraft-android`  
**Integration Branch:** `android-port`

---

## 1. Executive Summary

PdfCraft is an open-source, multi-crate Rust workspace implementing an advanced PDF engine, rendering pipeline, digital signature framework, forms engine, and automation tool table. The project's desktop interface is built using `egui` and `eframe`.

This audit evaluates the feasibility, architectural boundaries, dependency chains, and platform requirements for porting PdfCraft to a fully native Android application using Kotlin, Jetpack Compose, Material 3, and a high-performance native Rust cdylib bridge driven via UniFFI.

A prior community reference implementation (`artcraft-android/pdfcraft-android`) demonstrated the viability of embedding the PdfCraft engine via `pdfcraft_automation::Automation` into a native Compose touch UI, targeting Android 10+ (API 29+). This port builds upon those architectural foundations while targeting Android 12+ (API 31+), incorporating upstream v0.17.0 engine improvements, updating the application ID to `io.github.dikshant363.pdfcraft`, hardening offline security and SAF integration, and establishing reproducible CI/CD pipelines.

---

## 2. Actual Repository Structure

The local workspace is configured with Cargo resolver 3 and Rust 2024 edition requirements:

```text
contribution project/ (pdfcraft-android root)
├── Cargo.toml                  # Workspace root (version 0.17.0, Rust edition 2024, MSRV 1.95)
├── Cargo.lock                  # Pinned dependency graph
├── crates/                     # 33 independent engine & support crates
│   ├── a11y/                   # PDF/UA accessibility, auto-tagging, content order
│   ├── annot/                  # Annotation appearance streams, AP streams, geometry
│   ├── audit/                  # PDF syntax and structure validation
│   ├── automation/             # Headless automation tool table (comments, forms, pages, etc.)
│   ├── compare/                # Visual and textual document comparison
│   ├── content/                # Content streams and graphics operators
│   ├── cos/                    # Low-level COS object model, parser, serializer, encryption
│   ├── create/                 # Document generation from images, text, and office files
│   ├── crypt/                  # Standard security handler, AES-256, RC4, MD5, SHA-256
│   ├── edit/                   # In-place text editing, flattening, content addition
│   ├── engine/                 # Central facade, Document lifecycle, catalog, fonts, commands
│   ├── export/                 # Export to images, Word, Excel, PowerPoint, RTF
│   ├── filters/                # Flate, LZW, ASCII85, ASCIIHex, CCITT Group 4, RunLength
│   ├── fonts/                  # TrueType, CFF, Type1, Standard 14, CJK fallback fonts
│   ├── forms/                  # AcroForm parser, widget calculations, field values
│   ├── geom/                   # 2D geometry primitives, affine transforms, bezier paths
│   ├── js/                     # Acrobat JavaScript engine (boa-based)
│   ├── measure/                # Dimensioning and distance measurement tools
│   ├── model/                  # High-level DOM representation of PDF structures
│   ├── ocr/                    # Optical character recognition (rten / ocrs)
│   ├── optimize/               # Stream compression, font deduplication, image downsampling
│   ├── oracle/                 # Differential testing against reference implementations
│   ├── organize/               # Page manipulation (rotate, extract, delete, insert, reorder)
│   ├── platform/               # OS integration primitives
│   ├── preflight/              # PDF/A and PDF/X conformance verification
│   ├── print/                  # System spooling and document pagination
│   ├── redact/                 # True visual and content stream redaction
│   ├── render/                 # High-performance rasterizer (hayro / vello_cpu)
│   ├── sign/                   # PKCS#7 / CMS digital signatures, PAdES, DSS, validation
│   ├── testkit/                # Shared testing utilities and fixtures
│   ├── ui-egui/                # Desktop UI (egui) — EXCLUDED from Android build
│   ├── xfa/                    # XML Forms Architecture template parser and layout
│   └── xfdf/                   # XFDF annotation and form data exchange
├── apps/
│   ├── pdfcraft/               # Desktop application binary (egui + eframe)
│   ├── pdfcraft-cli/           # CLI tool and MCP server
│   ├── pdfcraft-web/           # WebAssembly build
│   └── pdfcraft-android/       # Native Android application & Rust bridge
│       ├── Cargo.toml          # Rust cdylib crate (pdfcraft_android)
│       ├── uniffi.toml         # UniFFI Kotlin bindings definition
│       ├── bindgen/            # uniffi-bindgen runner crate
│       ├── src/                # Rust bridge source (MobileSession, types)
│       └── android/            # Gradle Android Studio project (Compose + Material 3)
├── assets/                     # Shared fonts (Inter, DancingScript), icons (Lucide), models
├── vendor/                     # Patched vendored crates (hayro, vello_cpu, lopdf, etc.)
└── docs/                       # Project documentation & architecture specs
```

---

## 3. Existing Capabilities Audit

| Subsystem | Existing Implementation | Android Readiness | Notes |
|---|---|---|---|
| **PDF Parsing & COS** | `crates/cos`, `vendor/lopdf` | **100% Ready** | Pure Rust, zero platform-specific C dependencies. |
| **Rasterization** | `crates/render`, `vendor/hayro` | **100% Ready** | Pure CPU rendering to premultiplied RGBA8 (`ARGB_8888`), directly blitted to Android `Bitmap`. |
| **Automation Tools** | `crates/automation` | **100% Ready** | Headless serial dispatcher executing atomic, undoable tools (`page_rotate`, `comment_add`, `form_fill`, etc.). |
| **Annotations** | `crates/annot` | **100% Ready** | Full AP appearance stream generation, highlight, underline, ink, free text, shapes. |
| **Form Filling** | `crates/forms` | **100% Ready** | Text fields, checkboxes, radios, push buttons, choice lists; JavaScript support via `crates/js`. |
| **Digital Signatures** | `crates/sign` | **100% Ready** | PKCS#12 (.p12/.pfx) reading, CMS signing, timestamping, SHA-256 fingerprinting. |
| **Text Layer** | `crates/render/src/text.rs` | **100% Ready** | Glyph spatial indexing, selection bounding rects, hit testing, word boundaries, text extraction. |
| **OCR & Text Recognition**| `crates/ocr` | **Ready** | Bundled offline `rten` models (`assets/models/`), loads purely offline on mobile CPU. |
| **Desktop UI (`egui`)** | `crates/ui-egui`, `apps/pdfcraft` | **Excluded** | Desktop-only egui code is cleanly excluded from the Android mobile build. |

---

## 4. Platform Incompatibilities & Resolutions

1. **Desktop GUI (`eframe`/`egui`)**:
   - *Issue*: `eframe` requires winit/X11/Wayland/Cocoa/Win32.
   - *Resolution*: Strictly decoupled. The Android application builds `apps/pdfcraft-android` as a standalone `cdylib` without pulling in `ui-egui` or `eframe`.

2. **Logging**:
   - *Issue*: Desktop logging uses standard `env_logger`/stdout. Android requires `__android_log_print` via `logcat`.
   - *Resolution*: Under `cfg(target_os = "android")`, `android_logger` crate (0.15) captures all `log::*` macros with tag `PdfCraft`.

3. **Memory Constrained Devices**:
   - *Issue*: Desktop workstations have abundant RAM; decoding entire high-res documents or massive page bitmaps on mobile triggers Linux OOM killer.
   - *Resolution*: Multi-level tiling pipeline (512x512 device pixels at octave steps), visible-page LRU tile cache with memory budget, and rapid request cancellation when flinging.

4. **Storage Access**:
   - *Issue*: Android 11+ enforces Scoped Storage; arbitrary POSIX paths cannot be opened by standard file APIs.
   - *Resolution*: Kotlin layer opens `content://` URIs via `ContentResolver`, stages to private cache directory, passes private working file path to Rust engine, and performs atomic write-back via `ParcelFileDescriptor` upon save.

---

## 5. Toolchain & Environment Verification

Environment audit performed on development host (macOS Sonoma / Apple Silicon):

- **Rust Host Toolchain**: `rustc 1.97.1`, `cargo 1.97.1`
- **Rust Android Targets**:
  - `aarch64-linux-android` (Installed)
  - `x86_64-linux-android` (Installed)
- **Cargo NDK**: `cargo-ndk 4.1.2` installed at `~/.cargo/bin/cargo-ndk`
- **Java Development Kit**: OpenJDK 17.0.20 (`/opt/homebrew/Cellar/openjdk@17/17.0.20/...`)
- **Android SDK**: `/opt/homebrew/share/android-commandlinetools`
- **Android Platforms**: `android-35` installed
- **Android Build-Tools**: `35.0.0` and `34.0.0` installed
- **Android NDK**: `29.0.14206865` installed at `$ANDROID_HOME/ndk/29.0.14206865`
- **ADB**: `/opt/homebrew/bin/adb` (Android Debug Bridge 1.0.41)

All toolchain requirements are met without missing dependencies.

---

## 6. Risks & Mitigation Plan

| Risk | Impact | Mitigation Strategy |
|---|---|---|
| **UniFFI Binding Version Mismatch** | App crash at load time (`UnsatisfiedLinkError` / version hash mismatch) | Pin `uniffi` in `pdfcraft-android` and `pdfcraft-android-bindgen` to identical release (`=0.32.2`), generating bindings during Gradle build directly from target `.so`. |
| **Rust Panics Unwinding into JVM** | Immediate SIGABRT / process crash | Set `#![forbid(unsafe_code)]`, eliminate `unwrap()` in production paths, catch errors with typed `MobileError`, and wrap entrypoints in `std::panic::catch_unwind`. |
| **Main Thread Blocking** | Android ANR (Application Not Responding) | Strictly run all `MobileSession` calls on Kotlin Coroutine dispatchers (`Dispatchers.Default` for CPU rendering, `Dispatchers.IO` for file I/O). |
| **Memory Exhaustion on Large PDFs** | Out of Memory crash | Memory-budgeted LRU cache in `PageImages.kt`, tile-based rendering for zoom levels > fit scale, discard off-screen tiles. |
| **Data Loss on Backgrounding** | Merchant / user edits lost | Auto-save on `ON_STOP` lifecycle events via Android Storage Access Framework with atomic temporary file swapping. |

---

## 7. Recommended Integration Architecture

```text
Kotlin Jetpack Compose UI (Material 3)
   │
   ▼
ViewModel & StateFlow (Coroutines / Asynchronous Dispatchers)
   │
   ▼
UniFFI Kotlin Bindings (io.github.dikshant363.pdfcraft.core)
   │  (JNA / Direct C-ABI dynamic library link)
   ▼
Rust cdylib (libpdfcraft_android.so)
   │
   ├── MobileSession (Thread-safe Mutex wrapper)
   ├── Automation Dispatcher (pdfcraft-automation)
   ├── Renderer & Text Pipeline (pdfcraft-render)
   └── Document COS & Engine (pdfcraft-engine)
```

The audit confirms full feasibility. Proceeding to architectural formalization and implementation.
