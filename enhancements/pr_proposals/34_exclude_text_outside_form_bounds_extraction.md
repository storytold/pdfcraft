# PR Proposal 34: Exclude Text Outside Form Bounds from Extraction

**Issue Reference:** #3628 (renderer extraction part of #3503)
**Crates Affected:** `crates/render`

## Summary
When text is drawn inside a Form XObject outside its transformed `/BBox`, `TextDevice` previously extracted it because it ignored `push_clip_path` and `pop_clip_path`. This caused search (Find) and plain text export to include text that is invisible and clipped on the rendered page.

## Implementation Details
- In `crates/render/src/text.rs`:
  - Added `clips: Vec<Option<Rect>>` to `TextDevice`.
  - Implemented `push_clip_path` to track intersected bounding boxes of active clip paths up to a depth of 128.
  - In `draw_glyph`, excluded glyphs whose bounding boxes are wholly outside the active clip rect (`outside = b.x1 < active_clip.x0 || b.x0 > active_clip.x1 || b.y1 < active_clip.y0 || b.y0 > active_clip.y1`).
  - Preserved boundary-touching and intersecting glyphs, and flagged `actual_filtered` when clipping occurs within `/ActualText` spans.
