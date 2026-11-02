# Upstream Synchronization and Compatibility Report

**Repository:** `https://github.com/dikshant363/pdfcraft-android`  
**Upstream Target:** `https://github.com/storytold/pdfcraft:main`  
**Integration Branch:** `android-port`  
**Sync Merge Commit:** `fdc635f4bfa1e4dfdd7b09ca6a908a5538e1469e`  
**Pre-Sync Backup Branch:** `backup/android-port-pre-sync` (`ad72d9afbb2cdd2b14b4c191a855e7de6962be92`)  
**Upstream Head SHA:** `e26151877cb019446d321516e889d18fa74dc8a3`  
**Audit Date:** 2026-11-02  

---

## 1. Executive Summary

The `android-port` development branch in `dikshant363/pdfcraft-android` has been fully synchronized with current upstream `storytold/pdfcraft:main` (`e26151877cb019446d321516e889d18fa74dc8a3`).

All changes from upstream were merged cleanly without merge conflicts. Following the merge:
- All 17 native Android bridge tests in `pdfcraft-android` passed.
- All 77 annotation tests in `pdfcraft-annot` passed.
- All 324 automation tests in `pdfcraft-automation` passed.
- All 54 Android JVM unit tests in `apps/pdfcraft-android/android` passed.
- Rust NDK libraries for both `arm64-v8a` and `x86_64` compiled cleanly.
- UniFFI Kotlin bindings remained completely stable.
- All 323 repository assets passed the strict asset policy check (`cargo xtask assets`).

---

## 2. Upstream Divergence and Integration Analysis

### 2.1 Upstream Changes Integrated

Between the fork branch base and upstream head `e2615187`, upstream added:
1. **Milestone 2.6 Image Allowances and Decode Budgets:**
   - Image stream size allowances and JBIG2 segment allocation budgets across `crates/cos`, `crates/filters`, `crates/optimize`, and `crates/redact`.
   - Hardened handling of deeply nested or repetitive dictionary streams.
2. **Dependency Updates:**
   - Update of `thiserror` crate to version 2.0.18 across `crates/cos` and related crates.
3. **Desktop egui Patches:**
   - Refinements to freehand hotbar and desktop-only UI controls (which are excluded from the Android build target).

### 2.2 Conflict Analysis by Subsystem

| Subsystem | Upstream Status | Android Port Status | Conflict Risk | Resolution |
| :--- | :--- | :--- | :--- | :--- |
| **Rust Engine (`crates/engine`)** | Intact | Unmodified engine core | None | Clean fast-merge |
| **PDF Renderer (`crates/render`)** | Clean | Exposes tile rendering to Android | None | Kept Android tile API additions |
| **Annotations (`crates/annot`)** | Clean | Added bounded RIFF/WAVE parser (`wav.rs`) | None | Upstream had no edits in `wav.rs`; clean merge |
| **Automation (`crates/automation`)** | Milestone 2.6 fixes | Surfaced AcroForm calculation alerts/requests | None | Upstream touched unrelated commands; clean merge |
| **Document Storage / COS (`crates/cos`)** | JBIG2 & thiserror updates | Clean | None | Adopted upstream updates |
| **Android Bridge (`apps/pdfcraft-android`)** | Android-specific | Maintained | None | Upstream does not have `apps/pdfcraft-android` |
| **Cargo Lockfile (`Cargo.lock`)** | Dependabot bumped thiserror | Re-locked cleanly | None | Unified lockfile without drift |

---

## 3. Post-Sync Verification Evidence

### 3.1 Host Rust Test Suite Execution

```bash
cargo test --locked -p pdfcraft-android
```
- **Exit Code:** `0`
- **Results:** 17 passed; 0 failed; 0 ignored.
- **Key Tests Verified:**
  - `tests::open_failures_are_told_apart`: PASS
  - `tests::opens_a_document_and_describes_its_pages`: PASS
  - `tests::flattens_the_outline`: PASS
  - `tests::closing_refuses_to_lose_edits`: PASS
  - `tests::renders_a_page_and_a_tile`: PASS
  - `tests::runs_tools_and_tracks_undo`: PASS
  - `tests::search_options_and_fresh_text_after_an_edit`: PASS
  - `tests::a_voice_comment_keeps_its_recording`: PASS
  - `tests::form_scripts_run_when_fields_are_filled_and_buttons_pressed`: PASS
  - `tests::signs_with_a_digital_id_and_trusts_its_signer`: PASS

```bash
cargo test --locked -p pdfcraft-annot
```
- **Exit Code:** `0`
- **Results:** 77 passed; 0 failed; 0 ignored.

```bash
cargo test --locked -p pdfcraft-automation
```
- **Exit Code:** `0`
- **Results:** 324 passed; 0 failed; 0 ignored.

### 3.2 Android JVM Unit Test Execution

```bash
cd apps/pdfcraft-android/android && ./gradlew testDebugUnitTest
```
- **Exit Code:** `0`
- **Results:** 54 tests across 12 test suites; 0 failures; 0 errors; 0 skipped.

### 3.3 Asset Attribution Verification

```bash
cargo xtask assets
```
- **Exit Code:** `0`
- **Status:** `assets: ok (323 repository assets, 6 bundled by dependencies, 22 fetched at build time)`

---

## 4. Upstream Contribution Branches Revalidation

The repository maintains two isolated upstream contribution branches intended for PR submission to `storytold/pdfcraft:main`. Both branches were evaluated for compatibility against `upstream/main` (`e2615187`) using non-destructive 3-way merge tree calculation:

### 4.1 `upstream/automation-apply-alerts`

- **Branch Head:** `f2526d6c`
- **Commit History:**
  - `f2526d6c`: `test(automation): add comprehensive AcroForm calculation alerts and requests tests`
  - `8246d8ce`: `fix(automation): surface form calculation alerts and requests in apply`
- **Base Commit:** `0cba87ae`
- **Merge Test Command:**
  ```bash
  git merge-tree $(git merge-base upstream/main upstream/automation-apply-alerts) upstream/main upstream/automation-apply-alerts
  ```
- **Result:** Merges cleanly with exit code 0. Zero conflicts against current upstream main.

### 4.2 `upstream/sound-annotations`

- **Branch Head:** `f9490027`
- **Commit History:**
  - `f9490027`: `fix(annot): implement bounded RIFF/WAVE parser with padding validation and memory limits`
  - `58dbec31`: `feat(annot): implement sound annotations (voice comments) per ISO 32000-2`
- **Base Commit:** `0cba87ae`
- **Merge Test Command:**
  ```bash
  git merge-tree $(git merge-base upstream/main upstream/sound-annotations) upstream/main upstream/sound-annotations
  ```
- **Result:** Merges cleanly with exit code 0. Zero conflicts against current upstream main.

---

## 5. Conclusion and Upstream Alignment

The `android-port` branch is strictly synchronized with upstream `main` (`e2615187`). No proprietary or breaking architectural departures have been introduced into shared Rust crates. The dual-track strategy remains completely uncompromised.
