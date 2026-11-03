# Combined Arabic and Syriac language package

Status: bounded implementation locally verified, 2026-11-03. Publication does not
establish upstream CI acceptance, maintainer approval or release qualification.
M4 physical keyboard/provider/browser and independent fluent-reader acceptance is
explicitly deferred by the contributor for this delivery. App-wide variable-font
conversion, OCR and general mixed-font/size/transform editing remain excluded.

## Foundation PRs and history

| Foundation | Reviewed source head | Role in this package |
|---|---|---|
| [Arabic #5731](https://github.com/storytold/pdfcraft/pull/5731) | `3392fb2ad373e0f17fa736661585076b75d77140` | Supported source-font editing, direction, native input safeguards and RTL reading foundations. |
| [Syriac #5776](https://github.com/storytold/pdfcraft/pull/5776) | `629d90abe1f914abb1ea66ccef2c2c9f2829ff8b` | Three-family Syriac content, form, annotation and page-mark foundations. |

Both foundation histories are retained as merge ancestry. The existing PRs remain
valuable review/evidence records and should stay open until maintainers decide
whether this consolidation replaces them. This branch does not claim either PR
has merged upstream and does not close or rewrite either source branch.

The foundations contain upstream baseline
`168a3cb91e90333a7069f2bac63835140c6d0f5b`. Newer upstream-main changes are not
silently included in this verified snapshot; later synchronization needs its own
overlap review and affected checks.

Earlier CodeRabbit conclusions apply to their inspected foundation heads:
[Arabic handoff](https://github.com/storytold/pdfcraft/pull/5731#issuecomment-6588532174),
[Syriac handoff](https://github.com/storytold/pdfcraft/pull/5776#issuecomment-6586599209).
They are not bot approval of this larger combined package.

## Added bounded work

- M1: one shared Arabic/Syriac generated-text path with effective formatting,
  measured fit/overflow, password masking and selected-font/family priority.
- M2: Auto Arabic single/multiline/combo/list/comb appearances; FreeText, callouts,
  typewriter text, headers/footers, text watermarks and contextual typed signatures.
- M3: original UTF-8 cluster provenance, whole-cluster colour edits that retain
  earlier colours across reopen/recolour, and uniform single-row quarter turns.

See [generated appearances](arabic-generated-appearances.md),
[bounded source editing](contextual-source-editing.md) and
[Syriac content](syriac-content.md) for actual admission/refusal boundaries.
Typed signatures remain outline artwork, without searchable-text claims.
Explicit-direction Arabic form shaping and general contextual mixed styles remain
outside these Auto-direction/generated and compatible-source cases.

## Reuse and font provenance

Generated appearance and related font/layout work adapts
[@Yousef-Dev921's PR #3465](https://github.com/storytold/pdfcraft/pull/3465), reviewed
at `285d4697cb6399f702d21bc8452180755a360104`, including module history `0499eaf3`
and `bf8097d2`. Attribution is retained in the adaptation commit and scope record.
This is not a claim to have originated that implementation, accepted the donor's
entire feature scope or qualified every donor test. Its branch is not rewritten.

The optional OFL font input is craft-fonts
`758bdc0ea05af952d66ded15572d127f877866f1`; no font binary is committed here.
Syriac delivery still depends on
[craft-fonts #61](https://github.com/storytold/craft-fonts/pull/61) and authorized
official release-input pin adoption. The source-edit test oracle is an external
FontTools 4.60.1 static default instance of the pinned Noto Sans Arabic face
(`wght=400`, `wdth=100`); that fixture is not application variable-font instancing.

## Verification and documentation

Repository optimization defaults and at most two compiler jobs were retained.
The tested Rust/workspace content digest before the final comments-only pass was
`ed9b38843b7d28bbaab6a9968e91fcf23df87a36226537f5824b97597b9c9eb8`.
The final documentation changes preserve every non-comment Rust token in their
affected files; prior runtime/compiler evidence is retained with that attribution.

| Check | Recorded local result |
|---|---|
| Affected native tests | 900 unit tests and 13 integration cases passed. |
| Strict all-target Clippy | fonts/forms/annot/edit/engine/automation passed with `-D warnings`; identical tested source reused rather than repeated. |
| WASM | fonts/forms/annot/edit checks passed. |
| Metadata and format | Checkout-rooted layers/assets/parity and Rustfmt passed; final comments are format/token checked. |
| Fresh CLI | Built successfully; SHA-256 `2cda7de437474e1feedafe4779b376d2943ede478f338de16ed4ef26eaa881fb`. |
| M2 public tools | 36 successful operations; logical field/comment/mark readback, three page raster pairs identical through save/reopen; three expected refusal sessions. |
| M3 public tools | Four source orientations, preview/Apply bounds, full-resource presence, logical extraction, unchanged artwork, repeated cluster recolouring and save/reopen raster equality passed. |
| Visual inspection | Three M2 pages and four orientations plus final painted M3 page inspected, without an obvious visible blocker. Agent observation, not fluent-reader acceptance. |
| Documentation | Local lexical inventory: 342/342 touched functions and 270/270 complexity-selected functions have Rustdoc; zero parse errors. This is not CodeRabbit's denominator or a provider-issued score. |

The affected commands were:

```text
cargo test -p pdfcraft-fonts -p pdfcraft-forms -p pdfcraft-annot -p pdfcraft-edit -j 2 --no-fail-fast
cargo clippy -p pdfcraft-fonts -p pdfcraft-forms -p pdfcraft-annot -p pdfcraft-edit -p pdfcraft-engine -p pdfcraft-automation -j 2 --all-targets -- -D warnings
cargo check -p pdfcraft-fonts -p pdfcraft-forms -p pdfcraft-annot -p pdfcraft-edit --target wasm32-unknown-unknown -j 2
cargo xtask layers
cargo xtask assets
cargo xtask parity
cargo fmt --all --check
cargo build -p pdfcraft-cli -j 2
```

Font-dependent qualification supplied `CRAFT_FONTS_DIR`, `CRAFT_FONTS_REQUIRED=1`
and the external static test-oracle path through
`PDFCRAFT_TEST_ARABIC_STATIC_FONT`. Tests that skip optional inputs do not establish
no-font-build acceptance. There was no full workspace sweep.

The resumed native attempt initially failed to link interrupted annotation/edit
incremental artifacts. Preserving/regenerating only those caches produced the
passing run, without changing source, optimization or assertions. The first replay
scripts also had two expectation mistakes: absent stamp Contents is JSON null,
and replacing a short Helvetica source box with taller Arabic correctly reports
overflow. Corrected expectations retained explicit overflow checks, geometry and
save/reopen assertions and reused the same CLI binary.

The attempted M4 desktop build hit its first bounded timeout. Its one continuation
was stopped when the contributor deferred M4. Neither is desktop/input acceptance
or a demonstrated product regression. No fresh desktop execution is claimed.

Required exact-head upstream CI authorization/results, maintainer acceptance,
official font adoption and broader M4 qualification remain separate gates.
