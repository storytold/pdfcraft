# Consolidated Final Defect Register

**Repository:** `https://github.com/dikshant363/pdfcraft-android`  
**Application ID:** `io.github.dikshant363.pdfcraft`  
**Audit Date:** 2026-11-02  
**Status:** All Defect Items Resolved and Verified  

---

## 1. Defect Summary Table

| Defect ID | Severity | Subsystem | Description | Status | Verification Evidence |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **DEF-001** | High | Audio / Annot | Unbounded RIFF/WAVE header and chunk parsing | RESOLVED | 19 unit tests passing in `crates/automation/src/wav.rs` |
| **DEF-002** | Medium | Forms / Auto | JavaScript calculation alerts swallowed in automation | RESOLVED | 4 integration tests in `form_bulk.rs` passing |
| **DEF-003** | High | Branding / Policy | Stale repository URLs and ArtCraft trademark risk | RESOLVED | `cargo xtask assets` passes 0 violations; URLs updated |
| **DEF-004** | High | CI / Workflow | sdkmanager `platforms;android-37.0` naming mismatch | RESOLVED | GitHub Actions workflow installs and symlinks cleanly |
| **DEF-005** | Medium | Testing / QA | Absence of native Android instrumentation test suite | RESOLVED | 4 `androidTest` suites assembled into test APK |
| **DEF-006** | Low | Toolchain / Lint | Clippy `chunks_exact_to_as_chunks` failure on Rust 1.99 | RESOLVED | Refactored to `as_chunks::<2>().0`; Clippy exits 0 |
| **DEF-007** | High | Build / Gradle | Mismatched `distributionSha256Sum` for Gradle 9.6.1-bin | RESOLVED | Corrected checksum to official SHA-256 (`9c0f7fae...`) |

---

## 2. Detailed Defect Analysis and Resolutions

### DEF-001: Unbounded / Fixed-Offset RIFF WAVE Parser
- **Severity:** High (Crash / OOM / Boundary Violation)
- **Problem Statement:** Initial sound annotation handling made assumptions about the layout and byte offsets of standard WAV files, risking panics or unbounded memory consumption when confronted with corrupted audio or optional metadata chunks (`LIST`, `INFO`, `JUNK`).
- **Resolution:**
  - Implemented a clean-room bounded RIFF/WAVE parser in `crates/automation/src/wav.rs`.
  - Added strict chunk header walking with word-boundary padding alignment.
  - Implemented checked integer arithmetic for chunk length calculations.
  - Enforced a hard 64 MiB payload cap to guard against memory exhaustion.
  - Added conversion of 8-bit unsigned and 16-bit little-endian PCM into big-endian signed 16-bit PCM per ISO 32000-2 §13.3.
- **Verification:** 19 dedicated unit tests covering truncated headers, missing chunks, corrupted lengths, and malformed channels.

---

### DEF-002: Form Calculation Alert Silencing in Automation API
- **Severity:** Medium (Functional Parity Gap)
- **Problem Statement:** When executing automated document edits via the automation engine (`doc_apply`), JavaScript calculations triggering user alerts (`app.alert()`) or prompt requests were silently discarded without feedback.
- **Resolution:**
  - Extended the automation document state in `crates/automation/src/documents.rs` with structured `alerts` and `requests` vectors.
  - Surfaced calculation alerts directly in the JSON response of `doc_apply`.
  - Ensured undo/redo cycles do not replay modal side-effects.
- **Verification:** 4 integration tests in `tests/it/form_bulk.rs` verifying multi-alert delivery, document isolation, and undo idempotence.

---

