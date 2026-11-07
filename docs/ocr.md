# Local text recognition

Scan & OCR → Recognize text offers **English** (`en`) and **Chinese (Simplified) and English**
(`zh`) for the built-in engine. The Chinese recogniser reads Chinese, Latin letters and numbers
in the same line. Choose the document language in the dialog before recognising the current
page, a range, all pages or multiple files. The interface language and OCR document language
are independent.

The output is a searchable image: original page images and content stay intact, with an
invisible text layer for selection, copying, search and text export. Recognition is one
undoable step. By default, pages that already contain text are skipped. Searchable output
keeps every Unicode character in Type 3 fonts with empty character programs and a ToUnicode
map, so Chinese is searchable without embedding a display font. The Editable text output
opens the OCR Verify screen and writes visible text with the bundled fonts.

## Install the models

From a source checkout:

```sh
cargo xtask models
```

This downloads the models and licence notices into `assets/models/`, verifies their pinned
SHA-256 hashes, and leaves them outside Git. English needs `text-detection.onnx` and
`text-recognition.onnx` (a complete legacy `text-detection.rten` /
`text-recognition.rten` pair still works); Chinese needs the detector, `chinese-recognition.onnx`
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

An installed `tesseract` program is an optional second engine (see
[development](development.md)); it reads the language packs it has installed, and `zh` maps to
its `chi_sim` pack. No language pack ships with PdfCraft.

## Automation

`ocr_recognize`, `ocr_recognize_files`, `ocr_words` and the review tools accept
`"language": "zh"`. `ocr_status` lists each language with an `available` flag for the built-in
models and an `engines` list (ocrs, tesseract) with each engine's own availability. The legacy
top-level `available` flag continues to report English availability. For example, after
`doc_open` returns document 1:

```json
{"tool":"ocr_recognize","args":{"doc":1,"language":"zh","pages":[1]}}
```

## Limits

This is an initial Chinese searchable-image workflow, not an editable-text reconstruction.
It targets horizontal printed text. Handwriting, vertical text and rotated-line recognition
remain unsupported; auto-rotate, deskew, denoise and binarize are available preprocessing
steps, but reliable table reconstruction is not. Reading order uses the existing detector's
line analysis; complex layouts can need review. Chinese selection boxes cover a recognised
line, with approximate character positions within it.

OCR does not make visible text in a scan directly editable. Existing selectable PDF text
uses Edit PDF → Edit text and images, whose font/subset and CJK limitations are separate.
Vector artwork and nested image objects are also separate from OCR.

The core Chinese round-trip test generates its own Chinese/English scan using the permitted
BIZ UDPGothic font from `CRAFT_FONTS_DIR`; it requires the model files. Unicode-layer tests
run without models or fonts and verify save/reopen, undo, supplementary characters, font
code boundaries and unchanged rendered pixels.
