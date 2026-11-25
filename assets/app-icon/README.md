# PdfChocolate app icon

<img src="pdfchocolate-512.png" alt="PdfChocolate app icon: a pixel-art chocolate bar in a red wrapper on a beige tile" width="128">

A chocolate bar, half unwrapped from a red wrapper with a gold band, as pixel art on a 192-dot grid
(3 dots per 8 px at 512 px), on a beige rounded tile. Licence: [LICENSE.txt](LICENSE.txt)
(`MIT OR Apache-2.0`, like the repo).

## Files

| File | What it is |
|---|---|
| `pdfchocolate-512.png` | the master; every file below is rendered from it |
| `pdfcraft.ico` | Windows app icon (16–256 px), resource 1 of `pdfchocolate.exe` (`apps/pdfcraft/build.rs`) |
| `pdfcraft-document.ico` | Windows PDF file icon (16–256 px), resource 2: a page with a folded corner, the chocolate bar and a red PDF label |
| `pdfcraft-1024.png` | 1024 px; the runtime Dock icon on macOS |
| `pdfcraft.icns` | macOS app bundle icon (PNG entries, 16–1024 px) |
| `pdfcraft.svg`, `pdfcraft-small.svg` | the pixel grid as SVG rectangles, pixel-exact at 512 px; the macOS DMG background shows `pdfcraft.svg` |
| `hicolor/<n>x<n>/apps/io.github.latechocolate.pdfchocolate.png` | 16–512 px; the 256 px one is the runtime window icon on Windows and Linux |
| `hicolor/scalable/apps/io.github.latechocolate.pdfchocolate.svg` | same SVG, for the Linux icon theme |

The file names keep their upstream `pdfcraft` names so the build and packaging scripts need no changes.

Windows PDF files show resource 2 (`"pdfchocolate.exe",-2`) once PdfChocolate is the default PDF app;
the program itself keeps resource 1.

## Regenerate

```powershell
powershell -File packaging/icons.ps1   # Windows; System.Drawing only
```

Then update the sha256 values in `ATTRIBUTION.toml` and run `cargo xtask assets --write`.
