# Arabic PDF editing: execution plan

Status: Batch A core checkpoint published as PR #5623; review follow-up in progress,
2026-11-01. This qualifies supported fixtures, not complete Arabic support or UI/IME readiness.

Post-sprint publication scope (2026-11-01): the contributor subsequently authorized
synchronizing with upstream main, merging and pushing the verified core checkpoint and
feature/test documentation to the fork. The original no-push rule below records the A6
sprint boundary. Unfinished direction controls are preserved locally and excluded from
publication; desktop launch and Batch B interaction qualification remain follow-up work.

## 1. Objective and working rules

Deliver a dependable workflow for supported Arabic PDFs:

**Open → select → edit → preview → undo/redo → save → reopen → copy/search.**

Correct appearance and exact logical Unicode are separate requirements. Preserve code points;
do not normalize Arabic, reverse strings globally, reshape already-positioned source glyphs
indiscriminately, or substitute a flattened image for editable text.

Work in coherent milestone batches. Keep implementation commits small and green, but do not
open a PR for each small fix. The next submission is one core-editing batch; interaction,
export and OCR follow as separate batches when their own acceptance criteria pass.

Use the existing checkout. No additional worktree, new shaping engine, default OCR runtime,
font asset, CI change or readiness-percentage increase is part of the core batch.

## 2. Historical A6 sprint state (before publication)

The table records the original no-push sprint boundary. Publication superseded that
boundary; the current source/test review map and remaining gates are in
[arabic-features.md](arabic-features.md). Historical test counts are not new-head CI results.

| Item | Actual state |
| --- | --- |
| PR #5603: mapped-diacritic copy/search | Submitted from `Sader-IT-Solutions/pdfcraft`; open at the last public inspection. Its historical isolated tests passed. |
| PR #5602: Arabic numeric separators | Submitted; open at the last public inspection. Its historical isolated tests passed. |
| Source-font foundation | Commit `b5fd6936` on `fix/arabic-complete-source-edit` is pushed. It passed 359 editor tests, 18 engine integration tests and 22 CLI steps, with its documented ignores/limits. |
| Consolidated work | `feat/arabic-editing-milestone`, based on `b5fd6936`. The local core gates pass; checkpoint kept local. No feature push or new PR is part of this batch. |
| Leading-mark baseline | A correction is implemented. Targeted real-font tests pass for 7 phrases × 3 rise values, including 63 insert/delete cycles, history and reopen checks. The former ignored position regression is active. |
| Glyph mappings and auxiliaries | Unicode-owner, empty-CMap and empty-auxiliary corrections are incorporated and included in the passing local core qualification. |
| A4 transition regression | Reproduced on restored code: **5 passed, 1 failed, zero ignored**. The strict Latin → Arabic → Latin pixel assertion remains active and now passes after both bounded corrections below. |
| Latest engine gate | **20 passed, 0 failed, 2 existing private-input tests ignored.** These ignores are separate from the active strict transition test. |
| Character spacing | Reproduced a 1.625 pt combining-mark displacement under 1.25 pt tracking at 65% scale. Tracking now advances between complete clusters; the position test passes all **36 controls**. |
| Focused controls | Space-width tests: **2 passed**; long-draft cache test: **1 passed**; tracking/scale test: **1 passed**. Removing each correction separately made its regression fail, then final sources were restored exactly. |
| GitHub administration | #5602/#5603 remain separate. No feature push, PR posting, workflow request or CI modification is part of this sprint. |

A4 had two causes. An unencoded space initially used an estimate (`0.33`), then a later
encoding exposed the source program's real advance (`0.26` in the pinned Arabic face).
Validated complete source metrics now supply the missing advance after explicit PDF space
widths. The metrics-only correction left **4.7968e-5 pt** of drift: a partial-resource space
split the Latin run and rounded an intermediate text matrix. Completing that uniform span
in one resource removes the split. Explicit font requests and encoded source spaces retain
their existing paths; unavailable/subset programs retain the bounded fallback.

The pixel assertion was not relaxed or ignored. Private restoration patches and local logs
remain outside the tracked contribution files.

Final local A6 evidence:

