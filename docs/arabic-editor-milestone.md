# Arabic editing milestone: complete embedded source faces

Status: first source-editing increment validated; leading-mark positioning remains unresolved, 2026-10-31. Base:
`13655d09922038ab62d93b84ba9cd2eae474abd7`. Overall Arabic readiness and estimates are unchanged.

## Supported milestone and contribution boundary

The first increment is a reliable engine workflow for an existing Arabic paragraph whose PDF
embeds a complete, editable static TrueType/OpenType face, while its PDF text encoding maps
only the characters used originally. Newly typed supported characters should come from that
same program rather than an optional matching variable face. One contextual source span must
not be split between the original partial encoding and the newly embedded full encoding.

The production change is limited to `crates/edit/src/text_regions/styled.rs`:
`FontResolver::encodable_font` prefers the validated complete source, and
`CharacterPreparation::prepare` selects one complete source resource for a contextual span
before resolving individual characters. This also avoids choosing older partial aliases after
save/reopen. Source subset rejection, variable-font/embedding validation, explicit resource choices,
and the existing refusal of genuinely mixed contextual styles remain intact.

This branch does not include the mapped-mark or numeric fixes submitted as #5603/#5602,
export-reader recovery, empty CMap/auxiliary corrections, or new shaped-glyph mappings.
It is independently based on upstream main rather than stacked on the broad Arabic branch.

## Acceptance evidence

An always-run unit regression in `crates/edit/src/tests.rs` uses the existing bundled Inter
asset: a source WinAnsi q plus a newly inserted combining acute must use one complete-source
encoding. This test requires no external font and has no optional early return.

`crates/engine/tests/it/document_fonts.rs` adds controlled, real-outline source PDFs and engine
history tests. The fixture begins with shaped sin letters, each with a nonempty Unicode
mapping, and a complete source program. This avoids depending on unrelated zero-advance
auxiliary/empty-CMap fixes. It uses the existing embedding/shaping APIs, not an independent
shaper or third-party producer document.

The target acceptance checks include four Arabic replacement cases plus a Latin-only control:

| Behavior | Required assertion |
| --- | --- |
| Add harakat and new Arabic letters | Apply accepts the text and reuses the exact embedded static program. |
| Mixed text | Vocalized greeting, Western digits and Arabic/English replacements retain exact logical Unicode. |
| Preview | Draft bounds match Apply; background preview removes source ink without changing the document/history. Marks extending the old line retain the overflow warning. |
| Copy/search | Full-phrase selection and search return exact code points without normalization. |
| Undo/redo | Original and edited Unicode/pixels are restored through the real Session history API. |
| Save/reopen | Incremental bytes retain the complete original prefix, unknown page data, text and rendered pixels. |
| Repeat editing | Insert/delete a suffix after reopen; retain exact text and source program. Each resulting state is stable through history and serialization. A separate position-drift regression remains below. |
| Unsupported contextual style split | Return the existing explicit refusal and preserve bytes, dirty state and history atomically. |

These are engine paragraph replacements, not simulated keyboard events. Full-span selection
is checked; precise word/caret selection inside ActualText remains separate work. Pixel
identity after history/serialization proves stability, not external typography conformance.

## Newly isolated layout limitation

A separate regression remains explicitly ignored:
`leading_arabic_marks_retain_position_when_an_inserted_suffix_is_deleted`. Starting from
`سَسَ`, inserting a suffix and deleting it again can retain the same logical text but change
its page pixels/position. The isolated test uses explicit static-font registration to bypass
source completion, so the layout failure is independent of this change. The observed baseline
moves from `[30.0, 157.984]` to `[30.0, 155.968]` in the 16-point fixture. The changed encoding
is now accepted; anchoring/rise preservation
across repeated rewrites is a separate unresolved defect. Do not count this test as a pass.
Run it deliberately with `--ignored` to inspect the failing assertion. This is the next
layout task to coordinate with #3465 before claiming complete Arabic editing. Investigate
`text/interp.rs` baseline/rise capture, `text/lines.rs` source origins and the shaped rewrite
in `text_regions/styled.rs`; do not suppress overflow warnings or alter unrelated scripts.

The active tests distinguish state stability across undo/redo/save from identical appearance
after a sequence of new rewrite operations. They assert the former and preserve the latter's
known failure explicitly; neither is silently normalized away.

## Inputs and reproduction

