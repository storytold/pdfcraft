# pdfcraft-optimize

Layer L4: the PDF Optimizer (execution plan M11.1; Acrobat's Reduce File Size and Optimize PDF ▸
Advanced optimization).

```rust
let report = optimize(&mut doc, &Settings::default())?;   // Reduce File Size's choices
let report = optimize(&mut doc, &Preset::Mobile.settings())?; // smaller, for screens
// Or with progress: called before each image and the clean-up; `false` cancels.
let report = optimize_with_progress(&mut doc, &settings, &mut |stage| keep_going)?;
```

- **Presets:** Standard (Reduce File Size's choices: images above 225 ppi to 150 ppi, JPEG
  medium) and Mobile (above 144 ppi to 96 ppi, JPEG high), as in Acrobat's Settings menu.
- **Merge image fragments** (`fragments`, on by default): some PDF printers draw a page picture
  as thousands of images one pixel row high. Runs of upright 8-bit grey or RGB images in one
  colour space, sharing their left and right edges and stacked edge to edge (at least four,
  drawn with nothing else between them), become one lossless image at their finest resolution,
  which the image settings then resample; the strips are dropped by Clean Up. A real page
  printed this way went from 4.3 MB to 231 KB (Standard) and 161 KB (Mobile). Not merged yet:
  side-by-side tiles, rotated strips, masks. Content arrays use the shared space boundary
  policy, so trailing comments, split operands and graphics state keep their meaning after merging.
- **Images:** the effective resolution of each image is measured where pages draw it (the CTM
  through form XObjects, up to 12 deep; the smallest over all uses). If a use can't be
  measured (forms nested deeper, or drawn too many times), no image is resampled. Colour and grayscale images above a
  threshold are resampled with a bicubic (Catmull-Rom) filter to a target resolution and
  stored as JPEG (Acrobat's quality steps) or ZIP; soft masks are resampled with their image.
  An image is replaced only if the result is smaller. Left alone: CMYK and other colour spaces,
  image masks, decode arrays, more than 8 bits, JPEG 2000, JBIG2 and CCITT. Samples are read with
  `/DecodeParms` given by reference too (`Document::with_direct_parms`), here and below: without
  them a predictor's tag bytes would shift every row of an image written back.
- **Monochrome images** (`Settings::mono`, CCITT Group 4 by default, or JBIG2): a 1-bit image
  with one colour component (grey, indexed, a separation) or an image mask, stored with no filter
  or with general-purpose ones, is stored again as CCITT Group 4 (`pdfcraft_filters::encode_g4`)
  or lossless JBIG2 (`encode_jbig2_generic`; a document below PDF 1.4 gets `/Version /1.4` in
  its catalog) when that is smaller as stored. On 300 dpi text JBIG2 is about a third smaller
  than Group 4. It is lossless: the samples decode to the same bits, so the colour
  space, decode array and masks stay. Group 4 wins on scans, whose ragged edges defeat Flate;
  on clean computer-drawn 1-bit art Flate can be smaller, and the image then stays. Not yet:
  JBIG2 symbol coding (lossy or lossless), monochrome downsampling, inline images, soft masks and `/Mask` images, and rows wider
  than 32,767 samples.
- **Discard objects:** thumbnails, alternate images, document tags, print settings.
- **Clean up:** Flate for streams with no filter; for streams with an outer ASCII85 or ASCIIHex
  layer, that layer is removed when the result is smaller (the inner filters keep their encoded
  bytes). Streams with `/DecodeParms`, images and object streams are left alone.

The engine runs Remove Hidden Information (`pdfcraft-redact`) for the user-data categories,
then this, then merges identical objects and writes a full, compressed save
(`pdfcraft_engine::optimizer::OptimizeJob`, which the UI runs on a worker thread with a
progress bar). JPEG decoding and
encoding and resampling come from the `image` crate.
