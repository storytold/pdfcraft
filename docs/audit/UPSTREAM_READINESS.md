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
- **Latest Verified Commit:** `447d4a2d` (`fix(automation): preserve script errors and console diagnostics in apply`)
- **Base Commit:** `0cba87ae` (`storytold/pdfcraft:main`)
- **CodeRabbit Review Remediation:**
  - Surfaced `errors`, `doc_errors`, and `console` diagnostics in `Automation::apply` output (`crates/automation/src/documents.rs`).
  - Added comprehensive docstring documentation to `apply`.
  - Added integration test `form_fill_surfaces_script_errors_and_console_diagnostics` in `tests/it/form_bulk.rs` verifying multi-alert delivery, console diagnostics, document isolation, repeated call drainage, and undo/redo non-replay.
- **Verification Evidence:**
  - `cargo test --locked -p pdfcraft-automation`: 324 passed, 0 failed.
  - `cargo clippy -p pdfcraft-automation --tests --locked -- -D warnings`: 0 warnings.
  - Pushed to `origin/upstream/automation-apply-alerts` and updated GitHub PR description.

### 2.2 PR #5834: ISO 32000-2 Sound Annotations & Bounded RIFF Parser
- **Target Pull Request:** `https://github.com/storytold/pdfcraft/pull/5834`
- **Topic Branch:** `upstream/sound-annotations`
- **Latest Verified Commit:** `81aaf0e3` (`fix(annot): validate stereo frames and bound WAV file loading to 64 MiB`)
- **Base Commit:** `0cba87ae` (`storytold/pdfcraft:main`)
- **CodeRabbit Review Remediation:**
  - Audio frame calculation: In `crates/annot/src/create.rs`, validates channel count before computing frame size (`bytes_per_frame = 2 * channels`); rejects non-multiples of whole frames (e.g. 2-byte or 6-byte stereo samples).
  - Bounded WAV reading: In `crates/automation/src/comments.rs`, performs pre-allocation file metadata check rejecting files $>64\text{ MiB}$; caps reading with `take(MAX_WAV_FILE_SIZE + 1)`.
  - Added unit test `sound_annotation_validates_channel_frames_and_samples` in `crates/annot/src/tests.rs`.
  - Added regression test `sound_comment_rejects_oversized_and_malformed_audio_files` in `crates/automation/tests/it/automation.rs`.
- **Verification Evidence:**
  - `cargo test --locked -p pdfcraft-annot`: 78 passed, 0 failed.
  - `cargo test --locked -p pdfcraft-automation`: 320 integration passed, 60 unit passed, 0 failed.
  - `cargo clippy -p pdfcraft-annot -p pdfcraft-automation --tests --locked -- -D warnings`: 0 warnings.
  - `cargo xtask assets`: 289 assets, 0 violations.
  - Pushed to `origin/upstream/sound-annotations` and updated GitHub PR description with Never-Crash, Security, Parity, and Reviewer-Focus sections.

---

## 3. PR #5907: Native Android Platform Port
- **Target Pull Request:** `https://github.com/storytold/pdfcraft/pull/5907`
- **Head Branch:** `dikshant363:android-port`
- **Status:** Open for maintainer review.
- **Tenets:** Independent `apps/pdfcraft-android` module, native Rust PDF engine via UniFFI, offline-first Material 3 Compose UI, no impact on desktop-only contributors.
