# PR: Search: Centre Active Find Match in Viewport (#3547, #3530)

## Summary
- **Issue Reference:** Resolves #3547, #3530
- **Crate:** `crates/ui-egui`
- **Scope:** Search match navigation and view panning in `flash_match` (`crates/ui-egui/src/canvas.rs`).

## Root Cause
When searching in a document (via Ctrl+F, Find next/previous, Enter, or the Search panel), `flash_match` previously computed only a vertical fraction:
```rust
let frac = ((y - 60.0) / h).clamp(0.0, 1.0);
self.goto = Some((page, frac));
```
This hardcoded a fixed 60pt offset from the window top and completely ignored horizontal position. On zoomed-in pages (e.g. 150%, Fit to Page, or columns), search matches frequently appeared off-screen or were cut off at viewport edges.

## Fix
- Extended `PageView` to track both `page_widths` and `page_heights`.
- Updated `flash_match` to calculate the exact bounding box of the active match across its glyph range.
- Computed the normalized center point `(fx, fy)` across the page dimensions.
- Configured `goto_point` with a centered window offset (`rel = vec2(viewport_w, viewport_h) / 2.0`), smoothly centering the search match both horizontally and vertically in the viewport.
- Clamped within page and content bounds so unreachable coordinates never pan outside the document.
