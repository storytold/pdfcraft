# Native form-field detection

> **Last reviewed:** 2026-10-17 · **Last updated:** 2026-10-17 · **Change:** modal input blocking, duplicate filtering and offline notice repair · **Status:** native desktop and automation implemented; public download awaits publication of the model release asset; browser retains geometry detection

Prepare a form uses FFDetr to find text fields, checkboxes and signature areas
on printed or scanned forms. It runs locally in Rust using RTen. No document is
uploaded, and neither Python nor an inference server is required.

## Desktop

On the first use, Prepare a form offers **Download and detect**, explaining the
121 MB download from GitHub. Nothing is fetched at startup or before that
consent. The user can cancel or explicitly use basic detection instead.
Download progress, detection progress, cancellation and retry remain in the app;
there is no model picker or technical configuration to complete.

The consent and progress dialogs block document shortcuts and canvas input.
Escape or Cancel dismisses the dialog and requests cancellation. Document input
resumes when the dialog closes, even if a worker still needs time to stop.

The download verifies its exact size and SHA-256, installs atomically and keeps
licence/attribution notices beside it. A failed or cancelled transfer preserves
the previous model and leaves the document unchanged. Retries restart the transfer.
A verified installed model works offline. One detector is retained between jobs
so repeated detection does not reload the weights. Corrupt installations offer
the download again; filesystem errors are reported.

An explicit install request repairs missing or incorrect licence notices beside
an existing verified model without downloading it again. Correct notices stay
in place, so repeated installation needs no notice writes.

Models live in the user's application data directory: `$XDG_DATA_HOME/pdfcraft/models/ffdetr`
(or `~/.local/share/pdfcraft/models/ffdetr`) on Unix, `~/Library/Application Support/PdfCraft/models/ffdetr`
on macOS, and `%LOCALAPPDATA%/PdfCraft/models/ffdetr` on Windows. They are separate
from the application installer and shared across app versions.

**Publication prerequisite:** the versioned model artifact is configured at
`https://github.com/storytold/pdfcraft/releases/download/models-ffdetr-576-v1/ffdetr-576.onnx`.
That release has not been published as part of this work. Until maintainers publish
it, a clean installation reports that the download is unavailable and can retry
later. The local development machine already has the verified evaluation model,
so native desktop detection can be tested immediately.

## CLI/MCP

The same functionality is available headlessly:

- `form_detection_status`: verifies the local model without network access and
  reports availability, location, download URL and size.
- `form_detection_install` with `confirm_download: true`: explicitly downloads
  and verifies the pinned release. An installed valid model needs no network.
- `form_detect_fields` with `backend: "ffdetr"`: uses the installed model.
  `directory` optionally selects a managed model directory for constrained runs.
- `form_detect_fields` with `model`: retains explicit-file support for developers.
  Omission of both options preserves the CLI's existing geometry behavior.

For a root directory containing a PDF, this script installs the optional model
there and detects its fields (after the model asset is published):

```json
[
  {"tool":"form_detection_install","args":{"directory":"models","confirm_download":true}},
  {"tool":"doc_open","args":{"path":"input.pdf"}},
  {"tool":"form_detect_fields","args":{"doc":1,"backend":"ffdetr","directory":"models"}},
  {"tool":"doc_save","args":{"doc":1,"path":"fillable.pdf"}}
]
```

All model reads and installation writes, including nested files, honour `--root`
and separate automation read/write roots. Installation needs both read and write
access. Detection never implicitly downloads a model. The automation calls are
synchronous; the desktop uses workers and reports progress.

`add: false` previews proposals. `pages` selects 1-based page numbers (default:
all). `confidence` is finite and within 0–1, default 0.3; it requires FFDetr.
Results include kind, name, confidence and rectangle. Detection rectangles use
**PDF user-space points, bottom-left origin**, unlike `form_fields` display
coordinates. New fields are transparent and skip existing fields. Signature
proposals create empty PDF signature fields, not signatures.

## Engine and correctness

`Session::form_detection_job` captures immutable document state. `Job::run` accepts
a reusable detector and cancellation/progress callback. `apply_detected_fields`
rejects changed or closed documents and applies the complete result as one
undoable batch through the normal permission, transaction and save mechanisms.
Cancellation never applies partial results. The UI uses the original document ID,
so switching tabs cannot apply results to another document.

A proposal duplicates an existing widget only when their intersection covers at
least half of each rectangle. A small widget inside a larger distinct proposal
does not suppress that proposal.

Rendering is page-at-a-time, up to 144 dpi, capped at 3000px on the longest side.
The image detector is limited to 12 million pixels. Coordinates account for crop
origin and page rotation; widget text retains its displayed orientation. Names
come from the existing text layer; image-only pages use default names.

Tests cover bounded/verified downloads, preserved files after failure, consent,
retry, live cancellation, offline operation, detector reuse, confinement, preview,
fill/save/reopen, undo/redo, rotations, duplicate rejection and stale snapshots.
Synthetic ONNX fixtures are generated in Rust, without downloading model weights.
The real export was also validated through the CLI on a private ten-page PDF.
On an identical reference page image, native inference returned the same 53
proposals as CommonForms with normalized coordinate error below 0.000001.
PdfCraft's renderer produces some different proposals from PDFium; this is an
inference compatibility check, not a general accuracy benchmark.

## Separate model release

The provenance and hash are recorded in `ATTRIBUTION.toml` as `optional-model`.
It is deliberately excluded from `cargo xtask models` and all app installers.
Prepare the independently distributable files with:

```sh
cargo xtask form-model-package /path/to/ffdetr-576.onnx
```

The command verifies the input and writes the model, Apache licence, attribution
and checksums to `dist/models-ffdetr-576-v1/`. It never uploads anything. Maintainers
publish those files as assets on GitHub release **models-ffdetr-576-v1**,
marked **pre-release** and **not latest** so it cannot replace the app release
returned by the existing update checker. Changing
the export requires a new versioned release URL, hash, size and attribution entry.
The supported export configuration is in the [inference README](../crates/form-detect/README.md).

Remaining work: publish that release asset, evaluate a broader document set,
and separately design browser model delivery/scheduling. Radio grouping, comb
properties and OCR-assisted label association are not inferred yet.

## Revision history

| Date | Change |
|---|---|
| 2026-10-17 | Detection dialogs own document input, duplicates require coverage of both rectangles, and explicit installation repairs notices offline. |
| 2026-10-17 | Desktop download consent, progress/cancel/retry, reusable native detector, managed-model automation and separate release packaging. |
| 2026-10-17 | Native FFDetr engine jobs and explicit-model CLI/MCP support. |
