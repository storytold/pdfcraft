<p align="center">
  <img src="assets/app-icon/pdfcraft.svg" width="96" height="96" alt="Linkco PDF Editor logo" />
</p>

<h1 align="center">Linkco PDF Editor</h1>

<p align="center">
  <strong>Professional PDF tools for Linkco (Al Rawabet Commercial Services &amp; Contracting Company W.L.L.)</strong><br />
  <sub>Doha, State of Qatar · C.R. No.: 32942 · ISO 9001, 14001 &amp; 45001 Certified · <code>www.linkco.com.qa</code></sub>
</p>

---

## Overview

**Linkco PDF Editor** is a native desktop PDF reader, editor, and document processing suite developed for **Linkco** (**Al Rawabet Commercial Services and Contracting Company W.L.L.** — `www.linkco.com.qa`, Doha, State of Qatar). Written entirely in safe Rust with zero `unsafe` blocks across the workspace, it provides a fast, offline-first environment for viewing, editing, organizing, converting, signing, redacting, and validating PDF documents across engineering, contracting, facility maintenance, and commercial workflows.

All document rendering and editing operations execute locally on your workstation without requiring an account, cloud upload, or telemetry connection.

---

## Features

- **Document Viewing & Navigation** — Continuous, single-page, two-up, and cover-page layouts; smooth zoom and pan; page thumbnails; hierarchical bookmarks; Optional Content Group (OCG) layer controls; file attachments; article threads; and side-by-side synchronous document comparison.
- **Page Organization** — Interactive thumbnail grid for rotating, reordering, deleting, extracting, duplicating, inserting, replacing, cropping, and splitting pages (by page count, file size, or top-level bookmarks), plus multi-file PDF combining.
- **Direct Content Editing** — Reflow-aware paragraph and line text editing, new text blocks, image insertion and replacement, headers and footers, watermarks, backgrounds, Bates numbering, link editing, and vector object inspection.
- **Annotations & Markup** — Sticky notes, highlights, underlines, strikethroughs, squiggly lines, free-text callouts, ink drawings, stamps, polygons, polylines, rectangles, ellipses, lines, arrows, carets, redaction marks, and XFDF comment import/export.
- **Interactive Forms & E-Sign** — Full AcroForm field filling and authoring (text, checkbox, radio button, combo box, list box, push button, signature field, barcode), sandboxed `AF*` calculation/validation scripts, and **Fill & Sign** tools (text, checkmarks, crosses, dots, lines, dates, drawn/typed/image signatures, and initials).
- **Digital Signatures & Certificates** — PKCS#7 / CMS and PAdES (`B-B`, `B-T`, `B-LT`, `B-LTA`) digital signature verification and signing, X.509 chain validation, RFC 3161 timestamping, and DocMDP modification detection.
- **Security & True Redaction** — Password encryption (AES-256, AES-128, RC4) and permission enforcement, content-stream glyph and image redaction with verifiable byte removal, metadata scrubbing, and hidden-data sanitization.
- **Export & Conversion** — Export PDFs to Microsoft Word (`.docx`), PNG images, extracted embedded images, HTML web pages, Rich Text Format (`.rtf`), and plain text (`.txt`), or create PDFs from images, plain text, HTML, or blank page templates.
- **High-Resolution Printing & Windows Print Spooler Integration** — Full imposition engine (Fit, Actual Size, Shrink, Custom Scale, Multiple pages per sheet with Cut & Stack, Saddle-Stitch Booklet, and Tiled Poster with cut marks), live high-DPI sheet preview, 150 / 300 / 600 DPI print rendering, and native Windows Print Spooler (`Win32_Printer` / `.NET` `System.Drawing.Printing`) and CUPS printer detection with default-printer and offline-status awareness.
- **Windows File Explorer PDF Preview Handler** — Out-of-process `IPreviewHandler` shell extension (`LinkcoPdfPreviewHandler.dll`) hosted by `prevhost.exe` that renders PDF pages directly inside the Windows 10/11 File Explorer Preview Pane with page navigation and zoom without launching the full editor.
- **Scan & OCR** — Pluggable Optical Character Recognition (OCR) pipeline with page deskew, background cleanup, and invisible searchable text layer (`Tr 3`) generation.
- **Print Production, Standards & Accessibility** — PDF/A, PDF/X, and PDF/UA validation; color-separation and ink-coverage preview; transparency flattening; hairline fixing; color space conversion; page box (`TrimBox`, `BleedBox`, `ArtBox`, `CropBox`) editing; printer marks; and Matterhorn accessibility checks with reading-order and structure-tag editors.