| Gate | Result and limits |
| --- | --- |
| Native fonts/edit/render | **945 passed, 0 failed, 1 existing corpus ignore**: 365 final editor tests plus 580 tests from unchanged fonts/render code. |
| Engine | **20 passed, 0 failed, 2 existing private-input ignores** on the final combined code. |
| Static/platform checks | Four-package all-target Clippy with `-D warnings`, four-package WASM, formatting, layers and assets pass. WASM retains five unchanged renderer configuration warnings. |
| Rebuilt CLI journey | **34 steps**, **8 exact Unicode states**, **5 strict PNG pairs** pass, including incremental source-prefix preservation, reopen, search and history. Matching before/reopen renders were also visually inspected. |

Qualification used a local-only optimization-level-zero override for the unrelated Boa
JavaScript dependency. No JavaScript performance qualification is claimed. These results
complete the bounded local core batch; UI/IME, mixed-style shaping, OCR, export fidelity and
broader document compatibility remain outside its qualification. Readiness estimates do not change.

## 3. Batch A: core source editing and Unicode fidelity

Freeze this batch to the following work packages. New unrelated findings enter the backlog;
only findings required to satisfy these acceptance criteria expand the batch.

| ID | Work package | State / completion criterion |
| --- | --- | --- |
| A1 | Complete embedded source-font reuse | Implemented foundation. Repeated edits use a validated complete source program and one contextual encoding; explicit choices and embedding restrictions remain respected. |
| A2 | Leading-mark source origins | Implemented, targeted tests pass. Zero-advance leading marks must not make their temporary rise the paragraph baseline. Preserve genuine raised/lowered baselines and conservative controls. |
| A3 | Unicode mappings and auxiliary glyphs | Incorporated. Exact Unicode belongs to the appropriate advancing body; explicit empty mappings remain empty; auxiliary ink/operators survive until an edit removes them, without inventing spaces. |
| A4 | Latin/Arabic transition stability | Corrected source-space measurement and uniform completion of spans with unencoded spaces. Strict pixel/history/reopen regression passes; correction-only reversions demonstrate sensitivity. |
| A5 | Uniform formatting qualification | Corrected tracking between complete clusters while retaining total spacing. Glyph-position qualification passes 36 Latin/Arabic, mark, tracking and horizontal-scale controls. This is not mixed-style shaping. |
| A6 | Combined verification and handoff | Complete for the supported local core fixtures: native, engine, Clippy, WASM, formatting, layers/assets and rebuilt CLI gates pass, with existing ignores/warnings and limits recorded separately. Local core checkpoint; no feature push or PR. |

### Exact implementation areas

- `crates/edit/src/text_regions/styled.rs`: source selection, contextual span preparation,
  shaping emission, spacing and calibration.
- `crates/edit/src/text/interp.rs`: glyph-advance information and bounded ActualText origin
  recovery. Keep glyph ink and operator ownership intact.
- `crates/edit/src/text/lines.rs`: semantic gaps/pen movement around empty auxiliaries.
- `crates/fonts/src/source_shaping.rs`: PDF Unicode ownership independent of shaping/spacing
  representation; existing glyph IDs and positions remain authoritative.
- `crates/fonts/src/pdf.rs`: explicit empty destination decoding and existing CMap budgets.
- `crates/edit/src/tests.rs`, `crates/fonts/src/pdf/tests.rs`, and
  `crates/engine/tests/it/document_fonts.rs`: regression and acceptance evidence.

Do not duplicate the already-submitted #5602/#5603 changes in this batch. Coordinate overlapping
editing/mapping work with #3465 and #4920 rather than introduce a competing broad rewrite.

### A4 evidence and invariant

1. Reproduce the failing Latin control alone using the pinned static font and existing fixture.
2. Capture before/after logical text, source font bytes, font size, scale, spacing, baseline,
   bounding boxes and the emitted glyph positions/operators.
3. Separate legitimate shaping differences from unintended origin, scale or tracking drift.
4. Define the invariant: with the same final logical text and requested style, repeated edits
   must not introduce unintended placement/formatting changes. If an intentional shaping-policy
   difference is proposed, document and review that policy before changing the test contract.
5. Add the smallest regression, observe it fail, implement the bounded correction, and prove
   sensitivity by temporarily reverting only that correction.
