# pdfcraft-print

Layer L4: printing (execution plan M10.5).

```rust
let pages = select_pages(count, Some("1-3, ii, 9-"), &labels, Subset::All, false)?;
let settings = Settings { pages, paper: PAPERS[0].1, layout: Layout::multiple(4), ..Default::default() };
let sheets = layout(&display_sizes, &settings)?;   // geometry only (previews)
let pdf = impose(&doc, &settings)?;                // the print-ready PDF
spool::submit(&pdf, &spool::Job { printer: None, copies: 2, ..Default::default() })?;
```

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
- **Properties… on CUPS** reads the queue's PPD and its current defaults (`lpoptions -l`) off
  the GUI thread, and gives `lpoptions` ten seconds: a CUPS that has stopped answering leaves
  the PPD's own defaults in force instead of holding the dialog (#1818; `spool::output_within`
  kills and reaps the command at the deadline).

Each source page becomes a Form XObject (its content wrapped in q/Q, plus the printable
annotations); sheets place them with a clip. The result is a fresh, unencrypted,
garbage-collected file (callers check the print permission).

The GUI Print dialog remembers its last selected paper size across application sessions,
including when Print is cancelled. The preference stores a canonical name such as `A4`;
unrecognized saved values retain the current selection (US Letter on a fresh app).
This change does not alter persistence of other print preferences.
Headless `doc_print` defaults are unchanged.

`spool` delegates discovery, driver options, preferences and submission to
[`craft-print`](https://github.com/storytold/craft-libs/tree/main/crates/print).
PdfCraft retains page selection, imposition, annotations and print-permission checks.
CUPS jobs stream through stdin with `fit-to-page=false`. Windows renders sheets through
`Windows.Data.Pdf` and spools them at their physical PDF size with per-page paper and
orientation, zero added margins and hardware-edge clipping. Long-edge and short-edge duplex
use the corresponding Windows binding edges. Random, exclusively created Windows job files
are removed on success and failure. Other targets offer the print-ready PDF instead.

The shared dependency requires Rust 1.95. Transport and native Windows regression tests
live in craft-libs; local tests retain composition, facade, engine, automation and UI coverage.
The desktop UI discovers printers and submits jobs on workers, so neither operation blocks
the dialog; the Windows backend starts without a console window. Physical printer output
still needs hardware validation.

In a browser there is no spooler: the app (`pdfcraft-ui-egui`'s `web_print`) draws the print-ready
PDF's sheets with PdfCraft's renderer and prints them through the browser's print dialog, which
sets copies, two-sided printing and colour. This crate stays free of web APIs.

Not yet: print as image, poster labels, PostScript output, colour conversion for
grayscale.

## Architectural sheets and scale

The Paper menu and `doc_print` share Letter, Legal, Tabloid, A3/A4/A5/A6 plus
ARCH A/B/C/D/E1/E, ANSI C/D/E and ISO A0/A1/A2. ARCH D is 24 × 36 inches;
ANSI D is 22 × 34 inches. Dimensions follow the [HP DesignJet paper-size table](https://support.hp.com/hr-en/document/bpp01676).
Automation accepts short names such as `"ARCH D"` (case-insensitive) or the full menu label.
For example, `doc_print` with `paper: "ARCH D"`, `layout: "actual"`,
`orientation: "auto"` and `path: "plan.pdf"` writes a print-ready PDF at 100%.

**Match page at 100%** uses the currently viewed document page to choose a standard
sheet (either orientation, within 0.5 pt for rounded metric dimensions), Actual size
and Auto orientation. It leaves the selected page range intact. It is disabled
for a custom page size with no matching preset. One preset applies to the whole job;
this is not per-page paper-source selection.

**PDF scale** describes the source-to-sheet transform in the previewed print-ready
PDF, not measured physical output. Fit reserves an 18 pt margin and can reduce even
a matching page. On Windows craft-print sends the sheets as vectors with a PrintTicket
for the sheet's exact paper and no scaling (#599's approach), validated by the driver:
a job is refused, not shrunk, when the driver lacks the paper or would change it. Verify
the driver's paper settings and physical calibration before relying on drawing scale.
