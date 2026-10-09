# Local text recognition

Scan & OCR → Recognize text offers **English** (`en`) and **Chinese (Simplified) and English**
(`zh`). The Chinese recogniser reads Chinese, Latin letters and numbers in the same line.
Choose the document language in the dialog before recognising the current page, a range,
all pages or multiple files. The interface language and OCR document language are independent.

The output is a searchable image: original page images and content stay intact, with an
invisible text layer for selection, copying, search and text export. Recognition is one
undoable step. By default, pages that already contain text are skipped.

## Install the models

From a source checkout:

```sh
cargo xtask models
```

This downloads the models and licence notices into `assets/models/`, verifies their pinned
SHA-256 hashes, and leaves them outside Git. English needs `text-detection.rten` and
`text-recognition.rten`; Chinese needs `text-detection.rten`, `chinese-recognition.onnx`
and `chinese-keys.txt`. Sources, versions, hashes and licences are in
[ATTRIBUTION.toml](../ATTRIBUTION.toml).

For an installed desktop app, put those files in a directory and launch with
`PDFCRAFT_MODELS` pointing to its absolute path. Alternatively, place `models/` beside
the executable, or in `PdfCraft.app/Contents/Resources/` on macOS. The source tree's
`assets/models/` is the final fallback for development builds. Model availability is
checked separately for each language; installing English alone does not enable Chinese.
Current release installers do not automatically include these model files.

Recognition runs entirely on the CPU in Rust, using RTen. English uses ocrs;
Chinese uses the Apache-2.0 PP-OCRv4 mobile recognition model converted to ONNX by RapidAI,
with PaddleOCR's character dictionary and the existing ocrs detector. Only model installation
needs network access. Recognition never uploads a PDF or calls a cloud service.

## Automation

`ocr_recognize` and `ocr_recognize_files` accept `"language": "zh"`. `ocr_status` lists
each language and its `available` flag. The legacy top-level `available` flag continues
to report English availability. For example, after `doc_open` returns document 1:

```json
{"tool":"ocr_recognize","args":{"doc":1,"language":"zh","pages":[1]}}
```

## Limits

This is an initial Chinese searchable-image workflow, not an editable-text reconstruction.
It targets horizontal printed text. Handwriting, vertical text, automatic deskew, reliable
table reconstruction and rotated-line recognition remain unsupported. Reading order uses
the existing detector's line analysis; complex layouts can need review. Chinese selection
boxes cover a recognised line, with approximate character positions within it.

OCR does not make visible text in a scan directly editable. Existing selectable PDF text
uses Edit PDF → Edit text and images, whose font/subset and CJK limitations are separate.
Vector artwork and nested image objects are also separate from OCR.

The core Chinese round-trip test generates its own Chinese/English scan using the permitted
BIZ UDPGothic font from `CRAFT_FONTS_DIR`; it requires the model files. Unicode-layer tests
run without models or fonts and verify save/reopen, undo, supplementary characters, font
code boundaries and unchanged rendered pixels.
