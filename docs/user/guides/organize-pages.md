---
title: Rotate, delete, insert and reorder pages
description: Use Organize pages in PdfCraft to rotate, delete, insert, replace and move pages, then save.
order: 20
verified: 0.7.1
---

Open **Menu**, **Pages**, **Organize pages** to see every page as a thumbnail. Select pages, use the toolbar above the grid, then save.

Shortcuts on this page use Ctrl. On macOS, use Cmd instead of Ctrl.

## Steps

1. Open the PDF.
2. Click the **Menu** button, then **Pages**, then **Organize pages**.
3. Select pages. Click one page, `Ctrl`-click (`Cmd`-click on macOS) or tick the checkbox in a page's corner to add or remove single pages, or `Shift`-click to select a range. You can also type page numbers in **Selected pages** above the toolbar, such as `1, 12, 16-18`, and click **Apply**. The toolbar shows how many pages are selected.
4. Use the toolbar buttons above the grid. Hover over a button to see its name.

   | Button                                  | What it does                                  |
   |-----------------------------------------|-----------------------------------------------|
   | Rotate counterclockwise                 | Turns the selected pages 90 degrees left      |
   | Rotate clockwise                        | Turns the selected pages 90 degrees right     |
   | Delete pages (Delete)                   | Removes the selected pages                    |
   | Insert a blank page after the selection | Adds an empty page                            |
   | Insert pages from a file…               | Adds pages from another PDF                   |
   | Replace pages…                          | Swaps pages for pages from another file       |
   | Extract pages to a new document         | See [Split and extract](split-and-extract.md) |
   | Split into files…                       | See [Split and extract](split-and-extract.md) |
   | Move earlier, Move later                | Moves the selected pages                      |

5. Click **Save pages** to save, or choose **File**, **Save** (`Ctrl+S`). **Save pages** is greyed out until you change something. Click **Close** to go back to the document.

To rotate or delete one page, point at it: buttons to rotate it either way or delete it appear under the thumbnail.

The **Select pages** button in the toolbar offers **All pages**, **Odd pages**, **Even pages**, **Landscape pages**, **Portrait pages** and **None**. **Larger pages** and **Smaller pages** change the thumbnail size.

## Undo

Every change is one step in the undo history. Choose **Edit**, then **Undo** (`Ctrl+Z`), or **Redo** (`Ctrl+Shift+Z`). The Edit menu names the step, for example "Undo Rotate pages". Undo still works after you save.

## Good to know

- You cannot delete every page: the **Delete pages (Delete)** button is off when all pages are selected.
- **Replace pages…** changes only the page content. Links, comments, form fields and bookmarks on the original pages stay.
- Saving adds your changes to the end of the file and leaves the original bytes untouched. The file is written to a temporary copy first, then swapped in.
- A document restricted by its author may grey out these buttons. See [Protect a PDF with a password](protect-with-password.md).
