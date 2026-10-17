# Hardware and platform parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-13 · **Change:** minor (Windows WIA scanner acquire; earlier: web printing through the browser's print dialog; earlier: shared craft-print spooler with passing Windows CI; Windows printers row and 1:1 touchpad scrolling, #756, #759) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

Each hardware feature Acrobat Pro uses, per platform, ours against Acrobat's. Acrobat runs on macOS
and Windows (plus a limited web viewer/editor in Document Cloud); PdfCraft runs on macOS, Windows,
Linux, FreeBSD and the web. Gaps ranked in [gaps.md](gaps.md).

**Hardware ≈ 35% ready, 25–50 h** (additional to the features total; scanners, smart cards and
printing are counted in their feature areas). **Platforms ≈ 70% ready, 15–30 h.**

## Hardware

| Feature | Acrobat (macOS / Windows) | PdfCraft macOS | PdfCraft Windows | PdfCraft Linux / FreeBSD | PdfCraft web | Evidence |
|---|---|---|---|---|---|---|
| GPU page rendering | yes (Page Display ▸ GPU-accelerated rendering) | no: CPU raster, GPU for UI compositing (wgpu, Metal) | no (wgpu DX12, GL fallback) | no (wgpu Vulkan/GL) | no (WebGL/WebGPU UI) | `crates/render`; gap 20 |
| Multi-threaded rendering | yes | yes (render pool, tiles, watchdog) | yes | yes | single thread | `view.render-priority` |
| HiDPI / Retina | yes | yes | yes, per-monitor DPI v2 (#324, #725) | yes | yes | text blurry at 1080p/1440p (#730) |
| Multiple monitors | yes, multiple windows | one window | one window; surfaces fit the GPU limit (#577) | one window | n/a | `view.multiple-windows` planned |
| Printers | all, PostScript, printer properties | CUPS via craft-print | Windows PDF raster spooler via craft-print (#756, PR #847) | CUPS via craft-print | the browser's print dialog (every printer the system knows): sheets drawn by PdfCraft at 150–300 dpi; copies, two-sided and colour set there | `crates/print/README.md`, `crates/ui-egui/src/web_print.rs` |
| Scanners (TWAIN, WIA, ICA) | yes | none | WIA, Windows only: dialog, CLI and tools (`ocr.scanner-acquire`, partial) | none (SANE); reports unsupported | n/a | other backends are future work behind the same seam (`docs/scan-stack.md`) |
| Smart cards and USB tokens | yes (PKCS #11, CryptoTokenKit, CNG) | Keychain identities (software keys) | CNG store keys, with PIN prompts for cards (#742) | none (PKCS #11 planned) | none | `sign.pkcs11` planned |
| Pen and touch | Touch mode, pen ink, Windows Ink | trackpad pinch zoom; ink from pointer, no pressure | ink from pointer, no pressure or touch mode | as macOS | pointer | touchpad scrolling 1:1 (#759, PR #868) |
| Keyboard input methods (IME) | yes | yes; Japanese and Korean IME fixes patched into winit | yes (Microsoft Korean cursor fix) | X11 and Wayland IME fixes | browser | `vendor/README.md` winit patches 2–10 |
| Screen readers | VoiceOver, NVDA, JAWS | AccessKit labels (partial) | AccessKit (partial) | AccessKit (partial) | partial | `a11y.app-screen-reader` partial |
| Text to speech (Read Out Loud) | yes | none | none | none | none | `a11y.read-aloud` planned |
| Apple Silicon / ARM64 | native | native | ARM64 build (#61) | ARM64 builds | n/a | `release.yml` |

### Windows WIA (scanner acquire)

Windows/WIA only in this version: other platforms report unsupported (the dialog, the CLI and the tools say so), and other backends are future work behind the same seam.

`pdfcraft-cli scan devices` lists scanners without feeding paper and, by default, without
opening any device: each row is `{id, name, port, connection, details: "skipped"}`. `--device TEXT`
(id, name or port substring, case-insensitive; empty is an error) opens only the matching devices
and returns full rows (`details: "loaded"`: handling status such as feeder/flat ready, jam and
cover, feeder capacity when published, and per-source capabilities); `--details` opens every
device. A device that fails to open stays listed with `details: "error"` and the engine's `error`
sentence. The automation tool `scan_devices` takes the same `device` / `details` arguments, and the
output is `{count, scanners}` in both, plus `errors` (`[{backend, error}]`) when one backend failed while another answered; ids are `<backend>:<native id>` (`wia:<WIA id>`). `scan devices --dump` prints the backend's diagnostic text (for WIA, every published property).
The From scanner… dialog works the same way (on a platform without a backend it shows "Scanning needs Windows (WIA) in this version." instead of a device list): it lists every installed scanner without opening any
(a busy or offline one stays listed), then reads the capabilities of the selected device only, on a
background thread. If that fails, the engine's sentence is shown with Try again, and Scan still
works with conservative defaults.
Each device in the list carries a connection tag (USB, or Network for WSD; none when unknown). When
one printer is listed twice under the same name (a trailing ` [..]` id is ignored) over USB and the
network, each entry's tooltip says so, and with no remembered scanner the USB entry is selected first.
A remembered scanner is always kept.
Acquire (`scan acquire`, UI **Scan & OCR ▸ From scanner…**) transfers pages in memory;
partial results on failure surface as `ScanError::Incomplete` and the CLI can still write pages
that arrived before the error.

**Preflight:** jam, cover open, multi-feed, and lamp error bits from WIA status are checked
before Download with the sentences from the WIA backend's tables (`pdfcraft_wia::rules`; for example paper jam and cover open).

**Feeder “All pages” (`pages = 0`):** `ScanSettings.pages = 0` or `pdfcraft-cli scan --pages all
--source feeder` scans until the feeder is empty. The UI must not rewrite feeder `0` to `1` when
WIA lists `WIA_DPS_PAGES` as 1..N without zero (common on inbox WSD). Regression: `cargo test -p pdfcraft-ui-egui snap_choices`.

**Source Auto and an empty feeder:** Auto resolves to the feeder when WIA reports `FEED_READY`, but an empty
tray on WSD still reports it. The feeder transfer then fails after the printer's own ~10 s
timeout with `busy_or_feeder_empty` (or `feeder_empty`) and zero pages. In that case only (Auto, feeder resolved,
glass present, no page received) `acquire` retries once on the glass: one page, same settings, so the cost is
that ~10 s wait. `auto_fallback_to_glass` is the pure decision; an explicit Feeder, any received page, and a
jam, cover, multi-feed, lamp, attention or plain busy error never fall back. `CRAFT_SCAN_TRACE` (or the older `PDFCRAFT_SCAN_TRACE`) records an
`auto_fallback` phase (`from`, `to`, `reason`). `Acquired.source` and `ProgressToken::source()` report the source
actually used, and the dialog follows it.

**WSD:** network scanners (`wsdscdrv`) use file transfer (`IWiaTransfer::Download`), not banded
callback transfer. Whole-stack feeder jobs may need a large ticket page cap when the driver maps
`IPS_PAGES = 0` to a single sheet. `FEED_READY` is unreliable on some WSD stacks; acquire may
continue when the port is WSD and a multi-page job was requested.

**Duplex:** hardware duplex is `ScanSettings.duplex` (`FEEDER|DUPLEX`). Manual duplex is two
feeder passes plus `interleave_manual_duplex` / CLI `--reverse`. In the scan dialog (UI **Reverse sides**)
nothing is buffered: the fronts enter the document as they arrive, and in the second pass each back is
inserted right after its front (the stack is flipped as a whole, so the first back goes after the last
front). Cancelling, or never scanning the backs, leaves the fronts in the document. Manual duplex
keeps the fronts in one file (**Multiple files** is ignored for the first pass).

**Black and white:** a scanner with no native 1-bit mode (or a driver that falls back to an RGB24 ticket
after the device refuses the 1-bit one) answers a black-and-white request with 8- or 24-bit data, and a JPEG
request at black and white is always a 24-bit JPEG. Windows does not threshold it. For `Color::BlackAndWhite`
the scan crate (`bw.rs`) therefore turns any page deeper than 1 bit into a real 1-bit BMP (palette
`[black, white]`, bottom-up) after it arrives: BT.601 luminance below the request's threshold (128 when it
carries none, the value clamped to 0..=255) is ink, the rest is paper. Pages that are already 1-bit,
and gray and color requests, are untouched. BMP (4, 8, 24, 32 bit) is read directly; JPEG, PNG, TIFF and the
other BMP forms go through the `image` decoders, and are delivered as the 1-bit BMP (`Page.format` is `bmp`).
`Page.thresholded` is true for converted pages (`thresholded` in the CLI page JSON), and `CRAFT_SCAN_TRACE` records a `bw_software_threshold` phase (`index` zero-based,
`from_bits`, `to_bits`, `threshold`). The WIA threshold property is still written to the device; a device
whose threshold range is not 0..=255 sees the same number in software.

**Imaging quirks:** if glass acquire fails with missing bed size (`0x80070057`), remove the device
in Windows **Devices and Printers** and **re-Add** it while the scanner stays on — reconnect alone
may not rebuild an empty flatbed item. Set `PDFCRAFT_SCAN_STATUS_LOG=1` to log transfer STATUS
during a job.

## Platforms

| Platform | Acrobat | PdfCraft | Packaging | Gaps |
|---|---|---|---|---|
| macOS | yes | yes | signed and notarized DMG | — |
| Windows (x64, x86, ARM64) | yes (x64, ARM64) | yes | signed installers | — |
| Linux | no | yes | AppImage, deb, rpm, Flatpak, tarball | Chinese faces in Flatpak (#689) |
| FreeBSD | no | yes | package | — |
| Web | Acrobat online (cloud) | yes (WASM, offline in the browser) | static site | OCR, crash recovery, PKCS #12 signing, hosted deployment |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-11 | minor | Web printers row: printing through the browser's print dialog (gap 4 closed); Web platform row no longer lists printing |
| 2026-10-10 | major | Created from the code, `vendor/README.md`, `.github/workflows/release.yml`, the print crate's README and user issues |
| 2026-10-10 | minor | Windows printing shipped (#756): the Windows row's gap is closed; the web row keeps "printing" |
| 2026-10-11 | minor | Shared craft-print spooler; Windows preserves PDF sheet size/orientation and uses corrected duplex binding edges. Physical output remains a hardware check. |
| 2026-10-11 | minor | The Printers row's Windows cell still said none: now the Windows spooler (#756, PR #847). Touchpad scrolling 1:1 (#759, PR #868) |
| 2026-10-13 | minor | Scanners: Windows WIA acquire is partial (`ocr.scanner-acquire`); other platforms report unsupported; TWAIN / ICA / SANE still open |
