# pdfcraft-form-detect

Local FFDetr inference on page images. Layer L4; no PDF parser, UI, Python runtime,
network access or model downloads. Uses the same RTen 0.26 runtime as PdfCraft OCR.

This is the first native integration milestone. The engine exposes a cancellable
snapshot job, and CLI/MCP `form_detect_fields` accepts an explicit local `model`
path. Prepare a form still uses the existing detector until model packaging and
background UI integration land. See [the integration guide](../../docs/form-field-detection.md).

## API

```rust,no_run
use pdfcraft_form_detect::Detector;
let detector = Detector::load(std::path::Path::new("ffdetr-576.onnx"))?;
// Pixels are premultiplied RGBA8, as returned by PdfCraft's page renderer.
let fields = detector.detect(&rgba, width, height, 0.3)?;
```

Retain a `Detector` between jobs. It uses at most two CPU inference threads.
Proposals contain type (`Text`, `CheckBox`, `Signature`), confidence and normalized
top-left `[left, top, right, bottom]` coordinates. The engine owns naming,
page-coordinate conversion, filtering existing fields and undoable edits.

Preprocessing matches the measured CommonForms reference: square 576px input,
bilinear resize with half-pixel centres and **no antialiasing**, ImageNet RGB mean
and standard deviation, NCHW float tensor. Outputs are normalized centre/size
boxes and class logits; the supported classes are 0/1/2. Unused checkpoint class
slots are ignored. Highest-confidence overlapping proposals compete across
classes using IoU 0.1 suppression, matching the tested reference result.

## Model provenance and limits

The initial supported export comes from Joe Barrow's
[FFDetr](https://huggingface.co/jbarrow/FFDetr), declared Apache-2.0 in its model
card and [confirmed by the author](https://github.com/jbarrow/commonforms/issues/9#issuecomment-3578531925).
It is separate from FFDNet-S/L, whose weight licensing remains unresolved.

- Upstream revision: `56f4e4235e28dcb2953513dc020bb191a2f54cfe`.
- Checkpoint: `FFDetr.pth`, SHA-256
  `f852e1bac18c8f435b82270fc8ff8e2ca4a2cd8869c411fa8f473f16e69585ef`.
- Self-contained ONNX export: `ffdetr-576.onnx`, SHA-256
  `5e5920c8cbf7287b9fef4d67c0d20728a7155e884ae638ee75926b1bbae79370`.
- Export configuration: RF-DETR Medium, 576 square pixels, batch 1, float32,
  opset 17; CommonForms 0.2.1, RF-DETR 1.11.2, PyTorch 2.14.1+cpu.
- Apache-2.0 text: [LICENSE-APACHE](../../LICENSE-APACHE).

No weights are committed, fetched by the build, or bundled by this milestone.
The caller supplies an existing local export. A release artifact needs its own
`ATTRIBUTION.toml` entry, pinned download and notices before packaging is enabled.

Only the approved export hash is accepted **before parsing or execution**.
This prevents arbitrary user-supplied ONNX graphs from requesting enormous
intermediate allocations or referring to other files. Model reads are capped
at 256 MiB; page inputs at 12 million pixels; output dimensions are checked.
External tensor files and arbitrary replacement models are unsupported.

The `test-model` feature exposes a tiny, contributor-original constant-output
ONNX fixture, generated in Rust from the public ONNX protobuf schema. It adds
only that fixture's hash to the allowlist for downstream integration tests.
Applications must not enable it. The fixture tests the complete tool/edit/save
path; it is not evidence of learned-model accuracy.

## Development check

```sh
cargo test -p pdfcraft-form-detect -j 2
cargo run -p pdfcraft-form-detect --release -j 2 --example detect -- \
  /path/to/ffdetr-576.onnx /path/to/page.png
```

The example prints proposals as JSON. Actual inference does not require image
codecs; the PNG dependency is limited to examples/tests.
