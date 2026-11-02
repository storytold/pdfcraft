# Upstream Change Classification Matrix

This matrix classifies every change currently present in the Android port fork (`android-port`) to evaluate its suitability for upstream contribution to `storytold/pdfcraft:main`.

---

## 1. Subsystem Classification Matrix

| File / Component | Category | Upstream Reusability | Target Subsystem | Architectural Rationale & Upstream Value | Proposed PR Topic |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `crates/automation/src/documents.rs`<br>`crates/automation/src/a11y/form_tools.rs` | Bug Fix / API Enhancement | **HIGH (Immediate)** | `pdfcraft-automation` | When `Automation::apply` executes document edits (e.g., `form_fill`), form calculation or validation scripts can fire alerts or requests. Previously these were discarded or left in session state. Surfacing them in `out["alerts"]` and `out["requests"]` allows callers (CLI, MCP, GUI) to show the alerts. | `upstream/automation-apply-alerts` |
| `crates/annot/src/types.rs`<br>`crates/annot/src/create.rs`<br>`crates/annot/src/appearance.rs`<br>`crates/annot/src/appearance/symbols.rs`<br>`crates/engine/src/edit/labels.rs` | Feature / Conformance | **HIGH (Immediate)** | `pdfcraft-annot`<br>`pdfcraft-engine` | Implements ISO 32000-2 §12.5.6.15 Sound Annotations (`Shape::Sound`). Allows creating, viewing, and exporting voice comments in PDF documents with vector mic icon appearances. | `upstream/sound-annotations` |
| `crates/automation/src/comments.rs` | Tool Feature | **HIGH (Immediate)** | `pdfcraft-automation` | Adds `"sound"` comment creation tool handling 16-bit PCM WAV audio import with sample endianness conversion. Extends the headless comment API. | `upstream/sound-annotations` |
| `assets/icons/` (34 Lucide SVGs)<br>`ATTRIBUTION.toml`<br>`ATTRIBUTION.md` | Asset Addition | **MEDIUM (Conditional)** | `assets/`<br>`root` | 34 Lucide ISC icons added for mobile UI navigation (`arrow-left`, `volume-2`, `pause`, etc.). Fully attributed with verified SHA-256 hashes. If upstream adopts mobile UI or wishes to expand its desktop icon set, these are 100% compliant. | `upstream/mobile-assets` (or keep in fork) |
| `apps/pdfcraft-android/src/lib.rs`<br>`apps/pdfcraft-android/src/sound.rs`<br>`apps/pdfcraft-android/src/tests.rs`<br>`apps/pdfcraft-android/Cargo.toml`<br>`apps/pdfcraft-android/uniffi.toml` | New Platform Crate | **CONDITIONAL (RFC)** | `apps/pdfcraft-android` | Native UniFFI bridge, mobile session handle, tile rendering API, voice comment WAV export. Requires maintainer consensus on whether Android belongs in upstream tree. | `upstream/android-bridge` (after RFC) |
| `apps/pdfcraft-android/bindgen/` | Tooling Crate | **CONDITIONAL (RFC)** | `apps/pdfcraft-android` | Build-time binary runner for UniFFI Kotlin binding generation. | `upstream/android-bridge` (after RFC) |
| `apps/pdfcraft-android/android/` (Kotlin App, Compose UI, Gradle) | Native Mobile App | **FORK ONLY (Track A)** | `apps/` | Complete Kotlin Jetpack Compose application. Should remain in `dikshant363/pdfcraft-android` unless maintainers explicitly request in-tree mobile frontend maintenance. | **Track A Only** |
| `.github/workflows/android.yml` | CI Workflow | **FORK ONLY (Track A)** | `.github/workflows/` | Per `AGENTS.md` and `CLAUDE.md`, upstream CI workflows use shared `storytold/craft-common-actions`. No custom CI workflows are accepted in feature PRs. | **Track A Only** |
| `docs/android/` | Documentation | **FORK ONLY (Track A)** | `docs/` | Android-specific architectural documentation, build instructions, and security guidelines. | **Track A Only** |

---

## 2. Priority Upstream Batches

### Batch 1: Automation Tool Script Alerts (`upstream/automation-apply-alerts`)
- **Files:** `crates/automation/src/documents.rs`, `crates/automation/src/a11y/form_tools.rs`, `crates/automation/src/a11y.rs`.
- **Scope:** 15 lines of code, 0 breaking changes.
- **Upstream Benefit:** When CLI or MCP tools fill form fields, any alert boxes or navigation requests raised by AcroForm calculations are returned in the JSON response rather than lost.

### Batch 2: ISO 32000-2 Sound Annotation Support (`upstream/sound-annotations`)
- **Files:** `crates/annot/src/*`, `crates/engine/src/edit/labels.rs`, `crates/automation/src/comments.rs`.
- **Scope:** 80 lines of code, `#![forbid(unsafe_code)]`, tested by unit and integration tests.
- **Upstream Benefit:** Extends Acrobat parity for comment annotations by supporting Voice Comments and Sound streams.

### Batch 3: RFC Issue Proposal (Android Native Integration)
- **Scope:** Formal discussion issue before submitting any Android-specific code.
