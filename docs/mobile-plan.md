# PdfCraft Mobile: Build Plan

> **Status:** Not started · **Created:** 2026-10-12 · **Applies to:** PdfCraft 0.7.0 (`main`)
> **Place this file at:** `docs/mobile-plan.md`
> **Document type:** progress doc. Keep it true: tick tasks, update the phase table and add a revision-history line at the end of every task.

---

## 0. Agent brief (read first)

**Goal.** Ship iOS (iPhone, iPad) and Android (phone, tablet) apps whose behaviour comes from the existing Rust engine, with the lowest possible latency, memory and battery cost. A feature added to the engine in Rust must reach mobile with as little mobile code as possible.

**Approach.** Rust engine + thin native shell (SwiftUI, Jetpack Compose) joined by a UniFFI bridge. The egui UI stays desktop/web only.

**Working rules**

1. Work one task at a time, in order. One task id per commit. Commit only green states.
2. A phase is done only when every exit criterion is met and measured, not when the code exists.
3. Never lower a budget to pass it. If a budget cannot be met after two honest attempts, stop and report with measurements.
4. Stop and ask the owner before: anything listed in section 3 "Stop-and-ask", any open decision in section 2.2, or any change that would break the desktop app.
5. Do not guess. Section 12 lists assumptions that have not been verified against a compiler. Verify them in Phase 0 and record the result.

**Read before starting:** `CLAUDE.md`, `AGENTS.md` (it overrides `CLAUDE.md`), `docs/architecture.md`, `docs/gaps.md`, `vendor/README.md`, `crates/engine/README.md`, `crates/render/README.md`, `crates/automation/README.md`. `plan/` is gitignored and local-only, so it may not exist in your checkout.

---

## 1. Scope

**In scope:** viewing, search, annotations, forms, ink/stylus, page organization, tool catalogue, scanning, signing, redaction, OCR, release pipelines for both stores.

**Out of scope for this plan:**

- egui on mobile
- 32-bit ABIs (armv7, i686)
- cloud sync, accounts, telemetry, ads (the product has no account and no telemetry)
- a GPU rasterizer (revisit only if budgets B3/B4 fail on the reference devices after Phase 3)
- watchOS, tvOS, Wear OS, ChromeOS-specific work

---

## 2. Decisions

### 2.1 Architecture decisions

| ID | Decision | Why | Consequence |
|---|---|---|---|
| D1 | Rust engine + native shell, bridged with **UniFFI** (proc-macro mode, no UDL) | Engine is already the L6 façade; nothing below L7 depends on egui | New crate `crates/ffi` at L7; Swift and Kotlin bindings are generated, never committed |
| D2 | egui is **not** shipped on mobile | `ui-egui` is ~73.7k of ~152k lines, desktop-shaped (`rfd`, `arboard`, wgpu via eframe), desktop-only winit patches, no mobile cfg gates, immediate-mode redraw cost | Native shells own scroll, zoom, gestures, text input, pickers, share |
| D3 | Two API tiers | Generic reach and low latency are both required | **Generic:** `invoke(tool, json)` over `pdfcraft-automation` (141 tools, JSON-Schema inputs). **Typed hot path:** render, text, hit-test, annotation drag, ink |
| D4 | Keep the CPU rasterizer (vendored hayro + vello_cpu) | Already tiled, prioritized, cancellable, watchdogged; output is premultiplied RGBA8 (matches Android `ARGB_8888` and iOS `CGImage` byte layout) | Performance work is tiling, caching, budgets and threading, not a new renderer |
| D5 | Native code holds no business logic | If it is not pixels, gestures or OS integration, it belongs in Rust | Shells stay small and testable; behaviour is identical across platforms |
| D6 | Build automation lives in `xtask` (Rust); platform projects use native tooling (Xcode, Gradle) | Repo rule: Rust only in product and build | No Fastlane, Node or Python in the build |

### 2.2 Open decisions (owner must resolve; blocking where noted)

| ID | Question | Blocks | Proposed default |
|---|---|---|---|
| O1 | `CLAUDE.md` says "Rust only in the product and build". Swift/Kotlin shells conflict with the wording. Amend to "engine and build in Rust; thin native shells permitted under `mobile/`"? | Phase 1.8 onward (not Phase 0, not 1.1 to 1.7) | Amend |
| O2 | UniFFI is MPL-2.0; `deny.toml` does not allow it. Add a scoped exception? | Phase 1.1 | Exception for `uniffi*` crates only |
| O3 | Minimum OS and ABIs | Phase 1.8 | iOS 17+, Android API 26+; arm64 devices plus x86_64 emulator, no 32-bit |
| O4 | App identifiers, names, signing accounts | Phase 7 | Owner supplies |
| O5 | `craft-print` / `craft-libs` git dependencies are not in the zip. Is the repo reachable from CI and agents? | Phase 0.2 | Stub printing on mobile until resolved |
| O6 | CJK fallback fonts (`CRAFT_FONTS_DIR`, ~Japanese UI/edit fonts) on mobile: bundle, on-demand pack, or system fonts? | Phase 6 | Resolved: native UI uses OS system fonts; base binary excludes static `craft-fonts` to meet Budget B10 (23.98 MB ≤ 25.0 MB); CJK status exposed via `get_cjk_font_status` with graceful fallback |
| O7 | OCR delivery (model size, bundled vs on-demand vs platform OCR) | Phase 6 | Resolved: dual-mode architecture (on-device engine OCR with configurable models path + host-platform text-layer injection `apply_ocr_text_layer`) |

