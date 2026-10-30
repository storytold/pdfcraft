# pdfcraft-render

Page rasterization through the vendored Hayro/Vello bootstrap, with document inspection for the engine and UI.
The public `PageRenderer` API also backs the `page_render` and `doc_export_images` automation tools.

Culled geometry stores coarse signed winding in `i32`, matching the strip accumulator and covering the existing `MAX_LINES_PER_PATH` walk.
This prevents integer coarse-winding overflow for nonzero fill, even-odd parity and opposite-contour cancellation when paths surround or cross the viewport.
Even-odd coverage reduces integer winding modulo two before conversion to floating point, both at a row start and across strip gaps.
Small tiler-to-strip regressions cover signed counters beyond the exact-integer range of `f32`, fractional edge coverage and nonzero cancellation without allocating millions of contours.
The original rectangle regressions in [culled_winding.rs](tests/it/culled_winding.rs) exercise both contour directions, counts beyond the `i8` and `i16` ranges, and visible edge tiles.

The renderer retains its existing path-complexity and floating-point coverage limits.
The bootstrap implementation and broader rendering-fidelity gaps remain unchanged.
