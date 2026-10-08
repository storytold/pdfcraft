# pdfcraft-model

Layer L2: typed views over the `pdfcraft-cos` object graph. It starts small, with what several
L3/L4 crates need:

- `pages(doc)`: leaf pages in order, each with its inherited attributes (`Resources`,
  `MediaBox`, `CropBox`, `Rotate`, §7.7.3.4) resolved;
- `Page::crop`, `Page::rotation` and `Page::view_matrix`: the displayed page's geometry, and the
  matrix from "display space" (origin at the bottom-left of the page as shown, after `/Rotate`)
  to user space, so content can be placed the way the reader sees the page.

`organize`, `annot` and `forms` still carry their own small walkers; they move here as the model
grows (ADR-0004).
