# PdfCraft CLI

Command-line entry points live in the `pdfcraft-cli` crate (see the repository README for the full command list).

## Scan (`pdfcraft-cli scan`)

Scanning is Windows/WIA only in this version: on other platforms `pdfcraft-cli scan` exits with the line `scan: scanning is only supported on Windows (WIA) in this version`, and the `scan_devices` and `scan_acquire` tools return the same sentence. Other backends are future work behind the same seam.

Windows acquire uses WIA through `pdfcraft-wia` (the COM layer); the shared rules and the backend registry are the `craft-scan` crate from craft-libs. Device ids carry the backend prefix: `wia:<WIA id>`. See [hardware-parity.md](hardware-parity.md#windows-wia-scanner-acquire) for devices, feeder, WSD, and duplex behavior.

**Checking the scan stack (`pdfcraft-cli scan check`)**

`scan check` reports what the scan stack sees, so real hardware can be validated without scanning. Nothing is written unless `--out` is given, and nothing moves unless `--acquire` is given.

| Command | What it does |
|---------|--------------|
| `scan check` | Lists the WIA devices (id, name, port, connection). No device is opened. |
| `scan check --details` | Opens every device, read-only, and prints sources (glass, feeder, duplex), resolutions, colour modes, formats, bed and sheet sizes, and the document-handling state. |
| `scan check --device TEXT` | The same for exactly one device. TEXT is a device id (exact, `wia:<WIA id>`) or a unique part of the id, name or port; zero or several matches is an error. |
| `scan check --device TEXT --acquire [--out DIR]` | Also makes one 100 dpi grey acquire and reports format, size and byte count. The glass is used when the device has one, else the feeder. Prints a one-line warning to stderr first. The pixels are saved only into `--out`. `--acquire` without `--device` is refused. |
| `--json` | One line of JSON instead of text. Keys: `supported`, `mode` (`list`, `details`, `device`), `count`, `scanners` (the same rows as `scan devices`), `acquire` (`null`, or `{ok, source, requested_dpi, count, pages[{format, width, height, dpi, dpi_y, requested_dpi, bytes, path}]}`, or `{ok: false, error, error_info}`), `ok`, `problems`. |

Exit status is 1 when a device would not open, the acquire failed, or the arguments are wrong; the report is still printed first. On other platforms every `scan check` ends with `scan check: scanning is only supported on Windows (WIA) in this version`.

**Scanning as a job (`scan --out DIR --yes`, `scan jobs`, `scan job`, `scan --continue`)**

`scan --out DIR --yes ...` runs as one `craft-scan` `ScanJob` (see [scan-stack.md](scan-stack.md#scan-jobs)): pages are saved to `DIR` as they arrive, `DIR/scan-job.json` records the job, stdout gets one JSON line (the old keys `source`, `requested_source`, `reverse`, `count`, `pages`, plus `state`, `exit_class`, `exit_code`, `journal` and `job`, the serialised `craft_job::JobView`), and the exit status says how it ended.

| Exit status | Class | Meaning |
|-------------|-------|---------|
| 0 | `completed` | Every wanted page arrived (or the feeder emptied normally). |
| 10 | `needs_attention` | Interrupted (jam, cover, empty feeder with pages still wanted), finished short, or the outcome is unknown (link dropped, process cut off). A person looks first. |
| 11 | `failed` | Stopped by a failure nothing in the job can cure. |
| 12 | `cancelled` | Stopped on purpose, by this side or by the scanner. |
| 1 | | The command itself failed before any job state existed: wrong arguments, an unreadable folder, a folder that already holds a job. |

On a non-zero job exit the sentence goes to stderr as `scan: ...`, followed, when the job has an error, by the one JSON line `{"scan_error": {kind, phase, hresult, code_name, retryable, resumable, message}}`.

| Command | What it does |
|---------|--------------|
| `scan jobs [DIR...] [--json]` | Lists the `scan-job.json` journals in each `DIR` (default: the current folder and the folders directly below it; nothing deeper). One line each: id, state, pages kept / wanted, attempts, last error kind, updated (UTC), folder; a note when the page files on disk do not match the journal. `--json`: an array of the journal records plus `dir`, `state`, `exit_class`, `exit_code`, `page_files`, `page_mismatch`. Opens no scanner. |
| `scan job DIR [--json]` | One job in detail: the summary line, state and exit class, pages kept against page files on disk, the last error, warnings, the attempt timeline of every run, and the next step. Opens no scanner. |
| `scan --continue DIR (--append` or `--restart) --yes [--i-checked-the-scanner]` | Goes on with a job from its journal and request (no scan option may be given beside it). With pages kept, `--append` or `--restart` is required: `--append` keeps them and asks the scanner only for the missing pages; `--restart` first moves the kept page files (and a copy of the journal) to `DIR/discarded-N`, never deleting them, then scans everything again. A job whose outcome is unknown (`needs_attention`, including one whose process was killed mid-scan) is refused until you have looked at the scanner and give `--i-checked-the-scanner`: a repeated acquire moves the carriage and pulls paper, and sheets that were scanned but lost are not scanned again unless you put them back. It also refuses when the page files in `DIR` do not match the journal, a completed job, and a manual-duplex `--reverse` pass. |

`--page-length driver|size|auto|infer` chooses how the page length is decided (default `driver`: the scanner's own). `size` is the `--page-size` length as extents. `auto` uses the scanner's length detection when it publishes one, else the length of `--page-size`; `infer` adds a guess of the sheet end from the image when neither applies (feeder only, **approximate**). Each saved page reports `length.source` (`driver`, `device`, `size`, `inferred`). See [scan-stack.md](scan-stack.md#page-length).

One job per output folder: a fresh `scan --out DIR` refuses a folder that already holds a journal or page files (`--reverse` after a completed front pass in the same folder is the one exception). Ctrl-C is not handled (that needs a dependency or `unsafe`): terminating the process leaves the journal in state `acquiring`, which every reader treats as an unknown outcome.

**Diagnostics**

| Variable | Effect |
|----------|--------|
| `CRAFT_SCAN_TRACE` (older name `PDFCRAFT_SCAN_TRACE`, still read) | `1` or `true`: append one JSON object per acquire phase to stderr. Any other non-empty value is treated as a path and lines are appended to that JSONL file. Phases include `device_open`, `props_written`, `transfer_start`, `transfer_end`, and `status_read` (handling bits only). No device names, ids, ports, or image data. |
| `PDFCRAFT_SCAN_STATUS_LOG` | `1`: log each WIA transfer STATUS callback (`percent`, `transferred` bytes) to stderr during file transfer. |

Example (stderr trace during acquire):

```powershell
$env:CRAFT_SCAN_TRACE = "1"
pdfcraft-cli scan acquire --device "…" --source feeder --pages 1 --out-dir .\out
```

Manual check: run a short acquire with `CRAFT_SCAN_TRACE=1` and confirm phase lines appear on stderr in order through `transfer_end`.
