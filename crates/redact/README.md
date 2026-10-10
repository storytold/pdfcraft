# pdfcraft-redact

Layer L4 (may use `edit`): redaction, architecture §11.2.

```rust
// Marks are Redact annotations (pdfcraft-annot: Shape::Redact { quads, overlay }).
let marks: Vec<Mark> = marks(&doc);                 // page, areas (user space), fill, overlay text
let report = apply(&mut doc, None)?;                 // everything, or Some(&[pages])
clear_marks(&mut doc, None)?;                        // remove marks without applying
```

`apply` removes, under every marked area:

- **text**: glyphs are cut out of `Tj`/`TJ`/`'`/`"` and replaced by a `TJ` displacement of the
  same advance, so the rest of the line keeps its exact position. Widths come from `/Widths`,
  `/W`/`/DW` (composite fonts, embedded CMaps with codespace and CID ranges), Type 3 font
  matrices, and approximations for the standard 14. A glyph goes when it overlaps an area by
  more than a fifth of its size (or a point) both ways; zero-size text goes when its origin is
  inside;
- **images**: fully covered → removed; partly covered → a copy with the covered pixels cleared
  (8/16-bit and 1/2/4-bit images, image masks; Flate/LZW/RL/A85/AHx). Codecs PdfCraft can't
  re-encode (DCT, JPX, JBIG2, CCITT) are removed whole (fail-closed); inline images under an
  area are removed;
- **vectors**: covered paths are removed; partly covered paths and shadings are clipped so
  nothing paints inside the areas;
- **form XObjects**: rewritten recursively into new objects (pages sharing the original keep it);
- **annotations** whose rectangle overlaps (with their pop-ups), and **form fields** with a
  widget under a mark;
- the marks themselves, replaced by boxes in their fill colour (and overlay text) drawn into the
  page as a `/PCMark /Redaction` stream.

A verification pass re-reads each redacted page and fails the operation if any glyph or inline
image is still under an area. Content that can't be decoded makes the operation fail rather than
leave it unredacted.

## Remove hidden information / Sanitize

```rust
let found = sanitize::scan(&doc);                          // [(Hidden, count)] per category
sanitize::remove_hidden(&mut doc, &[Hidden::Comments])?;   // chosen categories
sanitize::sanitize(&mut doc)?;                             // all of them
```

Categories: metadata (`/Info`, XMP), file attachments, comments, form fields (flattened so the
values stay visible), hidden text (render modes 3 and 7, or wholly off the page), hidden layers
(content of off optional-content groups, then the groups), bookmarks, links/actions/JavaScript,
and `/PieceInfo` private data. Both make the next save a full rewrite.

`patterns` finds phone numbers, e-mail addresses, credit card numbers (Luhn-checked), US Social
Security numbers and dates for Search & Redact.

`codes` holds the redaction code sets (U.S. FOIA and U.S. Privacy Act exemptions):
`CodeSet::from_id("foia")?.overlay(&["(b)(6)"])` gives the overlay text for the picked codes.
Custom code sets aren't supported yet.

Not yet: pattern locales, struct-tree/alt-text cleanup, metadata and hidden-information
sanitising (M8.4), re-encoding of DCT images (they are removed instead).
