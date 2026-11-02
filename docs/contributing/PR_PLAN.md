# Upstream Pull Request Plan

This document outlines the concrete Pull Request sequence for upstreaming improvements to `storytold/pdfcraft:main`.

---

## PR 1: Surface Form Calculation Alerts and Requests in Automation Apply

- **Topic Branch:** `upstream/automation-apply-alerts`
- **Base Branch:** `upstream/main`
- **Type:** `fix(automation)`
- **Target Subsystem:** `crates/automation`
- **Status:** Ready for branch creation and submission

### 1.1 Problem Statement
When an automation tool executes document edits via `Automation::apply` (for example, `form_fill`), document scripts or field calculation scripts may execute and trigger alert dialogs (via `app.alert(...)`) or navigation/print requests.
Previously, `apply` discarded the accumulated `JsOutput` or left it uncollected, returning only document metadata summaries and edit warnings. Callers driving the engine headlessly (CLI, MCP agents, or alternative UI frontends) were unable to observe script alerts raised during form interactions.

### 1.2 Proposed Solution
1. In `crates/automation/src/a11y/form_tools.rs`, expose `pub(crate) fn alert_json` and `pub(crate) fn request_json`.
2. In `crates/automation/src/documents.rs`, call `self.session.take_js_output(id)` after `self.session.apply(id, edit)`.
3. If alerts are present, populate `out["alerts"]`. If requests are present, populate `out["requests"]`.
4. Add a regression test verifying that filling a form field that triggers a calculation alert includes the alert message in the JSON output.

### 1.3 Quality Gates & Verification Evidence
- `cargo fmt --check`: PASS
- `cargo clippy -p pdfcraft-automation -- -D warnings`: PASS
- `cargo test -p pdfcraft-automation`: PASS (318 passed)
- Regression test: `form_scripts_run_when_fields_are_filled_and_buttons_pressed` passes.

---

## PR 2: Implement Sound Annotations (Voice Comments) per ISO 32000-2

- **Topic Branch:** `upstream/sound-annotations`
- **Base Branch:** `upstream/main`
- **Type:** `feat(annot)`
- **Target Subsystem:** `crates/annot`, `crates/engine`, `crates/automation`
- **Status:** Ready for branch creation and submission

### 1.1 Problem Statement
ISO 32000-2 §12.5.6.15 defines Sound Annotations, which attach audio recordings (such as voice comments) to PDF documents. While PdfCraft supports Text notes, File attachments, and Caret annotations, Sound annotations were unrepresented in `Shape`.

### 1.2 Proposed Solution
1. **Model:** Add `Shape::Sound { at: [f64; 2], rate: u32, channels: u16, samples: Vec<u8> }` to `crates/annot/src/types.rs`.
2. **Creation:** Implement `add_annotation` in `crates/annot/src/create.rs` to construct the `/Sound` stream with sampling rate `/R`, channels `/C`, bit depth `/B 16`, and encoding `/E /Signed`.
3. **Appearance:** Generate standard microphone icon vector graphics in `crates/annot/src/appearance/symbols.rs` (`sound_icon`).
4. **Undo / Redo:** Register `"Add voice comment"` in `crates/engine/src/edit/labels.rs`.
5. **Headless Automation:** Support `comment_add` with `"sound"` type in `crates/automation/src/comments.rs`.
6. **Safety:** Validate sampling rates (4 to 96 kHz), channels (mono/stereo), and non-empty sample buffers; `#![forbid(unsafe_code)]` preserved.

### 1.3 Quality Gates & Verification Evidence
- `cargo fmt --check`: PASS
- `cargo clippy -p pdfcraft-annot -p pdfcraft-engine -p pdfcraft-automation -- -D warnings`: PASS
- `cargo test -p pdfcraft-annot`: PASS
- `cargo test -p pdfcraft-automation`: PASS (318 passed)
- Regression tests: Round-trip WAV sample preservation and Big-Endian byte stream formatting verified.

---

## PR 3 (Conditional): Native UniFFI Mobile Bridge (`apps/pdfcraft-android`)

- **Topic Branch:** `upstream/android-bridge`
- **Base Branch:** `upstream/main`
- **Type:** `feat(platform)`
- **Target Subsystem:** `apps/pdfcraft-android`
- **Status:** Blocked on maintainer consultation (RFC Issue)

### 1.1 Scope
Provides the standalone `cdylib` and UniFFI interface definition allowing mobile frontends (Android, and potentially iOS in the future) to consume the Rust engine.

### 1.2 Prerequisites
Must NOT be submitted before maintainers review and accept the proposal issue (see `docs/contributing/PROPOSAL_ISSUE_DRAFT.md`).
