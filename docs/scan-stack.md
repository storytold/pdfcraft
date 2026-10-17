# Scan stack

Scanning is Windows/WIA only in this version. On other platforms the same code builds, no backend
is registered, and every scan request reports one sentence ("scanning is only supported on Windows
(WIA) in this version"). Other backends plug in behind `craft_scan::Backend`; device ids are
`<backend>:<native id>`, so a WIA scanner is `wia:<WIA id>`.

| Crate | Role |
|------|------|
| `craft-scan` (storytold/craft-libs, [#114](https://github.com/storytold/craft-libs/pull/114)) | The portable core: scan settings, errors and recovery, document-handling status, the acquire contract (progress and cancel tokens, in-memory pages), software 1-bit conversion, a BMP parser, tracing, the `Backend` trait with its registry, and `ScanJob` (one acquisition across its attempts, the journal, the exit classes). No platform code and no unsafe. Pinned in the workspace `Cargo.toml` to `refs/pull/114/head` (which builds on the `craft-job` crate of [#120](https://github.com/storytold/craft-libs/pull/120); `craft-job` is pinned to the same ref so there is one copy of it) until those are merged; the app enables its optional `serde` feature for the journal. |
| `pdfcraft-wia` | The Windows WIA backend (COM), registered as `wia`. The only crate in the stack with `unsafe`; see [unsafe-audit.md](unsafe-audit.md). Its `rules` module holds the HRESULT tables, the sentences and the status-bit rules, with no COM, so they are tested on every platform. |
| `pdfcraft-platform` | `pdfcraft_platform::scan`: re-exports `craft-scan` and registers the WIA backend once (`register_backends`, idempotent; the scan functions call it first, so no entry point needs start-up code). `scan::supported()` answers at run time: true once a backend is registered. |
| `pdfcraft-create` | Keeps a black-and-white 1-bit BMP as a 1-bit image when creating a PDF. |
| `pdfcraft-automation` | The `scan_devices`, `scan_acquire` and read-only `scan_jobs` tools (and their MCP form), and the `scan_jobs` module that reads and writes the job journal for the CLI and the tool. |
| `pdfcraft-ui-egui` | The From scanner… dialog (`scan_ui.rs`): hosts the shared `craft_ui::scanning` device list, settings and presets, and owns the preview, region, advanced options, Create PDF, Insert scanned pages, manual duplex and the interruption prompt. |
| `pdfcraft-cli` | `pdfcraft-cli scan devices`, `pdfcraft-cli scan check` (what the stack sees; one optional 100 dpi acquire), `pdfcraft-cli scan --out DIR --yes` (a `ScanJob`), `scan jobs`, `scan job DIR` and `scan --continue DIR`; see [cli.md](cli.md). |

Devices, feeder, WSD and duplex behaviour are described in
[hardware-parity.md](hardware-parity.md#windows-wia-scanner-acquire).

Tests never open real scanners unless `PDFCRAFT_TEST_REAL_SCANNERS` is set. `CRAFT_SCAN_TRACE`
records the acquire phases; the older `PDFCRAFT_SCAN_TRACE` is still read when the new name is not set.

## Page length

A document feeder often cannot tell how long the sheet it pulled is. In the measured case (a WIA device on USB and on WSD) Windows
itself reports that the feeder cannot detect the page size, and WIA lists one page size plus an adjustable vertical extent, but
nothing that reports the sheet length. `--page-length` (CLI) and `page_length` (the `scan_acquire` tool, and the `request` of the job
journal) choose what to do, in this priority order. `craft-scan`'s README has the vocabulary; this is what the WIA backend does.

| `--page-length` | What happens |
| --- | --- |
| `driver` (default) | Nothing new. The driver keeps whatever extent it has. A `--page-size` is applied as it always was. |
| `size` | The `--page-size` length as extents (step 2 below) even when the scanner could detect one. Without a size that names a length (`auto` and `custom` do not) it is the driver default. |
| `auto` | 1. **Device detection**, when the item *publishes* it: `WIA_IPS_AUTO_CROP` accepting `WIA_AUTO_CROP_SINGLE` (1), a `WIA_IPS_PAGE_SIZE` list holding `WIA_PAGE_AUTO` (100), or `WIA_IPS_LONG_DOCUMENT` accepting `WIA_LONG_DOCUMENT_ENABLED` (1), in that order. Recorded as `device`. 2. **An explicit `--page-size`** (a4, letter, legal, business-card): the page size is written when the item lists it, then the vertical extent is set to that length at the scan resolution, inside the item's published range; a size the item does not list is written as the extent only, never as an unlisted page-size value. A length more than one step outside the range is an error (`The scanner cannot scan a page 14.00 in long at 200 dpi (its longest is 11.69 in)`), not a silent clamp. Recorded as `size`. 3. Otherwise the driver default, recorded as `driver`. Never looks at pixels. |
| `infer` | `auto`, and when neither device detection nor a size applies **on the feeder**: request the item's longest vertical extent and cut each page at the end of the sheet found in the image. **Approximate**; recorded as `inferred` with `approximate: true`. |

An explicit region (`--left/--top/--width/--height`) wins over every mode: it already says which area to scan. `--page-size custom` and
`auto` name no length. Width is never changed by the length policy.

**How inference decides** (`craft_scan::infer_sheet_end`): the scanner that was measured fills everything beyond the sheet end with pure
white (255,255,255) while the paper itself carries scanner noise. The last row that is not pure white is the sheet end (plus a 0.01 in
margin). The rule refuses unless the rows above the end show that noise, so a white margin that is exactly 255 everywhere is not mistaken
for fill and the end of the printing is not taken for the end of the sheet. It runs on the delivered pixels before any black-and-white
thresholding. It was measured on one scanner (sheet end 10.73 to 10.76 in, at 200 and 600 dpi); a scanner that pads differently is simply
refused and the page keeps its full extent (`driver`). It is never the default.

**Support is three-state.** `scan devices --details` / `scan check --details` print, per item, `scanner detection unknown|absent|present`
and the longest page; JSON: `scanners[].page_length.{glass,feeder}.{device_detect, max_length_mils}`. `absent` means the item's
properties were read and none of the three is published; `unknown` means they could not be read. Each saved page reports `length`
(`{source, length_mils, length_inches, length_mm, approximate}`) in the CLI line, the `scan_acquire` result and the job journal.
The `device` path has only been exercised by unit tests over a fake property table, not on a scanner that publishes detection.

## Scan jobs

`pdfcraft-cli scan --out DIR --yes` runs the scan as one `craft_scan::job::ScanJob`, following the loop in craft-scan's
`docs/scan-job.md`: the main thread owns the job and polls; a worker thread runs `acquire` and gets only the plan's settings.
Pages are written to `DIR` as `record_progress` returns them, so a jam after twelve sheets leaves twelve files. The job decides
what may happen next: a busy scanner with nothing kept and an `Auto` source that finds the feeder empty are retried by the
command; everything else waits for a person.

**The journal.** `DIR/scan-job.json` is rewritten atomically (a temporary file in `DIR`, then a rename) at every state change:
when an attempt begins, when pages arrive, when it ends. It never holds page bytes.

```json
{
  "version": 1,
  "id": "scan-1760000000000-1a2b",
  "started": 1760000000000,
  "updated": 1760000012345,
  "request": { "device": "wia:...", "source": "feeder", "dpi": 200, "color": "gray", "pages": 20, "duplex": false, "reverse": false },
  "journal": { "version": 1, "title": "invoices", "state": "interrupted", "expected_pages": 20, "attempts_total": 1, "pages": [ ... ], "last_error": { ... }, "warnings": [] },
  "view": { "kind": "scan", "state": "needs_attention", "progress": { "done": 12, "total": 20 }, ... },
  "attempts": [ { "number": 1, "outcome": "interrupted", "pages_gained": 12, ... } ]
}
```

`request` is every scan setting the command line gave (optional ones only when set), so `--continue` can repeat them; the craft-scan
journal does not carry settings. `journal` is `ScanJobJournal`, `view` is `craft_job::JobView`, `attempts` is the attempt timeline
across every run of the job (the journal itself keeps only the count and the last error). Times are milliseconds since the Unix epoch.
A reader ignores fields it does not know and defaults ones an older writer lacked; a `version` newer than the reader's is refused with a
message to upgrade. The journal does not verify the page files: `scan jobs` and `scan job` count `page-NNN.*` in the folder and say
when the number differs from the journal, and `--continue` refuses to go on until they agree.

**Exit status.** 0 `completed`, 10 `needs_attention`, 11 `failed`, 12 `cancelled` (craft-scan's `ExitClass`); 1 is a failure of the
command itself (arguments, files). A job left `acquiring` (the process was killed) reads as `needs_attention`. The final stdout line
is JSON including the job view; details in [cli.md](cli.md).

**Ctrl-C.** There is no handler: std has none and a safe one needs a new dependency or `unsafe`. Interrupting the command stops it
where it is, with the journal as of the last change (`acquiring`). Sheets the scanner had already taken may be lost. Treat such a
job as an unknown outcome: look at the scanner, then `scan --continue DIR --append --i-checked-the-scanner --yes`.

**Continuing.** `scan --continue DIR` rebuilds the job with `ScanJob::continuing_from(settings, config, journal)` from the journal and
its `request`. When pages are kept it asks you to choose: `--append` scans only the remaining count, `--restart` moves the kept page
files to `DIR/discarded-N` first (nothing is deleted) and scans everything again. A job in `needs_attention` is refused until
`--i-checked-the-scanner` is given, because a duplicate acquire moves the carriage and pulls paper; the job never decides that for you.

### Scheduling

Use the operating system's scheduler (Task Scheduler on Windows, cron elsewhere); `pdfcraft-cli` has no scheduler of its own.

- One job per output folder. Give every run a fresh folder (for example one named for the date); the command refuses a folder that
  already has a journal, so a repeated schedule cannot overwrite the record of an earlier run.
- Run `pdfcraft-cli scan --out D:\scans\2026-10-14 --yes --source feeder --pages all` and look at the exit status: 0 is done; 10, 11 and 12
  are not, and the one JSON line on stdout and `scan jobs D:\scans` say why.
- Check later with `pdfcraft-cli scan jobs D:\scans --json` (or without `--json` for a line per job). `scan jobs` and `scan job` only read
  files and are safe to run from a monitoring task.
- Do not re-run a failed scheduled job blindly. `needs_attention` can mean the scanner took sheets and the pages were lost; running the
  same command again, or `--continue` in a script, pulls more paper through a scanner that may already have moved. A scripted
  `--continue` is only safe for a job in the `interrupted` state (a cause a person cured, with pages kept and a choice made), and
  `--i-checked-the-scanner` should be given by a person, not by the schedule.
- A scanner that is busy at the scheduled time is retried by the job itself (waiting, within a small budget); an empty feeder or a jam is not.
