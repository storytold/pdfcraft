# PDFCRAFT DEFECT REGISTER

**Status:** ALL DEFECTS RESOLVED & VERIFIED  
**Audit Date:** 2026-11-02  
**Audit Scope:** `storytold/pdfcraft` upstream compatibility and `dikshant363/pdfcraft-android` native port  
**Auditor:** Principal Systems & Security Engineer

---

## 1. Summary of Registered Defects

| Defect ID | Severity | Subsystem | Title / Description | Status | Verification Commit |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **DEF-001** | **HIGH** | `sound` / `automation` | Naive fixed-offset WAV parser with missing padding, bounds, and overflow validation | **RESOLVED** | `f9490027` / `642e14f3` |
| **DEF-002** | **MEDIUM** | `android` / `manifest` | Obsolete `requestLegacyExternalStorage` attribute in AndroidManifest.xml (API 31+) | **RESOLVED** | `3cb894fb` |
| **DEF-003** | **MEDIUM** | `android` / `lifecycle` | Stale intent retention on external document launch in `MainActivity.onNewIntent` | **RESOLVED** | `3cb894fb` |
| **DEF-004** | **LOW** | `pdfcraft-android` | Clippy redundant cast `*f as f32` on existing `f32` in sound stream decoding | **RESOLVED** | `3cb894fb` |
| **DEF-005** | **LOW** | `annot` / `tests` | Outdated assertion in `comments_without_appearances_get_one_for_display_only` | **RESOLVED** | `642e14f3` |
| **DEF-006** | **LOW** | `automation` / `clippy` | Rust 1.97 `is_multiple_of` clippy lint in RIFF padding validation | **RESOLVED** | `642e14f3` |

---

## 2. Detailed Defect Reports & Remediations

### DEF-001: Naive Fixed-Offset WAV Parser Vulnerability
- **Severity:** HIGH (CWE-125, CWE-128, CWE-190)
- **Subsystems Affected:** `crates/automation/src/comments.rs`, `apps/pdfcraft-android/src/sound.rs`
- **Root Cause:**
  The initial sound annotation implementation assumed standard 44-byte WAV headers with fixed byte offsets (e.g. expecting `fmt ` at offset 12 and exactly 16 bytes). In real-world audio recording and exchange:
  1. Broadcast WAV (BWF) files place metadata chunks (such as `bext`, `INFO`, `JUNK`) before or between `fmt ` and `data`.
  2. The RIFF/WAVE specification mandates that any chunk with an odd payload size must be followed by a 1-byte pad (`\0`) so that chunk headers align to 2-byte word boundaries. The previous code did not account for word padding, throwing off subsequent chunk offsets.
  3. Chunk lengths and total byte offsets were read without checked arithmetic, vulnerable to integer overflow wrapping when parsing maliciously crafted files.
  4. Truncated chunk headers and payloads were either parsed out-of-bounds or triggered index panics.
  5. There was no cap on uncompressed decoded sample duration, allowing potential denial of service via memory exhaustion.
- **Reproduction:**
  Supplying a WAV file with a `bext` metadata chunk before `fmt `, or an odd-sized metadata chunk, resulted in immediate `not a PCM WAV` rejection or misaligned sample playback.
- **Remediation:**
  Created `crates/automation/src/wav.rs` (661 lines) implementing a strict, bounded RIFF/WAVE parser:
  - Validates `RIFF` and `WAVE` 4-character ID signatures.
  - Implements stateful chunk iteration using `read_u32` and `checked_add` with explicit bounds against the input slice.
  - Correctly advances over 1-byte word padding when `chunk_len` is odd (`chunk_len % 2 != 0`).
  - Caps maximum input file size to 64 MiB (`MAX_WAV_FILE_SIZE`).
  - Caps maximum decoded audio duration to 3600 seconds (1 hour, `MAX_AUDIO_DURATION_SECONDS`).
  - Enforces sample rates strictly within 4 kHz to 96 kHz.
  - Rejects duplicate `fmt ` or `data` chunks deterministically.
  - Rejects unsupported compression (only uncompressed linear PCM `0x0001` accepted).
  - Converts 8-bit unsigned PCM and 16-bit little-endian PCM into big-endian signed 16-bit PCM per PDF ISO 32000-2 §13.3 (`/E /Signed /B 16`).
- **Verification:**
  19 unit tests in `crates/automation/src/wav.rs` covering all corner cases (odd-chunk padding, pre/post metadata, missing fmt/data, duplicates, overflow attempts, fuzzed inputs) and 1 end-to-end integration test in `crates/automation/tests/it/automation.rs`. Exit code: 0.

