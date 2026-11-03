# PDFCRAFT UPSTREAM CONTRIBUTION READINESS REPORT

**Target Repository:** `https://github.com/storytold/pdfcraft` (`main` @ `0cba87ae`)  
**Origin Fork:** `https://github.com/dikshant363/pdfcraft-android`  
**Audit Date:** 2026-11-03  
**Status:** PR #5833, PR #5834, AND PR #5907 OPEN ON UPSTREAM; CODERABBIT REVIEW FINDINGS FULLY REMEDIATED

---

## 1. Compliance with Upstream Standards (`AGENTS.md` & `CLAUDE.md`)

Before preparing contributions, every patch was audited against upstream's mandatory governance rules:

1. **Asset Policy (Absolute Cleanliness — `AGENTS.md` §1):**
   - Zero Adobe assets, logos, fonts, or cursors.
   - All UI icons sourced strictly from Lucide (ISC).
   - All assets cataloged in `ATTRIBUTION.toml` and verified via `cargo xtask assets` (289 repository assets, 0 violations).
2. **Clean-Room Rules (`AGENTS.md` §2):**
   - Implementations derived purely from ISO 32000-2 and public RFC specifications. Zero Acrobat decompilation or binary inspection.
3. **Headless Automation Reachability (`AGENTS.md` §3):**
   - All functionality exposed headlessly through `crates/automation` tool commands (`comment_add`, `form_fill`, etc.).
4. **Never Crash Principle (`AGENTS.md` §4):**
   - No `unwrap()` or `expect()` in non-test production code.
   - Workspace-wide `#![forbid(unsafe_code)]`.
   - Input bounded arithmetic (`checked_add`, `checked_mul`) on all external binary data.

---

## 2. Upstream Pull Request Status and Remediation Breakdown

### 2.1 PR #5833: Automation Apply Alerts, Requests & Diagnostics
- **Target Pull Request:** `https://github.com/storytold/pdfcraft/pull/5833`
- **Topic Branch:** `upstream/automation-apply-alerts`
- **Latest Verified Commit:** `ee786423` (`test(automation): add document-level script error and cross-doc isolation regression tests`)
- **Base Commit:** `0cba87ae` (`storytold/pdfcraft:main`)
- **CodeRabbit Review Remediation:**
  - Surfaced `errors`, `doc_errors`, and `console` diagnostics in `Automation::apply` output (`crates/automation/src/documents.rs`).
  - Added comprehensive docstring documentation to `apply`.
  - Added integration test `form_fill_surfaces_script_errors_and_console_diagnostics` in `tests/it/form_bulk.rs` verifying multi-alert delivery, console diagnostics, document isolation, repeated call drainage, and undo/redo non-replay.
- **Verification Evidence:**
  - `cargo test --locked -p pdfcraft-automation`: 382 passed (61 unit, 321 integration), 0 failed.
  - `cargo clippy -p pdfcraft-automation --tests --locked -- -D warnings`: 0 warnings.
  - Pushed to `origin/upstream/automation-apply-alerts` and updated GitHub PR description.

### 2.2 PR #5834: ISO 32000-2 Sound Annotations & Bounded RIFF Parser
- **Target Pull Request:** `https://github.com/storytold/pdfcraft/pull/5834`
- **Topic Branch:** `upstream/sound-annotations`
- **Latest Verified Commit:** `183bd806` (`fix(annot,automation): address review feedback on sound annotations and bounded WAV reader`)
- **Base Commit:** `0cba87ae` (`storytold/pdfcraft:main`)
- **CodeRabbit Review Remediation:**
  - Audio frame calculation: In `crates/annot/src/create.rs`, validates channel count before computing frame size (`bytes_per_frame = 2 * channels`); rejects non-multiples of whole frames (e.g. 2-byte or 6-byte stereo samples).
  - Bounded WAV reading: In `crates/automation/src/comments.rs`, performs pre-allocation file metadata check rejecting files $>64\text{ MiB}$ and special files (FIFOs); caps reading with `take(MAX_WAV_FILE_SIZE + 1)`.
  - Added unit tests in `crates/annot/src/tests.rs` and `crates/automation/src/wav.rs`.
  - Added regression tests in `crates/automation/tests/it/automation.rs`.
- **Verification Evidence:**
  - `cargo test --locked -p pdfcraft-annot`: 78 passed, 0 failed.
  - `cargo test --locked -p pdfcraft-automation`: 382 passed (61 unit, 321 integration), 0 failed.
  - `cargo clippy -p pdfcraft-annot -p pdfcraft-automation --tests --locked -- -D warnings`: 0 warnings.
  - `cargo xtask assets`: 289 assets, 0 violations.
  - Pushed to `origin/upstream/sound-annotations` and updated GitHub PR description with Never-Crash, Security, Parity, and Reviewer-Focus sections.

---

## 3. PR #5907: Native Android Platform Port
- **Target Pull Request:** `https://github.com/storytold/pdfcraft/pull/5907`
- **Head Branch:** `dikshant363:android-port`
- **Latest Verified Commit:** `c2444102` (`fix(android): address PR #5907 review findings and form-field back persistence`)
- **Status:** Open for maintainer review; review remediations implemented and verified.
- **CodeRabbit Review Remediation:**
  - Uncommitted form field text preserved on Android Back / doc close via `DisposableEffect.onDispose` and `commitFieldEdit()`.
  - LRU cache bounds (64 items) on `PagesScreen` thumbnails; top-8 priority eviction on `PageImages`.
  - Safe atomic file writes using `copyTo(overwrite = true)` + `delete()` in place of destructive delete-before-rename.
  - Audio recording header write before `startRecording()`; immediate stop on negative error codes; unique play cache files.
  - `sound.rs` decompression bomb prevention via `decompressed_content_with_limit(MAX_SOUND_BYTES)`.
  - Document liveness verification in `return_renderer`.
  - Clean asset attribution in `ATTRIBUTION.toml` (327 verified assets, 0 violations).
- **Verification Evidence:**
  - `cargo test -p pdfcraft-android`: 17 passed, 0 failed.
  - `./gradlew testDebugUnitTest`: BUILD SUCCESSFUL (all unit tests passed).
  - `cargo clippy -p pdfcraft-android --tests --locked -- -D warnings`: 0 warnings.
  - `cargo fmt --check`: passed.
  - `cargo xtask assets`: 327 assets, 0 violations.