---

## Recommended Tools

The Home dashboard provides one-click access to the core tools implemented in the workspace (`crates/ui-egui/src/home.rs`):

| Tool | Description | Primary Commands |
| :--- | :--- | :--- |
| **Organize pages** | Page grid · Rotate · Delete | Opens the interactive page-organization grid (`organize`) |
| **Edit a PDF** | Edit text & images · Add text | Activates direct PDF content editing (`edit`) |
| **Combine files** | Merge PDFs · Reorder · Insert | Combines multiple PDF documents into a single file (`page.combine`) |
| **Compress a PDF** | Reduce file size · Optimize PDF | Deduplicates streams, subsets fonts, and optimizes file size (`file.reduce_size`) |
| **Export a PDF** | Word · Image · HTML · Text | Exports the active document to `.docx`, `.png`, `.html`, `.rtf`, or `.txt` (`export`) |
| **Scan & OCR** | Recognize text · Searchable PDF | Runs optical character recognition to generate a searchable text layer (`ocr.recognize`) |
| **Fill & Sign** | Fill form fields · Sign document | Places text, checkmarks, dates, initials, and signatures on pages (`fill_sign`) |
| **Protect a PDF** | Password security · Sanitize | Applies password encryption, permissions, or document sanitization (`protect`) |
| **Open file** | Browse local filesystem | Opens one or more PDF documents from disk (`file.open`) |

---

## Screenshots

### Home Dashboard & Recommended Tools

![Linkco PDF Editor — Home Dashboard](docs/images/linkco-home-dashboard.png)

### Organize Pages View

![Linkco PDF Editor — Organize Pages](docs/images/linkco-organize-pages.png)

---

## System Requirements

| Platform | Minimum Requirements |
| :--- | :--- |
| **Operating System** | Windows 10 / 11 (x64 or ARM64), macOS 12+ (Apple Silicon or Intel), or Linux (x86_64 / aarch64 with X11 or Wayland) |
| **Processor** | 64-bit dual-core CPU (quad-core recommended for concurrent rendering and OCR) |
| **Memory** | 4 GB RAM minimum (8 GB RAM recommended for large multi-hundred-page documents) |
| **Graphics** | OpenGL 3.3+, Direct3D 11/12, Metal, or Vulkan compatible GPU / software rasterizer |
| **Disk Space** | 100 MB for installed application binaries |
| **Build Toolchain** | Rust 1.85+ (`edition = "2024"`, validated on Rust 1.92) |

---

## Installation

### From a Packaged Installer

1. Build or obtain the native installer for your platform from `dist/release/`:
   - **Windows (NSIS Setup EXE):** `LinkcoPDFEditorSetup.exe`
   - **Windows (MSI Package):** `LinkcoPDFEditorSetup-<version>-windows-<arch>.msi`
2. Run the installer and follow the on-screen prompts.
3. Launch **Linkco PDF Editor** from the Windows Start Menu, Desktop shortcut, or by opening any `.pdf` file.

### From Source