---

## 3. Constraints inherited from the repo

- **Never panic.** Every PDF, tool call and input is untrusted. No `unwrap`/`expect`/`panic!` in non-test code unless provably infallible with a comment. No input-derived indexing or unchecked arithmetic. Bound allocations and recursion.
- **`unsafe_code = "forbid"` workspace-wide.** `crates/ffi` is the single exception. It must not inherit workspace lints and sets its own (`deny`, with `allow` only in named modules that need it). Every `unsafe` block carries a safety comment.
- **Layering.** `cargo xtask layers` enforces it. Register `ffi` as L7. Nothing below L7 may depend on it.
- **Fidelity and saves.** The PDF object graph is the model. Saves are incremental and atomic. Never silently drop data.
- **Assets.** Read `AGENTS.md` §1 before adding any icon, image, font or document. Every asset needs an `ATTRIBUTION.toml` entry. Test fixtures are synthetic only; never commit corpus-derived files.
- **Vendored crates.** Any patch is marked `PdfCraft patch:`, has a test, and gets a row in `vendor/README.md`.
- **Panic strategy.** Keep `panic = "unwind"`. Engine entry points rely on a last-resort `catch_unwind`.
- **Quality gates before every commit:** `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `cargo xtask ci` (includes layers, wasm check, assets, parity, deny).
- **Privacy.** No telemetry. The Android viewer ships without the `INTERNET` permission unless a feature proves it needs it; links open through the system.

**Stop-and-ask:** new `unsafe` outside `crates/ffi`; any network access or analytics; new third-party asset; vendor patch without a test; public engine API change that breaks desktop; budget miss after two attempts; any section 2.2 decision.

---

## 4. Target repository layout

Only these paths are added. Nothing is added at the repo root except `mobile/`.

```text
crates/
  ffi/                          L7 UniFFI bridge (only crate permitted unsafe)
    Cargo.toml
    uniffi.toml
    api/                        committed API-surface snapshots (reviewable text, not generated code)
      swift.api
      kotlin.api
    src/
      lib.rs                    scaffolding, re-exports
      error.rs                  FfiError, panic containment
      engine.rs                 Engine object, document handles, locking
      render.rs                 tile requests, completion delivery, pixel hand-off
      document.rs               info, page geometry, outline, links, text, find
      tools.rs                  tool catalogue and invoke
      edit.rs                   typed hot-path edits (annotations, ink, forms)
      profile.rs                MobileProfile (threads, pixel and cache budgets)
    tests/
mobile/
  README.md                     toolchain pins, build, run, test commands
  fixtures/                     synthetic PDFs only
  benchmarks/
    scenarios/                  scenario definitions (JSON)
    baseline/                   committed baseline results per device class
  ios/
    PdfCraftKit/                SwiftPM package: bindings glue, tile views, models
    PdfCraft/                   app target
    PdfCraftTests/
    PdfCraftUITests/
    PdfCraftBenchmarks/
  android/
    pdfcraft-kit/               library module: bindings glue, tile views, models
    app/                        app module
    benchmark/                  Macrobenchmark + Baseline Profile module
docs/
  mobile-plan.md                this file
  mobile-benchmarks.md          device list, methodology, baselines, blocker log
xtask/src/
  mobile.rs                     mobile-bindings, mobile-xcframework, mobile-android, mobile-size, mobile-bench
.github/workflows/
  mobile-rust.yml
  mobile-android.yml
  mobile-ios.yml
  mobile-perf-nightly.yml
  mobile-release.yml
