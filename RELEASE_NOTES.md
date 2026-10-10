# Linkco PDF Editor v0.3.0 — Release Notes

**Product:** Linkco PDF Editor  
**Version:** `0.3.0`  
**Publisher:** Al Rawabet Commercial Services & Contracting Company W.L.L. (Linkco)  
**Location:** Building 159, Street 220, Zone 24, P.O. Box 32282, Doha – State of Qatar (`www.linkco.com.qa`)

---

## Highlights in v0.3.0

### 1. Official Linkco Brand Identity & Enterprise Home Dashboard
- Rebranded desktop application, CLI, WebAssembly target, and installers to **Linkco PDF Editor** by **Al Rawabet Commercial Services & Contracting Company W.L.L. (Linkco)**.
- Designed new multi-resolution application icons (`.svg`, `.ico`, `.icns`, and 16 px–1024 px `.png`) featuring the Linkco 4-node connected network emblem on deep navy (`#01131C`) and crimson red (`#F22424`).
- Updated the **Home** welcome view (`crates/ui-egui/src/home.rs`) with the Linkco corporate header banner, 8 verified **Recommended Tools** (`Organize pages`, `Edit a PDF`, `Combine files`, `Compress a PDF`, `Export a PDF`, `Scan & OCR`, `Fill & Sign`, `Protect a PDF`), an `Open file` card, `View all tools →` navigation to all 19 tool groups, recent files list, and command palette search (`Ctrl+K` / `⌘K`).
- Removed all legacy `PrintCraft` branding, `Discord` references, and third-party promotional links.

### 2. Windows File Explorer PDF Preview Handler (`IPreviewHandler`)
- Added `LinkcoPdfPreviewHandler.dll` (`packaging/windows/PreviewHandler.cs`, CLSID `{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}`), an out-of-process Windows Shell `IPreviewHandler` COM server hosted by `prevhost.exe` (`{6d2b5079-2f0b-48dd-ab7f-97cec514d30b}`).
- Added `pdfcraft-cli preview` (`apps/pdfcraft-cli/src/main.rs`) for fast single-page preview rendering with structured status reporting (`OK`, `PASSWORD_REQUIRED`, `EMPTY_PDF`, `INVALID_PDF`, `RENDER_ERROR`, `IO_ERROR`).
- Supports multi-page navigation (`Prev` / `Next`, `PgUp` / `PgDn`, `Home` / `End`), zoom (`−` / `+` / `Fit`, `Ctrl`+Wheel, click-drag pan), Windows Explorer Light/Dark theme adaptation, and non-destructive registration that backs up and restores any previously registered PDF preview handler on uninstall.

### 3. Windows Printer Detection & High-Resolution Printing (150 / 300 / 600 DPI)
- Implemented 3-tier Windows printer enumeration in `crates/print/src/spool.rs`:
  1. WMI/CIM `Win32_Printer` (`Get-CimInstance` / `Get-WmiObject`)
  2. `.NET` `System.Drawing.Printing.PrinterSettings::InstalledPrinters`
  3. Windows Registry `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Devices` & `Windows /v Device`
- Detects local, USB, TCP/IP, WSD, and UNC network printers, identifies the system default printer, and reports printer availability (`Ready`, `Printing`, `Paused`, `Offline`, `Error`) with an in-dialog refresh button and offline status warning.
- Upgraded Print dialog preview sharpness (`crates/ui-egui/src/canvas.rs`, `PRINT_PREVIEW_W = 860.0`, per-page thumbnail scaling) and added high-DPI sheet rendering (`Session::render_print_sheets` at 150 / 300 / 600 DPI, default 300 DPI) for crisp Windows print output via `.NET` `PrintDocument` with `HardMarginX/Y` compensation.

### 4. Windows Installer & Build Automation
- Added `build.py` and `installer.py` for automated Windows release builds and packaging:
  - `dist/release/LinkcoPDFEditorSetup.exe` (supports both Administrator machine-wide installation to `C:\Program Files\Linkco\Linkco PDF Editor` and standard per-user installation to `%LOCALAPPDATA%\Programs\Linkco\Linkco PDF Editor`, Start Menu and optional Desktop shortcuts, Windows Installed Apps registration, and clean uninstallation preserving user settings by default).
  - `dist/release/LinkcoPDFEditorSetup-0.3.0-windows-x64.msi` (WiX Toolset v5 MSI package).
  - `dist/release/LinkcoPDFEditor-0.3.0-windows-x64-portable.zip` (portable archive).

---

## Licensing & Notices

Linkco PDF Editor is dual-licensed under the **Apache License, Version 2.0** (`LICENSE-APACHE`) and the **MIT License** (`LICENSE-MIT`). Third-party asset and library notices are provided in `NOTICE` and `ATTRIBUTION.md`.