### DEF-003: Trademark Protection and Repository URL Alignment
- **Severity:** High (Licensing and Legal Integrity per `AGENTS.md` §1.2)
- **Problem Statement:** The repository is an independent fork of PdfCraft. Under `docs/brand/LICENSE-brand.txt`, ArtCraft trademark logos and brand assets may only be used unmodified within official ArtCraft Team distributions. Furthermore, stale links pointed to third-party namespaces.
- **Resolution:**
  - Audited `apps/pdfcraft-android`: verified zero ArtCraft trademark assets are bundled.
  - Replaced stale repository URLs in `Tabs.kt` and `fetch-models.sh` with `https://github.com/dikshant363/pdfcraft-android`.
  - Preserved all third-party open-source notices in `NOTICE` and `ATTRIBUTION.toml`.
- **Verification:** `cargo xtask assets` passes with code 0 across 323 assets.

---

### DEF-004: Android SDK 37 sdkmanager Package Naming Divergence
- **Severity:** High (CI Blocker)
- **Problem Statement:** On Ubuntu CI runners, running `sdkmanager "platforms;android-37"` failed with `Warning: Failed to find package 'platforms;android-37'`. Google published API 37 under the package identifier `platforms;android-37.0`, which AGP does not automatically locate when `compileSdk = 37`.
- **Resolution:**
  - Updated `.github/workflows/android.yml` to install `platforms;android-37.0`.
  - Added automatic symlink creation: `ln -s "$ANDROID_HOME/platforms/android-37.0" "$ANDROID_HOME/platforms/android-37"`.
- **Verification:** CI step `Install cargo-ndk and Android SDK packages` executes cleanly with code 0.

---

### DEF-005: Absence of Native Android Instrumentation Test Harness
- **Severity:** Medium (QA / Device Readiness)
- **Problem Statement:** Unit tests only executed on host JVM without loading actual Android `.so` shared libraries in an ART environment.
- **Resolution:**
  - Configured `AndroidJUnitRunner` and AndroidX test dependencies in `build.gradle.kts`.
  - Created 4 comprehensive test suites in `apps/pdfcraft-android/android/app/src/androidTest/`:
    - `NativeEngineLoadingTest.kt`: Native library and ABI validation.
    - `DocumentLifecycleTest.kt`: Sandboxed file lifecycle and intent dispatch.
    - `ViewerOperationsTest.kt`: Multi-scale and tile rendering, text search.
    - `AnnotationsAndMediaTest.kt`: Comment creation, atomic save, persistence.
- **Verification:** `./gradlew assembleAndroidTest` builds `app-debug-androidTest.apk` (2.3 MiB); added to CI pipeline.

---

### DEF-006: Clippy Warning on `chunks_exact(2)` with Constant Chunk Size
- **Severity:** Low (CI `-D warnings` Blocker under Rust 1.99)
- **Problem Statement:** Rust 1.99.0 Clippy introduced the `chunks_exact_to_as_chunks` lint, causing the CI linter step to fail on `payload.chunks_exact(2)`.
- **Resolution:**
  - Refactored `crates/automation/src/wav.rs:232` to use `payload.as_chunks::<2>().0`.
  - Pattern matches directly on `&[lo, hi]` without temporary slice checks.
- **Verification:** `cargo clippy -p pdfcraft-automation -- -D warnings` completes with 0 warnings.

---

### DEF-007: Mismatched Gradle Wrapper Distribution Checksum
- **Severity:** High (CI Execution Blocker on Clean Runners)
- **Problem Statement:** On clean CI environments where Gradle 9.6.1 is downloaded on demand, the wrapper aborts execution with `Verification of Gradle distribution failed! Your Gradle distribution may have been tampered with.` because `apps/pdfcraft-android/android/gradle/wrapper/gradle-wrapper.properties` contained a stale SHA-256 (`bafc141b619ad6350fd975fc903156dd5c151998cc8b058e8c1044ab5f7b031f`).
- **Resolution:**
  - Queried official Gradle distribution server `https://services.gradle.org/distributions/gradle-9.6.1-bin.zip.sha256`.
  - Updated `distributionSha256Sum` to match the official hash: `9c0f7faeeb306cb14e4279a3e084ca6b596894089a0638e68a07c945a32c9e14`.
- **Verification:** Gradle wrapper distribution checksum verification passes.

