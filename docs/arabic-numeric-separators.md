# Arabic decimal and grouping separators in edit-text recovery

Status: focused regression fix, refreshed against upstream `47f42ba0` on 2026-11-01.
Original base: upstream main
`13655d09922038ab62d93b84ba9cd2eae474abd7`. Estimates and readiness claims are unchanged.

## Reproduced behavior and correction

The edit-text visual/logical recovery can change `١٢٣٫٤٥` into `٤٥٫١٢٣`. Its digit-only
classification treats Arabic U+066B DECIMAL SEPARATOR and U+066C THOUSANDS SEPARATOR as
right-to-left script characters and splits the surrounding numeric sequence.

`crates/edit/src/bidi.rs` now recognizes Unicode bidi classes EN and AN as numbers, using
the existing unicode-bidi dependency. Both separators have class AN. The same classification
is used when excluding numbers from RTL script characters and when grouping recovery runs.
No dependency, lockfile, font, PDF serializer, renderer or export-format changes are needed.

The regression covers Arabic-Indic digits with a decimal separator, decimal plus grouping,
Persian digits, and Western decimal digits in a mixed phrase. It asserts exact logical
Unicode after visual/logical round trip; no string normalization or digit conversion occurs.

## Validation

The isolated branch runs the editor crate's existing suite, all-target Clippy with warnings
denied, formatting, layers, assets and an editor wasm32-unknown-unknown library check.
Restoring only the old digit classifier while retaining the final test must make the new
regression fail; restore the production file byte-for-byte before the passing run.

Results on the isolated branch: all 359 editor unit tests pass, with the pinned craft-fonts
input enabled; there are no integration or documentation tests in this branch. The new
regression fails with the old classifier restored and passes after restoration of the fixed
file. Formatting, all-target Clippy with warnings denied, the scoped editor WASM library
check, layering and assets pass. Only the existing optional egui patch notices remain.

```sh
timeout 900 cargo test --offline --locked -p pdfcraft-edit -j 2
timeout 900 cargo clippy --offline --locked -p pdfcraft-edit --all-targets -j 2 -- -D warnings
timeout 900 cargo check --offline --locked -p pdfcraft-edit --target wasm32-unknown-unknown -j 2
cargo fmt --all -- --check
cargo xtask layers
cargo xtask assets
```

## Boundaries and coordination

The 2026-11-01 refresh retains both roadmap histories and changes no production logic
relative to the original fix. The old-classifier mutation fails the numeric-order
regression; the restored source passes all **359 editor tests**, with no failures or
ignores. Scoped Clippy with warnings denied passes. Remaining final gate results are
recorded in the PR description for the submitted head; historical results above are
identified separately from this refresh. No engine/GUI/workspace sweep is implied.

This corrects the numeric classifier in the existing bounded recovery algorithm. It does not
promise universal inverse bidi, repair missing Unicode mappings, fix the separate Office
reader/export direction metadata, or qualify all source-PDF producers. The branch is
independent of the mapped-diacritic viewer fix and the larger Arabic source-editing work.

Merged #2736 fixes related mixed-number/punctuation behavior in viewer extraction. Open
#3465 covers broader Arabic editing/ActualText behavior; its inspected bidi module still uses
the old digit classifier and its additional cluster helper is separate from this correction.

## Revision history

- 2026-11-01: integrate current upstream, retain both roadmap records, repeat classifier
  sensitivity proof and editor qualification, and keep publication evidence head-specific.
- 2026-10-31: isolate the EN/AN numeric-separator correction and its regression from the
  broader Arabic investigation, with no new dependency or font requirement for the regression.
