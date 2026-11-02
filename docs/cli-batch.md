# Batch conversion, OCR and compression

`pdfcraft-cli batch` runs the same steps over files or folders, one file at a time.
It writes separate PDF outputs and a JSON report to stdout. It does not change
source files or documents open in the editor.

```sh
# Compress all PDFs, including subfolders.
pdfcraft-cli batch scans --root . --recursive --steps optimize --out-dir compressed

# Convert images and text, then compress. Existing PDFs are accepted too.
pdfcraft-cli batch inputs --root . --recursive --steps convert,optimize --out-dir pdfs --ppi 150 --quality 80

# Add searchable text before compression. Requires the installed OCR models.
pdfcraft-cli batch scans --root . --recursive --steps ocr,optimize --out-dir searchable
```

Multiple input files or folders can be listed before the options. Directories are
read only one level deep unless `--recursive` is present. Paths within each input
folder are retained inside the output folder. Files encountered more than once
through overlapping selections run once; the first selection determines their
relative output path. Processing order is sorted by input path.

Steps are `convert`, `ocr` and `optimize`, each at most once. `convert`, when used,
must come first. Conversion uses the same supported extensions as Create PDF;
new engine importers become available here automatically. Without `convert`,
only `.pdf` inputs are processed. Unsupported extensions are reported as skipped.

- `optimize` uses Reduce File Size settings: JPEG quality 60, resampling to 150 ppi
  above 225 ppi (black-and-white images to 300 ppi above 450 ppi, stored as CCITT
  Group 4 when smaller), plus the existing document cleanup. `--ppi` (9–1600) sets the
  target and a threshold of 1.5 times that value; black-and-white images go to the
  larger of `--ppi` and 300 ppi, so a lower value keeps scanned text legible and a
  higher one keeps their resolution. `--quality` (1–100) sets JPEG quality.
  These settings can lose image detail. A candidate that is not smaller retains
  the bytes entering that step. This is not an images-only or lossless preset.
- `ocr` uses the installed engine and its supported languages. `--dpi` (72–600,
  default 300), `--language` (default `en`) and `--ocr-all-pages` configure it.
  By default pages already containing text are skipped. Missing models are
  reported as file errors; nothing is downloaded or installed by the batch.
- `--stop-on-error` stops processing after the first file error. By default the
  remaining files continue. Earlier successful outputs stay, and unprocessed
  files are listed as skipped. A file is published only after all its steps succeed.

Existing output names are never replaced, even by concurrent batch writers:
`book.pdf`, `book (2).pdf`, and so on. A temporary file in the destination folder
is published without overwriting. Sources are never moved or deleted. An output
folder inside an input folder is excluded from discovery, including on reruns.
The output folder cannot equal or contain a selected input folder.

The report contains `completed`, `failed`, `skipped`, `stopped` and `files`.
Each file records its status, output and step results, or the error/skip reason.
The `batch` command exits nonzero on any file failure while still printing the
report. Redirect stdout to retain it; keep the report outside the input folders.

## MCP and JSON scripts

The same implementation is available as `doc_batch`:

```json
{
  "paths": ["inputs"],
  "steps": ["convert", "optimize"],
  "recursive": true,
  "out_dir": "pdfs",
  "ppi": 150,
  "quality": 80,
  "stop_on_error": false
}
```

In `pdfcraft-cli run --script`, put this object in the `args` of a `doc_batch`
step. MCP marks a report with failed files as `isError: true` and retains the
structured report. Tool names and OCR settings match the existing automation API
(`skip_text_pages: false` corresponds to `--ocr-all-pages`).

## Boundaries

Discovery completes before writing. Invalid arguments, unreadable input folders
or paths outside `--root` refuse the call. Traversal accepts at most 1,000 selected
paths, 10,000 visited entries and 64 folder levels. Discovered symbolic links,
Windows reparse points and special files are skipped; selecting a link as a root
is refused. Individual input files are limited to 512 MiB, in addition to engine
limits. Signed documents cannot be optimized, and encrypted PDFs requiring a
password are not supported by this command.

This is a synchronous, one-shot job. It has no folder watcher, scheduler, resume
database, concurrent file processing or GUI progress dialog. Each file has its
own temporary engine session; completed documents are not retained in memory.
