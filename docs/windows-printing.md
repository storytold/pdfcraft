# Windows print jobs rejected before submission

A report on PdfCraft 0.19.0 described the preview closing without output while Edge printed the same PDF. On the investigated Windows machine, the HUAWEI CV81Z driver accepted the PrintTicket (paper, grayscale, duplex and collation), and its capabilities XML parsed successfully.

The native submission test failed before queueing: `StorageFile::GetFileFromPathAsync` returned `0x80070005` for the temporary PDF. Ordinary desktop file access could read it. This isolates one reproducible failure; it does not establish the cause of every report of an empty print queue.

The winprint loader uses `CreateRandomAccessStreamOnFile` with read-only access, then calls `PdfDocument::LoadFromStreamAsync`. The stream is backed by the job file and reads on demand. There is no whole-job `InMemoryRandomAccessStream` copy, `DataWriter`, transfer loop or per-chunk `StoreAsync().join()`. The path is made absolute and embedded NUL characters are rejected before calling the native API. The temporary file remains alive throughout rendering and submission through craft-print's existing lifetime management. PDF decoding and rendering can still allocate memory; the change removes the loader's full-file copy, not all printing memory use.

Upstream loader: https://github.com/storytold/winprint.rs/pull/15 (`03a4fb63`). Dependency follow-up: https://github.com/storytold/craft-libs/pull/636 (`e6489d92`, includes merged #605). PdfCraft updates only its normal `craft-print` dependency. There is no global patch or personal-fork source. The loader revision descends from `64e7afd4`, preserving the multi-threaded Direct2D factory fix for #5006. Upstream reviews are still required before release.

The public `craft_print::rgb` GDI transport remains pinned to main's `64e7afd4` revision. It is independent of the internal PDF-loader pin `03a4fb63`: retaining the public source identity avoids incompatible Rust types when a downstream application also uses that GDI revision directly. PdfCraft resolves one GDI package at that stable revision.

## Evidence

- Original Windows loader: synthetic A4 submission failed at PDF loading with `0x80070005`. The earlier memory-stream prototype submitted the same HUAWEI job; physical output was not independently confirmed.
- Final file-backed loader: five focused tests pass, covering a hidden PDF, read-only stream behavior, reading the final byte of a 256 MiB file, a missing file, and a path containing NUL.
- The hidden-file test was observed failing with the original StorageFile loader. The read-only test was observed failing with the earlier in-memory implementation and passing with the final loader.
- Isolated craft-print suite with the final loader and multi-threaded Direct2D source: 63 passed, 3 hardware tests ignored. These diagnostic tests are not a full build of the final dependency graph; the disabled PDFium downloader build dependencies were omitted locally.
- The file-backed loader at `b334ebbb` was subsequently submitted natively to the HUAWEI CV81Z; the reporter confirmed physical output of the synthetic A4 page. This used an isolated diagnostic workspace and writable TEMP directory, not the packaged UI. See https://github.com/storytold/winprint.rs/pull/15#issuecomment-6704470691.
- The current loader pin `03a4fb63` also includes the review repairs: standard-library NUL validation, GetPage(0) before document/file cleanup, RAII temporary-file cleanup, and the explicit WinRT feature. In a complete Windows checkout with normal build dependencies, five loader tests and 14 Print Schema tests passed; injected failures left no temporary files behind.
- Full final-graph build/quality gates must be rerun for the updated pins. Packaged-UI and wider printer/duplex qualification remain unverified; single-page output does not establish those.

When reporting a different failure, include the PdfCraft version, printer/driver name, displayed error and whether a job appears in the Windows queue. Saving the print-ready PDF and printing it in another viewer remains a workaround until the fix ships.
