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
  Formatting travels both ways as XFDF 3.0 names it: rich text (`<contents-richtext>`, the `/RC`
  XHTML as markup; plain text in it is the comment's text), default style (`<defaultstyle>`),
  border width, style and dash pattern (`width`, `style`, `dashes`; also from an older `/Border`
  array), cloudy borders (`style="cloudy"`, `intensity`), `rotation`, `justification` (free text
  `left`/`centered`/`right`, redaction `0`–`2`), `fringe` (`/RD`) on every type, line leaders and
  captions (`leaderLength`, `leaderExtend`, `leader-offset`, `caption`, `caption-style`,
  `caption-offset-h`/`-v`), the caret `symbol` and redaction `overlay-text`/`overlay-text-repeat`.
  The intent (`/IT`) is written as both `intent` and `IT`, and either is read.
- **Appearances** (`<appearance>`, `appearance.rs`): a stamp's `/AP`, as XFDF 3.0 defines it for
  stamps, base 64 of the dictionary written as `DICT`/`STREAM`/`ARRAY`/`NAME`/`INT`/`FIXED`/`BOOL`/
  `STRING`/`NULL` elements with stream data in `DATA` (XFDF 3.0 "Stream encoding": `filtered`/`ascii`
  or `raw`/`hex`), the shape XFDF files from other tools carry. Copy and paste carries every
  comment's appearance. Import keeps an appearance it can read (except on a redaction mark, which
  PdfCraft always draws) and otherwise draws the comment and records the reason in
  `Report::skipped` (the engine puts it in the repair log); it refuses one over 16 MiB, 64 MiB per
  import, 64 nesting levels or 100,000 values. Export leaves out one past those limits or whose
  rect has no area, and stops at the 64 MiB an import reads; where `/AS` selects one of several
  states, the selected stream is written (and import, with no `/AS` in XFDF, refuses a normal
  appearance of several states). Import and FDF import are all or nothing: on an error the
  document is left as it was.
- **FDF**: fields (with `/Kids` hierarchies) and indirect comments. Nested annotation values
  are copied under FDF object numbers; shared references and cycles remain shared. A stamp's appearance
  (`/AP`, with the forms and images it draws) is copied, as in XFDF, and an `/AP` in the file comes
  back on import as objects of the document (a stream shared by several comments once); document links are excluded. Source `/Page` and `/Pages` dictionaries become null
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
  encoded bytes. FDF import copies a comment's streams only when the comment lands on a page, and
  refuses the file (nothing imported) when its comments expand past 1,000,000 values or 64 MiB, or
  nest more than 256 levels, rather than import part of an appearance.
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

Import replaces comments with the same `/NM` (and drops the replaced comment's pop-up), keeps the
appearance the file carries or draws one where PdfCraft can, and sets values through the form's own checks (formats,
validation, recalculation); read-only fields take imported values, as in Acrobat. XML is read
with `roxmltree`.
