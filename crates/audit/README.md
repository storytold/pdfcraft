# pdfcraft-audit

Layer L4 (may use `edit`): verification tooling, the faux-redaction audit.

```rust
let findings: Vec<Finding> = audit_redactions(&doc)?;  // page, kind, rect, covered_text
```

It reports two kinds of finding:

- **faux redaction**: an opaque filled rectangle painted *after* text it overlaps, while the
  text is still extractable underneath. Its colour doesn't matter: a white-out box over black
  text, or a black box over grey or coloured text, hides it as surely as black over black;
- **unapplied mark**: a Redact annotation that was marked but never applied.

A properly applied redaction removes the text, so it never flags. Page backgrounds and table
shading painted before the text don't either, nor do highlights drawn translucent (`/ca` below
1) or with a blend mode such as Multiply, which leave the text visible.

Covers come from the page content, the form XObjects it draws, and the normal appearance of
every annotation a viewer paints (not Hidden or NoView, not a popup), placed by its `/Rect`,
`/BBox` and `/Matrix` (ISO 32000-2 §12.5.5) and painted after the page content: a box drawn
over text with a shape or highlight tool is audited like one in the content.

A rectangle covers a text line when it hides at least half of one of the line's words, at
least 40% of the area of the words the line draws, or at least 40% of the line's box together
with a positive-area intersection with one of its words. The gap an applied redaction leaves (a `TJ` adjustment where the text was) is in the line's box but
holds no word, so a redaction box over it alone does not count. The finding's `covered_text` is
the whole text of every line the cover hides, not only the hidden glyphs.

Limits, by design and documented rather than hidden:

- only filled rectangles count as covers, drawn with `re` or as a single `m`/`l` subpath whose
  corners form an axis-aligned rectangle (not other paths, shadings or patterns), and one that
  is translucent (`/ca` below 1) or in a blend mode other than Normal or Compatible is not
  flagged; an opaque light rectangle painted over text for another reason (shading added after
  the text) is flagged, since it does hide the text;
- covers inside form XObjects and annotation appearances are found, but only text drawn
  directly in the page content is checked under them;
- an annotation whose `/AP /N` holds several states and no `/AS` has no appearance to show and
  contributes nothing;
- annotation rectangles are compared in unrotated user space, so `/Rotate` pages may misalign;
- a cover that hides less than half of every word it touches, less than 40% of a line's box and
  less than 40% of its words (a sliver of a long value) is not flagged.

It fails closed: an undecodable content or appearance stream, malformed `/Annots`, form
XObjects nested deeper than 32 or in a cycle, or a walk past the operator bound is an error,
never "no findings".

Surfaces: the `doc_audit_redactions` automation tool and `pdfcraft-cli audit`. Both report each
rectangle in points from the top-left of the page as displayed (y down, after `/Rotate`), like
the other tools; `AuditFinding::rect` itself stays in PDF user space.
