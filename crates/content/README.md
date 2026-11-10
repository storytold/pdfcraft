# pdfcraft-content

Layer L2: content streams (ISO 32000-2 §7.8.2, §8, §9).

```rust
let parsed = parse(&stream.decoded()?);       // operators with operands and source spans
for op in &parsed.ops { if op.is("Tj") { /* … */ } }
let bytes = serialize_ops(&parsed.ops);       // write back (inline images kept whole)
let m = Matrix::from_operands(&op.operands);  // cm / Tm arithmetic, invert, bbox
```

The parser is tolerant (unreadable bytes are skipped and counted, never fatal) and never
panics. `redact` and (later) `edit` build their interpreters on it; the graphics-state tracker
and the `Patch` API of architecture §6 grow here.

An inline image ends where the renderer's tokenizer (`hayro-syntax`) ends it, so the parser
and the renderer read the same operators after the image, and redaction removes what the page
actually draws. Only where that tokenizer reads no image (its parameters don't read, or no `EI`
passes its checks) does the parser locate `EI` by its own rules: unfiltered images with built-in
or Indexed colour spaces, and image masks, by their sample layout (including per-row byte
padding), other images by the tolerant delimiter heuristic. Such an image is counted in
`Parsed::ambiguous_images`, because the operators after it may not be the ones the page draws.
So is an image that a reader which decodes the data (Poppler) may end at another `EI`. Such a
reader reads the data until it has the samples, or until the filter's end-of-data marker, and
ends the image at the first `EI` after that, whatever surrounds it; an image it can't decode it
ends at the first `EI` after `ID`. Where it stops is worked out without decompressing: exactly
with no filter or one ASCIIHex or ASCII85 filter and a layout this parser knows (a device colour
space, an Indexed array written in the image, or an image mask), otherwise from the marker, and
the image counts unless no `EI` comes before the renderer's. Without a filter, that misses a
reader that reads past the renderer's `EI` by a layout this parser doesn't know: a colour space
named as a resource (or a device name the page's resources redefine), which it can't look up,
or an array other than Indexed (CalRGB, Lab, Separation and the like). A layout taken from
parameters a reader refuses (a `/BPC` of `8.0`, an Indexed lookup too short) can put the stop
elsewhere too. Behind another filter (Flate, LZW, RunLength, CCITT, DCT) the renderer's end stands,
as it can't be told without decoding; behind one no reader decodes, the first `EI` after `ID` is
the end. An image that gives a key in both spellings (`/Filter` and `/F`, `/Width` and `/W` and the
rest of its layout), differently, counts: readers may take either.
