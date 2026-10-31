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
  reading cap; every failure comes back through `OcrError` with its cause named. A configured
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
