# Document Properties sessions and interaction replay

Status: proposed integration, 2026-10-25. This document describes the consumer patch;
it does not claim upstream adoption of either experimental library.

Document Properties binds its Description, Initial View and accepted Custom values to
the document identity, edit generation and modification permission at opening. A
different document, reload, edit or Undo/Redo closes the stale dialog with a notice.
Returning to the original values through Undo still invalidates the old draft.
The notice is included in the translation-gap scan; catalogs without its translation
retain the standard English fallback.
Clean inspection tabs (Fonts, Advanced, Security, Space Audit) retain live refresh for
the same document; they never rebase accepted edits or pending Custom Name/Value input.

[`edit-session`](https://github.com/PerchAnton/edit-session) 0.1.0-alpha.2 is a
dependency-free `no_std` library, pinned to `033f7c3a67b825b8f222288788cc61234c0c945d`.
It checks freshness and consumes a draft once. PdfCraft retains its form validation,
permission checks, PDF serialization and undo history. The UI bridge is
`crates/ui-egui/src/properties_session.rs`; it stores small property values, not PDF bytes.

Typing does not edit the document. OK rechecks freshness on the serialized UI command
path and dispatches one existing `Edit::Batch`. An engine refusal leaves the draft open
for correction without applying part of the batch. Cancel/Escape, a no-op and stale
dismissal dispatch no edit. Description whitespace and Custom name validation retain
their existing behavior; Custom Name/Value input becomes part of the draft only through
Add/Update. No new Reset control or metadata field is introduced.

The same document changes remain available through `Session::apply`, `doc_set_info`
and `doc_initial_view`. The existing opt-in `ui.set`, `ui.type`, `ui.click` and `ui.key`
control channel reaches the dialog. This patch adds no server or automation permission.
It covers Document Properties only; the broader dialog lifecycle issue #3917 and OS
open queueing are separate work.

## Replaying the real dialog

The consumer owns `crates/ui-egui/examples/properties_scenarios.rs`, built in PdfCraft's
workspace so it uses the same vendored patches and UI as the app. It is a development
example, not a shipped executable. It runs the real egui controls through `egui_kittest`,
then reports actual draft/document/history state as JSONL protocol 1.

[`interaction-check`](https://github.com/PerchAnton/interaction-check) is a standard-library
Python runner, pinned in CI to `7722d66c0ede7855ff58d1df3d1632fd4e5e66a5`. It sends only
events; expected values stay in the runner. Each scenario starts a fresh adapter process
with two synthetic, two-page PDFs. No corpus, network, proprietary fonts or committed
PDF fixture is required.

From this repository, with Python 3.10+ and Rust 1.99.1:

```sh
git clone https://github.com/PerchAnton/interaction-check ../interaction-check
git -C ../interaction-check checkout 7722d66c0ede7855ff58d1df3d1632fd4e5e66a5
cargo build --locked -j 2 -p pdfcraft-ui-egui --example properties_scenarios
mkdir -p log/properties-interactions
PDFCRAFT_SYSTEM_FONTS=0 PDFCRAFT_LOCALE=en-US \
  python3 ../interaction-check/interaction_check.py --timeout 30 \
  --report log/properties-interactions/report.json \
  --junit log/properties-interactions/junit.xml \
  tests/interactions/document-properties/*.json -- target/debug/examples/properties_scenarios
```

On Windows, use `properties_scenarios.exe` and set environment variables with your shell.
The CI workflow uploads JSON/JUnit reports; it does not require a GPU.

Events `open`, `type`, `click`, `escape`, `undo` and `redo` use actual UI commands/controls.
`external_edit`, `external_edit_undo`, `reload`, `switch` and `close_document` simulate
changes arriving outside that modal via the real session or tab selection. `save_reopen`
serializes with the engine and opens the resulting bytes in a fresh session. `inspect`
only advances frames and reports state. Document numbers are 1-based. Unsupported or
disabled controls return a state error; the scenarios assert the resulting state after each action.
Sixteen simulated frames after each event let the modal's animated layout settle.

Ten scenarios cover cross-tab commit/save/reopen, one-step Undo/Redo, Cancel/Escape,
no-op with Redo, intervening edits, edit-then-Undo, same-ID reload, tab switch, target
close, Custom validation and live inspection without rebasing pending input. Rust tests additionally cover an engine refusal/retry and
the freshness check immediately before commit without an intervening UI frame.

For optional local screenshots and `saved.pdf`, append `--artifacts DIR` after the
adapter executable while running **one** scenario. This requires an available GPU or
software renderer. Keep `PDFCRAFT_SYSTEM_FONTS=0`; generated artifacts stay under ignored
`log/`. The headless UI proves widget/engine behavior, not native OS input or PDF fidelity
in a separate reader. Existing renderer and interoperability tests remain necessary.

## Dependency review

Both projects are contributor-maintained, experimental and `MIT OR Apache-2.0`.
`deny.toml` proposes a source exception for `edit-session`; CI allows only its exact
package/version/revision outside the existing storytold namespace. No other repository
under the contributor's account is allowed. Updating either pin requires review.
Python and the replay tool are development dependencies only.

Adoption requires the supply-chain policy owner's explicit approval of both the
runtime library and execution of the external replay runner in CI. Exact pins bound
the reviewed source; they do not establish that approval.

The shared release workflow currently rejects this external Git dependency even
when the PR's CI passes. [craft-common-actions #84](https://github.com/storytold/craft-common-actions/pull/84)
proposes the corresponding exception for the official `storytold/pdfcraft` caller
and this exact package/version/revision, with regression tests. It is a draft policy
proposal. Merge this integration only after the dependency decision and the shared
release change; keep using the official shared release workflow.

## Revision history

- 2026-10-25: document the pending dependency decisions and shared release prerequisite;
  add function documentation for the session bridge, adapter and regression tests,
  and cover discovery of the stale notice by the translation scanner.
- 2026-10-25: initial Document Properties integration and real UI replay adapter.
