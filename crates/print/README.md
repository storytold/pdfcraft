# pdfcraft-print

Layer L4: printing (execution plan M10.5).

```rust
let pages = select_pages(count, Some("1-3, ii, 9-"), &labels, Subset::All, false)?;
let settings = Settings { pages, paper: PAPERS[0].1, layout: Layout::multiple(4), ..Default::default() };
let sheets = layout(&display_sizes, &settings)?;   // geometry only (previews)
let pdf = impose(&doc, &settings)?;                // the print-ready PDF
spool::submit(&pdf, &spool::Job { printer: None, copies: 2, ..Default::default() })?;
```

- **Paper**: Letter, Legal, Tabloid, ARCH A/B/C/D/E1/E, ANSI C/D/E and ISO A0–A5.
  Match page at 100% chooses the matching standard sheet and Actual size; the preview
  shows the effective scale. Arbitrary custom driver sizes are not supported.
- **Size**: fit (to the sheet minus an 18 pt margin), actual size, shrink oversized, custom %;
  centred; auto orientation turns the sheet for landscape pages.
- **Multiple**: 2/4/6/9/16 (or any n) pages per sheet, horizontal/vertical (reversed) order,
  page borders, auto-rotation of pages that don't match the cell.
- **Cut and stack** (Multiple's page order): consecutive pages in each cell's pile,
  with aligned cut marks in the gutters. For example, 10 pages at 4 per sheet give
  `[1, 4, 7, 10]`, `[2, 5, 8, blank]`, `[3, 6, 9, blank]`. Print single-sided,
  keep sheets in output order, cut at the marks, then stack the cell piles from
  left to right, top to bottom. Blank cells can be discarded. Page ranges and
  Reverse pages are applied before imposition. Duplex imposition is not supported.
  Headless: `doc_print` with `layout: "multiple"`, `order: "cut-stack"` and
  `per_sheet: 4` (or any supported grid size), plus `path` or `printer`.
- **Booklet**: saddle-stitch imposition padded to a multiple of 4; both sides, front or back
  only; left or right binding.
- **Poster**: tile scale, overlap shared by neighbouring tiles, cut marks.
- **Comments & forms**: document, + markups, + stamps, or form fields only; annotations print
  only with their Print flag, drawn from their appearance streams (Algorithm 8.1).

Each source page becomes a Form XObject (its content wrapped in q/Q, plus the printable
annotations); sheets place them with a clip. The result is a fresh, unencrypted,
garbage-collected file (callers check the print permission).

`spool` talks to CUPS (`lpstat -p -d`, `lp` with copies, collation, duplex and monochrome
options). The job is piped to `lp` on stdin and never written to a temp file, where another
local user could read or swap it. Windows discovers local/connected queues and submits PDFs through Windows.Data.Pdf and
Direct2D using the safe `winprint` wrapper. The job uses a private temporary directory
which is removed after submission. Validated PrintTickets carry copies, collation,
duplex, colour, paper size and orientation. Driver media names are preserved, and
substituted dimensions, orientation or scaling are rejected before submission.
One Windows job has one sheet size and orientation; choose a fixed orientation or
submit differing sheets separately. The print-ready PDF can always be saved.

Not yet: web spoolers, native printer-properties UI, print as image, poster labels, PostScript output, colour
conversion for grayscale.

Windows regression coverage lives in this crate, the UI's `tests/print.rs`, and the
automation printing tests. Default tests never submit a physical print job. The ignored
virtual-printer tests require a dedicated `PdfCraft Validation` queue using Microsoft's
PDF driver and a file port; they verify that configuration before submission. Driver
ticket inspection is separately opt-in via `PDFCRAFT_TEST_PRINTER` and sends no job.
The `pdfcraft-ui-egui` example `print_dialog` renders a synthetic ARCH D dialog without
querying printers (`PDFCRAFT_SYSTEM_FONTS=0` for publication-safe screenshots).
