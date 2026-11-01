# Arabic editing: feature and qualification record

Status: core checkpoint `a8a1e696` verified on 2026-11-01. The core publication integrates
upstream `eea00a61`. Explicit direction controls are saved as unfinished follow-up and
excluded from this publication.

This page records the contribution for project managers and reviewers. It distinguishes
implemented behavior from later interaction work; it does not claim complete Arabic support.

| Feature | Behavior at the verified core checkpoint |
| --- | --- |
| Complete embedded source faces | New supported characters reuse the paragraph's validated, editable static source font. Contextual spans use one encoding, including after reopen. |
| Leading Arabic marks | A temporary rise on an initial zero-advance mark no longer becomes the paragraph baseline. Genuine raised/lowered text retains its baseline. |
| Exact Unicode | Advancing letter bodies own cluster Unicode; dots and marks keep their original glyph positions. Explicit empty CMap destinations remain empty. |
| Auxiliary glyphs | Empty Unicode auxiliaries retain ink and operators without introducing spaces or moving the word pen. |
| Latin/Arabic transitions | Latin → Arabic → Latin preserves strict page pixels. Missing encoded spaces use validated source metrics; uniform span completion avoids intermediate matrix rounding. |
| Character/word spacing | Tracking advances between complete shaped clusters, retaining the offsets of marks and dots and the final advance. |
| Horizontal scale | Glyph positions are checked at 65%, 100% and 135%, with positive, zero and negative tracking. |
| Preview | The supported draft and Apply agree on bounds; read-only preview preserves document state and history. |
| History | Undo/redo restores the expected logical text and rendered appearance. |
| Save/reopen | Incremental saves retain original bytes; reopened text, source programs, search geometry and page images remain stable. |
| Unsupported inputs | Contextual mixed-style rows remain explicitly refused without altering the document or history. Restricted, unavailable or incomplete font programs retain existing safeguards. |
| Explicit Auto/LTR/RTL controls | Deferred follow-up for page text, added text and form properties; excluded from this publication. Direction is separate from alignment and must not modify logical Unicode. |

## Completed verification

- Native fonts/editor/renderer: **945 passed, 0 failed, 1 existing corpus-dependent ignore**.
- Final editor rerun: **365 passed, 0 failed, 0 ignored**, included in the native total.
- Final engine integration: **20 passed, 0 failed, 2 existing private-input ignores**.
- Four-package all-target Clippy with `-D warnings`, four-package WASM, formatting,
  dependency layers and asset checks passed. WASM retains five existing renderer configuration warnings.
- Rebuilt CLI journey: **34 operations**, **8 exact Unicode states**, **5 strict image pairs**;
  incremental original-byte-prefix preservation, preview, history, search and reopen verified.
- Before-fix regressions and correction-only reversions failed as expected. Pixel assertions
  were neither relaxed nor ignored. Matching Arabic renders before save and after reopen were inspected.

Fonts were external, pinned and hash verified. No font/PDF/image binary, private handoff or CI
change belongs to this contribution. Qualification used a local-only optimization override for
the unrelated Boa dependency; it provides no JavaScript performance result.

## Upstream integration verification

The publication includes all 35 commits fetched from upstream through `eea00a61`.
Both roadmap insertion conflicts retained the Arabic and upstream progress records.

On the combined code, the engine suite passed **393 unit tests** and **20 integration tests**,
with two existing manual timing probes and two private-fixture tests ignored. The suite ran
serially: an initial parallel run hit two existing XFA JavaScript wall-clock limits under the
local Boa optimization override; the same tests passed without code or limit changes when
isolated from contention. This is functional qualification, not a JavaScript performance claim.

Four-package all-target Clippy with `-D warnings`, four-package WASM, formatting, layers and
the upstream source licensing/asset check passed again. The CLI was rebuilt from the combined
code and its **34 steps / 8 exact Unicode states / 5 strict image pairs** passed again.

## Remaining qualification

Explicit direction controls, precise caret/hit testing, selection/deletion, native/browser IME,
rich mixed formatting, paragraph reflow, arbitrary real PDFs, Arabic form appearance coverage,
OCR and export interoperability require their own evidence. Core engine replacement tests do
not qualify these user interactions. Existing focused PRs #5602 and #5603 remain separate.

See [execution plan](arabic-editing-execution-plan.md) for acceptance gates and
[PR test/progress text](arabic-editing-pr-summary.txt) for the review attachment.

## Revision history

- 2026-11-01: consolidate verified core features and results, integrate upstream `eea00a61`,
  record combined-code checks, and retain explicit direction controls as deferred work.
