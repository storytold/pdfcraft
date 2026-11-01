---
title: Split a PDF or extract pages
description: Cut a PDF into several files, or copy chosen pages into a new document, in PdfCraft.
order: 40
verified: 0.7.1
---

To copy chosen pages into one new document, use **Extract pages…**. To cut a whole document into several files, use **Split document…**. Both are in the **Pages** menu.

## Extract pages

1. Click the **Menu** button, then **Pages**, then **Organize pages**.
2. Select the pages you want. See [Rotate, delete, insert and reorder pages](organize-pages.md).
3. Click **Extract pages to a new document** in the toolbar. The **Extract pages** dialog says how many pages are selected.
4. Optional: change the **File name**.
5. Optional: tick **Delete pages after extracting** to remove the pages from the original, or **Extract pages as separate files** to get one file per page. With separate files, PdfCraft asks you to choose a folder.
6. Click **Extract**.

Without **Extract pages as separate files**, the pages open as a new document. PdfCraft names it "<name> (extract).pdf" unless you typed another file name. Save it with **File**, **Save as…**. Separate files are named "<name> (page N).pdf".

The same command is also in the **Pages** menu as **Extract pages…**.

## Split a document

1. Open the PDF. To split before certain pages, first select those pages in **Organize pages**.
2. Click the **Menu** button, then **Pages**, then **Split document…**. In **Organize pages** you can click **Split into files…** instead.
3. Choose how to split:

   | Option                                                   | What you get                                                            |
   |----------------------------------------------------------|-------------------------------------------------------------------------|
   | **Number of pages**                                      | A new file every set number of pages (**Pages per file**)               |
   | **File size**                                            | Files of at most the size you set under **At most**, in MB              |
   | **Top-level bookmarks (n)**                              | One file per top-level bookmark. Off when the PDF has no bookmarks      |
   | **Before each selected page (select pages in Organize)** | A new file starts at each selected page. Off when no pages are selected |

4. Check the line under the options, for example "Creates 13 files from 13 pages."
5. Click **Split** and choose a folder when PdfCraft asks ("Choose a folder for the split files").

**Split** stays off if the settings would make only one file. Files are named "<name> (pages 1-10).pdf", "<name> (page 3).pdf", or "<name> - <bookmark title>.pdf" for bookmark splits.

## Command line

```
pdfcraft-cli extract report.pdf --pages 1,3,5 --out highlights.pdf
pdfcraft-cli split   report.pdf --every 10 --out-dir parts/
```

On macOS and Linux the command-line tool is a separate `pdfcraft-cli` download on the release page. On Windows, `pdfcraft-cli.exe` is in the portable zip.
