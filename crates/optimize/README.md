# pdfcraft-optimize

Layer L4: the PDF Optimizer (execution plan M11.1; Acrobat's Reduce File Size and Optimize PDF ▸
Advanced optimization).

```rust
let report = optimize(&mut doc, &Settings::default())?;   // Reduce File Size's choices
let report = optimize(&mut doc, &Preset::Mobile.settings())?; // smaller, for screens
let report = optimize(&mut doc, &Preset::Presentation.settings())?; // slide decks
// Or with progress: called before each slide, each image and the clean-up; `false` cancels.
let report = optimize_with_progress(&mut doc, &settings, &mut |stage| keep_going)?;
```

- **Presets:** Standard (Reduce File Size's choices: images above 225 ppi to 150 ppi, JPEG
  medium), Mobile (above 144 ppi to 96 ppi, JPEG high) and Presentation (flatten tiled slides,
  no image longer than 1920 px, JPEG 2000 and palette images decoded, JPEG 70), as in Acrobat's
  Settings menu; the Presentation preset is PdfCraft's own. In Standard and Mobile, black-and-white
  images are averaged down to 300 ppi above 450 ppi, and stored as CCITT Group 4 whenever that is smaller.
- **Flatten tiled slides** (`flatten`, off unless the Presentation preset is chosen): a page that
  draws many small clipped image tiles — a slide exported from InDesign, Illustrator, Canva or
  Keynote, whose transparency was flattened into a tile grid — becomes one picture. The page is
  rendered without its text at `max_dimension` pixels into a JPEG, its text objects are drawn over
  it again (still text: searchable, selectable, sharp at any zoom), and the tiles are detached.
  Page `/XObject`s that Type 3 glyph procedures draw are kept, as is every other page resource.
  Left as it is: rotated and `/UserUnit` pages, pages with markup annotations, pages whose form
  XObjects hold text, unreadable content, encrypted documents, and pages where the picture would
  not be smaller. Known limits: text is drawn last, and a clip that clipping text (`Tr` 4–7) put
  on later text is not repeated in the overlay.
- **Merge image fragments** (`fragments`, on by default): some PDF printers draw a page picture
  as thousands of images one pixel row high. Runs of upright 8-bit grey or RGB images in one
  colour space, sharing their left and right edges and stacked edge to edge (at least four,
  drawn with nothing else between them), become one lossless image at their finest resolution,
  which the image settings then resample; the strips are dropped by Clean Up. A real page
  printed this way went from 4.3 MB to 231 KB (Standard) and 161 KB (Mobile). Not merged yet:
  side-by-side tiles, rotated strips, masks. Content arrays use the shared space boundary
  policy, so trailing comments, split operands and graphics state keep their meaning after merging.
