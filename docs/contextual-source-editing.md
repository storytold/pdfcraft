# Contextual source-text editing: selected cases

Status: locally verified candidate, 2026-11-03. Affected library/public-API tests
and the fresh CLI/MCP render/save/reopen replay pass. Upstream, platform and
native-reader acceptance remain separate.

Existing contextual text supports recoloring whole shaping clusters within one
compatible complete face, size, spacing and baseline. The row is shaped together,
so changing a color does not break joining or reposition attached marks. A selection
that splits an actual shaping cluster is refused before changing the document.

PdfCraft preserves logical colors using a bounded private PDF hint alongside
standard `ActualText`. Recoloring survives reopening and a subsequent recoloring.
Further text replacement on mixed-color contextual spans is currently refused.
If another editor removes the hint, PdfCraft retains readable text and refuses
edits that would flatten mixed source colors. The hint never changes rendering
geometry and malformed or over-budget hints are ignored safely.

Uniform, single-row source text can also be edited at quarter-turn orientations.
Preview and apply use the same baseline frame, including after reopening and on
pages with their own display rotation. General angles, skew, nonuniform transforms,
mixed transforms, wrapping, and range styling on turned rows remain unsupported.
Contextual scripts require a complete usable source face; arbitrary incomplete
subsets and legacy visual-order encodings are not repaired by this feature.

The existing `text_paragraphs`, `text_edit_layout`, `text_edit`, and font-import
automation operations expose these cases. Paragraph identifiers must be refreshed
after each edit. Source-font data and unrelated page artwork are preserved.

Variable-font static instancing remains a separate investigation. Raw variable
fonts retain their existing embedding refusal; a test-only external static font
fixture is not evidence that the application implements variable-font conversion.

The synthetic public-tool replay covers all four quarter-turn orientations,
preview/Apply bounds, retained full-face resources, logical extraction and
byte-identical save/reopen rasters. Upright cluster recolouring retains the first
colour after a second edit and reopening, with unrelated artwork unchanged.
Initial Arabic replacement exceeds the short Helvetica source box and correctly
reports preview overflow; the requested width controls wrapping, not that source
box comparison. Paint-only edits retain their no-overflow result. Signed baseline
angles, serialized font-byte identity and mixed paint on rotated text are not
independently established by this replay.