---

### DEF-002: Obsolete Legacy Storage Request in Manifest
- **Severity:** MEDIUM (Android Platform Hygiene & Security Best Practices)
- **Subsystem:** `apps/pdfcraft-android/android/app/src/main/AndroidManifest.xml`
- **Root Cause:**
  `android:requestLegacyExternalStorage="true"` was included in the application element.
  `minSdk = 31` (Android 12). Google Play and Android 11+ completely ignore `requestLegacyExternalStorage` for applications targeting API 30+. Retaining it creates a false impression of relying on deprecated filesystem access and triggers linter warnings.
- **Remediation:**
  Removed `android:requestLegacyExternalStorage="true"`. Documented in `SECURITY_REVIEW.md` that PdfCraft Android exclusively accesses files through the Storage Access Framework (`ACTION_OPEN_DOCUMENT`, `ACTION_CREATE_DOCUMENT`, and `content://` Uris resolved via `ContentResolver.openFileDescriptor`).
- **Verification:**
  APK manifest verified via `aapt dump xmltree`. Build exit code: 0.

---

### DEF-003: Stale Intent Retention on External Document Launch
- **Severity:** MEDIUM (Application State & Document Lifecycle Integrity)
- **Subsystem:** `apps/pdfcraft-android/android/app/src/main/java/io/github/dikshant363/pdfcraft/MainActivity.kt`
- **Root Cause:**
  `MainActivity` handles document launch intents from system file managers. When the activity is alive in the background and a new document is launched from an external app, Android invokes `onNewIntent(intent: Intent)`.
  The original implementation called `openFromIntent(intent)` but omitted `setIntent(intent)`. Consequently, `activity.intent` retained the initial document intent. If the activity was subsequently destroyed and recreated (e.g. process death, configuration change), the activity reloaded the stale first document rather than the most recently requested one.
- **Remediation:**
  Added `setIntent(intent)` at line 156 of `MainActivity.kt`.
- **Verification:**
  Android JVM unit test suite `./gradlew testDebugUnitTest` passed (54 tests, exit code 0).

---

### DEF-004: Clippy Redundant Cast in Sound Stream Decoding
- **Severity:** LOW (Code Quality & Zero-Warning Gate)
- **Subsystem:** `apps/pdfcraft-android/src/sound.rs`
- **Root Cause:**
  In `sound.rs:41` and `sound.rs:44`, `Object::Real(f)` was matched and cast as `*f as f32`. In the `lopdf` crate, `Object::Real` contains `f32`. Casting `f32` as `f32` triggered Clippy's `unnecessary_cast` lint.
- **Remediation:**
  Replaced `*f as f32` with `*f`.
- **Verification:**
  `cargo clippy -p pdfcraft-android` passed with 0 warnings. Exit code: 0.

---

### DEF-005: Outdated Display Appearance Assertion for Sound Annotations
- **Severity:** LOW (Test Suite Correctness)
- **Subsystem:** `crates/annot/src/tests.rs`
- **Root Cause:**
  The test `comments_without_appearances_get_one_for_display_only` tests `with_missing_appearances`. In `origin/main`, Sound annotations had no appearance generator, so the test asserted that the 7th annotation (`Subtype /Sound`) did not receive an appearance (`[true, true, true, true, false, false, false]`).
  With the implementation of `feat(annot): implement sound annotations per ISO 32000-2`, Sound annotations now implement `build_appearance`, generating a microphone vector icon for on-screen display. The test assertion was stale, failing with `left != right`.
- **Remediation:**
  Updated the assertion in `crates/annot/src/tests.rs:1020, 1026` to `[true, true, true, true, false, false, true]`, correctly verifying that Sound annotations receive display appearances.
- **Verification:**
  `cargo test -p pdfcraft-annot` -> 77 passed, 0 failed. Exit code: 0.

---

### DEF-006: Clippy `is_multiple_of` Lints in RIFF Parsing
- **Severity:** LOW (Rust 1.97 Toolchain Compatibility)
- **Subsystem:** `crates/automation/src/wav.rs`, `crates/automation/tests/it/automation.rs`
- **Root Cause:**
  `pcm.len() % 2 != 0` triggered Clippy's `manual_is_multiple_of` lint on newer toolchains.
- **Remediation:**
  Replaced with `!pcm.len().is_multiple_of(2)`.
- **Verification:**
  `cargo clippy -p pdfcraft-automation --tests` passed with 0 warnings. Exit code: 0.