- **Images:** the effective resolution of each image is measured where pages draw it (the CTM
  through form XObjects, up to 12 deep; the smallest over all uses).
  Page-local `/UserUnit` scaling is included, so large-format pages are measured in physical inches.
  `max_image_dimension` caps an image's longer side in pixels whatever size the page draws it at,
  which is how a slide defined as 3840 points stops carrying 8K pictures that measure 144 ppi.
  If a use can't be
  measured (forms nested deeper, or drawn too many times), no image is resampled. Colour and grayscale images above a
  threshold are resampled with a bicubic (Catmull-Rom) filter to a target resolution and
  stored as JPEG (Acrobat's quality steps) or ZIP; soft masks are resampled with their image.
  An image is replaced only if the result is smaller. With `extended_formats` on (the
  Presentation preset), JPEG 2000 (`/JPXDecode`, through `hayro-jpeg2000`) and palette
  (`/Indexed`) images are decoded too — a palette to its base colour space — and stored like any
  other image. Left alone by these colour and grey settings: CMYK and other colour spaces,
  image masks, decode arrays, more than 8 bits, JBIG2 and CCITT (black-and-white images, CCITT
  among them, are below). A JPEG is read with its colour transform (an Adobe marker's, else
  `/ColorTransform`, ISO 32000-2 §7.4.8), so one stored untransformed keeps its colours; one
  whose marker and `/ColorTransform` disagree is left alone, since readers differ on which wins.
  Samples are read with `/DecodeParms` given by reference too, the numbers inside included
  (`Document::with_direct_parms`), here and below: without them a predictor's tag bytes would shift
  every row of an image written back.
- **Target file size** (`target`): with `Settings::target_mb` set, `optimize_to_target` bisects an
  18-rung ladder of (longest side, JPEG quality) and returns the sharpest rung whose saved file
  fits, or the smallest rung when none does. Each rung is a whole optimization of a copy, so a
  search costs at most five of them; a document whose own save already fits is copied unchanged.
- **Monochrome images** (`Settings::mono`, `MonoSettings::REDUCE` by default: to 300 ppi above
  450 ppi, CCITT Group 4). Black-and-white images are a 1-bit image with one colour component
  (grey, indexed, a separation) or an image mask, stored with no filter, with general-purpose ones
  or with CCITT fax after any of those (decoded by `Document::decoded_image_within`, no further
  than its rows can need; byte-aligned Group 3 with end-of-line codes but none before the first
  row, not as T.4 sends them, is read as PdfCraft draws it, which at `/K 0` may be other rows than
  were coded, and an image stored again keeps them). Packed to one bit first, unless a colour-key `/Mask` is in the image's
  own values: eight-bit grey of only 0 and 255; eight-bit `DeviceRGB` of only black and white,
  which becomes `DeviceGray` (not calibrated RGB or RGB with a profile, nor one drawn or listed
  where `/DefaultGray` or `/DefaultRGB` is in force, which would then draw it otherwise, nor one
  referred to from anywhere but the page and form resources the optimizer reads, where they may
  be: an annotation's appearance or a pattern drawing it, directly or through resources or a form
  it shares with a page, or another image's `/Alternates`; nor one whose soft mask has a
  `/Matte`); and an indexed image of 2, 4 or 8 bits that uses only palette entries
  0 and 1, which keeps its palette, whatever its two colours. A decode array stays when it means
  the same at one bit (grey's, or RGB's if the same for each component, as grey's), and an indexed
  image's default one goes, as the default at one bit; otherwise the image is left alone. The grey
  or colour settings take a packed image instead when they make it smaller still, its soft mask
  counted (resampled with it there, kept as it is here), as they can a dithered scan by
  downsampling it to JPEG.
  - **Downsampling:** one whose effective resolution is above `above_ppi` is averaged down to
    `target_ppi`: each new pixel takes the sample covering more of its area, and the dark one on a
    tie, so a dark line half a new pixel wide stays: black in grey and the painted part of an image
    mask (a 0, or a 1 with an inverting decode array), full ink in a separation (a 1, or a 0), and
    entry 0 of an indexed image, whatever colour its palette gives it (the palette isn't read yet).
    Not one with a soft mask, whose own size would have to follow; not a halftone or a dither,
    most of whose new pixels would cover both black and white, so that taking the sample covering
    more would turn its tones solid (a 50% screen black, a 25% one white); and not one wider than
    a fax row (`pdfcraft_filters::MAX_CCITT_COLUMNS`), as the averaging keeps a counter for each
    column. On a 600 ppi text page, Group 4 at 300 ppi is half the size.
  - **Compression:** CCITT Group 4 (`pdfcraft_filters::encode_g4`), or 1-bit Flate when that is
    smaller, which only a resampled image or one packed from more bits may be: a 1-bit image that
    is only stored again becomes Group 4 or stays. Group 4 alone is only decoded to be resampled;
    Group 3, or Group 4 inside another filter such as `ASCII85Decode`, is stored again as Group 4
    alone when that is smaller. Damaged Group 3 with end-of-line codes is stored as the decoder
    recovers it, a bad row picked up again at the next code, as it reports no damage there; without
    them, damage stops the decoding and the image is left alone. With Retain, a resampled image
    keeps its kind of compression (CCITT, or Flate) and nothing else changes; an image of more bits
    is then left to the grey or colour settings. Every change is kept only when it is smaller as
    stored.
  - Apart from the downsampling it is lossless: the samples keep their meaning, so the colour
    space, decode array and `/Mask` stay, but for packed RGB, which becomes grey with one decode
    pair. Group 4 wins on scans, whose ragged edges defeat Flate; on clean computer-drawn 1-bit
    art Flate can be smaller. Not yet: inline images, an image's own `/SMask` or `/Mask`
    image (only images drawn on a page are visited), and Group 4 for rows wider than 32,767
    samples.

  - Lossless JBIG2 uses bounded generic-region encoding or exact-copy symbol dictionaries when
    smaller. No lossy symbol substitution is used. Export reads embedded images and bounded
    shared globals.
- **Discard objects:** thumbnails, alternate images, document tags, print settings.
- **Clean up:** Flate for streams with no filter; for streams with an outer ASCII85 or ASCIIHex
  layer, that layer is removed when the result is smaller (the inner filters keep their encoded
  bytes; streams with `/DecodeParms`, images and object streams keep their ASCII layer. LZW-compressed
  streams, content and images alike, are decoded and stored with Flate instead (Acrobat's "In
  streams that use LZW encoding, use Flate instead"): a predictor carries over to Flate without
  `EarlyChange`, filters applied before the LZW one go with it and those after it keep their
  bytes. A stream whose LZW data doesn't decode cleanly is left as it is. Not yet: inline images.

The engine runs Remove Hidden Information (`pdfcraft-redact`) for the user-data categories,
then this, then merges identical objects and writes a full, compressed save
(`pdfcraft_engine::optimizer::OptimizeJob`, which the UI runs on a worker thread with a
progress bar). JPEG decoding comes from `zune-jpeg` (the decoder behind `image` and the
renderer), JPEG encoding and resampling from the `image` crate. Flattening renders pages through
`pdfcraft-render` (the only way to composite clipped, layered tiles exactly as they are shown),
and JPEG 2000 decoding from `hayro-jpeg2000`, the decoder that renderer already uses.

## Target PDF version: partial compatibility

The target selects the header version, clears the catalog `/Version`, controls object
streams (disabled for PDF 1.4), and checks encryption requirements. It does **not**
detect or convert content newer than the target: optional content, JPEG 2000 images,
16-bit images, OpenType fonts and PDF 2.0 features can remain. A PDF 1.4 header
therefore does not guarantee compatibility with PDF 1.4 readers or validators.
`optimize.compatibility-level` remains partial until content compatibility is checked.
