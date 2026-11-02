# PDFCRAFT TEST VERIFICATION & EVIDENCE LOG

**Repository:** `dikshant363/pdfcraft-android` (Fork) & `storytold/pdfcraft` (Upstream)  
**Verification Date:** 2026-11-02  
**Overall Status:** 100% REPRODUCED & VERIFIED (Zero Failures, Exit Code 0 Across All Suites)

---

## 1. Test Verification Summary

| Test Domain | Subsystem / Target | Executed Command | Test Count | Result | Exit Code | Evidence Log |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Rust Engine (Host)** | `pdfcraft-android` | `cargo test -p pdfcraft-android` | 17 | **PASS** | 0 | `docs/audit/evidence/01_rust_host_tests.log` |
| **Annotations (Host)** | `pdfcraft-annot` | `cargo test -p pdfcraft-annot` | 77 | **PASS** | 0 | Host test execution |
| **Automation Suite** | `pdfcraft-automation` | `cargo test -p pdfcraft-automation` | 324 | **PASS** | 0 | `docs/audit/evidence/02_automation_tests.log` |
| **WAV Parser Unit** | `wav.rs` | `cargo test -p pdfcraft-automation --lib wav` | 19 | **PASS** | 0 | Inline execution log |
| **AcroForm Integration**| `form_bulk.rs` | `cargo test -p pdfcraft-automation --test it -- form_fill` | 7 | **PASS** | 0 | Inline execution log |
| **Sound Annot E2E** | `automation.rs` | `cargo test -p pdfcraft-automation --test it -- sound_comment` | 1 | **PASS** | 0 | Inline execution log |
| **Asset Policy** | Entire Workspace | `cargo xtask assets` | 323 assets | **PASS** | 0 | `docs/audit/evidence/03_asset_policy.log` |
| **Rust Code Style** | Entire Workspace | `cargo fmt --check` | Clean | **PASS** | 0 | `docs/audit/evidence/04_cargo_fmt.log` |
| **Rust Linter** | Workspace & Tests | `cargo clippy --all-targets -- -D warnings` | 0 warnings | **PASS** | 0 | `docs/audit/evidence/04_cargo_clippy.log` |
| **UniFFI Bindings** | `pdfcraft-android-bindgen` | `cargo run -p pdfcraft-android-bindgen -- generate ...` | 1 file (3.2k lines) | **PASS** | 0 | `docs/audit/evidence/05_uniffi_bindgen.log` |
| **Native NDK Build** | `arm64-v8a`, `x86_64` | `cargo ndk -t arm64-v8a -t x86_64 ...` | 2 ABIs | **PASS** | 0 | `docs/audit/evidence/05_cargo_ndk.log` |
| **Android JVM Tests** | Compose UI & Engine | `./gradlew testDebugUnitTest --rerun-tasks` | 54 | **PASS** | 0 | `docs/audit/evidence/06_android_jvm_tests.log` |
| **Debug APK Build** | Gradle Application | `./gradlew assembleDebug` | Multi-ABI APK | **PASS** | 0 | `docs/audit/evidence/07_assemble_debug.log` |
| **Release Build** | Gradle Application | `./gradlew assembleRelease bundleRelease` | APK + AAB | **PASS** | 0 | `docs/audit/evidence/08_release_build.log` |

---

## 2. Granular Suite Breakdown

