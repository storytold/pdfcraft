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

Every function returns a `pdfcraft_cos::Document`; the caller writes it.

PNG pHYs, JPEG JFIF and TIFF ResolutionUnit=1 densities without a physical unit preserve the declared pixel aspect ratio.
TIFF uses the paired positive XResolution/YResolution rationals from each directory, including fractions below one.
A missing, zero or malformed unitless TIFF pair keeps square pixels at 72 dpi.
Their denser axis uses a 72-dpi reference and the other axis expands, so proportional density values produce identical natural page sizes.
Missing or zero densities keep the square-pixel 72-dpi fallback; absolute meter, inch and centimeter densities keep their existing interpretation.
An explicit `ImageResolution::Dpi` still overrides both axes, including any embedded aspect ratio.
Pixels are not resampled for aspect correction, and ordinary JPEG data stays byte-for-byte.
Existing EXIF correction can re-encode a JPEG; it now retains valid unitless JFIF density and swaps the density axes on quarter turns.
Its existing color-profile limitations remain a separate contribution (#4669).
Page placement uses the same PDF real precision as the page box, including positive dimensions below 0.001 point after the existing page-side cap.
Natural Add image fitting also keeps positive sub-point boxes, with precise clipping and placement transforms.

Image export and Save Image As apply each lossless image's Decode pair before converting its color components.
Indexed Decode values select palette entries after rounding and clipping. Color-key masks compare the original samples.
Missing, null or malformed Decode arrays keep the default mapping.
Soft-mask Decode intervals map samples to opacity before the mask is resized, including reversed ranges and indirect entries.
The existing 16-bit path still uses each sample's high byte.
JPEG image Decode handling remains limited.
ICC transforms and calibrated color conversion remain outside this export path.
