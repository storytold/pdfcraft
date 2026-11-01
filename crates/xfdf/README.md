# pdfcraft-xfdf

Layer L4: comment and form data exchange (execution plan M5.6, M6.7).

```rust
let xml = export_xfdf(&doc, true, true, "form.pdf");   // comments and/or field values
let fdf = export_fdf(&doc, false, true, "form.pdf")?;
let txt = export_data(&doc, Format::Txt);               // or Xml, Csv
let report = import(&mut doc, &bytes)?;                 // XFDF, FDF, XML, CSV or text, detected
```

- **XFDF** (ISO 19444-1): every comment type PdfCraft knows (text, free text, line, square,
  circle, polygon, polyline, markup, stamp, caret, ink, attachments, sound, redact) with rect,
  name, author, subject, dates, flags, colours, opacity, width, icon, quads, line ends, ink,
  default appearance, callouts (`callout`, `fringe`, `intent`, `head`), pop-ups and reply threads
  (`inreplyto`); field values nested by name, with text preserved across XML comments and processing instructions.
- **FDF**: fields (with `/Kids` hierarchies) and indirect comments. Nested annotation values
  are copied under FDF object numbers; shared references and cycles remain shared. Document
  links and appearances are excluded. Source `/Page` and `/Pages` dictionaries become null
  at every copy depth, so nested destination actions cannot carry the source page tree or content.
  Type names may be indirect; lookup checks each reference generation, is iterative, stops at
  cycles and refuses chains beyond
  64 references. Discovery borrows direct arrays and dictionaries,
  retains array owners lazily and records annotation positions instead of resolved bodies.
  Retained entries are budgeted before their keys or values are cloned, including when
  saved-PDF cache eviction causes annotation bodies to be parsed again. Export returns
  `Result` and refuses copied graphs above 100,000 comments/objects/values, 64 direct nesting
  levels or a conservative 64 MiB serialization budget instead of exporting partial data.
  These are copied-graph limits: source parsing and page discovery retain their existing
  COS/parser limits, with no total source-memory bound. Unknown indirect streams retain their
  encoded bytes. FDF import semantics are unchanged.
- **Form data**: Acrobat's XML (`xfdf:original` keeps names that aren't XML names), CSV and
  tab-delimited text (a row of names, a row of values).

XML and XFDF exports preserve tabs and line breaks in field names and comment metadata with
character references, so XML attribute normalization cannot change their identities.

Plain AcroForm XML imports retain all direct text around comments and processing instructions,
including CDATA and character references. Nested-element and XFA interpretation are unchanged.

CSV export and spreadsheet merging quote tabs, and import distinguishes quoted tabs from TSV
delimiters without changing field identities. Automatic detection of externally authored, ambiguous
tables remains heuristic. Both write a cell that a spreadsheet would run as a formula (`=` or `@`,
or `+` or `-` when it isn't a plain number) with a leading `'`; CSV import takes off the `'` that
export added, so exported data imports back unchanged.

Import replaces comments with the same `/NM` (and drops the replaced comment's pop-up), draws
appearances PdfCraft can draw, and sets values through the form's own checks (formats,
validation, recalculation); read-only fields take imported values, as in Acrobat. XML is read
with `roxmltree`.
