# Recognizing scanned text while editing

Open **Edit a PDF → Edit text & images**. When the current page is predominantly
an image and has no text layer, PdfCraft offers a small recognition prompt:

- **Current page** recognizes only the page in view.
- **All pages** recognizes the document, leaving pages that already have text alone.
- **Cancel** leaves the document unchanged and dismisses the prompt for that page.

**Recognize scanned text automatically**, in the Edit panel and the prompt, controls
these automatic offers. It is on by default and remembered across restarts. Turning
it off does not remove text already recognized. Turning it back on allows dismissed
pages to be offered again. Viewing a file in Read mode does not trigger recognition.

Recognition runs locally in the existing background worker, using the language and
resolution selected under **Scan & OCR → Recognize text**. The Windows preview
includes the model files. Progress can be cancelled with the existing recognition
control; completed pages are retained. The result is one undoable **Recognize text**
step. Undo does not immediately offer recognition again. Save to retain the text layer.

Recognized text can be searched, selected and copied. Clicking a recognized phrase in
Edit text & images uses PdfCraft's existing scan-text editor. The scan image stays;
applying a text edit covers the original lettering with a sampled flat paper colour.
This does not reconstruct the original font or separate photographs from lettering.

The automatic prompt detects an image covering at least half the page. Vector-only
outlined text, pages that already have a text layer, and small pictures are not
automatically offered; use **Scan & OCR → Recognize text** explicitly when needed.
The bundled recognizer currently handles English/Latin text without accents. OCR can
misread text, especially handwriting, tables, poor scans and patterned backgrounds.
Review the result. Read-only documents are not changed, and results captured before
an intervening edit, undo or reload are discarded.

Automation uses the existing `ocr_status` and `ocr_recognize` tools. Supply `pages: [1]`
for one page or omit `pages` for the document. The UI control channel exposes the
remembered toggle as `set auto_ocr on|off` and reports `auto_ocr` in `state`.

The detection approach builds on [PR #1609](https://github.com/storytold/pdfcraft/pull/1609)
by @philip-tam. The stale-result safeguard is integrated from
[PR #1876](https://github.com/storytold/pdfcraft/pull/1876) by @MarekSpodBiedry,
with its original commit authorship retained.
