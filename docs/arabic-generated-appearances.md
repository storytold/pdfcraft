# Arabic generated appearances

Status: review-correction checkpoint, 2026-11-03. The affected tests, strict lint,
WASM, metadata and fresh CLI results below apply to baseline `c30bb5de`. Subsequent
review corrections still require post-fix validation. Platform, native-reader,
upstream and release acceptance remain pending.

This slice builds on the Arabic/Syriac local integration and reuses the generated
appearance implementation from [PR #3465](https://github.com/storytold/pdfcraft/pull/3465),
head `285d4697cb6399f702d21bc8452180755a360104`. Its module history includes
`0499eaf3` and `bf8097d2`; original work is by @Yousef-Dev921. The adaptation keeps
the existing Syriac selected-family writer and reconciles Arabic fallback with
effective formatted text, measured fit/overflow and staged resource installation.

## Bounded behavior

- Auto-direction text, combo, list and comb fields can use the optional Arabic
  document face when the selected form encoding cannot represent the displayed text.
- Native formatting is applied before selecting the appearance, so a numeric value
  with an Arabic currency suffix/prefix takes the same route as literal Arabic.
- Drawing and fit/overflow use the same shaped advances and mark bounds.
- An already encodable selected form font is preserved. This does not repair
  contextual joining in every custom-font form; that remains a separate case.
- Multiline drawing, fitting and overflow share the same complete row layout.
  List labels share ink-safe row metrics with selection scrolling and highlights;
  export values and selected indices remain unchanged.
- Comb fields shape independent grapheme cells, keeping marks with their bases.
  Joining and ligatures do not cross cell boundaries. Existing scalar-count
  `MaxLen` validation remains unchanged; it is distinct from visual cell count.
- Passwords retain the existing masked path and save policy. Explicit LTR/RTL
  shaping remains outside these Auto-direction form fallbacks.
- Syriac keeps its selected family. Unsupported explicit direction on a later
  Syriac text value is reported rather than silently losing the warning.
- New Arabic Type3 resources are materialized within the document; regeneration
  does not reuse document references from another display copy.

Arabic FreeText, callouts and typewriter text use contextual shaping and measured
ink bounds. An absent alignment follows paragraph direction; explicitly selected
left alignment stays left. Failed supported-text redraws preserve the document.
Headers, footers and text watermarks use private font resources without replacing
existing page resources, including on rotated pages. Typed Arabic signatures use
bounded contextual outlines; unsupported names are refused as a whole. Signatures
remain artwork, with no new searchable-text claim. Syriac retains its selected-family
path on these surfaces.

Header/footer and watermark requests select their Syriac family independently.
Omitting `syriac_family` selects Estrangela, including during replacement; pass the
family again to retain the previous face.

Missing fonts retain the existing warning/refusal policy. These changes do not add
arbitrary foreign annotation-font reuse, inherited-only choice-option support, or
general variable-font embedding.

The implementation uses existing form automation operations. No new font assets,
system-font embedding, CI changes or interface controls are introduced.

## Qualification boundary

The combined candidate passed 900 affected unit tests and 13 integration cases.
Its fresh CLI replay performed 36 successful generated-surface operations,
verified field/comment/text readback and byte-identical rasters on all three pages
through save/reopen, and exercised three expected refusals: explicit Arabic form
direction, scalar comb MaxLen and an unsupported typed-signature glyph. These are
synthetic engine results with the pinned optional font input, not no-font-build,
physical keyboard or fluent-reader acceptance. Typed signatures are outline
artwork; absent annotation Contents is correctly returned as JSON null.

Follow the local implementation tracker for exact commands and results. Tests,
CLI rendering, real keyboard/browser input, linguistic review, upstream acceptance
and release font adoption are separate gates. This document does not claim all
those gates have passed. The existing PR's reported tests are not substituted for
tests of this combined implementation.
