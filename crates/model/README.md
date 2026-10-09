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

## Embedded 3D geometry

`three_d::Scene` decodes U3D meshes, point sets and line sets, including raw and compressed geometry, indexed normal/color pools and assembly transforms. Instances retain visibility; model-space accessors resolve actual transformed triangles, lines and points. Surface ray picking honors depth and face visibility.

The decoder bounds block lengths, record allocations, arithmetic/topology work and object graph traversal across the complete scene. Errors never return a partial scene. The normal prediction convention is explicit because published coordinate differences and the observed reference writer's rotations can give different valid results for the same bytes. Contributor-original compressed/raw fixtures are checked against an independent reader's public APIs; no reference implementation algorithms are incorporated.
