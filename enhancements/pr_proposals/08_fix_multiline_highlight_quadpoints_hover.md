# Pull Request: fix(canvas): test exact QuadPoints for text markup annotation hover popups (#3541, #3620)

## Summary
In `crates/ui-egui/src/canvas.rs`, annotation hover detection for comment preview tooltips tested the cursor coordinate against `a.rect` (`xf.user_rect(info, i, a.rect).contains(p)`). 

For multi-line text markup annotations (`Highlight`, `Underline`, `StrikeOut`, `Squiggly`), `a.rect` is the bounding box spanning across all lines. Testing this bounding box caused hover popups to trigger over unhighlighted text beside the marked lines (e.g. before the highlight starts on line 1, or after it ends on line 2), and interfered with selecting unhighlighted text on those lines (#3620).

## Changes
- `crates/ui-egui/src/canvas.rs`:
  - When `!a.quads.is_empty()` on text markup annotations, test cursor hit against each quadrilateral in `a.quads` rather than the union bounding rectangle `a.rect`.
  - Falls back to `sr.contains(p)` for single-rect or non-markup annotations.
