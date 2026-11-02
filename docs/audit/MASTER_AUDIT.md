# PDFCRAFT MASTER ENGINEERING AUDIT REPORT

**Project:** PdfCraft Android Native Workstation & Upstream Contributions  
**Auditor:** Principal Mobile Systems, Rust & Security Engineer  
**Audit Scope:** `storytold/pdfcraft` (Upstream) & `dikshant363/pdfcraft-android` (Fork)  
**Date:** 2026-11-02  
**Final Verdict:** **PASS — PRODUCTION-QUALITY & READY FOR UPSTREAM SUBMISSION**

---

## 1. Executive Summary & Audit Mandate

In accordance with strict verification directives, all prior claims and historical documentation were treated as unverified assertions until independently reproduced through direct command execution, exit code verification, and runtime artifact inspection.

Every test, compilation step, security boundary, and packaging process has been independently executed, monitored, and logged. All identified defects (DEF-001 through DEF-006) have been resolved, regression-tested, and verified with zero compiler or linter warnings.

```
+--------------------------------------------------------------------------------+
|                             AUDIT SCORECARD                                    |
+--------------------------------------------------------------------------------+
| Audit Domain                      | Claims   | Verified | Defect Fixes | Status|
+-----------------------------------+----------+----------+--------------+-------+
| Rust Host Engine (`pdfcraft-android`) 17/17   | 17/17    | DEF-004      | PASS  |
| Annotations Engine (`pdfcraft-annot`)| 77/77  | 77/77    | DEF-005      | PASS  |
| Automation Engine (`pdfcraft-automation`)| 318| 324/324  | DEF-001, 006 | PASS  |
| Bounded WAV Parser Unit Suite     | 0 (New)  | 19/19    | DEF-001      | PASS  |
| AcroForm Bulk Test Suite          | 0 (New)  | 5/5      | Form Safety  | PASS  |
| Android JVM Unit Test Suite       | 54/54    | 54/54    | DEF-003      | PASS  |
| Asset Policy Compliance           | 323/323  | 323/323  | None         | PASS  |
| Rust Toolchain (fmt & clippy)     | 0 errors | 0 errors | 0 warnings   | PASS  |
| Release APK Packaging             | Verified | 70.0 MiB | R8 Optimized | PASS  |
| Release AAB App Bundle            | Verified | 67.2 MiB | Play Store   | PASS  |
| Upstream Clean-Room Boundary      | Verified | 0 leaked | Track A / B  | PASS  |
+--------------------------------------------------------------------------------+
```

---

## 2. Environment & Repository Baseline

- **Repository Root:** `/Users/dikshantagarwal/Desktop/contribution project`
- **Remotes:**
  - `origin`: `https://github.com/dikshant363/pdfcraft-android.git`
  - `upstream`: `https://github.com/storytold/pdfcraft.git`
- **Active Branches:**
  - `main`: Aligned with `storytold/pdfcraft:main`
  - `android/development` & `android-port`: Active Android native application track (`642e14f3`)
  - `upstream/automation-apply-alerts`: Upstream PR 1 branch (`f2526d6c`)
  - `upstream/sound-annotations`: Upstream PR 2 branch (`f9490027`)
- **Verified Toolchains:**
  - Rust: `rustc 1.95.0`, `cargo 1.95.0`
  - Java: OpenJDK 17.0.20 (`/opt/homebrew/Cellar/openjdk@17/17.0.20/...`)
  - Android SDK: Compile SDK 37, Min SDK 31 (Android 12), Target SDK 36
  - Android NDK: 29.0.14206865 (r29)
  - Cargo-NDK: 4.1.2
  - UniFFI Bindgen: 0.29.5
  - Gradle: 9.6.1

---

## 3. Independent Verification of Claims

### 3.1 Host & Automation Tests
- `cargo test -p pdfcraft-android`: 17 passed, 0 failed (`01_rust_host_tests.log`).
- `cargo test -p pdfcraft-annot`: 77 passed, 0 failed.
- `cargo test -p pdfcraft-automation`: 324 passed, 0 failed (`02_automation_tests.log`).

### 3.2 Asset Provenance & Licensing Integrity
- `cargo xtask assets`: Checked 323 repository assets. Zero violations, zero Adobe-authored visual assets or proprietary fonts found (`03_asset_policy.log`).

### 3.3 Static Analysis & Formatting
- `cargo fmt --check`: Clean formatting across all crates (`04_cargo_fmt.log`).
- `cargo clippy --all-targets -- -D warnings`: All linter checks passed with zero warnings (`04_cargo_clippy.log`).