Fonts remain external. Use the existing CI-pinned craft-fonts checkout at
`b076417b5880bd4154bd22f00695b1d28c6bea83`. Its Noto Sans Arabic source is OFL-1.1, from
`google/fonts@7085eb89a950e85db5b166b7a58d414544b4140c/ofl/notosansarabic`.
Source SHA-256: `63111b5b2e074dd48cc67692e0a2726d86ee94c1c37fe8598257b7b4e87e869e`.
The local all-default static instance (wght 400, wdth 100; fontTools 4.61.1 with original name,
license and timestamps retained) has SHA-256
`65efad4885c590b640d9601b8cc6d9b66026e9ff74154ac600207600842c0f70`.
Keep the original OFL.txt with any redistributed generated outputs; commit no font or PDF.

```sh
export RUSTUP_HOME=/workspace/.pdfcraft-rustup
export CARGO_HOME=/workspace/.pdfcraft-cargo
export PATH="$CARGO_HOME/bin:$PATH"
export CRAFT_FONTS_DIR=/workspace/arabic-deps/craft-fonts
export CRAFT_FONTS_REQUIRED=1
export PDFCRAFT_TEST_ARABIC_FONT=/workspace/arabic-deps/NotoSansArabic-Regular.ttf
export PDFCRAFT_ARABIC_CORPUS_REQUIRED=1
export PDFCRAFT_ARABIC_EDIT_ARTIFACT_DIR=/tmp/pdfcraft-arabic-edit-milestone
cd /workspace/pdfcraft
timeout 900 cargo test --offline --locked -p pdfcraft-engine --test it document_fonts -j 2
```

The required-input gate makes missing static-font inputs fail rather than silently counting an
optional early return. Artifacts are optional and outside the checkout. Native CLI validation
must use the branch's rebuilt binary, not the earlier combined-branch binary.

## Validation results

- All 359 editor unit tests pass, including the always-run bundled-font regression. Restoring
  per-character-only selection makes that new unit test fail on the contextual encoding guard;
  the final production file is restored exactly before the passing run.
- The original engine acceptance case failed with upstream code on the optional variable-font
  refusal. With the complete-span selection disabled but source preference retained, it fails
  on the existing mixed-encoding guard. Final production source was restored byte-for-byte.
- The active document-font acceptance tests pass with mandatory static Arabic inputs, including
  four Arabic replacements and a Latin-only control, preview/apply bounds, read-only background,
  exact full-phrase selection/search, source program identity, history and incremental reopen.
- Scoped engine integration reports 18 passed and 3 ignored: two pre-existing private-corpus
  ignores plus the explicitly documented leading-mark regression. A full engine unit/workspace
  run is not claimed. The new source-font cases do not silently skip with the required gate.
- Native all-target Clippy for edit/engine passes with warnings denied. Their WASM library
  check passes (1m 33s); the full web application and UI input are not qualified.
- The rebuilt branch CLI passes all 22 scripted steps: open, inspect, edit a vocalized greeting,
  extract, undo/redo, save/reopen, edit again, search and render. Seven extraction outputs match
  literal logical Unicode, with one warning-free search result. Three CLI previews are emitted;
  the edited and reedited previews were inspected for joined letters/marks.
- The known leading-mark test is run separately with `--ignored`; it fails as documented and
  is never counted as successful position preservation. Its explicit font selection bypasses
  the source-completion change and isolates the existing layout issue.

The CLI script/report and OFL license are in `/tmp/pdfcraft-arabic-edit-milestone`, outside the
checkout. No PDF or font is committed. History/save pixel identity does not establish identical
appearance after arbitrary new rewrite sequences; the explicit position regression records
that remaining boundary.

## Coordination and next increments

Open #3465 contains broader Arabic fallback/ActualText/cursor work and #4920 covers cluster
Unicode mappings. The inspected #3465 head is `f83f6375ec5824fd3d944156ad5c0e26e8678904`;
it retains the original source-face selection logic. Offer this small correction and its
regression evidence as a supplement; do not create a competing broad RTL rewrite.

Next: fix the isolated leading-mark rise/anchor drift, then qualify RTL cursor/hit selection in
`crates/ui-egui/src/tools/edit_text_ui.rs` and `tools/edit_text_ui/background.rs`, using the
engine's `text_edit_layout`/preview APIs and the existing egui text infrastructure. Test native
and browser keyboard/IME, cluster deletion and mixed-direction boundaries independently.
Then tackle contextual mixed styles, broader font/subset cases, complex layouts and export
RTL metadata. Arabic OCR is a separate later capability.

## Revision history

- 2026-10-31: define a bounded complete-source editing milestone, independent real-font
  history/save regressions, input provenance and explicit UI/selection limitations.
