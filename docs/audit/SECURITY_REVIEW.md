# PDFCRAFT SECURITY AUDIT & THREAT MODEL

**Target:** `storytold/pdfcraft` & `dikshant363/pdfcraft-android`  
**Security Posture:** Hardened Local-First Offline PDF Workstation  
**Date:** 2026-11-02  
**Status:** PASSED (Zero Critical Vulnerabilities, 100% Offline, Strict Memory Bounds)

---

## 1. Threat Modeling Overview

PdfCraft operates directly on untrusted binary documents supplied by arbitrary third parties. The security model must assume that every opened PDF, embedded audio stream, font table, and AcroForm JavaScript script is potentially hostile.

```
+--------------------------------------------------------------------------------+
|                               ATTACK SURFACES                                  |
+--------------------------------------------------------------------------------+
| 1. Malicious PDF Files       --> Parser crashes, OOM, stack exhaustion, loops |
| 2. Audio/WAV Payloads        --> Heap overflow, integer wrapping, DoS memory   |
| 3. AcroForm Calculation JS   --> Sandbox escape, infinite loop, data leakage   |
| 4. Content Provider / SAF    --> Path traversal, unauthorized file overwrites  |
| 5. External Network Exfil    --> Data exfiltration, telemetry, unauthorized    |
|                                  outbound connections                          |
+--------------------------------------------------------------------------------+
```

---

## 2. Threat Vector Analysis & Hardening Controls

### 2.1 Threat Vector A: Malicious Audio & Sound Annotations
- **Threat:** Heap exhaustion, integer wrapping, or out-of-bounds reads via malformed RIFF/WAVE structures embedded in PDF `/Sound` streams or attached via voice comments.
- **Controls Implemented:**
  1. **Strict File Size Cap:** Maximum WAV file size strictly capped at 64 MiB (`MAX_WAV_FILE_SIZE = 64 * 1024 * 1024`).
  2. **Decoded Audio Duration Cap:** Total duration capped at 3,600 seconds (1 hour, `MAX_AUDIO_DURATION_SECONDS = 3600`).
  3. **Sample Rate Sanitization:** Bounded to the range `4,000 Hz <= rate <= 96,000 Hz`. Absurd or negative rates are rejected.
  4. **Channel Bounding:** Only mono (1) and stereo (2) channels are accepted.
  5. **Integer Overflow Prevention:** All offset arithmetic uses `checked_add` and `checked_mul`. Total chunk length validation explicitly verifies that `offset + chunk_len <= file_len`.
  6. **Word Alignment Enforcement:** In compliance with the RIFF specification, chunks with odd sizes require a 1-byte pad byte. The parser deterministically skips padding and checks slice bounds.
  7. **Format Whitelisting:** Only linear uncompressed PCM (`0x0001`) is parsed. Compressed or proprietary codecs are safely rejected with descriptive errors (`format {fmt_tag} is not uncompressed PCM`).

### 2.2 Threat Vector B: AcroForm JavaScript Execution
- **Threat:** Malicious JavaScript calculation or validation scripts attempting to hang the UI thread, leak data across documents, or execute unauthorized side-effects during undo/redo cycles.
- **Controls Implemented:**
  1. **Isolated ECMAScript Runtime:** Powered by the pure-Rust `boa_engine` sandbox. The JavaScript engine has zero bindings to the Android OS, filesystem, sockets, or host processes.
  2. **Per-Document State Isolation:** Evaluator state and output queues are strictly document-scoped. Verified by `form_fill_two_open_documents_do_not_leak_alerts`: actions in Document 1 cannot observe or trigger alerts in Document 2.
  3. **Idempotent Undo/Redo:** Document transactions snapshot values. Undoing or redoing a mutation restores field values without replaying JavaScript event handlers or generating duplicate user alerts (verified by `form_fill_undo_redo_does_not_replay_alert_side_effects`).
  4. **Syntax Error Resilience:** Unparseable scripts or runtime exceptions log errors gracefully without panicking or interrupting bulk form operations (verified by `form_fill_invalid_script_does_not_panic_or_break_fill`).

### 2.3 Threat Vector C: Android Storage Access Framework (SAF)
- **Threat:** Path traversal (`../`), arbitrary file overwrite outside selected directory, or permission escalation.
- **Controls Implemented:**
  1. **Zero Legacy Storage Reliance:** `android:requestLegacyExternalStorage="true"` was removed. The application relies entirely on modern Android Scoped Storage.
  2. **Persistable URI Permissions:** External file operations use `content://` Uris obtained through system pickers (`ActivityResultContracts.OpenDocument` and `CreateDocument`). Persistable read/write permissions are taken via `contentResolver.takePersistableUriPermission`.
  3. **Isolated File Descriptors:** File reading and writing pass through `ContentResolver.openFileDescriptor(uri, "r" | "w")`. The native Rust engine operates on detached native file descriptors or memory buffers, completely precluding canonical path traversal attacks.

### 2.4 Threat Vector D: Zero Network Exfiltration
- **Threat:** Silent telemetry, remote asset fetching, or exfiltration of sensitive PDF contents.
- **Controls Implemented:**
  1. **No Internet Permission:** `android.permission.INTERNET` is completely absent from `AndroidManifest.xml`.
  2. **No Network Stacks:** The Android native bridge `pdfcraft-android` links zero networking crates (no `reqwest`, `hyper`, `tokio-net`).
  3. **100% Offline-First:** Font measurement, rendering, OCR, form calculation, and digital signatures function entirely without network access.

### 2.5 Threat Vector E: Memory Safety & Unsafe Code Boundaries
- **Threat:** Memory corruption, use-after-free, double-free, or buffer overflows in native C/Rust code.
- **Controls Implemented:**
  1. **Workspace Unsafe Code Ban:** All workspace crates enforce `#![forbid(unsafe_code)]`. No raw pointers or manual memory management exist in the application code.
  2. **Safe UniFFI FFI Boundary:** Cross-language calls between Kotlin and Rust are generated by Mozilla's UniFFI generator, ensuring memory safety, automatic reference counting, and panic-catching boundaries across the JNI layer.
  3. **Panic Containment:** Rust engine entry points wrap execution in `std::panic::catch_unwind`, transforming unexpected internal panics into structured error types (`Result<T, MobileException>`) rather than terminating the Android process.

---

## 3. Security Verification Audit Matrix

| Verification Check | Target Component | Command / Test | Result | Evidence File |
| :--- | :--- | :--- | :--- | :--- |
| **WAV Parser Security** | `crates/automation/src/wav.rs` | `cargo test -p pdfcraft-automation --lib wav` | **PASS (19 tests)** | `02_automation_tests.log` |
| **Form Script Sandbox** | `crates/automation/tests/it/form_bulk.rs` | `cargo test -p pdfcraft-automation --test it -- form_fill` | **PASS (7 tests)** | `02_automation_tests.log` |
| **Manifest Permissions** | `AndroidManifest.xml` | `grep -rn "uses-permission" apps/pdfcraft-android/` | **PASS (No INTERNET)** | `07_apk_libs.log` |
| **Native Unsafe Ban** | All Crates | `grep -rn "#!\[forbid(unsafe_code)\]" crates/ apps/` | **PASS (Enforced)** | `01_rust_host_tests.log` |
| **Static Code Analysis** | Full Workspace | `cargo clippy --all-targets -- -D warnings` | **PASS (0 warnings)** | `04_cargo_clippy.log` |
| **Asset License Integrity**| Asset Catalog | `cargo xtask assets` | **PASS (323 items)** | `03_asset_policy.log` |