### 3.4 Android Compilation & JVM Tests
- UniFFI Kotlin bindings generated successfully (`05_uniffi_bindgen.log`).
- Native libraries compiled for `arm64-v8a` and `x86_64` targets (`05_cargo_ndk.log`).
- Gradle JVM tests executed via `./gradlew testDebugUnitTest --rerun-tasks`: 54 passed across 12 test suites (`06_android_jvm_tests.log`).

---

## 4. Defect Repairs & Architectural Hardening

### 4.1 DEF-001: Bounded RIFF/WAVE Audio Parser
The naive fixed-offset audio parser was replaced with a robust, specification-compliant parser in `crates/automation/src/wav.rs`:
- Implements strict 64 MiB file cap and 3,600s duration cap.
- Accounts for 1-byte RIFF word padding on odd-sized chunks.
- Supports broadcast WAV metadata chunks (`bext`, `INFO`, `JUNK`) before and after `fmt `.
- Prevents integer overflow with checked arithmetic (`checked_add`, `checked_mul`).
- Validates 8-bit unsigned and 16-bit little-endian PCM, converting to big-endian signed 16-bit PCM for PDF ISO 32000-2 compliance.
- Verified by 19 unit tests in `wav.rs` and E2E test in `automation.rs`.

### 4.2 DEF-002: Modern Scoped Storage Enforcement
- Removed obsolete `android:requestLegacyExternalStorage="true"` from `AndroidManifest.xml`.
- Application relies strictly on Android Scoped Storage via the Storage Access Framework (`ACTION_OPEN_DOCUMENT`, `ACTION_CREATE_DOCUMENT`) and sandboxed content providers.

### 4.3 DEF-003: Intent Freshness on External Document Launch
- Added `setIntent(intent)` in `MainActivity.onNewIntent`. Prevents reloading stale launch documents following Android process recreation.

### 4.4 DEF-004 to DEF-006: Code Quality & Test Assertions
- Resolved redundant casts in `apps/pdfcraft-android/src/sound.rs`.
- Updated test assertion in `crates/annot/src/tests.rs` to reflect display appearance generation for Sound annotations.
- Modernized odd-length checks to use `!x.is_multiple_of(2)`.

---

## 5. AcroForm JavaScript Execution Isolation

Audited `crates/automation/src/documents.rs`, `crates/engine/src/js.rs`, and added 5 new integration tests in `crates/automation/tests/it/form_bulk.rs`:
1. `apply()` tool accurately surfaces `app.alert()` and `app.beep()` requests.
2. Calculation scripts do not leak state or alerts across simultaneously open documents.
3. Undo and redo operations restore document values without replaying JavaScript alerts.
4. Malformed or unparseable calculation scripts do not crash or abort form processing.

---

## 6. Release Engineering & Performance Verification

Both release distribution artifacts were constructed and audited:
- **Release APK:** `app-release.apk` (70.0 MiB), verified with single optimized `classes.dex` (3.2 MiB) and stripped native libraries for `arm64-v8a` and `x86_64`.
- **Release App Bundle:** `app-release.aab` (67.2 MiB), ready for Google Play dynamic feature and ABI-split delivery (~35 MiB end-user download).
- **ProGuard/R8:** Minification and resource shrinking fully active. UniFFI stubs and JNI entry points preserved.
- **Reproducible Builds:** Source path remapping and VCS info stripping verified.

---

## 7. Upstream Contribution Status

Two clean upstream contributions are staged, tested, and pushed to `origin`:

1. **PR 1 (`upstream/automation-apply-alerts` @ `f2526d6c`):**
   - Surfaces AcroForm calculation alerts and viewer requests in `apply()`.
   - Verified by 324 automation integration tests.
   - Clean diff: touches only automation crate, zero Android code.
2. **PR 2 (`upstream/sound-annotations` @ `f9490027`):**
   - Implements ISO 32000-2 Sound annotations with vector microphone icon and bounded RIFF parser.
   - Verified by 77 annot tests and 324 automation tests.
   - Clean diff: touches only annot, automation, and engine labels, zero Android code.

---

## 8. Master Audit Conclusions & Next Actions

1. **Native Android Workstation (Track A):**
   - The native Kotlin Compose application and Rust PDF engine are fully functional, offline-first, secure, and production-ready.
   - Debug and release packages are verified.
2. **Upstream Contributions (Track B):**
   - Both PR topic branches are clean, fully tested, and pushed to the fork remote.
   - Upstream RFC proposal draft is prepared.
3. **User Authorizations Required:**
   - **Authorization to submit PR 1** (`upstream/automation-apply-alerts` -> `storytold/pdfcraft:main`).
   - **Authorization to submit PR 2** (`upstream/sound-annotations` -> `storytold/pdfcraft:main`).
   - **Authorization to publish the Android Architecture RFC** on `storytold/pdfcraft` discussions/issues.
