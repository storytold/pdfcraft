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
  comment's picture upright inside its user-space `/Rect` on a turned page;
- `say(wording, values)`: a wording with its `{name}` holes filled in one pass, so a value that
  contains braces is never read as a hole (a11y findings, print errors, form table messages).

The page-tree walk is `PageLeaves`, the one `pages`, `page_refs`, `forms`, `annot` and `organize`
use. It keeps one cursor per ancestor and shares inherited attribute owners (`PageLeaf::inherited`
reads them without copying). Repeated child references are skipped before anything is copied,
with first depth-first occurrence order preserved; a `/Kids` array shared by several nodes is read
once; objects are told apart by number. It visits at most `MAX_PAGE_TREE_NODES` nodes, and ends at
the first `/Kids` entry past `MAX_KIDS_ENTRIES` (read, so at most `MAX_KIDS_ENTRIES + 1` in all,
but not followed); `PageLeaves::truncated` says when either cap stopped it. The entry cap matters
even though each array is read once by object number: an object stream can give many numbers
the same bytes. A `/Type /Pages` node without `/Kids` lists no pages.
`pages` returns page dictionaries that own their effective attributes, and copies at most
`MAX_INHERITED_COPIES` (1 GiB, approximate heap bytes) of inherited values into the leaves; past
any of these limits the walk stops and the pages after that point are not listed.