6. Rerun the Arabic, Latin, mixed-direction, genuine-rise and history/reopen cases together.

This sequence established the space-metric and split-matrix causes above. Preserve the exact
pixel and Unicode assertions when reviewing or integrating the corrections.

### Batch A acceptance gate

- Exact logical Unicode through the supported edit/copy/search/save paths, including harakat,
  ligatures, Arabic/English text and digits. Include negative and non-Arabic controls.
- Preview and Apply agree on bounds; read-only preview does not change bytes or history.
- No unintended baseline, scale, tracking or appearance drift in repeated-edit scenarios.
- Undo/redo and serialization restore each expected state; incremental saves preserve original
  bytes and unknown PDF data.
- Requested source fonts remain the intended programs. Unsupported inputs return actionable
  errors and leave document/history intact.
- Mandatory font-input gates are enabled for qualification. Optional early returns, ignores,
  expected failures and unrun cases are reported separately.
- No unexplained failing test in the agreed batch scope. No new ignored acceptance test used to
  bypass a release blocker.

## 4. Batch B: user interaction and richer editing

Begin after Batch A's core behavior is stable. Reuse the engine and existing egui/shaping
infrastructure; do not build a second bidi engine.

| ID | Work package | Acceptance |
| --- | --- | --- |
| B1 | Caret and hit testing | Visual arrow movement and mouse placement map to correct logical positions across Arabic/Latin boundaries. |
| B2 | Selection and deletion | Shift-selection, word selection, clipboard and Backspace/Delete respect the chosen grapheme/cluster policy and never split UTF-8 or silently lose marks. |
| B3 | Keyboard/IME and history | Native and browser Arabic input, composition/cancellation, undo/redo and save/reopen work through actual UI events. |
| B4 | Formatting | Qualify uniform size/color/spacing first, then contextual mixed styles/fonts. Preserve joins where supported; retain explicit refusal where correct output cannot be produced. |
| B5 | Reflow and anchoring | Right alignment, justification, multiline/wrapped text and paragraph resizing retain the intended anchors and logical content across repeated edits. |

Primary UI modules: `crates/ui-egui/src/tools/edit_text_ui.rs`,
`tools/edit_text_ui/background.rs`, and `canvas/selection.rs`; engine draft/preview APIs are in
`crates/engine/src/text_preview.rs`. Follow the existing page-pointer and cursor rules.

Gate: automated headless interaction tests where possible, actual native/browser inspection
for platform-dependent behavior, recorded input/output evidence, and no unsupported workflow
presented as complete. A successful engine paragraph replacement is not a keyboard/IME test.

## 5. Batch C and later work

- **Document compatibility:** qualify real full/subset fonts, missing-font fallback, columns,
  tables, rotated/cropped pages, annotations and form fields using source-generated or otherwise
  policy-compliant fixtures with known ground truth.
- **Ambiguous or missing mappings:** distinguish reliable metadata from heuristic recovery.
  Report uncertainty; do not promise exact reconstruction when the source discarded information.
- **Export:** verify logical input and actual DOCX/HTML/RTF/PPTX direction/layout separately.
  Coordinate #5042. Correct strings alone do not establish correct Word layout.
- **OCR:** a separate optional Arabic/English backend and Unicode-safe invisible text layer,
  with pinned licensed models and recognition-quality measurements. No large default dependency.

These are not requirements for opening the bounded Batch A contribution, and are not silently
claimed as completed by it.

## 6. Test inputs and verification protocol

Keep fonts external and pinned. The existing Noto Sans Arabic source/static hashes and OFL
provenance are recorded in `arabic-editor-milestone.md`. Use the existing bundled Inter only
through its established asset entry. Commit fixture-generating Rust code, not font/PDF binaries.

The corpus matrix separates:

- logical Unicode and word/cluster selection;
- rendered glyph geometry and page pixels;
- history/serialization stability;
- native/browser input behavior;
- external reader/export behavior.

Compare exact code points. Do not normalize, reverse or strip controls from an external
extractor's result to manufacture agreement. Pixel comparisons must use controlled font/style,
page geometry and render settings. Explain legitimate differences before altering assertions.

Run scoped checks with timeouts and `-j 2`; a full workspace build is CI's responsibility:

