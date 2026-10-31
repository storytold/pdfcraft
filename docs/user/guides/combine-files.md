---
title: Combine PDF files
description: Merge several PDFs into one document with Combine files in PdfCraft.
order: 30
verified: 0.7.1
---

Choose **File**, **Combine files…**, add your files, put them in order and click **Combine**. The result opens as a new document called Combined.pdf.

Shortcuts on this page use Ctrl. On macOS, use Cmd instead of Ctrl.

## Steps

1. Click the **Menu** button, then **File**, then **Combine files…**. A **Combine files** tab opens.
2. Add files. Click **Add files…**, or drop PDFs, images, text files or folders on the tab. **Add open documents** adds the documents you already have open in PdfCraft. Images (PNG, JPEG, TIFF, GIF, BMP, JPEG 2000) and `.txt` files become PDF pages.
3. Put the files in order. The tab says: "Files are combined in the order shown. Drag them to change it." You can also select a file and use **Move up** or **Move down**.
4. Optional: switch between **Grid view** and **List view**. In the list, type page numbers in the pages column to take only some pages from a file. Leave it empty to take every page ("All pages").
5. Click **Combine**. The button stays off until you have added at least two files.
6. Check the new Combined.pdf, then choose **File**, **Save as…** (`Ctrl+Shift+S`) to keep it.

## What the combined file keeps

PdfCraft adds a bookmark for each file and nests that file's own bookmarks under it. Links and named destinations are rewired to the copied pages. Form fields stay interactive, layers keep their on/off defaults and attachments come along.

## Problem files

- A password-protected file is marked "Password-protected". Select it and click **Unlock…**, then type the password. The password is only used to read the file and is not saved. The combined file will not be password-protected.
- A file whose security settings do not allow copying pages is marked "Its security settings don't allow copying pages". Select it, click **Unlock…** and enter its permissions password.
- A file PdfCraft cannot read is marked "Can't be read" with the reason. When you click **Combine**, PdfCraft asks "Leave out files that can't be read?". Click **Combine the rest** to combine the other files without it, or **Cancel**.
- If **Combine** stays off, hover over it to see which file is the problem.
- A file you added twice is marked "Added more than once". It is still combined twice unless you remove one copy.

## Command line

`pdfcraft-cli combine report.pdf appendix.pdf --out combined.pdf` does the same without the app. On macOS and Linux the command-line tool is a separate `pdfcraft-cli` download on the release page. On Windows, `pdfcraft-cli.exe` is in the portable zip.
