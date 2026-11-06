# pdfcraft-ocr

Layer L4 (may use `fonts` for the text layers' metrics): Scan & OCR — recognising the words in
a page raster and turning them back into page content (execution plan M10).

```rust
// The caller renders a page to pixels and hands them over:
let image = OcrImage::new(w, h, rgba)?;                  // validated by construction
let lines = recognizer.recognize(&image, &options)?;     // Vec<Line>: words in reading order
let placed: Vec<PlacedWord> = …;                         // PlacedWord::place maps boxes to user space
// The editable output draws real glyphs: plan the words, register the display faces into the
// plan (`pdfcraft_edit::add_type3_fonts`), then generate the layer. An unregistered plan falls
// back to WinAnsi, and characters no registered face has become '?'.
let plan = TextPlan::for_words(placed.iter().map(|w| w.text.as_str()));
add_type3_fonts(&mut doc, page, &mut plan)?;             // names the plan's fonts on the page
let layer = visible_text_layer(&placed, &plan);          // visible text: the editable output
// The searchable layer needs no display fonts: the engine's own layer (engine::ocr::layer)
// keeps every Unicode character with empty Type3 programs and a ToUnicode map beyond WinAnsi.
```

- **Engines behind one trait.** A [`Recognizer`] reports its `id`, whether it is `available`,
  the languages it reads, and reads `Vec<Line>` from an image. Two exist: the pure-Rust
  `OcrsRecognizer` (the `ocrs` crate, MIT/Apache-2.0, with its pre-trained models,
  CC-BY-SA-4.0, fetched by `cargo xtask models`; found via `$PDFCRAFT_MODELS` or `models/`
  beside the executable — see ATTRIBUTION.toml), and — off the web build —
  `tesseract::TesseractCli`, which drives an installed `tesseract` binary as an external
  process: never linked, bundled or downloaded. Implementations are shared across threads
  (`Send + Sync`); nothing spawns on wasm32.
- **The tesseract process is fenced in.** The program comes from a user-configured location or
  the PATH, never from document data (the engine validates a configured path as an absolute
  path to an existing executable file before it is used). The language code is allow-listed
  (`tesseract::language_code`, 12 codes) before anything reaches a process; the page image
  goes to a private temp file (a fresh 0700 directory holding a 0600 file on Unix) that is
  deleted afterwards; the run has a 120 s timeout, a 64 MiB output cap and a 50 000-word
  reading cap (an incomplete reading is rejected rather than applied); every failure comes back through `OcrError` with its cause named. A configured
  location that is invalid fails closed (it is never replaced by the PATH search). Availability
  is probed with `--list-langs` (its list is read from stdout, or from stderr where older
  versions print it); a failed probe is remembered for 30 s, so an install that appears later
  is found without a restart. Known limit: at the timeout or the output cap only the direct
  child is killed; a wrapper script's own children may outlive it.
- **Confidence drives review.** One classifier (`band_for`) puts a word in the High
  (≥ 90), Medium (70–89) or Low (< 70) band — the review overlay's colours, the legend and the
  counts; the batch flag rule (`low_confidence`, default threshold 60) is deliberately a
  separate threshold. The `ocrs` models report no confidence (its words carry `None`);
  a reading whose confidence is unknown is treated as unknown, never as good.
- **Two engines can be merged (ROVER).** Strategies: `PrimaryOnly` (the default), and two
  ensembles — `ConfidenceWeighted` (the secondary engine joins when the primary's mean
  confidence is under 70 or unknown; an empty primary keeps the fast path) and `RoverVote`
  (both engines always run). The merge lets every primary word adopt the best-overlapping
  unused secondary word at IoU ≥ 0.5; the higher confidence wins text, box and confidence (a
  tie keeps the primary; against a primary word of unknown confidence — ocrs — the secondary
  wins only at 70 or more, `UNKNOWN_CONFIDENCE_BAR`), the survivor is named `"ROVER"`, and
  unclaimed secondary words are
  appended under their engine's name. A merge is capped at 100 M word-pair comparisons (over
  it, the primary reading stands alone), and a failing partner degrades to the primary with a
  recorded note — a page is never skipped because a partner could not read it.
- **Preprocessing between the raster and recognition**, a pure function in fixed order: DPI
  normalization to 300 (only when one of the steps below is on, since they are calibrated for
  it; with all of them off the raster is read exactly as rendered, and a scan at or above 300
  is left alone), then the opt-in steps —
  orientation (90°/180°/270° from projection-profile banding), deskew (± 5°, shear-scored,
  below 0.1° is a no-op), denoise (3×3 median) and binarization (Sauvola, window 8, k 0.34).
  Every geometric step composes one exact affine `InverseTransform`, and callers map every
  word box back to the original raster before placing it. The heuristics are conservative and
  panic-free: too small to judge, blank, or no clear winner is a no-op, never a guess.
- **Searchable output preserves Unicode without display fonts.** The engine uses empty
  Type3 character programs and `/ToUnicode` maps for non-WinAnsi words, leaving the scan
  unchanged. WinAnsi words use `/PCHelv`; the `/OCR` layer is fitted to each word's box.
