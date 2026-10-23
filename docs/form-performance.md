# AcroForm JavaScript performance

Status: implemented; proposed as an independent performance PR. Last reviewed and updated:
2026-10-19. Measurements below are historical synthetic evidence collected before this split,
not fresh measurements on upstream main. No private document is needed for the benchmark.

## Architecture

- `crates/ui-egui/src/forms_ui.rs` queues `Edit::SetFieldValue` for a text commit or checkbox
  toggle. `crates/automation/src/forms.rs::form_fill` uses the same edit API.
- `crates/engine/src/lib.rs::Session::apply` runs edits on a copy, preserving rollback and undo.
  `run_edit` calls `pdfcraft_forms::set_value_with` with an engine `JsRunner`.
- `crates/forms/src/lib.rs` validates and writes the edited value, then runs one calculation
  pass. `/CO` precedes calculated fields omitted from `/CO`. Each script sees prior writes.
  There is no dependency graph or dependency-based filtering: dynamic references and script
  side effects are evaluated on every pass.
- `crates/engine/src/js.rs::JsRunner::run` snapshots the fields, invokes `pdfcraft_js::run_shared`,
  then converts changed JavaScript field states into form updates.
- `crates/js/src/lib.rs::run_shared` starts a scoped worker per script. `run_here` builds a fresh Boa
  context, installs the API, evaluates document scripts and evaluates the action. No script or
  bytecode cache is present in this AcroForm path. XFA uses a separate session implementation.
- `crates/forms/src/scripting.rs::apply_changes` applies script side effects before the next
  calculation. It rereads the field snapshot when side effects occur. Calculation results
  equal to the existing value already skip value writes and appearance generation.
- Checkboxes write the field `/V` and widget `/AS`; they enter the same calculation pass as
  text edits. JavaScript exposes the export value or `Off`, and `isBoxChecked`.
- `Session::refresh_scoped` writes an incremental working revision, rereads fields and rebuilds
  the render pool. `crates/ui-egui/src/editing.rs::apply_edit` compares old/new fields and
  invalidates affected pages (including dependent fields on other pages). It does not blindly
  invalidate every page for an ordinary AcroForm fill. Rendering uses page/tile requests;
  widget appearance streams are separate from cached page rasters.

## Findings and changes

The dominant measured cost was the bridge, not executing calculation logic:

1. `JsRunner` expanded a sparse `Outcome::changed` into a complete second snapshot. `changes`
   then searched the first snapshot linearly for **each** field, even when JavaScript had not
   changed any field directly. That repeated quadratic comparison ran after every calculation.
   It now compares the sparse outcome only, with an indexed original snapshot when needed.
   Empty outcomes return immediately; unchanged writes still produce no form updates. The
   console/button path uses the same conversion.
2. Once that was removed, field-state construction dominated. Reading every widget's display
   flags on every script traversed the same immutable pre-edit COS snapshot. This workload
   exceeds COS's 1,024-object editing-cache cap, repeatedly reparsing widget dictionaries.
   The runner now memoizes only those scalar display readings by widget reference. Each edit
   creates a new runner; current values, read-only/required flags and options still come from
   the current field snapshot. No global cache or object-cache limit changes were needed.

All calculation actions still execute in the same order. No dependency filtering, script
removal, context reuse, bytecode caching or recursive scheduling changes were introduced.
Equal calculated values already skipped appearance regeneration; this behavior remains.
The Release investigation also found a full field-state copy inside the JavaScript host on
**every** script invocation. `pdfcraft_js::run_shared` now accepts an immutable shared snapshot;
`Arc::make_mut` detaches it on the first field write. The engine retains the initial snapshot
for comparing effects. Read-only calculations no longer copy every field again. Each script
still gets a fresh context and thread; exceptions, globals and uncommitted writes remain
isolated. Scripts that write fields retain the previous copy isolation. The existing borrowed
`run` API remains available. No new dependencies were added.

## Measurements

