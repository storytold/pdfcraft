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
  area are removed. Images with separate `/SMask` or `/Mask` data are removed whole;
  cleared copies drop `/Alternates` so alternate pixels cannot survive;
- **vectors**: covered paths are removed; partly covered paths and shadings are clipped so
  nothing paints inside the areas;
- **form XObjects**: rewritten recursively into new objects (pages sharing the original keep it);
- **annotations** whose rectangle overlaps (with their pop-ups and their replies, `/IRT`), and **form fields** with a
  widget under a mark;
- the marks themselves, replaced by boxes in their fill colour (and overlay text) drawn into the
  page as a `/PCMark /Redaction` stream.

A verification pass re-reads each redacted page and fails the operation if any glyph or inline
image is still under an area. Content that can't be decoded makes the operation fail rather than
leave it unredacted.

Before changing any page, `apply` prepares the redaction appearances with a shared 8 MiB
budget for their uncompressed PDF content across the selected pages. It bounds repeated-text
counts and working line buffers before allocating them, and measures and encodes each repeated
line once. Invalid or excessive appearances return `RedactError::OverlayLimit` without changing
the document. Reduce the marked area, disable repeated text, or apply fewer pages at a time.
This limits appearance generation; it is not a limit on the input PDF or the whole process's memory.

Structure cleanup resolves named MCIDs and indirect content references. Form content
references are updated to the cleaned copy; unchanged sibling descriptions remain.

Replacement descriptions are removed only from marked-content spans changed by cleanup.
Unchanged spans retain their descriptions; unused property entries are retired during redaction
and hidden-content cleanup, including aliases in shared/inherited resources. Descriptions still
used by surviving pages or forms are kept.

A redacted page that inherits `/Resources` from the page tree gets them pinned (by reference;
a direct inherited dictionary is made one shared object first) before it is rewritten. The parent's
`/Resources` is then retired unless another page still inherits it, so it cannot keep a removed
XObject reachable. Pages that aren't redacted are left untouched.

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
