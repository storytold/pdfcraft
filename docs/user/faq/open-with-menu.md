---
title: PdfCraft is missing from Open with
description: Why PdfCraft may be missing from Open with or Default apps on Windows 11, and how to open PDFs with it anyway.
order: 30
verified: 0.7.1
---

The MSI installer registers PdfCraft for PDF files, so it appears under **Open with** and in Windows' **Default apps** settings. It does not change which app opens PDFs by default. Since 0.7.0 it also adds **Edit with PdfCraft** to the right-click menu of PDF files. The portable zip registers nothing.

## If you do not see it

1. Check how you installed PdfCraft. A user in [issue #1080](https://github.com/storytold/pdfcraft/issues/1080) was told the portable zip registers nothing, by design. Install the `.msi` as administrator to get PdfCraft under **Open with**.
2. If you want to keep the portable version, right-click a PDF, choose **Open with**, then **Choose another app**, and point it at your `pdfcraft.exe`.

## Edit with PdfCraft

On Windows 11, right-click a PDF and choose **Show more options** to see **Edit with PdfCraft**. It opens the file in PdfCraft whatever your default PDF app is. Select several PDFs to open them as tabs in one window.

The installer adds this entry unless you untick its checkbox on the first screen of the installer. Version 0.6.0 and earlier do not have it.

## Installed with the MSI and still missing

One user installed `pdfcraft-0.6.0-windows-x64.msi` with administrator rights and did not see PdfCraft in **Open with** ([issue #1080](https://github.com/storytold/pdfcraft/issues/1080), still open). Use **Edit with PdfCraft** or the second step above, and tell us on the issue if you see the same.

Ask in [Discord](https://discord.gg/artcraft) if this still does not work.