Historically measured under Rust 1.99.1 on Linux x86_64 with the default optimized development
profile, two-job builds. Serial benchmark runs had no concurrent compilation. These are
**synthetic engine measurements, not measurements of the original Pathfinder PDF or GUI input
latency**. Five edits: ranks 1, checkbox on, ranks 2, checkbox off, ranks 0. Every edit runs all
190 custom calculations among 3,129 fields on a six-page document. Widgets are colocated
on the first page; the fixture is an engine calculation stress test, not a layout replica.

| Synchronous edit | Before (ms) | After (ms) |
|---|---:|---:|
| ranks 1 | 4,425.274 | 424.465 |
| checkbox on | 4,378.642 | 458.716 |
| ranks 2 | 4,291.131 | 521.721 |
| checkbox off | 4,391.942 | 492.851 |
| ranks 0 | 4,322.570 | 453.392 |
| Median | **4,378.642** | **458.716** |

Median speedup: **9.55×**, approximately **89.5% less synchronous editing time**. The synthetic
scripts are deliberately small; this does not predict the exact result for other documents.
Raw logs and the baseline test executable are retained locally in `target/form-performance/`.

Historical serial runs collected with optional instrumentation (mean per edit; inclusive
nested spans). That instrumentation is not included in this PR:

| Phase | Before (ms) | After (ms) |
|---|---:|---:|
| JavaScript, including thread/context setup | 225.934 | 201.584 |
| Changed-field comparison | 2,814.805 | 0.024 |
| Entire script dispatch, all 190 scripts | 4,320.643 | 352.733 |
| Calculation pass, including changed appearances | 4,399.084 | 390.958 |
| Direct field write | 0.201 | 0.196 |
| Appearance generation, including Format | 61.225 | 22.981 |
| Form event, including direct write and calculations | 4,411.767 | 402.981 |
| Engine refresh/serialization | 23.499 | 21.826 |
| Whole engine edit | 4,442.144 | 431.539 |

An intermediate run after sparse comparison alone measured 1,193.419 ms/edit in field-state
construction. After display memoization it measured 101.821 ms/edit. The intermediate run's
whole engine edit averaged 1,592.101 ms. Appearance time also benefits from the changed cache
working set; there was no change to which calculated appearances are generated.

Rendering and compositor time are not included in these edit numbers. The original sheet and
GUI acceptance run remain required to quantify them.

## Release comparison and remaining cost

Historical comparison used the repository's Release profile (thin LTO, one code-generation
unit) and an optional standalone profiler, which is not included in this PR.
A fresh in-memory fixture was exported with create-new semantics to
`target/form-performance/synthetic-large.pdf` (SHA-256
`300431eeae117458d96e52877e12a9e63ea8dcbb8ad4500da717ba00ebb2f1e1`). It remained unchanged.
The example alternated `ranks` between 1 and 2 for six samples per process, always executing
all 190 scripts. Page rendering was timed separately.

Initial matched comparison, before the additional shared-snapshot change:

| Build | Samples | Median engine edit (ms) |
|---|---:|---:|
| Development | 6 | 460.697 |
| Release | 6 | 408.358 |

Release alone reduced the median by about 11%. A detailed Release run measured 106.947 ms/edit
constructing field states, 174.696 ms in JavaScript setup/execution, and 19.808 ms refreshing
engine view data. Calculation-order setup was only 0.319 ms. That supported investigating the
host's extra copy, rather than changing calculation scheduling or disabling calculations.

The pre-change Release profiler was retained, then compared with the shared-snapshot Release
profiler in alternating runs (old, new, old, new). No compilation ran during measurement.

| Release version | Samples | Median edit (ms) | Range (ms) |
|---|---:|---:|---:|
| Before shared snapshots | 12 | 381.380 | 353.468–447.833 |
| With shared snapshots | 12 | 267.063 | 258.545–313.777 |

