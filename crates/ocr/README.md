# Local scan processing

`cargo xtask models` downloads the hash-pinned, openly licensed models in
`ATTRIBUTION.toml`. No document is uploaded and no model is downloaded at runtime.
Release packages ship these files beside the application.

- English, German, French, Spanish, Catalan, Italian, Dutch, Danish, Norwegian Bokmål,
  Swedish, Finnish, Portuguese, horizontal Japanese and Chinese (traditional/simplified) use PaddlePaddle's
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
default. With `skip_text_pages: false`, recognition also reads pictures on pages
that already contain text. Words overlapping existing text are counted as
`already_on_page` and omitted; this comparison maps all four word-box corners
back through crop, rotation and deskew into the original page view. Per-page `crop_box` results are in original PDF user space, before corrections.
Corrections and OCR form one undo step; stale worker results and
cancelled UI runs do not modify the document.

The UI initially selects the primary OS/browser language independently of its interface
translation. Regional locales such as `de-AT`, `nl-BE` and `pt-BR` use `de`, `nl` and
`pt`; unavailable or unsupported locales fall back to English. Users can select a
different document language. CLI/MCP calls retain English as their deterministic default.

Limits: this scan pipeline does not correct perspective or dewarp pages. Camera photos
use the separate Enhance camera image command. No guarantee on photos or ambiguous text,
no vertical Japanese or Chinese claim, and deskew refuses annotated pages rather than separating
annotations from their targets. Supported OCR languages are deliberately limited to
those exposed and tested, despite the model's larger vocabulary.

Recognition includes a small margin around detected lines to retain capital accents
and final punctuation; invisible text retains the detected line's original height.
For the Latin languages, decoding excludes CJK, kana, full-width forms and letters
outside ASCII/Latin-1/Latin Extended-A, while preserving other European names,
digits and punctuation. Japanese and both Chinese choices retain the full dictionary. This is not a
language-specific spelling filter. Community French checks still found missed
`ï`, capital `Œ`/`Æ` and `Ë`, and inconsistent straight/curly apostrophes; the model
does not guarantee exact transcription. Search treats `'`, `’`, `‘`, `ʼ` and `＇` as one
character in both case modes, so either apostrophe finds the other.

Lines skewed by up to 20° whose baseline climbs more than a quarter of the line height
from end to end (for example in photographed pages read without deskew) are resampled
straight along their detected direction before recognition, so the recognizer sees neither
the neighbouring lines their bounding box would take in nor a sloping baseline. Chinese keeps axis-aligned crops, which its joined line fragments need.

After recognition, `typography.rs` repairs a few characters the model reliably confuses.
Each rule needs letters around the repaired character, so names, numbers and literal
text keep what was read:

| Languages | Read as | Becomes |
|---|---|---|
| all but Japanese and Chinese | `-` standing alone between two words | `–` |
| German | `"` or `,,` before a word / `"` after a word | `„` / `“` (`5"` stays) |
| French, Italian, Catalan | `1'` or `1’` starting a word, before a letter | `l'` / `l’` |
| Catalan | `l-l`, `l.l`, `l•l` between same-case `l` with letters around | `l·l` |

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
save/reopen. The same model is used without per-language spelling restrictions.
Icelandic remains unclaimed because community tests found weak recognition of þ/Þ and Í.

Historical print and handwriting use a separate optional model, downloaded only on
request. `historical::Historical::load` reads locally installed files;
`Ocr::recognize_historical` shares the ordinary print detector;
`Ocr::recognize_handwriting` finds regularly spaced, upright handwriting lines in
one text column using bounded ink projections. Both return source-image word boxes.
The model recognizes Fraktur and Kurrent with bounded 96-pixel line
inference, historical spelling and NFC output. See
[historical OCR](../../docs/historical-ocr.md) for provenance, test conditions and limits.
The French save/reopen regression covers accented capitals, ç, œ, straight
and curly apostrophes (allowing the model's normalization) and line-final punctuation,
with unchanged scan pixels. Decoder unit
tests cover European names and curly apostrophes without depending on model files.

Chinese choices use `zh-Hant` and `zh-Hans`. Explicit script subtags take precedence;
`zh-TW`, `zh-HK`, and `zh-MO` select traditional Chinese, while `zh-CN`, `zh-SG`,
and bare `zh` select simplified Chinese. Both use the same existing model and full
dictionary, without script conversion. Decoder, locale, UI selection, public-tool
dispatch and Unicode save/reopen tests cover this wiring; real Chinese scan accuracy
has not yet been measured. This adds no CJK font: Chinese interface glyph coverage
still depends on the available interface fonts (see docs/gaps.md, gap 5).

Community Italian checks report improved accents and word spacing, but still find
paragraphs in the wrong reading order, occasional joined words, and model spelling
errors. These are remaining recognition/layout limits, not guaranteed transcriptions.

Horizontal Chinese detections are joined before recognition when their vertical ink
bands substantially overlap and the horizontal gap is at most 1.25 ink heights
(about one em). Overlapping fragments are recognized once; separate rows and wider
column gaps remain separate. Searches are bounded to nearby fragments. Geometry
regressions include actual detector boxes from an original OFL BIZ UDPGothic fixture;
a local model comparison recovered its three lines and centred commas in reading
order. This is not a general reading-order solution or a guarantee for every font,
vertical writing, or thin numerals the detector misses entirely.

Catalan (`ca`, including `ca-ES`, `ca-AD` and Valencian locale variants) uses the same
Latin decoder. A model-backed regression checks accents after save/reopen with
unchanged pixels. Community checks still report missing `ï`. The geminated `l·l`,
which the model reads as `l-l` or `l.l`, is repaired only between two `l` of the same
case with letters on both sides (`col·legi`, `IL·LUSIÓ`), so names, addresses and
literal text such as `ball-2` or `www.ll.cat` stay intact. The current model dictionary
also lacks Spanish `¡`; a decoder filter cannot recover a class the model does not have.
