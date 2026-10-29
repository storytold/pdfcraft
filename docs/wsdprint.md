# Printing to a WSD network printer

Status: partial (parity entry `print.network-printer-wsd`). Revision history: first added with an
app-local WS-Print client; now built on the WS-Print backend of the shared `craft-print` crate.

PdfCraft's Print dialog hands print-ready PDFs to the operating system's spooler. The WS-Print
backend of `craft-print` is a second, direct path: it speaks the Web Services for Devices print
service to a network printer, so a document can be sent without a driver or a print queue.

Where things live:

* **`storytold/craft-libs`** holds the protocol and the backend. `craft-wsd` is the protocol-agnostic
  core (bounded XML, SOAP/WS-Addressing, MTOM, the numeric local-address policy, the blocking HTTP
  transport). `craft-print` has the `Backend` trait and `Registry`, the non-blocking
  submit/`JobWatch`/retry vocabulary, and, behind its `wsd` cargo feature, the WS-Print backend
  (operations, print ticket, capabilities, fault classification, every listed document format,
  printer resolutions) and the registry's document conversion. Cargo pins the pull request that adds
  them (`rev = "refs/pull/116/head"`, which carries the `craft-wsd` core and `craft-job`) until it merges. The app
  sends through the registry: a `SubmitRequest` with a `Document`, a `Job` (copies, sides, title) and
  `JobOptions` (paper, orientation, quality including `photo`, resolution, colour, the document's
  page count), and follows the typed `JobId` it gets back; no WS-Print id is built by hand.
