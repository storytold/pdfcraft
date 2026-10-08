# PdfCraft showcase PDF

`showcase.html` is the source of a 13-page, US Letter test document for
viewing and rendering. The PDF is built on your machine and is not committed:

```sh
cargo xtask demo-pdf            # writes dist/demo/pdfcraft-showcase.pdf
cargo xtask demo-pdf --chrome /path/to/chrome --out /tmp/showcase.pdf
```

You need Google Chrome, Chromium or Edge. The tool looks for it in the usual
install locations and on `PATH`, or uses `--chrome` or the `PDFCRAFT_CHROME`
environment variable if you set one.

## How it is built

1. **Raster.** The xtask renders a Mandelbrot image (Seahorse Valley) and
   encodes it as a PNG. It then inlines the PNG into the HTML as a `data:` URI
   and points `{{ASSETS}}` at this repository's `assets/` directory (for the
   Inter and JetBrains Mono fonts).
2. **Chrome.** Headless Chrome prints the page to PDF with
   `--headless=new --no-pdf-header-footer --generate-pdf-document-outline --print-to-pdf`.
   Skia produces 12 tagged pages with embedded font subsets. Variable
   system-ui weights come out as Type 3 fonts and emoji as Type 3 bitmap
   glyphs. The pages also get shadings, blend modes, soft masks, tiling
   patterns, links, named destinations and bookmarks built from the headings.
3. **Post-processing with `lopdf`** (`xtask/src/demo_pdf/`) adds what a
   browser cannot emit:
   - **Annotations.** The HTML contains marker links to
     `https://mark.pdfcraft.invalid/<kind>/<id>`. Chrome turns each one into
     a Link annotation whose `/Rect` follows the layout. The xtask replaces
     each marker with a real annotation: Highlight, Underline, StrikeOut,
     Squiggly, Caret, Text with Popup and a threaded reply and review state,
     FreeText with `/DA` and `/RC`, Line with arrow, Square, Circle, Polygon,
     PolyLine, Ink, a custom "APPROVED" Stamp and a FileAttachment. Every
     annotation has its own `/AP` appearance stream, plus `/T`, `/M`, `/NM`
     and `/Contents`.
   - **An appended AcroForm page**, drawn directly with the standard 14 fonts:
     - text fields, including date and amount fields with
       `AFDate_*`/`AFNumber_*` format actions, and a multiline field
     - checkboxes, a radio group, a combo box and a multi-select list box
     - a push button whose JavaScript action calls `app.alert`
     - an unsigned signature field

     Every widget has `/AP` appearance streams, so `NeedAppearances` is false.
   - **Layers.** "Draft watermark" (on) puts a diagonal DRAFT on the review
     page. "Print-only notes" is off on screen and on when printed, with
     `/AS` usage events.
   - **Page labels.** The pages are labelled Cover, i, ii, then 1 to 10.
   - **Bookmarks.** Chrome's heading outline is kept, with its titles tidied,
     and an entry is added for the form page.
   - **Metadata and attachments.** The Info dictionary and an XMP metadata
     stream carry the same values. `/ViewerPreferences /DisplayDocTitle` and
     `/PageMode /UseOutlines` are set. `showcase-data.csv` is attached in
     `/EmbeddedFiles` and `/AF`, and `review-notes.txt` is attached through
     the file-attachment annotation.

Chrome embeds subsets of **macOS system fonts** (Hoefler Text, Didot, Avenir
Next, Geeza Pro, Kohinoor Devanagari, Hiragino, Apple Color Emoji and others),
so the output depends on the host and we must not redistribute it. This is why
`dist/` is gitignored. On Linux or Windows the CSS falls back to other fonts
and the layout will differ. The xtask refuses to continue if Chrome's page
count is not 12, because the post-processing uses fixed page indices.

## Editing the showcase

- Each `<section class="page">` is exactly one Letter page with
  `overflow: hidden`. Keep every section's content inside its page.
- The page order is fixed in `xtask/src/demo_pdf/postprocess.rs`
  (`PAGE_CONTENTS`, `PAGE_SETTING_TEXT`, `PAGE_REVIEW`).
- Marker links use `white-space: nowrap`, so each one produces a single
  rectangle.
- All prose is original. The chart values in `showcase-data.csv` are
  illustrative only.

To check the result, look at the output of `pdfinfo`, `pdffonts` and
`qpdf --check`, and render the pages with
`pdftoppm -r 60 -png dist/demo/pdfcraft-showcase.pdf /tmp/showcase`.
