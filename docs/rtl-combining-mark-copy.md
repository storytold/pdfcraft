# RTL copy: separately mapped combining marks

Status: focused regression fix, 2026-10-31. Base: upstream main
`13655d09922038ab62d93b84ba9cd2eae474abd7`. Estimates and readiness claims are unchanged.

## Reproduced behavior

A PDF paints Arabic letters and fatha separately and maps each with ToUnicode. Viewer copy
and extraction return `َباب` instead of `بَاب`; searching for the intended word fails.
The minimal layout case similarly returns `َبب` instead of `بَب`.

`crates/render/src/text/order.rs::reverse_rtl` now groups an all-NSM glyph with its preceding
visual unit when no inferred word gap separates them. The existing bidi pass reorders those
units; emission expands the original glyphs without rewriting their text, boxes or quads.
Unicode classification uses the already-present unicode-bidi 0.3 dependency; the lockfile
adds only the renderer dependency edge and updates no versions.

This is a viewer extraction/selection correction. It changes no PDF bytes, font shaping,
rasterization, editor fallback or Office export reader. It does not normalize Unicode.
Grouping/emission is linear in glyph count with additional linear index/range storage;
no dedicated throughput benchmark was run.

## Regression coverage

`crates/render/src/text/tests.rs` supplies three regressions and a reusable synthetic mapping
fixture helper. Coverage includes Arabic and Hebrew marks, multiple marks, mixed Latin,
orphan preservation, original glyph geometry, word and rectangular copy, search and a
PdfCraft serializer full-save/reopen round trip. Fixtures are contributor-original Rust
source under the project license. Helvetica outlines with synthetic Unicode mappings test
semantics, not Arabic font appearance; no font or generated PDF is committed.

## Validation

The isolated branch is checked with scoped renderer tests, all-target Clippy with warnings
denied, formatting, layers, assets and a renderer wasm32-unknown-unknown library check.
The production ordering module is temporarily restored from upstream while retaining the
new tests: both Arabic regressions must fail on the original misplaced-mark strings. The
fixed file is restored byte-for-byte before the full passing run.

Results on this isolated branch: 423 unit and 63 integration tests pass; one pre-existing
integration test remains ignored. Both new Arabic regressions fail with upstream production
ordering restored, then pass after the fixed file is restored. Formatting, native all-target
Clippy with warnings denied, layers and assets pass. The renderer WASM library check passes
with five existing target-specific warnings in unchanged pool/pixel/raster modules. No complete
web application or native clipboard integration was tested.

Exact commands:

```sh
timeout 900 cargo test --offline --locked -p pdfcraft-render -j 2
timeout 900 cargo clippy --offline --locked -p pdfcraft-render --all-targets -j 2 -- -D warnings
timeout 900 cargo check --offline --locked -p pdfcraft-render --target wasm32-unknown-unknown -j 2
cargo fmt --all -- --check
cargo xtask layers
cargo xtask assets
```

## Boundaries and coordination

This does not infer a base across a word gap, solve arbitrarily shifted marks, or reconstruct
all missing logical direction information. Full paragraph ActualText can remain atomic for
selection. Real-font appearance, OS clipboard integration and arbitrary producer-specific
PDFs are not qualified by the synthetic fixtures.

Merged #2736 already covers mixed RTL numbers/punctuation. Open #3465 covers broader Arabic
editing/ActualText work; its inspected ordering module is unchanged. This focused regression
is a supplement, not a replacement or a claim to resolve those broader issues.

## Revision history

- 2026-10-31: isolate the mapped-mark renderer fix from the broader Arabic investigation,
  retaining independent regressions and explicit scope limits.
