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
  area are removed. Images with separate `/SMask` or `/Mask` data, and images whose data decodes
  to more than twice their declared raster (plus a byte per row and 4 KiB), are removed whole;
  cleared copies drop `/Alternates` so alternate pixels cannot survive;
- **vectors**: covered paths are removed; partly covered paths and shadings are clipped so
  nothing paints inside the areas;
- **form XObjects**: rewritten recursively into new objects (pages sharing the original keep it);
- **annotations** whose rectangle overlaps (with their pop-ups and their replies, `/IRT`), and **form fields** with a
  widget under a mark;
- the marks themselves, replaced by boxes in their fill colour (and overlay text) drawn into the
  page as a `/PCMark /Redaction` stream.

A verification pass re-reads each redacted page and fails the operation if any glyph or inline
image is still under an area. It checks every content stream the page had, whatever its
dictionary calls it; only the overlay streams this `apply` added (`Report::overlays`) are skipped.
Text is placed with the font and size set by `Tf` or by an ExtGState's `/Font` through `gs`, and a
form XObject starts from the text state of the content that draws it. Content that can't be
decoded makes the operation fail rather than leave it unredacted, and so does an inline image the
renderer's tokenizer reads no image for (`RedactError::Unreadable`): where the page's operators
resume after it would be a guess. A Redact annotation with neither a usable `/QuadPoints` array nor a usable
`/Rect` makes it fail with `RedactError::UnreadableMark` before anything changes, instead of
skipping that mark. The count covers the marks on the pages being applied.

Before changing any page, `apply` prepares the redaction appearances with a shared 8 MiB
budget for their uncompressed PDF content across the selected pages. It bounds repeated-text
counts and working line buffers before allocating them, and measures and encodes each repeated
line once. Invalid or excessive appearances return `RedactError::OverlayLimit` without changing
the document. Reduce the marked area, disable repeated text, or apply fewer pages at a time.
This limits appearance generation; it is not a limit on the input PDF or the whole process's memory.

Structure cleanup resolves named MCIDs and indirect content references. Form content
references are updated to the cleaned copy; unchanged sibling descriptions remain. The structure
tree's object references (`/OBJR`) to the annotations and form fields `apply` removes are dropped,
as Sanitize drops them, so a full rewrite of a tagged PDF can't keep a removed comment or field value.
A tree where that can't be done (an element written in place, or one too large to walk) makes
`apply` fail with `RedactError::Tags` rather than leave the annotation reachable.

Replacement descriptions (`/Alt`, `/ActualText`, `/E`) are removed from marked-content spans
changed by cleanup, from every structure element above them (found through both `/P` and `/K`),
and from elements that lose an annotation reference, which also lose `/T`. An element written in
place above one whose descriptions go makes `apply` fail with `RedactError::Tags`. Unchanged
spans retain their descriptions; unused property entries are retired during redaction
and hidden-content cleanup, including aliases in shared/inherited resources. Descriptions still
used by surviving pages or forms are kept.

A redacted page that inherits `/Resources` from the page tree gets them pinned (by reference;
a direct inherited dictionary is made one shared object first) before it is rewritten. The parent's
`/Resources` is then retired unless another page still inherits it, so it cannot keep a removed
XObject reachable. Surviving form XObjects lose the resource entries for removed XObjects their own
content doesn't draw, for the same reason. Pages that aren't redacted are left untouched.

## Remove hidden information / Sanitize

```rust
let found = sanitize::scan(&doc);                          // [(Hidden, count)] per category
sanitize::remove_hidden(&mut doc, &[Hidden::Comments])?;   // chosen categories
sanitize::sanitize(&mut doc)?;                             // all of them
```

Categories: metadata (`/Info`, XMP), file attachments, comments, form fields (flattened so the
values stay visible), hidden text (render modes 3 and 7, or wholly off the page), hidden layers
(content of off optional-content groups, then the groups), bookmarks, links/actions/JavaScript,
and `/PieceInfo` private data. XMP metadata, private data, associated files (`/AF`) and
additional actions (`/AA`) are removed from every object reachable from the catalog that owns
them (images, fonts, Forms, annotations and fields as well as the catalog and pages), never from
name maps such as resource dictionaries. Content is read strictly: a page whose content can't be
read in full is refused, not cleaned in part. Both make the next save a full rewrite.

`patterns` finds phone numbers, e-mail addresses, credit card numbers (Luhn-checked), US Social
Security numbers and dates for Search & Redact.

`codes` holds the redaction code sets (U.S. FOIA and U.S. Privacy Act exemptions):
`CodeSet::from_id("foia")?.overlay(&["(b)(6)"])` gives the overlay text for the picked codes.
Custom code sets aren't supported yet.

Not yet: pattern locales, struct-tree/alt-text cleanup, metadata and hidden-information
sanitising (M8.4), re-encoding of DCT images (they are removed instead).
