# Native form-field detection

> **Last reviewed:** 2026-10-17 · **Last updated:** 2026-10-17 · **Change:** initial native FFDetr engine and automation integration · **Status:** partial; explicit local model, no bundled weights or UI default switch

PdfCraft can use a local FFDetr model to propose and create text, checkbox and
signature fields over a flat or scanned PDF. This runs in Rust using RTen;
neither Python nor a server is required during detection.

The first milestone is available through the engine and the existing CLI/MCP
`form_detect_fields` tool. The desktop Prepare a form command still uses the
existing geometry detector. Running the tool without `model` keeps its existing
behavior. No new model is automatically downloaded or included in releases yet.

## CLI/MCP

Put the approved self-contained ONNX export inside the CLI's permitted read
directory. The model path follows the same `--root`, symlink and read-root
checks as document paths. The loader verifies the model's SHA-256 before
parsing it. Provenance and the supported hash are recorded in the
[inference crate README](../crates/form-detect/README.md#model-provenance-and-limits).

For a directory containing `input.pdf` and `ffdetr-576.onnx`, a script is:

```json
[
  {"tool":"doc_open","args":{"path":"input.pdf"}},
  {"tool":"form_detect_fields","args":{"doc":1,"model":"ffdetr-576.onnx","confidence":0.3}},
  {"tool":"doc_save","args":{"doc":1,"path":"fillable.pdf"}}
]
```

Run it using:

```sh
cargo run -p pdfcraft-cli -j 2 -- run --root /path/to/directory --script /path/to/steps.json
```

Use `add: false` to inspect proposals without modifying the document. `pages`
selects 1-based page numbers; omission means all pages. `confidence` must be
between 0 and 1 and requires `model`. The default is 0.3. Results include the
backend (`ffdetr`), kind, name, confidence and rectangle. Detection rectangles
retain the existing tool convention: **PDF user-space points, bottom-left
origin**, unlike the displayed coordinates returned by `form_fields`.

All fields are created as one undoable batch, with transparent fill and no
border so the printed form remains visible. Existing fields are skipped.
Model inference runs once per request; adding fields reuses those proposals.
Signature proposals create empty PDF signature fields, not signatures.

## Engine jobs

`Session::form_detection_job` captures immutable page/document state. `Job::run`
accepts a reusable detector and a progress callback; returning false cancels
without producing a partial result. `Session::apply_detected_fields` rejects
results if the source document changed or closed. Adding fields goes through
the normal permission checks, transaction, save and undo mechanisms.

Pages are rendered one at a time at up to 144 dpi with the longest side capped
at 3000px. The image detector itself is limited to 12 million pixels. Model
coordinates are transformed through the page's displayed geometry, including
rotation and crop origin. Turned widgets retain their text orientation.

Labels are currently taken from the existing text layer using the shared
nearby-label rules. Image-only pages get default field names. Automatic radio
grouping, comb properties, improved label association and model-resolution
selection remain future work. Confidence indicates model certainty, not a
guarantee that the proposed field is correct.

## Validation and remaining work

Unit tests cover preprocessing, invalid buffers, output shapes, class selection
and duplicate suppression. Engine tests cover rotations, duplicate detection,
cancellation, stale snapshots, undo/redo and save/reopen. Tool tests generate a
tiny original ONNX model at runtime and exercise preview, creation, filling,
save/reopen, invalid inputs and model-path confinement without downloading weights.
The tiny model does not test FFDetr accuracy; that needs separate evaluation
with the real pinned export and representative documents.

The real export has also been run through the CLI on a private ten-page PDF,
including save/reopen, duplicate rejection and rendered-page inspection. On the
same reference page image, native inference returned the same 53 proposals as
CommonForms, with maximum normalized coordinate error below 0.000001. Rendering
the PDF with PdfCraft rather than PDFium changes some proposals, so this is
evidence for inference compatibility, not a general accuracy benchmark.

Before a default desktop rollout: publish and attribute a verified model
artifact; package it alongside OCR models; add background UI progress/cancel
and model reuse; validate a broader document set and measure memory/CPU time.
Keep the geometry detector available when the model is absent. Browser model
delivery and scheduling are separate work; the Rust inference crate must
continue to compile for WASM.

## Revision history

| Date | Change |
|---|---|
| 2026-10-17 | Native FFDetr engine jobs and explicit-model CLI/MCP support. |
