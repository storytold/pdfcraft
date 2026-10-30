# pdfcraft-create

Layer L4. Create a PDF (Acrobat's Create a PDF tool), execution plan M10.2:

- `blank(width, height, pages)`: an empty document;
- `from_images(&[(name, bytes)])`: one page per image, sized from the image's resolution
  (PNG `pHYs`, JPEG JFIF density; 72 dpi when absent). JPEG data is embedded as is
  (`/DCTDecode`, grey, RGB or Adobe-inverted CMYK); PNG is decoded and stored with Flate,
  with transparency as a soft mask. A picture of only pure black and white (a 1-bit PNG or TIFF,
  eight-bit grey, a two-colour palette, RGB without a profile) is stored at one bit per pixel, as
  CCITT Group 4 or Flate, whichever is smaller as stored (`pdfcraft_filters::encode_g4`). An ICC profile in the file (JPEG `APP2 ICC_PROFILE`, PNG
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

Every function returns a `pdfcraft_cos::Document`; the caller writes it.

Image export and Save Image As apply each lossless image's Decode pair before converting its color components.
JBIG2 images and masks (JBIG2 as their only filter) are decoded with `hayro-jbig2`, their `/JBIG2Globals` included,
as `JBIG2Decode` gives them: black as 0. An image whose JBIG2 page has another width than the image, or more
pixels than an exported image may have, or whose globals decode to more than 32 MiB (they are decoded before the
page's size is known, so they have a limit of their own), is skipped; such a mask is left out and the image
exported opaque, as JPX and CCITT masks are.
Indexed Decode values select palette entries after rounding and clipping. Color-key masks compare the original samples.
Missing, null or malformed Decode arrays keep the default mapping.
Soft-mask Decode intervals map samples to opacity before the mask is resized, including reversed ranges and indirect entries.
The existing 16-bit path still uses each sample's high byte.
JPEG image Decode handling remains limited.
ICC transforms and calibrated color conversion remain outside this export path.
CCITT fax images (Group 3 and 4) are decoded for export through `pdfcraft_cos::Document::decoded_image` with their
`/DecodeParms` (`/Columns` has to be the image's width; up to `/Height` rows, whatever `/Rows` says), behind other filters such as
`ASCII85Decode` too. Byte-aligned Group 3 rows are read by their end-of-line codes, whether the fill starts each code on a byte or
ends it on one, when the data starts with one (T.4 sends one before the first row), and on bytes otherwise, whatever `/EndOfLine`
says; data that comes up short is reported. Data with codes but none before the first row is read on bytes, and is reported or,
at `/K 0`, may come out wrong. JPEG 2000 and JBIG2 images are still reported, not exported.
