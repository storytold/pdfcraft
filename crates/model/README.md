# pdfcraft-model

Layer L2: typed views over the `pdfcraft-cos` object graph. It starts small, with what several
L3/L4 crates need:

- `pages(doc)`: leaf pages in order, each with its inherited attributes (`Resources`,
  `MediaBox`, `CropBox`, `Rotate`, §7.7.3.4) resolved;
- `Page::crop`, `Page::rotation`, `Page::user_unit` and `Page::view_matrix`: the displayed page's
  geometry, and the matrix from "display space" (points, origin at the bottom-left of the page as
  shown, after `/UserUnit` and `/Rotate`) to user space, so content can be placed the way the
  reader sees the page. `/UserUnit` is read as Acrobat reads it: the page's own entry, a number
  from 1 up, at most 75,000; anything else counts as 1.
  `view_matrix_for(rotation, rect)` is the same matrix for any rectangle, e.g. to draw a
  comment's picture upright inside its user-space `/Rect` on a turned page.

`organize`, `annot` and `forms` still carry their own small walkers; they move here as the model
grows (ADR-0004).
