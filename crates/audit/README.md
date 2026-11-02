# pdfcraft-audit

Layer L4 (may use `edit`): verification tooling, the faux-redaction audit.

```rust
let findings: Vec<Finding> = audit_redactions(&doc)?;  // page, kind, rect, covered_text
```

It reports two kinds of finding:

- **faux redaction**: an opaque filled rectangle painted *after* text it overlaps, dark enough
  against the text's colour (WCAG contrast below 3) to hide it, while the text is still
  extractable underneath; opaque filled Square annotations count as covers too;
- **unapplied mark**: a Redact annotation that was marked but never applied.

A properly applied redaction removes the text, so it never flags. Page backgrounds, table
shading and highlights don't either: they're painted before the text, or light against it.

A rectangle covers a text line when it hides at least half of one of the line's words, at
least 40% of the area of the words the line draws, or at least 40% of the line's box together
with a positive-area intersection with one of its words. The gap an applied redaction leaves (a `TJ` adjustment where the text was) is in the line's box but
holds no word, so a redaction box over it alone does not count.

Limits, by design and documented rather than hidden:

- only filled rectangles count as covers, drawn with `re` or as a single `m`/`l` subpath whose
  corners form an axis-aligned rectangle (not other paths, shadings or patterns), and a
  translucent one (`/ca` below 1) is not flagged;
- covers inside form XObjects are found, but only text drawn directly in the page content is
  checked under them;
- annotation rectangles are compared in unrotated user space, so `/Rotate` pages may misalign;
- a cover that hides less than half of every word it touches, less than 40% of a line's box and
  less than 40% of its words (a sliver of a long value) is not flagged.

It fails closed: an undecodable content stream, malformed `/Annots`, form XObjects nested deeper
than 32 or in a cycle, a walk past the operator bound, more than 50 million cover-against-word
comparisons on a page, or more than 64 MiB of covered text in the report is an error, never "no
findings".

A document whose permissions withhold copying (Table 22, bit 5) still gets every finding, with
an empty `covered_text`: the audit hands out no text that copying would refuse.

Surfaces: the `doc_audit_redactions` automation tool and `pdfcraft-cli audit`. Both report each
rectangle in points from the top-left of the page as displayed (y down, after `/Rotate`), like
the other tools; `AuditFinding::rect` itself stays in PDF user space.
