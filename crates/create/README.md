# pdfcraft-create

Layer L4. Create a PDF (Acrobat's Create a PDF tool), execution plan M10.2:

- `blank(width, height, pages)`: an empty document;
- `from_images(&[(name, bytes)])`: one page per image, sized from the image's resolution
  (PNG `pHYs`, JPEG JFIF density; 72 dpi when absent). JPEG data is embedded as is
  (`/DCTDecode`, grey, RGB or Adobe-inverted CMYK); PNG is decoded and stored with Flate,
  with transparency as a soft mask. An ICC profile in the file (JPEG `APP2 ICC_PROFILE`, PNG
  `iCCP`) that matches the image's colour space is kept as an `/ICCBased` colour space with
  the device space as `/Alternate`; images with the same profile share one profile object.
  JPEG metadata scanning skips marker fill bytes and reads application segments after the
  frame header, stopping at the first scan or end marker. Broken or mismatched ICC profiles
  still fall back to the device colour space;
- `from_images_with_resolution(images, ImageResolution::Dpi(dpi))`: override the page
  resolution (1–1200 dpi), without resampling or recompressing image pixels. Use 72 dpi
  for one point per pixel, or `ImageResolution::Embedded` for the same behavior as
  `from_images`. The largest page side is still capped at 14,400 points;
- `from_text(text, …)`: plain text set in Helvetica, wrapped and paginated. Text Helvetica
  can't draw is refused, naming the line and the character, never set as `?`.
- `from_text_escaped(text, …)`: the same for reports PdfCraft generates from documents
  (comment summaries, compare reports): such characters are written as `[U+XXXX]` and the first
  line says so (`ESCAPED_NOTE`).
- `from_office(name, bytes)`: a Word (`.docx`), PowerPoint (`.pptx`) or Excel (`.xlsx`) file, read
  natively from its Office Open XML package (`office/`): Word flows onto its sections' pages with
  styles, lists, tables, images, headers and footers; PowerPoint gives one page per visible slide
  with master and layout inheritance; Excel prints each visible sheet's used range with its number
  formats. Text is set in the standard-14 faces chosen from each run's font family, with Turkish,
  Central European, Baltic and Romanian letters drawn through a second encoding of the same face
  (`office/latin.rs`); characters they can't draw are written as `[U+XXXX]`. Returns the document and an `OfficeReport` counting
  what was approximated (charts and vector images become outlined boxes). See
  [Word, PowerPoint and Excel to PDF](../../docs/office-import.md).

`source_kind` recognises Office packages (and encrypted ones, which `from_office` refuses with a
reason) by name and signature as `SourceKind::Office`.

Every function returns a `pdfcraft_cos::Document`; the caller writes it. `from_office` also returns its
`OfficeReport`, which the caller shows: it is the only record of what the conversion approximated.

Image export and Save Image As apply each lossless image's Decode pair before converting its color components.
Indexed Decode values select palette entries after rounding and clipping. Color-key masks compare the original samples.
Missing, null or malformed Decode arrays keep the default mapping.
The existing 16-bit path still uses each sample's high byte. JPEG and mask Decode handling remain limited.
ICC transforms and calibrated color conversion remain outside this export path.
