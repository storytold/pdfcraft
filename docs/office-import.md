# Word, PowerPoint and Excel to PDF

> **Status:** M10 candidate, native import of Office Open XML · **Revision history:** 2026-10-14 first version; 2026-10-16 #2871 workflow completion and reporting; 2026-10-22 Turkish and Central European letters drawn, UI notices translated and shown only when something was approximated; 2026-10-25 review fixes (Word 2010 text boxes, nested boxes, master placeholders, merged table cells, stored column widths) and the known gaps listed; 2026-10-27 wording: what can be left out, which workflows convert off the UI thread; 2026-10-31 body page fields, ISO date cells, Office names refused as PDF destinations; 2026-11-11 the native converter becomes the fallback to LibreOffice, which main added for File ▸ Open.

PdfCraft opens Office documents as new, unsaved PDFs in two ways. Where LibreOffice is installed,
the desktop app and automation convert through it (`pdfcraft-create`'s `office.rs`; see the
[create README](../crates/create/README.md)): it reads every Office format and lays documents out
as LibreOffice does. Where it isn't, and wherever waiting seconds for it isn't allowed, Word
(`.docx`, `.docm`, `.dotx`), PowerPoint (`.pptx`, `.pptm`, `.ppsx`, `.potx`) and Excel (`.xlsx`,
`.xlsm`, `.xltx`) files are converted natively instead, by the Rust converter this page describes
(`pdfcraft-create`'s `from_office`): no office suite, converter process or network is involved,
and it works in the browser build too. If LibreOffice fails on one of these files, the native
converter is tried before giving up. The source file is only read either way.

The engine picks the converter (`Session::convert_to_pdf_reported`): LibreOffice when it is
installed and the caller may wait for it, the native converter otherwise. Insert pages, Combine
files and Create from multiple files convert on the frame thread, so they always use the native
converter (`Session::convert_to_pdf_inline_reported`), and refuse the legacy binary formats,
OpenDocument and RTF, which only LibreOffice reads, pointing to File ▸ Open.
`Session::without_libreoffice` makes a session use the native converter on every machine (tests).

## Where it is reached

- **File ▸ Open** (and Create ▸ PDF from File) has an *Office documents* filter. The open worker
  converts in the background and the result opens in a new tab; Save As chooses where it goes. If
  the native converter approximated something, a notice says what.
- **Create PDF from multiple files**, **Combine** and **Insert pages** accept Word, PowerPoint and
  Excel files the same way they accept images and text, converted natively, and say what each
  conversion approximated. **Batch convert** goes through `Session::convert_to_pdf`.
- **Automation:** `doc_create` with `{"from": "office", "path": "report.docx"}` returns the new
  document like `doc_open`, plus an `office` object (`kind`, `pages`, `escaped`, `notes`).
  `--root` confines the path as for every other tool.
- **Direct automation open:** `doc_open` converts Office inputs into unsaved PDFs with no source
  save path. It returns the same `office` report as `doc_create`.
- **Organize Pages insertion:** `page_insert_file` accepts converted Office/image/text inputs as
  the UI does, keeps 1-based source page selection, and remains undoable. Office conversion returns
  an `office` report; a failed conversion leaves the destination unchanged.
- **Conversion notices (#2871 follow-up):** Open (native and in-memory) and Insert say, in the
  interface language, what was approximated; an exact conversion opens without a notice. Combine
  puts the same sentence in the row's warnings. Automation results carry an English `notice` that
  also states the standard-font caveat, and `doc_combine` returns a `conversions` array with
  per-source paths, approximation counts and notices. Deep content skipped at the nesting
  limit is counted. Source files are never written by these operations, and the automation tools
  that write a PDF (`doc_save`, and `doc_combine` and `doc_create_multiple` with `out`) refuse a
  Word, PowerPoint or Excel file name as the destination, so a source can't be overwritten.

```json
[
  {"tool": "doc_create", "args": {"from": "office", "path": "report.docx"}},
  {"tool": "doc_save", "args": {"doc": 1, "path": "report.pdf"}}
]
```

## What is carried over

- **Word:** sections (page size, orientation, margins), paragraph and character styles with their
  `basedOn` chains and document defaults, direct formatting (font family, size, bold, italic,
  underline, strike-through, colour, highlight and shading, super- and subscript, caps, hidden
  text), alignment including justification, indents and hanging indents, spacing and line
  spacing, tab stops, numbered and bulleted lists (`numbering.xml`, nine levels, restarts),
  tables (column grid, spans, vertical merges, cell shading, table-style and cell borders, rows
  split across pages), inline and anchored pictures, page breaks, headers and footers per section
  (first-page variants) with live `PAGE` and `NUMPAGES` fields, footnotes and endnotes (placed at
  the end with their marks), equations as plain text, and text boxes' text.
- **PowerPoint:** one page per visible slide at the slide size; backgrounds (solid, gradient as one
  colour, picture); master and layout decorations; placeholder geometry and text styles inherited
  from the layout and master (`titleStyle`, `bodyStyle`, list styles); theme colours with
  `lumMod`/`lumOff`/`tint`/`shade`, theme fonts; bullets (characters and automatic numbering);
  `normAutofit` scaling; text anchoring and insets; shapes with fills, outlines, rotation and
  flips (common preset geometries and custom paths); pictures with cropping; groups; connectors;
  tables (with a default banded style); slide-number fields. Hidden slides are left out, as
  PowerPoint does when saving a PDF.
- **Excel:** each visible sheet's used range at 100 %, column widths and row heights (hidden ones
  skipped), pages ordered down then across, gridlines when the sheet asks for them; cell values
  with their number formats (General, decimals and thousands, percentages, scientific, currency
  tags such as `[$R$-416]` with the locale's decimal comma, dates and times with English or
  Portuguese month and day names, sections for negatives and zero, `[Red]`); fonts, fills,
  borders, horizontal and vertical alignment, indents, wrapped text, merged cells, overflow into
  empty neighbours and `###` for numbers that don't fit. Formulas show their saved results.

## What is approximated

Content-specific approximations are counted in the report (`notes`); automation's `notice` also
states the standard-font substitution. Nothing in the source is changed.

- **Fonts:** text is set in the standard PDF faces (Helvetica, Times, Courier and their bold and
  italic styles), chosen from each run's font family: Calibri, Arial, Aptos → Helvetica;
  Cambria, Georgia, Times New Roman → Times; Consolas, Courier New → Courier. No font program is
  embedded, so the PDF stays small and searchable, but line breaks can differ from Office's.
  Beyond WinAnsi, the Turkish, Central European, Baltic and Romanian letters (ğ ı İ ş, ą ć ę ł ń
  ś ź ż, č ř ů, ő ű, ā ē ī ū, ș ț…) are drawn from the same faces through a second encoding with a
  `/ToUnicode` map, so they look like the surrounding text and stay searchable. `ď ľ Ľ ť ģ Đ đ`
  are not among them: viewers that stand in for the standard faces (PdfCraft's renderer included)
  build accented letters from a base letter and a plain accent and can't build these. They, and
  everything the faces can't draw at all (Greek, Cyrillic, CJK, Arabic, emoji…), are written as
  `[U+XXXX]` and counted; they are never replaced by `?`. Embedding real faces would cover them.
- **Not drawn, shown as outlined boxes:** charts, SmartArt, embedded objects, and pictures in a
  format PdfCraft can't decode (EMF, WMF, SVG) or linked rather than embedded.
- **Simplified:** gradients use their middle colour; arcs in custom shapes are drawn straight;
  preset shapes without a specific outline use their bounding rectangle; vertical text is drawn
  horizontally; text boxes in Word join the text flow after their paragraph; multi-column
  sections are set in one column; Excel charts and pictures, chart sheets, headers and footers,
  and print areas are not used.
- **Known gaps, not yet handled** (the source file is never changed, but some content is
  simplified or left out of the PDF; the conversion report notes many of these cases, not all of
  them, so an empty report doesn't mean an exact copy: check this list):
  - Word: right, centre and decimal tab stops are set as left stops, so a "title ⇥ page" line in a
    table of contents or footer can wrap; dot leaders are not drawn; a table style's paragraph
    spacing and banded shading are not applied to its cells; tables in headers and footers are
    set as one line per row; a picture's crop (`srcRect`) is ignored, so a cropped picture is
    squeezed; paragraph borders (`pBdr`) and paragraph shading inside table cells are not drawn.
  - PowerPoint: transparency (`alpha`) is ignored, so a translucent overlay is drawn opaque;
    curved and bent connectors are drawn as boxes; Wingdings bullets show as the letters they are
    stored as; text in a flipped shape is flipped with it; placeholders inherit no fill or
    outline from the layout or master.
  - Excel: a number that just fits its column in Calibri can be shown as `####`, as Helvetica is
    wider; General numbers aren't shortened to fit; outside borders of merged ranges and their
    wrap width come from the first cell only; rows without a stated height aren't grown for
    large fonts.
  - Desktop app: Insert pages, Combine and Create PDF from multiple files convert each file on the
    UI thread (as they do images and text), so a large Office file pauses the window while it
    converts; File ▸ Open converts off the UI thread.
- **Not supported natively:** password-protected files and the legacy binary formats (`.doc`,
  `.ppt`, `.xls`) — they are refused with a message saying so — and OpenDocument and RTF files.
  LibreOffice, where installed, opens the legacy and OpenDocument formats and RTF from File ▸ Open
  and in automation.

## Safety

Packages are untrusted input. The zip reader accepts only stored or deflated parts, refuses zip64
and encryption, caps each part at 128 MiB and a package at 512 MiB decompressed, and never
inflates past a part's declared size, counting what a failed part inflated. A sheet keeps at most
64 MiB of cell text (the rest is left out and reported). XML is parsed with DTDs disallowed and a node limit. Style
chains, nesting, table spans, counts and every number read from the file are bounded, so a
hostile file is refused or approximated, never a crash. Conversion runs under the engine's panic
guard, and in File ▸ Open it runs off the UI thread. Macros (`vbaProject.bin`) are never read.
LibreOffice's own safeguards (a private folder, a fresh profile with macros off, a time limit) are
described in the [create README](../crates/create/README.md).

## Verification

`crates/create/src/ooxml/tests.rs` builds Word, PowerPoint and Excel packages from synthetic text
and pixels in the test itself (no committed fixture) and checks the reopened PDF: page sizes,
fonts, text, list labels, table cells, images, header page numbers, inherited placeholder styles,
theme colours, autofit, number formats, merged cells, hidden content, and hostile input.
`crates/automation/tests/it/automation.rs::creating_pdfs_from_word_and_powerpoint_natively` exports a
PDF to `.docx` and `.pptx` with PdfCraft's own exporter and imports them back through `doc_create`,
with LibreOffice turned off (`Automation::without_libreoffice`) so the native converter runs on every
machine. `crates/engine/src/tests.rs::office_documents_go_to_libreoffice_or_the_native_converter`
checks which converter each case gets.
