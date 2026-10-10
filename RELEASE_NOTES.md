# Linkco PDF Editor v0.5.0 — Release Notes

**Product:** Linkco PDF Editor  
**Version:** `0.5.0`  
**Publisher:** Al Rawabet Commercial Services & Contracting Company W.L.L. (Linkco)  
**Location:** Building 159, Street 220, Zone 24, P.O. Box 32282, Doha – State of Qatar (`www.linkco.com.qa`)

---

## Highlights in v0.5.0

### 1. Official Linkco Brand Identity & Enterprise Home Dashboard
- Rebranded desktop application, CLI, WebAssembly target, and installers to **Linkco PDF Editor** by **Al Rawabet Commercial Services & Contracting Company W.L.L. (Linkco)**.
- Designed multi-resolution application icons (`.svg`, `.ico`, `.icns`, and 16 px–1024 px `.png`) featuring the Linkco 4-node connected network emblem on deep navy (`#01131C`) and crimson red (`#F22424`).
- Updated the **Home** welcome view (`crates/ui-egui/src/home.rs`) with the Linkco corporate header banner, 8 verified **Recommended Tools** (`Organize pages`, `Edit a PDF`, `Combine files`, `Compress a PDF`, `Export a PDF`, `Scan & OCR`, `Fill & Sign`, `Protect a PDF`), an `Open file` card, `View all tools →` navigation to all 19 tool groups, pinned folders, recent files list (`File ▸ Open Recent`), session tab restoration (`Reopen last session`), and command palette search (`Ctrl+K` / `⌘K`).
- Removed all legacy branding, community chat links, and third-party promotional links.

### 2. Windows File Explorer PDF Preview Handler (`IPreviewHandler`)
- Added `LinkcoPdfPreviewHandler.dll` (`packaging/windows/PreviewHandler.cs`, CLSID `{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}`), an out-of-process Windows Shell `IPreviewHandler` COM server hosted by `prevhost.exe` (`{6d2b5079-2f0b-48dd-ab7f-97cec514d30b}`).
- Added `pdfcraft-cli preview` (`apps/pdfcraft-cli/src/main.rs`) for fast single-page preview rendering with structured status reporting (`OK`, `PASSWORD_REQUIRED`, `EMPTY_PDF`, `INVALID_PDF`, `RENDER_ERROR`, `IO_ERROR`).
- Supports multi-page navigation (`Prev` / `Next`, `PgUp` / `PgDn`, `Home` / `End`), zoom (`−` / `+` / `Fit`, `Ctrl`+Wheel, click-drag pan), Windows Explorer Light/Dark theme adaptation, and non-destructive registration that backs up and restores any previously registered PDF preview handler on uninstall.

### 3. Windows Printer Detection, Native Driver Properties & High-Resolution Printing (150 / 300 / 600 DPI)
- Implemented 3-tier Windows printer enumeration in `crates/print/src/spool.rs`:
  1. WMI/CIM `Win32_Printer` (`Get-CimInstance` / `Get-WmiObject`)
  2. `.NET` `System.Drawing.Printing.PrinterSettings::InstalledPrinters`
  3. Windows Registry `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Devices` & `Windows /v Device`
- Detects local, USB, TCP/IP, WSD, and UNC network printers (plus driverless `lpstat -e` queues on Unix/CUPS), identifies the system default printer, and reports printer availability (`Ready`, `Printing`, `Paused`, `Offline`, `Error`) with an in-dialog refresh button and offline status warning.
- Added native printer driver `Properties…` (`rundll32 printui.dll,PrintUIEntry /e /n <printer>` on Windows; CUPS `lpoptions -l` PPD option editor on Unix).
- Upgraded Print dialog preview sharpness (`crates/ui-egui/src/canvas.rs`, `PRINT_PREVIEW_W = 860.0`, per-page thumbnail scaling) and added high-DPI sheet rendering (`Session::render_print_sheets` at 150 / 300 / 600 DPI, default 300 DPI) for crisp Windows print output via `.NET` `PrintDocument` with `HardMarginX/Y` compensation.

### 4. Content Editing, Multilingual Typography, Digital Signatures & Forms (`v0.4.0`–`v0.5.0`)
- **Direct Content Editing & Scripts (`crates/edit`, `crates/fonts`):** Multi-stream paragraph editing, grouped Form XObject figure selection and editing, Arabic shaping (`harfrust`) and bidirectional (`unicode-bidi`) Type 3 text rendering, Cyrillic/Greek/WinAnsi `0x80–0x9F` encoding, and Japanese CID/katakana font fallback.
- **Digital Signatures (`crates/sign`):** Native Windows Certificate Store (`CNG` via `rustls-cng`) signing alongside macOS Keychain and `.p12`/`.pfx` files, BER PKCS#12/PKCS#7 parsing, expanded RSA/PSS/ECDSA/Ed25519 algorithms, RFC 3161 `/DocTimeStamp` signature verification, and strict X.509 chain validation.
- **Forms, XFA & Fill & Sign (`crates/forms`, `crates/xfa`, `crates/ui-egui`):** Image signatures with automatic white-background removal and live corner resize handles, optional flatten Fill & Sign on save, localized date formats, bulk multi-field property editing, rotated `/MK /R` widget appearances, `/Opt` export values, and a full XFA FormCalc interpreter.
- **Rendering, Memory & GPU (`crates/cos`, `crates/render`, `apps/pdfcraft`):** Zero-copy file-backed PDF stream slices, zero-copy pixmap transfers, 1:1 texel page rasters with zoom tile retention, automatic OpenGL (`glow`) fallback on `wgpu` initialization failure, Windows primary-display GPU adapter selection, and Windows 11 multi-monitor DPI scaling fixes.
- **Localization (`crates/ui-egui/src/i18n`):** 15 interface languages — English, Japanese (`ja`), Brazilian Portuguese (`pt-br`), Spanish (`es`), Telugu (`te`), Czech (`cs`), Simplified Chinese (`zh-hans`), Traditional Chinese (`zh-hant`), French (`fr`), German (`de`), Italian (`it`), Russian (`ru`), Ukrainian (`uk`), Bulgarian (`bg`), and Hungarian (`hu`).

### 5. Windows Installer, Portable Mode & Build Automation
- Added `build.py` and `installer.py` for automated Windows release builds and packaging:
  - `dist/release/LinkcoPDFEditorSetup.exe` (supports both Administrator machine-wide installation to `C:\Program Files\Linkco\Linkco PDF Editor` and standard per-user installation to `%LOCALAPPDATA%\Programs\Linkco\Linkco PDF Editor`, Start Menu and optional Desktop shortcuts, Windows Installed Apps registration, and clean uninstallation preserving user settings by default).
  - `dist/release/LinkcoPDFEditorSetup-0.5.0-windows-x64.msi` (WiX Toolset v5 MSI package with per-machine scope guard and bundled OCR models).
  - `dist/release/LinkcoPDFEditor-0.5.0-windows-x64-portable.zip` (portable archive with `portable.txt` / `LinkcoPDFEditor.portable` marker support storing user data in `PdfCraftData\` beside the executable).

---

## Licensing & Notices

Linkco PDF Editor is dual-licensed under the **Apache License, Version 2.0** (`LICENSE-APACHE`) and the **MIT License** (`LICENSE-MIT`). Third-party asset and library notices are provided in `NOTICE` and `ATTRIBUTION.md`.
