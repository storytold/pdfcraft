---
title: No printers are listed in the Print dialog
description: Why the PdfCraft Print dialog can show only Save as PDF on Windows 11 and Linux, and how to fix it or print meanwhile.
order: 20
verified: 0.8.0
---

If the **Print** dialog shows only **Save as PDF**, wait a moment: PdfCraft 0.8.0 reads the printer list in the background, and your default printer appears once it is ready. If no printers ever appear, check that the printer is installed and the Print Spooler service is running. To print right away, choose **Save as PDF** and print the saved file from another app.

Shortcuts on this page use Ctrl. On macOS, use Cmd instead of Ctrl.

## Symptoms

- **File**, **Print…** (`Ctrl+P`) opens the **Print** dialog, but the **Printer:** box shows **Save as PDF**.
- The list has no printers, or only **Save as PDF** ([issue #1031](https://github.com/storytold/pdfcraft/issues/1031), [issue #1084](https://github.com/storytold/pdfcraft/issues/1084)).

## Fix

First check your version in **Help**, **About PdfCraft**.

### Version 0.8.0: the list is still loading

The first time you open **Print**, the printer list is not ready yet, so the **Printer:** box shows **Save as PDF**. If you click **Print** then, PdfCraft saves a PDF instead of printing.

1. Wait a moment after the dialog opens. The box switches to your default printer when the list arrives.
2. Or open the **Printer:** box. It shows **Looking for printers…** until the list is ready.

A fix that makes **Print** wait for the list is merged but not yet in a release ([pull request #2332](https://github.com/storytold/pdfcraft/pull/2332)).

### No printers at all

1. In Windows **Settings**, **Bluetooth & devices**, **Printers & scanners**, check that the printer is installed for your Windows user. A printer shared from another PC must be added there too.
2. Check that the **Print Spooler** service is running: open **Services**, find **Print Spooler**, and start it if it is stopped.
3. Close the **Print** dialog and open it again.

On Linux, printers come from CUPS. Check that your printer shows in your system's printer settings. PdfCraft also lists CUPS printers that have no permanent queue.

### Version 0.7.1 or earlier

Install the latest release: see [Install PdfCraft](../getting-started/install.md). Version 0.6.0 and earlier cannot list printers on Windows at all.

### Print meanwhile

1. Choose **Save as PDF** in the **Print** dialog and save the file.
2. Open the saved PDF in another app and print it from there. A reporter printed with Google Chrome.

## Why this happens

Since 0.8.0, PdfCraft gets the printer list straight from the Windows print spooler: your local printers and the shared printers you are connected to. It reads the list in the background so the dialog opens quickly, which is why it can be empty for a moment.

Versions 0.7.0 and 0.7.1 asked Windows PowerShell for the list ([pull request #847](https://github.com/storytold/pdfcraft/pull/847)), which could be slow or blocked. Version 0.6.0 and earlier had no way to list Windows printers. When PdfCraft has no printer list, the dialog offers only **Save as PDF**.

## Still stuck?

Ask in [Discord](https://discord.gg/artcraft) or comment on [issue #1084](https://github.com/storytold/pdfcraft/issues/1084). Say your system, your PdfCraft version from **Help**, **About PdfCraft**, and the printer model.