The additional optimization reduced median engine time by **30.0%** (1.43× speedup).
This is a separate comparison from the earlier five-edit benchmark; do not combine their
speedup factors. The page-render/setup cost in the initial matched comparison was about
6–9 ms for this deliberately overlapping synthetic layout, not for the original sheet.

After shared snapshots, a historical Release run with optional instrumentation averaged:

| Phase | Mean per edit (ms) |
|---|---:|
| Field-state construction | 75.754 |
| JavaScript setup/execution | 83.198 |
| Complete script dispatch | 200.480 |
| Calculation pass | 237.780 |
| Calculated-value writes, including appearances | 25.089 |
| Appearances | 23.712 |
| All complete field enumerations (three per edit) | 27.387 |
| Calculation-order setup | 0.305 |
| Engine refresh | 19.128 |
| Whole engine edit | 272.807 |

These inclusive timings show that scheduling is now negligible on the fixture. The main
remaining costs are constructing/disposing field snapshots and creating/executing isolated
JavaScript contexts. Further snapshot caching or context reuse would need additional semantic
work and real-document evidence; no dependency-based skipping or global/context reuse was
introduced. The current shared-snapshot change was retained because its paired measurement
showed a substantial improvement and isolation regressions passed.

**Remaining limit:** these numbers do not quantify real-document or compositor latency.
A representative document and delayed field are still needed for a reproducible engine/render/UI
comparison. Prior manual acceptance on the integrated release reported responsive field editing;
that is not a timing measurement or independent validation of this split.

## Reproduce

Use the same compiler, build profile and machine for before/after runs. The default development
profile optimizes workspace crates at level 1 and dependencies at level 2. Build with two jobs
and a separate target directory, then run the benchmark without concurrent compilation:

```sh
CARGO_TARGET_DIR=target/acroform-benchmark cargo test -p pdfcraft-engine -j 2 --lib acroform_interaction_benchmark --no-run
CARGO_TARGET_DIR=target/acroform-benchmark cargo test -p pdfcraft-engine -j 2 --lib acroform_interaction_benchmark -- --ignored --nocapture
```

The benchmark creates a six-page, 3,129-field/190-calculation workload from contributor-original
PDF objects in memory and times five synchronous edits with `Instant`. It also checks the final
calculated chain and an independent field. Widgets overlap on page one; this is an engine stress
test, not a realistic page layout. There is no machine-dependent timing threshold in CI.

To export a synthetic fixture for a standalone comparison, set `PDFCRAFT_BENCH_EXPORT` to a new
filename when running the ignored benchmark. Its parent must exist; export refuses to overwrite
any existing file. The ordinary benchmark writes no PDF.

## Validation

Regression coverage includes shared-snapshot write/error/global isolation, duplicate field names,
scripted display changes with a warm cache and a fresh edit, ordered checkbox/rank calculations,
dynamic field lookup, property/value side effects, undo/save/reopen, and unchanged appearances.
The existing JavaScript and engine suites cover the unchanged public scripting behavior.

Before the split, the ignored benchmark passed both before and after optimization. Earlier
scoped native/WASM checks, Clippy, layering, asset and parity checks, UI form/JavaScript tests,
and automation `javascript_through_tools` checks also passed on the mixed review history.
Those historical checks do not certify these independent branches or hosted CI.

## Independent branch validation

Validated on upstream `a12fe8f4116984ebbc870eb514346ea922233316` using a fresh, isolated
Cargo target directory, offline locked dependencies and two build jobs:

- All five focused regressions passed separately: duplicate names, warm/new-edit display
  caches, ordered checkbox/rank dependents, dynamic lookup/side effects and shared-snapshot
  write/error/global isolation.
- `cargo test -p pdfcraft-js -p pdfcraft-engine -p pdfcraft-forms -j 2`: engine 285 unit
  and 9 integration tests, JavaScript 47 tests, and forms 80 tests passed. Two unit timing
  probes and two private-file acceptance tests were ignored in the regular suite.
