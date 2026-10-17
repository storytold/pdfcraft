# Local scan processing

`cargo xtask models` downloads the hash-pinned, openly licensed models in
`ATTRIBUTION.toml`. No document is uploaded and no model is downloaded at runtime.
Release packages ship these files beside the application.

- English, German, French, Spanish, Italian, Dutch, Danish, Norwegian Bokmål,
  Swedish, Finnish, Portuguese and horizontal Japanese use PaddlePaddle's
  PP-OCRv6 small model through the existing Rust RTen runtime. The legacy English
  recognizer API remains available to existing callers.
- PP-LCNet classifies quarter-turn orientation. Preprocessing centres on ink bounds,
  resizes the short side to 256, crops to 224 and uses the model's RGB normalization.
  Only predictions at or above 0.90 confidence are applied.
- Deskew searches projection profiles at bounded resolution over ±12 degrees. Weak,
  blank and dense-image results stay unchanged. Fine correction scales the whole page
  to fit inside its original bounds; it does not crop or recompress its images.
- Optional paper-edge cropping runs before alignment and OCR. A long, coherent edge
  against a black/white scanner background defines the page, including blank paper and
  colored covers. Indistinguishable white-on-white borders are retained; text bounds
  never define the crop. A 0–20 pt safety margin defaults to 2 pt. The PDF CropBox changes
  without resampling or deleting the original image; cropped content remains in the file.
- OCR operates on corrected pixels, maps recognized boxes back to source coordinates,
  then transforms the PDF content and invisible Unicode text together. The engine
  uses empty Type 3 glyphs with ToUnicode mappings, so OCR needs no Japanese font asset.

The UI exposes independent rotation, deskew, paper-edge crop and searchable-text switches. Automation:
`ocr_recognize` and `ocr_recognize_files` accept `language`, `auto_rotate`, `deskew`
and `recognize`, plus `auto_crop` and `crop_margin`; `ocr_enhance` corrects scans without
adding text and enables cropping by default, including pages with existing OCR text.
Rotation/deskew are enabled by default. Disable all three corrections for exact-image
output. Ordinary recognition leaves cropping off and skips existing text pages by
default. Per-page `crop_box` results are in original PDF user space, before corrections.
Corrections and OCR form one undo step; stale worker results and
cancelled UI runs do not modify the document.

The UI initially selects the primary OS/browser language independently of its interface
translation. Regional locales such as `de-AT`, `nl-BE` and `pt-BR` use `de`, `nl` and
`pt`; unavailable or unsupported locales fall back to English. Users can select a
different document language. CLI/MCP calls retain English as their deterministic default.

Limits: no perspective correction/dewarping, no guarantee on photos or ambiguous text,
no vertical Japanese claim, and deskew refuses annotated pages rather than separating
annotations from their targets. Supported OCR languages are deliberately limited to
those exposed and tested, despite the model's larger vocabulary.

Validation: `cargo test -p pdfcraft-engine --test scan_ocr` covers actual models,
languages, all quarter-turns, deskew, crop offsets and padding, unchanged exact-mode pixels, save/reopen, undo
and stale results. Model tests report a skip when models are absent. The Japanese
synthetic fixture uses the optional release `CRAFT_FONTS_DIR` checkout; generated
fixtures and third-party document samples are not committed.

The Italian regression recognizes all six accented vowels (`à è é ì ò ù`) in a
synthetic sentence and checks that they survive saving/reopening without changing
the scanned pixels. This is a regression fixture, not a real-document accuracy benchmark.

The additional Latin-language regression checks Dutch `één`, `coöperatie`, `België`
and accented words in Danish, Norwegian, Swedish, Finnish and Portuguese after
save/reopen. The same model is used without per-language character restrictions.
Icelandic remains unclaimed because community tests found weak recognition of þ/Þ and Í.
