# pdfcraft-model

Layer L2: typed views over the `pdfcraft-cos` object graph. It starts small, with what several
L3/L4 crates need:

- `pages(doc)`: leaf pages in order, each with its inherited attributes (`Resources`,
  `MediaBox`, `CropBox`, `Rotate`, §7.7.3.4) resolved;
- `Page::crop`, `Page::rotation`, `Page::user_unit` and `Page::view_matrix`: the displayed page's
  geometry, and the matrix from "display space" (points, origin at the bottom-left of the page as
  shown, after `/UserUnit` and `/Rotate`) to user space, so content can be placed the way the
  reader sees the page. `/UserUnit` is read as Acrobat reads it: the page's own entry, a number
  from 1 up, at most 75,000; anything else counts as 1. A `/Rotate` that isn't a multiple of 90
  counts as 0, as the renderer draws it.
  `view_matrix_for(rotation, rect)` is the same matrix for any rectangle, e.g. to draw a
  comment's picture upright inside its user-space `/Rect` on a turned page.

The page-tree walk is `PageNodes` (every node, page-tree nodes before the nodes under them) and
`PageLeaves` (its leaves), the one walk `pages`, `try_pages`, `page`, `page_refs`, `forms`,
`annot`, `organize` and redact's resource holders use. It keeps one cursor per ancestor and shares
inherited attribute owners (`PageNode::inherited` reads them without copying, and
`PageNode::inherited_from` names the node that sets them). Repeated child references are skipped
before anything is copied, with first depth-first occurrence order preserved; a `/Kids` array
shared by several nodes is read once; objects are told apart by number and given at the generation
the document holds them at. A `/Type /Pages` node without `/Kids` lists no pages, and `/Kids` that
refers to anything but an array lists nothing.

Limits: at most `MAX_PAGE_TREE_NODES` nodes and `MAX_KIDS_ENTRIES` `/Kids` entries (every entry
counts; the entry cap matters even though each array is read once by number, because an object
stream can give many numbers the same bytes), and `MAX_INHERITED_COPIES` (1 GiB, approximate heap
bytes) of inherited values copied into the leaves of one walk. A walk that meets a limit yields
`Err(PageTreeLimit)` and then ends; it never ends early as if it had read the whole tree.
`page_refs`, `try_pages` and `page` return that error. `pages` stays lenient for its many read-only
callers: it gives the pages before the limit. `page(doc, index)` copies only that page's inherited
values, so a page past the whole walk's copy budget is still found.