```sh
# Use normal local Rust 1.95.0 with rustfmt, Clippy and wasm32-unknown-unknown installed.
# Replace /ABS paths with absolute local paths; keep fonts outside the repository.
export CRAFT_FONTS_DIR=/ABS/craft-fonts
export CRAFT_FONTS_REQUIRED=1
export PDFCRAFT_TEST_ARABIC_FONT=/ABS/arabic-deps/NotoSansArabic-Regular.ttf
export PDFCRAFT_ARABIC_CORPUS_REQUIRED=1
cd /ABS/pdfcraft

timeout 900 cargo test --offline --locked -p pdfcraft-fonts -p pdfcraft-edit -p pdfcraft-render -j 2
timeout 900 cargo test --offline --locked -p pdfcraft-engine --test it -j 2
timeout 900 cargo clippy --offline --locked -p pdfcraft-fonts -p pdfcraft-edit -p pdfcraft-render -p pdfcraft-engine --all-targets -j 2 -- -D warnings
timeout 900 cargo check --offline --locked -p pdfcraft-fonts -p pdfcraft-edit -p pdfcraft-render -p pdfcraft-engine --target wasm32-unknown-unknown -j 2
cargo fmt --all -- --check
cargo xtask layers
cargo xtask assets
timeout 900 cargo build --offline --locked -p pdfcraft-cli -j 2
```

On a fresh machine, omit `--offline` until locked dependencies are cached. `timeout` is
shell/platform-specific; use the equivalent local process timeout on Windows.

Use the rebuilt batch CLI for edit, preview, search, undo/redo, save/reopen and render checks;
an earlier branch's binary does not qualify the current code. Re-run affected gates after
production changes. Historical passing counts are not a release stamp for new WIP.

## 7. Historical sprint contribution protocol

These were the original local-only sprint instructions. The later publication and
PR #5623 review update are authorized; the direction-control WIP remains excluded.

1. Keep #5602/#5603 separate; do not modify their branches as part of this sprint.
2. Prepare only local review notes with verified results. Do not publish private handoff material,
   push the feature branch, open a PR, or post prepared descriptions during this sprint.
3. Upstream maintainers control workflow approval; local tests do not replace upstream CI.
   Do not modify CI or post an approval request as part of this batch.
4. Keep Batch A on `feat/arabic-editing-milestone`; commit only verified green increments.
   Do not force-push or modify the already-submitted focused branches as part of this batch.
5. After A6, record a local review checkpoint with exact tests, remaining limits, provenance and
   the relevant upstream overlap. Future publication requires separate authorization and a fresh
   check of upstream status.
6. Interaction, export and OCR use later milestone batches, not a stream of tiny PRs and not
   one unreviewable all-Arabic change.

## 8. Responsibilities and immediate sequence

- **Implementation:** reproduce, minimize, fix, test, document and produce a reviewable batch.
- **Contributor (`eliassader`):** review the handoff and submit the consolidated contribution
  from `Sader-IT-Solutions/pdfcraft` when its gates pass.
- **Upstream maintainers:** workflow approval, technical review and merge decisions.

Current immediate order: **address PR #5623 review feedback, validate the bounded core,
update its feature branch and description, then obtain native-reader and maintainer review
with passing exact-head CI before merge**. Do not expand this contribution to Batch B.
The next bounded checkpoint is Batch B UI qualification: caret/hit testing, selection/deletion
and actual native/browser Arabic input and IME behavior. It requires its own evidence; the
engine/CLI results do not qualify those interactions. Rich mixed formatting, export and OCR
remain separate work. No feature push or PR is authorized by completion of these local gates.

## Revision history

- 2026-11-01: identify the published PR #5623 review phase; label the earlier local-only
  boundary as historical and keep interaction/direction work outside this contribution.
- 2026-11-01: record reproduced A4/A5 causes, correction-only sensitivity checks and completed
  scoped local A6 qualification with exact ignores/warnings and CLI evidence; keep UI/IME and
  later milestones separate, and use machine-independent dependency examples.
- 2026-10-31: freeze a coherent core-editing batch; record the actual WIP failure, explicit
  acceptance criteria, follow-on milestones, verification commands and contribution ownership.
