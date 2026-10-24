# PdfCraft Printer (personal build, experimental)

The optional native printer appears as **PdfCraft Printer** in other applications.
It never deliberately renders a whole page into an image. Source applications can still
send text as outlines or pixels; those inputs cannot become selectable text automatically.

## Windows

Requires Windows 11 24H2/build 26100 or newer, x64. The personal combined setup installs
the normal PdfCraft MSI and a separate per-user Print Support App MSIX. Windows supplies
the virtual print interface and Save As dialog; PdfCraft converts OpenXPS through its own
COS writer. It does not use Microsoft Print to PDF. Installation preserves the previous
default printer and does not change trust stores, Developer Mode, or PDF associations.

Build with the Windows 26100 SDK, Rust MSVC and an already validated main installer:

```powershell
packaging/windows/package-printer.ps1 -MainInstaller path/to/pdfcraft.msi -Revision 1
```

The result is `dist/pdfcraft-<version>-with-printer-x64.exe`. Run normally for the full install;
`--printer-only --quiet` installs only its embedded printer plugin. The non-bundled
`pdfcraft-printer-setup` component exposes `--status` and `--remove-printer` for lifecycle
testing. Removing the plugin removes its queue; it never deletes previously saved PDFs.
The app MSI and printer MSIX have separate uninstall entries. This is a local unsigned
personal package, not a signed public release. Windows asks for normal elevation at setup. Setup exits without an extra success popup;
failures still show an error unless `--quiet` was requested.

Unsupported OpenXPS drawing fails the job; see [converter limits](../../docs/xps-print-converter.md).
Errors are recorded in the package's private `LocalState/last-print-error.txt`, without
capturing document contents. Windows may leave an empty Save As destination after a failed
job. There is no automatic OCR or page-image fallback in the printer.

## Linux

The same workspace builds the Linux app and `pdfcraft-cups`. Native deb/rpm packages include
the backend and configure a local, unshared **PdfCraft Printer** queue when CUPS 2.x is
running. CUPS is a package dependency. If setup runs before the local spooler starts:

```sh
sudo /usr/share/pdfcraft/printer/printer-queue.sh install
```

For the tar distribution or extracted AppImage, run its
`usr/share/pdfcraft/printer/install-printer.sh` with sudo to copy the printer component to
the host and register it. A mounted AppImage is not used as the host's CUPS backend.
This requires the standard `/usr/lib/cups` backend layout and `/run/cups/cups.sock`.

PDF-producing applications send their text/vector PDF through a pass-through PPD. The
backend preserves those bytes, writes atomically without overwriting, and caps jobs at
256 MiB. Completed files appear in `/var/spool/pdfcraft-printer/<your numeric user id>/`,
owned by that user with mode 0600 in a private 0700 directory. No backend writes into a
user's home directory. The queue requires CUPS's authenticated policy; unmapped system
accounts and root-owned jobs are refused. Authentication may be requested by the print
dialog. Unsupported formats fail instead of invoking a rasterizer.

The installer keeps the default printer and existing queues. Removal refuses pending
jobs and preserves printed files. A tar/AppImage printer can be removed with
`sudo /usr/share/pdfcraft/printer/printer-queue.sh remove`; remove its installed backend
and PPD after that if no longer needed. CUPS 3.x/IPP printer applications, distribution AppArmor/SELinux policy
packaging, arbitrary CUPS layouts, and a Linux per-job Save As dialog are not implemented.

## Verification

`cargo test -p pdfcraft-create -p pdfcraft-printer -j2` tests conversion, malformed jobs,
interleaved package parts, private output and cancellation. A native Linux smoke test is
`packaging/linux/test-printer.sh` on a disposable CUPS machine; its workflow also builds the
Linux GUI and CLI. Cross-target compilation alone does not verify Linux queue operation.
Only synthetic private Word jobs may be used for the ignored native capture test; never
commit their source fonts, captured spool files or output.

This upstream draft retains the unsigned personal Windows package identity for prototype
review. The Windows combined installer is not wired into release CI; a production publisher/signing plan and upstream
owner approval for the two WinRT activation exports are required before release. The
local fork passed component tests, native queue installation/default preservation and
transactional Windows output tests. Final live Word printing and native Linux app/CUPS
validation remain pending; the Linux backend has compile and strict lint coverage only.