- **Editable output uses display glyphs.** `visible_text_layer` writes visible text whose
  size comes from the box height and whose horizontal scaling uses real font advances.
  Beyond WinAnsi, attributed craft-fonts faces supply outline Type3 glyphs with `/ToUnicode`.
  Uncovered characters become `?` and are reported in editable output notes; searchable
  output retains them. The engine refuses nonfinite geometry or oversized input before
  planning fonts: 4 MiB of text and 65,536 words, across all pages for editable export.
  Empty geometry is skipped; rotated words carry `across`/`up` vectors and retain direction.
- **Hostile input is rejected, not panicked at.** `OcrImage` holds private, validated state:
  non-zero sides, a 10 000-pixel side cap, a 64-megapixel area cap (checked arithmetic — the
  counts come from documents, so they are untrusted), and an exact buffer length.

## Where it is wired

The engine's `ocr` module renders pages into `OcrImage`s, runs jobs (`OcrJob::run`) and applies
the words: searchable-image mode draws the invisible layer over the page's own image and leaves
that image untouched; editable mode writes a NEW `<name>_ocr.pdf` of visible text (the scan
image is not carried over, the source is never overwritten). Skip probes keep pages — and
whole files — that already contain text out of the work unless Force OCR overrides them. A
searchable result applies as one undoable step; the Editable text output opens the OCR Verify
screen first, and Scan & OCR ▸ Correct recognized text reviews any recognition there before it
is applied (confidence-coloured boxes, corrections, a per-language user dictionary — a
plain-text file in the preferences folder, so words added from a document persist on disk
outside it until removed — Re-OCR of a region or a page, Accept/Reject), a
multiple-file run writes searchable copies as it goes, and the automation tools `ocr_recognize`,
`ocr_words`, `ocr_status` and `ocr_recognize_files` expose recognition headlessly.
`ocr_words` with `retain_review` returns a bounded session-local handle; `ocr_review_apply`
applies text corrections at the stored original boxes through its stale-document guard,
or discards the result. Empty corrections remove words. Searchable acceptance is one undo
step; editable acceptance writes a separate file. `ocr_dictionary` reads or atomically
updates the UI's per-language dictionary format through an explicitly root-confined path.

Not yet: languages beyond the tested set (the `ocrs` models read only unaccented Latin; the
Tesseract side is bounded by its allow-list and whatever its installed language packs cover),
OCR in the browser (the tesseract module is compiled out for wasm32), the deep scan-cleanup
options (descreen, background removal, edge-shadow removal), and editable glyph coverage
beyond the available attributed faces.

## Multilingual recognizer and scan correction

`cargo xtask models` downloads the hash-pinned, openly licensed models in
`ATTRIBUTION.toml`. No document is uploaded and no model is downloaded at runtime.
Release packages ship these files beside the application.

- The ocrs recognizer reads English; German, French, Spanish, Catalan, Italian, Dutch, Danish,
  Norwegian Bokmål, Swedish, Finnish, Portuguese, horizontal Japanese and Chinese
  (traditional/simplified) use PaddlePaddle's
  PP-OCRv6 small model through the existing Rust RTen runtime, over the lines ocrs detects,
  with a mean character confidence for the review overlay. Tesseract remains a separate engine.
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

The UI exposes independent page rotation, page straightening, paper-edge crop and searchable-text switches,
separate from the recognition-only preprocessing above. Automation:
`ocr_recognize` and `ocr_recognize_files` accept `language`, `rotate_pages`, `straighten_pages`
and `recognize`, plus `auto_crop` and `crop_margin`; `ocr_enhance` corrects scans without
adding text and enables cropping by default, including pages with existing OCR text.
Page rotation and straightening are on by default, in the Recognize Text dialog and
headless callers alike. Disable all three corrections for exact-image
output. Ordinary recognition leaves cropping off and skips existing text pages by
default. With `skip_text_pages: false`, recognition also reads pictures on pages
that already contain text. Words overlapping existing text are counted as
`already_on_page` and omitted; this comparison maps all four word-box corners
back through crop, rotation and deskew into the original page view. Per-page `crop_box` results are in original PDF user space, before corrections.
Corrections and OCR form one undo step; stale worker results do not modify the
document.

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
and final punctuation; each word's invisible text spans its own ink within the detected
line (not the padded crop, nor the whole line), as tight as the other engines' word boxes.
For the Latin languages, decoding excludes CJK, kana, full-width forms and letters
outside ASCII/Latin-1/Latin Extended-A, while preserving other European names,
digits and punctuation. Japanese and both Chinese choices retain the full dictionary. This is not a
language-specific spelling filter. Community French checks still found missed
`ï`, capital `Œ`/`Æ` and `Ë`, and inconsistent straight/curly apostrophes; the model
does not guarantee exact transcription. Search treats `'`, `’`, `‘`, `ʼ` and `＇` as one
character in both case modes, so either apostrophe finds the other.

In the multilingual recognizer (every language but English, which the ocrs model reads),
lines skewed by up to 20° whose baseline climbs more than a quarter of the line height
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

Validation: `cargo test -p pdfcraft-engine --test it scan_ocr` covers actual models,
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
request. It is a third engine beside ocrs and Tesseract: `historical::HistoricalRecognizer`
implements [`Recognizer`] with the ids `historical` (print) and `historical-handwriting`, and
reads alone (no ensemble). `historical::Historical::load` reads locally installed files;
`Ocr::recognize_historical` shares the ordinary print detector;
`Ocr::recognize_handwriting` finds regularly spaced, upright handwriting lines in
one text column using bounded ink projections. Both return source-image word boxes, with no
confidence; the page orientation comes from the ocrs models' classifier.
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
