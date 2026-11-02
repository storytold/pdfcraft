# Upstream Compatibility and Risk Analysis

This document details the architectural compatibility between **PdfCraft Android** and upstream **PdfCraft** (`storytold/pdfcraft:main`).

---

## 1. Engine & API Compatibility

### 1.1 Upstream v0.17.0 Alignment
The Android port was developed directly against upstream commit `0cba87ae` (v0.17.0):
- **Rust Edition:** 2024
- **Rust Minimum Supported Version (MSRV):** 1.95+ (built and tested on 1.97.1)
- **Crate Dependencies:** Reuses existing workspace crates without pinning conflicting forks.

### 1.2 Upstream Evolution Handling
Key differences between older forks (e.g., v0.14.0) and current upstream were addressed:
1. **RenderConfig Refactoring:** Upstream replaced `hide_notes: bool` with `hide_comments: bool`. The Android bridge conforms to `hide_comments`.
2. **Annotation Struct Evolution:** In upstream v0.17.0, `Annotation` was modularized into `crates/render/src/inspect/model.rs`. Direct access to raw internal fields was adapted to accessor methods and `CommentInfo` mapping.
3. **Automation Tool Structure:** Upstream split `a11y.rs` into `a11y/form_tools.rs`, `a11y/documents.rs`, and `a11y/ocr.rs`. Our automation changes conform to this modular layout.
4. **Script Alert Format:** Upstream returns structured `Alert` objects (`message`, `title`, `icon`, `buttons`) rather than flat strings. Both our tests and bridge consume this structured JSON format.

---

## 2. Safety & Invariant Guarantees

1. **`#![forbid(unsafe_code)]` Compliance:**
   - The workspace forbids unsafe Rust. No `unsafe` blocks are introduced in any engine modification or in `apps/pdfcraft-android/src/lib.rs`.
2. **Panic Boundary Isolation:**
   - Every FFI call in `apps/pdfcraft-android` is guarded with `std::panic::catch_unwind(AssertUnwindSafe(...))`.
   - Panics cannot escape into the Android JVM host process.
3. **No Network Invocation:**
   - The Android implementation does not request `android.permission.INTERNET`.
   - Engine and automation tools remain strictly local and offline.
4. **Confinement & SAF:**
   - File access strictly honours Android Storage Access Framework URIs and scoped app directories.

---

## 3. Asset & Licensing Compliance (`AGENTS.md` §1)

1. **Zero Adobe Assets:**
   - No Adobe icons, fonts, or copyrighted binaries exist in the port.
   - All icons are sourced from Lucide under the permissive ISC license.
   - Fonts used are SIL Open Font License (Inter).
2. **Attribution Integrity:**
   - Every bundled asset has a dedicated entry in `ATTRIBUTION.toml` with author, source, SPDX license, and SHA-256 hash.
   - `cargo xtask assets` passes with 0 violations across 323 repository assets.

---

## 4. Regression Analysis

| Subsystem | Change | Potential Risk | Mitigation & Evidence |
| :--- | :--- | :--- | :--- |
| `pdfcraft-automation` | Surfacing `alerts` and `requests` in `apply` | Caller JSON schema extension | `out["alerts"]` and `out["requests"]` are only populated when scripts actually fire. Backwards-compatible; all 318 automation tests pass unchanged. |
| `pdfcraft-annot` | Adding `Shape::Sound` enum variant | Non-exhaustive match arms | Exhaustive match arms updated across `types.rs`, `create.rs`, `appearance.rs`, `symbols.rs`, and `labels.rs`. All existing annotation tests pass. |
| `pdfcraft-engine` | Undo label `"Add voice comment"` | Missing label in localized tables | English label registered in `crates/engine/src/edit/labels.rs`. Non-breaking for unlocalized views. |
