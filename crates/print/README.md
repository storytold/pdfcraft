# pdfcraft-print

Layer L4: printing (execution plan M10.5).

```rust
let pages = select_pages(count, Some("1-3, ii, 9-"), &labels, Subset::All, false)?;
let settings = Settings { pages, paper: PAPERS[0].1, layout: Layout::multiple(4), ..Default::default() };
let sheets = layout(&display_sizes, &settings)?;   // geometry only (previews)
let pdf = impose(&doc, &settings)?;                // the print-ready PDF
spool::submit(&pdf, &spool::Job::default())?; // Flatpak opens the system dialog for copies/duplex.
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
Annotation appearances receive unused XObject names, preserving existing page resources even when they use the same naming prefix.

The GUI Print dialog remembers its last selected paper size across application sessions,
including when Print is cancelled. The preference stores a canonical name such as `A4`;
unrecognized saved values retain the current selection (US Letter on a fresh app).
This change does not alter persistence of other print preferences.
Headless `doc_print` defaults are unchanged.

`spool` delegates native discovery, driver options, preferences and submission to
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

Not yet: poster labels, PostScript output and vector colour conversion for grayscale
(use Print as image for grayscale PDF output).

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

## Flatpak system printing

On Linux, `/.flatpak-info` or a nonempty `FLATPAK_ID` selects the XDG Desktop Print Portal.
AppImage, DEB, RPM and other native installations retain craft-print's CUPS path; Windows,
macOS and the browser keep their existing backends. Flatpak never enumerates CUPS queues.
The UI offers **System print dialog…** separately from **Save as PDF**, even with no printers.
The `printers` automation tool returns `system_dialog: true` and an empty printer list;
`doc_print` with `printer: "system-dialog"` (or `"default"`) opens the host dialog and returns
`status: "accepted"` or `"cancelled"`. Copies, collate, duplex and driver options must be
chosen in that dialog, so supplying those automation arguments is rejected.

The Linux-only `portal` module uses the maintained safe Rust zbus already used by the platform
crate, with its async-io backend; it adds no Tokio or GUI toolkit dependency. ashpd was evaluated,
but an explicit Request lifecycle lets PdfCraft close requests during shutdown and bound waits
without losing the request handle. The workflow is **PreparePrint followed by Print with its
token**: one host dialog chooses physical output options, then Print uses the token without opening a second dialog. Direct Print
was evaluated first: on the tested KDE 6.7.5 backend it returned response 2 immediately, with no
dialog. The two-stage workflow is required for a usable KDE integration here. PdfCraft keeps its
already-composed PDF and supplies identity page/layout hints. Conflicting host page selection,
N-up, reverse or scaling settings are refused before handoff, rather than applied twice or
silently ignored. Copies, collate, duplex and the output destination belong to the host token.
Response subscriptions precede both method calls, with unique per-connection request paths, so
early responses are retained. Tokens and returned settings are validated before the PDF is sent.
A method reply alone is never success: response 0 means portal acceptance, not physical output;
response 1 is cancellation, and failed or malformed responses are errors.

Page ranges/current page, orientation, scaling, N-up, booklet, poster, annotations and form values
are composed once in PdfCraft. Print as image still works; grayscale in the Flatpak is rendered
as grayscale images before handoff. The portal sends those sheets unchanged. In the host dialog
keep **All pages**, **one page per sheet** and **100% scale**, and match the PDF's sheet size.
Host dialog defaults can differ; the portal cannot force a driver's physical paper, scaling,
collation or duplex. For a both-sides booklet choose short-edge duplex; for cut and stack choose
single-sided and keep sheet order. Physical calibration and driver compatibility require hardware
validation. Save as PDF remains the independent, atomic file-save path.

Composition and D-Bus waits run on a worker from a permission-checked snapshot, so closing or
editing the source document cannot invalidate a pending job. The temporary PDF is anonymous from creation and reopened
read-only through `/proc/self/fd` before handoff; descriptors live until the response and are then dropped.
Shutdown cancels pending requests with Request.Close. Startup and dialog waits are bounded
(30 seconds for connection, 15 minutes for the request; Close gets at most 2 seconds).
Repeated system submissions are refused while a dialog is pending; a cancelled/finished request
allows the next one. The egui worker repaints on completion and cancellation produces no success
notice. Closing a pending portal request does not guarantee recalling a physical job already
handed to the host. An unavailable/unsupported service reports an actionable error and suggests
Save as PDF.

X11 windows supply the portal's `x11:` parent identifier. eframe does not expose a safe exported
Wayland xdg-foreign handle: the Wayland dialog is unparented and nonmodal rather than passing a
raw surface pointer as a handle. It remains usable on KDE/Wayland; ownership/focus is a known
limitation until the toolkit can export a handle. A user may cancel in the host dialog or the
application's progress card; closing the application also closes a pending request. No host spawning, CUPS socket, network or broader
filesystem permissions are added.

Regression tests use a private dbus-daemon and a mock Print service, including a real Unix FD
handoff containing PdfCraft's imposed PDF. This verifies transport and PDF semantics without
requiring a physical printer. See the [validation evidence](../../docs/flatpak-print-validation.md).