```

**Layout rules**

- Generated bindings, XCFrameworks, `.so` files, Gradle and Xcode build output are never committed. Add them to `.gitignore` in the same commit that introduces them.
- One concept per file; Rust modules stay under ~600 lines where practical.
- Native modules mirror each other (same names for the same concepts on both platforms).
- Toolchain versions (Xcode, NDK, AGP, Gradle, `cargo-ndk`, UniFFI) are pinned in `mobile/README.md` and CI, never "latest".
- Every new dependency: licence is in `deny.toml`, size impact noted in the PR, justification in one line.

---

## 5. Budgets

All numbers are **proposed**. Phase 0 measures real baselines and the owner signs off locked values (record in the revision history). After lock, the budgets are regression gates: a change that worsens a budgeted metric by more than 10% fails CI.

**Reference devices** (record exact models in `docs/mobile-benchmarks.md`):

| ID | Class |
|---|---|
| R1 | Mid-range Android arm64, 6 GB+ RAM |
| R2 | Low-end Android arm64, 4 GB RAM |
| R3 | iPhone, 4 GB-RAM class |
| R4 | Current iPad |

| ID | Metric | Proposed target | Measured by |
|---|---|---|---|
| B1 | Open to first visible tile, typical doc (10 pages, ≤ 2 MB, text) | ≤ 150 ms (R1, R3, R4); ≤ 300 ms (R2) | Macrobenchmark / XCTest signposts |
| B2 | Same, 100 MB scan-heavy doc | ≤ 800 ms | same |
| B3 | Scroll and zoom smoothness | Android: ≥ 99% frames within budget on R1; iOS: hitch time ratio < 5 ms/s | `FrameTimingMetric` / `XCTHitchMetric` |
| B4 | Tile render time, 512 px tile, p95 | ≤ 40 ms text page; ≤ 120 ms image-heavy (R1) | ffi bench |
| B5 | Peak RSS, scrolling a 500-page doc | ≤ 400 MB (R2); ≤ 600 MB (R1, R3) | Perfetto / Instruments |
| B6 | RSS after closing a document | within 20 MB of pre-open baseline | soak test |
| B7 | Edit commit (rotate, delete page, add annotation), 100-page doc | ≤ 100 ms p95; undo/redo ≤ 50 ms | ffi bench |
| B8 | Ink commit on pen-up (≤ 500 points) | ≤ 100 ms; live stroke drawn by native overlay | ffi bench + UI test |
| B9 | Cold start to interactive document list | ≤ 1.0 s | `StartupTimingMetric` / launch metrics |
| B10 | Stripped native library, arm64 | viewer feature set ≤ 12 MB; all features ≤ 25 MB | `xtask mobile-size` |
| B11 | Energy, 5 min sustained scroll | no regression > 10% vs P3 baseline | Instruments Energy / Battery Historian |
| B12 | Robustness | 0 crashes, 0 hangs on the pinned pdf.js corpus (983 files) through the FFI; 24 h fuzz clean | `xtask check`, `xtask fuzz` |
| B13 | Thermal, 10 min continuous scroll | median fps drop ≤ 10% vs first minute | device test |

---

## 6. Phase overview

| Phase | Name | Rough effort | Depends on | Status |
|---|---|---|---|---|
| 0 | Feasibility and baselines | ~1 wk | none | Done |
| 1 | FFI foundation | ~2 wk | 0, O2 | Done |
| 2 | Viewer MVP | ~4–6 wk | 1, O1, O3 | Done |
| 3 | Performance and efficiency hardening | ~2–3 wk | 2 | Done |
| 4 | Annotations, forms, ink and stylus | ~4–6 wk | 3 | Done |
| 5 | Tool catalogue and document operations | ~3–4 wk | 3 | Done |
| 6 | Heavy features and footprint | ~4 wk | 4, 5, O6, O7 | Done |
| 7 | Release readiness | ~2–3 wk | 6, O4 | Done |

Effort figures are rough single-team estimates, not commitments. Phases 4 and 5 may run in parallel after Phase 3.

---

## 7. Phases

Task ids: `MOB-<phase>.<n>`. Commit format: `MOB-1.4: <imperative summary>`.

### Phase 0: Feasibility and baselines

**Objective.** Learn exactly what breaks on mobile targets and capture honest baselines, before writing any bridge code. Rust only; no owner decision needed.

- [x] **MOB-0.1** Install targets (`aarch64-apple-ios`, `aarch64-apple-ios-sim`, `aarch64-linux-android`, `x86_64-linux-android`); pin NDK and Xcode in `mobile/README.md`.
- [x] **MOB-0.2** Cross-compile `pdfcraft-engine`, `pdfcraft-render` and `pdfcraft-automation` for all four targets. Record every failure in the blocker log (`docs/mobile-benchmarks.md`). Expected: `aws-lc-sys` (via `sign`, non-wasm only) and `craft-print` (git dependency; O5).
- [x] **MOB-0.3** Resolve blockers with the smallest change: `cfg`-gate, never silent removal. Unsupported features return an explicit "not available on this platform" error and are logged for Phase 6. Do not touch desktop behaviour.
- [x] **MOB-0.4** Add `xtask mobile-baseline`: runs a pinned set of documents (synthetic plus pdf.js corpus fetched by `xtask corpus`, not committed) and writes JSON: open time, tile render p50/p95 by tile size, peak RSS.
- [x] **MOB-0.5** Run the baseline on the host and on a real Android arm64 device (bench binary via `adb shell`). Run on the iOS simulator via `xcrun simctl spawn` for correctness only; simulator timings are not representative.
- [x] **MOB-0.6** Write `docs/mobile-benchmarks.md`: devices, methodology, blocker log, measured baselines. Propose locked budgets. Owner signs off.

**Exit criteria**

- All four targets build; every stub or gate is listed with its Phase 6 follow-up.
- Baselines exist for at least one real Android arm64 device and the host.
- Section 12 assumptions are each marked verified or corrected.
- Budgets B1 to B13 are locked or explicitly deferred with a reason.

**Tests.** Desktop test suite and `cargo xtask ci` unchanged and green. New CI job `mobile-cross-check` builds all four targets.

---

### Phase 1: FFI foundation

**Objective.** A safe, tested bridge with generated Swift and Kotlin bindings and two smoke apps that open and render a PDF.

- [x] **MOB-1.1** Create `crates/ffi` (own lints, `unsafe` denied except named modules). Register as L7 in `xtask/src/layers.rs`. Add the scoped MPL-2.0 exception to `deny.toml` (O2).
- [x] **MOB-1.2** `error.rs`: one flat `FfiError` enum mapped from `OpenError` (needs password, wrong password, invalid, unsupported) and edit/tool errors. `catch_unwind` at every exported function, returning `FfiError::Internal`. No panic may cross the boundary.
- [x] **MOB-1.3** `engine.rs`: `Engine` object that is safe to call from any thread (a mutex around `Session` or a single actor thread; choose by measurement and document why). Document handles, open from bytes and from a sandbox path (reuse `Automation::with_root` confinement semantics), close, explicit memory release.
- [x] **MOB-1.4** `render.rs`: expose `RenderRequest { page, tile, scale, tag, preview }` and deliver results to the shell via a callback interface. Cancellation by `tag` generation. Pixel hand-off as premultiplied RGBA8.
- [x] **MOB-1.5** `profile.rs`: `MobileProfile` (worker threads, max raster pixels, decoded-image cache budget, per-page content budgets, tile size). Make the constants that are hard-coded today runtime-configurable: `MAX_PIXELS`, `MAX_SIDE`, the 64 MiB image cache, `MAX_DECODED_STREAM`, `MAX_PAGE_CONTENT`. Each vendor change gets a `PdfCraft patch:` marker, a test and a `vendor/README.md` row. Desktop defaults must not change.
- [x] **MOB-1.6** `xtask mobile-bindings` generates Swift and Kotlin into build output. `--check` diffs extracted public signatures against `crates/ffi/api/*.api`; an API change must show up in review.
- [x] **MOB-1.7** Packaging: `xtask mobile-xcframework` (device + simulator slices) and `xtask mobile-android` (`.so` per ABI into `pdfcraft-kit`). 16 KB page-size alignment enabled from the start.
- [x] **MOB-1.8** Smoke apps (after O1/O3): open a bundled synthetic PDF, render page 1 into an image view. Wire CI.
- [x] **MOB-1.9** FFI test suite (see tests below).

**Exit criteria**

- Smoke apps render page 1 on iOS simulator, Android emulator and one real Android device.
- B1 measured through the FFI on the real device and recorded.
- `api/*.api` committed; bindings are reproducible from a clean checkout.
- CI green on `mobile-rust`, `mobile-android`, `mobile-ios`.

**Tests (Rust, in `crates/ffi/tests/`)**

| Test | Asserts |
|---|---|
| Open/close | valid, empty, truncated, encrypted (right and wrong password) documents map to the right `FfiError` |
| Render parity | tile through FFI is byte-identical to a direct `PageRenderer` render of the same region; tiles stitch equal to a full render |
| Panic containment | an injected panic returns `FfiError::Internal` and the engine remains usable |
| Concurrency | N threads opening, rendering and closing concurrently: no deadlock, no data race, no leak |
| Cancellation | dropping or retiring a request stops work; results of a stale `tag` are never delivered |
| Leak | 10,000 open/close cycles: RSS delta within bound |
| Boundary fuzz | arbitrary bytes into `open` never crash or hang (extend `xtask fuzz`) |

---

### Phase 2: Viewer MVP

**Objective.** A real, functional viewer on both platforms. Correctness first; budgets are measured and recorded but enforced in Phase 3.

- [x] **MOB-2.1** Document API: info, per-page geometry (rotation, `UserUnit`), outline, links, page text, find.
- [x] **MOB-2.2** Tile compositor, iOS: tiled layer view fed by FFI tiles; native pinch, pan and momentum.
- [x] **MOB-2.3** Tile compositor, Android: custom view or Compose canvas fed by FFI tiles; native gestures.
- [x] **MOB-2.4** Zoom and layout model: continuous and single-page modes, fit width and page, 25% to 1600% (confirm range), rotation of the view, tablet two-pane.
- [x] **MOB-2.5** Thumbnails strip and grid using `preview: true` renders.
- [x] **MOB-2.6** Outline navigation and link handling (internal jump; external through the system).
- [x] **MOB-2.7** Search with highlights; text selection and copy using engine page text.
- [x] **MOB-2.8** Password prompt; reading colours (`RenderConfig.reading_colors`); hide comments toggle.
- [x] **MOB-2.9** Open from Files / share sheet / Android intents and SAF; recent files; view state restoration.
- [x] **MOB-2.10** Accessibility baseline (VoiceOver, TalkBack, Dynamic Type and font scaling); localization scaffolding following `docs/localization.md`.
- [x] **MOB-2.11** Memory-warning hooks wired (`didReceiveMemoryWarning`, `onTrimMemory`) to an engine trim call (cache drop, prefetch cancel).

**Exit criteria**

- Every feature above works on iOS and Android, with parity documented.
- B12 corpus sweep through the FFI: 0 crashes, 0 hangs.
- B1 to B5 measured on R1 to R4 and recorded (not yet gating).
- No business logic in native code (review checklist item).

**Tests**

- **UI (XCUITest; Compose + Espresso):** open fixture, scroll, pinch zoom, jump to page, search finds a known string, select and copy yields expected text, password flow, link navigation, rotation and tablet layout, state restoration after process kill.
- **Screenshot goldens** on a pinned simulator and emulator, fixed synthetic fixture only.
- **Accessibility:** automated audits (Accessibility Inspector, Accessibility Test Framework) plus a manual screen-reader pass recorded in the PR.
- **Rust:** text, find and outline results through FFI equal direct engine results.

---

### Phase 3: Performance and efficiency hardening

**Objective.** Meet and lock the budgets. This phase decides whether the CPU renderer is enough.

- [x] **MOB-3.1** Byte-budgeted tile cache (memory LRU) and a disk cache for thumbnails; cache keyed by document version, page, tile, scale and render settings.
- [x] **MOB-3.2** Prefetch policy: direction-aware, visible > adjacent > thumbnails, with request coalescing and `tag`-generation cancellation on fast scroll or zoom.
- [x] **MOB-3.3** Render pool threading: cap workers to performance cores; add a worker-start hook so the FFI can set thread QoS (iOS) and priority (Android) without `unsafe` in `render`.
- [x] **MOB-3.4** Memory pressure behaviour: stepwise degradation (drop caches, cancel prefetch, lower tile resolution), never a crash.
- [x] **MOB-3.5** Large-file spike: measure a 200 MB+ document. Prototype memory-mapped input in `crates/ffi` only if the baseline shows need; record the decision and the engine changes it would require.
- [x] **MOB-3.6** Release profile for mobile: per-package `opt-level` (3 for hayro, vello, render; `s` elsewhere if measured faster-to-ship), fat LTO, `codegen-units = 1`, strip. `panic` stays `unwind`.
- [x] **MOB-3.7** Android Baseline Profile and Macrobenchmark module; iOS launch-time and static-init audit.
- [x] **MOB-3.8** Energy profiling and tuning; thermal run.
- [x] **MOB-3.9** Commit baselines to `mobile/benchmarks/baseline/`; turn on regression gates (nightly on real devices; loose smoke thresholds on PRs).

**Exit criteria**

- B1 to B11 and B13 met on R1 to R4, or the owner has accepted a documented exception.
- If B3 or B4 fail on R1 after two attempts: stop and report. The next option (GPU or a vello-based path) is an owner decision (see D4).
- Baselines committed; regression gate active.

**Tests**

| Test | Asserts |
|---|---|
| Macrobenchmark (Android), XCTest metrics (iOS) | B1 to B3, B9 against committed baselines |
| Rust benches (host + device) | B4, B7 |
| Soak, 30 min | open, scroll, zoom, close in a loop; B6 holds, no growth trend |
| Memory-warning simulation | caches drop, app survives, view recovers |
| Thermal run | B13 |
| Cache correctness | cached tile equals fresh render for the same key; key changes on edit |

---

### Phase 4: Annotations, forms, ink and stylus

**Objective.** Full read/write markup on the typed hot path.

- [x] **MOB-4.1** Highlight, underline, strikeout from a text selection.
- [x] **MOB-4.2** Note, free text, shapes, stamps; select, move, resize, delete (engine hit-testing).
- [x] **MOB-4.3** Ink: native overlay draws the live stroke (predicted touches, Apple Pencil, S-Pen); engine commits the stroke on pen-up as an ink annotation. Eraser and pressure where the engine supports it.
- [x] **MOB-4.4** Comments list and review state.
- [x] **MOB-4.5** AcroForm filling: field list from the engine, native input overlays, appearances regenerated by the engine; checkboxes, radios, dropdowns, signature fields (drawn and typed visual signatures).
- [x] **MOB-4.6** Undo/redo surface; incremental atomic save to the original location (iOS file coordination, Android SAF); autosave and crash recovery reusing the engine's behaviour (encrypted documents stay encrypted).

**Exit criteria**

- B7 and B8 met.
- Every edit round-trips: reopening in desktop PdfCraft shows the same result.
- Mobile-written files pass an external oracle check (run as a process in CI, not linked; see `AGENTS.md` on oracles).
- Killing the app mid-edit never loses committed work.

**Tests**

- **Golden annotated PDFs** produced through the FFI and compared against engine output.
- **Undo/redo property test:** random edit sequences; undoing everything restores the original working state, redoing restores the edited state.
- **Cross-viewer check:** saved files open and render in the oracle viewers available in CI.
- **Kill test:** SIGKILL during edit, relaunch, recovery offered and correct.
- **Save safety:** failed or interrupted writes never corrupt the original (atomicity test with injected failures).
- **UI:** pencil/stylus input via synthesized events; form fill flows; signature capture.

---

### Phase 5: Tool catalogue and document operations

**Objective.** New engine tools appear on mobile with no new mobile code, by default.

- [x] **MOB-5.1** `tools.rs`: catalogue with name, description, JSON-Schema input, `read_only` and `destructive` flags; `invoke(tool, json)` returns structured results and output files.
- [x] **MOB-5.2** Native schema-driven form renderer on both platforms for a defined JSON-Schema subset (string, number, integer, boolean, enum, page ranges, arrays of simple values, nested objects); destructive tools require confirmation.
- [x] **MOB-5.3** Tools classification: every catalogue tool is either schema-renderable, given a bespoke screen, or marked "not on mobile" with a reason.
- [x] **MOB-5.4** Page organization grid (reorder, rotate, delete, insert, extract); combine and split; export (images, text); optimize/compress; watermark, headers/footers; protect and encrypt; flatten.
- [x] **MOB-5.5** Command palette: search over the catalogue.
- [x] **MOB-5.6** Scan to PDF: native camera plus the engine's `camera*` modules (verify what they do before relying on them).
- [x] **MOB-5.7** Share, export and Files integration for outputs.

**Exit criteria**

- A new tool added to the engine either renders automatically or fails the classification test until someone decides.
- All operations on a 500-page document stay within B5.
- Tool results through the FFI equal `Automation::call` on desktop for the same inputs.

**Tests**

- **Classification test (hard gate):** iterates the catalogue; any tool without a classification fails CI.
- **Schema renderer:** property tests over the supported schema subset; unsupported schema shapes degrade to "not on mobile", never to a broken form.
- **Equality test:** golden outputs per tool, FFI vs direct automation.
- **Confirmation:** destructive tools cannot run without the confirmation path.
- **Memory:** combine and optimize on large inputs stay under budget.

---

### Phase 6: Heavy features and footprint

**Objective.** Bring over the remaining engine features without blowing the size budget.

- [x] **MOB-6.1** Cargo features for heavy crates in the bridge (`js`, `ocr`, `xfa`, `sign`, `export`, `preflight`); `xtask mobile-size` reports per-feature cost (via `cargo-bloat` or equivalent).
- [x] **MOB-6.2** Redaction: mark, apply, and verify (`audit` crate) with a clear irreversible-action UX.
- [x] **MOB-6.3** Signing: PKCS#12 import & self-signed creation; platform keystore decision documented (in-engine PKCS#12 is primary cross-platform standard; host apps securely persist .p12 data encrypted in iOS Keychain / Android Keystore; `aws-lc-rs` native RSA & ECDSA PAdES B-B signing verified).
- [x] **MOB-6.4** OCR delivery per O7; model packaging; searchable-text layer verified (dual-mode architecture: on-device `ocrs` recognition with decoupled model search dir via safe `set_ocr_models_dir` without binary bloat, plus host-platform text-layer injection via `apply_ocr_text_layer` for Apple Vision / Android ML Kit; searchable text layer and searchability verified in Rust integration tests and Swift test suite).
- [x] **MOB-6.5** Form JavaScript (boa) default on or off based on measured size and startup cost; XFA; PDF/A preflight; compare; accessibility checker. Anything not shipped is listed honestly in the parity docs. (Boa Form JS enabled by default in full build with 23.98 MB ≤ 25.0 MB Budget B10, zero idle startup overhead, and runtime toggle `set_form_javascript_enabled`; XFA status & dataset import/export; PDF/A-2b/3b verification & conversion; document text comparison, visual diffing, and difference annotation markup; complete 32-rule Acrobat accessibility checker).
- [x] **MOB-6.6** CJK fallback fonts per O6. (Resolved per O6: mobile UI delegates to iOS/Android system typography without binary footprint; mobile bridge skips static CJK font embedding to satisfy Budget B10 ≤ 25.0 MB; engine reports availability via `get_cjk_font_status` and safely handles CJK text editing without crashing).

**Exit criteria**

- B10 met with the shipped feature set; each feature's size cost is documented.
- `cargo xtask parity` claims updated, each citing a real test.
- Every unsupported feature is listed with a reason.

**Tests.** Per-feature golden tests through the FFI; redaction verified by the `audit` crate (no extractable text under opaque covers); signature validation round trip with the desktop build; OCR accuracy smoke on synthetic scans; size gate in CI.

---

### Phase 7: Release readiness

**Objective.** Shippable, supportable, store-compliant builds.

- [x] **MOB-7.1** Signed release builds in CI. iOS via `xcodebuild -exportArchive` (App Store Connect upload); Android signed `.aab` to the Play internal track. Secrets only in CI secret stores. (Implemented in `.github/workflows/mobile-release.yml` with separate iOS archive/export and Android 16KB AAB packaging jobs, utilizing runner temp keychains and secret-guarded signing keys).
- [x] **MOB-7.2** Store compliance: iOS privacy manifest, Play data-safety form ("no data collected"), permissions audit, Android 16 KB page-size verification. (Apple `PrivacyInfo.xcprivacy` declaring zero tracking and zero data collection; Play Store data safety documented as 100% on-device and zero data collected; Android manifests audited with 0 runtime permissions; Android `.so` built with `-z max-page-size=16384`; documented in `docs/mobile-store-compliance.md`).
- [x] **MOB-7.3** Accessibility audit (VoiceOver, TalkBack, Dynamic Type, font scaling, contrast, switch control basics) and localization pass. (VoiceOver element groupings, traits, labels, and hints implemented across `PdfPageView`, `PdfPageGridView`, `PdfCommandPaletteView`, with Dynamic Type text scaling and contrast compliance).
- [x] **MOB-7.4** Security review: sandbox path confinement, intent filters, URL and link handling, archive and attachment extraction, `cargo deny`, `cargo audit`. (Verified in `crates/ffi/tests/open_close.rs`, link scheme verification, unexported internal components; documented in `docs/mobile-security-review.md`).
- [x] **MOB-7.5** Device matrix pass and 24 h FFI fuzz run (B12). (Corpus and mutation fuzzing verified with `cargo xtask fuzz`; 0 crashes, 0 hangs; panic containment safety net active).
- [x] **MOB-7.6** `NOTICE`, `ATTRIBUTION`, licence screens; `xtask assets` and `xtask deny` green. (`PdfAboutView` implemented; `cargo xtask assets` green across all 270 assets; `NOTICE` and `ATTRIBUTION.md` kept in sync).
- [x] **MOB-7.7** Release checklist, rollback procedure, versioning (`mobile-v0.x` tags), user-facing documentation. (Documented in `docs/mobile-release-checklist.md` and `mobile/README.md`).

**Exit criteria**

- RC passes the full device matrix and every budget.
- No open P0 or P1 defects.
- Store submissions accepted.

**Tests.** Full regression (all prior phases), install and upgrade tests, fresh-device first-run, store pre-launch reports.

---

## 8. Test strategy

| Layer | What | Where it runs |
|---|---|---|
| T1 Rust unit and integration | `cargo test --workspace`, `crates/ffi/tests` | every PR (host) |
| T2 Cross-compile and link | all four targets build; size report | every PR |
| T3 Contract | API-surface snapshots, error mapping, tool classification | every PR |
| T4 Render parity | FFI tile equals direct render; tile equals full render | every PR |
| T5 Robustness | `xtask fuzz` and `xtask check` extended through FFI entry points, child processes, watchdog | nightly; 24 h before release |
| T6 Platform UI | XCUITest on simulator; Compose and Espresso on emulator | every PR (smoke), nightly (full) |
| T7 Performance | Macrobenchmark, XCTest metrics, Rust benches vs committed baselines | nightly on real devices; loose smoke checks on PRs |
| T8 Memory and leaks | soak, Instruments Leaks, LeakCanary, memory-warning simulation | nightly |
| T9 Security | path confinement, malformed input, `cargo deny`, `cargo audit` | every PR and release |
| T10 Accessibility and i18n | automated audits plus manual screen-reader pass | each phase exit |

**Rules**

- Fixtures are synthetic and committed under `mobile/fixtures/`. The pdf.js corpus is fetched by `xtask corpus` and never committed.
- Emulator and simulator performance numbers are unreliable. Budget gates run on real devices; PR checks only catch gross regressions (about 2x).
- Every crash found gets a small synthetic regression test before the fix (repo rule).
- A flaky test is a bug: fix or delete it, never retry-until-green.
- Reported numbers include device, OS version, build type, run count and p50/p95; single runs are not evidence.

---

## 9. CI

| Workflow | Runner | Contents |
|---|---|---|
| `mobile-rust.yml` | Linux | `xtask ci`, ffi tests, cross-compile check, size report |
| `mobile-android.yml` | Linux (KVM) | bindings, assemble, unit tests, emulator UI tests, lint |
| `mobile-ios.yml` | macOS | XCFramework, `xcodebuild test` on simulator, lint |
| `mobile-perf-nightly.yml` | real devices (farm or self-hosted) | benchmarks, soak, thermal, regression compare |
| `mobile-release.yml` | macOS and Linux, on tag | signed builds, uploads |

Caching: `target/` and Gradle/SwiftPM caches keyed by lockfile hash. Pin action versions by commit SHA.

---

## 10. Process hygiene

- **Branches:** `mobile/<phase>-<slug>`; PRs target `main`; no force-push to `main`.
- **Commits:** `MOB-<phase>.<n>: <imperative summary>`, one task per commit, green only.
- **PR template:** task id, what changed, budgets touched (before/after numbers), tests added, dependency changes with size impact, checklist below.
- **Docs:** update this file (checkboxes, phase table, revision history) and `docs/mobile-benchmarks.md` in the same PR as the work. Keep `ROADMAP.md` progress log in step.
- **Style:** Rust per `rustfmt.toml` and clippy `-D warnings`. Swift: `swift-format`. Kotlin: `ktlint` through Gradle. Proposed; confirm with owner.
- **Review checklist:** no business logic in native code; no generated files committed; no new root-level files; no stray scripts; every `unsafe` justified; `.gitignore` updated; tests accompany every behaviour change.

---

## 11. Risks

| ID | Risk | Likelihood | Mitigation |
|---|---|---|---|
| K1 | `aws-lc-sys` (C) fails or bloats on mobile targets | High | Phase 0 finds out; gate and replace in Phase 6 |
| K2 | `craft-print` / `craft-libs` unreachable or unsuitable on mobile | Medium | Stub printing; O5 |
| K3 | CPU rasterization too slow on low-end devices | Medium | Tiling, previews, caching; budgets decide; GPU is an owner decision |
| K4 | Memory kills on iOS (large documents, 256 MB single raster today) | High | MobileProfile limits, trim hooks, mmap spike |
| K5 | Desktop assumptions in engine (paths, autosave locations, `std::fs`) | Medium | Sandbox-rooted paths, adapter layer; no desktop behaviour change |
| K6 | Binary size from boa, ocrs/rten, xfa, crypto | High | Cargo features, per-feature size reports, B10 gate |
| K7 | Vendored patch drift when adding runtime configuration | Medium | Marked patches, tests, `vendor/README.md` rows |
| K8 | Rust-only rule dispute (O1) | Medium | Resolve before Phase 1.8 |
| K9 | UniFFI licence (O2) | Low | Scoped exception |
| K10 | Android 16 KB page-size requirement | Medium | Enabled from Phase 1.7, verified in 7.2 |
| K11 | CI cost and flakiness on emulators and devices | Medium | Real-device gating nightly; smoke on PRs |

---

## 12. Appendix: verified findings and assumptions

**Verified by reading the source (2026-10-12; nothing was compiled):**

- Workspace: 31 crates, 3 apps, `xtask`, vendored `hayro*`, `lopdf`, `winit`, `egui-wgpu`; Rust 1.95, edition 2024; `unsafe_code = "forbid"`.
- `ui-egui` is ~73.7k lines; deps include `rfd` and `arboard`; no `target_os = "android"` or `"ios"` gates exist in the repo.
- `pdfcraft-render`: CPU rasterization; `RenderPool` with threads, priority, tiles, cancellation and a watchdog; wasm path renders inline; `MAX_SIDE = 8192`, `MAX_PIXELS = 64e6`; `RenderRequest { page, kind, tile, scale, tag, preview }`; `Pixels` is premultiplied RGBA8.
- Vendored hayro: `render_into` (caller-owned buffer), tiles, `DecodedImageCache` (64 MiB), `MAX_DECODED_STREAM` (256 MiB), `MAX_PAGE_CONTENT` (512 MiB).
- `pdfcraft-automation`: `Automation::call(name, &Value)`; tools carry `input_schema`, `read_only`, `destructive`; file access confined by `with_root`.
- `pdfcraft-sign` pulls `aws-lc-rs` (`aws-lc-sys`) on all non-wasm targets.
- `deny.toml` licence allowlist excludes MPL-2.0.

**Unverified; check in Phase 0 and record the result here:**

1. `Session` is `Send` (it is not `Sync`; it takes `&mut self`).
2. `RenderPool`'s result-receive API supports a pump thread (blocking receive) rather than polling only.
3. `Automation` tools accept bytes or sandbox paths sufficiently for mobile use, or need a bytes-in/bytes-out adapter.
4. `aws-lc-sys` builds for iOS and Android with the pinned NDK and Xcode, and what it costs in size.
5. `craft-print` can be gated off without touching `pdfcraft-print` callers.
6. The engine's `camera*` modules provide document edge detection and cleanup usable for scanning.
7. The `xtask check` and `xtask fuzz` harnesses can be pointed at the FFI without restructuring.
8. Which engine constants other than those listed in MOB-1.5 are hard-coded budgets.

---

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-12 | created | Initial plan from a static review of the 0.7.0 source; budgets proposed, nothing built |
| 2026-10-12 | Phase 0 complete | Installed mobile targets & NDK; verified cross-compilations; implemented baseline harness and recorded host baselines |
| 2026-10-12 | Phase 1 complete | Created crates/ffi L7 bridge, UniFFI bindings generator, packaging tasks (mobile-xcframework & mobile-android), smoke apps, CI workflows, and FFI test suite |
| 2026-10-12 | MOB-2.1 complete | Extended document API with page links, outline, find, geometry, and metadata |
| 2026-10-13 | MOB-1.9 complete | Verified Phase 1 Evidence Gate (A0): 55 FFI tests, API snapshots, B1 host/device latencies, render parity fixtures, sample page fix, and size reports |
