# Hardware and platform parity

> **Last reviewed:** 2026-11-05 · **Last updated:** 2026-11-05 · **Change:** minor (Wayland tablet pen: the eraser end erases and pressure shapes the live freehand preview behind a preference; saved Ink stays fixed-width. Earlier: web printing through the browser's print dialog; shared craft-print spooler with passing Windows CI; Windows printers row and 1:1 touchpad scrolling, #756, #759) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

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
| Printers | all, PostScript, printer properties | CUPS via craft-print | Windows PDF raster spooler via craft-print (#756, PR #847) | CUPS via craft-print | the browser's print dialog (every printer the system knows): sheets drawn by PdfCraft at 150–300 dpi; copies, two-sided and colour set there | `crates/print/README.md`, `crates/ui-egui/src/workflows/web_print.rs` |
| Scanners (TWAIN, WIA, ICA) | yes | none | none | none (SANE) | n/a | `ocr.scanner-acquire` planned |
| Smart cards and USB tokens | yes (PKCS #11, CryptoTokenKit, CNG) | Keychain identities (software keys) | CNG store keys, with PIN prompts for cards (#742) | none (PKCS #11 planned) | none | `sign.pkcs11` planned |
| Pen and touch | Touch mode, pen ink, Windows Ink | trackpad pinch zoom; ink from pointer, no pressure | ink from pointer, no pressure or touch mode | ink from a Wayland tablet pen draws through the pointer pipeline and the pen's eraser end erases with the selected comment erase; pen pressure shapes the freehand ink's live preview behind a preference (Preferences ▸ Comment: on/off, minimum/maximum width, smoothing), and the saved Ink keeps the tool's configured fixed width; touch mode none | pointer | touchpad scrolling 1:1 (#759, PR #868); `vendor/README.md` winit patch 14; pen drawing live-confirmed (XP-Pen, niri); eraser erase, pressure preview and its knobs are covered by simulated tests only (fork/app machine tests, headless UI tests) — eraser end on-page erase and pressure preview live confirmation pending |
| Keyboard input methods (IME) | yes | yes; Japanese and Korean IME fixes patched into winit | yes (Microsoft Korean cursor fix) | X11 and Wayland IME fixes | browser | `vendor/README.md` winit patches 2–10 |
| Screen readers | VoiceOver, NVDA, JAWS | AccessKit labels (partial) | AccessKit (partial) | AccessKit (partial) | partial | `a11y.app-screen-reader` partial |
| Text to speech (Read Out Loud) | yes | none | none | none | none | `a11y.read-aloud` planned |
| Apple Silicon / ARM64 | native | native | ARM64 build (#61) | ARM64 builds | n/a | `release.yml` |

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
| 2026-11-05 | minor | Wayland tablet pen (Linux/FreeBSD): the eraser end erases and pressure shapes the freehand ink's live preview behind Preferences ▸ Comment knobs; the saved Ink stays fixed-width. Covered by simulated tests only — eraser end on-page erase and pressure preview need XP-Pen confirmation |
| 2026-10-11 | minor | Web printers row: printing through the browser's print dialog (gap 4 closed); Web platform row no longer lists printing |
| 2026-10-10 | major | Created from the code, `vendor/README.md`, `.github/workflows/release.yml`, the print crate's README and user issues |
| 2026-10-10 | minor | Windows printing shipped (#756): the Windows row's gap is closed; the web row keeps "printing" |
| 2026-10-11 | minor | Shared craft-print spooler; Windows preserves PDF sheet size/orientation and uses corrected duplex binding edges. Physical output remains a hardware check. |
| 2026-10-11 | minor | The Printers row's Windows cell still said none: now the Windows spooler (#756, PR #847). Touchpad scrolling 1:1 (#759, PR #868) |
