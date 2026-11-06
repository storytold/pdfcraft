# Optional historical OCR

Historical recognition is a separate text style in **Scan and text recognition**.
It uses an optional model for historical print, including Fraktur, and handwriting,
including German Kurrent. The standard installation and the ordinary OCR language
choices continue to use the modern recognizer.

Select **Historical print (Fraktur)** or **Historical handwriting (Kurrent)**,
then explicitly download the shared add-on. Handwriting requires upright,
horizontal lines in **one text column**; columns, tables and marginal notes are
not supported by this profile. The print profile retains the ordinary layout detector.
The model and character dictionary total about 13 MB. Once installed, recognition
works offline; document images and text are never sent to the model host. Opening
the dialog, selecting the text style, checking status and running OCR do not download
anything. The add-on can be removed separately from documents and the application.

CLI/MCP use `ocr_addon_status`, `ocr_addon_install` and `ocr_addon_remove` for the
same model management. `ocr_recognize`, `ocr_words` and `ocr_recognize_files` accept
`"recognizer": "historical"` for print or `"historical-handwriting"` for a single
column of handwriting; their default is `"modern"`. A historical recognizer reads alone,
in place of the modern engine choice (ocrs, Tesseract or Automatic), which stays
available for modern print. Model-management paths
obey the automation root. Historical recognition with a missing or corrupt add-on
reports the problem and leaves the document unchanged.

Recognition uses the existing page rendering, scan correction and
invisible Unicode text layer. Historical spelling, long-s and abbreviations are
preserved; canonical Unicode composition improves searching without modernizing
the transcript. Corrections and text remain one undoable document edit. Word boxes
are estimated from CTC positions, as with the modern model.

## Model and limitations

The weights are Benjamin Kiessling/INRIA's historical PP-OCRv6 small model,
[Zenodo record 21788405](https://zenodo.org/records/21788405), licensed Apache-2.0.
This is a different trained model from the modern PaddlePaddle recognizer. Its
published safetensors metadata specifies 96-pixel RGB lines and 1,623 CTC classes;
inference follows that metadata. Conversion to ONNX changes the container and graph
representation, not the trained weights. The application runs it through RTen in
Rust, with no Python, Tesseract, cloud service or native inference sidecar.
The downloadable artifact is the pinned Linux CI export. Windows export differs
only in floating-point rounding of folded Conv weights (maximum absolute delta
9.54e-7); its graph and metadata are identical and all 62 reference transcripts
match byte for byte. Conversion sources accompany the model release.

The model was trained on printed and handwritten historical material, including
Fraktur and Kurrent sources. That is not a guarantee for every historical typeface,
writer, language or layout. Abbreviations, faint characters, overlapping lines,
ornate initials, very long lines and complex page layouts need human review.
There is no automatic paleographic transcription correction or modern-spelling
conversion. Handwriting uses a bounded ink-projection segmenter that estimates
regular baseline spacing; it does not infer arbitrary manuscript layouts. Use the
print profile for printed documents. Pages without reliable handwriting line spacing
are reported as skipped, rather than adding guessed text. Scan frames and rules
are removed only from segmentation analysis, never from the visible page.
Border suppression is heuristic: an exceptionally long connected stroke at the
page edge can resemble a rule. Ordinary words touching the edge are retained and
covered by a synthetic regression.
The historical model detects its trained scripts automatically: the modern
document-language selection is retained but is not used by this model. Japanese
is not part of the historical model; use modern recognition for Japanese print.
The source model also lists Arabic, Armenian, Cyrillic, Ethiopic, Georgian, Greek,
Hebrew, Malayalam and Syriac training material. Those scripts, Schwabacher and
Sütterlin have not been independently checked here and are not advertised as
validated recognition profiles. Printed Fraktur and handwritten Kurrent are
distinct cases, even though the same optional weights serve both.

## Verification

- The Rust decoder rejects malformed shapes and non-finite probabilities, handles
  repeated characters separated by CTC blanks, preserves historical characters,
  composes Unicode and excludes padding-only words.
- The historical engine regression checks the real model through rendering,
  invisible-text insertion, save/reopen, unchanged image pixels and undo, including
  a complete Kurrent manuscript page rather than only supplied line crops.
- UI tests check that choosing the historical text style keeps document-language
  selection and requires explicit installation before recognition can start.
- Model tests skip explicitly if the optional files are not installed. Source
  checkouts need the ordinary OCR models as well as the optional add-on.

Local conversion verification compared 62 real line images: 30 Fraktur lines from
[`ubtue/gt-fraktur`](https://github.com/ubtue/gt-fraktur/tree/479511242648b5438e346c599ef872d39842788c)
and 32 Kurrent lines from
[`bertsky/mkn-kurrent-gt`](https://github.com/bertsky/mkn-kurrent-gt/tree/8ddc7276f24504cfccde655aa901f08fea2fb7eb).
PyTorch and Rust/RTen produced identical character sequences before NFC composition
on all 62 lines. With NFC and whitespace normalization, without modernizing long-s
or historical spelling, the line-level character error counts were 14/1,769 for
Fraktur and 45/858 for Kurrent. These are small regression samples from corpora
named in the model's training sources, **not an independent accuracy benchmark**.
The historical documents and line images are not bundled with the application.

The complete original pages were also imported as images and recognized through
the actual PDF rendering/text-layer pipeline at 300 dpi, without rotation, deskew
or crop. The Fraktur page had 160/1,798 character edits with modern recognition
and 9/1,798 with the historical print profile. The Kurrent page had 572/889 edits
with modern recognition, 476/889 when the historical model used the print detector,
and 53/889 with the single-column handwriting profile. Page-level counts include
the spaces between lines; they are not directly interchangeable with isolated-line
counts. The full-page engine test additionally checks representative passages,
save/reopen, pixel identity and undo at 150 dpi. The same training-overlap caveat
applies to every number: these are small integration regressions, not an accuracy
promise for other manuscripts.

To repeat the external fixture tests, set `PDFCRAFT_HISTORICAL_FIXTURES` to a
directory named `lines` containing `drey1834_0031_line02.png` and
`kurrent-00000024_line28.png`. The line rectangles in the PAGE XML are expanded
by five pixels and clipped to the scan bounds. Place `kurrent-00000024.jpg` beside
the `lines` directory for the complete-page test. The pinned Fraktur PAGE file is
`drey1834/drey1834/page/drey1834_0031.xml` (CC0); the pinned Kurrent PAGE file is
`data/GN_1765_1_GN_A_109/GT-PAGE/00000024.xml` (CC-BY-SA-4.0 transcription,
public-domain source scan). Follow those repositories' image references; their
institutional banners are not product assets. Install the optional model before
running `cargo test -p pdfcraft-engine --test scan_ocr historical`.
