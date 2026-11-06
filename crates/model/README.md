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

The page-tree walk is `PageNodes` (every node, in document order) and `PageLeaves` (its leaves).
`pages`, `try_pages`, `page`, `page_by_ref` and `page_refs` use it; its doc comment gives the
rules (leaf rule, object identity by number, a shared `/Kids` array read once). annot, forms,
organize, sign, redact and engine still carry their own walkers, each with its own leaf rule and
limits; they move onto `PageNodes`/`PageLeaves` one crate at a time (ADR-0004). Each `/Kids` entry is resolved as cos resolves references (#1143): a
stale generation is null and lists nothing, a repaired alias is the object. Each `PageNode` is
given at its canonical reference (`Document::canonical_ref`; destinations are compared the same
way, after cos accepts them), and reads its inherited attributes without copying them (`inherited`,
`inherited_from`).

Its limits are `MAX_PAGE_TREE_NODES`, `MAX_KIDS_ENTRIES` and `MAX_INHERITED_COPIES` (the
inherited copies of one walk; leaf dictionaries themselves are cloned uncharged, so this is not a
bound on all allocation). Exceeding one yields `Err(PageTreeLimit)`, never a shorter walk that looks
whole. `page_refs`, `try_pages` and `page` return it; `pages` stays lenient and gives the pages
before the limit. `page` and `page_by_ref` copy one page. `with_limits` (the `test-support` feature,
for other crates' tests) lowers the limits on one thread.
