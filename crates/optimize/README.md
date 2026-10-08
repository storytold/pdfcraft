# pdfcraft-optimize

Layer L4: the PDF Optimizer (execution plan M11.1; Acrobat's Reduce File Size and Optimize PDF ▸
Advanced optimization).

```rust
let report = optimize(&mut doc, &Settings::default())?;   // Reduce File Size's choices
```

- **Images:** the effective resolution of each image is measured where pages draw it (the CTM
  through form XObjects; the smallest over all uses). Colour and grayscale images above a
  threshold are resampled with a bicubic (Catmull-Rom) filter to a target resolution and
  stored as JPEG (Acrobat's quality steps) or ZIP; soft masks are resampled with their image.
  An image is replaced only if the result is smaller. Left alone: CMYK and other colour spaces,
  image masks, decode arrays, more than 8 bits, JPEG 2000, JBIG2 and CCITT.
- **Discard objects:** thumbnails, alternate images, document tags, print settings.
- **Clean up:** Flate for streams with no filter.

The engine runs Remove Hidden Information (`pdfcraft-redact`) for the user-data categories,
then this, then merges identical objects and writes a full, compressed save. JPEG decoding and
encoding and resampling come from the `image` crate.