1. Install the Rust toolchain via [rustup](https://www.rust-lang.org/tools/install).
2. Clone this repository and build the release binaries using `build.py` (or `cargo`):

```bash
git clone https://github.com/b-lincko/linkco-pdf.git
cd linkco-pdf
python build.py
```

---

## Windows Installer

The Windows packaging pipeline lives in `packaging/windows/` and embeds full Linkco product metadata into both the executable (`apps/pdfcraft/build.rs`) and the installers:

- **Product Name:** `Linkco PDF Editor`
- **Publisher / Company Name:** `Al Rawabet Commercial Services & Contracting Company W.L.L.`
- **Internal Name:** `LinkcoPDFEditor`
- **Original Filename:** `LinkcoPDFEditor.exe`
- **Application Icon:** `assets/app-icon/pdfcraft.ico`
- **Shortcuts Created:**
  - Start Menu: `Linkco PDF Editor`
  - Desktop: `Linkco PDF Editor`
- **Windows File Explorer Preview Pane (`IPreviewHandler`):** Installs and registers `LinkcoPdfPreviewHandler.dll` (`CLSID {D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}`) for `.pdf` files, backing up any previously registered preview handler and restoring it cleanly on uninstall.
- **Installed Apps Registration:** Registers **Linkco PDF Editor** in Windows *Installed Apps / Add or Remove Programs* with version, icon, publisher, and clean uninstaller support, and registers `.pdf` under *Open With* without overriding the user's default PDF handler.

### Building the Windows Installer

On Windows (with Python 3 and the Rust toolchain installed), run `installer.py`:

```bash
python installer.py
```

Or from PowerShell using WiX Toolset v5 (`wix`) and optionally NSIS (`makensis`):

```powershell
pwsh -File packaging/windows/package.ps1
```

This builds the Windows release binaries and produces in `dist/release/`:
- `dist/release/LinkcoPDFEditorSetup.exe` (standalone Windows GUI installer — built via NSIS `makensis` when installed, or automatically via Windows' built-in `.NET` `csc.exe` compiler)
- `dist/release/LinkcoPDFEditorSetup-<version>-windows-<arch>.msi` (when WiX v5 is installed; validated by `packaging/windows/test-msi.ps1`)
- `dist/release/LinkcoPDFEditor-<version>-windows-<arch>-portable.zip` (portable ZIP containing `LinkcoPDFEditor.exe`, `pdfcraft.exe`, and `pdfcraft-cli.exe`)

---

## Usage

### Desktop GUI

Launch the desktop editor directly or pass one or more PDF files on the command line:

```bash
# Open the Home dashboard
cargo run --release -p pdfcraft

# Open specific PDF files at a target page
cargo run --release -p pdfcraft -- --page 1 document.pdf
```

### Command-Line Interface (`pdfcraft-cli`)

The companion CLI binary supports headless inspection, text extraction, page rendering, encryption, optimization, and batch processing:

```bash
# Inspect document metadata, page count, and PDF version
cargo run --release -p pdfcraft-cli -- info document.pdf

# Render pages to PNG images at 150 DPI
cargo run --release -p pdfcraft-cli -- render document.pdf --dpi 150 --out page-1.png

# Generate a single-page preview image with structured status output (used by the Windows Preview Handler)
cargo run --release -p pdfcraft-cli -- preview document.pdf --page 1 --dpi 150 --out preview.png

# Extract plain text from a PDF
cargo run --release -p pdfcraft-cli -- text document.pdf
```

---

## Development

### Prerequisites

- Rust toolchain (`rustc` and `cargo` supporting Rust 2024 edition)
- On Linux, standard GUI development headers (`libxcb`, `libxkbcommon`, `libwayland`, `libfontconfig`, `libasound2`)

### Useful Workspace Commands

```bash
# Verify asset attribution and regenerate ATTRIBUTION.md
cargo xtask assets

# Run repository engineering gates (no unsafe, no panics, licence & metadata checks)
cargo xtask gates

# Verify workspace version consistency across packaging manifests
cargo xtask version

# Verify feature parity matrix against engine commands
cargo xtask parity
```

---

## Build

```bash
# Build release binaries into dist/release/ using the Python build script
python build.py

# Build the Windows application + Windows Setup installer (LinkcoPDFEditorSetup.exe)
python installer.py

# Or build directly with Cargo
cargo build --release -p pdfcraft -p pdfcraft-cli
```

Compiled binaries are placed in `dist/release/LinkcoPDFEditor.exe` (`dist/release/LinkcoPDFEditor` on Linux/macOS) as well as `target/release/pdfcraft` and `target/release/pdfcraft-cli`.

---

## Testing

Run the workspace unit and integration test suites:

```bash
# Run all workspace library and integration tests
cargo test --workspace

# Run UI integration tests (home dashboard, links, pickers, keyboard shortcuts)
cargo test -p pdfcraft-ui-egui

# Run engine unit tests (catalog, commands, links, redaction, editing)
cargo test -p pdfcraft-engine

# Run clippy lints across all targets
cargo clippy --workspace --all-targets -- -D warnings
```

---

## Project Structure

```text
linkco-pdf/
├── apps/
│   ├── pdfcraft/          # Desktop GUI binary (Linkco PDF Editor) & Windows resource script
│   ├── pdfcraft-cli/      # Headless CLI, batch runner, and inspection tool
│   └── pdfcraft-web/      # WebAssembly browser application target
├── crates/
│   ├── core/              # PDF 1.7 / 2.0 parser, xref/object model, writer, and encryption
│   ├── render/            # Display list compiler, 2D software rasterizer, fonts, and color
│   ├── layout/            # Text extraction, reading order, and full-text search
│   ├── forms/             # AcroForm fields, appearance generation, and XFA support
│   ├── annot/             # 18 PDF annotation types and XFDF import/export
│   ├── sign/              # PKCS#7 / CMS, PAdES, X.509 certificates, and RFC 3161 timestamps
│   ├── ocr/               # Pluggable OCR engine, deskew, and searchable PDF layer builder
│   ├── compliance/        # PDF/A, PDF/X, PDF/UA validation and Matterhorn accessibility
│   ├── scripting/         # Sandboxed QuickJS runtime for Acrobat form scripts
│   ├── engine/            # High-level document session, command dispatcher, and tool catalog
│   └── ui-egui/           # Immediate-mode desktop UI, Home dashboard, dialogs, and i18n
├── assets/
│   ├── app-icon/          # Linkco PDF Editor application icons (.svg, .ico, .icns, .png)
│   ├── fonts/             # Bundled UI and PDF base fonts (Inter, Liberation, Noto)
│   └── icons/             # Bundled Lucide UI icons (ISC licence)
├── docs/
│   └── images/            # Application screenshots used in documentation
├── packaging/
│   ├── windows/           # Windows WiX (.wxs), NSIS (installer.nsi), and PowerShell scripts
│   ├── macos/             # macOS .app / .dmg / .pkg packaging scripts
│   ├── linux/             # Linux .deb, .rpm, AppImage, Flatpak, and tarball scripts
│   └── freebsd/           # FreeBSD packaging scripts
└── xtask/                 # Build verification, asset attribution, and parity tasks
```

---

## Security & Privacy

- **100% Offline Operation:** Linkco PDF Editor processes all PDF files locally. It contains no analytics, telemetry, crash-reporting beacons, advertisements, or background network calls.
- **Memory-Safe Architecture:** `#![forbid(unsafe_code)]` is enforced across the workspace, eliminating buffer overflows and memory corruption vulnerabilities when parsing untrusted PDF files.
- **External Link Protection:** Clicking a link inside a PDF document never opens a browser or executes a local file path silently; only `https://`, `http://`, and `mailto:` schemes are permitted, and every external URL requires explicit user confirmation in a modal dialog showing the full destination address.
- **Sandboxed Scripting:** Document JavaScript (`crates/scripting`) executes inside an isolated, memory- and instruction-capped QuickJS context with zero filesystem or network access.

---

## License

Dual-licensed under either of:

- **Apache License, Version 2.0** ([`LICENSE-APACHE`](LICENSE-APACHE))
- **MIT License** ([`LICENSE-MIT`](LICENSE-MIT))

at your option. Third-party font and icon attributions are cataloged in [`ATTRIBUTION.md`](ATTRIBUTION.md).

Copyright © Al Rawabet Commercial Services & Contracting Company W.L.L. (Linkco)  
Building 159, Street 220, Zone 24, P.O. Box 32282, Doha – Qatar · Phone: +974 4437 2511 · Email: `info@linkco.com.qa` · Web: `www.linkco.com.qa`
