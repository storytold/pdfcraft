# pdfcraft-create

Layer L4. Create a PDF (Acrobat's Create a PDF tool), execution plan M10.2:

- `blank(width, height, pages)`: an empty document;
- `from_images(&[(name, bytes)])`: one page per image, sized from the image's resolution
  (PNG `pHYs`, JPEG JFIF density; 72 dpi when absent). JPEG data is embedded as is
  (`/DCTDecode`, grey, RGB or Adobe-inverted CMYK); PNG is decoded and stored with Flate,
  with transparency as a soft mask;
- `from_text(text, …)`: plain text set in Helvetica, wrapped and paginated.

Every function returns a `pdfcraft_cos::Document`; the caller writes it.