* **`craft_ui::printing::network`** (craft-libs#112) is the dialog's model and widgets.
* **This repository** keeps the app side: `pdfcraft_print::network` (one `Printer` over a private
  registry, shared by the tools and the command line), the tools `wsd_printer_info` and `print_wsd`,
  `pdfcraft-cli print --wsd`, the paper-name to media-size mapping, and `PdfToRaster`, the
  `DocumentConverter` that renders a PDF to PWG raster (with PdfCraft's renderer and
  `craft_print::pwg`) for printers that list no PDF.

## Formats and conversion

The registry decides, with the printer's own list of formats (`GetPrinterElements`):

* A document in a format the printer lists is **sent as it is**: PDF, PWG raster, PostScript, JPEG,
  PNG, TIFF, plain text, XPS and Open XPS (the printer's own spelling of the MIME type is kept).
* Otherwise it goes through the converter the app registered once at start-up
  (`pdfcraft_print::network::register_pdf_converter`; `Automation::new` and the GUI do it).
  `PdfToRaster` makes PWG raster from a PDF, at the resolution the job asked for (`dpi`), else the
  one the printer lists nearest 300 dpi, else 300; colour follows `color` and the printer's own
  report. The result says `converted_from` and `resolution`.
* Otherwise nothing is sent: the print fails with `UnsupportedFormat` and a message that lists the
  document's format, what the printer accepts and what the converter can make.

`Printer::submit` reads the printer once (`GetPrinterElements`) and hands those details to
`Registry::submit_job_with_details`, so the registry does not read them again; a caller that has just
shown the printer (the Print dialog's query, `Printer::info`/`remember`) pays no read of its own at
all. The backend still reads the capabilities once to plan the ticket, so a converted print costs
two reads from a standing start and one when the details are already in hand.

## Two phases: accepted, then followed

A print is two steps and the tools say so. The printer **accepts** the document (the reply to
`SendDocument`; a bounded call that never waits for paper), and the job is then **followed** by
polling `GetJobElements` until it ends, the printer forgets it, or `wait_secs` runs out. The result
says `accepted: true` once the printer has the document, and `outcome` says how following ended:
`completed`, `completed_with_warnings`, `completed_with_errors`, `canceled`, `aborted`, `forgotten`
(the printer no longer lists it; most likely finished), `still_running` (the wait ran out; the job
continues on the printer), `accepted_unknown` (the printer cannot report) or `failed` (following
failed; the document was still accepted).

**A document is never sent twice by PdfCraft.** If sending fails in a way that leaves it unknown
whether the printer got the document (the connection closed or timed out after the document was
written), the error says to check the printer before sending again, and no job that may be printing
is cancelled. Failures that prove nothing was accepted (a refused address, a printer that is busy)
say that sending again is safe; the caller decides, nothing retries a submit on its own.

## Command line

```sh
# What the printer is and what it accepts (reads only; no job)
pdfcraft-cli print --wsd http://192.168.1.50:5357/service --info

# Print a PDF, two copies, double-sided; --yes is required
pdfcraft-cli print --wsd http://192.168.1.50:5357/service report.pdf --yes --copies 2 --duplex long-edge

# Send a file as it is (its type comes from its content or extension, or --format)
pdfcraft-cli print --wsd http://127.0.0.1:5357/print photo.jpg --yes --quality photo
```

Options: `--copies N`, `--duplex off|long-edge|short-edge`, `--orientation portrait|landscape|reverse-portrait|reverse-landscape`,
`--paper NAME` (a media size name from `--info`, e.g. `iso_a4_210x297mm`), `--quality draft|normal|high|photo`,
`--dpi N`, `--color auto|color|mono`, `--job-name TEXT`, `--strict`, `--wait SECS` (default 120; the job
continues on the printer if the wait ends), `--root DIR`. The report is JSON; the exit status is
non-zero unless the job ended as `completed` or `completed_with_warnings`.

A file sent as it is must be in a format the printer lists and craft-print can name: PDF, PWG raster,
PostScript, JPEG, PNG, TIFF, plain text, XPS, Open XPS or the printer's own language
(`application/octet-stream`). The format is detected from the file's content (`DocumentFormat::detect`;
an XPS package is a ZIP and is never sniffed) and then its name; `--format` says it when neither
does. A file the printer does not list is refused before a job exists, with the printer's list.

A PDF is opened as a document: it is sent as `application/pdf` when the printer lists that,
otherwise rendered page by page to `image/pwg-raster` when the printer lists that, otherwise
nothing is sent and the printer's formats are reported. Printing permissions of the document are
enforced.

## Tools

`wsd_printer_info` (read-only) and `print_wsd` in `pdfcraft-cli run`, MCP and scripts. `print_wsd`
needs `user_confirmed: true`, set only after the person has agreed to this print to this printer;
without it nothing is looked up, let alone sent.

## Printers that stall on the ticket (quirk mode)

Some printer firmware never answers a `CreatePrintJob` whose ticket contains `JobProcessing` or
`DocumentProcessing`. The client sends the full ticket by default, because that is what the schema
allows and what other printers need. For a printer like that there is an opt-in, vendor-neutral
setting:

| Tool argument | CLI option | Meaning |
|---------------|------------|---------|
| `ticket: "full"` (default) | `--ticket full` | `JobDescription` (name, user), `JobProcessing` (copies) and `DocumentProcessing` for whatever is set |
| `ticket: "description"` | `--ticket description` | `JobDescription` only, with both its schema-required children. The recommended workaround. |
| `ticket: "job_name"` | `--ticket job_name` | only `JobDescription/JobName` (omits a schema-required element) |
| `ticket: "empty"` | `--ticket empty` | an empty `PrintTicket` |
| `drop_ticket_settings: true` | `--drop-ticket-settings` | with a reduced ticket, drop settings it cannot carry (a warning each) instead of refusing the print |

With a reduced ticket, options that need the omitted elements (copies, duplex, orientation,
paper, quality, dpi) are **refused up front** (nothing sent) unless `drop_ticket_settings` is set.
`SendDocument` carries no `DocumentProcessing` either. Colour is unaffected because it is chosen
when the picture is made.

## Network policy

* The printer URL is `http://` plus a **numeric** loopback, link-local or private address
  (`127.0.0.0/8`, `169.254.0.0/16`, `10/8`, `172.16/12`, `192.168/16`, `::1`, `fe80::/10`, `fc00::/7`).
  Host names, public addresses and `https` are refused, so a name lookup or a redirect cannot send a
  document somewhere else.
* No proxy, no redirects, a timeout on every request, capped reply sizes.
* Every print needs a confirmation: `UserConfirmed` (`user_confirmed: true` for the tools, `--yes`
  on the command line). Reading a printer needs none and sends no job.
* Nothing discovers printers, sends multicast or starts a listener; the printer is the one the
  person typed.
* The feature is the cargo feature `network-print` (on by default) of `pdfcraft-print`,
  `pdfcraft-automation` and `pdfcraft-cli`; without it (`--no-default-features`) the tools and the
  command refuse, and the build contains no WS-Print code and no HTTP client. It is never built for
  wasm32.

## Print dialog

Print offers a "Network printer (WS-Print)" destination. The person types the printer address
(a numeric local address, the same policy as above), presses "Query printer", and the dialog shows
what the printer reports. Each setting (copies, sides, orientation, quality) follows one of three
rules: a setting the printer lists is offered; a setting the printer explicitly marks unavailable is
disabled, with the reason; a setting the printer does not state either way stays enabled with a
"Not confirmed by the printer" hint. The paper is the dialog's own paper, mapped to the media size
the printer lists (A3, A4, A5, Letter, Legal, Tabloid) when it lists it. A job needs its own confirmation tick ("I confirm sending this
document to this printer"); it is cleared after every job and never stored. The Print button is
disabled until the address was queried, the printer takes PDF or PWG raster, and the box is ticked.

The destination, its rules and its widgets are `craft_ui::printing::network` (craft-libs#112). The
app supplies the wording (the interface-language catalogs), the address check, the worker that
asks the printer (`pdfcraft_print::network::Printer::info`) and the worker that sends a job and
follows it (`submit`, then craft-print's `JobWatch`), and maps craft-print's `Submitted`,
`JobStatus` and `WatchOutcome` onto craft-ui's `Accepted`, `Status` and `Report`.

The job has the same two phases as the tools, and the dialog shows both. While the document is
being sent it says "Sending to ..."; when the printer has it the line becomes "Accepted by ..."
and then follows what the printer says (waiting, held, printing, paused, sheets printed) until
the job ends; "Sent. The printer can't say whether it printed." is a printer that cannot report. A
send whose outcome is unknown is shown with "Check the printer before sending again", and the
confirmation is spent, so sending again takes a new tick: nothing is retried automatically.
Nothing blocks the UI thread: the workers run on their own threads and the dialog drains their
channels once per frame. Cancel works in both phases (it cancels the job on the printer once; the
worker then keeps following it to its end state). When the dialog is closed while a job runs, the
result arrives as a notice.

**Sheet totals.** WS-Print reports sheets completed, not a total. The dialog passes the number of
sheets it sends to the worker (`Request::expected_pages`, which the worker gives craft-print as
`JobOptions::expected_pages`), and craft-ui turns it into an estimated total (halved for a
two-sided job, times the copies): "Sheets printed: 1 of 4" with a progress bar. A total the
printer reports always wins; with no page count the dialog shows only "Sheets printed: N", with no
bar that could never fill.

**Settings the printer leaves out.** Each setting follows craft-ui's three-state capability
(`craft_ui::capability`). A list or range the printer published is offered as it is. A setting
the printer published as not available (a printer that is not colour: Black and white only) is shown
disabled with a reason. A setting the printer did not state at all (WS-Print cannot say "an empty
list", so an empty list means "not stated") stays enabled with the note "Not confirmed by the
printer", and its value is sent as a hint. Colour and resolution are offered the same way and go
into the job options (`JobOptions::with_colour`, `with_resolution`).

**Quality and formats.** The Photo choice is craft-print's `Quality::Photo`: it is offered when the
printer lists it, goes into the ticket like the other choices, and a printer that lists no matching
value gets the job with a warning instead. The dialog sends the print-ready PDF; a printer that
lists PDF gets it as it is, and one that lists only PWG raster gets the pages rendered by
`PdfToRaster` (the registry converts, at the resolution the printer lists nearest 300 dpi). The
`DocumentConverter` is registered once when the destination is created, and the printer's details
that "Query printer" read are handed to the job, so the job costs no read before the registry
decides (the backend still reads the capabilities once to plan the ticket).

The destination is the `network-print` cargo feature of `pdfcraft-ui-egui`, on by default and
never built for the web. A packager who wants no network client in the app builds with
`--no-default-features`: the destination, its code and craft-print's `wsd` backend are then absent
and the Print dialog is unchanged.

## Testing

* `cargo test -p pdfcraft-automation --test wsd_print` and `cargo test -p pdfcraft-cli --test print`
  run the tools against the in-process fake printer (`craft_print::wsd::testing`: `FakePrinter`,
  `MemTransport`, `printer_server`) and, for the command line, a loopback server around it.
  craft-print's `testing` feature is enabled in dev-dependencies only; no shipped build has it.
* `cargo test -p pdfcraft-ui-egui --lib network_glue` runs the dialog's workers (query, send, accepted,
  follow, cancel, a lost reply) against the same fake printer.
* `cargo test -p pdfcraft-automation --no-default-features --test wsd_print_off` and
  `cargo test -p pdfcraft-cli --no-default-features --test print_off` check the build with network
  printing left out.
* The backend's own tests (messages, classification, hostile replies) live in `craft-libs`.
* Never point tests at a real printer. The tests here use loopback and in-memory fakes only.

What is not done: finding printers (WS-Discovery), event subscription, PDF to XPS/PCL/PostScript/image
output (the one converter makes PWG raster only), a raster page larger than the renderer's limits (it
is refused, asking for a lower resolution, rather than printed at the wrong size), imposed sheets on
the raster path, and checking the PWG raster writer against a third-party reader and real devices.
The dialog has been exercised in tests, not looked at on a real device.

Remaining limit of the craft-print API (reported to its authors): the ticket shape, strict mode and
the transfer encoding are backend-wide, so each print opens its own printer with the settings it was
given.
