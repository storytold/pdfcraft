# Local text recognition

Scan & OCR → Recognize text offers **English** (`en`), **Chinese (Simplified) and English**
(`zh`), **Latin (accented)** (`la`), and the individual languages French, German, Spanish,
Portuguese, Italian, Dutch, Danish, Norwegian, Swedish, Icelandic, Finnish, Czech and Polish.
The Chinese recogniser reads Chinese, Latin letters and numbers in the same line. `la` and
those named languages share the Latin recogniser. A named language prefers its own letters
when two readings are close (`å` rather than `à` for Danish) and still keeps a letter from
another language when that reading is clearly ahead. The dictionary in
[ATTRIBUTION.toml](../ATTRIBUTION.toml) lists the exact character set. Choose the document
language in the dialog before recognising the current page, a range, all pages or multiple
files. The interface language and OCR document language are independent.

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
and `chinese-keys.txt`; Latin needs `text-detection.rten`, `latin-recognition.onnx`
and `latin-keys.txt`. Sources, versions, hashes and licences are in
[ATTRIBUTION.toml](../ATTRIBUTION.toml).

For an installed desktop app, put those files in a directory and launch with
`PDFCRAFT_MODELS` pointing to its absolute path. Alternatively, place `models/` beside
the executable, or in `PdfCraft.app/Contents/Resources/` on macOS. The source tree's
`assets/models/` is the final fallback for development builds. Model availability is
checked separately for each language; installing English alone does not enable Chinese
or Latin. Desktop release packages include the models. The Linux CLI-only archive does not.

Recognition runs entirely on the CPU in Rust, using RTen. English uses ocrs;
Chinese uses the Apache-2.0 PP-OCRv4 mobile recognition model and Latin the PP-OCRv5
Latin one, both converted to ONNX by RapidAI, with PaddleOCR's character dictionaries and
the existing ocrs detector. Only model installation needs network access. Recognition
never uploads a PDF or calls a cloud service.

## Automation

`ocr_recognize` and `ocr_recognize_files` accept `"language": "zh"`, `"language": "la"`, or a
named language such as `"fr"` or `"es"`.
`ocr_status` lists each language and its `available` flag. The legacy top-level `available`
flag continues to report English availability. For example, after `doc_open` returns
document 1:

```json
{"tool":"ocr_recognize","args":{"doc":1,"language":"zh","pages":[1]}}
```

## Limits

This is an initial searchable-image workflow, not an editable-text reconstruction.
It targets horizontal printed text. Handwriting, vertical text, automatic deskew, reliable
table reconstruction and rotated-line recognition remain unsupported. Reading order uses
the existing detector's line analysis; complex layouts can need review. Chinese and Latin
selection boxes cover a recognised line, or each piece of a line wider than about forty times
its height, with approximate character positions within a piece. A line that long used to be
refused, and the whole page with it; it is now read in pieces split at the gaps between words.
A single word that is still wider than the recogniser's input is squeezed into that input
instead of being dropped.
The Latin recogniser reads only the characters in its dictionary: accented letters outside
it (e.g. some East European ones) are still dropped. A letter that is in the dictionary can
still come back as another letter in it: on Árvíztűrő tükörfúrógép the model read ű as ü.
The same is true, by more than the close-call margin that `preferred_letters` can bridge, of
Danish, Norwegian, Swedish and Finnish lowercase å (misread as à, á or ã), Icelandic þ (misread
as a plain "b"), Czech's comma-shaped mark on ď, ť and ň and its ring on ů (all dropped), and
Polish ą, ź and ł (dropped) and ż (misread as Czech's ž). Each language's other accented
letters, including its vowels, come back correctly; `crates/engine/src/tests.rs`'s
`latin_ocr_keeps_*` tests cover the letters that do and name the ones that do not, language by
language.

OCR does not make visible text in a scan directly editable. Existing selectable PDF text
uses Edit PDF → Edit text and images, whose font/subset and CJK limitations are separate.
Vector artwork and nested image objects are also separate from OCR.

The core round-trip tests generate their own scans: a Chinese/English one using the permitted
BIZ UDPGothic font from `CRAFT_FONTS_DIR`, and a German one using the committed Inter face,
with umlauts and `ß` that the English models drop as `?`. Both require the model files.
Unicode-layer tests run without models or fonts and verify save/reopen, undo, supplementary
characters, font code boundaries and unchanged rendered pixels.
