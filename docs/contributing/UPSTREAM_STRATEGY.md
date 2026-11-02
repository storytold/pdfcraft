# Upstream Contribution Strategy

This document defines the strategy for contributing improvements from the **PdfCraft Android** fork back to the canonical upstream repository:

- **Upstream Repository:** [https://github.com/storytold/pdfcraft](https://github.com/storytold/pdfcraft) (`main` branch)
- **Fork Repository:** [https://github.com/dikshant363/pdfcraft-android](https://github.com/dikshant363/pdfcraft-android)
- **Maintainer Focus:** Clean-room Acrobat Pro parity, zero panics, memory safety, `#![forbid(unsafe_code)]`, strict asset provenance, headless automation tools.

---

## 1. Dual Objective

1. **Objective A — Independent Android Product (Track A):**
   - Provide a 100% native Kotlin + Jetpack Compose + Material 3 Android PDF reader and editor.
   - Maintain rapid iteration, Android-specific features (SAF, Jetpack Compose, edge-to-edge UI, tile rasterizer).
   - Hosted on `dikshant363/pdfcraft-android` branches (`android/development`, `feature/android-reader`, `feature/android-rendering`).

2. **Objective B — Upstream Contribution (Track B):**
   - Prepare and submit high-value, small, independently verifiable improvements to `storytold/pdfcraft:main`.
   - Ensure every upstream contribution adheres strictly to upstream rules (`AGENTS.md` and `CLAUDE.md`).
   - Maintain dedicated `upstream/<topic>` topic branches branched cleanly from `upstream/main`.

---

## 2. Maintainer Reconnaissance & Upstream Culture

Analysis of merged and reviewed PRs (#5677, #5474, #5472, #5471, #5530, #5581, #5566) reveals key maintainer expectations:

1. **Small, Atomic Scopes:**
   - Maintainers reject large, mixed PRs. Changes must address one specific concern with an unambiguous title (e.g., `fix(annot): ...`, `M8: ...`).
2. **Never-Panic Invariant (`AGENTS.md` §4):**
   - No `unwrap()`, `expect()`, `panic!`, `todo!`, or `unimplemented!` outside tests.
   - Non-test code must return typed `Result` errors with `?`.
   - Arithmetic on input-derived numbers must be checked or saturating; recursion must be bounded; string slicing must be on character boundaries.
3. **Asset Provenance & Clean-Room (`AGENTS.md` §1 & §2):**
   - Zero Adobe assets, icons, fonts, or JS extractions.
   - Every asset must have an entry in `ATTRIBUTION.toml` and pass `cargo xtask assets`.
   - No GPL/AGPL code.
4. **Offline & Safety Boundaries:**
   - Never add outbound network requests.
   - Tools must respect `--root` confinement.
5. **Headless Automation Parity:**
   - Every user-facing feature should be reachable headlessly via `crates/automation` (`pdfcraft-cli run` and MCP).

---

## 3. Contribution Track Separation

```
┌────────────────────────────────────────────────────────────────────────┐
│                   PdfCraft Ecosystem Architecture                      │
├───────────────────────────────────┬────────────────────────────────────┤
│     Track A (Fork-Exclusive)      │     Track B (Upstream-Targeted)    │
├───────────────────────────────────┼────────────────────────────────────┤
│ • Kotlin Jetpack Compose UI       │ • Core engine bug fixes            │
│ • Android Activity / Lifecycle    │ • Voice Comment (Shape::Sound)     │
│ • Android SAF file pickers        │ • Automation tool alerts in apply  │
│ • Android TTS SpeechService       │ • Sound annotation appearance gen  │
│ • apps/pdfcraft-android (cdylib)  │ • ISO 32000-2 conformance tests    │
│ • .github/workflows/android.yml   │ • Portable API & documentation     │
└───────────────────────────────────┴────────────────────────────────────┘
```

**Rule:** Never submit Android-only UI or Gradle packaging code disguised as a generic Rust improvement. All upstream PRs must be platform-neutral, thoroughly tested on Linux, macOS, and Windows.

---

## 4. Upstream Contribution Priority

Contributions are prioritized in the following order:

1. **Phase 1 — Independent Engine Improvements & Bug Fixes:**
   - Surfacing script alerts and requests in `Automation::apply` (`crates/automation/src/documents.rs`).
   - ISO 32000-2 Sound Annotation support (`Shape::Sound`) in `crates/annot` and `crates/automation`.
2. **Phase 2 — Maintainer Consultation via RFC Issue:**
   - Present a detailed proposal to upstream maintainers outlining the native Android architecture and requesting guidance on whether an `apps/pdfcraft-android` subcrate is desired upstream.
3. **Phase 3 — Shared Infrastructure (Conditional on Maintainer Approval):**
   - If maintainers approve Android in-tree support, contribute `apps/pdfcraft-android` as a standalone crate with CI integration.
   - If maintainers prefer out-of-tree mobile development, maintain `apps/pdfcraft-android` in the fork while upstreaming all core engine needs.

---

## 5. Quality Gate Checklist for Every Upstream PR

Before any branch is pushed or proposed for PR:

- [ ] Branched directly from latest `upstream/main`.
- [ ] Code formatted via `cargo fmt --check`.
- [ ] Clippy clean with `cargo clippy -p <crate> --all-targets -- -D warnings`.
- [ ] Tests pass locally: `cargo test -p <crate>`.
- [ ] Asset compliance: `cargo xtask assets`.
- [ ] Layering check: `cargo xtask layers`.
- [ ] Regression test included, with verified failure before the fix.
- [ ] PR description follows `.github/pull_request_template.md`.
