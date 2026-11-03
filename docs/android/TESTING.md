# PdfCraft Android Testing and Verification Guide

This document details the test suites, automated verification workflows, and testing methodology for **PdfCraft Android** (`io.github.dikshant363.pdfcraft`).

---

## 1. Test Architecture Overview

PdfCraft Android uses a multi-tier testing strategy ensuring engine correctness, native bridge safety, and UI component reliability:

```
┌────────────────────────────────────────────────────────┐
│                   Android Unit Tests                   │
│   (54 tests: Geometry, Speech, Tools, Forms, Snapping) │
├────────────────────────────────────────────────────────┤
│                Native Bridge Unit Tests                │
│    (17 tests: Document I/O, Tiles, JS, Forms, Crypt)   │
├────────────────────────────────────────────────────────┤
│                 Rust Engine & Automation               │
│        (318 tests: ISO 32000-2, Redaction, XFA)        │
├────────────────────────────────────────────────────────┤
│                 Asset & Policy Linter                  │
│             (xtask assets: 327 assets verified)        │
└────────────────────────────────────────────────────────┘
```

---

## 2. Test Execution Commands

### 2.1 Native Host Unit Tests (Rust)
Tests the UniFFI bridge, document loading, page geometry, tile rendering, voice comments, JS forms, and digital signatures:

```bash
cargo test -p pdfcraft-android
```

Expected result:
```text
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### 2.2 Automation Engine Tests
Validates the underlying engine automation tools, roots confinement, batch jobs, form calculations, and redaction:

```bash
cargo test -p pdfcraft-automation
```

Expected result:
```text
test result: ok. 318 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### 2.3 Android Unit Tests (Kotlin & Jetpack Compose)
Runs the Android unit tests on JVM using OpenJDK 17:

```bash
cd apps/pdfcraft-android/android
./gradlew testDebugUnitTest
```

Expected result:
```text
BUILD SUCCESSFUL
54 tests completed across 12 test suites:
- CommentListTest: 3 passed
- EditListsTest: 4 passed
- FreeTextLayoutTest: 4 passed
- PdfDateTest: 4 passed
- QuickToolbarTest: 3 passed
- ReadAloudLanguageTest: 3 passed
- ReadAloudWordsTest: 2 passed
- ScanSessionTest: 4 passed
- ShapeSnapTest: 6 passed
- SignatureBoxTest: 5 passed
- UiTextTest: 1 passed
- ViewerGeometryTest: 15 passed
```

### 2.4 Asset and Licensing Policy Check
Validates that every icon, font, and asset in the repository is open-source (OFL, ISC, MIT), free of proprietary content, and matches its SHA-256 hash in `ATTRIBUTION.toml`:

```bash
cargo xtask assets
```

Expected result:
```text
assets: ok (323 repository assets, 6 bundled by dependencies, 22 fetched at build time)
```

---

## 3. Test Suites Breakdown

### 3.1 Document Lifecycle & Security Tests
- `open_failures_are_told_apart`: Differentiates missing files, corrupt headers, and encryption errors.
- `asks_for_a_password_and_opens_with_the_right_one`: Validates user/owner password prompts and AES decryption.
- `closing_refuses_to_lose_edits`: Verifies dirty state tracking before document teardown.
- `signs_with_a_digital_id_and_trusts_its_signer`: Tests digital signature application and cryptographic trust validation.

### 3.2 Rendering & Geometry Tests
- `renders_a_page_and_a_tile`: Validates full-page rasterization and 512×512 tile rendering with correct stride and RGBA format.
- `refuses_bad_render_requests`: Guards against negative coordinates, NaN scales, or out-of-bounds tile requests.
- `ViewerGeometryTest`: 15 comprehensive tests validating octave zoom calculations, visible tile detection, pinch-to-zoom offsets, and viewport culling.

### 3.3 Form and Interactive Features
- `form_scripts_run_when_fields_are_filled_and_buttons_pressed`: Tests calculation scripts, field change recalculation, alert dialog event queuing, and button click handlers.
- `a_voice_comment_keeps_its_recording`: Validates voice recording PCM WAV encoding, big-endian PDF stream conversion, and round-trip audio decoding.
- `reading_blocks_split_paragraphs_and_map_characters_to_glyphs`: Verifies text extraction, paragraph clustering, and TTS reading order.