- The ignored 3,129-field/190-calculation benchmark was then run explicitly and passed;
  its final calculated chain and independent field remained correct. This is a diagnostic
  timing run, not a new before/after comparison on this upstream base.
- Scoped `cargo clippy --all-targets -- -D warnings` and WASM checks of those three
  crates passed, as did formatting, whitespace, layering, asset and parity checks.
- `javascript_through_tools` passed. A CLI built in the isolated target directory then
  reproduced bonuses 1 → 4 → 5 → 2 → 0; save/reopen and `page_render` passed, and the
  synthetic page image was inspected.

These are local checks, not hosted CI. No GUI executable was rebuilt and no private PDF
was used for this independent validation.

## Rebase validation

Revalidated on upstream `932bf9c6bfdd63e2a21bb831696a3447b9def9cf` (2026-10-18).
The performance implementation and all five focused regressions are unchanged from the
previously reviewed fix. Upstream's newer tests and CSV documentation are retained.

- Engine: 286 unit and 9 integration tests passed; JavaScript: 48 passed; forms: 82 passed.
  Two unit timing probes and two private-file acceptance tests remain ignored in the
  regular suite.
- All five focused regressions and the explicit 3,129-field/190-calculation benchmark passed.
- Scoped all-targets Clippy with `-D warnings`, WASM, formatting, whitespace, layering,
  asset and parity checks passed.
- `javascript_through_tools` and a freshly built CLI's fill/save/reopen/render smoke check
  passed. The bonus chain remained 1 → 4 → 5 → 2 → 0, and the synthetic render was inspected.

Validation used a separate Cargo target directory. No GUI build or private PDF was needed;
these are local checks, not hosted CI or a new before/after performance comparison.

## PR #3318 refresh

Rebased onto freshly fetched upstream main
`e397efa85451be00f8d56df7649807fe31e019c7` on 2026-10-19. Both roadmap conflicts
retain upstream's menu, button-label and content-editing notes alongside the performance
notes. Every performance source, regression-test and parity hunk is exactly the original
performance patch applied to current upstream; upstream's additional tests are retained.
Main advanced after initial validation on `43550c44`; the non-overlapping redaction update
was incorporated and all checks below were repeated on the final base.

- All five focused regressions passed.
- Engine: 287 unit and 15 integration tests passed. JavaScript: 49 tests passed. Forms:
  84 tests passed. Doc tests passed. The regular suite still ignored two timing probes
  and two private-file tests; the synthetic benchmark was then run explicitly and passed.
- The 3,129-field/190-calculation benchmark preserved the expected calculated values. Its
  five diagnostic edits took 317.347–328.488 ms; this is not a new before/after comparison.
- Scoped all-targets Clippy with warnings denied, WASM, formatting, whitespace, layering,
  assets and parity passed.
- Automation JavaScript and CLI checks passed: bonuses 1 → 4 → 5 → 2 → 0, save/reopen,
  and page rendering. The synthetic render was inspected.

Builds used an isolated Cargo target outside the worktrees. No GUI executable was built,
no private PDF was used, and the compatibility branch was not changed. These are local
checks, not hosted CI or a new performance speedup claim.

## Revision history

- 2026-10-19: refresh PR #3318 onto upstream `e397efa8`, retain upstream roadmap additions
  and all performance changes, and repeat focused, scoped, benchmark and headless validation.

- 2026-10-18: rebase onto upstream `932bf9c6`; retain all performance source edits and regression
  tests; preserve upstream CSV documentation and repeat scoped native/WASM and headless checks.
- 2026-10-18: separate performance changes for review on current upstream main; add duplicate-name
  and scripted-display coverage; keep historical optional instrumentation results as evidence only.
- 2026-10-18: retain shared snapshots after a paired 30% synthetic Release improvement and
  verify snapshot isolation.
- 2026-10-17: remove quadratic outcome conversion and repeated display reads; record the
  synthetic before/after benchmark.
