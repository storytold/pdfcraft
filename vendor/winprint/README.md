# winprint

[![crates.io](https://img.shields.io/crates/v/winprint.svg)](https://crates.io/crates/winprint)
[![Released API docs](https://docs.rs/winprint/badge.svg)](https://docs.rs/winprint)
[![BSD 3 Clause licensed](https://img.shields.io/badge/license-BSD%203%20Clause-blue)](./LICENSE.md)

A crate for printing to a Windows printer device using Windows API.

## About Safety
This crate interfaces with several Windows API functions, necessitating the use of unsafe code blocks. Nevertheless, this crate is designed to be safe and sound to use. If you find any case that will break safety or soundness, please report it as a bug.

## Development
### Testing on Windows
Just run `cargo test` on a Windows machine, null printer will be automatically created and used for testing. The tests cover basic printer operations like getting printer information, writing to the printer, etc.

Three visual regression tests also run under the `test-utils` feature (plus
`pdfium` for the PDF test):

- `tests/visual_regression_pdfium.rs` — `PdfiumPrinter`
- `tests/visual_regression_xps.rs` — `XpsPrinter`
- `tests/visual_regression_image.rs` — `ImagePrinter`

Each prints the matching origin file from
`test_data/visual_regression/<kind>/origin.*` through a virtual Microsoft PWG
Raster printer, decodes the PWG bitstream, and SSIM-compares each page
against the corresponding `expected.tiff` rendered by a deliberately
**different** renderer (Poppler for PDF, libgxps for XPS, ImageMagick for
Image). That second-source property means a shared regression between
renderer and baseline would have to coincidentally produce matching pixels
for the test to silently pass. Set `WINPRINT_DUMP_DIR=<path>` to have failing
tests dump the actual/expected page PNGs for inspection.

Day-to-day development never needs to regenerate these fixtures — the repo
ships them and `cargo test` consumes them as-is. If you do need to rebuild
them (e.g. after tweaking the source drawing or bumping a baseline renderer),
run `python utils/gen_visual_regression_data/run.py`; see the README in that
directory for tool prerequisites and determinism guarantees.

For testing with multiple features in one command, [`cargo hack`](https://github.com/taiki-e/cargo-hack) may be used for convenience.

### Testing on Linux Host
Since this crate is designed for Windows, it cannot be directly tested on a Linux host. However, a script is provided to run the test suite inside a Windows 11 VM on a Linux host.

**Prerequisites:** Docker with BuildKit, KVM support (`/dev/kvm`), and sufficient disk space & memory for the VM.

```bash
./utils/test-in-windows-vm/run.sh
```

The script will spins up a Windows VM and then run the test suite inside it. On first run it initializes the VM (very slowly). Subsequent runs reuse the cached VM state (relatively fast).

To clear the cached VM state, run with `--destroy` flag:

```bash
./utils/test-in-windows-vm/run.sh --destroy
```

## License
Licensed under [BSD 3 Clause](./LICENSE.md)

### Contribution
Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this crate by you, shall be licensed as BSD 3 Clause, without any additional terms or conditions.

## Craft Apps maintenance branch

This branch retains the upstream BSD-3-Clause licence and copyright notices.
It is based on ArcticLampyrid/winprint.rs v0.2.1, commit
`406ed733f15d60403d8d029035c807b5d8d33d31`.

The narrow changes used by craft-print are:

- DXGI jobs retain the terminal package HRESULT, report failed/canceled states, and
  time out after two minutes waiting for a missing terminal notification after Close.
  An unsuccessful job requests cancellation; the status connection is unadvised
  before its COM apartment is released. The timeout does not bound calls into a
  printer driver or an interactive output-file dialog.
- `WinPdfPrinter::print_with_progress(path, ticket, callback)` reports `(processed,
  total)` after each successful Direct2D AddPage, preserving one job and its ticket.
  These counts measure page processing, never physical printing or terminal success.
  Existing `FilePrinter::print` remains a wrapper without progress.
- Completion regression tests cover a late failed HRESULT, cancellation, inconsistent
  completed HRESULT, missing notification, and repeated terminal notifications.

The shared completion state uses reference-counted Rust synchronization rather than
an event HANDLE borrowed from the callback's lifetime. Applications must still check
that printing's final Result is successful, even when progress reached its total.
