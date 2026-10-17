# pdfcraft-redact

Layer L4 (may use `edit`, `organize`): redaction, architecture §11.2.

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
- **annotations** whose rectangle overlaps (with their pop-ups), and **form fields** with a
  widget under a mark;
- **document metadata** that repeats removed text: `/Info` entries whose value contains a removed
  text run (case-insensitive) are cleared — through `pdfcraft_organize::set_info`, which also
  removes the XMP property mirroring each entry (`dc:title`, `dc:creator`, `dc:description`,
  `pdf:Keywords`, `xmp:CreatorTool`, `pdf:Producer`) while leaving the rest of the packet
  byte-for-byte intact;
- the marks themselves, replaced by boxes in their fill colour (and overlay text) drawn into the
  page as a `/PCMark /Redaction` stream.

A verification pass re-reads each redacted page and fails the operation if any glyph or inline
image is still under an area. Content that can't be decoded makes the operation fail rather than
leave it unredacted.

## Metadata redaction (#2448)

What the marks removed must not survive in metadata either. The interpreter decodes every glyph
run it cuts out into *fragments*: maximal runs that join across `TJ` kerning numbers (spacing
inside one text run: no space, no split) **and operators** — text drawn one glyph per operator,
as many generators do, still collects as one run — whenever the next removed glyph starts within
1.5 glyph heights of where the last one's advance (character and word spacing included) landed
in device space; a repositioning gap past 0.15 heights becomes a space, and kept glyphs, codes
with no Unicode meaning, and page ends still split runs.
Forward `TJ` displacements can also encode word spacing. Those gaps are optional spaces during
metadata comparison, retaining the unspaced interpretation for intra-word kerning. Mixed kerns
and word gaps are compared without enumerating combinations; literal spaces remain required.
After verification, fragments and values are lowercased with internal whitespace collapsed.
Each `/Info` string entry containing a complete fragment of two characters or more — or whose
complete value is contained in a fragment — is cleared with its mirrored XMP property. The
author of a whole redacted sentence is removed text too. Then every catalog and page
`/Metadata` stream is parsed as XML and checked value by value (decoded text, comments,
attribute values and processing-instruction data). Tag names, attribute names, namespace declarations
and processing-instruction targets are excluded. XML entities and CDATA are decoded by the parser. A packet is removed whole
when a fragment or part of one survives in a value — custom schemas cannot be
edited per-entry — or when the stream cannot be decoded and read as UTF-8 within 16 MiB or parsed
as XML. DTDs are unsupported and take this conservative whole-packet fallback. A
trailer `/Info` that is a direct dictionary (the one shape an entry clear cannot edit) is
removed whole when it still matches. All of this happens before the forced full save, so the
previous revision's bytes leave the file.

The match errs towards clearing, in both directions:

- **over-clearing**: a fully redacted common word is a fragment, and an unrelated entry or packet
  merely containing it goes too; glyphs removed under *different* marks also join when they land
  within 1.5 em of each other in stream order, so two one-letter removals can form a two-character
  needle; the wholesale fallback drops a whole stream for one custom-schema hit. A short complete
  XML value such as language `en` or version `1.7` can still match part of a longer removed run;
- **under-matching**: text inside images never becomes a fragment, runs shorter than two
  characters are ignored, fragments split at gaps wider than 1.5 em and carry narrower
  repositioning gaps as spaces, so a value that spells the same text with or without one of those
  spaces matches only the spelling the run collected — only glyphs shown in stream order at
  adjacent positions join, so overprint-interleaved, character-batched or logical-order RTL
  generators still leave one-character runs, and codes with no Unicode meaning still split
  them — and `/Metadata` on component objects (images, fonts, form XObjects) is not scanned — the
  same scope as Remove hidden information.

Matching still requires a complete removed fragment inside a value, or a complete value inside
a fragment. Partial phrases such as Title `Case: James Bond` after redacting `JAMES BOND IS A SPY`,
reordered names such as `Smith, John` versus `John Smith`, and Unicode normalization remain
unsupported. A whole removed `James Bond` still matches `Case: James Bond`; the unsupported case
is a subphrase of a longer removed run. No word-term deletion is performed: unrelated Subject
`Spy stories` stays during selective matching.

Collection is limited across all selected pages to 1,024 runs, 16 KiB per run and 256 KiB of
run text in total, checked before appending (optional-gap copies have the same bounds).
Metadata comparison has one shared budget of 8 Mi work units, including failed prefix attempts
in repetitive text, value normalization, key scanning and XML inspection. Each prefix character
costs 16 units, including its optional-gap lookup; scanning a string costs its byte length,
and normalization/XML input costs four units per byte. A value is limited to 1 MiB and an
information dictionary to 1,024 entries. Encoded packet bytes are charged before decoding, which
is limited before allocation by the remaining allowance as well as the existing 16 MiB ceiling.
Decoding failures conservatively exhaust the allowance because the decoder does not expose
consumed work. These are bounds on this metadata
pass, not on the whole PDF or the process's memory.

If collection, a value or the shared work/decoding allowance exceeds its limit, the pass removes
**all** `/Info` and catalog/page `/Metadata`, including unrelated values and earlier clean packets.
It never silently truncates needles or treats unfinished matching as clean. The library report's
`metadata_cleared_on_limit` records this conservative fallback, alongside the actual removal counts;
the normal full rewrite removes retired objects. This does not expand the scan to component-level
metadata or change the partial-name matching policy.

Remove hidden information (metadata category, `doc_remove_hidden`) is therefore the certain
follow-up when a document's metadata must be provably free of the redacted text.

Before changing any page, `apply` prepares the redaction appearances with a shared 8 MiB
budget for their uncompressed PDF content across the selected pages. It bounds repeated-text
counts and working line buffers before allocating them, and measures and encodes each repeated
line once. Invalid or excessive appearances return `RedactError::OverlayLimit` without changing
the document. Reduce the marked area, disable repeated text, or apply fewer pages at a time.
This limits appearance generation; it is not a limit on the input PDF or the whole process's memory.

Structure cleanup resolves named MCIDs and indirect content references. Form content
references are updated to the cleaned copy; unchanged sibling descriptions remain.

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