### 2.1 Android Native Bridge Host Tests (`pdfcraft-android`)
- **Command:** `cargo test -p pdfcraft-android`
- **Output:** 17 passed; 0 failed; 0 ignored; finished in 0.62s
- **Verified Capabilities:**
  1. `tests::open_failures_are_told_apart`: Password requirements, corrupt files, and missing paths return discrete `MobileException` variants.
  2. `tests::opens_a_document_and_describes_its_pages`: Page geometry, rotation, media box dimensions accurately extracted.
  3. `tests::closing_refuses_to_lose_edits`: Attempting to close a dirty document without `discard_changes: true` safely errors.
  4. `tests::flattens_the_outline`: Bookmarks and hierarchical outline tree flattened to linear table with indentation depth.
  5. `tests::reports_tool_errors_instead_of_failing_hard`: Engine errors return structured JSON error descriptions.
  6. `tests::reading_blocks_split_paragraphs_and_map_characters_to_glyphs`: Text extraction for TTS and reading mode maps glyph bounding boxes.
  7. `tests::runs_tools_and_tracks_undo`: Transactional mutations generate undo/redo checkpoints.
  8. `tests::renders_a_page_and_a_tile`: Multi-scale rasterization and tiled rendering for pinch-to-zoom viewports.
  9. `tests::search_options_and_fresh_text_after_an_edit`: Case-insensitive text search and live cache invalidation after edits.
  10. `tests::tells_document_attachments_from_comment_attachments`: Strict distinction between file-level and annotation-level attachments.
  11. `tests::lists_comments_links_widgets_and_thumbnails`: Bulk enumerator returns all interactive canvas overlays.
  12. `tests::selects_words_and_reads_their_text`: Spatial hit-testing for text selection handles and drag actions.
  13. `tests::a_voice_comment_keeps_its_recording`: Voice comment creation stores valid `/Sound` stream and recovers audio.
  14. `tests::form_scripts_run_when_fields_are_filled_and_buttons_pressed`: Calculation scripts execute on value changes.
  15. `tests::refuses_bad_render_requests`: Non-finite, NaN, and negative scale factors rejected safely.
  16. `tests::asks_for_a_password_and_opens_with_the_right_one`: RC4 and AES-128/256 password challenge handling.
  17. `tests::signs_with_a_digital_id_and_trusts_its_signer`: PKCS#12 certificate parsing, digital signature creation and trust verification.

### 2.2 Android JVM Unit Tests
- **Command:** `./gradlew testDebugUnitTest --rerun-tasks`
- **Output:** 54 passed; 0 failed; 12 suites; execution duration: 13s
- **Verified Test Classes:**
  - `DigitalIdTest`: PKCS#12 digital identity loading and password validation (4 tests).
  - `EngineStateTest`: Session state transitions and document cache lifecycles (5 tests).
  - `FreeTextLayoutTest`: Rich text alignment, bounding box calculation, font size auto-scaling (6 tests).
  - `LocalFilesTest`: Sandboxed cache file creation, MIME validation, temporary cleanup (4 tests).
  - `ReadAloudLanguageTest`: Locale matching, TTS voice synthesis parameters, paragraph splitting (5 tests).
  - `RecentFilesTest`: Persisted document history, MRU ordering, URI de-duplication (4 tests).
  - `ScanSessionTest`: Document scanner page reordering, deskew metadata, multi-page stitch (4 tests).
  - `ShapeSnapTest`: Geometric snapping to grid, 45/90 degree constraint angles (5 tests).
  - `SpellCheckTest`: Android TextServices spellchecker integration and suggestion parsing (3 tests).
  - `UserBookmarksTest`: User-created page bookmark persistence and synchronization (4 tests).
  - `ViewerGeometryTest`: Matrix coordinate transforms between PDF points and Compose screen pixels, pinch-zoom tile slicing (7 tests).
  - `VoiceCommentPlayerTest`: Audio playback state machine, WAV buffer streaming, scrubber positioning (3 tests).

### 2.3 Automation & WAV Parser Verification
- **Command:** `cargo test -p pdfcraft-automation`
- **Output:** 324 passed; 0 failed; finished in 5.3s
- **Coverage Highlights:**
  - 19 RIFF/WAVE parser tests in `wav.rs` (odd chunk padding, corrupt headers, oversized payloads, duplicate chunks, conversion to big-endian signed 16-bit PCM).
  - 5 AcroForm bulk tests in `form_bulk.rs` (verifying alert and request surfacing in `apply()`, multi-document isolation, undo/redo side-effect idempotency, invalid script safety).
  - End-to-end sound annotation creation and round-trip verification in `automation.rs`.

---

## 3. Toolchain & Environment Attestation

```
Host OS:           Darwin 24.6.0 (macOS Sequoia, arm64)
Rust Toolchain:    rustc 1.95.0 / cargo 1.95.0
Java Runtime:      OpenJDK 17.0.20 (Homebrew)
Android SDK:       Compile SDK 37, Min SDK 31, Target SDK 36
Android NDK:       NDK 29.0.14206865 (r29)
Cargo-NDK:         cargo-ndk 4.1.2
UniFFI Bindgen:    uniffi-bindgen 0.29.5
Gradle Version:    Gradle 9.6.1
```
