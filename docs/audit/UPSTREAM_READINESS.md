# PDFCRAFT UPSTREAM CONTRIBUTION READINESS REPORT

**Target Repository:** `https://github.com/storytold/pdfcraft` (`main` @ `0cba87ae`)  
**Origin Fork:** `https://github.com/dikshant363/pdfcraft-android`  
**Audit Date:** 2026-11-02  
**Status:** READY FOR USER AUTHORIZATION TO SUBMIT

---

## 1. Compliance with Upstream Standards (`AGENTS.md` & `CLAUDE.md`)

Before preparing contributions, every patch was audited against upstream's mandatory governance rules:

1. **Asset Policy (Absolute Cleanliness — `AGENTS.md` §1):**
   - Zero Adobe assets, logos, fonts, or cursors.
   - All UI icons sourced strictly from Lucide (ISC).
   - All assets cataloged in `ATTRIBUTION.toml` and verified via `cargo xtask assets` (323 items verified, 0 violations).
2. **Clean-Room Rules (`AGENTS.md` §2):**
   - Implementations derived purely from ISO 32000-2 and public RFC specifications. Zero Acrobat decompilation or binary inspection.
3. **Headless Automation Reachability (`AGENTS.md` §3):**
   - All functionality exposed headlessly through `crates/automation` tool commands (`comment_add`, `form_fill`, etc.).
4. **Never Crash Principle (`AGENTS.md` §4):**
   - No `unwrap()` or `expect()` in non-test production code.
   - Workspace-wide `#![forbid(unsafe_code)]`.
   - Input bounded arithmetic (`checked_add`, `checked_mul`) on all external binary data.

---

## 2. Contribution Track B Breakdown

### 2.1 PR 1: Automation Apply Alerts & Viewer Requests
- **Topic Branch:** `upstream/automation-apply-alerts`
- **Verified Commit:** `f2526d6c` (`test(automation): add comprehensive AcroForm calculation alerts and requests tests`)
- **Base Commit:** `0cba87ae` (`storytold/pdfcraft:main`)
- **Changeset:**
  - `crates/automation/src/documents.rs`: Harvests alerts and viewer requests from `take_js_output(id)` and attaches them to the `apply()` tool response payload.
  - `crates/automation/src/a11y.rs`: Aligns accessibility output.
  - `crates/automation/src/a11y/form_tools.rs`: Aligns form calculation output.
  - `crates/automation/tests/it/form_bulk.rs`: 5 new integration tests verifying single-doc alerts, multi-alert arrays, cross-document isolation, undo/redo idempotency, and syntax error safety.
- **Verification:**
  - `cargo test -p pdfcraft-automation`: 324 passed, 0 failed.
  - `cargo clippy -p pdfcraft-automation --tests`: 0 warnings.
  - `git diff origin/main..upstream/automation-apply-alerts`: **Zero Android files touched.**
- **Action Required:** Open PR against `storytold/pdfcraft:main` upon user authorization.

### 2.2 PR 2: ISO 32000-2 Sound Annotations & Bounded RIFF Parser
- **Topic Branch:** `upstream/sound-annotations`
- **Verified Commit:** `f9490027` (`fix(annot): implement bounded RIFF/WAVE parser with padding validation and memory limits`)
- **Base Commit:** `0cba87ae` (`storytold/pdfcraft:main`)
- **Changeset:**
  - `crates/annot/src/types.rs`: Implements `Shape::Sound` and Sound annotation dictionary serialization.
  - `crates/annot/src/create.rs`: Constructs `/Subtype /Sound` annotation dictionary with attached sound object stream (`/R`, `/C`, `/B`, `/E /Signed`).
  - `crates/annot/src/appearance/symbols.rs` & `appearance.rs`: Draws standard vector microphone icon appearance stream with user stroke color.
  - `crates/automation/src/wav.rs`: Bounded, spec-compliant RIFF/WAVE parser with 64 MiB cap, 3600s duration cap, odd-chunk padding verification, and checked arithmetic.
  - `crates/automation/src/comments.rs`: Integrates `wav_samples` for `comment_add` tool.
  - `crates/annot/src/tests.rs`: Unit tests for sound stream dictionary and display appearances.
  - `crates/automation/tests/it/automation.rs`: End-to-end sound comment creation, save, and reopen test.
- **Verification:**
  - `cargo test -p pdfcraft-annot`: 77 passed, 0 failed.
  - `cargo test -p pdfcraft-automation`: 324 passed, 0 failed.
  - `cargo clippy -p pdfcraft-annot -p pdfcraft-automation --tests`: 0 warnings.
  - `git diff origin/main..upstream/sound-annotations`: **Zero Android files touched.**
- **Action Required:** Open PR against `storytold/pdfcraft:main` upon user authorization.

---

## 3. RFC Proposal: Native Android Platform Support

- **Document Location:** `docs/contributing/PROPOSAL_ISSUE_DRAFT.md`
- **Objective:** Propose first-class native Android support in PdfCraft without imposing Android NDK or SDK toolchain requirements on desktop-only contributors.
- **Key Tenets of Proposal:**
  - Dedicated crate `apps/pdfcraft-android` using UniFFI library mode.
  - Core engine reuse (rendering, editing, forms, cryptography, signing, OCR).
  - Kotlin + Jetpack Compose UI with offline-first SAF architecture.
  - Separate GitHub Actions workflow (`android.yml`) running only when Android directories are modified.
- **Status:** Complete draft prepared. Awaiting user authorization to post on upstream GitHub issues.
